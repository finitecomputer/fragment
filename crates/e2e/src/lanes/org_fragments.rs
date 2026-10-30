//! Fragments an org owns (docs/cloudflare-v1.md, decisions 66 to 71;
//! docs/api.md, An org's fragments), at their public routes on the fakes.
//! An org's content admin makes one, its editor; the org owns and pays for
//! it: its own AI steps and its hosting bill the org's ledger, never its
//! people's, and every AI step there meets its cap and the org's budget,
//! its admins' too. Its content admins administer who is in (one may give
//! themselves access, which its events show), a member who is not one does
//! not, and no agent does. An offboarding fences a member at once, at
//! every request, before anything is cleaned up; an invite from before it
//! admits no one, and a new one admits them at their raised epoch. A batch
//! whose acknowledgement is lost, sent again after a restart, bills the
//! org once. It hands out no storage token, and its admin deletes it.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::jobs::{settle, started};
use super::ledger::{charged, entries, lever};
use crate::api::{Api, Reply};
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
    let op = api.identity(&s.operator)?;
    let grant = api.signed(&s.operator, "POST", &format!("/api/ledger/{ledger}/grant"), Some(&json!({ "id": "fund", "micros": 20_000_000, "by": op, "why": "an org's fragments" })))?;
    let budget = api.signed(&ada, "PUT", &format!("{root}/budget"), Some(&json!({ "id": "b1", "micros": 10_000_000 })))?;
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
    // a second fragment of the org's, an invite to it waiting as they leave
    let made2 = api.signed(&ada, "POST", &format!("{root}/fragments"), Some(&json!({ "label": s.name("orgnotes"), "template": "calories" })))?;
    let name2 = made2.body["name"].as_str().context("the org's second fragment")?.to_string();
    invite(&name2, &cal)?;
    let place = |who: &str| api.signed(&ada, "GET", &format!("{root}/content"), None).ok().and_then(|r| r.body["people"].as_array().and_then(|p| p.iter().find(|x| x["person"] == who).cloned())).unwrap_or(Value::Null);
    let before = place(&cal_id);
    let out = api.signed(&ada, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "cal-1", "person": cal_id, "expectedEpoch": 1 })))?;
    let fenced_read = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    let fenced_write = api.op(&cal, &name, "log_food", "f2", json!({ "who": "cal", "food": "a pear", "calories": 100 }))?;
    let fenced_sub = api.signed(&cal, "POST", &format!("/api/f/{name}/subscriptions"), Some(&json!({ "channel": "replies", "url": "https://example.com/hook" })))?;
    let still_listed = api.signed(&ada, "GET", &format!("/api/f/{name}/members"), None)?.body["members"].as_array().is_some_and(|m| m.iter().any(|x| x["principal"] == cal_id.as_str()));
    s.ok(
        "an org's member is fenced at every request the moment they are offboarded, before anything is cleaned up (their row is still there): no read, no write, no subscription (403); a member who is no admin invites no one (403)",
        cal_in.status == 200 && cal_reads.status == 200 && cal_writes.status == 200 && not_admin.status == 403 && before["epoch"] == 1
            && out.status == 200 && out.body["epoch"] == 2 && out.body["active"] == false
            && fenced_read.status == 403 && fenced_write.status == 403 && fenced_sub.status == 403 && still_listed,
        json!([cal_in.body, out.body, fenced_read.body, fenced_write.body, fenced_sub.body]),
    );

    // an invite from before they left admits no one; a new one admits them at the raised epoch
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
    let renewed = api.signed(&ada, "DELETE", &format!("/api/f/{name}/members/{cal_id}"), None).and_then(|_| invite(&name, &cal))?;
    let back = accept(&name, &cal)?;
    let reads = api.signed(&cal, "GET", &format!("/api/f/{name}/status"), None)?;
    s.ok(
        "offboard_fence_survives_restart: an invite from before they left admits them no more (403); after a restart they are still fenced; a new invite admits them at their raised epoch",
        stale.status == 403 && held.status == 403 && renewed.status == 200 && back.status == 200 && reads.status == 200 && place(&cal_id)["epoch"] == 2 && place(&cal_id)["active"] == true,
        json!([stale.body, held.body, back.body, reads.body["role"]]),
    );

    // org_admin_cannot_bypass_cap: its budget stops its admin's AI steps too
    let tight = api.signed(&ada, "PUT", &format!("{root}/budget"), Some(&json!({ "id": "b2", "micros": 1 })))?;
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
