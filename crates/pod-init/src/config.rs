//! pod-init's configuration: a JSON file (`--config`) naming the volume
//! and the files beside and inside it, and the listening address.

use serde::Deserialize;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_PORT: u16 = 7000;
pub const DEFAULT_OWNER_FILE: &str = "pod/owner";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// A file or block device holding the LUKS volume.
    pub volume_image: PathBuf,
    /// Size of a new image file, made at init when `volume_image` does
    /// not exist.
    pub volume_size_gb: u64,
    pub mapper_name: String,
    pub mount: PathBuf,
    /// Inside the mounted volume: who the owner is.
    #[serde(default = "default_owner_file")]
    pub owner_file: PathBuf,
    /// Beside the image, outside the volume: who the owner says it is.
    /// Unset: the hint is kept in the volume's own LUKS2 header instead
    /// (`luks::HINT_TOKEN`), for a pod whose root is read-only and whose
    /// /var is memory, with nowhere else that survives a reboot.
    #[serde(default)]
    pub owner_hint: Option<PathBuf>,
    /// The pod's TLS root certificate (PEM); may not exist yet.
    pub tls_root: PathBuf,
    /// Run after the volume is mounted.
    #[serde(default)]
    pub open_hook: Option<PathBuf>,
}

fn default_owner_file() -> PathBuf {
    PathBuf::from(DEFAULT_OWNER_FILE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn parse(json: &[u8]) -> Result<Self, ConfigError> {
        let c: Config = serde_json::from_slice(json).map_err(|e| ConfigError(format!("config: {e}")))?;
        c.check()?;
        Ok(c)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = std::fs::read(path).map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
        Self::parse(&bytes)
    }

    /// Names go into cryptsetup's argv and paths into mount's, so refuse
    /// anything that is not plainly what it claims to be.
    fn check(&self) -> Result<(), ConfigError> {
        let bad = |s: String| Err(ConfigError(s));
        let name_ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if self.mapper_name.is_empty() || self.mapper_name.len() > 64 || !self.mapper_name.chars().all(name_ok) {
            return bad(format!("mapper_name {:?}: 1 to 64 of A-Z a-z 0-9 - _", self.mapper_name));
        }
        let hint = self.owner_hint.as_ref().map(|h| ("owner_hint", h));
        let paths = [("volume_image", &self.volume_image), ("mount", &self.mount), ("tls_root", &self.tls_root)];
        for (name, p) in paths.into_iter().chain(hint) {
            if !p.is_absolute() {
                return bad(format!("{name} {} must be an absolute path", p.display()));
            }
        }
        if let Some(h) = &self.open_hook {
            if !h.is_absolute() {
                return bad(format!("open_hook {} must be an absolute path", h.display()));
            }
        }
        let inside = self.owner_file.components().all(|c| matches!(c, Component::Normal(_)));
        if !inside || self.owner_file.as_os_str().is_empty() {
            return bad(format!("owner_file {} must be a relative path inside the mount", self.owner_file.display()));
        }
        if self.volume_size_gb == 0 {
            return bad("volume_size_gb must be at least 1".into());
        }
        Ok(())
    }

    pub fn owner_path(&self) -> PathBuf {
        self.mount.join(&self.owner_file)
    }

    pub fn mapper_path(&self) -> PathBuf {
        Path::new("/dev/mapper").join(&self.mapper_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub config: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub configfs: PathBuf,
    pub ccel: PathBuf,
}

pub const USAGE: &str = "usage: pod-init --config <file> [--bind <addr>] [--port <port>] [--configfs <dir>] [--ccel <file>]
  --bind      address to listen on (default 0.0.0.0)
  --port      port (default 7000)
  --configfs  report directory root (default /sys/kernel/config/tsm/report)
  --ccel      event log (default /sys/firmware/acpi/tables/data/CCEL)";

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut config = None;
    let mut out = Args {
        config: PathBuf::new(),
        bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port: DEFAULT_PORT,
        configfs: PathBuf::from(crate::tsm::CONFIGFS_REPORT),
        ccel: PathBuf::from(crate::tsm::CCEL_DATA),
    };
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--config" => config = Some(PathBuf::from(value()?)),
            "--bind" => {
                let v = value()?;
                out.bind = v.parse().map_err(|_| format!("--bind {v:?} is not an IP address"))?;
            }
            "--port" => {
                let v = value()?;
                out.port = v.parse().ok().filter(|p| *p != 0).ok_or(format!("--port {v:?} is not a port"))?;
            }
            "--configfs" => out.configfs = PathBuf::from(value()?),
            "--ccel" => out.ccel = PathBuf::from(value()?),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    out.config = config.ok_or("--config is required")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"volume_image":"/var/lib/pod/volume.img","volume_size_gb":4,
        "mapper_name":"pod","mount":"/srv/pod","owner_hint":"/var/lib/pod/owner.hint",
        "tls_root":"/srv/pod/tls/root.pem"}"#;

    fn with(key: &str, value: serde_json::Value) -> Result<Config, ConfigError> {
        let mut v: serde_json::Value = serde_json::from_str(GOOD).unwrap();
        v[key] = value;
        Config::parse(&serde_json::to_vec(&v).unwrap())
    }

    #[test]
    fn parses_with_defaults() {
        let c = Config::parse(GOOD.as_bytes()).unwrap();
        assert_eq!(c.owner_path(), PathBuf::from("/srv/pod/pod/owner"));
        assert_eq!(c.mapper_path(), PathBuf::from("/dev/mapper/pod"));
        assert_eq!(c.open_hook, None);
        assert_eq!(c.owner_hint, Some(PathBuf::from("/var/lib/pod/owner.hint")));
    }

    #[test]
    fn without_a_hint_file_the_hint_is_the_headers() {
        let mut v: serde_json::Value = serde_json::from_str(GOOD).unwrap();
        v.as_object_mut().unwrap().remove("owner_hint");
        let c = Config::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(c.owner_hint, None);
        assert!(with("owner_hint", "owner.hint".into()).is_err());
    }

    #[test]
    fn refuses_unsafe_names_and_paths() {
        assert!(with("mapper_name", "../x".into()).is_err());
        assert!(with("mapper_name", "".into()).is_err());
        assert!(with("mount", "srv/pod".into()).is_err());
        assert!(with("owner_file", "../owner".into()).is_err());
        assert!(with("owner_file", "/etc/owner".into()).is_err());
        assert!(with("open_hook", "hook.sh".into()).is_err());
        assert!(with("volume_size_gb", 0.into()).is_err());
        assert!(with("surprise", 1.into()).is_err());
    }

    #[test]
    fn args_default_to_all_ipv4_on_7000() {
        let a = parse_args(["--config".to_owned(), "/etc/pod.json".to_owned()]).unwrap();
        assert_eq!((a.bind, a.port), (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7000));
        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(["--config", "c", "--port", "0"].map(str::to_owned)).is_err());
        assert!(parse_args(["--config", "c", "--what"].map(str::to_owned)).is_err());
    }
}
