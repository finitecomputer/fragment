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
//! …}}`). Only regular files inside `chats/` are read (no link out of it,
//! no device), each once and at most `FILE_MAX_BYTES`, and each
//! conversation goes to the spool as it is read.
//!
//! **Read as typed records, streaming.** A file is read through as the
//! records it holds, never a whole tree of it and a copy, and each message
//! is kept as it is read (`Kept`): the base64 of a picture Hermes stored
//! with a message (which export.py copied as it was stored) is held one
//! message at a time, so reading a file holds about what its conversations
//! keep.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_core::session::{Kind, Message, Session};
use serde::de::IgnoredAny;
use serde::Deserialize;

use super::{said_id, Spool};

/// The export's chats' format.
pub const FORMAT: &str = "finite-chat-archive/1";
/// The origin its conversations are keyed under: Hermes', whose ids they are.
pub const ORIGIN: &str = "hermes";
/// Which read this is (`Session::reader`).
pub const READER: u32 = 10;
/// The largest of its files read: a month's runs share one, and a chat's
/// holds its pictures' base64 as Hermes stored them (read through, never
/// held whole: the module's doc).
const FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A conversation as the export wrote it: as its index lists it
/// (`Record<IgnoredAny>`, its messages a count), and as its file holds it
/// (`Record<Kept>`).
#[derive(Debug, Deserialize)]
struct Record<M> {
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
    messages: Option<M>,
}

type Listed = Record<IgnoredAny>;

/// A conversation's messages, each kept as it is read
/// (`Message::normalized`): never more of a message's text held than one.
#[derive(Debug)]
struct Kept(Vec<Message>);

impl<'de> Deserialize<'de> for Kept {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Kept, D::Error> {
        struct Each;
        impl<'de> serde::de::Visitor<'de> for Each {
            type Value = Kept;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a list of messages")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Kept, A::Error> {
                let mut kept = Vec::new();
                while let Some(m) = seq.next_element::<Message>()? {
                    kept.push(m.normalized());
                }
                Ok(Kept(kept))
            }
        }
        d.deserialize_seq(Each)
    }
}

/// A month's runs (`chats/runs/…`): each conversation by its id.
#[derive(Debug, Deserialize)]
struct Runs {
    conversations: BTreeMap<String, Record<Kept>>,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    format: String,
    /// The agent it exported.
    #[serde(default)]
    agent: Option<String>,
    conversations: Vec<Listed>,
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
    let index: IndexFile = serde_json::from_reader(open_file(root, "chats/index.json")?).with_context(|| format!("reading {}", index_path.display()))?;
    if index.format != FORMAT {
        bail!("{} is format {:?}, not the old Finite's export ({FORMAT})", index_path.display(), index.format);
    }
    let mut problems = Vec::new();
    let mut by_file: BTreeMap<String, Vec<Listed>> = BTreeMap::new();
    for r in index.conversations {
        match r.file.clone().filter(|f| inside_chats(f)) {
            Some(f) => by_file.entry(f).or_default().push(r),
            None => problems.push(format!("conversation {}: its file is not one of the export's chats", said_id(&r.id))),
        }
    }
    for (file, records) in by_file {
        // a month's runs name each by its id (`chats/runs/`); a chat's file
        // is the conversation
        let read: Result<BTreeMap<String, Record<Kept>>> = open_file(root, &file).and_then(|r| {
            if file.starts_with("chats/runs/") {
                serde_json::from_reader::<_, Runs>(r).map(|runs| runs.conversations).with_context(|| file.clone())
            } else {
                serde_json::from_reader::<_, Record<Kept>>(r).map(|h| BTreeMap::from([(h.id.clone(), h)])).with_context(|| file.clone())
            }
        });
        let mut held = match read {
            Ok(held) => held,
            Err(e) => {
                problems.push(format!("{file}: {e:#}"));
                continue;
            }
        };
        for listed in records {
            let Some(h) = held.remove(&listed.id) else {
                problems.push(format!("conversation {}: not in {file}", said_id(&listed.id)));
                continue;
            };
            match session(h, &listed, index.agent.as_deref()) {
                Ok(s) => spool.add(s)?,
                Err(e) => problems.push(format!("conversation {} in {file}: {e}", said_id(&listed.id))),
            }
        }
    }
    Ok(problems)
}

/// A conversation from its file's record (`held`), its index's `listed`
/// filling in what the file left out.
fn session(held: Record<Kept>, listed: &Listed, agent: Option<&str>) -> Result<Session, String> {
    let kind_name = held.kind.as_deref().or(listed.kind.as_deref()).unwrap_or("chat");
    let kind = Kind::parse(kind_name).ok_or_else(|| format!("its kind {:?} is none of chat, cron, subagent", said_id(kind_name)))?;
    let Some(Kept(messages)) = held.messages else {
        return Err("its file holds no messages".into());
    };
    let or = |a: Option<String>, b: &Option<String>| a.or_else(|| b.clone());
    Ok(Session {
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
    })
}

/// Whether an index's `file` is one of the export's chats: relative,
/// under `chats/`, never out of it.
fn inside_chats(file: &str) -> bool {
    let p = Path::new(file);
    file.starts_with("chats/") && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// `file` under `root`, to be read through: a regular file, every folder on
/// the way to it a folder (no link out of the export), at most
/// `FILE_MAX_BYTES` (as its size says, and read so whatever it says).
fn open_file(root: &Path, file: &str) -> Result<impl std::io::Read> {
    let path = root.join(file);
    let mut at = root.to_path_buf();
    let mut size = 0;
    for part in Path::new(file).components() {
        at.push(part);
        let m = std::fs::symlink_metadata(&at).with_context(|| format!("reading {}", path.display()))?;
        let ok = if at == path { m.file_type().is_file() } else { m.file_type().is_dir() };
        if !ok {
            bail!("{} is no regular file of the export's (a link, or a device)", at.display());
        }
        size = m.len();
    }
    if size > FILE_MAX_BYTES {
        bail!("{} is past the {} MiB a file of an export may be", path.display(), FILE_MAX_BYTES / (1024 * 1024));
    }
    // a file that grows while read is cut, and so fails to parse
    let f = std::fs::File::open(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(std::io::BufReader::new(f.take(FILE_MAX_BYTES)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{json, Value};
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

    /// Goal: a file is read as what its conversations keep, so one past
    /// the size they keep (a picture's base64, which export.py copied as
    /// Hermes stored it) reads, and one past `FILE_MAX_BYTES` is refused
    /// alone without being read. Method: a chat holding a 4 MiB picture,
    /// and a sparse file a byte past the cap.
    #[test]
    fn an_export_reads_as_what_it_keeps() {
        let root = super::super::tempdir();
        export(&root);
        let picture = "A".repeat(4 * 1024 * 1024);
        let parts = json!([{ "type": "text", "text": "look" }, { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{picture}") } }]);
        let photo = json!({ "id": "p1", "file": "chats/sessions/p1.json", "kind": "chat", "started_at": "2026-10-03T09:00:00Z",
            "messages": [{ "role": "user", "at": "2026-10-03T09:00:00Z", "text": format!("\u{0}json:{parts}") }, { "role": "assistant", "at": "2026-10-03T09:00:01Z", "text": "a bean" }] });
        std::fs::write(root.join("chats/sessions/p1.json"), photo.to_string()).unwrap();
        std::fs::File::create(root.join("chats/sessions/big.json")).unwrap().set_len(FILE_MAX_BYTES + 1).unwrap();
        let index = root.join("chats/index.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
        for (id, file) in [("p1", "chats/sessions/p1.json"), ("b1", "chats/sessions/big.json")] {
            v["conversations"].as_array_mut().unwrap().push(json!({ "id": id, "file": file }));
        }
        std::fs::write(&index, v.to_string()).unwrap();
        let mut s = spool();
        let problems = read(&root, &mut s).unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("past the 512 MiB"), "{problems:?}");
        let p = read_back(&s, "hermes-p1");
        assert_eq!(p.messages[0].text.as_deref(), Some("look\n[image]"));
        assert!(s.bytes < 64 * 1024, "the spool holds what is kept: {}", s.bytes);
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
