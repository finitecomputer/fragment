//! `fragment acp` as an ACP client runs it: the binary, speaking ACP over
//! its stdio, signed with this machine's key, following the chat over a
//! real WebSocket. The protocol's every case is tested in-process
//! (src/acp/); the e2e's `computers` section runs it against a node and a
//! computer.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fragment_fakes::http::{Request, Response, Server};
use fragment_proto::live::LiveOut;
use fragment_proto::ChannelRecord;
use serde_json::{json, Value};

const FRED: &str = "npub1fred";
const ME: &str = "npub1paul";
const CHAT: &str = "fred-chat-2--a2b3";
const SOON: Duration = Duration::from_secs(20);

/// What the host saw, and what its socket was asked.
#[derive(Default)]
struct Seen {
    /// Each request: method, path, its signature's scheme, its agent header.
    asked: Vec<(String, String, Option<String>, Option<String>)>,
    /// The reply its turn posted, once the socket said it: the chat's own.
    replied: bool,
    /// The message posted on the chat: its body.
    posted: Option<Value>,
    /// The socket's subscribes, as sent.
    subscribed: Vec<Value>,
}

fn record(channel: &str, seq: i64, principal: &str, body: Value) -> ChannelRecord {
    serde_json::from_value(json!({ "channel": channel, "seq": seq, "at": seq * 1000, "principal": principal, "kind": "message", "body": body })).expect("a record")
}

/// The chat's live socket, as the platform serves one: it takes the two
/// subscribes, waits for the message, then says the turn it caused: its
/// start, a reply drafted and posted, and its end.
fn live(stream: std::net::TcpStream, seen: Arc<Mutex<Seen>>) {
    let mut socket = tungstenite::WebSocket::from_raw_socket(stream, tungstenite::protocol::Role::Server, None);
    for _ in 0..2 {
        match socket.read() {
            Ok(tungstenite::Message::Text(frame)) => seen.lock().unwrap().subscribed.push(serde_json::from_str(&frame).expect("a frame is JSON")),
            other => panic!("the socket subscribes first: {other:?}"),
        }
    }
    let until = Instant::now() + SOON;
    while seen.lock().unwrap().posted.is_none() {
        assert!(Instant::now() < until, "the message is posted once the socket is open");
        std::thread::sleep(Duration::from_millis(10));
    }
    let draft = |text: &str| LiveOut::Draft { channel: "chat".into(), principal: FRED.into(), turn: "t1".into(), text: Some(text.into()), at: 1, thinking: None }.encode();
    let frames = [
        LiveOut::record_frame(&record("chat", 1, ME, json!({ "text": "hi there", "to": [FRED] }))),
        LiveOut::record_frame(&record("work", 1, FRED, json!({ "kind": "turn.start", "turn": "t1", "asker": ME, "agent": FRED, "cause": { "fragment": CHAT, "channel": "chat", "seq": 1 }, "life": "0" }))),
        // a frame a later platform adds is passed over
        json!({ "type": "later-kind", "what": "ever" }).to_string(),
        draft("echo: "),
        draft("echo: [paul] hi"),
        LiveOut::record_frame(&record("chat", 2, FRED, json!({ "text": "echo: [paul] hi there", "turn": "t1" }))),
        LiveOut::record_frame(&record("work", 2, FRED, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" }))),
    ];
    seen.lock().unwrap().replied = true;
    for frame in frames {
        socket.send(tungstenite::Message::Text(frame.into())).expect("the socket is open");
    }
    // held open until the client leaves
    while socket.read().is_ok() {}
}

/// A host with one person, their agent fred, and the chat a session makes.
fn host() -> (Server, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = Arc::clone(&seen);
    let server = Server::start(
        0,
        Arc::new(move |r: &Request| {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            let scheme = r.header("authorization").map(|a| a.split(' ').next().unwrap_or("").to_string());
            log.lock().unwrap().asked.push((r.method.clone(), r.path.clone(), scheme, r.header("x-fragment-agent").map(str::to_string)));
            let of_chat = r.path.strip_prefix(&format!("/api/f/{CHAT}/"));
            match (r.method.as_str(), r.path.as_str(), of_chat) {
                ("GET", "/api/identities/me", _) => Response::json(200, &json!({ "id": ME, "kind": "person" })),
                ("GET", "/api/computers", _) => Response::json(200, &json!({ "computers": [{ "agents": [{ "fragment": "fred--k3x9", "identity": FRED, "name": "fred" }] }] })),
                ("GET", "/api/fragments", _) => Response::json(200, &json!({ "fragments": [{ "name": "fred--k3x9", "kind": "agent", "role": "owner", "agents": [] }] })),
                ("POST", "/api/fragments", _) => {
                    Response::json(200, &json!({ "name": CHAT, "npub": "n", "owner": ME, "visibility": "members", "viewToken": "v", "inboxToken": "i", "repo": "", "canonical": "https://x/" }))
                }
                ("GET", _, Some("channels")) => Response::json(200, &json!({ "channels": [{ "name": "chat", "post": "viewer", "seq": 0 }, { "name": "work", "seq": 0 }] })),
                ("POST", _, Some("channels/chat")) => {
                    log.lock().unwrap().posted = Some(body["body"].clone());
                    Response::json(200, &json!({ "record": record("chat", 1, ME, body["body"].clone()), "replayed": false }))
                }
                // the chat's records after a seq: none before the turn, and
                // once its reply is posted, none past it
                ("GET", _, Some(read @ ("channels/chat" | "channels/work"))) => {
                    let after: i64 = r.query.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
                    let next = if log.lock().unwrap().replied { after.max(2) } else { after };
                    Response::json(200, &json!({ "channel": read.trim_start_matches("channels/"), "records": [], "next": next, "more": false }))
                }
                ("GET", path, _) if path == format!("/f/{CHAT}/__live") => {
                    let key = r.header("sec-websocket-key").unwrap_or("");
                    let headers = [("upgrade", "websocket".to_string()), ("connection", "Upgrade".to_string()), ("sec-websocket-accept", tungstenite::handshake::derive_accept_key(key.as_bytes()))];
                    let seen = Arc::clone(&log);
                    Response {
                        status: 101,
                        headers: headers.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                        body: Vec::new(),
                        unanswered: false,
                        upgrade: Some(Box::new(move |stream| live(stream, seen))),
                    }
                }
                _ => Response::json(404, &json!({ "error": "not_found", "message": "nothing here" })),
            }
        }),
    )
    .expect("start the fake host");
    (server, seen)
}

/// A HOME of its own: with `key`, a config holding that secret, where the
/// CLI reads it on this system (`dirs::config_dir`: Linux's and macOS's).
fn home(tag: &str, key: Option<&str>) -> PathBuf {
    let home = std::env::temp_dir().join(format!("fragment-acp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    for dir in [".config/fragment", "Library/Application Support/fragment"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
        if let Some(key) = key {
            std::fs::write(home.join(dir).join("config.json"), json!({ "secret_key": key }).to_string()).unwrap();
        }
    }
    home
}

fn fragment(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fragment"));
    c.env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("FRAGMENT_HOST")
        .env_remove("FRAGMENT_AS_AGENT")
        .env_remove("FRAGMENT_FOR")
        .env_remove("FRAGMENT_API")
        // even asked for --json envelopes, stdout is the protocol's alone
        .env("FRAGMENT_OUTPUT", "json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

/// Each stdout line of the CLI, as the JSON-RPC message it is.
fn messages(stdout: std::process::ChildStdout) -> Receiver<Value> {
    let (said, heard) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let message = serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout carries only JSON-RPC: {line:?}: {e}"));
            if said.send(message).is_err() {
                return;
            }
        }
    });
    heard
}

/// Goal: a person's ACP client runs `fragment acp <agent>`: a session is a
/// chat made with the agent, a prompt a message there, and the turn it
/// causes streams back over the chat's live socket, every request signed
/// with this machine's key. Method: the binary over its stdio and a real
/// WebSocket, a whole session; it ends when the client closes its stdin.
#[test]
fn a_person_talks_to_their_agent_over_stdio() {
    let (server, seen) = host();
    let home = home("key", Some(&"07".repeat(32)));
    let mut child = fragment(&home).args(["acp", "fred", "--host", &server.url]).spawn().expect("run the CLI");
    let mut stdin = child.stdin.take().unwrap();
    let heard = messages(child.stdout.take().unwrap());
    let mut ask = |request: Value| -> (Vec<Value>, Value) {
        writeln!(stdin, "{request}").unwrap();
        let mut updates = Vec::new();
        loop {
            let message = heard.recv_timeout(SOON).unwrap_or_else(|e| panic!("an answer to {request}: {e}"));
            if message["id"] == request["id"] {
                return (updates, message);
            }
            updates.push(message);
        }
    };
    let (_, hello) = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": 2, "clientCapabilities": {}, "clientInfo": { "name": "t", "version": "1" } } }));
    assert_eq!((&hello["result"]["protocolVersion"], &hello["result"]["agentInfo"]["title"]), (&json!(1), &json!("fred")), "{hello}");
    let (_, made) = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": { "cwd": "/work/repo", "mcpServers": [] } }));
    assert_eq!(made["result"]["sessionId"], CHAT, "{made}");
    let (updates, answer) = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": { "sessionId": CHAT, "prompt": [{ "type": "text", "text": "hi there" }] } }));
    assert_eq!(answer["result"], json!({ "stopReason": "end_turn" }), "{answer}");
    let said: Vec<&str> = updates
        .iter()
        .map(|u| {
            assert_eq!((&u["method"], &u["params"]["sessionId"], &u["params"]["update"]["sessionUpdate"]), (&json!("session/update"), &json!(CHAT), &json!("agent_message_chunk")), "{u}");
            u["params"]["update"]["content"]["text"].as_str().unwrap_or("")
        })
        .collect();
    assert_eq!(said, ["echo: ", "[paul] hi", " there"], "the reply as it was written");

    drop(stdin);
    let out = child.wait_with_output().unwrap();
    std::fs::remove_dir_all(&home).ok();
    assert!(out.status.success(), "it ends when the client closes its stdin: {}", String::from_utf8_lossy(&out.stderr));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.posted, Some(json!({ "text": "hi there", "to": [FRED] })));
    assert_eq!(seen.subscribed, [json!({ "type": "subscribe", "channel": "work", "after": 0 }), json!({ "type": "subscribe", "channel": "chat", "after": 0 })]);
    assert!(seen.asked.iter().all(|r| r.2.as_deref() == Some("Nostr") && r.3.is_none()), "each request signed with the key, the socket's too: {:?}", seen.asked);
    let order: Vec<(&str, &str)> = seen.asked.iter().filter(|r| r.1.contains(CHAT)).map(|r| (r.0.as_str(), r.1.rsplit('/').next().unwrap_or(""))).collect();
    assert_eq!(
        order[..5],
        [("GET", "channels"), ("GET", "work"), ("GET", "chat"), ("GET", "__live"), ("POST", "chat")],
        "the chat's newest records read, then followed from there, before the message is posted: {order:?}"
    );
}

/// A server that cannot start says why on stderr and leaves stdout, the
/// protocol's, empty: a person with no key.
#[test]
fn a_server_that_cannot_start_says_why_on_stderr() {
    let home = home("refused", None);
    let out = fragment(&home).args(["acp", "fred"]).stdin(Stdio::null()).output().unwrap();
    std::fs::remove_dir_all(&home).ok();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains("fragment login"), "{}", String::from_utf8_lossy(&out.stderr));
}
