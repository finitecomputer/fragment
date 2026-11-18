//! The Finite Agent spike (never merged; spike/finite-agent/README.md):
//! an agent whose loop is pi-durable in its own fragment's code, answering
//! in an ordinary chat with the records the bridge writes, its model the
//! platform's model route, its two tools (`run`, `file`) run on its
//! computer, which wakes only for a tool. Run by name: locally on the
//! fakes, `cargo xtask e2e --only finite-agent`; on a preview's real model,
//! `--hosted --config <file> --branch <b> --only finite-agent`.
//!
//! - valid: a turn with no tool is answered as the agent, its turn started
//!   and ended on `work`, its draft live on a page, its computer asleep;
//! - a tool turn wakes the sleeping computer, and its step is on `work`;
//! - a tool turn on the awake computer: the tool call's added hop;
//! - replay: the same record run again is one turn, one reply;
//! - measures: post to first draft, post to reply, the share of turns
//!   that never touched the computer, each tool's time.

use std::time::{Duration, Instant};

use anyhow::Result;
use fragment_fakes::workers_ai::{Pace, Reply};
use serde_json::{json, Value};

use super::computers::{agent_replies, phase, turn_of, work_of, CHAT_JSON};
use super::jobs::{records, settle, started};
use super::ledger::entries;
use crate::api::{Api, Socket};
use crate::{Need, Suite};

const APP: &[u8] = include_bytes!("../../fixtures/finite-agent/app.mjs");
const MANIFEST: &[u8] = include_bytes!("../../fixtures/finite-agent/fragment.json");
const BUNDLE: &[u8] = include_bytes!("../../fixtures/finite-agent/applib/agent.mjs");

/// Paid calls a hosted run lends its owner: under the spike's budget of 100.
const PAID_CALLS: u64 = 90;
const TURN: Duration = Duration::from_secs(120);

/// Everyday asks for the hosted measure: about half need the computer.
const ASKS: [(&str, bool); 12] = [
    ("What is the capital of Portugal? One word.", false),
    ("Give me a synonym for 'quick'. One word.", false),
    ("Use the run tool to run `uname -s` and tell me what it printed.", true),
    ("What is 17 times 23? Just the number.", false),
    ("Use the file tool to write the text 'hello pi' to notes/hello.txt, then read it back and tell me what it says.", true),
    ("Name one primary color. One word.", false),
    ("Use the run tool to run `ls /data/work` and tell me how many entries there are.", true),
    ("Say hello in French. One word.", false),
    ("Use the run tool to run `date -u +%Y` and tell me the year it printed.", true),
    ("Is a tomato a fruit? Yes or no.", false),
    ("Use the run tool to run `echo $((6*7))` and tell me the number.", true),
    ("What's the opposite of 'cold'? One word.", false),
];

struct Measured {
    ask: String,
    tool_asked: bool,
    computer_touched: bool,
    first_draft_ms: Option<u128>,
    reply_ms: Option<u128>,
    timing: Value,
}

pub fn finite_agent(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name("finite-agent", &[Need::Computers, Need::Models, Need::Levers], "the Finite Agent spike") {
        return Ok(());
    }
    let fakes = !s.hosted();
    let owner = api.person_paying(PAID_CALLS)?;
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let computer = r.body["computer"].as_str().unwrap_or("").to_string();
    s.ok("its owner's computer, asleep", r.status == 200 && r.body["phase"] == "asleep", &r);

    // the agent: a fragment of kind agent running the spike's own code
    let agent_name = s.named(api, &owner, "pi")?;
    let agent = s.create(api, &owner, &agent_name)?;
    s.commit(&agent, &[("fragment.json", Some(MANIFEST)), ("app.mjs", Some(APP)), ("applib/agent.mjs", Some(BUNDLE))]);
    s.deploy(&agent);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{agent_name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the agent made members-only: {r}");
    // its identity is registered at its first assignment to a computer
    // (runs_on.rs `assign`): assigned once, then not, so the computer's
    // bridge never runs its turns; the agent uses the computer, which runs
    // nothing of it (phase 1b, C)
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{computer}/agents/{agent_name}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    let r2 = api.signed(&owner, "DELETE", &format!("/api/computers/{computer}/agents/{agent_name}"), None)?;
    s.ok("the agent has its identity, and no computer runs it", r.status == 200 && !identity.is_empty() && r2.status == 200 && r2.body["agents"].as_array().is_some_and(Vec::is_empty), json!({ "assigned": r.body, "unassigned": r2.body }));

    let chat_name = s.named(api, &owner, "pi-chat")?;
    let chat = s.create(api, &owner, &chat_name)?;
    s.commit(&chat, &[("fragment.json", Some(CHAT_JSON))]);
    s.deploy(&chat);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    s.ok("the agent joins the chat", r.status == 200, &r);

    // it follows the chat, as itself
    let r = api.op(&owner, &agent_name, "drive", "follow-1", json!({ "follow": chat_name }))?;
    let run = settle(api, &owner, &agent_name, started(&r), &["succeeded", "held"], TURN);
    let subs = api.signed(&owner, "GET", &format!("/api/f/{chat_name}/subscriptions"), None)?;
    let follows = subs.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["channel"] == "chat" && x["url"] == format!("fragment:{agent_name}").as_str()));
    s.ok("the agent follows the chat, a subscription naming its fragment (a live call, as the agent)", run["status"] == "succeeded" && follows, json!({ "run": run, "subscriptions": subs.body }));
    s.ok("nothing woke the computer", phase(api, &owner, &computer) == "asleep", phase(api, &owner, &computer));
    // a call's run goes through a Workflow (a followed record's is called at once: jobs.rs `live_direct`)
    println!("      (the follow, a call through a Workflow: created to finished {:?} ms)", run["finishedAt"].as_i64().zip(run["createdAt"].as_i64()).map(|(f, c)| f - c));

    let mut page = Socket::open(api, &chat_name, "__live", Some(&owner), None)?;
    page.until("hello", 5)?;
    page.send(&json!({ "type": "subscribe", "channel": "chat", "after": 0 }))?;
    page.until("subscribed", 20)?;
    let mut n = 0u32;
    let mut say = |s: &mut Suite, page: &mut Socket, text: &str| -> Result<(String, Option<u128>, Option<u128>, Value, Value)> {
        n += 1;
        let t0 = Instant::now();
        let r = api.signed(&owner, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": format!("m{n}"), "body": { "text": text } })))?;
        anyhow::ensure!(r.status == 200, "a message to the chat: {r}");
        let seq = r.body["record"]["seq"].as_i64().unwrap_or(0);
        let turn = turn_of(&agent_name, &chat_name, "chat", seq);
        let draft = page.until_where("draft", 400, |d| d["turn"] == turn.as_str() && d["text"].as_str().is_some_and(|t| !t.is_empty()));
        let first_draft = draft.as_ref().ok().map(|_| t0.elapsed().as_millis());
        let replied = s.eventually(TURN, || agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).iter().any(|r| r["body"]["turn"] == turn.as_str()));
        let reply_ms = replied.then(|| t0.elapsed().as_millis());
        s.eventually(Duration::from_secs(20), || work_of(&records(api, &owner, &chat_name, "work"), &turn).iter().any(|r| r["body"]["kind"] == "turn.end"));
        let end = work_of(&records(api, &owner, &chat_name, "work"), &turn).into_iter().find(|r| r["body"]["kind"] == "turn.end").unwrap_or(Value::Null);
        // where the time went, by the platform's clocks: the post's record,
        // the run its delivery started (the agent's), the agent's own
        // receipt, the reply's record
        let post_at = r.body["record"]["at"].as_i64().unwrap_or(0);
        let reply_at = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).iter().find(|r| r["body"]["turn"] == turn.as_str()).and_then(|r| r["at"].as_i64());
        let runs = api.signed(&owner, "GET", &format!("/api/f/{agent_name}/runs?op=drive"), None).map(|r| r.body["runs"].clone()).unwrap_or(Value::Null);
        let created = runs.as_array().into_iter().flatten().filter(|r| r["via"] == "followed").filter_map(|r| r["createdAt"].as_i64()).filter(|at| *at >= post_at).min();
        let received = end["body"]["timing"]["received_at"].as_i64();
        let split = json!({
            "delivery_ms": created.map(|c| c - post_at),
            "run_start_ms": created.zip(received).map(|(c, r)| r - c),
            "agent_ms": end["body"]["timing"]["answered_ms"],
            "post_to_reply_record_ms": reply_at.map(|a| a - post_at),
        });
        Ok((turn, first_draft, reply_ms, end, split))
    };

    // ---- valid: a turn with no tool
    if fakes {
        // paced, as a model streams: its words come over a second or two,
        // so a page sees the answer grow in the draft before the reply
        s.ai.pace_next(&[Pace { every_ms: 200, thinking: 0, hold_ms: 0 }]);
        s.ai.reply_next(&[Reply::Text("Hello from pi, in a fragment.".into())]);
    }
    let (turn, first_draft, reply_ms, end, split) = say(s, &mut page, "hi pi")?;
    let replies = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity);
    let work = work_of(&records(api, &owner, &chat_name, "work"), &turn);
    let kinds: Vec<&str> = work.iter().filter_map(|r| r["body"]["kind"].as_str()).collect();
    s.ok(
        "the agent answers as itself, naming its turn, with the bridge's records (turn.start, then turn.end idle)",
        replies.iter().any(|r| r["body"]["turn"] == turn.as_str() && (!fakes || r["body"]["text"] == "Hello from pi, in a fragment."))
            && kinds.first() == Some(&"turn.start")
            && kinds.last() == Some(&"turn.end")
            && end["body"]["outcome"] == "idle"
            && work.iter().all(|r| r["principal"] == identity.as_str()),
        json!({ "replies": replies, "work": work }),
    );
    s.ok("a page sees its draft live, then the reply", first_draft.is_some() && reply_ms.is_some(), format!("draft {first_draft:?} ms, reply {reply_ms:?} ms"));
    s.ok("a turn with no tool leaves the computer asleep", phase(api, &owner, &computer) == "asleep", phase(api, &owner, &computer));
    println!("      (no tool, computer asleep: first draft {first_draft:?} ms, reply {reply_ms:?} ms after the post; split {split}; {})", end["body"]["timing"]);

    // ---- a tool turn wakes the sleeping computer
    if fakes {
        s.ai.reply_next(&[Reply::Tools(vec![("run".into(), json!({ "command": "echo pi-was-here" }))]), Reply::Text("It printed pi-was-here.".into())]);
    }
    let calls = if fakes { s.ai.calls().len() } else { 0 };
    let (turn, first_draft, reply_ms, end, split) = say(s, &mut page, if fakes { "run a command please" } else { "Use the run tool to run `echo pi-was-here` and tell me what it printed." })?;
    let work = work_of(&records(api, &owner, &chat_name, "work"), &turn);
    let step = work.iter().find(|r| r["body"]["kind"] == "turn.step").cloned().unwrap_or(Value::Null);
    let result_seen = !fakes || s.ai.calls()[calls..].iter().any(|c| c.body.to_string().contains("pi-was-here"));
    s.ok(
        "a tool call runs on the computer, woken for it: its call and its step on work, its output the model's next input",
        step["body"]["tool"] == "run" && step["body"]["ok"] == true && work.iter().any(|r| r["body"]["kind"] == "turn.call") && result_seen && phase(api, &owner, &computer) == "awake",
        json!({ "work": work, "phase": phase(api, &owner, &computer) }),
    );
    println!("      (a tool, computer asleep: first draft {first_draft:?} ms, reply {reply_ms:?} ms; split {split}; {})", end["body"]["timing"]);

    // ---- a tool turn on the awake computer: the hop
    if fakes {
        s.ai.reply_next(&[
            Reply::Tools(vec![("file".into(), json!({ "path": "notes/hello.txt", "text": "hello pi" }))]),
            Reply::Tools(vec![("file".into(), json!({ "path": "notes/hello.txt" }))]),
            Reply::Text("It says hello pi.".into()),
        ]);
    }
    let calls = if fakes { s.ai.calls().len() } else { 0 };
    let (turn, _, reply_ms, end, split) = say(s, &mut page, if fakes { "write and read a file" } else { ASKS[4].0 })?;
    let steps: Vec<Value> = work_of(&records(api, &owner, &chat_name, "work"), &turn).into_iter().filter(|r| r["body"]["kind"] == "turn.step").collect();
    let read_back = !fakes || s.ai.calls()[calls..].iter().any(|c| c.body.to_string().contains("hello pi"));
    s.ok("a file is written and read back on the computer, each a step", steps.len() >= 2 && steps.iter().all(|r| r["body"]["ok"] == true) && read_back, json!(steps));
    println!("      (tools, computer awake: reply {reply_ms:?} ms; split {split}; {})", end["body"]["timing"]);

    // ---- replay: the same record run again is one turn
    let chat_records = records(api, &owner, &chat_name, "chat");
    let first = chat_records.iter().find(|r| r["body"]["text"] == "hi pi").cloned().unwrap_or(Value::Null);
    let r = api.op(&owner, &agent_name, "drive", "again-1", json!({ "fragment": chat_name, "channel": "chat", "record": first }))?;
    let run = settle(api, &owner, &agent_name, started(&r), &["succeeded", "held"], TURN);
    let turn = turn_of(&agent_name, &chat_name, "chat", first["seq"].as_i64().unwrap_or(0));
    let replies: Vec<Value> = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).into_iter().filter(|r| r["body"]["turn"] == turn.as_str()).collect();
    let starts = work_of(&records(api, &owner, &chat_name, "work"), &turn).iter().filter(|r| r["body"]["kind"] == "turn.start").count();
    s.ok("the same record run again is the same turn: one start, one reply, no model call", run["status"] == "succeeded" && replies.len() == 1 && starts == 1, json!({ "run": run, "replies": replies }));

    // ---- the measure: everyday asks, each its own turn (hosted: the real model)
    let mut measured: Vec<Measured> = vec![];
    if !fakes {
        for (ask, tool) in ASKS {
            let before = phase(api, &owner, &computer);
            let (_, first_draft, reply_ms, end, split) = say(s, &mut page, ask)?;
            let touched = end["body"]["timing"]["tools"].as_array().is_some_and(|t| !t.is_empty());
            println!("      [{}] tool asked {tool}, used {touched}, computer was {before}: first draft {first_draft:?} ms, reply {reply_ms:?} ms; split {split}; {}", ask, end["body"]["timing"]);
            measured.push(Measured { ask: ask.into(), tool_asked: tool, computer_touched: touched, first_draft_ms: first_draft, reply_ms, timing: json!({ "split": split, "agent": end["body"]["timing"], "computer_before": before }) });
        }
        let untouched = measured.iter().filter(|m| !m.computer_touched).count();
        // the model calls' charges on the owner's ledger, every turn of the section's
        let owner_id = api.identity(&owner)?;
        let aig = entries(api, &owner_id, "aig:");
        let charged: i64 = aig.iter().filter_map(|e| e["entry"]["charge"].as_i64()).sum();
        println!("      model calls: {} charged {charged} micro-dollars in all, over {} turns", aig.len(), measured.len() + 4);
        println!("      {untouched} of {} turns never touched the computer", measured.len());
        let out = json!(measured.iter().map(|m| json!({ "ask": m.ask, "tool_asked": m.tool_asked, "computer_touched": m.computer_touched, "first_draft_ms": m.first_draft_ms, "reply_ms": m.reply_ms, "timing": m.timing })).collect::<Vec<_>>());
        let _ = std::fs::write(s.scratch.join("finite-agent-measures.json"), serde_json::to_vec_pretty(&out)?);
        s.ok("every ask answered", measured.iter().all(|m| m.reply_ms.is_some()), out);
    } else {
        s.skip("every everyday ask answered (the measure)", "it needs a real model: run it hosted");
    }
    page.close();
    // its computer asleep again, as it was: nothing left running at the end
    let r = api.patient(Duration::from_secs(900)).signed(&owner, "POST", &format!("/api/computers/{computer}/sleep"), Some(&json!({})))?;
    s.ok("its computer goes back to sleep", r.status == 200 && r.body["phase"] == "asleep", &r);
    Ok(())
}
