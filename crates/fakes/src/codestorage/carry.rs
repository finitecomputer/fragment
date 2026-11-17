//! The fake's saved state (`Options::state_file`, a dev stack's
//! `target/devstack/codestorage.json`), carried into macrofiche: each repo
//! as `macrofiche import` takes one (macrofiche's docs/design.md, Import),
//! a fast-import stream of its whole history, with the url form and id the
//! cell holds it by, and each commit at `refs/macrofiche/legacy/<its fake
//! id>` so a cell's pins (fake ids, not git's) still name it there.
//!
//! What the fake keeps of a commit is all a stream says: its parents (a
//! merge's two), its whole tree, its message, its author's name (the fake
//! keeps no email: `<name>@users.fragment`, as the CLI's are) and its time.
//! Every file is `100644`, as the fake lists them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};

/// The ref each carried commit is also at: `refs/macrofiche/legacy/<id>`.
pub const LEGACY_PREFIX: &str = "refs/macrofiche/legacy/";

/// One repo of the fake's state, read.
pub struct StateRepo {
    pub name: String,
    pub url: String,
    pub repo_id: String,
    pub created_at_ms: i64,
    /// Branch to its head (a fake id).
    pub branches: BTreeMap<String, String>,
    /// Each commit's tree: path to its blob's git id and size.
    pub trees: BTreeMap<String, BTreeMap<String, (String, u64)>>,
    /// Each blob's bytes, by its git id.
    pub blobs: BTreeMap<String, Vec<u8>>,
    /// The commits, parents first.
    order: Vec<String>,
    commits: BTreeMap<String, super::Commit>,
}

/// One repo, as `macrofiche import` takes it.
pub struct Carried {
    pub name: String,
    pub url: String,
    pub repo_id: String,
    pub created_at_ms: i64,
    pub commits: usize,
    /// A fast-import stream ending in `done`.
    pub stream: Vec<u8>,
}

/// Every repo the fake's state at `state_file` holds (the deleted ones are
/// gone from it: a wipe's, whose names no one makes again).
pub fn read_state(state_file: &Path) -> Result<Vec<StateRepo>> {
    let st: super::State = serde_json::from_slice(&std::fs::read(state_file).with_context(|| format!("read {}", state_file.display()))?)
        .with_context(|| format!("{} is no code.storage fake's state", state_file.display()))?;
    st.repos.into_values().map(state_repo).collect()
}

fn state_repo(r: super::Repo) -> Result<StateRepo> {
    let order = parents_first(&r.url, &r.commits)?;
    for (b, head) in &r.branches {
        if !r.commits.contains_key(head) {
            bail!("{}'s {b} is at {head}, a commit it does not hold", r.url);
        }
    }
    let trees = r.commits.iter().map(|(id, c)| (id.clone(), c.tree.iter().map(|(p, e)| (p.clone(), (e.blob.clone(), e.size))).collect())).collect();
    Ok(StateRepo { name: r.name, url: r.url, repo_id: r.repo_id, created_at_ms: r.created_at_ms, branches: r.branches, trees, blobs: r.blobs, order, commits: r.commits })
}

/// The commits of a repo, each after its parents (Kahn's), the oldest of
/// those ready first; refused when a parent is missing, or a cycle.
fn parents_first(url: &str, commits: &BTreeMap<String, super::Commit>) -> Result<Vec<String>> {
    let mut waiting: BTreeMap<&str, usize> = BTreeMap::new();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, c) in commits {
        for p in &c.parents {
            if !commits.contains_key(p) {
                bail!("{url}'s commit {id} names a parent it does not hold, {p}");
            }
            children.entry(p.as_str()).or_default().push(id.as_str());
        }
        waiting.insert(id.as_str(), c.parents.len());
    }
    let mut ready: BTreeSet<(i64, &str)> = waiting.iter().filter(|(_, n)| **n == 0).map(|(id, _)| (commits[*id].at_ms, *id)).collect();
    let mut order = Vec::with_capacity(commits.len());
    // bounded by the commits: each is taken once
    while let Some(next) = ready.pop_first() {
        order.push(next.1.to_string());
        for child in children.get(next.1).into_iter().flatten() {
            let n = waiting.get_mut(child).expect("a child is a commit");
            *n -= 1;
            if *n == 0 {
                ready.insert((commits[*child].at_ms, child));
            }
        }
    }
    if order.len() != commits.len() {
        bail!("{url}'s commits name each other as parents in a cycle");
    }
    Ok(order)
}

/// A path as fast-import takes one: C-quoted, so any byte a repo path
/// holds (a space, a quote) is the path's.
fn quoted(path: &str) -> String {
    let mut q = String::with_capacity(path.len() + 2);
    q.push('"');
    for c in path.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// A fake author's name as a commit's person: no `<`, `>` or newline.
fn person(name: &str) -> (String, String) {
    let clean: String = name.chars().map(|c| if matches!(c, '<' | '>' | '\n' | '\r') { ' ' } else { c }).collect();
    let clean = clean.trim();
    let clean = if clean.is_empty() { "unknown" } else { clean };
    let local: String = clean.rsplit('/').next().unwrap_or(clean).chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
    let local = if local.is_empty() { "unknown".to_string() } else { local };
    (clean.to_string(), format!("{local}@users.fragment"))
}

impl StateRepo {
    /// The repo as `macrofiche import` takes it: each blob once, then each
    /// commit (parents first) on its legacy id's ref with its whole tree,
    /// then each branch, then `done`.
    pub fn carried(&self) -> Carried {
        let mut s: Vec<u8> = Vec::new();
        let mut marks: BTreeMap<&str, usize> = BTreeMap::new();
        for (id, bytes) in &self.blobs {
            let mark = marks.len() + 1;
            s.extend(format!("blob\nmark :{mark}\ndata {}\n", bytes.len()).as_bytes());
            s.extend(bytes);
            s.push(b'\n');
            marks.insert(id.as_str(), mark);
        }
        let mut commit_marks: BTreeMap<&str, usize> = BTreeMap::new();
        for id in &self.order {
            let c = &self.commits[id];
            let mark = marks.len() + commit_marks.len() + 1;
            let (name, email) = person(&c.author);
            let at = c.at_ms.div_euclid(1000);
            s.extend(format!("commit {LEGACY_PREFIX}{id}\nmark :{mark}\nauthor {name} <{email}> {at} +0000\ncommitter {name} <{email}> {at} +0000\ndata {}\n{}\n", c.message.len(), c.message).as_bytes());
            for (i, p) in c.parents.iter().enumerate() {
                let verb = if i == 0 { "from" } else { "merge" };
                s.extend(format!("{verb} :{}\n", commit_marks[p.as_str()]).as_bytes());
            }
            s.extend(b"deleteall\n");
            for (path, e) in &c.tree {
                let blob = marks.get(e.blob.as_str()).unwrap_or_else(|| panic!("{}'s commit {id} names blob {}, which the fake keeps", self.url, e.blob));
                s.extend(format!("M 100644 :{blob} {}\n", quoted(path)).as_bytes());
            }
            s.push(b'\n');
            commit_marks.insert(id.as_str(), mark);
        }
        for (branch, head) in &self.branches {
            s.extend(format!("reset refs/heads/{branch}\nfrom :{}\n\n", commit_marks[head.as_str()]).as_bytes());
        }
        s.extend(b"done\n");
        Carried { name: self.name.clone(), url: self.url.clone(), repo_id: self.repo_id.clone(), created_at_ms: self.created_at_ms, commits: self.order.len(), stream: s }
    }

    /// Whether every blob a tree names is kept (a carry checks it first).
    pub fn whole(&self) -> Result<()> {
        for (id, tree) in &self.trees {
            for (path, (blob, _)) in tree {
                if !self.blobs.contains_key(blob) {
                    bail!("{}'s commit {id} names {path}'s blob {blob}, which the fake does not keep", self.url);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state of one repo: two commits on main, live at the first.
    fn state() -> super::super::State {
        let cs = super::super::CodeStorage::start(super::super::Options::default()).unwrap();
        cs.seed_repo("todo--abc", &[("index.html", b"<p>one</p>"), ("a dir/q\"uote.txt", b"q")]);
        let one = cs.branch("todo--abc", "main").unwrap();
        cs.set_branch("todo--abc", "live", &one);
        cs.external_commit("todo--abc", "main", &[("index.html", Some(b"<p>two</p>")), ("a dir/q\"uote.txt", None)], "two");
        let st = cs.with(|st| serde_json::to_vec(st).unwrap());
        serde_json::from_slice(&st).unwrap()
    }

    // Goal: a merge commit keeps both its parents, its first as `from`
    // and its second as `merge`, each written before it. Method: a merge
    // of live's tip and main's added to the state (the fake makes one as a
    // deploy's merge does), then its stream read back.
    #[test]
    fn a_merge_keeps_both_parents() {
        let mut st = state();
        let repo = st.repos.values_mut().next().unwrap();
        let (main, live) = (repo.branches["main"].clone(), repo.branches["live"].clone());
        let mut merged = repo.commits[&main].clone();
        merged.parents = vec![live.clone(), main.clone()];
        merged.message = "merge".into();
        merged.at_ms += 1000;
        let id = "c".repeat(40);
        repo.commits.insert(id.clone(), merged);
        repo.branches.insert("live".into(), id.clone());
        let r = state_repo(st.repos.into_values().next().unwrap()).unwrap();
        assert_eq!(r.order.last(), Some(&id), "after both its parents");
        let text = String::from_utf8(r.carried().stream).unwrap();
        let block = &text[text.find(&format!("commit {LEGACY_PREFIX}{id}\n")).unwrap()..];
        let block = &block[..block.find("\n\n").unwrap()];
        let mark = |c: &str| text[text.find(&format!("commit {LEGACY_PREFIX}{c}\n")).unwrap()..].lines().nth(1).unwrap().trim_start_matches("mark ").to_string();
        assert!(block.contains(&format!("from {}\nmerge {}\n", mark(&live), mark(&main))), "{block}");
    }

    // Goal: a carried repo's stream holds every blob once, every commit
    // after its parents on its legacy id's ref with its whole tree (a
    // deleted file gone, a quoted path whole), and every branch, ending in
    // `done`. Method: a seeded repo's stream read back.
    #[test]
    fn a_stream_carries_every_commit_after_its_parents() {
        let st = state();
        let r = state_repo(st.repos.into_values().next().unwrap()).unwrap();
        r.whole().unwrap();
        let c = r.carried();
        let text = String::from_utf8(c.stream.clone()).unwrap();
        assert_eq!((c.commits, text.matches("\nblob\n").count() + usize::from(text.starts_with("blob\n"))), (2, 3), "{text}");
        let (first, second) = (&r.order[0], &r.order[1]);
        let at_first = text.find(&format!("commit {LEGACY_PREFIX}{first}\n")).unwrap();
        let at_second = text.find(&format!("commit {LEGACY_PREFIX}{second}\n")).unwrap();
        assert!(at_first < at_second, "parents first");
        let second_block = &text[at_second..];
        assert!(second_block.contains("from :4\ndeleteall\nM 100644 :") && second_block.contains(" \"index.html\"\n"), "{second_block}");
        assert!(!second_block[..second_block.find("\n\n").unwrap()].contains("uote"), "the deleted file is gone from its tree");
        assert!(text[at_first..at_second].contains("\"a dir/q\\\"uote.txt\""), "a path quoted whole");
        assert!(text.contains(&format!("reset refs/heads/live\nfrom :{}\n", 4)) && text.contains(&format!("reset refs/heads/main\nfrom :{}\n", 5)));
        assert!(text.ends_with("done\n"));
    }

    // Goal: a state whose history is not whole is refused before anything
    // is carried. Method: a parent missing, a branch at a commit not held.
    #[test]
    fn a_broken_history_is_refused() {
        let mut st = state();
        let repo = st.repos.values_mut().next().unwrap();
        repo.branches.insert("main".into(), "f".repeat(40));
        assert!(state_repo(st.repos.into_values().next().unwrap()).is_err());
        let mut st = state();
        let repo = st.repos.values_mut().next().unwrap();
        let first = repo.commits.keys().next().cloned().unwrap();
        repo.commits.get_mut(&first).unwrap().parents.push("e".repeat(40));
        assert!(state_repo(st.repos.into_values().next().unwrap()).is_err());
    }

    // Goal: a fake author is a person git takes. Method: odd names.
    #[test]
    fn an_author_is_a_person() {
        assert_eq!(person("fragment/1a2b3c4d"), ("fragment/1a2b3c4d".into(), "1a2b3c4d@users.fragment".into()));
        assert_eq!(person("a <b>\nc"), ("a  b  c".into(), "abc@users.fragment".into()));
        assert_eq!(person("  "), ("unknown".into(), "unknown@users.fragment".into()));
    }
}
