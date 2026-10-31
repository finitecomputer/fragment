//! The runtime side of the bridge: whatever runs the agents. The bridge's
//! core speaks only these commands and events; each runtime translates them
//! into its own protocol. Two ship here:
//!
//! - `relay`: Hermes' Relay (docs/hermes-relay.md). Hermes dials the
//!   bridge, which is its connector.
//! - `script`: a deterministic scripted agent (the stub image), so the
//!   platform's lanes run without any agent runtime at all.
//!
//! Another runtime brings its own module here, or its own bridge: nothing
//! the platform does depends on which one runs (docs/cloudflare-v1.md, the
//! rule).
//!
//! A runtime owns the in-flight turn (lesson 1): the bridge hands it a turn
//! once, and hears back. It never talks to the fragment API: the bridge
//! downloads a turn's attachments before handing it, and uploads a reply's.
//! A turn is handed only in the life that claimed it (docs/chat-records.md,
//! `turn.start`), and a turn is claimed only while its agent's runtime says
//! it can take one (`Event::Connected`).
//!
//! A bridge runs one or more runtimes (`Runtimes`), each by the name agents
//! give it: an agent's `agent.json` `runtime` (`named`), `DEFAULT` when it
//! names none. Its turns go to that runtime, and that runtime's menu is its
//! commands (docs/bridge.md, "Which runtime runs an agent").

pub mod claude;
pub mod relay;
pub mod script;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use tokio::sync::{mpsc, watch};

use crate::records::{PromptOption, Step};

/// The runtime an agent whose `agent.json` names none runs (Paul,
/// 2026-10-30: "`runtime: hermes` default").
pub const DEFAULT: &str = "hermes";

/// A runtime's name, as agents give it: `^[a-z0-9-]{1,32}$`.
pub fn valid_name(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The runtime an agent's `agent.json` names (its `runtime`), from its bytes
/// as the platform answered them (none: there is no file). `None`: it names
/// none, so it runs `DEFAULT`; a file that is not JSON names none, as its
/// `home` does (`api::Home::of`). A value that is no runtime's name (not a
/// string, or not `valid_name`) is kept as it is, cut to 64 bytes: no
/// runtime is run by it, so the agent's turns are refused saying so, never
/// handed to the default.
pub fn named(agent_json: Option<&[u8]>) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(agent_json?).ok()?;
    match &v["runtime"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(crate::records::cut_bytes(s, 64)),
        other => Some(crate::records::cut_bytes(&other.to_string(), 64)),
    }
}

/// The runtimes a bridge runs, each with the name agents give it, in the
/// order they start: at most `limits::RUNTIMES_MAX`, each name once.
pub struct Runtimes(Vec<(String, Box<dyn Runtime>)>);

impl Runtimes {
    pub fn new() -> Runtimes {
        Runtimes(Vec::new())
    }

    /// Adds `runtime`, run for the agents that name `name`.
    pub fn with(mut self, name: &str, runtime: Box<dyn Runtime>) -> Runtimes {
        assert!(valid_name(name), "a runtime's name: {name:?}");
        assert!(self.0.iter().all(|(n, _)| n != name), "a runtime is run once: {name}");
        assert!(self.0.len() < crate::limits::RUNTIMES_MAX, "a bridge runs at most {} runtimes", crate::limits::RUNTIMES_MAX);
        self.0.push((name.to_string(), runtime));
        self
    }

    pub fn into_inner(self) -> Vec<(String, Box<dyn Runtime>)> {
        assert!(!self.0.is_empty(), "a bridge runs a runtime");
        self.0
    }
}

impl Default for Runtimes {
    fn default() -> Runtimes {
        Runtimes::new()
    }
}

/// One runtime, run for the agents that name none (`DEFAULT`).
impl From<Box<dyn Runtime>> for Runtimes {
    fn from(runtime: Box<dyn Runtime>) -> Runtimes {
        Runtimes::new().with(DEFAULT, runtime)
    }
}

/// An agent the computer runs (`GET /api/computer`'s `agents`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Agent {
    /// The agent fragment's name (`<label>--<suffix>`): who requests act as
    /// (`x-fragment-agent`).
    pub fragment: String,
    /// The agent's identity (its fragment's npub): who its records are by.
    pub identity: String,
    /// Its display name, which `@mentions` use.
    pub name: String,
    /// Its owner: the only one who answers its prompts (decision 42).
    pub owner: String,
    /// The credentials it may use now (docs/computers.md, "Connections and
    /// operator keys"): each a placeholder that names the agent, which an
    /// image puts in the environment variables named, for the computer's
    /// swap to fill on the way to the provider's hosts. The bridge sends
    /// none itself.
    #[serde(default)]
    pub credentials: Vec<Credential>,
    /// Its identity's key (64 hex), which its fragment signs as: a door's
    /// ticket is checked with it (simplex/door.rs). None from a platform
    /// before doors, which starts no link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The runtime it runs on, as its `agent.json` names it (`named`): none,
    /// `DEFAULT`. Never the platform's: the driver reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

/// One credential of an agent's (`GET /api/computer`'s `agents[].credentials`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    pub provider: String,
    /// `connection`, `operator` or `own`.
    pub kind: String,
    /// The environment variables it goes in (the vendor's SDK's names).
    pub env: Vec<String>,
    /// `fcx_<provider>_<tag>` or `fck_<provider>_<tag>`.
    pub placeholder: String,
    /// The only hosts it is swapped for.
    pub hosts: Vec<String>,
    /// An own key's provider that serves models: its chat-completions base
    /// URL, where an agent whose `agent.json` names one of its models sends
    /// its calls, the placeholder its key.
    #[serde(default, rename = "modelBase", skip_serializing_if = "Option::is_none")]
    pub model_base: Option<String>,
}

/// The most credentials one agent is given (the platform's catalog holds
/// at most 64 providers).
pub const CREDENTIALS_MAX: usize = 64;

/// A file on the bridge's disk: an attachment, downloaded for the runtime
/// or written by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub path: PathBuf,
    pub media_type: String,
    pub name: String,
    pub size: u64,
}

/// Files a person sent with an answer (`Command::Tell`, `Command::Answer`),
/// as their message names them. The driver downloads them as a turn's
/// attachments are, adds each one's name and where it is on this computer
/// to the answer's words (`with_files`), and takes them off the command
/// before the runtime hears it: a runtime takes an answer in words alone
/// (Hermes' clarify does), so its words are how the agent learns of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    /// The agent, and the chat its blobs are read from.
    pub agent: String,
    pub fragment: String,
    /// Who sent them, as the agent calls them.
    pub by_name: String,
    pub refs: Vec<crate::records::AttachmentRef>,
}

/// A message's words, then a line for each downloaded file: its original name,
/// its type, and where it is on this computer; and one for any it could not
/// read. Names are the sender's: anything but plain text in them is a space.
pub fn with_files(words: &str, by_name: &str, expected: usize, files: &[LocalFile]) -> String {
    let plain = |s: &str| s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let by = plain(by_name);
    let mut lines: Vec<String> = Vec::new();
    if !words.trim().is_empty() {
        lines.push(words.to_string());
    }
    for f in files {
        lines.push(format!("[{by} sent a file: '{}' ({}). It is on your computer at: {}]", plain(&f.name), plain(&f.media_type), f.path.display()));
    }
    let unread = expected.saturating_sub(files.len());
    if unread > 0 {
        lines.push(format!("[{by} also sent {unread} file{} that could not be read]", if unread == 1 { "" } else { "s" }));
    }
    lines.join("\n")
}

/// One of the commands a runtime takes from its agent's owner (a chat's
/// `{kind: "command"}` record, docs/chat-records.md): its menu, which the
/// bridge publishes in each chat (`commands` on `work`), and how the
/// bridge carries each. Nothing outside the menu reaches the runtime as a
/// command: a person's text that starts with `/` is a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem {
    /// `^[a-z0-9_-]{1,32}$`, without its `/`.
    pub name: &'static str,
    pub description: &'static str,
    /// What its argument is, for the person typing it; `None`: it takes none.
    pub args: Option<&'static str>,
    pub how: How,
    /// Words of its argument that refuse it, each with why: its turn ends
    /// saying so, and the runtime never hears it.
    pub refuse: &'static [(&'static str, &'static str)],
}

/// How the bridge carries a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// A turn of its own, queued as a message's is: the runtime is handed
    /// `/<name> <args>` as the turn's text, said as it is
    /// (`TurnStart::command`).
    Turn,
    /// Stops the agent's running turn in the chat, whoever asked it, and
    /// ends its waiting ones there (they never run). No turn of its own.
    Stop,
    /// As `Stop`, then a turn of its own: a new session for the chat.
    Restart,
    /// Its argument is a message (a turn like any); with none, a `Turn`.
    Message,
    /// Its argument goes into the agent's running turn in the chat at once
    /// (`Command::Aside`); with no turn running, it is a message.
    Steer,
    /// Said beside whatever runs, at once, no turn of its own
    /// (`Command::Aside`): what the runtime answers is a message of its own.
    Aside,
}

/// The message a person's message quotes (`reply_to`), as the runtime is
/// handed it: read from the chat by the driver before the turn is handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// The quoted record's id to the runtime (its turn id, as if it were one).
    pub id: String,
    /// Its text, at most `limits::QUOTE_TEXT_MAX_BYTES` (a file without
    /// words, its name).
    pub text: String,
    /// It is the agent's own.
    pub own: bool,
}

/// A turn handed to a runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnStart {
    pub turn: String,
    pub agent: Agent,
    /// The chat the turn answers in.
    pub fragment: String,
    /// The chat's label, for the runtime to call it by.
    pub chat_name: String,
    /// The record that started it (its seq in its channel).
    pub seq: u64,
    /// Who asked, and what the runtime calls them.
    pub asker: String,
    pub asker_name: String,
    pub text: String,
    /// The message's attachments, as the record names them.
    pub attachments: Vec<crate::records::AttachmentRef>,
    /// The same attachments on disk (the driver downloads them before the
    /// runtime hears of the turn).
    pub files: Vec<LocalFile>,
    /// A routine (its agent's cron) rather than a person's message.
    pub routine: bool,
    /// Where the turn's claim (its `turn.start`) is on its chat's `work`:
    /// the journal before it says what the turn is told (`note`). `None`
    /// when the platform's answer did not say.
    pub claim_seq: Option<u64>,
    /// What the runtime tells the agent before the message, once
    /// (docs/durable-computers.md, P5): its turn before this one in this
    /// chat was cut by a restart, what that turn was asked, the steps and
    /// replies the journal recorded of it, and to check what was done before
    /// doing any of it again. Built from the chat's journal alone
    /// (crate::note), by the driver, before the runtime hears of the turn;
    /// `None` when that turn ended any other way. Every runtime is handed
    /// it: Relay as the inbound's read-only `context`, the scripted agent
    /// echoes it. It also says what the agent no longer remembers of this
    /// chat (`forgotten`).
    pub note: Option<String>,
    /// The agent's turns in this chat that another life ran since the
    /// `/data` this life restored (their claims answered 409: a rollback
    /// sent the cursor back), the newest at most `NOTE_FORGOTTEN_MAX`, and
    /// how many more: in no memory of the runtime's, so the note says what
    /// they were (crate::note, "forgotten"). Said once, at the agent's
    /// first turn in the chat after them.
    pub forgotten: Vec<String>,
    pub forgotten_more: u32,
    /// Its text is a command of the runtime's menu (`/<name> <args>`), said
    /// to it as it is; a message's never is.
    pub command: bool,
    /// The seq on `chat` of the message the turn's message quotes, and that
    /// message (`quote`, read by the driver before the turn is handed;
    /// `None` when it could not be read, or is no message).
    pub reply_to: Option<u64>,
    pub quote: Option<Quote>,
    /// Its chat is its owner's SimpleX chat while linked (`Engine::mirror`):
    /// what the agent says there reaches their phone, whatever started the
    /// turn. Relay tells its agent so in that chat's session
    /// (`relay::wire::SIMPLEX_TOPIC`).
    pub simplex: bool,
    /// It continues the turn named, which a restart cut (engine.rs,
    /// `continue_turn`): its note is that turn's, by its id (driver.rs,
    /// `cut_note`); said by the computer, never as its asker's words.
    pub continues: Option<String>,
}

/// What the bridge asks of a runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Start(Box<TurnStart>),
    /// The turn's asker pressed Stop.
    Stop { turn: String },
    /// A prompt's answer (`option`), or its expiry (`None`): only ever once
    /// per prompt. `seq` is the answering record's (a fresh message id).
    /// `words`: what was typed for an option answered in words, on its card
    /// or as a message in its chat (that option's answer). `attached`: the
    /// files sent with such a message, which the driver puts in its words
    /// and takes off (`Attached`): a runtime never sees them here.
    Answer { turn: String, prompt: String, option: Option<String>, seq: u64, by: String, words: Option<String>, attached: Option<Attached> },
    /// The bridge ended the turn itself (it went quiet too long, or its
    /// agent left): forget it.
    Forget { turn: String },
    /// The asker's next message in the turn's chat, while the turn asked
    /// them something to answer in words (`Event::Asked`): the answer,
    /// handed to the running turn at once, never a turn of its own. `seq`
    /// is the message's record (a fresh message id). Its words, its files
    /// or both: `attached`, which the driver puts in its words and takes off
    /// (`Attached`): a runtime never sees them here.
    Tell { turn: String, seq: u64, by: String, by_name: String, text: String, attached: Option<Attached> },
    /// A command of the menu said in the agent's chat at once, beside
    /// whatever turn runs there, no turn of its own (`How::Steer` into the
    /// running turn `turn`, `How::Aside`): `/<name> <args>`, its id the
    /// record's (a turn id, as if it were one). What the runtime answers it
    /// with is a message of its own (`Event::Say`).
    /// `simplex`: its chat is its owner's SimpleX chat, as a turn's is
    /// (`TurnStart::simplex`).
    Aside { agent: String, fragment: String, chat_name: String, simplex: bool, turn: Option<String>, id: String, by: String, by_name: String, text: String },
}

/// What a runtime tells the bridge. Every event but `Connected`, `Say` and
/// `Notice` names its turn; one for a turn the bridge no longer holds is
/// dropped.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The runtime has work outside a chat turn (delegated children).
    /// Live only: a new bridge life starts with no such work.
    Working(bool),
    /// The runtime can take turns now (true), or cannot (false). The bridge
    /// claims a turn only while it can, so a turn claimed is one the runtime
    /// is there to run (Relay: Hermes has dialed and said hello). A runtime
    /// that can always take one says so as it starts.
    Connected(bool),
    /// The reply being written, its whole text so far (shown live, never
    /// stored).
    Draft { turn: String, text: String },
    /// Reply `part` (from 1, in order) of the turn, its whole text now. A
    /// later part, a step, a prompt, or the end posts it.
    Reply { turn: String, part: u32, text: String },
    /// Reply `part`, if still open, is interim: the turn worked on after it
    /// (its step came before it, and a later reply says so). A part posted
    /// already stays as it was.
    Interim { turn: String, part: u32 },
    /// A file reply `part` carries (opened empty if it was not).
    Attachment { turn: String, part: u32, file: LocalFile },
    /// Reply `part` taken back before it was posted.
    Retract { turn: String, part: u32 },
    /// A tool call the model asked for, before it runs (`records::Call`):
    /// its intent, recorded so a turn cut while it ran is told of it.
    Called { turn: String, call: crate::records::Call },
    /// A tool call, ended.
    Step { turn: String, step: Step },
    /// A question with buttons; the turn waits for its answer.
    Prompt { turn: String, prompt: String, text: String, options: Vec<PromptOption>, ttl_ms: Option<u64> },
    /// The turn asked its asker something to answer in words (the question
    /// itself is a reply part before this): it waits, running, for their
    /// next message in its chat, which the bridge hands it (`Command::Tell`)
    /// instead of queueing it as a turn behind this one.
    Asked { turn: String },
    /// The turn is over.
    End { turn: String, outcome: crate::records::Outcome },
    /// Something the runtime said with no turn running (a reminder it set):
    /// posted as a turn of its own in that chat.
    Say { agent: String, fragment: String, text: String, files: Vec<LocalFile> },
    /// A notice of the runtime's about the agent's work in a chat, neither a
    /// step nor a reply (it seems stuck, its context was compressed, it
    /// updated its memory): the agent's turn running there, or, with none,
    /// the last that ran there in this life, posts it (`turn.notice`).
    Notice { agent: String, fragment: String, category: crate::records::NoticeCategory, text: String },
}

/// Why a runtime stopped.
#[derive(Debug)]
pub enum RuntimeError {
    /// It could not listen (Relay's port), or read its configuration.
    Setup(String),
    /// Its loop failed for good.
    Failed(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Setup(m) => write!(f, "runtime setup: {m}"),
            RuntimeError::Failed(m) => write!(f, "runtime failed: {m}"),
        }
    }
}

/// A runtime's two queues, and the bridge's shutdown signal.
pub struct RuntimeIo {
    pub commands: mpsc::Receiver<Command>,
    pub events: mpsc::Sender<Event>,
    pub shutdown: watch::Receiver<bool>,
}

pub type RuntimeFuture = Pin<Box<dyn Future<Output = Result<(), RuntimeError>> + Send>>;

/// An agent runtime the bridge drives.
pub trait Runtime: Send + 'static {
    /// Its name in the log and in `--runtime`.
    fn name(&self) -> &'static str;
    /// The commands it takes from its agents' owners (none by default).
    fn menu(&self) -> &'static [MenuItem] {
        &[]
    }
    /// Runs until `shutdown` turns true (then returns promptly: the whole
    /// bridge has `limits::SHUTDOWN_MS_MAX`), or fails for good.
    fn run(self: Box<Self>, io: RuntimeIo) -> RuntimeFuture;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, at: &str) -> LocalFile {
        LocalFile { path: PathBuf::from(at), media_type: "image/png".into(), name: name.into(), size: 1 }
    }

    /// Goal: an agent's runtime is its `agent.json`'s `runtime`. Absent (no
    /// file, no field, `null`, a file that is not JSON): it names none, and
    /// runs the default. Anything else is kept as its name, cut to 64 bytes,
    /// so a value that is no runtime's is refused, never run as the default.
    #[test]
    fn an_agents_runtime_is_what_its_agent_json_names() {
        assert_eq!(named(None), None);
        assert_eq!(named(Some(b"{\"tier\":\"cheap\"}")), None);
        assert_eq!(named(Some(b"{\"runtime\":null}")), None);
        assert_eq!(named(Some(b"not json")), None);
        assert_eq!(named(Some(b"{\"runtime\":\"claude\"}")).as_deref(), Some("claude"));
        assert_eq!(named(Some(b"{\"runtime\":7}")).as_deref(), Some("7"));
        let long = format!("{{\"runtime\":\"{}\"}}", "x".repeat(100));
        assert_eq!(named(Some(long.as_bytes())).map(|n| n.len()), Some(64));
        assert!(valid_name(DEFAULT) && valid_name("codex") && !valid_name("Claude") && !valid_name("") && !valid_name(&"a".repeat(33)));
    }

    /// Goal: a bridge runs each runtime once, by a valid name, at most
    /// `RUNTIMES_MAX`; one runtime alone is the default's.
    #[test]
    fn runtimes_are_named_once() {
        let script = || -> Box<dyn Runtime> { Box::new(script::Script { config: script::ScriptConfig { pace: std::time::Duration::ZERO, scratch: PathBuf::from("/tmp/x"), data: PathBuf::from("/tmp/y") } }) };
        let one: Runtimes = script().into();
        assert_eq!(one.into_inner().iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec![DEFAULT]);
        assert!(std::panic::catch_unwind(|| Runtimes::new().with("a", script()).with("a", script())).is_err(), "twice");
        assert!(std::panic::catch_unwind(|| Runtimes::new().with("No", script())).is_err(), "not a name");
        let full = (0..crate::limits::RUNTIMES_MAX).fold(Runtimes::new(), |r, i| r.with(&format!("r{i}"), script()));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| full.with("more", script()))).is_err(), "past the bound");
    }

    /// Goal: an answer's words name each file sent with it and where it is
    /// on the computer, a line each after the words (files alone: no words
    /// line); those not downloaded are counted; a sender's name or a file's
    /// that would break a line is one line still.
    #[test]
    fn an_answers_files_are_named_in_its_words() {
        let a = file("a.png", "/tmp/bridge-media/aa");
        let line = "[paul sent a file: 'a.png' (image/png). It is on your computer at: /tmp/bridge-media/aa]";
        assert_eq!(with_files("these", "paul", 1, std::slice::from_ref(&a)), format!("these\n{line}"));
        assert_eq!(with_files(" ", "paul", 1, std::slice::from_ref(&a)), line, "files alone");
        assert_eq!(with_files("", "paul", 3, std::slice::from_ref(&a)).lines().last(), Some("[paul also sent 2 files that could not be read]"));
        assert_eq!(with_files("ok", "paul", 1, &[]), "ok\n[paul also sent 1 file that could not be read]", "one unread, said so");
        let odd = with_files("", "pa\nul", 1, &[file("a]\n[b.png", "/tmp/x")]);
        assert_eq!(odd.lines().count(), 1, "{odd}");
    }
}
