//! The folder's journal (`.fragment/state.json`), its lock, and atomic writes.

use anyhow::{anyhow, Result};
use fragment_proto::RepoPath;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// stat cache v3: content sha + last-seen remote commit per path.
/// Anything else (or a v2 file from the old engine) reads as fresh —
/// corrupt or old states never wedge the folder.
#[derive(Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32, // 3
    pub name: String,
    /// url-form repo identity this folder last synced against, plus the
    /// fragment host it synced through. The mirror trusts the journal to
    /// mean "same world": a journal from another (a recreated fragment, or
    /// the same name on another host) makes every file look remotely
    /// changed, and the pull mirrors the wrong world over the folder.
    /// None on old journals; bound on the first sync.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// by path, in order: the journal serializes the same for the same
    /// state, so an unchanged one is recognized and not written again. A
    /// key that is not a repo path makes the journal unreadable, so it
    /// starts fresh.
    pub files: BTreeMap<RepoPath, FileState>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileState {
    pub sha256: String,
    pub size: u64,
    pub mtime_ns: i128,
    /// last commit SHA we saw touch this path (the listing's
    /// last_commit_sha) — our per-file remote version marker
    pub commit: String,
}

pub(super) fn state_path(dir: &Path) -> PathBuf {
    dir.join(".fragment").join("state.json")
}

/// The single-watcher lock: an advisory flock on .fragment/sync.lock, held
/// for the process lifetime. Stale by construction (the OS drops it when
/// the holder dies).
pub struct SyncLock(#[allow(dead_code)] File); // the field IS the lock (RAII)
impl SyncLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir.join(".fragment"))?;
        let p = dir.join(".fragment").join("sync.lock");
        let f = File::create(&p)?;
        f.try_lock_exclusive()
            .map_err(|_| anyhow!("another fragment sync holds this folder ({}). If that's wrong, no process should own it; otherwise stop it or use a different folder.", p.display()))?;
        Ok(SyncLock(f))
    }
}

pub fn load_state(dir: &Path, name: &str) -> Result<SyncState> {
    Ok(read_state(dir, name)?.0)
}

/// The journal, and the bytes it was read from (`None` when there was
/// none, or it was unreadable and starts fresh).
pub(super) fn read_state(dir: &Path, name: &str) -> Result<(SyncState, Option<Vec<u8>>)> {
    let fresh = || SyncState { schema_version: 3, name: name.to_string(), host: None, repo: None, files: BTreeMap::new() };
    let p = state_path(dir);
    if !p.exists() {
        return Ok((fresh(), None));
    }
    let bytes = fs::read(&p)?;
    match serde_json::from_slice::<SyncState>(&bytes) {
        Ok(mut s) if s.schema_version == 3 => {
            // a journal a sync named by its bare label wrote (`todo`, before
            // a sync resolved it) is the same fragment's, named in full now
            let bare = fragment_proto::split_fragment_name(name).is_some_and(|(label, _)| label == s.name);
            if s.name != name && !bare {
                anyhow::bail!("directory is synced to fragment '{}', not '{}'", s.name, name);
            }
            s.name = name.to_string();
            Ok((s, Some(bytes)))
        }
        _ => {
            eprintln!("warning: {} unreadable or old format — rebuilding state", p.display());
            Ok((fresh(), None))
        }
    }
}

/// atomic write: tmp in the same directory (unique name), fsync, rename
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = partial_path(path);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Where `atomic_write` puts `path`'s bytes before the rename: beside it,
/// named after its whole file name and a count of this process's writes,
/// so files written at once never share one (`a.md` and `a.txt` did, by
/// extension, when a pull wrote one file at a time). `syncable` leaves
/// it out by its `.fragment-partial` mark.
fn partial_path(path: &Path) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!("{name}.fragment-partial-{}-{n}", std::process::id()))
}

/// Writes the journal, unless it is byte for byte what was read (`before`):
/// a pass that changed nothing leaves the file alone, with no fsync'd
/// rewrite.
pub(super) fn save_state(dir: &Path, state: &SyncState, before: Option<&[u8]>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if before == Some(bytes.as_slice()) {
        return Ok(());
    }
    atomic_write(&state_path(dir), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::scan::syncable;
    use crate::sync::tests::tmpdir;

    #[test]
    fn atomic_write_leaves_no_partial() {
        let dir = tmpdir("atomic");
        atomic_write(&dir.join("a.txt"), b"content").unwrap();
        assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"content");
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains("partial")).collect();
        assert!(leftovers.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_state_reads_fresh() {
        let dir = tmpdir("oldstate");
        fs::create_dir_all(dir.join(".fragment")).unwrap();
        fs::write(dir.join(".fragment/state.json"), b"{\"name\":\"x\",\"files\":{\"a\":{\"rev\":1}}}").unwrap();
        let st = load_state(&dir, "x").unwrap();
        assert!(st.files.is_empty()); // v2 → fresh
        assert_eq!(st.schema_version, 3);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: files written at once never share a partial file. Before,
    /// `a.md` and `a.txt` both wrote `a.fragment-partial-<pid>`.
    #[test]
    fn partial_files_are_named_for_their_whole_file() {
        let dir = Path::new("/folder/notes");
        let (md, txt) = (partial_path(&dir.join("a.md")), partial_path(&dir.join("a.txt")));
        assert_ne!(md, txt);
        assert_ne!(partial_path(&dir.join("a.md")), md, "and each write its own");
        assert_eq!(md.parent(), Some(dir), "beside its file, for the rename");
        let name = md.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("a.md.") && !syncable(&format!("notes/{name}")), "{name}");
    }
}
