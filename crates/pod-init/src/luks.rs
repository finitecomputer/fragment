//! The real volume: LUKS2 through cryptsetup, ext4, mount. The key only
//! ever goes to cryptsetup on stdin (`--key-file -`), never into argv
//! or the environment, where other processes could read it.

use crate::config::Config;
use crate::mountinfo;
use crate::volume::{Volume, VolumeError};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Command, Stdio};

/// The first LUKS header's magic (LUKS1 and LUKS2 alike).
const LUKS_MAGIC: &[u8; 6] = b"LUKS\xba\xbe";
/// cryptsetup's exit code for "no key available with this passphrase".
const EXIT_NO_PERMISSION: i32 = 2;
const GIB: u64 = 1 << 30;
/// The LUKS2 token slot the owner hint is kept in when the config names
/// no hint file (`Config::owner_hint`): in the header, outside the
/// encrypted area, so it survives a reboot of a pod whose root is
/// read-only and whose /var is memory. It is no secret and no proof: the
/// host can rewrite it, as it could the file.
pub const HINT_TOKEN: &str = "7";
const HINT_TYPE: &str = "pod-owner-hint";
/// The most a program's standard output is read (a token is ~100 bytes).
const STDOUT_MAX: u64 = 64 * 1024;

pub struct Luks {
    config: Config,
}

impl Luks {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// The key is 64 random bytes, so a slow KDF adds nothing: PBKDF2 at its
/// minimum count makes opening instant without weakening anything.
pub fn format_args(image: &Path) -> Vec<OsString> {
    let mut a = os(&[
        "luksFormat", "--batch-mode", "--type", "luks2", "--cipher", "aes-xts-plain64", "--key-size", "512",
        "--sector-size", "4096", "--pbkdf", "pbkdf2", "--pbkdf-force-iterations", "1000", "--key-file", "-",
    ]);
    a.push(image.into());
    a
}

/// No dm-crypt work queues (lower latency on a VM's virtual disk) and
/// discards passed through, so a sparse image stays sparse.
pub fn open_args(image: &Path, name: &str) -> Vec<OsString> {
    let mut a = os(&[
        "open", "--type", "luks2", "--key-file", "-", "--perf-no_read_workqueue", "--perf-no_write_workqueue",
        "--allow-discards",
    ]);
    a.push(image.into());
    a.push(name.into());
    a
}

struct Exit {
    code: Option<i32>,
    detail: String,
}

/// Run `program args`, writing `key` (if any) to its stdin and nothing else.
fn run(program: &str, args: &[OsString], key: Option<&DiskKey>) -> Result<(), Exit> {
    run_with(program, args, key.map(|k| k.as_bytes().as_slice())).map(drop)
}

/// Run `program args` with `input` (if any) on its stdin; its stdout,
/// bounded, on success.
fn run_with(program: &str, args: &[OsString], input: Option<&[u8]>) -> Result<Vec<u8>, Exit> {
    let fail = |detail: String| Exit { code: None, detail };
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("{program}: {e}")))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // A broken pipe means the program exited early; its status says why.
        let _ = stdin.write_all(input);
    }
    let mut stdout = Vec::new();
    if let Some(o) = child.stdout.take() {
        let _ = o.take(STDOUT_MAX).read_to_end(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.by_ref().take(4096).read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| fail(format!("{program}: {e}")))?;
    if status.success() {
        return Ok(stdout);
    }
    Err(Exit { code: status.code(), detail: format!("{program} {status}: {}", stderr.trim()) })
}

/// The owner hint as a LUKS2 token: a type of our own, bound to no key slot.
pub fn hint_token(owner: &[u8; 32]) -> Vec<u8> {
    serde_json::json!({ "type": HINT_TYPE, "keyslots": [], "owner": hex::encode(owner) }).to_string().into_bytes()
}

/// The owner a hint token names, refusing any other token.
pub fn parse_hint_token(json: &[u8]) -> Result<[u8; 32], String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("hint token: {e}"))?;
    if v["type"] != HINT_TYPE {
        return Err(format!("token {HINT_TOKEN} is not a {HINT_TYPE}"));
    }
    let owner = v["owner"].as_str().ok_or("hint token: no owner")?;
    fragment_pod::unlock::wire::hex32("owner", owner).map_err(|e| format!("hint token: {e}"))
}

pub fn hint_import_args(image: &Path) -> Vec<OsString> {
    let mut a = os(&["token", "import", "--token-id", HINT_TOKEN, "--token-replace", "--json-file", "-"]);
    a.push(image.into());
    a
}

pub fn hint_export_args(image: &Path) -> Vec<OsString> {
    let mut a = os(&["token", "export", "--token-id", HINT_TOKEN]);
    a.push(image.into());
    a
}

fn step(name: &'static str) -> impl Fn(Exit) -> VolumeError {
    move |e| VolumeError::Step { step: name, detail: e.detail }
}

fn io_step(name: &'static str) -> impl Fn(io::Error) -> VolumeError {
    move |e| VolumeError::Step { step: name, detail: e.to_string() }
}

/// Blank unless the image exists and starts with a LUKS header.
pub fn has_luks_header(image: &Path) -> Result<bool, VolumeError> {
    let mut magic = [0u8; 6];
    match fs::File::open(image).and_then(|mut f| f.read_exact(&mut magic)) {
        Ok(()) => Ok(&magic == LUKS_MAGIC),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof) => Ok(false),
        Err(e) => Err(VolumeError::Step { step: "read header", detail: format!("{}: {e}", image.display()) }),
    }
}

impl Luks {
    fn mapper_exists(&self) -> bool {
        fs::metadata(self.config.mapper_path()).is_ok()
    }

    fn mount(&self) -> Result<(), VolumeError> {
        fs::create_dir_all(&self.config.mount).map_err(io_step("mkdir mount"))?;
        let mapper = self.config.mapper_path();
        let args = [OsString::from("-o"), "discard".into(), mapper.into(), self.config.mount.clone().into()];
        run("mount", &args, None).map_err(step("mount"))
    }

    fn luks_open(&self, key: &DiskKey) -> Result<(), VolumeError> {
        run("cryptsetup", &open_args(&self.config.volume_image, &self.config.mapper_name), Some(key)).map_err(|e| {
            if e.code == Some(EXIT_NO_PERMISSION) {
                VolumeError::WrongKey
            } else {
                step("cryptsetup open")(e)
            }
        })
    }

    fn luks_close(&self) -> Result<(), VolumeError> {
        run("cryptsetup", &os(&["close", &self.config.mapper_name]), None).map_err(step("cryptsetup close"))
    }

    /// A sparse file of volume_size_gb, if the image does not exist yet.
    /// Never under /dev: a missing disk (not attached, or not yet named
    /// by udev) must fail the init, not become a file in devtmpfs.
    fn ensure_image(&self) -> Result<(), VolumeError> {
        let image = &self.config.volume_image;
        if image.exists() {
            return Ok(());
        }
        if image.starts_with("/dev") {
            return Err(VolumeError::Step { step: "create image", detail: format!("no device {}: is the data disk attached?", image.display()) });
        }
        let size = self.config.volume_size_gb.checked_mul(GIB).ok_or_else(|| VolumeError::Step {
            step: "create image",
            detail: "volume_size_gb is too large".into(),
        })?;
        let f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(image);
        f.and_then(|f| f.set_len(size)).map_err(io_step("create image"))
    }
}

impl Volume for Luks {
    fn state(&self) -> Result<State, VolumeError> {
        if !has_luks_header(&self.config.volume_image)? {
            return Ok(State::Blank);
        }
        let Ok(meta) = fs::metadata(self.config.mapper_path()) else {
            return Ok(State::Locked);
        };
        let info = fs::read_to_string("/proc/self/mountinfo").map_err(io_step("read mountinfo"))?;
        let open = mountinfo::mounted_from(&info, &self.config.mount, mountinfo::split_dev(meta.rdev()));
        Ok(if open { State::Open } else { State::Locked })
    }

    fn create(&self, key: &DiskKey) -> Result<(), VolumeError> {
        self.ensure_image()?;
        run("cryptsetup", &format_args(&self.config.volume_image), Some(key)).map_err(step("cryptsetup luksFormat"))?;
        self.luks_open(key)?;
        let mkfs = [OsString::from("-q"), self.config.mapper_path().into()];
        run("mkfs.ext4", &mkfs, None).map_err(step("mkfs.ext4"))?;
        self.mount()
    }

    fn open(&self, key: &DiskKey) -> Result<(), VolumeError> {
        // A mapping left by an interrupted unlock is closed first, so the
        // key is always checked by cryptsetup on this request.
        if self.mapper_exists() {
            self.luks_close()?;
        }
        self.luks_open(key)?;
        if let Err(e) = self.mount() {
            let _ = self.luks_close();
            return Err(e);
        }
        Ok(())
    }

    fn close(&self) -> Result<(), VolumeError> {
        run("umount", &[self.config.mount.clone().into()], None).map_err(step("umount"))?;
        self.luks_close()
    }

    /// No token (or no header) reads as no hint, as a missing file does;
    /// cryptsetup's own message is kept for the log.
    fn header_hint(&self) -> Result<Option<[u8; 32]>, VolumeError> {
        match run_with("cryptsetup", &hint_export_args(&self.config.volume_image), None) {
            Ok(json) => parse_hint_token(&json).map(Some).map_err(|detail| VolumeError::Step { step: "header hint", detail }),
            Err(e) => {
                eprintln!("pod-init: no owner hint in the header: {}", e.detail);
                Ok(None)
            }
        }
    }

    fn set_header_hint(&self, owner: &[u8; 32]) -> Result<(), VolumeError> {
        run_with("cryptsetup", &hint_import_args(&self.config.volume_image), Some(&hint_token(owner))).map(drop).map_err(step("cryptsetup token import"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_read_from_stdin_and_never_named_in_argv() {
        let f = format_args(Path::new("/var/lib/pod/volume.img"));
        let o = open_args(Path::new("/var/lib/pod/volume.img"), "pod");
        for args in [&f, &o] {
            let at = args.iter().position(|a| a == "--key-file").unwrap();
            assert_eq!(args[at + 1], "-");
        }
        let f: Vec<_> = f.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(f.join(" "), "luksFormat --batch-mode --type luks2 --cipher aes-xts-plain64 --key-size 512 --sector-size 4096 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --key-file - /var/lib/pod/volume.img");
        assert_eq!(o.last().unwrap(), "pod");
        assert!(o.iter().any(|a| a == "--allow-discards"));
    }

    #[test]
    fn a_missing_short_or_plain_image_is_blank() {
        let dir = crate::scratch::Scratch::new();
        let image = dir.path().join("volume.img");
        assert!(!has_luks_header(&image).unwrap());
        fs::write(&image, b"LUK").unwrap();
        assert!(!has_luks_header(&image).unwrap());
        fs::write(&image, [0u8; 4096]).unwrap();
        assert!(!has_luks_header(&image).unwrap());
        fs::write(&image, [LUKS_MAGIC.as_slice(), &[0, 2]].concat()).unwrap();
        assert!(has_luks_header(&image).unwrap());
    }

    #[test]
    fn the_hint_token_round_trips_and_other_tokens_are_refused() {
        let t = hint_token(&[0xab; 32]);
        assert_eq!(parse_hint_token(&t), Ok([0xab; 32]));
        let v: serde_json::Value = serde_json::from_slice(&t).unwrap();
        assert_eq!(v["keyslots"], serde_json::json!([]), "bound to no key slot");
        assert!(parse_hint_token(br#"{"type":"systemd-tpm2","keyslots":["0"]}"#).is_err());
        assert!(parse_hint_token(br#"{"type":"pod-owner-hint","keyslots":[],"owner":"ab"}"#).is_err());
        assert!(parse_hint_token(b"not json").is_err());
        let i: Vec<_> = hint_import_args(Path::new("/dev/disk/by-id/google-pod-data")).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(i.join(" "), "token import --token-id 7 --token-replace --json-file - /dev/disk/by-id/google-pod-data");
    }

    #[test]
    fn a_missing_device_is_never_made_as_a_file() {
        let config = Config::parse(br#"{"volume_image":"/dev/disk/by-id/pod-init-test-missing","volume_size_gb":1,
            "mapper_name":"podtest","mount":"/nonexistent/mnt","tls_root":"/nonexistent/root.pem"}"#)
        .unwrap();
        let err = Luks::new(config).create(&DiskKey::from_bytes(&[b'k'; 64]).unwrap()).unwrap_err();
        assert!(matches!(&err, VolumeError::Step { step: "create image", detail } if detail.contains("data disk")), "{err}");
        assert!(!Path::new("/dev/disk/by-id/pod-init-test-missing").exists());
    }

    #[test]
    fn a_wrong_key_is_cryptsetups_exit_2() {
        // Stand in for cryptsetup with sh: exit 2 once all 64 key bytes
        // have arrived on stdin, and see that exit code come back.
        let key = DiskKey::from_bytes(&[b'k'; 64]).unwrap();
        let script = os(&["-c", "test \"$(wc -c)\" -eq 64 && exit 2"]);
        let err = run("sh", &script, Some(&key)).err().unwrap();
        assert_eq!(err.code, Some(EXIT_NO_PERMISSION));
        assert!(run("true", &[], None).is_ok());
        assert!(run("/nonexistent/cryptsetup", &[], None).is_err());
    }
}
