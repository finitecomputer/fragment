//! The volume, behind a trait: the real one is LUKS through cryptsetup
//! (src/luks.rs); the tests use a fake. Its state is read from the
//! system each time and never remembered, so pod-init started on a
//! machine whose volume is already open says so.

use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeError {
    /// The key does not open this volume.
    WrongKey,
    /// A step failed: which, and what it said (never the key).
    Step { step: &'static str, detail: String },
}

impl fmt::Display for VolumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKey => write!(f, "the key does not open the volume"),
            Self::Step { step, detail } => write!(f, "{step}: {detail}"),
        }
    }
}

impl std::error::Error for VolumeError {}

pub trait Volume {
    /// Blank (no image, or no LUKS header), open (mapped and mounted at
    /// the mount point), or otherwise locked.
    fn state(&self) -> Result<State, VolumeError>;
    /// Format a new volume under `key`, make its filesystem, open and
    /// mount it. Only for a blank volume.
    fn create(&self, key: &DiskKey) -> Result<(), VolumeError>;
    /// Open the existing volume with `key` and mount it.
    fn open(&self, key: &DiskKey) -> Result<(), VolumeError>;
    /// Unmount and close it.
    fn close(&self) -> Result<(), VolumeError>;
    /// The owner hint kept in the volume's header, outside the encrypted
    /// area (no `owner_hint` file in the config); None if it has none.
    /// Like the file it replaces, the host can rewrite it.
    fn header_hint(&self) -> Result<Option<[u8; 32]>, VolumeError> {
        Err(no_header_hint())
    }
    /// Record the owner hint in the volume's header.
    fn set_header_hint(&self, owner: &[u8; 32]) -> Result<(), VolumeError> {
        let _ = owner;
        Err(no_header_hint())
    }
}

fn no_header_hint() -> VolumeError {
    VolumeError::Step { step: "header hint", detail: "this volume keeps no hint in its header".into() }
}
