//! Where a pass writes in the folder: never through a link; and a pulled file's landing.

use super::at_once::Gate;
use super::journal::{atomic_write, FileState};
use super::SyncError;
use crate::codestorage::CodeStorage;
use fragment_core::blob::sha256_hex;
use fragment_core::codestorage::TreeEntry;
use fragment_proto::RepoPath;
use std::fs;
use std::path::{Path, PathBuf};

/// Whether nothing at all is at `at`, not even a link or a pipe: the one
/// test of absence, for a pull's write and a push's deletion. A folder on
/// the way that is a file means nothing is there either.
pub(super) fn vacant(at: &Path) -> Result<bool, SyncError> {
    match fs::symlink_metadata(at) {
        Ok(_) => Ok(false),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Ok(true),
        Err(e) => Err(SyncError::Io(format!("{}: {e}", at.display()))),
    }
}

/// Where a repo path is in the folder, for a write or a delete, or `None`
/// when a folder on its way is a symlink. `RepoPath::under` keeps the path
/// under `dir` as a string; a symlinked folder (`cfg` linked to
/// `~/.config`) would carry a write outside all the same. The scan never
/// follows one, so sync never writes or deletes through one either. `dir`
/// itself may be a link: it is the person's to choose.
pub(super) fn place(dir: &Path, path: &RepoPath) -> Result<Option<PathBuf>, SyncError> {
    let segments: Vec<&str> = path.as_str().split('/').collect();
    let mut at = dir.to_path_buf();
    // the folders on the way, every segment but the file's own: a repo
    // path's few (at most PATH_MAX_BYTES / 2)
    for seg in &segments[..segments.len() - 1] {
        at.push(seg);
        match fs::symlink_metadata(&at) {
            Ok(md) if md.file_type().is_symlink() => return Ok(None),
            Ok(_) => {}
            // the rest of the way is made fresh, as folders
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(SyncError::Io(format!("{}: {e}", at.display()))),
        }
    }
    let target = path.under(dir);
    assert!(target.starts_with(&at), "the way checked leads to the target");
    Ok(Some(target))
}

/// What sync says of a path it would write or delete through a symlink.
pub(super) fn through_link(path: &RepoPath) -> String {
    format!("{path}: a folder on its way in this folder is a symlink, and sync writes and deletes nothing through one")
}

/// Fetches one file at `rev` (the listing's commit) into the folder: its
/// journal row, or `None` when a folder on its way is a symlink. Files are
/// fetched READS_AT_ONCE at a time; a blob's bytes (up to `BLOB_MAX_BYTES`)
/// BLOB_TRANSFERS_AT_ONCE at a time, through `blob_gate`.
pub(super) fn pull_file(storage: &CodeStorage, blobs: &crate::blobs::Blobs<'_>, blob_gate: &Gate, dir: &Path, rev: &str, entry: &TreeEntry) -> Result<Option<FileState>, SyncError> {
    let (path, commit) = (&entry.path, entry.last_commit_sha.as_str());
    // the one join of a listing's path onto the folder, checked before
    // anything is fetched for it
    let Some(target) = place(dir, path)? else { return Ok(None) };
    let git = storage.read_file(path.as_str(), rev)?;
    let (sha, md) = land(
        blob_gate,
        git,
        |git| blobs.resolve(git).map_err(|e| SyncError::blob(path.as_str(), e)),
        |bytes| {
            atomic_write(&target, bytes).map_err(|e| SyncError::Io(e.to_string()))?;
            fs::metadata(&target).map_err(|e| SyncError::Io(e.to_string()))
        },
    )?;
    let mtime_ns = md.modified().map(|m| m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0)).unwrap_or(0);
    Ok(Some(FileState { sha256: sha, size: md.len(), mtime_ns, commit: commit.to_string() }))
}

/// A file's bytes from what git holds (`resolve`), their hash, and what
/// `write` made of them. A pointer's bytes (a blob, up to
/// `BLOB_MAX_BYTES`) are fetched and written with a place at `gate`, held
/// until they are written and dropped: however many files a pull fetches at
/// once, the gate's few blobs are in memory. A plain file takes no place.
fn land<T>(
    gate: &Gate,
    git: Vec<u8>,
    resolve: impl FnOnce(Vec<u8>) -> Result<Vec<u8>, SyncError>,
    write: impl FnOnce(&[u8]) -> Result<T, SyncError>,
) -> Result<(String, T), SyncError> {
    let _place = fragment_core::blob::parse(&git).is_some().then(|| gate.enter());
    let bytes = resolve(git)?;
    let sha = sha256_hex(&bytes);
    let wrote = write(&bytes)?;
    drop(bytes);
    Ok((sha, wrote))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::at_once::BLOB_TRANSFERS_AT_ONCE;
    use crate::sync::tests::{client_for, opts, tmpdir};
    use crate::sync::{sync_once, Mode};

    /// Goal: a repo file that does not sync is left alone both ways: never
    /// pulled, and never deleted for being absent from the folder (the
    /// scan skips it). Method: a repo with a dotfile and a node_modules
    /// file; a mirror pass, then another.
    #[test]
    fn repo_files_that_do_not_sync_are_left_alone() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), (".env.example", b"X=1"), ("node_modules/x.js", b"x")]);
        let c = client_for(&mock);
        let dir = tmpdir("left-alone");
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"]);
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.deleted_remote.is_empty() && second.pushed.is_empty(), "{second:?}");
        assert_eq!(mock.paths("t--k3x9", "main"), [".env.example", "a.md", "node_modules/x.js"]);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a listing's path that is not a repo path writes nothing,
    /// anywhere (the audit's M5: an absolute path joined onto the folder
    /// replaced it). Method: a repo seeded past the cell, as a storage
    /// token's commit is, with an absolute path to a file outside the
    /// folder, a `..` path, and a good file; a mirror pass, then another.
    /// A file in the folder whose name is no repo path is not pushed.
    #[test]
    fn a_listing_path_that_is_not_a_repo_path_writes_nothing() {
        let mock = crate::mockcs::start();
        let outside = tmpdir("escape-target");
        let planted = outside.join("planted.txt");
        let planted_path = planted.to_string_lossy().to_string();
        assert!(planted_path.starts_with('/'));
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), (planted_path.as_str(), b"owned"), ("../escaped.txt", b"owned"), ("sub/../../up.txt", b"owned")]);
        let c = client_for(&mock);
        let dir = tmpdir("escape");
        fs::write(dir.join("back\\slash.md"), b"local").unwrap();
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"]);
        assert!(first.pushed.is_empty(), "a name that is no repo path is not pushed: {first:?}");
        assert!(!planted.exists(), "nothing is written at an absolute path");
        assert!(!dir.join("../escaped.txt").exists() && !dir.join("../up.txt").exists(), "nothing is written above the folder");
        let mut refused = first.refused.clone();
        refused.sort();
        assert_eq!(refused.len(), 4, "{refused:?}");
        for (named, path) in refused.iter().zip(["\"../escaped.txt\"", &format!("{planted_path:?}"), "\"back\\\\slash.md\"", "\"sub/../../up.txt\""]) {
            assert!(named.starts_with(path), "{named} names {path}");
        }
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.pulled.is_empty() && second.pushed.is_empty() && second.deleted_remote.is_empty(), "{second:?}");
        assert_eq!(second.refused.len(), 4, "refused again, each pass");
        assert_eq!(mock.paths("t--k3x9", "main").len(), 4, "the repo is left as it was");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    /// Goal: a repo path is never written through a symlinked folder in
    /// the sync folder (a folder `cfg` linked to `~/.config`: the path is
    /// under the folder as a string, its bytes would not be). Method: a
    /// link to a folder outside, a repo file beneath it and one beside it;
    /// a mirror pass, then another.
    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_symlinked_folder() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), ("cfg/fish/config.fish", b"owned"), ("cfg/top.txt", b"owned")]);
        let c = client_for(&mock);
        let dir = tmpdir("through-link");
        let outside = tmpdir("through-link-target");
        std::os::unix::fs::symlink(&outside, dir.join("cfg")).unwrap();
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"], "{first:?}");
        assert!(!outside.join("fish/config.fish").exists() && !outside.join("top.txt").exists(), "nothing is written through the link");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "the folder outside is left as it was");
        assert_eq!(first.refused.len(), 2, "{:?}", first.refused);
        assert!(first.refused.iter().all(|r| r.contains("is a symlink")), "{:?}", first.refused);
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.pushed.is_empty() && second.deleted_remote.is_empty() && second.pulled.is_empty(), "the repo's files are left alone: {second:?}");
        assert_eq!(mock.paths("t--k3x9", "main").len(), 3);
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    /// Goal: a blob's bytes keep their place at the gate until they are
    /// written and dropped, however many files a pull fetches at once.
    /// Method: land a pointer's bytes and a plain file's, reading the
    /// gate from inside the write. (With the place let go once the bytes
    /// arrived, as a317d948 did, the write sees none: checked.)
    #[test]
    fn a_blobs_gate_is_held_until_its_bytes_are_written() {
        let gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let pointer = fragment_core::blob::pointer(&"a".repeat(64), 2 * 1024 * 1024).into_bytes();
        let (sha, (len, held)) = land(&gate, pointer, |_| Ok(vec![7u8; 16]), |bytes| Ok((bytes.len(), gate.held()))).unwrap();
        assert_eq!((len, held), (16, 1), "a place is held through a blob's write");
        assert_eq!(sha, sha256_hex(&[7u8; 16]), "the hash is the blob's bytes'");
        assert_eq!(gate.held(), 0, "and let go after");
        let (_, held) = land(&gate, b"plain".to_vec(), Ok, |_| Ok(gate.held())).unwrap();
        assert_eq!(held, 0, "a plain file takes no place");
    }
}
