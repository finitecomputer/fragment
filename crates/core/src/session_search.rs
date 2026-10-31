//! What the platform's search holds of a session archive's conversation
//! (`session-archive/1`): one source a conversation's file
//! (`sessions/<key>.json`), its revision the file's sha256, and one
//! section a turn. The importer writes it as it files the conversation,
//! holding its bytes (`fragment session import`; cli/src/session/search.rs).
//!
//! A turn is a person's message and what the agent said until their next
//! one, as "Person: … / Agent: …": a tool's output left out and its call only
//! named, reasoning left out, the files a message held named, and the summary
//! a compression wrote a section of its own. A turn past
//! `SECTION_TEXT_MAX_CHARS` splits at its messages (a message past it,
//! within the message, at a space where it can). Turns are numbered from the
//! start (`t<n>`, a split's later parts `t<n>.<k>`), so a conversation that
//! grew keeps its earlier sections' ids, and only its new turns are embedded
//! again.
//!
//! The search capability's limits (its contract) are held here: at most
//! `SECTIONS_MAX` sections of `SECTION_TEXT_MAX_CHARS`, ids of
//! `SECTION_ID_MAX_BYTES`, at most 4 short fields, and an upsert of at most
//! `UPSERT_MAX_BYTES` as sent. A conversation past them keeps its first
//! sections, and says how many it left out.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::session::{Message, Session};

/// The capability's limits, as its contract sets them.
pub const SECTIONS_MAX: usize = 2000;
pub const SECTION_TEXT_MAX_CHARS: usize = 8000;
pub const SECTION_ID_MAX_BYTES: usize = 64;
pub const UPSERT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const SOURCE_MAX_BYTES: usize = 512;
pub const REVISION_MAX_BYTES: usize = 128;
pub const FIELDS_MAX: usize = 4;
/// A field, a context item, a tool's or a file's name: short.
pub const FIELD_MAX_CHARS: usize = 64;
pub const TITLE_MAX_CHARS: usize = 200;

/// One source, replacing every section it had (`POST
/// /api/f/{name}/search/upsert`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upsert {
    pub source: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

/// Where a section is in its conversation: its first and last message's
/// places (both in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub from: usize,
    pub to: usize,
}

/// A conversation's upsert, and how many of its sections the limits left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub upsert: Upsert,
    pub left: usize,
}

/// `text`'s first `max` characters.
fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// A name or a filter: its whitespace one space, at most `max` characters.
fn short(text: &str, max: usize) -> String {
    cut(&text.split_whitespace().collect::<Vec<_>>().join(" "), max).to_string()
}

/// A message as its turn reads it, or `None` when search keeps nothing of it.
fn line_of(m: &Message) -> Option<String> {
    let text = m.text.as_deref().unwrap_or_default().trim();
    if m.summary {
        return (!text.is_empty()).then(|| format!("Summary: {text}"));
    }
    let names = |names: Vec<String>| names.into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ");
    let files = names(m.attachments.iter().map(|a| short(&a.name, FIELD_MAX_CHARS)).collect());
    let files = (!files.is_empty()).then(|| format!("[{files}]"));
    let (who, used) = match m.role.as_str() {
        "user" => ("Person", None),
        "assistant" => {
            let used = names(m.tool_calls.iter().map(|c| short(&c.name, FIELD_MAX_CHARS)).collect());
            ("Agent", (!used.is_empty()).then(|| format!("(used {used})")))
        }
        // a tool's output, the system's prompt: left out
        _ => return None,
    };
    let said: Vec<&str> = [Some(text).filter(|t| !t.is_empty()), files.as_deref(), used.as_deref()].into_iter().flatten().collect();
    (!said.is_empty()).then(|| format!("{who}: {}", said.join(" ")))
}

struct Turn {
    n: usize,
    at: Option<String>,
    summary: bool,
    /// Each message it keeps: its place, and its line.
    lines: Vec<(usize, String)>,
}

/// The conversation's turns, numbered in order: one at each person's
/// message (or at the first message, before any), a summary one of its own.
fn turns_of(messages: &[Message]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut open = false;
    for (i, m) in messages.iter().enumerate() {
        if m.summary || m.role == "user" || !open {
            turns.push(Turn { n: turns.len(), at: m.at.clone(), summary: m.summary, lines: Vec::new() });
            open = !m.summary;
        }
        if let (Some(line), Some(turn)) = (line_of(m), turns.last_mut()) {
            turn.lines.push((i, line));
        }
    }
    turns.retain(|t| !t.lines.is_empty());
    turns
}

/// A turn's parts: its lines joined up to the limit, one longer than it cut
/// at its last space in reach (or where it must be).
fn parts_of(turn: &Turn) -> Vec<(Location, String)> {
    let max = SECTION_TEXT_MAX_CHARS;
    let mut parts: Vec<(Location, String)> = Vec::new();
    let mut cur: Option<(Location, String, usize)> = None;
    for (i, line) in &turn.lines {
        let mut rest = line.as_str();
        while rest.chars().count() > max {
            parts.extend(cur.take().map(|(l, t, _)| (l, t)));
            let reach = cut(rest, max);
            let piece = match reach.rfind(char::is_whitespace) {
                Some(at) if reach[..at].chars().count() > max / 2 => &reach[..at],
                _ => reach,
            };
            parts.push((Location { from: *i, to: *i }, piece.to_string()));
            rest = rest[piece.len()..].trim_start();
        }
        if rest.is_empty() {
            continue;
        }
        let n = rest.chars().count();
        cur = match cur.take() {
            Some((l, mut t, c)) if c + 1 + n <= max => {
                t.push('\n');
                t.push_str(rest);
                Some((Location { from: l.from, to: *i }, t, c + 1 + n))
            }
            other => {
                parts.extend(other.map(|(l, t, _)| (l, t)));
                Some((Location { from: *i, to: *i }, rest.to_string(), n))
            }
        };
    }
    parts.extend(cur.map(|(l, t, _)| (l, t)));
    parts
}

/// The upsert of `session`, filed at `source` with `revision` (its file's
/// sha256), within the capability's limits.
pub fn upsert_of(source: &str, revision: &str, session: &Session) -> Indexed {
    let context: Vec<String> = [session.source.as_deref(), session.agent.as_deref()].into_iter().flatten().map(|v| short(v, FIELD_MAX_CHARS)).filter(|v| !v.is_empty()).collect();
    let mut fields = BTreeMap::from([("kind".to_string(), session.kind.as_str().to_string()), ("origin".to_string(), short(&session.origin, FIELD_MAX_CHARS))]);
    if let Some(s) = session.source.as_deref().map(|s| short(s, FIELD_MAX_CHARS)).filter(|s| !s.is_empty()) {
        fields.insert("source".into(), s);
    }
    fields.retain(|_, v| !v.is_empty());
    let title = Some(short(&session.title, TITLE_MAX_CHARS)).filter(|t| !t.is_empty());
    let mut upsert = Upsert { source: source.to_string(), revision: revision.to_string(), title, at: session.started_at.clone(), fields, sections: Vec::new() };
    // as sent: the envelope, and each section with its comma
    let mut bytes = serde_json::to_vec(&upsert).map_or(0, |v| v.len());
    let mut left = 0;
    for turn in turns_of(&session.messages) {
        for (k, (location, text)) in parts_of(&turn).into_iter().enumerate() {
            let section = Section {
                id: if k == 0 { format!("t{}", turn.n) } else { format!("t{}.{k}", turn.n) },
                text,
                heading: turn.summary.then(|| "Summary".to_string()),
                context: context.clone(),
                at: turn.at.clone(),
                location: Some(location),
            };
            let size = serde_json::to_vec(&section).map_or(usize::MAX, |v| v.len()) + 1;
            if upsert.sections.len() >= SECTIONS_MAX || bytes.saturating_add(size) > UPSERT_MAX_BYTES {
                left += 1;
                continue;
            }
            bytes += size;
            upsert.sections.push(section);
        }
    }
    Indexed { upsert, left }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Attachment, Kind, ToolCall};

    fn said(role: &str, at: &str, text: &str) -> Message {
        Message { role: role.into(), at: Some(at.into()), text: Some(text.into()), ..Default::default() }
    }

    fn session(messages: Vec<Message>) -> Session {
        Session {
            format: String::new(),
            id: "c1".into(),
            origin: "hermes".into(),
            reader: 10,
            kind: Kind::Chat,
            agent: Some("Juniper".into()),
            title: "Beans".into(),
            source: Some("telegram".into()),
            with: Some("Ann".into()),
            model: None,
            started_at: Some("2026-10-01T09:00:00.000Z".into()),
            ended_at: None,
            parts: vec![],
            messages,
        }
    }

    fn sha(n: u8) -> String {
        format!("{n:064x}")
    }

    /// Goal: a conversation is searched as its turns, as a person reads
    /// them. Method: a chat with a picture, tool calls and their output, a
    /// system prompt, reasoning, a compression's summary, and a reply.
    #[test]
    fn a_conversation_is_searched_as_its_turns() {
        let picture = Attachment { name: "plot.png".into(), media_type: "image/png".into(), size: 3, sha256: sha(1), path: String::new() };
        let messages = vec![
            said("system", "2026-10-01T09:00:00.000Z", "You are Juniper."),
            Message { attachments: vec![picture], ..said("user", "2026-10-01T09:00:01.000Z", "when do I plant beans?") },
            Message { tool_calls: vec![ToolCall { id: None, name: "web_search".into(), arguments: "{}".into() }], reasoning: Some("think".into()), text: None, ..said("assistant", "2026-10-01T09:00:02.000Z", "") },
            Message { tool: Some("web_search".into()), ..said("tool", "2026-10-01T09:00:03.000Z", "a page of results") },
            said("assistant", "2026-10-01T09:00:04.000Z", "In spring, after the frost."),
            Message { summary: true, ..said("user", "2026-10-02T09:00:00.000Z", "Ann plants beans in spring.") },
            said("assistant", "2026-10-02T09:00:01.000Z", "Picking up where we were."),
            said("user", "2026-10-02T09:01:00.000Z", "and peas?"),
        ];
        let Indexed { upsert, left } = upsert_of("sessions/hermes-c1.json", &sha(9), &session(messages));
        assert_eq!(left, 0);
        assert_eq!((upsert.source.as_str(), upsert.revision, upsert.title.as_deref(), upsert.at.as_deref()), ("sessions/hermes-c1.json", sha(9), Some("Beans"), Some("2026-10-01T09:00:00.000Z")));
        assert_eq!(upsert.fields, BTreeMap::from([("kind".into(), "chat".into()), ("origin".into(), "hermes".into()), ("source".into(), "telegram".into())]));
        let got: Vec<(&str, &str, Option<&str>, Location)> = upsert.sections.iter().map(|s| (s.id.as_str(), s.text.as_str(), s.heading.as_deref(), s.location.unwrap())).collect();
        assert_eq!(
            got,
            [
                ("t1", "Person: when do I plant beans? [plot.png]\nAgent: (used web_search)\nAgent: In spring, after the frost.", None, Location { from: 1, to: 4 }),
                ("t2", "Summary: Ann plants beans in spring.", Some("Summary"), Location { from: 5, to: 5 }),
                ("t3", "Agent: Picking up where we were.", None, Location { from: 6, to: 6 }),
                ("t4", "Person: and peas?", None, Location { from: 7, to: 7 }),
            ],
            "the system prompt, the tool's output and the reasoning left out (its turn, t0, holds nothing)"
        );
        assert_eq!(upsert.sections[0].at.as_deref(), Some("2026-10-01T09:00:01.000Z"), "a turn is when it began");
        assert_eq!(upsert.sections[0].context, ["telegram", "Juniper"]);
    }

    /// Goal: a conversation that grew keeps its earlier sections as they
    /// were, so only its new turns are embedded again. Method: a chat, then
    /// it with two more turns.
    #[test]
    fn a_conversation_that_grew_keeps_its_earlier_sections() {
        let first = vec![said("user", "2026-10-01T09:00:00.000Z", "hello"), said("assistant", "2026-10-01T09:00:01.000Z", "hi"), said("user", "2026-10-01T09:01:00.000Z", "beans?")];
        let mut grown = first.clone();
        grown.extend([said("assistant", "2026-10-01T09:01:01.000Z", "spring"), said("user", "2026-10-01T09:02:00.000Z", "peas?"), said("assistant", "2026-10-01T09:02:01.000Z", "also spring")]);
        let (a, b) = (upsert_of("sessions/k.json", &sha(1), &session(first)).upsert, upsert_of("sessions/k.json", &sha(2), &session(grown)).upsert);
        assert_eq!(a.sections[0], b.sections[0]);
        assert_eq!(b.sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["t0", "t1", "t2"]);
        assert_eq!(b.sections[1].text, "Person: beans?\nAgent: spring", "the turn that grew is embedded again");
    }

    /// Goal: every upsert is within the capability's limits, whatever the
    /// conversation. Method: a turn past the section limit (split at its
    /// messages), a message past it (split within, at a space), one with no
    /// space, and a conversation of more turns than an upsert takes.
    #[test]
    fn every_upsert_is_within_the_capabilitys_limits() {
        let words = "bean ".repeat(SECTION_TEXT_MAX_CHARS / 5 * 3);
        let solid = "x".repeat(SECTION_TEXT_MAX_CHARS + 10);
        let half = "pea ".repeat(SECTION_TEXT_MAX_CHARS / 4 / 2 + 10);
        let messages = vec![said("user", "2026-10-01T09:00:00.000Z", &half), said("assistant", "2026-10-01T09:00:01.000Z", &half), said("assistant", "2026-10-01T09:00:02.000Z", &words), said("user", "2026-10-01T09:01:00.000Z", &solid)];
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(messages));
        assert_eq!(left, 0);
        let ids: Vec<&str> = upsert.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["t0", "t0.1", "t0.2", "t0.3", "t0.4", "t0.5", "t1", "t1.1"]);
        assert_eq!(upsert.sections[1].location, Some(Location { from: 1, to: 1 }), "a turn splits at its messages");
        for s in &upsert.sections {
            assert!(s.text.chars().count() <= SECTION_TEXT_MAX_CHARS && s.id.len() <= SECTION_ID_MAX_BYTES, "{}", s.id);
        }
        assert!(upsert.sections[2].text.ends_with("bean") && upsert.sections[3].text.starts_with("bean"), "a message splits at a space");
        assert_eq!(upsert.sections[6].text.chars().count(), SECTION_TEXT_MAX_CHARS, "one with none where it must");

        let many: Vec<Message> = (0..SECTIONS_MAX + 5).map(|i| said("user", "2026-10-01T09:00:00.000Z", &format!("turn {i}"))).collect();
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(many));
        assert_eq!((upsert.sections.len(), left), (SECTIONS_MAX, 5), "its first sections, the rest counted");

        let big = "y".repeat(SECTION_TEXT_MAX_CHARS);
        let large: Vec<Message> = (0..1000).map(|_| said("user", "2026-10-01T09:00:00.000Z", &big)).collect();
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(large));
        assert!(left > 0 && serde_json::to_vec(&upsert).unwrap().len() <= UPSERT_MAX_BYTES, "an upsert as sent is at most 4 MiB");
        assert!(upsert.fields.len() <= FIELDS_MAX);
    }
}
