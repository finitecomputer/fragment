//! One pass: the world binding, a push with expected-parent CAS, the pull, the journal saved.

use super::adopt::adopt_identical;
use super::at_once::{at_once, Gate, BLOB_TRANSFERS_AT_ONCE, READS_AT_ONCE};
use super::conflicts::{record_conflicts, ConflictCtx};
use super::folder::{place, pull_file, through_link, vacant};
use super::journal::{save_state, FileState};
use super::listing::{list_branch, list_main};
use super::outside::{drop_outside_sync, keep_unscanned};
use super::plan::{mass_delete_trips, push_plan};
use super::scan::{read_local, Local};
use super::{Mode, Report, SyncError, SyncOptions};
use crate::api::{Client, Code, CodedError};
use crate::codestorage::{Author, Change, CodeStorage, CsError, LIVE, MAIN, MAX_CAS_ATTEMPTS};
use fragment_core::codestorage::TreeEntry;
use fragment_proto::RepoPath;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// The fragment `name` names, in full (`Client::fragment`): a folder's
/// journal names it so, whether a command was given its label (`todo`)
/// or its full name (`todo--k3x9`).
pub fn full_name(client: &Client, name: &str) -> Result<String, SyncError> {
    client.fragment(name).map_err(|e| match e.downcast::<CodedError>() {
        Ok(refused) => SyncError::Host(refused),
        Err(e) => SyncError::Io(format!("naming {name}: {e:#}")),
    })
}

/// `fragment sync` (and a deploy's): connects, runs one pass, and nudges
/// the cell's pins when a commit landed.
pub fn sync_once(client: &Client, name: &str, dir: &Path, opts: &SyncOptions) -> Result<Report, SyncError> {
    let name = &full_name(client, name)?;
    let storage = CodeStorage::connect(client, name, opts.mode.wants())?;
    let report = pass_over(client, &storage, name, dir, opts, read_local(dir, name)?)?;
    if report.landed {
        refresh_pins(client, name, MAIN);
    }
    Ok(report)
}

/// One pass over a folder already read (`read_local`: a watcher reads it
/// first to learn whether it needs a pass at all), with a connected
/// `storage`: a command's own, or a watcher's held one. It does not nudge
/// the cell's pins: a caller whose pass `landed` calls `refresh_pins`.
///
/// The invariant every phase keeps (pull, push, mirror, a folder with no
/// journal, a viewer made an editor, a pass again): **a pass changes only
/// what its scan accounted for.**
/// - In the folder, a write or a deletion lands only where the scan saw
///   the bytes the journal says git holds, or where nothing at all is
///   (`vacant`). Anything else there (an edit, a file never synced, one
///   outside sync, a pipe) stays, and the fragment's bytes land beside it
///   under a name nothing holds (`save_copy`).
/// - On main, a push sends only bytes the scan read, and deletes a path
///   only where the folder has nothing at all. A synced path the scan no
///   longer takes, with something still there (a `.gitignore`d file, what
///   is not a regular file, a path through a symlinked folder), is outside
///   sync: its journal row is dropped before any plan reads it, nothing is
///   sent or deleted for it, and the report names it (`drop_outside_sync`).
/// - A journal row's hash is always bytes git holds, so a conflict records
///   the fragment's bytes, never the folder's unsent ones
///   (`record_conflicts`): a folder's own bytes are never "in sync" until
///   sent.
///
/// It holds for the folder as scanned; a save during the pass's own
/// requests is a race this does not close.
/// Tested as a matrix: `every_pass_changes_only_what_its_scan_accounted_for`.
pub fn pass_over(client: &Client, storage: &CodeStorage, name: &str, dir: &Path, opts: &SyncOptions, local: Local) -> Result<Report, SyncError> {
    let Local { mut state, journal, files: local, stats, refused } = local;
    // World binding (`SyncState::host`): refuse before any read or write.
    let bound_host = client.host.trim_end_matches('/').to_string();
    match (&state.host, &state.repo) {
        (Some(h), Some(r)) if h != &bound_host || r != storage.repo() => {
            return Err(SyncError::Repo(format!(
                "this folder is bound to {} repo {r}, but fragment '{name}' is {} repo {} — the fragment was likely recreated or this folder synced a different host.\n  to sync here anyway: rm {}/.fragment/state.json (rebinds; remote content survives in git history)",
                h, bound_host, storage.repo(), dir.display()
            )));
        }
        _ => {
            state.host = Some(bound_host);
            state.repo = Some(storage.repo().to_string());
        }
    }
    let outside_sync = drop_outside_sync(dir, &local, &mut state)?;
    // a token that only reads (a viewer's): the pass pulls alone, and what
    // a mirror would have sent stays in the folder (`unsent`)
    let read_only = !storage.writes();
    if read_only && opts.mode == Mode::Push {
        return Err(SyncError::Host(CodedError { code: Code::Forbidden, msg: format!("you may read {name}, not change it: a push is an editor's (`--mode pull` reads it)") }));
    }
    let mode = if read_only { Mode::Pull } else { opts.mode };
    let author = Author::writer(&opts.writer_id);
    let blobs = crate::blobs::Blobs::new(client, name);
    let outside_sync = outside_sync.iter().map(|p| p.to_string()).collect();
    let mut report = Report { scan: stats, mode: format!("{:?}", opts.mode).to_lowercase(), read_only, outside_sync, ..Default::default() };
    let mut recorded: HashSet<RepoPath> = HashSet::new();

    // main as this pass sees it: read once, reused by the push's first
    // attempt and by the pull, and after our own commit derived rather than
    // listed again
    assert!(!opts.live || opts.mode == Mode::Pull, "live is only pulled");
    let mut listing = list_branch(storage, if opts.live { LIVE } else { MAIN })?;
    report.refused = refused.iter().chain(&listing.refused).map(|not| not.to_string()).collect();
    for not in &listing.refused {
        eprintln!("warning: refusing the repo's {not}: nothing is written for it");
    }
    adopt_identical(storage, &local, &listing, &mut state)?;
    // candidate local deletions: known remotely before, gone from the
    // listing now, local copy untouched since we saw it
    let local_delete_candidates: Vec<RepoPath> = state
        .files
        .iter()
        .filter(|(p, st)| {
            !listing.files.contains_key(*p)
                && local.get(*p).is_some_and(|lf| lf.sha256 == st.sha256)
        })
        .map(|(p, _)| p.clone())
        .collect();
    let pre = push_plan(&local, &listing.files, &state);
    if read_only && opts.mode == Mode::Mirror {
        report.unsent = pre.upserts.iter().chain(&pre.deletes).chain(&pre.conflicts).map(|p| p.to_string()).collect();
        report.unsent.sort();
    }
    // pull mode never deletes remotely, so would-be push deletions must not
    // trip the guard there (a wiped folder in pull mode just re-downloads)
    let push_side = if mode == Mode::Pull { 0 } else { pre.deletes.len() };
    if let Some(n) = mass_delete_trips(push_side, local_delete_candidates.len(), state.files.len(), opts.apply_mass_delete) {
        report.mass_delete_guard = Some(n);
        // dropping stale rows keeps the guard from re-tripping forever on
        // state that no longer matches either side
        for p in local_delete_candidates {
            state.files.remove(&p);
        }
        save_state(dir, &state, journal.as_deref()).map_err(|e| SyncError::Io(e.to_string()))?;
        report.head = listing.head;
        return Ok(report);
    }

    // ---- push: one commit with expected-parent CAS, bounded rebuilds ----
    if mode != Mode::Pull {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            if attempt > 1 {
                listing = list_main(storage)?;
                // a commit whose answer was lost may have landed: what it
                // wrote now equals the folder, and adopting it (here and in
                // record_conflicts) is what keeps it from reading as a new
                // file to push again or as a conflict with our own bytes
                adopt_identical(storage, &local, &listing, &mut state)?;
            }
            let plan = push_plan(&local, &listing.files, &state);
            record_conflicts(
                ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                &plan.conflicts,
            )?;
            if plan.upserts.is_empty() && plan.deletes.is_empty() {
                break; // nothing to commit (clean, or a replay absorbed)
            }
            let mut changes: Vec<Change> = Vec::with_capacity(plan.upserts.len() + plan.deletes.len());
            // read BLOB_TRANSFERS_AT_ONCE at a time: a file is its bytes, a
            // blob its pointer once uploaded (a HEAD, and the PUT the store
            // lacks). Nothing is committed if one fails.
            let stored = at_once(&plan.upserts, BLOB_TRANSFERS_AT_ONCE, |p| {
                let bytes = fs::read(p.under(dir)).map_err(|e| SyncError::Io(format!("read {p}: {e}")))?;
                blobs.store(bytes).map_err(|e| SyncError::blob(p.as_str(), e))
            });
            let mut failure = None;
            for (p, got) in plan.upserts.iter().zip(stored) {
                match got {
                    Some(Ok(bytes)) => changes.push(Change::Upsert { path: p.clone(), bytes }),
                    Some(Err(e)) => {
                        failure.get_or_insert(e);
                    }
                    None => {}
                }
            }
            if let Some(e) = failure {
                return Err(e);
            }
            for p in &plan.deletes {
                changes.push(Change::Delete { path: p.clone() });
            }
            let msg = format!(
                "fragment sync {}: +{} -{}",
                name,
                plan.upserts.len(),
                plan.deletes.len()
            );
            match storage.commit(listing.head.as_deref(), &msg, &author, &changes) {
                Ok(tip) => {
                    for p in &plan.upserts {
                        if let Some(lf) = local.get(p) {
                            state.files.insert(
                                p.clone(),
                                FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: tip.clone() },
                            );
                        }
                        report.pushed.push(p.to_string());
                    }
                    for p in &plan.deletes {
                        state.files.remove(p);
                        report.deleted_remote.push(p.to_string());
                    }
                    listing = listing.landed(tip, &changes);
                    report.landed = true;
                    break;
                }
                Err(CsError::CasRejected { .. }) if attempt < MAX_CAS_ATTEMPTS => {
                    eprintln!("  branch moved under us — refetching head and rebuilding the diff (attempt {attempt}/{MAX_CAS_ATTEMPTS})");
                    continue;
                }
                Err(CsError::OutcomeUnknown(detail)) if attempt < MAX_CAS_ATTEMPTS => {
                    // never resent blind: the rebuilt diff is empty if it
                    // landed, and the same commit on a fresh head if not
                    eprintln!("  the commit's answer was lost ({detail}) — re-reading the branch to see whether it landed (attempt {attempt}/{MAX_CAS_ATTEMPTS})");
                    report.landed = true; // perhaps: the refresh nudge is harmless either way
                    continue;
                }
                Err(CsError::CasRejected { detail }) => {
                    return Err(SyncError::Cs(CsError::CasRejected {
                        detail: format!("branch kept moving after {MAX_CAS_ATTEMPTS} attempts ({detail}); re-run fragment sync"),
                    }));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // ---- pull: make the folder match main (fetch new/changed; propagate
    // remote deletions per mode) ----
    if mode != Mode::Push {
        let mut fetches: Vec<&TreeEntry> = vec![];
        for rf in listing.files.values() {
            let l = local.get(&rf.path);
            let s = state.files.get(&rf.path);
            // what the scan did not take, at a path the fragment has: none,
            // or something outside sync (`keep_unscanned`)
            let occupied = match l {
                None => match place(dir, &rf.path)? {
                    Some(at) if !vacant(&at)? => Some(at),
                    _ => None,
                },
                Some(_) => None,
            };
            let fetch = match (l, s) {
                (None, _) if occupied.is_some() => {
                    let target = occupied.expect("matched above");
                    keep_unscanned(
                        ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                        &rf.path,
                        &target,
                    )?;
                    false
                }
                // nothing there: fetch it
                (None, _) => true,
                // journal-absent but present locally, and content-different
                // (the bootstrap pass adopted every match): the folder's own
                // bytes, never sent, stay, and the remote's land beside them.
                // A writer's mirror pushed them already, so this is a pass
                // that only reads.
                (Some(_), None) => {
                    record_conflicts(
                        ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                        std::slice::from_ref(&rf.path),
                    )?;
                    false
                }
                (Some(lf), Some(st)) => {
                    let local_changed = st.sha256 != lf.sha256;
                    let remote_changed = st.commit != rf.last_commit_sha;
                    if local_changed && remote_changed {
                        // both changed and push didn't resolve it (push
                        // modes off, or a race) — same conflict treatment
                        record_conflicts(
                            ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                            std::slice::from_ref(&rf.path),
                        )?;
                        false
                    } else {
                        remote_changed && !local_changed
                    }
                }
            };
            if fetch {
                fetches.push(rf);
            }
        }
        // read READS_AT_ONCE at a time; the journal takes them only if
        // every one landed (it is not saved on a failure, so a partial
        // pull never reads as a complete one: the next pass adopts what
        // was written by content and fetches the rest)
        fetches.sort_by(|a, b| a.path.cmp(&b.path));
        let blob_gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let fetched = at_once(&fetches, READS_AT_ONCE, |rf| pull_file(storage, &blobs, &blob_gate, dir, listing.rev(), rf));
        let mut failure = None;
        for (rf, got) in fetches.iter().zip(fetched) {
            match got {
                Some(Ok(Some(row))) => {
                    state.files.insert(rf.path.clone(), row);
                    report.pulled.push(rf.path.to_string());
                }
                Some(Ok(None)) => report.refused.push(through_link(&rf.path)),
                Some(Err(e)) => {
                    failure.get_or_insert(e);
                }
                None => {}
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        // remote deletions: known before, gone now, local copy untouched
        let mut paths: Vec<RepoPath> = state.files.keys().cloned().collect();
        paths.sort();
        for p in paths {
            if listing.files.contains_key(&p) {
                continue;
            }
            let untouched = local.get(&p).is_some_and(|lf| Some(&lf.sha256) == state.files.get(&p).map(|s| &s.sha256));
            if !untouched {
                continue;
            }
            if opts.mode == Mode::Pull && !opts.prune {
                // keep the state row: dropping it here made the deletion
                // unknowable, so a later --prune pass could never apply it.
                // Re-reporting each pass is honest — it is still pending.
                report.withheld_deletions.push(p.to_string());
                continue;
            }
            let Some(target) = place(dir, &p)? else {
                report.refused.push(through_link(&p));
                continue;
            };
            fs::remove_file(target).map_err(|e| SyncError::Io(format!("delete {p}: {e}")))?;
            state.files.remove(&p);
            report.deleted_local.push(p.to_string());
        }
    }

    save_state(dir, &state, journal.as_deref()).map_err(|e| SyncError::Io(e.to_string()))?;
    report.head = listing.head;
    Ok(report)
}

/// Tells the cell its fragment's `moved` ref moved. Our commits and ref moves are
/// EXTERNAL pushes from the cell's perspective: without this nudge its
/// pins wait out the 5-minute poll backstop before app reads and serving
/// see them. Best-effort: the poll covers a missed nudge.
pub fn refresh_pins(client: &Client, name: &str, moved: &str) {
    // the cell reads again only the ref we moved (a cell before that reads both)
    let asked = fragment_proto::Refresh { moved: Some(moved.to_string()) };
    if let Err(e) = client.post_json(&format!("/api/f/{name}/refresh"), &asked) {
        eprintln!("warning: cell pin refresh failed ({e:#}); the poll backstop will catch up");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestorage::Wants;
    use crate::mockcs::MockServer;
    use crate::sync::journal::{load_state, state_path};
    use crate::sync::listing::Listing;
    use crate::sync::tests::{client_for, opts, rp, tmpdir};
    use std::collections::BTreeMap;

    // ---- end-to-end against the mock code.storage ----

    /// The fake's request counts, as a test states them.
    fn counts(routes: &[(&str, u32)]) -> BTreeMap<String, u32> {
        routes.iter().map(|(route, n)| (route.to_string(), *n)).collect()
    }

    /// Goal: a pass reads main's head and listing once. Method: count the
    /// fake's requests for passes over a synced folder. Before, a pass with
    /// nothing to push read the head three times and listed twice, and one
    /// that pushed listed again after its own commit.
    #[test]
    fn a_pass_reads_the_head_and_the_listing_once() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a"), ("b.txt", b"b")]);
        let c = client_for(&mock);
        let dir = tmpdir("reads-once");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        mock.take_requests("");
        let read_once = counts(&[("GET branch", 1), ("GET files/metadata", 1), ("GET storage-token", 1)]);

        let idle = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(idle.pushed.is_empty() && idle.pulled.is_empty() && !idle.landed, "{idle:?}");
        assert_eq!(mock.take_requests(""), read_once, "a mirror pass with nothing to do");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert_eq!(mock.take_requests(""), read_once, "a pull pass with nothing to do");

        fs::write(dir.join("a.txt"), b"changed").unwrap();
        fs::remove_file(dir.join("b.txt")).unwrap();
        let pushed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!((pushed.pushed.as_slice(), pushed.deleted_remote.as_slice()), (&["a.txt".to_string()][..], &["b.txt".to_string()][..]));
        assert!(pushed.pulled.is_empty() && pushed.landed, "{pushed:?}");
        assert_eq!(pushed.head, mock.branch("t--k3x9", "main"), "the pass ends on its own commit");
        assert_eq!(
            mock.take_requests(""),
            counts(&[("GET branch", 1), ("GET files/metadata", 1), ("POST commit-pack", 1), ("GET storage-token", 1), ("POST refresh", 1)]),
            "a pass that pushes lists once, and its pull reads the listing it derived"
        );

        // the derived listing named our commit, so nothing reads as changed
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.pushed.is_empty() && again.pulled.is_empty() && again.conflicts.is_empty(), "{again:?}");
        assert_eq!(mock.take_requests(""), read_once);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: the journal is written when a pass changed it, and only
    /// then. Method: its inode across an idle pass and a pushing one (the
    /// write is a rename, so a new inode). Before, every pass rewrote and
    /// fsynced it.
    #[cfg(unix)]
    #[test]
    fn the_journal_is_written_only_when_it_changed() {
        use std::os::unix::fs::MetadataExt;
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a")]);
        let c = client_for(&mock);
        let dir = tmpdir("journal-writes");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        let journal = || fs::metadata(dir.join(".fragment/state.json")).unwrap().ino();
        let first = journal();
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(journal(), first, "an idle pass leaves it alone");
        fs::write(dir.join("b.txt"), b"b").unwrap();
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_ne!(journal(), first, "a pass that pushed wrote it");
        assert!(load_state(&dir, "t--k3x9").unwrap().files.contains_key("b.txt"));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: the listing a pass derives after its own commit is the one
    /// main lists. Method: commit changes (an edit, a new file, a
    /// deletion) on a listing's head, derive, list main, and compare every
    /// entry.
    #[test]
    fn a_derived_listing_is_the_listing_of_main() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a"), ("b.txt", b"b"), ("c/d.txt", b"d")]);
        let c = client_for(&mock);
        let storage = CodeStorage::connect(&c, "t--k3x9", Wants::Write).unwrap();
        let before = list_main(&storage).unwrap();
        let changes = [
            Change::Upsert { path: rp("a.txt"), bytes: b"a, longer now".to_vec() },
            Change::Upsert { path: rp("c/new.txt"), bytes: b"new".to_vec() },
            Change::Delete { path: rp("b.txt") },
        ];
        let tip = storage.commit(before.head.as_deref(), "edit", &Author::writer("deadbeef"), &changes).unwrap();
        let derived = before.landed(tip, &changes);
        let listed = list_main(&storage).unwrap();
        let entries = |l: &Listing| -> BTreeMap<RepoPath, (u64, String, String)> {
            l.files.values().map(|f| (f.path.clone(), (f.size, f.mode.clone(), f.last_commit_sha.clone()))).collect()
        };
        assert_eq!(derived.head, listed.head);
        assert_eq!(entries(&derived), entries(&listed));
        assert_eq!(entries(&listed).len(), 3, "a.txt, c/d.txt, c/new.txt");
    }

    #[test]
    fn push_conflicting_parent_retries_then_succeeds() {
        // competitor moves the tip once: our pack 409s, we refetch,
        // rebuild, and land on the new tip — both changes survive
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[]);
        mock.sabotage_commit_packs(1);
        let c = client_for(&mock);
        let dir = tmpdir("push-conflict-once");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert_eq!(report.pushed.len(), 1);
        assert!(mock.file_at("t--k3x9", "main", "competitor.txt").is_some(), "competitor commit survives");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.txt").unwrap(), b"alpha");
        assert!(mock.commit_pack_count() >= 2, "one rejected attempt plus one landing");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_conflicting_parent_forever_is_bounded() {
        // every attempt is sabotaged: explicit error after MAX_CAS_ATTEMPTS
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[]);
        mock.sabotage_commit_packs(99);
        let c = client_for(&mock);
        let dir = tmpdir("push-conflict-forever");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        let err = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap_err();
        assert!(err.to_string().contains("branch kept moving"), "got: {err}");
        assert_eq!(mock.commit_pack_count(), MAX_CAS_ATTEMPTS, "exactly the bounded number of attempts");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a sync whose commit landed but lost its answer ends clean.
    /// Method: the fake applies the pass's commit and drops the answer. The
    /// old client sent the pack again (a 409), then recorded a conflict
    /// with a copy of its own bytes and exited 3. Now: one pack, no
    /// conflict, exit 0, and the next pass has nothing to do.
    #[test]
    fn a_lost_commit_answer_adopts_what_landed() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("doc.md", b"base"), ("gone.md", b"g"), ("kept.md", b"k")]);
        let c = client_for(&mock);
        let dir = tmpdir("lost-answer");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join("doc.md"), b"ours, changed").unwrap();
        fs::write(dir.join("new.md"), b"new").unwrap();
        fs::remove_file(dir.join("gone.md")).unwrap();
        mock.drop_commit_answers("t--k3x9", 1);
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(mock.commit_pack_count(), 1, "the pack was sent once");
        let copies = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".conflict-")).count();
        assert_eq!(copies, 0, "no conflict copy");
        assert_eq!(mock.file_at("t--k3x9", "main", "doc.md").unwrap(), b"ours, changed");
        assert_eq!(mock.file_at("t--k3x9", "main", "new.md").unwrap(), b"new");
        assert!(mock.file_at("t--k3x9", "main", "gone.md").is_none());
        assert_eq!(fs::read(dir.join("doc.md")).unwrap(), b"ours, changed");
        let st = load_state(&dir, "t--k3x9").unwrap();
        assert!(!st.files.contains_key("gone.md"), "the landed deletion leaves no row");
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.pushed.is_empty() && again.pulled.is_empty() && again.conflicts.is_empty(), "{again:?}");
        assert_eq!(mock.commit_pack_count(), 1, "and nothing is left to send");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a push whose blob is not stored commits nothing, and says
    /// which file. Method: a folder of notes and one file of 1 MiB, whose
    /// upload fails (the fake host answers no blob threshold), the notes
    /// read at once beside it.
    #[test]
    fn a_push_whose_blob_is_not_stored_commits_nothing() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("seed.md", b"s")]);
        let c = client_for(&mock);
        let dir = tmpdir("push-blob-fails");
        for i in 0..12 {
            fs::write(dir.join(format!("n-{i:02}.md")), format!("note {i}")).unwrap();
        }
        fs::write(dir.join("big.bin"), vec![7u8; fragment_core::blob::BLOB_MIN_BYTES + 17]).unwrap();
        let before = mock.commit_pack_count();
        let failed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap_err();
        assert!(failed.to_string().contains("big.bin"), "{failed}");
        assert_eq!(mock.commit_pack_count(), before, "nothing committed");
        assert!(!state_path(&dir).exists(), "and no journal");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a pull reads its files READS_AT_ONCE at a time, and a failed
    /// read leaves no journal that reads as complete. Method: a 40-file
    /// main pulled into an empty folder (valid), again (replay); into
    /// another with one file's read failing (a typed error, no journal),
    /// then again once it answers (restart): each file is read once in
    /// all, what the failed pass wrote adopted by its content.
    #[test]
    fn a_pull_reads_at_once_and_a_failed_read_saves_no_journal() {
        let mock = crate::mockcs::start();
        let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("d{}/f-{i:02}.md", i % 3), format!("file {i}\n").into_bytes())).collect();
        let seeded: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
        mock.seed_repo("t--k3x9", &seeded);
        let c = client_for(&mock);
        let all_there = |dir: &Path| {
            for (p, b) in &files {
                assert_eq!(fs::read(dir.join(p)).ok().as_deref(), Some(b.as_slice()), "{p}");
            }
            assert_eq!(load_state(dir, "t--k3x9").unwrap().files.len(), files.len(), "a journal row for each");
        };
        let reads = |mock: &MockServer| mock.take_requests("").get("GET file").copied().unwrap_or(0);

        let dir = tmpdir("pull-at-once");
        mock.take_requests("");
        let pulled = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert_eq!(pulled.pulled.len(), files.len());
        all_there(&dir);
        assert_eq!(reads(&mock), 40, "each file read once");
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert!(again.pulled.is_empty(), "{again:?}");
        assert_eq!(reads(&mock), 0, "a replay reads nothing");

        let cut = tmpdir("pull-cut");
        mock.fail_file_read(Some("d1/f-19.md"));
        let failed = sync_once(&c, "t--k3x9", &cut, &opts(Mode::Pull)).unwrap_err();
        assert!(matches!(failed, SyncError::Cs(_)), "{failed}");
        assert!(!state_path(&cut).exists(), "a failed pull saves no journal");
        assert!(!cut.join("d1/f-19.md").exists());
        mock.fail_file_read(None);
        mock.take_requests("");
        let resumed = sync_once(&c, "t--k3x9", &cut, &opts(Mode::Pull)).unwrap();
        all_there(&cut);
        assert!(resumed.pulled.contains(&"d1/f-19.md".to_string()), "{resumed:?}");
        assert_eq!(reads(&mock), 40, "each file read once more: what landed adopted by its content, the rest fetched");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&cut).ok();
    }
}
