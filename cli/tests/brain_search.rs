//! Run the real CLI without credentials or a reachable host. Mutations happen
//! between independent processes: no watcher or cached import can help them.
use fragment_core::blob::sha256_hex;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Folder(PathBuf);
impl Folder {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("fragment-search-{tag}-{}", std::process::id()));
        fs::create_dir_all(p.join("wiki")).unwrap();
        fs::create_dir_all(p.join(".fragment")).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, body: &str) {
        fs::write(self.0.join(path), body).unwrap();
    }
    fn journal(&self, body: &str, conflicted: bool) {
        fs::create_dir_all(self.0.join(".fragment")).unwrap();
        self.write(".fragment/state.json", &json!({"schemaVersion":3,"name":"garden--test","host":"http://127.0.0.1:1","files":{"wiki/a.md":{"sha256":sha256_hex(body.as_bytes()),"size":body.len(),"mtime_ns":0,"commit":"base","conflicted":conflicted}}}).to_string());
    }
    fn find(&self, q: &str) -> Value {
        search(&self.0, q)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command(dir: &Path, q: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fragment"));
    c.args(["--json", "search", "garden--test", q, "--dir"])
        .arg(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("empty-config"))
        .env("FRAGMENT_HOST", "http://127.0.0.1:1")
        .env_remove("FRAGMENT_AS_AGENT")
        .env_remove("FRAGMENT_ACTING_FOR");
    c
}
fn search(dir: &Path, q: &str) -> Value {
    let out = command(dir, q).output().unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    v["data"].clone()
}

#[test]
fn current_edits_new_files_deletions_and_restarts_offline() {
    let f = Folder::new("current");
    let base = "# Root\n## Leaf\noldword\n";
    f.write("wiki/a.md", base);
    f.journal(base, false);
    let hit = &f.find("oldword")["results"][0];
    assert_eq!(hit["state"], "synced");
    assert_eq!(hit["fragment"], "garden--test");
    assert_eq!(hit["path"], "wiki/a.md");
    assert_eq!(hit["heading"], "Leaf");
    assert_eq!(hit["ancestry"], json!(["Root"]));
    // Same size; a stat cache must not hide this change.
    f.write("wiki/a.md", "# Root\n## Leaf\nnewword\n");
    assert!(f.find("oldword")["results"].as_array().unwrap().is_empty());
    let hit = &f.find("newword")["results"][0];
    assert_eq!(hit["state"], "local_only");
    assert!(hit["snippet"].as_str().unwrap().contains("newword"));
    f.write("wiki/new.md", "# Discovery\nlocalword");
    assert_eq!(f.find("localword")["results"][0]["state"], "local_only");
    fs::remove_file(f.0.join("wiki/a.md")).unwrap();
    assert!(f.find("newword")["results"].as_array().unwrap().is_empty());
    // Search makes no persistent index, journal changes, or synced files.
    assert_eq!(fs::read_dir(f.0.join(".fragment")).unwrap().count(), 1);
}

#[test]
fn conflicts_are_evidence_not_published_truth() {
    let f = Folder::new("conflict");
    let body = "# Local\nconflictword";
    f.write("wiki/a.md", body);
    f.journal(body, true); // copy could not be saved
    assert_eq!(f.find("conflictword")["results"][0]["state"], "conflicted");
    f.journal(body, false); // old journal, no conflict flag
    f.write(
        "wiki/a.conflict-1700000000-deadbeef.md",
        "# Remote\nremoteword",
    );
    assert_eq!(f.find("conflictword")["results"][0]["state"], "conflicted");
    assert!(f.find("remoteword")["results"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::remove_file(f.0.join("wiki/a.conflict-1700000000-deadbeef.md")).unwrap();
    f.write(
        "wiki/a.md",
        "<<<<<<< ours\nconflictword\n=======\ntheirs\n>>>>>>> remote",
    );
    assert_eq!(f.find("conflictword")["results"][0]["state"], "conflicted");
}

#[test]
fn ranked_prefix_words_limits_skips_and_invalid_input() {
    let f = Folder::new("ranking");
    f.write("wiki/a.md", "# Orchards\napple");
    f.write("wiki/b.md", "# Other\nOrchards apple");
    let answer = f.find("orch app");
    assert_eq!(answer["results"][0]["path"], "wiki/a.md");
    assert_eq!(answer["results"].as_array().unwrap().len(), 2);
    assert!(f.find("apple OR absent")["results"]
        .as_array()
        .unwrap()
        .is_empty());
    f.write("wiki/AGENTS.md", "instructionword");
    f.write("wiki/_index.md", "instructionword");
    f.write("wiki/.secret.md", "instructionword");
    assert!(f.find("instructionword")["results"]
        .as_array()
        .unwrap()
        .is_empty());
    f.write("wiki/large.md", &"x".repeat(1024 * 1024));
    fs::write(f.0.join("wiki/binary.md"), [255]).unwrap();
    assert_eq!(f.find("apple")["skipped"].as_array().unwrap().len(), 2);
    for q in ["***".to_string(), "x ".repeat(17), "x".repeat(257)] {
        let out = command(&f.0, &q).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["error"]["code"], "invalid_usage");
    }
    let out = command(&f.0, "apple")
        .args(["--limit", "1"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["results"].as_array().unwrap().len(), 1);
    f.write(".fragment/state.json", "broken"); // disposable journal: all local
    assert_eq!(f.find("apple")["results"][0]["state"], "local_only");
    f.journal("# Orchards\napple", false);
    let out = command(&f.0, "apple")
        .args(["--limit", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = Command::new(env!("CARGO_BIN_EXE_fragment"))
        .args(["search", "other--test", "apple", "--dir"])
        .arg(&f.0)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("garden--test"));
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_content() {
    let f = Folder::new("symlinks");
    let outside = Folder::new("outside");
    outside.write("wiki/a.md", "secretword");
    std::os::unix::fs::symlink(outside.0.join("wiki"), f.0.join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.0.join("wiki/a.md"), f.0.join("wiki/link.md")).unwrap();
    assert!(f.find("secretword")["results"]
        .as_array()
        .unwrap()
        .is_empty());
}
