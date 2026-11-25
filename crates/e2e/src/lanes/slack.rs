//! The Slack connector (templates/slack), end to end, in the computers lane
//! on its stub agent and Slack's fake (FIN-508): its owner's setup (the bot
//! token and signing secret as its secrets, `connect`), Slack's URL check,
//! the door (its owner binds it to their agent), pairing (a code its owner
//! sends the bot in a DM, confirmed); then
//! - its owner's DM reaches the agent as theirs, in the chat's own
//!   conversation, and the reply goes back to that DM;
//! - a teammate's mention in a channel reaches the agent as someone heard
//!   there, stamped the door's, with the channel's context; their turn is
//!   asked as them, in that room, and the reply goes to the message's
//!   thread, 👀 on it while the agent works;
//! - a teammate's DM is a room of its own;
//! - a delivery not signed by its secret is refused, and one Slack sends
//!   again is taken once.
//!
//! A self-hosted stack allows no Slack (its platform is not on the
//! internet): its door is refused, and the rest skipped.

use std::time::Duration;

use anyhow::Result;
use fragment_fakes::slack::{Slack, BOT_NAME, TEAM_NAME};
use serde_json::{json, Value};

use super::computers::agent_replies;
use super::jobs::records;
use crate::api::{Api, Call, Reply};
use crate::Suite;

const TOKEN: &str = "xoxb-e2e-slack-bot-token";
const SIGNING: &str = "e2e0slack0signing0secret0f1e2d3c";
const PAUL: &str = "U0E2EPAUL";
const MAPLE: &str = "U0E2EMAPLE";
const SAM: &str = "U0E2ESAM";
const DEPLOYS: &str = "C0E2EDEPLOY";

/// Who the checks are of: the lane's owner, their agent and its identity.
pub(super) struct Agent<'a> {
    pub owner: &'a crate::Keys,
    pub owner_id: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    pub wake: Duration,
}

pub(super) fn checks(s: &mut Suite, api: &Api, c: &Agent<'_>) -> Result<()> {
    let slack = Slack::start(TOKEN)?;
    let name = s.named(api, c.owner, "slack")?;
    let made = api.create_with(c.owner, json!({ "name": name, "template": "slack" }))?;
    anyhow::ensure!(made.status == 200, "making the Slack connector: {made}");
    s.owned(&made.body, c.owner);
    if s.on_celld() {
        let r = api.signed(c.owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "link", "agent": c.agent })))?;
        s.ok("a self-hosted stack links no Slack: its owner's binding of the connector as their agent's door is refused, 403, as a connector the deployment does not allow", r.status == 403 && r.text.contains("does not allow"), &r);
        s.skip("the Slack connector end to end", "a self-hosted stack does not allow Slack, whose events need inbound internet (docs/self-host.md)");
        return Ok(());
    }
    let inbox = format!("{}/api/f/{name}/inbox", api.base);
    let hmac = json!({ "secret": "SLACK_SIGNING_SECRET", "header": "x-slack-signature", "prefix": "v0=", "signed": "v0:{x-slack-request-timestamp}:{body}", "fresh": "x-slack-request-timestamp", "within_s": 300 });
    let decl = fragment_core::inbox::HmacDecl::read(&hmac).map_err(anyhow::Error::msg)?;
    // Slack's delivery, signed by `key` as Slack signs one
    let deliver = |payload: &Value, key: &[u8]| -> Result<Reply> {
        let body = payload.to_string().into_bytes();
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs().to_string();
        let sig = decl.sign(key, &|h: &str| (h == "x-slack-request-timestamp").then(|| at.clone()), &body).unwrap_or_default();
        api.call(Call { method: "POST", url: inbox.clone(), body: Some(body), content_type: Some("application/json"), extra: vec![("x-slack-request-timestamp", at), ("x-slack-signature", sig)], ..Call::default() })
    };
    let ran = |r: &Reply| super::jobs::settle(api, c.owner, &name, r.body["runs"][0].as_i64().unwrap_or(0), &["succeeded", "held"], c.wake);
    let state = || api.op(c.owner, &name, "state", "state", json!({})).map(|r| r.body["result"].clone()).unwrap_or_default();

    // its secrets as its owner's page sets them, toward the fake's host
    for (key, value) in [("SLACK_SIGNING_SECRET", SIGNING), ("SLACK_BOT_TOKEN", TOKEN)] {
        let r = api.signed(c.owner, "PUT", &format!("/api/f/{name}/secrets/{key}"), Some(&json!({ "value": value, "hosts": ["127.0.0.1"] })))?;
        anyhow::ensure!(r.status == 200, "its secret {key}: {r}");
    }
    let r = api.op(c.owner, &name, "connect", "connect", json!({ "api": slack.url }))?;
    let connected = s.eventually(c.wake, || state()["bot"] == json!({ "name": BOT_NAME, "team": TEAM_NAME }));
    s.ok("its owner's connect checks the app's bot token and keeps its workspace and bot", r.status == 200 && connected, json!({ "connect": r.body, "state": state() }));

    // Slack's URL check: signed by another secret, refused; by its own,
    // answered with its challenge, and the page told Slack sends here
    let forged = deliver(&Slack::url_check("nope"), b"another secret")?;
    let check = deliver(&Slack::url_check("e2e-challenge-7f3a"), SIGNING.as_bytes())?;
    let verified = s.eventually(c.wake, || state()["verified"] == true);
    s.ok(
        "a delivery not signed by its signing secret is refused; Slack's URL check, signed, is answered with its challenge, and its setup says Slack sends here",
        forged.status == 403 && check.status == 200 && check.text == "e2e-challenge-7f3a" && verified,
        json!({ "forged": forged.status, "check": check.text, "state": state() }),
    );

    // bound as its agent's slack door, by its owner
    let r = api.signed(c.owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "link", "agent": c.agent })))?;
    s.ok("its owner binds it as their agent's slack door", r.status == 200 && r.body["door"] == "slack" && r.body["agent"] == c.agent, &r);

    // its owner pairs their own Slack account: the code, sent in a DM
    slack.person(PAUL, "Paul");
    slack.person(MAPLE, "Maple");
    slack.person(SAM, "Sam");
    let code = api.op(c.owner, &name, "pair", "pair", json!({}))?.body["result"]["code"].as_str().unwrap_or("").to_string();
    let proposed = ran(&deliver(&Slack::dm(PAUL, &format!("pair {code}"), "1790000001.000100"), SIGNING.as_bytes())?);
    let candidate = state()["pairing"]["candidate"].clone();
    let confirmed = api.op(c.owner, &name, "confirm", "confirm", json!({ "user": PAUL }))?;
    s.ok(
        "its owner pairs their Slack account: the code sent in a DM proposes it, and its owner confirms it; the code reaches no agent",
        proposed["output"]["proposed"] == true && candidate == json!({ "user": PAUL, "name": "Paul" }) && confirmed.status == 200 && state()["paired"]["name"] == "Paul" && records(api, c.owner, &name, "chat").is_empty(),
        json!({ "proposed": proposed, "candidate": candidate, "confirmed": confirmed.body }),
    );

    let sent_to = |channel: &str, text: &str| slack.sent().into_iter().find(|m| m["channel"] == channel && m["markdown_text"].as_str().is_some_and(|t| t.contains(text)));
    let chat = || records(api, c.owner, &name, "chat");
    let start_of = |turn: &Value| records(api, c.owner, &name, "work").into_iter().find(|r| r["body"]["kind"] == "turn.start" && r["body"]["turn"] == *turn).unwrap_or_default();

    // its owner's DM: theirs, in the chat's own conversation, answered in that DM
    let run = ran(&deliver(&Slack::dm(PAUL, "hello from slack", "1790000002.000100"), SIGNING.as_bytes())?);
    let rec = chat().into_iter().find(|r| r["body"]["text"] == "hello from slack").unwrap_or_default();
    let answered = s.eventually(c.wake, || sent_to(&format!("D{PAUL}"), "hello from slack").is_some());
    let reply = agent_replies(&chat(), c.identity).into_iter().find(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains("hello from slack"))).unwrap_or_default();
    let start = start_of(&reply["body"]["turn"]);
    s.ok(
        "its owner's DM is published as theirs to their agent, through its slack door, in no room; it is asked as them, and the reply goes back to that DM",
        run["output"]["relayed"] == true && rec["body"]["relayed"] == json!({ "via": "slack", "for": c.owner_id, "agent": c.agent }) && rec["body"].get("room").is_none() && answered && start["body"]["asker"] == c.owner_id && start["body"].get("room").is_none_or(Value::is_null),
        json!({ "run": run, "record": rec, "sent": slack.sent(), "turn.start": start }),
    );

    // a teammate's mention in a channel: someone heard, in its room, with
    // what was said there before; asked as them; answered in its thread
    slack.channel(DEPLOYS, "deploys", vec![json!({ "ts": "1790000003.000100", "user": SAM, "text": "deploy started" })]);
    let asked = "is the deploy green?";
    let mention = Slack::mention(DEPLOYS, MAPLE, asked, "1790000004.000100");
    let run = ran(&deliver(&mention, SIGNING.as_bytes())?);
    let rec = chat().into_iter().find(|r| r["body"]["text"] == asked).unwrap_or_default();
    s.ok(
        "a teammate's mention in a channel is published as someone heard there, stamped the door's, with its room and what was said there just before",
        run["output"]["heard"] == true
            && rec["author"] == "door"
            && rec["body"]["heard"] == json!({ "via": "slack", "from": MAPLE, "name": "Maple" })
            && rec["body"]["room"] == json!({ "id": DEPLOYS, "name": "#deploys", "thread": "1790000004.000100" })
            && rec["body"]["context"] == json!([{ "name": "Sam", "text": "deploy started", "at": 1_790_000_003_000i64 }])
            && rec["body"].get("relayed").is_none(),
        json!({ "run": run, "record": rec }),
    );
    let threaded = s.eventually(c.wake, || sent_to(DEPLOYS, asked).is_some_and(|m| m["thread_ts"] == "1790000004.000100"));
    let reply = agent_replies(&chat(), c.identity).into_iter().find(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains(asked))).unwrap_or_default();
    let start = start_of(&reply["body"]["turn"]);
    let eyes: Vec<Value> = slack.reactions().into_iter().filter(|r| r["timestamp"] == "1790000004.000100").collect();
    s.ok(
        "its agent answers them asked as them (`slack:<user>`), in that room, and the reply goes to the message's thread, 👀 on it until then",
        threaded
            && start["body"]["asker"] == format!("slack:{MAPLE}")
            && start["body"]["room"] == DEPLOYS
            && reply["body"]["room"] == DEPLOYS
            && eyes.iter().map(|r| r["method"].as_str().unwrap_or("")).collect::<Vec<_>>() == ["reactions.add", "reactions.remove"],
        json!({ "sent": slack.sent(), "reply": reply, "turn.start": start, "reactions": eyes }),
    );
    let twice = ran(&deliver(&mention, SIGNING.as_bytes())?);
    s.ok("a delivery Slack sends again is taken once", twice["output"]["ignored"] == "taken already", &twice);

    // a teammate's DM: a room of its own
    let run = ran(&deliver(&Slack::dm(MAPLE, "a quiet question", "1790000005.000100"), SIGNING.as_bytes())?);
    let rec = chat().into_iter().find(|r| r["body"]["text"] == "a quiet question").unwrap_or_default();
    let answered = s.eventually(c.wake, || sent_to(&format!("D{MAPLE}"), "a quiet question").is_some());
    s.ok(
        "a teammate's DM is heard in a room of its own, and answered in that DM",
        run["output"]["heard"] == true && rec["body"]["room"] == json!({ "id": format!("D{MAPLE}"), "name": "DM" }) && answered,
        json!({ "run": run, "record": rec, "sent": slack.sent() }),
    );
    Ok(())
}
