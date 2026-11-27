//! The operators' move of a computer's SimpleX link into a door of its own
//! (D4, FIN-731; cell/src/simplex_move.rs, docs/bridge.md "SimpleX"), on
//! the real platform alone (`--hosted --only simplex-move`, by name): a
//! deployment that runs doors, whose computers run a Hermes image with this
//! release's bridge, and a SimpleX app of the run's own on SimpleX's
//! preset servers (the lab's client, as `simplex-link`'s phone):
//!
//! - its person's always-on computer runs their agent, its SimpleX linked
//!   there by their own ticket, and the phone's message is answered in its
//!   computer's SimpleX chat;
//! - its save damaged on its way out of its computer each time its move
//!   asks for it (FIN-775): never imported, and the move goes back at once,
//!   its computer running its link again; its making stopped before its
//!   record says made: its record names its connector, asked again it
//!   refuses and makes nothing, `--back` archives it, and a fresh move
//!   leaves exactly one live; and its handover failing once its
//!   core may have stopped: the same way back; and moved, its computer
//!   then not always on: the way back refuses, its door keeping the link,
//!   until always on again (no model call before them);
//! - `operator simplex-move` plans it, then goes: the connector made and
//!   bound as theirs, the link handed over and imported, its door connected
//!   and linked with the same contact, the old chat archived; a message the
//!   phone sends as it moves (its computer's core stopped, its door not
//!   yet up) arrives once, in the connector's chat; the phone's next
//!   message lands there, once, nothing in the old one, and the agent's
//!   answer reaches the phone through the door;
//! - its way back refused busy while its agent works in the connector's
//!   chat, then while its reply waits to go, and that reply reaching the
//!   phone once, through the door;
//! - back: a message the door's runner held as it fenced lands in its
//!   computer's chat, answered; its computer runs the link again, the
//!   phone's next message lands in its computer's chat, and the connector
//!   is unbound and archived, its history kept.
//!
//! Paid calls: the agent's answers (six turns, a few calls each).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::computers::{lever_with, members_only, phase, AGENT_JSON};
use super::jobs::records;
use super::simplex_setup::Phone;
use crate::api::{Api, Call};
use crate::{Need, Suite};

pub const NAME: &str = "simplex-move";

/// The paid calls its owner is lent: four answers.
const PAID_CALLS: u64 = 15;
/// A first start on a preview: the image pulled, Hermes booted.
const WAKE: Duration = Duration::from_secs(420);
/// A door's start, SimpleX's servers, a real model's answer.
const SLOW: Duration = Duration::from_secs(300);

pub fn simplex_move(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name(NAME, &[Need::Computers, Need::Doors, Need::Levers, Need::Models, Need::Operator], "the real platform alone: SimpleX's servers, a Hermes image with this release's bridge, doors") {
        return Ok(());
    }
    if !s.hosted() {
        s.skip("a computer's SimpleX link moved into a door, and back", "the real platform alone: no SimpleX server is reachable here");
        return Ok(());
    }
    let operator = s.wiper.clone().context("Need::Operator lends an operator key")?;
    let owner = api.person_paying(PAID_CALLS)?;
    let owner_id = api.identity(&owner)?;
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let id = r.body["computer"].as_str().unwrap_or("").to_string();
    let origin = r.body["origin"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && !id.is_empty(), "its computer: {r}");
    let agent = s.named(api, &owner, "juniper")?;
    let made = s.create(api, &owner, &agent)?;
    s.commit(&made, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&made);
    members_only(api, &owner, &agent)?;
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{id}/agents/{agent}"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "its agent assigned: {r}");
    // it starts its computer, and answers once that is up: as patient as
    // a wake (a new image's first start on a preview passed a minute)
    let r = lever_with(&api.patient(WAKE), &id, json!({ "op": "always-on", "on": true }))?;
    anyhow::ensure!(r.status == 200, "always on: {r}");
    let woke = api.patient(WAKE).signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})))?;
    let awake = s.eventually(WAKE, || phase(api, &owner, &id) == "awake");
    // its bridge's SimpleX, through its screen port, as its owner's page
    // reaches it: a port ticket's session, from that port's own origin
    let t = api.signed(&owner, "POST", &format!("/api/computers/{id}/ports/6080/ticket"), Some(&json!({})))?;
    let signed_in = api.call(Call { method: "GET", url: t.body["url"].as_str().unwrap_or("").to_string(), ..Call::default() })?;
    let cookie = signed_in.cookies().into_iter().find(|c| c.starts_with("fragment_computer=")).map(|c| c.split(';').next().unwrap_or("").to_string()).context("its port's session")?;
    let port = |method: &'static str, path: &str, body: Option<&Value>| {
        api.call(Call {
            method,
            url: format!("{origin}/p/6080{path}"),
            body: body.map(|b| b.to_string().into_bytes()),
            content_type: body.map(|_| "application/json"),
            cookie: Some(cookie.clone()),
            extra: vec![("origin", origin.clone())],
            ..Call::default()
        })
    };
    let mut state = Value::Null;
    let up = s.eventually(SLOW, || {
        state = port("GET", &format!("/simplex/state?agent={agent}"), None).map(|r| r.body).unwrap_or(Value::Null);
        state["phase"] == "off"
    });
    s.ok("its person's always-on computer runs their agent, its bridge's SimpleX ready", woke.status == 200 && awake && up, json!({ "woke": woke.status, "state": state }));

    // linked from its computer, by its owner's own ticket, as Settings links one
    let ticket = api.signed(&owner, "POST", &format!("/api/computers/{id}/agents/{agent}/door"), Some(&json!({ "door": "simplex", "act": "link" })))?;
    anyhow::ensure!(ticket.status == 200, "its owner's link ticket: {ticket}");
    let nonce = ticket.body["nonce"].as_str().unwrap_or("").to_string();
    let linking = port("POST", &format!("/simplex/link?agent={agent}"), Some(&ticket.body))?;
    anyhow::ensure!(linking.status == 200, "its link, asked of its bridge through its port: {linking}");
    let mut invite = Value::Null;
    let invited = s.eventually(SLOW, || {
        invite = port("GET", &format!("/simplex/state?agent={agent}&s={nonce}"), None).map(|r| r.body).unwrap_or(Value::Null);
        invite["link"].is_string()
    });
    if !invited {
        s.ok("its computer's SimpleX shows its owner an invitation", false, &invite);
        return Ok(());
    }
    let phone_dir = s.dir("simplex-move-phone");
    let mut phone = Phone::start(&phone_dir, &s.name("phone"))?;
    phone.cmd(&format!("/c {}", invite["link"].as_str().unwrap_or("")))?;
    let contact = phone.wait(SLOW, |e| super::simplex_setup::tag(e) == "contactConnected")?["contact"]["contactId"].as_i64().unwrap_or(0);
    let linked = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["phase"] == "linked"));
    s.ok("its computer's SimpleX linked to the phone, by its owner's ticket", linking.status == 200 && linked && contact > 0, json!({ "link": linking.status, "invite": invite["phase"] }));
    let old = s.eventually(SLOW, || simplex_chat(api, &owner, &agent).is_some()).then(|| simplex_chat(api, &owner, &agent)).flatten().unwrap_or_default();
    let mv = |act: &str| api.signed(&operator, "POST", &format!("/api/admin/people/{owner_id}/simplex-move"), Some(&json!({ "agent": agent, "act": act })));

    // its making stopped once its connector was made, before its record
    // says made (as its Worker stopping there would, or a secret's write
    // failing): its record names that connector, the one its person holds;
    // asked again, the move refuses (`--back` first) and makes nothing; its
    // `--back` archives it (never deletes: kept, out of its person's way),
    // and a fresh move leaves exactly one live (the review of #1541's `make`)
    let label = agent.rsplit_once("--").map_or(agent.as_str(), |(l, _)| l).to_string();
    // its person's connectors for this agent, but its old chat: live, then archived
    let listed_as = |archived: bool| -> Vec<String> { listed(api, &owner).into_iter().filter(|f| (f["archived"] == true) == archived).filter_map(|f| f["name"].as_str().map(str::to_string)).filter(|n| *n != old && n.rsplit_once("--").is_some_and(|(l, _)| l == format!("{label}-simplex"))).collect() };
    let connectors = || listed_as(false);
    let stop_made = || lever_with(api, &id, json!({ "op": "fail-set", "path": "made", "times": 1 }));
    anyhow::ensure!(stop_made()?.status == 200, "the fail-set lever");
    let stopped = mv("go")?;
    let named = mv("plan")?.body["record"].clone();
    let first_name = named["connector"].as_str().unwrap_or("").to_string();
    let first = connectors();
    s.ok(
        "its making stopped once its connector was made: its record names that connector, made as its person's, the one they hold",
        stopped.status != 200 && named["step"] == "making" && !first_name.is_empty() && first == vec![first_name.clone()],
        json!({ "go": stopped.body, "record": named, "connectors": first }),
    );
    if first_name.is_empty() {
        return Ok(());
    }
    let again = mv("go")?;
    let still = connectors();
    let back = mv("back")?;
    let (gone, kept) = (connectors(), listed_as(true));
    anyhow::ensure!(stop_made()?.status == 200, "the fail-set lever");
    let fresh = mv("go")?;
    let fresh_name = mv("plan")?.body["record"]["connector"].as_str().unwrap_or("").to_string();
    let one = connectors();
    s.ok(
        "asked again, the move refuses (`already_exists`: `--back` first) and makes nothing new; `--back` archives its connector, never deleted; a fresh move leaves exactly one live",
        again.status == 409 && again.body["error"] == "already_exists" && still == vec![first_name.clone()]
            && back.status == 200 && back.body["connector"] == first_name.as_str() && gone.is_empty() && kept == vec![first_name.clone()]
            && fresh.status != 200 && !fresh_name.is_empty() && fresh_name != first_name && one == vec![fresh_name.clone()],
        json!({ "again": again.body, "still": still, "back": back.body, "gone": gone, "kept": kept, "fresh": fresh.body, "one": one }),
    );
    // that one deleted by its person: its `--back` reads it gone (the typed
    // answer of a connector never made) and archives nothing
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{fresh_name}"), None)?;
    let cleared = mv("back")?;
    let (none, archived, after) = (connectors(), listed_as(true), mv("plan")?.body["record"].clone());
    s.ok(
        "its connector deleted by its person, `--back` finds it gone and archives nothing; the move is forgotten",
        deleted.status == 200 && cleared.status == 200 && cleared.body["connector"].is_null() && none.is_empty() && archived == vec![first_name.clone()] && after.is_null(),
        json!({ "deleted": deleted.status, "cleared": cleared.body, "none": none, "archived": archived, "record": after }),
    );

    // its link's save damaged on its way out of its computer each of the
    // times its move asks for it (FIN-775): never imported, and the move
    // goes back at once, its computer running its link again, never both
    // ends stopped (Codex's round on #1541)
    let r = lever_with(api, &id, json!({ "op": "damage", "path": "/simplex/handover", "times": 3 }))?;
    anyhow::ensure!(r.status == 200, "the damage lever: {r}");
    let failed = mv("go")?;
    let again = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["running"] == true && r.body["phase"] == "linked"));
    let forgot = mv("plan")?;
    let said = failed.body["message"].as_str().unwrap_or("");
    s.ok(
        "its save damaged on its way out each time its move asks: never imported, and it goes back at once, its computer running its link again",
        failed.status != 200 && said.contains("not_its_sha") && said.contains("went back") && again && forgot.body["record"].is_null(),
        json!({ "go": failed.body, "plan": forgot.body }),
    );
    if !again {
        return Ok(());
    }
    // its handover failing once its core may have stopped (a pack's disk
    // error, an answer lost: here a 500 after its bridge packed), whatever
    // the failure, and its computer's mark then unread too (a computer
    // gone unavailable): the same way back, its computer running its link
    // again (Codex's rounds on 45fc87eee and 6f714bbde)
    let r = lever_with(api, &id, json!({ "op": "fail", "path": "/simplex/handover", "times": 1 }))?;
    anyhow::ensure!(r.status == 200, "the fail lever: {r}");
    let r = lever_with(api, &id, json!({ "op": "fail-read", "times": 1 }))?;
    anyhow::ensure!(r.status == 200, "the fail-read lever: {r}");
    let failed = mv("go")?;
    let again = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["running"] == true && r.body["phase"] == "linked"));
    let forgot = mv("plan")?;
    let said = failed.body["message"].as_str().unwrap_or("");
    s.ok(
        "its handover failing once its core may have stopped (an answer lost after it packed), whatever the failure, its mark then unread: the same way back, its computer running its link again",
        failed.status != 200 && said.contains("went back") && again && forgot.body["record"].is_null(),
        json!({ "go": failed.body, "plan": forgot.body }),
    );
    if !again {
        return Ok(());
    }
    // moved (no message yet: no model call), then its computer not always
    // on (its person may turn that off once their SimpleX is in its door):
    // the way back refuses, its door keeping the link, never stopped for a
    // computer that would not run it (Codex's round on 8cacd32d1); always
    // on again, it goes back
    let t0 = Instant::now();
    let mut went = Value::Null;
    // bounded: SLOW and a door's start
    while t0.elapsed() < SLOW * 2 && went["record"]["step"] != "done" {
        let r = mv("go")?;
        went = r.body.clone();
        if r.status != 200 && r.status != 409 {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let door = went["record"]["door"].as_str().unwrap_or("").to_string();
    let r = lever_with(api, &id, json!({ "op": "always-on", "on": false }))?;
    anyhow::ensure!(r.status == 200, "always on off: {r}");
    let refused = mv("back")?;
    let st = api.unsigned("POST", "/api/test/door", Some(&json!({ "door": door, "op": "state" }))).map(|r| r.body).unwrap_or(Value::Null);
    let r = lever_with(&api.patient(WAKE), &id, json!({ "op": "always-on", "on": true }))?;
    anyhow::ensure!(r.status == 200, "always on again: {r}");
    let awake = s.eventually(WAKE, || phase(api, &owner, &id) == "awake");
    // its bridge's answer to the way back damaged on its way out once: read
    // as damaged, nothing marked, asked again (its bridge then says it holds
    // the link already: Codex's round on f434be3bb)
    let r = lever_with(api, &id, json!({ "op": "damage", "path": "/simplex/back", "times": 1 }))?;
    anyhow::ensure!(r.status == 200, "the damage lever: {r}");
    let back = mv("back")?;
    let again = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["running"] == true && r.body["phase"] == "linked"));
    s.ok(
        "moved, its computer then not always on: the way back refuses, its door keeping the link; always on again, it goes back (its bridge's answer damaged once, asked again), its computer running its link",
        went["record"]["step"] == "done"
            && refused.status == 409
            && refused.body["message"].as_str().is_some_and(|m| m.contains("always on"))
            && st["runner"]["phase"] == "linked"
            && st["door"]["stopped"].is_null()
            && awake
            && back.status == 200
            && again,
        json!({ "went": went["record"], "refused": refused.body, "door": { "runner": st["runner"]["phase"], "stopped": st["door"]["stopped"] }, "back": back.body }),
    );
    if !again {
        return Ok(());
    }
    // its agent's answer: any word from it after the phone's (it may quote it)
    let before = answered(s, api, &owner, &mut phone, contact, "before the move", &old);
    let relayed_before = relayed(api, &owner, &old);
    s.ok("the phone's message lands in its computer's SimpleX chat, and its agent's answer reaches the phone", before.is_ok() && relayed_before == ["before the move"], json!({ "chat": old, "relayed": relayed_before, "answer": format!("{before:?}") }));

    // the move: planned, then gone through step by step
    let plan = mv("plan")?;
    s.ok("its plan says what goes first, and changes nothing", plan.status == 200 && plan.body["blocked"].is_null() && plan.body["record"].is_null(), &plan);
    let t0 = Instant::now();
    let mut went = Value::Null;
    let mut during = false;
    // bounded: SLOW and a door's start
    while t0.elapsed() < SLOW * 2 {
        let r = mv("go")?;
        went = r.body.clone();
        if r.body["record"]["step"] == "done" || (r.status != 200 && r.status != 409) {
            break;
        }
        // as it moves: its computer's core stopped, its door starting, the
        // connector not yet told which door is its (the cutover)
        if !during && r.body["record"]["step"] == "connecting" {
            phone.drain(Duration::from_secs(1))?;
            phone.say(contact, "during the move")?;
            during = true;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let connector = went["record"]["connector"].as_str().unwrap_or("").to_string();
    let computer_off = port("GET", &format!("/simplex/state?agent={agent}"), None).map(|r| r.body["running"] == false).unwrap_or(false);
    let archived = listed(api, &owner).iter().any(|f| f["name"] == old.as_str() && f["archived"] == true);
    s.ok(
        "go: its connector made and bound as its person's, the link handed over and imported, its door linked with the same contact, its computer's core stopped, the old chat archived",
        went["record"]["step"] == "done" && !connector.is_empty() && computer_off && archived,
        json!({ "went": went, "computer_off": computer_off, "archived": archived, "secs": t0.elapsed().as_secs() }),
    );
    // what the phone said as it moved: once, in the connector's chat alone,
    // and answered (taken, so its answer is not the next one's); a door
    // linked within the `go` that started it left no cutover to say it in
    let cutover = "a message the phone sent as it moved (its computer's core stopped, its door starting) arrives once, in the connector's chat, and is answered";
    if during {
        let landed = s.eventually(SLOW, || relayed(api, &owner, &connector).iter().any(|t| t == "during the move"));
        let heard = landed && phone.received(SLOW, |got| got.iter().any(|g| !g.trim().is_empty())).is_ok();
        let (in_door, in_old) = (relayed(api, &owner, &connector), relayed(api, &owner, &old));
        s.ok(cutover, landed && heard && in_door.iter().filter(|t| *t == "during the move").count() == 1 && !in_old.iter().any(|t| t == "during the move"), json!({ "door": in_door, "old": in_old, "answered": heard }));
    } else {
        s.skip(cutover, "its door linked within the `go` that started it: no cutover to send in");
    }
    let after = answered(s, api, &owner, &mut phone, contact, "after the move", &connector);
    let (in_door, in_old) = (relayed(api, &owner, &connector), relayed(api, &owner, &old));
    // what its connector's chat and its door hold now, said when an answer does not come
    let door = went["record"]["door"].as_str().unwrap_or("").to_string();
    let held = || {
        let chat: Vec<Value> = records(api, &owner, &connector, "chat").iter().map(|r| json!({ "by": r["principal"], "text": r["body"]["text"], "relayed": r["body"]["relayed"].is_object() })).collect();
        let st = api.unsigned("POST", "/api/test/door", Some(&json!({ "door": door, "op": "state" }))).map(|r| r.body).unwrap_or(Value::Null);
        json!({ "chat": chat, "runner": st["runner"], "unsent": st["unsent"], "running": st["running"] })
    };
    s.ok(
        "the phone's next message lands in the connector's chat, once, nothing more in the old one, and the agent's answer reaches the phone through the door",
        after.is_ok() && in_door.iter().filter(|t| *t == "after the move").count() == 1 && in_old == ["before the move"],
        json!({ "door": in_door, "old": in_old, "answer": format!("{after:?}"), "held": if after.is_err() { held() } else { Value::Null } }),
    );

    // its way back waits while anything is on its way through its connector
    // (the review of 354324b90): its agent at work there, then its reply
    // held (its mirror paused, so its run waits blocked); each refused busy,
    // naming it, its door running on; the reply reaches the phone once,
    // through its door, once its run goes
    let busy_back = |why: &str| mv("back").map(|r| (r.status == 409 && r.body["message"].as_str().is_some_and(|m| m.contains(why)), r.body));
    let at_work = || records(api, &owner, &connector, "work").iter().rev().find(|r| r["body"]["kind"].as_str().is_some_and(|k| k.starts_with("turn."))).is_some_and(|r| matches!(r["body"]["kind"].as_str(), Some("turn.start" | "turn.step")));
    let pause = |on: bool| api.signed(&owner, "POST", &format!("/api/f/{connector}/pause"), Some(&json!({ "op": "mirror", "paused": on })));
    let blocked_run = || api.signed(&owner, "GET", &format!("/api/f/{connector}/runs?op=mirror&status=blocked&limit=1"), None).ok().and_then(|r| r.body["runs"][0]["id"].as_i64());
    anyhow::ensure!(pause(true)?.status == 200, "pausing its connector's mirror");
    phone.drain(Duration::from_secs(5))?;
    phone.say(contact, "Run `sleep 20` in your terminal, then reply with just the word done.")?;
    let working = s.eventually(SLOW, at_work);
    let (refused_working, said_working) = busy_back("at work")?;
    let blocked = s.eventually(SLOW, || blocked_run().is_some());
    let (refused_reply, said_reply) = busy_back("mirror run blocked")?;
    anyhow::ensure!(pause(false)?.status == 200, "unpausing its connector's mirror");
    let replayed = api.signed(&owner, "POST", &format!("/api/f/{connector}/replay"), Some(&json!({ "run": blocked_run().unwrap_or(0) })))?;
    let heard = phone.received(SLOW, |got| got.iter().any(|g| g.to_lowercase().contains("done"))).unwrap_or_default();
    s.ok(
        "its way back is refused busy while its agent is at work in its connector's chat, then while its reply waits to go (each named), its door running on; that reply reaches the phone once, through its door",
        working && refused_working && blocked && refused_reply && replayed.status == 200 && heard.iter().filter(|g| g.to_lowercase().contains("done")).count() == 1,
        json!({ "working": said_working, "reply": said_reply, "replayed": replayed.status, "heard": heard }),
    );

    // a message its door's runner holds as the door fences for its way back
    // (refused, kept past its cursor) goes in its save to its computer, which
    // delivers it into its own chat; its agent answers it there, once (the
    // review of 354324b90: the door's stop delivers nothing new)
    let door_lever = |op: &str| api.unsigned("POST", "/api/test/door", Some(&json!({ "door": door, "op": op })));
    anyhow::ensure!(door_lever("fence")?.status == 200, "fencing its door");
    phone.drain(Duration::from_secs(5))?;
    phone.say(contact, "held at the door")?;
    let held_back = s.eventually(SLOW, || door_lever("state").is_ok_and(|r| r.body["runner"]["waiting"].as_u64().unwrap_or(0) >= 1));
    // back: its computer runs the link again, the connector gone; asked
    // again while its agent's last turn still ends
    let mut back = mv("back")?;
    // bounded: SLOW
    let t0 = Instant::now();
    while back.status == 409 && back.body["message"].as_str().is_some_and(|m| m.contains("busy")) && t0.elapsed() < SLOW {
        std::thread::sleep(Duration::from_secs(3));
        back = mv("back")?;
    }
    let landed = s.eventually(SLOW, || relayed(api, &owner, &old).iter().any(|t| t == "held at the door"));
    let answered_held = landed && phone.received(SLOW, |got| got.iter().any(|g| !g.trim().is_empty())).is_ok();
    let (in_door, in_old) = (relayed(api, &owner, &connector), relayed(api, &owner, &old));
    s.ok(
        "a message its runner held as its door fenced for its way back goes in its save to its computer: it lands once in its computer's chat, never the connector's, and its agent's answer reaches the phone",
        held_back && back.status == 200 && landed && answered_held && in_old.iter().filter(|t| *t == "held at the door").count() == 1 && !in_door.iter().any(|t| t == "held at the door"),
        json!({ "held": held_back, "back": back.body, "old": in_old, "door": in_door }),
    );
    let running = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["running"] == true && r.body["phase"] == "linked"));
    let unbound = api.signed(&owner, "GET", &format!("/api/f/{connector}/door"), None).map(|r| r.body["bound"].is_null()).unwrap_or(false);
    let kept = listed(api, &owner).iter().any(|f| f["name"] == connector.as_str() && f["archived"] == true) && relayed(api, &owner, &connector).iter().any(|t| t == "after the move");
    let again = answered(s, api, &owner, &mut phone, contact, "after the way back", &old);
    let in_old = relayed(api, &owner, &old);
    s.ok(
        "back: its computer runs the link again, the phone's next message lands in its computer's chat, and the connector is unbound and archived, its history kept",
        back.status == 200 && running && unbound && kept && again.is_ok() && in_old == ["before the move", "held at the door", "after the way back"],
        json!({ "back": back.body, "old": in_old, "answer": format!("{again:?}") }),
    );
    let _ = lever_with(api, &id, json!({ "op": "always-on", "on": false }));
    Ok(())
}

/// Its agent's answer to `text` in `chat`: the phone's earlier messages let
/// go first, then `text` said and found in `chat` (relayed), then any word
/// from it after that (it may quote it).
fn answered(s: &Suite, api: &Api, owner: &crate::Keys, phone: &mut Phone, contact: i64, text: &str, chat: &str) -> Result<Vec<String>> {
    phone.drain(Duration::from_secs(5))?;
    phone.say(contact, text)?;
    anyhow::ensure!(s.eventually(SLOW, || relayed(api, owner, chat).iter().any(|t| t == text)), "{text:?} never landed in {chat}");
    phone.received(SLOW, |got| got.iter().any(|g| !g.trim().is_empty() && !g.contains("You're linked")))
}

/// The agent's SimpleX chat its computer made as it linked: its person's
/// chat labelled `<label>-simplex`.
fn simplex_chat(api: &Api, owner: &crate::Keys, agent: &str) -> Option<String> {
    let label = agent.rsplit_once("--").map_or(agent, |(l, _)| l);
    listed(api, owner).into_iter().filter_map(|f| f["name"].as_str().map(str::to_string)).find(|n| n.rsplit_once("--").is_some_and(|(l, _)| l == format!("{label}-simplex")))
}

fn listed(api: &Api, owner: &crate::Keys) -> Vec<Value> {
    api.signed(owner, "GET", "/api/fragments", None).ok().and_then(|r| r.body["fragments"].as_array().cloned()).unwrap_or_default()
}

/// The texts relayed from SimpleX on `chat`, in order.
fn relayed(api: &Api, owner: &crate::Keys, chat: &str) -> Vec<String> {
    records(api, owner, chat, "chat").iter().filter(|r| r["body"]["relayed"]["via"] == "simplex").filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect()
}
