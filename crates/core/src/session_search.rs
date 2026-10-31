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
//! `TEXT_MAX_CHARS` splits at its messages (a message past it,
//! within the message, at a space where it can). Turns are numbered from the
//! start (`t<n>`, a split's later parts `t<n>.<k>`), so a conversation that
//! grew keeps its earlier sections' ids, and only its new turns are embedded
//! again.
//!
//! It is within what the platform takes of an upsert (`searchable::check`,
//! the cell's own check, which the importer runs before it sends): at most
//! `SECTIONS_MAX` sections of `TEXT_MAX_CHARS`, as sent at most
//! `UPSERT_MAX_BYTES`, its title, fields and context bounded in bytes, and
//! no control character but a line's end and a tab. A conversation past
//! them keeps its first sections, and says how many it left out.

use std::collections::BTreeMap;

use fragment_proto::search::{Location, Section, Upsert, SECTIONS_MAX, TEXT_MAX_CHARS, UPSERT_MAX_BYTES};

use crate::searchable::{CONTEXT_ITEM_MAX_BYTES, FIELD_VALUE_MAX_BYTES, TITLE_MAX_BYTES};
use crate::session::{Message, Session};

/// A tool's or a file's name, as a turn names it.
const NAME_MAX_BYTES: usize = 64;

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

/// `text`'s first `max` bytes, ending on a character's boundary.
fn cut_bytes(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A name, a title or a filter: its whitespace (and any control
/// character) one space, at most `max` bytes, no `=` (a filter's
/// separator).
fn short(text: &str, max: usize) -> String {
    let words = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").replace('=', " ");
    cut_bytes(&words, max).trim_end().to_string()
}

/// A message's text as a section holds it: no control character but a
/// line's end and a tab.
fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// A message as its turn reads it, or `None` when search keeps nothing of it.
fn line_of(m: &Message) -> Option<String> {
    let text = plain(m.text.as_deref().unwrap_or_default());
    let text = text.trim();
    if m.summary {
        return (!text.is_empty()).then(|| format!("Summary: {text}"));
    }
    let names = |names: Vec<String>| names.into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ");
    let files = names(m.attachments.iter().map(|a| short(&a.name, NAME_MAX_BYTES)).collect());
    let files = (!files.is_empty()).then(|| format!("[{files}]"));
    let (who, used) = match m.role.as_str() {
        "user" => ("Person", None),
        "assistant" => {
            let used = names(m.tool_calls.iter().map(|c| short(&c.name, NAME_MAX_BYTES)).collect());
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
    let max = TEXT_MAX_CHARS;
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
            parts.push((Location { from: *i as u64, to: *i as u64 }, piece.to_string()));
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
                Some((Location { from: l.from, to: *i as u64 }, t, c + 1 + n))
            }
            other => {
                parts.extend(other.map(|(l, t, _)| (l, t)));
                Some((Location { from: *i as u64, to: *i as u64 }, rest.to_string(), n))
            }
        };
    }
    parts.extend(cur.map(|(l, t, _)| (l, t)));
    parts
}

/// The upsert of `session`, filed at `source` with `revision` (its file's
/// sha256), within the capability's limits.
pub fn upsert_of(source: &str, revision: &str, session: &Session) -> Indexed {
    let context: Vec<String> = [session.source.as_deref(), session.agent.as_deref()].into_iter().flatten().map(|v| short(v, CONTEXT_ITEM_MAX_BYTES)).filter(|v| !v.is_empty()).collect();
    let mut fields = BTreeMap::from([("kind".to_string(), session.kind.as_str().to_string()), ("origin".to_string(), short(&session.origin, FIELD_VALUE_MAX_BYTES))]);
    if let Some(s) = session.source.as_deref() {
        fields.insert("source".into(), short(s, FIELD_VALUE_MAX_BYTES));
    }
    fields.retain(|_, v| !v.is_empty());
    let title = Some(short(&session.title, TITLE_MAX_BYTES)).filter(|t| !t.is_empty());
    let mut upsert = Upsert { source: source.to_string(), revision: revision.to_string(), replaces: None, title, at: session.started_at.clone(), fields, sections: Vec::new() };
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
        assert_eq!((upsert.source.as_str(), upsert.revision.clone(), upsert.title.as_deref(), upsert.at.as_deref()), ("sessions/hermes-c1.json", sha(9), Some("Beans"), Some("2026-10-01T09:00:00.000Z")));
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
        assert_eq!(crate::searchable::check(&upsert), Ok(()));
    }

    /// Goal: whatever a conversation holds, its upsert is one the platform
    /// takes (`searchable::check`, the cell's own). Invalid: control
    /// characters in its text, title and source, `=` in a filter, a title
    /// and an agent's name far past their bytes in a script of three bytes
    /// a character.
    #[test]
    fn an_upsert_is_one_the_platform_takes_whatever_the_conversation_holds() {
        let mut s = session(vec![said("user", "2026-10-01T09:00:00.000Z", "bell\u{7} and\u{1b}[31m color\ttab\nline"), said("assistant", "2026-10-01T09:00:01.000Z", "\u{0}")]);
        s.title = format!("{}\u{1}", "標題".repeat(200));
        s.source = Some("tele=gram\u{2}".into());
        s.agent = Some("名".repeat(200));
        let Indexed { upsert, .. } = upsert_of("sessions/hermes-c1.json", &sha(1), &s);
        assert_eq!(crate::searchable::check(&upsert), Ok(()), "{upsert:#?}");
        assert_eq!(upsert.sections.len(), 1, "a message of control characters alone says nothing");
        assert_eq!(upsert.sections[0].text, "Person: bell and[31m color\ttab\nline");
        assert_eq!(upsert.fields["source"], "tele gram");
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
        let words = "bean ".repeat(TEXT_MAX_CHARS / 5 * 3);
        let solid = "x".repeat(TEXT_MAX_CHARS + 10);
        let half = "pea ".repeat(TEXT_MAX_CHARS / 4 / 2 + 10);
        let messages = vec![said("user", "2026-10-01T09:00:00.000Z", &half), said("assistant", "2026-10-01T09:00:01.000Z", &half), said("assistant", "2026-10-01T09:00:02.000Z", &words), said("user", "2026-10-01T09:01:00.000Z", &solid)];
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(messages));
        assert_eq!(left, 0);
        let ids: Vec<&str> = upsert.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["t0", "t0.1", "t0.2", "t0.3", "t0.4", "t0.5", "t1", "t1.1"]);
        assert_eq!(upsert.sections[1].location, Some(Location { from: 1, to: 1 }), "a turn splits at its messages");
        for s in &upsert.sections {
            assert!(s.text.chars().count() <= TEXT_MAX_CHARS && s.id.len() <= fragment_proto::search::SECTION_ID_MAX_BYTES, "{}", s.id);
        }
        assert!(upsert.sections[2].text.ends_with("bean") && upsert.sections[3].text.starts_with("bean"), "a message splits at a space");
        assert_eq!(upsert.sections[6].text.chars().count(), TEXT_MAX_CHARS, "one with none where it must");

        let many: Vec<Message> = (0..SECTIONS_MAX + 5).map(|i| said("user", "2026-10-01T09:00:00.000Z", &format!("turn {i}"))).collect();
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(many));
        assert_eq!((upsert.sections.len(), left), (SECTIONS_MAX, 5), "its first sections, the rest counted");

        let big = "y".repeat(TEXT_MAX_CHARS);
        let large: Vec<Message> = (0..1000).map(|_| said("user", "2026-10-01T09:00:00.000Z", &big)).collect();
        let Indexed { upsert, left } = upsert_of("sessions/k.json", &sha(1), &session(large));
        assert!(left > 0 && serde_json::to_vec(&upsert).unwrap().len() <= UPSERT_MAX_BYTES, "an upsert as sent is at most 4 MiB");
        assert_eq!(crate::searchable::check(&upsert), Ok(()));
    }
}
