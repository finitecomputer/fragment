//! A chat's records (docs/chat-records.md): what the bridge reads from a
//! chat's `chat` channel and an agent's `tasks` channel, what it posts to
//! `chat` and `work`, and the ids that make every post once.
//!
//! Reading is strict at the edges: a body that does not hold its kind's
//! shape is `Said::Other` (a page's own record, or a broken one), never a
//! guess. Building is total: every body here is one the doc defines.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::limits;

/// The chat's channel for what is said: people's messages, agents' replies,
/// Stop, and prompt answers.
pub const CHAT: &str = "chat";
/// The chat's channel for an agent's progress: only the chat's agent
/// members post, each as itself (`"from": "agent"`), so a person cannot
/// forge a step or a prompt.
pub const WORK: &str = "work";
/// An agent fragment's channel for what is asked of it outside a chat.
pub const TASKS: &str = "tasks";

/// A record as the platform gives it (`GET …/channels/{c}`, `__live`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub channel: String,
    pub seq: u64,
    #[serde(default)]
    pub at: i64,
    pub principal: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub body: Value,
}

/// A file a record carries: one of the fragment's blobs, by hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub sha256: String,
    pub size: u64,
    #[serde(rename = "type")]
    pub media_type: String,
    pub name: String,
}

/// A message: a person's, or an agent's reply (it names its `turn`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    pub text: String,
    /// Whom it is for: agents' identities. Empty: whoever the chat's rules
    /// pick (an `@mention`, else the lead).
    pub to: Vec<String>,
    pub attachments: Vec<AttachmentRef>,
    /// An agent's reply names the turn it answers.
    pub turn: Option<String>,
    /// How many agent hand-offs led here (0 for a person's message).
    pub hop: u32,
    /// The seq on `chat` of the message it quotes (a person's reply to it).
    pub reply_to: Option<u64>,
    /// An ask's: the chat its asker's answer runs in (docs/chat-records.md,
    /// "An ask"). Honored only from another agent of this computer.
    pub return_to: Option<String>,
    /// Where its attachments' blobs are: none, the chat it was said in; an
    /// answer's, the chat its question was asked in. Never read from a
    /// message: only an agent's own answer names another chat.
    pub attachments_in: Option<String>,
}

/// A command for an agent's runtime (`{kind: "command"}`), from a person:
/// one of the runtime's menu (`runtime::MenuItem`), answered only from the
/// agent's owner.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandSaid {
    /// `^[a-z0-9_-]{1,32}$`, without its `/`.
    pub name: String,
    /// Its argument, trimmed, at most `MESSAGE_TEXT_MAX_BYTES` (more is cut).
    pub args: String,
    /// The agents it is for; empty: the lead.
    pub to: Vec<String>,
}

/// What a `chat` record says to an agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    Message(Message),
    /// Stop the turn it names (or the asker's running one).
    Stop { turn: Option<String> },
    /// An answer to a prompt: its id, the option picked, the words of an
    /// option answered in words, and the card it answers (`card`: its
    /// agent's identity and its turn, as the chat's page names them). Prompt
    /// ids are each agent's own, so two agents of a chat may show cards of
    /// one id: an answer naming another agent's card is not this agent's.
    PromptResponse { prompt: String, option: String, text: Option<String>, card: Option<Card> },
    /// A command for an agent's runtime.
    Command(CommandSaid),
    /// Anything else: a page's own kind, or a body that is not a message.
    Other,
}

/// The card a prompt's answer names: whose turn asked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// The asking agent's identity (an npub).
    pub agent: String,
    /// Its turn's id.
    pub turn: String,
}

/// What a `tasks` record asks of an agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// A routine fired (the agent fragment's cron posted it): a turn in
    /// `chat`, as the agent's owner asked it.
    Routine { text: String, chat: String },
    /// The agent was added to a fragment: list them again now. That
    /// fragment's members changed, so every view of it read before is stale
    /// (another agent of this computer in it would miss the new one).
    Joined { fragment: Option<String> },
    /// The answer to a question the agent asked another (`answer`): a turn
    /// in `chat`, told `text` by that agent (`by`, its `name`), `hop`
    /// hand-offs deep; `refused`: the question was never taken, and the
    /// turn never runs. `files`: what the asked agent sent with its answer,
    /// blobs of the chat it was asked in (`asked`), which the asker reads
    /// as itself; none from an answer that names no chat, or any it cannot
    /// read whole.
    Answer { chat: String, text: String, by: String, name: String, hop: u32, refused: bool, files: Vec<AttachmentRef>, asked: Option<String> },
    Other,
}

fn text_field(o: &Map<String, Value>, key: &str) -> Option<String> {
    match o.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Whether a record's principal is an identity: an npub (a person, an
/// agent, or a fragment's own key: its cron's and the platform's records,
/// and on an agent's fragment its agent's identity), not `platform` or an
/// anonymous visitor.
pub fn is_identity(principal: &str) -> bool {
    principal.starts_with("npub1")
}

/// Whether `s` is an id of the alphabet records use for prompts and
/// options (`^[A-Za-z0-9._-]{1,max}$`).
pub fn valid_token(s: &str, max: usize) -> bool {
    let len_ok = !s.is_empty() && s.len() <= max;
    len_ok && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `s` is a command's name (`^[a-z0-9_-]{1,32}$`, Hermes' rule for
/// its command manifest's names).
pub fn valid_command(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Whether `s` is a post id the platform takes (`^[A-Za-z0-9._:-]{1,128}$`).
pub fn valid_post_id(s: &str) -> bool {
    let len_ok = !s.is_empty() && s.len() <= 128;
    len_ok && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

fn attachment(v: &Value) -> Option<AttachmentRef> {
    let a: AttachmentRef = serde_json::from_value(v.clone()).ok()?;
    let sha_ok = a.sha256.len() == 64 && a.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    let size_ok = a.size > 0 && a.size <= limits::ATTACHMENT_MAX_BYTES;
    let names_ok = !a.media_type.is_empty() && a.media_type.len() <= 100 && !a.name.is_empty() && a.name.len() <= 200;
    if sha_ok && size_ok && names_ok {
        Some(a)
    } else {
        None
    }
}

/// Reads a `chat` record's body.
pub fn said(body: &Value) -> Said {
    let o = match body {
        Value::String(text) => return Said::Message(Message { text: text.clone(), ..Message::default() }),
        Value::Object(o) => o,
        _ => return Said::Other,
    };
    match o.get("kind") {
        None => message(o),
        Some(Value::String(k)) if k == "message" => message(o),
        Some(Value::String(k)) if k == "stop" => match o.get("turn") {
            None | Some(Value::Null) => Said::Stop { turn: None },
            Some(Value::String(t)) if valid_post_id(t) => Said::Stop { turn: Some(t.clone()) },
            Some(_) => Said::Other,
        },
        Some(Value::String(k)) if k == "command" => command(o),
        Some(Value::String(k)) if k == "prompt_response" => {
            let (prompt, option) = (text_field(o, "prompt"), text_field(o, "option"));
            let text = match o.get("text") {
                None | Some(Value::Null) => None,
                Some(Value::String(t)) if t.trim().is_empty() => None,
                Some(Value::String(t)) => Some(cut_bytes(t.trim(), limits::MESSAGE_TEXT_MAX_BYTES)),
                Some(_) => return Said::Other,
            };
            // a card named whole, or not at all (the API's bare answer)
            let card = match (o.get("agent"), o.get("turn")) {
                (None, None) => None,
                (Some(Value::String(agent)), Some(Value::String(turn))) if is_identity(agent) && agent.len() <= 128 && valid_post_id(turn) => Some(Card { agent: agent.clone(), turn: turn.clone() }),
                _ => return Said::Other,
            };
            match (prompt, option) {
                (Some(p), Some(opt)) if valid_token(&p, 64) && valid_token(&opt, 32) => Said::PromptResponse { prompt: p, option: opt, text, card },
                _ => Said::Other,
            }
        }
        Some(_) => Said::Other,
    }
}

/// The agents a record is `to`: at most 16 identities, or none; `None` for
/// any other shape.
fn to_of(o: &Map<String, Value>) -> Option<Vec<String>> {
    match o.get("to") {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(ids)) if ids.len() <= 16 => {
            let parsed: Vec<String> = ids.iter().filter_map(|v| v.as_str()).filter(|s| is_identity(s) && s.len() <= 128).map(str::to_string).collect();
            (parsed.len() == ids.len()).then_some(parsed)
        }
        Some(_) => None,
    }
}

fn command(o: &Map<String, Value>) -> Said {
    let Some(name) = text_field(o, "command").filter(|n| valid_command(n)) else { return Said::Other };
    let args = match o.get("args") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(a)) => cut_bytes(a.trim(), limits::MESSAGE_TEXT_MAX_BYTES),
        Some(_) => return Said::Other,
    };
    let Some(to) = to_of(o) else { return Said::Other };
    Said::Command(CommandSaid { name, args, to })
}

fn message(o: &Map<String, Value>) -> Said {
    let attachments = match o.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.len() <= limits::ATTACHMENTS_MAX => {
            let parsed: Vec<AttachmentRef> = items.iter().filter_map(attachment).collect();
            if parsed.len() != items.len() {
                return Said::Other;
            }
            parsed
        }
        Some(_) => return Said::Other,
    };
    let text = match o.get("text") {
        Some(Value::String(t)) => t.clone(),
        None if !attachments.is_empty() => String::new(),
        _ => return Said::Other,
    };
    let Some(to) = to_of(o) else { return Said::Other };
    let turn = match o.get("turn") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if valid_post_id(t) => Some(t.clone()),
        Some(_) => return Said::Other,
    };
    let hop = match o.get("hop") {
        None | Some(Value::Null) => 0,
        Some(v) => match v.as_u64().and_then(|h| u32::try_from(h).ok()) {
            Some(h) => h,
            None => return Said::Other,
        },
    };
    let reply_to = match o.get("reply_to") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64().filter(|s| *s >= 1) {
            Some(seq) => Some(seq),
            None => return Said::Other,
        },
    };
    let return_to = match o.get("return_to") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) if fragment_name_ok(c) => Some(c.clone()),
        Some(_) => return Said::Other,
    };
    Said::Message(Message { text, to, attachments, turn, hop, reply_to, return_to, attachments_in: None })
}

/// Whether `s` is a fragment's name in full, in shape: lowercase letters,
/// digits and `-`, at most 128 bytes.
pub fn fragment_name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The door SimpleX's messages come through (docs/chat-records.md, "A
/// relayed message").
pub const VIA_SIMPLEX: &str = "simplex";

/// Whether `door` is a door's id, as the platform's are (a connector's:
/// 1 to 32 lower-case letters, digits and single inner dashes, a letter
/// first; `fragment_core::connectors::id_ok`).
pub fn door_ok(door: &str) -> bool {
    (1..=32).contains(&door.len())
        && door.as_bytes()[0].is_ascii_lowercase()
        && door.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !door.ends_with('-')
        && !door.contains("--")
}

/// What a relayed message says it relays: the person it speaks for, how
/// they said it, and whose door it came through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relayed {
    /// The person whose message it is (an agent relays only its owner's).
    pub for_: String,
    /// The door it came through (`door_ok`): `simplex`, `telegram`, ….
    pub via: String,
    /// The agent fragment whose door it came through.
    pub agent: String,
}

/// A relayed message's `relayed` (docs/chat-records.md): what an agent's
/// owner said elsewhere, carried into a chat through one of that agent's
/// doors (`{"via": <door>, "for": <owner>, "agent": <its fragment>, …}`).
/// The platform takes such a record only through a door its owner opened
/// (signed by the door's key, or published by the connector they bound),
/// and no other. None for any other body, a message of another shape, or
/// a `via` that is no door's id. Whether it is honored (by whom, for whom)
/// is the engine's.
pub fn relayed(body: &Value) -> Option<Relayed> {
    let Value::Object(o) = body else { return None };
    if !matches!(o.get("kind"), None | Some(Value::Null)) && o.get("kind") != Some(&Value::String("message".into())) {
        return None;
    }
    let r = o.get("relayed")?.as_object()?;
    let via = r.get("via")?.as_str().filter(|v| door_ok(v))?.to_string();
    let for_ = r.get("for")?.as_str().filter(|p| is_identity(p) && p.len() <= 128)?.to_string();
    let agent = r.get("agent")?.as_str().filter(|a| fragment_name_ok(a))?.to_string();
    matches!(message(o), Said::Message(_)).then_some(Relayed { for_, via, agent })
}

/// A relayed message's body: `text` (cut as a person's is), `to` its agent
/// alone, what it relays (for its owner, through its door), and its files
/// (`attachments`, uploaded first). Its door signs it as it is posted
/// (simplex/door.rs `Key::seal`).
pub fn relayed_message(text: &str, agent: &crate::runtime::Agent, via: &'static str, attachments: &[AttachmentRef]) -> Value {
    assert!(is_identity(&agent.identity) && is_identity(&agent.owner), "a relayed message is an agent's, for a person");
    let relayed = json!({ "via": via, "for": agent.owner, "agent": agent.fragment });
    let mut body = json!({ "text": cut_bytes(text, limits::MESSAGE_TEXT_MAX_BYTES), "to": [agent.identity], "relayed": relayed });
    if !attachments.is_empty() {
        body["attachments"] = json!(attachments);
    }
    body
}

/// A relayed message's post id: `sx:` and 32 hex of SHA-256 of `<agent
/// fragment>|<its sender's id for it>`: the same however often the agent's
/// SimpleX core reads it, a catch-up or a rollback's included.
pub fn relayed_id(agent: &str, sender_id: &str) -> String {
    let digest = Sha256::digest(format!("{agent}|{sender_id}").as_bytes());
    let id = format!("sx:{}", &hex(&digest)[..32]);
    assert!(valid_post_id(&id), "a relayed id is a post id: {id}");
    id
}

/// Whether a `tasks` record is a routine that names no chat (`{kind:
/// "routine", text}`): its agent's home's, whichever chat that is as it
/// fires (docs/api.md, "An agent's home"), which the bridge reads then and
/// names (`homed`). A bridge before homes passes one over (`task`: Other).
pub fn homeless(body: &Value) -> bool {
    let Value::Object(o) = body else { return false };
    o.get("kind").and_then(Value::as_str) == Some("routine") && text_field(o, "text").is_some() && matches!(o.get("chat"), None | Some(Value::Null))
}

/// A routine that names no chat, naming the one its agent's home is now.
pub fn homed(body: &Value, chat: &str) -> Value {
    assert!(homeless(body), "only a routine that names no chat is given one");
    let mut named = body.clone();
    named["chat"] = json!(chat);
    named
}

/// Reads a `tasks` record's body.
pub fn task(body: &Value) -> Task {
    let Value::Object(o) = body else { return Task::Other };
    match o.get("kind").and_then(Value::as_str) {
        Some("routine") => match (text_field(o, "text"), text_field(o, "chat")) {
            (Some(text), Some(chat)) if !chat.is_empty() && chat.len() <= 128 => Task::Routine { text, chat },
            _ => Task::Other,
        },
        Some("joined") => Task::Joined { fragment: text_field(o, "fragment").filter(|f| !f.is_empty() && f.len() <= 128) },
        Some("answer") => {
            let hop = o.get("hop").and_then(Value::as_u64).and_then(|h| u32::try_from(h).ok());
            let refused = match o.get("refused") {
                None | Some(Value::Null) => Some(false),
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => None,
            };
            let by = text_field(o, "by").filter(|b| is_identity(b) && b.len() <= 128);
            // its files, all of them read or none: an answer is kept whatever
            // they say, as its question's only delivery
            let asked = o.get("asked").and_then(|a| a.get("fragment")).and_then(Value::as_str).filter(|f| fragment_name_ok(f)).map(str::to_string);
            let files = match (o.get("files"), &asked) {
                (Some(Value::Array(items)), Some(_)) if items.len() <= limits::ATTACHMENTS_MAX => {
                    let parsed: Vec<AttachmentRef> = items.iter().filter_map(attachment).collect();
                    if parsed.len() == items.len() { parsed } else { Vec::new() }
                }
                _ => Vec::new(),
            };
            match (text_field(o, "chat"), text_field(o, "text"), by, text_field(o, "name"), hop, refused) {
                (Some(chat), Some(text), Some(by), Some(name), Some(hop), Some(refused)) if fragment_name_ok(&chat) && !name.trim().is_empty() && name.len() <= limits::ANSWER_NAME_MAX_BYTES => Task::Answer { chat, text, by, name, hop, refused, files, asked },
                _ => Task::Other,
            }
        }
        _ => Task::Other,
    }
}

/// The `@name` words of a message, lowercased, in order (a word is
/// `[A-Za-z0-9_-]+` right after an `@` that does not follow a word
/// character, so an email address is no mention).
pub fn mentions(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // bounded by the text's length: each pass moves `i` forward
    while i < bytes.len() {
        let after_word = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if bytes[i] == b'@' && !after_word {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-') {
                end += 1;
            }
            if end > start {
                out.push(text[start..end].to_ascii_lowercase());
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

// ---- ids ----

/// A turn's id: what one agent does about one record, so a record caught up
/// on twice (after a restart, a wake, a deploy) is one turn (lesson 2), and
/// two agents answering one message are two turns.
pub fn turn_id(agent: &str, fragment: &str, channel: &str, seq: u64) -> String {
    let digest = Sha256::digest(format!("{agent}|{fragment}/{channel}/{seq}").as_bytes());
    hex(&digest)[..24].to_string()
}

/// The turn that continues the cut turn `cut` (engine.rs,
/// `continue_turn`): one id a cut turn, the same in every life, so its
/// claim makes it one run.
pub fn continuation_id(cut: &str) -> String {
    let digest = Sha256::digest(format!("continue\0{cut}").as_bytes());
    hex(&digest)[..24].to_string()
}

/// A turn of an agent's own (a message its runtime sent with no turn
/// running): numbered by the bridge's own counter within its life. The
/// counter is kept in `/data`, which a rollback sends back, so the life is
/// in the id too: a number said again in a later life names a new turn,
/// never one an earlier life posted with other text (a 409).
pub fn said_turn_id(agent: &str, life: &str, n: u64) -> String {
    assert!(valid_life(life), "a life is 32 lowercase hex: {life}");
    let digest = Sha256::digest(format!("{agent}|said/{life}/{n}").as_bytes());
    hex(&digest)[..24].to_string()
}

/// Whether `s` is a life: one bridge process's 128 random bits, as 32
/// lowercase hex (docs/chat-records.md, `turn.start`).
pub fn valid_life(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 0xf)] as char);
    }
    out
}

/// A progress record's post id: `wk:<turn>:<part>`.
pub fn work_id(turn: &str, part: &str) -> String {
    let id = format!("wk:{turn}:{part}");
    assert!(valid_post_id(&id), "a work id is a post id: {id}");
    id
}

/// A reply's post id: `rp:<turn>:<n>`, its replies numbered from 1.
pub fn reply_id(turn: &str, n: u32) -> String {
    assert!(n >= 1, "replies number from 1");
    let id = format!("rp:{turn}:{n}");
    assert!(valid_post_id(&id), "a reply id is a post id: {id}");
    id
}

// ---- what the bridge posts ----

/// `text` cut to at most `max` characters, a cut marked with `…`.
pub fn cut(text: &str, max: usize) -> String {
    assert!(max > 0, "a cut keeps something");
    let text = text.trim();
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((at, _)) => {
            let keep = text[..at].char_indices().nth(max - 1).map_or(at, |(i, _)| i);
            format!("{}…", &text[..keep])
        }
    }
}

/// `text` cut to at most `max` bytes on a character boundary, marked `…`.
pub fn cut_bytes(text: &str, max: usize) -> String {
    assert!(max > 4, "a cut keeps something");
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// What started a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cause {
    pub fragment: String,
    pub channel: String,
    pub seq: u64,
}

/// A turn's claim: "this life runs this turn" (docs/chat-records.md). The
/// platform answers the same id and body again as a replay and another
/// body as 409, so with the life in the body this life's own retry is a
/// replay and any other life's claim of the turn is a 409.
pub fn turn_start(turn: &str, asker: &str, agent: &str, cause: &Cause, life: &str) -> Value {
    assert!(valid_life(life), "a life is 32 lowercase hex: {life}");
    json!({ "kind": "turn.start", "turn": turn, "asker": asker, "agent": agent,
        "cause": { "fragment": cause.fragment, "channel": cause.channel, "seq": cause.seq }, "life": life })
}

/// What a step does, whatever its runtime calls the tool: a page shows each
/// with its own icon and verb (docs/chat-records.md, `turn.step`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A command, or code, run.
    Shell,
    /// A search of the web.
    Web,
    /// A file, a page, or a skill read, or files searched.
    Read,
    /// A file written or edited.
    Write,
    /// The agent's browser, or its desktop, driven.
    Browser,
    /// An image made.
    Image,
    /// Work handed to another agent.
    Delegate,
    /// What the agent remembers, changed.
    Memory,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Shell => "shell",
            Category::Web => "web",
            Category::Read => "read",
            Category::Write => "write",
            Category::Browser => "browser",
            Category::Image => "image",
            Category::Delegate => "delegate",
            Category::Memory => "memory",
            Category::Other => "other",
        }
    }
}

/// One step of a turn (a tool call), numbered from 1. The model's words
/// before the call are no part of it: they are a reply of their own,
/// posted before it (docs/chat-records.md; an older bridge's step kept
/// them as its `text`, which pages still read).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tool: String,
    pub args: String,
    pub ok: bool,
    pub excerpt: String,
    pub category: Category,
    pub evidence: Option<ToolEvidence>,
}

/// How a call ended, as its runtime observed it; old records and runtimes
/// omit it (docs/chat-records.md).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEvidence {
    pub tool_call_id: String,
    pub status: ToolStatus,
    pub duration_ms: u64,
    /// The model response whose calls were running (`Call::response`), when
    /// its runtime says: it pairs the step with that response's calls alone.
    pub response: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    Blocked,
}

pub fn turn_step(turn: &str, n: u32, s: &Step) -> Value {
    assert!(n >= 1, "steps number from 1");
    let mut body = json!({
        "kind": "turn.step",
        "turn": turn,
        "step": n,
        "tool": cut(&s.tool, limits::STEP_TOOL_MAX_CHARS),
        "category": s.category.as_str(),
        "args": cut_or_empty(&s.args, limits::STEP_ARGS_MAX_CHARS),
        "ok": s.ok,
        "excerpt": cut_or_empty(&s.excerpt, limits::STEP_EXCERPT_MAX_CHARS),
    });
    if let Some(e) = &s.evidence {
        body["tool_call_id"] = json!(e.tool_call_id);
        body["status"] = json!(e.status);
        body["duration_ms"] = json!(e.duration_ms);
        if let Some(r) = &e.response {
            assert!(valid_post_id(r), "a response's id is a post id: {r}");
            body["response"] = json!(r);
        }
    }
    body
}

/// A tool call the model asked for, before it runs, as its runtime says
/// so (our Hermes image: `post_api_request`, before Hermes runs any call
/// of the response; Claude Code: its `tool_use`). Asked is no more than
/// that: its runtime may refuse it, and only its step (`turn.step`, of the
/// same response) says it ended. One a cut turn asked for and never ended
/// may or may not have run (note.rs).
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The model response that asked for it: an id its runtime gives each
    /// response once (a post id), the same when the call is said again.
    pub response: String,
    /// Its place among that response's calls, from 0.
    pub position: u32,
    /// The provider's id of the call (a post id). It is no call's identity:
    /// some providers name every response's first call `call_0`, and one
    /// may reuse an id across a batch.
    pub tool_call_id: String,
    pub tool: String,
    pub args: String,
    pub category: Category,
}

/// A call's record id: one per turn, response and place in it, so a call
/// said again (a frame's replay) is one record, and a provider's id reused
/// by a later response is another call. Hashed into the record's (a
/// response's id runs long); the body keeps the response.
pub fn call_id(turn: &str, response: &str, position: u32) -> String {
    let h = hex(&Sha256::digest(format!("{response}\0{position}").as_bytes()));
    work_id(turn, &format!("call:{}", &h[..24]))
}

pub fn turn_call(turn: &str, c: &Call) -> Value {
    assert!(valid_post_id(&c.response) && valid_post_id(&c.tool_call_id), "a call's ids are post ids: {} {}", c.response, c.tool_call_id);
    json!({
        "kind": "turn.call",
        "turn": turn,
        "response": c.response,
        "tool_call_id": c.tool_call_id,
        "tool": cut(&c.tool, limits::STEP_TOOL_MAX_CHARS),
        "category": c.category.as_str(),
        "args": cut_or_empty(&c.args, limits::STEP_ARGS_MAX_CHARS),
    })
}

/// A turn asking `asks` something to answer in words: their next message
/// to the agent in the chat is the answer.
pub fn turn_asked(turn: &str, asks: &str) -> Value {
    json!({ "kind": "turn.asked", "turn": turn, "asks": asks })
}

/// What a runtime's notice is about (docs/chat-records.md, `turn.notice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeCategory {
    /// How the turn goes: a wait, a confirmation, its context compressed.
    Info,
    /// Something went wrong, or may: it seems stuck, a sub-agent failed.
    Warning,
    /// What the agent remembers changed (its review after a turn).
    Memory,
}

impl NoticeCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeCategory::Info => "info",
            NoticeCategory::Warning => "warning",
            NoticeCategory::Memory => "memory",
        }
    }
}

/// A runtime's notice in a turn: neither a step nor a reply.
pub fn turn_notice(turn: &str, category: NoticeCategory, text: &str) -> Value {
    json!({ "kind": "turn.notice", "turn": turn, "category": category.as_str(), "text": cut(text, limits::NOTICE_TEXT_MAX_CHARS) })
}

fn cut_or_empty(text: &str, max: usize) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        cut(text, max)
    }
}

/// One choice of a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Answered in words (a clarify's "Other"): its answer carries them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub words: bool,
    /// Answered on the agent's computer, never in a record (`words` too):
    /// what its person gives that computer alone, a sign-in's code (the
    /// chat's page hands it to the shell, which hands it to the computer's
    /// `/answer` page: docs/bridge.md, "An answer for the computer").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub computer: bool,
}

pub fn turn_prompt(turn: &str, prompt: &str, text: &str, options: &[PromptOption], asks: &str, expires_at: u64) -> Value {
    assert!(!options.is_empty() && options.len() <= limits::PROMPT_OPTIONS_MAX, "a prompt offers 1 to {} options", limits::PROMPT_OPTIONS_MAX);
    json!({ "kind": "turn.prompt", "turn": turn, "prompt": prompt, "text": cut(text, limits::PROMPT_TEXT_MAX_CHARS),
        "options": options, "asks": asks, "expiresAt": expires_at })
}

/// How a prompt closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Closed {
    /// A late answer whose card's journal could not be read whole.
    #[serde(rename = "history_unavailable")]
    Unavailable,
    Answered,
    Expired,
    Stopped,
}

impl Closed {
    pub fn as_str(self) -> &'static str {
        match self {
            Closed::Unavailable => "history_unavailable",
            Closed::Answered => "answered",
            Closed::Expired => "expired",
            Closed::Stopped => "stopped",
        }
    }
}

/// `answer`: the option, who answered, and the words of one answered in words.
pub fn turn_prompt_closed(turn: &str, prompt: &str, closed: Closed, answer: Option<(&str, &str, Option<&str>)>) -> Value {
    let mut body = json!({ "kind": "turn.prompt.closed", "turn": turn, "prompt": prompt, "outcome": closed.as_str() });
    match (closed, answer) {
        (Closed::Answered, Some((option, by, words))) => {
            body["option"] = json!(option);
            body["by"] = json!(by);
            if let Some(words) = words {
                body["text"] = json!(words);
            }
        }
        (Closed::Answered, None) => panic!("an answered prompt names its answer"),
        (_, _) => {}
    }
    body
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It answered (or finished without words).
    Idle,
    /// Its asker stopped it.
    Stopped,
    /// It failed, with what was wrong.
    Error(String),
}

pub fn turn_end(turn: &str, outcome: &Outcome) -> Value {
    match outcome {
        Outcome::Idle => json!({ "kind": "turn.end", "turn": turn, "outcome": "idle" }),
        Outcome::Stopped => json!({ "kind": "turn.end", "turn": turn, "outcome": "stopped" }),
        Outcome::Error(e) => json!({ "kind": "turn.end", "turn": turn, "outcome": "error", "error": cut(e, limits::ERROR_MAX_CHARS) }),
    }
}

/// An answer's post id on its asking agent's `tasks`: `an:<the asked
/// agent's turn>`, one per question and asked agent, whatever life posts it.
pub fn answer_id(turn: &str) -> String {
    let id = format!("an:{turn}");
    assert!(valid_post_id(&id), "an answer id is a post id: {id}");
    id
}

/// The answer to an ask (docs/chat-records.md, "An ask"), posted by the
/// asking agent on its own `tasks` once the asked agent's turn is over:
/// where its turn runs (`chat`), what it is told (`text`), who answered
/// (`by`, called `name`), the question's record (`asked`), that turn and
/// how it ended, and how many hand-offs deep the asking agent's turn is
/// (`hop`); `refused` when the question was never taken (its turn there
/// never runs).
#[allow(clippy::too_many_arguments)]
pub fn answer(chat: &str, text: &str, by: &str, name: &str, asked: &Cause, turn: &str, outcome: &Outcome, hop: u32, refused: bool) -> Value {
    let end = turn_end(turn, outcome);
    let mut body = json!({ "kind": "answer", "chat": chat, "text": text, "by": by, "name": cut_bytes(name, limits::ANSWER_NAME_MAX_BYTES),
        "asked": { "fragment": asked.fragment, "seq": asked.seq }, "turn": turn, "outcome": end["outcome"], "hop": hop });
    if let Some(e) = end.get("error") {
        body["error"] = e.clone();
    }
    if refused {
        body["refused"] = json!(true);
    }
    body
}

/// What an answer tells its asking agent: who answered which question
/// where, how it ended, and what the asked agent said there (`said`, its
/// replies but interim ones; `files`, how many it sent).
pub fn answer_text(by: &str, chat: &str, asked: &str, said: &str, files: u32, outcome: &Outcome) -> String {
    // one line each, no control characters (the body's escaped bound)
    let flat = |s: &str| s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let (by, asked) = (flat(by), cut_bytes(&flat(asked.trim()), limits::ANSWER_ASKED_MAX_BYTES));
    let head = match outcome {
        Outcome::Idle if said.trim().is_empty() => format!("{by} finished your question in {chat} (\"{asked}\") and said nothing."),
        Outcome::Idle => format!("{by} answered your question in {chat} (\"{asked}\"):"),
        Outcome::Stopped => format!("{by}'s turn on your question in {chat} (\"{asked}\") was stopped."),
        Outcome::Error(e) => format!("{by} could not answer your question in {chat} (\"{asked}\"): {e}."),
    };
    let mut text = head;
    if !said.trim().is_empty() {
        if !matches!(outcome, Outcome::Idle) {
            text.push_str(" It had said:");
        }
        text.push_str("\n\n");
        text.push_str(&cut_bytes(said.trim(), limits::ANSWER_TEXT_MAX_BYTES));
    }
    if files > 0 {
        text.push_str(&format!("\n\n({by} sent {files} file{} there.)", if files == 1 { "" } else { "s" }));
    }
    text
}

/// What a refused question's answer tells its asking agent, the reason
/// first (its turn's end, cut to `ERROR_MAX_CHARS`, says it whole): the
/// agent was never asked, why, and which question.
pub fn refused_text(by: &str, chat: &str, asked: &str, why: &str) -> String {
    let flat = |s: &str| s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    format!("{} wasn't asked: {why}. (Your question in {chat}: \"{}\")", flat(by), cut_bytes(&flat(asked.trim()), limits::ANSWER_ASKED_MAX_BYTES))
}

/// The commands an agent's runtime takes from its owner (its menu,
/// `runtime::MenuItem`), as the chat's page offers them: each one's name,
/// what it does, and what its argument is. Posted by the agent on a chat's
/// `work` as it first runs a turn there in a life (`menu_id`), so the
/// page's window of `work` holds it while the chat is used.
pub fn commands(agent: &str, menu: &[crate::runtime::MenuItem]) -> Value {
    assert!(!menu.is_empty() && menu.iter().all(|m| valid_command(m.name)), "a menu names its commands");
    let items: Vec<Value> = menu
        .iter()
        .map(|m| {
            let mut c = json!({ "name": m.name, "description": m.description });
            if let Some(a) = m.args {
                c["args"] = json!(a);
            }
            c
        })
        .collect();
    json!({ "kind": "commands", "agent": agent, "commands": items })
}

/// The menu's post id in a chat: once a life (`cm:<life>`), so a menu a
/// life posted again is a replay, and a new life's is a record of its own.
pub fn menu_id(life: &str) -> String {
    assert!(valid_life(life), "a life is 32 lowercase hex: {life}");
    format!("cm:{life}")
}

/// An agent's reply on `chat`: its text and turn; `to` and `hop` when it
/// hands off to another agent; `interim` when its turn went on working
/// after it (its words before a tool call: docs/chat-records.md), which
/// the chat's push leaves out. Attachments are added by whoever uploads
/// them.
pub fn reply(text: &str, turn: &str, to: &[String], hop: u32, interim: bool) -> Value {
    let mut body = json!({ "text": cut_bytes(text, limits::REPLY_TEXT_MAX_BYTES), "turn": turn });
    if !to.is_empty() {
        body["to"] = json!(to);
        body["hop"] = json!(hop);
    }
    if interim {
        body["interim"] = json!(true);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a relayed message is a message that names a door (any door's
    /// id) and a person, nothing else; its body round-trips through `said` as a
    /// message to its agent; its id is the same for the same message.
    #[test]
    fn a_relayed_message_reads_strictly() {
        let juniper = crate::runtime::Agent { fragment: "juniper--k3x9".into(), identity: "npub1juniper".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None };
        let body = relayed_message("hi from my phone", &juniper, VIA_SIMPLEX, &[]);
        assert_eq!(body, json!({ "text": "hi from my phone", "to": ["npub1juniper"], "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9" } }));
        assert_eq!(relayed(&body), Some(Relayed { for_: "npub1paul".into(), via: VIA_SIMPLEX.into(), agent: "juniper--k3x9".into() }));
        let mut telegram = body.clone();
        telegram["relayed"]["via"] = json!("telegram");
        assert_eq!(relayed(&telegram).map(|r| r.via), Some("telegram".into()), "any door's id: a hosted connector's too");
        let Said::Message(m) = said(&body) else { panic!("a message") };
        assert_eq!((m.text.as_str(), m.to.as_slice()), ("hi from my phone", ["npub1juniper".to_string()].as_slice()));
        for bad in [
            json!({ "text": "x", "relayed": { "via": "../simplex", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x", "relayed": { "via": "SimpleX", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x", "relayed": { "via": "a--b", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x", "relayed": { "via": "", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x", "relayed": { "via": "simplex", "for": "paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x", "relayed": { "via": "simplex", "for": "npub1paul" } }),
            json!({ "text": "x", "relayed": { "via": "simplex", "for": "npub1paul", "agent": "../x" } }),
            json!({ "text": "x", "relayed": { "via": "simplex" } }),
            json!({ "text": "x", "relayed": "simplex" }),
            json!({ "kind": "stop", "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": 3, "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9" } }),
            json!({ "text": "x" }),
            json!("x"),
        ] {
            assert_eq!(relayed(&bad), None, "{bad}");
        }
        let voice = AttachmentRef { sha256: "a".repeat(64), size: 10, media_type: "audio/mp4".into(), name: "voice.m4a".into() };
        let with = relayed_message("", &juniper, VIA_SIMPLEX, std::slice::from_ref(&voice));
        assert!(relayed(&with).is_some(), "a voice message, its words none");
        let Said::Message(m) = said(&with) else { panic!("a message") };
        assert_eq!(m.attachments, vec![voice]);
        assert_eq!(relayed_id("juniper--k3x9", "RlBFOStpcVFyc1VrNDJyZQ=="), relayed_id("juniper--k3x9", "RlBFOStpcVFyc1VrNDJyZQ=="));
        assert_ne!(relayed_id("juniper--k3x9", "a"), relayed_id("maple--k3x9", "a"), "each agent's own");
        assert_eq!(relayed_id("juniper--k3x9", "a").len(), 35);
    }

    #[test]
    fn messages_read_strictly() {
        assert_eq!(said(&json!({ "text": "hi" })), Said::Message(Message { text: "hi".into(), ..Message::default() }));
        assert_eq!(said(&json!({ "kind": "message", "text": "hi" })), Said::Message(Message { text: "hi".into(), ..Message::default() }));
        assert_eq!(said(&json!("plain")), Said::Message(Message { text: "plain".into(), ..Message::default() }));
        let sha = "a".repeat(64);
        let m = said(&json!({ "text": "look", "to": ["npub1aa"], "attachments": [{ "sha256": sha, "size": 3, "type": "image/png", "name": "a.png" }] }));
        let Said::Message(m) = m else { panic!("a message") };
        assert_eq!(m.to, vec!["npub1aa".to_string()]);
        assert_eq!(m.attachments.len(), 1);
        // a reply names its turn and may hand off
        let Said::Message(r) = said(&json!({ "text": "ok", "turn": "abc", "to": ["npub1b"], "hop": 2 })) else { panic!() };
        assert_eq!((r.turn.as_deref(), r.hop), (Some("abc"), 2));
        // invalid shapes are never guessed at
        for bad in [
            json!(12),
            json!({ "text": 3 }),
            json!({}),
            json!({ "text": "x", "to": "npub1a" }),
            json!({ "text": "x", "to": ["someone"] }),
            json!({ "text": "x", "attachments": [{ "sha256": "zz", "size": 1, "type": "a", "name": "b" }] }),
            json!({ "text": "x", "attachments": [{ "sha256": "A".repeat(64), "size": 1, "type": "a", "name": "b" }] }),
            json!({ "text": "x", "attachments": [{ "sha256": "a".repeat(64), "size": 0, "type": "a", "name": "b" }] }),
            json!({ "text": "x", "turn": "has space" }),
            json!({ "text": "x", "hop": -1 }),
            json!({ "kind": "draft", "text": "x" }),
            json!({ "kind": 3 }),
        ] {
            assert_eq!(said(&bad), Said::Other, "{bad}");
        }
    }

    /// Goal: a command reads as the doc says, `{kind: "command", command,
    /// args?, to?}`, its args trimmed and cut as a message's text; a
    /// person's message may quote one (`reply_to`, a seq). Invalid shapes
    /// are never guessed at.
    #[test]
    fn commands_and_quotes_read_strictly() {
        assert_eq!(said(&json!({ "kind": "command", "command": "usage" })), Said::Command(CommandSaid { name: "usage".into(), ..CommandSaid::default() }));
        assert_eq!(
            said(&json!({ "kind": "command", "command": "steer", "args": "  use blue \n", "to": ["npub1a"] })),
            Said::Command(CommandSaid { name: "steer".into(), args: "use blue".into(), to: vec!["npub1a".into()] })
        );
        let long = said(&json!({ "kind": "command", "command": "btw", "args": "a".repeat(limits::MESSAGE_TEXT_MAX_BYTES + 9) }));
        assert!(matches!(long, Said::Command(c) if c.args.len() == limits::MESSAGE_TEXT_MAX_BYTES), "cut as a message's text is");
        for bad in [
            json!({ "kind": "command" }),
            json!({ "kind": "command", "command": "" }),
            json!({ "kind": "command", "command": "/usage" }),
            json!({ "kind": "command", "command": "Usage" }),
            json!({ "kind": "command", "command": "a".repeat(33) }),
            json!({ "kind": "command", "command": "usage", "args": 3 }),
            json!({ "kind": "command", "command": "usage", "to": ["someone"] }),
            json!({ "kind": "command", "command": 7 }),
        ] {
            assert_eq!(said(&bad), Said::Other, "{bad}");
        }
        let Said::Message(m) = said(&json!({ "text": "that one", "reply_to": 12 })) else { panic!("a message") };
        assert_eq!(m.reply_to, Some(12));
        let Said::Message(m) = said(&json!({ "text": "none", "reply_to": null })) else { panic!("a message") };
        assert_eq!(m.reply_to, None);
        for bad in [json!({ "text": "x", "reply_to": 0 }), json!({ "text": "x", "reply_to": -1 }), json!({ "text": "x", "reply_to": "12" }), json!({ "text": "x", "reply_to": 1.5 })] {
            assert_eq!(said(&bad), Said::Other, "{bad}");
        }
        assert!(valid_command("reload-mcp") && valid_command("a_b9") && !valid_command("a b") && !valid_command("é"));
        use crate::runtime::{How, MenuItem};
        let menu = [MenuItem { name: "steer", description: "Steer it", args: Some("What to tell it"), how: How::Steer, refuse: &[] }, MenuItem { name: "usage", description: "Usage", args: None, how: How::Turn, refuse: &[] }];
        assert_eq!(
            commands("npub1a", &menu),
            json!({ "kind": "commands", "agent": "npub1a", "commands": [{ "name": "steer", "description": "Steer it", "args": "What to tell it" }, { "name": "usage", "description": "Usage" }] })
        );
        assert!(valid_post_id(&menu_id(&"0".repeat(32))));
    }

    #[test]
    fn stops_and_answers() {
        assert_eq!(said(&json!({ "kind": "stop" })), Said::Stop { turn: None });
        assert_eq!(said(&json!({ "kind": "stop", "turn": "t1" })), Said::Stop { turn: Some("t1".into()) });
        assert_eq!(said(&json!({ "kind": "stop", "turn": 1 })), Said::Other);
        assert_eq!(said(&json!({ "kind": "prompt_response", "prompt": "ab.12", "option": "once" })), Said::PromptResponse { prompt: "ab.12".into(), option: "once".into(), text: None, card: None });
        assert_eq!(
            said(&json!({ "kind": "prompt_response", "prompt": "ab.12", "option": "once", "agent": "npub1a", "turn": "t1" })),
            Said::PromptResponse { prompt: "ab.12".into(), option: "once".into(), text: None, card: Some(Card { agent: "npub1a".into(), turn: "t1".into() }) },
            "the card it answers: its agent and turn"
        );
        for half in [json!({ "agent": "npub1a" }), json!({ "turn": "t1" }), json!({ "agent": "paul", "turn": "t1" }), json!({ "agent": "npub1a", "turn": "a b" }), json!({ "agent": 7, "turn": "t1" })] {
            let mut body = json!({ "kind": "prompt_response", "prompt": "ab.12", "option": "once" });
            body.as_object_mut().unwrap().extend(half.as_object().unwrap().clone());
            assert_eq!(said(&body), Said::Other, "a card named in part, or not as one, is no answer: {half}");
        }
        // an option answered in words carries them, trimmed; blank words are none
        assert_eq!(said(&json!({ "kind": "prompt_response", "prompt": "q.1", "option": "other", "text": " purple \n" })), Said::PromptResponse { prompt: "q.1".into(), option: "other".into(), text: Some("purple".into()), card: None });
        assert_eq!(said(&json!({ "kind": "prompt_response", "prompt": "q.1", "option": "other", "text": "  " })), Said::PromptResponse { prompt: "q.1".into(), option: "other".into(), text: None, card: None });
        let long = said(&json!({ "kind": "prompt_response", "prompt": "q.1", "option": "other", "text": "a".repeat(limits::MESSAGE_TEXT_MAX_BYTES + 9) }));
        assert!(matches!(long, Said::PromptResponse { text: Some(t), .. } if t.len() == limits::MESSAGE_TEXT_MAX_BYTES), "cut as a message's text is");
        for bad in [json!({ "kind": "prompt_response", "prompt": "ab" }), json!({ "kind": "prompt_response", "prompt": "a b", "option": "x" }), json!({ "kind": "prompt_response", "prompt": "a", "option": "" }), json!({ "kind": "prompt_response", "prompt": "a", "option": "x", "text": 3 })] {
            assert_eq!(said(&bad), Said::Other, "{bad}");
        }
    }

    #[test]
    fn tasks_read() {
        assert_eq!(task(&json!({ "kind": "routine", "text": "water", "chat": "c--k3x9" })), Task::Routine { text: "water".into(), chat: "c--k3x9".into() });
        assert_eq!(task(&json!({ "kind": "joined", "fragment": "c--k3x9" })), Task::Joined { fragment: Some("c--k3x9".into()) });
        let answer = json!({ "kind": "answer", "chat": "home--k3x9", "text": "t", "by": "npub1rowan", "name": "Rowan", "hop": 2 });
        assert_eq!(task(&answer), Task::Answer { chat: "home--k3x9".into(), text: "t".into(), by: "npub1rowan".into(), name: "Rowan".into(), hop: 2, refused: false, files: vec![], asked: None });
        let mut refused = answer.clone();
        refused["refused"] = json!(true);
        assert!(matches!(task(&refused), Task::Answer { refused: true, .. }));

        // #721: the files the asked agent sent, blobs of the chat it was
        // asked in; any one unread, or more than a message carries, and
        // none are (the answer is kept: its question's only delivery)
        let file = |name: &str| json!({ "sha256": "a".repeat(64), "size": 3, "type": "text/plain", "name": name });
        let mut with = answer.clone();
        with["asked"] = json!({ "fragment": "talk--k3x9", "seq": 4 });
        with["files"] = json!([file("drawing.txt")]);
        let read = AttachmentRef { sha256: "a".repeat(64), size: 3, media_type: "text/plain".into(), name: "drawing.txt".into() };
        assert!(matches!(task(&with), Task::Answer { ref files, ref asked, .. } if *files == vec![read.clone()] && asked.as_deref() == Some("talk--k3x9")), "{:?}", task(&with));
        let mut bad = with.clone();
        bad["files"] = json!([file("drawing.txt"), { "sha256": "zz", "size": 1, "type": "a", "name": "b" }]);
        let mut many = with.clone();
        many["files"] = json!((0..=limits::ATTACHMENTS_MAX).map(|i| file(&format!("f{i}"))).collect::<Vec<_>>());
        let mut nowhere = with.clone();
        nowhere["asked"] = json!({ "fragment": "Not A Chat" });
        let mut unasked = with.clone();
        unasked.as_object_mut().unwrap().remove("asked");
        for (why, b) in [("an unread file", bad), ("past the bound", many), ("no chat to read them from", nowhere), ("no asked chat", unasked)] {
            assert!(matches!(task(&b), Task::Answer { ref files, .. } if files.is_empty()), "{why}: {:?}", task(&b));
        }
        // a name at its bound, written as the writer cuts one (51 emoji, 67
        // CJK characters: a title of 120 characters is more), is read
        for long in ["🌱".repeat(120), "森".repeat(120)] {
            let written = super::answer("home--k3x9", "t", "npub1rowan", &long, &Cause { fragment: "talk--k3x9".into(), channel: "chat".into(), seq: 1 }, "t1", &Outcome::Idle, 2, false);
            assert!(written["name"].as_str().unwrap().len() <= limits::ANSWER_NAME_MAX_BYTES);
            assert!(matches!(task(&written), Task::Answer { .. }), "{long}");
        }
        for (key, bad) in [("chat", json!("Home Chat")), ("by", json!("rowan")), ("by", json!(null)), ("name", json!(" ")), ("name", json!("a".repeat(201))), ("hop", json!(-1)), ("text", json!(null)), ("refused", json!("yes"))] {
            let mut broken = answer.clone();
            broken[key] = bad;
            assert_eq!(task(&broken), Task::Other, "{key}");
        }
        assert_eq!(task(&json!({ "kind": "joined" })), Task::Joined { fragment: None }, "a join naming no fragment still lists them again");
        assert_eq!(task(&json!({ "kind": "joined", "fragment": "" })), Task::Joined { fragment: None });
        assert_eq!(task(&json!({ "kind": "routine", "text": "water" })), Task::Other);
        assert_eq!(task(&json!("routine")), Task::Other);
    }

    /// Goal (an agent's home): a routine that names no chat is homeless,
    /// and named its home's chat it is a routine there; anything else is
    /// not homeless (a chat named, even empty; no text; another kind).
    #[test]
    fn a_routine_with_no_chat_runs_at_home() {
        for homeless_one in [json!({ "kind": "routine", "text": "water" }), json!({ "kind": "routine", "text": "water", "chat": null })] {
            assert!(homeless(&homeless_one), "{homeless_one}");
            assert_eq!(task(&homed(&homeless_one, "juniper-simplex--w9x2")), Task::Routine { text: "water".into(), chat: "juniper-simplex--w9x2".into() });
        }
        for not in [
            json!({ "kind": "routine", "text": "water", "chat": "c--k3x9" }),
            json!({ "kind": "routine", "text": "water", "chat": "" }),
            json!({ "kind": "routine" }),
            json!({ "kind": "routine", "text": 3 }),
            json!({ "kind": "joined", "text": "water" }),
            json!("routine"),
        ] {
            assert!(!homeless(&not), "{not}");
        }
    }

    #[test]
    fn mentions_are_words_after_an_at() {
        assert_eq!(mentions("@Juniper can you, @bob-2?"), vec!["juniper", "bob-2"]);
        assert_eq!(mentions("mail me at a@b.com"), Vec::<String>::new());
        assert_eq!(mentions("@ alone, @@x"), vec!["x"]);
        assert_eq!(mentions(""), Vec::<String>::new());
        assert_eq!(mentions("é@ana"), vec!["ana"], "a non-ASCII letter is no word character here");
    }

    #[test]
    fn ids_are_stable_and_valid() {
        let t = turn_id("juniper--k3x9", "talk--k3x9", "chat", 12);
        assert_eq!(t.len(), 24);
        assert_eq!(t, turn_id("juniper--k3x9", "talk--k3x9", "chat", 12), "the same record, the same turn");
        assert_ne!(t, turn_id("rowan--k3x9", "talk--k3x9", "chat", 12), "another agent, another turn");
        assert_ne!(t, turn_id("juniper--k3x9", "talk--k3x9", "chat", 13));
        for id in [work_id(&t, "start"), work_id(&t, "7"), work_id(&t, "p:ab12cd.0011aabb"), work_id(&t, "end"), reply_id(&t, 1)] {
            assert!(valid_post_id(&id), "{id}");
        }
        let (one, two) = ("0".repeat(32), "1".repeat(32));
        assert_ne!(said_turn_id("a", &one, 1), said_turn_id("a", &one, 2));
        assert_ne!(said_turn_id("a", &one, 1), said_turn_id("a", &two, 1), "a counter said again in another life is another turn");
        assert_eq!(said_turn_id("a", &one, 1), said_turn_id("a", &one, 1));
        for bad in ["", "0", &"A".repeat(32), &"g".repeat(32), &"0".repeat(33)] {
            assert!(!valid_life(bad), "{bad}");
        }
    }

    #[test]
    fn bodies_are_the_docs() {
        let c = Cause { fragment: "talk--k3x9".into(), channel: "chat".into(), seq: 4 };
        let life = "0123456789abcdef0123456789abcdef";
        assert_eq!(turn_start("t", "npub1p", "npub1a", &c, life), json!({ "kind": "turn.start", "turn": "t", "asker": "npub1p", "agent": "npub1a", "cause": { "fragment": "talk--k3x9", "channel": "chat", "seq": 4 }, "life": life }));
        let s = Step { tool: "terminal".into(), args: "ls".into(), ok: true, excerpt: String::new(), category: Category::Shell, evidence: None };
        assert_eq!(turn_step("t", 1, &s), json!({ "kind": "turn.step", "turn": "t", "step": 1, "tool": "terminal", "category": "shell", "args": "ls", "ok": true, "excerpt": "" }), "no words: they are a reply");
        assert_eq!(reply("Let me look.", "t", &[], 1, false), json!({ "text": "Let me look.", "turn": "t" }));
        assert_eq!(reply("Let me look.", "t", &[], 1, true), json!({ "text": "Let me look.", "turn": "t", "interim": true }));
        assert_eq!(reply("@rowan yours", "t", &["npub1r".into()], 2, true), json!({ "text": "@rowan yours", "turn": "t", "to": ["npub1r"], "hop": 2, "interim": true }));
        let long = "w".repeat(limits::REPLY_TEXT_MAX_BYTES + 9);
        assert_eq!(reply(&long, "t", &[], 1, true)["text"].as_str().map(str::len), Some(limits::REPLY_TEXT_MAX_BYTES), "a reply's own bound, never a step's 300");
        assert_eq!(turn_asked("t", "npub1p"), json!({ "kind": "turn.asked", "turn": "t", "asks": "npub1p" }));
        assert_eq!(turn_notice("t", NoticeCategory::Memory, " Memory updated "), json!({ "kind": "turn.notice", "turn": "t", "category": "memory", "text": "Memory updated" }));
        assert_eq!(turn_notice("t", NoticeCategory::Warning, &"x".repeat(400))["text"].as_str().map(|t| t.chars().count()), Some(limits::NOTICE_TEXT_MAX_CHARS));
        assert_eq!([NoticeCategory::Info, NoticeCategory::Warning, NoticeCategory::Memory].map(NoticeCategory::as_str), ["info", "warning", "memory"]);
        let every = [Category::Shell, Category::Web, Category::Read, Category::Write, Category::Browser, Category::Image, Category::Delegate, Category::Memory, Category::Other];
        assert_eq!(every.map(Category::as_str), ["shell", "web", "read", "write", "browser", "image", "delegate", "memory", "other"], "docs/chat-records.md's nine");
        assert_eq!(turn_end("t", &Outcome::Error("x".repeat(400))).get("error").and_then(Value::as_str).map(|e| e.chars().count()), Some(limits::ERROR_MAX_CHARS));
        assert_eq!(turn_prompt_closed("t", "p", Closed::Answered, Some(("once", "npub1p", None))), json!({ "kind": "turn.prompt.closed", "turn": "t", "prompt": "p", "outcome": "answered", "option": "once", "by": "npub1p" }));
        assert_eq!(turn_prompt_closed("t", "p", Closed::Answered, Some(("other", "npub1p", Some("purple"))))["text"], "purple");
        let other = PromptOption { id: "other".into(), label: "Other".into(), style: None, words: true, computer: false };
        let once = PromptOption { id: "once".into(), label: "Allow once".into(), style: None, words: false, computer: false };
        assert_eq!(turn_prompt("t", "p", "Which?", &[once, other], "npub1p", 9)["options"], json!([{ "id": "once", "label": "Allow once" }, { "id": "other", "label": "Other", "words": true }]));
        assert_eq!(turn_prompt_closed("t", "p", Closed::Expired, None)["outcome"], "expired");
        assert_eq!(reply("hi", "t", &[], 0, false), json!({ "text": "hi", "turn": "t" }));
        assert_eq!(reply("hi @b", "t", &["npub1b".to_string()], 1, false), json!({ "text": "hi @b", "turn": "t", "to": ["npub1b"], "hop": 1 }));
    }

    #[test]
    #[should_panic(expected = "an answered prompt names its answer")]
    fn an_answer_names_its_option() {
        turn_prompt_closed("t", "p", Closed::Answered, None);
    }

    #[test]
    fn cuts() {
        assert_eq!(cut("abcdefg", 6), "abcde…");
        assert_eq!(cut_bytes("abc", 10), "abc");
        let long = "é".repeat(100);
        let c = cut_bytes(&long, 21);
        assert!(c.len() <= 21 && c.ends_with('…'), "{c}");
    }
}
