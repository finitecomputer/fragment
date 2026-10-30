//! A person's session archive (templates/sessions/README.md): the conversations
//! they had with an agent elsewhere, brought in by `fragment session
//! import`, kept as files in a fragment of their own of kind `sessions`.
//! One file a conversation (`sessions/<key>.json`) and one index a kind
//! (`index/<kind>.json`), newest first. Pure: the format, its limits,
//! where each conversation lives, and how an import merges into what the
//! archive holds. The CLI reads each origin's own store into it (Hermes'
//! `state.db`, the old Finite's export) and writes it; the shell reads it.
//!
//! **Files, not the app's SQL.** An archive outgrows the app's 16 MiB
//! (`limits::APP_DB_MAX_BYTES`), and as files it is git's: one commit lands
//! a batch whole or not at all, a file of 1 MiB or more is a blob, and an
//! agent reads it with `fragment sync` and its own tools.
//!
//! **The index is a projection of `sessions/`.** An import writes its
//! conversations first, then the indexes in its last commit, which also
//! adds any conversation the indexes lack (an import that stopped before
//! its last commit) and drops entries whose file is gone (`Merge`). So a
//! crash leaves no entry naming a missing file, and the next import of
//! anything puts the indexes right.
//!
//! **Keyed by the original id.** A conversation's key is its origin and its
//! id there (`key`), so importing the same one again (a re-run, a retry
//! after a crash, the same chats from another of the origin's stores)
//! finds it, and replaces it only with a newer copy of it (`Entry::newer`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What each file of an archive names as its format.
pub const FORMAT: &str = "session-archive/1";

/// The blessed template, and kind, of a person's archive; the label and the
/// title it is made with.
pub const TEMPLATE: &str = "sessions";
pub const LABEL: &str = "sessions";
pub const TITLE: &str = "Session Archive";

/// Conversations one archive holds, and the bytes of their files: an import
/// that would pass either is refused whole, before it writes anything. Both
/// sit inside what a person's export takes of one fragment (its files, and
/// its bytes: `export::limits`), with room for the archive's other files,
/// so an archive never fails its person's export. The count bounds each
/// index too (at about 500 bytes an entry, 9 MB if every one is a run).
pub const SESSIONS_MAX: usize = 18_000;
pub const ARCHIVE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const _: () = assert!(SESSIONS_MAX + 2_000 <= crate::export::limits::FILES_MAX, "an archive's files fit its person's export");
const _: () = assert!(2 * ARCHIVE_MAX_BYTES <= crate::export::limits::FRAGMENT_MAX_BYTES, "an archive's bytes fit its person's export");
/// One conversation's file: one larger is refused, and the rest imported.
pub const SESSION_MAX_BYTES: usize = 8 * 1024 * 1024;
/// A tool's output, an assistant's reasoning, and a tool call's arguments
/// are cut to this many characters (what the person and the agent said
/// never is), as the old Finite's export cuts them.
pub const TOOL_TEXT_MAX_CHARS: usize = 2_000;
pub const REASONING_MAX_CHARS: usize = 2_000;
pub const ARGUMENTS_MAX_CHARS: usize = 1_000;
/// A conversation's id in its origin, and its origin's name.
pub const ID_MAX_BYTES: usize = 200;
pub const ORIGIN_MAX_BYTES: usize = 32;
/// A title, a preview, and the short fields (`source`, `with`, `model`, a
/// role, a tool's name), in characters.
pub const TITLE_MAX_CHARS: usize = 120;
pub const PREVIEW_MAX_CHARS: usize = 160;
pub const FIELD_MAX_CHARS: usize = 200;
/// Where an untitled conversation's title comes from: its first words.
const TITLE_FROM_TEXT_CHARS: usize = 80;

/// What a conversation was, as its origin ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A conversation a person had with the agent.
    Chat,
    /// A scheduled job's run.
    Cron,
    /// A side task the agent ran on its own (a delegate, a tool's run).
    Subagent,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Chat, Kind::Cron, Kind::Subagent];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Chat => "chat",
            Kind::Cron => "cron",
            Kind::Subagent => "subagent",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A tool call an assistant's message made: its tool, what it passed, and
/// its id where its origin kept one (a tool's result names it: `call`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
}

/// One message of a conversation, in its order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Message {
    /// `user`, `assistant`, `tool` or `system`, as its origin named it.
    pub role: String,
    /// When it was said: `YYYY-MM-DDTHH:MM:SS.sssZ` (`iso`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A tool's output: the tool it came from, and the call it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// The summary its origin wrote when it compressed a long conversation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summary: bool,
}

impl Message {
    /// Whether a person wrote it or the agent said it to them: what a
    /// conversation's count and preview are made of.
    pub fn said(&self) -> bool {
        matches!(self.role.as_str(), "user" | "assistant") && self.text.as_deref().is_some_and(|t| !t.trim().is_empty()) && !self.summary && self.tool_calls.is_empty()
    }
}

/// One conversation: a file of its own, `sessions/<key>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub format: String,
    /// Its id in its origin (Hermes': the session it began as).
    pub id: String,
    /// The system it came from (`hermes`).
    pub origin: String,
    /// Which read made it (`Entry::newer`): the old Finite's export 10,
    /// Hermes' own database 20; a reader fixed later takes the next number.
    #[serde(default)]
    pub reader: u32,
    pub kind: Kind,
    /// The agent it was with, as its origin named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub title: String,
    /// Where it was said, as its origin named it (`telegram`, `cli`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Whom the agent spoke with, as its origin named them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When it was last said (its last message's time, else its end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// The origin's own records it joins, in order (Hermes starts a new
    /// session each time it compresses a long conversation).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    pub messages: Vec<Message>,
}

/// Why a conversation is not imported (the rest are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Its id or origin is none an archive keys a conversation by.
    Id(String),
    /// Its file would pass `SESSION_MAX_BYTES`.
    TooLarge(usize),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Id(why) => f.write_str(why),
            Refused::TooLarge(bytes) => write!(f, "it is {} MiB, past the {} MiB a conversation holds", mib(*bytes), SESSION_MAX_BYTES / (1024 * 1024)),
        }
    }
}

fn mib(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

/// The key a conversation is filed under: its origin and its id there,
/// `hermes-20261029_183012_ab12cd`. An id that is no file name's (another
/// character, too long) is keyed by its hash, `hermes--<32 hex>`, which no
/// id's own key can be (an id's starts with a letter or a digit).
pub fn key(origin: &str, id: &str) -> Result<String, Refused> {
    let origin_ok = (1..=ORIGIN_MAX_BYTES).contains(&origin.len()) && origin.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) && origin.as_bytes()[0].is_ascii_lowercase();
    if !origin_ok {
        return Err(Refused::Id(format!("its origin {origin:?} is not a name (lowercase letters and digits)")));
    }
    if id.is_empty() || id.len() > ID_MAX_BYTES || id.chars().any(char::is_control) {
        return Err(Refused::Id(format!("its id is not one (1 to {ID_MAX_BYTES} bytes, no control characters)")));
    }
    let plain = id.as_bytes()[0].is_ascii_alphanumeric() && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) && id.len() <= 100;
    Ok(if plain {
        format!("{origin}-{id}")
    } else {
        format!("{origin}--{}", &hex::encode(Sha256::digest(id.as_bytes()))[..32])
    })
}

/// Where a conversation's file is.
pub fn session_path(key: &str) -> String {
    format!("sessions/{key}.json")
}

/// The key a path under `sessions/` files, when it is one.
pub fn key_of_path(path: &str) -> Option<&str> {
    path.strip_prefix("sessions/")?.strip_suffix(".json").filter(|k| !k.is_empty() && !k.contains('/'))
}

/// Where a kind's index is.
pub fn index_path(kind: Kind) -> String {
    format!("index/{}.json", kind.as_str())
}

impl Session {
    /// The conversation as an archive keeps it: tool output, reasoning and
    /// arguments cut (`clip`), structured text its origin stored as JSON
    /// read as text (`text_of`), the short fields bounded, the title made
    /// from its first words when it has none, and its end its last
    /// message's time. Applying it again changes nothing.
    pub fn normalized(mut self) -> Session {
        self.format = FORMAT.to_string();
        for m in &mut self.messages {
            m.role = bounded(&m.role);
            m.text = m.text.take().map(|t| text_of(&t)).filter(|t| !t.is_empty()).map(|t| if m.role == "tool" { clip(&t, TOOL_TEXT_MAX_CHARS) } else { t });
            m.tool = m.tool.take().map(|t| bounded(&t)).filter(|t| !t.is_empty());
            m.call = m.call.take().map(|t| bounded(&t)).filter(|t| !t.is_empty());
            m.reasoning = m.reasoning.take().filter(|r| !r.is_empty()).map(|r| clip(&r, REASONING_MAX_CHARS));
            for call in &mut m.tool_calls {
                call.id = call.id.take().map(|t| bounded(&t)).filter(|t| !t.is_empty());
                call.name = bounded(&call.name);
                call.arguments = clip(&call.arguments, ARGUMENTS_MAX_CHARS);
            }
            m.at = m.at.take().and_then(|at| normal_iso(&at));
        }
        for field in [&mut self.source, &mut self.with, &mut self.model, &mut self.agent] {
            *field = field.take().map(|v| bounded(&v)).filter(|v| !v.is_empty());
        }
        self.started_at = self.started_at.take().and_then(|at| normal_iso(&at));
        let last = self.messages.iter().rev().find_map(|m| m.at.clone());
        self.ended_at = last.or_else(|| self.ended_at.take().and_then(|at| normal_iso(&at)));
        let first = self.first_words();
        let titled = first_line(&self.title, TITLE_MAX_CHARS);
        self.title = if !titled.is_empty() {
            titled
        } else if !first.is_empty() {
            first_line(&first, TITLE_FROM_TEXT_CHARS)
        } else {
            "Untitled".to_string()
        };
        self
    }

    /// What a person said first in it, or nothing.
    fn first_words(&self) -> String {
        self.messages.iter().find(|m| m.said() && m.role == "user").and_then(|m| m.text.clone()).unwrap_or_default()
    }

    /// Its file: pretty JSON, so an agent's line tools read it, and the
    /// same conversation always makes the same bytes.
    pub fn file(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("a conversation serializes");
        bytes.push(b'\n');
        bytes
    }

    /// Its key, its file and its index entry, or why it is not imported.
    /// It must be `normalized` first.
    pub fn filed(&self) -> Result<(Entry, Vec<u8>), Refused> {
        let key = key(&self.origin, &self.id)?;
        let bytes = self.file();
        if bytes.len() > SESSION_MAX_BYTES {
            return Err(Refused::TooLarge(bytes.len()));
        }
        let preview = self.first_words();
        let entry = Entry {
            key,
            id: self.id.clone(),
            origin: self.origin.clone(),
            reader: self.reader,
            kind: self.kind,
            title: self.title.clone(),
            source: self.source.clone(),
            with: self.with.clone(),
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
            messages: self.messages.iter().filter(|m| m.said()).count(),
            preview: preview.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PREVIEW_MAX_CHARS).collect(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.len() as u64,
        };
        Ok((entry, bytes))
    }
}

/// A conversation as its kind's index lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub id: String,
    pub origin: String,
    /// Which read made it (`Session::reader`).
    #[serde(default)]
    pub reader: u32,
    pub kind: Kind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// How many messages were said in it (`Message::said`).
    pub messages: usize,
    /// The first words a person said in it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview: String,
    /// Its file's SHA-256 and size: an import of the same bytes writes
    /// nothing.
    pub sha256: String,
    pub bytes: u64,
}

impl Entry {
    /// When it was last said, else begun: what an index sorts by.
    fn when(&self) -> Option<&str> {
        self.ended_at.as_deref().or(self.started_at.as_deref())
    }

    /// Whether this copy of a conversation should replace `held`: it was
    /// last said later, or as late by a better read of it (a reader that
    /// shows it as its origin did, or one fixed since: `reader`). An older
    /// copy (an earlier export of it) never replaces a newer one, nor an
    /// equal read one as late.
    pub fn newer(&self, held: &Entry) -> bool {
        (self.when(), self.reader) > (held.when(), held.reader)
    }
}

/// A kind's index: its conversations, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub format: String,
    pub kind: Kind,
    pub sessions: Vec<Entry>,
}

impl Index {
    /// The index of `kind` over `entries` (any order; those of other kinds
    /// left out), newest first, then by key.
    pub fn of(kind: Kind, entries: &BTreeMap<String, Entry>) -> Index {
        let mut sessions: Vec<Entry> = entries.values().filter(|e| e.kind == kind).cloned().collect();
        sessions.sort_by(|a, b| b.when().cmp(&a.when()).then_with(|| a.key.cmp(&b.key)));
        Index { format: FORMAT.to_string(), kind, sessions }
    }

    pub fn file(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).expect("an index serializes");
        bytes.push(b'\n');
        bytes
    }
}

/// An import merged into what an archive holds; `P` is where each
/// conversation's file is (its bytes, or a file they were spooled to).
#[derive(Debug, PartialEq, Eq)]
pub struct Merge<P> {
    /// The conversations to write, by key, with their files.
    pub writes: BTreeMap<String, P>,
    /// Every entry after the import: what the indexes list.
    pub entries: BTreeMap<String, Entry>,
    /// The kinds whose index changes.
    pub indexes: Vec<Kind>,
    pub added: usize,
    pub updated: usize,
    /// Held as they are: the same bytes, or a newer copy held already.
    pub unchanged: usize,
}

impl<P> Default for Merge<P> {
    fn default() -> Self {
        Merge { writes: BTreeMap::new(), entries: BTreeMap::new(), indexes: Vec::new(), added: 0, updated: 0, unchanged: 0 }
    }
}

/// An import past an archive's limits: the conversations, or the bytes of
/// their files, it would hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TooMuch {
    Conversations(usize),
    Bytes(u64),
}

impl std::fmt::Display for TooMuch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TooMuch::Conversations(n) => write!(f, "the archive would hold {n} conversations, past the {SESSIONS_MAX} it holds (`--kind chat` imports only the chats)"),
            TooMuch::Bytes(n) => write!(f, "the archive would hold {} MiB of conversations, past the {} MiB it holds (`--kind chat` imports only the chats)", n / (1024 * 1024), ARCHIVE_MAX_BYTES / (1024 * 1024)),
        }
    }
}

/// Whether `entries` (an archive's, or an import's alone) are within an
/// archive's limits.
pub fn within(count: usize, bytes: u64) -> Result<(), TooMuch> {
    if count > SESSIONS_MAX {
        return Err(TooMuch::Conversations(count));
    }
    if bytes > ARCHIVE_MAX_BYTES {
        return Err(TooMuch::Bytes(bytes));
    }
    Ok(())
}

/// An import into an archive: `indexed` is what its indexes list,
/// `stored` the keys `sessions/` holds files for, `found` the entry of
/// each of those whose file the indexes do not describe (they lack it, or
/// name another copy than the one there: read from its file), and
/// `incoming` the import's conversations. Every write is a conversation new
/// or newer; the indexes change when a conversation does, or when they
/// disagree with `sessions/` (an entry without its file is dropped, a file
/// is listed by `found`'s entry). One that grows the archive past its
/// limits is refused whole.
pub fn merge<P>(indexed: &BTreeMap<String, Entry>, stored: &std::collections::BTreeSet<String>, found: Vec<Entry>, incoming: Vec<(Entry, P)>) -> Result<Merge<P>, TooMuch> {
    let mut m = Merge::default();
    let mut changed: std::collections::BTreeSet<Kind> = std::collections::BTreeSet::new();
    for (key, entry) in indexed {
        if stored.contains(key) {
            m.entries.insert(key.clone(), entry.clone());
        } else {
            changed.insert(entry.kind);
        }
    }
    for entry in found {
        if stored.contains(&entry.key) && m.entries.get(&entry.key) != Some(&entry) {
            changed.insert(entry.kind);
            if let Some(stale) = m.entries.insert(entry.key.clone(), entry) {
                changed.insert(stale.kind);
            }
        }
    }
    for (entry, file) in incoming {
        match m.entries.get(&entry.key) {
            Some(held) if held.sha256 == entry.sha256 || !entry.newer(held) => m.unchanged += 1,
            held => {
                if held.is_some() {
                    m.updated += 1;
                } else {
                    m.added += 1;
                }
                if let Some(h) = held {
                    changed.insert(h.kind);
                }
                changed.insert(entry.kind);
                m.writes.insert(entry.key.clone(), file);
                m.entries.insert(entry.key.clone(), entry);
            }
        }
    }
    if m.added > 0 || m.updated > 0 {
        within(m.entries.len(), m.entries.values().map(|e| e.bytes).sum())?;
    }
    m.indexes = changed.into_iter().collect();
    Ok(m)
}

/// `text` as an archive keeps it: a value Hermes stored as structured
/// content (`\u{0}json:` and its JSON) read as its text, with a mark for
/// each image, recording or file it held (`[image]`); anything else as it
/// is. The old Finite's export copied such values as they were stored.
pub fn text_of(text: &str) -> String {
    match text.strip_prefix("\u{0}json:").and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) {
        Some(serde_json::Value::String(s)) => s,
        Some(v @ serde_json::Value::Array(_)) => parts_text(&v).unwrap_or_else(|| text.to_string()),
        _ => text.to_string(),
    }
}

/// A message's parts (`[{type, text}, {type: "image_url", …}]`) as text:
/// each text part's, and a mark for each medium. `None` when it is not a
/// list of parts.
pub fn parts_text(v: &serde_json::Value) -> Option<String> {
    let parts = v.as_array()?;
    if parts.is_empty() || !parts.iter().all(|p| p.get("type").is_some_and(|t| t.is_string())) {
        return None;
    }
    let texts: Vec<String> = parts
        .iter()
        .filter_map(|p| {
            let kind = p["type"].as_str().unwrap_or_default();
            if matches!(kind, "text" | "input_text" | "output_text") {
                return p["text"].as_str().map(str::to_string);
            }
            ["image", "audio", "video", "file"].into_iter().find(|w| kind.contains(w)).map(|w| format!("[{w}]"))
        })
        .filter(|t| !t.is_empty())
        .collect();
    Some(texts.join("\n"))
}

/// `text` cut to `max` characters, saying how many more there were; a text
/// already cut so (by this, or by the old Finite's export) is left as it is.
pub fn clip(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    if let Some((head, tail)) = text.rsplit_once("… [") {
        let count = tail.strip_suffix(" more characters not kept]").or_else(|| tail.strip_suffix(" more characters not exported]"));
        let marked = count.is_some_and(|c| (1..=20).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit()));
        if marked && head.chars().count() <= max {
            return text.to_string();
        }
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}… [{} more characters not kept]", n - max)
}

/// The first line of `text` with words, at most `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    line.chars().take(max).collect()
}

/// A short field, its control characters gone, at most `FIELD_MAX_CHARS`.
fn bounded(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(FIELD_MAX_CHARS).collect::<String>().trim().to_string()
}

// ---- time ----

/// Seconds since the epoch (as Hermes keeps a time) as an archive writes
/// one: `YYYY-MM-DDTHH:MM:SS.sssZ`, in UTC, to the millisecond, so times
/// sort as text. `None` for no time a calendar holds.
pub fn iso(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || !(0.0..=253_402_300_799.0).contains(&seconds) {
        return None;
    }
    let ms = (seconds * 1000.0).round() as i64;
    let s = ms.div_euclid(1000);
    let (y, mo, d) = crate::cron::civil(s.div_euclid(86_400));
    let t = s.rem_euclid(86_400);
    Some(format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", t / 3600, t / 60 % 60, t % 60, ms.rem_euclid(1000)))
}

/// An RFC 3339 time (as the old Finite's export wrote them: a fraction of
/// any length, `Z` or an offset) as `iso` writes it, or `None`.
pub fn normal_iso(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> { b.get(r.clone()).filter(|d| d.iter().all(u8::is_ascii_digit)).map(|_| text[r].parse().unwrap()) };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, s) = (digits(0..4)?, digits(5..7)?, digits(8..10)?, digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut i = 19;
    let mut fraction = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        fraction = format!("0.{}", &text[start..i]).parse::<f64>().ok()?;
    }
    let offset = match &text[i..] {
        "Z" | "z" => 0,
        zone if zone.len() == 6 && matches!(zone.as_bytes()[0], b'+' | b'-') && zone.as_bytes()[3] == b':' => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            sign * (zone[1..3].parse::<i64>().ok()? * 3600 + zone[4..6].parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    iso((days * 86_400 + h * 3600 + mi * 60 + s - offset) as f64 + fraction)
}

/// (year, month 1-12, day 1-31) → days since 1970-01-01 (Hinnant's
/// days_from_civil), `cron::civil`'s inverse.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn said(role: &str, at: &str, text: &str) -> Message {
        Message { role: role.into(), at: Some(at.into()), text: Some(text.into()), ..Default::default() }
    }

    fn chat(id: &str, messages: Vec<Message>) -> Session {
        Session {
            format: String::new(),
            id: id.into(),
            origin: "hermes".into(),
            reader: 10,
            kind: Kind::Chat,
            agent: None,
            title: String::new(),
            source: Some("telegram".into()),
            with: Some("Ann".into()),
            model: None,
            started_at: Some("2026-10-01T09:00:00Z".into()),
            ended_at: None,
            parts: vec![id.into()],
            messages,
        }
    }

    fn filed(s: Session) -> (Entry, Vec<u8>) {
        s.normalized().filed().expect("filed")
    }

    /// Goal: a conversation keeps what was said whole and cuts only what
    /// the format cuts, the same way every time. Method: one with a long
    /// tool output, reasoning and arguments, Hermes' structured text, and
    /// no title; normalized, and normalized again.
    #[test]
    fn a_conversation_is_kept_as_said_and_cut_where_the_format_cuts() {
        let long = "x".repeat(5_000);
        let mut s = chat("20261001_090000_ab12cd", vec![
            said("user", "2026-10-01T09:00:00.5+00:00", "  \n Plan the garden\nwith beans"),
            Message { role: "assistant".into(), at: Some("2026-10-01T09:00:01Z".into()), reasoning: Some(long.clone()), tool_calls: vec![ToolCall { id: Some("c1".into()), name: "search".into(), arguments: long.clone() }], ..Default::default() },
            Message { role: "tool".into(), tool: Some("search".into()), text: Some(long.clone()), ..Default::default() },
            said("assistant", "2026-10-01T09:00:02Z", &long),
            said("user", "2026-10-01T09:01:00Z", "\u{0}json:[{\"type\":\"text\",\"text\":\"look\"},{\"type\":\"image_url\",\"image_url\":{\"url\":\"data:…\"}}]"),
        ]);
        s.title = "  ".into();
        let n = s.normalized();
        assert_eq!(n.title, "Plan the garden");
        assert_eq!(n.messages[0].at.as_deref(), Some("2026-10-01T09:00:00.500Z"));
        assert_eq!(n.messages[1].reasoning.as_ref().unwrap().chars().count(), REASONING_MAX_CHARS + "… [3000 more characters not kept]".chars().count());
        assert!(n.messages[1].tool_calls[0].arguments.ends_with("… [4000 more characters not kept]"));
        assert!(n.messages[2].text.as_ref().unwrap().ends_with("… [3000 more characters not kept]"), "a tool's output is cut");
        assert_eq!(n.messages[3].text.as_deref(), Some(long.as_str()), "what the agent said never is");
        assert_eq!(n.messages[4].text.as_deref(), Some("look\n[image]"));
        assert_eq!(n.ended_at.as_deref(), Some("2026-10-01T09:01:00.000Z"), "its end is its last message's time");
        assert_eq!(n.clone().normalized(), n, "normalizing again changes nothing");
        let (e, bytes) = n.filed().unwrap();
        assert_eq!((e.key.as_str(), e.messages, e.preview.as_str()), ("hermes-20261001_090000_ab12cd", 3, "Plan the garden with beans"));
        assert_eq!(serde_json::from_slice::<Session>(&bytes).unwrap(), n, "its file reads back as itself");
        assert_eq!(n.file(), bytes, "and the same conversation makes the same bytes");
    }

    /// Goal: a cut is said once, whoever cut it. Valid: a text cut here or
    /// by the old Finite's export stays as it is. Invalid: a text that only
    /// looks cut (its head too long) is cut.
    #[test]
    fn a_cut_text_is_not_cut_again() {
        let ours = clip(&"a".repeat(30), 10);
        assert_eq!(ours, format!("{}… [20 more characters not kept]", "a".repeat(10)));
        assert_eq!(clip(&ours, 10), ours);
        let theirs = format!("{}… [7 more characters not exported]", "b".repeat(10));
        assert_eq!(clip(&theirs, 10), theirs);
        let fake = format!("{}… [7 more characters not kept]", "c".repeat(11));
        assert!(clip(&fake, 10).starts_with(&"c".repeat(10)) && clip(&fake, 10).ends_with("more characters not kept]") && clip(&fake, 10) != fake);
        assert_eq!(clip("short", 10), "short");
        // a marker after anything but its count is no cut: a long output
        // ending in one is cut all the same
        let crafted = format!("{}… [7{} more characters not kept]", "d".repeat(5), "x".repeat(10_000));
        assert!(clip(&crafted, 10).chars().count() < 60, "{}", clip(&crafted, 10).chars().count());
    }

    /// Goal: a conversation is keyed by its origin and id, one key each.
    /// Valid: an id that is a file name's is its own key; any other is its
    /// hash's, which no plain id's key can be. Invalid: no id, a control
    /// character, a long one, an origin that is no name.
    #[test]
    fn a_conversation_is_keyed_by_its_origin_and_id() {
        assert_eq!(key("hermes", "20261001_090000_ab12cd").unwrap(), "hermes-20261001_090000_ab12cd");
        let hashed = key("hermes", "a/b c").unwrap();
        assert!(hashed.starts_with("hermes--") && hashed.len() == "hermes--".len() + 32, "{hashed}");
        assert_ne!(key("hermes", "a/b c").unwrap(), key("hermes", "a/b d").unwrap());
        assert!(key("hermes", "-lead").unwrap().starts_with("hermes--"), "an id that does not start with a letter or digit is hashed");
        assert!(key("hermes", &"x".repeat(150)).unwrap().starts_with("hermes--"));
        for bad in ["", "a\u{0}b", &"x".repeat(ID_MAX_BYTES + 1)] {
            assert!(matches!(key("hermes", bad), Err(Refused::Id(_))), "{bad:?}");
        }
        for bad in ["", "Hermes", "her mes", "1hermes"] {
            assert!(matches!(key(bad, "x"), Err(Refused::Id(_))), "{bad:?}");
        }
        let k = key("hermes", "x1").unwrap();
        assert_eq!(key_of_path(&session_path(&k)), Some(k.as_str()));
        assert!(fragment_proto::valid_repo_path(&session_path(&hashed)));
        assert_eq!(key_of_path("sessions/a/b.json"), None);
        assert_eq!(index_path(Kind::Cron), "index/cron.json");
    }

    /// Goal: one conversation too large for an archive is refused alone.
    #[test]
    fn a_conversation_past_the_limit_is_refused() {
        let s = chat("big", vec![said("user", "2026-10-01T09:00:00Z", &"w".repeat(SESSION_MAX_BYTES))]).normalized();
        assert!(matches!(s.filed(), Err(Refused::TooLarge(n)) if n > SESSION_MAX_BYTES));
    }

    fn held(entries: &[&Entry]) -> (BTreeMap<String, Entry>, BTreeSet<String>) {
        (entries.iter().map(|e| (e.key.clone(), (*e).clone())).collect(), entries.iter().map(|e| e.key.clone()).collect())
    }

    /// Goal: an import adds each conversation once, however often it runs
    /// (replay), and only ever replaces one with a newer copy of it. Method:
    /// an archive holding one; an import of it again (nothing), of an older
    /// copy (nothing), of a newer copy (updated), and of a new one (added);
    /// then copies as late as the one held: another of the same read
    /// (nothing, whatever it holds), and a better read (updated).
    #[test]
    fn an_import_adds_nothing_twice_and_keeps_the_newest() {
        let one = vec![said("user", "2026-10-01T09:00:00Z", "hello")];
        let (a, _) = filed(chat("a", one.clone()));
        let (indexed, stored) = held(&[&a]);
        let again = merge(&indexed, &stored, vec![], vec![filed(chat("a", one.clone()))]).unwrap();
        assert_eq!((again.writes.len(), again.unchanged, again.indexes.len()), (0, 1, 0), "the same again writes nothing");

        let mut two = one.clone();
        two.push(said("assistant", "2026-10-02T09:00:00Z", "hi"));
        let (newer, _) = filed(chat("a", two.clone()));
        let (indexed2, stored2) = held(&[&newer]);
        let older = merge(&indexed2, &stored2, vec![], vec![filed(chat("a", one))]).unwrap();
        assert_eq!((older.writes.len(), older.unchanged), (0, 1), "an older copy never replaces a newer one");

        let m = merge(&indexed, &stored, vec![], vec![filed(chat("a", two)), filed(chat("b", vec![said("user", "2026-10-03T09:00:00Z", "new")]))]).unwrap();
        assert_eq!((m.added, m.updated, m.unchanged), (1, 1, 0));
        assert_eq!(m.writes.keys().collect::<Vec<_>>(), ["hermes-a", "hermes-b"]);
        assert_eq!(m.indexes, vec![Kind::Chat]);
        let index = Index::of(Kind::Chat, &m.entries);
        assert_eq!(index.sessions.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["hermes-b", "hermes-a"], "newest first");
        assert!(Index::of(Kind::Cron, &m.entries).sessions.is_empty());

        // as late: the export's copy that kept a duplicate never replaces a
        // copy as late, and a better read of it (or a fixed one) does
        let (late, _) = filed(chat("c", vec![said("user", "2026-10-03T09:00:00Z", "x")]));
        let (indexed3, stored3) = held(&[&late]);
        let dup = filed(chat("c", vec![said("user", "2026-10-03T09:00:00Z", "x"), said("user", "2026-10-03T09:00:00Z", "x")]));
        assert_eq!(merge(&indexed3, &stored3, vec![], vec![dup]).map(|m| m.unchanged), Ok(1));
        let better = filed(Session { reader: 20, ..chat("c", vec![said("user", "2026-10-03T09:00:00Z", "x!")]) });
        assert_eq!(merge(&indexed3, &stored3, vec![], vec![better]).map(|m| m.updated), Ok(1));
    }

    /// Goal: after a crash (restart), the next import puts the indexes
    /// right. Method: an archive whose `sessions/` holds a file its index
    /// lacks (an import stopped before its last commit) and whose index
    /// names a file gone; an import of nothing new fixes both.
    #[test]
    fn an_import_repairs_an_index_a_crash_left() {
        let (a, _) = filed(chat("a", vec![said("user", "2026-10-01T09:00:00Z", "a")]));
        let (gone, _) = filed(chat("gone", vec![said("user", "2026-10-01T09:00:00Z", "g")]));
        let (orphan, _) = filed(Session { kind: Kind::Cron, ..chat("orphan", vec![said("user", "2026-10-02T09:00:00Z", "o")]) });
        let (indexed, _) = held(&[&a, &gone]);
        let stored: BTreeSet<String> = [a.key.clone(), orphan.key.clone()].into();
        let m = merge::<Vec<u8>>(&indexed, &stored, vec![orphan.clone()], vec![]).unwrap();
        assert!(m.writes.is_empty());
        assert_eq!(m.entries.keys().collect::<Vec<_>>(), [&a.key, &orphan.key]);
        assert_eq!(m.indexes, vec![Kind::Chat, Kind::Cron], "both indexes are written again");

        // a file whose index entry names another copy (an update whose index
        // commit never landed) is listed by its own entry
        let (newer_a, _) = filed(chat("a", vec![said("user", "2026-10-05T09:00:00Z", "a, later")]));
        let m = merge::<Vec<u8>>(&indexed, &stored, vec![orphan, newer_a.clone()], vec![]).unwrap();
        assert_eq!(m.entries.get(&a.key), Some(&newer_a));
        assert_eq!(m.indexes, vec![Kind::Chat, Kind::Cron]);
    }

    /// Goal: an archive past its limits (its conversations, or their
    /// bytes) is refused before anything is written, and one already full
    /// still takes newer copies of what it holds.
    #[test]
    fn an_import_past_the_limits_is_refused_whole() {
        let base = filed(chat("x", vec![said("user", "2026-10-01T09:00:00Z", "x")])).0;
        let indexed: BTreeMap<String, Entry> = (0..SESSIONS_MAX).map(|i| (format!("hermes-{i}"), Entry { key: format!("hermes-{i}"), ..base.clone() })).collect();
        let stored: BTreeSet<String> = indexed.keys().cloned().collect();
        let more = merge(&indexed, &stored, vec![], vec![filed(chat("new", vec![said("user", "2026-10-01T09:00:00Z", "n")]))]);
        assert_eq!(more.err(), Some(TooMuch::Conversations(SESSIONS_MAX + 1)));
        let newer = filed(chat("0", vec![said("user", "2026-10-05T09:00:00Z", "n")]));
        assert_eq!(merge(&indexed, &stored, vec![], vec![newer]).map(|m| m.updated), Ok(1));

        let heavy = Entry { bytes: ARCHIVE_MAX_BYTES / 2, ..base.clone() };
        let (indexed, stored) = held(&[&heavy]);
        let (light, bytes) = filed(chat("light", vec![said("user", "2026-10-01T09:00:00Z", "l")]));
        let past = merge(&indexed, &stored, vec![], vec![(Entry { bytes: ARCHIVE_MAX_BYTES / 2 + 1, ..light }, bytes)]);
        assert_eq!(past.err(), Some(TooMuch::Bytes(ARCHIVE_MAX_BYTES + 1)));
        assert_eq!(within(SESSIONS_MAX, ARCHIVE_MAX_BYTES), Ok(()));
    }

    /// Goal: times read as the export wrote them and Hermes kept them, and
    /// written one way. Invalid: no time a calendar holds.
    #[test]
    fn times_are_written_one_way() {
        assert_eq!(iso(1_790_000_000.25).as_deref(), Some("2026-09-21T14:13:20.250Z"));
        assert_eq!(normal_iso("2026-09-21T14:13:20.25Z"), iso(1_790_000_000.25));
        assert_eq!(normal_iso("2026-09-21T16:13:20.250000+02:00"), iso(1_790_000_000.25));
        assert_eq!(normal_iso("2026-09-21T14:13:20Z").as_deref(), Some("2026-09-21T14:13:20.000Z"));
        assert_eq!(normal_iso("1970-01-01T00:00:00Z").as_deref(), Some("1970-01-01T00:00:00.000Z"));
        assert_eq!(normal_iso("2000-02-29T12:00:00Z").as_deref(), iso(951_825_600.0).as_deref());
        for bad in ["", "yesterday", "2026-13-01T00:00:00Z", "2026-10-01T00:00:00", "2026-10-01T00:00:00.Z", "2026-10-01T00:00:00+0200"] {
            assert_eq!(normal_iso(bad), None, "{bad:?}");
        }
        assert_eq!(iso(f64::NAN), None);
        assert_eq!(iso(-1.0), None);
    }
}
