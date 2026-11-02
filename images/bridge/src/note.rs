//! The note a turn carries after a cut one (docs/computers.md, Durability, P5):
//! the first turn an agent runs in a chat after one of its turns there was
//! ended as lost (a restart cut it; P1, one life per turn, never runs it
//! again) is told so, with what that turn was asked, the steps and replies
//! the journal recorded of it, and to check what was done before doing any
//! of it again.
//!
//! Built from the chat's journal alone: `work` (the turns, their steps and
//! cards) and `chat` (what was asked, what was replied), which never go
//! back in time, so a note is the same in every life and after any rollback
//! or loss of `/data`. Pure: records in, a note out; the driver reads the
//! records (driver.rs, `note_for`), and runtimes are handed the text
//! (`TurnStart::note`).
//!
//! The rule: the agent's turn before this one in this chat ended as lost.
//! "Before" is the journal's order: the latest of the agent's `turn.start`
//! records before this turn's own (its claim), passing over a refusal's,
//! which never ran. At the turn after, the turn before is this one, so the
//! note is said once. A turn before it with no end in the journal (its end
//! never landed) is told nothing: only a recorded loss is a cut.
//!
//! A call the cut turn asked for (`turn.call`, recorded before it ran) that
//! no step of its response ended never reported back: it may or may not
//! have run, and the note names it first among what to check.

use serde_json::Value;

use crate::engine::{LOST, REFUSED_BUSY, REFUSED_QUEUED};
use crate::limits;
use crate::records::{self, Cause, Record, Said, Task};

/// One step a cut turn had recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutStep {
    pub tool: String,
    pub args: String,
    pub ok: bool,
}

/// A call a cut turn asked for and never heard the end of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutCall {
    pub tool: String,
    pub args: String,
}

/// A card a cut turn had shown, and how it closed (`None`: never closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutCard {
    pub text: String,
    pub closed: Option<String>,
}

/// A turn of the agent's that a restart cut, as its chat's `work` recorded
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub turn: String,
    /// The record that started it: what it was asked.
    pub cause: Cause,
    /// When its claim was recorded (the platform's `at`, ms): its replies
    /// on `chat` come after.
    pub started_at: i64,
    /// Its steps in order, at most `NOTE_STEPS_MAX`, and how many it had.
    pub steps: Vec<CutStep>,
    pub steps_total: usize,
    /// The calls it asked for with no step ending them, in order, at most
    /// `NOTE_STEPS_MAX`, and how many it had.
    pub unended: Vec<CutCall>,
    pub unended_total: usize,
    /// Its cards in order, at most `NOTE_CARDS_MAX`.
    pub cards: Vec<CutCard>,
}

/// What the journal before a turn's claim says of the agent's turn before
/// it in the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Before {
    /// No turn of the agent's in the records read: read further back.
    Unread,
    /// Its turn before this one did not end as lost (or there was none
    /// before the first record: `previous` is told the records start there).
    Kept,
    /// Its turn before this one was cut by a restart.
    Cut(Cut),
}

fn field<'a>(b: &'a Value, k: &str) -> &'a str {
    b[k].as_str().unwrap_or("")
}

/// The agent's own record of `kind` (posted as it, naming it where the
/// record names an agent).
fn own<'a>(r: &'a Record, agent: &str, kind: &str) -> Option<&'a Value> {
    (r.principal == agent && r.body["kind"] == kind).then_some(&r.body)
}

/// Whether a `turn.end` is a refusal: the turn never ran.
fn refused(end: &Value) -> bool {
    end["outcome"] == "error" && matches!(field(end, "error"), REFUSED_QUEUED | REFUSED_BUSY)
}

/// The agent's turn before a claim, from `work`: a run of the chat's `work`
/// records ending just before the claim, oldest first (the claim itself and
/// anything after it left out). `from_start`: the run begins at the
/// channel's first record, so a turn found nowhere in it means none.
/// `agent` is the agent's identity.
pub fn previous(work: &[Record], agent: &str, from_start: bool) -> Before {
    // newest first: a turn's end comes after its start, so it is met first
    let mut ends: Vec<(&str, &Value)> = Vec::new();
    for (i, r) in work.iter().enumerate().rev() {
        if let Some(end) = own(r, agent, "turn.end") {
            ends.push((field(end, "turn"), end));
            continue;
        }
        let Some(start) = own(r, agent, "turn.start") else { continue };
        if start["agent"] != agent {
            continue;
        }
        let turn = field(start, "turn");
        let end = ends.iter().find(|(t, _)| *t == turn).map(|(_, e)| *e);
        match end {
            Some(e) if refused(e) => continue,
            Some(e) if e["outcome"] == "error" && field(e, "error") == LOST => {
                return cut(&work[i..], agent, turn, start, r.at).map_or(Before::Kept, Before::Cut);
            }
            _ => return Before::Kept,
        }
    }
    if from_start {
        Before::Kept
    } else {
        Before::Unread
    }
}

/// The cut turn `turn` as the records from its start on hold it.
fn cut(from: &[Record], agent: &str, turn: &str, start: &Value, started_at: i64) -> Option<Cut> {
    let cause: Cause = serde_json::from_value(start["cause"].clone()).ok()?;
    let mine = |r: &&Record| r.principal == agent && r.body["turn"] == turn;
    let steps: Vec<CutStep> = from
        .iter()
        .filter(mine)
        .filter(|r| r.body["kind"] == "turn.step")
        .map(|r| CutStep { tool: field(&r.body, "tool").to_string(), args: field(&r.body, "args").to_string(), ok: r.body["ok"] != false })
        .collect();
    let steps_total = steps.len();
    // Each step ends a call it pairs with, among the calls of the response
    // it ran in (Hermes runs a response's calls before it asks the model
    // again; a step naming no response, an older image's, among all the
    // turn's): by its id, else, for one its runtime renamed after saying it
    // (Hermes de-duplicates a session's ids, `_d<n>`, splits composite ones
    // and rewrites some providers'), by its tool and arguments. One step to
    // one call, the first still open; a call no step pairs with is unended.
    let calls: Vec<&Value> = from.iter().filter(mine).map(|r| &r.body).filter(|b| b["kind"] == "turn.call").collect();
    let ends: Vec<&Value> = from.iter().filter(mine).map(|r| &r.body).filter(|b| b["kind"] == "turn.step").collect();
    let mut open: Vec<bool> = vec![true; calls.len()];
    let mut paired: Vec<bool> = vec![false; ends.len()];
    let rules: [fn(&Value, &Value) -> bool; 2] = [|c, end| end["tool_call_id"].is_string() && c["tool_call_id"] == end["tool_call_id"], |c, end| c["tool"] == end["tool"] && c["args"] == end["args"]];
    for pairs in rules {
        for (e, end) in ends.iter().enumerate() {
            let of_its_response = |c: &Value| end.get("response").is_none_or(|r| c["response"] == *r);
            if let Some(i) = (0..calls.len()).find(|&i| !paired[e] && open[i] && of_its_response(calls[i]) && pairs(calls[i], end)) {
                open[i] = false;
                paired[e] = true;
            }
        }
    }
    let unended: Vec<CutCall> = calls
        .iter()
        .zip(&open)
        .filter(|(_, open)| **open)
        .map(|(c, _)| CutCall { tool: field(c, "tool").to_string(), args: field(c, "args").to_string() })
        .collect();
    let unended_total = unended.len();
    let closed = |prompt: &str| -> Option<String> {
        let c = from.iter().filter(mine).map(|r| &r.body).find(|b| b["kind"] == "turn.prompt.closed" && b["prompt"] == prompt)?;
        Some(match field(c, "outcome") {
            "answered" => format!("answered: {}", field(c, "option")),
            other => other.to_string(),
        })
    };
    let cards: Vec<CutCard> = from
        .iter()
        .filter(mine)
        .filter(|r| r.body["kind"] == "turn.prompt")
        .take(limits::NOTE_CARDS_MAX)
        .map(|r| CutCard { text: field(&r.body, "text").to_string(), closed: closed(field(&r.body, "prompt")) })
        .collect();
    Some(Cut {
        turn: turn.to_string(),
        cause,
        started_at,
        steps: steps.into_iter().take(limits::NOTE_STEPS_MAX).collect(),
        steps_total,
        unended: unended.into_iter().take(limits::NOTE_STEPS_MAX).collect(),
        unended_total,
        cards,
    })
}

/// What a cause record asked: a message's text (or what it carried), or a
/// routine's.
pub fn asked(cause: &Record) -> Option<String> {
    let text = match cause.channel.as_str() {
        records::CHAT => match records::said(&cause.body) {
            Said::Message(m) if !m.text.trim().is_empty() => m.text,
            Said::Message(m) if !m.attachments.is_empty() => format!("({} attached files)", m.attachments.len()),
            _ => return None,
        },
        records::TASKS => match records::task(&cause.body) {
            Task::Routine { text, .. } => format!("your routine: {text}"),
            _ => return None,
        },
        _ => return None,
    };
    Some(text)
}

/// What the agent had replied in `turn`, from `chat` records, in order.
pub fn replies(chat: &[Record], agent: &str, turn: &str) -> Vec<String> {
    chat.iter()
        .filter(|r| r.principal == agent && r.body["turn"] == turn)
        .filter_map(|r| match records::said(&r.body) {
            Said::Message(m) if !m.text.trim().is_empty() => Some(m.text),
            Said::Message(m) if !m.attachments.is_empty() => Some(format!("({} files)", m.attachments.len())),
            _ => None,
        })
        .collect()
}

/// One line of the note, quoted and on one line.
fn quoted(text: &str, max: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("“{}”", records::cut(&one_line, max))
}

/// The note, bounded (`NOTE_MAX_BYTES`): what it is and what to do first,
/// then what the cut turn was asked, its steps and cards, and what it had
/// replied.
pub fn text(cut: &Cut, asked: Option<&str>, replies: &[String]) -> String {
    let mut lines = vec![
        "Your previous turn in this chat was cut short: your computer restarted before it finished. Check what it already did before you do any of it again; the message below is new.".to_string(),
    ];
    if let Some(a) = asked.filter(|a| !a.trim().is_empty()) {
        lines.push(format!("It was answering: {}", quoted(a, limits::NOTE_ASKED_MAX_CHARS)));
    }
    lines.extend(unended(cut));
    lines.extend(steps(cut));
    for c in &cut.cards {
        let closed = c.closed.as_deref().unwrap_or("unanswered");
        lines.push(format!("It asked: {} ({closed})", quoted(&c.text, limits::NOTE_CARD_MAX_CHARS)));
    }
    lines.extend(replied(replies));
    let note = records::cut_bytes(&lines.join("\n"), limits::NOTE_MAX_BYTES);
    assert!(note.len() <= limits::NOTE_MAX_BYTES, "a note is bounded");
    note
}

/// The turn `turn` of the agent's, as `work` holds it from its start on;
/// `None` when its start is not among the records, or names no cause.
pub fn turn(work: &[Record], agent: &str, turn: &str) -> Option<Cut> {
    let i = work.iter().position(|r| own(r, agent, "turn.start").is_some_and(|b| b["turn"] == turn))?;
    cut(&work[i..], agent, turn, &work[i].body, work[i].at)
}

/// Whether the agent's turn `turn`, as `work` holds it, waited on its
/// person: a card it showed that never closed, or a question it asked in
/// words (`turn.asked`). What a life's own state says of a turn it ran
/// (`asking`, `Phase::Waiting`), read from the journal for one another
/// life ran after the `/data` this life restored (its claim answered 409:
/// `ClaimAnswer::Theirs`), so a restart continues neither (engine.rs,
/// `continued`). A question in words counts answered or not: an answer
/// that life heard is a message this life reads again and no life
/// claimed, so it runs as its own turn.
pub fn waited(work: &[Record], agent: &str, turn: &str) -> bool {
    let mine = |r: &&Record| r.principal == agent && r.body["turn"] == turn;
    let closed = |prompt: &Value| work.iter().filter(mine).any(|r| r.body["kind"] == "turn.prompt.closed" && r.body["prompt"] == *prompt);
    work.iter().filter(mine).any(|r| r.body["kind"] == "turn.asked" || (r.body["kind"] == "turn.prompt" && !closed(&r.body["prompt"])))
}

/// What a fresh turn from a late answer to its card is told (cards.rs),
/// bounded (`NOTE_MAX_BYTES`): that the card's turn ended before the
/// answer and is not resumed, and to check what it did before doing any of
/// it again; who asked it what (`by`: their name; the request cut to
/// `NOTE_LATE_ASKED_MAX_CHARS`, said so); its steps; its last replies.
/// Context, not the person's words: Relay hands it as the inbound's
/// read-only `context`, as it hands `text`.
pub fn late(cut: &Cut, by: &str, asked: Option<&str>, replies: &[String]) -> String {
    let mut lines = vec![
        "The message below answers a question your earlier turn in this chat asked. That turn ended before the answer came and is not resumed: this is a new turn. Check what it already did before you do any of it again, and don't ask the question again.".to_string(),
    ];
    match asked.filter(|a| !a.trim().is_empty()) {
        Some(a) => {
            let one_line = a.split_whitespace().collect::<Vec<_>>().join(" ");
            let more = one_line.chars().count() > limits::NOTE_LATE_ASKED_MAX_CHARS;
            lines.push(format!("It was answering {by}: {}{}", quoted(a, limits::NOTE_LATE_ASKED_MAX_CHARS), if more { " (the rest is cut)" } else { "" }));
        }
        None => lines.push(format!("It was answering {by}; what they asked is not in this chat's record.")),
    }
    lines.extend(unended(cut));
    lines.extend(steps(cut));
    lines.extend(replied(replies));
    let note = records::cut_bytes(&lines.join("\n"), limits::NOTE_MAX_BYTES);
    assert!(note.len() <= limits::NOTE_MAX_BYTES, "a note is bounded");
    note
}

/// A turn's steps on one line: at most `NOTE_STEPS_MAX`, the rest counted.
fn steps(cut: &Cut) -> Option<String> {
    if cut.steps.is_empty() {
        return None;
    }
    let mut steps: Vec<String> = cut
        .steps
        .iter()
        .map(|s| {
            let args = s.args.split_whitespace().collect::<Vec<_>>().join(" ");
            let args = if args.is_empty() { String::new() } else { format!(" {}", records::cut(&args, limits::NOTE_ARGS_MAX_CHARS)) };
            let tool = if s.tool.trim().is_empty() { "a step".to_string() } else { records::cut(&s.tool, limits::NOTE_TOOL_MAX_CHARS) };
            format!("{tool}{args} ({})", if s.ok { "ok" } else { "failed" })
        })
        .collect();
    if cut.steps_total > cut.steps.len() {
        steps.push(format!("and {} more", cut.steps_total - cut.steps.len()));
    }
    Some(format!("Its steps, as recorded: {}", steps.join("; ")))
}

/// How long ago a turn's message was said, in plain words, when it is
/// handed at least `MESSAGE_AGE_TOLD_MS` after it (an outage, a restart, a
/// sleep, a backlog), so its agent can judge whether it is still wanted;
/// `None` sooner, for a time unknown (`said_at` 0), or for a routine or a
/// command (`exempt`: a routine's message says its hour). A continuation
/// (`continues`) is told the age of what its cut turn was answering.
pub fn age(said_at: i64, now: u64, exempt: bool, continues: bool) -> Option<String> {
    let late = i64::try_from(now).ok()?.checked_sub(said_at)?;
    if said_at <= 0 || exempt || late < i64::try_from(limits::MESSAGE_AGE_TOLD_MS).ok()? {
        return None;
    }
    let minutes = late / 60_000;
    let ago = match minutes {
        ..120 => format!("{minutes} minutes"),
        120..2880 => format!("{} hours", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    };
    let what = if continues { format!("What that turn was answering was sent about {ago} ago.") } else { format!("This message was sent about {ago} ago, and is only reaching you now.") };
    let line = format!("{what} If it may no longer be wanted, check before acting on it.");
    assert!(line.len() <= limits::AGE_NOTE_MAX_BYTES, "an age is one short line: {line}");
    Some(line)
}

/// The calls a turn asked for and never heard the end of, on one line:
/// each may or may not have run (the computer stopped while it ran, or
/// before), at most `NOTE_STEPS_MAX`, the rest counted.
fn unended(cut: &Cut) -> Option<String> {
    if cut.unended.is_empty() {
        return None;
    }
    let mut calls: Vec<String> = cut
        .unended
        .iter()
        .map(|c| {
            let args = c.args.split_whitespace().collect::<Vec<_>>().join(" ");
            let args = if args.is_empty() { String::new() } else { format!(" {}", records::cut(&args, limits::NOTE_ARGS_MAX_CHARS)) };
            let tool = if c.tool.trim().is_empty() { "a tool".to_string() } else { records::cut(&c.tool, limits::NOTE_TOOL_MAX_CHARS) };
            format!("{tool}{args}")
        })
        .collect();
    if cut.unended_total > cut.unended.len() {
        calls.push(format!("and {} more", cut.unended_total - cut.unended.len()));
    }
    Some(format!("It had started these and never heard back, so each may or may not have run, perhaps partly; check before running any again: {}", calls.join("; ")))
}

/// A turn's last `NOTE_REPLIES_MAX` replies, a line each.
fn replied(replies: &[String]) -> impl Iterator<Item = String> + '_ {
    let shown = replies.len().min(limits::NOTE_REPLIES_MAX);
    replies[replies.len() - shown..].iter().map(|r| format!("It had replied: {}", quoted(r, limits::NOTE_REPLY_MAX_CHARS)))
}

/// One of the agent's turns in a chat that another life ran since the
/// `/data` this life restored ("forgotten", below): what it was asked, and
/// what it replied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgot {
    pub turn: String,
    pub asked: Option<String>,
    pub replies: Vec<String>,
}

/// The agent's turns `turns` (oldest first) as the chat's journal holds
/// them: each one's start on `work` (its cause, which `chat` holds when it
/// is this chat's message) and its replies on `chat`. A turn whose start
/// is not among the records read is left out (nothing of it can be said),
/// and so is one a restart cut: it is the agent's turn before this one
/// (one turn of an agent runs in a chat at a time, and the cut one was its
/// life's last), whose own note says what was cut (`previous`, `text`).
/// Pure, as `previous` is.
///
/// "Forgotten": a rollback of `/data` sends the bridge's cursors back, and
/// the turns it reads again were claimed by the life that ran them (their
/// claims answer 409), so its runtime, restored from the older save,
/// remembers none of them. The agent's next turn in the chat is told what
/// they were, from the journal, which never goes back in time.
pub fn forgotten(work: &[Record], chat: &[Record], agent: &str, turns: &[String]) -> Vec<Forgot> {
    let mut out = Vec::with_capacity(turns.len());
    for turn in turns {
        let start = work.iter().find(|r| own(r, agent, "turn.start").is_some_and(|b| b["turn"] == turn.as_str() && b["agent"] == agent));
        let Some(start) = start else { continue };
        let Ok(cause) = serde_json::from_value::<Cause>(start.body["cause"].clone()) else { continue };
        let cut = work.iter().filter_map(|r| own(r, agent, "turn.end")).any(|e| e["turn"] == turn.as_str() && e["outcome"] == "error" && field(e, "error") == LOST);
        if cut {
            continue;
        }
        let asked = chat.iter().find(|r| r.channel == records::CHAT && r.seq == cause.seq && cause.channel == records::CHAT).and_then(asked);
        out.push(Forgot { turn: turn.clone(), asked, replies: replies(chat, agent, turn) });
    }
    assert!(out.len() <= turns.len(), "each forgotten turn is told once at most");
    out
}

/// What the agent is told of its forgotten turns in the chat, bounded
/// (`NOTE_MAX_BYTES`): what happened and what to do first, then each
/// turn's request and its last replies, oldest first, and how many earlier
/// ones there were.
pub fn forgotten_text(items: &[Forgot], more: u32) -> String {
    let mut lines = vec![
        "Your memory of this chat is behind: your computer went back to an earlier save, so you do not remember these turns of yours here, which came after it (the chat keeps them). What they did may have had effects: check before you do any of it again.".to_string(),
    ];
    if more > 0 {
        lines.push(format!("(And {more} earlier.)"));
    }
    for f in items {
        let asked = f.asked.as_deref().filter(|a| !a.trim().is_empty()).map_or_else(|| "something".to_string(), |a| quoted(a, limits::NOTE_ASKED_MAX_CHARS));
        lines.push(format!("You were asked: {asked}"));
        let shown = f.replies.len().min(limits::NOTE_REPLIES_MAX);
        for r in &f.replies[f.replies.len() - shown..] {
            lines.push(format!("You replied: {}", quoted(r, limits::NOTE_REPLY_MAX_CHARS)));
        }
    }
    let note = records::cut_bytes(&lines.join("\n"), limits::NOTE_MAX_BYTES);
    assert!(note.len() <= limits::NOTE_MAX_BYTES, "a note is bounded");
    note
}

/// The note a chat continued from its owner's Session Archive carries at
/// its agent's first turn there (docs/chat-records.md, "History"): the
/// conversation it goes on from, what was summarized of it, and the newest
/// of what was said, the agent's own as "you" (it is the same agent, moved
/// here), and where the whole of it is. From the `history` record `r` alone,
/// and only its owner's (`owner`, the agent's owner): anyone else's tells
/// nothing. At most `HISTORY_NOTE_MAX_BYTES`, the oldest messages dropped
/// first.
pub fn history(r: &Record, owner: &str) -> Option<String> {
    let b = &r.body;
    if r.principal != owner || b["kind"] != "history" {
        return None;
    }
    let short = |k: &str, max: usize| records::cut(field(b, k), max);
    let day = |k: &str| field(b, k).get(..10).filter(|d| d.bytes().all(|c| c.is_ascii_digit() || c == b'-')).map(str::to_string);
    let when = match (day("started_at"), day("ended_at")) {
        (Some(a), Some(z)) if a != z => format!(" ({a} to {z})"),
        (Some(d), _) | (None, Some(d)) => format!(" ({d})"),
        (None, None) => String::new(),
    };
    let title = Some(short("title", 120)).filter(|t| !t.is_empty()).unwrap_or_else(|| "an earlier chat".into());
    let named = |k: &str, ok: fn(u8) -> bool| Some(field(b, k)).filter(|v| !v.is_empty() && v.len() <= 160 && v.bytes().all(ok));
    let archive = named("archive", |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    let key = named("key", |c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
    // an archived chat of theirs (`chat`), or an import's; with another
    // agent (`agent`, its name), or with this one
    let chat = named("chat", |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    let other = Some(short("agent", 120)).filter(|a| !a.trim().is_empty());
    let with = other.as_deref().unwrap_or("you");
    let moved = if chat.is_some() { "" } else { " before they moved here" };
    let mut head = vec![format!(
        "This chat goes on from a conversation your owner had with {with}{moved}, “{title}”{when}. It is read-only history: what was said there, not new messages."
    )];
    if let Some(c) = chat {
        head.push(format!("All of it is in their chat {c}."));
    } else if let (Some(a), Some(k)) = (archive, key) {
        head.push(format!("All of it is in their Session Archive: `fragment sync {a} --dir <folder> --mode pull`, then sessions/{k}.json."));
    }
    let summary = field(b, "summary").trim();
    if !summary.is_empty() {
        head.push(format!("What came before, as it was summarized then:\n{}", records::cut_bytes(summary, limits::HISTORY_NOTE_MAX_BYTES / 3)));
    }
    let said: Vec<String> = b["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let text = m["text"].as_str().map(str::trim).filter(|t| !t.is_empty())?;
            let who = match m["role"].as_str() {
                Some("user") => "Your owner",
                Some("assistant") => other.as_deref().unwrap_or("You"),
                _ => return None,
            };
            Some(format!("{who}: {}", records::cut_bytes(text, limits::HISTORY_NOTE_MAX_BYTES / 4)))
        })
        .collect();
    let earlier = b["earlier"].as_u64().unwrap_or(0);
    let head = head.join("\n\n");
    // the newest that fit beside the head, oldest first
    let mut room = limits::HISTORY_NOTE_MAX_BYTES.saturating_sub(head.len() + 200);
    let mut kept: Vec<&String> = Vec::new();
    for line in said.iter().rev() {
        if line.len() + 1 > room {
            break;
        }
        room -= line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    let dropped = earlier + (said.len() - kept.len()) as u64;
    let mut text = head;
    if !kept.is_empty() {
        let place = if chat.is_some() { "that chat" } else { "the archive" };
        let before = if dropped > 0 { format!(" ({dropped} earlier ones are in {place})") } else { String::new() };
        text.push_str(&format!("\n\nIts last messages, oldest first{before}:\n"));
        text.push_str(&kept.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n"));
    }
    let text = records::cut_bytes(&text, limits::HISTORY_NOTE_MAX_BYTES);
    assert!(text.len() <= limits::HISTORY_NOTE_MAX_BYTES, "a history note is bounded");
    Some(text)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::records::{turn_end, turn_prompt_closed, turn_start, turn_step, Closed, Outcome, Step};

    const ME: &str = "npub1juniper";
    const LIFE: &str = "0123456789abcdef0123456789abcdef";

    /// A `work` channel, as the agent `ME` (and another agent) post it.
    #[derive(Default)]
    struct Work(Vec<Record>);

    impl Work {
        fn post(&mut self, by: &str, body: Value) -> u64 {
            let seq = self.0.len() as u64 + 1;
            self.0.push(Record { channel: "work".into(), seq, at: seq as i64 * 10, principal: by.into(), kind: "message".into(), body });
            seq
        }

        fn start(&mut self, by: &str, turn: &str, seq: u64) -> u64 {
            let cause = Cause { fragment: "talk--k3x9".into(), channel: "chat".into(), seq };
            self.post(by, turn_start(turn, "npub1paul", by, &cause, LIFE))
        }

        fn end(&mut self, by: &str, turn: &str, outcome: Outcome) -> u64 {
            self.post(by, turn_end(turn, &outcome))
        }

        /// The records before `seq` (a claim's), as the driver reads them.
        fn before(&self, seq: u64) -> &[Record] {
            &self.0[..usize::try_from(seq - 1).unwrap()]
        }
    }

    fn history_record(by: &str, body: Value) -> Record {
        Record { channel: "chat".into(), seq: 1, at: 10, principal: by.into(), kind: "message".into(), body }
    }

    /// Goal: a chat continued from its owner's Session Archive tells its
    /// agent what was said there, its own words as "you", and where the
    /// rest is. Invalid: anyone else's history, another kind. Bounded: the
    /// oldest messages give way first.
    #[test]
    fn a_continued_chat_tells_its_agent_its_history() {
        let body = json!({ "kind": "history", "title": "Beans", "archive": "sessions--k3x9", "key": "hermes-20261001_ab", "started_at": "2026-10-01T09:00:00.000Z",
            "ended_at": "2026-10-04T09:00:00.000Z", "summary": "We planned the beds.", "earlier": 3,
            "messages": [{ "role": "user", "text": "plant beans?" }, { "role": "assistant", "text": "in spring" }, { "role": "system", "text": "skipped" }] });
        let text = history(&history_record("npub1paul", body.clone()), "npub1paul").expect("its owner's history");
        assert!(text.contains("“Beans” (2026-10-01 to 2026-10-04)"), "{text}");
        assert!(text.contains("`fragment sync sessions--k3x9 --dir <folder> --mode pull`, then sessions/hermes-20261001_ab.json"), "{text}");
        assert!(text.contains("summarized then:\nWe planned the beds."), "{text}");
        assert!(text.ends_with("Its last messages, oldest first (3 earlier ones are in the archive):\nYour owner: plant beans?\nYou: in spring"), "{text}");
        assert_eq!(history(&history_record("npub1mallory", body.clone()), "npub1paul"), None, "only its owner's");
        let mut message = body.clone();
        message["kind"] = json!("message");
        assert_eq!(history(&history_record("npub1paul", message), "npub1paul"), None);
        // a name that is no archive's or key's leaves the pointer out
        let mut odd = body.clone();
        odd["archive"] = json!("../x");
        assert!(!history(&history_record("npub1paul", odd), "npub1paul").unwrap().contains("fragment sync"));
        // past its bound, the newest messages are kept and the rest counted
        let long: Vec<Value> = (0..40).map(|i| json!({ "role": "user", "text": format!("{i} {}", "w".repeat(2_000)) })).collect();
        let big = history(&history_record("npub1paul", json!({ "kind": "history", "title": "Long", "messages": long })), "npub1paul").unwrap();
        assert!(big.len() <= limits::HISTORY_NOTE_MAX_BYTES, "{}", big.len());
        assert!(big.contains("Your owner: 39 ") && !big.contains("Your owner: 0 ") && big.contains("earlier ones are in the archive"), "the newest kept");
    }

    /// Goal: a history continued with another agent than the one the
    /// conversation was with (`agent`) tells this one whose words they
    /// were, never "you"; one from an archived chat (`chat`) says that chat
    /// is where the rest is. Valid: both; a history saying neither reads as
    /// before.
    #[test]
    fn a_history_says_whose_words_and_where_the_rest_is() {
        let body = json!({ "kind": "history", "title": "Beans", "chat": "fred-chat--k3x9", "agent": "Fred", "started_at": "2026-10-01T09:00:00.000Z", "earlier": 2,
            "messages": [{ "role": "user", "text": "plant beans?" }, { "role": "assistant", "text": "in spring" }] });
        let text = history(&history_record("npub1paul", body), "npub1paul").expect("its owner's history");
        assert!(text.starts_with("This chat goes on from a conversation your owner had with Fred, “Beans” (2026-10-01)."), "{text}");
        assert!(text.contains("All of it is in their chat fred-chat--k3x9."), "{text}");
        assert!(text.ends_with("(2 earlier ones are in that chat):\nYour owner: plant beans?\nFred: in spring"), "{text}");
        assert!(!text.contains("You:") && !text.contains("moved here") && !text.contains("fragment sync"), "{text}");
        let imported = json!({ "kind": "history", "title": "Beans", "archive": "sessions--k3x9", "key": "hermes-ab", "agent": "Juniper", "messages": [{ "role": "assistant", "text": "hi" }] });
        let text = history(&history_record("npub1paul", imported), "npub1paul").unwrap();
        assert!(text.contains("had with Juniper before they moved here") && text.ends_with("Juniper: hi") && text.contains("sessions/hermes-ab.json"), "{text}");
    }

    fn step(tool: &str, args: &str, ok: bool) -> Step {
        Step { tool: tool.into(), args: args.into(), ok, excerpt: String::new(), category: crate::records::Category::Shell, evidence: None }
    }

    /// A step that ended the call `id` of `response` (`None`: an older
    /// image's, naming none).
    fn ended(response: Option<&str>, id: &str, tool: &str, args: &str) -> Step {
        let evidence = crate::records::ToolEvidence { tool_call_id: id.into(), status: crate::records::ToolStatus::Completed, duration_ms: 5, response: response.map(Into::into) };
        Step { evidence: Some(evidence), ..step(tool, args, true) }
    }

    fn call(response: &str, position: u32, id: &str, tool: &str, args: &str) -> crate::records::Call {
        crate::records::Call { response: response.into(), position, tool_call_id: id.into(), tool: tool.into(), args: args.into(), category: crate::records::Category::Shell }
    }

    /// Goal (FIN-236): a message handed at least `MESSAGE_AGE_TOLD_MS` after
    /// it was said is told its age in plain words; sooner, or a routine's or
    /// a command's (`exempt`), or a time unknown (0, or one ahead of the
    /// clock), none. A continuation is told what its cut turn answered.
    #[test]
    fn a_late_message_is_told_its_age() {
        let (minute, now) = (60_000_i64, 100 * 86_400_000_u64);
        let at = |late: i64| i64::try_from(now).expect("a time") - late;
        let bound = i64::try_from(limits::MESSAGE_AGE_TOLD_MS).expect("a bound");
        let told = |late: i64| age(at(late), now, false, false);
        assert_eq!(told(1_000), None, "fresh");
        assert_eq!(told(bound - 1), None, "under the bound");
        let then = " If it may no longer be wanted, check before acting on it.";
        for (late, ago) in [(bound, "5 minutes"), (119 * minute, "119 minutes"), (3 * 60 * minute + 59_000, "3 hours"), (47 * 60 * minute, "47 hours"), (3 * 1440 * minute, "3 days")] {
            assert_eq!(told(late), Some(format!("This message was sent about {ago} ago, and is only reaching you now.{then}")), "{late} ms");
        }
        assert_eq!(age(at(2 * 60 * minute), now, false, true), Some(format!("What that turn was answering was sent about 2 hours ago.{then}")));
        assert_eq!(age(at(3 * 60 * minute), now, true, false), None, "a routine's or a command's");
        assert_eq!(age(0, now, false, false), None, "a time unknown");
        assert_eq!(age(at(-minute), now, false, false), None, "a time ahead of the clock");
    }

    /// Goal (phase 4): a cut turn's note names, first, each call it asked
    /// for (`turn.call`, recorded before it ran) with no step ending it:
    /// each may or may not have run. A step ends a call of its own response
    /// alone, by its id or (its runtime renamed it before it ran) its tool
    /// and arguments: a provider's id reused by a later response
    /// (`call_0`, every response), with another preview or the same, is
    /// another call, which the earlier one's step never ends. Another
    /// agent's call, or another turn's, is not this one's. Past
    /// `NOTE_STEPS_MAX` they are counted. Replay: a call said again is one
    /// record (`call_id`), and its reused id in a later response another.
    #[test]
    fn a_cut_turns_unended_calls_are_named_first() {
        use crate::records::{call_id, turn_call};
        let mut w = Work::default();
        w.start(ME, "cut", 3);
        // the first response: two calls, both ended (one renamed first)
        w.post(ME, turn_call("cut", &call("r1", 0, "call_0", "terminal", "ls")));
        w.post(ME, turn_call("cut", &call("r1", 1, "call_1", "write_file", "/tmp/a")));
        w.post(ME, turn_step("cut", 1, &ended(Some("r1"), "call_0", "terminal", "ls")));
        w.post(ME, turn_step("cut", 2, &ended(Some("r1"), "call_1_d2", "write_file", "/tmp/a")));
        // the second reuses its ids, one with another preview, one the
        // same, and a restart cuts it before either ends
        w.post(ME, turn_call("cut", &call("r2", 0, "call_0", "terminal", "make deploy")));
        w.post(ME, turn_call("cut", &call("r2", 1, "call_1", "terminal", "ls")));
        w.post("npub1rowan", turn_call("cut", &call("r2", 2, "c9", "terminal", "not mine")));
        w.post(ME, turn_call("other", &call("r2", 3, "c8", "terminal", "another turn's")));
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        let next = w.start(ME, "next", 9);
        let Before::Cut(c) = previous(w.before(next), ME, true) else { panic!("a cut turn") };
        let named = |c: &Cut| c.unended.iter().map(|c| format!("{} {}", c.tool, c.args)).collect::<Vec<_>>();
        assert_eq!(named(&c), ["terminal make deploy", "terminal ls"], "{c:?}");
        let note = text(&c, Some("ship it"), &[]);
        assert_eq!(
            note,
            "Your previous turn in this chat was cut short: your computer restarted before it finished. Check what it already did before you do any of it again; the message below is new.\n\
             It was answering: “ship it”\n\
             It had started these and never heard back, so each may or may not have run, perhaps partly; check before running any again: terminal make deploy; terminal ls\n\
             Its steps, as recorded: terminal ls (ok); write_file /tmp/a (ok)"
        );
        assert_eq!(call_id("cut", "r2", 0), call_id("cut", "r2", 0), "said again, one record");
        assert_ne!(call_id("cut", "r2", 0), call_id("cut", "r1", 0), "its id reused by a later response, another call");
        assert_ne!(call_id("cut", "r2", 0), call_id("cut", "r2", 1));
        assert_ne!(call_id("cut", "r2", 0), call_id("other", "r2", 0));

        // a step of a response whose calls never landed ends none of
        // another response's; an older image's step, naming none, pairs
        // among all the turn's
        let mut w = Work::default();
        w.start(ME, "cut", 3);
        w.post(ME, turn_call("cut", &call("r2", 0, "call_0", "terminal", "make deploy")));
        w.post(ME, turn_step("cut", 1, &ended(Some("r1"), "call_0", "terminal", "make deploy")));
        w.post(ME, turn_call("cut", &call("r3", 0, "call_7", "terminal", "make test")));
        w.post(ME, turn_step("cut", 2, &ended(None, "call_7", "terminal", "make test")));
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        let next = w.start(ME, "next", 9);
        let Before::Cut(c) = previous(w.before(next), ME, true) else { panic!("a cut turn") };
        assert_eq!(named(&c), ["terminal make deploy"], "{c:?}");

        let mut w = Work::default();
        w.start(ME, "many", 3);
        for i in 0..limits::NOTE_STEPS_MAX + 3 {
            w.post(ME, turn_call("many", &call("r1", i as u32, &format!("c{i}"), "terminal", &format!("step {i}"))));
        }
        w.end(ME, "many", Outcome::Error(LOST.into()));
        let next = w.start(ME, "next", 9);
        let Before::Cut(c) = previous(w.before(next), ME, true) else { panic!("a cut turn") };
        assert_eq!((c.unended.len(), c.unended_total), (limits::NOTE_STEPS_MAX, limits::NOTE_STEPS_MAX + 3));
        assert!(text(&c, None, &[]).contains("; and 3 more"));
    }

    /// Goal (P5): the turn after a cut one is told of it, with its steps and
    /// its card as the journal recorded them, and the turn after that is told
    /// nothing: the note is said once.
    #[test]
    fn the_turn_after_a_cut_one_is_told_once() {
        let mut w = Work::default();
        w.start(ME, "t1", 1);
        w.end(ME, "t1", Outcome::Idle);
        w.start(ME, "cut", 3);
        w.post(ME, turn_step("cut", 1, &step("terminal", "`ls -la`", true)));
        w.post("npub1rowan", turn_step("other", 1, &step("search", "not mine", true)));
        w.post(ME, turn_step("cut", 2, &step("terminal", "rm -rf /tmp/x", false)));
        w.post(ME, json!({ "kind": "turn.prompt", "turn": "cut", "prompt": "p1", "text": "Run `rm -rf /tmp/x`?", "options": [], "asks": "npub1paul", "expiresAt": 1 }));
        w.post(ME, turn_prompt_closed("cut", "p1", Closed::Expired, None));
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        let next = w.start(ME, "next", 9);

        let Before::Cut(c) = previous(w.before(next), ME, true) else { panic!("the turn after a cut one is told of it") };
        assert_eq!(c.turn, "cut");
        assert_eq!(c.cause, Cause { fragment: "talk--k3x9".into(), channel: "chat".into(), seq: 3 });
        assert_eq!(c.steps, vec![CutStep { tool: "terminal".into(), args: "`ls -la`".into(), ok: true }, CutStep { tool: "terminal".into(), args: "rm -rf /tmp/x".into(), ok: false }], "its own steps, in order, another agent's left out");
        assert_eq!(c.cards, vec![CutCard { text: "Run `rm -rf /tmp/x`?".into(), closed: Some("expired".into()) }]);
        let note = text(&c, Some("do the risky thing"), &["Starting.".into()]);
        assert_eq!(
            note,
            "Your previous turn in this chat was cut short: your computer restarted before it finished. Check what it already did before you do any of it again; the message below is new.\n\
             It was answering: “do the risky thing”\n\
             Its steps, as recorded: terminal `ls -la` (ok); terminal rm -rf /tmp/x (failed)\n\
             It asked: “Run `rm -rf /tmp/x`?” (expired)\n\
             It had replied: “Starting.”"
        );

        // the turn after: the turn before it is `next`, which was not cut
        w.end(ME, "next", Outcome::Idle);
        let after = w.start(ME, "after", 11);
        assert_eq!(previous(w.before(after), ME, true), Before::Kept, "said once");
        // and the cut turn's own claim is told of the turn before it, not of itself
        assert_eq!(previous(w.before(3), ME, true), Before::Kept);
    }

    /// Goal: the journal says a turn waited on its person as its own state
    /// would: while a card of its stayed open, or once it asked in words.
    /// Valid: its card closed (answered or expired), steps alone. Invalid:
    /// another agent's card under the same turn, a turn not in the records.
    #[test]
    fn a_turn_waited_while_its_card_stayed_open_or_once_it_asked_in_words() {
        let card = |turn: &str, prompt: &str| json!({ "kind": "turn.prompt", "turn": turn, "prompt": prompt, "text": "Run it?", "options": [], "asks": "npub1paul", "expiresAt": 1 });
        let mut w = Work::default();
        w.start(ME, "ran", 1);
        w.post(ME, turn_step("ran", 1, &step("terminal", "ls", true)));
        w.start(ME, "carded", 2);
        w.post(ME, card("carded", "p1"));
        w.start(ME, "answered", 3);
        w.post(ME, card("answered", "p2"));
        w.post(ME, turn_prompt_closed("answered", "p2", Closed::Answered, Some(("once", "npub1paul", None))));
        w.start(ME, "expired", 4);
        w.post(ME, card("expired", "p3"));
        w.post(ME, turn_prompt_closed("expired", "p3", Closed::Expired, None));
        w.start(ME, "worded", 5);
        w.post(ME, records::turn_asked("worded", "npub1paul"));
        w.post("npub1rowan", card("ran", "p4"));
        let waits = |turn: &str| waited(&w.0, ME, turn);
        assert!(waits("carded") && waits("worded"), "an open card, a question in words");
        assert!(!waits("answered") && !waits("expired") && !waits("ran"), "a card closed, steps alone, another agent's card");
        assert!(!waits("nowhere"), "a turn not in the records");
    }

    /// Goal: only a recorded loss is a cut. A turn that ended any other way,
    /// one whose end never landed, or none at all, is told nothing; a
    /// refusal (it never ran) is passed over, so the turn after a cut one
    /// is told of it even with a refusal between; another agent's cut turn
    /// is not this agent's.
    #[test]
    fn only_a_recorded_loss_is_told() {
        for outcome in [Outcome::Idle, Outcome::Stopped, Outcome::Error("the agent stopped answering".into())] {
            let mut w = Work::default();
            w.start(ME, "t", 1);
            w.end(ME, "t", outcome.clone());
            let n = w.start(ME, "n", 2);
            assert_eq!(previous(w.before(n), ME, true), Before::Kept, "{outcome:?}");
        }
        let mut w = Work::default();
        w.start(ME, "t", 1);
        let n = w.start(ME, "n", 2);
        assert_eq!(previous(w.before(n), ME, true), Before::Kept, "no end landed: no recorded loss");
        assert_eq!(previous(&[], ME, true), Before::Kept, "the first turn");
        assert_eq!(previous(&[], ME, false), Before::Unread, "nothing read yet");

        let mut w = Work::default();
        w.start(ME, "cut", 1);
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        for (i, why) in [REFUSED_QUEUED, REFUSED_BUSY].into_iter().enumerate() {
            let t = format!("refused{i}");
            w.start(ME, &t, 2 + i as u64);
            w.end(ME, &t, Outcome::Error(why.into()));
        }
        w.start("npub1rowan", "rowans", 5);
        w.end("npub1rowan", "rowans", Outcome::Error(LOST.into()));
        let n = w.start(ME, "n", 6);
        assert!(matches!(previous(w.before(n), ME, true), Before::Cut(c) if c.turn == "cut"), "refusals and another agent's turns passed over");
        assert!(matches!(previous(w.before(n), "npub1rowan", true), Before::Cut(c) if c.turn == "rowans"), "rowan's turn before is rowan's own");

        // a forged start (another principal naming this agent) is no turn of its
        let mut w = Work::default();
        let cause = Cause { fragment: "talk--k3x9".into(), channel: "chat".into(), seq: 1 };
        w.post("npub1mallory", turn_start("forged", "npub1paul", ME, &cause, LIFE));
        w.post("npub1mallory", turn_end("forged", &Outcome::Error(LOST.into())));
        let n = w.start(ME, "n", 2);
        assert_eq!(previous(w.before(n), ME, true), Before::Kept);
    }

    /// Goal: a run of records that does not reach back to the agent's turn
    /// before says so (`Unread`), so the driver reads further back; once it
    /// does, the answer is the whole channel's.
    #[test]
    fn a_short_read_reads_further_back() {
        let mut w = Work::default();
        w.start(ME, "cut", 1);
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        for i in 0..5 {
            w.start("npub1rowan", &format!("r{i}"), 10 + i);
            w.end("npub1rowan", &format!("r{i}"), Outcome::Idle);
        }
        let n = w.start(ME, "n", 20);
        let before = w.before(n);
        assert_eq!(previous(&before[4..], ME, false), Before::Unread, "the tail holds only rowan's");
        // the end alone, its start not read yet, is no answer either
        assert_eq!(previous(&before[1..], ME, false), Before::Unread);
        assert!(matches!(previous(before, ME, false), Before::Cut(c) if c.turn == "cut"));
    }

    /// Goal: a note is bounded whatever the cut turn did (a long request,
    /// many steps, long replies), and its first line, what it is and what
    /// to do, is always whole.
    #[test]
    fn a_note_is_bounded() {
        let mut w = Work::default();
        w.start(ME, "cut", 1);
        for n in 1..=150 {
            w.post(ME, turn_step("cut", n, &step(&"t".repeat(140), &"a".repeat(140), true)));
        }
        for n in 0..20 {
            w.post(ME, json!({ "kind": "turn.prompt", "turn": "cut", "prompt": format!("p{n}"), "text": "x".repeat(2000), "options": [], "asks": "npub1paul", "expiresAt": 1 }));
        }
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        let n = w.start(ME, "n", 2);
        let Before::Cut(c) = previous(w.before(n), ME, true) else { panic!("cut") };
        assert_eq!((c.steps.len(), c.steps_total, c.cards.len()), (limits::NOTE_STEPS_MAX, 150, limits::NOTE_CARDS_MAX));
        // as long as the records allow, ASCII: every part fits whole
        let long = "x".repeat(10_000);
        let note = text(&c, Some(&long), &vec![long.clone(); 10]);
        assert!(note.len() <= limits::NOTE_MAX_BYTES, "{}", note.len());
        assert!(note.contains("and 142 more") && note.lines().filter(|l| l.starts_with("It had replied: ")).count() == limits::NOTE_REPLIES_MAX && !note.ends_with('…'), "{note}");
        // far from ASCII: the tail is cut, marked, and the first line is whole
        let long = "é".repeat(10_000);
        let note = text(&c, Some(&long), &vec![long.clone(); 10]);
        assert!(note.len() <= limits::NOTE_MAX_BYTES, "{}", note.len());
        assert!(note.starts_with("Your previous turn in this chat was cut short") && note.lines().next().unwrap().ends_with("the message below is new."));
        // what was asked, read strictly from its record
        let msg = Record { channel: "chat".into(), seq: 1, at: 0, principal: "npub1paul".into(), kind: "message".into(), body: json!({ "text": "do\nthe risky thing" }) };
        assert_eq!(asked(&msg).as_deref(), Some("do\nthe risky thing"));
        let routine = Record { channel: "tasks".into(), body: json!({ "kind": "routine", "text": "water the plants", "chat": "talk--k3x9" }), ..msg.clone() };
        assert_eq!(asked(&routine).as_deref(), Some("your routine: water the plants"));
        assert_eq!(asked(&Record { body: json!({ "kind": "stop" }), ..msg.clone() }), None);
        let reply = |turn: &str, by: &str, text: &str| Record { principal: by.into(), body: json!({ "text": text, "turn": turn }), ..msg.clone() };
        assert_eq!(replies(&[reply("cut", ME, "one"), reply("other", ME, "x"), reply("cut", "npub1rowan", "y"), reply("cut", ME, "two")], ME, "cut"), vec!["one".to_string(), "two".to_string()]);
        // on one line, so the note's lines stay its own
        assert!(text(&c, Some("a\n\nb"), &[]).contains("It was answering: “a b”"));
    }

    /// Goal (a rollback): the turns another life ran that this life's
    /// runtime does not remember are told from the journal, oldest first:
    /// what each was asked and what it replied; one a restart cut is the
    /// cut note's, another agent's and a turn the records read do not hold
    /// are left out; the note is bounded, its first line whole. Valid,
    /// invalid.
    #[test]
    fn forgotten_turns_are_told_from_the_journal() {
        let msg = |seq: u64, by: &str, body: Value| Record { channel: "chat".into(), seq, at: seq as i64, principal: by.into(), kind: "message".into(), body };
        let chat = vec![
            msg(1, "id:paul", json!({ "text": "remember the zebra" })),
            msg(2, ME, json!({ "text": "noted: zebra", "turn": "t1" })),
            msg(3, "id:paul", json!({ "text": "and the\ngiraffe" })),
            msg(4, ME, json!({ "text": "noted: giraffe", "turn": "t2" })),
            msg(5, ME, json!({ "text": "both noted", "turn": "t2" })),
            msg(6, "id:paul", json!({ "text": "a slow one" })),
            msg(7, "id:paul", json!({ "text": "rowan, hi" })),
        ];
        let mut w = Work::default();
        w.start(ME, "t1", 1);
        w.end(ME, "t1", Outcome::Idle);
        w.start(ME, "t2", 3);
        w.end(ME, "t2", Outcome::Idle);
        w.start(ME, "cut", 6);
        w.end(ME, "cut", Outcome::Error(LOST.into()));
        w.start("id:rowan", "r1", 7);
        let turns: Vec<String> = ["t1", "t2", "cut", "r1", "nowhere"].iter().map(|t| t.to_string()).collect();
        let got = forgotten(&w.0, &chat, ME, &turns);
        assert_eq!(
            got,
            vec![
                Forgot { turn: "t1".into(), asked: Some("remember the zebra".into()), replies: vec!["noted: zebra".into()] },
                Forgot { turn: "t2".into(), asked: Some("and the\ngiraffe".into()), replies: vec!["noted: giraffe".into(), "both noted".into()] },
            ],
            "its own, in order; the cut one (its own note's), another agent's and one not read left out"
        );
        let note = forgotten_text(&got, 3);
        assert_eq!(
            note,
            "Your memory of this chat is behind: your computer went back to an earlier save, so you do not remember these turns of yours here, which came after it (the chat keeps them). What they did may have had effects: check before you do any of it again.\n\
             (And 3 earlier.)\n\
             You were asked: “remember the zebra”\n\
             You replied: “noted: zebra”\n\
             You were asked: “and the giraffe”\n\
             You replied: “noted: giraffe”\n\
             You replied: “both noted”"
        );
        assert!(forgotten(&w.0, &chat, "id:nobody", &turns).is_empty(), "none of another's");
        // bounded, its first line whole
        let long = Forgot { turn: "t".into(), asked: Some("é".repeat(10_000)), replies: vec!["é".repeat(10_000); 5] };
        let note = forgotten_text(&vec![long; limits::NOTE_FORGOTTEN_MAX], u32::MAX);
        assert!(note.len() <= limits::NOTE_MAX_BYTES && note.lines().next().unwrap().ends_with("before you do any of it again."), "{}", note.len());
        assert!(forgotten_text(&[Forgot { turn: "t".into(), asked: None, replies: vec![] }], 0).ends_with("You were asked: something"));
    }
}
