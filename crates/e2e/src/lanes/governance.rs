//! An org's content governance (docs/cloudflare-v1.md, decisions 69 to
//! 71; docs/api.md, Org content), at its public routes on the Stripe
//! fake. Who pays offers the first content admin's place, and once anyone
//! accepts one, their other "firsts" lapse; no billing admin, seat,
//! stranger or agent holds any content power. An accepted admin offers the
//! next, withdraws an offer, and steps admins down (never the last), each
//! one logged in the change's own turn; a client's id never collides with
//! the platform's. An offer is answered once, and accepted only while its
//! grounds hold: one an admin made lapses as they step down, across a
//! restart, while a new one is accepted. An org's content outlives its
//! billing; an org dropped takes its offers with it, so no one's list of
//! offers breaks; a sole admin's wipe waits on an operator's recovery,
//! which no offer waiting blocks.

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
    let id_of = |r: &Reply| r.body["id"].as_str().unwrap_or("").to_string();

    // billing_admin_has_no_content_power, nor an agent
    let cat = api.person()?;
    let cat_id = api.identity(&cat)?;
    let hand = Keys::generate();
    let reg = "/api/identities";
    let made = api.signed(&cat, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hand, "POST", reg, &cat) })))?;
    anyhow::ensure!(made.status == 200, "an agent of cat's: {made}");
    let refused = [
        ("a billing admin offers no place", offer(api, &bea, "bea-1", &cat)?, 403),
        ("nor reads its content", api.signed(&bea, "GET", &format!("{root}/content"), None)?, 403),
        ("nor steps anyone down", api.signed(&bea, "DELETE", &format!("{root}/content-admins/{cat_id}"), None)?, 403),
        ("who pays reads none of it", api.signed(&pam, "GET", &format!("{root}/content"), None)?, 403),
        ("an agent offers none", offer(api, &hand, "agent-1", &cat)?, 403),
        ("nor reads it", api.signed(&hand, "GET", &format!("{root}/content"), None)?, 403),
        ("nor lists offers", api.signed(&hand, "GET", "/api/orgs/offers", None)?, 403),
        ("nor answers one", reply(api, &hand, "cof-0000000000000000", "accept")?, 403),
        ("nor withdraws one", api.signed(&hand, "DELETE", &format!("{root}/content-admin-offers/cof-0000000000000000"), None)?, 403),
        ("nor steps anyone down", api.signed(&hand, "DELETE", &format!("{root}/content-admins/{cat_id}"), None)?, 403),
    ];
    let all = refused.iter().all(|(_, r, status)| r.status == *status);
    s.ok(
        "billing_admin_has_no_content_power: a billing admin offers no content admin's place, reads none of its content and steps no one down; who pays reads none; an agent does none of it (403)",
        all,
        json!(refused.iter().map(|(w, r, _)| json!([w, r.status, r.body])).collect::<Vec<_>>()),
    );

    // who pays offers the first; once anyone accepts one, the rest lapse
    let first = offer(api, &pam, "first", &cat)?;
    let first_id = id_of(&first);
    let again = offer(api, &pam, "first", &cat)?;
    let other = offer(api, &pam, "first", &bea)?;
    let to_bea = offer(api, &pam, "first-bea", &bea)?;
    let theirs = api.signed(&cat, "GET", "/api/orgs/offers", None)?;
    let sal = api.person()?;
    let not_theirs = reply(api, &sal, &first_id, "accept")?;
    let took = reply(api, &cat, &first_id, "accept")?;
    let bea_late = reply(api, &bea, &id_of(&to_bea), "accept")?;
    let pam_again = offer(api, &pam, "second", &sal)?;
    s.ok(
        "who pays offers the first content admin's place, once by its id (another email under it: 409), waiting on its email and no one else's to answer (404); once one is accepted, its other \"firsts\" lapse (409) and who pays offers no more (403)",
        first.status == 200
            && first.body["grounds"] == "payer"
            && first.body["answer"].is_null()
            && again.body["id"] == first_id.as_str()
            && other.status == 409
            && theirs.body["offers"].as_array().is_some_and(|o| o.iter().any(|x| x["id"] == first_id.as_str()))
            && not_theirs.status == 404
            && took.status == 200
            && took.body["answer"] == "accepted"
            && bea_late.status == 409
            && pam_again.status == 403,
        json!([first.body, other.body, not_theirs.body, took.body, bea_late.body, pam_again.body]),
    );

    // role_answer_conflict
    let replayed = reply(api, &cat, &first_id, "accept")?;
    let declined = reply(api, &cat, &first_id, "decline")?;
    s.ok(
        "role_answer_conflict: its accept again answers as it did; a decline after it is refused (409) and changes nothing",
        replayed.status == 200 && replayed.body["answer"] == "accepted" && declined.status == 409 && person(&content(api, &cat), &cat_id)["admin"] == true,
        json!([replayed.body, declined.body]),
    );

    // accepted_admin_controls: offers, withdraws, steps down, never the last
    let dan = api.person()?;
    let dan_id = api.identity(&dan)?;
    let to_dan = offer(api, &cat, "dan-1", &dan)?;
    let dan_took = reply(api, &dan, &id_of(&to_dan), "accept")?;
    let fay = api.person()?;
    let to_fay = offer(api, &cat, "fay-1", &fay)?;
    let withdrawn = api.signed(&cat, "DELETE", &format!("{root}/content-admin-offers/{}", id_of(&to_fay)), None)?;
    let fay_late = reply(api, &fay, &id_of(&to_fay), "accept")?;
    let down = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{dan_id}"), None)?;
    let last = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{cat_id}"), None)?;
    let view = content(api, &cat);
    let actions: Vec<&str> = view["log"].as_array().into_iter().flatten().filter_map(|e| e["action"].as_str()).collect();
    let no_email = !view["log"].to_string().contains("@e2e.test");
    s.ok(
        "accepted_admin_controls: a content admin offers the next, withdraws an offer (its answer then 404), steps an admin down (still in its content), never the last (409); each change is in its governance log, which names no email",
        dan_took.status == 200
            && withdrawn.status == 200
            && withdrawn.body["answer"] == "withdrawn"
            && fay_late.status == 404
            && down.status == 200
            && down.body["admin"] == false
            && last.status == 409
            && person(&view, &cat_id)["admin"] == true
            && ["admin-stepped-down", "admin-offer-withdrawn", "admin-accepted", "admin-offered"].iter().all(|a| actions.contains(a))
            && no_email,
        json!({ "withdrawn": withdrawn.body, "down": down.body, "last": last.body, "log": actions }),
    );

    // a client's id never takes the platform's: an offer named for another's answer
    let gus = api.person()?;
    let to_gus = offer(api, &cat, "gus-1", &gus)?;
    let squat = offer(api, &cat, &format!("answer:{}", id_of(&to_gus)), &sal)?;
    let gus_took = reply(api, &gus, &id_of(&to_gus), "accept")?;
    let logged = content(api, &cat)["log"].as_array().into_iter().flatten().any(|e| e["action"] == "admin-accepted" && e["request"] == format!("answer:{}", id_of(&to_gus)).as_str());
    s.ok(
        "a client's id is kept as its kind's (offer:…), never the platform's: an offer named for another's answer leaves that answer logged",
        squat.status == 200 && gus_took.status == 200 && logged,
        json!([squat.body, gus_took.body]),
    );

    // an admin's offer lapses as they step down, across a restart; a new one is accepted
    let to_dan = offer(api, &cat, "dan-2", &dan)?;
    reply(api, &dan, &id_of(&to_dan), "accept")?;
    let hal = api.person()?;
    let dans = offer(api, &dan, "hal-1", &hal)?;
    let dan_self = offer(api, &dan, "self-1", &sal)?;
    let down = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{dan_id}"), None)?;
    anyhow::ensure!(dans.status == 200 && dan_self.status == 200 && down.status == 200, "dan's offers made, then dan stepped down: {dans}; {down}");
    let restarted;
    let api = if s.runs("governance", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    let hal_late = reply(api, &hal, &id_of(&dans), "accept")?;
    let fresh = offer(api, &cat, "hal-2", &hal)?;
    let hal_took = reply(api, &hal, &id_of(&fresh), "accept")?;
    s.ok(
        "stale_offer_refused_after_restart: an offer whose maker stepped down was taken back with them, and after a restart is still refused (404); a new one is accepted",
        hal_late.status == 404 && fresh.status == 200 && hal_took.status == 200 && person(&content(api, &cat), &api.identity(&hal)?)["admin"] == true,
        json!([hal_late.body, fresh.body, hal_took.body]),
    );

    // an org dropped takes its offers with it: no one's list breaks
    let ivy = api.person()?;
    let ivy_org = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&ivy), "kind": "seat" })))?;
    let ivy_org = ivy_org.body["org"]["id"].as_str().context("ivy's comped org")?.to_string();
    let joy = api.person()?;
    let orphan = api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{ivy_org}/content-admin-offers"), Some(&json!({ "id": "recover-ivy", "email": Api::email_of(&joy) })))?;
    let kept = offer(api, &cat, "joy-1", &joy)?;
    let r = api.signed(&pam, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&ivy) })))?;
    let moved = answer(api, &ivy, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    let gone = s.eventually(PUSHED, || api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{ivy_org}"), None).is_ok_and(|r| r.status == 404));
    let joys = api.signed(&joy, "GET", "/api/orgs/offers", None)?;
    s.ok(
        "an org dropped with no one in its content takes its offers with it: their recipient's list still answers, the other org's offer in it",
        orphan.status == 200 && kept.status == 200 && moved.status == 200 && gone && joys.status == 200
            && joys.body["offers"].as_array().is_some_and(|o| o.len() == 1 && o[0]["id"] == kept.body["id"]),
        json!([orphan.body, moved.body, joys.body]),
    );

    // its content outlives its billing
    let sub = subscription_of(s, &org).context("the org's subscription")?;
    s.stripe.set_status(&sub, "canceled").map_err(anyhow::Error::msg)?;
    anyhow::ensure!(s.eventually(PUSHED, || seat(api, &pam)["seat"]["good"] == false), "the subscription lapses: {}", seat(api, &pam));
    // who paid steps down and leaves while the other admin stays; then the
    // other, alone there, takes a seat in another org, which ends its billing
    let ivy_left = api.signed(&ivy, "POST", "/api/seat/leave", Some(&json!({})))?;
    let pam_row = seat(api, &pam)["seat"]["id"].as_str().unwrap_or("").to_string();
    let pam_down = api.signed(&pam, "DELETE", &format!("/api/org/admins/{pam_row}"), None)?;
    let pam_left = api.signed(&pam, "POST", "/api/seat/leave", Some(&json!({})))?;
    let fio = api.person()?;
    buys(s, api, &fio)?;
    let to_bea = api.signed(&fio, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&bea) })))?;
    let bea_moved = answer(api, &bea, to_bea.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    anyhow::ensure!(ivy_left.status == 200 && pam_down.status == 200 && pam_left.status == 200 && bea_moved.status == 200, "the billing side goes: {ivy_left}; {pam_down}; {pam_left}; {bea_moved}");
    let billing_gone = s.eventually(PUSHED, || org_of(api, &pam).is_empty() && org_of(api, &bea) != org);
    let kept = api.signed(&cat, "GET", &format!("{root}/content"), None)?;
    let listed = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}"), None)?;
    s.ok(
        "its billing ended and everyone in it gone, the org stays for its content: its admins read it, and an operator finds it",
        billing_gone && kept.status == 200 && listed.status == 200,
        json!([kept.body, listed.status]),
    );

    let refused = [
        ("unsigned (401)", api.unsigned("GET", &format!("{root}/content"), None)?, 401),
        ("an org that is none (404)", api.signed(&cat, "GET", "/api/orgs/org-0000000000000000/content", None)?, 404),
        ("a log page's before that is no number (400)", api.signed(&cat, "GET", &format!("{root}/content?before=soon"), None)?, 400),
        ("an offer with a field it does not take (400)", api.signed(&cat, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "z", "email": "a@e2e.test", "admin": true })))?, 400),
        ("an offer to no email (400)", api.signed(&cat, "POST", &format!("{root}/content-admin-offers"), Some(&json!({ "id": "z", "email": "nobody" })))?, 400),
        ("an operator's recovery while it has admins (403)", api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/content-admin-offers"), Some(&json!({ "id": "r0", "email": Api::email_of(&sal) })))?, 403),
    ];
    let all = refused.iter().all(|(_, r, status)| r.status == *status);
    s.ok("and refused: each says why", all, json!(refused.iter().map(|(w, r, _)| json!([w, r.status, r.body])).collect::<Vec<_>>()));

    // a sole admin's wipe: their place goes, and an operator recovers it,
    // whatever offers wait
    if !s.runs("governance", &[Need::Operator]) {
        s.skip("a sole content admin's wipe leaves the org to an operator's recovery", "it needs an operator key no person holds (a wipe's)");
        return Ok(());
    }
    let wiper = s.wiper.clone().context("Need::Operator lends an operator key")?;
    for who in [&dan, &gus, &hal] {
        let id = api.identity(who)?;
        let down = api.signed(&cat, "DELETE", &format!("{root}/content-admins/{id}"), None)?;
        anyhow::ensure!(down.status == 200, "{id} steps down: {down}");
    }
    let waiting = offer(api, &cat, "waits", &sal)?;
    let (wiped, _) = wipe_until_done(api, &wiper, &cat_id)?;
    let eve = api.person()?;
    let recover = api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/content-admin-offers"), Some(&json!({ "id": "recover-1", "email": Api::email_of(&eve) })))?;
    let eve_took = reply(api, &eve, &id_of(&recover), "accept")?;
    let sal_late = reply(api, &sal, &id_of(&waiting), "accept")?;
    let view = content(api, &eve);
    let wiped_entry = view["log"].as_array().into_iter().flatten().any(|e| e["action"] == "wiped" && e["subject"] == cat_id.as_str());
    let recovered = view["log"].as_array().into_iter().flatten().any(|e| e["action"] == "admin-offered" && e["detail"]["grounds"] == "operator");
    s.ok(
        "a sole content admin's wipe leaves the org to an operator's recovery: their place and their offers went with them (logged), an operator offers the next, and it is accepted",
        waiting.status == 200 && wiped.status == 200 && recover.status == 200 && eve_took.status == 200 && sal_late.status == 404 && person(&view, &cat_id)["admin"] == false && wiped_entry && recovered,
        json!({ "recover": recover.body, "sal": sal_late.body, "log": view["log"] }),
    );
    Ok(())
}
