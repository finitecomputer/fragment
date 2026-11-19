//! Both sides changed: the folder keeps its bytes, and the remote's are saved beside them.

use super::folder::{place, through_link};
use super::journal::{atomic_write, FileState, SyncState};
use super::listing::Listing;
use super::scan::LocalFile;
use super::{Report, SyncError};
use crate::codestorage::CodeStorage;
use fragment_proto::{NotRepoPath, RepoPath};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

/// Both sides changed: keep local, save the remote bytes beside it
/// (`save_copy`), then record the remote's bytes at its commit, never the
/// folder's: the folder's stay a change, so a writer's next pass sends them
/// (theirs survives in the copy and in git history), a pass that only reads
/// keeps them unsent, and neither ever fetches over them. The remote's next
/// change is a conflict again. Both sides changed to the SAME bytes is no
/// conflict (our own commit whose answer was lost, or two writers
/// agreeing): that adopts silently.
pub(super) fn record_conflicts(
    ctx: ConflictCtx<'_>,
    conflicts: &[RepoPath],
) -> Result<(), SyncError> {
    for path in conflicts {
        // by path, not by report line: `a.md`'s line starts with "a", so
        // matching the text skipped a real conflict on `a` whenever `a.md`
        // was recorded first
        if !ctx.recorded.insert(path.clone()) {
            continue; // already recorded this pass
        }
        let rf_commit = ctx.listing.files.get(path).map(|r| r.last_commit_sha.clone()).unwrap_or_default();
        let git_bytes = ctx.storage.read_file(path.as_str(), ctx.listing.rev());
        let remote_sha = git_bytes.as_ref().ok().map(|bytes| crate::blobs::content_sha(bytes));
        let same = match (&remote_sha, ctx.local.get(path)) {
            (Some(sha), Some(lf)) => *sha == lf.sha256,
            _ => false,
        };
        if !same {
            match git_bytes.map_err(anyhow::Error::from).and_then(|b| ctx.blobs.resolve(b)) {
                Ok(bytes) => match save_copy(ctx.dir, path, ctx.writer_id, epoch_s(), &bytes)? {
                    Ok(copy) => ctx.report.conflicts.push(format!("{path} (remote copy: {copy})")),
                    Err(why) => ctx.report.conflicts.push(format!("{path} (remote copy not saved, {why}; it is in git history)")),
                },
                Err(e) => ctx.report.conflicts.push(format!("{path} (remote copy unavailable: {e})")),
            }
        }
        let entry = match ctx.local.get(path) {
            // the remote commit's bytes, not the folder's: an mtime no file
            // has keeps the scan hashing the folder's
            Some(_) if !same => {
                let size = ctx.listing.files.get(path).map_or(0, |r| r.size);
                FileState { sha256: remote_sha.clone().unwrap_or_default(), size, mtime_ns: -1, commit: rf_commit }
            }
            Some(lf) => FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: rf_commit },
            None => match ctx.state.files.get(path) {
                Some(st) => FileState { commit: rf_commit, ..st.clone() },
                None => continue,
            },
        };
        ctx.state.files.insert(path.clone(), entry);
    }
    Ok(())
}

/// Conflict copies of one path `save_copy` names within one second: past
/// it, the copy is not saved (its bytes are in git history), and the
/// report says so.
const COPIES_A_SECOND: usize = 100;

/// The `n`th name (from 1) for the remote's bytes of `path` beside the
/// folder's own in second `ts`: `<stem>.conflict-<ts>-<writer><ext>`, then
/// `<ts>-2`, `<ts>-3`…; or why there is no room.
fn conflict_copy(path: &RepoPath, writer_id: &str, ts: u64, n: usize) -> Result<RepoPath, NotRepoPath> {
    let text = path.as_str();
    let (stem, ext) = match text.rfind('.') {
        Some(i) if !text[i..].contains('/') => (&text[..i], &text[i..]),
        _ => (text, ""),
    };
    let time = if n == 1 { ts.to_string() } else { format!("{ts}-{n}") };
    RepoPath::parse(&format!("{stem}.conflict-{time}-{writer_id}{ext}"))
}

/// Saves the remote's `bytes` of `path` beside the folder's own as a new
/// file (`conflict_copy`), never over anything, an earlier copy included:
/// the name is taken before the bytes land, and a taken one moves on to the
/// next. Its name, or why it was not saved (a path near the limit has no
/// room for the copy's mark; a folder on its way is a symlink). `ts`: the
/// second it is named for (`epoch_s`).
pub(super) fn save_copy(dir: &Path, path: &RepoPath, writer_id: &str, ts: u64, bytes: &[u8]) -> Result<Result<RepoPath, String>, SyncError> {
    for n in 1..=COPIES_A_SECOND {
        let copy = match conflict_copy(path, writer_id, ts, n) {
            Ok(copy) => copy,
            Err(not) => return Ok(Err(format!("its name {not}"))),
        };
        let Some(target) = place(dir, &copy)? else { return Ok(Err(through_link(&copy))) };
        let io = |e: std::io::Error| SyncError::Io(format!("{}: {e}", target.display()));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(_) => {
                atomic_write(&target, bytes).map_err(|e| SyncError::Io(e.to_string()))?;
                return Ok(Ok(copy));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io(e)),
        }
    }
    Ok(Err(format!("{COPIES_A_SECOND} copies of it were saved this second")))
}

/// Seconds since the Unix epoch: the time in a conflict copy's name.
pub(super) fn epoch_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Everything record_conflicts needs; keeps it at one argument.
pub(super) struct ConflictCtx<'a> {
    pub(super) storage: &'a CodeStorage,
    pub(super) blobs: &'a crate::blobs::Blobs<'a>,
    pub(super) dir: &'a Path,
    pub(super) local: &'a BTreeMap<RepoPath, LocalFile>,
    pub(super) listing: &'a Listing,
    pub(super) state: &'a mut SyncState,
    pub(super) report: &'a mut Report,
    /// the paths recorded as conflicts this pass (the push plan and the
    /// pull can both name one)
    pub(super) recorded: &'a mut HashSet<RepoPath>,
    pub(super) writer_id: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestorage::Wants;
    use crate::sync::listing::list_main;
    use crate::sync::scan::scan_local;
    use crate::sync::tests::{client_for, opts, rp, tmpdir};
    use crate::sync::{sync_once, Mode};

    #[test]
    fn conflict_both_changed_saves_remote_copy() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("shared.txt", b"base")]);
        let c = client_for(&mock);
        let dir = tmpdir("conflict");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        // remote side changes it
        let other = tmpdir("conflict-other");
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
        fs::write(other.join("shared.txt"), b"theirs").unwrap();
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Push)).unwrap();
        // local side changes it too
        fs::write(dir.join("shared.txt"), b"ours").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.exit_code(), 3);
        assert_eq!(fs::read(dir.join("shared.txt")).unwrap(), b"ours", "local keeps ours");
        // the copy name splits stem/ext like the old engine: shared.conflict-<ts>-<w>.txt
        let copy = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).find(|n| n.starts_with("shared.conflict-"));
        assert!(copy.is_some(), "remote copy saved beside it");
        let copy_bytes = fs::read(dir.join(copy.unwrap())).unwrap();
        assert_eq!(copy_bytes, b"theirs");
        assert_eq!(mock.file_at("t--k3x9", "main", "shared.txt").unwrap(), b"theirs", "remote untouched by our conflict");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&other).ok();
    }

    /// Goal: a conflict on `a` is recorded even when one on `a.md` was
    /// recorded first. Method: record them in that order within one pass
    /// (the pull walks a HashMap, so in a real pass the order is chance);
    /// matching report text found "a.md (remote copy: …)" starting with "a"
    /// and skipped `a` silently, on every pass.
    #[test]
    fn conflicts_are_tracked_by_path_not_report_text() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"theirs a"), ("a.md", b"theirs a.md")]);
        let c = client_for(&mock);
        let dir = tmpdir("conflict-paths");
        fs::write(dir.join("a"), b"ours a").unwrap();
        fs::write(dir.join("a.md"), b"ours a.md").unwrap();
        let storage = CodeStorage::connect(&c, "t--k3x9", Wants::Write).unwrap();
        let blobs = crate::blobs::Blobs::new(&c, "t--k3x9");
        let listing = list_main(&storage).unwrap();
        let local = scan_local(&dir, None, true).unwrap().files;
        let mut state = SyncState { schema_version: 3, name: "t--k3x9".into(), host: None, repo: None, files: BTreeMap::new() };
        let mut report = Report::default();
        let mut recorded = HashSet::new();
        for path in ["a.md", "a", "a.md", "a"] {
            let ctx = ConflictCtx { storage: &storage, blobs: &blobs, dir: &dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: "deadbeef" };
            record_conflicts(ctx, &[rp(path)]).unwrap();
        }
        assert_eq!(report.conflicts.len(), 2, "each path once: {:?}", report.conflicts);
        assert!(report.conflicts[0].starts_with("a.md (remote copy: a.conflict-"), "{:?}", report.conflicts);
        assert!(report.conflicts[1].starts_with("a (remote copy: a.conflict-"), "{:?}", report.conflicts);
        let copies: Vec<Vec<u8>> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".conflict-")).map(|e| fs::read(e.path()).unwrap()).collect();
        assert!(copies.contains(&b"theirs a".to_vec()) && copies.contains(&b"theirs a.md".to_vec()), "both remote copies saved");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_records_both_a_and_a_md_conflicts() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"base a"), ("a.md", b"base a.md")]);
        let c = client_for(&mock);
        let dir = tmpdir("pull-conflict-paths");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        mock.external_commit("t--k3x9", "main", &[("a", Some(b"theirs a")), ("a.md", Some(b"theirs a.md"))], "theirs");
        // a different size, so the scan re-hashes whatever the mtime says
        fs::write(dir.join("a"), b"ours, a").unwrap();
        fs::write(dir.join("a.md"), b"ours, a.md").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let mut paths: Vec<&str> = report.conflicts.iter().filter_map(|c| c.split(' ').next()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a", "a.md"], "{:?}", report.conflicts);
        assert_eq!(fs::read(dir.join("a")).unwrap(), b"ours, a");
        assert_eq!(fs::read(dir.join("a.md")).unwrap(), b"ours, a.md");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a conflict copy never replaces a file, an earlier copy of the
    /// same second included: each takes the next free name. Invalid: past
    /// `COPIES_A_SECOND` of one second, or through a symlinked folder, none
    /// is saved, and nothing is written. Method: `save_copy` at one fixed
    /// second, over a copy someone is editing.
    #[test]
    fn a_conflict_copy_never_replaces_a_file() {
        let dir = tmpdir("copy-names");
        let at = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        fs::write(dir.join("a.conflict-7-deadbeef.md"), "my merge, half done").unwrap();
        assert_eq!(save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v2").unwrap().unwrap(), rp("a.conflict-7-2-deadbeef.md"));
        assert_eq!(save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v3").unwrap().unwrap(), rp("a.conflict-7-3-deadbeef.md"));
        assert_eq!((at("a.conflict-7-deadbeef.md"), at("a.conflict-7-2-deadbeef.md"), at("a.conflict-7-3-deadbeef.md")), ("my merge, half done".into(), "v2".into(), "v3".into()));
        assert_eq!(save_copy(&dir, &rp("n/b"), "deadbeef", 7, b"x").unwrap().unwrap(), rp("n/b.conflict-7-deadbeef"), "its folder is made");

        for n in 4..=COPIES_A_SECOND {
            fs::write(dir.join(format!("a.conflict-7-{n}-deadbeef.md")), "taken").unwrap();
        }
        let files = fs::read_dir(&dir).unwrap().count();
        let full = save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v4").unwrap().unwrap_err();
        assert!(full.contains("this second") && fs::read_dir(&dir).unwrap().count() == files, "{full}");
        #[cfg(unix)]
        {
            let outside = tmpdir("copy-names-outside");
            std::os::unix::fs::symlink(&outside, dir.join("cfg")).unwrap();
            let linked = save_copy(&dir, &rp("cfg/c.md"), "deadbeef", 7, b"x").unwrap().unwrap_err();
            assert!(linked.contains("is a symlink") && fs::read_dir(&outside).unwrap().count() == 0, "{linked}");
            fs::remove_dir_all(&outside).ok();
        }
        fs::remove_dir_all(&dir).ok();
    }
}
