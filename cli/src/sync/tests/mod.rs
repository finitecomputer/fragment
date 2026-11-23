//! Sync's shared test harness, and whole passes against the mock code.storage.

mod binding;
mod matrix;
mod modes;
mod viewers;

use crate::api::Client;
use crate::auth;
use crate::mockcs::MockServer;
use crate::sync::{Mode, SyncOptions};
use fragment_proto::RepoPath;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn client_for(mock: &MockServer) -> Client {
    Client::new(&mock.url, auth::fixed(7))
}

pub(super) fn opts(mode: Mode) -> SyncOptions {
    SyncOptions { mode, writer_id: "deadbeef".into(), ..Default::default() }
}

pub(super) fn rp(path: &str) -> RepoPath {
    RepoPath::parse(path).unwrap()
}

pub(super) fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fragment-sync3-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// The `.conflict-` copies in a folder, their text by name.
pub(super) fn conflict_copies(dir: &Path) -> Vec<String> {
    let mut copies: Vec<String> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.contains(".conflict-")).collect();
    copies.sort();
    copies.iter().map(|n| fs::read_to_string(dir.join(n)).unwrap()).collect()
}

/// A folder git keeps, ignoring `ignored` (the scan honors .gitignore
/// inside a git working tree).
pub(super) fn git_folder(tag: &str, ignored: &str) -> PathBuf {
    let dir = tmpdir(tag);
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".gitignore"), format!("{ignored}\n")).unwrap();
    dir
}

/// A change made to a folder, by a test.
pub(crate) type FolderChange = fn(&Path);

/// The ways a browser profile comes into a synced folder after a pass
/// began: one is made, or `.gitignore` stops keeping one out (`browser/`).
pub(crate) const PROFILE_CHANGES: [(&str, FolderChange); 2] = [
    ("made", |dir| {
        fs::create_dir_all(dir.join("chrome/Default")).unwrap();
        fs::write(dir.join("chrome/Default/Cookies"), b"x").unwrap();
    }),
    ("unignored", |dir| fs::write(dir.join(".gitignore"), "").unwrap()),
];

/// Runs `read`, a pass over `dir`, with `change` made to the folder while
/// the pass is held at its journal: past anything it checks first, before
/// its scan. The journal becomes a pipe, so the pass's read of it waits;
/// a thread opens the pipe's other end (which waits for the pass to open
/// it), makes `change`, writes the journal's bytes, and closes it, which
/// ends the read. A browser made its profile in that window, by chance,
/// and a pass that checked the folder before its scan sent the profile.
#[cfg(unix)]
pub(crate) fn while_a_pass_reads<T>(dir: &Path, change: impl FnOnce(&Path) + Send + 'static, read: impl FnOnce() -> T) -> T {
    use std::io::Write;
    let journal = crate::sync::journal::state_path(dir);
    let bytes = fs::read(&journal).expect("a journal to hold the pass at");
    fs::remove_file(&journal).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&journal).status().unwrap().success(), "mkfifo");
    let (changed, made) = std::sync::mpsc::channel();
    let at = dir.to_path_buf();
    let writer = std::thread::spawn(move || {
        let mut pipe = fs::OpenOptions::new().write(true).open(&journal).unwrap();
        change(&at);
        pipe.write_all(&bytes).unwrap();
        // said before the pipe closes, so before the pass's read ends
        changed.send(()).unwrap();
    });
    let out = read();
    // a pass that never read its journal leaves the thread waiting: fail, never hang
    assert!(made.try_recv().is_ok(), "the pass read its journal, and the change was made while it did");
    writer.join().unwrap();
    out
}
