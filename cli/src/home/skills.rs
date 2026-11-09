//! An agent's skills, each with where it came from (FIN-463 §6): the
//! ones its person or the agent made come over; a platform's own (one
//! Hermes bundled, its hub's, a managed one) does not, since this
//! platform has its own, unless it was edited, when its changes are kept.
//! Decided by content, as the old export did (`export.py`
//! `skill_provenance`): a skill counts as a platform's only when it is
//! byte for byte the platform's copy.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use md5::{Digest as _, Md5};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;

use super::rules;
use super::source::Source;
use super::survey::{attention_lines, frontmatter, Line};

/// Lines naming the old platform one skill lists, at most.
const ATTENTION_PER_SKILL: usize = 100;
/// Skills one survey lists, at most.
const SKILLS_MAX: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    /// Made by its person, or by hand: comes over.
    Custom,
    /// Made by the agent (Hermes' usage record says so): comes over.
    AgentCreated,
    /// One of Hermes' bundled skills, edited.
    ModifiedBundled,
    /// A copy of one of its platform's managed skills, unchanged.
    Managed,
    /// One of its platform's managed skills, edited: its changes are kept.
    ModifiedManaged,
    /// Installed unchanged from Hermes' skills hub.
    Hub,
    /// From the hub, edited.
    ModifiedHub,
    /// From the hub, with nothing recorded to check it against.
    HubUnchecked,
}

impl Provenance {
    pub fn comes_over(self) -> bool {
        matches!(self, Provenance::Custom | Provenance::AgentCreated)
    }

    fn reason(self) -> Option<&'static str> {
        Some(match self {
            Provenance::Custom | Provenance::AgentCreated => return None,
            Provenance::ModifiedBundled => "one of Hermes' own skills, edited there: keep the changes, not the skill",
            Provenance::Managed => "an unchanged copy of one of its platform's skills: this platform has its own",
            Provenance::ModifiedManaged => "one of its platform's skills, edited there: keep the changes, not the skill",
            Provenance::Hub => "installed unchanged from Hermes' skills hub",
            Provenance::ModifiedHub => "from Hermes' skills hub, edited there: keep the changes, not the skill",
            Provenance::HubUnchecked => "from Hermes' skills hub, with nothing recorded to check it against: read it for changes of its own",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Skill {
    pub name: String,
    /// Its folder, relative to the Hermes home's `skills/`.
    pub path: String,
    pub provenance: Provenance,
    pub comes_over: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub files: usize,
    pub bytes: u64,
    /// For one edited from its platform's managed copy: the files that
    /// differ from it (added, removed or changed).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<Line>,
}

#[derive(Debug, Serialize)]
pub struct Skills {
    pub listed: Vec<Skill>,
    /// Hermes' bundled skills as shipped: not listed, since every Hermes
    /// has them.
    pub bundled_as_shipped: usize,
}

/// The folders under `root` holding a `SKILL.md`, not inside another
/// skill (whose references may hold one), with no hidden or dependency
/// folder on the way: each relative to `root`.
fn skill_dirs(src: &Source, root: &str) -> Vec<String> {
    let prefix = format!("{root}/");
    let mut found: Vec<String> = Vec::new();
    for e in src.under(root) {
        let rel = &e.path[prefix.len()..];
        let Some(dir) = rel.strip_suffix("/SKILL.md") else { continue };
        if dir.split('/').any(|c| c.starts_with('.') || rules::dependency_dir(c)) {
            continue;
        }
        found.push(dir.to_string());
    }
    found.sort();
    // a skill inside another is the other's
    let mut out: Vec<String> = Vec::new();
    for d in found {
        if !out.iter().any(|o| d.starts_with(&format!("{o}/"))) {
            out.push(d);
        }
    }
    out
}

/// A skill's files: (path relative to its folder, source path).
fn files_of<'a>(src: &'a Source, dir: &str, without_caches: bool) -> Vec<(String, &'a str)> {
    let prefix = format!("{dir}/");
    src.under(dir).map(|e| (e.path[prefix.len()..].to_string(), e.path.as_str())).filter(|(rel, _)| !(without_caches && rel.split('/').any(|c| c == "__pycache__"))).collect()
}

fn feed(src: &Source, path: &str, mut add: impl FnMut(&[u8])) -> Result<()> {
    src.with_reader(path, |r| {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            add(&buf[..n]);
        }
    })
}

/// Hermes' change hash of a skill (`tools/skills_sync.py` `_dir_hash`):
/// MD5 over each file's relative path and bytes, in path order (Python's,
/// folder by folder).
fn dir_hash(src: &Source, dir: &str, without_caches: bool) -> Result<String> {
    let mut files = files_of(src, dir, without_caches);
    files.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
    let mut h = Md5::new();
    for (rel, path) in files {
        h.update(rel.as_bytes());
        feed(src, path, |b| h.update(b))?;
    }
    Ok(hex::encode(h.finalize()))
}

/// The hash Hermes' hub records for a skill in its lock
/// (`tools/skills_guard.py` `_content_digest`): SHA-256 over each file's
/// relative path, a NUL and its bytes, in the order of the paths as text.
fn hub_hash(src: &Source, dir: &str, without_caches: bool) -> Result<String> {
    let mut files = files_of(src, dir, without_caches);
    files.sort();
    let mut h = Sha256::new();
    for (rel, path) in files {
        h.update(rel.as_bytes());
        h.update([0u8]);
        feed(src, path, |b| h.update(b))?;
    }
    Ok(hex::encode(h.finalize()))
}

/// Each file's SHA-256, by its path relative to `dir`.
fn file_hashes(src: &Source, dir: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (rel, path) in files_of(src, dir, true) {
        let mut h = Sha256::new();
        feed(src, path, |b| h.update(b))?;
        out.insert(rel, hex::encode(h.finalize()));
    }
    Ok(out)
}

/// The files of `edited` that differ from `original`'s: added, removed or changed.
fn changed(src: &Source, edited: &str, original: &str) -> Result<Vec<String>> {
    let (a, b) = (file_hashes(src, original)?, file_hashes(src, edited)?);
    let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    Ok(names.into_iter().filter(|n| a.get(*n) != b.get(*n)).cloned().collect())
}

fn json(src: &Source, path: &str) -> Value {
    src.get(path).and_then(|_| src.read(path).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

/// Every skill in the Hermes home's `skills/` but Hermes' bundled ones as
/// shipped, each with its provenance; `managed`, its platform's copies of
/// its managed skills (the layout's), if any.
pub fn skills(src: &Source, home_skills: &str, managed: Option<&str>, words: &[String]) -> Result<Skills> {
    let text = |p: &str| src.get(p).and_then(|_| src.text(p).ok().flatten()).unwrap_or_default();
    let bundled: BTreeMap<String, String> = text(&format!("{home_skills}/.bundled_manifest"))
        .lines()
        .filter_map(|l| l.split_once(':').map(|(n, h)| (n.trim().to_string(), h.trim().to_string())))
        .collect();
    let hub = json(src, &format!("{home_skills}/.hub/lock.json"));
    let made: BTreeSet<String> = match json(src, &format!("{home_skills}/.usage.json")) {
        Value::Object(m) => m.into_iter().filter(|(_, r)| r["created_by"] == "agent" || r["agent_created"] == Value::Bool(true)).map(|(n, _)| n).collect(),
        _ => BTreeSet::new(),
    };
    let managed_dirs: BTreeMap<String, String> = managed
        .map(|m| skill_dirs(src, m).into_iter().map(|d| (d.rsplit('/').next().unwrap_or(&d).to_string(), format!("{m}/{d}"))).collect())
        .unwrap_or_default();
    let mut listed = Vec::new();
    let mut shipped = 0;
    for rel in skill_dirs(src, home_skills).into_iter().take(SKILLS_MAX) {
        let dir = format!("{home_skills}/{rel}");
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let mut differs = Vec::new();
        let provenance = if let Some(recorded) = bundled.get(&name) {
            if *recorded == dir_hash(src, &dir, false)? || *recorded == dir_hash(src, &dir, true)? {
                shipped += 1;
                continue;
            }
            Provenance::ModifiedBundled
        } else if let Some(original) = managed_dirs.get(&name) {
            differs = changed(src, &dir, original)?;
            if differs.is_empty() {
                Provenance::Managed
            } else {
                Provenance::ModifiedManaged
            }
        } else if let Some(entry) = hub["installed"].get(&name).filter(|e| e.is_object()) {
            let recorded = entry["content_hash"].as_str().unwrap_or_default().trim_start_matches("sha256:").to_ascii_lowercase();
            if recorded.len() < 16 {
                Provenance::HubUnchecked
            } else if hub_hash(src, &dir, false)?.starts_with(&recorded) || hub_hash(src, &dir, true)?.starts_with(&recorded) {
                Provenance::Hub
            } else {
                Provenance::ModifiedHub
            }
        } else if made.contains(&name) {
            Provenance::AgentCreated
        } else {
            Provenance::Custom
        };
        let meta = frontmatter(&text(&format!("{dir}/SKILL.md")));
        let files: Vec<_> = src.under(&dir).collect();
        // what the import reads (its own, or an edit to keep) is what it adapts
        let read = !matches!(provenance, Provenance::Managed | Provenance::Hub);
        let mut attention = Vec::new();
        for e in files.iter().filter(|_| read) {
            if attention.len() >= ATTENTION_PER_SKILL {
                break;
            }
            if let Some(t) = (e.size <= super::survey::SCAN_MAX_BYTES).then(|| src.text(&e.path).ok().flatten()).flatten() {
                attention.extend(attention_lines(&e.path[dir.len() + 1..], &t, words));
            }
        }
        attention.truncate(ATTENTION_PER_SKILL);
        listed.push(Skill {
            name,
            path: rel,
            comes_over: provenance.comes_over(),
            reason: provenance.reason(),
            provenance,
            description: meta.get("description").cloned(),
            author: meta.get("author").cloned(),
            files: files.len(),
            bytes: files.iter().map(|e| e.size).sum(),
            changed: differs,
            attention,
        });
    }
    Ok(Skills { listed, bundled_as_shipped: shipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::source::tests::Temp;

    fn put(root: &std::path::Path, files: &[(&str, &str)]) {
        for (p, b) in files {
            std::fs::create_dir_all(root.join(p).parent().unwrap()).unwrap();
            std::fs::write(root.join(p), b).unwrap();
        }
    }

    /// The hashes as Hermes computes them: Python's own `_dir_hash` (MD5,
    /// in `pathlib`'s order, folder by folder) and the hub's
    /// `_content_digest` (SHA-256, in the paths' text order), run on these
    /// files with python3, are these.
    #[test]
    fn hashes_as_hermes_computes_them() {
        let t = Temp::new();
        put(&t.0, &[("skill/a/b", "one"), ("skill/a.b", "two"), ("skill/a-b", "three"), ("skill/SKILL.md", "---\nname: s\n---\n"), ("skill/__pycache__/x.pyc", "pyc"), ("skill/refs/Z.md", "four")]);
        let src = Source::open(&t.0).unwrap();
        assert_eq!(dir_hash(&src, "skill", false).unwrap(), "6da99e4082eaf7a8bc176b92694b23d0");
        assert_eq!(dir_hash(&src, "skill", true).unwrap(), "26ca2f102e7fa0c2a3256d7fd8ef65f6");
        assert_eq!(hub_hash(&src, "skill", false).unwrap(), "ceb637ef95c00793859883734ce39c67025b6e0b0a96f21f806c5cd5d593938e");
        assert_eq!(hub_hash(&src, "skill", true).unwrap(), "9b8f87a49a7f370b78d468d38d1984eded9f4d3201a2bcb9b36ea027247f0f2d");
    }

    #[test]
    fn each_skill_by_its_content() {
        let t = Temp::new();
        let skill = |n: &str| format!("---\nname: {n}\ndescription: does {n}\n---\nbody\n");
        let (shipped, edited, own, made, copy, changed_copy, hubbed, hub_edited) =
            (skill("arxiv"), skill("arxiv2"), skill("recipes"), skill("helper"), skill("apps"), skill("brains"), skill("weather"), skill("news"));
        put(&t.0, &[
            ("home/skills/research/arxiv/SKILL.md", &shipped),
            ("home/skills/research/arxiv2/SKILL.md", &edited),
            ("home/skills/recipes/SKILL.md", &own),
            ("home/skills/recipes/references/inner/SKILL.md", "a reference, not a skill"),
            ("home/skills/helper/SKILL.md", &made),
            ("home/skills/sw/apps/SKILL.md", &copy),
            ("home/skills/sw/brains/SKILL.md", &changed_copy),
            ("home/skills/sw/brains/extra.md", "mine: run fsite deploy"),
            ("home/skills/weather/SKILL.md", &hubbed),
            ("home/skills/news/SKILL.md", &hub_edited),
            ("home/skills/.usage.json", r#"{"helper": {"created_by": "agent"}}"#),
            ("home/skills/node_modules/x/SKILL.md", "a dependency's"),
            ("managed/sw/apps/SKILL.md", &copy),
            ("managed/sw/brains/SKILL.md", &changed_copy),
        ]);
        let src = Source::open(&t.0).unwrap();
        // what Hermes and its hub recorded, as they compute it
        let arxiv_hash = dir_hash(&src, "home/skills/research/arxiv", false).unwrap();
        let weather_hash = hub_hash(&src, "home/skills/weather", false).unwrap();
        put(&t.0, &[
            ("home/skills/.bundled_manifest", &format!("arxiv:{arxiv_hash}\narxiv2:0123456789abcdef0123456789abcdef\n")),
            ("home/skills/.hub/lock.json", &format!(r#"{{"installed": {{"weather": {{"content_hash": "sha256:{}"}}, "news": {{"content_hash": "sha256:{}"}}, "old": {{}}}}}}"#, &weather_hash[..20], "f".repeat(64))),
        ]);
        let src = Source::open(&t.0).unwrap();
        let s = skills(&src, "home/skills", Some("managed"), &["fsite".into()]).unwrap();
        let got: Vec<(&str, Provenance, bool)> = s.listed.iter().map(|k| (k.path.as_str(), k.provenance, k.comes_over)).collect();
        assert_eq!(got, vec![
            ("helper", Provenance::AgentCreated, true),
            ("news", Provenance::ModifiedHub, false),
            ("recipes", Provenance::Custom, true),
            ("research/arxiv2", Provenance::ModifiedBundled, false),
            ("sw/apps", Provenance::Managed, false),
            ("sw/brains", Provenance::ModifiedManaged, false),
            ("weather", Provenance::Hub, false),
        ]);
        assert_eq!(s.bundled_as_shipped, 1, "arxiv, as shipped, is not listed");
        let brains = &s.listed[5];
        assert_eq!(brains.changed, ["extra.md"]);
        assert_eq!(brains.attention.len(), 1);
        assert_eq!((brains.attention[0].file.as_str(), brains.attention[0].line), ("extra.md", 1));
        assert_eq!(s.listed[2].description.as_deref(), Some("does recipes"));
        assert_eq!(s.listed[2].files, 2, "its reference is its own file");
        assert!(s.listed[0].reason.is_none() && s.listed[1].reason.is_some());
    }
}
