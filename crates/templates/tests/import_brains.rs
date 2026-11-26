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
        self.lay();
        self.start(left_out).unwrap()
    }

    /// The brain's work folder made from the founder's copy, before `start`.
    fn lay(&self) {
        for (path, text) in self.files("founder") {
            self.put(&format!("brain/{path}"), &text);
        }
    }

    /// `start`, naming the folders left out, as the profile runs it.
    fn start(&self, left_out: &[&str]) -> Result<Value, String> {
        self.run(&[&["start", OLD, "founder.json", "brain", "founder"], left_out].concat())
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
    let mut all = json!({"added": 0, "updated": 0, "assets": 0, "both": [], "gone": [], "unsynced": [], "missing": [], "older": [], "withheld": [], "extra": [], "refused": [], "unshared": [], "unread": [], "settled": false});
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
    assert_eq!(d.found(&at10, &[], &["Leadership"]), json!({"versions": true, "recorded": 4, "shared": ["Board", "Notes"], "not shared": ["Leadership"], "removed": ["Leadership/x.md"], "withheld": [], "refused": []}));
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
    // a YAML comment after its asset's name, as a note may have
    let (private, shared) = ("---\nresource: raw/assets/private.pdf\n---\np1", "---\nresource: raw/assets/shared.png # the diagram\n---\na1");
    let at10 = Old {
        sequence: 10,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", true), ("Board", "b", true), ("Team", "t", true), ("Team/Public", "tp", true), ("Archive", "ar", false), ("Drafts", "dr", true), ("Notes/Old", "no", false)],
        pages: vec![
            ("Notes/a.md", "a", 1, shared),
            ("Notes/Private/p.md", "p", 1, private),
            ("Board/k.md", "k", 1, "k1"),
            ("Team/t.md", "t", 1, "t1"),
            ("Team/Public/u.md", "u", 1, "u1"),
            ("Archive/listed.md", "al", 1, "al1"),
            ("Drafts/y.conflict-1.md", "y", 1, "y1"),
        ],
    };
    d.copy("founder", &at10, &[]);
    d.put("founder/Notes/raw/assets/private.pdf", "the private folder's");
    d.put("founder/Notes/raw/assets/shared.png", "a shared note's");
    // a sync conflict copy names the private asset too: sync never carries it
    d.put("founder/Notes/a.conflict-1.md", "---\nresource: raw/assets/private.pdf\n---\nan old conflict");
    d.put("founder/Notes/raw/assets/old.png", "a folder the old agent lost's");
    d.put("founder/Archive/old.md", "from before the old agent lost it");
    d.put("founder/Drafts/draft.md", "a draft never synced");
    d.put("founder/Notes/Old/n.md", "---\nresource: raw/assets/old.png\n---\nfrom before");
    d.lay();
    // the stale note only in the work folder (laid from an older copy)
    d.rm("founder/Notes/Old");
    let started = json!({
        "versions": true,
        "recorded": 4,
        "shared": ["Board", "Drafts", "Notes", "Team", "Team/Public"],
        "not shared": ["Archive", "Notes/Old", "Notes/Private"],
        "removed": ["Archive/listed.md", "Archive/old.md", "Notes/Old/n.md", "Notes/Private/p.md", "Notes/raw/assets/old.png", "Notes/raw/assets/private.pdf"],
        "withheld": ["Notes/raw/assets/old.png", "Notes/raw/assets/private.pdf"],
        "refused": ["Drafts/y.conflict-1.md"],
    });
    assert_eq!(d.start(&["Notes/Private"]), Ok(started.clone()), "a folder's assets go with it, wherever its note is");
    let made = d.files("brain");
    assert!(made.contains_key("Notes/raw/assets/shared.png") && made.contains_key("Drafts/draft.md"), "{made:?}");
    assert!(!made.keys().any(|p| p.contains("private") || p.contains("Old") || p.contains("old.png") || p.starts_with("Archive")), "{made:?}");
    let mut again = started;
    again["removed"] = json!([]);
    assert_eq!(d.start(&["Notes/Private"]), Ok(again), "replay: the same record, nothing more to take out");
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
        answer(json!({"updated": 1, "added": 1, "withheld": ["Team/raw/assets/v.png"], "unshared": ["Board", "Finance", "Notes/Private"], "unread": ["Team"]}))
    );
    let after = d.files("brain");
    let changed: Vec<_> = after.keys().filter(|p| before.get(*p) != after.get(*p)).collect();
    assert_eq!(changed, ["Notes/a.md", "Team/Public/v.md", "imported-from-finite.json"], "only the shared folders it could read, and the record");
    assert!(d.record()["withheld"].as_array().unwrap().contains(&json!("Team/raw/assets/v.png")), "what it withheld, recorded");

    // a quoted name it can't read exactly names nothing, never another
    // file: here a stale attachment no note names, while a folder is out
    let q = Dir::new("quoted");
    let at10 = Old {
        sequence: 10,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", true)],
        pages: vec![
            ("Notes/a.md", "a", 1, "---\nresource: 'raw/assets/report.pdf''s notes.pdf'\n---\na1"),
            ("Notes/b.md", "b", 1, "---\nresource: raw/assets/report.pdf's notes.pdf\n---\nb1"),
            ("Notes/Private/p.md", "p", 1, "p1"),
        ],
    };
    q.copy("founder", &at10, &[]);
    q.put("founder/Notes/raw/assets/report.pdf", "the private folder's");
    q.put("founder/Notes/raw/assets/report.pdf's notes.pdf", "a shared note's");
    assert_eq!(
        q.make(&["Notes/Private"]),
        json!({"versions": true, "recorded": 2, "shared": ["Notes"], "not shared": ["Notes/Private"], "removed": ["Notes/Private/p.md", "Notes/raw/assets/report.pdf"], "withheld": ["Notes/raw/assets/report.pdf"], "refused": []})
    );
}

/// Goal: "changed" is by the old brain's revisions, never by bytes. An
/// edit an old agent never synced is never taken, nor written over; a page
/// deleted here never comes back; another page moved onto a path is for a
/// person; a record that lost a page's entry has it again.
#[test]
fn versions_decide_never_bytes() {
    let d = Dir::new("versions");
    let pages = ["a", "u", "v", "w", "d", "e", "p", "h"].map(|x| (format!("Notes/{x}.md"), x, format!("{x}1")));
    let mut at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: pages.iter().map(|(p, id, t)| (p.as_str(), *id, 1, t.as_str())).collect() };
    at10.pages.push(("Notes/r.md", "r", 3, "r3"));
    // the founder's old agent edited u, v and w, and put r back as its
    // revision 1 was, and never synced them
    d.found(&at10, &[("Notes/u.md", "u, never synced"), ("Notes/v.md", "v, never synced"), ("Notes/w.md", "w, never synced"), ("Notes/r.md", "r1")], &[]);
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
        pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/u.md", "u", 2, "u2"), ("Notes/v.md", "v", 1, "v1"), ("Notes/w.md", "w", 2, "w2"), ("Notes/d.md", "d", 1, "d1"), ("Notes/e.md", "e", 2, "e2"), ("Notes/p.md", "q", 1, "q1"), ("Notes/h.md", "h", 1, "h1"), ("Notes/r.md", "r", 1, "r1")],
    };
    // this teammate's old agent edited a, and never synced it
    d.copy("t20", &at20, &[("Notes/a.md", "a, never synced")]);
    // v: the brain took the founder's edit, never revision 1's text, which only this copy holds
    assert_eq!(d.catch_up("t20"), answer(json!({"both": ["Notes/e.md", "Notes/p.md", "Notes/u.md", "Notes/v.md", "Notes/w.md"], "unsynced": ["Notes/a.md"]})));
    let brain = d.files("brain");
    assert_eq!((brain["Notes/a.md"].as_str(), brain["Notes/u.md"].as_str(), brain["Notes/p.md"].as_str()), ("a1", "u, never synced", "p1"));
    assert!(["w", "d", "e"].iter().all(|x| !brain.contains_key(&format!("Notes/{x}.md"))), "deleted here: never back");
    assert_eq!(d.record()["pages"]["Notes/h.md"], json!({"id": [null, "h"], "revision": 1, "written": hex("h1")}), "healed");
    assert_eq!(d.record()["pages"]["Notes/r.md"], json!({"id": [null, "r"], "revision": 3, "written": null}), "never healed to an older revision");
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
            ("Notes/scan.md", "scan", 1, "---\nresource: raw/assets/scan.conflict-1.png\n---\nscan"),
            (long.as_str(), "long", 1, "l1"),
        ],
    };
    d.copy("t20", &at20, &[]);
    for (asset, text) in [("s", "s-png"), ("q", "q-png"), ("diff", "diff there"), ("scan.conflict-1", "never synced")] {
        d.put(&format!("t20/Notes/raw/assets/{asset}.png"), text);
    }
    d.link("t20/Notes/raw/assets/link.png", "outside/secret");
    d.link("t20/Notes/sym.md", "outside/secret");
    assert_eq!(
        d.catch_up("t20"),
        answer(json!({
            "added": 1, "updated": 1, "assets": 1,
            "both": ["Notes/diff.md", "Notes/dir.md", "Notes/file/x.md", "Notes/link.md", "Notes/scan.md"],
            "refused": ["Notes/../../escape.md", long, "Notes/sym.md", "Notes/x.conflict-1.md"],
            "extra": ["Notes/raw/assets/diff.png", "Notes/raw/assets/link.png", "Notes/raw/assets/scan.conflict-1.png"],
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
    // a name as long as a file's may be, less its `.md`
    let long = format!("Notes/{}.md", "n".repeat(252));
    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: vec![("Notes/big.md", "big", 1, "small"), (long.as_str(), "long", 1, "l1")] };
    d.found(&at10, &[], &[]);
    d.put("brain/Notes/.big.md.partial", "someone else's, never synced");
    let big = "b".repeat(8192);
    d.copy("t20", &Old { sequence: 20, folders: vec![("Notes", "n", true)], pages: vec![("Notes/big.md", "big", 2, &big), (long.as_str(), "long", 2, "l2")] }, &[]);
    let args = ["catch-up", OLD, "t20.json", "brain", "t20"];
    assert_eq!(d.run_capped("4", &args).unwrap(), answer(json!({"updated": 1, "both": ["Notes/big.md"]})));
    let brain = d.files("brain");
    assert!(!brain.keys().any(|p| p.contains("fragment-partial")), "nothing partial left beside it: {brain:?}");
    assert_eq!((brain["Notes/big.md"].as_str(), brain[&long].as_str()), ("small", "l2"));
    assert_eq!(d.run(&args).unwrap(), answer(json!({"updated": 1, "settled": true})));
    assert_eq!(d.read("brain/Notes/big.md"), Some(big));
    assert_eq!(d.read("brain/Notes/.big.md.partial").as_deref(), Some("someone else's, never synced"), "another file is never touched");
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
    for args in [["copied", OLD, "old.json", "old"], ["copied", OLD, "none.json", "old"]] {
        assert!(d.run(&args).unwrap_err().starts_with("NO VERSIONS"), "{args:?}");
    }
    assert!(d.run(&["copied", OLD, "junk.json", "old"]).unwrap_err().starts_with("STOP"));

    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true)], pages: vec![("Notes/a.md", "a", 1, "a1")] };
    let started = d.found(&at10, &[], &[]);
    assert_eq!(d.start(&[]), Ok(started), "start again before the push");
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
    assert!(d.start(&[]).unwrap_err().starts_with("STOP"), "start over a brain that changed");
    assert_eq!(d.read(RECORD).unwrap(), record);

    // a folder left out that the copy doesn't have stops it; with nothing
    // out, an asset no note names is no one else's
    let o = Dir::new("stray");
    o.copy("founder", &Old { sequence: 10, folders: vec![("Notes", "n", true), ("Notes/Private", "np", true)], pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/Private/p.md", "p", 1, "p1")] }, &[]);
    o.put("founder/Notes/raw/assets/stray.png", "no note names it");
    o.lay();
    assert!(o.start(&["Notes/Privte"]).unwrap_err().starts_with("STOP: Notes/Privte is no folder"), "a folder left out is one the copy has");
    assert!(o.read(RECORD).is_none() && o.read("brain/Notes/Private/p.md").is_some(), "nothing recorded, nothing taken out");
    let whole = o.start(&[]).unwrap();
    assert_eq!((&whole["removed"], &whole["withheld"]), (&json!([]), &json!([])), "with nothing out, it's no one else's: it stays");

    // what it must take out of the push, and can't: it stops, recording nothing
    let e = Dir::new("stuck");
    e.copy("founder", &Old { sequence: 10, folders: vec![("Notes", "n", true), ("Archive", "ar", false)], pages: vec![("Notes/a.md", "a", 1, "a1")] }, &[]);
    e.put("founder/Archive/old.md", "from before the old agent lost it");
    e.lay();
    let archive = e.0.join("brain/Archive");
    let mode = |m: u32| std::fs::set_permissions(&archive, std::os::unix::fs::PermissionsExt::from_mode(m)).unwrap();
    mode(0o555);
    if std::fs::write(archive.join("probe"), "").is_err() {
        assert!(e.start(&[]).unwrap_err().starts_with("STOP"));
        assert!(e.read(RECORD).is_none());
    }
    mode(0o755);
}

/// Goal (FIN-774): a folder left out never reaches the team, with versions
/// or without (an export, or a stage from before FIN-773): `start` takes it
/// out of the founder's push, with the attachments its notes name, and one
/// a kept page names too stays out for a person. Invalid: without versions
/// it once stopped first, and a left-out folder's payroll went to everyone
/// invited. Replay: again, nothing more comes out and nothing is recorded.
#[test]
fn without_versions_a_folder_left_out_stays_out() {
    let d = Dir::new("unversioned");
    for (path, text) in [
        ("Notes/a.md", "---\nresource: raw/assets/shared.png\n---\nkept"),
        ("Notes/both.md", "---\nresource: raw/assets/both.png\n---\nkept"),
        ("Notes/Private/payroll.md", "---\nresource: raw/assets/payroll.pdf\n---\nleft out"),
        ("Notes/Private/both.md", "---\nresource: raw/assets/both.png\n---\nleft out"),
        ("Notes/raw/assets/shared.png", "shared"),
        ("Notes/raw/assets/both.png", "both"),
        ("Notes/raw/assets/payroll.pdf", "payroll"),
    ] {
        d.put(&format!("founder/{path}"), text);
    }
    d.put("founder.json", r#"{"name": "Team", "kind": "organization", "owner": "other", "versions": null}"#);
    assert!(d.run(&["copied", OLD, "founder.json", "founder"]).unwrap_err().starts_with("NO VERSIONS"), "kept without versions");
    d.lay();
    let started = json!({
        "versions": false, "recorded": 0, "shared": [], "not shared": ["Notes/Private"],
        "removed": ["Notes/Private/both.md", "Notes/Private/payroll.md", "Notes/raw/assets/both.png", "Notes/raw/assets/payroll.pdf"],
        "withheld": ["Notes/raw/assets/both.png", "Notes/raw/assets/payroll.pdf"], "refused": [],
    });
    assert_eq!(d.start(&["Notes/Private"]), Ok(started.clone()));
    let pushed = || d.files("brain").into_keys().collect::<Vec<_>>();
    assert_eq!(pushed(), ["Notes/a.md", "Notes/both.md", "Notes/raw/assets/shared.png"], "the push holds what is kept, and no record");
    let again = d.start(&["Notes/Private"]).unwrap();
    assert_eq!((&again["removed"], &again["withheld"]), (&json!([]), &started["withheld"]), "replay: the same withheld, nothing more to take out");
    assert_eq!(pushed(), ["Notes/a.md", "Notes/both.md", "Notes/raw/assets/shared.png"]);
}

/// Goal (FIN-774): an attachment goes up only when no note of a folder the
/// brain doesn't bring in names it, and, while a folder is out, some note
/// does. One withheld is recorded, so no later catch-up brings it in, even
/// beside its unchanged note; its note still comes, and a person lets it in
/// by adding it. Invalid: what re-added a kept-out attachment at the next
/// teammate's import, and published a stale attachment of a folder the
/// founder's agent couldn't read, with no note of it staged.
#[test]
fn a_withheld_attachment_stays_out() {
    let d = Dir::new("withheld");
    let kept = "---\nresource: raw/assets/both.png\n---\nkept";
    let at10 = Old {
        sequence: 10,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", true), ("Notes/Board", "nb", false)],
        pages: vec![("Notes/both.md", "both", 1, kept), ("Notes/Private/p.md", "p", 1, "---\nresource: raw/assets/both.png\n---\nleft out")],
    };
    d.copy("founder", &at10, &[]);
    d.put("founder/Notes/raw/assets/both.png", "both");
    // a stale attachment of the folder the founder's agent can't read, no note of it staged
    d.put("founder/Notes/raw/assets/payroll.pdf", "payroll");
    let started = d.make(&["Notes/Private"]);
    let withheld = json!(["Notes/raw/assets/both.png", "Notes/raw/assets/payroll.pdf"]);
    assert_eq!((&started["withheld"], &d.record()["withheld"]), (&withheld, &withheld), "withheld, and recorded");
    assert!(d.read("brain/Notes/raw/assets/both.png").is_none() && d.read("brain/Notes/raw/assets/payroll.pdf").is_none());

    // the next teammate: the same note, a new one naming a withheld
    // attachment, and one an unread folder's stale note names too
    let (new, c) = ("---\nresource: raw/assets/payroll.pdf\n---\nnew", "---\nresource: raw/assets/c.png\n---\nc");
    let at20 = Old {
        sequence: 20,
        folders: vec![("Notes", "n", true), ("Notes/Private", "np", false), ("Notes/Board", "nb", false)],
        pages: vec![("Notes/both.md", "both", 1, kept), ("Notes/new.md", "new", 1, new), ("Notes/c.md", "c", 1, c)],
    };
    d.copy("t20", &at20, &[]);
    for asset in ["both.png", "payroll.pdf", "c.png"] {
        d.put(&format!("t20/Notes/raw/assets/{asset}"), asset);
    }
    d.put("t20/Notes/Private/stale.md", "---\nresource: raw/assets/c.png\n---\nfrom before");
    let listed = json!({
        "withheld": ["Notes/raw/assets/both.png", "Notes/raw/assets/c.png", "Notes/raw/assets/payroll.pdf"],
        "extra": ["Notes/Private/stale.md"], "unread": ["Notes/Board", "Notes/Private"],
    });
    let mut first = listed.clone();
    first["added"] = json!(2);
    assert_eq!(d.catch_up("t20"), answer(first), "the notes come, their attachments don't");
    let brain = d.files("brain");
    assert!(brain.contains_key("Notes/new.md") && brain.contains_key("Notes/c.md") && !brain.keys().any(|p| p.contains("raw/assets")), "{brain:?}");
    assert_eq!(d.record()["withheld"], listed["withheld"], "recorded, for every later catch-up");
    assert_eq!(d.catch_up("t20"), answer(listed), "replay");

    // a person lets one in by adding it: nothing more to keep out of it
    d.put("brain/Notes/raw/assets/both.png", "both");
    assert_eq!(d.catch_up("t20")["withheld"], json!(["Notes/raw/assets/c.png", "Notes/raw/assets/payroll.pdf"]));
    assert_eq!(d.read("brain/Notes/raw/assets/both.png").as_deref(), Some("both"));

    // and with nothing left out, only a folder the founder's agent couldn't read
    let e = Dir::new("withheld-unread");
    e.copy("founder", &Old { sequence: 10, folders: vec![("Notes", "n", true), ("Notes/Private", "np", false)], pages: vec![("Notes/a.md", "a", 1, "a1")] }, &[]);
    e.put("founder/Notes/raw/assets/payroll.pdf", "payroll");
    assert_eq!(e.make(&[])["withheld"], json!(["Notes/raw/assets/payroll.pdf"]), "no note names it, and a folder is out");
    assert!(e.read("brain/Notes/raw/assets/payroll.pdf").is_none());

    // a brain made from its stage, as its own is: the stage's note names its
    // old place, the work folder's names its asset, and that goes up
    let f = Dir::new("withheld-own");
    let stage = "---\nresource: /data/x.png\n---\nx";
    f.stage("stage", &Old { sequence: 10, folders: vec![("Notes", "n", true), ("Notes/Private", "np", false)], pages: vec![("Notes/x.md", "x", 1, stage)] }, &[]);
    f.put("brain/Notes/x.md", "---\nresource: raw/assets/x.png\n---\nx");
    f.put("brain/Notes/raw/assets/x.png", "x");
    let made = f.run(&["start", OLD, "stage.json", "brain", "stage"]).unwrap();
    assert_eq!((&made["withheld"], &made["recorded"]), (&json!([]), &json!(1)));
    assert_eq!(f.read("brain/Notes/raw/assets/x.png").as_deref(), Some("x"));
}

/// Goal (FIN-774): an attachment is withheld for good the moment any copy
/// shows a note of a folder the brain doesn't bring in naming it, whether
/// or not the file is there and whether or not that note comes. Method:
/// the founder's left-out note names a file it doesn't hold; teammate A,
/// who reads that folder, shows another; teammate B, who can't read it,
/// brings shared notes naming both. Invalid: what published both at B's
/// import, as neither had been recorded.
#[test]
fn an_attachment_is_withheld_on_sight() {
    let d = Dir::new("on-sight");
    let private = |asset: &str| format!("---\nresource: raw/assets/{asset}\n---\nprivate");
    let (missing, secret) = (private("missing.pdf"), private("secret.pdf"));
    let at10 = Old { sequence: 10, folders: vec![("Notes", "n", true), ("Notes/Private", "np", true)], pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/Private/p.md", "p", 1, &missing)] };
    assert_eq!(d.found(&at10, &[], &["Notes/Private"])["withheld"], json!(["Notes/raw/assets/missing.pdf"]), "the founder's, though the file isn't there");

    // A reads the left-out folder: its note there names another attachment
    let at20 = Old { sequence: 20, folders: vec![("Notes", "n", true), ("Notes/Private", "np", true)], pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/Private/q.md", "q", 1, &secret)] };
    d.copy("a20", &at20, &[]);
    d.put("a20/Notes/raw/assets/secret.pdf", "secret");
    let a = d.catch_up("a20");
    assert_eq!((&a["withheld"], &a["unshared"], &a["extra"]), (&json!(["Notes/raw/assets/secret.pdf"]), &json!(["Notes/Private"]), &json!([])));
    assert_eq!(d.record()["withheld"], json!(["Notes/raw/assets/missing.pdf", "Notes/raw/assets/secret.pdf"]), "A's, though its note never comes");

    // B can't read it: shared notes naming both, both files in B's copy
    let (b, m) = ("---\nresource: raw/assets/secret.pdf\n---\nb", "---\nresource: raw/assets/missing.pdf\n---\nm");
    let at30 = Old { sequence: 30, folders: vec![("Notes", "n", true), ("Notes/Private", "np", false)], pages: vec![("Notes/a.md", "a", 1, "a1"), ("Notes/b.md", "b", 1, b), ("Notes/m.md", "m", 1, m)] };
    d.copy("b30", &at30, &[]);
    d.put("b30/Notes/raw/assets/secret.pdf", "secret");
    d.put("b30/Notes/raw/assets/missing.pdf", "missing");
    let caught = d.catch_up("b30");
    assert_eq!((&caught["added"], &caught["withheld"]), (&json!(2), &json!(["Notes/raw/assets/missing.pdf", "Notes/raw/assets/secret.pdf"])));
    let brain = d.files("brain");
    assert!(brain.contains_key("Notes/b.md") && brain.contains_key("Notes/m.md") && !brain.keys().any(|p| p.contains(".pdf")), "{brain:?}");
}
