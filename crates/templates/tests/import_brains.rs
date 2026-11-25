//! The import's team-brain helper (`import-agent-home/profiles/
//! finite-computer.brains.py`, FIN-774), run as the profile runs it
//! (`python3 -I`), on brains made of files: a team's brain on finite.chat
//! comes together one teammate's copy at a time, by the old brain's own
//! versions (FIN-773), and never takes what a person must decide.
#![cfg(unix)]

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

/// The helper, from this package's folder, where cargo runs its tests.
const HELPER: &str = "../../templates/skills/skills/productivity/import-agent-home/profiles/finite-computer.brains.py";
const OLD: &str = "brain-1";
const RECORD: &str = "brain/imported-from-finite.json";

/// The old brain at one sequence, as a move stages it: its folders (path,
/// id, whether the old agent could read it) and its pages (path, id,
/// revision, that revision's text). A folder it couldn't read lists none.
struct Old<'a> {
    sequence: u64,
    folders: Vec<(&'a str, &'a str, bool)>,
    pages: Vec<(&'a str, &'a str, u64, &'a str)>,
}

fn hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// A folder of the test's own: the team's brain (`brain`, as its agents
/// sync it) and each teammate's copy beside it.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("import-brains-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn put(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn link(&self, rel: &str, to: &str) {
        let p = self.0.join(rel);
        let _ = std::fs::remove_file(&p);
        std::os::unix::fs::symlink(self.0.join(to), p).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(rel)).ok()
    }

    fn rm(&self, rel: &str) {
        let p = self.0.join(rel);
        let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir_all(&p));
    }

    /// Every file under `rel` with its text, a link as `-> <target>`.
    fn files(&self, rel: &str) -> BTreeMap<String, String> {
        fn walk(root: &std::path::Path, at: &std::path::Path, out: &mut BTreeMap<String, String>) {
            for e in std::fs::read_dir(at).unwrap() {
                let p = e.unwrap().path();
                let name = p.strip_prefix(root).unwrap().to_str().unwrap().to_string();
                match std::fs::symlink_metadata(&p).unwrap() {
                    m if m.is_symlink() => drop(out.insert(name, format!("-> {}", std::fs::read_link(&p).unwrap().display()))),
                    m if m.is_dir() => walk(root, &p, out),
                    _ => drop(out.insert(name, std::fs::read_to_string(&p).unwrap())),
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.0.join(rel), &self.0.join(rel), &mut out);
        out
    }

    /// The helper, as the profile runs it: its answer, or what it said
    /// when it refused. Never a traceback.
    fn run(&self, args: &[&str]) -> Result<Value, String> {
        self.run_capped("unlimited", args)
    }

    /// The helper with each file it writes capped at `kb` KiB (`ulimit
    /// -f`), as a full disk stops a write partway.
    fn run_capped(&self, kb: &str, args: &[&str]) -> Result<Value, String> {
        let helper = std::fs::canonicalize(HELPER).expect("the helper, from crates/templates");
        let script = format!("ulimit -f {kb} && exec python3 -I \"$@\"");
        let out = Command::new("bash").arg("-c").arg(script).arg("bash").arg(helper).args(args).current_dir(&self.0).output().expect("bash runs");
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        assert!(!said.contains("Traceback"), "{args:?}: {said}");
        match out.status.success() {
            true => Ok(serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}"))),
            false => Err(said),
        }
    }

    /// `old` staged at `at` with its name file beside it, as a move stages
    /// it: each page its revision's text, but those the old agent `edited`
    /// and never synced.
    fn stage(&self, at: &str, old: &Old, edited: &[(&str, &str)]) {
        for (path, _, _, text) in &old.pages {
            self.put(&format!("{at}/{path}"), edited.iter().find(|(p, _)| p == path).map_or(text, |(_, t)| t));
        }
        let folders: Vec<Value> = old.folders.iter().map(|(path, id, read)| json!({"path": path, "brain": null, "id": id, "read": read})).collect();
        let pages: Vec<Value> = old.pages.iter().map(|(path, id, rev, text)| json!({"path": path, "brain": null, "object": id, "revision": rev, "sha256": hex(text)})).collect();
        let versions = json!({"sequence": old.sequence, "folders": folders, "pages": pages});
        self.put(&format!("{at}.json"), &json!({"name": "Team", "kind": "organization", "owner": "other", "versions": versions}).to_string());
    }

    /// A teammate's reference copy, kept as the profile keeps one.
    fn copy(&self, at: &str, old: &Old, edited: &[(&str, &str)]) {
        self.stage(at, old, edited);
        self.run(&["copied", OLD, &format!("{at}.json"), at]).unwrap();
    }

    /// The team's brain, made from its founder's copy as the profile makes
    /// it: the copy's files, less the folders left out, then `start`.
    fn found(&self, old: &Old, edited: &[(&str, &str)], left_out: &[&str]) -> Value {
        self.copy("founder", old, edited);
        self.make(left_out)
    }

    /// The brain made from the founder's copy as it stands (`found`).
    fn make(&self, left_out: &[&str]) -> Value {
        for (path, text) in self.files("founder") {
            self.put(&format!("brain/{path}"), &text);
        }
        for folder in left_out {
            self.rm(&format!("brain/{folder}"));
        }
        self.start().unwrap()
    }

    fn start(&self) -> Result<Value, String> {
        self.run(&["start", OLD, "founder.json", "brain", "founder"])
    }

    fn catch_up(&self, at: &str) -> Value {
        self.run(&["catch-up", OLD, &format!("{at}.json"), "brain", at]).unwrap()
    }

    fn record(&self) -> Value {
        serde_json::from_str(&self.read(RECORD).unwrap()).unwrap()
    }
}

/// A catch-up's answer: nothing taken and nothing listed, but `said`.
fn answer(said: Value) -> Value {
    let mut all = json!({"added": 0, "updated": 0, "assets": 0, "both": [], "gone": [], "unsynced": [], "missing": [], "older": [], "extra": [], "refused": [], "unshared": [], "unread": [], "settled": false});
    for (k, v) in said.as_object().unwrap() {
        assert!(all.get(k).is_some(), "{k}");
        all[k] = v.clone();
    }
    all
}

/// Goal (FIN-774): each teammate's catch-up takes what changed only in the
/// old brain, keeps what changed only here, lists what changed on both or
/// was deleted there, and the last to move leaves the brain whole. Two
/// copies at different sequences: the older takes nothing a newer one
/// took, but may catch up a folder only it could read. Replay: the same
/// copy again moves nothing and lists the same.
#[test]
fn a_team_brain_comes_together_one_move_at_a_time() {
    let d = Dir::new("together");
    let at10 = Old {
        sequence: 10,
        folders: vec![("Notes", "n", true), ("Board", "b", true), ("Leadership", "l", true)],
        pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/b.md", "b", 1, "b1"), ("Notes/g.md", "g", 1, "g1"), ("Board/m.md", "m", 1, "m1"), ("Leadership/x.md", "x", 1, "x1")],
    };
    // the founder leaves the restricted folder out: never shared, nor named
    assert_eq!(d.found(&at10, &[], &["Leadership"]), json!({"recorded": 4, "shared": ["Board", "Notes"], "not shared": ["Leadership"], "removed": [], "refused": []}));
    assert!(!d.read(RECORD).unwrap().contains("Leadership"));
    d.put("brain/Notes/b.md", "b1, edited here");

    // a newer copy, of a teammate who never could read Board; its note's
    // asset is brought in and the note rewritten after `copied`
    let (note, rewritten) = ("---\nresource: /data/c.png\n---\nc1", "---\nresource: raw/assets/c.png\n---\nc1");
    let at30 = Old {
        sequence: 30,
        folders: vec![("Notes", "n", true), ("Board", "b", false), ("Leadership", "l", true)],
        pages: vec![("Notes/a.md", "a", 2, "a2"), ("Notes/b.md", "b", 2, "b2"), ("Notes/c.md", "c", 1, note), ("Leadership/x.md", "x", 2, "x2")],
    };
    d.copy("t30", &at30, &[]);
    d.put("t30/Notes/c.md", rewritten);
    d.put("t30/Notes/raw/assets/c.png", "c-png");
    let listed = json!({"both": ["Notes/b.md"], "gone": ["Notes/g.md"], "unshared": ["Leadership"], "unread": ["Board"]});
    let mut first = listed.clone();
    first.as_object_mut().unwrap().extend([("updated".into(), json!(1)), ("added".into(), json!(1)), ("assets".into(), json!(1))]);
    assert_eq!(d.catch_up("t30"), answer(first));
    let brain = d.files("brain");
    assert_eq!(brain["Notes/a.md"], "a2", "changed only there: taken");
    assert_eq!(brain["Notes/b.md"], "b1, edited here", "changed on both: kept, and listed");
    assert_eq!((brain["Notes/c.md"].as_str(), brain["Notes/raw/assets/c.png"].as_str()), (rewritten, "c-png"), "new there: added, with its asset");
    assert_eq!(brain["Notes/g.md"], "g1", "deleted there: listed, never deleted");
    assert_eq!(brain["Board/m.md"], "m1", "a folder this copy couldn't read: untouched");
    assert!(!brain.keys().any(|p| p.starts_with("Leadership/")), "a folder left out never comes in");
    assert_eq!(d.catch_up("t30"), answer(listed), "replay");
    assert_eq!(d.files("brain"), brain, "replay: nothing moves");

    // an older copy, of a teammate who could read Board: Notes takes
    // nothing, not even a page the old brain made and deleted since; Board
    // was taken last from an older copy still, so it catches up
    let at20 = Old {
        sequence: 20,
        folders: vec![("Notes", "n", true), ("Board", "b", true), ("Leadership", "l", false)],
        pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/b.md", "b", 1, "b1"), ("Notes/g.md", "g", 1, "g1"), ("Notes/o.md", "o", 1, "o1"), ("Board/m.md", "m", 2, "m2")],
    };
    d.copy("t20", &at20, &[]);
    assert_eq!(d.catch_up("t20"), answer(json!({"updated": 1, "older": ["Notes/o.md"], "unread": ["Leadership"]})));
    assert_eq!((d.read("brain/Board/m.md").as_deref(), d.read("brain/Notes/a.md").as_deref()), (Some("m2"), Some("a2")));
    assert!(d.read("brain/Notes/o.md").is_none());

    // a person settles what was listed; the last to move finds it whole
    d.put("brain/Notes/b.md", "b2");
    d.rm("brain/Notes/g.md");
    let at40 = Old {
        sequence: 40,
        folders: vec![("Notes", "n", true), ("Board", "b", true), ("Leadership", "l", false)],
        pages: vec![("Notes/a.md", "a", 2, "a2"), ("Notes/b.md", "b", 2, "b2"), ("Notes/c.md", "c", 1, note), ("Board/m.md", "m", 2, "m2")],
    };
    d.copy("t40", &at40, &[]);
    d.put("t40/Notes/c.md", rewritten);
    d.put("t40/Notes/raw/assets/c.png", "c-png");
    assert_eq!(d.catch_up("t40"), answer(json!({"unread": ["Leadership"], "settled": true})));
    assert_eq!(d.record()["pages"]["Notes/b.md"]["revision"], 2, "the same text here: recorded as the newer version");
}

/// Goal: a catch-up brings nothing into a folder the brain doesn't share
/// (left out when it was made, a child left out, one made since, another
/// folder at a shared path), nor an asset into a folder this copy couldn't
/// read; a folder it couldn't read is untouched. Nor does the founder's
/// push carry a folder left out: not its assets, which sit in its parent's
/// wiki, nor a folder's files the old agent could no longer read. Invalid:
/// what would open a restricted folder to the whole team.
#[test]
fn what_the_brain_doesnt_share_never_comes_in() {
    let d = Dir::new("folders");
    let (private, shared) = ("---\nresource: raw/assets/private.pdf\n---\np1", "---\nresource: raw/assets/shared.png\n---\na1");
    let at10 = Old {
        sequence: 10,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", true), ("Board", "b", true), ("Team", "t", true), ("Team/Public", "tp", true), ("Archive", "ar", false)],
        pages: vec![("Notes/a.md", "a", 1, shared), ("Notes/Private/p.md", "p", 1, private), ("Board/k.md", "k", 1, "k1"), ("Team/t.md", "t", 1, "t1"), ("Team/Public/u.md", "u", 1, "u1")],
    };
    d.copy("founder", &at10, &[]);
    d.put("founder/Notes/raw/assets/private.pdf", "the private folder's");
    d.put("founder/Notes/raw/assets/shared.png", "a shared note's");
    d.put("founder/Archive/old.md", "from before the old agent lost it");
    assert_eq!(
        d.make(&["Notes/Private"]),
        json!({"recorded": 4, "shared": ["Board", "Notes", "Team", "Team/Public"], "not shared": ["Archive", "Notes/Private"], "removed": ["Archive/old.md", "Notes/raw/assets/private.pdf"], "refused": []})
    );
    let made = d.files("brain");
    assert!(made.contains_key("Notes/raw/assets/shared.png") && !made.keys().any(|p| p.contains("private") || p.starts_with("Archive")), "{made:?}");
    let note = "---\nresource: raw/assets/v.png\n---\nv1";
    let at20 = Old {
        sequence: 20,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", true), ("Finance", "f", true), ("Board", "b2", true), ("Team", "t", false), ("Team/Public", "tp", true)],
        pages: vec![
            ("Notes/a.md", "a", 2, "a2"),
            ("Notes/Private/p.md", "p", 2, "p2"),
            ("Finance/f.md", "f", 1, "f1"),
            ("Board/z.md", "z", 1, "z1"),
            ("Team/Public/u.md", "u", 1, "u1"),
            ("Team/Public/v.md", "v", 1, note),
        ],
    };
    d.copy("t20", &at20, &[]);
    d.put("t20/Team/raw/assets/v.png", "v-png");
    let before = d.files("brain");
    assert_eq!(
        d.catch_up("t20"),
        answer(json!({"updated": 1, "both": ["Team/Public/v.md"], "extra": ["Team/raw/assets/v.png"], "unshared": ["Board", "Finance", "Notes/Private"], "unread": ["Team"]}))
    );
    let after = d.files("brain");
    let changed: Vec<_> = after.keys().filter(|p| before.get(*p) != after.get(*p)).collect();
    assert_eq!(changed, ["Notes/a.md", "imported-from-finite.json"], "only the shared folder it could read, and the record");
}

/// Goal: "changed" is by the old brain's revisions, never by bytes. An
/// edit an old agent never synced is never taken, nor written over; a page
/// deleted here never comes back; another page moved onto a path is for a
/// person; a record that lost a page's entry has it again.
#[test]
fn versions_decide_never_bytes() {
    let d = Dir::new("versions");
    let pages = ["a", "u", "w", "d", "e", "p", "h"].map(|x| (format!("Notes/{x}.md"), x, format!("{x}1")));
    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: pages.iter().map(|(p, id, t)| (p.as_str(), *id, 1, t.as_str())).collect() };
    // the founder's old agent edited u and w, and never synced them
    d.found(&at10, &[("Notes/u.md", "u, never synced"), ("Notes/w.md", "w, never synced")], &[]);
    assert_eq!(d.record()["pages"]["Notes/u.md"]["written"], Value::Null);
    for deleted in ["w", "d", "e"] {
        d.rm(&format!("brain/Notes/{deleted}.md"));
    }
    let mut record = d.record();
    record["pages"].as_object_mut().unwrap().remove("Notes/h.md");
    d.put(RECORD, &record.to_string());

    let at20 = Old {
        sequence: 20,
        folders: vec![("Notes", "n", true)],
        pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/u.md", "u", 2, "u2"), ("Notes/w.md", "w", 2, "w2"), ("Notes/d.md", "d", 1, "d1"), ("Notes/e.md", "e", 2, "e2"), ("Notes/p.md", "q", 1, "q1"), ("Notes/h.md", "h", 1, "h1")],
    };
    // this teammate's old agent edited a, and never synced it
    d.copy("t20", &at20, &[("Notes/a.md", "a, never synced")]);
    assert_eq!(d.catch_up("t20"), answer(json!({"both": ["Notes/e.md", "Notes/p.md", "Notes/u.md", "Notes/w.md"], "unsynced": ["Notes/a.md"]})));
    let brain = d.files("brain");
    assert_eq!((brain["Notes/a.md"].as_str(), brain["Notes/u.md"].as_str(), brain["Notes/p.md"].as_str()), ("a1", "u, never synced", "p1"));
    assert!(["w", "d", "e"].iter().all(|x| !brain.contains_key(&format!("Notes/{x}.md"))), "deleted here: never back");
    assert_eq!(d.record()["pages"]["Notes/h.md"], json!({"id": [null, "h"], "revision": 1, "written": hex("h1")}), "healed");
}

/// Goal: a catch-up reads and writes only inside the brain and its copy:
/// a path that climbs out or goes through a link is refused, an asset is
/// one its note's frontmatter names, never written over, never through a
/// link, and something in the way is listed, never a crash.
#[test]
fn paths_and_assets_stay_in_the_brain() {
    let d = Dir::new("paths");
    let with = |name: &str| format!("---\ntitle: {name}\nresource: raw/assets/{name}.png\n---\n{name}");
    let (s, q, diff, link) = (with("s"), with("q"), with("diff"), with("link"));
    // over the 300 bytes a repo path may be, which sync never carries
    let long = format!("Notes/{}/long.md", vec!["d".repeat(100); 3].join("/"));
    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: vec![("Notes/s.md", "s", 1, s.as_str()), ("Notes/q.md", "q", 1, "q1"), ("Notes/k.md", "k", 1, "k1")] };
    d.found(&at10, &[], &[]);
    d.put("brain/Notes/raw/assets/q.png", "q-png");
    d.put("brain/Notes/raw/assets/diff.png", "diff here");
    d.rm("brain/Notes/k.md");
    d.put("brain/Notes/k.md/inside", "a folder at a recorded page's path");
    d.put("brain/Notes/dir.md/inside", "a folder where a page goes");
    d.put("brain/Notes/file", "a file where a folder goes");
    d.put("outside/secret", "never in the brain");

    let at20 = Old {
        sequence: 20,
        folders: vec![("Notes", "n", true)],
        pages: vec![
            ("Notes/s.md", "s", 1, s.as_str()),
            ("Notes/q.md", "q", 2, q.as_str()),
            ("Notes/k.md", "k", 1, "k1"),
            ("Notes/diff.md", "diff", 1, diff.as_str()),
            ("Notes/link.md", "link", 1, link.as_str()),
            ("Notes/body.md", "body", 1, "---\ntitle: body\n---\nresource: raw/assets/none.png"),
            ("Notes/dir.md", "dir", 1, "dir1"),
            ("Notes/file/x.md", "x", 1, "x1"),
            ("Notes/../../escape.md", "esc", 1, "esc1"),
            ("Notes/sym.md", "sym", 1, "sym1"),
            ("Notes/x.conflict-1.md", "conflict", 1, "c1"),
            (long.as_str(), "long", 1, "l1"),
        ],
    };
    d.copy("t20", &at20, &[]);
    for (asset, text) in [("s", "s-png"), ("q", "q-png"), ("diff", "diff there")] {
        d.put(&format!("t20/Notes/raw/assets/{asset}.png"), text);
    }
    d.link("t20/Notes/raw/assets/link.png", "outside/secret");
    d.link("t20/Notes/sym.md", "outside/secret");
    assert_eq!(
        d.catch_up("t20"),
        answer(json!({
            "added": 1, "updated": 1, "assets": 1,
            "both": ["Notes/diff.md", "Notes/dir.md", "Notes/file/x.md", "Notes/link.md"],
            "refused": ["Notes/../../escape.md", long, "Notes/sym.md", "Notes/x.conflict-1.md"],
            "extra": ["Notes/raw/assets/diff.png", "Notes/raw/assets/link.png"],
        }))
    );
    let brain = d.files("brain");
    assert_eq!(brain["Notes/raw/assets/s.png"], "s-png", "the same note, with an asset only the copy holds: the asset comes");
    assert_eq!((brain["Notes/q.md"].as_str(), brain["Notes/raw/assets/q.png"].as_str()), (q.as_str(), "q-png"), "its asset here already, the same");
    assert_eq!(brain["Notes/raw/assets/diff.png"], "diff here", "never written over");
    assert_eq!(brain["Notes/body.md"], "---\ntitle: body\n---\nresource: raw/assets/none.png", "a resource after the frontmatter is no asset");
    assert_eq!(brain["Notes/k.md/inside"], "a folder at a recorded page's path");
    assert_eq!(brain["Notes/file"], "a file where a folder goes");
    assert!(!brain.values().any(|text| text.contains("never in the brain")) && !brain.keys().any(|p| p.contains("link") || p.contains("sym")));
    assert!(d.read("escape.md").as_deref() == Some("esc1") && !d.files("brain").contains_key("escape.md"));
    assert!(!brain.keys().any(|p| p.contains(".conflict-") || p.contains("long.md")), "what sync never carries never looks taken");
}

/// Goal: a write that fails partway (a full disk) leaves the brain's page
/// as it was, never part of the newer one that its push would carry.
/// Replay: with room again, the same catch-up takes it.
#[test]
fn a_write_that_fails_leaves_the_page_as_it_was() {
    let d = Dir::new("full");
    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: vec![("Notes/big.md", "big", 1, "small")] };
    d.found(&at10, &[], &[]);
    let big = "b".repeat(8192);
    d.copy("t20", &Old { sequence: 20, folders: vec![("Notes", "n", true)], pages: vec![("Notes/big.md", "big", 2, &big)] }, &[]);
    let args = ["catch-up", OLD, "t20.json", "brain", "t20"];
    assert_eq!(d.run_capped("4", &args).unwrap(), answer(json!({"both": ["Notes/big.md"]})));
    assert_eq!(d.files("brain").keys().collect::<Vec<_>>(), ["Notes/big.md", "imported-from-finite.json"], "nothing partial left beside it");
    assert_eq!(d.read("brain/Notes/big.md").as_deref(), Some("small"));
    assert_eq!(d.run(&args).unwrap(), answer(json!({"updated": 1, "settled": true})));
    assert_eq!(d.read("brain/Notes/big.md"), Some(big));
}

/// Goal: an import that can't tell versions compares by hand, as before
/// (an export, a stage from before FIN-773, a copy kept without them, a
/// brain made without a record), and one that finds a record it didn't
/// write changes nothing. Replay: `start` again before the push is safe;
/// over a brain that changed since, it stops.
#[test]
fn without_versions_or_a_record_it_compares_by_hand() {
    let d = Dir::new("invalid");
    d.put("old.json", r#"{"name": "Team", "owner": "other", "versions": null}"#);
    d.put("old/Notes/a.md", "a1");
    d.put("junk.json", "not JSON");
    for args in [["copied", OLD, "old.json", "old", ""], ["copied", OLD, "none.json", "old", ""], ["start", OLD, "old.json", "brain", "old"]] {
        let args: Vec<&str> = args.into_iter().filter(|a| !a.is_empty()).collect();
        assert!(d.run(&args).unwrap_err().starts_with("NO VERSIONS"), "{args:?}");
    }
    assert!(d.run(&["copied", OLD, "junk.json", "old"]).unwrap_err().starts_with("STOP"));

    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: vec![("Notes/a.md", "a", 1, "a1")] };
    let started = d.found(&at10, &[], &[]);
    assert_eq!(d.start(), Ok(started), "start again before the push");
    let at20 = Old { sequence: 20, folders: vec![("Notes", "n", true)], pages: vec![("Notes/a.md", "a", 2, "a2")] };
    d.stage("bare", &at20, &[]);
    assert!(d.run(&["catch-up", OLD, "bare.json", "brain", "bare"]).unwrap_err().starts_with("NO VERSIONS"), "a copy kept without `copied`");

    d.copy("t20", &at20, &[]);
    let record = d.read(RECORD).unwrap();
    let mut string_id = d.record();
    string_id["folders"]["Notes"]["id"] = json!("n");
    for bad in [record.replace(OLD, "brain-2"), "null".into(), r#"{"from": "finite.computer"}"#.into(), string_id.to_string()] {
        d.put(RECORD, &bad);
        assert!(d.run(&["catch-up", OLD, "t20.json", "brain", "t20"]).unwrap_err().starts_with("STOP"), "{bad}");
        assert_eq!(d.read("brain/Notes/a.md").as_deref(), Some("a1"), "nothing changes");
    }
    d.put("elsewhere.json", &record);
    d.link(RECORD, "elsewhere.json");
    assert!(d.run(&["catch-up", OLD, "t20.json", "brain", "t20"]).unwrap_err().starts_with("STOP"), "a record through a link");
    d.rm(RECORD);
    assert!(d.run(&["catch-up", OLD, "t20.json", "brain", "t20"]).unwrap_err().starts_with("NO RECORD"));

    d.put(RECORD, &record);
    d.put("brain/Notes/a.md", "a1, edited here");
    assert!(d.start().unwrap_err().starts_with("STOP"), "start over a brain that changed");
    assert_eq!(d.read(RECORD).unwrap(), record);
}
