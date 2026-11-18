//! A viewer's pass: it pulls, sends nothing, and keeps the viewer's own changes.

use super::{client_for, conflict_copies, opts, tmpdir};
use crate::api::Code;
use crate::codestorage::CsError;
use crate::sync::{sync_once, Mode, SyncError};
use std::fs;

/// Goal: a viewer syncs a fragment read-only (its token only reads),
/// and never loses an edit: a mirror pulls, sends nothing, and keeps
/// what it would have sent in the folder, named and exiting 5; a file
/// deleted there comes back; both sides' change is a conflict with
/// theirs beside it (exit 3); a pull reads whoever asks. Method: the
/// host routes hold the caller a viewer; count the commit packs.
#[test]
fn a_viewers_mirror_pulls_and_sends_nothing() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.md", b"theirs 1"), ("b.md", b"b")]);
    mock.view_only("t--k3x9", true);
    let c = client_for(&mock);
    let dir = tmpdir("viewer-mirror");
    let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert!(first.read_only && first.unsent.is_empty() && first.exit_code() == 0, "{first:?}");
    assert_eq!(first.pulled, ["a.md", "b.md"]);

    // edited, made and deleted here: none of it sent, none of it lost
    fs::write(dir.join("a.md"), "mine").unwrap();
    fs::write(dir.join("c.md"), "new").unwrap();
    fs::remove_file(dir.join("b.md")).unwrap();
    let held = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(held.unsent, ["a.md", "b.md", "c.md"]);
    assert_eq!(held.exit_code(), 5);
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine");
    assert_eq!(fs::read_to_string(dir.join("c.md")).unwrap(), "new");
    assert_eq!(fs::read_to_string(dir.join("b.md")).unwrap(), "b", "a file deleted here is pulled back");
    assert_eq!(mock.file_at("t--k3x9", "main", "a.md").unwrap(), b"theirs 1");
    assert!(mock.file_at("t--k3x9", "main", "c.md").is_none());

    // its editor changes what this viewer changed: theirs lands beside
    mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"theirs 2"))], "theirs");
    let both = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(both.conflicts.len(), 1, "{both:?}");
    assert_eq!(both.exit_code(), 3);
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "the viewer's copy stays");
    let beside: Vec<String> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.contains(".conflict-")).collect();
    assert_eq!(beside.len(), 1, "theirs saved beside: {beside:?}");
    assert_eq!(fs::read_to_string(dir.join(&beside[0])).unwrap(), "theirs 2");

    // a pull asks for a token that only reads, so a viewer's pulls
    let other = tmpdir("viewer-pull");
    let pulled = sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
    assert!(pulled.read_only && pulled.unsent.is_empty() && fs::read_to_string(other.join("a.md")).unwrap() == "theirs 2", "{pulled:?}");
    assert_eq!(mock.commit_pack_count(), 0, "nothing was ever sent");
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&other).ok();
}

/// Goal: a viewer's push is refused before anything moves, in the
/// host's words, and an editor's mirror still writes. Method: the host
/// routes hold the caller a viewer, then its editor again.
#[test]
fn a_viewers_push_is_refused_and_an_editors_mirror_writes() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.md", b"1")]);
    mock.view_only("t--k3x9", true);
    let c = client_for(&mock);
    let dir = tmpdir("viewer-push");
    fs::write(dir.join("b.md"), "mine").unwrap();
    let refused = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap_err();
    assert!(matches!(&refused, SyncError::Cs(CsError::Host(e)) if e.code == Code::Forbidden), "{refused}");
    assert!(refused.to_string().contains("editor"), "{refused}");
    assert_eq!(mock.commit_pack_count(), 0);

    mock.view_only("t--k3x9", false);
    let wrote = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert!(!wrote.read_only && wrote.pushed == ["b.md"] && wrote.unsent.is_empty(), "{wrote:?}");
    fs::remove_dir_all(&dir).ok();
}

/// Goal: a viewer's bytes are never overwritten, where the folder has
/// no journal for them (a folder its own, or a file the fragment adds
/// later beside the viewer's own new one): theirs lands beside, the
/// viewer's stays unsent, and the next pass leaves both. Method: the
/// host routes hold the caller a viewer; a folder without a journal,
/// then a remote addition colliding with an unsent new file.
#[test]
fn a_viewers_own_file_is_never_fetched_over() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.md", b"remote-original")]);
    mock.view_only("t--k3x9", true);
    let c = client_for(&mock);
    let dir = tmpdir("viewer-bootstrap");
    fs::write(dir.join("a.md"), "local-unpublished").unwrap();
    let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "local-unpublished");
    assert_eq!(conflict_copies(&dir), ["remote-original"]);
    assert!(first.unsent == ["a.md"] && first.exit_code() == 3, "{first:?}");
    let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "local-unpublished");
    assert!(again.unsent == ["a.md"] && again.conflicts.is_empty() && again.exit_code() == 5, "the same remote is no new conflict: {again:?}");
    assert_eq!(conflict_copies(&dir).len(), 1);

    // a new file of the viewer's, then the fragment adds one there
    fs::write(dir.join("c.md"), "mine, new").unwrap();
    let unsent = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(unsent.unsent, ["a.md", "c.md"]);
    mock.external_commit("t--k3x9", "main", &[("c.md", Some(b"theirs, new"))], "theirs");
    let collided = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(fs::read_to_string(dir.join("c.md")).unwrap(), "mine, new");
    assert!(conflict_copies(&dir).contains(&"theirs, new".to_string()), "{collided:?}");
    assert_eq!(collided.exit_code(), 3);
    assert_eq!(mock.commit_pack_count(), 0);
    fs::remove_dir_all(&dir).ok();
}

/// Goal: a viewer's edit outlives every later remote change: each new
/// remote version is a conflict again (theirs beside, the edit kept),
/// across a restart (a fresh process reads the journal), and a remote
/// deletion leaves the edit. Method: the host routes hold the caller a
/// viewer; three remote changes after one local edit.
#[test]
fn a_viewers_edit_outlives_each_later_remote_change() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.md", b"v1")]);
    mock.view_only("t--k3x9", true);
    let c = client_for(&mock);
    let dir = tmpdir("viewer-edit-lives");
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    fs::write(dir.join("a.md"), "mine").unwrap();
    mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v2"))], "theirs");
    let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(second.exit_code(), 3);
    mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v3"))], "theirs again");
    let third = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "never fetched over: {third:?}");
    assert!(third.exit_code() == 3 && third.unsent == ["a.md"], "{third:?}");
    // theirs beside it, each version its own copy
    let mut copies = conflict_copies(&dir);
    copies.sort();
    assert_eq!(copies, ["v2", "v3"]);
    let quiet = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert!(quiet.conflicts.is_empty() && quiet.unsent == ["a.md"] && quiet.exit_code() == 5, "{quiet:?}");

    mock.external_commit("t--k3x9", "main", &[("a.md", None)], "removed");
    let removed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "a remote deletion leaves the edit: {removed:?}");
    assert!(removed.deleted_local.is_empty() && removed.unsent == ["a.md"] && removed.exit_code() == 5, "{removed:?}");

    // its editor again, the same folder sends the edit
    mock.view_only("t--k3x9", false);
    let sent = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert!(sent.pushed == ["a.md"] && mock.file_at("t--k3x9", "main", "a.md").as_deref() == Some(&b"mine"[..]), "{sent:?}");
    fs::remove_dir_all(&dir).ok();
}
