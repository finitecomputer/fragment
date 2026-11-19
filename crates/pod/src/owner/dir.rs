//! A pod's directory on the owner's machine: `disk.key`, `state.json`,
//! and the `policy.json` the owner writes. The disk key is created 0600
//! and never overwritten; the directory is created 0700. The owner's
//! signing key is not here: it is the caller's (the fragment CLI's own).

use anyhow::{bail, Context, Result};
use crate::unlock::keys::DiskKey;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub const DISK_KEY: &str = "disk.key";
pub const STATE: &str = "state.json";
pub const POLICY: &str = "policy.json";
pub const STATE_LOCK: &str = "state.json.lock";
const STATE_TMP: &str = "state.json.tmp";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PodState {
    /// The floor: the newest release seq this owner checked and sent a key
    /// to, whether or not the pod's answer came back. A key never goes to
    /// an older one (a rollback to a release with a known flaw).
    #[serde(default)]
    pub last_unlocked_seq: Option<u64>,
}

pub struct Dir(PathBuf);

impl Dir {
    pub fn new(path: &Path) -> Self {
        Self(path.to_owned())
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn ensure(&self) -> Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.0)
            .with_context(|| format!("creating {}", self.0.display()))
    }

    /// A new 0600 file holding `text`; refuses if `name` exists.
    fn create_secret(&self, name: &str, text: &str) -> Result<()> {
        self.ensure()?;
        let path = self.path(name);
        let mut f = match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => bail!("{} exists; refusing to overwrite a key", path.display()),
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        };
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).with_context(|| format!("writing {}", path.display()))
    }

    fn read_secret(&self, name: &str) -> Result<Zeroizing<String>> {
        let path = self.path(name);
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Zeroizing::new(text))
    }

    pub fn has_disk_key(&self) -> bool {
        self.path(DISK_KEY).exists()
    }

    pub fn write_disk_key(&self, key: &DiskKey) -> Result<()> {
        let text = Zeroizing::new(format!("{}\n", key.to_hex().as_str()));
        self.create_secret(DISK_KEY, &text)
    }

    pub fn disk_key(&self) -> Result<DiskKey> {
        DiskKey::from_hex(&self.read_secret(DISK_KEY)?).with_context(|| format!("{}", self.path(DISK_KEY).display()))
    }

    pub fn state(&self) -> Result<PodState> {
        let path = self.path(STATE);
        match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{}", path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PodState::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Raises the floor to `state`'s, never lowers it: under an exclusive
    /// lock on `state.json.lock`, the saved seq and this one's maximum is
    /// written to a synced temporary file, renamed over `state.json`, and
    /// the directory synced. So two runs finishing in either order keep the
    /// higher, and a crash keeps the old record or the new one. Answers
    /// what is saved now.
    pub fn write_state(&self, state: &PodState) -> Result<PodState> {
        self.ensure()?;
        let path = self.path(STATE);
        let at = |what: &'static str, p: &Path| format!("{what} {}", p.display());
        let lock_path = self.path(STATE_LOCK);
        let lock = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&lock_path).with_context(|| at("opening", &lock_path))?;
        lock.lock().with_context(|| at("locking", &lock_path))?;
        let saved = self.state()?;
        let merged = PodState { last_unlocked_seq: saved.last_unlocked_seq.max(state.last_unlocked_seq) };
        let tmp = self.path(STATE_TMP);
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).with_context(|| at("creating", &tmp))?;
        f.write_all(&serde_json::to_vec_pretty(&merged)?).and_then(|_| f.sync_all()).with_context(|| at("writing", &tmp))?;
        fs::rename(&tmp, &path).with_context(|| at("writing", &path))?;
        fs::File::open(&self.0).and_then(|d| d.sync_all()).with_context(|| at("syncing", &self.0))?;
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fresh directory under the system's temporary one, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("fragment-pod-dir-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_disk_key_is_private_and_never_overwritten() {
        let tmp = Scratch::new("keys");
        let dir = Dir::new(&tmp.0.join("pod"));
        assert!(!dir.has_disk_key());
        let d = DiskKey::random().unwrap();
        dir.write_disk_key(&d).unwrap();
        assert_eq!(mode(&tmp.0.join("pod")), 0o700);
        assert_eq!(mode(&dir.path(DISK_KEY)), 0o600);
        assert_eq!(dir.disk_key().unwrap().as_bytes(), d.as_bytes());
        let err = dir.write_disk_key(&DiskKey::random().unwrap()).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(dir.disk_key().unwrap().as_bytes(), d.as_bytes());
    }

    #[test]
    fn state_defaults_and_round_trips() {
        let tmp = Scratch::new("state");
        let dir = Dir::new(&tmp.0);
        assert_eq!(dir.state().unwrap(), PodState::default());
        dir.write_state(&PodState { last_unlocked_seq: Some(3) }).unwrap();
        assert_eq!(dir.state().unwrap().last_unlocked_seq, Some(3));
        assert_eq!(mode(&dir.path(STATE)), 0o600);
    }

    /// Two runs that finish out of order never lower the floor: a lower
    /// seq written over a higher one keeps the higher.
    #[test]
    fn the_floor_is_never_lowered() {
        let tmp = Scratch::new("floor");
        let dir = Dir::new(&tmp.0);
        dir.write_state(&PodState { last_unlocked_seq: Some(5) }).unwrap();
        dir.write_state(&PodState { last_unlocked_seq: Some(3) }).unwrap();
        assert_eq!(dir.state().unwrap().last_unlocked_seq, Some(5));
        dir.write_state(&PodState { last_unlocked_seq: None }).unwrap();
        assert_eq!(dir.state().unwrap().last_unlocked_seq, Some(5));
        dir.write_state(&PodState { last_unlocked_seq: Some(6) }).unwrap();
        assert_eq!(dir.state().unwrap().last_unlocked_seq, Some(6));
    }
}
