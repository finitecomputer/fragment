//! Fragments an org owns (docs/cloudflare-v1.md, decisions 66 to 71;
//! docs/api.md, An org's fragments), at their public routes on the fakes.
//! An org's content admin makes one, its editor; the org owns and pays for
//! it: its own AI steps and its hosting bill the org's ledger, never its
//! people's, and every AI step there meets its cap and the org's budget,
//! its admins' too. Its content admins administer who is in (one may give
//! themselves access, which its events show), a member who is not one does
//! not, and no agent does. An offboarding fences a member at once, at
//! every request, before anything is cleaned up, and at what reaches them
//! unasked: deliveries, their list, their search, their live sockets (a
//! silent one too, and one held through their agent's membership) and an
//! agent's post as an agent; an invite from before it
//! admits no one, and a new one admits them afresh. A batch
//! whose acknowledgement is lost, sent again after a restart, bills the
//! org once. It hands out no storage token, and its admin deletes it.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::jobs::{settle, started};
use super::ledger::{charged, entries, lever};
use crate::api::{Api, Call, Reply, Socket};
use crate::{Need, Suite};

/// How long a run, a meter's flush or a list's row may take here.
const WAIT: Duration = Duration::from_secs(40);

pub fn org_fragments(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("org-fragments", &[Need::Fakes]) {
        return Ok(());
    }
    // an org of an operator's comp, its first content admin its recovery
    // (no one pays for a comp), funded by an operator, its budget set
    let ada = api.person()?;
    let ada_id = api.identity(&ada)?;
    let comped = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&ada), "kind": "seat" })))?;
    let org = comped.body["org"]["id"].as_str().context("ada's comped org")?.to_string();
    let root = format!("/api/orgs/{org}");
    let ledger = format!("org:{org}");
    let offer = api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/content-admin-offers"), Some(&json!({ "id": "first", "email": Api::email_of(&ada) })))?;
    let took = api.signed(&ada, "POST", &format!("{root}/content-admin-offers/{}/accept", offer.body["id"].as_str().unwrap_or("")), Some(&json!({})))?;
    // the fleet's operator, a person who holds its key (as the ledger lane
    // makes them; approving it again changes nothing)
    let op_session = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&op_session, &s.operator);
    let op = api.identity(&s.operator)?;
    let grant = api.signed(&s.operator, "POST", &format!("/api/ledger/{ledger}/grant"), Some(&json!({ "id": "fund", "micros": 20_000_000, "by": op, "why": "an org's fragments" })))?;
    let budget = api.signed(&s.operator, "POST", &format!("/api/ledger/{ledger}/budget"), Some(&json!({ "id": "b1", "micros": 10_000_000 })))?;
    anyhow::ensure!(took.status == 200 && grant.status == 200 && budget.status == 200, "the org's admin, credit and budget: {took}; {grant}; {budget}");
    let org_ledger = |api: &Api| api.signed(&ada, "GET", &format!("{root}/ledger"), None).map(|r| r.body).unwrap_or(Value::Null);

    // org_usage_charges_org
    let made = api.signed(&ada, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgcal"), "template": "calories" })))?;
    let name = made.body["name"].as_str().context("the org's fragment")?.to_string();
    let status = api.signed(&ada, "GET", &format!("/api/f/{name}/status"), None)?;
    let r = api.op(&ada, &name, "summarize", "s1", json!({}))?;
    let run = settle(api, &ada, &name, started(&r), &["succeeded", "held"], WAIT);
    let billed = s.eventually(WAIT, || entries(api, &ledger, &format!("step:{name}@")).iter().any(|e| charged(e).is_some_and(|c| c > 0)));
    let personal = entries(api, &ada_id, &format!("step:{name}@"));
    let listed = s.eventually(WAIT, || {
        api.signed(&ada, "GET", &format!("{root}/fragments"), None).is_ok_and(|r| r.body["fragments"].as_array().is_some_and(|f| f.iter().any(|x| x["name"] == name.as_str() && x["role"] == "owner")))
    });
    s.ok(
        "org_usage_charges_org: its content admin makes the org's fragment, its editor; the org owns it (its inventory lists it), and its own AI step bills the org's ledger, never theirs",
        made.status == 200
            && made.body["owner"] == format!("org:{org}").as_str()
            && status.body["ownerRef"] == json!({ "kind": "org", "id": org })
            && status.body["role"] == "editor"
            && status.body["visibility"] == "members"
            && run["status"] == "succeeded"
            && billed
            && personal.is_empty()
            && listed
            && org_ledger(api)["spentMicros"].as_i64().is_some_and(|s| s > 0),
        json!({ "made": made.body, "run": run, "org": org_ledger(api) }),
    );

    // who may make one, and the raw git it never hands out
    let ben = api.person()?;
    let ben_id = api.identity(&ben)?;
    let hand = Keys::generate();
    let reg = "/api/identities";
    let agent = api.signed(&ada, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hand, "POST", reg, &ada) })))?;
    anyhow::ensure!(agent.status == 200, "ada's agent: {agent}");
    let refused = [
        ("someone in no org's content makes none (403)", api.signed(&ben, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgx") })))?, 403),
        ("nor does its admin's agent (403)", api.signed(&hand, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgy") })))?, 403),
        ("nor lists its inventory (403)", api.signed(&ben, "GET", &format!("{root}/fragments"), None)?, 403),
        ("an agent fragment is a person's (400)", api.signed(&ada, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgagent"), "template": "agent" })))?, 400),
        ("its editor gets no storage token: an offboarding could not take one back (403)", api.signed(&ada, "GET", &format!("/api/f/{name}/storage-token"), None)?, 403),
        ("its cap is no one's to change: its admin's neither (403)", api.signed(&ada, "PUT", &format!("/api/f/{name}/cap"), Some(&json!({ "id": "c1", "micros": 50_000_000 })))?, 403),
        ("a fragment never made has no owner to read: its channels are 404, not 500", api.signed(&ada, "GET", &format!("/api/f/{}/channels", s.named(api, &ada, "nevermade")?), None)?, 404),
    ];
    let all = refused.iter().all(|(_, r, status)| r.status == *status);
    s.ok("and refused: each says why", all, json!(refused.iter().map(|(w, r, _): &(&str, Reply, u16)| json!([w, r.status, r.body])).collect::<Vec<_>>()));

    // a content admin who is no member administers it, and may give themselves access
    let to_ben = api.signed(&ada, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "ben", "email": Api::email_of(&ben) })))?;
    api.signed(&ben, "POST", &format!("{root}/content-admin-offers/{}/accept", to_ben.body["id"].as_str().unwrap_or("")), Some(&json!({})))?;
    let unseen = api.signed(&ben, "GET", &format!("/api/f/{name}/status"), None)?;
    let members = api.signed(&ben, "GET", &format!("/api/f/{name}/members"), None)?;
    let joined = api.signed(&ben, "PUT", &format!("/api/f/{name}/members/{ben_id}"), Some(&json!({ "role": "viewer" })))?;
    let seen = api.signed(&ben, "GET", &format!("/api/f/{name}/status"), None)?;
    let events = api.signed(&ben, "GET", &format!("/api/f/{name}/events?since=0"), None)?;
    let shown = events.body["events"].as_array().into_iter().flatten().any(|e| e["kind"] == "member.set" && e["data"]["by"] == ben_id.as_str());
    s.ok(
        "a content admin who is no member reads none of it (403) but lists its members, and gives themselves access, which its events show",
        unseen.status == 403 && members.status == 200 && joined.status == 200 && seen.status == 200 && seen.body["role"] == "viewer" && shown,
        json!([unseen.body, joined.body, seen.body["role"]]),
    );

    // a member is fenced the moment they are offboarded
    let cal = api.person()?;
    let cal_id = api.identity(&cal)?;
    let invite = |name: &str, who: &Keys| api.signed(&ada, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": Api::email_of(who), "role": "editor" })));
    let accept = |name: &str, who: &Keys| api.signed(who, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})));
    invite(&name, &cal)?;
    let cal_in = accept(&name, &cal)?;
    let cal_reads = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    let cal_writes = api.op(&cal, &name, "log_food", "f1", json!({ "who": "cal", "food": "an apple", "calories": 95 }))?;
    let not_admin = invite(&name, &ben).and_then(|_| api.signed(&cal, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": "x@e2e.test", "role": "viewer" }))))?;
    // what reaches a member without their asking (Alex's review): a channel
    // listed on its members' rows, and a webhook of theirs on its replies,
    // each seen to work while they are in
    let mut manifest: Value = serde_json::from_str(include_str!("../../../../templates/calories/fragment.json"))?;
    manifest["channels"]["notes"] = json!({ "read": "viewer", "post": "viewer", "listed": { "kinds": ["note"], "fields": ["text"] } });
    // and a channel every member reads and none lists: its messages reach
    // its people's searches, and no list row is sent for them
    manifest["channels"]["talk"] = json!({ "read": "viewer", "post": "viewer" });
    manifest["operations"]["notify"] = json!({
        "kind": "mutation", "role": "editor", "description": "Pushes a title to the browsers subscribed under a tag (`*`: all).",
        "input": { "type": "object", "required": ["title", "who"], "additionalProperties": false, "properties": { "title": { "type": "string", "maxLength": 100 }, "who": { "type": "string", "maxLength": 100 } } }
    });
    let app = include_str!("../../../../templates/calories/app.mjs").replacen(
        "export class App extends DurableObject {\n",
        "export class App extends DurableObject {\n  notify({ title, who }, call) {\n    call.push(who, { title });\n    return { ok: true };\n  }\n\n",
        1,
    );
    let files = json!([{ "path": "fragment.json", "text": serde_json::to_string_pretty(&manifest)? }, { "path": "app.mjs", "text": app }]);
    let wrote = api.signed(&ada, "POST", &format!("/api/f/{name}/files"), Some(&json!({ "key": "listed-notes", "message": "a listed channel", "files": files })))?;
    let deployed = api.signed(&ada, "POST", &format!("/api/f/{name}/deploy"), Some(&json!({})))?;
    let note = |id: &str, text: &str| api.signed(&ada, "POST", &format!("/api/f/{name}/channels/notes"), Some(&json!({ "id": id, "body": { "kind": "note", "text": text } })));
    let lists = |who: &Keys, text: &str| super::posts::listed_of(api, who, &name).iter().any(|(_, _, _, f)| f["text"] == text);
    let say = |id: &str, text: &str| api.signed(&ada, "POST", &format!("/api/f/{name}/channels/talk"), Some(&json!({ "id": id, "body": { "text": text } })));
    let finds = |who: &Keys, word: &str| {
        api.signed(who, "GET", &format!("/api/search?q={word}"), None)
            .is_ok_and(|r| r.body["messages"].as_array().is_some_and(|m| m.iter().any(|x| x["fragment"] == name.as_str())))
    };
    let hook = format!("{}/notify", s.push.url);
    let subs = [&cal, &ada].map(|k| api.signed(k, "POST", &format!("/api/f/{name}/subscriptions"), Some(&json!({ "channel": "replies", "url": hook }))));
    let replies = |s: &Suite| s.push.notified().iter().filter(|f| f["fragment"] == name.as_str() && f["channel"] == "replies").count();
    // held on the queue (Alex's review): a webhook of theirs and browsers
    // of theirs (an arbitrary tag, and their identity's) that fail while
    // they are in, so what is queued to them waits for a retry; an active
    // member's browsers under the same tags; and one of theirs under a tag
    // nothing pushes to until their row from before is cleaned up
    s.push.fail_hook("cal-held", 1000);
    let held = api.signed(&cal, "POST", &format!("/api/f/{name}/subscriptions"), Some(&json!({ "channel": "replies", "url": format!("{}/notify/cal-held", s.push.url) })))?;
    let browser = |who: &Keys, id: &str, tag: &str, seed: u8| {
        let sub = s.push.subscribe(id, seed);
        let body = json!({ "who": tag, "endpoint": sub.endpoint, "p256dh": sub.p256dh, "auth": sub.auth });
        api.call(Call { method: "POST", url: api.site_url(&name, "__push-sub"), body: Some(body.to_string().into_bytes()), content_type: Some("application/json"), keys: Some(who), ..Call::default() })
    };
    let browsers = [
        browser(&cal, "cal-tagged", "updates", 21)?,
        browser(&cal, "cal-self", &cal_id, 22)?,
        browser(&ada, "ada-tagged", "updates", 23)?,
        browser(&cal, "cal-later", "later", 24)?,
        browser(&ada, "ada-later", "later", 25)?,
    ];
    s.push.fail("cal-tagged", 1000);
    s.push.fail("cal-self", 1000);
    let heard = replies(s);
    let noted = note("n1", "before they left")?;
    let r = api.op(&ada, &name, "summarize", "s-in", json!({}))?;
    settle(api, &ada, &name, started(&r), &["succeeded", "held"], WAIT);
    let both_heard = s.eventually(WAIT, || replies(s) == heard + 2);
    let listed_in = s.eventually(WAIT, || lists(&cal, "before they left"));
    let said = say("t1", "zebra crossing while in")?;
    let searched_in = s.eventually(WAIT, || finds(&cal, "zebra"));
    let pushed = [("p-tagged", "updates"), ("p-self", cal_id.as_str())].map(|(id, who)| api.op(&ada, &name, "notify", id, json!({ "title": "while in", "who": who })));
    let active_heard = s.eventually(WAIT, || s.push.received("ada-tagged").iter().any(|p| p["title"] == "while in"));
    s.ok(
        "while in, a member's list carries a listed channel's newest note, their search finds a message, and their webhook hears its replies (beside its admin's); their failing webhook and browsers hold what is queued to them, while an active member's browser hears the push",
        wrote.status == 200 && deployed.status == 200 && noted.status == 200 && subs.iter().all(|r| r.as_ref().is_ok_and(|r| r.status == 200)) && both_heard && listed_in
            && said.status == 200 && searched_in
            && held.status == 200 && browsers.iter().all(|r| r.status == 200) && pushed.iter().all(|r| r.as_ref().is_ok_and(|r| r.status == 200)) && active_heard
            && s.push.hooked("cal-held").is_empty() && s.push.received("cal-tagged").is_empty() && s.push.received("cal-self").is_empty(),
        json!([wrote.body, deployed.body, noted.body, replies(s) - heard, browsers.iter().map(|r| r.status).collect::<Vec<_>>()]),
    );
    // live sockets (the audit), each on its `talk`: a member's own; a
    // content admin's who is no member, reading through their agent's
    // membership alone; and an active member's
    let hal = api.person()?;
    let hal_id = api.identity(&hal)?;
    let to_hal = api.signed(&ada, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "hal", "email": Api::email_of(&hal) })))?;
    let hal_admin = api.signed(&hal, "POST", &format!("{root}/content-admin-offers/{}/accept", to_hal.body["id"].as_str().unwrap_or("")), Some(&json!({})))?;
    let hal_hand = Keys::generate();
    let hal_made = api.signed(&hal, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hal_hand, "POST", reg, &hal) })))?;
    let hal_hand_id = api.identity(&hal_hand)?;
    let hal_agent_in = api.signed(&hal, "PUT", &format!("/api/f/{name}/members/{hal_hand_id}"), Some(&json!({ "role": "viewer" })))?;
    let hal_reads = api.signed(&hal, "GET", &format!("/api/f/{name}/status"), None)?;
    let live_on = |who: &Keys| -> Result<(Socket, Value)> {
        let mut sock = Socket::open(api, &name, "__live", Some(who), None)?;
        let hello = sock.until("hello", 5)?;
        sock.send(&json!({ "type": "subscribe", "channel": "talk", "after": 0 }))?;
        sock.until("subscribed", 50)?;
        sock.patience(Duration::from_secs(15))?;
        Ok((sock, hello))
    };
    let (mut cal_live, _) = live_on(&cal)?;
    let (mut hal_live, hal_hello) = live_on(&hal)?;
    let (mut ada_live, _) = live_on(&ada)?;
    let said_live = say("t-live", "heard live while in")?;
    let live_heard = [&mut cal_live, &mut hal_live, &mut ada_live].map(|sock| sock.until_where("record", 20, |r| r["body"]["text"] == "heard live while in").is_ok());
    s.ok(
        "while in, a member's socket, and a content admin's that reads through their agent's membership alone, hear its new message live, beside an active member's",
        hal_admin.status == 200 && hal_made.status == 200 && hal_agent_in.status == 200 && hal_reads.status == 200 && hal_reads.body["role"] == "viewer"
            && hal_hello["role"] == "viewer" && said_live.status == 200 && live_heard.iter().all(|h| *h),
        json!({ "halAgent": hal_agent_in.body, "halReads": hal_reads.body["role"], "heard": live_heard }),
    );
    // a second fragment of the org's, an invite to it waiting as they leave
    let made2 = api.signed(&ada, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgnotes"), "template": "calories" })))?;
    let name2 = made2.body["name"].as_str().context("the org's second fragment")?.to_string();
    invite(&name2, &cal)?;
    let place = |who: &str| api.signed(&ada, "GET", &format!("{root}/content"), None).ok().and_then(|r| r.body["people"].as_array().and_then(|p| p.iter().find(|x| x["person"] == who).cloned())).unwrap_or(Value::Null);
    let before = place(&cal_id);
    let out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "cal-1", "person": cal_id })))?;
    let fenced_read = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    let fenced_write = api.op(&cal, &name, "log_food", "f2", json!({ "who": "cal", "food": "a pear", "calories": 100 }))?;
    let fenced_sub = api.signed(&cal, "POST", &format!("/api/f/{name}/subscriptions"), Some(&json!({ "channel": "replies", "url": "https://example.com/hook" })))?;
    let still_listed = api.signed(&ada, "GET", &format!("/api/f/{name}/members"), None)?.body["members"].as_array().is_some_and(|m| m.iter().any(|x| x["principal"] == cal_id.as_str()));
    s.ok(
        "an org's member is fenced at every request the moment they are offboarded, before anything is cleaned up (their row is still there): no read, no write, no subscription (403); a member who is no admin invites no one (403)",
        cal_in.status == 200 && cal_reads.status == 200 && cal_writes.status == 200 && not_admin.status == 403 && before["person"] == cal_id.as_str()
            && out.status == 200 && place(&cal_id).is_null()
            && fenced_read.status == 403 && fenced_write.status == 403 && fenced_sub.status == 403 && still_listed,
        json!([cal_in.body, out.body, fenced_read.body, fenced_write.body, fenced_sub.body]),
    );
    // nor do their sockets go on (the audit): offboarded, with every
    // socket's identity check due, the admin's agent-only socket is closed
    // at its next frame (it holds less than it opened with), and a member's
    // socket that sends nothing is closed (4001) before a new message
    // reaches it; an active member reconnects and reads it, they do not
    let hal_out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "hal-1", "person": hal_id })))?;
    let aged = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "age-live", "ms": fragment_proto::limits::LIVE_IDENTITY_MS })))?;
    let closed_by = |sock: &mut Socket| -> (Vec<Value>, String) {
        let mut texts = vec![];
        loop {
            match sock.next() {
                Ok(frame) => texts.push(frame["body"]["text"].clone()),
                Err(e) => break (texts, e.to_string()),
            }
        }
    };
    hal_live.send(&json!({ "type": "ping" }))?;
    let (hal_frames, hal_closed) = closed_by(&mut hal_live);
    let said_after = say("t-out", "said after they left")?;
    let (cal_frames, cal_closed) = closed_by(&mut cal_live);
    let (_, ada_closed) = closed_by(&mut ada_live);
    let ada_back = Socket::open(api, &name, "__live", Some(&ada), None).and_then(|mut sock| {
        sock.until("hello", 5)?;
        sock.send(&json!({ "type": "subscribe", "channel": "talk", "after": 0 }))?;
        Ok(sock.until_where("record", 50, |r| r["body"]["text"] == "said after they left").is_ok())
    });
    let cal_back = Socket::open(api, &name, "__live", Some(&cal), None).map(|_| "opened".to_string()).unwrap_or_else(|e| format!("{e:#}"));
    let after = json!("said after they left");
    s.ok(
        "offboarded, their sockets go no further: the one through their agent's membership closes at its next frame (4001), a silent one closes (4001) before the new message reaches it, and they cannot reconnect (403), while an active member reconnects and reads it",
        hal_out.status == 200 && aged.status == 200 && hal_closed.contains("4001")
            && said_after.status == 200 && cal_closed.contains("4001") && !cal_frames.contains(&after) && !hal_frames.contains(&after)
            && ada_closed.contains("4001") && ada_back.as_ref().is_ok_and(|b| *b) && cal_back.contains("403"),
        json!({ "hal": [hal_closed, hal_frames], "cal": [cal_closed, cal_frames], "ada": ada_closed, "adaBack": format!("{ada_back:?}"), "calBack": cal_back }),
    );
    // nor does their search take its new messages (a channel no list row
    // carries, so nothing else is sent them): its admin's search finds the
    // new one, theirs does not, and their row of it goes, with what their
    // search held of it
    let said = say("t2", "quokka sighted after they left")?;
    let admin_finds = s.eventually(WAIT, || finds(&ada, "quokka"));
    let row_gone = s.eventually(WAIT, || super::posts::row_of(api, &cal, &name).is_null());
    s.ok(
        "offboarded, a member's search takes none of its new messages, though its admin's does: their row goes, and their search finds it no more",
        said.status == 200 && admin_finds && !finds(&cal, "quokka") && row_gone && !finds(&cal, "zebra"),
        json!({ "said": said.body, "adminFinds": admin_finds, "rowGone": row_gone }),
    );
    // what was held on the queue for them, retried once their endpoints
    // answer, is refused before it is sent: each such subscription ends,
    // its event says so, and nothing reaches them (Alex's review)
    let fenced_events = || api.signed(&ada, "GET", &format!("/api/f/{name}/events?tail=200"), None).map(|r| r.text.matches("\"subscription.fenced\"").count()).unwrap_or(0);
    s.push.fail_hook("cal-held", 0);
    s.push.fail("cal-tagged", 0);
    s.push.fail("cal-self", 0);
    let refused = s.eventually(WAIT, || fenced_events() >= 3);
    s.ok(
        "offboarded, what was held on the queue for them (a record to their webhook, a push to their browsers under a tag and under their identity) is refused at its retry: nothing reaches them, each subscription ended",
        refused && s.push.hooked("cal-held").is_empty() && s.push.received("cal-tagged").is_empty() && s.push.received("cal-self").is_empty(),
        json!({ "fenced": fenced_events(), "hook": s.push.hooked("cal-held"), "tagged": s.push.received("cal-tagged"), "self": s.push.received("cal-self") }),
    );
    // nor by what reaches them unasked: their webhook from before hears no
    // new reply (its admin's does, in the same batch), and their list takes
    // no new listed note, its row of the fragment gone (its admin's has it)
    let heard = replies(s);
    let r = api.op(&ada, &name, "summarize", "s-out", json!({}))?;
    let ran = settle(api, &ada, &name, started(&r), &["succeeded", "held"], WAIT);
    let admins_heard = s.eventually(WAIT, || replies(s) > heard);
    let noted = note("n2", "after they left")?;
    let admin_lists = s.eventually(WAIT, || lists(&ada, "after they left"));
    let cal_row = super::posts::row_of(api, &cal, &name);
    s.ok(
        "offboarded, a member's webhook from before hears none of its new replies though its admin's does, and their list takes no new listed note: its row of the fragment goes",
        ran["status"] == "succeeded" && admins_heard && replies(s) == heard + 1 && noted.status == 200 && admin_lists && !lists(&cal, "after they left") && cal_row.is_null(),
        json!({ "heard": replies(s) - heard, "calRow": cal_row, "run": ran["status"] }),
    );

    // an invite from before they left admits no one; a new one admits them afresh
    let stale = accept(&name2, &cal)?;
    let restarted;
    let api = if s.runs("org-fragments", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    let held = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    // invited back with nothing cleaned up first: their row from before is
    // no grant, so the invite is theirs to accept (Alex's review)
    let renewed = invite(&name, &cal)?;
    // their row from before cleaned up by that invite, a push to a tag a
    // browser of theirs from before is under reaches the active member's
    // browser alone: a missing row is no grant either
    let later = api.op(&ada, &name, "notify", "p-later", json!({ "title": "after the cleanup", "who": "later" }))?;
    let later_heard = s.eventually(WAIT, || s.push.received("ada-later").iter().any(|p| p["title"] == "after the cleanup"));
    let cal_later = s.push.received("cal-later");
    let back = accept(&name, &cal)?;
    let reads = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    // back in by that one, the invite from before they left still admits no one
    let stale_again = accept(&name2, &cal)?;
    let in_name2 = api.signed(&ada, "GET", &format!("/api/f/{name2}/members"), None)?.body["members"].as_array().is_some_and(|m| m.iter().any(|x| x["principal"] == cal_id.as_str()));
    s.ok(
        "offboard_fence_survives_restart: an invite from before they left admits them no more (403); after a restart they are still fenced; a new invite, with their old row still there, admits them afresh; back in, the invite from before still admits no one",
        stale.status == 403 && held.status == 403 && renewed.status == 200 && back.status == 200 && reads.status == 200 && !place(&cal_id).is_null()
            && stale_again.status != 200 && !in_name2
            && later.status == 200 && later_heard && !cal_later.iter().any(|p| p["title"] == "after the cleanup"),
        json!([stale.body, held.body, renewed.body, back.body, reads.body["role"], [stale_again.status, stale_again.body.clone()]]),
    );

    // a returning admin's own add, nothing cleaned up first: in afresh
    let ben_out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "ben-1", "person": ben_id })))?;
    let ben_fenced = api.signed(&ben, "GET", &format!("/api/f/{name}/status"), None)?;
    let to_ben = api.signed(&ada, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "ben-2", "email": Api::email_of(&ben) })))?;
    let ben_back = api.signed(&ben, "POST", &format!("{root}/content-admin-offers/{}/accept", to_ben.body["id"].as_str().unwrap_or("")), Some(&json!({})))?;
    // back in, his row from before opens nothing (a rehire starts fresh: Paul, 2026-10-31)
    let ben_fresh = api.signed(&ben, "GET", &format!("/api/f/{name}/status"), None)?;
    let regrant = api.signed(&ben, "PUT", &format!("/api/f/{name}/members/{ben_id}"), Some(&json!({ "role": "viewer" })))?;
    let ben_reads = api.signed(&ben, "GET", &format!("/api/f/{name}/status"), None)?;
    s.ok(
        "a content admin offboarded and appointed again starts fresh: their row from before opens nothing (403) until they give themselves access again, then they read it",
        ben_out.status == 200 && ben_fenced.status == 403 && ben_back.status == 200 && ben_fresh.status == 403 && regrant.status == 200 && ben_reads.status == 200 && place(&ben_id)["admin"] == true,
        json!([ben_out.body, ben_fenced.status, ben_back.body, regrant.body, ben_reads.status]),
    );

    // a rehire's new agent inherits nothing from before (Alex's review): in
    // A, offboarded, back in the org by a fresh invite to B, she makes an
    // agent while A is still on her list; neither it nor she opens A. An
    // active member's new agent comes in beside them, as before
    let eve = api.person()?;
    let eve_id = api.identity(&eve)?;
    invite(&name, &eve)?;
    let eve_in = accept(&name, &eve)?;
    let eve_out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "eve-1", "person": eve_id })))?;
    invite(&name2, &eve)?;
    let eve_back = accept(&name2, &eve)?;
    let still_listed = !super::posts::row_of(api, &eve, &name).is_null();
    let reg = "/api/identities";
    let eve_hand = Keys::generate();
    let eve_made = api.signed(&eve, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&eve_hand, "POST", reg, &eve) })))?;
    let agent_in_a = api.signed(&eve_hand, "GET", &format!("/api/f/{name}/status"), None)?;
    let eve_in_a = api.signed(&eve, "GET", &format!("/api/f/{name}/status"), None)?;
    let eve_in_b = api.signed(&eve, "GET", &format!("/api/f/{name2}/status"), None)?;
    let ada_hand = Keys::generate();
    let ada_made = api.signed(&ada, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&ada_hand, "POST", reg, &ada) })))?;
    let ada_agent_in_a = api.signed(&ada_hand, "GET", &format!("/api/f/{name}/status"), None)?;
    s.ok(
        "a rehire's new agent inherits nothing of a membership from before: neither it nor its person opens the old fragment (403), though it was still on her list; an active member's new agent comes in beside them",
        eve_in.status == 200 && eve_out.status == 200 && eve_back.status == 200 && still_listed && eve_made.status == 200
            && agent_in_a.status == 403 && eve_in_a.status == 403 && eve_in_b.status == 200 && ada_made.status == 200 && ada_agent_in_a.status == 200,
        json!({ "agentInA": [agent_in_a.status, agent_in_a.body.clone()], "eveInA": eve_in_a.status, "eveInB": eve_in_b.status, "adaAgentInA": [ada_agent_in_a.status, ada_agent_in_a.body.clone()], "listed": still_listed }),
    );

    // an offboarded agent's row from before posts as no agent (the audit):
    // a public org fragment, made with its maker's own agent, whose agents'
    // channel anyone may post to but only its agent members as agents
    let ada_hand_id = api.identity(&ada_hand)?;
    let made3 = api.signed(&ada, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgpub"), "visibility": "public", "agent": ada_hand_id })))?;
    let name3 = made3.body["name"].as_str().context("the org's public fragment")?.to_string();
    let files = json!([{ "path": "fragment.json", "text": r#"{"channels":{"work":{"read":"public","post":"public","from":"agent"}}}"# }]);
    let wrote3 = api.signed(&ada, "POST", &format!("/api/f/{name3}/files"), Some(&json!({ "key": "agents-channel", "message": "an agents' channel", "files": files })))?;
    let deployed3 = api.signed(&ada, "POST", &format!("/api/f/{name3}/deploy"), Some(&json!({})))?;
    let gus = api.person()?;
    let gus_id = api.identity(&gus)?;
    invite(&name3, &gus)?;
    let gus_in = accept(&name3, &gus)?;
    let gus_listed = s.eventually(WAIT, || !super::posts::row_of(api, &gus, &name3).is_null());
    let gus_hand = Keys::generate();
    let gus_made = api.signed(&gus, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&gus_hand, "POST", reg, &gus) })))?;
    let step = |who: &Keys, id: &str| api.signed(who, "POST", &format!("/api/f/{name3}/channels/work"), Some(&json!({ "id": id, "body": { "kind": "turn.step", "turn": "t1", "text": "a step" } })));
    let gus_posts = step(&gus_hand, "g1")?;
    let gus_out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "gus-1", "person": gus_id })))?;
    let gus_after = step(&gus_hand, "g2")?;
    let ada_agent_posts = step(&ada_hand, "a1")?;
    s.ok(
        "an org's public fragment's agents' channel takes a member's agent's post while its person is in; offboarded, that agent's row from before makes it no agent member there (403), while its maker's own agent, made a member with it, still posts",
        made3.status == 200 && wrote3.status == 200 && deployed3.status == 200 && gus_in.status == 200 && gus_listed && gus_made.status == 200
            && gus_posts.status == 200 && gus_out.status == 200 && gus_after.status == 403 && gus_after.message().contains("agent members only") && ada_agent_posts.status == 200,
        json!({ "made": made3.body, "gusPosts": gus_posts.body, "gusAfter": [gus_after.status, gus_after.body.clone()], "adaAgent": [ada_agent_posts.status, ada_agent_posts.body.clone()] }),
    );

    // org_admin_cannot_bypass_cap: its budget stops its admin's AI steps too
    let tight = api.signed(&s.operator, "POST", &format!("/api/ledger/{ledger}/budget"), Some(&json!({ "id": "b2", "micros": 1 })))?;
    let _ = api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": ledger, "op": "forget-gate" })));
    let r = api.op(&ada, &name, "summarize", "s2", json!({}))?;
    let run = settle(api, &ada, &name, started(&r), &["succeeded", "held"], WAIT);
    s.ok(
        "org_admin_cannot_bypass_cap: the org's budget spent, its own admin's AI step is held, charged nothing, saying why; no one sets its fragment's cap",
        tight.status == 200 && run["status"] == "held" && run["error"].as_str().is_some_and(|e| e.contains("budget")),
        &run,
    );

    // meter_ack_loss_charges_once, and pending usage keeps its payer across a restart
    let had: Vec<Value> = entries(api, &ledger, &format!("req:{name}@"));
    lever(api, &name, "fail-meter-acks", json!({ "times": 2 }))?;
    for n in 0..3 {
        api.op(&ada, &name, "today", &format!("q{n}"), json!({}))?;
    }
    lever(api, &name, "meter-now", json!({ "sample": false }))?;
    let restarted;
    let api = if s.runs("org-fragments", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    lever(api, &name, "meter-now", json!({ "sample": false, "resend": true }))?;
    let acked = s.eventually(Duration::from_secs(60), || lever(api, &name, "meter", json!({})).is_ok_and(|r| r.body["batches"] == json!([]) && r.body["rows"] == json!([])));
    let new: Vec<Value> = entries(api, &ledger, &format!("req:{name}@")).into_iter().filter(|e| !had.contains(e)).collect();
    let refs: std::collections::BTreeSet<&str> = new.iter().filter_map(|e| e["ref"].as_str()).collect();
    s.ok(
        "meter_ack_loss_charges_once and pending_usage_keeps_payer_after_restart: its hosting, its batch's acknowledgement lost and sent again across a restart, bills the org once a reference, and none of its people",
        acked && !new.is_empty() && refs.len() == new.len() && entries(api, &ada_id, &format!("req:{name}@")).is_empty(),
        json!({ "new": new }),
    );

    // its admin deletes it, as themselves (decision 70)
    let by_cal = api.signed(&cal, "DELETE", &format!("/api/f/{name}"), None)?;
    let gone = api.signed(&ada, "DELETE", &format!("/api/f/{name}"), None)?;
    let unlisted = s.eventually(WAIT, || {
        api.signed(&ada, "GET", &format!("{root}/fragments"), None).is_ok_and(|r| r.body["fragments"].as_array().is_some_and(|f| !f.iter().any(|x| x["name"] == name.as_str())))
    });
    s.ok(
        "its content admin deletes it, as themselves; an editor does not (403); its inventory lists it no more",
        by_cal.status == 403 && gone.status == 200 && unlisted,
        json!([by_cal.body, gone.body]),
    );
    Ok(())
}
