//! A turn as an ACP client is told it: the records and drafts of one
//! agent's turn in a chat (docs/chat-records.md), read as ACP's session
//! updates (agentclientprotocol.com, protocol version 1). Pure: `Follow`
//! is handed what the chat's live socket carries and answers what to tell
//! the client, so each case is a test with no network.
//!
//! - A **draft** is the reply being written, whole so far: its new words
//!   are an `agent_message_chunk`.
//! - A **reply** (on `chat`) ends that part: what of it no draft said is a
//!   chunk, and the next part starts a paragraph on (ACP's chunks only
//!   append, and a client may show a turn's as one message).
//! - A **call** (`turn.call`) is a `tool_call` in progress, and its
//!   **step** (`turn.step`) that call's `tool_call_update`, completed or
//!   failed. A step no call said is a `tool_call` already ended. A call no
//!   step ended by the turn's end is closed saying it may have run.
//! - A **card** (`turn.prompt`) is said in words, with where to answer it.
//!   It is never ACP's `session/request_permission`: that asks about a
//!   tool call, and a client answers it by its own policy (T3 Code's Auto
//!   approves), where a card is its owner's alone to answer.
//! - **`turn.asked`** ends the prompt (`End::Asked`): the person's next
//!   prompt is the answer, and the same turn goes on with it (`asking`
//!   says which turn waits, from the chat's records, as each prompt comes).
//! - **`turn.end`** ends it: idle, stopped, or its error. A turn that
//!   asked and ends as an error, with nothing said of the answer, ends no
//!   prompt yet (`gone`): most likely a restart cut it (docs/bridge.md:
//!   its computer slept while it waited), and the answer is then a turn of
//!   its own, which is followed. Words of the gone turn that come after
//!   its end (records of two channels reach a socket in no one order) say
//!   it took the answer after all, and are told.
//!
//! Only the agent's own records count (a reader attributes a record by its
//! principal), and only its turn's: the one whose `turn.start` names this
//! prompt's message as its cause, or the one an earlier prompt left asking.

use serde_json::{json, Value};

/// One record of a chat's channel, its body parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub channel: String,
    pub seq: i64,
    pub at: i64,
    pub principal: String,
    pub body: Value,
}

impl Record {
    /// A record as a channel's page holds it (`{channel, seq, at,
    /// principal, body}`), or none for a value that is no record.
    pub fn of(v: &Value) -> Option<Record> {
        Some(Record {
            channel: v["channel"].as_str()?.to_string(),
            seq: v["seq"].as_i64()?,
            at: v["at"].as_i64().unwrap_or(0),
            principal: v["principal"].as_str()?.to_string(),
            body: v.get("body").cloned().unwrap_or(Value::Null),
        })
    }

    fn kind(&self) -> &str {
        self.body["kind"].as_str().unwrap_or("")
    }

    /// Words said: a body with text and no kind but "message"
    /// (docs/chat-records.md; a bare string is a message with that text).
    pub fn said(&self) -> Option<&str> {
        match &self.body {
            Value::String(text) => Some(text),
            body if matches!(body["kind"].as_str(), None | Some("message")) => body["text"].as_str(),
            _ => None,
        }
    }
}

/// How a followed turn left the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// It ended having said what it had to.
    Idle,
    /// Its asker stopped it.
    Stopped,
    /// It ended as an error: why.
    Error(String),
    /// It asked its person something in words and waits, running, for
    /// their next message.
    Asked,
}

/// A call the model asked for that no step has ended.
#[derive(Debug, Clone)]
struct Call {
    id: String,
    response: String,
    tool_call_id: String,
    tool: String,
    args: String,
}

/// One prompt's turn, as its records and drafts come.
#[derive(Debug)]
pub struct Follow {
    /// The agent's identity: only its records count.
    agent: String,
    /// The seq, on `chat`, of the message this prompt posted.
    cause: i64,
    /// A turn an earlier prompt left asking its person in words.
    waiting: Option<String>,
    turn: Option<String>,
    /// The words sent of the reply being written.
    part: String,
    /// The reply before it, whole: a draft that says no more than it was
    /// sent late, and is no new part.
    last: String,
    /// Whether this prompt has sent any words.
    said: bool,
    calls: Vec<Call>,
    end: Option<End>,
    /// The turn that asked, and how it ended, when it ended as an error
    /// with nothing said of the answer: the message may be a turn of its
    /// own, not yet started.
    gone: Option<(String, End)>,
    links: Links,
}

/// Where a chat's own things are, for a client's person to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Links {
    /// The chat, in the shell: where a card is answered.
    pub chat: String,
    /// The chat's blobs (docs/api.md, Blobs): a file is `<blobs>/<sha256>`.
    pub blobs: String,
}

impl Links {
    /// A chat's links on `host`, the platform's origin.
    pub fn of(host: &str, chat: &str) -> Links {
        Links { chat: format!("{host}/?chat={chat}"), blobs: format!("{host}/api/f/{chat}/blobs") }
    }
}

/// The id of the turn `agent` (its fragment) makes of the message at `seq`
/// on `chat`'s `chat` channel, as its bridge names it
/// (docs/chat-records.md, Turns): the first 24 hex digits of SHA-256 of
/// `<agent fragment>|<fragment>/<channel>/<seq>`. A Stop names a turn by
/// it before the turn has started.
pub fn id(agent: &str, chat: &str, seq: i64) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(format!("{agent}|{chat}/chat/{seq}").as_bytes());
    hex::encode(&digest[..12])
}

/// An ACP tool kind for a step's category (docs/chat-records.md,
/// `turn.step`): what a client shows the call as.
pub fn kind(category: &str) -> &'static str {
    match category {
        "shell" => "execute",
        "web" | "browser" => "fetch",
        "read" => "read",
        "write" => "edit",
        _ => "other",
    }
}

/// A call's title: its tool, and its arguments' preview when it has one.
pub fn title(tool: &str, args: &str) -> String {
    match (tool.trim(), args.trim()) {
        ("", "") => "tool".to_string(),
        (tool, "") => tool.to_string(),
        ("", args) => args.to_string(),
        (tool, args) => format!("{tool}: {args}"),
    }
}

/// How a step says its call ended, as ACP's status: `completed` only for a
/// call that completed (an older bridge's step says `ok` alone).
pub fn status(step: &Value) -> &'static str {
    let completed = match step["status"].as_str() {
        Some(status) => status == "completed",
        None => step["ok"] != false,
    };
    if completed {
        "completed"
    } else {
        "failed"
    }
}

/// A chunk of words: `kind` is `agent_message_chunk` or `user_message_chunk`.
pub fn chunk(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

/// A reply's files, each a link to the chat's blob (docs/api.md, Blobs):
/// its poster's own words say what it is.
pub fn files(body: &Value, blobs: &str) -> Vec<Value> {
    let listed = body["attachments"].as_array().map(Vec::as_slice).unwrap_or_default();
    listed
        .iter()
        .filter_map(|f| {
            let sha = f["sha256"].as_str()?;
            let mut link = json!({ "type": "resource_link", "uri": format!("{blobs}/{sha}"), "name": f["name"].as_str().unwrap_or(sha) });
            if let Some(kind) = f["type"].as_str() {
                link["mimeType"] = json!(kind);
            }
            if let Some(size) = f["size"].as_i64() {
                link["size"] = json!(size);
            }
            Some(json!({ "sessionUpdate": "agent_message_chunk", "content": link }))
        })
        .collect()
}

/// A card's question and its choices, in words, and where to answer it.
pub fn card(body: &Value, chat_url: &str) -> String {
    let mut out = body["text"].as_str().unwrap_or("").trim().to_string();
    for option in body["options"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let Some(label) = option["label"].as_str() {
            out.push_str(&format!("\n- {label}"));
        }
    }
    out.push_str(&format!("\n\nAnswer in the chat: {chat_url}"));
    out
}

/// A step as a tool call that has ended.
pub fn stepped(id: &str, step: &Value) -> Value {
    let (tool, args) = (step["tool"].as_str().unwrap_or(""), step["args"].as_str().unwrap_or(""));
    let mut call = json!({
        "sessionUpdate": "tool_call", "toolCallId": id, "title": title(tool, args),
        "kind": kind(step["category"].as_str().unwrap_or("")), "status": status(step),
    });
    if let Some(content) = excerpt(step) {
        call["content"] = content;
    }
    call
}

/// What a step kept of its call's result (a command's exit code, never its
/// output), as a tool call's content.
fn excerpt(step: &Value) -> Option<Value> {
    let text = step["excerpt"].as_str().map(str::trim).filter(|t| !t.is_empty())?;
    Some(json!([{ "type": "content", "content": { "type": "text", "text": text } }]))
}

impl Follow {
    /// The turn of the message at `cause` on `chat`, answered by `agent`
    /// (its identity); `waiting` is a turn an earlier prompt left asking.
    pub fn new(agent: &str, cause: i64, waiting: Option<String>, links: Links) -> Follow {
        Follow {
            agent: agent.to_string(),
            cause,
            waiting,
            turn: None,
            part: String::new(),
            last: String::new(),
            said: false,
            calls: Vec::new(),
            end: None,
            gone: None,
            links,
        }
    }

    /// The turn followed, once its start (or a waiting turn's next record)
    /// has come.
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// How the turn left the prompt, once it has.
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// How the turn that asked ended, while no turn of the message's own
    /// has started: the prompt waits for one.
    pub fn gone(&self) -> Option<&End> {
        self.gone.as_ref().map(|(_, end)| end)
    }

    /// No turn of the message's own came: the turn that asked took the
    /// answer and ended, and its end is the prompt's.
    pub fn settle(&mut self) {
        if self.turn.is_none() && self.end.is_none() {
            self.took_it();
        }
    }

    /// The gone turn is the prompt's after all: followed, and ended as it
    /// did.
    fn took_it(&mut self) {
        if let Some((turn, end)) = self.gone.take() {
            (self.turn, self.end) = (Some(turn), Some(end));
        }
    }

    /// Whether `turn` is the one followed. A turn that asked is taken up
    /// by a record of it that says it `went_on` (words, a call, a step, a
    /// card, another question, its end), since the prompt's message was
    /// then its answer; one that ended first (`gone`), by such a record
    /// that comes after. A notice or a card's closing says no such thing
    /// (a notice may follow its turn's end), and takes up neither.
    fn mine(&mut self, turn: &str, went_on: bool) -> bool {
        if self.turn.is_none() && went_on {
            if self.waiting.as_deref() == Some(turn) {
                self.turn = self.waiting.take();
            } else if self.gone.as_ref().is_some_and(|(gone, _)| gone == turn) {
                self.took_it();
            }
        }
        self.turn.as_deref() == Some(turn)
    }

    /// Words to send, a paragraph on from a part before them.
    fn words(&mut self, new: &str) -> Value {
        let lead = if self.part.is_empty() && self.said { "\n\n" } else { "" };
        self.said = true;
        chunk("agent_message_chunk", &format!("{lead}{new}"))
    }

    /// A draft of `turn` by `principal`: its whole text so far, or none
    /// once it stopped.
    pub fn draft(&mut self, principal: &str, turn: &str, text: Option<&str>) -> Vec<Value> {
        let Some(text) = text else { return Vec::new() };
        if principal != self.agent || !self.mine(turn, true) {
            return Vec::new();
        }
        // sent late, after the reply it drafted: no new part
        if self.part.is_empty() && self.last.starts_with(text) {
            return Vec::new();
        }
        // a draft rewritten says nothing new: its reply settles the part
        let Some(new) = text.strip_prefix(self.part.as_str()).filter(|new| !new.is_empty()) else { return Vec::new() };
        let update = self.words(new);
        self.part = text.to_string();
        vec![update]
    }

    /// A record of the chat's `chat` or `work`.
    pub fn record(&mut self, r: &Record) -> Vec<Value> {
        if r.principal != self.agent {
            return Vec::new();
        }
        match r.channel.as_str() {
            "chat" => self.reply(r),
            "work" => self.work(r),
            _ => Vec::new(),
        }
    }

    fn reply(&mut self, r: &Record) -> Vec<Value> {
        let (Some(text), Some(turn)) = (r.said(), r.body["turn"].as_str()) else { return Vec::new() };
        if !self.mine(turn, true) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let new = match text.strip_prefix(self.part.as_str()) {
            Some(rest) => rest,
            // the draft said other words: the record is the reply
            None => {
                self.part.clear();
                text
            }
        };
        if !new.is_empty() {
            out.push(self.words(new));
        }
        out.extend(files(&r.body, &self.links.blobs));
        self.last = text.to_string();
        self.part.clear();
        out
    }

    fn work(&mut self, r: &Record) -> Vec<Value> {
        let body = &r.body;
        let Some(turn) = body["turn"].as_str() else { return Vec::new() };
        if r.kind() == "turn.start" {
            let caused = body["cause"]["channel"] == "chat" && body["cause"]["seq"].as_i64() == Some(self.cause);
            if caused && body["agent"] == self.agent.as_str() {
                self.turn = Some(turn.to_string());
                (self.waiting, self.gone) = (None, None);
            }
            return Vec::new();
        }
        // the turn that asked ends as an error, and nothing of it said it
        // took the answer: it is gone, and the message may start a turn of
        // its own. One that ends idle or stopped took what the prompt sent
        // (only an answered turn ends idle), and is the prompt's below.
        if r.kind() == "turn.end" && self.turn.is_none() && self.waiting.as_deref() == Some(turn) {
            if let end @ End::Error(_) = ended(body) {
                (self.waiting, self.gone) = (None, Some((turn.to_string(), end)));
                return Vec::new();
            }
        }
        if !self.mine(turn, matches!(r.kind(), "turn.call" | "turn.step" | "turn.prompt" | "turn.asked" | "turn.end")) {
            return Vec::new();
        }
        let text = |key: &str| body[key].as_str().unwrap_or("").to_string();
        match r.kind() {
            "turn.call" => {
                let call = Call { id: format!("{turn}:{}", r.seq), response: text("response"), tool_call_id: text("tool_call_id"), tool: text("tool"), args: text("args") };
                let update = json!({
                    "sessionUpdate": "tool_call", "toolCallId": call.id, "title": title(&call.tool, &call.args),
                    "kind": kind(&text("category")), "status": "in_progress",
                });
                self.calls.push(call);
                vec![update]
            }
            "turn.step" => match self.ended(body) {
                Some(call) => {
                    let mut update = json!({ "sessionUpdate": "tool_call_update", "toolCallId": call.id, "status": status(body) });
                    if let Some(content) = excerpt(body) {
                        update["content"] = content;
                    }
                    vec![update]
                }
                None => vec![stepped(&format!("{turn}:{}", r.seq), body)],
            },
            "turn.prompt" => {
                let said = card(body, &self.links.chat);
                vec![self.words(&said)]
            }
            "turn.asked" => {
                self.end = Some(End::Asked);
                Vec::new()
            }
            "turn.end" => {
                self.end = Some(ended(body));
                // a call no step ended: its outcome is not known
                let open = std::mem::take(&mut self.calls);
                open.iter()
                    .map(|call| {
                        json!({
                            "sessionUpdate": "tool_call_update", "toolCallId": call.id, "status": "failed",
                            "content": [{ "type": "content", "content": { "type": "text", "text": "No step recorded how this call ended: it may have run." } }],
                        })
                    })
                    .collect()
            }
            // a notice, a card's closing, a kind a later bridge adds: nothing to tell
            _ => Vec::new(),
        }
    }

    /// The call a step ended (docs/chat-records.md, `turn.call`), as the
    /// chat's page pairs them (templates/chat/site/chat.js, `unended`): a
    /// call of the step's response (of any, for a step that names none:
    /// the stub's, an older bridge's), the one with its `tool_call_id`,
    /// else (its runtime renamed the call) the first still open with its
    /// tool and arguments. One step ends one call.
    ///
    /// The page pairs once it holds every step, each exact id before any
    /// fallback. A client is told as each step comes, so a step paired by
    /// its tool and arguments is not paired again when a later step names
    /// that call by its id: with two calls alike, one renamed and no
    /// response named, the two are told crossed. No runtime that ships
    /// says steps so (ours name their response, the stub names no id).
    fn ended(&mut self, step: &Value) -> Option<Call> {
        let response = step["response"].as_str();
        let of_response = |c: &Call| response.is_none_or(|named| c.response == named);
        let by_id = step["tool_call_id"].as_str().and_then(|id| self.calls.iter().position(|c| of_response(c) && c.tool_call_id == id));
        let found = by_id.or_else(|| self.calls.iter().position(|c| of_response(c) && step["tool"] == c.tool.as_str() && step["args"] == c.args.as_str()))?;
        Some(self.calls.remove(found))
    }
}

/// How a `turn.end` says its turn ended.
fn ended(body: &Value) -> End {
    match body["outcome"].as_str() {
        Some("stopped") => End::Stopped,
        Some("error") => End::Error(body["error"].as_str().unwrap_or("").to_string()),
        _ => End::Idle,
    }
}

/// The agent's turn that asks `me` something in words and has not gone on
/// (docs/chat-records.md, `turn.asked`), from the newest of the chat's
/// records: a prompt sent now is its answer. None once a later record of
/// the turn says it went on, or `me` has said something to the agent
/// since: a message `to` others alone is no answer to it.
pub fn asking(chat: &[Record], work: &[Record], agent: &str, me: &str) -> Option<String> {
    let asked = work.iter().rev().find(|r| r.principal == agent && r.kind() == "turn.asked")?;
    let turn = asked.body["turn"].as_str()?;
    if asked.body["asks"] != me {
        return None;
    }
    let went_on = work.iter().any(|r| r.seq > asked.seq && r.principal == agent && r.body["turn"] == turn && matches!(r.kind(), "turn.step" | "turn.call" | "turn.end"));
    let to_agent = |r: &Record| r.body["to"].as_array().is_none_or(|to| to.iter().any(|x| x == agent));
    let answered = chat.iter().any(|r| r.at >= asked.at && r.principal == me && r.said().is_some() && to_agent(r));
    (!went_on && !answered).then(|| turn.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "npub1fred";
    const ME: &str = "npub1paul";
    fn links() -> Links {
        Links::of("https://x", "c--k3x9")
    }

    fn work(seq: i64, principal: &str, body: Value) -> Record {
        Record { channel: "work".into(), seq, at: seq * 10, principal: principal.into(), body }
    }

    fn chat(seq: i64, principal: &str, body: Value) -> Record {
        Record { channel: "chat".into(), seq, at: seq * 10, principal: principal.into(), body }
    }

    fn start(seq: i64, turn: &str, cause: i64) -> Record {
        work(seq, AGENT, json!({ "kind": "turn.start", "turn": turn, "asker": ME, "agent": AGENT, "cause": { "fragment": "c--k3x9", "channel": "chat", "seq": cause }, "life": "0" }))
    }

    fn end(seq: i64, turn: &str, outcome: &str) -> Record {
        work(seq, AGENT, json!({ "kind": "turn.end", "turn": turn, "outcome": outcome }))
    }

    /// The words a list of updates says, each chunk as sent.
    fn said(updates: &[Value]) -> Vec<&str> {
        updates.iter().filter(|u| u["sessionUpdate"] == "agent_message_chunk").filter_map(|u| u["content"]["text"].as_str()).collect()
    }

    /// Goal: a reply streams as its drafts grow, and its record adds only
    /// what no draft said; the turn's end is the prompt's. Replay: the same
    /// draft again says nothing.
    #[test]
    fn a_reply_streams_as_its_draft_grows() {
        let mut f = Follow::new(AGENT, 7, None, links());
        assert_eq!(f.draft(AGENT, "t1", Some("early")), Vec::<Value>::new(), "a draft before the turn's start is no turn's yet");
        assert!(f.record(&start(1, "t1", 7)).is_empty());
        assert_eq!(f.turn(), Some("t1"));
        assert_eq!(said(&f.draft(AGENT, "t1", Some("echo: "))), ["echo: "]);
        assert_eq!(said(&f.draft(AGENT, "t1", Some("echo: [paul]"))), ["[paul]"]);
        assert!(f.draft(AGENT, "t1", Some("echo: [paul]")).is_empty(), "the same draft again");
        assert!(f.draft(AGENT, "t1", None).is_empty(), "a draft stopped");
        assert_eq!(said(&f.record(&chat(8, AGENT, json!({ "text": "echo: [paul] hi", "turn": "t1" })))), [" hi"]);
        assert_eq!(f.end(), None);
        assert!(f.record(&end(2, "t1", "idle")).is_empty());
        assert_eq!(f.end(), Some(&End::Idle));
    }

    /// Goal: each reply is a part of its own, a paragraph on from the one
    /// before; a reply no draft said is sent whole. Invalid: a draft that
    /// arrives after the reply it drafted starts no new part, and a draft
    /// rewritten says nothing until its reply settles it.
    #[test]
    fn each_reply_is_a_paragraph_of_its_own() {
        let mut f = Follow::new(AGENT, 7, None, links());
        f.record(&start(1, "t1", 7));
        assert_eq!(said(&f.record(&chat(8, AGENT, json!({ "text": "Let me look.", "turn": "t1", "interim": true })))), ["Let me look."]);
        assert!(f.draft(AGENT, "t1", Some("Let me")).is_empty(), "a draft sent late, after its reply");
        assert_eq!(said(&f.draft(AGENT, "t1", Some("Found"))), ["\n\nFound"]);
        assert!(f.draft(AGENT, "t1", Some("I found")).is_empty(), "rewritten: its reply settles it");
        assert_eq!(said(&f.record(&chat(9, AGENT, json!({ "text": "I found it.", "turn": "t1" })))), ["\n\nI found it."], "the record is the reply when its draft said other words");
        assert_eq!(said(&f.record(&chat(10, AGENT, json!("a bare string is a message")))), Vec::<&str>::new(), "a reply names its turn");
    }

    /// Goal: only the turn's agent's records of the prompt's own turn
    /// count. Invalid: another agent's turn of the same message, the
    /// agent's turn of another message, a person's record naming the turn,
    /// and a start that names another agent.
    #[test]
    fn only_the_prompts_own_turn_counts() {
        let mut f = Follow::new(AGENT, 7, None, links());
        assert!(f.record(&work(1, "npub1maple", json!({ "kind": "turn.start", "turn": "t-maple", "agent": "npub1maple", "cause": { "channel": "chat", "seq": 7 } }))).is_empty());
        assert!(f.record(&start(2, "t-other", 6)).is_empty());
        assert!(f.record(&work(3, AGENT, json!({ "kind": "turn.start", "turn": "t-tasks", "agent": AGENT, "cause": { "channel": "tasks", "seq": 7 } }))).is_empty());
        assert!(f.record(&work(4, AGENT, json!({ "kind": "turn.start", "turn": "t-forged", "agent": "npub1maple", "cause": { "channel": "chat", "seq": 7 } }))).is_empty());
        assert_eq!(f.turn(), None);
        f.record(&start(5, "t1", 7));
        assert!(f.record(&chat(8, AGENT, json!({ "text": "another turn's", "turn": "t-other" }))).is_empty());
        assert!(f.record(&chat(9, ME, json!({ "text": "a person's", "turn": "t1" }))).is_empty());
        assert!(f.draft("npub1maple", "t1", Some("not its agent's")).is_empty());
        assert!(f.record(&work(6, ME, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }))).is_empty());
        assert_eq!(f.end(), None, "a person ends no turn");
        assert!(f.record(&end(7, "t-other", "idle")).is_empty());
        assert_eq!(f.end(), None, "another turn's end");
    }

    /// Goal: a call is a tool call in progress and its step that call's
    /// end; a step no call said is a tool call already ended; a call no
    /// step ended is closed at the turn's end, saying it may have run.
    /// Invalid: a step of another response ends no call; a failed step.
    #[test]
    fn calls_and_steps_are_tool_calls() {
        let mut f = Follow::new(AGENT, 7, None, links());
        f.record(&start(1, "t1", 7));
        let call = |seq, response: &str, id: &str, tool: &str, args: &str| work(seq, AGENT, json!({ "kind": "turn.call", "turn": "t1", "response": response, "tool_call_id": id, "tool": tool, "category": "shell", "args": args }));
        let step = |seq, response: &str, id: &str, tool: &str, args: &str, status: &str| {
            work(seq, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": seq, "response": response, "tool_call_id": id, "tool": tool, "category": "shell", "args": args, "ok": status == "completed", "status": status, "excerpt": "Exit code: 0" }))
        };
        assert_eq!(
            f.record(&call(2, "r1", "call_0", "terminal", "ls -la")),
            [json!({ "sessionUpdate": "tool_call", "toolCallId": "t1:2", "title": "terminal: ls -la", "kind": "execute", "status": "in_progress" })]
        );
        f.record(&call(3, "r1", "call_1", "terminal", "make"));
        // every response's first call may be `call_0`: the response names the call
        f.record(&call(4, "r2", "call_0", "terminal", "pwd"));
        assert_eq!(
            f.record(&step(5, "r2", "call_0", "terminal", "pwd", "completed")),
            [json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1:4", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "Exit code: 0" } }] })]
        );
        // a call its runtime renamed is found by its tool and arguments
        let renamed = f.record(&step(6, "r1", "toolu_9", "terminal", "make", "timed_out"));
        assert_eq!((renamed[0]["toolCallId"].as_str(), renamed[0]["status"].as_str()), (Some("t1:3"), Some("failed")));
        // a step no call said (an older bridge's, with `ok` alone)
        let alone = f.record(&work(7, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": 3, "tool": "search", "category": "web", "args": "weather", "ok": true })));
        assert_eq!(alone, [json!({ "sessionUpdate": "tool_call", "toolCallId": "t1:7", "title": "search: weather", "kind": "fetch", "status": "completed" })]);
        // the first call had no step: closed at the end
        let closed = f.record(&end(8, "t1", "idle"));
        assert_eq!(closed.len(), 1);
        assert_eq!((closed[0]["toolCallId"].as_str(), closed[0]["status"].as_str()), (Some("t1:2"), Some("failed")));
        assert!(closed[0]["content"][0]["content"]["text"].as_str().is_some_and(|t| t.contains("may have run")));
    }

    /// Goal: a step that names no response (a runtime that says none of its
    /// steps: the stub's, an older bridge's) ends a call of any response:
    /// by its `tool_call_id` when it names one, else the first still open
    /// with its tool and arguments, one step to one call, as the chat's
    /// page pairs them (templates/chat/site/chat.js, `unended`). Invalid:
    /// a step of another tool ends none.
    #[test]
    fn a_step_that_names_no_response_ends_its_call() {
        let mut f = Follow::new(AGENT, 7, None, links());
        f.record(&start(1, "t1", 7));
        let call = |seq, id: &str, tool: &str, args: &str| work(seq, AGENT, json!({ "kind": "turn.call", "turn": "t1", "response": "script-abc", "tool_call_id": id, "tool": tool, "category": "web", "args": args }));
        f.record(&call(2, "call-search", "search", "hi"));
        f.record(&call(3, "call-search", "search", "hi"));
        f.record(&call(4, "call-read", "read_file", "a.txt"));
        // as the stub's script says a step: its tool, its arguments, no more
        let bare = |seq, tool: &str, args: &str| work(seq, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": seq, "tool": tool, "category": "web", "args": args, "ok": true, "excerpt": "3 results" }));
        assert_eq!(
            f.record(&bare(5, "search", "hi")),
            [json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1:2", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "3 results" } }] })],
            "the first still open"
        );
        assert_eq!(f.record(&bare(6, "search", "hi"))[0]["toolCallId"], "t1:3", "one step to one call");
        let other = f.record(&bare(7, "terminal", "ls"));
        assert_eq!((other[0]["sessionUpdate"].as_str(), other[0]["toolCallId"].as_str()), (Some("tool_call"), Some("t1:7")), "no call of its tool: a call of its own");
        // by its id alone, whatever its arguments were cut to
        let named = f.record(&work(8, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": 4, "tool": "read", "category": "read", "args": "a", "ok": true, "tool_call_id": "call-read", "status": "completed" })));
        assert_eq!((named[0]["sessionUpdate"].as_str(), named[0]["toolCallId"].as_str()), (Some("tool_call_update"), Some("t1:4")));
        assert!(f.record(&end(9, "t1", "idle")).is_empty(), "every call ended: none left to close");
    }

    /// Goal: how a turn ends is how its prompt does: an error with its
    /// reason, a stop, and a question in words, which leaves the turn
    /// waiting for the next prompt.
    #[test]
    fn a_turn_ends_its_prompt() {
        let ended = |record: Record| {
            let mut f = Follow::new(AGENT, 7, None, links());
            f.record(&start(1, "t1", 7));
            f.record(&record);
            f.end().cloned()
        };
        assert_eq!(ended(work(2, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }))), Some(End::Error("the runtime failed".into())));
        assert_eq!(ended(end(2, "t1", "stopped")), Some(End::Stopped));
        assert_eq!(ended(work(2, AGENT, json!({ "kind": "turn.asked", "turn": "t1", "asks": ME }))), Some(End::Asked));
        assert_eq!(ended(work(2, AGENT, json!({ "kind": "turn.notice", "turn": "t1", "category": "info", "text": "Compressing context" }))), None);
        assert_eq!(ended(work(2, AGENT, json!({ "kind": "turn.later-kind", "turn": "t1" }))), None, "a kind a later bridge adds");
    }

    /// Goal: a prompt after a question in words is its answer: the waiting
    /// turn goes on with it, taken up by its next record or draft. A
    /// message that starts a turn of its own (the waiting one had ended)
    /// is followed as any other.
    #[test]
    fn an_answer_goes_on_in_the_turn_that_asked() {
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        assert_eq!(f.turn(), None);
        assert_eq!(said(&f.draft(AGENT, "t1", Some("(told: Bean)"))), ["(told: Bean)"]);
        assert_eq!(f.turn(), Some("t1"));
        f.record(&chat(10, AGENT, json!({ "text": "(told: Bean)", "turn": "t1" })));
        f.record(&end(3, "t1", "idle"));
        assert_eq!(f.end(), Some(&End::Idle));

        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&start(3, "t2", 9));
        assert_eq!(f.turn(), Some("t2"), "the message started a turn of its own");
        assert!(f.record(&end(4, "t1", "error")).is_empty());
        assert_eq!(f.end(), None, "the turn that had asked is not this prompt's");
    }

    /// Goal: an answer sent after the agent's computer slept is a turn of
    /// its own: the turn that asked ends as lost first (its restart cut
    /// it), which ends no prompt, and the message's own turn is followed.
    /// A turn that asked and ended with nothing said of the answer, and no
    /// turn of the message's own after it, is the prompt's end only once
    /// its caller settles for it.
    #[test]
    fn an_answer_after_a_restart_is_a_turn_of_its_own() {
        let lost = || work(3, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "lost when the computer restarted" }));
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        assert!(f.record(&lost()).is_empty());
        assert_eq!((f.end(), f.turn()), (None, None), "the lost turn's end is not the prompt's");
        assert_eq!(f.gone(), Some(&End::Error("lost when the computer restarted".into())));
        f.record(&start(4, "t2", 9));
        assert_eq!((f.turn(), f.gone()), (Some("t2"), None));
        assert_eq!(said(&f.record(&chat(10, AGENT, json!({ "text": "Bean it is.", "turn": "t2" })))), ["Bean it is."]);
        f.record(&end(5, "t2", "idle"));
        assert_eq!(f.end(), Some(&End::Idle));

        // it took the answer and failed at once, saying nothing: no turn
        // follows, and its end is settled for
        let failed = || work(3, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }));
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&failed());
        assert_eq!((f.end(), f.gone()), (None, Some(&End::Error("the runtime failed".into()))));
        f.settle();
        assert_eq!((f.end(), f.turn()), (Some(&End::Error("the runtime failed".into())), Some("t1")), "settled for: the turn that asked ended");
        // nothing to settle for: a prompt whose turn has not shown itself goes on
        let mut f = Follow::new(AGENT, 9, None, links());
        f.settle();
        assert_eq!(f.end(), None);
    }

    /// Goal: only a record that says the turn went on takes up a turn that
    /// asked (a reply, a draft, a call, a step, a card, another question,
    /// its end). Invalid: a notice or a card's closing says no such thing
    /// (a notice may come after its turn's end): neither takes up the
    /// waiting turn, nor one that ended as lost, so neither fails the
    /// prompt before the answer's own turn starts.
    #[test]
    fn a_notice_takes_up_no_turn_that_asked() {
        let notice = |seq| work(seq, AGENT, json!({ "kind": "turn.notice", "turn": "t1", "category": "memory", "text": "Self-improvement review: Memory updated" }));
        let closed = |seq| work(seq, AGENT, json!({ "kind": "turn.prompt.closed", "turn": "t1", "prompt": "p1", "outcome": "expired" }));
        let lost = |seq| work(seq, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "lost when the computer restarted" }));
        // before the lost end: the turn is not taken up, so its end is no prompt's
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        assert!(f.record(&notice(3)).is_empty() && f.record(&closed(4)).is_empty());
        assert_eq!(f.turn(), None, "a notice says nothing of an answer");
        f.record(&lost(5));
        assert_eq!((f.end(), f.gone().is_some()), (None, true), "its lost end is still no prompt's end");
        // after it: the gone turn is not taken up either
        f.record(&notice(6));
        f.record(&closed(7));
        assert_eq!((f.end(), f.turn(), f.gone().is_some()), (None, None, true));
        f.record(&start(8, "t2", 9));
        f.record(&end(9, "t2", "idle"));
        assert_eq!((f.end(), f.turn()), (Some(&End::Idle), Some("t2")), "the answer's own turn is the prompt's");
        // a step does say it went on
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&work(3, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": 1, "tool": "search", "category": "web", "args": "x", "ok": true })));
        assert_eq!(f.turn(), Some("t1"));
    }

    /// Goal: a turn that asked and then ends idle or stopped took what
    /// the prompt sent (an answer, a Stop): only a turn that was answered
    /// ends idle, so its end is the prompt's at once. And no word of it
    /// is lost to the order records come in: a reply that reaches the
    /// socket after its turn's end (a page of `work` before a page of
    /// `chat`, as a socket dialed again sends them) is still told, before
    /// the end is settled for and after.
    #[test]
    fn a_turn_that_asked_loses_no_words_to_the_order_they_come_in() {
        for (outcome, ends) in [("idle", End::Idle), ("stopped", End::Stopped)] {
            let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
            f.record(&end(3, "t1", outcome));
            assert_eq!((f.end(), f.turn(), f.gone()), (Some(&ends), Some("t1"), None), "{outcome}");
            assert_eq!(said(&f.record(&chat(10, AGENT, json!({ "text": "(told: Bean)", "turn": "t1" })))), ["(told: Bean)"], "its words, read once it ended");
        }
        let failed = || work(3, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }));
        // its error first, then the words it said before failing
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&failed());
        assert_eq!(said(&f.record(&chat(10, AGENT, json!({ "text": "Bean, then.", "turn": "t1" })))), ["Bean, then."]);
        assert_eq!((f.end(), f.turn(), f.gone()), (Some(&End::Error("the runtime failed".into())), Some("t1"), None), "it took the answer after all");
        // settled for, and its words read after
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&failed());
        f.settle();
        assert_eq!(said(&f.record(&chat(10, AGENT, json!({ "text": "Bean, then.", "turn": "t1" })))), ["Bean, then."]);
        // another turn's words are none of it
        let mut f = Follow::new(AGENT, 9, Some("t1".into()), links());
        f.record(&failed());
        assert!(f.record(&chat(10, AGENT, json!({ "text": "another's", "turn": "t7" }))).is_empty());
        assert_eq!(f.turn(), None);
    }

    /// Goal: a card is said in words with its choices and where to answer
    /// it, never as a permission request; a reply's file is a link to the
    /// chat's blob.
    #[test]
    fn a_card_is_said_in_words_and_a_file_is_a_link() {
        let mut f = Follow::new(AGENT, 7, None, links());
        f.record(&start(1, "t1", 7));
        f.record(&chat(8, AGENT, json!({ "text": "One thing first.", "turn": "t1" })));
        let card = f.record(&work(2, AGENT, json!({
            "kind": "turn.prompt", "turn": "t1", "prompt": "p1", "text": "Make it public?",
            "options": [{ "id": "c0", "label": "Yes, make it public" }, { "id": "c1", "label": "No, keep it private" }], "asks": ME, "expiresAt": 1,
        })));
        assert_eq!(said(&card), ["\n\nMake it public?\n- Yes, make it public\n- No, keep it private\n\nAnswer in the chat: https://x/?chat=c--k3x9"]);
        assert!(card.iter().all(|u| u["sessionUpdate"] == "agent_message_chunk"));
        assert!(f.record(&work(3, AGENT, json!({ "kind": "turn.prompt.closed", "turn": "t1", "prompt": "p1", "outcome": "answered", "option": "c1", "by": ME }))).is_empty());
        let sha = "ab".repeat(32);
        let drew = f.record(&chat(9, AGENT, json!({ "text": "Here.", "turn": "t1", "attachments": [{ "sha256": sha, "size": 12, "type": "text/plain", "name": "drawing.txt" }] })));
        assert_eq!(said(&drew), ["\n\nHere."]);
        assert_eq!(
            drew[1],
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "resource_link", "uri": format!("https://x/api/f/c--k3x9/blobs/{sha}"), "name": "drawing.txt", "mimeType": "text/plain", "size": 12 } })
        );
    }

    /// Goal: a chat's records say which turn is asking its person in words
    /// still. Invalid: one that went on, one its person answered since,
    /// another agent's, and a chat where none asked.
    #[test]
    fn a_turn_still_asking_is_read_from_the_records() {
        let asked = |seq, agent: &str| work(seq, agent, json!({ "kind": "turn.asked", "turn": "t1", "asks": ME }));
        assert_eq!(asking(&[], &[start(1, "t1", 7), asked(2, AGENT)], AGENT, ME), Some("t1".into()));
        assert_eq!(asking(&[], &[start(1, "t1", 7)], AGENT, ME), None);
        assert_eq!(asking(&[], &[asked(2, "npub1maple")], AGENT, ME), None);
        assert_eq!(asking(&[], &[asked(2, AGENT), end(3, "t1", "error")], AGENT, ME), None, "it went on");
        assert_eq!(asking(&[chat(1, ME, json!({ "text": "before it asked" }))], &[asked(2, AGENT)], AGENT, ME), Some("t1".into()));
        assert_eq!(asking(&[chat(3, ME, json!({ "text": "Bean" }))], &[asked(2, AGENT)], AGENT, ME), None, "answered since");
        assert_eq!(asking(&[chat(3, ME, json!({ "text": "Bean", "to": [AGENT] }))], &[asked(2, AGENT)], AGENT, ME), None, "answered, by name");
        assert_eq!(asking(&[chat(3, "npub1ada", json!({ "text": "not the one asked" }))], &[asked(2, AGENT)], AGENT, ME), Some("t1".into()));
        // a message to another agent of the chat is no answer to this one
        assert_eq!(asking(&[chat(3, ME, json!({ "text": "and you, maple?", "to": ["npub1maple"] }))], &[asked(2, AGENT)], AGENT, ME), Some("t1".into()));
        // it asked someone else: this person's prompt is no answer
        assert_eq!(asking(&[], &[asked(2, AGENT)], AGENT, "npub1ada"), None);
    }

    /// Goal: a turn's id is the one its bridge gives it (docs/chat-records.md,
    /// Turns), so a Stop can name a turn before it starts. A fixed vector:
    /// SHA-256 of `fred--k3x9|c--a2b3/chat/7`, its first 24 hex digits.
    #[test]
    fn a_turns_id_is_its_bridges() {
        assert_eq!(id("fred--k3x9", "c--a2b3", 7), "7f0da38f5aef585e43909dd2");
        assert_ne!(id("maple--p2m4", "c--a2b3", 7), id("fred--k3x9", "c--a2b3", 7), "two agents answering one message are two turns");
    }

    /// Goal: a record is read from a channel's page as the platform sends
    /// it. Invalid: a value that is no record.
    #[test]
    fn a_record_is_read_from_a_page() {
        let v = json!({ "channel": "chat", "seq": 3, "at": 9, "principal": ME, "kind": "message", "body": { "text": "hi" } });
        let r = Record::of(&v).expect("a record");
        assert_eq!((r.channel.as_str(), r.seq, r.at, r.principal.as_str(), r.said()), ("chat", 3, 9, ME, Some("hi")));
        assert_eq!(Record::of(&json!({ "seq": 3 })), None);
        assert_eq!(chat(1, ME, json!({ "kind": "stop", "turn": "t1" })).said(), None, "a stop says no words");
        assert_eq!(chat(1, ME, json!("bare")).said(), Some("bare"));
    }
}
