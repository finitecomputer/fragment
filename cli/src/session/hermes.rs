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
//! **Its kind**: `cron` for a scheduled job's run; `chat` for a session
//! with no parent, a branch, or the new conversation a reset starts (what
//! Hermes' session picker lists); else `subagent`, a side task (a
//! delegate, a tool's run, a continuation Hermes passed over).
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

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use fragment_core::session::{self, Kind, Message, Session, ToolCall};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

/// The origin Hermes' conversations are keyed under.
pub const ORIGIN: &str = "hermes";

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
    /// its newest message's time (in the window)
    last_message_at: Option<f64>,
}

impl Row {
    /// A marker Hermes writes in a child's `model_config` as it makes it:
    /// present when it names any value (json_extract's `IS NOT NULL`).
    fn marker(&self, key: &str) -> bool {
        self.model_config.as_ref().and_then(|c| c.get(key)).is_some_and(|v| !v.is_null())
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

/// A continuation of `parent` after it compressed (`_CHAIN_STEP_SQL`).
fn is_continuation(s: &Row, parent: &Row) -> bool {
    parent.end_reason_is("compression") && !s.marker("_branched_from") && !s.marker("_delegate_from") && !is_reset(s, parent) && s.source.as_deref() != Some("tool")
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
    content: Option<String>,
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
            ToolCall { name: f["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("tool").to_string(), arguments }
        })
        .collect();
    Message {
        role: if m.role.is_empty() { "unknown".into() } else { m.role.clone() },
        at: m.timestamp.and_then(session::iso),
        text,
        tool: m.tool_name.clone(),
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

/// What a read of a database found.
#[derive(Debug, Default)]
pub struct Read {
    pub sessions: Vec<Session>,
    /// What it could not read, each in a sentence (the rest was read).
    pub problems: Vec<String>,
}

/// The conversations in the Hermes database at `path`.
pub fn read(path: &Path) -> Result<Read> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("opening {} read-only", path.display()))?;
    // one read transaction: a Hermes writing meanwhile is read as of now
    db.execute_batch("BEGIN").with_context(|| format!("reading {}", path.display()))?;
    let read = read_in(&db).with_context(|| format!("reading {} (is it Hermes' state.db?)", path.display()));
    let _ = db.execute_batch("COMMIT");
    read
}

/// A table's columns, so a database from an older Hermes (one without
/// `compacted`, say) is read with each missing one as its default.
fn columns(db: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut q = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = q.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<HashSet<String>>>()?;
    anyhow::ensure!(!names.is_empty(), "it has no {table} table");
    Ok(names)
}

/// `SELECT` of `want`, each column the table lacks as NULL.
fn select(have: &HashSet<String>, want: &[&str]) -> String {
    want.iter().map(|c| if have.contains(*c) { format!("\"{c}\"") } else { format!("NULL AS \"{c}\"") }).collect::<Vec<_>>().join(", ")
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

const SESSION_COLUMNS: [&str; 12] = ["id", "source", "session_key", "display_name", "model", "model_config", "parent_session_id", "started_at", "ended_at", "end_reason", "title", "last_activity_at"];
const MESSAGE_COLUMNS: [&str; 15] =
    ["id", "session_id", "role", "content", "tool_call_id", "tool_calls", "tool_name", "timestamp", "reasoning", "reasoning_content", "active", "compacted", "_compressed_summary", "display_kind", "display_metadata"];

fn read_in(db: &Connection) -> Result<Read> {
    let session_cols = columns(db, "sessions")?;
    let message_cols = columns(db, "messages")?;
    anyhow::ensure!(session_cols.contains("id") && message_cols.contains("session_id"), "its sessions and messages tables are not Hermes'");
    let mut out = Read::default();

    let mut q = db.prepare(&format!("SELECT {} FROM sessions ORDER BY started_at, id", select(&session_cols, &SESSION_COLUMNS)))?;
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

    let at: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.id.as_str(), i)).collect();
    let mut q = db.prepare(&format!("SELECT {} FROM messages WHERE session_id = ?1 ORDER BY id", select(&message_cols, &MESSAGE_COLUMNS)))?;
    for chain in chains(&rows) {
        let root = &rows[chain[0]];
        let last = &rows[*chain.last().expect("a chain is never empty")];
        let parent = root.parent.as_deref().and_then(|p| at.get(p)).map(|&i| &rows[i]);
        let mut found = Vec::new();
        for &i in &chain {
            let read = q.query_map([&rows[i].id], |r| {
                let v = |i: usize| r.get::<_, Sql>(i);
                let metadata = text(&v(14)?).and_then(|m| serde_json::from_str::<Value>(&m).ok());
                Ok(MessageRow {
                    id: match v(0)? {
                        Sql::Integer(i) => i,
                        _ => 0,
                    },
                    role: text(&v(2)?).unwrap_or_default(),
                    content: text(&v(3)?),
                    tool_call_id: text(&v(4)?),
                    tool_calls: text(&v(5)?),
                    tool_name: text(&v(6)?),
                    timestamp: real(&v(7)?),
                    reasoning: text(&v(8)?).or(text(&v(9)?)).filter(|r| !r.is_empty()),
                    active: flag(&v(10)?, true),
                    compacted: flag(&v(11)?, false),
                    summary: flag(&v(12)?, false),
                    display_kind: text(&v(13)?),
                    model_only: metadata.as_ref().map(|m| &m["model_only"]).is_some_and(|f| f.as_bool().unwrap_or(false) || f.as_i64().is_some_and(|i| i != 0)),
                })
            });
            match read.and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>()) {
                Ok(mut ms) => found.append(&mut ms),
                Err(e) => out.problems.push(format!("the messages of session {}: {e}", rows[i].id)),
            }
        }
        found.sort_by_key(|m| m.id);
        let title = chain.iter().find_map(|&i| rows[i].title.clone().filter(|t| !t.trim().is_empty())).unwrap_or_default();
        out.sessions.push(Session {
            format: String::new(),
            id: root.id.clone(),
            origin: ORIGIN.to_string(),
            kind: kind(root, parent),
            title,
            source: root.source.clone(),
            with: root.display_name.clone(),
            model: last.model.clone().or_else(|| root.model.clone()),
            started_at: root.started_at.and_then(session::iso),
            ended_at: last.ended_at.or(last.last_activity_at).and_then(session::iso),
            parts: chain.iter().map(|&i| rows[i].id.clone()).collect(),
            messages: messages(found),
        });
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A database with the tables and columns of Hermes v0.21.7's that a
    /// read uses (hermes_state_common.py's SCHEMA_SQL, these columns only),
    /// so a test writes the rows Hermes writes.
    pub const SCHEMA: &str = "
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, session_key TEXT, display_name TEXT, model TEXT, model_config TEXT,
            parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, last_activity_at REAL);
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

    fn read_sql(sql: &str) -> Read {
        let dir = tempdir();
        let path = dir.join("state.db");
        database(&path, sql);
        read(&path).unwrap()
    }

    pub fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fragment-session-{}-{}", std::process::id(), rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn texts(s: &Session) -> Vec<String> {
        s.messages.iter().map(|m| format!("{}{}:{}", if m.summary { "summary " } else { "" }, m.role, m.text.clone().unwrap_or_default())).collect()
    }

    /// Goal: a conversation Hermes compressed twice is one conversation,
    /// its messages once each in the order Hermes wrote them. Method: a
    /// root, a continuation that compressed again, and its continuation;
    /// a stale sibling continuation (a reaped socket's) beside the one
    /// Hermes follows; the tail each compression copies; a summary with the
    /// person's next words after it; a row taken back; a model-only row.
    #[test]
    fn a_compressed_conversation_is_one_with_each_message_once() {
        let r = read_sql(
            r#"
            INSERT INTO sessions VALUES ('s1','telegram','tg:1','Ann','m1',NULL,NULL,100,200,'compression','Garden',NULL);
            INSERT INTO sessions VALUES ('s2','telegram','tg:1','Ann','m1','{}','s1',200,300,'compression',NULL,NULL);
            INSERT INTO sessions VALUES ('stale','telegram','tg:1','Ann','m1','{}','s1',201,205,'ws_orphan_reap',NULL,NULL);
            INSERT INTO sessions VALUES ('s3','telegram','tg:1','Ann','m2','{}','s2',300,NULL,NULL,NULL,450);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','user','plant beans',110);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s1','assistant','in spring',120);
            INSERT INTO messages (session_id,role,content,timestamp,_compressed_summary,display_kind) VALUES ('s2','user','We talked beans. --- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---
and peas?',210,1,'hidden');
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s2','assistant','in spring',120);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s2','assistant','peas too',220);
            INSERT INTO messages (session_id,role,content,timestamp,active) VALUES ('s2','user','undone',230,0);
            INSERT INTO messages (session_id,role,content,timestamp,display_metadata) VALUES ('s2','user','for the model',231,'{"model_only":true}');
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('stale','user','stray',202);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s3','user','thanks',400);
            "#,
        );
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let chat: Vec<&Session> = r.sessions.iter().filter(|s| s.kind == Kind::Chat).collect();
        assert_eq!(chat.len(), 1, "{:#?}", r.sessions);
        let c = chat[0];
        assert_eq!((c.id.as_str(), c.title.as_str(), c.parts.clone()), ("s1", "Garden", vec!["s1".to_string(), "s2".into(), "s3".into()]));
        assert_eq!(c.model.as_deref(), Some("m2"), "the model it ended on");
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
        assert_eq!(c.ended_at.as_deref(), session::iso(450.0).as_deref(), "an open session ends at its last activity");
        let stale: Vec<&Session> = r.sessions.iter().filter(|s| s.id == "stale").collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].kind, Kind::Subagent, "a continuation Hermes passed over is kept, as a side task");
    }

    /// Goal: what a session is follows Hermes' markers. Valid: a branch and
    /// a reset are chats of their own, a cron run is `cron`, a delegate and
    /// a tool's run are side tasks. Invalid: a branch or a reset of a
    /// compressed session is never its continuation.
    #[test]
    fn branches_resets_runs_and_side_tasks_are_what_hermes_says() {
        let r = read_sql(
            r#"
            INSERT INTO sessions VALUES ('root','cli','k',NULL,NULL,NULL,NULL,100,200,'compression',NULL,NULL);
            INSERT INTO sessions VALUES ('cont','cli','k',NULL,NULL,'{}','root',200,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('branch','cli','k',NULL,NULL,'{"_branched_from":"root"}','root',201,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('reset','cli','k',NULL,NULL,'{"_reset_from":"root"}','root',202,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('old','telegram','tg',NULL,NULL,NULL,NULL,10,20,'idle',NULL,NULL);
            INSERT INTO sessions VALUES ('old2','telegram','tg',NULL,NULL,NULL,'old',30,NULL,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('job','cron',NULL,NULL,NULL,NULL,NULL,50,60,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('dlg','cli',NULL,NULL,NULL,'{"_delegate_from":"cont"}','cont',210,220,NULL,NULL,NULL);
            INSERT INTO sessions VALUES ('tool','tool',NULL,NULL,NULL,NULL,'cont',211,221,NULL,NULL,NULL);
            "#,
        );
        let kind_of = |id: &str| r.sessions.iter().find(|s| s.id == id).map(|s| (s.kind, s.parts.clone()));
        assert_eq!(kind_of("root"), Some((Kind::Chat, vec!["root".into(), "cont".into()])));
        assert_eq!(kind_of("branch").map(|k| k.0), Some(Kind::Chat));
        assert_eq!(kind_of("reset").map(|k| k.0), Some(Kind::Chat));
        assert_eq!(kind_of("old2").map(|k| k.0), Some(Kind::Chat), "a child on its parent's routing key after an idle reset is a new chat");
        assert_eq!(kind_of("job").map(|k| k.0), Some(Kind::Cron));
        assert_eq!(kind_of("dlg").map(|k| k.0), Some(Kind::Subagent));
        assert_eq!(kind_of("tool").map(|k| k.0), Some(Kind::Subagent));
        assert_eq!(kind_of("cont"), None, "a continuation is its conversation's, not one of its own");
        assert_eq!(r.sessions.len(), 8);
    }

    /// Goal: Hermes' stored forms read as text. Method: structured content
    /// with an image (`\0json:`), a legacy list of parts, tool calls with
    /// their arguments, and a tool's result.
    #[test]
    fn stored_content_reads_as_text_and_calls() {
        let r = read_sql(
            r#"
            INSERT INTO sessions VALUES ('s','cli',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user',char(0) || 'json:[{"type":"text","text":"see"},{"type":"image_url","image_url":{"url":"data:x"}}]',2);
            INSERT INTO messages (session_id,role,content,timestamp) VALUES ('s','user','[{"type":"input_text","text":"old"}]',3);
            INSERT INTO messages (session_id,role,content,timestamp,tool_calls,reasoning) VALUES ('s','assistant','',4,'[{"id":"c1","function":{"name":"web","arguments":"{\"q\":1}"}}]','thinking');
            INSERT INTO messages (session_id,role,content,timestamp,tool_name,tool_call_id) VALUES ('s','tool','result',5,'web','c1');
            "#,
        );
        let s = &r.sessions[0];
        assert_eq!(s.messages[0].text.as_deref(), Some("see\n[image]"));
        assert_eq!(s.messages[1].text.as_deref(), Some("old"));
        assert_eq!(s.messages[2].tool_calls, vec![ToolCall { name: "web".into(), arguments: "{\"q\":1}".into() }]);
        assert_eq!((s.messages[2].text.as_deref(), s.messages[2].reasoning.as_deref()), (None, Some("thinking")));
        assert_eq!((s.messages[3].tool.as_deref(), s.messages[3].text.as_deref()), (Some("web"), Some("result")));
        assert_eq!(s.messages[0].at.as_deref(), Some("1970-01-01T00:00:02.000Z"));
    }

    /// Goal: a database from an older Hermes, without the columns a newer
    /// one added, reads with each as its default (every row live). Invalid:
    /// a file that is no Hermes database is refused, saying so.
    #[test]
    fn an_older_database_reads_and_another_file_is_refused() {
        let dir = tempdir();
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
        let r = read(&path).unwrap();
        assert_eq!(texts(&r.sessions[0]), ["user:hi"]);
        let other = dir.join("other.db");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        let e = read(&other).unwrap_err();
        assert!(format!("{e:#}").contains("is it Hermes' state.db?"), "{e:#}");
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not a database").unwrap();
        assert!(read(&text).is_err());
    }

    /// Goal: a read changes nothing: the database is opened read-only.
    #[test]
    fn a_read_writes_nothing() {
        let dir = tempdir();
        let path = dir.join("state.db");
        database(&path, "INSERT INTO sessions VALUES ('a','cli',NULL,NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL);");
        let before = std::fs::read(&path).unwrap();
        read(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
