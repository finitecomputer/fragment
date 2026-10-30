//! The old Finite's export of an agent's chats (`export.py`'s `chats/`,
//! format `finite-chat-archive/1`) as a session archive's conversations.
//! Its records are Hermes' sessions as the export joined them, keyed by
//! their Hermes ids, so the same chats from another of Hermes' stores are
//! the same conversations (`session::key`).
//!
//! Its layout: `chats/index.json` lists every conversation (`file`, a path
//! under the export's root); a chat's file is the conversation itself
//! (`chats/sessions/<id>.json`); a month's scheduled runs or side tasks
//! share one (`chats/runs/<kind>-<YYYY-MM>.json`, `{conversations: {<id>:
//! …}}`). A message's files are named by its `attachments` (`{name,
//! media_type, size, sha256, path}`, `path` under `chats/`), their bytes
//! checked against their hash. Only regular files inside `chats/` are read
//! (no link out of it, no device), each once and bounded, and each
//! conversation goes to the spool as it is read.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_core::session::{self, Kind, Message, Session};
use serde::Deserialize;
use serde_json::Value;

use super::{said_id, Spool};

/// The export's chats' format.
pub const FORMAT: &str = "finite-chat-archive/1";
/// The origin its conversations are keyed under: Hermes', whose ids they are.
pub const ORIGIN: &str = "hermes";
/// Which read this is (`Session::reader`).
pub const READER: u32 = 10;
/// The largest of its files read (a month's runs share one; a chat's is
/// held to `SESSION_MAX_BYTES` once filed, and its record's text is said
/// whole by the export, so a little over).
const FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;

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
    /// The agent it exported.
    #[serde(default)]
    agent: Option<String>,
    conversations: Vec<Record>,
}

/// Where an export's chats are, from a path a person names: its `chats/`
/// (holding `index.json`), or the folder holding that. The export's root
/// (which every `file` is under) is the folder holding `chats/`.
pub fn root_of(path: &Path) -> Option<PathBuf> {
    let is_index = |p: &Path| regular(&p.join("index.json")) && p.file_name().is_some_and(|n| n == "chats");
    if is_index(path) {
        return path.parent().map(Path::to_path_buf);
    }
    is_index(&path.join("chats")).then(|| path.to_path_buf())
}

/// Whether `path` is a regular file itself, never through a link.
fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// The conversations of the export whose root is `root` (`root_of`), each
/// into `spool` as it is read. What it could not read, each in a sentence
/// (the rest is read); an import past an archive's limits stops, refused.
pub fn read(root: &Path, spool: &mut Spool) -> Result<Vec<String>> {
    let index_path = root.join("chats/index.json");
    let index: IndexFile = serde_json::from_slice(&read_file(root, "chats/index.json", FILE_MAX_BYTES)?).with_context(|| format!("reading {}", index_path.display()))?;
    if index.format != FORMAT {
        bail!("{} is format {:?}, not the old Finite's export ({FORMAT})", index_path.display(), index.format);
    }
    let mut problems = Vec::new();
    let mut by_file: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for r in index.conversations {
        match r.file.clone().filter(|f| inside_chats(f)) {
            Some(f) => by_file.entry(f).or_default().push(r),
            None => problems.push(format!("conversation {}: its file is not one of the export's chats", said_id(&r.id))),
        }
    }
    for (file, records) in by_file {
        let v: Value = match read_file(root, &file, FILE_MAX_BYTES).and_then(|b| serde_json::from_slice(&b).with_context(|| file.clone())) {
            Ok(v) => v,
            Err(e) => {
                problems.push(format!("{file}: {e:#}"));
                continue;
            }
        };
        for listed in records {
            // a chat's file is the conversation; a month's names each by id
            let held = if v.get("conversations").is_some() { v["conversations"].get(&listed.id).cloned() } else { Some(v.clone()).filter(|c| c["id"] == listed.id.as_str()) };
            let Some(held) = held else {
                problems.push(format!("conversation {}: not in {file}", said_id(&listed.id)));
                continue;
            };
            match serde_json::from_value::<Record>(held).map_err(|e| e.to_string()).and_then(|r| session(r, &listed, index.agent.as_deref())) {
                Ok((mut s, files)) => {
                    media(root, &mut s, files, spool, &mut problems)?;
                    spool.add(s)?;
                }
                Err(e) => problems.push(format!("conversation {} in {file}: {e}", said_id(&listed.id))),
            }
        }
    }
    Ok(problems)
}

/// A file a message names: its message's place, its attachment's place,
/// and its path under `chats/`.
type Named = (usize, usize, String);

/// The files its messages hold, into `spool`: each read from `chats/` and
/// kept when its bytes are what its attachment names; one that is not (or
/// is missing, or too large) is left out of its message, said.
fn media(root: &Path, s: &mut Session, files: Vec<Named>, spool: &mut Spool, problems: &mut Vec<String>) -> Result<()> {
    let mut lost: Vec<(usize, usize)> = Vec::new();
    for (i, j, path) in files {
        let a = &s.messages[i].attachments[j];
        let kept = match Some(format!("chats/{path}")).filter(|p| inside_chats(p)).map(|p| read_file(root, &p, session::MEDIA_FILE_MAX_BYTES)) {
            Some(Ok(bytes)) => spool.add_media(a, &bytes)?,
            _ => false,
        };
        if !kept {
            problems.push(format!("conversation {}: its file {} is not what it names, or not there", said_id(&s.id), said_id(&a.name)));
            lost.push((i, j));
        }
    }
    for (i, j) in lost.into_iter().rev() {
        s.messages[i].attachments.remove(j);
    }
    Ok(())
}

/// A conversation from its file's record (`held`), its index's `listed`
/// filling in what the file left out; with each file its messages name
/// (message, attachment, its path under `chats/`).
fn session(held: Record, listed: &Record, agent: Option<&str>) -> Result<(Session, Vec<Named>), String> {
    let kind_name = held.kind.as_deref().or(listed.kind.as_deref()).unwrap_or("chat");
    let kind = Kind::parse(kind_name).ok_or_else(|| format!("its kind {:?} is none of chat, cron, subagent", said_id(kind_name)))?;
    let files: Vec<Named> = held.messages.as_array().into_iter().flatten().enumerate()
        .flat_map(|(i, m)| m["attachments"].as_array().into_iter().flatten().enumerate().map(move |(j, a)| (i, j, a["path"].as_str().unwrap_or_default().to_string())))
        .collect();
    let messages: Vec<Message> = match held.messages {
        Value::Array(_) => serde_json::from_value(held.messages).map_err(|e| format!("its messages: {e}"))?,
        _ => return Err("its file holds no messages".into()),
    };
    let or = |a: Option<String>, b: &Option<String>| a.or_else(|| b.clone());
    let s = Session {
        format: String::new(),
        id: held.id,
        origin: ORIGIN.to_string(),
        reader: READER,
        kind,
        agent: agent.map(str::to_string),
        title: or(held.title, &listed.title).unwrap_or_default(),
        source: or(held.source, &listed.source),
        with: or(held.with, &listed.with),
        model: or(held.model, &listed.model),
        started_at: or(held.started_at, &listed.started_at),
        ended_at: or(held.ended_at, &listed.ended_at),
        parts: if held.sessions.is_empty() { listed.sessions.clone() } else { held.sessions },
        messages,
    };
    Ok((s, files))
}

/// Whether an index's `file` is one of the export's chats: relative,
/// under `chats/`, never out of it.
fn inside_chats(file: &str) -> bool {
    let p = Path::new(file);
    file.starts_with("chats/") && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// The bytes of `file` under `root`: a regular file, every folder on the
/// way to it a folder (no link out of the export), at most `max` bytes
/// (read so, whatever it says its size is).
fn read_file(root: &Path, file: &str, max: u64) -> Result<Vec<u8>> {
    let path = root.join(file);
    let mut at = root.to_path_buf();
    for part in Path::new(file).components() {
        at.push(part);
        let m = std::fs::symlink_metadata(&at).with_context(|| format!("reading {}", path.display()))?;
        let ok = if at == path { m.file_type().is_file() } else { m.file_type().is_dir() };
        if !ok {
            bail!("{} is no regular file of the export's (a link, or a device)", at.display());
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path).with_context(|| format!("reading {}", path.display()))?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bail!("{} is past the {} MiB it may be", path.display(), max / (1024 * 1024));
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

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

    fn spool() -> Spool {
        Spool::new(BTreeSet::new()).unwrap()
    }

    fn read_back(spool: &Spool, key: &str) -> Session {
        serde_json::from_slice(&std::fs::read(spool.path(key)).unwrap()).unwrap()
    }

    /// Goal: an export's chats read as the conversations it holds, each
    /// as it was written. Method: a chat and a scheduled run, found from
    /// the export's root and from its `chats/`.
    #[test]
    fn an_export_reads_as_its_conversations() {
        let root = super::super::tempdir();
        export(&root);
        assert_eq!(root_of(&root).as_deref(), Some(root.as_path()));
        assert_eq!(root_of(&root.join("chats")).as_deref(), Some(root.as_path()));
        assert_eq!(root_of(&root.join("chats/sessions")), None);
        let mut s = spool();
        let problems = read(&root, &mut s).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        let c = read_back(&s, "hermes-c1");
        assert_eq!((c.kind, c.title.as_str(), c.with.as_deref(), c.agent.as_deref(), c.reader, c.parts.len(), c.messages.len()), (Kind::Chat, "Beans", Some("Ann"), Some("Juniper"), READER, 2, 4));
        assert_eq!(c.messages[1].tool_calls[0].name, "web");
        let run = read_back(&s, "hermes-r1");
        assert_eq!((run.kind, run.messages[0].text.as_deref()), (Kind::Cron, Some("digest")), "Hermes' structured text, which the export copied raw, reads as text");
    }

    /// Goal: a message's files come with it, by their hash, each once; one
    /// whose bytes are not what it names, missing, or outside `chats/` is
    /// left out of its message, said, and the message kept.
    #[test]
    fn a_messages_files_come_with_it() {
        let root = super::super::tempdir();
        let chats = root.join("chats");
        std::fs::create_dir_all(chats.join("media")).unwrap();
        let png = b"\x89PNG fake bytes";
        std::fs::write(chats.join("media/plot.png"), png).unwrap();
        std::fs::write(root.join("outside.png"), png).unwrap();
        let sha = fragment_core::blob::sha256_hex(png);
        let file = |path: &str, sha: &str| json!({ "name": "plot.png", "media_type": "image/png", "size": png.len(), "sha256": sha, "path": path });
        let chat = json!({ "format": FORMAT, "id": "c1", "kind": "chat", "messages": [
            { "role": "user", "at": "2026-10-01T09:00:00Z", "attachments": [file("media/plot.png", &sha), file("media/plot.png", &"0".repeat(64)), file("../outside.png", &sha), file("media/missing.png", &sha)] },
            { "role": "user", "at": "2026-10-01T09:01:00Z", "text": "again", "attachments": [file("media/plot.png", &sha)] },
        ] });
        std::fs::write(chats.join("c1.json"), chat.to_string()).unwrap();
        std::fs::write(chats.join("index.json"), json!({ "format": FORMAT, "conversations": [{ "id": "c1", "file": "chats/c1.json" }] }).to_string()).unwrap();
        let mut s = spool();
        let problems = read(&root, &mut s).unwrap();
        assert_eq!(problems.len(), 3, "{problems:?}");
        let c = read_back(&s, "hermes-c1");
        assert_eq!(c.messages[0].attachments.len(), 1);
        assert_eq!(c.messages[0].attachments[0].path, format!("media/{sha}.png"));
        assert_eq!(c.messages[1].attachments.len(), 1, "a file two messages hold");
        assert_eq!(s.media.len(), 1, "is kept once");
        assert_eq!(std::fs::read(&s.media[&format!("media/{sha}.png")]).unwrap(), png);
    }

    /// Goal: an export is read only inside its `chats/`, and what it cannot
    /// read is a problem said, the rest read. Invalid: an index naming a
    /// file outside `chats/`, a file that is missing, a link out of the
    /// export, a device, another format.
    #[test]
    fn an_export_reads_nothing_outside_its_chats() {
        let root = super::super::tempdir();
        export(&root);
        std::fs::write(root.join("secret.json"), "{}").unwrap();
        std::os::unix::fs::symlink(root.join("secret.json"), root.join("chats/sessions/linked.json")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", root.join("chats/sessions/zero.json")).unwrap();
        std::os::unix::fs::symlink(root.join("chats/sessions"), root.join("chats/through")).unwrap();
        let index = root.join("chats/index.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
        for (id, file) in [("x", "chats/../secret.json"), ("y", "chats/sessions/missing.json"), ("l", "chats/sessions/linked.json"), ("z", "chats/sessions/zero.json"), ("t", "chats/through/c1.json")] {
            v["conversations"].as_array_mut().unwrap().push(json!({ "id": id, "file": file }));
        }
        std::fs::write(&index, v.to_string()).unwrap();
        let mut s = spool();
        let problems = read(&root, &mut s).unwrap();
        assert_eq!(s.entries.len(), 2);
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert!(problems.iter().filter(|p| p.contains("no regular file of the export's")).count() == 3, "{problems:?}");
        v["format"] = json!("finite-chat-archive/2");
        std::fs::write(&index, v.to_string()).unwrap();
        assert!(format!("{:#}", read(&root, &mut spool()).unwrap_err()).contains("not the old Finite's export"));
    }
}
