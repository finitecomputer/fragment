//! A fragment acting elsewhere (docs/api.md, "Acting in another
//! fragment"): an agent's fragment, running its own code, posts, drafts
//! and reads in a room as its agent, through its `FRAGMENTS` capability
//! (`this.otherFragments`; cell/src/acting.rs). Its agent is made by an
//! assignment to its owner's computer, undone before it joins anything, so
//! no computer is ever woken.
//!
//! - valid: a post there names the agent, and its record's runs are one hop
//!   deeper than the call that posted it; a draft is taken; the records
//!   read back;
//! - replay: the same id and body again is that record, another body 409;
//! - invalid: the room checks the agent as it would the agent's own request
//!   (a channel its role may not post to, a fragment it is no member of, a
//!   `relayed` record no door signed), and a fragment that is no agent's
//!   acts nowhere;
//! - depth: a post from a job at the hop budget runs there; one a hop past
//!   it is blocked there, as a chain within one fragment is.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_proto::limits;
use serde_json::{json, Value};

use super::app::ship;
use super::jobs::{inbox, records, runs, settle};
use crate::api::Api;
use crate::{Need, Suite};

/// The agent's own code: `act` (a query) acts once; `relay` (a job its
/// inbox triggers) posts what it is given.
const APP: &[u8] = include_bytes!("../../fixtures/elsewhere.mjs");
const MANIFEST: &[u8] = include_bytes!("../../fixtures/elsewhere.json");
/// The room: `chat` anyone may post to, `notes` its editors; each `chat`
/// record runs `heard`.
const ROOM_APP: &[u8] = include_bytes!("../../fixtures/heard.mjs");
const ROOM_JSON: &[u8] = include_bytes!("../../fixtures/heard.json");

const WAIT: Duration = Duration::from_secs(30);

pub fn elsewhere(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("elsewhere", &[Need::Computers]) {
        return Ok(());
    }
    let owner = api.person()?;
    // the agent: a fragment running its own code, members only
    let agent = s.named(api, &owner, "acts")?;
    let made = s.create(api, &owner, &agent)?;
    ship(s, &made, APP, MANIFEST);
    let token = made["inboxToken"].as_str().context("its inbox token")?.to_string();
    let r = api.signed(&owner, "PUT", &format!("/api/f/{agent}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the agent made members-only: {r}");
    // its key becomes its agent's identity at its first assignment; undone
    // at once, so no join wakes the computer
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let computer = r.body["computer"].as_str().with_context(|| format!("a computer: {r}"))?.to_string();
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{computer}/agents/{agent}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    let r2 = api.signed(&owner, "DELETE", &format!("/api/computers/{computer}/agents/{agent}"), None)?;
    anyhow::ensure!(fragment_core::npub::is_identity(&identity) && r2.status == 200, "the agent's identity: {r} {r2}");

    // the room, where the agent is a viewer; another the agent is not in
    let room = s.named(api, &owner, "room")?;
    let c = s.create(api, &owner, &room)?;
    ship(s, &c, ROOM_APP, ROOM_JSON);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{room}/members/{identity}"), Some(&json!({ "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins the room: {r}");
    let closed = s.named(api, &owner, "closed")?;
    let c = s.create(api, &owner, &closed)?;
    ship(s, &c, ROOM_APP, ROOM_JSON);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{closed}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the closed room made members-only: {r}");

    let act = |from: &str, act: &str, name: &str, channel: &str, arg: Value| -> Result<Value> {
        let r = api.op(&owner, from, "act", &format!("{act}-{}", fragment_nip98::Keys::generate().pubkey_hex()), json!({ "act": act, "name": name, "channel": channel, "arg": arg }))?;
        anyhow::ensure!(r.status == 200, "act {act}: {r}");
        Ok(r.body["result"].clone())
    };

    // valid: a post as the agent, its record's run a hop deeper than the query that posted it
    let posted = act(&agent, "post", &room, "chat", json!({ "id": "hello", "body": { "text": "hello from the agent's code" } }))?;
    let record = &posted["answer"]["record"];
    let run = heard(s, api, &owner, &room, 1);
    s.ok(
        "an agent's fragment posts in a room as its agent, through its own code",
        record["principal"] == identity.as_str() && record["body"]["text"] == "hello from the agent's code" && posted["answer"]["replayed"] == false,
        &posted,
    );
    s.ok("its record's run is one hop deeper than the call that posted it", run.len() == 1 && run[0]["depth"] == 1 && run[0]["status"] == "succeeded", json!(run));
    let drafted = act(&agent, "draft", &room, "chat", json!({ "turn": "t1", "text": "typing" }))?;
    s.ok("it drafts there as its agent", drafted["answer"]["ok"] == true, &drafted);
    let read = act(&agent, "records", &room, "chat", json!({ "after": 0 }))?;
    let seqs: Vec<&Value> = read["answer"]["records"].as_array().map(|l| l.iter().map(|r| &r["seq"]).collect()).unwrap_or_default();
    s.ok("it reads the room's records back", seqs == [&record["seq"]], &read);

    // replay: the same post is its record; another body under its id is refused
    let again = act(&agent, "post", &room, "chat", json!({ "id": "hello", "body": { "text": "hello from the agent's code" } }))?;
    let other = act(&agent, "post", &room, "chat", json!({ "id": "hello", "body": { "text": "something else" } }))?;
    s.ok(
        "the same post again is the same record, appended once; another body under its id is 409",
        again["answer"]["replayed"] == true && again["answer"]["record"]["seq"] == record["seq"] && other["error"]["status"] == 409 && records(api, &owner, &room, "chat").len() == 1,
        json!({ "again": again, "other": other }),
    );

    // invalid: checked as the agent's own request would be
    let notes = act(&agent, "post", &room, "notes", json!({ "id": "n1", "body": { "text": "a viewer's note" } }))?;
    s.ok("a channel its role may not post to refuses it (403), as it would the agent", notes["error"]["status"] == 403 && records(api, &owner, &room, "notes").is_empty(), &notes);
    let stranger = act(&agent, "post", &closed, "chat", json!({ "id": "s1", "body": { "text": "let me in" } }))?;
    s.ok(
        "a fragment it is no member of refuses it",
        matches!(stranger["error"]["status"].as_u64(), Some(403 | 404)) && records(api, &owner, &closed, "chat").is_empty(),
        &stranger,
    );
    let relayed = act(&agent, "post", &room, "chat", json!({ "id": "r1", "body": { "text": "from the owner", "relayed": { "via": "simplex" } } }))?;
    s.ok("a relayed record no door signed is refused (403), as the agent's own is", relayed["error"]["status"] == 403 && records(api, &owner, &room, "chat").len() == 1, &relayed);
    let named = act(&agent, "post", "no such name", "chat", json!({ "id": "x", "body": {} }))?;
    s.ok("an act names a fragment (400)", named["error"]["status"] == 400, &named);
    // the same code on a fragment that is no agent's acts nowhere
    let plain = s.named(api, &owner, "plain")?;
    let c = s.create(api, &owner, &plain)?;
    ship(s, &c, APP, MANIFEST);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{plain}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins the plain fragment: {r}");
    let nobody = act(&plain, "post", &room, "chat", json!({ "id": "p1", "body": { "text": "as whom?" } }))?;
    s.ok("a fragment that is no agent's acts nowhere (403)", nobody["error"]["status"] == 403 && records(api, &owner, &room, "chat").len() == 1, &nobody);

    // depth: a job at the hop budget posts, and its record runs there; one
    // a hop past it posts a record whose run is blocked
    let budget = limits::HOP_DEPTH_MAX;
    let mut relayed_runs = vec![];
    for (i, hops) in [budget - 1, budget].into_iter().enumerate() {
        let r = inbox(api, &agent, &token, &json!({ "payload": { "name": room, "id": format!("deep-{i}"), "body": { "text": format!("from depth {hops}") } } }), Some(hops))?;
        let id = r.body["runs"][0].as_i64().unwrap_or(0);
        relayed_runs.push(settle(api, &owner, &agent, id, &["succeeded", "held", "blocked"], WAIT));
    }
    // newest first: the relays were one after the other
    let deep = heard(s, api, &owner, &room, 3);
    let (past, at) = (deep.first().cloned().unwrap_or_default(), deep.get(1).cloned().unwrap_or_default());
    s.ok(
        "a job's post carries its hop depth: at the budget its record runs, a hop past it is blocked",
        relayed_runs.iter().all(|r| r["status"] == "succeeded")
            && at["depth"] == budget
            && at["status"] == "succeeded"
            && past["depth"] == budget + 1
            && past["status"] == "blocked",
        json!({ "relayed": relayed_runs, "at": at, "past": past }),
    );
    Ok(())
}

/// The room's `heard` runs, newest first, once `want` of them are done.
fn heard(s: &Suite, api: &Api, owner: &fragment_nip98::Keys, room: &str, want: usize) -> Vec<Value> {
    s.eventually(WAIT, || runs(api, owner, room, "&op=heard").iter().filter(|r| r["status"] != "queued" && r["status"] != "running").count() >= want);
    runs(api, owner, room, "&op=heard")
}
