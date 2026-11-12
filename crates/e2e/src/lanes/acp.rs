//! `fragment acp` (cli/GUIDE.md, "Your agent in an ACP client") against a
//! real computer: the computers section's stub, awake, whose scripted
//! runtime answers the same every time. The CLI is run as an ACP client
//! runs it, over its stdio, as the agent's owner; what it says is checked
//! against the chat's own records. Its protocol's every case is the CLI's
//! own tests (cli/src/acp/); this proves the path a client's prompt takes:
//! the platform's routes, the chat's live socket, the computer's bridge.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::computers::phase;
use super::jobs::records;
use crate::api::Api;
use crate::Suite;

pub(super) const CHECKS: [&str; 4] = [
    "fragment acp serves its owner's agent to an ACP client over stdio: a session is a new chat with the agent in it, a prompt a message to it there, and its turn streams back: its words, its tool call as it is asked for and as it ends, then its end",
    "an ACP client's cancel stops the prompt's turn: a Stop on the chat that names it, the turn ended stopped or never run, and the prompt answered cancelled",
    "a question the agent asks in words ends its prompt, and the client's next prompt is its answer: the turn that asked goes on with it to its end, and no turn starts for the answer",
    "asked, and its computer put to sleep: its owner's answer wakes it, the turn that asked ends as lost, the answer is a turn of its own, and the prompt is answered with that turn's reply, never failed by the lost one",
];

/// The CLI as an ACP client holds it: its stdin, and each message it says.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    heard: Receiver<Value>,
}

impl Client {
    fn start(s: &Suite, api: &Api, home: &std::path::Path, agent: &str) -> Result<Client> {
        let mut child = s.bare_cli().args(["acp", agent]).env("HOME", home).env("FRAGMENT_HOST", &api.base).env_remove("FRAGMENT_OUTPUT").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
        let stdin = child.stdin.take().context("its stdin is piped")?;
        let stdout = child.stdout.take().context("its stdout is piped")?;
        let (said, heard) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let message = serde_json::from_str(&line).unwrap_or_else(|_| json!({ "not JSON-RPC": line }));
                if said.send(message).is_err() {
                    return;
                }
            }
        });
        let mut client = Client { child, stdin: Some(stdin), heard };
        client.send(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1, "clientCapabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } } }))?;
        let (_, hello) = client.answer(0, Duration::from_secs(30))?;
        anyhow::ensure!(hello["result"]["protocolVersion"] == 1, "fragment acp initializes: {hello}");
        Ok(client)
    }

    fn send(&mut self, message: Value) -> Result<()> {
        writeln!(self.stdin.as_mut().context("its stdin is open")?, "{message}")?;
        Ok(())
    }

    /// The session updates said until the answer to `id`, and the answer,
    /// within `limit`.
    fn answer(&mut self, id: i64, limit: Duration) -> Result<(Vec<Value>, Value)> {
        let (until, mut updates) = (Instant::now() + limit, Vec::new());
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let message = self.heard.recv_timeout(left).with_context(|| format!("no answer to request {id} in {limit:?}; it said {updates:?}"))?;
            if message["id"] == id {
                return Ok((updates, message));
            }
            updates.push(message["params"]["update"].clone());
        }
    }

    fn ask(&mut self, id: i64, method: &str, params: Value, limit: Duration) -> Result<(Vec<Value>, Value)> {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        self.answer(id, limit)
    }

    /// The client closes its end: whether the CLI then ends cleanly.
    fn end(mut self) -> bool {
        self.stdin = None;
        self.child.wait().is_ok_and(|status| status.success())
    }
}

/// The words a list of updates says for `kind`, as one text.
fn words(updates: &[Value], kind: &str) -> String {
    updates.iter().filter(|u| u["sessionUpdate"] == kind).filter_map(|u| u["content"]["text"].as_str()).collect()
}

/// `owner` talks to their `agent` (its fragment, its `identity`) through
/// `fragment acp`, on their computer `id`, which is awake. It is put to
/// sleep once and woken by a prompt; starts and ends awake, two chats more
/// in the owner's list.
pub(super) fn checks(s: &mut Suite, api: &Api, owner: &crate::Keys, id: &str, agent: &str, identity: &str, wake: Duration) -> Result<()> {
    let home = s.dir("computers-acp");
    let config = home.join(if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" }).join("fragment");
    std::fs::create_dir_all(&config)?;
    std::fs::write(config.join("config.json"), json!({ "secret_key": owner.secret_hex() }).to_string())?;

    // a session, a prompt, its turn: the script's `tool` says its words, a
    // call, the call's step, then its reply
    let mut first = Client::start(s, api, &home, agent)?;
    let (_, made) = first.ask(1, "session/new", json!({ "cwd": "/work/repo", "mcpServers": [] }), Duration::from_secs(60))?;
    let session = made["result"]["sessionId"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(!session.is_empty(), "a session is made: {made}");
    let listed = api.signed(owner, "GET", "/api/fragments", None)?.body;
    let row = listed["fragments"].as_array().into_iter().flatten().find(|f| f["name"] == session.as_str()).cloned().unwrap_or(Value::Null);
    let asked = "tool please, from acp";
    let (updates, answer) = first.ask(2, "session/prompt", json!({ "sessionId": session, "prompt": [{ "type": "text", "text": asked }] }), wake)?;
    let said = records(api, owner, &session, "chat");
    let posted = said.iter().any(|r| r["principal"] == api.identity(owner).unwrap_or_default().as_str() && r["body"] == json!({ "text": asked, "to": [identity] }));
    let call = updates.iter().find(|u| u["sessionUpdate"] == "tool_call").cloned().unwrap_or(Value::Null);
    let ended = updates.iter().find(|u| u["sessionUpdate"] == "tool_call_update").cloned().unwrap_or(Value::Null);
    let spoke = words(&updates, "agent_message_chunk");
    s.ok(
        CHECKS[0],
        row["kind"] == "chat"
            && row["agents"] == json!([identity])
            && posted
            && answer["result"] == json!({ "stopReason": "end_turn" })
            && spoke.starts_with("Let me look.\n\necho: [")
            && spoke.ends_with(asked)
            && call["status"] == "in_progress"
            && call["kind"] == "fetch"
            && call["title"].as_str().is_some_and(|t| t.starts_with("search: "))
            && ended["status"] == "completed"
            && ended["toolCallId"] == call["toolCallId"]
            // the call before its end (its words before it are a record of
            // another channel, and may reach the socket either side of it),
            // every call ended by its step, and its reply last
            && updates.iter().position(|u| *u == call) < updates.iter().position(|u| *u == ended)
            && !updates.iter().any(|u| u["status"] == "failed")
            && updates.last().is_some_and(|u| u["sessionUpdate"] == "agent_message_chunk"),
        json!({ "session": row, "updates": updates, "answer": answer }),
    );

    // cancelled as it is sent: stopped by the name its message gives it,
    // as it runs (the script's slow tool call runs two seconds on) or
    // before its bridge read the message, when it never runs at all
    let slow = "a slow tool, cancelled from acp";
    first.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": { "sessionId": session, "prompt": [{ "type": "text", "text": slow }] } }))?;
    first.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }))?;
    let (_, answer) = first.answer(3, wake)?;
    let stop = records(api, owner, &session, "chat").into_iter().find(|r| r["body"]["kind"] == "stop").unwrap_or(Value::Null);
    let turn = stop["body"]["turn"].as_str().unwrap_or("").to_string();
    let work = records(api, owner, &session, "work");
    let its: Vec<&Value> = work.iter().filter(|r| r["body"]["turn"] == turn.as_str()).collect();
    let stopped = its.is_empty() || its.iter().any(|r| r["body"]["kind"] == "turn.end" && r["body"]["outcome"] == "stopped");
    let left = first.end();
    s.ok(CHECKS[1], answer["result"] == json!({ "stopReason": "cancelled" }) && !turn.is_empty() && stopped && left, json!({ "answer": answer, "stop": stop, "ended cleanly": left }));

    // a question in words (the script's `ask-me`), in a chat of its own:
    // the prompt ends at it, and the next prompt answers it
    let mut asker = Client::start(s, api, &home, agent)?;
    let (_, made) = asker.ask(1, "session/new", json!({ "cwd": "/work/repo", "mcpServers": [] }), Duration::from_secs(60))?;
    let asking = made["result"]["sessionId"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(!asking.is_empty(), "a session is made: {made}");
    let prompt = |text: &str| json!({ "sessionId": asking, "prompt": [{ "type": "text", "text": text }] });
    let starts = |work: &[Value]| work.iter().filter(|r| r["principal"] == identity && r["body"]["kind"] == "turn.start").count();
    let (question, paused) = asker.ask(2, "session/prompt", prompt("ask-me from acp"), wake)?;
    let (told_back, answered) = asker.ask(3, "session/prompt", prompt("Bean"), wake)?;
    let work = records(api, owner, &asking, "work");
    let spoke = words(&told_back, "agent_message_chunk");
    s.ok(
        CHECKS[2],
        paused["result"] == json!({ "stopReason": "end_turn" })
            && words(&question, "agent_message_chunk") == "What should I call it?"
            && answered["result"] == json!({ "stopReason": "end_turn" })
            && spoke.starts_with("echo: [")
            && spoke.ends_with("ask-me from acp (told: Bean)")
            && starts(&work) == 1
            && work.iter().any(|r| r["body"]["kind"] == "turn.end" && r["body"]["outcome"] == "idle"),
        json!({ "question": question, "paused": paused, "told": told_back, "answered": answered, "work": work.iter().map(|r| &r["body"]).collect::<Vec<_>>() }),
    );

    // asked again, then its computer sleeps (waiting on its person holds no
    // computer awake): the answer wakes it, and its bridge's next life ends
    // the turn that asked as lost and takes the answer as a turn of its own
    // (docs/bridge.md, "An answer in words goes only to its turn's life")
    let (_, paused) = asker.ask(4, "session/prompt", prompt("ask-me once more"), wake)?;
    let slept = api.signed(owner, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})))?;
    let (told_back, answered) = asker.ask(5, "session/prompt", prompt("Fern"), wake * 2)?;
    let said = records(api, owner, &asking, "chat");
    let work = records(api, owner, &asking, "work");
    let fern = said.iter().find(|r| r["body"]["text"] == "Fern").and_then(|r| r["seq"].as_i64()).unwrap_or(0);
    let lost = work.iter().any(|r| r["body"]["kind"] == "turn.end" && r["body"]["outcome"] == "error" && r["body"]["error"] == "lost when the computer restarted");
    let own = work.iter().find(|r| r["body"]["kind"] == "turn.start" && r["body"]["cause"]["seq"].as_i64() == Some(fern)).and_then(|r| r["body"]["turn"].as_str()).unwrap_or("").to_string();
    let own_ended = work.iter().any(|r| r["body"]["kind"] == "turn.end" && r["body"]["turn"] == own.as_str() && r["body"]["outcome"] == "idle");
    let spoke = words(&told_back, "agent_message_chunk");
    s.ok(
        CHECKS[3],
        paused["result"] == json!({ "stopReason": "end_turn" })
            && slept.status == 200
            && slept.body["phase"] == "asleep"
            && answered["result"] == json!({ "stopReason": "end_turn" })
            && spoke.starts_with("echo: [")
            && spoke.contains("] Fern")
            && lost
            && !own.is_empty()
            && own_ended
            && starts(&work) == 3
            && phase(api, owner, id) == "awake"
            && asker.end(),
        json!({ "slept": slept.body["phase"], "told": told_back, "answered": answered, "work": work.iter().map(|r| &r["body"]).collect::<Vec<_>>() }),
    );
    Ok(())
}
