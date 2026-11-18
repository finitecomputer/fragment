//! What a push would send, and the mass-deletion guard.

use super::journal::SyncState;
use super::scan::LocalFile;
use fragment_core::codestorage::TreeEntry;
use fragment_proto::RepoPath;
use std::collections::{BTreeMap, HashMap};

/// What one push pass wants to do, computed purely from (local, remote,
/// state) so CAS retries can rebuild it against a fresh listing.
pub(super) struct PushPlan {
    pub(super) upserts: Vec<RepoPath>,
    pub(super) deletes: Vec<RepoPath>, // paths to delete remotely (deleted locally)
    pub(super) conflicts: Vec<RepoPath>,
}

pub(super) fn push_plan(
    local: &BTreeMap<RepoPath, LocalFile>,
    remote: &HashMap<RepoPath, TreeEntry>,
    state: &SyncState,
) -> PushPlan {
    let mut plan = PushPlan { upserts: Vec::new(), deletes: Vec::new(), conflicts: Vec::new() };
    let mut paths: Vec<&RepoPath> = local.keys().chain(remote.keys()).collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let l = local.get(path);
        let r = remote.get(path);
        let s = state.files.get(path);
        match (l, r, s) {
            (Some(lf), Some(rf), Some(st)) => {
                let local_changed = st.sha256 != lf.sha256;
                let remote_changed = st.commit != rf.last_commit_sha;
                if local_changed && remote_changed {
                    plan.conflicts.push(path.clone());
                } else if local_changed {
                    plan.upserts.push(path.clone());
                }
                // local unchanged: remote-side changes are the pull
                // phase's business, never a push
            }
            (Some(_), Some(_), None) => {
                // stateless bootstrap: equal size is NOT identity — the
                // bootstrap pass in sync_once adopts verified-equal files
                // into the journal, so anything still journal-absent here
                // is content-different (or the pass could not run) and
                // local wins; the prior remote content survives in git
                // history
                plan.upserts.push(path.clone());
            }
            (Some(lf), None, Some(st)) => {
                if st.sha256 != lf.sha256 {
                    plan.upserts.push(path.clone()); // content beats deletion
                }
                // else: deleted remotely, local untouched -> pull phase
                // propagates the deletion (guarded)
            }
            (Some(_), None, None) => plan.upserts.push(path.clone()), // new file
            (None, Some(rf), Some(st)) => {
                // deleted locally, remotely untouched since we saw it -> delete remotely
                if st.commit == rf.last_commit_sha {
                    plan.deletes.push(path.clone());
                }
                // else remote moved on: leave it; pull re-materializes
            }
            (None, Some(_), None) => {} // remote-only: pull fetches
            (None, None, _) => {}       // stale state row; pull phase cleans
        }
    }
    plan
}

/// The mass-deletion guard's floor: a pass may always delete this many
/// files. It is 3, not 10: with a floor of 10, deleting 8 of a 10-file
/// fragment (a whole world change, found live) sailed through because
/// 8 ≤ 10 while being 80% of the folder. Small folders deserve the
/// percent protection most.
pub const MASS_DELETE_FLOOR: usize = 3;
/// Past the floor, the share of the known files (percent) a pass may delete.
pub const MASS_DELETE_PERCENT: usize = 30;

/// Mass-deletion guard (ported): a pass that would delete more than
/// max(MASS_DELETE_FLOOR, MASS_DELETE_PERCENT%) of the known files, or ALL
/// of them (the unmounted-disk / replaced-folder case the old
/// root-identity check covered), is refused until --apply-mass-delete.
/// Counts both directions — deletions pushed remotely and deletions
/// applied locally.
pub(super) fn mass_delete_trips(push_deletes: usize, local_deletes: usize, known: usize, apply: bool) -> Option<usize> {
    if apply {
        return None;
    }
    let pending = push_deletes + local_deletes;
    let known = known.max(1);
    let over_share = pending > MASS_DELETE_FLOOR && pending * 100 > known * MASS_DELETE_PERCENT;
    if over_share || (pending == known && pending > 0) {
        Some(pending)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guide states the guard with the code's own numbers.
    #[test]
    fn the_guide_names_the_guards_numbers() {
        let guide = include_str!("../../GUIDE.md").split_whitespace().collect::<Vec<_>>().join(" ");
        let rule = format!("more than max({MASS_DELETE_FLOOR}, {MASS_DELETE_PERCENT}%) of known files, or all of them");
        assert!(guide.contains(&rule), "GUIDE.md should say: {rule}");
    }

    #[test]
    fn mass_delete_guard_semantics() {
        // rule: pending > max(3, 30% of known), or ALL of them
        assert!(mass_delete_trips(11, 0, 30, false).is_some());
        assert!(mass_delete_trips(11, 0, 100, false).is_none()); // 11% only
        assert!(mass_delete_trips(9, 0, 30, false).is_none()); // 30% exactly
        // the live incident: 8 of a 10-file folder (80%) must trip — the
        // old floor of 10 let a whole world change through (8 ≤ 10)
        assert!(mass_delete_trips(0, 8, 10, false).is_some());
        assert!(mass_delete_trips(8, 0, 10, false).is_some());
        // small legitimate deletions stay quiet: 3 or fewer, or a small
        // share of a big folder
        assert!(mass_delete_trips(3, 0, 10, false).is_none());
        assert!(mass_delete_trips(3, 0, 1000, false).is_none());
        // full wipe trips regardless of count (unmounted-folder case)
        assert!(mass_delete_trips(3, 0, 3, false).is_some());
        assert!(mass_delete_trips(1, 0, 1, false).is_some());
        // override
        assert!(mass_delete_trips(11, 0, 30, true).is_none());
        // pull-side deletions count toward the same guard
        assert!(mass_delete_trips(0, 11, 30, false).is_some());
        // empty state floors `known` at 1: mass deletions without state
        // cannot happen for real, but the wipe rule still catches them
        assert!(mass_delete_trips(100, 0, 1, false).is_some());
    }
}
