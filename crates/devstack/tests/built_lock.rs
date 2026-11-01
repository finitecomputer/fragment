//! The cell's build directory's lock (`fragment_devstack::built`), in a test
//! binary of its own. A flock lives on an open file description, and a child
//! another test of the same process spawns holds a copy of the process's
//! fds from its fork to its exec, this lock's among them (std opens them
//! close-on-exec, which closes them only at the exec): a lock dropped
//! meanwhile is still held. The lib's tests spawn children (signals.rs, a
//! scripted node); no test here does, so a lock dropped is let go at once.

use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};

use fragment_devstack::built::{building, reading, release, BuiltError, RELEASE, WASM};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    Alone,
    Shared,
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fragment-built-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Whether a hold on `dir`'s lock (`<dir>.lock`, as `cell/build.lock` is
/// `cell/build`'s), taken now, would wait: a reader's (`Shared`), or a
/// build's (`Alone`).
fn would_wait(dir: &Path, hold: Hold) -> bool {
    let file = File::open(dir.with_extension("lock")).unwrap();
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
    fs::remove_file(dir.with_extension("lock")).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
