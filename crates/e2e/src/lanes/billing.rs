//! Seats bought through Stripe (docs/billing.md; docs/api.md, Billing), on
//! the Stripe fake: a person's Checkout for their own seat, which makes
//! nothing until it is paid, then an org of their own; the seat theirs
//! once Stripe says it is paid, by its webhook or by the Checkout's return
//! when no webhook comes; the subscription's lapse and a second purchase,
//! on the org's own customer; the portal. Every subscription the platform
//! writes is fetched from Stripe again, so a forged, stale, foreign,
//! repeated or out-of-order event changes nothing it should not. An org's
//! seats and admins are offers, no one's until they accept them; who pays
//! is no other admin's to remove. Valid, invalid, replay, and a webhook
//! lost. Then the security audit's H1, each of its steps refused
//! (`consent`), and its M4: a wipe cancels what the wiped would pay, its
//! customer kept, across a restart mid-step (`wiped`).

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{url_enc, Api};
use crate::browser::{Lease, Page};
use crate::{Need, Suite};

/// How long a seat's push may take to reach a ledger (the registry's alarm).
pub(super) const PUSHED: Duration = Duration::from_secs(20);

fn ledger(api: &Api, keys: &Keys) -> Value {
    api.signed(keys, "GET", "/api/ledger", None).map(|r| r.body).unwrap_or(Value::Null)
}

/// The credit settings say is left (`Credit this month`, then `$1,234.50
/// left`, shell.js `usd`), in micro-dollars.
fn credit_shown(text: &str) -> Option<i64> {
    let line = &text[text.find("Credit this month")?..];
    let amount = line[line.find('$')? + 1..].split(" left").next()?;
    amount.contains('.').then_some(())?;
    amount.chars().filter(char::is_ascii_digit).collect::<String>().parse::<i64>().ok().map(|cents| cents * 10_000)
}

/// Checkout's return to settings (`/settings?pack=<session>`) in a page of
/// its own, its POST held before it is sent; settings opened again once
/// the query is gone, and left until nothing but that POST is under way;
/// then the POST let go, and the page left until all is done. Whether each
/// step came, and what settings show then.
fn held_return(b: &mut Lease, api: &Api, session: &str, wait: Duration) -> Result<(bool, Value)> {
    let script = format!("if (window === window.top) {}", super::shell::hold("POST", "^/api/billing/sessions/", super::shell::Hold::Request));
    let page = b.open_with_script(&format!("{}/settings?pack={session}", api.base), &script)?;
    let held = b.until(&page, "location.search === '' && typeof window.__held?.release === 'function'", wait);
    b.eval(&page, "(document.getElementById('settings').click(), true)")?;
    let waiting = b.until(&page, "window.__held.inflight === 1 && performance.now() - window.__held.quiet > 500", wait);
    b.eval(&page, "(window.__held.release(), true)")?;
    let settled = b.until(&page, super::shell::HELD_SETTLED, wait);
    let seen = b.eval(&page, "({ text: document.getElementById('settings-page')?.innerText ?? '', at: location.pathname + location.search })")?;
    b.close(page)?;
    Ok((held && waiting && settled, seen))
}

/// What settings say of a pack's return that was not applied (billing.js
/// `NOT_HERE`).
const NOT_HERE: &str = "Your purchase isn't here yet. If you were charged, the credit arrives within a few minutes: open Settings again to see it.";

/// A page's script, before the shell runs: its read of the computers
/// names a help link (`support`, as a deployment's `support_url` does),
/// and what it warns of in the console is kept (`window.__warned`).
const HELP_AND_CONSOLE: &str = "if (window === window.top) { const real = window.fetch; window.fetch = async (url, o) => { const r = await real(url, o); \
    if (String(url) !== '/api/computers' || o?.method !== 'GET' || !r.ok) return r; const body = await r.json(); \
    return new Response(JSON.stringify({ ...body, support: 'mailto:help@e2e.test' }), { status: r.status, headers: { 'content-type': 'application/json' } }); }; \
    const warn = console.warn; window.__warned = []; console.warn = (...a) => { window.__warned.push(a.map(String).join(' ')); warn(...a); }; }";

pub(super) fn seat(api: &Api, keys: &Keys) -> Value {
    api.signed(keys, "GET", "/api/seat", None).map(|r| r.body).unwrap_or(Value::Null)
}

/// The org's live subscription's id (not one that ended), from what the
/// platform asked Stripe for.
pub(super) fn subscription_of(s: &Suite, org: &str) -> Option<String> {
    s.stripe.requests().iter().rev().find_map(|r| {
        let id = r.path.strip_prefix("/v1/subscriptions/")?;
        let sub = s.stripe.subscription(id)?;
        (sub.metadata.get("fragment_org").map(String::as_str) == Some(org) && sub.status != "canceled").then(|| id.to_string())
    })
}

pub fn billing(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("billing", &[Need::Fakes]) {
        return Ok(());
    }
    let op_session = api.sign_in("operator@e2e.test")?;
    // the ledger section approves the operator's key when it runs first
    let _ = api.approve(&op_session, &s.operator);

    // ---- a person buys a $200 seat: nothing is made until they pay
    let ann = api.person()?;
    let ann_id = api.identity(&ann)?;
    let ann_email = Api::email_of(&ann);
    let customers = s.stripe.customers().len();
    let r = api.signed(&ann, "POST", "/api/billing/checkout", Some(&json!({ "kind": "seat_always_on" })))?;
    let session = r.body["session"].as_str().unwrap_or("").to_string();
    s.ok(
        "a person's Checkout for a $200 seat: Stripe's page, to pay on",
        r.status == 200 && session.starts_with("cs_") && r.body["url"].as_str().is_some_and(|u| u.starts_with(&s.stripe.url)),
        &r,
    );
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("a Checkout was asked for")?;
    let f = |k: &str| asked.form.get(k).cloned().unwrap_or_default();
    let org = f("metadata[fragment_org]");
    s.ok(
        "it takes a card always, its tax, promotion codes, one seat at its lookup key's price, names this deployment, the org it will make and its buyer, bills a customer Stripe makes for the buyer's email (none named), and returns to the welcome, paid or canceled",
        f("mode") == "subscription"
            && f("payment_method_collection") == "always"
            && f("automatic_tax[enabled]") == "true"
            && f("allow_promotion_codes") == "true"
            && f("customer_email") == ann_email
            && !asked.form.contains_key("customer")
            && !asked.form.keys().any(|k| k.starts_with("customer_update"))
            && f("line_items[0][quantity]") == "1"
            && f("metadata[fragment_deployment]") == api.base
            && f("subscription_data[metadata][fragment_deployment]") == api.base
            && f("subscription_data[metadata][fragment_org]") == org
            && f("metadata[fragment_person]") == ann_id
            && f("metadata[fragment_kind]") == "seat_always_on"
            && f("success_url").ends_with("/welcome?checkout={CHECKOUT_SESSION_ID}")
            && f("cancel_url").ends_with("/welcome?checkout=canceled")
            && fragment_core::org::valid_org_id(&org),
        format!("{:?}", asked.form),
    );
    let before = seat(api, &ann);
    let none = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}"), None)?;
    s.ok(
        "before they pay, nothing is made: they hold no seat and are in no org, the org their Checkout names is none (404), and Stripe made no customer",
        before["seat"].is_null() && before["org"].is_null() && before["admin"] == false && none.status == 404 && s.stripe.customers().len() == customers,
        json!({ "seat": before, "org": none.status, "customers": [customers, s.stripe.customers().len()] }),
    );
    s.stripe.complete(&session).map_err(anyhow::Error::msg)?;
    let pushed = s.eventually(PUSHED, || {
        let v = ledger(api, &ann);
        v["plan"] == "seat_always_on" && v["seat"] == "active"
    });
    let mine = seat(api, &ann);
    let view = api.signed(&ann, "GET", "/api/org", None)?;
    s.ok(
        "paid, Stripe's webhook makes the org their Checkout named: they admin it and hold its seat, paid not comped, they pay for it, and its plan reaches their ledger",
        pushed
            && mine["seat"]["kind"] == "seat_always_on"
            && mine["seat"]["comped"] == false
            && mine["seat"]["good"] == true
            && mine["admin"] == true
            && mine["org"]["id"] == org.as_str()
            && view.body["payer"] == ann_id.as_str()
            && view.body["members"].as_array().map(Vec::len) == Some(1),
        json!([mine, ledger(api, &ann), view.body]),
    );
    let delivered = s.stripe.delivered();
    s.ok("each event was answered 200", !delivered.is_empty() && delivered.iter().all(|(_, _, status)| *status == 200), format!("{delivered:?}"));
    let back = api.signed(&ann, "POST", &format!("/api/billing/sessions/{session}"), Some(&json!({})))?;
    s.ok("the Checkout's return again changes nothing, and answers their seat", back.status == 200 && back.body["seat"] == mine["seat"], &back);
    let again = api.signed(&ann, "POST", "/api/billing/checkout", Some(&json!({})))?;
    s.ok("someone who holds a seat buys none (409)", again.status == 409, &again);
    let sub = subscription_of(s, &org).context("the org's subscription was fetched")?;

    // ---- webhooks that are not Stripe's, or not this deployment's
    let object = s.stripe.subscription_json(&sub).context("the subscription")?;
    let (body, sig) = s.stripe.signed_event("customer.subscription.updated", object.clone(), None);
    let forged = s.stripe.deliver_raw(&body, Some(&sig.replace("v1=", "v1=00"))).map_err(anyhow::Error::msg)?;
    let unsigned = s.stripe.deliver_raw(&body, None).map_err(anyhow::Error::msg)?;
    let (stale_body, stale_sig) = s.stripe.signed_event("customer.subscription.updated", object.clone(), Some(crate::api::now_s() - 600));
    let stale = s.stripe.deliver_raw(&stale_body, Some(&stale_sig)).map_err(anyhow::Error::msg)?;
    s.ok("an event with a forged signature, none, or one ten minutes old is refused (401)", [forged, unsigned, stale] == [401, 401, 401], json!([forged, unsigned, stale]));
    let mut finite = object.clone();
    finite["metadata"] = json!({ "finite_customer_org_id": "x" });
    let mut other = object.clone();
    other["metadata"]["fragment_deployment"] = json!("https://another.example");
    let foreign: Vec<u16> = [finite, other]
        .into_iter()
        .map(|o| {
            let (b, sig) = s.stripe.signed_event("customer.subscription.deleted", o, None);
            s.stripe.deliver_raw(&b, Some(&sig)).unwrap_or(0)
        })
        .collect();
    let replayed = s.stripe.deliver_raw(&body, Some(&sig)).map_err(anyhow::Error::msg)?;
    s.ok(
        "finite-mono's event and another deployment's are answered and dropped; a real one again is no change",
        foreign == [200, 200] && replayed == 200 && seat(api, &ann)["seat"]["good"] == true,
        json!([foreign, replayed]),
    );

    // ---- a lapse: past due is still good; canceled stops agents
    s.stripe.set_status(&sub, "past_due").map_err(anyhow::Error::msg)?;
    std::thread::sleep(Duration::from_secs(2));
    s.ok("past due is good standing: Stripe's retries are the grace", seat(api, &ann)["seat"]["good"] == true && ledger(api, &ann)["seat"] == "active", ledger(api, &ann));
    // out of order: the events of active, then canceled, arrive reversed
    s.stripe.hold_events(true);
    s.stripe.set_status(&sub, "active").map_err(anyhow::Error::msg)?;
    s.stripe.set_status(&sub, "canceled").map_err(anyhow::Error::msg)?;
    s.stripe.release_events(true);
    let lapsed = s.eventually(PUSHED, || {
        let v = ledger(api, &ann);
        v["seat"] == "canceled" && v["standing"] == json!({ "standing": "agents_stopped", "why": "seat_canceled" })
    });
    s.ok(
        "canceled, whatever order its events came in, their seat lapses: agents stop, the plan is kept",
        lapsed && seat(api, &ann)["seat"]["good"] == false && ledger(api, &ann)["plan"] == "seat_always_on",
        json!([seat(api, &ann), ledger(api, &ann)]),
    );

    // ---- buying again: a new subscription replaces the one that ended,
    // on the org's own customer, for its paid seats
    let hundred = api.signed(&ann, "POST", "/api/billing/checkout", Some(&json!({ "kind": "seat" })))?;
    let r = api.signed(&ann, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let session2 = r.body["session"].as_str().unwrap_or("").to_string();
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("a second Checkout was asked for")?;
    let own = s.stripe.subscription(&sub).map(|x| x.customer).context("the ended subscription's customer")?;
    let reused = asked.form.get("customer") == Some(&own) && !asked.form.contains_key("customer_email") && asked.form.get("line_items[0][quantity]").map(String::as_str) == Some("1");
    s.stripe.complete(&session2).map_err(anyhow::Error::msg)?;
    let restored = s.eventually(PUSHED, || {
        let v = ledger(api, &ann);
        v["seat"] == "active" && v["plan"] == "seat_always_on"
    });
    s.ok(
        "their org's admin buys again, naming no kind (the seat sold; a $100 one is refused, 400), on its own customer (none made) for its one paid seat: the new subscription replaces the ended one, and the seat is good",
        hundred.status == 400
            && hundred.message().contains("not sold")
            && r.status == 200
            && reused
            && restored
            && seat(api, &ann)["seat"]["kind"] == "seat_always_on"
            && seat(api, &ann)["org"]["id"] == org.as_str(),
        json!([hundred.body, seat(api, &ann), ledger(api, &ann), format!("{:?}", asked.form)]),
    );
    let old = s.stripe.subscription_json(&sub).context("the old subscription")?;
    let (b, sig) = s.stripe.signed_event("customer.subscription.updated", old, None);
    let late = s.stripe.deliver_raw(&b, Some(&sig)).map_err(anyhow::Error::msg)?;
    std::thread::sleep(Duration::from_secs(2));
    s.ok("the ended subscription's late event changes nothing", late == 200 && seat(api, &ann)["seat"]["good"] == true, seat(api, &ann));

    // ---- a webhook lost: the Checkout's return carries it
    let bo = api.person()?;
    let r = api.signed(&bo, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let session3 = r.body["session"].as_str().unwrap_or("").to_string();
    s.stripe.hold_events(true);
    s.stripe.complete(&session3).map_err(anyhow::Error::msg)?;
    let none_yet = seat(api, &bo)["seat"].is_null();
    let back = api.signed(&bo, "POST", &format!("/api/billing/sessions/{session3}"), Some(&json!({})))?;
    s.ok("with no webhook, the Checkout's return gives them their seat at once", none_yet && back.status == 200 && back.body["seat"]["kind"] == "seat_always_on", &back);
    s.stripe.release_events(false);
    std::thread::sleep(Duration::from_secs(1));
    s.ok("its events arriving late change nothing", seat(api, &bo)["seat"] == back.body["seat"], seat(api, &bo));
    let theirs = api.signed(&ann, "POST", &format!("/api/billing/sessions/{session3}"), Some(&json!({})))?;
    let none = api.signed(&bo, "POST", "/api/billing/sessions/cs_nonesuch", Some(&json!({})))?;
    s.ok("another person's Checkout is none of theirs (404), nor one Stripe never made", theirs.status == 404 && none.status == 404, json!([theirs.status, none.status]));

    // ---- the portal: card and invoices, the org's admins only
    let r = api.signed(&ann, "POST", "/api/billing/portal", Some(&json!({})))?;
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/billing_portal/sessions").context("a portal session was asked for")?;
    s.ok(
        "an org's admin opens Stripe's portal, on this deployment's own configuration",
        r.status == 200 && r.body["url"].as_str().is_some_and(|u| u.contains("/portal/")) && asked.form.get("configuration").map(String::as_str) == Some(crate::STRIPE_PORTAL),
        &r,
    );
    let stranger = api.person()?;
    let r = api.signed(&stranger, "POST", "/api/billing/portal", Some(&json!({})))?;
    s.ok("someone in no org opens none (403)", r.status == 403, &r);

    // ---- an org's admin gives seats: each billed from its invite
    let sub2 = subscription_of(s, &org).context("the org's new subscription")?;
    let quantities = |s: &Suite| s.stripe.subscription(&sub2).map(|x| x.items.iter().map(|(_, k, q)| (k.clone(), *q)).collect::<Vec<_>>()).unwrap_or_default();
    let seat_q = |q: u64| vec![("fragment_seat_always_on_month".to_string(), q)];
    let cy_email = format!("billing-cy-{}@e2e.test", &Keys::generate().pubkey_hex()[..10]);
    let hundred = api.signed(&ann, "POST", "/api/org/seats", Some(&json!({ "email": cy_email, "kind": "seat" })))?;
    let r = api.signed(&ann, "POST", "/api/org/seats", Some(&json!({ "email": cy_email })))?;
    let cy_seat = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let replay = api.signed(&ann, "POST", "/api/org/seats", Some(&json!({ "email": cy_email, "kind": "seat_always_on" })))?;
    s.ok(
        "an org's admin offers a seat to an email, the always-on one sold (a $100 seat is refused, 400): an offer, no one's, mailed; again it is the same offer",
        hundred.status == 400
            && hundred.message().contains("not sold")
            && r.status == 200
            && r.body["created"] == true
            && r.body["seat"]["person"].is_null()
            && r.body["seat"]["seat"] == "seat_always_on"
            && r.body["seat"]["comped"] == false
            && replay.body["created"] == false
            && replay.body["seat"]["id"] == cy_seat.as_str(),
        json!([hundred.body, r.body, replay.body]),
    );
    if !s.hosted() {
        let sent = s.mail.sent_to(&cy_email);
        s.ok(
            "the email is mailed who offers it, and where to accept or decline it: nothing is theirs until they accept",
            r.body["mailed"] == true && sent.len() == 1 && sent[0].text.contains("offers you") && sent[0].text.contains(&ann_email) && sent[0].text.contains("accept or decline"),
            format!("{sent:?}"),
        );
    }
    let billed = s.eventually(PUSHED, || quantities(s) == seat_q(2));
    s.ok("Stripe bills it from the offer: the subscription's quantity follows the seats", billed, format!("{:?}", quantities(s)));
    let cy = Keys::generate();
    let cy_session = api.sign_in(&cy_email)?;
    api.approve(&cy_session, &cy)?;
    let waiting = seat(api, &cy);
    let offer = &waiting["offered"][0];
    s.ok(
        "signing in with the email attaches nothing: no seat, no org; they see the offer, its org's name, the seat, and who made it",
        waiting["seat"].is_null()
            && waiting["org"].is_null()
            && waiting["offered"].as_array().map(Vec::len) == Some(1)
            && offer["id"] == cy_seat.as_str()
            && offer["org"]["id"] == org.as_str()
            && offer["kind"] == "seat_always_on"
            && offer["admin"] == false
            && offer["from"] == ann_email.as_str()
            && ledger(api, &cy)["plan"] != "seat_always_on",
        &waiting,
    );
    let accepted = api.signed(&cy, "POST", &format!("/api/seat/offers/{cy_seat}/accept"), Some(&json!({})))?;
    let held = s.eventually(PUSHED, || ledger(api, &cy)["seat"] == "active" && ledger(api, &cy)["plan"] == "seat_always_on");
    s.ok(
        "they accept it: the seat is theirs, paid, in its org, and their ledger follows",
        accepted.status == 200 && accepted.body["seat"]["id"] == cy_seat.as_str() && accepted.body["seat"]["admin"] == false && accepted.body["offered"] == json!([]) && held,
        json!([accepted.body, ledger(api, &cy)]),
    );
    let again = api.signed(&cy, "POST", &format!("/api/seat/offers/{cy_seat}/accept"), Some(&json!({})))?;
    let declined = api.signed(&cy, "POST", &format!("/api/seat/offers/{cy_seat}/decline"), Some(&json!({})))?;
    let stranger_takes = api.signed(&api.person()?, "POST", &format!("/api/seat/offers/{cy_seat}/accept"), Some(&json!({})))?;
    s.ok(
        "accepting it again answers the same seat (a replay); declining it now, or anyone else accepting it, finds no offer (404)",
        again.status == 200 && again.body["seat"] == accepted.body["seat"] && declined.status == 404 && stranger_takes.status == 404 && seat(api, &cy)["seat"]["id"] == cy_seat.as_str(),
        json!([again.status, declined.status, stranger_takes.status]),
    );
    let r = api.signed(&ann, "PATCH", &format!("/api/org/seats/{cy_seat}"), Some(&json!({ "kind": "seat" })))?;
    s.ok("a paid seat's kind changes nowhere: one seat is sold, so there is no route (404)", r.status == 404 && seat(api, &cy)["seat"]["kind"] == "seat_always_on", &r);
    let prorated = s.stripe.requests().iter().filter(|a| a.path == format!("/v1/subscriptions/{sub2}") && a.method == "POST").all(|a| a.form.get("proration_behavior").map(String::as_str) == Some("create_prorations"));
    s.ok("every change is prorated onto the next invoice", prorated, "");

    // the seats are the truth: Stripe's count changed behind them is pushed back
    s.stripe.set_quantity(&sub2, "fragment_seat_always_on_month", 7).map_err(anyhow::Error::msg)?;
    s.stripe.set_status(&sub2, "active").map_err(anyhow::Error::msg)?;
    let repaired = s.eventually(PUSHED, || quantities(s) == seat_q(2));
    s.ok("a quantity changed in Stripe's dashboard is pushed back to the seats' count", repaired, format!("{:?}", quantities(s)));

    let not_admin = api.signed(&cy, "POST", "/api/org/seats", Some(&json!({ "email": "x@e2e.test" })))?;
    let not_admin_view = api.signed(&cy, "GET", "/api/org", None)?;
    s.ok("a seat's holder who is no admin changes nothing, nor sees the org (403)", not_admin.status == 403 && not_admin_view.status == 403, json!([not_admin.status, not_admin_view.status]));
    let r = api.signed(&ann, "DELETE", &format!("/api/org/seats/{cy_seat}"), None)?;
    let removed = s.eventually(PUSHED, || quantities(s) == seat_q(1));
    let canceled = s.eventually(PUSHED, || ledger(api, &cy)["seat"] == "canceled");
    s.ok("its admin removes it: the quantity goes down, and its holder's seat is canceled", r.status == 200 && removed && canceled, format!("{:?} {}", quantities(s), ledger(api, &cy)));
    let own = seat(api, &ann)["seat"]["id"].as_str().unwrap_or("").to_string();
    let last = api.signed(&ann, "DELETE", &format!("/api/org/seats/{own}"), None)?;
    s.ok("the org's last paid seat is not removed here: its subscription is canceled in the portal (400)", last.status == 400 && last.message().contains("cancel"), &last);

    // ---- admins: offered too, one at least, and who pays steps down only themselves
    let dee = api.person()?;
    let r = api.signed(&ann, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&dee) })))?;
    let dee_row = r.body["admin"]["id"].as_str().unwrap_or("").to_string();
    let offered = seat(api, &dee);
    let unseen = api.signed(&dee, "GET", "/api/org", None)?;
    s.ok(
        "an admin offers someone in no org an admin's place: an offer only; they are in no org, see it offered (an admin's place, no seat), and nothing of the org (403)",
        r.status == 200
            && r.body["created"] == true
            && r.body["admin"]["person"].is_null()
            && offered["org"].is_null()
            && offered["admin"] == false
            && offered["offered"][0]["id"] == dee_row.as_str()
            && offered["offered"][0]["admin"] == true
            && offered["offered"][0]["kind"].is_null()
            && unseen.status == 403,
        json!([r.body, offered, unseen.status]),
    );
    let accepted = api.signed(&dee, "POST", &format!("/api/seat/offers/{dee_row}/accept"), Some(&json!({})))?;
    let sees = api.signed(&dee, "GET", "/api/org", None)?;
    s.ok("they accept it: an admin, who sees the org and holds no seat", accepted.status == 200 && accepted.body["admin"] == true && sees.status == 200 && seat(api, &dee)["seat"].is_null(), &sees);
    let ann_row = sees.body["members"].as_array().into_iter().flatten().find(|m| m["person"] == ann_id.as_str()).and_then(|m| m["id"].as_str()).unwrap_or("").to_string();
    let removed = api.signed(&dee, "DELETE", &format!("/api/org/admins/{ann_row}"), None)?;
    let portal = api.signed(&dee, "POST", "/api/billing/portal", Some(&json!({})))?;
    s.ok(
        "who pays for the org is no other admin's to remove (403), and only they open its billing: another admin's portal is 403",
        removed.status == 403 && removed.message().contains("pays for this org") && seat(api, &ann)["admin"] == true && portal.status == 403 && seat(api, &ann)["pays"] == true && seat(api, &dee)["pays"] == false,
        json!([removed.body, portal.body]),
    );
    let r = api.signed(&ann, "DELETE", &format!("/api/org/admins/{dee_row}"), None)?;
    let gone = api.signed(&dee, "GET", "/api/org", None)?;
    s.ok("who pays removes another admin; one with no seat leaves the org with it", r.status == 200 && gone.status == 403 && seat(api, &dee)["org"].is_null(), &r);
    let last_admin = api.signed(&ann, "DELETE", &format!("/api/org/admins/{ann_row}"), None)?;
    s.ok("the last admin, who pays, stays (409: cancel its billing first)", last_admin.status == 409 && last_admin.message().contains("cancel its subscription"), &last_admin);

    // ---- credit packs: $25, kept until spent, once by their Checkout
    let purchased = |k: &Keys| ledger(api, k)["purchasedMicros"].as_i64().unwrap_or(-1);
    let before = purchased(&ann);
    let r = api.signed(&ann, "POST", "/api/billing/packs", Some(&json!({})))?;
    let pack1 = r.body["session"].as_str().unwrap_or("").to_string();
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("a pack's Checkout")?;
    let f = |k: &str| asked.form.get(k).cloned().unwrap_or_default();
    s.ok(
        "a seat's holder opens a pack's Checkout: a payment at the pack's price, for their own ledger",
        r.status == 200 && f("mode") == "payment" && f("metadata[fragment_person]") == ann_id && f("metadata[fragment_buyer]") == ann_id && f("metadata[fragment_pack]") == "25",
        format!("{:?}", asked.form),
    );
    s.stripe.complete(&pack1).map_err(anyhow::Error::msg)?;
    let granted = s.eventually(PUSHED, || purchased(&ann) == before + 25_000_000);
    let back = api.signed(&ann, "POST", &format!("/api/billing/sessions/{pack1}"), Some(&json!({})))?;
    std::thread::sleep(Duration::from_secs(1));
    s.ok("paid, $25 of credit is theirs, once (its return again adds nothing)", granted && back.status == 200 && purchased(&ann) == before + 25_000_000, ledger(api, &ann));
    let fay = api.person()?;
    let r = api.signed(&ann, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&fay) })))?;
    let fay_row = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let waits = api.signed(&ann, "POST", "/api/billing/packs", Some(&json!({ "member": fay_row })))?;
    let took = api.signed(&fay, "POST", &format!("/api/seat/offers/{fay_row}/accept"), Some(&json!({})))?;
    s.ok(
        "a seat offered to someone in no org is theirs once they accept it (no credit is bought for it before: 400)",
        r.status == 200 && r.body["seat"]["person"].is_null() && waits.status == 400 && took.status == 200 && took.body["seat"]["id"] == fay_row.as_str(),
        json!([r.body, waits.body, took.body]),
    );
    let fay_before = purchased(&fay);
    let r = api.signed(&ann, "POST", "/api/billing/packs", Some(&json!({ "member": fay_row })))?;
    let pack2 = r.body["session"].as_str().unwrap_or("").to_string();
    s.stripe.complete(&pack2).map_err(anyhow::Error::msg)?;
    let theirs = s.eventually(PUSHED, || purchased(&fay) == fay_before + 25_000_000);
    s.ok("an org's admin buys a pack for a member who holds a seat: the credit is the member's", r.status == 200 && theirs && purchased(&ann) == before + 25_000_000, ledger(api, &fay));
    let not_admin = api.signed(&fay, "POST", "/api/billing/packs", Some(&json!({ "member": fay_row })))?;
    let seatless = api.signed(&api.person()?, "POST", "/api/billing/packs", Some(&json!({})))?;
    s.ok("a member who is no admin buys none for others (403); someone with no seat buys none (400)", not_admin.status == 403 && seatless.status == 400, json!([not_admin.status, seatless.status]));

    // ---- an admin's place for one of its members is an offer too; its
    // accept merges it into their row, and replays
    let r = api.signed(&ann, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&fay) })))?;
    let promo = r.body["admin"]["id"].as_str().unwrap_or("").to_string();
    let unseen = api.signed(&fay, "GET", "/api/org", None)?;
    let waiting = seat(api, &fay);
    let took = answer(api, &fay, &promo, "accept")?;
    let again = answer(api, &fay, &promo, "accept")?;
    s.ok(
        "an admin's place for one of the org's members is an offer: they are no admin, and see nothing of the org (403), until they accept it; accepted, it joins their own row (their seat kept), and accepting it again answers the same (a replay)",
        r.status == 200
            && r.body["admin"]["person"].is_null()
            && promo != fay_row
            && unseen.status == 403
            && waiting["admin"] == false
            && waiting["offered"][0]["id"] == promo.as_str()
            && took.status == 200
            && took.body["admin"] == true
            && took.body["seat"]["id"] == fay_row.as_str()
            && took.body["offered"] == json!([])
            && again.status == 200
            && again.body == took.body,
        json!([r.body, took.body, again.body]),
    );

    // ---- invalid
    let refused = [
        ("the $100 seat, no longer sold (400)", api.signed(&stranger, "POST", "/api/billing/checkout", Some(&json!({ "kind": "seat" })))?, 400),
        ("a kind that is no seat's (400)", api.signed(&stranger, "POST", "/api/billing/checkout", Some(&json!({ "kind": "guest" })))?, 400),
        ("a field it does not take (400)", api.signed(&stranger, "POST", "/api/billing/checkout", Some(&json!({ "kind": "seat_always_on", "quantity": 3 })))?, 400),
        ("unsigned (401)", api.unsigned("POST", "/api/billing/checkout", Some(&json!({})))?, 401),
        ("an offer that is none (404)", api.signed(&stranger, "POST", "/api/seat/offers/mem-0000000000000000/accept", Some(&json!({})))?, 404),
        ("an offer's id that is no id (404)", api.signed(&stranger, "POST", "/api/seat/offers/nonesuch/decline", Some(&json!({})))?, 404),
        ("an answer with a body (400)", api.signed(&stranger, "POST", &format!("/api/seat/offers/{cy_seat}/accept"), Some(&json!({ "seat": "seat_always_on" })))?, 400),
        ("an answer unsigned (401)", api.unsigned("POST", &format!("/api/seat/offers/{cy_seat}/accept"), Some(&json!({})))?, 401),
        ("leaving with no org (400)", api.signed(&stranger, "POST", "/api/seat/leave", Some(&json!({})))?, 400),
    ];
    for (what, r, status) in refused {
        s.ok(&format!("refused: {what}"), r.status == status, &r);
    }
    let payer = consent(s, api)?;
    payer_goes(s, api)?;
    orphan(s, api)?;
    org_credit(s, api)?;
    wiped(s, api, &payer)
}

/// An org's own credit (decisions 66 to 68): orgs pay out of band (a
/// Stripe link, an invoice), so an operator credits an org's ledger and
/// sets its budget, with `fragment operator credit|budget`, once by its
/// id, logged; and names who pays (`fragment operator payer`), in its
/// governance log. Its billing admins read it; no one else does, nor sets
/// any of it, and the self-serve pack and budget are not there. Its credit
/// and budget read back after a restart.
fn org_credit(s: &mut Suite, api: &Api) -> Result<()> {
    let vic = api.person()?;
    let vic_id = api.identity(&vic)?;
    let org = buys(s, api, &vic)?;
    let root = format!("/api/orgs/{org}");
    let read = |api: &Api, who: &Keys| api.signed(who, "GET", &format!("{root}/ledger"), None).map(|r| r.body).unwrap_or(Value::Null);
    let fresh = read(api, &vic);
    s.ok(
        "an org's ledger starts with nothing: no credit, a budget of zero, read-only, and no plan, seat or included credit",
        fresh["org"] == org.as_str()
            && fresh["balanceMicros"] == 0
            && fresh["budgetMicros"] == 0
            && fresh["standing"] == json!({ "standing": "read_only", "why": "overdrawn" })
            && fresh.get("plan").is_none()
            && fresh.get("includedMicros").is_none(),
        &fresh,
    );
    // the operator's CLI, with their key's file
    let dir = s.dir("org-credit");
    let file = dir.join("operator.key");
    std::fs::write(&file, format!("{}\n", s.operator.secret_hex()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    let (home, key) = (s.dir("org-credit-cli"), file.to_string_lossy().to_string());
    let credit = s.cli_json(api, &home, &["operator", "credit", &org, "20", "--why", "a pilot, invoiced", "--id", "fund-1", "--key-file", &key, "--json"]);
    let after = read(api, &vic);
    let logged = api.signed(&s.operator, "GET", "/api/admin/log", None)?.body["entries"].as_array().is_some_and(|e| e.iter().any(|x| x["action"] == "ledger-grant" && x["target"] == format!("org:{org}").as_str()));
    s.ok(
        "operator_credit_funds_org: `fragment operator credit` credits the org's own ledger, logged; it takes writes, and none of its people's ledgers moved",
        credit.as_ref().is_ok_and(|v| v["org"] == org.as_str()) && after["balanceMicros"] == 20_000_000 && after["standing"] == json!({ "standing": "ok" }) && logged,
        json!({ "cli": format!("{credit:?}"), "org": after }),
    );
    let again = s.cli_json(api, &home, &["operator", "credit", &org, "20", "--why", "a pilot, invoiced", "--id", "fund-1", "--key-file", &key, "--json"]);
    let budget = s.cli_json(api, &home, &["operator", "budget", &org, "10", "--id", "b-1", "--key-file", &key, "--json"]);
    let budget_again = s.cli_json(api, &home, &["operator", "budget", &org, "10", "--id", "b-1", "--key-file", &key, "--json"]);
    // each logged once, in the org's governance log and the operators' log
    let logs = |api: &Api| -> Result<(usize, usize)> {
        let governed = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?.body["log"].as_array().map_or(0, |l| l.iter().filter(|e| e["action"] == "ledger-grant").count());
        let noted = api.signed(&s.operator, "GET", "/api/admin/log", None)?.body["entries"].as_array().map_or(0, |e| e.iter().filter(|x| x["action"] == "ledger-grant" && x["target"] == format!("org:{org}").as_str()).count());
        Ok((governed, noted))
    };
    let logged = logs(api)?;
    s.ok(
        "credit_granted_once: the same credit's id again credits nothing more, and is logged once, in the org's governance log and the operators'; its budget, set by `fragment operator budget`, likewise once by its id",
        again.is_ok() && budget.is_ok() && budget_again.is_ok() && read(api, &vic)["balanceMicros"] == 20_000_000 && read(api, &vic)["budgetMicros"] == 10_000_000 && logged == (1, 1),
        json!([format!("{again:?}"), format!("{budget:?}"), read(api, &vic), logged]),
    );
    let payer = s.cli_json(api, &home, &["operator", "payer", &org, &Api::email_of(&vic), "--id", "p-1", "--key-file", &key, "--json"]);
    let view = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?;
    let named = view.body["log"].as_array().is_some_and(|l| l.iter().any(|e| e["action"] == "payer-named" && e["subject"] == vic_id.as_str()));
    s.ok(
        "`fragment operator payer` names who the org's credit is invoiced to, in its governance log",
        payer.as_ref().is_ok_and(|v| v["person"] == vic_id.as_str()) && named,
        json!({ "cli": format!("{payer:?}"), "log": view.body["log"] }),
    );
    // payer_named_once: the same id again is the same entry; another person under it, 409
    let payer_at = format!("/api/admin/orgs/{org}/payer");
    let again = api.signed(&s.operator, "POST", &payer_at, Some(&json!({ "id": "p-1", "person": Api::email_of(&vic) })))?;
    let yan = api.person()?;
    let other = api.signed(&s.operator, "POST", &payer_at, Some(&json!({ "id": "p-1", "person": Api::email_of(&yan) })))?;
    let entries = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?.body["log"].as_array().map_or(0, |l| l.iter().filter(|e| e["action"] == "payer-named").count());
    s.ok(
        "payer_named_once: naming the payer again by its id answers the same entry, another person under it is 409, and the log holds one",
        again.status == 200 && payer.as_ref().is_ok_and(|v| v["seq"] == again.body["seq"]) && entries == 1 && other.status == 409,
        json!({ "again": again.body, "other": [other.status, other.body], "entries": entries }),
    );

    // nonoperator_cannot_credit, and the self-serve routes are not there
    let wen = api.person()?;
    let r = api.signed(&vic, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&wen) })))?;
    answer(api, &wen, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    let xan = api.person()?;
    let op = api.identity(&s.operator)?;
    // an agent of who pays, signing as itself
    let hand = Keys::generate();
    let reg = "/api/identities";
    let made = api.signed(&vic, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hand, "POST", reg, &vic) })))?;
    anyhow::ensure!(made.status == 200, "an agent of vic's: {made}");
    let refused = [
        ("a billing admin credits nothing: an operator's (403)", api.signed(&wen, "POST", &format!("/api/ledger/org:{org}/grant"), Some(&json!({ "id": "mine", "micros": 1, "by": api.identity(&wen)?, "why": "" })))?, 403),
        ("nor sets its budget (403)", api.signed(&wen, "POST", &format!("/api/ledger/org:{org}/budget"), Some(&json!({ "id": "mine", "micros": 1 })))?, 403),
        ("nor names its payer (403)", api.signed(&wen, "POST", &format!("/api/admin/orgs/{org}/payer"), Some(&json!({ "id": "mine", "person": Api::email_of(&wen) })))?, 403),
        ("nor reads it as an operator (403)", api.signed(&wen, "GET", &format!("/api/admin/orgs/{org}/ledger"), None)?, 403),
        ("an agent of who pays credits nothing (403)", api.signed(&hand, "POST", &format!("/api/ledger/org:{org}/grant"), Some(&json!({ "id": "bot", "micros": 1, "by": api.identity(&hand)?, "why": "" })))?, 403),
        ("nor sets its budget (403)", api.signed(&hand, "POST", &format!("/api/ledger/org:{org}/budget"), Some(&json!({ "id": "bot", "micros": 1 })))?, 403),
        ("nor names its payer (403)", api.signed(&hand, "POST", &format!("/api/admin/orgs/{org}/payer"), Some(&json!({ "id": "bot", "person": Api::email_of(&vic) })))?, 403),
        ("the self-serve budget is not there (404)", api.signed(&wen, "PUT", &format!("{root}/budget"), Some(&json!({ "id": "b", "micros": 1 })))?, 404),
        ("nor is the self-serve pack (404)", api.signed(&vic, "POST", &format!("{root}/billing/pack"), Some(&json!({})))?, 404),
        ("someone not in it reads none of it (403)", api.signed(&xan, "GET", &format!("{root}/ledger"), None)?, 403),
        ("an org that is not there takes no credit (404)", api.signed(&s.operator, "POST", "/api/ledger/org:org-0000000000000000/grant", Some(&json!({ "id": "x", "micros": 1, "by": op, "why": "" })))?, 404),
        ("its ledger takes no plan (403)", api.signed(&s.operator, "POST", &format!("/api/ledger/org:{org}/plan"), Some(&json!({ "id": "p", "plan": "seat" })))?, 403),
        ("a budget past its limit (400)", api.signed(&s.operator, "POST", &format!("/api/ledger/org:{org}/budget"), Some(&json!({ "id": "huge", "micros": 100_000_000_001_i64 })))?, 400),
        ("a payer no one on the fleet is (404)", api.signed(&s.operator, "POST", &format!("/api/admin/orgs/{org}/payer"), Some(&json!({ "id": "nobody", "person": "nobody@e2e.test" })))?, 404),
    ];
    let mut all = true;
    for (what, r, status) in &refused {
        all &= r.status == *status;
        if r.status != *status {
            println!("      {what}: {} {}", r.status, r.body);
        }
    }
    let bad = s.cli(api, &home, &["operator", "credit", "org-nope", "5", "--why", "x", "--id", "x", "--key-file", &key]);
    s.ok(
        "nonoperator_cannot_credit: no one but an operator credits it, budgets it or names its payer; the self-serve routes are not there; the CLI names no org it cannot parse",
        all && !bad.status.success(),
        json!(refused.iter().map(|(w, r, _)| json!([w, r.status])).collect::<Vec<_>>()),
    );

    // funded_org_outlives_its_people (Alex's review): an org's credit is in
    // its log before its ledger takes it, so its last person leaving while
    // the grant is cut short (its ledger not reached) drops nothing, and
    // the same command after a restart finishes it, logged once
    let zed = api.person()?;
    let comped = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&zed), "kind": "seat" })))?;
    let zorg = comped.body["org"]["id"].as_str().context("zed's comped org")?.to_string();
    let grant = json!({ "id": "inv-z", "micros": 5_000_000, "by": op, "why": "an invoice" });
    let grant_at = format!("/api/ledger/org:{zorg}/grant");
    let lever = api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": format!("org:{zorg}"), "op": "fail-grants", "times": 1 })))?;
    let cut = api.signed(&s.operator, "POST", &grant_at, Some(&grant))?;
    // and one no operator credited, its person moving after: once it is
    // dropped (its push, queued after the other's, ran), the other's ran too
    let yul = api.person()?;
    let comped = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&yul), "kind": "seat" })))?;
    let yorg = comped.body["org"]["id"].as_str().context("yul's comped org")?.to_string();
    let mut moved = Vec::new();
    for who in [&zed, &yul] {
        let r = api.signed(&vic, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(who) })))?;
        moved.push(answer(api, who, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?.status);
    }
    let control_gone = s.eventually(PUSHED, || api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{yorg}"), None).is_ok_and(|r| r.status == 404));
    // a billing admin adds people to the org as its content admins do (Paul,
    // 2026-10-31): a seat accepted there puts its holder in its content,
    // the join logged as theirs
    let content = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{org}/content"), None)?.body;
    let zed_id = api.identity(&zed)?;
    let in_content = content["people"].as_array().is_some_and(|p| p.iter().any(|x| x["person"] == zed_id.as_str() && x["admin"] == false));
    let joined = content["log"].as_array().is_some_and(|l| l.iter().any(|e| e["action"] == "joined" && e["subject"] == zed_id.as_str() && e["detail"]["by"] == "seat"));
    s.ok("a seat a billing admin offered, accepted, puts its holder in the org's content, the join logged as theirs", in_content && joined, &content);
    let kept = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{zorg}"), None)?;
    let none_yet = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{zorg}/ledger"), None)?;
    s.ok(
        "funded_org_outlives_its_people: an org's grant cut short after its log took it (its ledger not reached), its only person gone to another org, the org stays, while one with nothing of its own is dropped",
        lever.status == 200 && cut.status >= 500 && moved == [200, 200] && control_gone && org_of(api, &zed) == org && kept.status == 200 && none_yet.body["balanceMicros"] == 0,
        json!([lever.body, [cut.status, cut.body.clone()], moved, control_gone, kept.status, none_yet.body]),
    );

    // org_terms_survive_restart: its credit and budget, read back after a restart
    let restarted;
    let api = if s.runs("billing", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    let back = read(api, &vic);
    s.ok(
        "org_terms_survive_restart: after a restart the org's ledger is its own still, its credit and its budget (its own row, beside the head) as they were",
        back["balanceMicros"] == 20_000_000 && back["budgetMicros"] == 10_000_000 && back["standing"] == json!({ "standing": "ok" }),
        &back,
    );
    // the cut grant, sent again after the restart, lands once; the same
    // id with another body is refused before any ledger sees it
    let done = api.signed(&s.operator, "POST", &grant_at, Some(&grant))?;
    let again = api.signed(&s.operator, "POST", &grant_at, Some(&grant))?;
    let other = api.signed(&s.operator, "POST", &grant_at, Some(&json!({ "id": "inv-z", "micros": 9_000_000, "by": op, "why": "an invoice" })))?;
    let zl = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{zorg}/ledger"), None)?;
    let governed = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{zorg}/content"), None)?.body["log"].as_array().map_or(0, |l| l.iter().filter(|e| e["action"] == "ledger-grant" && e["request"] == "grant:inv-z").count());
    let noted = api.signed(&s.operator, "GET", "/api/admin/log", None)?.body["entries"].as_array().map_or(0, |e| e.iter().filter(|x| x["action"] == "ledger-grant" && x["target"] == format!("org:{zorg}").as_str()).count());
    s.ok(
        "a cut-short grant, sent again after a restart, credits the org once, its credit there for an operator; logged once in its governance log and the operators'; its id with another body is 409",
        done.status == 200 && again.status == 200 && other.status == 409 && zl.status == 200 && zl.body["balanceMicros"] == 5_000_000 && governed == 1 && noted == 1,
        json!([done.body, again.body, [other.status, other.body.clone()], zl.body, governed, noted]),
    );
    Ok(())
}

/// The admins' view of `org`, as an operator lists it (its customer and
/// subscription the registry keeps), or null.
fn listed_org(s: &Suite, api: &Api, org: &str) -> Value {
    let all = api.signed(&s.operator, "GET", "/api/admin/orgs", None).map(|r| r.body).unwrap_or(Value::Null);
    all["orgs"].as_array().into_iter().flatten().find(|o| o["id"] == org).cloned().unwrap_or(Value::Null)
}

/// Who pays (the review of #367; Alex's rule, Paul's call, 2026-10-18):
/// their seat and admin's place are no other admin's, and only they open
/// its billing; while its subscription lives (set to end at its period's
/// end too) they stay: their leave, step-down and move are each refused,
/// saying to cancel first; a non-payer admin goes as they will; once it
/// has ended the payer goes too; the lapsed org's offer is refused; and
/// another admin buying it again bills a customer of their own.
fn payer_goes(s: &mut Suite, api: &Api) -> Result<()> {
    let pia = api.person()?;
    let pia_id = api.identity(&pia)?;
    let p_org = buys(s, api, &pia)?;
    let p_sub = subscription_of(s, &p_org).context("the payer's subscription")?;
    let pia_customer = s.stripe.subscription(&p_sub).map(|x| x.customer).unwrap_or_default();
    let quin = api.person()?;
    let r = api.signed(&pia, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&quin) })))?;
    answer(api, &quin, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    let uri = api.person()?;
    let r = api.signed(&pia, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&uri) })))?;
    answer(api, &uri, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    let rho = api.person()?;
    let r = api.signed(&quin, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&rho) })))?;
    answer(api, &rho, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    let view = api.signed(&quin, "GET", "/api/org", None)?;
    let row_of = |id: &str| view.body["members"].as_array().into_iter().flatten().find(|m| m["person"] == id).and_then(|m| m["id"].as_str()).unwrap_or("").to_string();
    let pia_row = row_of(&pia_id);
    let uri_row = row_of(&api.identity(&uri)?);
    let seat_taken = api.signed(&quin, "DELETE", &format!("/api/org/seats/{pia_row}"), None)?;
    let place_taken = api.signed(&quin, "DELETE", &format!("/api/org/admins/{pia_row}"), None)?;
    let theirs = api.signed(&quin, "POST", "/api/billing/portal", Some(&json!({})))?;
    let own = api.signed(&pia, "POST", "/api/billing/portal", Some(&json!({})))?;
    s.ok(
        "who pays: another admin takes neither their seat nor their admin's place (403), nor opens its billing (403); they open it themselves",
        seat_taken.status == 403 && place_taken.status == 403 && theirs.status == 403 && own.status == 200 && view.body["payer"] == pia_id.as_str(),
        json!([seat_taken.body, place_taken.body, theirs.body, own.status]),
    );

    // a non-payer admin goes as they will, its subscription live: by
    // stepping down (an admin's leave is refused until then)
    let leaves = api.signed(&uri, "POST", "/api/seat/leave", Some(&json!({})))?;
    let stepped = api.signed(&uri, "DELETE", &format!("/api/org/admins/{uri_row}"), None)?;
    let gone = s.eventually(PUSHED, || org_of(api, &uri).is_empty());
    s.ok(
        "an admin who does not pay leaves only by stepping down first (400); stepping down while its subscription lives, holding no seat, they are out of the org",
        leaves.status == 400 && leaves.message().contains("step down") && stepped.status == 200 && gone && seat(api, &uri)["org"].is_null(),
        json!([leaves.body, stepped.body]),
    );

    // who pays stays while it lives, and while it is set to end
    let tia = api.person()?;
    buys(s, api, &tia)?;
    let r = api.signed(&tia, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&pia) })))?;
    let to_pia = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let tries = || -> Result<Vec<crate::api::Reply>> {
        Ok(vec![
            api.signed(&pia, "POST", "/api/seat/leave", Some(&json!({})))?,
            api.signed(&pia, "DELETE", &format!("/api/org/admins/{pia_row}"), None)?,
            answer(api, &pia, &to_pia, "accept")?,
        ])
    };
    let refused = |r: &[crate::api::Reply]| r.iter().all(|r| r.status == 409 && r.message().contains("cancel its subscription")) && org_of(api, &pia) == p_org && seat(api, &pia)["admin"] == true;
    let live = tries()?;
    s.ok("who pays stays while its subscription lives: their leave, step-down and move are each refused (409), saying to cancel it first", refused(&live), json!(live.iter().map(|r| &r.body).collect::<Vec<_>>()));
    s.stripe.set_cancel_at_end(&p_sub, true).map_err(anyhow::Error::msg)?;
    let set = s.eventually(PUSHED, || listed_org(s, api, &p_org)["cancelAtEnd"] == true);
    let ending = tries()?;
    s.ok(
        "and while it is set to end at its period's end (theirs to renew): each is refused still, and the subscription bills on, not canceled",
        set && refused(&ending) && s.stripe.subscription(&p_sub).is_some_and(|x| x.status == "active"),
        json!(ending.iter().map(|r| &r.body).collect::<Vec<_>>()),
    );

    // an offer waiting, answered after the org lapses
    let sam = api.person()?;
    let r = api.signed(&quin, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&sam) })))?;
    let sam_offer = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    // the period ends: it is canceled; who paid goes now
    s.stripe.set_status(&p_sub, "canceled").map_err(anyhow::Error::msg)?;
    let lapsed = s.eventually(PUSHED, || seat(api, &rho)["seat"]["good"] == false);
    let stepped = api.signed(&pia, "DELETE", &format!("/api/org/admins/{pia_row}"), None)?;
    let left = api.signed(&pia, "POST", "/api/seat/leave", Some(&json!({})))?;
    s.ok(
        "once its subscription has ended, who paid steps down and leaves: they are out of the org, its paid seats lapsed",
        lapsed && stepped.status == 200 && left.status == 200 && seat(api, &pia)["org"].is_null() && seat(api, &pia)["pays"] == false,
        json!([stepped.body, left.body]),
    );
    let late = answer(api, &sam, &sam_offer, "accept")?;
    let portal = api.signed(&quin, "POST", "/api/billing/portal", Some(&json!({})))?;
    s.ok(
        "the lapsed org's offer is refused (400) until it pays again, and its billing is no one else's (403)",
        late.status == 400 && late.message().contains("pays for nothing") && seat(api, &sam)["org"].is_null() && portal.status == 403,
        json!([late.body, portal.body]),
    );

    // another admin buys it again: on a customer of their own
    let paid_seats = api.signed(&quin, "GET", "/api/org", None)?.body["members"].as_array().map(|m| m.iter().filter(|m| m["seat"].is_string() && m["comped"] == false).count()).unwrap_or(0);
    let r = api.signed(&quin, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("the co-admin's Checkout")?;
    s.stripe.complete(r.body["session"].as_str().unwrap_or("")).map_err(anyhow::Error::msg)?;
    let pays = s.eventually(PUSHED, || seat(api, &quin)["seat"]["good"] == true);
    let again = subscription_of(s, &p_org).and_then(|id| s.stripe.subscription(&id)).map(|x| x.customer).unwrap_or_default();
    s.ok(
        "another admin buys the org again on a customer of their own (its email theirs), never who paid before's, for its paid seats and theirs; now they pay",
        r.status == 200
            && !asked.form.contains_key("customer")
            && asked.form.get("customer_email") == Some(&Api::email_of(&quin))
            && asked.form.get("line_items[0][quantity]") == Some(&(paid_seats + 1).to_string())
            && pays
            && !again.is_empty()
            && again != pia_customer
            && seat(api, &quin)["pays"] == true,
        json!({ "form": format!("{:?}", asked.form), "customer": again }),
    );
    Ok(())
}

/// A Checkout whose completion never reaches the platform (its webhook
/// lost, its buyer's return never made): its subscription's event names an
/// org the registry lacks, and the platform finds the Checkout that made
/// it and seats its buyer.
fn orphan(s: &mut Suite, api: &Api) -> Result<()> {
    let ola = api.person()?;
    let r = api.signed(&ola, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let session = r.body["session"].as_str().unwrap_or("").to_string();
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("the Checkout")?;
    let named = asked.form.get("metadata[fragment_org]").cloned().unwrap_or_default();
    s.stripe.hold_events(true);
    s.stripe.complete(&session).map_err(anyhow::Error::msg)?;
    s.stripe.drop_held("checkout.session.completed");
    s.stripe.release_events(false);
    let seated = s.eventually(PUSHED, || seat(api, &ola)["seat"]["good"] == true);
    s.ok(
        "a Checkout's completion lost, and no return: the subscription's own event names an org not made yet, and the platform finds the Checkout that made it and makes the org, its buyer seated",
        r.status == 200 && seated && org_of(api, &ola) == named,
        seat(api, &ola),
    );
    Ok(())
}

/// The id of the org `keys` is in (`GET /api/seat`'s), or "".
pub(super) fn org_of(api: &Api, keys: &Keys) -> String {
    seat(api, keys)["org"]["id"].as_str().unwrap_or("").to_string()
}

/// `keys` buys their own seat on the fake's Checkout and holds it, paid.
/// Answers the org it made.
pub(super) fn buys(s: &Suite, api: &Api, keys: &Keys) -> Result<String> {
    let r = api.signed(keys, "POST", "/api/billing/checkout", Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "a Checkout: {r}");
    s.stripe.complete(r.body["session"].as_str().unwrap_or("")).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(s.eventually(PUSHED, || seat(api, keys)["seat"]["good"] == true), "the seat bought holds: {}", seat(api, keys));
    Ok(org_of(api, keys))
}

/// Calls a wipe may take here to finish.
const WIPE_CALLS_MAX: usize = 5;

/// The wipe of the identity `who`, called again until its report says it
/// is wiped (its own completion signal), as `fragment operator wipe --yes`
/// calls it: each call goes on where the last stopped, so a step that
/// waits (its `billing` step, while a push a late webhook queued for its
/// org is still to land) is done by a later call. At most
/// `WIPE_CALLS_MAX` calls, none waiting on a clock. Answers the last
/// report, and every step each call ran, in order.
pub(super) fn wipe_until_done(api: &Api, wiper: &Keys, who: &str) -> Result<(crate::api::Reply, Vec<Value>)> {
    let mut ran = vec![];
    // bounded: WIPE_CALLS_MAX calls
    for call in 1..=WIPE_CALLS_MAX {
        let r = api.signed(wiper, "POST", &format!("/api/people/{who}/wipe"), Some(&json!({ "confirm": who })))?;
        ran.extend(r.body["ran"].as_array().cloned().unwrap_or_default());
        if r.status != 200 || r.body["state"] == "wiped" || call == WIPE_CALLS_MAX {
            return Ok((r, ran));
        }
        println!("      (the wipe of {who} goes on in call {}: {} waited: {})", call + 1, r.body["next"], billing_ran(&ran)["note"]);
    }
    unreachable!("the last call answers")
}

/// The `billing` step a wipe's calls ran last (`Null`: none ran it).
fn billing_ran(ran: &[Value]) -> Value {
    ran.iter().rev().find(|x| x["step"] == "billing").cloned().unwrap_or(Value::Null)
}

/// `who` answers the offer `id` (`accept` or `decline`).
pub(super) fn answer(api: &Api, who: &Keys, id: &str, verb: &str) -> Result<crate::api::Reply> {
    api.signed(who, "POST", &format!("/api/seat/offers/{id}/{verb}"), Some(&json!({})))
}

/// The security audit's H1 (2026-10-17), each step refused: no org or
/// customer before a payment; an admin's place or a seat offered attaches
/// no one, at an offer or a sign-in, until they accept; someone offered
/// buys their own seat on a customer of their own, never the offering
/// org's; they decline; and their own yes elsewhere moves them unless it
/// leaves people with no admin (the last admin is refused) or they pay for
/// their org while its subscription lives (they cancel first); an org of
/// one, its billing ended, ending with them, and one with only offers
/// waiting too, the offers withdrawn. A seat's holder leaves by their own
/// yes. Answers a person whose org pays, for `wiped`'s offers.
fn consent(s: &mut Suite, api: &Api) -> Result<Keys> {
    let mal = api.person()?;
    let mal_email = Api::email_of(&mal);
    let vic = api.person()?;
    let vic_email = Api::email_of(&vic);

    // 1. a Checkout begun and left makes nothing, so it offers no one anything
    let customers = s.stripe.customers().len();
    let r = api.signed(&mal, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let session = r.body["session"].as_str().unwrap_or("").to_string();
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("a Checkout was asked for")?;
    let named = asked.form.get("metadata[fragment_org]").cloned().unwrap_or_default();
    let looked = api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{named}"), None)?;
    let forced = api.signed(&mal, "POST", "/api/org/admins", Some(&json!({ "email": vic_email })))?;
    let seated = api.signed(&mal, "POST", "/api/org/seats", Some(&json!({ "email": vic_email })))?;
    s.ok(
        "H1: a Checkout begun and left makes no org (the one it names is none: 404) and no customer, so its buyer admins nothing and offers no one an admin's place or a seat (403)",
        r.status == 200 && seat(api, &mal)["org"].is_null() && looked.status == 404 && s.stripe.customers().len() == customers && forced.status == 403 && seated.status == 403,
        json!({ "org": looked.status, "admins": forced.body, "seats": seated.body }),
    );

    // 2. paid, its admin offers; an offer attaches no one, at once or at a sign-in
    s.stripe.complete(&session).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(s.eventually(PUSHED, || seat(api, &mal)["seat"]["good"] == true), "the attacker's seat holds");
    let a_org = org_of(api, &mal);
    let r = api.signed(&mal, "POST", "/api/org/admins", Some(&json!({ "email": vic_email })))?;
    let theirs = seat(api, &vic);
    let unseen = api.signed(&vic, "GET", "/api/org", None)?;
    let again = api.sign_in(&vic_email).map(|_| seat(api, &vic))?;
    s.ok(
        "H1: an admin's place offered to someone in no org attaches nothing: they are in none, and see the offer, its org and who made it, and nothing of the org (403); signing in again attaches nothing",
        r.status == 200
            && r.body["admin"]["person"].is_null()
            && theirs["org"].is_null()
            && theirs["admin"] == false
            && theirs["offered"][0]["org"]["id"] == a_org.as_str()
            && theirs["offered"][0]["admin"] == true
            && theirs["offered"][0]["from"] == mal_email.as_str()
            && unseen.status == 403
            && again["org"].is_null(),
        json!([r.body, theirs, unseen.status, again]),
    );
    let new_email = format!("billing-new-{}@e2e.test", &Keys::generate().pubkey_hex()[..10]);
    let r = api.signed(&mal, "POST", "/api/org/seats", Some(&json!({ "email": new_email })))?;
    let nu = Keys::generate();
    let nu_session = api.sign_in(&new_email)?;
    api.approve(&nu_session, &nu)?;
    let fresh = seat(api, &nu);
    s.ok(
        "H1: a seat offered to an email no one signs in with attaches no one at their first sign-in: they hold none, in no org, until they accept",
        r.status == 200 && fresh["seat"].is_null() && fresh["org"].is_null() && fresh["offered"][0]["kind"] == "seat_always_on" && ledger(api, &nu)["plan"] != "seat_always_on",
        &fresh,
    );

    // 3. offered, they buy their own seat: never on the offering org's customer
    let a_customer = subscription_of(s, &a_org).and_then(|id| s.stripe.subscription(&id)).map(|x| x.customer).context("the attacker's org's customer")?;
    let r = api.signed(&vic, "POST", "/api/billing/checkout", Some(&json!({})))?;
    let asked = s.stripe.requests().into_iter().rev().find(|a| a.path == "/v1/checkout/sessions").context("the victim's Checkout")?;
    let mal_id = api.identity(&mal)?;
    let none_other = s.stripe.requests().iter().filter(|q| q.path == "/v1/checkout/sessions").all(|q| q.form.get("customer") != Some(&a_customer) || q.form.get("metadata[fragment_person]") == Some(&mal_id));
    s.ok(
        "H1: with an offer waiting, they buy their own seat: a Checkout for a new org of theirs, on a customer Stripe makes for their email, and no Checkout but its buyer's own ever bills the offering org's customer",
        r.status == 200
            && !asked.form.contains_key("customer")
            && asked.form.get("customer_email") == Some(&vic_email)
            && asked.form.get("metadata[fragment_org]").is_some_and(|o| *o != a_org)
            && none_other,
        format!("{:?}", asked.form),
    );
    s.stripe.complete(r.body["session"].as_str().unwrap_or("")).map_err(anyhow::Error::msg)?;
    let paid = s.eventually(PUSHED, || seat(api, &vic)["seat"]["good"] == true);
    let b_org = org_of(api, &vic);
    let b_sub = subscription_of(s, &b_org).context("the victim's subscription")?;
    s.ok(
        "paid, they admin an org of their own, its customer their own, and the offer still waits on their answer",
        paid && b_org != a_org && seat(api, &vic)["admin"] == true && seat(api, &vic)["offered"].as_array().map(Vec::len) == Some(1) && s.stripe.subscription(&b_sub).is_some_and(|x| x.customer != a_customer),
        seat(api, &vic),
    );

    // 4. they decline it
    let offer = seat(api, &vic)["offered"][0]["id"].as_str().unwrap_or("").to_string();
    let declined = answer(api, &vic, &offer, "decline")?;
    let twice = answer(api, &vic, &offer, "decline")?;
    s.ok(
        "they decline it: it is gone; declining it again finds none (404); they stay in their own org",
        declined.status == 200 && declined.body["offered"] == json!([]) && twice.status == 404 && org_of(api, &vic) == b_org,
        json!([declined.body, twice.status]),
    );

    // 5. paying for their own org, they stay until its subscription ends;
    // then, alone in it, a move ends it
    let r = api.signed(&mal, "POST", "/api/org/seats", Some(&json!({ "email": vic_email })))?;
    let to_vic = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let b_customer = s.stripe.subscription(&b_sub).map(|x| x.customer).unwrap_or_default();
    let refused = answer(api, &vic, &to_vic, "accept")?;
    s.stripe.set_status(&b_sub, "canceled").map_err(anyhow::Error::msg)?;
    let lapsed = s.eventually(PUSHED, || seat(api, &vic)["seat"]["good"] == false);
    let moved = answer(api, &vic, &to_vic, "accept")?;
    let ended = s.eventually(PUSHED, || api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{b_org}"), None).is_ok_and(|r| r.status == 404));
    s.ok(
        "who pays for their own org stays while its subscription lives (409: cancel it first); once it has ended, alone in it, they accept another's seat: their org ends with them, its Stripe customer kept, and they hold the seat offered",
        refused.status == 409
            && refused.message().contains("cancel its subscription")
            && lapsed
            && moved.status == 200
            && moved.body["seat"]["id"] == to_vic.as_str()
            && moved.body["org"]["id"] == a_org.as_str()
            && ended
            && s.stripe.customers().contains(&b_customer),
        json!({ "refused": refused.body, "moved": moved.body }),
    );

    // 6. a seat's holder leaves by their own yes; an admin steps down first
    let left = api.signed(&vic, "POST", "/api/seat/leave", Some(&json!({})))?;
    let gone = s.eventually(PUSHED, || ledger(api, &vic)["seat"] == "canceled");
    let twice = api.signed(&vic, "POST", "/api/seat/leave", Some(&json!({})))?;
    let admin_leaves = api.signed(&mal, "POST", "/api/seat/leave", Some(&json!({})))?;
    s.ok(
        "a seat's holder who is no admin leaves: their seat ends and they are in no org; leaving again is 400; who pays for their org is refused leaving (409: cancel first)",
        left.status == 200 && left.body["org"].is_null() && gone && twice.status == 400 && admin_leaves.status == 409 && admin_leaves.message().contains("cancel its subscription"),
        json!([left.body, twice.status, admin_leaves.body]),
    );

    // 7. the last admin of an org others are in is not moved from it, and
    // an org that pays for nothing (an operator's comp) offers nothing
    let quinn = api.person()?;
    let rae = api.person()?;
    let comp = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&quinn), "kind": "seat" })))?;
    let q_org = comp.body["org"]["id"].as_str().unwrap_or("").to_string();
    let rae_comp = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&rae), "kind": "seat", "org": q_org })))?;
    let r = api.signed(&mal, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&quinn) })))?;
    let moved = answer(api, &quinn, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    let offers = api.signed(&quinn, "POST", "/api/org/admins", Some(&json!({ "email": vic_email })))?;
    s.ok(
        "the last admin of an org someone else is in is refused a move (409: no one would manage it); an org that pays for nothing offers no admin's place (400)",
        comp.status == 200 && rae_comp.status == 200 && rae_comp.body["seat"]["person"].is_string() && moved.status == 409 && moved.message().contains("last admin") && org_of(api, &quinn) == q_org && offers.status == 400,
        json!([moved.body, offers.body]),
    );

    // 8. offers are no one yet: the last admin of an org with only an offer
    // waiting moves (its billing ended), and their org ends, the offer withdrawn
    let uma = api.person()?;
    let u_org = buys(s, api, &uma)?;
    let u_sub = subscription_of(s, &u_org).context("the org's subscription")?;
    let vee = api.person()?;
    let r = api.signed(&uma, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&vee) })))?;
    let to_vee = r.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let offered = seat(api, &vee)["offered"].as_array().is_some_and(|o| o.iter().any(|o| o["id"] == to_vee.as_str()));
    s.stripe.set_status(&u_sub, "canceled").map_err(anyhow::Error::msg)?;
    let lapsed = s.eventually(PUSHED, || seat(api, &uma)["seat"]["good"] == false);
    let r = api.signed(&mal, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&uma) })))?;
    let moved = answer(api, &uma, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    let ended = s.eventually(PUSHED, || api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{u_org}"), None).is_ok_and(|r| r.status == 404));
    let withdrawn = seat(api, &vee)["offered"].as_array().is_some_and(|o| o.iter().all(|o| o["id"] != to_vee.as_str()));
    s.ok(
        "offers are no one yet: the last admin of an org with only an offer waiting, its billing ended, moves; their org ends, and the offer is withdrawn",
        offered && lapsed && moved.status == 200 && org_of(api, &uma) == a_org && ended && withdrawn,
        json!({ "moved": moved.body, "vee": seat(api, &vee) }),
    );
    Ok(mal)
}

/// The audit's M4 (2026-10-17): a wipe cancels what the wiped would pay.
/// Its `billing` step, of the sole admin of an org someone else holds a
/// seat in: the dry run says it ends, its subscription canceled and the
/// other seat ending; Stripe failing the cancel, the step waits (an offer
/// to the person meanwhile refused: they are being wiped), the node
/// restarts mid-step, and the wipe run again finishes it: the subscription
/// canceled, its customer kept, the org gone, the other seat ended. A seat
/// holder wiped leaves their org going on, its count pushed down; who pays,
/// wiped while another admin keeps their org, has its subscription
/// canceled and their customer forgotten.
/// `payer`'s org pays.
fn wiped(s: &mut Suite, api: &Api, payer: &Keys) -> Result<()> {
    const ENDS: &str = "M4: the dry run of the sole admin of an org someone else holds a seat in says the wipe ends it: its subscription canceled, the other's seat ending";
    const WAITS: &str = "M4: Stripe failing its cancel, the billing step takes their place, says it waits on Stripe, and is not done; an offer to them meanwhile is refused (404: being wiped)";
    const RESUMED: &str = "M4: run again (after a restart mid-step, locally), the billing step goes on where it stopped and the wipe finishes: the subscription canceled, its Stripe customer kept, the org gone, and the other's seat ended";
    const LEAVES: &str = "M4: a seat's holder wiped leaves their org going on: the dry run says so, and its seats' count is pushed down";
    const SURVIVES: &str = "M4: who pays, wiped, while another admin keeps their org: the dry run says it goes on, its subscription canceled; it is, its pending prorations deleted, the org forgets their customer, and its admin opens no billing of theirs";
    if !s.runs("billing", &[Need::Operator]) {
        for label in [ENDS, WAITS, RESUMED, LEAVES, SURVIVES] {
            s.skip(label, "it needs an operator key no person holds (a wipe's)");
        }
        return Ok(());
    }
    let wiper = s.wiper.clone().context("Need::Operator lends an operator key")?;
    let wes = api.person()?;
    let wes_email = Api::email_of(&wes);
    let wes_id = api.identity(&wes)?;
    let w_org = buys(s, api, &wes)?;
    let xi = api.person()?;
    let xi_email = Api::email_of(&xi);
    let r = api.signed(&wes, "POST", "/api/org/seats", Some(&json!({ "email": xi_email })))?;
    answer(api, &xi, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    anyhow::ensure!(s.eventually(PUSHED, || ledger(api, &xi)["seat"] == "active"), "the other's seat holds");
    let w_sub = subscription_of(s, &w_org).context("the org's subscription")?;
    let w_customer = s.stripe.subscription(&w_sub).map(|x| x.customer).unwrap_or_default();

    let dry = api.signed(&wiper, "GET", &format!("/api/people/{wes_email}/wipe"), None)?;
    let found = &dry.body["found"]["orgs"];
    s.ok(
        ENDS,
        dry.status == 200 && found.as_array().map(Vec::len) == Some(1) && found[0]["org"] == w_org.as_str() && found[0]["ends"] == true && found[0]["cancels"] == true && found[0]["lapses"] == json!([xi_email]),
        &dry,
    );

    // the cancel fails: the step waits on Stripe, the person locked
    s.stripe.fail("DELETE", "/v1/subscriptions/", true);
    let steps = fragment_core::wipe::Step::Billing.index() + 1;
    let r = api.signed(&wiper, "POST", &format!("/api/people/{wes_id}/wipe"), Some(&json!({ "confirm": wes_id, "steps": steps })))?;
    let billing = r.body["ran"].as_array().and_then(|l| l.iter().find(|x| x["step"] == "billing")).cloned().unwrap_or(Value::Null);
    let refused = api.signed(payer, "POST", "/api/org/seats", Some(&json!({ "email": wes_email })))?;
    s.ok(
        WAITS,
        r.status == 200
            && r.body["next"] == "billing"
            && billing["done"] == false
            && billing["note"].as_str().is_some_and(|n| n.contains("waiting on Stripe") && n.contains(&xi_email))
            && refused.status == 404
            && s.stripe.subscription(&w_sub).is_some_and(|x| x.status == "active"),
        json!({ "report": r.body, "offer": refused.body }),
    );

    // the node restarts mid-step (locally), then the wipe is run again
    let restarted;
    let api = if s.runs("billing", &[Need::Node]) {
        s.stop()?;
        restarted = s.start(false)?;
        &restarted
    } else {
        api
    };
    s.stripe.fail("DELETE", "/v1/subscriptions/", false);
    let (r, ran) = wipe_until_done(api, &wiper, &wes_id)?;
    let billing = billing_ran(&ran);
    let org_gone = s.eventually(PUSHED, || api.signed(&s.operator, "GET", &format!("/api/admin/orgs/{w_org}"), None).is_ok_and(|r| r.status == 404));
    let lapsed = s.eventually(PUSHED, || ledger(api, &xi)["seat"] == "canceled");
    s.ok(
        RESUMED,
        r.status == 200
            && r.body["state"] == "wiped"
            && billing["done"] == true
            && s.stripe.subscription(&w_sub).is_some_and(|x| x.status == "canceled")
            && s.stripe.customers().contains(&w_customer)
            && org_gone
            && lapsed
            && seat(api, &xi)["org"].is_null(),
        json!({ "report": r.body, "subscription": s.stripe.subscription(&w_sub).map(|x| x.status), "xi": seat(api, &xi) }),
    );

    // a seat's holder wiped: their org goes on without them, its count pushed
    let p_org = org_of(api, payer);
    let p_sub = subscription_of(s, &p_org).context("the payer's org's subscription")?;
    let yuki = api.person()?;
    let yuki_id = api.identity(&yuki)?;
    let r = api.signed(payer, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&yuki) })))?;
    answer(api, &yuki, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    let count = |s: &Suite| s.stripe.subscription(&p_sub).and_then(|x| x.items.first().map(|(_, _, q)| *q)).unwrap_or(0);
    let paid = api.signed(payer, "GET", "/api/org", None)?.body["members"].as_array().map(|m| m.iter().filter(|m| m["seat"].is_string() && m["comped"] == false).count() as u64).unwrap_or(0);
    let pushed = s.eventually(PUSHED, || count(s) == paid);
    let dry = api.signed(&wiper, "GET", &format!("/api/people/{yuki_id}/wipe"), None)?;
    let found = dry.body["found"]["orgs"][0].clone();
    let (r, ran) = wipe_until_done(api, &wiper, &yuki_id)?;
    let down = s.eventually(PUSHED, || count(s) == paid - 1);
    s.ok(
        LEAVES,
        pushed && found["org"] == p_org.as_str() && found["ends"] == false && found["cancels"] == false && r.body["state"] == "wiped" && down && s.stripe.subscription(&p_sub).is_some_and(|x| x.status == "active"),
        json!({ "found": found, "count": count(s), "paid": paid, "state": r.body["state"], "next": r.body["next"], "billing": billing_ran(&ran) }),
    );

    // who pays, wiped, while another admin keeps their org
    let zed = api.person()?;
    let zed_id = api.identity(&zed)?;
    let z_org = buys(s, api, &zed)?;
    let z_sub = subscription_of(s, &z_org).context("the payer's subscription")?;
    let z_customer = s.stripe.subscription(&z_sub).map(|x| x.customer).unwrap_or_default();
    let yan = api.person()?;
    let r = api.signed(&zed, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&yan) })))?;
    answer(api, &yan, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    // a seat offered: its proration pending on the payer's customer
    let xen = format!("billing-xen-{}@e2e.test", &Keys::generate().pubkey_hex()[..10]);
    api.signed(&zed, "POST", "/api/org/seats", Some(&json!({ "email": xen })))?;
    let prorated = s.eventually(PUSHED, || !s.stripe.pending_items(&z_sub).is_empty());
    let dry = api.signed(&wiper, "GET", &format!("/api/people/{zed_id}/wipe"), None)?;
    let found = dry.body["found"]["orgs"][0].clone();
    let (r, ran) = wipe_until_done(api, &wiper, &zed_id)?;
    let canceled = s.eventually(PUSHED, || s.stripe.subscription(&z_sub).is_some_and(|x| x.status == "canceled"));
    let forgotten = s.eventually(PUSHED, || {
        let o = listed_org(s, api, &z_org);
        o["id"] == z_org.as_str() && o["customer"].is_null() && o["subscription"].is_null()
    });
    let portal = api.signed(&yan, "POST", "/api/billing/portal", Some(&json!({})))?;
    s.ok(
        SURVIVES,
        found["org"] == z_org.as_str()
            && found["ends"] == false
            && found["cancels"] == true
            && r.body["state"] == "wiped"
            && canceled
            && prorated
            && s.stripe.pending_items(&z_sub).is_empty()
            && forgotten
            && s.stripe.customers().contains(&z_customer)
            && org_of(api, &yan) == z_org
            && seat(api, &yan)["admin"] == true
            && portal.status == 403,
        json!({ "found": found, "org": listed_org(s, api, &z_org), "portal": portal.body, "state": r.body["state"], "billing": billing_ran(&ran) }),
    );
    Ok(())
}


/// An amount of core's as the shell shows it: whole dollars (`$200`).
fn dollars(micros: i64) -> String {
    assert_eq!(micros % 1_000_000, 0, "core's prices are whole dollars");
    format!("${}", micros / 1_000_000)
}

/// The person's own fragments of `kind`, counted.
fn owned(api: &Api, keys: &Keys, kind: &str) -> usize {
    api.signed(keys, "GET", "/api/fragments", None)
        .ok()
        .and_then(|r| r.body["fragments"].as_array().map(|l| l.iter().filter(|f| f["kind"] == kind && f["role"] == "owner").count()))
        .unwrap_or(0)
}

/// The welcome is shown: its card, at its address, with its one seat.
const WELCOMED: &str = "location.pathname === '/welcome' && !document.getElementById('first-run').hidden && document.querySelectorAll('#first-run-card [data-plan]').length === 1";

/// What the first run's card shows: its address, its text, its seats and
/// whether they can be pressed, its trial code, the import's place, what
/// it says went wrong, and the first run's steps.
const SEEN: &str = "(() => { const card = document.getElementById('first-run-card'); const slot = card.querySelector('[data-slot=import]'); return { path: location.pathname, search: location.search, layout: !document.getElementById('layout').hidden, card: document.getElementById('first-run').hidden ? '' : card.innerText, plans: [...card.querySelectorAll('[data-plan]')].map((b) => b.dataset.plan), enabled: [...card.querySelectorAll('[data-plan], form button')].every((b) => !b.disabled), trial: card.querySelector('input[placeholder=\"Trial code\"]')?.value ?? null, importing: slot ? (slot.hidden ? 'hidden' : 'shown') : 'absent', errors: [...card.querySelectorAll('.form-error')].filter((e) => !e.hidden).map((e) => e.textContent), steps: [...card.querySelectorAll('.creating-steps li')].map((li) => `${li.className}: ${li.textContent}`) }; })()";

/// The first run's seat step is done (Checkout's return applied, its query
/// off), and the first agent is being made in its chat, at home's address
/// (the in-chat first run: a guest's creates are refused, so only a seat
/// gets there).
const SEATED: &str = "location.pathname === '/' && location.search === '' && document.getElementById('first-run').hidden && !document.getElementById('layout').hidden && !!document.querySelector('#frames iframe[data-fragment]')";

/// Clicks the first run's button (or link) whose text is `text`; answers
/// whether there was one.
fn press(text: &str) -> String {
    format!("(() => {{ const b = [...document.querySelectorAll('#first-run-card button, #first-run-card a')].find((b) => !b.hidden && b.textContent === {text:?}); b?.click(); return !!b; }})()")
}

/// The page's billing writes (every call to `/api/billing/…`, `/api/seat…`
/// or `/api/org/…` but a read) refused, each as `window.__next` names (a
/// status, and a refusal's `reason` or none; then a 503 with none again),
/// with the platform's words for the test; what it warns of in the console
/// kept (`window.__warned`), and every confirm said yes.
const REFUSE_BILLING: &str = "(() => { const real = window.fetch; window.__next = { status: 503 }; window.__warned = []; const warn = console.warn; \
    console.warn = (...a) => { window.__warned.push(a.map(String).join(' ')); warn(...a); }; window.confirm = () => true; \
    window.fetch = async (url, o) => { if ((o?.method ?? 'GET') === 'GET' || !/^\\/api\\/(billing\\/|seat|org\\/)/.test(String(url))) return real(url, o); \
    const { status, reason } = window.__next; window.__next = { status: 503 }; \
    return new Response(JSON.stringify({ error: 'refused', message: 'the platform said this for the test', ...(reason ? { reason } : {}) }), { status, headers: { 'content-type': 'application/json' } }); }; return true; })()";

/// Clicks the first element `selector` matches; answers whether there was one.
fn click(selector: &str) -> String {
    format!("(() => {{ const e = document.querySelector({selector:?}); e?.click(); return !!e; }})()")
}

/// Clicks the first of the elements `selector` matches whose text is
/// `text`; answers whether there was one.
fn click_text(selector: &str, text: &str) -> String {
    format!("(() => {{ const e = [...document.querySelectorAll({selector:?})].find((e) => e.textContent === {text:?}); e?.click(); return !!e; }})()")
}

/// Where a page is, what it shows, and which of the shell's files it has.
/// Resource Timing lists a file once its answer is in, so one the page
/// lacks is a request that never came back: its modules then never run,
/// and the page stays blank (CI, 2026-10-13: a page's billing.js and
/// vendor/split-grid.js never reached the node).
const PAGE: &str = "({ at: location.href, ready: document.readyState, firstRun: document.getElementById('first-run')?.hidden === false, layout: document.getElementById('layout')?.hidden === false, text: document.body?.innerText.slice(0, 300) ?? '', shell: performance.getEntriesByType('resource').filter((e) => e.name.includes('/__shell/')).map((e) => `${e.name.split('/__shell/')[1]} ${e.responseStatus}`) })";

/// `PAGE` on `page`, or why it could not be read.
fn page_state(b: &mut Lease, page: &Page) -> Value {
    b.eval(page, PAGE).unwrap_or_else(|e| json!(format!("{e:#}")))
}

/// Does `step` on `page`: an action that answers whether it found what it
/// acts on. When it did not, the step is a FAIL that says where the page is
/// and what it has, and the caller stops there: each step after it acts on
/// what this one makes.
fn act(s: &mut Suite, b: &mut Lease, page: &Page, what: &str, step: &str) -> Result<bool> {
    if b.eval(page, step)? == json!(true) {
        return Ok(true);
    }
    s.ok(&format!("{what}: it is on the page"), false, page_state(b, page));
    Ok(false)
}

/// Clicks `selector` as a person does (Stripe's pages), as `act` does an action.
fn act_at(s: &mut Suite, b: &mut Lease, page: &Page, what: &str, selector: &str) -> bool {
    match b.click(page, selector) {
        Ok(()) => true,
        Err(e) => {
            s.ok(&format!("{what}: {e:#}"), false, page_state(b, page));
            false
        }
    }
}

/// Waits until `expr` holds on `page`, a page of the shell just loaded or
/// navigated to. `wrangler dev`'s proxy now and then never answers a
/// request in a page's burst of module loads (CI, 2026-10-13: billing.js
/// and vendor/split-grid.js, twice, which never reached the node), and the
/// page's modules then never run. A page whose shell never ran (neither its
/// first run nor its layout shows) when the wait runs out is said, with
/// the files it has, and loaded again, once; anything else is the wait's
/// own answer.
#[track_caller]
fn view(b: &mut Lease, page: &Page, expr: &str, wait: Duration) -> bool {
    if b.until(page, expr, wait) {
        return true;
    }
    let state = page_state(b, page);
    if state["firstRun"] != false || state["layout"] != false || !state["at"].as_str().is_some_and(|at| at.starts_with("http")) {
        return false;
    }
    println!("      (the shell never ran, a request of its page never answered: {state}; loaded again)");
    b.reload(page).is_ok() && b.until(page, expr, wait)
}

/// The shell's Sign In from its signed-out page at `path`, the whole
/// round trip, in a browser context of its own. With WorkOS holding a
/// session as `email` in the browser (`held`: the fake's hold, as
/// AuthKit's cookie on its own domain), WorkOS answers at once and that
/// session is kept (a sign-in, not a switch), then let go; holding none,
/// it asks, and `email` is typed. Either way the browser must come back
/// to `path` (`landed`, a script's condition; `place` says it), signed in
/// as `email`.
#[allow(clippy::too_many_arguments)]
fn signed_in_back(s: &mut Suite, b: &mut Lease, api: &Api, path: &str, email: &str, held: bool, landed: &str, place: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let sid = held.then(|| s.workos.hold(email, 3600));
    let context = b.another_context()?;
    let page = b.open_in(&context, &format!("{}{path}", api.base))?;
    let href = format!("/auth/login?screen_hint=sign-in&return={}", url_enc(path));
    let offered = view(b, &page, &format!("document.querySelector('#first-run-card a.splash-secondary')?.getAttribute('href') === {href:?}"), wait);
    let mut pressed = offered && b.eval(&page, &click("#first-run-card a.splash-secondary"))? == json!(true);
    if pressed && !held {
        // WorkOS's form (the fake's): who is signing in
        pressed = b.until(&page, "!!document.querySelector('input[name=login_hint]')", wait)
            && b.eval(&page, &format!("(document.querySelector('input[name=login_hint]').value = {email:?}, document.querySelector('input[name=login_hint]').form.submit(), true)"))? == json!(true);
    }
    let back = pressed && view(b, &page, &format!("({landed}) && document.body.innerText.includes({email:?})"), wait);
    let kept = sid.as_ref().is_none_or(|sid| s.workos.held().as_ref() == Some(sid));
    let seen = json!({ "offered": offered, "pressed": pressed, "at": b.eval(&page, "location.pathname + location.search")?, "held": s.workos.held() });
    if let Some(sid) = &sid {
        let ended = api.external(&format!("{}/user_management/sessions/logout?session_id={sid}", s.workos.url))?;
        anyhow::ensure!(ended.status == 200 && s.workos.held().is_none(), "letting WorkOS's held session go: {ended}");
    }
    b.close(page)?;
    let how = if held { "with WorkOS holding a session in the browser, is that account at once, keeps that session," } else { "with none held, asks who is signing in," };
    s.ok(&format!("signed out at {path}, Sign In, {how} and lands back at {place}"), back && kept, seen);
    Ok(())
}

/// A page of the person signed in with `session`, open at `path`.
fn page_of(b: &mut Lease, api: &Api, session: &str, path: &str) -> Result<Page> {
    b.set_cookie(&format!("{}/", api.base), "fragment_session", session)?;
    let page = b.open(&format!("{}{path}", api.base))?;
    b.viewport(&page, 1280, 800, false)?;
    Ok(page)
}

/// What a page shows of buying, anywhere the shell offers it: a seat's
/// button, a trial code, Get a seat, Pay again, Stripe's portal, seats for
/// others, a pack; and any price in its text.
const BUYS: &str = "(() => { const shown = [...document.querySelectorAll('[data-plan], form.trial-code, a, button')].filter((e) => e.getClientRects().length); \
    return { buys: shown.filter((e) => e.matches('[data-plan], form.trial-code') || /^(Get a seat|Start a trial|Pay again|Payment and invoices|Add seats for others|Buy )/.test(e.textContent)).map((e) => e.textContent), \
    prices: document.body.innerText.match(/\\$\\d/g) ?? [] }; })()";

/// A deployment's default plan is no org seat. Its person can buy a
/// subscription from Settings without first becoming a guest; merely
/// opening Checkout changes neither their plan nor their credit.
fn default_plan_checkout(s: &mut Suite, b: &mut Lease, api: &Api, sells: bool, width: u32, shots: &std::path::Path) -> Result<()> {
    let wait = Duration::from_secs(30);
    let keys = api.person()?;
    let who = api.identity(&keys)?;
    let mine = seat(api, &keys);
    anyhow::ensure!(mine["guest"] == false && mine["seat"].is_null() && mine["org"].is_null(), "a default plan, no org seat: {mine}");
    let before = ledger(api, &keys);
    let session = api.sign_in(&Api::email_of(&keys))?;
    let page = page_of(b, api, &session, "/settings")?;
    b.viewport(&page, width, 844, width < 600)?;
    let shown = view(b, &page, "location.pathname === '/settings' && (document.getElementById('settings-page')?.innerText ?? '').includes('given to everyone here')", wait);
    let buying = b.eval(&page, BUYS)?;
    let buy = format!("Buy an always-on seat, {} a month", dollars(fragment_core::stripe::offer().seat_micros));
    s.ok(
        if sells { "a default-plan person with no org is offered the priced subscription in Settings, but no credit pack or portal" } else { "with no Stripe, a default-plan person's Settings offer no purchase" },
        shown && buying["buys"] == if sells { json!([buy]) } else { json!([]) },
        &buying,
    );
    if sells {
        b.eval(&page, "(document.querySelector('#settings-page [data-plan]')?.closest('section')?.scrollIntoView({ block: 'start' }), true)")?;
        let fits = b.eval(&page, "(() => { const r = document.querySelector('#settings-page [data-plan]')?.getBoundingClientRect(); return !!r && r.width > 0 && r.left >= 0 && r.right <= innerWidth; })()")?;
        s.ok(&format!("at {width}px, the subscription button fits the viewport"), fits == true, &fits);
        b.screenshot(&page, &shots.join(format!("default-plan-{width}.png")))?;
        let requests = s.stripe.requests().len();
        let customers = s.stripe.customers().len();
        if !act_at(s, b, &page, "the default-plan person's subscription", "#settings-page [data-plan=seat_always_on]") {
            b.close(page)?;
            return Ok(());
        }
        let checkout = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
        if checkout {
            b.screenshot(&page, &shots.join(format!("default-plan-checkout-{width}.png")))?;
        }
        let asked = s.stripe.requests().into_iter().skip(requests).find(|r| r.path == "/v1/checkout/sessions");
        let correct = asked.as_ref().is_some_and(|r| {
            r.form.get("mode").map(String::as_str) == Some("subscription")
                && r.form.get("line_items[0][quantity]").map(String::as_str) == Some("1")
                && r.form.get("metadata[fragment_person]") == Some(&who)
                && r.form.get("metadata[fragment_kind]").map(String::as_str) == Some("seat_always_on")
        });
        let after = ledger(api, &keys);
        s.ok(
            "the default-plan person's Settings open their own subscription Checkout; before payment no org, customer, plan or credit changes",
            checkout && correct && seat(api, &keys) == mine && s.stripe.customers().len() == customers && before["plan"] == after["plan"] && before["purchasedMicros"] == after["purchasedMicros"] && before["availableMicros"] == after["availableMicros"],
            json!({ "checkout": checkout, "correct": correct, "seat": seat(api, &keys), "plan": after["plan"] }),
        );
    }
    b.close(page)?;
    Ok(())
}

/// A deployment that sells nothing (no Stripe: billing off, as finite.chat
/// launched; `Shape::SellsNothing`): `GET /api/seat` offers nothing, and a
/// guest's welcome, Billing, and home beside what was shared with them
/// show no price and nothing to buy: that seats are not open, who is
/// signed in, and Use another account.
fn sells_nothing(s: &mut Suite, b: &mut Lease, api: &Api, guest: &dyn Fn(&Api, &str) -> Result<Keys>, shots: &std::path::Path) -> Result<()> {
    const CLOSED: &str = "Finite isn't open for new seats yet.";
    let wait = Duration::from_secs(30);
    let gus = guest(api, "billing-page-closed")?;
    let email = Api::email_of(&gus);
    let said = json!([super::shell::no_seat_said(&email)]);
    let mine = seat(api, &gus);
    s.ok("with no Stripe, GET /api/seat sells nothing: its offer is null", mine["guest"] == true && mine.get("offer") == Some(&Value::Null), &mine);
    let session = api.sign_in(&email)?;
    let page = page_of(b, api, &session, "/")?;
    let welcomed = view(b, &page, &format!("location.pathname === '/welcome' && !document.getElementById('first-run').hidden && document.getElementById('first-run-card').innerText.includes({CLOSED:?})"), wait);
    let _ = b.screenshot(&page, &shots.join("0-welcome-sells-nothing.png"));
    let welcome = json!({ "buying": b.eval(&page, BUYS)?, "who": b.eval(&page, super::shell::NO_SEAT_SEEN)? });
    b.eval(&page, "(location.assign('/settings'), true)")?;
    let billed = view(b, &page, &format!("location.pathname === '/settings' && /You are a guest/.test(document.getElementById('settings-page').innerText) && document.getElementById('settings-page').innerText.includes({CLOSED:?})"), wait);
    let billing = json!({ "buying": b.eval(&page, BUYS)?, "who": b.eval(&page, super::shell::NO_SEAT_SEEN)? });
    b.close(page)?;
    let none = json!({ "buys": [], "prices": [] });
    let switch = |back: &str| json!([format!("/auth/switch?return={}", url_enc(back)), "post", "Use another account"]);
    s.ok(
        "with no Stripe, a new guest's welcome and Billing show no price and nothing to buy (no seat, trial code or Get a seat): that Finite isn't open for new seats yet, who is signed in, and Use another account",
        welcomed
            && billed
            && welcome["buying"] == none
            && billing["buying"] == none
            && welcome["who"]["said"] == said
            && billing["who"]["said"] == said
            && welcome["who"]["switches"] == json!([switch("/welcome")])
            && billing["who"]["switches"] == json!([switch("/settings"), switch("/settings")]),
        json!({ "welcome": welcome, "billing": billing }),
    );
    // shared something, a guest is home beside it, told the same
    let owner = api.person()?;
    let made = api.signed(&owner, "POST", "/api/fragments", Some(&json!({ "label": format!("closed-{}", &Keys::generate().pubkey_hex()[..6]), "template": "todo" })))?;
    anyhow::ensure!(made.status == 200, "making the app to share: {made}");
    let app = made.body["name"].as_str().unwrap_or("").to_string();
    let r = api.signed(&owner, "POST", &format!("/api/f/{app}/invites"), Some(&json!({ "email": email, "role": "editor" })))?;
    let yes = api.signed(&gus, "POST", &format!("/api/invites/{app}/accept"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200 && yes.status == 200, "sharing it with the guest, and their yes: {r} {yes}");
    let page = page_of(b, api, &session, "/")?;
    let home = view(b, &page, &format!("location.pathname === '/' && !!document.querySelector('.viewer iframe[data-fragment={app:?}]') && (document.getElementById('notice')?.innerText ?? '').includes({CLOSED:?})"), wait);
    let seen = json!({ "buying": b.eval(&page, BUYS)?, "row": b.eval(&page, "[...document.querySelectorAll('#notice .notice-row > *')].map((e) => e.textContent)")? });
    b.close(page)?;
    s.ok(
        "with no Stripe, a guest home beside what was shared with them is told seats are not open, beside Use another account alone: no price, no Get a seat",
        home && seen["buying"] == none && seen["row"] == json!(["Use another account"]),
        &seen,
    );
    Ok(())
}

/// A new person's first run, and Billing, in Chrome (cell/shell/shell.js
/// `welcome`, billing.js): a new guest lands on the welcome, never
/// settings: what this is, the one seat and a trial code. A guest's
/// Billing in settings links to it. Checkout left by its Back comes back
/// to it, saying so; a trial code that is none is said there. Paid on
/// Stripe's page (the fake's), Checkout's return comes back to it, applies
/// the seat and makes their first agent; that return again goes home.
/// Billing then manages the seat: its org's seats wait until they ask for
/// seats for others, they add one, and buy credit. A guest an org offers a
/// seat sees the offer first on the welcome, accepts it, and goes on to
/// their first run. An invited guest lands on what was shared with them,
/// titled as its owner titled it, a seat offered beside it. Another's
/// Checkout returned is refused, and leads back to the seats; the node
/// restarts while a buyer is at Checkout, no webhook comes, and the return
/// alone seats them; a reload mid-wait makes nothing twice, and the webhook
/// come late changes nothing. A trial mailed brings a signed-out person
/// through sign-in and back with its code. A guest given a seat goes
/// straight to the first run. Stripe's pages are the fake's own. A step
/// that acts on the page does so only on what is there: one that finds
/// nothing is a FAIL saying where the page is and what it has, not an
/// error thrown in the page.
pub fn billing_page(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("billing-page", &[Need::Chrome, Need::Fakes, Need::Deployment, Need::Node]) {
        return Ok(());
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the welcome and Billing's page (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("billing-page");
    let wait = Duration::from_secs(30);
    let op_session = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&op_session, &s.operator);
    // a guest: the e2e's people are seats, so an operator makes one a guest
    let operator = s.operator.clone();
    let guest = |api: &Api, id: &str| -> Result<Keys> {
        let keys = api.person()?;
        let who = api.identity(&keys)?;
        let r = api.signed(&operator, "POST", &format!("/api/ledger/{who}/plan"), Some(&json!({ "id": id, "plan": "guest" })))?;
        anyhow::ensure!(r.status == 200, "making a guest: {r}");
        Ok(keys)
    };
    let text = |b: &mut crate::browser::Lease, p: &crate::browser::Page| b.eval(p, "document.getElementById('settings-page')?.innerText ?? ''").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default();
    // ---- a deployment that sells nothing, on a node of its own; then this one again
    s.stop()?;
    let off = s.start_selling_nothing()?;
    let checked = sells_nothing(s, &mut b, &off, &guest, &shots);
    let default_closed = default_plan_checkout(s, &mut b, &off, false, 1280, &shots);
    s.stop()?;
    s.start(false)?;
    checked?;
    default_closed?;
    // what is sold, as core has it: every price a page shows is these
    let offer = fragment_core::stripe::offer();
    let (seat_price, included, pack) = (dollars(offer.seat_micros), dollars(offer.included_micros), dollars(offer.pack_micros));
    default_plan_checkout(s, &mut b, api, true, 1280, &shots)?;
    default_plan_checkout(s, &mut b, api, true, 390, &shots)?;

    // ---- a new guest: the welcome, never settings
    let gus = guest(api, "billing-page-guest")?;
    let mine = seat(api, &gus);
    s.ok(
        "GET /api/seat says who is a guest (a guest's plan, no seat) and what is sold, at core's prices",
        mine["guest"] == true
            && mine["seat"].is_null()
            && mine["offer"] == json!({ "kind": "seat_always_on", "seatMicros": offer.seat_micros, "includedMicros": offer.included_micros, "packMicros": offer.pack_micros }),
        &mine,
    );
    let session = api.sign_in(&Api::email_of(&gus))?;
    let page = page_of(&mut b, api, &session, "/")?;
    let welcomed = view(&mut b, &page, WELCOMED, wait);
    let _ = b.screenshot(&page, &shots.join("1-welcome.png"));
    let seen = b.eval(&page, SEEN)?;
    s.ok(
        &format!("a new guest's first run is the welcome, never settings: what this is, the one seat sold ({seat_price} a month, always on, {included} of credit: core's prices, on its ticket), a trial code, what more credit costs, the import's place kept hidden, and who is signed in"),
        welcomed
            && seen["layout"] == false
            && seen["plans"] == json!(["seat_always_on"])
            && seen["trial"] == ""
            && seen["importing"] == "hidden"
            && seen["errors"] == json!([])
            && seen["card"].as_str().is_some_and(|t| {
                t.contains("Welcome to Finite")
                    && t.contains("Superintelligent agents for you and your organization")
                    && t.contains(&format!("{seat_price} / month"))
                    && t.contains("Always on")
                    && t.contains(&format!("{included} in monthly usage credit"))
                    && t.contains("Secure Stripe checkout")
                    && t.contains(&format!("{pack} a pack"))
                    && t.contains(&Api::email_of(&gus))
                    && t.contains("Sign out")
                    && !t.contains("Not now")
            }),
        &seen,
    );
    // settings' Billing manages a seat; a guest's links to the welcome
    b.eval(&page, "(location.assign('/settings'), true)")?;
    let billing = view(&mut b, &page, "location.pathname === '/settings' && document.querySelector('#settings-page h2')?.textContent === 'Billing' && /You are a guest/.test(document.getElementById('settings-page').innerText)", wait);
    let offered = b.eval(
        &page,
        "({ links: [...document.querySelectorAll('#settings-page a')].filter((a) => a.textContent === 'Get a seat').map((a) => a.getAttribute('href')), buys: [...document.querySelectorAll('#settings-page button, #settings-page input')].map((b) => b.textContent || b.placeholder).filter((t) => /seat|trial/i.test(t)), headings: [...document.querySelectorAll('#settings-page h2')].map((h) => h.textContent), warned: [...document.querySelectorAll('.settings-warning')].map((w) => w.textContent) })",
    )?;
    s.ok(
        "a guest's Billing in settings sells nothing itself: it names the one seat's price, its Get a seat is the welcome's, and nothing a guest has no use for shows (a computer, agents, skills, connections)",
        billing
            && text(&mut b, &page).contains(&format!("A seat, {seat_price} a month"))
            && offered["links"] == json!(["/welcome"])
            && offered["buys"] == json!([])
            && offered["warned"] == json!([])
            && offered["headings"].as_array().is_some_and(|h| ["Computer", "Agents", "Skills", "Connections"].iter().all(|x| !h.contains(&json!(x)))),
        &offered,
    );
    if !act(s, &mut b, &page, "settings' Get a seat", &click_text("#settings-page a", "Get a seat"))? {
        return Ok(());
    }
    let back = view(&mut b, &page, WELCOMED, wait);

    // ---- invalid: Checkout left by its Back; a trial code that is none
    if !act(s, &mut b, &page, "the welcome's seat", &click("#first-run-card [data-plan=seat_always_on]"))? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
    if !act_at(s, &mut b, &page, "Stripe's Back", "#back") {
        return Ok(());
    }
    let canceled = view(&mut b, &page, &format!("{WELCOMED} && location.search === '' && /Checkout was canceled: nothing was bought/.test(document.getElementById('first-run-card').innerText)"), wait);
    let _ = b.screenshot(&page, &shots.join("2-canceled.png"));
    s.ok(
        "Get a seat there opens the welcome; its seat goes to Stripe's Checkout, whose Back comes back to the welcome, saying nothing was bought, the seat offered again",
        back && at_stripe && canceled && seat(api, &gus)["seat"].is_null(),
        json!({ "seen": b.eval(&page, SEEN)?, "seat": seat(api, &gus) }),
    );
    let legal = b.eval(
        &page,
        "(() => { const f = document.querySelector('#first-run .page-footer'); \
         return { shown: !!f && f.getClientRects().length > 0, links: [...(f?.querySelectorAll('a[href^=\"/\"]') ?? [])].filter((a) => /^(Privacy Policy|Terms)$/.test(a.textContent)).map((a) => [a.textContent, a.getAttribute('href'), a.target]) }; })()",
    )?;
    s.ok(
        "the welcome's footer names Finite's Privacy Policy and Terms, each in a tab of its own",
        legal == json!({ "shown": true, "links": [["Privacy Policy", "/privacy", "_blank"], ["Terms", "/terms", "_blank"]] }),
        &legal,
    );
    let typed = "(() => { const f = document.querySelector('#first-run-card form.trial-code'); if (!f) return false; f.querySelector('input').value = 'ZZZZ-ZZZZ-ZZZZ-ZZZZ'; f.requestSubmit(); return true; })()";
    if !act(s, &mut b, &page, "the welcome's trial code", typed)? {
        return Ok(());
    }
    let refused = b.until(
        &page,
        "[...document.querySelectorAll('#first-run-card .form-error')].some((e) => !e.hidden && e.textContent === \"That trial code isn't one we know. Check it and try again.\") && !document.getElementById('first-run-card').innerText.includes('no such trial code')",
        wait,
    );
    let seen = b.eval(&page, SEEN)?;
    let _ = b.screenshot(&page, &shots.join("3-no-such-code.png"));
    s.ok(
        "a trial code that is none is said on the welcome in plain words (never the platform's), which stays, its choices open again; nothing is bought",
        refused && seen["path"] == "/welcome" && seen["enabled"] == true && seat(api, &gus)["seat"].is_null(),
        &seen,
    );
    // its words come by the platform's reason, not its status (#481
    // review): a code that has ended, and then, Stripe refusing Checkout
    // (a 400, as a 4xx of Stripe's is to the person), a valid code and the
    // seat each say Checkout did not open, neither "ended" nor "not on sale"
    let made = |name: &str| api.signed(&s.operator, "POST", "/api/admin/trials", Some(&json!({ "name": name, "kind": "seat_always_on", "days": 30, "capacity": 5 })));
    let valid = made("billing-page valid")?;
    let ended = made("billing-page ended")?;
    let ended_off = api.signed(&s.operator, "PATCH", &format!("/api/admin/trials/{}", ended.body["id"].as_str().unwrap_or("")), Some(&json!({ "revision": 1, "active": false })))?;
    let trial = |code: &str| format!("(() => {{ const f = document.querySelector('#first-run-card form.trial-code'); if (!f) return false; f.querySelector('input').value = {code:?}; f.requestSubmit(); return true; }})()");
    let error_is = |text: &str| format!("[...document.querySelectorAll('#first-run-card .form-error')].some((e) => !e.hidden && e.textContent === {text:?})");
    let mut said = vec![];
    b.eval(&page, &trial(ended.body["code"].as_str().unwrap_or("")))?;
    said.push(("an ended code", b.until(&page, &error_is("That trial code has ended."), wait)));
    s.stripe.refuse("POST", "/v1/checkout/sessions", Some(400));
    b.eval(&page, &trial(valid.body["code"].as_str().unwrap_or("")))?;
    said.push(("a valid code, Stripe refusing", b.until(&page, &format!("{} && document.querySelector('#first-run-card [data-plan]')?.disabled === false", error_is("Checkout didn't open. Try again in a moment.")), wait)));
    b.eval(&page, "(document.querySelector('#first-run-card .form-error').textContent = '', true)")?;
    b.eval(&page, &click("#first-run-card [data-plan=seat_always_on]"))?;
    said.push(("the seat, Stripe refusing", b.until(&page, &error_is("Checkout didn't open. Try again in a moment."), wait)));
    s.stripe.refuse("POST", "/v1/checkout/sessions", None);
    s.ok(
        "a trial code that has ended says so; with Stripe refusing Checkout, a valid code and the seat each say Checkout didn't open, to try again: the words come by the platform's reason, never its status",
        valid.status == 200 && ended_off.status == 200 && said.iter().all(|(_, ok)| *ok),
        json!({ "said": said.iter().map(|(what, ok)| json!({ "what": what, "said": ok })).collect::<Vec<_>>(), "seen": b.eval(&page, SEEN)? }),
    );
    // a seat's Checkout return that names no payment: said plainly, with
    // Try again and Choose a seat, which goes back to the welcome
    b.eval(&page, "(location.assign('/welcome?checkout=cs_e2e_nonesuch'), true)")?;
    let unfound = view(
        &mut b,
        &page,
        "(() => { const c = document.getElementById('first-run-card'); const e = c?.querySelector('.form-error'); return !!e && !e.hidden && e.textContent === \"This payment wasn't found. Choose a seat to start again.\" && !c.innerText.includes('Stripe') && [...c.querySelectorAll('button')].some((b) => !b.hidden && b.textContent === 'Try again'); })()",
        wait,
    );
    let card = b.eval(&page, "document.getElementById('first-run-card')?.innerText ?? ''")?;
    b.eval(&page, &click_text("#first-run-card button", "Choose a seat"))?;
    let chosen = view(&mut b, &page, WELCOMED, wait);
    s.ok(
        "a seat's Checkout return that names no payment says so in plain words (never the platform's), with Try again, and Choose a seat goes back to the welcome",
        unfound && chosen && seat(api, &gus)["seat"].is_null(),
        &card,
    );

    // ---- valid: the $200 seat paid; its return is the welcome's, then the first run
    if !act(s, &mut b, &page, "the welcome's $200 seat", &click("#first-run-card [data-plan=seat_always_on]"))? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
    let paid = b.eval(&page, "location.pathname.split('/').pop()")?.as_str().unwrap_or("").to_string();
    if !act_at(s, &mut b, &page, "Stripe's Pay", "form button") {
        return Ok(());
    }
    let seated = view(&mut b, &page, SEATED, wait);
    let _ = b.screenshot(&page, &shots.join("4-paid-creating.png"));
    let made = s.eventually(wait, || owned(api, &gus, "agent") == 1 && owned(api, &gus, "chat") == 1);
    let mine = seat(api, &gus);
    s.ok(
        "paid, Checkout's return comes back to the welcome: it applies the seat (a $200 always-on seat), takes its query off, and the first run goes on to their agent and its chat, a seat's creates",
        at_stripe && seated && made && mine["seat"]["kind"] == "seat_always_on" && mine["seat"]["good"] == true,
        json!({ "seen": b.eval(&page, SEEN)?, "seat": mine }),
    );
    // ---- replay: that return again, its first run made, goes home
    let purchased = ledger(api, &gus)["purchasedMicros"].clone();
    b.eval(&page, &format!("(location.assign({:?}), true)", format!("/welcome?checkout={paid}")))?;
    let home = view(&mut b, &page, "location.pathname === '/' && location.search === '' && document.getElementById('first-run').hidden && !document.getElementById('layout').hidden && !!document.querySelector('#frames iframe[data-fragment]')", wait);
    let again = seat(api, &gus);
    s.ok(
        "Checkout's return again goes home to their chat: the seat as it was, nothing made or credited twice",
        home && again["seat"] == mine["seat"] && owned(api, &gus, "agent") == 1 && owned(api, &gus, "chat") == 1 && ledger(api, &gus)["purchasedMicros"] == purchased,
        json!({ "seat": again, "at": b.eval(&page, "location.pathname + location.search")? }),
    );

    // ---- Billing manages the seat from here
    b.eval(&page, "(location.assign('/settings'), true)")?;
    let held = format!("Always-on seat, {seat_price} a month");
    let managed = view(
        &mut b,
        &page,
        &format!("location.pathname === '/settings' && (document.getElementById('settings-page')?.innerText ?? '').includes({held:?}) && [...document.querySelectorAll('#settings-page button')].some((b) => b.textContent === 'Add seats for others')"),
        wait,
    );
    let _ = b.screenshot(&page, &shots.join("5-billing.png"));
    let shown = text(&mut b, &page);
    let buttons = b.eval(&page, "[...document.querySelectorAll('#settings-page button')].map((b) => b.textContent)")?;
    let has = |t: &str| buttons.as_array().is_some_and(|l| l.contains(&json!(t)));
    let alone = b.eval(&page, "!document.querySelector('.billing-seats') && ![...document.querySelectorAll('#settings-page h2')].some((h) => h.textContent.startsWith('Org'))")?;
    let warned = b.eval(&page, "[...document.querySelectorAll('.settings-warning')].map((w) => w.textContent)")?;
    s.ok(
        &format!("their Billing shows their seat, an always-on seat at {seat_price} a month: an org of one shows no org's table, only its invoices and seats for others, and no first run of its own"),
        managed && alone == true && warned == json!([]) && has("Payment and invoices") && has("Add seats for others") && !has("Make your first agent") && !has(&format!("Buy an always-on seat, {seat_price} a month")),
        json!({ "shown": shown, "buttons": buttons, "warned": warned }),
    );
    if !act(s, &mut b, &page, "Billing's Add seats for others", &click_text("#settings-page button", "Add seats for others"))? {
        return Ok(());
    }
    let asked = b.until(&page, "!!document.querySelector('.billing-seats') && [...document.querySelectorAll('#settings-page form')].some(f => f.textContent.includes('Add a seat'))", wait);
    s.ok("asked, their org's seats show: theirs alone, and a seat to add by email", asked, text(&mut b, &page));
    let teammate = format!("billing-page-mate-{}@e2e.test", &Keys::generate().pubkey_hex()[..8]);
    let typed = format!("(() => {{ const f = [...document.querySelectorAll('#settings-page form')].find(f => f.textContent.includes('Add a seat')); if (!f) return false; f.querySelector('input[type=email]').value = {teammate:?}; f.requestSubmit(); return true; }})()");
    if !act(s, &mut b, &page, "Billing's Add a seat", &typed)? {
        return Ok(());
    }
    let added = b.until(&page, &format!("[...document.querySelectorAll('.billing-seats tr')].some(r => r.textContent.includes({teammate:?}) && r.textContent.includes('invited'))"), wait);
    let unasked = b.eval(&page, "![...document.querySelectorAll('#settings-page button')].some((b) => b.textContent === 'Add seats for others')")?;
    s.ok("as its admin they add a seat by email: it waits, invited, and their org's seats now show unasked", added && unasked == true, text(&mut b, &page));
    let before = ledger(api, &gus)["purchasedMicros"].as_i64().unwrap_or(0);
    let buy = format!("Buy {pack} of credit");
    if !act(s, &mut b, &page, &format!("Billing's {buy}"), &click_text("#settings-page button", &buy))? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
    if !act_at(s, &mut b, &page, "Stripe's Pay", "form button") {
        return Ok(());
    }
    let credited = view(&mut b, &page, "location.pathname === '/settings' && /Paid: the credit is yours/.test(document.getElementById('settings-page')?.innerText ?? '')", wait);
    s.ok(
        &format!("they buy {pack} of credit on Stripe's page, and it is theirs as they land back in settings"),
        at_stripe && credited && ledger(api, &gus)["purchasedMicros"].as_i64() == Some(before + offer.pack_micros),
        ledger(api, &gus),
    );
    b.close(page)?;
    // ---- Checkout's return settles once for every load of settings (#461
    // review): its POST held before it is sent, and settings opened again
    // meanwhile (the query gone); let go, the settings shown read the
    // ledger after it and say how it went. Its webhook held, so the return
    // alone applies it.
    s.stripe.hold_events(true);
    let r = api.signed(&gus, "POST", "/api/billing/packs", Some(&json!({})))?;
    let session = r.body["session"].as_str().unwrap_or("").to_string();
    s.stripe.complete(&session).map_err(anyhow::Error::msg)?;
    let purchased = |api: &Api| ledger(api, &gus)["purchasedMicros"].as_i64().unwrap_or(0);
    let (before, bought) = (ledger(api, &gus)["balanceMicros"].as_i64().unwrap_or(0), purchased(api));
    let (paid, seen) = held_return(&mut b, api, &session, wait)?;
    let shown = seen["text"].as_str().unwrap_or("");
    let credit = credit_shown(shown);
    // (their computer's use may meter a few micro-dollars meanwhile: the
    // pack's $25 is what must be there)
    s.ok(
        &format!("a pack's return whose settling answers after settings were opened again: the settings shown say it was paid, and their credit has its {pack}"),
        paid && purchased(api) == bought + offer.pack_micros && shown.contains("Paid: the credit is yours.") && credit.is_some_and(|c| c > before + offer.pack_micros - 1_000_000),
        json!({ "seen": seen, "before": before, "credit": credit }),
    );
    s.stripe.release_events(false);
    let (settled, seen) = held_return(&mut b, api, "cs_e2e_nonesuch", wait)?;
    s.ok(
        "a return that is refused, answering after settings were opened again: the settings shown say the purchase is not here yet",
        settled && seen["text"].as_str().is_some_and(|t| t.contains(NOT_HERE)),
        &seen,
    );

    // ---- a pack's return that is not applied (one Stripe never made) is
    // said in plain words, what to do, and where to get help (the
    // deployment's link: put in the page's read of the computers, as the
    // local node names none); the platform's words go to the console
    let refused = api.signed(&gus, "POST", "/api/billing/sessions/cs_e2e_nonesuch", Some(&json!({})))?;
    let page = b.open_with_script(&format!("{}/settings?pack=cs_e2e_nonesuch", api.base), HELP_AND_CONSOLE)?;
    let told = view(&mut b, &page, &format!("(document.getElementById('settings-page')?.innerText ?? '').includes({NOT_HERE:?})"), wait);
    let seen = b.eval(
        &page,
        "({ text: document.getElementById('settings-page').innerText, help: [...document.querySelectorAll('#settings-page a')].find((a) => a.textContent === 'get help')?.getAttribute('href') ?? null, warned: window.__warned })",
    )?;
    let said = seen["text"].as_str().unwrap_or("");
    let warned = seen["warned"].as_array().is_some_and(|l| l.iter().any(|w| w.as_str().is_some_and(|w| w.contains(refused.message()))));
    s.ok(
        "a pack's return that is not applied says so plainly: not here yet, the credit arrives within minutes if they were charged, and where to get help; the platform's words go to the console, never the page",
        refused.status == 404 && told && said.contains(&format!("{NOT_HERE} If it doesn't, get help.")) && !said.contains(refused.message()) && seen["help"] == "mailto:help@e2e.test" && warned,
        json!({ "seen": seen, "platform": refused.message() }),
    );
    b.close(page)?;

    // ---- every Billing and Org control whose call fails says so in plain
    // words, never the platform's (which go to the console): each refused
    // once by the page's fetch, by the status that picks its words
    let page = b.open(&format!("{}/settings", api.base))?;
    let ready = view(&mut b, &page, "!!document.querySelector('.billing-seats') && [...document.querySelectorAll('#settings-page label')].some((l) => l.textContent.includes('Let my computer sleep'))", wait);
    b.eval(&page, REFUSE_BILLING)?;
    let typed = |form: &str, email: &str| format!("(() => {{ const f = [...document.querySelectorAll('#settings-page form')].find((f) => f.textContent.includes({form:?})); f.querySelector('input[type=email]').value = {email:?}; f.requestSubmit(); return true; }})()");
    let steps = [
        ("the sleep switch", "{ status: 503 }", "([...document.querySelectorAll('#settings-page label')].find((l) => l.textContent.includes('Let my computer sleep')).querySelector('input').click(), true)".to_string(), "That wasn't changed. Try again in a moment.".to_string()),
        ("Buy credit, Stripe refusing", "{ status: 400, reason: 'stripe_refused' }", click_text("#settings-page button", &format!("Buy {pack} of credit")), "Checkout didn't open. Try again in a moment.".to_string()),
        ("Payment and invoices", "{ status: 503 }", click_text("#settings-page button", "Payment and invoices"), "Payment and invoices didn't open. Try again in a moment.".to_string()),
        (
            "Remove seat, the org's last paid one",
            "{ status: 400, reason: 'last_paid_seat' }",
            format!("([...document.querySelectorAll('.billing-seats tr')].find((r) => r.textContent.includes({teammate:?})).querySelector('button').click(), true)"),
            "This is your org's last paid seat: to end it, cancel the subscription in Payment and invoices.".to_string(),
        ),
        ("Add a seat, offers used up today", "{ status: 429, reason: 'offers_per_day' }", typed("Add a seat", "billing-page-more@e2e.test"), "Your org has sent as many offers as it can today. Try again tomorrow.".to_string()),
        ("Add an admin", "{ status: 503 }", typed("Add an admin", "billing-page-admin@e2e.test"), "No admin's place was offered to billing-page-admin@e2e.test. Try again in a moment.".to_string()),
    ];
    let mut said = vec![];
    for (what, next, act, want) in steps {
        b.eval(&page, &format!("(window.__next = {next}, true)"))?;
        b.eval(&page, &act)?;
        let shown = b.until(&page, &format!("[...document.querySelectorAll('#settings-page .billing-note')].some((n) => n.textContent === {want:?})"), wait);
        said.push(json!({ "what": what, "said": shown }));
    }
    let seen = b.eval(&page, "({ notes: [...document.querySelectorAll('#settings-page .billing-note')].map((n) => n.textContent), server: document.body.innerText.includes('the platform said this for the test'), warned: window.__warned.filter((w) => w.includes('the platform said this for the test')).length })")?;
    s.ok(
        "every Billing and Org control whose call fails says what happened and what to do, in plain words by its refusal; the platform's words go to the console, never the page",
        ready && said.iter().all(|x| x["said"] == true) && seen["server"] == false && seen["warned"] == 6,
        json!({ "steps": said, "seen": seen }),
    );
    b.close(page)?;

    // ---- a stale two-admin view (#481 review): an org whose subscription
    // has ended, its payer and another admin, and a member; the other admin
    // steps down while the payer's settings still show them both; the
    // payer's own Not admin is then refused as the last admin, and says to
    // make someone else an admin first (never to cancel a subscription)
    let pat = api.person()?;
    let pat_org = buys(s, api, &pat)?;
    let pat_sub = subscription_of(s, &pat_org).context("the payer's subscription")?;
    let ben = api.person()?;
    let r = api.signed(&pat, "POST", "/api/org/admins", Some(&json!({ "email": Api::email_of(&ben) })))?;
    answer(api, &ben, r.body["admin"]["id"].as_str().unwrap_or(""), "accept")?;
    let cal = api.person()?;
    let r = api.signed(&pat, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&cal) })))?;
    answer(api, &cal, r.body["seat"]["id"].as_str().unwrap_or(""), "accept")?;
    s.stripe.set_status(&pat_sub, "canceled").map_err(anyhow::Error::msg)?;
    let ended = s.eventually(PUSHED, || seat(api, &pat)["seat"]["good"] == false);
    let pat_session = api.sign_in(&Api::email_of(&pat))?;
    let page = page_of(&mut b, api, &pat_session, "/settings")?;
    let own_row = format!("[...document.querySelectorAll('.billing-seats tr')].find((r) => r.textContent.includes({:?}))", Api::email_of(&pat));
    let step_down = format!("[...({own_row})?.querySelectorAll('button') ?? []].find((b) => b.textContent === 'Not admin')");
    let both = view(&mut b, &page, &format!("!!{step_down}"), wait);
    let org_view = api.signed(&ben, "GET", "/api/org", None)?;
    let ben_id = api.identity(&ben)?;
    let ben_row = org_view.body["members"].as_array().into_iter().flatten().find(|m| m["person"] == ben_id.as_str()).and_then(|m| m["id"].as_str()).unwrap_or("").to_string();
    let stepped = api.signed(&ben, "DELETE", &format!("/api/org/admins/{ben_row}"), None)?;
    b.eval(&page, &format!("(window.confirm = () => true, {step_down}.click(), true)"))?;
    let told = b.until(&page, "[...document.querySelectorAll('#settings-page .billing-note')].some((n) => n.textContent === 'That would leave the org with no admin: make someone else an admin first.')", wait);
    let notes = b.eval(&page, "[...document.querySelectorAll('#settings-page .billing-note')].map((n) => n.textContent)")?;
    s.ok(
        "a stale two-admin view: the payer of an org whose subscription ended steps down after the other admin did, and is told it would leave the org with no admin (its last-admin refusal's reason), not to cancel a subscription",
        ended && both && stepped.status == 200 && told,
        json!({ "notes": notes, "stepped": stepped.status }),
    );
    b.close(page)?;

    // ---- a guest offered a seat: the welcome shows the offer first (its
    // org, who made it, Accept and Decline); accepted, on to their first run
    let kai = guest(api, "billing-page-offered")?;
    let r = api.signed(&gus, "POST", "/api/org/seats", Some(&json!({ "email": Api::email_of(&kai) })))?;
    anyhow::ensure!(r.status == 200, "offering the guest a seat: {r}");
    let session = api.sign_in(&Api::email_of(&kai))?;
    let page = page_of(&mut b, api, &session, "/")?;
    let offered = format!(
        "{WELCOMED} && (() => {{ const o = document.querySelector('#first-run-card .billing-offer'); return !!o && o.innerText.includes({:?}) && o.innerText.includes('offers you an always-on seat') && [...o.querySelectorAll('button')].map((b) => b.textContent).join() === 'Accept,Decline'; }})()",
        Api::email_of(&gus)
    );
    let shown = view(&mut b, &page, &offered, wait);
    let _ = b.screenshot(&page, &shots.join("5b-offered.png"));
    let seen = b.eval(&page, SEEN)?;
    if !act(s, &mut b, &page, "the welcome's Accept", &press("Accept"))? {
        return Ok(());
    }
    let seated = view(&mut b, &page, SEATED, wait);
    let made = s.eventually(wait, || owned(api, &kai, "agent") == 1);
    s.ok(
        "a guest offered a seat lands on the welcome, the offer first: its org, who made it, Accept and Decline; accepted, the seat is theirs and their first run makes their agent",
        shown && seated && made && seat(api, &kai)["seat"]["good"] == true && seat(api, &kai)["offered"] == json!([]),
        json!({ "seen": seen, "seat": seat(api, &kai) }),
    );
    b.close(page)?;

    // ---- an invited guest: someone with a seat invites the guest's email
    // to an app they titled; the guest lands home, not the welcome, the
    // invite in their sidebar (decision 63), and their Accept opens it
    let owner = api.person()?;
    let owner_email = Api::email_of(&owner);
    let label = format!("garden-{}", &Keys::generate().pubkey_hex()[..6]);
    let made = api.signed(&owner, "POST", "/api/fragments", Some(&json!({ "label": label, "template": "todo", "title": "Garden plans" })))?;
    anyhow::ensure!(made.status == 200, "making the app to share: {made}");
    let app = made.body["name"].as_str().unwrap_or("").to_string();
    let bea = guest(api, "billing-page-invited")?;
    let r = api.signed(&owner, "POST", &format!("/api/f/{app}/invites"), Some(&json!({ "email": Api::email_of(&bea), "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "sharing it with the guest: {r}");
    let session = api.sign_in(&Api::email_of(&bea))?;
    let page = page_of(&mut b, api, &session, "/")?;
    let invite = format!("#invites .invite[data-invite={app:?}]");
    let waits = view(
        &mut b,
        &page,
        &format!("location.pathname === '/' && !!document.querySelector({:?}) && !document.querySelector('#apps [data-key=\"app:{app}\"]') && /accept their invite in the sidebar/.test(document.getElementById('notice')?.innerText ?? '')", format!("{invite} .invite-accept")),
        wait,
    );
    let said = b.eval(&page, &format!("document.querySelector({invite:?})?.innerText ?? ''"))?;
    let _ = b.screenshot(&page, &shots.join("6-invited-guest-invite.png"));
    s.ok(
        "an invited guest lands home, not the welcome: the invite in their sidebar, titled as its owner titled it, from whom and as what, with Accept and Decline; nothing of it in their dock yet, and their note says where to accept it",
        waits && said.as_str().is_some_and(|t| t.contains("Garden plans") && t.contains(&format!("{owner_email} invited you to edit it")) && t.contains("Decline")),
        &said,
    );
    if !act(s, &mut b, &page, "the invite's Accept", &click(&format!("{invite} .invite-accept")))? {
        return Ok(());
    }
    let landed = view(
        &mut b,
        &page,
        &format!("location.pathname === '/' && !!document.querySelector('.viewer iframe[data-fragment={app:?}]') && /Get a seat/.test(document.getElementById('notice')?.innerText ?? '')"),
        wait,
    );
    let _ = b.screenshot(&page, &shots.join("6-invited-guest.png"));
    let row = b.eval(&page, &format!("document.querySelector('#apps [data-key=\"app:{app}\"] .label')?.textContent ?? null"))?;
    // every control that makes something (a guest's create is refused: the
    // dock's Add, the top bar's new chat, the launcher's start and send, the
    // desktop's requests), the launcher open with words typed: it only finds
    if !act(s, &mut b, &page, "the sidebar's new chat", &click("#sidebar-new-chat"))? {
        return Ok(());
    }
    let typed = "(() => { const i = document.getElementById('workspace-search'); if (!i) return false; i.value = 'garden'; i.dispatchEvent(new Event('input')); return true; })()";
    if !act(s, &mut b, &page, "the launcher's search", typed)? {
        return Ok(());
    }
    let found = b.until(&page, &format!("!!document.querySelector('#search-results [data-fragment={app:?}]')"), wait);
    let makes = b.eval(
        &page,
        "[document.getElementById('add-app') && 'add-app', !document.getElementById('new-chat').hidden && 'new-chat', !document.getElementById('search-send').disabled && 'search-send', document.querySelector('#search-results .search-start') && 'search-start', document.querySelector('#desktop-empty .desktop-suggestion') && 'suggestion'].filter(Boolean)",
    )?;
    b.eval(&page, "(document.getElementById('search-dialog')?.dispatchEvent(new Event('cancel')), true)")?;
    b.until(&page, "!document.getElementById('search-dialog')?.open", wait);
    s.ok(
        "their Accept opens what was shared with them: its window open and titled as its owner titled it, a seat offered beside it, and no control that only fails (their launcher only finds)",
        landed && row == json!("Garden plans") && found && makes == json!([]),
        json!({ "row": row, "makes": makes }),
    );
    // which email is signed in, in bold, and another account, beside the seat
    let who = b.eval(&page, super::shell::NO_SEAT_SEEN)?;
    s.ok(
        "beside it they are told which email is signed in, in bold, that it has no seat, and offered Use another account (back home) after Get a seat",
        who["said"] == json!([super::shell::no_seat_said(&Api::email_of(&bea))])
            && who["bold"] == json!([Api::email_of(&bea)])
            && who["switches"] == json!([["/auth/switch?return=%2F", "post", "Use another account"]])
            && b.eval(&page, "[...document.querySelectorAll('#notice .notice-row > *')].map((e) => e.textContent)")? == json!(["Get a seat", "Use another account"]),
        &who,
    );
    if !act(s, &mut b, &page, "the guest's Get a seat", &click("#notice .allow"))? {
        return Ok(());
    }
    let offered = b.until(&page, &format!("{WELCOMED} && [...document.querySelectorAll('#first-run-card a')].some((a) => a.textContent === 'Not now' && a.getAttribute('href') === '/')"), wait);
    s.ok("their Get a seat opens the welcome, whose Not now goes back to what is shared with them", offered, b.eval(&page, SEEN)?);
    b.close(page)?;

    // ---- another's Checkout returned; the node restarts while a buyer is
    // at Checkout, and no webhook comes; a reload mid-wait; the webhook late
    let ida = guest(api, "billing-page-restart")?;
    let session = api.sign_in(&Api::email_of(&ida))?;
    let page = page_of(&mut b, api, &session, &format!("/welcome?checkout={paid}"))?;
    let failed = view(
        &mut b,
        &page,
        "[...document.querySelectorAll('#first-run-card .creating-steps li')].some((li) => li.className === 'failed' && li.textContent === 'Setting up your seat') && [...document.querySelectorAll('#first-run-card button')].some((b) => !b.hidden && b.textContent === 'Choose a seat')",
        wait,
    );
    let seen = b.eval(&page, SEEN)?;
    let _ = b.screenshot(&page, &shots.join("7-not-theirs.png"));
    let chose = b.eval(&page, &press("Choose a seat"))? == json!(true) && b.until(&page, &format!("{WELCOMED} && location.search === ''"), wait);
    s.ok(
        "another's Checkout returned is refused and said in plain words (never the platform's), and applies nothing, its address kept until then; Choose a seat goes back to the welcome's seats",
        failed
            && seen["search"].as_str().is_some_and(|q| q.contains("checkout="))
            && seen["errors"].as_array().is_some_and(|e| e.iter().any(|t| t == "This payment wasn't found. Choose a seat to start again.") && !e.iter().any(|t| t.as_str().is_some_and(|t| t.contains("no Checkout"))))
            && chose
            && seat(api, &ida)["seat"].is_null(),
        &seen,
    );
    if !act(s, &mut b, &page, "the welcome's seat", &click("#first-run-card [data-plan=seat_always_on]"))? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
    // the webhook lost, and the node restarted, while they are on Stripe's page
    s.stripe.hold_events(true);
    let delivered = s.stripe.delivered().len();
    s.stop()?;
    let restarted = s.start(false);
    let seated = restarted.is_ok() && b.click(&page, "form button").is_ok() && view(&mut b, &page, SEATED, wait);
    let held = s.stripe.delivered().len() == delivered;
    let api = &restarted?;
    let mine = seat(api, &ida);
    s.ok(
        "the node restarted while they were at Checkout, and no webhook came: its return alone, after the restart, seats them (the always-on seat) and goes on to their first agent",
        at_stripe && seated && held && mine["seat"]["kind"] == "seat_always_on" && mine["seat"]["good"] == true,
        json!({ "seen": b.eval(&page, SEEN)?, "seat": mine, "held": held }),
    );
    // a reload while its computer starts (the first run's longest wait)
    let midway = b.until(&page, "[...document.querySelectorAll('#first-run-card .creating-steps li')].some((li) => li.textContent === 'Starting its computer' && li.className === 'doing') || !document.getElementById('layout').hidden", wait);
    b.reload(&page)?;
    let resumed = view(
        &mut b,
        &page,
        "location.pathname === '/' && (document.getElementById('first-run').hidden ? !!document.querySelector('#frames iframe[data-fragment]') : !!document.querySelector('#first-run-card .creating-steps li.doing'))",
        wait,
    );
    let _ = b.screenshot(&page, &shots.join("8-reloaded.png"));
    // the in-chat first run makes the chat before the agent: the agent may
    // still be on its way as the reloaded page shows the chat
    let once = s.eventually(wait, || owned(api, &ida, "agent") == 1 && owned(api, &ida, "chat") == 1);
    s.ok(
        "a reload mid-wait goes on where it stopped, at home's address: one agent, one chat, nothing made twice",
        midway && resumed && once,
        json!({ "seen": b.eval(&page, SEEN)?, "agents": owned(api, &ida, "agent"), "chats": owned(api, &ida, "chat") }),
    );
    s.stripe.release_events(false);
    let late = s.eventually(wait, || s.stripe.delivered().len() > delivered);
    let after = seat(api, &ida);
    s.ok(
        "the webhook, come late, changes nothing: the seat as the return made it",
        late && after["seat"] == mine["seat"] && s.stripe.delivered()[delivered..].iter().all(|(_, _, status)| *status == 200),
        json!({ "seat": after, "delivered": s.stripe.delivered()[delivered..].to_vec() }),
    );
    b.close(page)?;

    // ---- a trial mailed: signed out, its link signs in and comes back with
    // its code, filled in; its trial's Checkout returns to the welcome too
    let code = api.signed(&s.operator, "POST", "/api/admin/trials", Some(&json!({ "name": "Page", "days": 30, "capacity": 5 })))?;
    let code = code.body["code"].as_str().unwrap_or("").to_string();
    let nobody = b.another_context()?;
    let out = b.open_in(&nobody, &format!("{}/welcome?trial={code}", api.base))?;
    let to_welcome = format!("%2Fwelcome%3Ftrial%3D{code}");
    let (up, back_in) = (format!("/auth/login?screen_hint=sign-up&return={to_welcome}"), format!("/auth/login?screen_hint=sign-in&return={to_welcome}"));
    let asked = view(&mut b, &out, &format!("document.querySelector('#first-run-card a.primary')?.getAttribute('href') === {up:?}"), wait);
    let offered = b.eval(&out, "({ up: document.querySelector('#first-run-card a.primary')?.getAttribute('href') ?? null, in: document.querySelector('#first-run-card a.splash-secondary')?.getAttribute('href') ?? null })")?;
    s.ok(
        "signed out, a trial's link offers Sign Up and Sign In, each coming back to the welcome with its code",
        asked && offered == json!({ "up": up, "in": back_in }),
        &offered,
    );
    b.close(out)?;
    // Sign In, the whole round trip, from the trial's link and from
    // settings: with AuthKit holding a session in the browser and without
    let (held, asked) = (guest(api, "billing-page-sign-in-held")?, guest(api, "billing-page-sign-in-asked")?);
    let welcome_with_code = format!("{WELCOMED} && location.search === '' && document.querySelector('#first-run-card input[placeholder=\"Trial code\"]')?.value === {code:?}");
    let guest_settings = "location.pathname === '/settings' && /You are a guest/.test(document.getElementById('settings-page')?.innerText ?? '')";
    for (path, landed, place) in [(format!("/welcome?trial={code}"), welcome_with_code.as_str(), "the welcome, its trial code filled in"), ("/settings".to_string(), guest_settings, "settings")] {
        signed_in_back(s, &mut b, api, &path, &Api::email_of(&held), true, landed, place)?;
        signed_in_back(s, &mut b, api, &path, &Api::email_of(&asked), false, landed, place)?;
    }
    let jo = guest(api, "billing-page-trial")?;
    let session = api.sign_in(&Api::email_of(&jo))?;
    let page = page_of(&mut b, api, &session, &format!("/welcome?trial={code}"))?;
    let filled = view(&mut b, &page, &format!("{WELCOMED} && location.search === '' && document.querySelector('#first-run-card input[placeholder=\"Trial code\"]')?.value === {code:?}"), wait);
    // evidence for a person: the welcome as a mailed trial opens it (kept with a run's scratch)
    let _ = b.screenshot(&page, &shots.join("9-trial.png"));
    let started = "(() => { const f = document.querySelector('#first-run-card form.trial-code'); f?.requestSubmit(); return !!f; })()";
    if !act(s, &mut b, &page, "the welcome's Start a trial", started)? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?}) && /30-day trial/.test(document.body.innerText) && /no card asked/.test(document.body.innerText)", s.stripe.url), wait);
    if !act_at(s, &mut b, &page, "Stripe's Pay", "form button") {
        return Ok(());
    }
    let seated = view(&mut b, &page, SEATED, wait);
    let mine = seat(api, &jo);
    s.ok(
        "signed in, the link's welcome has its code filled in; its 30-day trial asks no card on Stripe's page and comes back to the welcome, which seats them in their trial of the always-on seat and goes on to their first agent",
        filled && at_stripe && seated && mine["seat"]["good"] == true && mine["seat"]["kind"] == "seat_always_on" && mine["seat"]["trialEnds"].as_i64().is_some(),
        json!({ "seen": b.eval(&page, SEEN)?, "seat": mine }),
    );
    b.close(page)?;
    // their Billing, in the trial: when it ends, and how to keep the seat after it
    let page = page_of(&mut b, api, &session, "/settings")?;
    let told = view(
        &mut b,
        &page,
        "location.pathname === '/settings' && ((t) => /Trial ends/.test(t) && /add a card in Payment and invoices/.test(t))(document.getElementById('settings-page')?.innerText ?? '') && [...document.querySelectorAll('#settings-page button')].some((b) => b.textContent === 'Payment and invoices')",
        wait,
    );
    s.ok(
        "in their trial, their Billing says when it ends and that a card added in Payment and invoices keeps the seat after it (none asked at its start)",
        told,
        json!({ "shown": text(&mut b, &page) }),
    );
    b.close(page)?;

    // ---- a seat given (an operator's comp; an org's admin gives one the
    // same way): its plan may not have reached their ledger as they land,
    // and either way they go straight to the first run, never the seats
    let max = guest(api, "billing-page-given")?;
    let given = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&max), "kind": "seat" })))?;
    let session = api.sign_in(&Api::email_of(&max))?;
    let page = page_of(&mut b, api, &session, "/")?;
    // its seat's own step while its plan is on its way, else the chat at once
    let straight = view(
        &mut b,
        &page,
        "location.pathname === '/' && !document.querySelector('#first-run-card [data-plan]') && (!!document.querySelector('#first-run-card .creating-steps') || (!document.getElementById('layout').hidden && !!document.querySelector('#frames iframe[data-fragment]')))",
        wait,
    );
    let made = s.eventually(wait, || owned(api, &max, "agent") == 1);
    s.ok(
        "a guest given a seat goes straight to their first run, never the welcome's seats, and it makes their agent",
        given.status == 200 && straight && made,
        json!({ "given": given.body, "seen": b.eval(&page, SEEN)? }),
    );

    // ---- their comp ends: their ledger keeps its plan, canceled (decision
    // 53), and they keep their chat; to the shell, its Billing and its
    // welcome alike they are a guest, and Get a seat reaches the seat
    let comp = given.body["seat"]["id"].as_str().unwrap_or("").to_string();
    let ended = api.signed(&s.operator, "DELETE", &format!("/api/admin/seats/{comp}"), None)?;
    let guested = s.eventually(PUSHED, || seat(api, &max)["guest"] == true);
    let (kept, mine) = (ledger(api, &max), seat(api, &max));
    s.ok(
        "their comp ended, GET /api/seat says they are a guest (no seat), though their ledger keeps the seat's plan, canceled",
        ended.status == 200 && guested && mine["seat"].is_null() && kept["plan"] == "seat" && kept["seat"] == "canceled" && owned(api, &max, "chat") == 1,
        json!({ "ended": ended.body, "seat": mine, "ledger": { "plan": kept["plan"], "seat": kept["seat"] } }),
    );
    b.reload(&page)?;
    let home = view(&mut b, &page, "location.pathname === '/' && !document.getElementById('layout').hidden && !!document.querySelector('#frames iframe[data-fragment]') && document.getElementById('sidebar-new-chat')?.getAttribute('aria-label') === 'Search chats and apps'", wait);
    let makes = b.eval(&page, "[!document.getElementById('new-chat').hidden && 'new-chat', !document.getElementById('sidebar-start-chat').hidden && 'sidebar-start-chat', document.getElementById('add-app') && 'add-app'].filter(Boolean)")?;
    b.eval(&page, "(location.assign('/settings'), true)")?;
    let billed = view(&mut b, &page, "location.pathname === '/settings' && /You are a guest/.test(document.getElementById('settings-page')?.innerText ?? '')", wait);
    let headings = b.eval(&page, "[...document.querySelectorAll('#settings-page h2')].map((h) => h.textContent)")?;
    s.ok(
        "the shell agrees: home, their chat open, makes nothing (its launcher only finds), and their Billing says they are a guest, with settings a guest's (no computer or agents)",
        home && makes == json!([]) && billed && headings.as_array().is_some_and(|h| ["Computer", "Agents"].iter().all(|x| !h.contains(&json!(x)))),
        json!({ "makes": makes, "headings": headings, "billing": text(&mut b, &page) }),
    );
    if !act(s, &mut b, &page, "their Billing's Get a seat", &click_text("#settings-page a", "Get a seat"))? {
        return Ok(());
    }
    let offered = view(
        &mut b,
        &page,
        &format!("{WELCOMED} && [...document.querySelectorAll('#first-run-card a')].some((a) => a.textContent === 'Not now') && document.getElementById('first-run-card').innerText.includes({:?})", format!("{seat_price} / month")),
        wait,
    );
    let _ = b.screenshot(&page, &shots.join("10-comp-ended.png"));
    let seen = b.eval(&page, SEEN)?;
    if !act(s, &mut b, &page, "the welcome's seat", &click("#first-run-card [data-plan=seat_always_on]"))? {
        return Ok(());
    }
    let at_stripe = b.until(&page, &format!("location.href.startsWith({:?})", s.stripe.url), wait);
    s.ok(
        "its Get a seat reaches the seat offer, though they own a chat (not home, nor a first run): the one seat at core's price, Not now back to their chat, and its seat goes to Stripe's Checkout",
        offered && at_stripe,
        json!({ "seen": seen, "at": b.eval(&page, "location.href")? }),
    );
    b.close(page)?;
    println!("      (screenshots: {})", shots.display());
    Ok(())
}
