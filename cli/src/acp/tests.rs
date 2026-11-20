//! `fragment acp` as an ACP client drives it, in-process: a fake host for
//! the platform's routes and a scripted feed for the chat's live socket,
//! so every case of the protocol runs with no network of its own. The
//! binary over a real socket is cli/tests/acp.rs; a real computer, the
//! e2e's `computers` section.

use std::io::{BufReader, Read, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fragment_fakes::http::{Request, Response, Server as FakeHost};
use serde_json::{json, Value};

use super::feed::{Event, Feed};
use super::turn::Record;
use super::*;
use crate::api::Client;

const FRED: &str = "npub1fred";
/// The person, as the platform names them (`GET /api/identities/me`):
/// an identity of their own, which is none of their keys'.
fn me() -> String {
    "npub1paul".to_string()
}
/// The chat the fake host makes, and one it already holds.
const MADE: &str = "fred-chat-2--a2b3";
const KEPT: &str = "fred-chat--c4d5";
/// How long a test waits for the server to say something.
const SOON: Duration = Duration::from_secs(10);

/// What the fake host holds: each chat's records, and what it was asked.
#[derive(Default)]
struct Held {
    chat: Vec<Value>,
    work: Vec<Value>,
    /// Each request: method, path with its query, and body.
    asked: Vec<(String, String, Value)>,
    /// Chats made by `POST /api/fragments`.
    made: Vec<Value>,
    /// The chat's records are refused to its reader (403).
    unread: bool,
}

type Shared = Arc<Mutex<Held>>;

fn record(channel: &str, seq: i64, principal: &str, body: Value) -> Value {
    json!({ "channel": channel, "seq": seq, "at": seq * 1000, "principal": principal, "kind": "message", "body": body })
}

/// A host with one person (paul), their agents fred and maple, and two
/// chats with fred: `KEPT`, which holds `held`'s records, and `MADE`,
/// which `POST /api/fragments` makes.
fn host(held: Held) -> (FakeHost, Shared) {
    let shared: Shared = Arc::new(Mutex::new(held));
    let state = Arc::clone(&shared);
    let host = FakeHost::start(
        0,
        Arc::new(move |r: &Request| {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            let mut held = state.lock().unwrap();
            let query = match r.query.get("after") {
                Some(after) => format!("?after={after}"),
                None => String::new(),
            };
            held.asked.push((r.method.clone(), format!("{}{query}", r.path), body.clone()));
            let after: i64 = r.query.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
            let chat = [MADE, KEPT].into_iter().find(|c| r.path.starts_with(&format!("/api/f/{c}/")));
            let rest = chat.map(|c| &r.path[format!("/api/f/{c}/").len()..]);
            let page = |channel: &str, records: &[Value]| {
                let after_it: Vec<&Value> = records.iter().filter(|x| x["seq"].as_i64().unwrap_or(0) > after).collect();
                let next = after_it.last().map_or(after, |x| x["seq"].as_i64().unwrap_or(after));
                Response::json(200, &json!({ "channel": channel, "records": after_it, "next": next, "more": false }))
            };
            match (r.method.as_str(), r.path.as_str(), rest) {
                ("GET", "/api/identities/me", _) => Response::json(200, &json!({ "id": &me(), "kind": "person" })),
                ("GET", "/api/computers", _) => Response::json(
                    200,
                    &json!({ "computers": [{ "agents": [
                        { "fragment": "fred--k3x9", "identity": FRED, "name": "fred" },
                        { "fragment": "maple--p2m4", "identity": "npub1maple", "name": "maple" },
                    ] }] }),
                ),
                ("GET", "/api/fragments", _) => Response::json(
                    200,
                    &json!({ "fragments": [
                        { "name": "fred--k3x9", "kind": "agent", "role": "owner", "title": "Frederick", "agents": [] },
                        { "name": KEPT, "kind": "chat", "role": "owner", "title": "Beans", "agents": [FRED], "previewAt": 1_790_000_000_000_i64, "preview": "ok" },
                    ] }),
                ),
                ("POST", "/api/fragments", _) => {
                    held.made.push(body);
                    Response::json(
                        200,
                        &json!({ "name": MADE, "npub": "n", "owner": &me(), "visibility": "members", "viewToken": "v", "inboxToken": "i", "repo": "", "canonical": "https://x/" }),
                    )
                }
                ("GET", _, Some("channels")) => {
                    let seq = |records: &[Value]| records.last().map_or(0, |x| x["seq"].as_i64().unwrap_or(0));
                    Response::json(
                        200,
                        &json!({ "channels": [
                            { "name": "chat", "read": "public", "post": "viewer", "seq": seq(&held.chat) },
                            { "name": "work", "read": "viewer", "post": "editor", "seq": seq(&held.work) },
                        ] }),
                    )
                }
                ("GET", _, Some("members")) => Response::json(
                    200,
                    &json!({ "members": [
                        { "principal": &me(), "role": "owner", "addedBy": &me(), "addedAt": 1 },
                        { "principal": FRED, "role": "editor", "addedBy": &me(), "addedAt": 2 },
                    ] }),
                ),
                ("POST", _, Some("channels/chat")) => {
                    let seq = held.chat.last().map_or(0, |x| x["seq"].as_i64().unwrap_or(0)) + 1;
                    let posted = record("chat", seq, &me(), body["body"].clone());
                    held.chat.push(posted.clone());
                    Response::json(200, &json!({ "record": posted, "replayed": false }))
                }
                ("GET", _, Some("channels/chat")) if held.unread => Response::json(403, &json!({ "error": "forbidden", "message": "not yours to read" })),
                ("GET", _, Some("channels/chat")) => page("chat", &held.chat),
                ("GET", _, Some("channels/work")) => page("work", &held.work),
                _ => Response::json(404, &json!({ "error": "not_found", "message": "no fragment named that" })),
            }
        }),
    )
    .expect("start the fake host");
    (host, shared)
}

/// A client's stdin: each message sent is one line; dropped, its end.
struct Lines(Receiver<Vec<u8>>, Vec<u8>);

impl Read for Lines {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.1.is_empty() {
            match self.0.recv() {
                Ok(line) => self.1 = line,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.1.len());
        buf[..n].copy_from_slice(&self.1[..n]);
        self.1.drain(..n);
        Ok(n)
    }
}

/// A client's stdout: each line the server writes, as the JSON it is.
struct Said(Sender<Value>, Vec<u8>);

impl Write for Said {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.1.extend_from_slice(buf);
        while let Some(end) = self.1.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.1.drain(..=end).collect();
            let message = serde_json::from_slice(&line).unwrap_or_else(|e| panic!("stdout carries only JSON-RPC: {:?}: {e}", String::from_utf8_lossy(&line)));
            let _ = self.0.send(message);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A chat's feed as a test scripts it.
struct Scripted(Arc<Mutex<Receiver<Event>>>);

impl Feed for Scripted {
    fn next(&mut self, wait: Duration) -> Event {
        self.0.lock().unwrap().recv_timeout(wait).unwrap_or(Event::Quiet)
    }
}

/// A session with the server: what a client sends and hears, and what the
/// chat's feed says.
struct Driven {
    to: Option<Sender<Vec<u8>>>,
    from: Receiver<Value>,
    feed: Sender<Event>,
    held: Shared,
    /// The chats a feed was opened on, and from where.
    opened: Arc<Mutex<Vec<(String, chats::Seqs)>>>,
    server: Option<std::thread::JoinHandle<()>>,
    _host: FakeHost,
}

impl Driven {
    fn start(held: Held, bounds: Bounds) -> Driven {
        let (host, held) = host(held);
        let (to, lines) = channel::<Vec<u8>>();
        let (said, from) = channel::<Value>();
        let (feed, events) = channel::<Event>();
        let events = Arc::new(Mutex::new(events));
        let opened: Arc<Mutex<Vec<(String, chats::Seqs)>>> = Arc::default();
        let (base, log) = (host.url.clone(), Arc::clone(&opened));
        let server = std::thread::spawn(move || {
            let key = crate::auth::fixed(7);
            let client = Client::new(&base, key);
            let open = move |_: &Client, chat: &str, seen: chats::Seqs| -> anyhow::Result<Box<dyn Feed>> {
                log.lock().unwrap().push((chat.to_string(), seen));
                Ok(Box::new(Scripted(Arc::clone(&events))))
            };
            serve_with(&client, "fred", BufReader::new(Lines(lines, Vec::new())), Said(said, Vec::new()), &open, bounds).expect("the session ends at its input's end");
        });
        Driven { to: Some(to), from, feed, held, opened, server: Some(server), _host: host }
    }

    /// A session as a client begins one.
    fn begun() -> Driven {
        let mut d = Driven::start(Held::default(), Bounds::default());
        let hello = d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
        assert_eq!(hello["result"]["protocolVersion"], 1, "{hello}");
        d
    }

    fn send(&self, message: Value) {
        self.to.as_ref().expect("the input is open").send(format!("{message}\n").into_bytes()).expect("the server reads");
    }

    /// The next message the server writes.
    #[track_caller]
    fn hear(&self) -> Value {
        match self.from.recv_timeout(SOON) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => panic!("the server said nothing in {SOON:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the server ended"),
        }
    }

    /// A request's answer, with nothing said before it.
    #[track_caller]
    fn ask(&mut self, request: Value) -> Value {
        self.send(request.clone());
        let answer = self.hear();
        assert_eq!(answer["id"], request["id"], "an answer to {request}: {answer}");
        answer
    }

    /// The server says nothing for a moment.
    #[track_caller]
    fn quiet(&self) {
        if let Ok(message) = self.from.recv_timeout(Duration::from_millis(300)) {
            panic!("the server said {message}");
        }
    }

    fn say(&self, event: Event) {
        self.feed.send(event).expect("a feed is read");
    }

    /// A record of the agent's the chat keeps, and no socket carries.
    fn keep(&self, channel: &str, seq: i64, body: Value) -> Value {
        let kept = record(channel, seq, FRED, body);
        let mut held = self.held.lock().unwrap();
        match channel {
            "chat" => held.chat.push(kept.clone()),
            _ => held.work.push(kept.clone()),
        }
        kept
    }

    /// The agent's record on `work`, as the platform has it: kept, and
    /// said to whoever follows the chat.
    fn work(&self, seq: i64, body: Value) {
        let kept = self.keep("work", seq, body);
        self.say(Event::Record(Record::of(&kept).unwrap()));
    }

    /// The agent's reply on `chat`, kept and said.
    fn reply(&self, seq: i64, body: Value) {
        let kept = self.keep("chat", seq, body);
        self.say(Event::Record(Record::of(&kept).unwrap()));
    }

    fn draft(&self, turn: &str, text: &str) {
        self.say(Event::Draft { principal: FRED.into(), turn: turn.into(), text: Some(text.into()) });
    }

    /// The bodies posted on `MADE`'s or `KEPT`'s chat, in order.
    fn posted(&self) -> Vec<Value> {
        let held = self.held.lock().unwrap();
        held.asked.iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/channels/chat")).map(|(_, _, b)| b.clone()).collect()
    }

    /// Waits until the host was posted `n` records.
    #[track_caller]
    fn posts(&self, n: usize) -> Vec<Value> {
        let until = std::time::Instant::now() + SOON;
        loop {
            let posted = self.posted();
            if posted.len() >= n {
                return posted;
            }
            assert!(std::time::Instant::now() < until, "{n} posts in {SOON:?}: {posted:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A new session: its id.
    #[track_caller]
    fn new_session(&mut self) -> String {
        let made = self.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/new", "params": { "cwd": "/work/repo", "mcpServers": [] } }));
        made["result"]["sessionId"].as_str().unwrap_or_else(|| panic!("a session: {made}")).to_string()
    }

    fn prompt(&self, id: i64, session: &str, text: &str) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": { "sessionId": session, "prompt": [{ "type": "text", "text": text }] } }));
    }

    /// The words of the updates heard until the answer to `id`, and it.
    #[track_caller]
    fn until_answer(&self, id: i64) -> (Vec<Value>, Value) {
        let mut updates = Vec::new();
        loop {
            let message = self.hear();
            if message["id"] == id {
                return (updates, message);
            }
            assert_eq!(message["method"], "session/update", "{message}");
            updates.push(message["params"].clone());
        }
    }

    /// The client closes its end: the server ends.
    fn close(mut self) {
        self.to = None;
        self.server.take().expect("the server").join().expect("the server ends cleanly");
    }
}

impl Drop for Driven {
    fn drop(&mut self) {
        self.to = None;
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn start(turn: &str, cause: i64) -> Value {
    json!({ "kind": "turn.start", "turn": turn, "asker": &me(), "agent": FRED, "cause": { "fragment": MADE, "channel": "chat", "seq": cause }, "life": "0" })
}

/// Goal: a client is told what the server speaks and may ask of it:
/// protocol version 1 whatever it offers, words only, a session it can
/// open again, and no sign-in of its own (the CLI's key).
/// Invalid: an agent that is none of the person's refuses the session,
/// naming theirs.
#[test]
fn a_client_is_told_what_it_may_ask() {
    let mut d = Driven::start(Held::default(), Bounds::default());
    let hello = d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 2, "clientCapabilities": { "fs": { "readTextFile": true }, "terminal": true } } }));
    assert_eq!(
        hello["result"],
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": false,
                "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
                "mcpCapabilities": { "http": false, "sse": false },
                "sessionCapabilities": { "resume": {} },
            },
            "agentInfo": { "name": "fragment", "title": "Frederick", "version": crate::VERSION },
            "authMethods": [],
        })
    );
    d.close();

    let (to, lines) = channel::<Vec<u8>>();
    let (said, from) = channel::<Value>();
    let (host, _) = host(Held::default());
    let base = host.url.clone();
    let server = std::thread::spawn(move || {
        let client = Client::new(&base, crate::auth::fixed(7));
        let open = |_: &Client, _: &str, _: chats::Seqs| -> anyhow::Result<Box<dyn Feed>> { anyhow::bail!("no feed") };
        serve_with(&client, "rowan", BufReader::new(Lines(lines, Vec::new())), Said(said, Vec::new()), &open, Bounds::default())
    });
    to.send(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n".to_vec()).unwrap();
    let refused = from.recv_timeout(SOON).expect("an answer");
    assert_eq!(refused["error"]["code"], RESOURCE_NOT_FOUND, "{refused}");
    let why = refused["error"]["message"].as_str().unwrap_or("");
    assert!(why.contains("rowan") && why.contains("Frederick (fred--k3x9)") && why.contains("maple"), "{why}");
    drop(to);
    server.join().unwrap().expect("the session ends at its input's end");
}

/// Goal: a new session is a new chat with the agent in it, made as the
/// shell makes one, and a prompt is a message to the agent there: its
/// reply streams as it is written, and its turn's end is the prompt's.
/// The chat is followed from before the message is posted, so no draft is
/// missed.
#[test]
fn a_prompt_is_a_message_and_its_reply_streams() {
    let mut d = Driven::begun();
    let session = d.new_session();
    assert_eq!(session, MADE);
    assert_eq!(d.held.lock().unwrap().made, [json!({ "label": "fred-chat-2", "template": "chat", "title": "New chat", "agent": FRED })]);

    d.prompt(2, &session, "hi there");
    let posted = d.posts(1);
    assert_eq!(posted[0]["body"], json!({ "text": "hi there", "to": [FRED] }));
    assert!(posted[0]["id"].as_str().is_some_and(|id| id.starts_with("acp-")), "{posted:?}");
    assert_eq!(*d.opened.lock().unwrap(), [(MADE.to_string(), chats::Seqs { chat: 0, work: 0 })], "followed from before the post");

    d.work(1, start("t1", 1));
    d.draft("t1", "echo: ");
    d.draft("t1", "echo: [paul] hi");
    d.reply(2, json!({ "text": "echo: [paul] hi there", "turn": "t1" }));
    d.work(2, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }));
    let chunk = |text: &str| json!({ "sessionId": MADE, "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } } });
    assert_eq!(updates, [chunk("echo: "), chunk("[paul] hi"), chunk(" there")]);
    d.close();
}

/// Goal: a turn's last words are told even when its end reaches the
/// socket first: they are read from the chat before the prompt answers.
#[test]
fn words_that_trail_the_turns_end_are_still_told() {
    let mut d = Driven::begun();
    let session = d.new_session();
    d.prompt(2, &session, "hi");
    d.posts(1);
    // the reply is on the chat, and the socket carried only the turn's end
    d.keep("chat", 2, json!({ "text": "echo: hi", "turn": "t1" }));
    d.work(1, start("t1", 1));
    d.work(2, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"]["stopReason"], "end_turn");
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(updates[0]["update"]["content"]["text"], "echo: hi");
    d.close();
}

/// Goal: a tool call is told as it is asked for and as it ends, and a turn
/// that ends as an error fails its prompt with the turn's own reason.
#[test]
fn tool_calls_are_told_and_an_error_fails_the_prompt() {
    let mut d = Driven::begun();
    let session = d.new_session();
    d.prompt(2, &session, "tool please");
    d.posts(1);
    d.work(1, start("t1", 1));
    d.work(2, json!({ "kind": "turn.call", "turn": "t1", "response": "r1", "tool_call_id": "call_0", "tool": "search", "category": "web", "args": "tool please" }));
    d.work(3, json!({ "kind": "turn.step", "turn": "t1", "step": 1, "response": "r1", "tool_call_id": "call_0", "tool": "search", "category": "web", "args": "tool please", "ok": true, "status": "completed" }));
    d.work(4, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }));
    let (updates, answer) = d.until_answer(2);
    let told: Vec<&Value> = updates.iter().map(|u| &u["update"]).collect();
    assert_eq!(*told[0], json!({ "sessionUpdate": "tool_call", "toolCallId": "t1:2", "title": "search: tool please", "kind": "fetch", "status": "in_progress" }));
    assert_eq!(*told[1], json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1:2", "status": "completed" }));
    assert_eq!(answer["error"]["code"], INTERNAL_ERROR, "{answer}");
    assert_eq!(answer["error"]["message"], "the runtime failed");
    assert_eq!(answer["error"]["data"], json!({ "turn": "t1", "chat": MADE }));
    d.close();
}

/// The id of the turn `fred` makes of the first message in `MADE`
/// (docs/chat-records.md, Turns): SHA-256 of
/// `fred--k3x9|fred-chat-2--a2b3/chat/1`, its first 24 hex digits.
const FIRST_TURN: &str = "ad13bf56407dcf0bea987abf";

fn stop(turn: &str) -> Value {
    json!({ "id": format!("stop:{turn}"), "body": { "kind": "stop", "turn": turn } })
}

/// Goal: a client's cancel is a Stop of the prompt's turn, as the chat's
/// page posts one, named at once by the id its message makes it, so a
/// turn that has not started never runs; the stopped turn answers the
/// prompt `cancelled`. A turn that starts under another id is stopped by
/// that one too. Invalid: a cancel of a session with no prompt running
/// stops nothing, and a cancel said twice stops once.
#[test]
fn a_cancel_stops_the_turn() {
    let mut d = Driven::begun();
    let session = d.new_session();
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    d.prompt(2, &session, "slowly now");
    d.posts(1);
    d.quiet();
    assert_eq!(d.posted().len(), 1, "an earlier cancel stops no later prompt");
    // cancelled before its turn starts: stopped by the name it will have
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    assert_eq!(d.posts(2)[1], stop(FIRST_TURN));
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    d.work(1, start(FIRST_TURN, 1));
    d.work(2, json!({ "kind": "turn.end", "turn": FIRST_TURN, "outcome": "stopped" }));
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "cancelled" }));
    assert_eq!(d.posted().len(), 2, "stopped once");

    // a turn that starts under another id (a later bridge's): stopped by it too
    d.prompt(3, &session, "slowly again");
    d.posts(3);
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    d.posts(4);
    d.work(3, start("t-other", 3));
    assert_eq!(d.posts(5)[4], stop("t-other"));
    d.work(4, json!({ "kind": "turn.end", "turn": "t-other", "outcome": "stopped" }));
    let (_, answer) = d.until_answer(3);
    assert_eq!(answer["result"], json!({ "stopReason": "cancelled" }));
    d.close();
}

/// Goal: a cancel of a prompt that answers a question stops the turn that
/// asked, whose answer it is, beside the turn the message would start.
#[test]
fn a_cancel_of_an_answer_stops_the_turn_that_asked() {
    let mut d = Driven::start(asking(), Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": KEPT } }));
    let stops: Vec<Value> = d.posts(3)[1..].iter().map(|p| p["body"]["turn"].clone()).collect();
    assert!(stops.contains(&json!("t1")) && stops.len() == 2, "the turn that asked, and the one the message would start: {stops:?}");
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "stopped" }));
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "cancelled" }));
    d.close();
}

/// Goal: a turn that will not stop does not hold its prompt: past the
/// grace a cancelled prompt answers `cancelled`; and a chat that says
/// nothing at all for the longest quiet fails the prompt, saying where
/// the turn may still be running.
#[test]
fn a_prompt_is_never_held_without_bound() {
    let fast = Bounds { tick: Duration::from_millis(10), stop_grace: Duration::from_millis(150), ..Bounds::default() };
    let mut d = Driven::start(Held::default(), fast);
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    let session = d.new_session();
    d.prompt(2, &session, "slowly");
    d.posts(1);
    d.work(1, start("t1", 1));
    d.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "cancelled" }));
    d.close();

    let fast = Bounds { tick: Duration::from_millis(10), quiet_max: Duration::from_millis(150), ..Bounds::default() };
    let mut d = Driven::start(Held::default(), fast);
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    let session = d.new_session();
    d.prompt(2, &session, "anyone?");
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["error"]["code"], INTERNAL_ERROR, "{answer}");
    let why = answer["error"]["message"].as_str().unwrap_or("");
    assert!(why.contains("Frederick") && why.contains(&format!("?chat={MADE}")), "{why}");
    d.close();
}

/// Goal: a turn that asks its person in words ends the prompt, and the
/// next prompt is the answer: the same turn goes on with it, to its end.
#[test]
fn a_question_in_words_is_answered_by_the_next_prompt() {
    let mut d = Driven::begun();
    let session = d.new_session();
    d.prompt(2, &session, "ask-me");
    d.posts(1);
    d.work(1, start("t1", 1));
    d.reply(2, json!({ "text": "What should I call it?", "turn": "t1" }));
    d.work(2, json!({ "kind": "turn.asked", "turn": "t1", "asks": &me() }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }));
    assert_eq!(updates[0]["update"]["content"]["text"], "What should I call it?");

    d.prompt(3, &session, "Bean");
    assert_eq!(d.posts(2)[1]["body"], json!({ "text": "Bean", "to": [FRED] }));
    // no turn starts for the answer: the one that asked goes on
    d.reply(4, json!({ "text": "(told: Bean)", "turn": "t1" }));
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(3);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }));
    assert_eq!(updates[0]["update"]["content"]["text"], "(told: Bean)");
    d.close();
}

/// Goal: the protocol's refusals: a session never opened, a second prompt
/// while one runs, a prompt with no words or with a picture, one past a
/// message's size, an unknown method, and a line that is no JSON. The
/// session goes on after each.
#[test]
fn what_is_refused_and_how() {
    let mut d = Driven::begun();
    let refusal = |d: &mut Driven, request: Value| -> (i64, String) {
        let answer = d.ask(request);
        (answer["error"]["code"].as_i64().unwrap_or(0), answer["error"]["message"].as_str().unwrap_or("").to_string())
    };
    let prompt = |session: &str, blocks: Value| json!({ "jsonrpc": "2.0", "id": 9, "method": "session/prompt", "params": { "sessionId": session, "prompt": blocks } });
    let (code, why) = refusal(&mut d, prompt(KEPT, json!([{ "type": "text", "text": "hi" }])));
    assert!(code == INVALID_PARAMS && why.contains("session/resume"), "{code} {why}");
    let session = d.new_session();
    for (blocks, says) in [
        (json!([]), "words"),
        (json!([{ "type": "text", "text": "  " }]), "words"),
        (json!([{ "type": "image", "data": "", "mimeType": "image/png" }]), "image"),
        (json!([{ "type": "text", "text": "x".repeat(PROMPT_MAX_BYTES + 1) }]), "at most"),
        (json!("hi"), "content blocks"),
    ] {
        let (code, why) = refusal(&mut d, prompt(&session, blocks.clone()));
        assert!(code == INVALID_PARAMS && why.contains(says), "{blocks}: {code} {why}");
    }
    assert!(d.posted().is_empty(), "nothing refused was posted");
    // a resource a client links is named in the message's words
    d.send(prompt(&session, json!([{ "type": "text", "text": "read this" }, { "type": "resource_link", "uri": "file:///work/a.rs", "name": "a.rs" }])));
    assert_eq!(d.posts(1)[0]["body"]["text"], "read this\n[a.rs](file:///work/a.rs)");
    let (code, why) = refusal(&mut d, json!({ "jsonrpc": "2.0", "id": 10, "method": "session/prompt", "params": { "sessionId": session, "prompt": [{ "type": "text", "text": "again" }] } }));
    assert!(code == INVALID_REQUEST && why.contains("already"), "{code} {why}");
    let (code, _) = refusal(&mut d, json!({ "jsonrpc": "2.0", "id": 11, "method": "fs/read_text_file", "params": {} }));
    assert_eq!(code, METHOD_NOT_FOUND);
    d.to.as_ref().unwrap().send(b"not json\n".to_vec()).unwrap();
    assert_eq!(d.hear()["error"]["code"], PARSE_ERROR);
    // the client leaves mid-prompt: the server ends, and the turn goes on in the chat
    d.close();
}

/// Goal: a session opened again is the same chat: `session/resume` tells
/// nothing and goes on from it, a turn still asking among it; a chat that
/// does not exist, one named wrongly, and a second resume are answered as
/// such.
#[test]
fn a_session_resumed_is_the_same_chat() {
    let mut d = Driven::start(asking(), Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    let resume = |id: i64, session: &str| json!({ "jsonrpc": "2.0", "id": id, "method": "session/resume", "params": { "sessionId": session, "cwd": "/work/repo" } });
    assert_eq!(d.ask(resume(1, "nope--z9z9"))["error"]["code"], RESOURCE_NOT_FOUND);
    assert_eq!(d.ask(resume(2, "../../api/secrets"))["error"]["code"], INVALID_PARAMS);
    assert_eq!(d.ask(resume(3, KEPT))["result"], json!({}));
    assert_eq!(d.ask(resume(4, KEPT))["result"], json!({}), "resumed again");

    // the chat's turn was asking: the prompt is its answer
    d.prompt(5, KEPT, "Bean");
    d.posts(1);
    assert_eq!(*d.opened.lock().unwrap(), [(KEPT.to_string(), chats::Seqs { chat: 2, work: 2 })]);
    d.reply(4, json!({ "text": "(told: Bean)", "turn": "t1" }));
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(5);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }));
    assert_eq!(updates[0]["update"]["content"]["text"], "(told: Bean)");
    d.close();
}

/// A chat whose agent asked its person something in words, and waits.
fn asking() -> Held {
    Held {
        chat: vec![record("chat", 1, &me(), json!({ "text": "ask-me", "to": [FRED] })), record("chat", 2, FRED, json!({ "text": "What should I call it?", "turn": "t1" }))],
        work: vec![record("work", 1, FRED, start("t1", 1)), record("work", 2, FRED, json!({ "kind": "turn.asked", "turn": "t1", "asks": &me() }))],
        ..Held::default()
    }
}

/// Goal: an answer sent after the agent's computer slept is a turn of its
/// own (docs/bridge.md, "An answer in words goes only to its turn's
/// life"): the turn that asked ends as lost, which does not fail the
/// prompt, and the turn the answer starts is the prompt's.
#[test]
fn an_answer_after_the_computer_slept_is_a_turn_of_its_own() {
    let mut d = Driven::start(asking(), Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "lost when the computer restarted" }));
    d.quiet();
    d.work(4, start("t2", 3));
    d.reply(4, json!({ "text": "Bean it is.", "turn": "t2" }));
    d.work(5, json!({ "kind": "turn.end", "turn": "t2", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }), "{answer}");
    assert_eq!(updates[0]["update"]["content"]["text"], "Bean it is.");
    d.close();
}

/// Goal: a turn that asked, took its answer and ended at once with nothing
/// said starts no turn after it: past the wait for one, its end is the
/// prompt's.
#[test]
fn a_turn_that_ends_on_its_answer_ends_the_prompt() {
    let fast = Bounds { tick: Duration::from_millis(10), fresh_start: Duration::from_millis(150), ..Bounds::default() };
    let mut d = Driven::start(asking(), fast);
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }));
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["error"]["message"], "the runtime failed", "{answer}");
    d.close();
}

/// Goal: which turn waits for an answer is read as each prompt comes, not
/// kept from when the session opened: a question asked since (the turn
/// ran on while the session was idle) is answered by the next prompt.
#[test]
fn a_question_asked_since_the_session_opened_is_answered() {
    let running = Held { chat: vec![record("chat", 1, &me(), json!({ "text": "ask-me later", "to": [FRED] }))], work: vec![record("work", 1, FRED, start("t1", 1))], ..Held::default() };
    let mut d = Driven::start(running, Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    // its turn asks, with no prompt running to hear it
    d.keep("chat", 2, json!({ "text": "What should I call it?", "turn": "t1" }));
    d.keep("work", 2, json!({ "kind": "turn.asked", "turn": "t1", "asks": &me() }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    d.reply(4, json!({ "text": "(told: Bean)", "turn": "t1" }));
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }), "{answer}");
    assert_eq!(updates[0]["update"]["content"]["text"], "(told: Bean)");
    d.close();
}

/// Goal: a turn's last words are never dropped unsaid: when they cannot be
/// read once the turn ended, the prompt fails, saying where they are.
#[test]
fn last_words_that_cannot_be_read_fail_the_prompt() {
    let mut d = Driven::begun();
    let session = d.new_session();
    d.prompt(2, &session, "hi");
    d.posts(1);
    d.held.lock().unwrap().unread = true;
    d.work(1, start("t1", 1));
    d.work(2, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (_, answer) = d.until_answer(2);
    assert_eq!(answer["error"]["code"], INTERNAL_ERROR, "{answer}");
    let why = answer["error"]["message"].as_str().unwrap_or("");
    assert!(why.contains("last words could not be read") && why.contains(&format!("?chat={MADE}")), "{why}");
    d.close();
}

/// Goal: a turn that asked, took its answer and ended loses no words to
/// the order its records reach the socket: its end before its reply (as a
/// socket dialed again sends a page of `work` before a page of `chat`)
/// still tells the reply, whether the turn ended idle or as an error.
#[test]
fn an_answered_turns_words_are_told_though_its_end_comes_first() {
    let mut d = Driven::start(asking(), Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    // its reply is on the chat, and the socket says its end first
    d.keep("chat", 4, json!({ "text": "(told: Bean)", "turn": "t1" }));
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }), "{answer}");
    assert_eq!(updates[0]["update"]["content"]["text"], "(told: Bean)");
    d.close();

    let mut d = Driven::start(asking(), Bounds::default());
    d.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 } }));
    d.ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/resume", "params": { "sessionId": KEPT, "cwd": "/" } }));
    d.prompt(2, KEPT, "Bean");
    d.posts(1);
    d.work(3, json!({ "kind": "turn.end", "turn": "t1", "outcome": "error", "error": "the runtime failed" }));
    d.reply(4, json!({ "text": "Bean, then.", "turn": "t1" }));
    let (updates, answer) = d.until_answer(2);
    assert_eq!(answer["error"]["message"], "the runtime failed", "{answer}");
    assert_eq!(updates[0]["update"]["content"]["text"], "Bean, then.", "told before the prompt failed");
    d.close();
}
