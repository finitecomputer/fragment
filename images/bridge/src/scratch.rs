//! A scratch folder bounded by bytes and files (`limits::SCRATCH_MAX_BYTES`,
//! `SCRATCH_FILES_MAX`): the message files downloaded for a runtime, and
//! Hermes' uploads and fetches. No save keeps it, but it is on the
//! computer's disk, which has filled before.
//!
//! - **A transfer reserves its bytes and a file first** (`reserve`), until
//!   it is done or given up, its retries and waits among them: two at once
//!   never take the same room. Its `.part` is never let go meanwhile; one
//!   no reservation names (a life that was cut) may go at once.
//! - **A file in use is never let go** (`InUse`): a running turn's files
//!   (the driver's, until the engine ends the turn), a reply's while it is
//!   uploaded, an upload Hermes may still send (the relay's media table).
//! - **Room is made from the rest**, those used least lately first (their
//!   modification time, which `touch` moves). When they are not enough, the
//!   reservation is refused and nothing goes: never a file in use, never
//!   past the bound.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::limits;

/// The suffix of a file still being written.
pub const PART: &str = "part";

/// Files in use, by path, process-wide: each `InUse` naming one counts.
static IN_USE: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

/// Files kept in use while it lives (scratch never lets them go).
#[derive(Debug)]
pub struct InUse(Vec<PathBuf>);

impl InUse {
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> InUse {
        let paths: Vec<PathBuf> = paths.into_iter().collect();
        let mut used = IN_USE.lock().expect("the files in use");
        for p in &paths {
            *used.entry(p.clone()).or_insert(0) += 1;
        }
        InUse(paths)
    }
}

impl Drop for InUse {
    fn drop(&mut self) {
        let mut used = IN_USE.lock().expect("the files in use");
        for p in &self.0 {
            if let Some(n) = used.get_mut(p) {
                *n -= 1;
                if *n == 0 {
                    used.remove(p);
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Scratch {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    /// Each live reservation's part and its bytes. Its lock is held while
    /// room is made, so making room, and taking a file already there into
    /// use (`reuse`), happen one at a time.
    reserved: Arc<Mutex<BTreeMap<PathBuf, u64>>>,
}

/// Why a file has no room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRoom {
    /// Larger than the whole folder may be.
    TooLarge { size: u64, max: u64 },
    /// What is in use and reserved leaves too little.
    Full { size: u64 },
    /// The folder did not read, or a file did not go.
    Disk(String),
}

impl std::fmt::Display for NoRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRoom::TooLarge { size, max } => write!(f, "{size} bytes, past the scratch's {max}"),
            NoRoom::Full { size } => write!(f, "no room for {size} bytes beside the files in use"),
            NoRoom::Disk(e) => write!(f, "the scratch folder: {e}"),
        }
    }
}

/// A transfer's room, held until it drops: its bytes and its part.
#[derive(Debug)]
pub struct Reservation {
    reserved: Arc<Mutex<BTreeMap<PathBuf, u64>>>,
    part: PathBuf,
}

impl Reservation {
    pub fn part(&self) -> &Path {
        &self.part
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.reserved.lock().expect("the reservations").remove(&self.part);
    }
}

impl Scratch {
    /// The folder, made if it is not there yet.
    pub fn new(dir: PathBuf, max_bytes: u64) -> std::io::Result<Scratch> {
        Scratch::bounded(dir, max_bytes, limits::SCRATCH_FILES_MAX)
    }

    pub fn bounded(dir: PathBuf, max_bytes: u64, max_files: usize) -> std::io::Result<Scratch> {
        std::fs::create_dir_all(&dir)?;
        Ok(Scratch { dir, max_bytes, max_files, reserved: Arc::new(Mutex::new(BTreeMap::new())) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Room for `size` bytes and one file, written at `<name>.part` and
    /// then renamed `<name>`: the files used least lately and in use by
    /// nothing go until both fit, or none goes and it is refused.
    pub fn reserve(&self, name: &str, size: u64) -> Result<Reservation, NoRoom> {
        if size > self.max_bytes {
            return Err(NoRoom::TooLarge { size, max: self.max_bytes });
        }
        let part = self.dir.join(format!("{name}.{PART}"));
        let mut reserved = self.reserved.lock().expect("the reservations");
        let files = self.files(&reserved).map_err(|e| NoRoom::Disk(e.to_string()))?;
        let mut bytes: u64 = files.iter().map(|f| f.size).sum::<u64>() + reserved.values().sum::<u64>();
        let mut count = files.len() + reserved.len();
        let fits = |bytes: u64, count: usize| bytes.saturating_add(size) <= self.max_bytes && count < self.max_files;
        // held while files are chosen and removed: a file taken into use
        // meanwhile (`pinned`) is either kept, or seen gone by its taker
        let used = IN_USE.lock().expect("the files in use");
        let mut free: Vec<&Entry> = files.iter().filter(|f| !used.contains_key(&f.path)).collect();
        free.sort_by_key(|f| f.used);
        let freeable: u64 = free.iter().map(|f| f.size).sum();
        if !fits(bytes.saturating_sub(freeable), count - free.len()) {
            return Err(NoRoom::Full { size });
        }
        let mut freed = (0usize, 0u64);
        for f in free {
            if fits(bytes, count) {
                break;
            }
            match std::fs::remove_file(&f.path) {
                Ok(()) => freed = (freed.0 + 1, freed.1 + f.size),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(NoRoom::Disk(format!("{}: {e}", f.path.display()))),
            }
            bytes -= f.size;
            count -= 1;
        }
        drop(used);
        if freed.0 > 0 {
            crate::ev!("scratch.freed", { "dir": self.dir.display().to_string(), "files": freed.0, "bytes": freed.1 });
        }
        assert!(fits(bytes, count), "the room counted is made");
        reserved.insert(part.clone(), size);
        Ok(Reservation { reserved: self.reserved.clone(), part })
    }

    /// `<name>`, already here at `size` bytes, taken into use (and so used
    /// last), or `None`.
    pub fn reuse(&self, name: &str, size: u64) -> Option<(PathBuf, InUse)> {
        let path = self.dir.join(name);
        let _making_room = self.reserved.lock().expect("the reservations");
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == size) {
            return None;
        }
        touch(&path);
        let used = InUse::new([path.clone()]);
        Some((path, used))
    }

    /// Every file in the folder but the reserved parts (counted as their
    /// reservations).
    fn files(&self, reserved: &BTreeMap<PathBuf, u64>) -> std::io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        // bounded by the folder: max_files each reservation keeps it to
        for e in std::fs::read_dir(&self.dir)? {
            let Ok(e) = e else { continue };
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() || reserved.contains_key(&e.path()) {
                continue;
            }
            out.push(Entry { path: e.path(), size: meta.len(), used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) });
        }
        Ok(out)
    }
}

/// `file`, pinned (`LocalFile::pin`): as it was if it already is, else
/// taken into use now if it is still there (a copy kept unpinned, as the
/// relay's table keeps a message's files, sent on in a reply). Atomic with
/// any scratch making room: never one it lets go after.
pub fn pinned(file: crate::runtime::LocalFile) -> crate::runtime::LocalFile {
    if file.pin.0.is_some() {
        return file;
    }
    let used = InUse::new([file.path.clone()]);
    if !file.path.is_file() {
        // gone already: its post says it could not be attached
        return file;
    }
    crate::runtime::LocalFile { pin: crate::runtime::Kept(Some(Arc::new(used))), ..file }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(dir: &Path, name: &str, size: usize, age_s: u64) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![7u8; size]).unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
        p
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bridge-scratch-{tag}-{}-{:x}", std::process::id(), crate::net::jitter_seed()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_least_lately_used_go_first() {
        let dir = tmp("lru");
        let s = Scratch::new(dir.clone(), 100).unwrap();
        let old = file(&dir, "old", 40, 300);
        let used = file(&dir, "used", 40, 200);
        let new = file(&dir, "new", 10, 100);
        touch(&used);
        drop(s.reserve("a", 50).unwrap());
        assert!(!old.exists() && used.exists() && new.exists(), "the oldest went, the one used just now stayed");
        drop(s.reserve("b", 60).unwrap());
        assert!(used.exists() && !new.exists(), "then the next least lately used: 40 + 60 fits");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_in_use_never_goes_and_too_little_room_is_refused_whole() {
        let dir = tmp("use");
        let s = Scratch::new(dir.clone(), 100).unwrap();
        let live = file(&dir, "live", 60, 900);
        let idle = file(&dir, "idle", 30, 100);
        let held = InUse::new([live.clone()]);
        assert_eq!(s.reserve("x", 50).unwrap_err(), NoRoom::Full { size: 50 }, "60 in use leaves 40");
        assert!(live.exists() && idle.exists(), "refused, nothing went");
        let r = s.reserve("y", 40).unwrap();
        assert!(live.exists() && !idle.exists(), "the idle one went, never the older one in use");
        drop((r, held));
        drop(s.reserve("z", 100).unwrap());
        assert!(!live.exists(), "let go, it goes like any other");
        assert_eq!(s.reserve("w", 101).unwrap_err(), NoRoom::TooLarge { size: 101, max: 100 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reservations_hold_their_room_until_they_drop() {
        let dir = tmp("reserve");
        let s = Scratch::bounded(dir.clone(), 100, 3).unwrap();
        let a = s.reserve("a", 80).unwrap();
        assert_eq!(s.reserve("b", 80).unwrap_err(), NoRoom::Full { size: 80 }, "two at once never take the same room");
        // its part, written and still for any while, is never let go
        std::fs::write(a.part(), vec![1u8; 80]).unwrap();
        let f = std::fs::File::options().write(true).open(a.part()).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        assert!(s.reserve("c", 30).is_err() && a.part().exists());
        let (b, c) = (s.reserve("b", 10).unwrap(), s.reserve("c", 10).unwrap());
        assert_eq!(s.reserve("d", 0).unwrap_err(), NoRoom::Full { size: 0 }, "three files at most, reservations counted");
        drop((b, c));
        let part = a.part().to_path_buf();
        drop(a);
        // a part no reservation names (a cut life's) goes like any file
        drop(s.reserve("e", 100).unwrap());
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_reused_is_taken_into_use() {
        let dir = tmp("reuse");
        let s = Scratch::new(dir.clone(), 100).unwrap();
        let f = file(&dir, "f", 50, 500);
        assert!(s.reuse("f", 49).is_none() && s.reuse("g", 50).is_none());
        let (path, used) = s.reuse("f", 50).unwrap();
        assert_eq!(path, f);
        assert!(s.reserve("x", 60).is_err(), "in use, so it stays");
        drop(used);
        drop(s.reserve("x", 60).unwrap());
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
