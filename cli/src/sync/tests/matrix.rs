//! The pass's invariant as a matrix: a pass changes only what its scan accounted for.

use super::{client_for, git_folder, opts};
use crate::api::Code;
use crate::codestorage::CsError;
use crate::mockcs::MockServer;
use crate::sync::at_once::at_once;
use crate::sync::scan::relative;
use crate::sync::{sync_once, Mode, Report, SyncError, SyncOptions};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---- the pass's invariant, as a matrix (`pass_over`) ----

/// What the folder holds at `a.md` when the pass runs, beside `b.md`
/// (synced, untouched).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Here {
    Same,
    Edited,
    /// nothing is there
    Deleted,
    /// synced, then `.gitignore`d, its bytes as synced
    Ignored,
    /// synced, then `.gitignore`d and edited
    IgnoredEdited,
    /// the folder's own file, never synced (main had none)
    New,
    NewIgnored,
    /// a folder with no journal holds its own `a.md`
    Own,
    OwnIgnored,
}

/// What main holds at `a.md` when the pass runs, against what the
/// folder last synced (a folder with no journal: the seed).
#[derive(Clone, Copy, Debug, PartialEq)]
enum There {
    Same,
    Edited,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Who {
    Viewer,
    Editor,
    /// synced as a viewer, made an editor before the pass
    Promoted,
}

/// One case's fragment and folder, and every (path, bytes) main has
/// ever held: the bytes git keeps.
struct World<'a> {
    mock: &'a MockServer,
    name: String,
    dir: PathBuf,
    held: HashSet<(String, Vec<u8>)>,
}

impl World<'_> {
    fn main(&self) -> BTreeMap<String, Vec<u8>> {
        self.mock.paths(&self.name, "main").into_iter().filter_map(|p| Some((p.clone(), self.mock.file_at(&self.name, "main", &p)?))).collect()
    }

    fn note_main(&mut self) {
        let main = self.main();
        self.held.extend(main);
    }

    fn commit(&mut self, bytes: Option<&[u8]>) {
        self.mock.external_commit(&self.name, "main", &[("a.md", bytes)], "theirs");
        self.note_main();
    }

    /// Every regular file in the folder but sync's and git's own.
    fn folder(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in fs::read_dir(at).unwrap().flatten() {
                let (path, kind) = (e.path(), e.file_type().unwrap());
                let rel = relative(root, &path);
                if rel == ".fragment" || rel == ".git" {
                    continue;
                }
                if kind.is_dir() {
                    walk(root, &path, out);
                } else if kind.is_file() {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.dir, &self.dir, &mut out);
        out
    }

    /// One pass, checked against the invariant: bytes git never held
    /// stay where they were; main loses a path only where the folder
    /// had nothing; what is outside sync is never sent; a pass that
    /// only reads sends nothing. `None`: a viewer's push, refused with
    /// nothing moved.
    fn pass(&mut self, mode: Mode, ignored: bool, case: &str) -> Option<Report> {
        self.note_main();
        let (folder, main) = (self.folder(), self.main());
        let packs = self.mock.requests(&self.name, "POST commit-pack");
        let o = SyncOptions { prune: true, ..opts(mode) };
        let got = sync_once(&client_for(self.mock), &self.name, &self.dir, &o);
        let (folder_after, main_after) = (self.folder(), self.main());
        let sent = self.mock.requests(&self.name, "POST commit-pack") - packs;
        let report = match got {
            Ok(report) => report,
            Err(SyncError::Cs(CsError::Host(e))) if e.code == Code::Forbidden && mode == Mode::Push => {
                assert!(folder == folder_after && main == main_after && sent == 0, "{case}: a refused push moves nothing");
                return None;
            }
            Err(e) => panic!("{case}: {e}"),
        };
        self.note_main();
        for (path, bytes) in &folder {
            if !self.held.contains(&(path.clone(), bytes.clone())) {
                assert_eq!(folder_after.get(path), Some(bytes), "{case}: {path}'s bytes, in no commit, are kept: {report:?}");
            }
        }
        for path in main.keys().filter(|p| !main_after.contains_key(*p)) {
            assert!(!folder.contains_key(path), "{case}: main lost {path}, which the folder still had: {report:?}");
        }
        if ignored {
            if let Some(mine) = folder.get("a.md").filter(|b| b.starts_with(b"mine")) {
                assert!(main_after.get("a.md") != Some(mine), "{case}: an ignored file was sent: {report:?}");
            }
        }
        if report.read_only {
            assert_eq!(sent, 0, "{case}: a pass that only reads sends nothing");
        }
        Some(report)
    }
}

/// Goal: one invariant holds for every pass, whoever runs it, in any
/// mode, whatever the folder and main did to a file: a pass changes
/// only what its scan accounted for. Bytes that are in no commit stay
/// in the folder; main loses a path only where the folder has nothing
/// at all; a file outside sync (here `.gitignore`d) is never sent, its
/// journal row is dropped and the pass names it once; a pass that only
/// reads sends nothing; and a writer's own bytes reach main. Method:
/// each (folder, main, role, mode) from a synced folder (or one with
/// no journal), then main changes the file again, then two more
/// passes, the last of which moves nothing. Replay and restart: each
/// pass is a fresh `sync_once`, which reads the journal again.
#[test]
fn every_pass_changes_only_what_its_scan_accounted_for() {
    let mock = crate::mockcs::start();
    let heres = [Here::Same, Here::Edited, Here::Deleted, Here::Ignored, Here::IgnoredEdited, Here::New, Here::NewIgnored, Here::Own, Here::OwnIgnored];
    let mut cases = Vec::new();
    for here in heres {
        for there in [There::Same, There::Edited, There::Deleted] {
            for who in [Who::Viewer, Who::Editor, Who::Promoted] {
                for mode in [Mode::Mirror, Mode::Pull, Mode::Push] {
                    let fresh = matches!(here, Here::New | Here::NewIgnored);
                    let own = matches!(here, Here::Own | Here::OwnIgnored);
                    if !(fresh && there == There::Deleted) && !(own && who == Who::Promoted) {
                        cases.push((cases.len(), here, there, who, mode));
                    }
                }
            }
        }
    }
    assert_eq!(cases.len(), 207);
    // each case its own fragment and folder, eight at a time
    let ran = at_once(&cases, 8, |&(i, here, there, who, mode)| {
        one_case(&mock, i, here, there, who, mode);
        Ok::<(), ()>(())
    });
    assert!(ran.iter().all(|r| matches!(r, Some(Ok(())))));
}

/// One case of the matrix above.
fn one_case(mock: &MockServer, i: usize, here: Here, there: There, who: Who, mode: Mode) {
    let case = format!("{here:?}/{there:?}/{who:?}/{mode:?}");
    let name = format!("m{i}--k3x9");
    let fresh = matches!(here, Here::New | Here::NewIgnored);
    let seed: &[(&str, &[u8])] = if fresh { &[("b.md", b"b")] } else { &[("a.md", b"v1"), ("b.md", b"b")] };
    mock.seed_repo(&name, seed);
    mock.view_only(&name, who != Who::Editor);
    let dir = git_folder(&format!("matrix-{i}"), "nothing.md");
    let mut w = World { mock, name, dir: dir.clone(), held: HashSet::new() };
    w.note_main();
    if !matches!(here, Here::Own | Here::OwnIgnored) {
        sync_once(&client_for(mock), &w.name, &dir, &opts(Mode::Mirror)).unwrap();
    }
    let ignore = || fs::write(dir.join(".gitignore"), "nothing.md\na.md\n").unwrap();
    match here {
        Here::Same => {}
        Here::Edited => fs::write(dir.join("a.md"), "mine: edited").unwrap(),
        Here::Deleted => fs::remove_file(dir.join("a.md")).unwrap(),
        Here::Ignored => ignore(),
        Here::IgnoredEdited | Here::NewIgnored | Here::OwnIgnored => {
            ignore();
            fs::write(dir.join("a.md"), "mine: ignored").unwrap();
        }
        Here::New | Here::Own => fs::write(dir.join("a.md"), "mine: own").unwrap(),
    }
    match there {
        There::Same => {}
        There::Edited => w.commit(Some(b"v2")),
        There::Deleted => w.commit(None),
    }
    if who == Who::Promoted {
        mock.view_only(&w.name, false);
    }
    let ignored = matches!(here, Here::Ignored | Here::IgnoredEdited | Here::NewIgnored | Here::OwnIgnored);
    let Some(first) = w.pass(mode, ignored, &case) else {
        assert!(who == Who::Viewer && mode == Mode::Push, "{case}: only a viewer's push is refused");
        fs::remove_dir_all(&dir).ok();
        return;
    };
    // a file synced, then put outside sync: named once, and the same
    // bytes on both sides are no conflict
    let left = matches!(here, Here::Ignored | Here::IgnoredEdited);
    assert_eq!(first.outside_sync, if left { vec!["a.md".to_string()] } else { vec![] }, "{case}: {first:?}");
    if here == Here::Ignored && there == There::Same {
        assert!(first.conflicts.is_empty() && first.exit_code() == 0 && w.main().get("a.md").map(Vec::as_slice) == Some(b"v1"), "{case}: {first:?}");
    }
    w.commit(Some(b"v3"));
    let second = w.pass(mode, ignored, &case).expect("a pass");
    let third = w.pass(mode, ignored, &case).expect("a pass");
    assert!(second.outside_sync.is_empty() && third.outside_sync.is_empty(), "{case}: named once: {second:?} {third:?}");
    let (folder, main) = (w.folder(), w.main());
    let last = w.pass(mode, ignored, &case).expect("a pass");
    let still = last.pushed.is_empty() && last.pulled.is_empty() && last.deleted_remote.is_empty() && last.deleted_local.is_empty();
    assert!(still && w.folder() == folder && w.main() == main, "{case}: the passes settle: {last:?}");
    // a writer's own bytes reach main (a conflict keeps them, theirs
    // beside, and the next pass sends them)
    let writes = who != Who::Viewer && mode != Mode::Pull;
    if writes && matches!(here, Here::Edited | Here::New | Here::Own) {
        let mine = if here == Here::Edited { "mine: edited" } else { "mine: own" };
        assert!(w.held.contains(&("a.md".to_string(), mine.as_bytes().to_vec())), "{case}: an editor's bytes are sent");
    }
    fs::remove_dir_all(&dir).ok();
}
