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
