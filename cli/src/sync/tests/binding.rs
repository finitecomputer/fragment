//! A folder is bound to one fragment, on one host: its label, another world, a recreated one.

use super::{client_for, opts, tmpdir};
use crate::sync::journal::state_path;
use crate::sync::{sync_once, verify, Mode, SyncError, SyncOptions};
use std::fs;

/// Goal: `fragment sync` (and `verify`) take a fragment's bare label,
/// as every other subcommand does (`Client::fragment`): a folder synced
/// as `t--k3x9` syncs as `t` (prod, 2026-11-02: `fragment sync
/// workarounds` refused a folder bound to `workarounds--f3xz`). Valid:
/// the label syncs the bound folder, and the journal still names it in
/// full. Restart: a later run's client, its names unresolved, verifies
/// it by the label. Invalid: another fragment's label, and a label of
/// none, refuse before anything moves. Replay: a journal an earlier
/// bare-label sync wrote (`"name": "t"`) syncs by either name, and is
/// named in full after.
#[test]
fn a_bare_label_syncs_the_folder_its_fragment_is_bound_to() {
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("a.md", b"a")]);
    mock.seed_repo("u--m4pq", &[("u.md", b"u")]);
    let dir = tmpdir("bare-label");
    let journal = || -> serde_json::Value { serde_json::from_slice(&fs::read(state_path(&dir)).unwrap()).unwrap() };
    sync_once(&client_for(&mock), "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    fs::write(dir.join("b.md"), b"b").unwrap();
    let report = sync_once(&client_for(&mock), "t", &dir, &opts(Mode::Mirror)).unwrap();
    assert_eq!(report.pushed, ["b.md"]);
    assert_eq!(journal()["name"], "t--k3x9", "the journal names it in full");
    let checked = verify(&client_for(&mock), "t", &dir).unwrap();
    assert_eq!((checked.exit_code(), checked.conflicts.len()), (0, 0), "{:?}", checked.conflicts);

    let packs = mock.commit_pack_count();
    fs::write(dir.join("c.md"), b"c").unwrap();
    let other = sync_once(&client_for(&mock), "u", &dir, &opts(Mode::Mirror)).unwrap_err();
    assert!(other.to_string().contains("synced to fragment 't--k3x9', not 'u--m4pq'"), "{other}");
    let none = sync_once(&client_for(&mock), "garden", &dir, &opts(Mode::Mirror)).unwrap_err();
    assert!(matches!(&none, SyncError::Host(e) if e.code == crate::api::Code::NotFound), "{none}");
    assert!(verify(&client_for(&mock), "u", &dir).is_err(), "verify refuses another fragment's label too");
    assert_eq!(mock.commit_pack_count(), packs, "a refused name sends nothing");
    assert!(!dir.join("u.md").exists(), "nor pulls anything");

    for name in ["t--k3x9", "t"] {
        let mut bare = journal();
        bare["name"] = "t".into();
        fs::write(state_path(&dir), serde_json::to_vec(&bare).unwrap()).unwrap();
        sync_once(&client_for(&mock), name, &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(journal()["name"], "t--k3x9", "a bare journal synced as {name} is named in full after");
    }
    let mut wrong = journal();
    wrong["name"] = "u".into();
    fs::write(state_path(&dir), serde_json::to_vec(&wrong).unwrap()).unwrap();
    let err = sync_once(&client_for(&mock), "t", &dir, &opts(Mode::Mirror)).unwrap_err();
    assert!(err.to_string().contains("synced to fragment 'u', not 't--k3x9'"), "another label's journal still refuses: {err}");
    fs::remove_dir_all(&dir).ok();
}

// ---- the world-change regressions (both found live in one day) ----

#[test]
fn journal_bound_to_other_world_refuses() {
    // a journal from a DIFFERENT world must refuse before mirroring
    // that world over the folder. Two dimensions, two incidents:
    // (a) different repo, same host — a recreated fragment
    // (b) same repo name, different host — dev vs prod (repo ids
    //     alone can't tell these apart when the service reports
    //     name-form identities)
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[("old.txt", b"remote world")]);
    let c = client_for(&mock);
    for (tag, journal) in [
        (
            "other repo, same host",
            format!("{{\"schemaVersion\":3,\"name\":\"t--k3x9\",\"host\":\"{}\",\"repo\":\"urn:some-other-repo\",\"files\":{{}}}}", mock.url),
        ),
        (
            "same repo, other host",
            r#"{"schemaVersion":3,"name":"t--k3x9","host":"https://fragment.club","repo":"t--k3x9","files":{}}"#.to_string(),
        ),
    ] {
        let dir = tmpdir("world-mismatch");
        fs::write(dir.join("local.txt"), b"local world").unwrap();
        fs::create_dir_all(dir.join(".fragment")).unwrap();
        fs::write(dir.join(".fragment/state.json"), journal).unwrap();
        let err = match sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)) {
            Err(SyncError::Repo(msg)) => msg,
            other => panic!("{tag}: expected Repo error, got {other:?}"),
        };
        assert!(err.contains("rebinds"), "{tag}: error offers the remedy: {err}");
        // nothing touched: local intact, no commits sent
        assert_eq!(fs::read(dir.join("local.txt")).unwrap(), b"local world", "{tag}");
        assert_eq!(mock.commit_pack_count(), 0, "{tag}");
        fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn recreated_remote_world_guard_holds_local_files() {
    // same repo identity, but the remote was reset (fresh seed, empty
    // tree): the journal's files look remotely-deleted and the mirror
    // would delete them locally — the guard must refuse instead
    let mock = crate::mockcs::start();
    mock.seed_repo("t--k3x9", &[]);
    let c = client_for(&mock);
    let dir = tmpdir("world-reset");
    for i in 0..5 {
        fs::write(dir.join(format!("f{i}.txt")), format!("content {i}").as_bytes()).unwrap();
    }
    sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap(); // binds the journal
    assert_eq!(mock.refresh_count(), 1, "landing sync nudges the pin refresh");
    mock.seed_repo("t--k3x9", &[]); // server-side world reset, same repo id
    let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
    assert!(report.mass_delete_guard.is_some(), "5 of 5 remotely-deleted must trip the guard");
    for i in 0..5 {
        assert!(dir.join(format!("f{i}.txt")).exists(), "f{i}.txt must survive the refusal");
    }
    // the escape hatch applies it
    let o = SyncOptions { apply_mass_delete: true, ..opts(Mode::Mirror) };
    let report = sync_once(&c, "t--k3x9", &dir, &o).unwrap();
    assert!(report.mass_delete_guard.is_none());
    assert_eq!(report.deleted_local.len() + report.pushed.len(), 5, "re-seeded local files push back");
    fs::remove_dir_all(&dir).ok();
}
