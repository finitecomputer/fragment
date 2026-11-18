//! Paths outside sync: kept as they are, never sent, and never pulled over.

use super::conflicts::{epoch_s, save_copy, ConflictCtx};
use super::folder::{place, vacant};
use super::journal::SyncState;
use super::scan::LocalFile;
use super::SyncError;
use fragment_proto::RepoPath;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Something the scan did not take sits where the fragment has a file: a
/// file outside sync (its .gitignore's), or what is not a regular file.
/// Pulled over, it was lost (nothing of it is in git), so it stays as it
/// is and outside sync: no journal row, so it is never sent nor its
/// absence a deletion. The remote's bytes land beside it once per version
/// (a copy of the same bytes there already is not written again): a
/// conflict, reported each pass while both are there. The same bytes on
/// both sides are nothing to report. Only a regular file is ever read,
/// never through a link: a pipe waits forever for a writer, a device
/// never ends.
pub(super) fn keep_unscanned(ctx: ConflictCtx<'_>, path: &RepoPath, target: &Path) -> Result<(), SyncError> {
    let regular = |at: &Path| fs::symlink_metadata(at).is_ok_and(|md| md.file_type().is_file());
    if !ctx.recorded.insert(path.clone()) {
        return Ok(());
    }
    ctx.state.files.remove(path);
    let git = ctx.storage.read_file(path.as_str(), ctx.listing.rev())?;
    let bytes = ctx.blobs.resolve(git).map_err(|e| SyncError::blob(path.as_str(), e))?;
    if regular(target) && fs::read(target).is_ok_and(|here| here == bytes) {
        return Ok(());
    }
    // a copy of these bytes beside it already: this version's was saved
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_string).unwrap_or_default();
    let theirs = name(target);
    let (stem, ext) = match theirs.rfind('.') {
        Some(i) if i > 0 => (&theirs[..i], &theirs[i..]),
        _ => (theirs.as_str(), ""),
    };
    let (prefix, suffix) = (format!("{stem}.conflict-"), format!("-{}{ext}", ctx.writer_id));
    let saved = target.parent().and_then(|at| fs::read_dir(at).ok()).is_some_and(|entries| {
        entries.flatten().any(|e| {
            let n = name(&e.path());
            n.starts_with(&prefix) && n.ends_with(&suffix) && e.file_type().is_ok_and(|t| t.is_file()) && fs::read(e.path()).is_ok_and(|b| b == bytes)
        })
    });
    let kept = "a file outside sync is here, kept";
    if saved {
        ctx.report.conflicts.push(format!("{path} ({kept}; remote copy beside it: {stem}.conflict-…{ext})"));
        return Ok(());
    }
    match save_copy(ctx.dir, path, ctx.writer_id, epoch_s(), &bytes)? {
        Ok(copy) => ctx.report.conflicts.push(format!("{path} ({kept}; remote copy: {copy})")),
        Err(why) => ctx.report.conflicts.push(format!("{path} ({kept}; remote copy not saved, {why}; it is in git history)")),
    }
    Ok(())
}

/// A synced path the scan no longer takes stands for a deletion only when
/// nothing at all is there. Anything else (a `.gitignore`d file, what is
/// not a regular file, a path through a symlinked folder, which the scan
/// never follows) is outside sync: its journal row is dropped here, before
/// any plan reads it, so it is never sent, its absence never deletes the
/// fragment's copy, and a pull keeps it (`keep_unscanned`). The paths, for
/// the report.
pub(super) fn drop_outside_sync(dir: &Path, local: &BTreeMap<RepoPath, LocalFile>, state: &mut SyncState) -> Result<Vec<RepoPath>, SyncError> {
    let mut outside = Vec::new();
    for path in state.files.keys().filter(|p| !local.contains_key(*p)) {
        let gone = match place(dir, path)? {
            Some(at) => vacant(&at)?,
            None => false,
        };
        if !gone {
            outside.push(path.clone());
        }
    }
    for path in &outside {
        state.files.remove(path);
    }
    Ok(outside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client;
    use crate::auth;
    use crate::sync::tests::{client_for, conflict_copies, git_folder, opts, tmpdir};
    use crate::sync::{sync_once, Mode};

    /// Goal: a file outside sync (.gitignore'd) where the fragment has one
    /// is never pulled over, for a viewer or an editor, in a folder's first
    /// pass or a later one: it stays as it is, theirs lands beside it once
    /// per version, the pass reports a conflict, and the ignored file is
    /// never sent. Method: a git folder ignoring a.md holds a draft of it;
    /// the fragment has another a.md; a viewer's mirror, then an editor's
    /// pull and mirror, then a new remote version.
    #[test]
    fn a_file_outside_sync_is_never_pulled_over() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"remote"), ("b.md", b"b")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = git_folder("outside-sync", "a.md");
        fs::write(dir.join("a.md"), "my private draft").unwrap();
        let copies = |dir: &Path| conflict_copies(dir);

        let viewer = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft", "{viewer:?}");
        assert!(viewer.pulled == ["b.md"] && viewer.conflicts.len() == 1 && viewer.unsent.is_empty() && viewer.exit_code() == 3, "{viewer:?}");
        assert_eq!(copies(&dir), ["remote"]);
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.conflicts.len() == 1 && again.pulled.is_empty(), "reported while both are there: {again:?}");
        assert_eq!(copies(&dir), ["remote"], "one copy a version");

        // its editor's pull and mirror keep it too, and never send it
        mock.view_only("t--k3x9", false);
        let pulled = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let mirrored = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft");
        assert!(pulled.conflicts.len() == 1 && mirrored.conflicts.len() == 1 && mirrored.pushed.is_empty() && mirrored.deleted_remote.is_empty(), "{pulled:?} {mirrored:?}");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.md").as_deref(), Some(&b"remote"[..]), "the ignored file is never sent, nor theirs deleted");
        assert_eq!(mock.commit_pack_count(), 0);

        // a new remote version lands beside it too
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"remote 2"))], "theirs");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft");
        assert!(copies(&dir).contains(&"remote 2".to_string()), "{:?}", copies(&dir));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: an editor's file, synced and then put outside sync (added to
    /// .gitignore), is kept when the fragment's copy moves on, and the
    /// fragment's is never deleted for it. Method: an editor syncs a.md,
    /// ignores it and edits it; another writer changes it; mirror twice.
    #[test]
    fn an_editors_file_put_outside_sync_is_kept() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"v1")]);
        let c = client_for(&mock);
        let dir = git_folder("editor-outside", "nothing.md");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join(".gitignore"), "a.md\n").unwrap();
        fs::write(dir.join("a.md"), "mine, now private").unwrap();
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v2"))], "theirs");
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine, now private", "{first:?}");
        assert!(first.conflicts.len() == 1 && second.deleted_remote.is_empty() && second.pushed.is_empty(), "{first:?} {second:?}");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.md").as_deref(), Some(&b"v2"[..]));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: what is not a regular file where the fragment has one (a pipe,
    /// a link to one) is kept and reported, never read: a pass ends. Method:
    /// a FIFO with no writer at a.md and a link to it at b.md; the pass runs
    /// on a thread, and if it has not ended in 20 s the test opens the
    /// pipe's write end (so the read ends) and fails.
    #[cfg(unix)]
    #[test]
    fn a_pipe_where_the_fragment_has_a_file_is_kept_unread() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"remote a"), ("b.md", b"remote b")]);
        mock.view_only("t--k3x9", true);
        let dir = tmpdir("pipe-kept");
        let fifo = dir.join("a.md");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success(), "mkfifo");
        std::os::unix::fs::symlink(&fifo, dir.join("b.md")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (url, at) = (mock.url.clone(), dir.clone());
        let pass = std::thread::spawn(move || {
            let c = Client::new(&url, auth::fixed(7));
            tx.send(sync_once(&c, "t--k3x9", &at, &opts(Mode::Mirror)).map(|r| r.conflicts.len())).ok();
        });
        let mut ended = rx.recv_timeout(std::time::Duration::from_secs(20));
        let stuck = ended.is_err();
        // stuck reading the pipe (through either path): its write end,
        // opened without waiting and closed, ends each read, at most 50
        // times, so the test fails and never hangs
        for _ in 0..50 {
            if ended.is_ok() {
                break;
            }
            use std::os::unix::fs::OpenOptionsExt;
            drop(fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo));
            ended = rx.recv_timeout(std::time::Duration::from_millis(200));
        }
        pass.join().unwrap();
        assert!(!stuck, "a pass read the pipe: {ended:?}");
        assert!(matches!(ended, Ok(Ok(2))), "the pass ended, both kept and reported: {ended:?}");
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(&fs::symlink_metadata(&fifo).unwrap().file_type()) && fs::symlink_metadata(dir.join("b.md")).unwrap().file_type().is_symlink());
        assert_eq!(conflict_copies(&dir).len(), 2, "theirs beside each");
        fs::remove_dir_all(&dir).ok();
    }
}
