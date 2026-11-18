//! A fragment acting elsewhere (docs/api.md, "Acting in another
//! fragment"): an agent's fragment, running its own code, drafts and reads
//! in a room as its agent through its `FRAGMENTS` capability
//! (`this.otherFragments`), and posts there and follows its channels as a
//! job's steps (`job.otherFragments`; cell/src/acting.rs). Its agent is made by an
//! assignment to its owner's computer, undone before it joins anything, so
//! no computer is ever woken.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use fragment_proto::limits;
use serde_json::{json, Value};

use super::app::ship;
use super::jobs::{events, inbox, records, runs, settle, started};
use crate::api::Api;
use crate::{Need, Suite};

/// The agent's own code: `act` (a query) drafts or reads; `send` (a job)
/// posts as a step; `relay` (a job its inbox triggers) posts what it is
/// given; `follow` (a job) follows a channel as a step; `seen` (its
/// `followed` trigger) keeps each record it hears.
const APP: &[u8] = include_bytes!("../../fixtures/elsewhere.mjs");
const MANIFEST: &[u8] = include_bytes!("../../fixtures/elsewhere.json");
/// The room: `chat` anyone may post to, `notes` its editors; each `chat`
/// record runs `heard`.
const ROOM_APP: &[u8] = include_bytes!("../../fixtures/heard.mjs");
const ROOM_JSON: &[u8] = include_bytes!("../../fixtures/heard.json");
/// The room again, its `notes` its editors' alone to read.
const ROOM_NOTES_EDITORS: &[u8] = br#"{
  "operations": { "heard": { "kind": "mutation" }, "log": { "kind": "query", "role": "public" } },
  "channels": { "chat": { "read": "public", "post": "public" }, "notes": { "read": "editor", "post": "editor" } },
  "triggers": [{ "channel": "chat", "run": "heard" }]
}"#;

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
    room_named(s, api, owner, &name, agents)?;
    Ok(name)
}

/// The room `name` made (a name deleted may be made again), its create answer.
fn room_named(s: &Suite, api: &Api, owner: &Keys, name: &str, agents: &[&Agent]) -> Result<Value> {
    let c = s.create(api, owner, name)?;
    ship(s, &c, ROOM_APP, ROOM_JSON);
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "the room made members-only: {r}");
    for x in agents {
        let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{}", x.identity), Some(&json!({ "role": "viewer" })))?;
        anyhow::ensure!(r.status == 200, "{} joins the room: {r}", x.name);
    }
    Ok(c)
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
    let r = api.share(&agent, editor.pubkey_hex(), "editor")?;
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

/// Follows (docs/api.md, subscriptions' `follow`; cell/src/subscriptions.rs):
/// an agent's fragment follows a room's channel as its agent
/// (`this.otherFragments.follow`), and each record there runs its own
/// `followed` trigger, handed from the room's outbox, never the queue.
///
/// - invalid: a person, a non-member, or a channel there is not, follows
///   nothing;
/// - valid: each follower hears each record once, as a run at the record's
///   depth, never its own agent's;
/// - depth: what one follower posts at the hop budget runs at another
///   follower; a hop past it is blocked there;
/// - replay: an answer lost after the follower took it hands it again, and
///   it runs once;
/// - restart: a node restart while a record waits to be handed again
///   leaves it run once;
/// - a removed member's follow ends, and so does a deleted follower's.
pub fn follows(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("follows", &[Need::Computers, Need::Levers, Need::Node]) {
        return Ok(());
    }
    let owner = api.person()?;
    let (a, b) = (agent(s, api, &owner, "follower-a")?, agent(s, api, &owner, "follower-b")?);
    let room = s.named(api, &owner, "followed")?;
    let room_made = room_named(s, api, &owner, &room, &[&a, &b])?;
    let closed = make_room(s, api, &owner, "unfollowed", &[])?;
    let subs = |api: &Api| -> Vec<Value> {
        api.signed(&owner, "GET", &format!("/api/f/{room}/subscriptions"), None).ok().and_then(|r| r.body["subscriptions"].as_array().cloned()).unwrap_or_default()
    };
    let post = |api: &Api, text: &str| -> Result<()> {
        let r = api.signed(&owner, "POST", &format!("/api/f/{room}/channels/chat"), Some(&json!({ "id": text, "body": { "text": text } })))?;
        anyhow::ensure!(r.status == 200, "the owner's post {text}: {r}");
        Ok(())
    };
    let times = |api: &Api, x: &Agent, text: &str| seen(api, &owner, x).iter().filter(|r| r["text"] == text).count();

    // invalid
    let r = api.signed(&owner, "POST", &format!("/api/f/{room}/subscriptions"), Some(&json!({ "channel": "chat", "follow": true })))?;
    s.ok("a person follows nothing: only an agent's own fragment does (403)", r.status == 403, &r);
    let starts = |v: &Value, status: &str| v["error"]["message"].as_str().is_some_and(|m| m.starts_with(status));
    let stranger = follow_in(api, &owner, &a.name, &closed, "chat")?;
    s.ok("an agent follows nothing in a fragment it is no member of", starts(&stranger, "403") || starts(&stranger, "404"), &stranger);
    let nowhere = follow_in(api, &owner, &a.name, &room, "nosuch")?;
    s.ok("nor a channel there is not (404)", starts(&nowhere, "404"), &nowhere);
    let audit = follow_in(api, &owner, &a.name, &room, "events")?;
    s.ok("nor the audit trail, which is read, never handed on (400)", starts(&audit, "400"), &audit);

    // valid
    let followed = follow_in(api, &owner, &a.name, &room, "chat")?;
    follow_in(api, &owner, &b.name, &room, "chat")?;
    let again = follow_in(api, &owner, &a.name, &room, "chat")?;
    let listed = subs(api);
    let follow_of = |x: &Agent| listed.iter().find(|f| f["follow"] == x.name.as_str()).cloned().unwrap_or_default();
    s.ok(
        "an agent's fragment follows a channel as its agent: the subscription names it and no URL; asked again, the same one",
        followed["answer"]["follow"] == a.name.as_str()
            && again["answer"]["id"] == followed["answer"]["id"]
            && [&a, &b].iter().all(|x| follow_of(x)["principal"] == x.identity.as_str() && follow_of(x)["url"].is_null() && follow_of(x)["channel"] == "chat"),
        json!({ "followed": followed, "again": again, "subscriptions": listed }),
    );
    post(api, "first")?;
    let both = s.eventually(WAIT, || times(api, &a, "first") == 1 && times(api, &b, "first") == 1);
    let run = runs(api, &owner, &a.name, "&op=seen");
    s.ok(
        "each follower hears a record once, as a run of its followed trigger at the record's depth",
        both && run.len() == 1 && run[0]["via"] == "followed" && run[0]["trigger"] == format!("{room}/chat").as_str() && run[0]["depth"] == 1 && run[0]["status"] == "succeeded",
        json!({ "a": seen(api, &owner, &a), "b": seen(api, &owner, &b), "run": run }),
    );
    send(api, &owner, &a.name, &room, "chat", json!({ "id": "own", "body": { "text": "from a" } }), false)?;
    let heard_by_b = s.eventually(WAIT, || seen(api, &owner, &b).iter().any(|r| r["text"] == "from a" && r["principal"] == a.identity.as_str()));
    s.ok("a follower never hears what its own agent posts; another follower does", heard_by_b && times(api, &a, "from a") == 0, json!(seen(api, &owner, &a)));

    // depth: a's job posts at the hop budget, then a hop past it; b's runs
    let budget = limits::HOP_DEPTH_MAX;
    for hops in [budget - 1, budget] {
        let text = format!("from depth {hops}");
        let before = runs(api, &owner, &b.name, "&op=seen").len();
        let r = inbox(api, &a.name, &a.token, &json!({ "payload": { "name": room, "id": format!("deep-{hops}"), "body": { "text": text } } }), Some(hops))?;
        settle(api, &owner, &a.name, r.body["runs"][0].as_i64().unwrap_or(0), &["succeeded", "held", "blocked"], WAIT);
        s.eventually(WAIT, || runs(api, &owner, &b.name, "&op=seen").iter().filter(|r| r["status"] != "queued" && r["status"] != "running").count() > before);
    }
    let deep = runs(api, &owner, &b.name, "&op=seen");
    let (past, at) = (deep.first().cloned().unwrap_or_default(), deep.get(1).cloned().unwrap_or_default());
    s.ok(
        "a follow carries its record's hop depth: at the budget the follower's run runs, a hop past it is blocked",
        at["depth"] == budget && at["status"] == "succeeded" && past["depth"] == budget + 1 && past["status"] == "blocked" && times(api, &b, &format!("from depth {budget}")) == 0,
        json!({ "at": at, "past": past }),
    );

    // replay: the room loses the first follower's answer, and hands the record again
    let lever = |api: &Api, times: u32| api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": room, "op": "fail-deliveries", "times": times })));
    let r = lever(api, 1)?;
    anyhow::ensure!(r.status == 200, "the room's lever: {r}");
    post(api, "second")?;
    let deferred = s.eventually(WAIT, || events(api, &owner, &room).iter().any(|e| e["kind"] == "delivery.deferred"));
    // handed again: one follower's answer lost, then each took it
    let second = seq_of(api, &owner, &room, "second");
    let again = s.eventually(WAIT, || {
        let (to_a, to_b) = (hand_offs(s, &a.name, second), hand_offs(s, &b.name, second));
        to_a.contains(&"up".to_string()) && to_b.contains(&"up".to_string()) && to_a.len() + to_b.len() >= 3 && to_a.iter().chain(&to_b).any(|o| o == "later")
    });
    let both = again && times(api, &a, "second") == 1 && times(api, &b, "second") == 1;
    let counted = |api: &Api, x: &Agent| runs(api, &owner, &x.name, "&op=seen").iter().filter(|r| r["status"] == "succeeded").count() == seen(api, &owner, x).len();
    s.ok(
        "a record handed again after its answer was lost runs once at each follower",
        deferred && both && counted(api, &a) && counted(api, &b),
        json!({ "a": seen(api, &owner, &a), "b": seen(api, &owner, &b) }),
    );

    // restart: the node goes down while the record waits to be handed
    // again (each follower's first three answers lost: about 14 s of tries)
    let r = lever(api, 6)?;
    anyhow::ensure!(r.status == 200, "the room's lever: {r}");
    post(api, "third")?;
    let taken = s.eventually(WAIT, || times(api, &a, "third") + times(api, &b, "third") > 0);
    s.crash()?;
    let restarted = s.start(false)?;
    let api = &restarted;
    // handed again by the restarted node, each follower taking it
    let third = seq_of(api, &owner, &room, "third");
    let handed = s.eventually(Duration::from_secs(90), || [&a, &b].iter().all(|x| hand_offs(s, &x.name, third).contains(&"up".to_string())));
    let both = handed && times(api, &a, "third") == 1 && times(api, &b, "third") == 1;
    s.ok(
        "a node restart while a record waits to be handed again leaves it run once at each follower",
        taken && both && times(api, &a, "third") == 1 && times(api, &b, "third") == 1 && counted(api, &a) && counted(api, &b),
        json!({ "a": seen(api, &owner, &a), "b": seen(api, &owner, &b) }),
    );

    // a follow ends once its agent's role may read the channel no more
    let notes = follow_in(api, &owner, &b.name, &room, "notes")?;
    s.commit(&room_made, &[("fragment.json", Some(ROOM_NOTES_EDITORS))]);
    s.deploy(&room_made);
    let r = api.signed(&owner, "POST", &format!("/api/f/{room}/channels/notes"), Some(&json!({ "id": "note", "body": { "text": "for editors" } })))?;
    anyhow::ensure!(r.status == 200, "the owner's note: {r}");
    let unfollowed = s.eventually(WAIT, || !subs(api).iter().any(|f| f["follow"] == b.name.as_str() && f["channel"] == "notes"));
    s.ok(
        "a follow ends once its agent may read the channel no more, hearing nothing it may not read",
        notes["answer"]["channel"] == "notes" && unfollowed && times(api, &b, "for editors") == 0 && subs(api).iter().any(|f| f["follow"] == b.name.as_str() && f["channel"] == "chat"),
        json!({ "notes": notes, "subscriptions": subs(api) }),
    );

    // a removed member's follow ends with its membership
    let r = api.signed(&owner, "DELETE", &format!("/api/f/{room}/members/{}", a.identity), None)?;
    anyhow::ensure!(r.status == 200, "a removed: {r}");
    let gone = !subs(api).iter().any(|f| f["follow"] == a.name.as_str());
    post(api, "fourth")?;
    let to_b = s.eventually(WAIT, || times(api, &b, "fourth") == 1);
    s.ok("a removed member's follow ends with its membership: it hears nothing more", gone && to_b && times(api, &a, "fourth") == 0, json!(subs(api)));
    // and, all this later, each record ran once
    s.ok(
        "every record a follower heard, it heard once",
        ["first", "second", "third"].iter().all(|t| times(api, &a, t) == 1) && ["first", "second", "third", "fourth"].iter().all(|t| times(api, &b, t) == 1),
        json!({ "a": seen(api, &owner, &a), "b": seen(api, &owner, &b) }),
    );
    // a follower deleted is no follower
    let r = api.signed(&owner, "DELETE", &format!("/api/f/{}", b.name), None)?;
    anyhow::ensure!(r.status == 200, "b deleted: {r}");
    post(api, "fifth")?;
    let ended = s.eventually(WAIT, || !subs(api).iter().any(|f| f["follow"] == b.name.as_str()));
    s.ok("a follower that is gone ends its follow", ended, json!(subs(api)));

    // a room deleted and made again under its name counts its records
    // again: its first record is a new one to its follower
    let again = s.named(api, &owner, "again")?;
    let mut heard_again = vec![];
    for life in ["life one", "life two"] {
        room_named(s, api, &owner, &again, &[&a])?;
        follow_in(api, &owner, &a.name, &again, "chat")?;
        let r = api.signed(&owner, "POST", &format!("/api/f/{again}/channels/chat"), Some(&json!({ "id": "first", "body": { "text": life } })))?;
        anyhow::ensure!(r.status == 200, "the owner's post in {again}: {r}");
        heard_again.push(s.eventually(WAIT, || times(api, &a, life) == 1));
        let r = api.signed(&owner, "DELETE", &format!("/api/f/{again}"), None)?;
        anyhow::ensure!(r.status == 200, "{again} deleted: {r}");
    }
    s.ok("a name deleted and made again is a new life: its first record, seq 1 again, is heard too", heard_again == [true, true], json!(seen(api, &owner, &a)));
    Ok(())
}

/// The seq of the room's `chat` record whose text is `text` (0: none).
fn seq_of(api: &Api, owner: &Keys, room: &str, text: &str) -> i64 {
    records(api, owner, room, "chat").iter().find(|r| r["body"]["text"] == text).and_then(|r| r["seq"].as_i64()).unwrap_or(0)
}

/// How each hand-off of record `seq` to `follower` went, in order, as this
/// node boot's log says (deliveries.rs, `delivery.follow`).
fn hand_offs(s: &Suite, follower: &str, seq: i64) -> Vec<String> {
    let log = s.node_log().unwrap_or_default();
    log.lines()
        .filter(|l| l.contains("delivery.follow"))
        .filter_map(|l| l.find('{').and_then(|at| serde_json::from_str::<Value>(&l[at..]).ok()))
        .filter(|v| v["follower"] == follower && v["seq"] == seq)
        .map(|v| v["outcome"].as_str().unwrap_or("").to_string())
        .collect()
}

/// What an agent's fragment heard of the channels it follows, in order.
fn seen(api: &Api, owner: &Keys, x: &Agent) -> Vec<Value> {
    api.op(owner, &x.name, "seen_log", "q", json!({})).ok().and_then(|r| r.body["result"].as_array().cloned()).unwrap_or_default()
}

/// A follow of `from`'s job's step of channel `channel` of fragment `name`:
/// the job's output, its answer or its refusal.
fn follow_in(api: &Api, owner: &Keys, from: &str, name: &str, channel: &str) -> Result<Value> {
    let id = format!("follow-{}", Keys::generate().pubkey_hex());
    let r = api.op(owner, from, "follow", &id, json!({ "name": name, "channel": channel }))?;
    anyhow::ensure!(r.status == 200, "follow: {r}");
    let run = settle(api, owner, from, started(&r), &["succeeded", "held", "blocked"], WAIT);
    anyhow::ensure!(run["status"] == "succeeded", "the follow job: {run}");
    Ok(run["output"].clone())
}
