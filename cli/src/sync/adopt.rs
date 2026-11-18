//! The bootstrap: the journal adopts what both sides already agree on.

use super::at_once::{at_once, READS_AT_ONCE};
use super::journal::{FileState, SyncState};
use super::listing::Listing;
use super::scan::LocalFile;
use super::SyncError;
use crate::codestorage::CodeStorage;
use fragment_core::codestorage::TreeEntry;
use fragment_proto::RepoPath;
use std::collections::BTreeMap;

/// Brings the journal up to what both sides already agree on, before any
/// plan is made from it:
/// - a journal-absent file present on both sides with the same content is
///   adopted (the stateless bootstrap, and a new file whose commit landed
///   though its answer was lost). Equal size used to be "provisionally
///   same" — but equal size is not identity, and a same-size
///   different-content file silently never pushed (found live). So each
///   equal-size unknown is fetched and hashed; mismatches stay
///   journal-absent and the push plan treats them as local-wins.
/// - a row for a path gone from both sides is dropped (a deletion that
///   landed, whoever made it); no plan reads it, and it would count toward
///   the mass-deletion guard's known files forever.
pub(super) fn adopt_identical(
    storage: &CodeStorage,
    local: &BTreeMap<RepoPath, LocalFile>,
    listing: &Listing,
    state: &mut SyncState,
) -> Result<(), SyncError> {
    let unknown: Vec<(&RepoPath, &TreeEntry, &LocalFile)> = listing
        .files
        .iter()
        .filter(|(p, _)| !state.files.contains_key(*p))
        .filter_map(|(p, rf)| local.get(p).map(|lf| (p, rf, lf)))
        .filter(|(_, rf, lf)| lf.size == rf.size || crate::blobs::could_point(rf.size, lf.size))
        .collect();
    // read as the pull reads, READS_AT_ONCE at a time: a folder a pull
    // left part-way is all of these on its next pass. Each is compared
    // where it is read, so a body is let go before the next read: what
    // these hold at once is READS_AT_ONCE files, not the folder's
    let same = at_once(&unknown, READS_AT_ONCE, |(p, _, lf)| storage.read_file(p.as_str(), listing.rev()).map(|bytes| crate::blobs::content_sha(&bytes) == lf.sha256));
    for ((p, rf, lf), got) in unknown.iter().zip(same) {
        match got {
            Some(Ok(true)) => {
                state.files.insert(
                    (*p).clone(),
                    FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: rf.last_commit_sha.clone() },
                );
            }
            Some(Ok(false)) | None => {}
            Some(Err(e)) => return Err(e.into()),
        }
    }
    state.files.retain(|p, _| local.contains_key(p) || listing.files.contains_key(p));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::sync::journal::load_state;
    use crate::sync::tests::{client_for, opts, tmpdir};
    use crate::sync::{sync_once, Mode};
    use std::fs;

    #[test]
    fn bootstrap_same_size_different_content_pushes() {
        // stateless bootstrap used to treat equal SIZE as same FILE — a
        // same-size different-content file silently never pushed (found
        // live). Content must decide.
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"aaaa")]);
        let c = client_for(&mock);
        let dir = tmpdir("bootstrap-diff");
        fs::write(dir.join("a.txt"), b"bbbb").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert_eq!(report.pushed, vec!["a.txt".to_string()], "equal size, different content must push");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.txt").unwrap(), b"bbbb");
        assert!(load_state(&dir, "t--k3x9").unwrap().files.contains_key("a.txt"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bootstrap_identical_content_adopts_without_commit() {
        // the flip side: byte-identical files adopt into the journal with
        // no commit and no re-fetch on the next pass
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"same")]);
        let c = client_for(&mock);
        let dir = tmpdir("bootstrap-same");
        fs::write(dir.join("a.txt"), b"same").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert!(report.pushed.is_empty(), "identical content is not a push");
        assert_eq!(mock.commit_pack_count(), 0, "no commit-packs sent — the seed commit is server-side");
        let r2 = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(r2.pushed.is_empty() && r2.pulled.is_empty(), "adopted file stays quiet");
        let st = load_state(&dir, "t--k3x9").unwrap();
        assert_eq!(st.repo.as_deref(), Some("t--k3x9"), "journal binds to the repo identity");
        assert_eq!(st.host.as_deref(), Some(mock.url.as_str()), "journal binds to the host identity");
        fs::remove_dir_all(&dir).ok();
    }
}
