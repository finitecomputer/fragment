//! Hermes' own session database (`state.db`) as a session archive's
//! conversations: the old Finite's export (`export.py`, its `chats/`) made
//! in Rust, checked against Hermes v0.21.7, whose `hermes_state*.py` this
//! follows where the two differ (named below). Read only: the database is
//! opened read-only and read in one transaction, so a Hermes still writing
//! it is read as of one moment and never changed.
//!
//! **A conversation** is a session and the sessions Hermes continued it in
//! each time it compressed its context (`chains`): a child of a session
//! that ended `compression`, never a branch, a delegate, a reset or a
//! tool's run. Of several such children Hermes takes one, as its
//! `_CHAIN_STEP_SQL` orders them (a compressed one, then one still open,
//! then the most recently active); the export took the earliest.
//!
//! **Its kind**: `cron` for a scheduled job's run; `subagent`, a side
//! task, for a delegate's or a tool's run, its parent gone or not (Hermes'
//! picker leaves out every `_delegate_from` row, and a deleted session's
//! children lose their parent); `chat` for a session with no parent, a
//! branch, or the new conversation a reset starts (what Hermes' session
//! picker lists); else `subagent` (a continuation Hermes passed over, which
//! it shows nowhere: kept, as it may hold messages, and folded away).
//!
//! **Its messages** are what Hermes shows a person (its display read):
//! rows that are live or compacted into history, never the ones it took
//! back (an undo, the originals of a tail it carried forward); in the
//! order it wrote them (by id: "clocks regress"), each once across the
//! conversation's sessions (the tail a compression copies into the next
//! is the same message: `dedupe`); none Hermes keeps for the model alone
//! (`model_only`, `display_kind: hidden`) but the summaries it wrote when
//! it compressed, kept as summaries, and the words a summary carried
//! (`split_summary`). The export read rows by timestamp, kept duplicates,
//! and read Hermes' structured content (`\0json:`) as raw text.
//!
//! **Bounded by what is kept.** SQLite hands over only what a
//! conversation keeps of each row (`kept_content`, `kept_calls`): a
//! structured message's text parts and a mark for each medium (never a
//! picture's data), each tool call's id, name and the start of its
//! arguments, a tool's output and reasoning their cut's worth, each with
//! its whole length, so a cut says how much more there was. A conversation
//! whose kept rows pass twice what one may hold is refused alone, as soon
//! as they do; a database with more sessions than `SESSIONS_READ_MAX`
//! whole: a multi-GB `state.db` is read in bounded memory. Each
//! conversation goes to the import's spool as it is read.
//!
//! **Not this computer's own.** On a platform computer (the CLI acting as
//! an agent), a database under the computer's Hermes root (any of its
//! agents', `$HERMES_HOME` or its `profiles/`) is refused: those
//! conversations are chats here already. A person's own Hermes elsewhere
//! (their laptop's, which sets `$HERMES_HOME` too) is theirs to import.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use fragment_core::session::{self, Kind, Message, Session, ToolCall, ARGUMENTS_MAX_CHARS, REASONING_MAX_CHARS, SESSION_MAX_BYTES, TOOL_TEXT_MAX_CHARS};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::Spool;

/// The origin Hermes' conversations are keyed under.
pub const ORIGIN: &str = "hermes";
/// Which read this is (`Session::reader`): better than the export's (10),
/// whose duplicated tails it never keeps.
pub const READER: u32 = 20;
/// Sessions a database may hold to be read (each a few hundred bytes read
/// before its chains are known): past it, refused whole.
const SESSIONS_READ_MAX: usize = 200_000;
/// What SQLite hands over of a tool's output and of reasoning, in bytes,
/// and of a text part or a call's arguments, in characters: their cut's
/// worth, however wide (`cut_as_clipped` says how many more there were,
/// from the whole's length).
const CUT_READ_BYTES: usize = 4 * TOOL_TEXT_MAX_CHARS;
const CUT_READ_CHARS: usize = 2 * TOOL_TEXT_MAX_CHARS;
const ARGUMENTS_READ_CHARS: usize = 2 * ARGUMENTS_MAX_CHARS;
/// A session's `model_config`, in bytes (it holds only markers here).
const CONFIG_READ_BYTES: usize = 4096;
/// What a row read holds besides its strings' bytes (`MessageRow` itself),
/// counted toward what its conversation may hold, so empty rows count too.
const ROW_HELD_BYTES: usize = std::mem::size_of::<MessageRow>();

/// Hermes' end reasons after which a child with the same routing key is a
/// new conversation (`_RESET_END_REASONS`, hermes_state_common.py).
const RESET_END_REASONS: [&str; 6] = ["session_reset", "session_switch", "idle", "daily", "suspended", "resume_pending_expired"];
/// The times Hermes trusts (`coerce_epoch`'s window): a cell outside it
/// (salvaged from a damaged page) is no time.
const EPOCH_MAX: f64 = 4_200_000_000.0;
/// How far a chain is walked (`_CHAIN_CAP`): a loop stops sooner.
const CHAIN_MAX: usize = 1_000;
/// Where a compaction summary ends and the words after it begin, and
/// where the words a summary carried before it end (context_compressor.py).
const SUMMARY_END: &str = "--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---";
const PRIOR_CONTEXT: &str = "[PRIOR CONTEXT — for reference only; not a new message]";
const PRIOR_CONTEXT_END: &str = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]";

/// A session row: what a conversation is made from.
#[derive(Debug, Clone, Default)]
struct Row {
    id: String,
    source: Option<String>,
    session_key: Option<String>,
    display_name: Option<String>,
    model: Option<String>,
    model_config: Option<Value>,
    parent: Option<String>,
    started_at: Option<f64>,
    ended_at: Option<f64>,
    end_reason: Option<String>,
    title: Option<String>,
    last_activity_at: Option<f64>,
    /// the Hermes profile it is (an agent's, in a home of several)
    profile: Option<String>,
    /// its newest message's time (in the window)
    last_message_at: Option<f64>,
}

impl Row {
    /// A marker Hermes writes in a child's `model_config` as it makes it:
    /// present when it names any value (json_extract's `IS NOT NULL`).
    fn marker(&self, key: &str) -> bool {
        self.model_config.as_ref().and_then(|c| c.get(key)).is_some_and(|v| !v.is_null())
    }

    /// The marker names `parent`: this session forked from it. A marker
    /// naming another session is one a continuation inherited (Hermes
    /// copies `model_config` onto a continuation), not a fork of this
    /// parent (`_non_continuation_child_sql`, `_is_explicit_fork_child_row`).
    fn forked_from(&self, key: &str, parent: &str) -> bool {
        self.model_config.as_ref().and_then(|c| c.get(key)).and_then(Value::as_str) == Some(parent)
    }

    fn end_reason_is(&self, reason: &str) -> bool {
        self.end_reason.as_deref() == Some(reason)
    }

    /// When it was last active: its newest message or heartbeat, else its
    /// start (`_sql_session_last_active`).
    fn last_active(&self) -> Option<f64> {
        let fresh = [self.last_activity_at, self.last_message_at].into_iter().flatten().filter(|t| in_window(*t)).fold(None, |m: Option<f64>, t| Some(m.map_or(t, |m| m.max(t))));
        fresh.or(self.started_at.filter(|t| in_window(*t)))
    }
}

fn in_window(t: f64) -> bool {
    (0.0..=EPOCH_MAX).contains(&t)
}

/// A branch of `parent` (`_BRANCH_CHILD_SQL`).
fn is_branch(s: &Row, parent: &Row) -> bool {
    s.marker("_branched_from") || (parent.end_reason_is("branched") && matches!((s.started_at, parent.ended_at), (Some(a), Some(b)) if a >= b))
}

/// The new conversation a reset of `parent` started (`_RESET_CHILD_SQL`).
fn is_reset(s: &Row, parent: &Row) -> bool {
    let key = s.session_key.as_deref().filter(|k| !k.is_empty());
    s.marker("_reset_from") || (parent.end_reason.as_deref().is_some_and(|r| RESET_END_REASONS.contains(&r)) && key.is_some() && key == parent.session_key.as_deref())
}

/// A continuation of `parent` after it compressed (`_CHAIN_STEP_SQL`), its
/// fork markers bound to the parent, as Hermes' pruning and display
/// lineage bind them: a branch, a delegate or a reset of it is not one.
fn is_continuation(s: &Row, parent: &Row) -> bool {
    let forked = ["_branched_from", "_delegate_from", "_reset_from"].iter().any(|k| s.forked_from(k, &parent.id));
    let legacy_reset = !s.marker("_reset_from") && is_reset(s, parent);
    parent.end_reason_is("compression") && !forked && !legacy_reset && s.source.as_deref() != Some("tool")
}

/// Which of a parent's continuations Hermes follows: the first by this
/// key (`_CHAIN_STEP_SQL`'s ORDER BY: a compressed one, then one still
/// open, then the rest; the most recently active; the latest started; the
/// greatest id). SQLite's NULLs sort last when descending.
type Preference = (u8, Reverse<Option<Ordered>>, Reverse<Option<Ordered>>, Reverse<String>);

fn preference(s: &Row) -> Preference {
    let class = if s.end_reason_is("compression") {
        0
    } else if s.ended_at.is_none() {
        1
    } else {
        2
    };
    (class, Reverse(s.last_active().map(Ordered)), Reverse(s.started_at.map(Ordered)), Reverse(s.id.clone()))
}

/// An f64 that orders (times are finite: NaN reads as no time).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ordered(f64);
impl Eq for Ordered {}
impl PartialOrd for Ordered {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ordered {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// The conversations: each session with the ones Hermes continued it in,
/// in order, from the sessions in `started_at, id` order. A session in no
/// chain (a loop of continuations, which Hermes never writes) is one of
/// its own.
fn chains(rows: &[Row]) -> Vec<Vec<usize>> {
    let at: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.id.as_str(), i)).collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, s) in rows.iter().enumerate() {
        if let Some(&p) = s.parent.as_deref().and_then(|p| at.get(p)) {
            if p != i && is_continuation(s, &rows[p]) {
                children.entry(p).or_default().push(i);
            }
        }
    }
    let next: HashMap<usize, usize> = children.into_iter().map(|(p, kids)| (p, *kids.iter().min_by_key(|&&k| preference(&rows[k])).expect("a parent with children"))).collect();
    let continued: HashSet<usize> = next.values().copied().collect();
    let mut covered = vec![false; rows.len()];
    let mut out = Vec::new();
    for i in 0..rows.len() {
        if continued.contains(&i) {
            continue;
        }
        let mut chain = vec![i];
        let mut seen: HashSet<usize> = HashSet::from([i]);
        while let Some(&n) = next.get(chain.last().expect("a chain is never empty")) {
            if chain.len() >= CHAIN_MAX || !seen.insert(n) {
                break;
            }
            chain.push(n);
        }
        for &c in &chain {
            covered[c] = true;
        }
        out.push(chain);
    }
    out.extend((0..rows.len()).filter(|&i| !covered[i]).map(|i| vec![i]));
    out
}

/// What a conversation is (the module's doc, "Its kind").
fn kind(root: &Row, parent: Option<&Row>) -> Kind {
    if root.source.as_deref() == Some("cron") {
        return Kind::Cron;
    }
    if root.marker("_delegate_from") || root.source.as_deref() == Some("tool") {
        return Kind::Subagent;
    }
    match parent {
        None => Kind::Chat,
        Some(p) if is_branch(root, p) || is_reset(root, p) => Kind::Chat,
        Some(_) => Kind::Subagent,
    }
}

/// A message row: what Hermes stored of one.
#[derive(Debug, Clone, Default)]
struct MessageRow {
    id: i64,
    role: String,
    /// What a conversation keeps of its content (`kept_content`), and the
    /// whole of that in bytes (`kept_length`).
    content: Option<String>,
    content_whole: Option<i64>,
    tool_call_id: Option<String>,
    tool_calls: Option<String>,
    tool_name: Option<String>,
    timestamp: Option<f64>,
    reasoning: Option<String>,
    active: bool,
    compacted: bool,
    summary: bool,
    display_kind: Option<String>,
    model_only: bool,
}

impl MessageRow {
    /// What it holds, counted toward its conversation's bound: itself and
    /// every string it read.
    fn held(&self) -> usize {
        let len = |s: &Option<String>| s.as_ref().map_or(0, String::len);
        ROW_HELD_BYTES + self.role.len() + len(&self.content) + len(&self.tool_call_id) + len(&self.tool_calls) + len(&self.tool_name) + len(&self.reasoning) + len(&self.display_kind)
    }
}

/// The key one message is known by across a compaction's copies of it
/// (`_display_dedupe_key`): an assistant's tool calls by their ids, so a
/// copy whose arguments a prune cut is still the same call; any other
/// message by all it holds: (role, content, time, tool_call_id,
/// tool_calls, tool_name).
type DedupeKey = (String, Option<String>, Option<u64>, Option<String>, Option<String>, Option<String>);

fn dedupe_key(m: &MessageRow) -> DedupeKey {
    let at = m.timestamp.map(f64::to_bits);
    if m.role == "assistant" {
        let ids: Vec<String> = serde_json::from_str::<Value>(m.tool_calls.as_deref().unwrap_or("[]"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|c| ["call_id", "id"].iter().find_map(|k| c[*k].as_str().map(str::trim).filter(|s| !s.is_empty())).map(|s| s.split('|').next().unwrap_or(s).trim().to_string()).unwrap_or_default())
            .collect();
        if !ids.is_empty() && ids.iter().all(|i| !i.is_empty()) {
            return ("assistant".into(), None, at, m.tool_call_id.clone(), Some(ids.join("\u{0}")), m.tool_name.clone());
        }
    }
    (m.role.clone(), m.content.clone(), at, m.tool_call_id.clone(), m.tool_calls.clone(), m.tool_name.clone())
}

/// Each message once (`_dedupe_display_generations`): of the copies a
/// compaction made of one, the live one, else the newest; placed where
/// its first copy was. `rows` are in id order.
fn dedupe(rows: Vec<MessageRow>) -> Vec<MessageRow> {
    let mut chosen: BTreeMap<_, MessageRow> = BTreeMap::new();
    let mut first: HashMap<_, i64> = HashMap::new();
    for m in rows {
        let key = dedupe_key(&m);
        first.entry(key.clone()).or_insert(m.id);
        match chosen.get(&key) {
            Some(held) if (held.active, held.id) >= (m.active, m.id) => {}
            _ => {
                chosen.insert(key, m);
            }
        }
    }
    let mut out: Vec<(i64, MessageRow)> = chosen.into_iter().map(|(k, m)| (first[&k], m)).collect();
    out.sort_by_key(|(id, _)| *id);
    out.into_iter().map(|(_, m)| m).collect()
}

/// A message's content as text: Hermes' structured content (`\0json:`,
/// or a list of parts as older Hermes stored it) as its text and a mark
/// for each medium (`session::parts_text`).
fn content_text(content: &str) -> String {
    if content.starts_with("\u{0}json:") {
        return session::text_of(content);
    }
    if content.trim_start().starts_with('[') {
        if let Some(text) = serde_json::from_str::<Value>(content).ok().as_ref().and_then(session::parts_text) {
            return text;
        }
    }
    content.to_string()
}

/// A summary row's text as Hermes shows it: the words a person said that
/// it carried after its end (`_SUMMARY_END_MARKER`), or the earlier
/// message it carried before it (`_MERGED_SUMMARY_DELIMITER`), apart
/// from the summary: (before, summary, after).
fn split_summary(text: &str) -> (Option<String>, String, Option<String>) {
    let some = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some((prior, summary)) = text.split_once(PRIOR_CONTEXT_END) {
        let prior = prior.trim().strip_prefix(PRIOR_CONTEXT).unwrap_or(prior.trim());
        let (summary, after) = match summary.split_once(SUMMARY_END) {
            Some((s, after)) => (format!("{}{SUMMARY_END}", s.trim_start()), some(after)),
            None => (summary.trim().to_string(), None),
        };
        return (some(prior), summary, after);
    }
    match text.split_once(SUMMARY_END) {
        Some((s, after)) => (None, format!("{s}{SUMMARY_END}"), some(after)),
        None => (None, text.to_string(), None),
    }
}

/// A message as an archive keeps it, or none (a row with nothing in it).
fn message(m: &MessageRow, text: Option<String>, summary: bool) -> Message {
    let calls: Vec<ToolCall> = m
        .tool_calls
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let f = &c["function"];
            let arguments = match &f["arguments"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let id = ["call_id", "id"].iter().find_map(|k| c[*k].as_str().map(str::trim).filter(|s| !s.is_empty())).map(str::to_string);
            // SQLite handed over their start (`kept_calls`) and how long they were
            let whole = c["chars"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
            let arguments = if whole > ARGUMENTS_MAX_CHARS && arguments.chars().count() < whole {
                format!("{}… [{} more characters not kept]", arguments.chars().take(ARGUMENTS_MAX_CHARS).collect::<String>(), whole - ARGUMENTS_MAX_CHARS)
            } else {
                arguments
            };
            ToolCall { id, name: f["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("tool").to_string(), arguments }
        })
        .collect();
    Message {
        role: if m.role.is_empty() { "unknown".into() } else { m.role.clone() },
        at: m.timestamp.and_then(session::iso),
        text,
        tool: m.tool_name.clone(),
        call: (m.role == "tool").then(|| m.tool_call_id.clone()).flatten(),
        tool_calls: calls,
        reasoning: m.reasoning.clone(),
        summary,
    }
}

/// The messages of a conversation's rows, as the module's doc says.
fn messages(rows: Vec<MessageRow>) -> Vec<Message> {
    let shown: Vec<MessageRow> = rows.into_iter().filter(|m| (m.active || m.compacted) && !m.model_only).collect();
    let mut out = Vec::new();
    for m in dedupe(shown) {
        let text = m.content.as_deref().map(content_text).filter(|t| !t.is_empty());
        // a tool's output, as SQLite cut it: cut as `clip` cuts, its whole's length said
        let text = if m.role == "tool" { cut_as_clipped(text, m.content_whole, TOOL_TEXT_MAX_CHARS) } else { text };
        if m.summary {
            let (before, summary, after) = split_summary(text.as_deref().unwrap_or_default());
            if before.is_some() {
                out.push(message(&MessageRow { tool_calls: None, reasoning: None, ..m.clone() }, before, false));
            }
            out.push(message(&MessageRow { tool_calls: None, reasoning: None, ..m.clone() }, Some(summary), true));
            if after.is_some() {
                out.push(message(&MessageRow { tool_calls: None, reasoning: None, ..m.clone() }, after, false));
            }
            continue;
        }
        if m.display_kind.as_deref() == Some("hidden") {
            continue;
        }
        out.push(message(&m, text, false));
    }
    out
}

/// The conversations in the Hermes database at `path`, each into `spool`
/// as it is read. What it could not read, each in a sentence (the rest is
/// read); a database too large, or under `own` (this platform computer's
/// Hermes home, when the CLI acts as its agent), is refused.
pub fn read(path: &Path, spool: &mut Spool, own: Option<&Path>) -> Result<()> {
    if let Some(home) = own {
        refuse_own(path, home)?;
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("opening {} read-only", path.display()))?;
    // one read transaction: a Hermes writing meanwhile is read as of now
    db.execute_batch("BEGIN").with_context(|| format!("reading {}", path.display()))?;
    let read = read_in(&db, spool).with_context(|| format!("reading {} (is it Hermes' state.db?)", path.display()));
    let _ = db.execute_batch("COMMIT");
    read
}

/// A platform computer's own Hermes databases are its agents'
/// conversations here, which are chats here already: importing them would
/// show each twice. Its Hermes root is `home` (`$HERMES_HOME`), or the
/// folder holding its `profiles/` when `home` is one of them.
fn refuse_own(db: &Path, home: &Path) -> Result<()> {
    let root = match home.parent() {
        Some(p) if p.file_name().is_some_and(|n| n == "profiles") => p.parent().map(Path::to_path_buf).unwrap_or(home.to_path_buf()),
        _ => home.to_path_buf(),
    };
    if let (Ok(db), Ok(root)) = (db.canonicalize(), root.canonicalize()) {
        if db.starts_with(&root) {
            bail!(super::usage(format!("{} is one of this computer's own Hermes databases: its conversations are your chats here already. Import another agent's (its state.db, or its export)", db.display())));
        }
    }
    Ok(())
}

/// A table's columns, so a database from an older Hermes (one without
/// `compacted`, say) is read with each missing one as its default.
fn columns(db: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut q = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = q.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<HashSet<String>>>()?;
    anyhow::ensure!(!names.is_empty(), "it has no {table} table");
    Ok(names)
}

/// A column of `have`'s, quoted, or `NULL` when the table lacks it.
fn col(have: &HashSet<String>, name: &str) -> String {
    if have.contains(name) {
        format!("\"{name}\"")
    } else {
        "NULL".to_string()
    }
}

fn text(v: &Sql) -> Option<String> {
    match v {
        Sql::Text(s) => Some(s.clone()),
        Sql::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
        Sql::Integer(i) => Some(i.to_string()),
        Sql::Real(f) => Some(f.to_string()),
        Sql::Null => None,
    }
}

fn real(v: &Sql) -> Option<f64> {
    match v {
        Sql::Integer(i) => Some(*i as f64),
        Sql::Real(f) if f.is_finite() => Some(*f),
        Sql::Text(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn flag(v: &Sql, default: bool) -> bool {
    match v {
        Sql::Null => default,
        Sql::Integer(i) => *i != 0,
        other => real(other).map_or(default, |f| f != 0.0),
    }
}

/// `expr`, or its first `max` bytes when it is longer: SQLite cuts a long
/// value as bytes (its text functions stop at a NUL, which Hermes'
/// `\0json:` content starts with), and hands a short one over whole.
fn bounded(expr: &str, max: usize) -> String {
    format!("CASE WHEN length(CAST({expr} AS BLOB)) > {max} THEN substr(CAST({expr} AS BLOB), 1, {max}) ELSE {expr} END")
}

/// Hermes' structured content (`\0json:` and its JSON, or a list of parts
/// as an older Hermes stored it) of `c`, as JSON text; NULL for plain text.
fn structured(c: &str) -> String {
    format!("(CASE WHEN substr(CAST({c} AS BLOB), 1, 6) = X'006A736F6E3A' THEN CAST(substr(CAST({c} AS BLOB), 7) AS TEXT) WHEN substr(CAST({c} AS BLOB), 1, 1) = X'5B' THEN {c} END)")
}

/// Whether the JSON list `j` is a list of typed parts: not empty, each an
/// object whose `type` is a string. An element that is no object (the
/// strings of a person's `["red", "blue"]`) is never handed to an object's
/// JSON function, which fails on it and with it the row (`CASE` is
/// evaluated in order; `AND` and `OR` need not be).
fn typed_parts(j: &str) -> String {
    format!(
        "(json_array_length({j}) > 0 AND (SELECT count(*) FROM json_each({j}) p \
           WHERE (CASE WHEN p.type = 'object' THEN json_type(p.value, '$.type') END) IS NOT 'text') = 0)"
    )
}

/// What a conversation keeps of a message's content `c`, at most `chars`
/// of each text: of structured content (valid JSON: a list of typed parts,
/// `typed_parts`; or a string), its text parts and each medium's type,
/// never its data (a picture's base64), as `\0json:` and that JSON
/// (`content_text` reads it); anything else (a list that is no parts
/// among it) as it is, at most `bytes` (`bounded`).
fn kept_content(c: &str, chars: usize, bytes: usize) -> String {
    let j = structured(c);
    let parts = format!(
        "(SELECT json_group_array(CASE WHEN json_extract(p.value, '$.type') IN ('text', 'input_text', 'output_text') \
           THEN json_object('type', 'text', 'text', substr(json_extract(p.value, '$.text'), 1, {chars})) \
           ELSE json_object('type', json_extract(p.value, '$.type')) END) FROM json_each({j}) p)"
    );
    let typed = typed_parts(&j);
    let raw = bounded(c, bytes);
    format!(
        "CASE WHEN {j} IS NULL THEN {raw} WHEN NOT json_valid({j}) THEN {raw} \
         ELSE CASE json_type({j}) \
           WHEN 'array' THEN CASE WHEN {typed} THEN char(0) || 'json:' || {parts} ELSE {raw} END \
           WHEN 'text' THEN char(0) || 'json:' || json_quote(substr(json_extract({j}, '$'), 1, {chars})) \
           ELSE {raw} END END"
    )
}

/// The whole length in bytes of what `kept_content` keeps of `c`: its text
/// parts', or its own.
fn kept_length(c: &str) -> String {
    let j = structured(c);
    let own = format!("length(CAST({c} AS BLOB))");
    let typed = typed_parts(&j);
    format!(
        "CASE WHEN {j} IS NULL THEN {own} WHEN NOT json_valid({j}) THEN {own} WHEN json_type({j}) IS NOT 'array' THEN {own} \
           WHEN {typed} THEN (SELECT coalesce(sum(length(CAST(json_extract(p.value, '$.text') AS BLOB))), 0) FROM json_each({j}) p) \
           ELSE {own} END"
    )
}

/// What a conversation keeps of a row's `tool_calls` `t`: each call's ids,
/// its name, the start of its arguments and their whole length (`chars`),
/// of each entry that is an object (`WHERE` is applied before the row's
/// values are made); NULL when it is no list.
fn kept_calls(t: &str) -> String {
    format!(
        "CASE WHEN {t} IS NULL OR NOT json_valid({t}) THEN NULL ELSE CASE json_type({t}) WHEN 'array' THEN \
           (SELECT json_group_array(json_object('id', json_extract(c.value, '$.id'), 'call_id', json_extract(c.value, '$.call_id'), \
             'function', json_object('name', json_extract(c.value, '$.function.name'), \
               'arguments', substr(json_extract(c.value, '$.function.arguments'), 1, {ARGUMENTS_READ_CHARS})), \
             'chars', length(json_extract(c.value, '$.function.arguments')))) FROM json_each({t}) c WHERE c.type = 'object') \
         END END"
    )
}

/// A row's text as SQLite handed it over (`bounded`), its whole length in
/// bytes beside it: one cut there is cut as `session::clip` cuts, saying
/// how many more there were (in bytes past the kept characters' own: a
/// character's count where the text is ASCII); a whole one is left to
/// `clip`.
fn cut_as_clipped(text: Option<String>, whole: Option<i64>, max: usize) -> Option<String> {
    let text = text?;
    let whole = whole.and_then(|c| usize::try_from(c).ok()).unwrap_or(0);
    if whole <= text.len() {
        return Some(text);
    }
    let head: String = text.chars().take(max).collect();
    Some(format!("{head}… [{} more characters not kept]", whole.saturating_sub(head.len())))
}

/// A message row's `display_metadata`, as Hermes reads it: an object, or
/// one Hermes once wrote as a JSON string holding it.
fn metadata(raw: Option<String>) -> Option<Value> {
    let v: Value = serde_json::from_str(&raw?).ok()?;
    match v {
        Value::String(inner) => serde_json::from_str(&inner).ok(),
        v => Some(v),
    }
}

const SESSION_COLUMNS: [&str; 13] = ["id", "source", "session_key", "display_name", "model", "model_config", "parent_session_id", "started_at", "ended_at", "end_reason", "title", "last_activity_at", "profile_name"];

fn read_in(db: &Connection, spool: &mut Spool) -> Result<()> {
    let session_cols = columns(db, "sessions")?;
    let m = columns(db, "messages")?;
    anyhow::ensure!(session_cols.contains("id") && m.contains("session_id"), "its sessions and messages tables are not Hermes'");
    let count: i64 = db.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
    if count as usize > SESSIONS_READ_MAX {
        return Err(anyhow::Error::new(crate::api::CodedError {
            code: crate::api::Code::TooLarge,
            msg: format!("it holds {count} sessions, past the {SESSIONS_READ_MAX} an import reads"),
        }));
    }

    let mut select: Vec<String> = SESSION_COLUMNS.iter().map(|c| col(&session_cols, c)).collect();
    select[5] = bounded(&col(&session_cols, "model_config"), CONFIG_READ_BYTES);
    let mut q = db.prepare(&format!("SELECT {} FROM sessions ORDER BY started_at, id", select.join(", ")))?;
    let mut rows: Vec<Row> = q
        .query_map([], |r| {
            let v = |i: usize| r.get::<_, Sql>(i);
            Ok(Row {
                id: text(&v(0)?).unwrap_or_default(),
                source: text(&v(1)?),
                session_key: text(&v(2)?),
                display_name: text(&v(3)?),
                model: text(&v(4)?),
                model_config: text(&v(5)?).and_then(|c| serde_json::from_str::<Value>(&c).ok()).filter(Value::is_object),
                parent: text(&v(6)?).filter(|p| !p.is_empty()),
                started_at: real(&v(7)?),
                ended_at: real(&v(8)?),
                end_reason: text(&v(9)?),
                title: text(&v(10)?),
                last_activity_at: real(&v(11)?),
                profile: text(&v(12)?).filter(|p| !p.is_empty()),
                last_message_at: None,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.retain(|r| !r.id.is_empty());

    let mut q = db.prepare(&format!("SELECT session_id, MAX(timestamp) FROM messages WHERE timestamp BETWEEN 0 AND {EPOCH_MAX} GROUP BY session_id"))?;
    let newest: HashMap<String, f64> = q.query_map([], |r| Ok((text(&r.get::<_, Sql>(0)?), real(&r.get::<_, Sql>(1)?))))?.filter_map(|r| r.ok()).filter_map(|(s, t)| Some((s?, t?))).collect();
    for r in &mut rows {
        r.last_message_at = newest.get(&r.id).copied();
    }

    // each row as SQLite hands it over: at most what a conversation could
    // keep of it (a tool's output and reasoning their cut's worth), with
    // its whole length in bytes
    let (content, reasoning) = (col(&m, "content"), format!("COALESCE({}, {})", col(&m, "reasoning"), col(&m, "reasoning_content")));
    let message_select = [
        col(&m, "id"),
        format!("CASE WHEN {} = 'tool' THEN {} ELSE {} END", col(&m, "role"), kept_content(&content, CUT_READ_CHARS, CUT_READ_BYTES), kept_content(&content, SESSION_MAX_BYTES + 1, SESSION_MAX_BYTES + 1)),
        kept_length(&content),
        col(&m, "role"),
        col(&m, "tool_call_id"),
        kept_calls(&col(&m, "tool_calls")),
        col(&m, "tool_name"),
        col(&m, "timestamp"),
        bounded(&reasoning, CUT_READ_BYTES),
        format!("length(CAST({reasoning} AS BLOB))"),
        col(&m, "active"),
        col(&m, "compacted"),
        col(&m, "_compressed_summary"),
        col(&m, "display_kind"),
        bounded(&col(&m, "display_metadata"), CONFIG_READ_BYTES),
    ];
    let at: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.id.as_str(), i)).collect();
    let mut q = db.prepare(&format!("SELECT {} FROM messages WHERE session_id = ?1 ORDER BY id", message_select.join(", ")))?;
    for chain in chains(&rows) {
        let root = &rows[chain[0]];
        let last = &rows[*chain.last().expect("a chain is never empty")];
        let parent = root.parent.as_deref().and_then(|p| at.get(p)).map(|&i| &rows[i]);
        let mut found = Vec::new();
        let mut held = 0usize;
        // why a session of it could not be read whole: then none of it is
        let mut failed: Option<String> = None;
        'chain: for &i in &chain {
            let read = q.query_map([&rows[i].id], |r| {
                let v = |i: usize| r.get::<_, Sql>(i);
                let role = text(&v(3)?).unwrap_or_default();
                let content_chars = match v(2)? {
                    Sql::Integer(n) => Some(n),
                    _ => None,
                };
                let reasoning_chars = match v(9)? {
                    Sql::Integer(n) => Some(n),
                    _ => None,
                };
                Ok(MessageRow {
                    id: match v(0)? {
                        Sql::Integer(i) => i,
                        _ => 0,
                    },
                    role,
                    content: text(&v(1)?),
                    content_whole: content_chars,
                    tool_call_id: text(&v(4)?),
                    tool_calls: text(&v(5)?),
                    tool_name: text(&v(6)?),
                    timestamp: real(&v(7)?),
                    reasoning: cut_as_clipped(text(&v(8)?), reasoning_chars, REASONING_MAX_CHARS).filter(|r| !r.is_empty()),
                    active: flag(&v(10)?, true),
                    compacted: flag(&v(11)?, false),
                    summary: flag(&v(12)?, false),
                    display_kind: text(&v(13)?),
                    model_only: metadata(text(&v(14)?)).as_ref().map(|m| &m["model_only"]).is_some_and(|f| f.as_bool().unwrap_or(false) || f.as_i64().is_some_and(|i| i != 0)),
                })
            });
            let rows_read = match read {
                Ok(it) => it,
                Err(e) => {
                    failed = Some(format!("session {}: {e}", super::said_id(&rows[i].id)));
                    break 'chain;
                }
            };
            for m in rows_read {
                match m {
                    Ok(m) => {
                        held += m.held();
                        found.push(m);
                    }
                    Err(e) => {
                        failed = Some(format!("session {}: {e}", super::said_id(&rows[i].id)));
                        break 'chain;
                    }
                }
                // a conversation whose kept rows pass twice what one may
                // hold is refused, as soon as they do, its rows let go
                if held > 2 * SESSION_MAX_BYTES {
                    break 'chain;
                }
            }
        }
        if let Some(why) = failed {
            spool.refused.push(super::RefusedOne { id: super::said_id(&root.id), why: format!("it could not be read whole ({why})") });
            continue;
        }
        if held > 2 * SESSION_MAX_BYTES {
            spool.refused.push(super::RefusedOne { id: super::said_id(&root.id), why: session::Refused::TooLarge(held).to_string() });
            continue;
        }
        found.sort_by_key(|m| m.id);
        let title = chain.iter().find_map(|&i| rows[i].title.clone().filter(|t| !t.trim().is_empty())).unwrap_or_default();
        spool.add(Session {
            format: String::new(),
            id: root.id.clone(),
            origin: ORIGIN.to_string(),
            reader: READER,
            kind: kind(root, parent),
            agent: root.profile.clone(),
            title,
            source: root.source.clone(),
            with: root.display_name.clone(),
            model: last.model.clone().or_else(|| root.model.clone()),
            started_at: root.started_at.and_then(session::iso),
            ended_at: last.ended_at.or(last.last_activity_at).and_then(session::iso),
            parts: chain.iter().map(|&i| rows[i].id.clone()).collect(),
            messages: messages(found),
        })?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A database with the tables and columns of Hermes v0.21.7's that a
    /// read uses (hermes_state_common.py's SCHEMA_SQL, these columns only),
    /// so a test writes the rows Hermes writes.
    pub const SCHEMA: &str = "
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, session_key TEXT, display_name TEXT, model TEXT, model_config TEXT,
            parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, last_activity_at REAL, profile_name TEXT);
        CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT,
            tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL NOT NULL, reasoning TEXT, reasoning_content TEXT,
            _compressed_summary INTEGER NOT NULL DEFAULT 0, active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0,
            display_kind TEXT, display_metadata TEXT);";

    /// A synthetic Hermes database at `path`, made of `sql` after the schema.
    pub fn database(path: &Path, sql: &str) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(sql).unwrap();
    }

    /// What a spool holds, its conversations as their files say.
    pub fn spooled(spool: &Spool) -> Vec<Session> {
        spool.entries.keys().map(|k| serde_json::from_slice(&std::fs::read(spool.path(k)).unwrap()).unwrap()).collect()
    }

    fn read_path(path: &Path) -> (Spool, Vec<Session>) {
        let mut spool = Spool::new(BTreeSet::new()).unwrap();
        read(path, &mut spool, None).unwrap();
        let sessions = spooled(&spool);
        (spool, sessions)
    }

    fn read_sql(sql: &str) -> (Spool, Vec<Session>) {
        let path = super::super::tempdir().join("state.db");
        database(&path, sql);
        read_path(&path)
    }

    fn texts(s: &Session) -> Vec<String> {
        s.messages.iter().map(|m| format!("{}{}:{}", if m.summary { "summary " } else { "" }, m.role, m.text.clone().unwrap_or_default())).collect()
    }

    fn by_id<'a>(all: &'a [Session], id: &str) -> Option<&'a Session> {
        all.iter().find(|s| s.id == id)
    }

    /// Goal: text that is a JSON list but no list of typed parts (a person's
    /// `["red", "blue"]`, a mixed list, an empty one) is kept as the text it
    /// is, with the messages around it, and typed parts still read as
    /// text; a call list of anything but objects drops its odd entries, not
    /// the conversation. Valid and invalid lists, a call list of strings.
    #[test]
    fn json_lists_that_are_no_parts_are_kept_as_text() {
        let (spool, all) = read_sql(
            r#"
            INSERT INTO sessions VALUES ('s1','cli',NULL,NULL,'m',NULL,NULL,100,200,NULL,'Lists',NULL,NULL);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user','before',110);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user','["red", "blue"]',111);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','assistant','[{"type":"text","text":"x"}, "y"]',112);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user','[]',113);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','tool','[1, [2], null]',114);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user',char(0) || 'json:[{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]',115);
            INSERT INTO messages (session_id,role,content,tool_calls,timestamp) VALUES ('s1','assistant','calling','["web", {"id":"c1","function":{"name":"web","arguments":"{}"}}]',116);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','assistant','after',117);
            "#,
        );
        assert!(spool.refused.is_empty(), "{:?}", spool.refused.iter().map(|r| &r.why).collect::<Vec<_>>());
        let s = by_id(&all, "s1").expect("the conversation");
        assert_eq!(
            texts(s),
            ["user:before", "user:[\"red\", \"blue\"]", "assistant:[{\"type\":\"text\",\"text\":\"x\"}, \"y\"]", "user:[]", "tool:[1, [2], null]", "user:look\n[image]", "assistant:calling", "assistant:after"]
        );
        assert_eq!(s.messages[6].tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["web"], "the call that is one");
    }

    /// Goal: a conversation SQLite cannot read whole is refused whole (its
    /// root's session or a continuation's), never kept as the part read
    /// before the failure, and the next conversation is read. Method: a
    /// view of the messages whose content fails for one row, in a
    /// continuation of one conversation; a second conversation beside it.
    #[test]
    fn a_conversation_that_cannot_be_read_whole_is_refused_whole() {
        let path = super::super::tempdir().join("state.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&SCHEMA.replace("CREATE TABLE messages ", "CREATE TABLE stored ")).unwrap();
        db.execute_batch(
            r#"
            CREATE VIEW messages AS SELECT id, session_id, role,
                CASE WHEN content = 'unreadable' THEN json_extract('not json', '$') ELSE content END AS content,
                tool_call_id, tool_calls, tool_name, timestamp, reasoning, reasoning_content, _compressed_summary, active, compacted, display_kind, display_metadata
              FROM stored;
            INSERT INTO sessions VALUES ('s1','cli',NULL,NULL,'m',NULL,NULL,100,200,'compression','Cut',NULL,NULL);
            INSERT INTO sessions VALUES ('s2','cli',NULL,NULL,'m','{}','s1',200,300,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('ok','cli',NULL,NULL,'m',NULL,NULL,400,500,NULL,'Fine',NULL,NULL);
            INSERT INTO stored (session_id,role,content,timestamp) VALUES ('s1','user','before',110);
            INSERT INTO stored (session_id,role,content,timestamp) VALUES ('s2','user','unreadable',210);
            INSERT INTO stored (session_id,role,content,timestamp) VALUES ('s2','user','after',220);
            INSERT INTO stored (session_id,role,content,timestamp) VALUES ('ok','user','hello',410);
            "#,
        )
        .unwrap();
        drop(db);
        let (spool, all) = read_path(&path);
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["ok"], "{all:#?}");
        assert_eq!(spool.refused.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["s1"]);
        assert!(spool.refused[0].why.contains("could not be read"), "{:?}", spool.refused[0].why);
    }

    /// Goal: every row a conversation reads counts toward what it may hold,
    /// so rows with nothing in them cannot pass the bound. Method: a
    /// session of more empty rows than the bound has room for.
    #[test]
    fn empty_rows_count_toward_what_a_conversation_holds() {
        let rows = 2 * SESSION_MAX_BYTES / ROW_HELD_BYTES + 10;
        let (spool, all) = read_sql(&format!(
            "INSERT INTO sessions VALUES ('e','cli',NULL,NULL,'m',NULL,NULL,100,200,NULL,'Empty',NULL,NULL);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {rows})
             INSERT INTO messages (session_id,role,content,timestamp) SELECT 'e','assistant',NULL,100 FROM n;"
        ));
        assert!(all.is_empty());
        assert_eq!(spool.refused.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["e"]);
    }

    /// Goal: a conversation Hermes compressed twice is one conversation,
    /// its messages once each in the order Hermes wrote them. Method: a
    /// root, a continuation that compressed again, and its continuation;
    /// a stale sibling continuation (a reaped socket's) beside the one
    /// Hermes follows; the tail each compression copies; a summary with the
    /// person's next words after it; a row taken back; a model-only row,
    /// one of them its metadata double-encoded as an older Hermes wrote it.
    #[test]
    fn a_compressed_conversation_is_one_with_each_message_once() {
        let (spool, all) = read_sql(
            r#"
            INSERT INTO sessions VALUES ('s1','telegram','tg:1','Ann','m1',NULL,NULL,100,200,'compression','Garden',NULL,'juniper');
            INSERT INTO sessions VALUES ('s2','telegram','tg:1','Ann','m1','{}','s1',200,300,'compression',NULL,NULL,'juniper');
            INSERT INTO sessions VALUES ('stale','telegram','tg:1','Ann','m1','{}','s1',201,205,'ws_orphan_reap',NULL,NULL,'juniper');
            INSERT INTO sessions VALUES ('s3','telegram','tg:1','Ann','m2','{}','s2',300,NULL,NULL,NULL,450,'juniper');
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user','plant beans',110);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','assistant','in spring',120);
            INSERT INTO messages (session_id,role,content,timestamp,_compressed_summary,display_kind) VALUES ('s2','user','We talked beans. --- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---
and peas?',210,1,'hidden');
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s2','assistant','in spring',120);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s2','assistant','peas too',220);
            INSERT INTO messages (session_id,role,content,timestamp,active) VALUES ('s2','user','undone',230,0);
            INSERT INTO messages (session_id,role,content,timestamp,display_metadata) VALUES ('s2','user','for the model',231,'{"model_only":true}');
            INSERT INTO messages (session_id,role,content,timestamp,display_metadata) VALUES ('s2','user','for the model too',232,'"{\"model_only\": true}"');
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('stale','user','stray',202);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s3','user','thanks',400);
            "#,
        );
        assert!(spool.refused.is_empty(), "{:?}", spool.refused.iter().map(|r| &r.why).collect::<Vec<_>>());
        let chats: Vec<&Session> = all.iter().filter(|s| s.kind == Kind::Chat).collect();
        assert_eq!(chats.len(), 1, "{all:#?}");
        let c = chats[0];
        assert_eq!((c.id.as_str(), c.title.as_str(), c.parts.clone(), c.reader), ("s1", "Garden", vec!["s1".to_string(), "s2".into(), "s3".into()], READER));
        assert_eq!((c.model.as_deref(), c.agent.as_deref()), (Some("m2"), Some("juniper")), "the model it ended on, the profile it was");
        assert_eq!(
            texts(c),
            [
                "user:plant beans",
                "assistant:in spring",
                "summary user:We talked beans. --- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---",
                "user:and peas?",
                "assistant:peas too",
                "user:thanks"
            ],
            "the tail copied into s2 is shown once, where it was first said"
        );
        assert_eq!(c.ended_at.as_deref(), session::iso(400.0).as_deref(), "it ends when it was last said");
        assert_eq!(by_id(&all, "stale").map(|s| s.kind), Some(Kind::Subagent), "a continuation Hermes passed over is kept, as a side task");
    }

    /// Goal: what a session is follows Hermes' markers. Valid: a branch and
    /// a reset are chats of their own, a cron run is `cron`, a delegate and
    /// a tool's run are side tasks, their parent gone or not (a deleted
    /// chat's delegates lose their parent). Invalid: a branch or a reset of
    /// a compressed session is never its continuation, and a reset that
    /// compressed is one conversation of its own.
    #[test]
    fn branches_resets_runs_and_side_tasks_are_what_hermes_says() {
        let (_, all) = read_sql(
            r#"
            INSERT INTO sessions VALUES ('root','cli','k',NULL,NULL,NULL,NULL,100,200,'compression',NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('cont','cli','k',NULL,NULL,'{}','root',200,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('branch','cli','k',NULL,NULL,'{"_branched_from":"root"}','root',201,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('reset','cli','k',NULL,NULL,'{"_reset_from":"root"}','root',202,250,'compression',NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('reset2','cli','k',NULL,NULL,'{"_reset_from":"root"}','reset',250,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('old','telegram','tg',NULL,NULL,NULL,NULL,10,20,'idle',NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('old2','telegram','tg',NULL,NULL,NULL,'old',30,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('job','cron',NULL,NULL,NULL,NULL,NULL,50,60,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('dlg','cli',NULL,NULL,NULL,'{"_delegate_from":"cont"}','cont',210,220,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('tool','tool',NULL,NULL,NULL,NULL,'cont',211,221,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('orphan','cli',NULL,NULL,NULL,'{"_delegate_from":"deleted"}',NULL,212,222,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('orphan_tool','tool',NULL,NULL,NULL,NULL,NULL,213,223,NULL,NULL,NULL,NULL);
            "#,
        );
        let kind_of = |id: &str| by_id(&all, id).map(|s| (s.kind, s.parts.clone()));
        assert_eq!(kind_of("root"), Some((Kind::Chat, vec!["root".into(), "cont".into()])));
        assert_eq!(kind_of("branch").map(|k| k.0), Some(Kind::Chat));
        assert_eq!(kind_of("reset"), Some((Kind::Chat, vec!["reset".into(), "reset2".into()])), "a reset that compressed goes on in its continuation, though it carries the marker");
        assert_eq!(kind_of("old2").map(|k| k.0), Some(Kind::Chat), "a child on its parent's routing key after an idle reset is a new chat");
        assert_eq!(kind_of("job").map(|k| k.0), Some(Kind::Cron));
        for side in ["dlg", "tool", "orphan", "orphan_tool"] {
            assert_eq!(kind_of(side).map(|k| k.0), Some(Kind::Subagent), "{side}");
        }
        assert_eq!(kind_of("cont"), None, "a continuation is its conversation's, not one of its own");
        assert_eq!(all.len(), 10);
    }

    /// Goal: Hermes' stored forms read as text, and a tool's output and
    /// reasoning come cut, saying how much more there was. Method:
    /// structured content with an image (`\0json:`), a legacy list of
    /// parts, tool calls with their ids, a tool's long result naming its
    /// call, long reasoning.
    #[test]
    fn stored_content_reads_as_text_and_calls() {
        let long = "r".repeat(TOOL_TEXT_MAX_CHARS * 5);
        let (_, all) = read_sql(&format!(
            r#"
            INSERT INTO sessions VALUES ('s','cli',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user',char(0) || 'json:[{{"type":"text","text":"see"}},{{"type":"image_url","image_url":{{"url":"data:x"}}}}]',2);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user','[{{"type":"input_text","text":"old"}}]',3);
            INSERT INTO messages (session_id,role,content,timestamp,tool_calls,reasoning) VALUES ('s','assistant','',4,'[{{"id":"c1","function":{{"name":"web","arguments":"{{\"q\":1}}"}}}}]','{long}');
            INSERT INTO messages (session_id,role,content,timestamp,tool_name,tool_call_id) VALUES ('s','tool','{long}',5,'web','c1');
            "#
        ));
        let s = &all[0];
        assert_eq!(s.messages[0].text.as_deref(), Some("see\n[image]"));
        assert_eq!(s.messages[1].text.as_deref(), Some("old"));
        assert_eq!(s.messages[2].tool_calls, vec![ToolCall { id: Some("c1".into()), name: "web".into(), arguments: "{\"q\":1}".into() }]);
        let more = |n: usize| format!("… [{n} more characters not kept]");
        assert!(s.messages[2].reasoning.as_ref().is_some_and(|r| r.ends_with(&more(TOOL_TEXT_MAX_CHARS * 5 - REASONING_MAX_CHARS))), "its reasoning, cut, saying how much more");
        assert_eq!((s.messages[3].tool.as_deref(), s.messages[3].call.as_deref()), (Some("web"), Some("c1")));
        assert!(s.messages[3].text.as_ref().is_some_and(|t| t.ends_with(&more(TOOL_TEXT_MAX_CHARS * 4))), "{:?}", s.messages[3].text.as_ref().map(|t| t.len()));
        assert_eq!(s.messages[0].at.as_deref(), Some("1970-01-01T00:00:02.000Z"));
    }

    /// Goal: a conversation too large is refused alone, its rows let go,
    /// and a database past what an import reads is refused whole.
    #[test]
    fn too_much_is_refused() {
        let (spool, all) = read_sql(&format!(
            r#"
            INSERT INTO sessions VALUES ('big','cli',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('small','cli',NULL,NULL,NULL,NULL,NULL,2,NULL,NULL,NULL,NULL,NULL);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('big','user',hex(zeroblob({half})),2);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('big','user',hex(zeroblob({half})),3);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('big','user',hex(zeroblob({half})),4);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('small','user','hi',3);
            "#,
            half = SESSION_MAX_BYTES / 2
        ));
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["small"]);
        assert_eq!(spool.refused.len(), 1);
        assert_eq!(spool.refused[0].id, "big");

        let path = super::super::tempdir().join("state.db");
        database(&path, &format!("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i <= {SESSIONS_READ_MAX}) INSERT INTO sessions (id, source, started_at) SELECT 's' || i, 'cli', i FROM n;"));
        let e = read(&path, &mut Spool::new(BTreeSet::new()).unwrap(), None).unwrap_err();
        assert!(format!("{e:#}").contains("past the 200000 an import reads"), "{e:#}");
    }

    /// Goal: a database from an older Hermes, without the columns a newer
    /// one added, reads with each missing one as its default (every row
    /// live). Invalid: a file that is no Hermes database is refused, saying
    /// so.
    #[test]
    fn an_older_database_reads_and_another_file_is_refused() {
        let dir = super::super::tempdir();
        let path = dir.join("state.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT, started_at REAL);
             CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp REAL);
             INSERT INTO sessions VALUES ('a','cli',1);
             INSERT INTO messages VALUES (1,'a','user','hi',2);",
        )
        .unwrap();
        drop(db);
        let (_, all) = read_path(&path);
        assert_eq!(texts(&all[0]), ["user:hi"]);
        let other = dir.join("other.db");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        let mut spool = Spool::new(BTreeSet::new()).unwrap();
        let e = read(&other, &mut spool, None).unwrap_err();
        assert!(format!("{e:#}").contains("is it Hermes' state.db?"), "{e:#}");
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a database").unwrap();
        assert!(read(&text, &mut spool, None).is_err());
    }

    /// Goal: a read changes nothing: the database is opened read-only.
    #[test]
    fn a_read_writes_nothing() {
        let path = super::super::tempdir().join("state.db");
        database(&path, "INSERT INTO sessions VALUES ('a','cli',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL);");
        let before = std::fs::read(&path).unwrap();
        read_path(&path);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// Goal: on a platform computer, its own Hermes databases are refused,
    /// any agent's of it: its root's, and each profile's; one elsewhere
    /// reads. Method: the home a profile's folder (as `$HERMES_HOME` is in
    /// an agent's terminal), the check given it, as the CLI gives it only
    /// as an agent; with none (a person's own Hermes), nothing is refused.
    #[test]
    fn a_platform_computers_own_databases_are_refused() {
        let root = super::super::tempdir();
        let (mine, sibling) = (root.join("profiles/juniper"), root.join("profiles/maple"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let elsewhere = super::super::tempdir().join("state.db");
        for db in [root.join("state.db"), sibling.join("state.db"), mine.join("state.db"), elsewhere.clone()] {
            database(&db, "");
        }
        let refused: Vec<bool> = [root.join("state.db"), sibling.join("state.db"), mine.join("state.db"), elsewhere.clone()].iter().map(|p| refuse_own(p, &mine).is_err()).collect();
        assert_eq!(refused, [true, true, true, false]);
        let (_, all) = read_path(&mine.join("state.db"));
        assert!(all.is_empty(), "a person's own Hermes reads, refused by nothing");
        let mut spool = Spool::new(BTreeSet::new()).unwrap();
        assert!(read(&mine.join("state.db"), &mut spool, Some(&mine)).is_err());
    }

    /// Goal: what a conversation keeps is read, not what Hermes stored:
    /// a screenshot's result reads as its words and a mark, never its
    /// data; photos a person sent do not make a chat too large, nor do a
    /// tool's long arguments, cut saying how much more there was; a list
    /// that is no parts, and a string Hermes encoded so, read as they are.
    #[test]
    fn what_is_kept_is_read_not_what_is_stored() {
        let data = "A".repeat(200_000);
        let photo = "B".repeat(3 * 1024 * 1024);
        let mut sql = String::from("INSERT INTO sessions VALUES ('s','telegram',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL);");
        sql += &format!("INSERT INTO messages (session_id,role,content,timestamp,tool_name) VALUES ('s','tool',char(0) || 'json:[{{\"type\":\"text\",\"text\":\"Screenshot taken\"}},{{\"type\":\"image_url\",\"image_url\":{{\"url\":\"data:image/png;base64,{data}\"}}}}]',2,'browser');");
        for i in 0..6 {
            sql += &format!("INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user',char(0) || 'json:[{{\"type\":\"text\",\"text\":\"meal {i}\"}},{{\"type\":\"image_url\",\"image_url\":{{\"url\":\"data:image/jpeg;base64,{photo}\"}}}}]',{});", 10 + i);
        }
        let args = "x".repeat(1024 * 1024);
        for i in 0..20 {
            sql += &format!("INSERT INTO messages (session_id,role,content,timestamp,tool_calls) VALUES ('s','assistant','',{},'[{{\"id\":\"w{i}\",\"function\":{{\"name\":\"write_file\",\"arguments\":\"{args}\"}}}}]');", 100 + i);
        }
        sql += "INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user','[1, 2]',200);";
        sql += "INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user',char(0) || 'json:\"[not parts]\"',201);";
        let (spool, all) = read_sql(&sql);
        assert!(spool.refused.is_empty(), "{:?}", spool.refused.iter().map(|r| &r.why).collect::<Vec<_>>());
        let s = &all[0];
        assert_eq!(s.messages[0].text.as_deref(), Some("Screenshot taken\n[image]"));
        assert_eq!(s.messages[1].text.as_deref(), Some("meal 0\n[image]"));
        let call = &s.messages[7].tool_calls[0];
        assert_eq!((call.id.as_deref(), call.name.as_str()), (Some("w0"), "write_file"));
        assert!(call.arguments.ends_with(&format!("… [{} more characters not kept]", 1024 * 1024 - ARGUMENTS_MAX_CHARS)), "{}", call.arguments.len());
        assert_eq!(s.messages[27].text.as_deref(), Some("[1, 2]"));
        assert_eq!(s.messages[28].text.as_deref(), Some("[not parts]"));
        assert!(spool.bytes < 200_000, "{}", spool.bytes);
    }

    /// What Hermes v0.21.7 itself writes (its own `SessionDB`, run in our
    /// image with no model: `regenerate_the_hermes_fixture`), its
    /// `sessions` and `messages` tables as it makes them and the rows it
    /// wrote, so a change in Hermes' schema or writes is seen here.
    const FIXTURE: &str = include_str!("hermes-v0.21.7.sql");

    /// What made `FIXTURE`, in Hermes' own Python: run in our image by
    /// `regenerate_the_hermes_fixture`.
    const FIXTURE_SCRIPT: &str = r#"import sqlite3, sys, tempfile, os
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
from pathlib import Path
path = Path(tempfile.mkdtemp()) / "state.db"
db = SessionDB(db_path=path)
t0 = 1790845200.0
r = "20261001_090000_root01"
db.create_session(r, "telegram", model="m1", session_key="tg:1", display_name="Ann")
db.append_message(r, "user", "plant beans?", timestamp=t0)
db.append_message(r, "assistant", None, tool_calls=[{"id": "call_1", "type": "function", "function": {"name": "web_search", "arguments": "{\"q\": \"beans\"}"}}], timestamp=t0 + 1)
db.append_message(r, "tool", "beans like warm soil", tool_name="web_search", tool_call_id="call_1", timestamp=t0 + 2)
db.append_message(r, "assistant", "In spring, once the soil is warm.", timestamp=t0 + 3)
db.append_message(r, "user", [{"type": "text", "text": "here is my plot"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}], timestamp=t0 + 4)
db.set_session_title(r, "Beans")
db.end_session(r, "compression")
c = "20261001_091000_cont01"
db.create_session(c, "telegram", model="m2", parent_session_id=r, session_key="tg:1")
db.append_message(c, "user", "Summary: we talked about beans.\n--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---\nand peas?", _compressed_summary=True, display_kind="hidden", timestamp=t0 + 600)
db.append_message(c, "assistant", "In spring, once the soil is warm.", timestamp=t0 + 3)
db.append_message(c, "assistant", "Peas too.", timestamp=t0 + 601)
db.append_message(c, "user", "for the model only", display_metadata={"model_only": True}, timestamp=t0 + 602)
b = "20261001_100000_branch"
db.create_session(b, "cli", parent_session_id=r, model_config={"_branched_from": r})
db.append_message(b, "user", "what about corn?", timestamp=t0 + 3600)
# a reset of the chat that then compressed: its continuation goes on in it
s1 = "20261002_090000_reset1"
db.create_session(s1, "telegram", parent_session_id=c, session_key="tg:1", model_config={"_reset_from": c})
db.append_message(s1, "user", "new day, new plan", timestamp=t0 + 86400)
db.end_session(s1, "compression")
s2 = "20261002_093000_reset2"
db.create_session(s2, "telegram", parent_session_id=s1, session_key="tg:1")
db.append_message(s2, "assistant", "here is the plan", timestamp=t0 + 88200)
db.create_session("cron_digest_20261003_0600", "cron")
db.append_message("cron_digest_20261003_0600", "user", "the morning digest", timestamp=t0 + 172800)
d = "20261001_091500_dlg001"
db.create_session(d, "cli", parent_session_id=c, model_config={"_delegate_from": c})
db.append_message(d, "user", "look up pea varieties", timestamp=t0 + 900)
# a delegate whose parent a prune let go (its parent NULLed, as pruning does)
o = "20261001_092000_dlg002"
db.create_session(o, "cli", parent_session_id=c, model_config={"_delegate_from": c})
db.append_message(o, "user", "a pruned side task", timestamp=t0 + 1200)
db.close() if hasattr(db, "close") else None
conn = sqlite3.connect(path)
conn.execute(f"UPDATE sessions SET parent_session_id = NULL WHERE id = '{o}'")
conn.commit()
# the tables a read uses, as Hermes made them, and the rows it wrote
for (sql,) in conn.execute("SELECT sql FROM sqlite_master WHERE type = 'table' AND name IN ('sessions', 'messages') ORDER BY name DESC"):
    print(sql + ";")
def literal(v):
    if v is None:
        return "NULL"
    if isinstance(v, (int, float)):
        return repr(v)
    if isinstance(v, bytes):
        return "X'" + v.hex() + "'"
    # a NUL (Hermes' structured content starts with one) as SQL says it
    return " || char(0) || ".join("'" + part.replace("'", "''") + "'" for part in v.split(chr(0)))
for table in ("sessions", "messages"):
    for row in conn.execute(f"SELECT * FROM {table} ORDER BY rowid"):
        print(f"INSERT INTO {table} VALUES(" + ",".join(literal(v) for v in row) + ");")
"#;

    /// Goal: the reader reads what Hermes v0.21.7 wrote, as Hermes shows
    /// it. Method: `FIXTURE`: a chat it compressed (its tail copied, an
    /// image, a summary carrying the person's next words, a model-only
    /// row), a branch of it, a reset that compressed in turn, a cron run, a
    /// delegate, and one whose parent a prune let go.
    #[test]
    fn what_hermes_wrote_reads_as_hermes_shows_it() {
        let path = super::super::tempdir().join("state.db");
        // its other tables (a prompt's hash) left out: their keys are not this test's
        Connection::open(&path).unwrap().execute_batch(&format!("PRAGMA foreign_keys = OFF;\n{FIXTURE}")).unwrap();
        let (spool, all) = read_path(&path);
        assert!(spool.refused.is_empty(), "{:?}", spool.refused.iter().map(|r| &r.why).collect::<Vec<_>>());
        let kinds: Vec<(&str, Kind, usize)> = all.iter().map(|s| (s.id.as_str(), s.kind, s.parts.len())).collect();
        assert_eq!(
            kinds,
            [
                ("20261001_090000_root01", Kind::Chat, 2),
                ("20261001_091500_dlg001", Kind::Subagent, 1),
                ("20261001_092000_dlg002", Kind::Subagent, 1),
                ("20261001_100000_branch", Kind::Chat, 1),
                ("20261002_090000_reset1", Kind::Chat, 2),
                ("cron_digest_20261003_0600", Kind::Cron, 1),
            ]
        );
        let beans = &all[0];
        assert_eq!(
            texts(beans),
            [
                "user:plant beans?",
                "assistant:",
                "tool:beans like warm soil",
                "assistant:In spring, once the soil is warm.",
                "user:here is my plot\n[image]",
                "summary user:Summary: we talked about beans.\n--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---",
                "user:and peas?",
                "assistant:Peas too.",
            ]
        );
        assert_eq!((beans.messages[1].tool_calls[0].id.as_deref(), beans.messages[2].call.as_deref()), (Some("call_1"), Some("call_1")), "a result names its call");
        assert_eq!(texts(&all[4]), ["user:new day, new plan", "assistant:here is the plan"]);
    }

    /// Makes `FIXTURE` again with Hermes' own code, in an image built from
    /// this tree (`HERMES_IMAGE`, e.g. a `docker build -f
    /// images/hermes/Dockerfile .`'s tag): run by name, when Hermes moves.
    #[test]
    #[ignore]
    fn regenerate_the_hermes_fixture() {
        let image = std::env::var("HERMES_IMAGE").expect("HERMES_IMAGE names an image of ours to run Hermes' code in");
        let out = std::process::Command::new("docker")
            .args(["run", "--rm", "-e", "HERMES_HOME=/tmp/home", "--entrypoint", "/opt/hermes/.venv/bin/python", &image, "-c", FIXTURE_SCRIPT])
            .output()
            .expect("docker runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/session/hermes-v0.21.7.sql"), out.stdout).unwrap();
    }
}
