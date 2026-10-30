//! The old Finite's export of an agent's chats (`export.py`'s `chats/`,
//! format `finite-chat-archive/1`) as a session archive's conversations.
//! Its records are Hermes' sessions as the export joined them, keyed by
//! the same ids, so the same chats imported from the export and from
//! Hermes' own database are the same conversations (`session::key`).
//!
//! Its layout: `chats/index.json` lists every conversation (`file`, a path
//! under the export's root); a chat's file is the conversation itself
//! (`chats/sessions/<id>.json`); a month's scheduled runs or side tasks
//! share one (`chats/runs/<kind>-<YYYY-MM>.json`, `{conversations: {<id>:
//! …}}`). A file is read only inside `chats/`, and only once.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_core::session::{Kind, Message, Session};
use serde::Deserialize;
use serde_json::Value;

use super::hermes::{Read, ORIGIN};

/// The export's chats' format.
pub const FORMAT: &str = "finite-chat-archive/1";
/// The largest of its files read (a month's runs share one).
const FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A conversation as the export wrote it (its index's record, and with
/// its messages in its file).
#[derive(Debug, Deserialize)]
struct Record {
    id: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    with: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    ended_at: Option<String>,
    /// The Hermes sessions it joins.
    #[serde(default)]
    sessions: Vec<String>,
    /// In a file, its messages (in the index, how many were said).
    #[serde(default)]
    messages: Value,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    format: String,
    conversations: Vec<Record>,
}

/// Where an export's chats are, from a path a person names: its `chats/`
/// (holding `index.json`), or the folder holding that. The export's root
/// (which every `file` is under) is the folder holding `chats/`.
pub fn root_of(path: &Path) -> Option<PathBuf> {
    let is_index = |p: &Path| p.join("index.json").is_file() && p.file_name().is_some_and(|n| n == "chats");
    if is_index(path) {
        return path.parent().map(Path::to_path_buf);
    }
    is_index(&path.join("chats")).then(|| path.to_path_buf())
}

/// The conversations of the export whose root is `root` (`root_of`).
pub fn read(root: &Path) -> Result<Read> {
    let index_path = root.join("chats/index.json");
    let index: IndexFile = serde_json::from_slice(&read_file(&index_path)?).with_context(|| format!("reading {}", index_path.display()))?;
    if index.format != FORMAT {
        bail!("{} is format {:?}, not the old Finite's export ({FORMAT})", index_path.display(), index.format);
    }
    let mut out = Read::default();
    let mut by_file: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for r in index.conversations {
        match r.file.clone().filter(|f| inside_chats(f)) {
            Some(f) => by_file.entry(f).or_default().push(r),
            None => out.problems.push(format!("conversation {}: its file is not one of the export's chats ({:?})", r.id, r.file)),
        }
    }
    for (file, records) in by_file {
        let bytes = match read_file(&root.join(&file)) {
            Ok(b) => b,
            Err(e) => {
                out.problems.push(format!("{file}: {e:#}"));
                continue;
            }
        };
        let v: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                out.problems.push(format!("{file}: {e}"));
                continue;
            }
        };
        for listed in records {
            // a chat's file is the conversation; a month's names each by id
            let held = if v.get("conversations").is_some() { v["conversations"].get(&listed.id).cloned() } else { Some(v.clone()).filter(|c| c["id"] == listed.id.as_str()) };
            let Some(held) = held else {
                out.problems.push(format!("conversation {}: not in {file}", listed.id));
                continue;
            };
            match serde_json::from_value::<Record>(held).map_err(|e| e.to_string()).and_then(|r| session(r, &listed)) {
                Ok(s) => out.sessions.push(s),
                Err(e) => out.problems.push(format!("conversation {} in {file}: {e}", listed.id)),
            }
        }
    }
    Ok(out)
}

/// A conversation from its file's record (`held`), its index's `listed`
/// filling in what the file left out.
fn session(held: Record, listed: &Record) -> Result<Session, String> {
    let kind_name = held.kind.as_deref().or(listed.kind.as_deref()).unwrap_or("chat");
    let kind = Kind::parse(kind_name).ok_or_else(|| format!("its kind {kind_name:?} is none of chat, cron, subagent"))?;
    let messages: Vec<Message> = match held.messages {
        Value::Array(_) => serde_json::from_value(held.messages).map_err(|e| format!("its messages: {e}"))?,
        _ => return Err("its file holds no messages".into()),
    };
    let or = |a: Option<String>, b: &Option<String>| a.or_else(|| b.clone());
    Ok(Session {
        format: String::new(),
        id: held.id,
        origin: ORIGIN.to_string(),
        kind,
        title: or(held.title, &listed.title).unwrap_or_default(),
        source: or(held.source, &listed.source),
        with: or(held.with, &listed.with),
        model: or(held.model, &listed.model),
        started_at: or(held.started_at, &listed.started_at),
        ended_at: or(held.ended_at, &listed.ended_at),
        parts: if held.sessions.is_empty() { listed.sessions.clone() } else { held.sessions },
        messages,
    })
}

/// Whether an index's `file` is one of the export's chats: relative,
/// under `chats/`, never out of it.
fn inside_chats(file: &str) -> bool {
    let p = Path::new(file);
    file.starts_with("chats/") && p.components().all(|c| matches!(c, Component::Normal(_)))
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    let size = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.len();
    if size > FILE_MAX_BYTES {
        bail!("{} is {} MiB, past the {} MiB a file of an export may be", path.display(), size / (1024 * 1024), FILE_MAX_BYTES / (1024 * 1024));
    }
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// An export's `chats/` under `root`, as export.py writes one: a chat
    /// of its own, and a month's scheduled runs together.
    pub fn export(root: &Path) {
        let chats = root.join("chats");
        std::fs::create_dir_all(chats.join("sessions")).unwrap();
        std::fs::create_dir_all(chats.join("runs")).unwrap();
        let chat = json!({ "id": "c1", "file": "chats/sessions/c1.json", "title": "Beans", "kind": "chat", "source": "finitechat", "with": "Ann",
            "model": "m", "started_at": "2026-10-01T09:00:00Z", "ended_at": "2026-10-01T10:00:00Z", "messages": 2, "preview": "plant", "sessions": ["c1", "c1b"] });
        let run = json!({ "id": "r1", "file": "chats/runs/cron-2026-10.json", "title": "Digest", "kind": "cron", "started_at": "2026-10-02T06:00:00Z", "messages": 1, "sessions": ["r1"] });
        std::fs::write(chats.join("index.json"), json!({ "format": FORMAT, "agent": "Juniper", "exported_at": "2026-10-29T18:30:00Z", "conversations": [run, chat] }).to_string()).unwrap();
        let mut chat_file = chat.clone();
        chat_file["format"] = json!(FORMAT);
        chat_file["messages"] = json!([
            { "role": "user", "at": "2026-10-01T09:00:00Z", "text": "plant beans" },
            { "role": "assistant", "at": "2026-10-01T09:00:05.123456Z", "tool_calls": [{ "name": "web", "arguments": "{}" }] },
            { "role": "tool", "at": "2026-10-01T09:00:06Z", "tool": "web", "text": "found" },
            { "role": "assistant", "at": "2026-10-01T09:00:07Z", "text": "in spring" },
        ]);
        std::fs::write(chats.join("sessions/c1.json"), chat_file.to_string()).unwrap();
        let mut run_file = run.clone();
        run_file["messages"] = json!([{ "role": "user", "at": "2026-10-02T06:00:00Z", "text": "\u{0}json:[{\"type\":\"text\",\"text\":\"digest\"}]" }]);
        std::fs::write(chats.join("runs/cron-2026-10.json"), json!({ "format": FORMAT, "conversations": { "r1": run_file } }).to_string()).unwrap();
    }

    /// Goal: an export's chats read as the conversations it holds, each
    /// as it was written. Method: a chat and a scheduled run, found from
    /// the export's root and from its `chats/`.
    #[test]
    fn an_export_reads_as_its_conversations() {
        let root = super::super::hermes::tests::tempdir();
        export(&root);
        assert_eq!(root_of(&root).as_deref(), Some(root.as_path()));
        assert_eq!(root_of(&root.join("chats")).as_deref(), Some(root.as_path()));
        assert_eq!(root_of(&root.join("chats/sessions")), None);
        let r = read(&root).unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let by_id: BTreeMap<&str, &Session> = r.sessions.iter().map(|s| (s.id.as_str(), s)).collect();
        let c = by_id["c1"];
        assert_eq!((c.kind, c.title.as_str(), c.with.as_deref(), c.parts.len(), c.messages.len()), (Kind::Chat, "Beans", Some("Ann"), 2, 4));
        assert_eq!(c.messages[1].tool_calls[0].name, "web");
        let run = by_id["r1"].clone().normalized();
        assert_eq!((run.kind, run.messages[0].text.as_deref()), (Kind::Cron, Some("digest")), "Hermes' structured text, which the export copied raw, reads as text");
    }

    /// Goal: an export is read only inside its `chats/`, and what it cannot
    /// read is a problem said, the rest read. Invalid: an index naming a
    /// file outside `chats/`, a file that is missing, another format.
    #[test]
    fn an_export_reads_nothing_outside_its_chats() {
        let root = super::super::hermes::tests::tempdir();
        export(&root);
        std::fs::write(root.join("secret.json"), "{}").unwrap();
        let index = root.join("chats/index.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
        v["conversations"].as_array_mut().unwrap().push(json!({ "id": "x", "file": "chats/../secret.json" }));
        v["conversations"].as_array_mut().unwrap().push(json!({ "id": "y", "file": "chats/sessions/missing.json" }));
        std::fs::write(&index, v.to_string()).unwrap();
        let r = read(&root).unwrap();
        assert_eq!(r.sessions.len(), 2);
        assert_eq!(r.problems.len(), 2, "{:?}", r.problems);
        assert!(r.problems[0].contains("not one of the export's chats") || r.problems[1].contains("not one of the export's chats"));
        v["format"] = json!("finite-chat-archive/2");
        std::fs::write(&index, v.to_string()).unwrap();
        assert!(format!("{:#}", read(&root).unwrap_err()).contains("not the old Finite's export"));
    }
}
