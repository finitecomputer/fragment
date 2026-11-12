//! `fragment acp <agent>`: one of a person's agents as an ACP agent, over
//! stdio (cli/GUIDE.md, "Your agent in an ACP client"; the Agent Client
//! Protocol, agentclientprotocol.com, version 1). JSON-RPC 2.0, one
//! message a line each way. Stdout carries the protocol's messages and
//! nothing else: a failure to start goes to stderr (main.rs).
//!
//! It is one more client of the platform's public API, signed with this
//! machine's key, as the chat's page is one. The agent runs where it
//! always does, on its owner's computer, through its bridge: nothing here
//! knows its runtime, and nothing of the platform knows ACP.
//!
//! - **A session is a chat**, named in full: its id is the chat's name.
//!   `session/new` makes one with the agent in it, as the shell's New chat
//!   does; `session/list` lists the person's chats with the agent;
//!   `session/load` replays one and `session/resume` opens one untold
//!   (history.rs). The server keeps nothing of a session but that it is
//!   open: what a prompt must know of its chat is read as the prompt comes.
//! - **A prompt is a message** to the agent in that chat, and the turn it
//!   causes is the prompt's: its drafts, replies, calls and steps are the
//!   session's updates as they come (turn.rs), over the chat's live socket
//!   (feed.rs), and its end is the prompt's answer. A turn's error fails
//!   the prompt with the turn's own reason.
//! - **`session/cancel` is a Stop** of that turn, as the chat's page posts
//!   one, named at once by the id its message makes it (`turn::id`): a
//!   turn that has not started yet never runs.
//! - **It takes words.** A linked resource is named in them; a picture,
//!   audio or an embedded file is refused (a file goes in the chat). The
//!   agent's tools are its own computer's: the client's files, terminals
//!   and MCP servers are never asked for.
//!
//! The wire is written by hand, as mcp.rs's is: the official schema crate
//! (`agent-client-protocol-schema`) turns on serde_json's `preserve_order`
//! for every crate built beside it, which this workspace's tests do not
//! run under.

mod chats;
mod feed;
mod history;
mod turn;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::Scope;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};

use crate::api::{Client, Code, CodedError};
use crate::ask::Agent;
pub use crate::jsonrpc::{INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR};
use crate::jsonrpc::{failure, line, read_line, success, Line, RpcError, MESSAGE_MAX_BYTES};
use chats::Seqs;
use feed::{Event, Feed, Live};
use turn::{End, Follow, Links, Record};

/// The protocol version this server speaks, whatever a client offers: a
/// client that speaks a later one too goes on in this one.
pub const PROTOCOL_VERSION: u64 = 1;
/// ACP's own error codes, beside JSON-RPC's.
pub const AUTH_REQUIRED: i64 = -32000;
pub const RESOURCE_NOT_FOUND: i64 = -32002;
/// A prompt's words at most: a message's text (docs/chat-records.md: more
/// is cut), refused here so nothing a person sent is dropped unsaid.
pub const PROMPT_MAX_BYTES: usize = 32 * 1024;

/// How long a prompt waits, for what.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// One read of the chat's feed: how soon a prompt hears a cancel, or
    /// its client leaving.
    pub tick: Duration,
    /// A stopped turn's end is waited for this long; past it the prompt
    /// answers `cancelled` all the same (a turn stopped before its bridge
    /// read its message has no end to wait for).
    pub stop_grace: Duration,
    /// A chat that says nothing at all this long fails its prompt. Twice
    /// the bridge's own bound on a silent turn (`TURN_IDLE_MS_MAX`, 15
    /// minutes), so while the agent's computer lives the turn's own end
    /// comes first.
    pub quiet_max: Duration,
    /// An answer to a turn that asked may be a turn of its own instead:
    /// the turn that asked was cut by a restart (its computer slept while
    /// it waited), ends as lost, and the new start takes the message once
    /// its runtime is up. Its start is waited for this long from that
    /// end; past it, the turn that asked took the answer and ended, and
    /// its end is the prompt's. A deployment's own image starts within
    /// this (the e2e's `HOSTED_WAKE`).
    pub fresh_start: Duration,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds { tick: Duration::from_millis(200), stop_grace: Duration::from_secs(10), quiet_max: Duration::from_secs(30 * 60), fresh_start: Duration::from_secs(5 * 60) }
    }
}

/// How a chat's feed is opened: its live socket, or a test's script.
pub type Open<'a> = dyn Fn(&'a Client, &str, Seqs) -> Result<Box<dyn Feed + 'a>> + Sync + 'a;

/// A session: a chat a client has open.
struct Session {
    chat: String,
    /// A prompt runs in it: one at a time.
    busy: AtomicBool,
    /// Its client cancelled the prompt that runs.
    cancel: AtomicBool,
}

/// Who a server serves.
struct Served {
    agent: Agent,
    /// Every agent of the person's, each as they call it.
    agents: Vec<Agent>,
    /// The person: their identity, as their records name them.
    me: String,
}

struct Server<'a, W: Write + Send> {
    client: &'a Client,
    /// The agent, as the command named it.
    who: String,
    /// Who is served, asked of the platform once.
    served: OnceLock<Served>,
    out: Mutex<W>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// The client left (its input ended, or a write to it failed).
    closing: AtomicBool,
    open: &'a Open<'a>,
    bounds: Bounds,
}

/// Serves `who`, one of the signer's agents, to the ACP client on `input`
/// and `output` until the client closes `input`.
pub fn serve(client: &Client, who: &str, input: impl BufRead, output: impl Write + Send) -> Result<()> {
    let live = |c, chat: &str, seen| -> Result<Box<dyn Feed + '_>> { Ok(Box::new(Live::open(c, chat, seen)?)) };
    serve_with(client, who, input, output, &live, Bounds::default())
}

fn serve_with<'a>(client: &'a Client, who: &str, mut input: impl BufRead, output: impl Write + Send, open: &'a Open<'a>, bounds: Bounds) -> Result<()> {
    let server = Server {
        client,
        who: who.to_string(),
        served: OnceLock::new(),
        out: Mutex::new(output),
        sessions: Mutex::default(),
        closing: AtomicBool::new(false),
        open,
        bounds,
    };
    // a prompt runs beside the reader, which goes on hearing its cancel
    std::thread::scope(|scope| {
        let mut buf = Vec::new();
        // one message a line, until the client closes its end
        let read = loop {
            match read_line(&mut input, &mut buf) {
                Ok(Line::End) => break Ok(()),
                Err(e) => break Err(e),
                Ok(Line::TooLong) => server.send(&failure(Value::Null, RpcError::new(INVALID_REQUEST, format!("a message is at most {MESSAGE_MAX_BYTES} bytes")))),
                Ok(Line::Text) if buf.iter().all(u8::is_ascii_whitespace) => {}
                Ok(Line::Text) => match serde_json::from_slice::<Value>(&buf) {
                    Ok(message) => server.handle(scope, message),
                    Err(e) => server.send(&failure(Value::Null, RpcError::new(PARSE_ERROR, format!("not JSON: {e}")))),
                },
            }
            if server.closing.load(Ordering::Relaxed) {
                break Ok(());
            }
        };
        // its prompts end with it; their turns go on in their chats
        server.closing.store(true, Ordering::Relaxed);
        read
    })?;
    Ok(())
}

/// A refusal of the platform's or the CLI's as the protocol's: its code
/// where ACP has one, and the CLI's own code as its `data`.
fn refused(e: anyhow::Error) -> RpcError {
    let coded = e.chain().find_map(|cause| cause.downcast_ref::<CodedError>()).map(|c| c.code);
    let code = match coded {
        Some(Code::AuthFailed) => AUTH_REQUIRED,
        Some(Code::NotFound) => RESOURCE_NOT_FOUND,
        Some(Code::InvalidUsage | Code::InvalidRequest) => INVALID_PARAMS,
        _ => INTERNAL_ERROR,
    };
    RpcError { code, message: format!("{e:#}"), data: coded.map(|c| json!({ "code": c.as_str() })) }
}

/// A prompt's words: its text blocks, and each linked resource named (the
/// one kind beside text every agent takes). Anything else is refused: the
/// server says it takes none (`promptCapabilities`).
fn words(prompt: &Value) -> Result<String, RpcError> {
    let blocks = prompt.as_array().ok_or_else(|| RpcError::new(INVALID_PARAMS, "a prompt is its content blocks (params.prompt, an array)"))?;
    let mut lines = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => lines.push(block["text"].as_str().unwrap_or("").to_string()),
            Some("resource_link") => {
                let uri = block["uri"].as_str().unwrap_or("");
                lines.push(format!("[{}]({uri})", block["name"].as_str().unwrap_or(uri)));
            }
            other => {
                let kind = other.unwrap_or("of no type");
                return Err(RpcError::new(INVALID_PARAMS, format!("a prompt here is words: a block that is {kind} is not taken (send a file in the chat itself)")));
            }
        }
    }
    let text = lines.join("\n");
    if text.trim().is_empty() {
        return Err(RpcError::new(INVALID_PARAMS, "a prompt says something: its words are empty"));
    }
    if text.len() > PROMPT_MAX_BYTES {
        return Err(RpcError::new(INVALID_PARAMS, format!("a prompt is at most {PROMPT_MAX_BYTES} bytes of words (a message's size), and this is {}", text.len())));
    }
    Ok(text)
}

impl<'a, W: Write + Send> Server<'a, W> {
    /// One message to the client. A write that fails is the client gone.
    fn send(&self, message: &Value) {
        let mut out = self.out.lock().expect("the output");
        if out.write_all(&line(message)).and_then(|()| out.flush()).is_err() {
            self.closing.store(true, Ordering::Relaxed);
        }
    }

    fn answer(&self, id: Value, answer: Result<Value, RpcError>) {
        self.send(&match answer {
            Ok(result) => success(id, result),
            Err(e) => failure(id, e),
        });
    }

    /// Session updates, in order.
    fn tell(&self, chat: &str, updates: Vec<Value>) {
        for update in updates {
            self.send(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": chat, "update": update } }));
        }
    }

    /// Who is served: the agent, the person's agents, and the person.
    fn served(&self) -> Result<&Served, RpcError> {
        if let Some(served) = self.served.get() {
            return Ok(served);
        }
        let (agent, agents) = chats::agent(self.client, &self.who).map_err(refused)?;
        let me = chats::me(self.client).map_err(refused)?;
        Ok(self.served.get_or_init(|| Served { agent, agents, me }))
    }

    /// The agent served.
    fn agent(&self) -> Result<&Agent, RpcError> {
        Ok(&self.served()?.agent)
    }

    /// Who is who in a chat of the agent's: it, the person, and the
    /// person's other agents by their names.
    fn who(&self) -> Result<history::Who, RpcError> {
        let served = self.served()?;
        let names = served.agents.iter().map(|a| (a.identity.clone(), a.shown().to_string())).collect();
        Ok(history::Who { agent: served.agent.identity.clone(), me: served.me.clone(), names: fragment_core::export::Names(names) })
    }

    fn session(&self, params: &Value) -> Result<Arc<Session>, RpcError> {
        let id = params["sessionId"].as_str().ok_or_else(|| RpcError::new(INVALID_PARAMS, "a session is named (params.sessionId)"))?;
        let found = self.sessions.lock().expect("the sessions").get(id).cloned();
        found.ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("no session {id:?} is open: open it with session/load or session/resume, or make one with session/new")))
    }

    /// Opens `chat` as a session: the one open already, or a new one.
    fn opened(&self, chat: &str) {
        let mut sessions = self.sessions.lock().expect("the sessions");
        sessions.entry(chat.to_string()).or_insert_with(|| Arc::new(Session { chat: chat.to_string(), busy: AtomicBool::new(false), cancel: AtomicBool::new(false) }));
    }

    /// One message: a request is answered (a prompt, once its turn ends),
    /// a notification acted on, and a client's own answer passed over
    /// (this server asks nothing of a client).
    fn handle<'scope>(&'scope self, scope: &'scope Scope<'scope, '_>, message: Value) {
        let Value::Object(mut message) = message else {
            return self.send(&failure(Value::Null, RpcError::new(INVALID_REQUEST, "a message is one JSON-RPC object (no batches)")));
        };
        let id = match message.remove("id") {
            None => None,
            Some(id @ (Value::String(_) | Value::Number(_))) => Some(id),
            Some(_) => return self.send(&failure(Value::Null, RpcError::new(INVALID_REQUEST, "an id is a string or a number"))),
        };
        let Some(method) = message.get("method").and_then(Value::as_str).map(str::to_string) else {
            if let Some(id) = id.filter(|_| !message.contains_key("result") && !message.contains_key("error")) {
                self.send(&failure(id, RpcError::new(INVALID_REQUEST, "a request names its method")));
            }
            return;
        };
        let params = message.remove("params").unwrap_or(Value::Null);
        let Some(id) = id else {
            if method == "session/cancel" {
                if let Ok(session) = self.session(&params) {
                    // only a prompt that runs is cancelled: the next clears it as it starts
                    session.cancel.store(session.busy.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            }
            return;
        };
        if method != "session/prompt" {
            return self.answer(id, self.request(&method, &params));
        }
        match self.begin(&params) {
            Ok((session, text)) => {
                scope.spawn(move || {
                    let answer = self.prompt(&session, &text);
                    session.busy.store(false, Ordering::Relaxed);
                    self.answer(id, answer);
                });
            }
            Err(e) => self.answer(id, Err(e)),
        }
    }

    fn request(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                let agent = self.agent()?;
                Ok(json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "agentCapabilities": {
                        "loadSession": true,
                        "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
                        "mcpCapabilities": { "http": false, "sse": false },
                        "sessionCapabilities": { "list": {}, "resume": {} },
                    },
                    "agentInfo": { "name": "fragment", "title": agent.shown(), "version": crate::VERSION },
                    // the CLI's own key signs: `fragment login`, before this runs
                    "authMethods": [],
                }))
            }
            "session/new" => {
                let chat = chats::make(self.client, self.agent()?).map_err(refused)?;
                self.opened(&chat);
                Ok(json!({ "sessionId": chat }))
            }
            "session/resume" => self.reopen(params, false),
            "session/load" => self.reopen(params, true),
            "session/list" => {
                let agent = self.agent()?;
                let listed = chats::listed(self.client).map_err(refused)?;
                Ok(json!({ "sessions": history::sessions(&listed, &agent.identity, params["cwd"].as_str().unwrap_or("/")) }))
            }
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("no method {other:?}: this agent serves initialize, session/new, session/list, session/load, session/resume, session/prompt and session/cancel"),
            )),
        }
    }

    /// Opens a chat the person already has as a session: `session/load`
    /// tells what it said first (`replay`), `session/resume` nothing.
    fn reopen(&self, params: &Value, replay: bool) -> Result<Value, RpcError> {
        let agent = self.agent()?;
        let chat = params["sessionId"].as_str().ok_or_else(|| RpcError::new(INVALID_PARAMS, "a session is named (params.sessionId)"))?;
        // a session's id goes into a path: only a fragment's name is one
        if !fragment_proto::valid_fragment_name(chat) {
            return Err(RpcError::new(INVALID_PARAMS, format!("{chat:?} is no session's id: one is a chat's name in full (`fred-chat-2--k3x9`)")));
        }
        match chats::answers(self.client, chat, agent).map_err(refused)? {
            None => return Err(RpcError::new(RESOURCE_NOT_FOUND, format!("no chat named {chat}"))),
            Some(false) => return Err(RpcError::new(INVALID_PARAMS, format!("{} does not answer in {chat}: add it to the chat, or start a session with session/new", agent.shown()))),
            Some(true) => {}
        }
        let seen = chats::seqs(self.client, chat).map_err(refused)?;
        if replay {
            let back = |seq: i64| (seq - chats::HISTORY_RECORDS_MAX).max(0);
            let (said, _) = chats::records(self.client, chat, "chat", back(seen.chat)).map_err(refused)?;
            let (work, _) = chats::records(self.client, chat, "work", back(seen.work)).map_err(refused)?;
            self.tell(chat, history::replay(&said, &work, &self.who()?, &Links::of(&self.client.host, chat)));
        }
        self.opened(chat);
        Ok(json!({}))
    }

    /// A prompt as it comes: its session, free to take one, and its words.
    fn begin(&self, params: &Value) -> Result<(Arc<Session>, String), RpcError> {
        let session = self.session(params)?;
        let text = words(&params["prompt"])?;
        if session.busy.swap(true, Ordering::Relaxed) {
            return Err(RpcError::new(INVALID_REQUEST, format!("a prompt is already running in {}: wait for its answer, or cancel it (session/cancel)", session.chat)));
        }
        session.cancel.store(false, Ordering::Relaxed);
        Ok((session, text))
    }

    /// One prompt, to its turn's end: the message posted, its turn told as
    /// it happens, and how it ended.
    fn prompt(&self, session: &Session, text: &str) -> Result<Value, RpcError> {
        let agent = self.agent()?;
        let chat = session.chat.as_str();
        let links = Links::of(&self.client.host, chat);
        // the turn that waits for an answer, if one does, read now: the
        // message is then its answer, and starts no turn of its own
        let newest = chats::seqs(self.client, chat).map_err(refused)?;
        let back = |seq: i64| (seq - chats::ASKING_BACK_RECORDS).max(0);
        let (work, work_seen) = chats::records(self.client, chat, "work", back(newest.work)).map_err(refused)?;
        let (before, chat_seen) = chats::records(self.client, chat, "chat", back(newest.chat)).map_err(refused)?;
        let waiting = turn::asking(&before, &work, &agent.identity, &self.served()?.me);
        // followed from where that read ended, so no record falls between,
        // and from before the message is posted: a draft is never stored,
        // so one said before the socket opened would be lost
        let mut feed = (self.open)(self.client, chat, Seqs { chat: chat_seen, work: work_seen }).map_err(refused)?;
        let posted = chats::say(self.client, chat, text, agent).map_err(refused)?;
        // the turns a cancel stops before any has shown itself: the one the
        // message makes, and one it may be the answer to
        let may_run: Vec<String> = [Some(turn::id(&agent.fragment, chat, posted.seq)), waiting.clone()].into_iter().flatten().collect();
        let mut follow = Follow::new(&agent.identity, posted.seq, waiting, links.clone());
        // the last record of `chat` told, and when the chat last said anything
        let (mut said, mut heard) = (posted.seq, Instant::now());
        // the turns stopped, and when the first was
        let (mut stops, mut stopped) = (Vec::<String>::new(), None::<Instant>);
        // when the turn that asked ended with nothing said of the answer
        let mut gone = None::<Instant>;
        // bounded: every pass is a tick, and a chat quiet for `quiet_max` ends it
        while follow.end().is_none() {
            if self.closing.load(Ordering::Relaxed) {
                return Err(RpcError::new(INTERNAL_ERROR, "the client left"));
            }
            // a Stop names its turn: the one followed, once it has shown
            // itself; until then, each it may be (a stop of none is nothing)
            if session.cancel.load(Ordering::Relaxed) {
                let named = follow.turn().map_or(may_run.clone(), |turn| vec![turn.to_string()]);
                for turn in named.into_iter().filter(|turn| !stops.contains(turn)).collect::<Vec<_>>() {
                    chats::stop(self.client, chat, &turn).map_err(refused)?;
                    stops.push(turn);
                    stopped.get_or_insert_with(Instant::now);
                }
            }
            // the turn that asked is gone: the answer's own turn is waited
            // for, and past the wait the gone turn's end is the prompt's
            if follow.gone().is_some() && gone.get_or_insert_with(Instant::now).elapsed() >= self.bounds.fresh_start {
                follow.settle();
                continue;
            }
            let updates = match feed.next(self.bounds.tick) {
                Event::Record(r) => {
                    if r.channel == "chat" {
                        said = said.max(r.seq);
                    }
                    follow.record(&r)
                }
                Event::Draft { principal, turn, text } => follow.draft(&principal, &turn, text.as_deref()),
                Event::Quiet => {
                    if stopped.is_some_and(|at| at.elapsed() >= self.bounds.stop_grace) {
                        return Ok(json!({ "stopReason": "cancelled" }));
                    }
                    if heard.elapsed() >= self.bounds.quiet_max {
                        let minutes = self.bounds.quiet_max.as_secs() / 60;
                        return Err(RpcError::new(INTERNAL_ERROR, format!("{} has said nothing for {minutes} minutes. Its turn may still be running: {}", agent.shown(), links.chat)));
                    }
                    continue;
                }
            };
            heard = Instant::now();
            self.tell(chat, updates);
        }
        // a turn's last words may reach the socket after its end: read
        // them. Unread, they are never passed over in silence: the prompt
        // fails, saying where they are.
        let (trailing, _): (Vec<Record>, i64) = chats::records(self.client, chat, "chat", said).map_err(|e| {
            let why = refused(e);
            RpcError { message: format!("{}'s turn ended, but its last words could not be read ({}). They are in the chat: {}", agent.shown(), why.message, links.chat), ..why }
        })?;
        for r in &trailing {
            self.tell(chat, follow.record(r));
        }
        match follow.end().expect("the loop ends at the turn's") {
            // a question in words: the next prompt reads that it waits
            End::Idle | End::Asked => Ok(json!({ "stopReason": "end_turn" })),
            End::Stopped => Ok(json!({ "stopReason": "cancelled" })),
            End::Error(why) => Err(RpcError { code: INTERNAL_ERROR, message: why.clone(), data: Some(json!({ "turn": follow.turn(), "chat": chat })) }),
        }
    }
}
