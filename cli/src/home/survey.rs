//! `fragment home survey`: what an import of an agent's home would bring,
//! read-only, as one plan (FIN-463 §6): its chats, SOUL, memories and
//! settings, its skills with where each came from, its jobs as routines
//! in UTC, the parts its layout brings its own way, and its files with the
//! cap's pick and what stays behind. The import skill reads it and decides
//! the rest; nothing here changes anything, here or there.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use super::layout::Layout;
use super::plan::{self, Counted};
use super::routines::{self, Routines};
use super::skills::{self, Skills};
use super::source::Source;

pub const FORMAT: &str = "fragment-home-survey/1";
/// A text file read to look for the old platform's words, at most.
pub const SCAN_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// A settings file read, at most.
const CONFIG_MAX_BYTES: u64 = 256 * 1024;
/// A line said, at most, in characters.
const LINE_SHOWN_MAX_CHARS: usize = 200;
/// The lines of one text listed, at most.
const LINES_MAX: usize = 100;
/// The files left out for their names the survey names, at most.
const SKIPPED_SHOWN: usize = 1_000;

/// A line of a text that names the old platform: its file, its number,
/// the words it holds, and the line.
#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub file: String,
    pub line: usize,
    pub words: Vec<String>,
    pub text: String,
}

/// The lines of `text` holding any of `words`, case aside.
pub fn attention_lines(file: &str, text: &str, words: &[String]) -> Vec<Line> {
    if words.is_empty() {
        return Vec::new();
    }
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.to_lowercase();
        let found: Vec<String> = words.iter().zip(&lower).filter(|(_, w)| l.contains(w.as_str())).map(|(w, _)| w.clone()).collect();
        if !found.is_empty() {
            out.push(Line { file: file.to_string(), line: n + 1, words: found, text: line.trim().chars().take(LINE_SHOWN_MAX_CHARS).collect() });
            if out.len() >= LINES_MAX {
                break;
            }
        }
    }
    out
}

/// The `key: value` lines of a leading `---` block.
pub fn frontmatter(text: &str) -> BTreeMap<String, String> {
    let Some(rest) = text.strip_prefix("---") else { return BTreeMap::new() };
    let Some(end) = rest.find("\n---") else { return BTreeMap::new() };
    rest[..end]
        .lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Survey {
    pub format: &'static str,
    pub layout: String,
    pub source: SourceFacts,
    /// Hermes' session database: what `fragment session import` reads.
    pub sessions: Option<FileFact>,
    pub soul: Option<FileFact>,
    pub memories: Vec<Memory>,
    /// Hermes' memory store, beside its memory files.
    pub memory_store: Option<FileFact>,
    pub config: Option<Config>,
    pub skills: Skills,
    /// None: the home kept no job list.
    pub routines: Option<Routines>,
    /// Each part the layout names: its folders.
    pub parts: BTreeMap<String, Vec<Part>>,
    pub files: Counted,
}

#[derive(Debug, Serialize)]
pub struct SourceFacts {
    pub kind: &'static str,
    pub files: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
    /// The files left out unread, their names saying they may hold a
    /// password or key, at most `SKIPPED_SHOWN` (`skipped_count` is all).
    pub skipped: Vec<String>,
    pub skipped_count: usize,
}

#[derive(Debug, Serialize)]
pub struct FileFact {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct Memory {
    pub path: String,
    pub bytes: u64,
    /// Its entries (Hermes separates them by a line holding only `§`).
    pub entries: usize,
}

/// The few public settings an import uses, read by name: the rest of the
/// file (keys, tokens, servers) is never read into the plan.
#[derive(Debug, Default, Serialize)]
pub struct Config {
    pub path: String,
    pub timezone: Option<String>,
    pub personality: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Part {
    /// The folder's name under the part (`.` for files directly in it).
    pub name: String,
    pub files: usize,
    pub bytes: u64,
}

fn fact(src: &Source, path: &str) -> Option<FileFact> {
    src.get(path).map(|e| FileFact { path: e.path.clone(), bytes: e.size })
}

/// A setting's value as the settings file writes one, when it is a name
/// (`Europe/Berlin`, `openrouter/model-x`, `warm`), or `None`: anything
/// else (a flow mapping or list, words, a URL) is not read, so no more of
/// the file than a name ever reaches the plan.
fn scalar(v: &str) -> Option<String> {
    let v = v.split(" #").next().unwrap_or_default().trim();
    let v = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|r| r.strip_suffix('\''))).unwrap_or(v);
    let name = !v.is_empty() && v.len() <= 128 && !v.contains("://") && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/:@+-".contains(&b));
    (name && !["null", "~"].contains(&v)).then(|| v.to_string())
}

/// `timezone`, `agent.personality` (or `personality`) and `model.default`
/// (or a scalar `model`), and nothing else, from Hermes' `config.yaml`: a
/// key at the top, or one of its section's children (at the indent of its
/// first child, spaces or tabs), each a name (`scalar`).
fn config(path: &str, text: &str) -> Config {
    let mut c = Config { path: path.to_string(), ..Config::default() };
    let (mut section, mut child_indent): (String, Option<String>) = (String::new(), None);
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent: String = line.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let (key, value) = (key.trim(), scalar(value));
        if indent.is_empty() {
            (section, child_indent) = (key.to_string(), None);
            match key {
                "timezone" => c.timezone = value.or(c.timezone.take()),
                "personality" => c.personality = value.or(c.personality.take()),
                "model" => c.model = value.or(c.model.take()),
                _ => {}
            }
            continue;
        }
        // a section's children are at its first child's indent; deeper is nested
        if *child_indent.get_or_insert_with(|| indent.clone()) != indent {
            continue;
        }
        match (section.as_str(), key) {
            ("agent", "personality") => c.personality = value.or(c.personality.take()),
            ("model", "default") => c.model = value.or(c.model.take()),
            _ => {}
        }
    }
    c
}

fn memories(src: &Source, layout: &Layout) -> Vec<Memory> {
    let dir = layout.in_home("memories");
    let prefix = format!("{dir}/");
    src.under(&dir)
        .filter(|e| {
            let name = &e.path[prefix.len()..];
            !name.contains('/') && !name.starts_with('.') && !name.ends_with(".lock")
        })
        .map(|e| {
            let text = (e.size <= SCAN_MAX_BYTES).then(|| src.text(&e.path).ok().flatten()).flatten().unwrap_or_default();
            let entries = text.split('\n').collect::<Vec<_>>().split(|l| l.trim() == "§").filter(|chunk| chunk.iter().any(|l| !l.trim().is_empty())).count();
            Memory { path: e.path.clone(), bytes: e.size, entries }
        })
        .collect()
}

fn parts(src: &Source, layout: &Layout) -> BTreeMap<String, Vec<Part>> {
    let mut out = BTreeMap::new();
    for (name, folder) in &layout.parts {
        let mut by: BTreeMap<String, Part> = BTreeMap::new();
        let skip = if folder == "." { 0 } else { folder.len() + 1 };
        for e in src.under(folder) {
            let rel = &e.path[skip..];
            let sub = rel.split_once('/').map_or(".", |(d, _)| d).to_string();
            let p = by.entry(sub.clone()).or_insert(Part { name: sub, files: 0, bytes: 0 });
            (p.files, p.bytes) = (p.files + 1, p.bytes + e.size);
        }
        out.insert(name.clone(), by.into_values().collect());
    }
    out
}

/// The plan, at `now_s` (what "once, later" and a zone's offset are read
/// against).
pub fn survey(src: &Source, layout: &Layout, now_s: i64) -> Result<Survey> {
    let config_path = layout.config_path();
    let config = src.get(&config_path).filter(|e| e.size <= CONFIG_MAX_BYTES).and_then(|_| src.text(&config_path).ok().flatten()).map(|t| config(&config_path, &t));
    let jobs_path = layout.in_home("cron/jobs.json");
    let routines = src.get(&jobs_path).map(|_| -> Result<Routines> {
        let jobs: serde_json::Value = serde_json::from_slice(&src.read(&jobs_path)?).unwrap_or(serde_json::Value::Null);
        Ok(routines::routines(&routines::jobs_of(&jobs), config.as_ref().and_then(|c| c.timezone.as_deref()), now_s, &layout.attention))
    });
    let plan = plan::plan(src, layout, plan::FILES_MAX_BYTES, plan::FILES_MAX)?;
    Ok(Survey {
        format: FORMAT,
        layout: layout.name.clone(),
        source: SourceFacts {
            kind: src.kind(),
            files: src.entries().len(),
            bytes: src.entries().iter().map(|e| e.size).sum(),
            unread: src.unread.clone(),
            skipped: src.skipped.iter().take(SKIPPED_SHOWN).map(|e| e.path.clone()).collect(),
            skipped_count: src.skipped.len(),
        },
        sessions: fact(src, &layout.in_home("state.db")),
        soul: fact(src, &layout.in_home("SOUL.md")),
        memories: memories(src, layout),
        memory_store: fact(src, &layout.in_home("memory_store.db")),
        config,
        skills: skills::skills(src, &layout.in_home("skills"), layout.managed_skills.as_deref(), &layout.attention)?,
        routines: routines.transpose()?,
        parts: parts(src, layout),
        files: plan.counted(plan::FILES_MAX_BYTES, plan::FILES_MAX),
    })
}

/// Bytes as a person reads them.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// A survey as a person reads it.
pub fn print(s: &Survey) {
    println!("A home ({}, {} files, {}), read as layout {}", s.source.kind, s.source.files, size(s.source.bytes), s.layout);
    match &s.sessions {
        Some(f) => println!("Chats: {} ({}); `fragment session import` brings them", f.path, size(f.bytes)),
        None => println!("Chats: none (no state.db)"),
    }
    println!("SOUL.md: {}; memories: {} files, {} entries", if s.soul.is_some() { "yes" } else { "none" }, s.memories.len(), s.memories.iter().map(|m| m.entries).sum::<usize>());
    if let Some(c) = &s.config {
        println!("Settings: time zone {}, model {}", c.timezone.as_deref().unwrap_or("UTC"), c.model.as_deref().unwrap_or("not named"));
    }
    let own = s.skills.listed.iter().filter(|k| k.comes_over).count();
    let edited = s.skills.listed.iter().filter(|k| !k.comes_over && !k.changed.is_empty() || matches!(k.provenance, skills::Provenance::ModifiedBundled | skills::Provenance::ModifiedHub | skills::Provenance::HubUnchecked)).count();
    println!("Skills: {own} come over, {edited} with changes to keep, {} replaced by this platform's", s.skills.listed.len() - own - edited + s.skills.bundled_as_shipped);
    if let Some(r) = &s.routines {
        let dst = if r.dst == Some(true) { "; its zone moves its clocks" } else { "" };
        println!("Routines: {} of {} jobs (in UTC, from {}{dst})", r.routines, r.jobs.len(), r.zone);
    }
    for (name, list) in &s.parts {
        println!("{name}: {}", if list.is_empty() { "none".to_string() } else { list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ") });
    }
    if s.source.skipped_count > 0 {
        println!("Left out unread, their names saying they may hold a password or key: {} ({})", s.source.skipped_count, s.source.skipped.iter().take(5).cloned().collect::<Vec<_>>().join(", "));
    }
    let f = &s.files;
    println!("Files: {} brought ({}), {} left behind ({})", f.brought.files, size(f.brought.bytes), f.left.files, size(f.left.bytes));
    for (why, r) in &f.left_by_why {
        println!("  {} file{} ({}): {}", r.files, if r.files == 1 { "" } else { "s" }, size(r.bytes), why.said());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_by_name_only() {
        let yaml = "timezone: Europe/Berlin\nmodel:\n  default: some/model # the one\n  api_key: sk-secret-value-123456789\nagent:\n  personality: \"kind\"\n  nested:\n    personality: no\ngateway:\n  platforms:\n    telegram:\n      token: 123456:abc\n";
        let c = config("config.yaml", yaml);
        assert_eq!((c.timezone.as_deref(), c.model.as_deref(), c.personality.as_deref()), (Some("Europe/Berlin"), Some("some/model"), Some("kind")));
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("secret") && !json.contains("123456"), "nothing else is read: {json}");
        assert_eq!(config("c", "model: plain/model\n").model.as_deref(), Some("plain/model"));
        // a flow mapping or sequence is no scalar: nothing of it is read
        let flow = config("c", "model: {default: example/model, api_key: SYNTHETIC_MUST_NOT_EXPORT}\ntimezone: [a, b]\nagent: {personality: kind, token: SYNTHETIC_TOO}\n");
        let json = serde_json::to_string(&flow).unwrap();
        assert!(!json.contains("SYNTHETIC") && flow.model.is_none() && flow.timezone.is_none() && flow.personality.is_none(), "{json}");
        // a child indented by four spaces, or a tab, is its section's
        let deep = config("c", "agent:\n    personality: warm\nmodel:\n\tdefault: tabbed/model\n");
        assert_eq!((deep.personality.as_deref(), deep.model.as_deref()), (Some("warm"), Some("tabbed/model")));
        // a value that is no name (words, a URL with a password) is not read
        assert!(config("c", "agent:\n  personality: you are a helpful bot\n").personality.is_none());
        assert!(config("c", "model: https://user:pw@example.org/m\n").model.is_none());
    }

    #[test]
    fn lines_naming_the_old_platform() {
        let lines = attention_lines("SKILL.md", "Use FSITE to deploy.\nfine\nsee /data/workspace and fsite", &["fsite".into(), "/data/workspace".into()]);
        let got: Vec<(usize, Vec<String>)> = lines.iter().map(|l| (l.line, l.words.clone())).collect();
        assert_eq!(got, [(1, vec!["fsite".to_string()]), (3, vec!["fsite".to_string(), "/data/workspace".to_string()])]);
        assert!(attention_lines("x", "fsite", &[]).is_empty());
        assert_eq!(frontmatter("---\nname: a\ndescription: \"b: c\"\n  nested: x\n---\nbody").get("description").map(String::as_str), Some("b: c"));
    }
}
