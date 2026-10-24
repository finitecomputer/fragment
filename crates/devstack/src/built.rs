//! Which build of the cell `cell/build/` holds: the release's, which a
//! deploy uploads and the e2e stages, or `cargo xtask dev`'s, which only
//! `wrangler dev` under it serves (xtask/src/build.rs, `Profile`). Each
//! build removes the mark before it starts and writes it when it is done,
//! naming its profile and its Wasm's SHA-256, so a build that failed, is
//! under way, or was overwritten by another leaves no mark that passes.
//!
//! `cargo xtask dev`'s rebuild, the e2e and a deploy share one `build/`,
//! so a lock beside it (`build.lock`, outside it: no copy or upload holds
//! it) keeps them apart: a build holds it alone from clearing the mark to
//! writing it (`building`), and a reader holds it shared while it uses the
//! files (`reading`): the e2e's staging while it copies them, a deploy
//! through its every wrangler try. Each waits for the other, so no build
//! writes among another's, its mark names the bytes it built, and nothing
//! replaces the files a reader checked before it is done with them.

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The mark, in the build's directory.
pub const MARK: &str = "built";
/// The Wasm it names, in the build's directory.
pub const WASM: &str = "index_bg.wasm";
/// The profile of the build a deploy and the e2e take, and no other.
pub const RELEASE: &str = "release";
/// The most a mark may hold: a profile's name and a hash.
const MARK_MAX_BYTES: u64 = 256;
/// The most Wasm a mark is checked against: far over any build's (the
/// release's is held to 6.5 MB, dev's is about 7.7 MB).
const WASM_MAX_BYTES: u64 = 64 << 20;

#[derive(Debug)]
pub enum BuiltError {
    /// No mark: no build, or one that failed or is under way.
    Unmarked { dir: PathBuf },
    /// A mark that is not `<profile> <sha256>`.
    Malformed { file: PathBuf },
    /// A build of another profile: `cargo xtask dev`'s, most likely.
    NotRelease { dir: PathBuf, profile: String },
    /// The Wasm is not the one its build marked: another build overwrote it.
    Moved { dir: PathBuf, marked: String, actual: String },
    TooLarge { file: PathBuf, max: u64 },
    Io { what: String, source: io::Error },
}

impl fmt::Display for BuiltError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const BUILD: &str = "run `cargo xtask build`, with no `cargo xtask dev` rebuilding the cell meanwhile";
        match self {
            BuiltError::Unmarked { dir } => write!(f, "{} holds no finished build of the cell: {BUILD}", dir.display()),
            BuiltError::Malformed { file } => write!(f, "{} is not a build's mark (`<profile> <sha256>`): {BUILD}", file.display()),
            BuiltError::NotRelease { dir, profile } => {
                write!(f, "{} holds the cell's {profile} build (`cargo xtask dev`'s), not the release's: only the release's is deployed or staged; {BUILD}", dir.display())
            }
            BuiltError::Moved { dir, marked, actual } => write!(f, "{}/{WASM} is {actual}, not the {marked} its build marked (another build wrote over it): {BUILD}", dir.display()),
            BuiltError::TooLarge { file, max } => write!(f, "{} is over {max} bytes", file.display()),
            BuiltError::Io { what, source } => write!(f, "{what}: {source}"),
        }
    }
}

/// Each message carries its cause's text, so it names no `source`.
impl std::error::Error for BuiltError {}

fn read_bounded(file: &Path, max: u64) -> Result<Vec<u8>, BuiltError> {
    let f = fs::File::open(file).map_err(|source| BuiltError::Io { what: format!("open {}", file.display()), source })?;
    let mut bytes = Vec::new();
    f.take(max + 1).read_to_end(&mut bytes).map_err(|source| BuiltError::Io { what: format!("read {}", file.display()), source })?;
    if bytes.len() as u64 > max {
        return Err(BuiltError::TooLarge { file: file.to_path_buf(), max });
    }
    Ok(bytes)
}

fn wasm_sha256(dir: &Path) -> Result<String, BuiltError> {
    let wasm = read_bounded(&dir.join(WASM), WASM_MAX_BYTES)?;
    Ok(Sha256::digest(&wasm).iter().map(|b| format!("{b:02x}")).collect())
}

/// The lock of `dir`, beside it: `cell/build.lock` for `cell/build`.
fn lock_path(dir: &Path) -> PathBuf {
    dir.with_extension("lock")
}

/// How a lock is held: by a build, alone, or by a reader, with others.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    Alone,
    Shared,
}

/// `dir`'s lock, held as `hold` asks; one held otherwise is waited for,
/// saying so.
fn lock(dir: &Path, hold: Hold) -> Result<File, BuiltError> {
    let shared = hold == Hold::Shared;
    let path = lock_path(dir);
    let io = |what: &str, source| BuiltError::Io { what: format!("{what} {}", path.display()), source };
    let file = fs::OpenOptions::new().write(true).create(true).truncate(false).open(&path).map_err(|e| io("open", e))?;
    match if shared { file.try_lock_shared() } else { file.try_lock() } {
        Ok(()) => return Ok(file),
        Err(TryLockError::Error(e)) => return Err(io("lock", e)),
        Err(TryLockError::WouldBlock) => eprintln!("waiting for {}: a build of the cell, or the e2e or a deploy reading it, holds it", path.display()),
    }
    if shared { file.lock_shared() } else { file.lock() }.map_err(|e| io("lock", e))?;
    Ok(file)
}

/// A build of `dir` under way (`building`): it holds `dir`'s lock alone
/// until it is dropped. Dropped before `finish` (the build failed, or
/// stopped), it leaves no mark; its process's end, however it comes,
/// releases the lock.
#[must_use = "the build holds its directory until it is dropped"]
pub struct Building {
    dir: PathBuf,
    profile: &'static str,
    _lock: File,
}

/// Before a build of `profile` writes into `dir`: it waits for any other
/// build and every reader, then clears the mark, so whatever `dir` holds
/// is no finished build until `finish`.
pub fn building(dir: &Path, profile: &'static str) -> Result<Building, BuiltError> {
    fs::create_dir_all(dir).map_err(|source| BuiltError::Io { what: format!("create {}", dir.display()), source })?;
    let lock = lock(dir, Hold::Alone)?;
    clear(dir)?;
    Ok(Building { dir: dir.to_path_buf(), profile, _lock: lock })
}

impl Building {
    /// The build is done writing: its mark names the Wasm it wrote, which
    /// no other build could replace meanwhile; then it lets `dir` go.
    pub fn finish(self) -> Result<(), BuiltError> {
        mark(&self.dir, self.profile)
    }
}

/// A reader's hold on `dir` (`reading`): no build writes it until this is
/// dropped.
#[must_use = "the reader holds the directory until it is dropped"]
pub struct Reading {
    _lock: File,
}

/// Before the files in `dir` are read and used (staged, uploaded): it
/// waits for a build under way to finish, then keeps any other from
/// starting until it is dropped.
pub fn reading(dir: &Path) -> Result<Reading, BuiltError> {
    Ok(Reading { _lock: lock(dir, Hold::Shared)? })
}

/// Before a build writes into `dir`: whatever it held is no finished build.
fn clear(dir: &Path) -> Result<(), BuiltError> {
    match fs::remove_file(dir.join(MARK)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(BuiltError::Io { what: format!("remove {}", dir.join(MARK).display()), source: e }),
        _ => Ok(()),
    }
}

/// After a build of `profile` finished writing `dir`.
fn mark(dir: &Path, profile: &str) -> Result<(), BuiltError> {
    assert!(!profile.is_empty() && !profile.contains(char::is_whitespace), "a profile is one word");
    let line = format!("{profile} {}\n", wasm_sha256(dir)?);
    fs::write(dir.join(MARK), line).map_err(|source| BuiltError::Io { what: format!("write {}", dir.join(MARK).display()), source })
}

/// The profile of the finished build in `dir`, its Wasm the one it marked.
pub fn profile(dir: &Path) -> Result<String, BuiltError> {
    let file = dir.join(MARK);
    let text = match read_bounded(&file, MARK_MAX_BYTES) {
        Err(BuiltError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => return Err(BuiltError::Unmarked { dir: dir.to_path_buf() }),
        r => r?,
    };
    let text = String::from_utf8(text).map_err(|_| BuiltError::Malformed { file: file.clone() })?;
    let mut words = text.split_whitespace();
    let (Some(profile), Some(marked), None) = (words.next(), words.next(), words.next()) else {
        return Err(BuiltError::Malformed { file });
    };
    if marked.len() != 64 || !marked.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(BuiltError::Malformed { file });
    }
    let actual = wasm_sha256(dir)?;
    if actual != marked {
        return Err(BuiltError::Moved { dir: dir.to_path_buf(), marked: marked.into(), actual });
    }
    Ok(profile.to_string())
}

/// That `dir` holds the release's finished build: what a deploy uploads
/// and the e2e stages, never `cargo xtask dev`'s.
pub fn release(dir: &Path) -> Result<(), BuiltError> {
    match profile(dir)? {
        p if p == RELEASE => Ok(()),
        profile => Err(BuiltError::NotRelease { dir: dir.to_path_buf(), profile }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fragment-built-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Goal: a deploy and the e2e take the release's finished build alone.
    /// Valid: the release's, marked, then checked again unchanged (a replay).
    /// Invalid: no mark, dev's build, a Wasm another build wrote over, a
    /// mark that is not one. Restart: a build that starts clears the mark,
    /// so one that fails or is under way passes as none.
    #[test]
    fn only_the_release_build_passes() {
        let dir = scratch("release");
        fs::write(dir.join(WASM), b"\0asm release").unwrap();
        assert!(matches!(release(&dir), Err(BuiltError::Unmarked { .. })));
        mark(&dir, RELEASE).unwrap();
        release(&dir).unwrap();
        release(&dir).unwrap();

        fs::write(dir.join(WASM), b"\0asm dev's, over it").unwrap();
        let moved = release(&dir).unwrap_err();
        assert!(matches!(moved, BuiltError::Moved { .. }), "{moved}");
        assert!(moved.to_string().contains("cargo xtask build"), "{moved}");

        mark(&dir, "devstack").unwrap();
        assert_eq!(profile(&dir).unwrap(), "devstack");
        let dev = release(&dir).unwrap_err();
        assert!(matches!(&dev, BuiltError::NotRelease { profile, .. } if profile == "devstack"), "{dev}");
        assert!(dev.to_string().contains("not the release's"), "{dev}");

        clear(&dir).unwrap();
        clear(&dir).unwrap();
        assert!(matches!(release(&dir), Err(BuiltError::Unmarked { .. })));

        for bad in ["", "release", "release abc", &format!("release {} extra", "a".repeat(64)), &format!("release {}", "g".repeat(64))] {
            fs::write(dir.join(MARK), bad).unwrap();
            assert!(matches!(release(&dir), Err(BuiltError::Malformed { .. })), "{bad:?}");
        }
        fs::write(dir.join(MARK), "x".repeat(MARK_MAX_BYTES as usize + 1)).unwrap();
        assert!(matches!(release(&dir), Err(BuiltError::TooLarge { .. })));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Whether a hold on `dir`'s lock, taken now, would wait.
    fn would_wait(dir: &Path, hold: Hold) -> bool {
        let file = File::open(lock_path(dir)).unwrap();
        match if hold == Hold::Shared { file.try_lock_shared() } else { file.try_lock() } {
            Ok(()) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(e)) => panic!("{e}"),
        }
    }

    /// Goal: no build writes among another's, or under a reader that
    /// checked the files and uses them still (the e2e's copy, a deploy's
    /// every wrangler try): alexlwn123's review of #596. Valid: a release
    /// build's mark names its own bytes; readers then hold them, at once,
    /// as often as asked (a replay). Invalid: dev's rebuild started between
    /// the release's budget and its mark, or under a reader: it waits.
    /// Restart: a build dropped unfinished (it failed, or stopped) leaves
    /// no mark, and lets go.
    #[test]
    fn a_build_and_its_readers_wait_for_each_other() {
        let dir = scratch("lock");
        let release_build = building(&dir, RELEASE).unwrap();
        fs::write(dir.join(WASM), b"\0asm release, within its budget").unwrap();
        let (started, dev_started) = std::sync::mpsc::channel();
        let dev = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let dev = building(&dir, "devstack").unwrap();
                started.send(()).unwrap();
                fs::write(dir.join(WASM), b"\0asm dev's, over the release's budget").unwrap();
                dev.finish().unwrap();
            }
        });
        assert!(dev_started.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "dev's rebuild waits for the release's");
        assert!(would_wait(&dir, Hold::Shared), "as a reader does");
        assert!(matches!(release(&dir), Err(BuiltError::Unmarked { .. })));
        release_build.finish().unwrap();
        dev.join().unwrap();
        // dev's bytes under dev's mark, never the release's
        assert!(matches!(release(&dir), Err(BuiltError::NotRelease { .. })));

        building(&dir, RELEASE).unwrap().finish().unwrap();
        let deploy = reading(&dir).unwrap();
        release(&dir).unwrap();
        let staging = reading(&dir).unwrap();
        release(&dir).unwrap();
        assert!(would_wait(&dir, Hold::Alone), "a rebuild waits for every reader");
        drop(deploy);
        assert!(would_wait(&dir, Hold::Alone));
        drop(staging);

        let stopped = building(&dir, "devstack").unwrap();
        fs::write(dir.join(WASM), b"\0asm half of dev's").unwrap();
        drop(stopped);
        assert!(!would_wait(&dir, Hold::Alone));
        assert!(matches!(release(&dir), Err(BuiltError::Unmarked { .. })));
        fs::remove_file(lock_path(&dir)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
