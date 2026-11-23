//! A SimpleX door on the platform (cell/door.mjs, docs/computers.md "F"):
//! its object starts its container from the door's image; the runner
//! restores (none: a new door), saves to the bucket and names each save to
//! its object; a crash starts it again from its newest save, in step; a
//! save that will not restore is passed over for the one before it; its
//! saves are kept to the newest three; and a planned stop saves last and
//! is not started again until asked. Then its connector (templates/simplex)
//! on a door of its own: its connect starts it, and its owner's message
//! goes from the door to the connector's inbox and its trigger.
//!
//! Locally on workerd and Docker, its containers reaching no SimpleX server
//! (the e2e's reach none), so no link here: SimpleX itself is the lab's
//! (images/bridge/tests/door.rs). Hosted, on a deployment whose config
//! names `doors`. Run by name: a local stack builds the door's image for
//! it alone (devstack::Fleet::doors).
//!
//! `connector` (in the computers lane, whose stub agent it binds, on a
//! stack that runs doors: `--only computers,simplex-door`): the SimpleX connector
//! (templates/simplex) end to end on a door. Its owner's `connect` starts
//! its life's door; no other fragment's job reaches a
//! door; bound to the agent, its owner's message as the door's runner
//! delivers it reaches the connector's inbox, is published there as its
//! owner's to the agent, whose computer, asleep, wakes and answers; the
//! answer goes to the door's runner (which has no SimpleX link here, and
//! says so); a delivery without the inbox's secret is refused, and one once
//! its owner unbound it reaches no one.

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use crate::api::{Api, Call};
use crate::{Keys, Need, Suite};

pub const NAME: &str = "simplex-door";

fn lever(api: &Api, door: &str, op: &str, more: Value) -> Result<Value> {
    let mut body = json!({ "door": door, "op": op });
    body.as_object_mut().expect("an object").extend(more.as_object().cloned().unwrap_or_default());
    let r = api.unsigned("POST", "/api/test/door", Some(&body))?;
    ensure!(r.status == 200, "the door's {op}: {r}");
    Ok(r.body)
}

/// Its state, once `done` holds (within `within`).
fn until(api: &Api, door: &str, within: Duration, done: impl Fn(&Value) -> bool) -> Result<Value> {
    let t0 = Instant::now();
    // bounded by `within`
    loop {
        let s = lever(api, door, "state", json!({}))?;
        if done(&s) {
            return Ok(s);
        }
        ensure!(t0.elapsed() < within, "the door is not there within {within:?}: {s}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Its runner up, at start `generation` or a later one: an exit Docker
/// restarts in place (a code of the runner's own) keeps its generation.
fn ready_from(generation: u64) -> impl Fn(&Value) -> bool {
    move |s| s["runner"]["ready"] == true && s["runner"]["generation"].as_u64().is_some_and(|g| g >= generation) && s["runner"]["generation"] == s["door"]["generation"]
}

fn generation(s: &Value) -> u64 {
    s["door"]["generation"].as_u64().unwrap_or(0)
}

/// The newest save's start and seq.
fn newest(s: &Value) -> (u64, u64) {
    s["saves"].as_array().into_iter().flatten().filter(|x| x["unreadable"] == 0).map(|x| (x["generation"].as_u64().unwrap_or(0), x["seq"].as_u64().unwrap_or(0))).max().unwrap_or((0, 0))
}

/// A run's door, named as a fragment is (`<label>--<suffix>`), from the
/// run's label for it (`Suite::name`: hosted, `e2e-<run>-door`, as a
/// preview's door lever takes it).
fn door_name(label: &str) -> String {
    format!("{label}--k3x9")
}

pub fn door(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name(NAME, &[Need::Levers, Need::Doors], "its stack builds the door's image (SimpleX's core and its runner)") {
        return Ok(());
    }
    // the run's own door: it ends exhausted (its saves lost), so another
    // run's on the same deployment, after it or beside it, is never this one
    let door = door_name(&s.name("door"));
    let door = door.as_str();
    // a new door: no save, a new profile, its first save in the bucket
    lever(api, door, "start", json!({ "agent": "Juniper" }))?;
    let st = until(api, door, Duration::from_secs(300), |x| ready_from(1)(x) && newest(x).0 == generation(x))?;
    s.ok("a new door starts with no save, and its first save is in the bucket", st["door"]["started"] == 1 && newest(&st) == (generation(&st), 1), &st);

    // a crash: started again (a new start) from its newest save, in step
    lever(api, door, "kill", json!({}))?;
    let st = until(api, door, Duration::from_secs(120), |x| ready_from(2)(x) && newest(x).0 == generation(x))?;
    s.ok("a crash starts it again from its newest save, in step (no sync)", st["runner"]["forcedSyncs"] == 0, &st);
    let crashed = generation(&st);

    // a save that will not restore: its runner says so and ends; it comes
    // up again on the one before it, and the bad one is never given again
    let corrupted = lever(api, door, "corrupt", json!({}))?;
    let bad = corrupted["corrupted"].as_str().unwrap_or_default().to_string();
    let st = until(api, door, Duration::from_secs(120), |x| ready_from(crashed + 1)(x) && newest(x).0 == generation(x))?;
    let given = st["saves"].as_array().into_iter().flatten().any(|x| format!("{}-{}", x["generation"], x["seq"]) == bad && x["unreadable"] == 0);
    s.ok("a save that will not restore is never given again: it comes up on the one before", !bad.is_empty() && !given, &st);

    // its saves kept to the newest three readable
    let readable = st["saves"].as_array().map_or(0, |a| a.iter().filter(|x| x["unreadable"] == 0).count());
    s.ok("its saves are kept to the newest three readable", (1..=3).contains(&readable), &st);

    // a planned stop: its last save, and it stays stopped until asked
    let before = newest(&st);
    lever(api, door, "stop", json!({}))?;
    let st = until(api, door, Duration::from_secs(60), |x| x["running"] == false)?;
    s.ok("a planned stop saves last", newest(&st) > before, &st);
    std::thread::sleep(Duration::from_secs(3));
    let later = lever(api, door, "state", json!({}))?;
    s.ok("a stopped door is not started again", later["running"] == false, &later);
    let stopped = generation(&st);
    lever(api, door, "start", json!({ "agent": "Juniper" }))?;
    let st = until(api, door, Duration::from_secs(120), ready_from(stopped + 1))?;
    s.ok("started again after its stop, from its last save, in step", st["runner"]["forcedSyncs"] == 0 && newest(&st).0 >= stopped, &st);

    // every save's file lost: a door that saved finds none to restore and
    // stops, never a new profile over them (review of #1435)
    lever(api, door, "lose", json!({}))?;
    let st = until(api, door, Duration::from_secs(120), |x| x["door"]["stopped"].is_string() && x["running"] == false)?;
    s.ok("a door whose saves are all lost stops: never a new profile over them", st["door"]["stopped"] == "no save of this door restores" && st["saves"].as_array().is_some_and(|a| a.iter().all(|x| x["unreadable"] == 1)), &st);

    // its connector (templates/simplex), on a door of its own: its owner's
    // message goes from the door to the connector's inbox, as a webhook's
    // does, and its trigger takes it (bound to no agent here, it reaches no
    // one: `connector`, in the computers lane, binds one)
    let owner = api.person()?;
    // its own name, never `<agent>-simplex` (the computer's SimpleX chat's)
    let named = s.named(api, &owner, "sxlane")?;
    let (name, door, _) = connected(s, api, &owner, named, SECRET)?;
    deliver(api, &door, "sx-lane-1", "hello from simplex")?;
    let kept = lever(api, &door, "deliveries", json!({}))?;
    let intake = newest_run(api, &owner, &name, "intake", Duration::from_secs(60));
    s.ok(
        "its owner's message reaches the connector's inbox from the door, and its inbox's trigger takes it (bound to no agent, it reaches no one)",
        kept.as_array().is_some_and(|k| k.iter().any(|d| d["id"] == "sx-lane-1" && d["sent"] == 1)) && intake["output"]["ignored"] == "not linked",
        json!({ "deliveries": kept, "intake": intake }),
    );
    // its name deleted and made again by someone else (the review of
    // #1450): their connector's door is their life's, never the earlier
    // one's (its SimpleX link), and the earlier door's messages reach their
    // inbox no more
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{name}"), None)?;
    ensure!(deleted.status == 200, "deleting the connector: {deleted}");
    let other = api.person()?;
    let (_, theirs, _) = connected(s, api, &other, name.clone(), "e2e-simplex-inbox-of-the-next-life-0a1b2c3d4e5f6a7b")?;
    deliver(api, &door, "sx-lane-2", "still there?")?;
    let old = lever(api, &door, "deliveries", json!({}))?;
    let runs = api.signed(&other, "GET", &format!("/api/f/{name}/runs?op=intake"), None)?;
    s.ok(
        "its name made again by someone else: their connect starts a door of their own, and the earlier door's messages reach their chat no more",
        theirs != door && old.as_array().is_some_and(|k| k.iter().any(|d| d["id"] == "sx-lane-2" && d["sent"] == 0)) && runs.status == 200 && runs.body["runs"].as_array().is_some_and(Vec::is_empty),
        json!({ "theirs": theirs, "earlier": door, "deliveries": old, "runs": runs.body }),
    );
    lever(api, &door, "stop", json!({}))?;
    lever(api, &theirs, "stop", json!({}))?;
    Ok(())
}

/// Whose connector it is, and the agent it binds (the computers lane's).
pub(super) struct Agent<'a> {
    pub owner: &'a Keys,
    pub owner_id: &'a str,
    pub computer: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    pub wake: Duration,
}

/// An app of its author's own: a job that asks what URL it is given, as
/// a door's connector would, naming a door of its choosing.
const REACHER_APP: &str = r#"import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  async reach({ url, door }, job) {
    const r = await job.fetch(url, { method: "POST", headers: { "content-type": "application/json", "x-fragment-door": door }, body: "{}" });
    return { status: r.status };
  }
}
"#;
const REACHER_JSON: &str = r#"{ "operations": { "reach": { "kind": "job" } } }"#;

/// The newest run of `op` on `name`, once it ends (within `within`).
fn newest_run(api: &Api, owner: &Keys, name: &str, op: &str, within: Duration) -> Value {
    let id = api.signed(owner, "GET", &format!("/api/f/{name}/runs?op={op}&limit=1"), None).ok().and_then(|r| r.body["runs"][0]["id"].as_i64()).unwrap_or(0);
    super::jobs::settle(api, owner, name, id, &["succeeded", "failed", "held"], within)
}

/// One of its owner's messages, as the door's runner delivers it.
fn deliver(api: &Api, door: &str, id: &str, text: &str) -> Result<Value> {
    lever(api, door, "deliver", json!({ "message": { "id": id, "kind": "text", "text": text, "at": "2026-11-21T00:00:00Z", "file": null } }))
}

/// A SimpleX connector of `owner`'s named `name`, its inbox's `secret`
/// set, connected: its own job's connect starts its life's door
/// (`fragment_core::door::door_id`) and gives it that secret. Its name,
/// its door's and the door's state.
fn connected(s: &mut Suite, api: &Api, owner: &Keys, name: String, secret: &str) -> Result<(String, String, Value)> {
    let made = api.create_with(owner, json!({ "name": name, "template": "simplex" }))?;
    ensure!(made.status == 200, "making the SimpleX connector: {made}");
    s.owned(&made.body, owner);
    let door = fragment_core::door::door_id(&name, made.body["npub"].as_str().unwrap_or_default()).ok_or_else(|| anyhow::anyhow!("no door for {name}: {made}"))?;
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/secrets/SIMPLEX_DOOR_SECRET"), Some(&json!({ "value": secret, "hosts": [fragment_core::door::DOOR_HOST] })))?;
    ensure!(r.status == 200, "its inbox's secret: {r}");
    let r = api.op(owner, &name, "connect", "connect", json!({ "name": "Juniper" }))?;
    let run = super::jobs::settle(api, owner, &name, r.body["result"]["run"].as_i64().unwrap_or(0), &["succeeded", "failed", "held"], Duration::from_secs(300));
    let st = until(api, &door, Duration::from_secs(300), ready_from(1))?;
    s.ok(
        "its owner's connect, the connector's own job, starts its life's own door and gives it the inbox's secret",
        run["output"]["connected"] == true && st["door"]["door"] == door.as_str() && st["door"]["agent"] == "Juniper" && st["inbox"] == true,
        json!({ "run": run, "door": st }),
    );
    Ok((name, door, st))
}

/// The inbox's secret a run's connector is set with.
const SECRET: &str = "e2e-simplex-inbox-5ec4e7a9f0b1c2d3e4f5061728394a5b";

pub(super) fn connector(s: &mut Suite, api: &Api, a: &Agent<'_>) -> Result<()> {
    if !s.runs_doors() {
        s.skip("the SimpleX connector on a door", &format!("the stack runs no doors: a local one builds their image for a run that names their lane (cargo xtask e2e --only computers,{NAME})"));
        return Ok(());
    }
    let owner = a.owner;
    let named = s.named(api, owner, "sxdoor")?;
    let (name, door, st) = connected(s, api, owner, named, SECRET)?;
    let newest = |op: &str| newest_run(api, owner, &name, op, a.wake);

    // no other fragment's job reaches a door, whichever it names
    let reacher = s.named(api, owner, "reacher")?;
    let c = s.create(api, owner, &reacher)?;
    super::app::ship(s, &c, REACHER_APP.as_bytes(), REACHER_JSON.as_bytes());
    let marks = st["door"]["marks"].clone();
    let r = api.op(owner, &reacher, "reach", "reach", json!({ "url": format!("https://{}/state", fragment_core::door::DOOR_HOST), "door": door }))?;
    let refused = super::jobs::settle(api, owner, &reacher, r.body["result"]["run"].as_i64().unwrap_or(0), &["succeeded", "failed", "held"], a.wake);
    let after = lever(api, &door, "state", json!({}))?;
    s.ok(
        "another fragment's job reaches no door: its fetch of the door's host fails unsent",
        refused["status"] != "succeeded" && refused["error"].as_str().is_some_and(|e| e.contains("a SimpleX connector's own door")) && after["door"]["marks"] == marks,
        json!({ "run": refused, "door": after }),
    );

    // bound to its owner's agent, whose computer then sleeps
    let r = api.signed(owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "link", "agent": a.agent })))?;
    ensure!(r.status == 200 && r.body["door"] == "simplex", "binding the connector as the agent's simplex door: {r}");
    let slept = api.signed(owner, "POST", &format!("/api/computers/{}/sleep", a.computer), Some(&json!({})))?;
    ensure!(slept.body["phase"] == "asleep", "the agent's computer asleep first: {slept}");

    // its owner's message, as the door's runner delivers it: on to the
    // connector's inbox, published there as theirs to their agent, which
    // wakes and answers; the answer goes to the door's runner
    deliver(api, &door, "sx-e2e-1", "hello from simplex")?;
    let kept = lever(api, &door, "deliveries", json!({}))?;
    let intake = newest("intake");
    let chat = || super::jobs::records(api, owner, &name, "chat");
    let relayed = chat().into_iter().find(|r| r["body"]["text"] == "hello from simplex").unwrap_or_default();
    s.ok(
        "its owner's message reaches the connector's inbox from the door, and is published there as theirs, to their agent, through its simplex door",
        kept.as_array().is_some_and(|k| k.iter().any(|d| d["id"] == "sx-e2e-1" && d["sent"] == 1))
            && intake["output"]["relayed"] == true
            && relayed["body"]["relayed"] == json!({ "via": "simplex", "for": a.owner_id, "agent": a.agent })
            && relayed["body"]["to"] == json!([a.identity]),
        json!({ "deliveries": kept, "intake": intake, "record": relayed }),
    );
    let answered = s.eventually(a.wake, || super::computers::agent_replies(&chat(), a.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains("hello from simplex"))));
    let mirror = newest("mirror");
    s.ok(
        "its agent's computer, asleep, wakes for it and answers; the answer goes to the door's runner, which has no SimpleX link here and says so",
        answered && mirror["output"] == json!({ "sent": 0, "why": "not linked" }),
        json!({ "replies": super::computers::agent_replies(&chat(), a.identity), "mirror": mirror }),
    );

    // a delivery without the inbox's secret, from anyone, starts nothing
    let url = format!("{}/api/f/{name}/inbox", api.base);
    let forged = api.call(Call { method: "POST", url, body: Some(json!({ "payload": { "id": "sx-forged", "kind": "text", "text": "forged", "at": "2026-11-21T00:00:00Z" } }).to_string().into_bytes()), content_type: Some("application/json"), extra: vec![("x-simplex-door-secret", "not-the-secret".into())], ..Call::default() })?;
    s.ok("a delivery without the inbox's secret is refused", forged.status == 403, &forged);

    // unbound by its owner: their next message reaches no one
    let r = api.signed(owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "unlink" })))?;
    ensure!(r.status == 200, "unbinding: {r}");
    deliver(api, &door, "sx-e2e-2", "anyone there?")?;
    let intake = newest("intake");
    let none = chat().into_iter().all(|r| r["body"]["text"] != "anyone there?");
    s.ok("unbound by its owner, their next message reaches no one", intake["output"]["ignored"] == "not linked" && none, &intake);
    // its door's container is the connector's alone to keep: let go here
    lever(api, &door, "stop", json!({}))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: each run its own door (the review of #1435: the lane ends with
    /// its door exhausted, so a second run on one deployment, with its
    /// storage kept, waited on a door that never starts again, and two at
    /// once shared one), and a hosted one a door the preview's lever takes.
    /// Method: two runs' labels, as a hosted suite makes them.
    #[test]
    fn each_run_has_a_door_of_its_own() {
        let (one, two) = (crate::hosted::sweep::label("0149355cab", "door"), crate::hosted::sweep::label("0149355cac", "door"));
        let (a, b) = (door_name(&one), door_name(&two));
        assert_ne!(a, b, "two runs, two doors");
        for d in [&a, &b] {
            assert!(fragment_core::levers::is_e2e_fragment(d), "a preview's lever takes {d}");
        }
    }
}
