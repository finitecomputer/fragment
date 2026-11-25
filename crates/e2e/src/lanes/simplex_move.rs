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
//! - `operator simplex-move` plans it, then goes: the connector made and
//!   bound as theirs, the link handed over and imported, its door connected
//!   and linked with the same contact, the old chat archived; a message the
//!   phone sends as it moves (its computer's core stopped, its door not
//!   yet up) arrives once, in the connector's chat; the phone's next
//!   message lands there, once, nothing in the old one, and the agent's
//!   answer reaches the phone through the door;
//! - back: its computer runs the link again, the phone's next message
//!   lands in its computer's chat, and the connector is unbound and
//!   archived, its history kept.
//!
//! Paid calls: the agent's answers (four turns, a few calls each).

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
    let r = lever_with(api, &id, json!({ "op": "always-on", "on": true }))?;
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
    // its agent's answer: any word from it after the phone's (it may quote it)
    let before = answered(s, api, &owner, &mut phone, contact, "before the move", &old);
    let relayed_before = relayed(api, &owner, &old);
    s.ok("the phone's message lands in its computer's SimpleX chat, and its agent's answer reaches the phone", before.is_ok() && relayed_before == ["before the move"], json!({ "chat": old, "relayed": relayed_before, "answer": format!("{before:?}") }));

    // the move: planned, then gone through step by step
    let mv = |act: &str| api.signed(&operator, "POST", &format!("/api/admin/people/{owner_id}/simplex-move"), Some(&json!({ "agent": agent, "act": act })));
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

    // back: its computer runs the link again, the connector gone
    let back = mv("back")?;
    let running = s.eventually(SLOW, || port("GET", &format!("/simplex/state?agent={agent}"), None).is_ok_and(|r| r.body["running"] == true && r.body["phase"] == "linked"));
    let unbound = api.signed(&owner, "GET", &format!("/api/f/{connector}/door"), None).map(|r| r.body["bound"].is_null()).unwrap_or(false);
    let kept = listed(api, &owner).iter().any(|f| f["name"] == connector.as_str() && f["archived"] == true) && relayed(api, &owner, &connector).iter().any(|t| t == "after the move");
    let again = answered(s, api, &owner, &mut phone, contact, "after the way back", &old);
    let in_old = relayed(api, &owner, &old);
    s.ok(
        "back: its computer runs the link again, the phone's next message lands in its computer's chat, and the connector is unbound and archived, its history kept",
        back.status == 200 && running && unbound && kept && again.is_ok() && in_old == ["before the move", "after the way back"],
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
