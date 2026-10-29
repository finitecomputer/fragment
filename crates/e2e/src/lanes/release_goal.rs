//! One release-goal template against workerd, with only its vendor faked.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use fragment_fakes::http::{Handler, Response, Server};
use serde_json::json;

use super::{app::ship, jobs};
use crate::api::Api;
use crate::Suite;

pub fn release_goal(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("release-goal", &[crate::Need::Fakes, crate::Need::Node, crate::Need::Levers]) { return Ok(()) }
    // Lose the first answer AFTER the vendor commits. A retry must observe
    // that receipt, not publish a second note. Different bodies are refused.
    let effects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
    let attempts = Arc::new(Mutex::new(Vec::<String>::new()));
    let (stored, seen) = (Arc::clone(&effects), Arc::clone(&attempts));
    let handler: Handler = Arc::new(move |req| {
        if req.path != "/release" || req.method != "POST" { return Response::json(404, &json!({})); }
        let key = req.header("idempotency-key").unwrap_or("").to_string();
        if key.is_empty() { return Response::json(400, &json!({})); }
        let mut effects = stored.lock().unwrap();
        let mut attempts = seen.lock().unwrap();
        attempts.push(key.clone());
        if let Some(old) = effects.get(&key) {
            if old != &req.body { return Response::json(409, &json!({"error":"key payload changed"})); }
        } else { effects.insert(key.clone(), req.body.clone()); }
        if attempts.len() == 1 { return Response::json(503, &json!({"error":"answer lost after commit"})); }
        Response::json(200, &json!({"id":"sandbox-release", "key":key}))
    });
    let vendor = Server::start(0, handler)?;
    let owner = api.person()?;
    let viewer = api.person()?;
    let name = s.named(api, &owner, "release-goal")?;
    let made = s.create(api, &owner, &name)?;
    ship(s, &made, include_bytes!("../../../../templates/releasegoal/app.mjs"), include_bytes!("../../../../templates/releasegoal/fragment.json"));
    api.share(&name, viewer.pubkey_hex(), "viewer")?;
    let key = format!("{name}:sandbox-release:v1");
    let input = json!({"text":"A durable sandbox release note", "vendor":format!("{}/release", vendor.url), "key":key});
    let r = api.op(&owner, &name, "draft", "draft", input.clone())?;
    s.ok("draft checkpoints the artifact and full goal identity", r.status == 200 && r.body["result"]["status"] == "scheduled" && r.body["result"]["effect"]["key"] == key, &r);
    let r = api.op(&owner, &name, "draft", "draft-again", input.clone())?;
    s.ok("a fresh draft call reconciles the same immutable goal", r.status == 200, &r);
    let mut changed = input.clone();
    changed["vendor"] = json!("https://example.com/release");
    let r = api.op(&owner, &name, "draft", "real-vendor", changed)?;
    s.ok("a real vendor is refused", r.status != 200, &r);
    let r = api.op(&owner, &name, "approve", "early", json!({"answer":"Publish", "source":"chat/1"}))?;
    s.ok("an approval before review is invalid", r.status != 200, &r);
    let r = api.op(&viewer, &name, "approve", "viewer", json!({"answer":"Publish", "source":"chat/1"}))?;
    s.ok("a viewer cannot approve", r.status == 403, &r);
    // Process death before the effect, then while awaiting the owner, then
    // after their answer. The state lives outside any Hermes disk save.
    s.stop()?;
    let api = s.start(false)?;
    let r = api.op(&owner, &name, "review", "review", json!({}))?;
    s.ok("review after restart checkpoints the owner wait", r.body["result"]["status"] == "waiting_owner", &r);
    s.stop()?;
    let api = s.start(false)?;
    let r = api.op(&owner, &name, "state", "read", json!({}))?;
    s.ok("the owner wait survives process death without an effect", r.body["result"]["status"] == "waiting_owner" && effects.lock().unwrap().is_empty(), &r);
    let premature = api.op(&owner, &name, "deliver", "unapproved", json!({}))?;
    let held = jobs::settle(&api, &owner, &name, jobs::started(&premature), &["held", "succeeded"], Duration::from_secs(40));
    s.ok("unapproved delivery is held and reaches no vendor", held["status"] == "held" && effects.lock().unwrap().is_empty(), &held);
    let answer = json!({"answer":"Publish", "source":"chat/turn/prompt/owner-answer"});
    let r = api.op(&owner, &name, "approve", "answer", answer.clone())?;
    s.ok("the late owner answer is checkpointed", r.body["result"]["status"] == "approved", &r);
    s.stop()?;
    let api = s.start(false)?;
    let r = api.op(&owner, &name, "approve", "answer-again", answer)?;
    s.ok("a repeated owner answer changes nothing", r.body["result"]["status"] == "approved", &r);
    let r = api.op(&owner, &name, "approve", "contradict", json!({"answer":"Keep private", "source":"chat/other"}))?;
    s.ok("a conflicting answer is refused", r.status != 200, &r);
    for id in ["release-delivery-v1", "different-run-same-goal"] {
        let r = api.op(&owner, &name, "deliver", id, json!({}))?;
        let run = jobs::settle(&api, &owner, &name, jobs::started(&r), &["succeeded", "held"], Duration::from_secs(60));
        s.ok("delivery settles with the vendor receipt", run["status"] == "succeeded" && run["output"]["effect"]["receipt"]["key"] == key, &run);
    }
    let r = api.op(&owner, &name, "state", "read", json!({}))?;
    let tried = attempts.lock().unwrap().clone();
    s.ok("goal completes after the lost vendor answer with one effect", r.body["result"]["status"] == "done" && effects.lock().unwrap().len() == 1 && tried.len() == 2 && tried.iter().all(|k| k == &key), &r);
    let r = api.op(&owner, &name, "review", "duplicate-followup", json!({}))?;
    s.ok("a duplicate follow-up cannot reopen a completed goal", r.body["result"]["status"] == "done", &r);
    Ok(())
}

/// Real Hermes on the real local cell, scripted only at the model boundary.
/// The direct lane above covers invalid/replay and lost vendor responses;
/// this joins the routine, original card, fresh late-answer and tool evidence.
pub(super) fn hermes(s: &mut Suite, api: &Api, owner: &fragment_nip98::Keys, computer: &str, agent: &str, chat: &str, identity: &str) -> Result<()> {
    use super::computers::{cron_now, turn_of, work_of};
    let effects = Arc::new(Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
    let saved = Arc::clone(&effects);
    let handler: Handler = Arc::new(move |req| {
        let key = req.header("idempotency-key").unwrap_or("").to_string();
        if req.path != "/release" || key.is_empty() { return Response::json(400, &json!({})); }
        let mut log = saved.lock().unwrap();
        if log.get(&key).is_some_and(|body| *body != req.body) { return Response::json(409, &json!({})); }
        log.insert(key.clone(), req.body.clone());
        Response::json(200, &json!({"id":"sandbox-release", "key":key}))
    });
    let vendor = Server::start(0, handler)?;
    let name = s.named(api, owner, "hermes-release")?;
    let made = s.create(api, owner, &name)?;
    ship(s, &made, include_bytes!("../../../../templates/releasegoal/app.mjs"), include_bytes!("../../../../templates/releasegoal/fragment.json"));
    let key = format!("{name}:sandbox:v1");
    api.op(owner, &name, "draft", "draft", json!({"text":"Tomorrow's sandbox release", "vendor":format!("{}/release",vendor.url), "key":key}))?;
    let wait = Duration::from_secs(180);
    let state = || api.op(owner, &name, "state", "read", json!({})).map(|r| r.body["result"].clone()).unwrap_or_default();
    let path = format!("/api/computers/{computer}");
    // A hard restart before the effect. The routine is the only follow-up.
    super::computers::lever(api, computer, "kill")?;
    let routine = json!({"cron":"0 0 1 1 *", "channel":"tasks", "record":{"kind":"routine", "text":format!("sandbox-review sandbox-goal:{name}"), "chat":chat}});
    let added = api.signed(owner, "POST", &format!("/api/f/{agent}/schedules"), Some(&routine))?;
    let schedule = added.body["schedule"]["id"].as_i64().unwrap_or(0);
    anyhow::ensure!(schedule > 0, "the routine: {added}");
    let fired = cron_now(api, agent)?;
    s.ok("the durable goal has exactly one scheduled follow-up", fired.body["records"] == 1, &fired);
    let card = || jobs::records(api, owner, chat, "work").into_iter().find(|r| r["body"]["kind"] == "turn.prompt" && r["body"].to_string().contains(&name));
    let asked = s.eventually(wait, || card().is_some());
    s.ok("the scheduled fresh Hermes turn reads the checkpoint and asks once", asked && state()["status"] == "waiting_owner", state());
    let card = card().ok_or_else(|| anyhow::anyhow!("no goal approval card"))?["body"].clone();
    api.signed(owner, "DELETE", &format!("/api/f/{agent}/schedules/{schedule}"), None)?;
    // Sleep while the person is away, then restart from the saved disk.
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let slept = s.eventually(wait, || api.signed(owner, "POST", &format!("{path}/sleep"), Some(&json!({}))).is_ok_and(|r| r.body["phase"] == "asleep"));
    s.ok("the goal waits for its owner with the computer asleep", slept && effects.lock().unwrap().is_empty(), state());
    api.signed(owner, "POST", &format!("{path}/restart"), Some(&json!({})))?;
    let answer = json!({"kind":"prompt_response", "agent":identity, "turn":card["turn"], "prompt":card["prompt"], "option":card["options"][0]["id"]});
    let answered = api.signed(owner, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({"id":"goal-owner-answer", "body":answer})))?;
    let fresh = turn_of(agent, chat, "chat", answered.body["record"]["seq"].as_i64().unwrap_or(0));
    s.ok("the answer starts a fresh turn", json!(fresh) != card["turn"], &answered);
    let approved = s.eventually(wait, || state()["status"] == "approved");
    s.ok("the late answer checkpoints approval before any external effect", approved && effects.lock().unwrap().is_empty(), state());
    // Kill the terminal after it checkpointed the answer, before its fetch.
    super::computers::lever(api, computer, "kill")?;
    let text = format!("run: fragment call {name} state --json && fragment call {name} deliver --id release-delivery-v1 --json");
    let continued = api.signed(owner, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({"id":"goal-continue", "body":{"text":text}})))?;
    let turn = turn_of(agent, chat, "chat", continued.body["record"]["seq"].as_i64().unwrap_or(0));
    let finished = s.eventually(wait, || state()["status"] == "done");
    s.ok("fresh continuation after the cut publishes the approved goal exactly once", finished && effects.lock().unwrap().len() == 1 && state()["effect"]["receipt"]["key"] == key, state());
    let evidenced = s.eventually(wait, || work_of(&jobs::records(api, owner, chat, "work"), &turn).iter().any(|r| r["body"]["kind"] == "turn.step" && r["body"]["tool"] == "terminal" && r["body"]["status"] == "completed" && r["body"]["result_ref"].is_string()));
    s.ok("the effect's CLI call has structured outcome and source evidence", evidenced, json!(work_of(&jobs::records(api, owner, chat, "work"), &turn)));
    api.signed(owner, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({"id":"goal-answer-replayed", "body":answer})))?;
    let r = api.op(owner, &name, "deliver", "goal-replay", json!({}))?;
    let run = jobs::settle(api, owner, &name, jobs::started(&r), &["succeeded", "held"], wait);
    s.ok("a duplicate answer and fresh job repeat no effect", run["status"] == "succeeded" && effects.lock().unwrap().len() == 1, &run);
    let cards = jobs::records(api, owner, chat, "work").into_iter().filter(|r| r["body"]["kind"] == "turn.prompt" && r["body"].to_string().contains(&name)).count();
    s.ok("the goal asked its owner exactly once across the restarts", cards == 1, cards);
    Ok(())
}
