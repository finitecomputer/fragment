//! A fresh directory under the system's temporary one, removed on drop:
//! the tests' stand-in for configfs, a volume's files and a hook. Not
//! the `tempfile` crate, which the root lock does not carry.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fragment-pod-init-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
