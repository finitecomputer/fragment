//! Orgs and seats (docs/billing.md; docs/api.md, Seats and orgs): an
//! operator comps a seat for an email, in a new org of one or in an org
//! named; a person who signs in with an email a seat is offered accepts it
//! (signing in attaches nothing); the registry pushes each seat's plan to
//! its holder's ledger (a comped seat is active whatever its org pays; one
//! that ends is canceled, the plan kept).
//! A seat's holder reads it and lets their always-on computer sleep; an
//! org's admins see it. The registry is the seat's one writer: a ledger an
//! operator's old `seat` command left ahead of its order takes its pushes
//! all the same. Valid, invalid and replay. A seat's computer staying
//! awake, and one made while its seat's push could not be asked for, are
//! the computers section's (it has one).

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

/// How long a seat's push may take to reach a ledger (the registry's alarm).
const PUSHED: Duration = Duration::from_secs(20);

fn ledger(api: &Api, keys: &Keys) -> Value {
    api.signed(keys, "GET", "/api/ledger", None).map(|r| r.body).unwrap_or(Value::Null)
}

fn email(i: &str) -> String {
    format!("orgs-{i}-{}@e2e.test", &Keys::generate().pubkey_hex()[..10])
}

pub fn orgs(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("orgs", &[Need::Deployment]) {
        return Ok(());
    }
    let op_session = api.sign_in("operator@e2e.test")?;
    // the ledger section approves the operator's key when it runs first
    let _ = api.approve(&op_session, &s.operator);
    let operator = s.operator.clone();
    let comp = |body: Value| api.signed(&operator, "POST", "/api/admin/seats", Some(&body));

    // ---- a person who signed in already: their comp is theirs at once
    let alice = api.person()?;
    let alice_id = api.identity(&alice)?;
    let alice_email = Api::email_of(&alice);
    let r = comp(json!({ "email": alice_email.to_uppercase(), "kind": "seat_always_on" }))?;
    let alice_seat = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let alice_org = r.body["org"]["id"].as_str().unwrap_or("").to_string();
    s.ok(
        "an operator comps a seat for a person's email (any case): theirs at once, in a new org of one named by it, which they admin",
        r.status == 200
            && r.body["created"] == true
            && r.body["seat"]["person"] == alice_id.as_str()
            && r.body["seat"]["email"] == alice_email.as_str()
            && r.body["seat"]["seat"] == "seat_always_on"
            && r.body["seat"]["comped"] == true
            && r.body["seat"]["admin"] == true
            && r.body["org"]["name"] == alice_email.as_str()
            && fragment_core::org::valid_org_id(&alice_org),
        &r,
    );
    let again = comp(json!({ "email": alice_email, "kind": "seat_always_on" }))?;
    s.ok(
        "the same comp again is the same seat, not made again",
        again.status == 200 && again.body["created"] == false && again.body["seat"]["id"] == alice_seat.as_str() && again.body["org"]["id"] == alice_org.as_str(),
        &again,
    );
    if !s.hosted() {
        // her seat's mails (she is mailed of the key she added too: Sign-in)
        let seat_mail = format!("You have a seat on {}", fragment_core::mail::PRODUCT);
        let sent: Vec<_> = s.mail.sent_to(&alice_email).into_iter().filter(|m| m.subject == seat_mail).collect();
        s.ok(
            "the platform mails them their seat, once (the replay mails nothing)",
            r.body["mailed"] == true && again.body["mailed"] == false && sent.len() == 1 && sent[0].subject == format!("You have a seat on {}", fragment_core::mail::PRODUCT) && sent[0].text.contains("always-on computer") && sent[0].text.contains("It is yours now"),
            format!("{sent:?}"),
        );
    }
    let pushed = s.eventually(PUSHED, || {
        let v = ledger(api, &alice);
        v["plan"] == "seat_always_on" && v["seat"] == "active"
    });
    s.ok("its plan reaches their ledger: seat_always_on, active, with its month's credit", pushed, ledger(api, &alice));
    let mine = api.signed(&alice, "GET", "/api/seat", None)?;
    s.ok(
        "they read their seat: its kind, comped, good, their org, and that they admin it",
        mine.status == 200
            && mine.body["seat"]["id"] == alice_seat.as_str()
            && mine.body["seat"]["kind"] == "seat_always_on"
            && mine.body["seat"]["comped"] == true
            && mine.body["seat"]["good"] == true
            && mine.body["seat"]["sleeps"] == false
            && mine.body["admin"] == true
            && mine.body["org"]["id"] == alice_org.as_str(),
        &mine,
    );
    // an operator's view of them says which they chose: why a good
    // always-on seat's computer is not always on
    let seen = |api: &Api| api.signed(&operator, "GET", &format!("/api/admin/people/{alice_id}"), None).map(|r| r.body["person"]["seat"].clone()).unwrap_or(Value::Null);
    let before = seen(api);
    let slept = api.signed(&alice, "PUT", "/api/seat", Some(&json!({ "sleeps": true })))?;
    let seen_slept = seen(api);
    let woke = api.signed(&alice, "PUT", "/api/seat", Some(&json!({ "sleeps": false })))?;
    let seen_woke = seen(api);
    s.ok("they let its computer sleep, and take it back", slept.status == 200 && slept.body["seat"]["sleeps"] == true && woke.body["seat"]["sleeps"] == false, json!([slept.body, woke.body]));
    s.ok(
        "an operator's view of them says whether they let it sleep, as they chose it",
        before["sleeps"] == false && seen_slept["sleeps"] == true && seen_slept["id"] == alice_seat.as_str() && seen_woke["sleeps"] == false,
        json!([before, seen_slept, seen_woke]),
    );

    // ---- an email no one has signed in with: the seat is offered it
    let bob_email = email("bob");
    let r = comp(json!({ "email": bob_email, "kind": "seat_always_on" }))?;
    let bob_seat = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    s.ok("a comp for an email no one signs in with is offered it, in a new org of one", r.status == 200 && r.body["seat"]["person"].is_null() && r.body["seat"]["admin"] == true, &r);
    let replay = comp(json!({ "email": bob_email, "kind": "seat_always_on" }))?;
    s.ok("and again is the same offer", replay.status == 200 && replay.body["created"] == false && replay.body["seat"]["id"] == bob_seat.as_str(), &replay);
    if !s.hosted() {
        let sent = s.mail.sent_to(&bob_email);
        s.ok("the email is mailed where to sign in to accept it", r.body["mailed"] == true && sent.len() == 1 && sent[0].text.contains("Sign in with this email address to accept it"), format!("{sent:?}"));
        s.mail.fail_next("E_RATE_LIMIT_EXCEEDED");
        let failed = comp(json!({ "email": email("dave"), "kind": "seat" }))?;
        s.ok("a mail that fails leaves the seat made, and says it was not mailed", failed.status == 200 && failed.body["created"] == true && failed.body["mailed"] == false, &failed);
    }
    let bob = Keys::generate();
    let bob_session = api.sign_in(&bob_email)?;
    let me = api.approve(&bob_session, &bob)?;
    let bob_id = me.body["id"].as_str().unwrap_or("").to_string();
    let waiting = api.signed(&bob, "GET", "/api/seat", None)?;
    let accepted = api.signed(&bob, "POST", &format!("/api/seat/offers/{bob_seat}/accept"), Some(&json!({})))?;
    let mine = api.signed(&bob, "GET", "/api/seat", None)?;
    s.ok(
        "whoever signs in with that email is offered the comp (given, with its admin's place), attached to nothing until they accept it; accepted, they hold it and admin its org",
        fragment_core::npub::is_identity(&bob_id)
            && waiting.body["seat"].is_null()
            && waiting.body["org"].is_null()
            && waiting.body["offered"][0]["id"] == bob_seat.as_str()
            && waiting.body["offered"][0]["comped"] == true
            && waiting.body["offered"][0]["admin"] == true
            && waiting.body["offered"][0]["from"] == "operator@e2e.test"
            && accepted.status == 200
            && mine.status == 200
            && mine.body["seat"]["id"] == bob_seat.as_str()
            && mine.body["seat"]["kind"] == "seat_always_on"
            && mine.body["admin"] == true,
        json!([waiting.body, accepted.body, mine.body]),
    );
    let pushed = s.eventually(PUSHED, || ledger(api, &bob)["plan"] == "seat_always_on");
    s.ok("its plan reaches their ledger too", pushed, ledger(api, &bob));

    // ---- an org named: a seat in someone's org, waiting
    let carol_email = email("carol");
    let r = comp(json!({ "email": carol_email, "kind": "seat", "org": alice_org }))?;
    let carol_seat = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    s.ok("an operator comps a seat in an org they name: not its admin", r.status == 200 && r.body["org"]["id"] == alice_org.as_str() && r.body["seat"]["admin"] == false, &r);
    let view = api.signed(&alice, "GET", "/api/org", None)?;
    let members = view.body["members"].as_array().cloned().unwrap_or_default();
    s.ok(
        "its admin sees the org: themselves, holding their seat, and the seat waiting on an email",
        view.status == 200
            && view.body["id"] == alice_org.as_str()
            && members.len() == 2
            && members[0]["person"] == alice_id.as_str()
            && members[1]["id"] == carol_seat.as_str()
            && members[1]["person"].is_null()
            && members[1]["email"] == carol_email.as_str(),
        &view,
    );
    let op_view = api.signed(&operator, "GET", &format!("/api/admin/orgs/{alice_org}"), None)?;
    s.ok("an operator sees any org", op_view.status == 200 && op_view.body == view.body, &op_view);
    let erin = api.person()?;
    let r = comp(json!({ "email": Api::email_of(&erin), "kind": "seat", "org": alice_org }))?;
    let not_admin = api.signed(&erin, "GET", "/api/org", None)?;
    let no_org = api.signed(&api.person()?, "GET", "/api/org", None)?;
    let own = api.signed(&bob, "GET", "/api/org", None)?;
    s.ok(
        "a seat's holder who is not its org's admin sees no org (403), nor does someone in none; an admin sees their own",
        r.status == 200 && not_admin.status == 403 && no_org.status == 403 && own.status == 200 && own.body["id"] != alice_org.as_str(),
        json!([r.status, not_admin.status, no_org.status, own.status]),
    );
    let erin_seat = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    api.signed(&operator, "DELETE", &format!("/api/admin/seats/{erin_seat}"), None)?;
    // FIN-562: a comp into another org was 409 here, kept so nobody joins
    // without their own yes; an offer keeps that, and lets staff place
    // someone (a moved person in an org of one) in the org that pays
    let bob_org = api.signed(&bob, "GET", "/api/seat", None)?.body["org"]["id"].clone();
    let offered = comp(json!({ "email": alice_email, "kind": "seat", "org": bob_org }))?;
    let again = comp(json!({ "email": alice_email, "kind": "seat", "org": bob_org }))?;
    let hers = api.signed(&alice, "GET", "/api/seat", None)?;
    let offer = offered.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let shown = hers.body["offered"].as_array().into_iter().flatten().any(|o| o["id"] == offer.as_str() && o["comped"] == true && o["org"]["id"] == bob_org);
    s.ok(
        "a person in an org comped into another is offered it there (a comp, waiting on their yes), not moved: they keep their own seat and org; again, the same offer",
        offered.status == 200
            && offered.body["created"] == true
            && offered.body["seat"]["person"].is_null()
            && offered.body["seat"]["comped"] == true
            && again.status == 200
            && again.body["created"] == false
            && again.body["seat"]["id"] == offer.as_str()
            && shown
            && hers.body["seat"]["id"] == alice_seat.as_str()
            && hers.body["org"]["id"] == alice_org.as_str(),
        json!([offered.body, again.body, hers.body]),
    );
    let declined = api.signed(&alice, "POST", &format!("/api/seat/offers/{offer}/decline"), Some(&json!({})))?;
    s.ok("they decline it, and stay where they are", declined.status == 200 && declined.body["seat"]["id"] == alice_seat.as_str() && declined.body["offered"] == json!([]), &declined);

    // ---- changing and ending comps
    let r = api.signed(&operator, "PATCH", &format!("/api/admin/seats/{alice_seat}"), Some(&json!({ "kind": "seat" })))?;
    s.ok("an operator changes a comped seat's kind", r.status == 200 && r.body["seat"] == "seat", &r);
    let pushed = s.eventually(PUSHED, || ledger(api, &alice)["plan"] == "seat");
    s.ok("and the holder's ledger follows", pushed, ledger(api, &alice));
    let r = api.signed(&operator, "DELETE", &format!("/api/admin/seats/{bob_seat}"), None)?;
    s.ok("an operator ends a comp", r.status == 200 && r.body["id"] == bob_seat.as_str(), &r);
    let pushed = s.eventually(PUSHED, || {
        let v = ledger(api, &bob);
        v["seat"] == "canceled" && v["standing"] == json!({ "standing": "agents_stopped", "why": "seat_canceled" })
    });
    s.ok("its holder's seat is canceled: agents stop; the plan is kept", pushed && ledger(api, &bob)["plan"] == "seat_always_on", ledger(api, &bob));
    let mine = api.signed(&bob, "GET", "/api/seat", None)?;
    s.ok(
        "an admin whose comp ended keeps their org, seatless, and is a guest (no seat; their ledger's plan kept, canceled)",
        mine.status == 200 && mine.body["seat"].is_null() && mine.body["admin"] == true && mine.body["org"].is_object() && mine.body["guest"] == true,
        &mine,
    );
    // the one guest test (FIN-615): the ledger agrees with the seat page
    let made = api.create(&bob, "after-comp")?;
    s.ok(
        "and, as a guest, makes no new fragment (403, decision 49: creating needs a seat)",
        made.status == 403 && made.message().starts_with("guests can't create fragments"),
        &made,
    );
    let r = api.signed(&operator, "DELETE", &format!("/api/admin/seats/{carol_seat}"), None)?;
    let view = api.signed(&alice, "GET", "/api/org", None)?;
    s.ok("a waiting seat that ends is gone from its org", r.status == 200 && view.body["members"].as_array().map(Vec::len) == Some(1), &view);
    let r = api.signed(&operator, "DELETE", &format!("/api/admin/seats/{carol_seat}"), None)?;
    s.ok("ending it again finds none (404)", r.status == 404, &r);
    let r = comp(json!({ "email": bob_email, "kind": "seat" }))?;
    let pushed = s.eventually(PUSHED, || {
        let v = ledger(api, &bob);
        v["seat"] == "active" && v["plan"] == "seat"
    });
    s.ok("a seatless admin comped again takes the seat in their own org, active again", r.status == 200 && r.body["seat"]["id"] == bob_seat.as_str() && pushed, ledger(api, &bob));
    let made = api.create(&bob, "comped-again")?;
    s.ok("and, the seat active again, makes fragments again", made.status == 200, &made);

    // ---- the registry is the seat's one writer: a ledger an operator's
    // `seat` command (gone 2026-10-16) left at an order an hour ahead of
    // the registry's refuses its next push as stale, naming its own, and
    // the registry orders past it: the push lands, and so do later ones
    const PAST: &str = "a ledger whose seat an old operator's command left canceled an hour ahead of the registry's order takes the registry's next push all the same: its seat active, its plan the comp's new kind";
    const LATER: &str = "and the registry's later pushes land too";
    if !s.runs("orgs", &[Need::Levers]) {
        s.skip(PAST, "it needs the ledger's levers");
        s.skip(LATER, "it needs the ledger's levers");
    } else {
        let ahead = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64 + 3_600_000;
        let frozen = api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": bob_id, "op": "legacy-seat", "seat": "canceled", "seq": ahead })))?;
        let canceled = frozen.status == 200 && ledger(api, &bob)["seat"] == "canceled";
        let r = api.signed(&operator, "PATCH", &format!("/api/admin/seats/{bob_seat}"), Some(&json!({ "kind": "seat_always_on" })))?;
        let pushed = s.eventually(PUSHED, || {
            let v = ledger(api, &bob);
            v["seat"] == "active" && v["plan"] == "seat_always_on" && v["standing"]["standing"] == "ok"
        });
        s.ok(PAST, canceled && r.status == 200 && pushed, json!({ "frozen": frozen.body, "kind": r.body, "ledger": ledger(api, &bob) }));
        let r = api.signed(&operator, "PATCH", &format!("/api/admin/seats/{bob_seat}"), Some(&json!({ "kind": "seat" })))?;
        let pushed = s.eventually(PUSHED, || ledger(api, &bob)["plan"] == "seat");
        let health = api.signed(&operator, "GET", "/api/admin/health", None)?;
        let stuck = health.body["failing"].as_array().is_none_or(|f| f.iter().any(|f| f["target"] == bob_id.as_str()));
        s.ok(LATER, r.status == 200 && pushed && !stuck, json!({ "ledger": ledger(api, &bob), "health": health.body }));
    }

    // ---- invalid
    let stranger = api.person()?;
    let refused = [
        ("a person who is no operator comps (403)", api.signed(&stranger, "POST", "/api/admin/seats", Some(&json!({ "email": email("x"), "kind": "seat" })))?, 403),
        ("an email that is none (400)", comp(json!({ "email": "not-an-email", "kind": "seat" }))?, 400),
        ("a kind that is no seat's (400)", comp(json!({ "email": email("y"), "kind": "guest" }))?, 400),
        ("a field it does not take (400)", comp(json!({ "email": email("z"), "kind": "seat", "paid": true }))?, 400),
        ("an org that is none (404)", comp(json!({ "email": email("w"), "kind": "seat", "org": "org-0000000000000000" }))?, 404),
        ("a held seat comped again as another kind (409)", comp(json!({ "email": alice_email, "kind": "seat_always_on" }))?, 409),
        ("a seat id that is none (404)", api.signed(&operator, "PATCH", "/api/admin/seats/mem-0000000000000000", Some(&json!({ "kind": "seat" })))?, 404),
        ("someone with no seat lets it sleep (400)", api.signed(&stranger, "PUT", "/api/seat", Some(&json!({ "sleeps": true })))?, 400),
    ];
    for (what, r, status) in refused {
        s.ok(&format!("refused: {what}"), r.status == status, &r);
    }
    let none = api.signed(&stranger, "GET", "/api/seat", None)?;
    let plan = ledger(api, &stranger)["plan"].clone();
    s.ok(
        "someone with no seat reads none, and is a guest only when their ledger's plan is (the deployment's plan for everyone, the e2e's seat, makes none)",
        none.status == 200 && none.body["seat"].is_null() && none.body["org"].is_null() && none.body["offered"] == json!([]) && none.body["guest"] == (plan == "guest"),
        json!({ "seat": none.body, "plan": plan }),
    );
    invoiced(s, api)
}

/// An org paid by invoice (FIN-824; docs/billing.md, "An org paid by
/// invoice"): an operator's mark, once by its id, in its governance log and
/// the operators'. While it is set its admins offer and remove seats, each
/// a comp, within its prepaid seats; cleared, its seats stay and no more
/// are offered. FIN-562's move into it is an offer, taken by its yes.
/// Valid, invalid and replay.
fn invoiced(s: &mut Suite, api: &Api) -> Result<()> {
    let operator = s.operator.clone();
    let comp = |body: Value| api.signed(&operator, "POST", "/api/admin/seats", Some(&body));
    let fresh = || format!("inv-{}", &Keys::generate().pubkey_hex()[..12]);

    // an org staff made for someone paying up front: comped, no subscription
    let admin = api.person()?;
    let made = comp(json!({ "email": Api::email_of(&admin), "kind": "seat_always_on" }))?;
    let org = made.body["org"]["id"].as_str().unwrap_or("").to_string();
    let add = |email: &str| api.signed(&admin, "POST", "/api/org/seats", Some(&json!({ "email": email })));
    let unpaid = add(&email("inv-a"))?;
    s.ok(
        "an admin of an org staff made, paid outside Stripe and not marked, offers no seat (400 org_not_paying)",
        made.status == 200 && unpaid.status == 400 && unpaid.body["reason"] == "org_not_paying",
        json!([made.body, unpaid.body]),
    );

    // ---- marked, with two prepaid seats
    let mark = |body: Value| api.signed(&operator, "PUT", &format!("/api/admin/orgs/{org}/invoiced"), Some(&body));
    let id = fresh();
    let set = mark(json!({ "id": id, "invoiced": true, "seats": 2 }))?;
    let replay = mark(json!({ "id": id, "invoiced": true, "seats": 2 }))?;
    let other = mark(json!({ "id": id, "invoiced": true, "seats": 3 }))?;
    let content = api.signed(&operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?;
    let entries = content.body["log"].as_array().into_iter().flatten().filter(|e| e["action"] == "invoiced" && e["request"] == format!("invoiced:{id}").as_str() && e["detail"]["seats"] == 2).count();
    let ops = api.signed(&operator, "GET", "/api/admin/log", None)?;
    let logged = ops.body["entries"].as_array().into_iter().flatten().filter(|e| e["action"] == "org-invoiced" && e["target"] == org.as_str()).count();
    s.ok(
        "an operator marks it paid by invoice, two prepaid seats; again by its id, the same, logged once in its governance log and the operators'; another body under its id 409",
        set.status == 200
            && set.body["id"] == org.as_str()
            && set.body["invoiced"]["seats"] == 2
            && set.body["invoiced"]["at"].is_i64()
            && replay.status == 200
            && replay.body["invoiced"] == set.body["invoiced"]
            && other.status == 409
            && entries == 1
            && logged == 1,
        json!({ "set": set.body, "replay": replay.body, "other": other.status, "log": content.body["log"], "entries": entries, "logged": logged }),
    );
    let view = api.signed(&admin, "GET", "/api/org", None)?;
    let mine = api.signed(&admin, "GET", "/api/seat", None)?;
    let listed = api.signed(&operator, "GET", "/api/admin/orgs", None)?;
    let row = listed.body["orgs"].as_array().into_iter().flatten().find(|o| o["id"] == org.as_str()).cloned().unwrap_or(Value::Null);
    s.ok(
        "its admin's view of it, their own seat and the operators' list say it is paid by invoice",
        view.body["invoiced"]["seats"] == 2 && mine.body["seat"]["invoiced"] == true && mine.body["seat"]["comped"] == true && row["invoiced"]["seats"] == 2,
        json!([view.body, mine.body, row]),
    );

    // ---- its admin's seats: comps, within its prepaid seats
    let dana_email = email("inv-dana");
    let offered = add(&dana_email)?;
    let again = add(&dana_email)?;
    let seat = offered.body["seat"]["id"].as_str().unwrap_or("").to_string();
    s.ok(
        "its admin offers a seat: a comp, outside Stripe, waiting on the email; again, the same",
        offered.status == 200
            && offered.body["created"] == true
            && offered.body["seat"]["comped"] == true
            && offered.body["seat"]["seat"] == "seat_always_on"
            && offered.body["seat"]["person"].is_null()
            && again.status == 200
            && again.body["created"] == false
            && again.body["seat"]["id"] == seat.as_str(),
        json!([offered.body, again.body]),
    );
    let full = add(&email("inv-e"))?;
    s.ok("past its two prepaid seats, held or offered, no seat is offered (400 prepaid_seats_taken)", full.status == 400 && full.body["reason"] == "prepaid_seats_taken", &full);
    let dana = Keys::generate();
    let session = api.sign_in(&dana_email)?;
    api.approve(&session, &dana)?;
    let accepted = api.signed(&dana, "POST", &format!("/api/seat/offers/{seat}/accept"), Some(&json!({})))?;
    let pushed = s.eventually(PUSHED, || {
        let v = ledger(api, &dana);
        v["plan"] == "seat_always_on" && v["seat"] == "active"
    });
    s.ok(
        "whoever signs in with it accepts it: a comped seat there, good, paid by invoice, its plan pushed to their ledger",
        accepted.status == 200
            && accepted.body["seat"]["id"] == seat.as_str()
            && accepted.body["seat"]["comped"] == true
            && accepted.body["seat"]["good"] == true
            && accepted.body["seat"]["invoiced"] == true
            && pushed,
        json!([accepted.body, ledger(api, &dana)]),
    );
    let removed = api.signed(&admin, "DELETE", &format!("/api/org/seats/{seat}"), None)?;
    let pushed = s.eventually(PUSHED, || ledger(api, &dana)["seat"] == "canceled");
    let theirs = api.signed(&dana, "GET", "/api/seat", None)?;
    s.ok(
        "its admin removes it: the comp ends, their seat canceled, in no org",
        removed.status == 200 && removed.body["id"] == seat.as_str() && pushed && theirs.body["seat"].is_null() && theirs.body["org"].is_null(),
        json!([removed.body, theirs.body]),
    );

    // ---- FIN-562: someone the move placed in an org of one is offered a
    // comp in it, and moves by their own yes
    let moved = api.person()?;
    let theirs = comp(json!({ "email": Api::email_of(&moved), "kind": "seat_always_on" }))?;
    let into = comp(json!({ "email": Api::email_of(&moved), "kind": "seat_always_on", "org": org }))?;
    let offer = into.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let before = api.signed(&moved, "GET", "/api/seat", None)?;
    let yes = api.signed(&moved, "POST", &format!("/api/seat/offers/{offer}/accept"), Some(&json!({})))?;
    s.ok(
        "an operator's comp into it for someone in an org of one of their own is an offer: they stay until they accept it, then hold its comped seat, paid by invoice",
        theirs.status == 200
            && into.status == 200
            && into.body["seat"]["person"].is_null()
            && into.body["seat"]["comped"] == true
            && before.body["org"]["id"] == theirs.body["org"]["id"]
            && before.body["offered"].as_array().into_iter().flatten().any(|o| o["id"] == offer.as_str())
            && yes.status == 200
            && yes.body["seat"]["org"]["id"] == org.as_str()
            && yes.body["seat"]["comped"] == true
            && yes.body["seat"]["invoiced"] == true,
        json!([theirs.body, into.body, before.body, yes.body]),
    );

    // ---- its seats changed, then the mark cleared: its seats stay
    let raised = mark(json!({ "id": fresh(), "invoiced": true }))?;
    let uncapped = add(&email("inv-g"))?;
    let cleared = mark(json!({ "id": fresh(), "invoiced": false }))?;
    let twice = mark(json!({ "id": fresh(), "invoiced": false }))?;
    let kept = api.signed(&admin, "GET", "/api/org", None)?;
    let seats = kept.body["members"].as_array().map_or(0, |m| m.iter().filter(|m| m["seat"].is_string()).count());
    let refused = add(&email("inv-h"))?;
    let content = api.signed(&operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?;
    let clears = content.body["log"].as_array().into_iter().flatten().filter(|e| e["action"] == "invoiced-cleared").count();
    s.ok(
        "its seats uncapped, its admin offers past two; cleared (once logged, again a no-op), its seats stay, held and offered, and its admin offers no more (400 org_not_paying)",
        raised.status == 200
            && raised.body["invoiced"].is_object()
            && raised.body["invoiced"]["seats"].is_null()
            && uncapped.status == 200
            && cleared.status == 200
            && cleared.body["invoiced"].is_null()
            && twice.status == 200
            && clears == 1
            && seats == 3
            && refused.status == 400
            && refused.body["reason"] == "org_not_paying",
        json!({ "raised": raised.body, "uncapped": uncapped.body, "cleared": cleared.body, "kept": kept.body, "refused": refused.body, "clears": clears }),
    );
    let theirs_still = api.signed(&moved, "GET", "/api/seat", None)?;
    s.ok("a seat held when it was cleared is a comp still, good", theirs_still.body["seat"]["good"] == true && theirs_still.body["seat"]["invoiced"] == false, &theirs_still);

    // ---- invalid
    let stranger = api.person()?;
    let refused = [
        ("a person who is no operator marks it (403)", api.signed(&stranger, "PUT", &format!("/api/admin/orgs/{org}/invoiced"), Some(&json!({ "id": fresh(), "invoiced": true })))?, 403),
        ("an org that is none (404)", api.signed(&operator, "PUT", "/api/admin/orgs/org-0000000000000000/invoiced", Some(&json!({ "id": fresh(), "invoiced": true })))?, 404),
        ("no prepaid seats (400)", mark(json!({ "id": fresh(), "invoiced": true, "seats": 0 }))?, 400),
        ("more prepaid seats than an org holds (400)", mark(json!({ "id": fresh(), "invoiced": true, "seats": fragment_core::org::SEATS_PER_ORG_MAX + 1 }))?, 400),
        ("seats named with a clear (400)", mark(json!({ "id": fresh(), "invoiced": false, "seats": 2 }))?, 400),
        ("a field it does not take (400)", mark(json!({ "id": fresh(), "invoiced": true, "until": 1 }))?, 400),
        ("an id that is none (400)", mark(json!({ "id": "", "invoiced": true }))?, 400),
    ];
    for (what, r, status) in refused {
        s.ok(&format!("refused, the mark: {what}"), r.status == status, &r);
    }
    Ok(())
}
