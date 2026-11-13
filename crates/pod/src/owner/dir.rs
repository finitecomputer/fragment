//! The owner's state directory: `owner.key`, `disk.key`, `state.json`,
//! and the `policy.json` the owner writes. Key files are created 0600
//! and never overwritten; the directory is created 0700.

use anyhow::{bail, Context, Result};
use crate::unlock::keys::DiskKey;
use crate::unlock::sign::OwnerKey;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub const OWNER_KEY: &str = "owner.key";
pub const DISK_KEY: &str = "disk.key";
pub const STATE: &str = "state.json";
pub const POLICY: &str = "policy.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PodState {
    /// The release seq of the last unlock: never release a key to an
    /// older one (a rollback to a release with a known flaw).
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

    pub fn write_owner_key(&self, key: &OwnerKey) -> Result<()> {
        let text = Zeroizing::new(format!("{}\n", key.to_hex().as_str()));
        self.create_secret(OWNER_KEY, &text)
    }

    pub fn owner_key(&self) -> Result<OwnerKey> {
        OwnerKey::from_hex(&self.read_secret(OWNER_KEY)?).with_context(|| format!("{}", self.path(OWNER_KEY).display()))
    }

    /// The public key, if there is an owner key here.
    pub fn owner_public(&self) -> Result<Option<[u8; 32]>> {
        if !self.path(OWNER_KEY).exists() {
            return Ok(None);
        }
        self.owner_key().map(|k| Some(k.public()))
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

    /// Replaced whole through a rename, so a crash keeps the old record.
    pub fn write_state(&self, state: &PodState) -> Result<()> {
        self.ensure()?;
        let path = self.path(STATE);
        let tmp = self.path("state.json.tmp");
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(state)?)?;
        f.sync_all()?;
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
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
    fn keys_are_private_and_never_overwritten() {
        let tmp = Scratch::new("keys");
        let dir = Dir::new(&tmp.0.join("pod"));
        let k = OwnerKey::generate().unwrap();
        dir.write_owner_key(&k).unwrap();
        assert_eq!(mode(&tmp.0.join("pod")), 0o700);
        assert_eq!(mode(&dir.path(OWNER_KEY)), 0o600);
        assert_eq!(dir.owner_public().unwrap(), Some(k.public()));
        let err = dir.write_owner_key(&OwnerKey::generate().unwrap()).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(dir.owner_key().unwrap().public(), k.public());

        let d = DiskKey::random().unwrap();
        dir.write_disk_key(&d).unwrap();
        assert_eq!(mode(&dir.path(DISK_KEY)), 0o600);
        assert_eq!(dir.disk_key().unwrap().as_bytes(), d.as_bytes());
        assert!(dir.write_disk_key(&DiskKey::random().unwrap()).is_err());
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
}
