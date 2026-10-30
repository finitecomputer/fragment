//! An org's content governance (docs/cloudflare-v1.md, decisions 69 to
//! 71; docs/api.md, Org content), at its public routes on the Stripe
//! fake: who pays appoints the first content admin by an offer its person
//! accepts, and no billing admin, seat or stranger holds any content
//! power; an accepted admin offers the next, steps admins down and
//! offboards people, never the last admin; an offer is answered once; an
//! offboarding raises the person's epoch at once and holds across a
//! restart, and coming back is at the raised one. The org's content
//! outlives its billing, and a wipe of its sole admin waits on an
//! operator's recovery, each logged in its governance log.

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::billing::{answer, buys, org_of, seat, subscription_of, wipe_until_done, PUSHED};
use crate::api::{Api, Reply};
use crate::{Need, Suite};

pub fn governance(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("governance", &[Need::Fakes]) {
        return Ok(());
    }
    let pam = api.person()?;
    let org = buys(s, api, &pam)?;
    let root = format!("/api/orgs/{org}");
    let bea = api.person()?;
    let r = api.signed(&pam, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&bea) })))?;
    answer(api, &bea, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    let content = |api: &Api, who: &Keys| -> Value { api.signed(who, "GET", &format!("{root}/content"), None).map(|r| r.body).unwrap_or(Value::Null) };
    let offer = |api: &Api, who: &Keys, id: &str, to: &Keys| api.signed(who, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": id, "email": Api::email_of(to) })));
    let reply = |api: &Api, who: &Keys, offer: &str, verb: &str| api.signed(who, "POST", &format!("{root}/content-admin-offers/{offer}/{verb}"), Some(&json!({})));
    let person = |v: &Value, id: &str| v["people"].as_array().into_iter().flatten().find(|p| p["person"] == id).cloned().unwrap_or(Value::Null);

    // billing_admin_has_no_content_power
    let cat = api.person()?;
    let cat_id = api.identity(&cat)?;
    let by_bea = offer(api, &bea, "bea-1", &cat)?;
    let bea_reads = api.signed(&bea, "GET", &format!("{root}/content"), None)?;
    let pam_reads = api.signed(&pam, "GET", &format!("{root}/content"), None)?;
    let bea_offboards = api.signed(&bea, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "x", "person": cat_id, "expectedEpoch": 1 })))?;
    s.ok(
        "billing_admin_has_no_content_power: a billing admin offers no content admin's place, reads none of its content and offboards no one (403); who pays reads none either",
        by_bea.status == 403 && bea_reads.status == 403 && pam_reads.status == 403 && bea_offboards.status == 403,
        json!([by_bea.body, bea_reads.body, pam_reads.body, bea_offboards.body]),
    );

    // who pays appoints the first, by an offer its person accepts
    let first = offer(api, &pam, "first", &cat)?;
    let first_id = first.body["id"].as_str().unwrap_or("").to_string();
    let again = offer(api, &pam, "first", &cat)?;
    let other = offer(api, &pam, "first", &bea)?;
    let theirs = api.signed(&cat, "GET", "/api/orgs/offers", None)?;
    let sal = api.person()?;
    let not_theirs = reply(api, &sal, &first_id, "accept")?;
    let took = reply(api, &cat, &first_id, "accept")?;
    let pam_again = offer(api, &pam, "second", &sal)?;
    s.ok(
        "who pays offers the first content admin's place, once by its id (another email under it: 409); it waits on its email, is no one else's to answer (404), and once it is accepted who pays offers no more (403)",
        first.status == 200
            && first.body["answer"].is_null()
            && again.body["id"] == first_id.as_str()
            && other.status == 409
            && theirs.body["offers"].as_array().is_some_and(|o| o.iter().any(|x| x["id"] == first_id.as_str()))
            && not_theirs.status == 404
            && took.status == 200
            && took.body["answer"] == "accepted"
            && pam_again.status == 403,
        json!([first.body, other.body, theirs.body, not_theirs.body, took.body, pam_again.body]),
    );

    // role_answer_conflict
    let replayed = reply(api, &cat, &first_id, "accept")?;
    let declined = reply(api, &cat, &first_id, "decline")?;
    s.ok(
        "role_answer_conflict: its accept again answers as it did; a decline after it is refused (409) and changes nothing",
        replayed.status == 200 && replayed.body["answer"] == "accepted" && declined.status == 409 && person(&content(api, &cat), &cat_id)["admin"] == true,
        json!([replayed.body, declined.body]),
    );

    // accepted_admin_controls
    let dan = api.person()?;
    let dan_id = api.identity(&dan)?;
    let to_dan = offer(api, &cat, "dan-1", &dan)?;
    reply(api, &dan, to_dan.body["id"].as_str().unwrap_or(""), "accept")?;
    let down = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{dan_id}"), None)?;
    let last = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{cat_id}"), None)?;
    let view = content(api, &cat);
    let actions: Vec<&str> = view["log"].as_array().into_iter().flatten().filter_map(|e| e["action"].as_str()).collect();
    s.ok(
        "accepted_admin_controls: a content admin offers the next, steps an admin down (still in its content), never the last (409), and every change is in its governance log",
        to_dan.status == 200
            && down.status == 200
            && down.body["admin"] == false
            && down.body["active"] == true
            && last.status == 409
            && person(&view, &cat_id)["admin"] == true
            && ["admin-removed", "admin-accepted", "admin-offered", "first-admin-offered"].iter().all(|a| actions.contains(a)),
        json!({ "down": down.body, "last": last.body, "log": actions }),
    );

    // offboarding: at once, once by its id, at the epoch read
    let off = json!({ "id": "off-dan-1", "person": dan_id, "expectedEpoch": 1 });
    let out = api.signed(&cat, "POST", &format!("{root}/offboard"), Some(&off))?;
    let out_again = api.signed(&cat, "POST", &format!("{root}/offboard"), Some(&off))?;
    let stale = api.signed(&cat, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "off-dan-2", "person": dan_id, "expectedEpoch": 1 })))?;
    let self_out = api.signed(&cat, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "off-cat", "person": cat_id, "expectedEpoch": 1 })))?;
    let dan_reads = api.signed(&dan, "GET", &format!("{root}/content"), None)?;
    s.ok(
        "an admin offboards a person: out at once at their next epoch, once by its id; at a stale epoch (409), and never the last admin (409); the offboarded read nothing of it (403)",
        out.status == 200
            && out.body["epoch"] == 2
            && out.body["active"] == false
            && out_again.status == 200
            && out_again.body["epoch"] == 2
            && stale.status == 409
            && self_out.status == 409
            && dan_reads.status == 403,
        json!([out.body, stale.body, self_out.body, dan_reads.body]),
    );

    // offboard_fence_survives_restart, and coming back is at the raised epoch
    let restarted;
    let api = if s.runs("governance", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    let held = person(&content(api, &cat), &dan_id);
    let back = offer(api, &cat, "dan-2", &dan)?;
    let back_took = reply(api, &dan, back.body["id"].as_str().unwrap_or(""), "accept")?;
    let now = person(&content(api, &cat), &dan_id);
    s.ok(
        "offboard_fence_survives_restart: after a restart they are still out at their raised epoch; offered again and accepting, they are back at it, never at the old one",
        held["epoch"] == 2 && held["active"] == false && back_took.status == 200 && now["epoch"] == 2 && now["active"] == true && now["admin"] == true,
        json!({ "held": held, "now": now }),
    );

    // its content outlives its billing
    let sub = subscription_of(s, &org).context("the org's subscription")?;
    s.stripe.set_status(&sub, "canceled").map_err(anyhow::Error::msg)?;
    anyhow::ensure!(s.eventually(PUSHED, || seat(api, &pam)["seat"]["good"] == false), "the subscription lapses: {}", seat(api, &pam));
    // who paid steps down and leaves while the other admin stays; then the
    // other, alone there, takes a seat in another org, which ends its billing
    let pam_row = seat(api, &pam)["seat"]["id"].as_str().unwrap_or("").to_string();
    let pam_down = api.signed(&pam, "DELETE", &format!("/api/org/admins/{pam_row}"), None)?;
    let pam_left = api.signed(&pam, "POST", "/api/seat/leave", Some(&json!({})))?;
    let fio = api.person()?;
    buys(s, api, &fio)?;
    let to_bea = api.signed(&fio, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&bea) })))?;
    let bea_moved = answer(api, &bea, to_bea.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    anyhow::ensure!(pam_down.status == 200 && pam_left.status == 200 && bea_moved.status == 200, "the billing side goes: {pam_down}; {pam_left}; {bea_moved}");
    let billing_gone = s.eventually(PUSHED, || org_of(api, &pam).is_empty() && org_of(api, &bea) != org);
    let kept = api.signed(&cat, "GET", &format!("{root}/content"), None)?;
    let listed = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}"), None)?;
    s.ok(
        "its billing ended and everyone in it gone, the org stays for its content: its admins read it, and an operator finds it",
        billing_gone && kept.status == 200 && listed.status == 200,
        json!([kept.body, listed.status]),
    );

    // a sole admin's wipe: their place goes at once, and an operator recovers it
    let refused = [
        ("an agent's or unsigned answer: unsigned (401)", api.unsigned("GET", &format!("{root}/content"), None)?, 401),
        ("an org that is none (404)", api.signed(&cat, "GET", "/api/orgs/org-0000000000000000/content", None)?, 404),
        ("an offer with a field it does not take (400)", api.signed(&cat, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "z", "email": "a@e2e.test", "admin": true })))?, 400),
        ("an offer to no email (400)", api.signed(&cat, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "z", "email": "nobody" })))?, 400),
        ("an operator's recovery while it has admins (403)", api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/content-admin-offers"), Some(&json!({ "id": "r0", "email": Api::email_of(&sal) })))?, 403),
    ];
    let all = refused.iter().all(|(_, r, status)| r.status == *status);
    s.ok("and refused: each says why", all, json!(refused.iter().map(|(w, r, _): &(&str, Reply, u16)| json!([w, r.status, r.body])).collect::<Vec<_>>()));
    if !s.runs("governance", &[Need::Operator]) {
        s.skip("a sole content admin's wipe leaves the org to an operator's recovery", "it needs an operator key no person holds (a wipe's)");
        return Ok(());
    }
    let wiper = s.wiper.clone().context("Need::Operator lends an operator key")?;
    let off = api.signed(&cat, "POST", &format!("{root}/offboard"), Some(&json!({ "id": "off-dan-3", "person": dan_id, "expectedEpoch": 2 })))?;
    anyhow::ensure!(off.status == 200, "dan out: {off}");
    let (wiped, _) = wipe_until_done(api, &wiper, &cat_id)?;
    let eve = api.person()?;
    let recover = api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/content-admin-offers"), Some(&json!({ "id": "recover-1", "email": Api::email_of(&eve) })))?;
    let eve_took = reply(api, &eve, recover.body["id"].as_str().unwrap_or(""), "accept")?;
    let view = content(api, &eve);
    let wiped_entry = view["log"].as_array().into_iter().flatten().any(|e| e["action"] == "wiped" && e["subject"] == cat_id.as_str());
    let recovered_entry = view["log"].as_array().into_iter().flatten().any(|e| e["action"] == "admin-offered-by-operator");
    s.ok(
        "a sole content admin's wipe leaves the org to an operator's recovery: their place went with them (logged), an operator offers the next, and it is accepted",
        wiped.status == 200 && recover.status == 200 && eve_took.status == 200 && person(&view, &cat_id)["active"] == false && wiped_entry && recovered_entry,
        json!({ "recover": recover.body, "log": view["log"] }),
    );
    Ok(())
}
