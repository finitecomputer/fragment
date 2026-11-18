//! Push, pull and mirror, and how a deletion travels in each.

use super::{client_for, opts, tmpdir};
use crate::sync::{sync_once, Mode, SyncOptions};
use std::fs;

#[test]
fn mirror_pushes_and_pulls() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("site/remote-only.txt", b"r")]);
    let c = client_for(&mock);
    let dir = tmpdir("mirror");
    fs::write(dir.join("local-only.txt"), b"l").unwrap();
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(report.pushed, vec!["local-only.txt"]);
    assert_eq!(report.pulled, vec!["site/remote-only.txt"]);
    assert_eq!(fs::read(dir.join("site/remote-only.txt")).unwrap(), b"r");
    assert!(mock.file_at("t--k3x9", "main", "local-only.txt").is_some());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn live_is_pulled_not_main() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.txt", b"a1"), ("gone.txt", b"g")]);
    mock.set_branch("t--k3x9", "live", &mock.branch("t--k3x9", "main").unwrap());
    mock.external_commit("t--k3x9", "main", &[("a.txt", Some(b"a2")), ("new.txt", Some(b"n"))], "not deployed");
    let c = client_for(&mock);
    let dir = tmpdir("live");
    let live = SyncOptions { live: true, prune: true, ..opts(Mode::Pull) };
    sync_once(&c, "t--k3x9", &dir, &live).unwrap();
    assert_eq!((fs::read(dir.join("a.txt")).unwrap(), dir.join("new.txt").exists()), (b"a1".to_vec(), false), "live's files, not main's");
    fs::write(dir.join("local.txt"), b"l").unwrap();
    // a deploy moves live: the next pull brings it, deletions included, and pushes nothing
    mock.external_commit("t--k3x9", "main", &[("gone.txt", None)], "remove");
    mock.set_branch("t--k3x9", "live", &mock.branch("t--k3x9", "main").unwrap());
    let report = sync_once(&c, "t--k3x9", &dir, &live).unwrap();
    assert_eq!((fs::read(dir.join("a.txt")).unwrap(), dir.join("new.txt").exists(), dir.join("gone.txt").exists()), (b"a2".to_vec(), true, false));
    assert!(report.pushed.is_empty() && mock.file_at("t--k3x9", "main", "local.txt").is_none());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn deletion_pushes_and_propagates_back() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
    let c = client_for(&mock);
    let dir = tmpdir("delete");
    // first pass: adopt both files locally
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    // delete one locally, push the deletion
    fs::remove_file(dir.join("gone.txt")).unwrap();
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
    assert_eq!(report.deleted_remote, vec!["gone.txt"]);
    assert!(mock.file_at("t--k3x9", "main", "gone.txt").is_none());
    assert!(mock.file_at("t--k3x9", "main", "kept.txt").is_some());
    // a second folder (fresh state) pulling sees the deletion
    let dir2 = tmpdir("delete-other");
    sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Pull)).unwrap();
    assert!(dir2.join("kept.txt").exists());
    assert!(!dir2.join("gone.txt").exists(), "pull of an empty path must not create it");
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&dir2).ok();
}

#[test]
fn remote_deletion_propagates_in_mirror() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
    let c = client_for(&mock);
    // two folders adopt both files
    let dir = tmpdir("rdelete");
    let dir2 = tmpdir("rdelete-other");
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    // dir2 pulls, deletes gone.txt, pushes the deletion
    sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Pull)).unwrap();
    fs::remove_file(dir2.join("gone.txt")).unwrap();
    sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Push)).unwrap();
    // mirror folder: local untouched → deletion propagates locally
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(report.deleted_local, vec!["gone.txt"]);
    assert!(!dir.join("gone.txt").exists());
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&dir2).ok();
}

#[test]
fn withheld_deletion_still_applies_on_a_later_prune_pass() {
    // pull without --prune withholds; the FOLLOWING pass with --prune
    // must apply it — dropping the state row on withhold made the
    // deletion unknowable (e2e: filesync modes sequence)
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
    let c = client_for(&mock);
    let dir = tmpdir("prune-late");
    let other = tmpdir("prune-late-other");
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
    sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
    fs::remove_file(other.join("gone.txt")).unwrap();
    sync_once(&c, "t--k3x9", &other, &opts(Mode::Push)).unwrap();
    // withhold first
    let withheld = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
    assert_eq!(withheld.withheld_deletions, vec!["gone.txt"]);
    assert!(dir.join("gone.txt").exists());
    // then apply
    let report = sync_once(&c, "t--k3x9", &dir, &SyncOptions { prune: true, ..opts(Mode::Pull) }).unwrap();
    assert_eq!(report.deleted_local, vec!["gone.txt"]);
    assert!(!dir.join("gone.txt").exists());
    assert!(dir.join("kept.txt").exists());
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&other).ok();
}

#[test]
fn mass_deletion_guard_blocks_push() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a", b"1"), ("b", b"2"), ("c", b"3")]);
    let c = client_for(&mock);
    let dir = tmpdir("guard");
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    // wipe the folder locally: full-wipe rule trips (3 known, 3 pending)
    fs::remove_file(dir.join("a")).unwrap();
    fs::remove_file(dir.join("b")).unwrap();
    fs::remove_file(dir.join("c")).unwrap();
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
    assert!(report.mass_delete_guard.is_some());
    assert_eq!(report.exit_code(), 4);
    assert!(mock.file_at("t--k3x9", "main", "a").is_some(), "nothing deleted while guarded");
    // override applies it
    let report = sync_once(&c, "t--k3x9", &dir, &SyncOptions { apply_mass_delete: true, ..opts(Mode::Push) }).unwrap();
    assert_eq!(report.deleted_remote.len(), 3);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn wiped_folder_in_pull_mode_re_downloads_not_guard() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a", b"1"), ("b", b"2"), ("c", b"3")]);
    let c = client_for(&mock);
    let dir = tmpdir("wipe-pull");
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
    fs::remove_file(dir.join("a")).unwrap();
    fs::remove_file(dir.join("b")).unwrap();
    fs::remove_file(dir.join("c")).unwrap();
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
    assert!(report.mass_delete_guard.is_none(), "pull must not trip on local deletions");
    assert_eq!(report.pulled.len(), 3, "everything re-downloads");
    fs::remove_dir_all(&dir).ok();
}
