//! A fragment acting elsewhere (docs/api.md, "Acting in another
//! fragment"): an agent's fragment, running its own code, drafts and reads
//! in a room as its agent through its `FRAGMENTS` capability
//! (`this.otherFragments`), and posts there as a job's step
//! (`job.otherFragments`; cell/src/acting.rs). Its agent is made by an
//! assignment to its owner's computer, undone before it joins anything, so
//! no computer is ever woken.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use fragment_proto::limits;
use serde_json::{json, Value};

use super::app::ship;
use super::jobs::{inbox, records, runs, settle, started};
use crate::api::Api;
use crate::{Need, Suite};

/// The agent's own code: `act` (a query) drafts or reads; `send` (a job)
/// posts as a step; `relay` (a job its inbox triggers) posts what it is
/// given.
const APP: &[u8] = include_bytes!("../../fixtures/elsewhere.mjs");
const MANIFEST: &[u8] = include_bytes!("../../fixtures/elsewhere.json");
/// The room: `chat` anyone may post to, `notes` its editors; each `chat`
/// record runs `heard`.
const ROOM_APP: &[u8] = include_bytes!("../../fixtures/heard.mjs");
const ROOM_JSON: &[u8] = include_bytes!("../../fixtures/heard.json");

const WAIT: Duration = Duration::from_secs(30);

/// An agent's fragment running the fixture's code, members only.
struct Agent {
    name: String,
    made: Value,
    identity: String,
    token: String,
}

/// An agent of `owner`'s: its key becomes its agent's identity at its
/// first assignment, undone at once, so no join wakes the computer.
fn agent(s: &Suite, api: &Api, owner: &Keys, base: &str) -> Result<Agent> {
    let name = s.named(api, owner, base)?;
    let made = s.create(api, owner, &name)?;
    ship(s, &made, APP, MANIFEST);
    let token = made["inboxToken"].as_str().context("its inbox token")?.to_string();
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the agent made members-only: {r}");
    let r = api.signed(owner, "POST", "/api/computers", Some(&json!({})))?;
    let computer = r.body["computer"].as_str().with_context(|| format!("a computer: {r}"))?.to_string();
    let r = api.signed(owner, "PUT", &format!("/api/computers/{computer}/agents/{name}"), Some(&json!({})))?;
    let identity = r.body["agents"].as_array().into_iter().flatten().find(|x| x["fragment"] == name.as_str()).and_then(|x| x["identity"].as_str()).unwrap_or("").to_string();
    let r2 = api.signed(owner, "DELETE", &format!("/api/computers/{computer}/agents/{name}"), None)?;
    anyhow::ensure!(fragment_core::npub::is_identity(&identity) && r2.status == 200, "the agent's identity: {r} {r2}");
    Ok(Agent { name, made, identity, token })
}

/// A room of `owner`'s, members only, that `agents` join as viewers.
fn make_room(s: &Suite, api: &Api, owner: &Keys, base: &str, agents: &[&Agent]) -> Result<String> {
    let name = s.named(api, owner, base)?;
    let c = s.create(api, owner, &name)?;
    ship(s, &c, ROOM_APP, ROOM_JSON);
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the room made members-only: {r}");
    for x in agents {
        let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{}", x.identity), Some(&json!({ "role": "viewer" })))?;
        anyhow::ensure!(r.status == 200, "{} joins the room: {r}", x.name);
    }
    Ok(name)
}

/// A draft or a read of `from`'s capability in fragment `name`: its
/// answer, or its refusal.
fn act_in(api: &Api, owner: &Keys, from: &str, act: &str, name: &str, channel: &str, arg: Value) -> Result<Value> {
    let id = format!("{act}-{}", Keys::generate().pubkey_hex());
    let r = api.op(owner, from, "act", &id, json!({ "act": act, "name": name, "channel": channel, "arg": arg }))?;
    anyhow::ensure!(r.status == 200, "act {act}: {r}");
    Ok(r.body["result"].clone())
}

/// A post of `from`'s job's step in fragment `name` (sleeping after, when
/// `sleep`): the job's output, its answer or its refusal.
fn send(api: &Api, owner: &Keys, from: &str, name: &str, channel: &str, record: Value, sleep: bool) -> Result<Value> {
    let id = format!("send-{}", Keys::generate().pubkey_hex());
    let r = api.op(owner, from, "send", &id, json!({ "name": name, "channel": channel, "record": record, "sleep": sleep }))?;
    anyhow::ensure!(r.status == 200, "send: {r}");
    let run = settle(api, owner, from, started(&r), &["succeeded", "held", "blocked"], WAIT);
    anyhow::ensure!(run["status"] == "succeeded", "the send job: {run}");
    Ok(run["output"].clone())
}

/// The room's `heard` runs, newest first, once `want` of them are done.
fn heard(s: &Suite, api: &Api, owner: &Keys, room: &str, want: usize) -> Vec<Value> {
    s.eventually(WAIT, || runs(api, owner, room, "&op=heard").iter().filter(|r| r["status"] != "queued" && r["status"] != "running").count() >= want);
    runs(api, owner, room, "&op=heard")
}

/// Acting:
///
/// - valid: a job's post there names the agent, and its record's runs are
///   one hop deeper than the job; a draft is taken; the records read back;
/// - replay: the same id and body again is that record, another body 409;
///   a job's body that runs again after its post posts nothing again;
/// - invalid: the room checks the agent as it would the agent's own request
///   (a channel its role may not post to, a fragment it is no member of, a
///   `relayed` record no door signed); the capability posts nothing; a
///   fragment that is no agent's acts nowhere, nor does one someone else may
///   edit, nor code put live while someone else could, even once they are
///   gone;
/// - depth: a post from a job at the hop budget runs there; one a hop past
///   it is blocked there, as a chain within one fragment is.
pub fn elsewhere(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("elsewhere", &[Need::Computers, Need::Levers]) {
        return Ok(());
    }
    let owner = api.person()?;
    let x = agent(s, api, &owner, "acts")?;
    let (agent, identity) = (x.name.clone(), x.identity.clone());
    let room = make_room(s, api, &owner, "room", &[&x])?;
    let closed = make_room(s, api, &owner, "closed", &[])?;
    let act = |verb: &str, name: &str, channel: &str, arg: Value| act_in(api, &owner, &agent, verb, name, channel, arg);
    let post = |name: &str, channel: &str, record: Value| send(api, &owner, &agent, name, channel, record, false);

    // valid: a job's post as the agent, its record's run a hop deeper than the job
    let posted = post(&room, "chat", json!({ "id": "hello", "body": { "text": "hello from the agent's code" } }))?;
    let record = &posted["answer"]["record"];
    let run = heard(s, api, &owner, &room, 1);
    s.ok(
        "an agent's fragment posts in a room as its agent, a step of its own job",
        record["principal"] == identity.as_str() && record["body"]["text"] == "hello from the agent's code" && posted["answer"]["replayed"] == false,
        &posted,
    );
    s.ok("its record's run is one hop deeper than the job that posted it", run.len() == 1 && run[0]["depth"] == 1 && run[0]["status"] == "succeeded", json!(run));
    let drafted = act("draft", &room, "chat", json!({ "turn": "t1", "text": "typing" }))?;
    s.ok("it drafts there as its agent", drafted["answer"]["ok"] == true, &drafted);
    let read = act("records", &room, "chat", json!({ "after": 0 }))?;
    let seqs: Vec<&Value> = read["answer"]["records"].as_array().map(|l| l.iter().map(|r| &r["seq"]).collect()).unwrap_or_default();
    s.ok("it reads the room's records back", seqs == [&record["seq"]], &read);

    // replay: the same post is its record; another body under its id is
    // refused; a job's body run again after its post posts nothing again
    let again = post(&room, "chat", json!({ "id": "hello", "body": { "text": "hello from the agent's code" } }))?;
    let other = post(&room, "chat", json!({ "id": "hello", "body": { "text": "something else" } }))?;
    s.ok(
        "the same post again is the same record, appended once; another body under its id is 409",
        again["answer"]["replayed"] == true
            && again["answer"]["record"]["seq"] == record["seq"]
            && other["error"]["message"].as_str().is_some_and(|m| m.starts_with("409"))
            && records(api, &owner, &room, "chat").len() == 1,
        json!({ "again": again, "other": other }),
    );
    let slept = send(api, &owner, &agent, &room, "chat", json!({ "id": "once", "body": { "text": "posted once" } }), true)?;
    let once = records(api, &owner, &room, "chat").iter().filter(|r| r["body"]["text"] == "posted once").count();
    s.ok("a job's post is a step: its body run again after a sleep posts nothing again", slept["answer"]["replayed"] == false && once == 1, json!({ "slept": slept, "once": once }));

    // invalid: checked as the agent's own request would be
    let starts = |v: &Value, status: &str| v["error"]["message"].as_str().is_some_and(|m| m.starts_with(status));
    let notes = post(&room, "notes", json!({ "id": "n1", "body": { "text": "a viewer's note" } }))?;
    s.ok("a channel its role may not post to refuses it (403), as it would the agent", starts(&notes, "403") && records(api, &owner, &room, "notes").is_empty(), &notes);
    let stranger = post(&closed, "chat", json!({ "id": "s1", "body": { "text": "let me in" } }))?;
    s.ok("a fragment it is no member of refuses it", (starts(&stranger, "403") || starts(&stranger, "404")) && records(api, &owner, &closed, "chat").is_empty(), &stranger);
    let relayed = post(&room, "chat", json!({ "id": "r1", "body": { "text": "from the owner", "relayed": { "via": "simplex" } } }))?;
    s.ok("a relayed record no door signed is refused (403), as the agent's own is", starts(&relayed, "403") && records(api, &owner, &room, "chat").len() == 2, &relayed);
    let named = post("no such name", "chat", json!({ "id": "x", "body": {} }))?;
    s.ok("an act names a fragment (400)", starts(&named, "400"), &named);
    let bound = act("post", &room, "chat", json!({ "id": "b1", "body": { "text": "not a step" } }))?;
    s.ok(
        "the capability posts nothing: a post is a job's step, at its run's depth",
        bound["error"]["status"].as_u64().is_some() && bound["error"]["message"].as_str().is_some_and(|m| m.contains("job.otherFragments")) && records(api, &owner, &room, "chat").len() == 2,
        &bound,
    );
    // the same code on a fragment that is no agent's acts nowhere
    let plain = s.named(api, &owner, "plain")?;
    let c = s.create(api, &owner, &plain)?;
    ship(s, &c, APP, MANIFEST);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{plain}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins the plain fragment: {r}");
    let nobody = send(api, &owner, &plain, &room, "chat", json!({ "id": "p1", "body": { "text": "as whom?" } }), false)?;
    s.ok("a fragment that is no agent's acts nowhere (403)", starts(&nobody, "403") && records(api, &owner, &room, "chat").len() == 2, &nobody);
    // someone else who may edit it: its code acts nowhere, even once they are gone
    let editor = api.person()?;
    let r = api.share(&agent, &editor.pubkey_hex(), "editor")?;
    anyhow::ensure!(r.status == 200, "an editor of the agent's fragment: {r}");
    let shared = act("records", &room, "chat", json!({}))?;
    s.commit(&x.made, &[("their.txt", Some(b"their code".as_slice()))]);
    let r = api.signed(&editor, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "the editor deploys: {r}");
    let them = api.identity(&editor)?;
    let r = api.signed(&owner, "DELETE", &format!("/api/f/{agent}/members/{them}"), None)?;
    anyhow::ensure!(r.status == 200, "the editor removed: {r}");
    let theirs = act("records", &room, "chat", json!({}))?;
    s.commit(&x.made, &[("their.txt", Some(b"our code again".as_slice()))]);
    s.deploy(&x.made);
    let ours = act("records", &room, "chat", json!({}))?;
    s.ok(
        "its code acts nowhere while someone else may edit it, nor code they put live once they are gone; deployed again by its owner, it acts",
        shared["error"]["status"] == 403 && theirs["error"]["status"] == 403 && ours["answer"]["records"].as_array().is_some_and(|l| l.len() == 2),
        json!({ "shared": shared, "theirs": theirs, "ours": ours }),
    );

    // depth: a job at the hop budget posts, and its record runs there; one
    // a hop past it posts a record whose run is blocked
    let budget = limits::HOP_DEPTH_MAX;
    let mut relays = vec![];
    for hops in [budget - 1, budget] {
        let r = inbox(api, &agent, &x.token, &json!({ "payload": { "name": room, "id": format!("deep-{hops}"), "body": { "text": format!("from depth {hops}") } } }), Some(hops))?;
        relays.push(settle(api, &owner, &agent, r.body["runs"][0].as_i64().unwrap_or(0), &["succeeded", "held", "blocked"], WAIT));
    }
    // newest first: the relays were one after the other
    let deep = heard(s, api, &owner, &room, 4);
    let (past, at) = (deep.first().cloned().unwrap_or_default(), deep.get(1).cloned().unwrap_or_default());
    s.ok(
        "a job's post carries its run's hop depth: at the budget its record runs there, a hop past it is blocked",
        relays.iter().all(|r| r["status"] == "succeeded") && at["depth"] == budget && at["status"] == "succeeded" && past["depth"] == budget + 1 && past["status"] == "blocked",
        json!({ "relays": relays, "at": at, "past": past }),
    );
    Ok(())
}
