//! A scratch folder bounded by bytes and files (`limits::SCRATCH_MAX_BYTES`,
//! `SCRATCH_FILES_MAX`): the message files downloaded for a runtime, and
//! Hermes' uploads and fetches. No save keeps it, but it is on the
//! computer's disk, which has filled before: before a file is written, the
//! files used least lately (by their modification time, which `touch`
//! moves) go until it fits.
//!
//! A transfer writes `<name>.part` and renames it whole once checked
//! (transfer.rs). A part still moving is never let go here; one left by a
//! life that was cut is, once still for twice the stall bound. Transfers
//! that run at once each make room for their own bytes, so the folder may
//! pass its bound by what is still in flight, never by more.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::limits;

/// The suffix of a file still being written.
pub const PART: &str = "part";

#[derive(Debug)]
pub struct Scratch {
    dir: PathBuf,
    max_bytes: u64,
    /// One pass of making room at a time.
    pass: tokio::sync::Mutex<()>,
}

/// Why a file has no room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRoom {
    /// Larger than the whole folder may be.
    TooLarge { size: u64, max: u64 },
    /// The folder did not read.
    Disk(String),
}

impl std::fmt::Display for NoRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRoom::TooLarge { size, max } => write!(f, "{size} bytes, past the scratch's {max}"),
            NoRoom::Disk(e) => write!(f, "the scratch folder: {e}"),
        }
    }
}

impl Scratch {
    /// The folder, made if it is not there yet.
    pub fn new(dir: PathBuf, max_bytes: u64) -> std::io::Result<Scratch> {
        std::fs::create_dir_all(&dir)?;
        Ok(Scratch { dir, max_bytes, pass: tokio::sync::Mutex::new(()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Room for `size` more bytes and one more file: the files used least
    /// lately go until both fit. `keep` is never let go (the files of the
    /// message the room is made for, downloaded already).
    pub async fn make_room(&self, size: u64, keep: &[PathBuf]) -> Result<(), NoRoom> {
        if size > self.max_bytes {
            return Err(NoRoom::TooLarge { size, max: self.max_bytes });
        }
        let _pass = self.pass.lock().await;
        let mut files = self.files().await.map_err(|e| NoRoom::Disk(e.to_string()))?;
        let mut bytes: u64 = files.iter().map(|f| f.size).sum();
        // oldest first; a part still moving is not among them (`files`)
        files.sort_by_key(|f| f.used);
        let mut count = files.len();
        let mut freed = Vec::new();
        for f in files.iter().filter(|f| f.evictable && !keep.contains(&f.path)) {
            if bytes.saturating_add(size) <= self.max_bytes && count < limits::SCRATCH_FILES_MAX {
                break;
            }
            match tokio::fs::remove_file(&f.path).await {
                Ok(()) => {
                    bytes = bytes.saturating_sub(f.size);
                    count -= 1;
                    freed.push(f.size);
                }
                // gone already (another pass, or its runtime let go of it)
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    bytes = bytes.saturating_sub(f.size);
                    count -= 1;
                }
                Err(e) => return Err(NoRoom::Disk(format!("{}: {e}", f.path.display()))),
            }
        }
        if !freed.is_empty() {
            crate::ev!("scratch.freed", { "dir": self.dir.display().to_string(), "files": freed.len(), "bytes": freed.iter().sum::<u64>() });
        }
        // what is left is in flight, or the message's own: past the bound
        // by those alone
        Ok(())
    }

    /// Every file in the folder, with whether it may go: a part only once
    /// still for twice the stall bound (a life's that was cut).
    async fn files(&self) -> std::io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.dir).await?;
        let still = SystemTime::now().checked_sub(Duration::from_millis(2 * limits::TRANSFER_STALL_MS)).unwrap_or(SystemTime::UNIX_EPOCH);
        // bounded by the folder: SCRATCH_FILES_MAX each pass keeps it to,
        // and the parts in flight
        while let Some(e) = dir.next_entry().await? {
            let Ok(meta) = e.metadata().await else { continue };
            if !meta.is_file() {
                continue;
            }
            let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let part = e.path().extension().is_some_and(|x| x == PART);
            out.push(Entry { path: e.path(), size: meta.len(), used, evictable: !part || used < still });
        }
        Ok(out)
    }
}

/// A file used now: the last to go.
pub fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
    evictable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &Path, name: &str, size: usize, age_s: u64) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![7u8; size]).unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
        p
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bridge-scratch-{tag}-{}-{}", std::process::id(), crate::net::jitter_seed()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[tokio::test]
    async fn the_least_lately_used_go_first() {
        let dir = tmp("lru");
        let s = Scratch::new(dir.clone(), 100).unwrap();
        let old = file(&dir, "old", 40, 300);
        let used = file(&dir, "used", 40, 200);
        let new = file(&dir, "new", 10, 100);
        touch(&used);
        s.make_room(50, &[]).await.unwrap();
        assert!(!old.exists() && used.exists() && new.exists(), "the oldest went, the one used just now stayed");
        s.make_room(60, &[]).await.unwrap();
        assert!(used.exists() && !new.exists(), "then the next least lately used: 40 + 60 fits");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_part_in_flight_and_a_kept_file_stay() {
        let dir = tmp("keep");
        let s = Scratch::new(dir.clone(), 100).unwrap();
        let moving = file(&dir, "a.part", 50, 0);
        let dead = file(&dir, "b.part", 20, 3 * limits::TRANSFER_STALL_MS / 1000);
        let mine = file(&dir, "mine", 20, 500);
        s.make_room(90, std::slice::from_ref(&mine)).await.unwrap();
        assert!(moving.exists() && mine.exists() && !dead.exists(), "only the cut life's part went");
        assert_eq!(s.make_room(101, &[]).await, Err(NoRoom::TooLarge { size: 101, max: 100 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn files_are_bounded_by_count_too() {
        let dir = tmp("count");
        let s = Scratch::new(dir.clone(), u64::MAX).unwrap();
        for i in 0..limits::SCRATCH_FILES_MAX + 3 {
            file(&dir, &format!("f{i}"), 1, (limits::SCRATCH_FILES_MAX + 10 - i) as u64);
        }
        s.make_room(1, &[]).await.unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), limits::SCRATCH_FILES_MAX - 1, "room for one more");
        assert!(!dir.join("f0").exists() && dir.join(format!("f{}", limits::SCRATCH_FILES_MAX + 2)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
