//! A deployment on a runtime without some of Cloudflare's services, as a
//! self-hosted one runs (docs/self-host.md): the node restarted without
//! those bindings (`Shape::Unbound`, `devstack::Fleet::unbound`), on
//! wrangler as on any runtime, so master's own runs check the cell without
//! them. Valid, each said and never a failure of the flow around it:
//! - no mailer (no Email Sending, no mail fake): an invite to someone new
//!   still waits for them, their first sign-in sees it and makes them a
//!   member, and each mail the platform would have sent is said in the
//!   node's log;
//! - no Browser Rendering: a deploy's card is skipped, saying why, and a
//!   shot asked for again is refused, typed;
//! - no Images: an icon's making, and an upload that needs fitting, are
//!   refused at once, typed, with nothing asked of the model;
//! - no containers (no computers): a person's wipe finishes.
//!
//! Then the node as the lanes expect it again.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use fragment_proto::ErrorCode;
use serde_json::json;

use crate::api::{Api, Reply};
use crate::{Need, Suite};

/// What a node with no mailer says of each mail it does not send
/// (cell/src/mail.rs).
const NO_MAILER: &str = "email not sent: this runtime has no mailer, not yet implemented";

pub fn unbound(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("unbound", &[Need::Node, Need::Deployment, Need::Levers, Need::Operator]) {
        return Ok(());
    }
    s.stop()?;
    let api = s.start_unbound()?;
    no_mailer(s, &api)?;
    no_browser(s, &api)?;
    no_images(s, &api)?;
    no_containers(s, &api)?;
    s.stop()?;
    s.start(false)?;
    Ok(())
}

/// No mailer (docs/technical-debt-ledger.md, "No mail without Email
/// Sending"): the invite and the sign-in's key, each said in the node's log.
fn no_mailer(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let name = s.named(api, &owner, "no-mailer")?;
    s.create(api, &owner, &name)?;
    let newcomer = Keys::generate();
    // not an e2e person's address, which the cell never mails (its test hooks)
    let email = format!("newcomer-{}@example.com", &newcomer.pubkey_hex()[..12]);
    let invited = api.signed(&owner, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": email, "role": "viewer" })))?;
    let session = api.sign_in(&email)?;
    api.approve(&session, &newcomer)?;
    let waiting = api.signed(&newcomer, "GET", "/api/invites", None)?;
    let yes = api.signed(&newcomer, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    let theirs = api.status(&newcomer, &name)?;
    s.ok(
        "with no mailer, an invite to someone new still waits for them, and their first sign-in sees it and makes them a member",
        invited.status == 200 && waiting.body["invites"].as_array().is_some_and(|l| l.iter().any(|i| i["fragment"] == name.as_str())) && yes.status == 200 && theirs.body["role"] == "viewer",
        format!("{invited} | {waiting} | {yes} | {theirs}"),
    );
    let said = s.eventually(Duration::from_secs(10), || s.node_log().is_some_and(|l| l.matches(NO_MAILER).count() >= 2));
    let log = s.node_log().unwrap_or_default();
    s.ok("and each mail it would have sent (the invite, the sign-in's key) is said in the node's log, not sent", said, log.lines().filter(|l| l.contains("mail.unsent")).collect::<Vec<_>>().join("\n"));
    Ok(())
}

/// No Browser Rendering: a deploy's card is skipped, said once
/// (`card.skipped`, `no_browser`), and a shot asked for again is refused,
/// typed (`host_failed`), saying why.
fn no_browser(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let name = s.named(api, &owner, "no-browser")?;
    let c = s.create(api, &owner, &name)?;
    // as anyone sees it: a page a shot would open
    api.signed(&owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "public" })))?;
    s.commit(&c, &[("site/index.html", Some(b"<!doctype html><p>no browser here</p>"))]);
    // live moves: its card is wanted
    s.deploy(&c);
    let skips = || -> Vec<serde_json::Value> {
        let r = api.signed(&owner, "GET", &format!("/api/f/{name}/events?tail=200"), None).map(|r| r.body).unwrap_or_default();
        r["events"].as_array().into_iter().flatten().filter(|e| e["kind"] == "card.skipped").map(|e| e["data"].clone()).collect()
    };
    s.eventually(super::site::CARD_WAIT, || !skips().is_empty());
    let skipped = skips();
    let asked = api.signed(&owner, "POST", &format!("/api/f/{name}/card"), Some(&json!({})))?;
    let live = api.status(&owner, &name)?.body["pins"]["live"].clone();
    s.ok(
        "with no browser, a deploy's card is skipped once, saying why (card.skipped, no_browser), and a shot asked for again is refused, typed (host_failed)",
        skipped.len() == 1 && skipped[0]["why"] == "no_browser" && asked.code() == Some(ErrorCode::HostFailed) && asked.message().contains("no browser to shoot cards with"),
        json!({ "skipped": skipped, "asked": asked.to_string(), "live": live, "events": super::site::event_kinds(api, &owner, &name) }),
    );
    Ok(())
}

/// No Images: a making, and an upload that needs fitting, refused at once,
/// typed, saying why, with nothing asked of the model.
fn no_images(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let name = s.named(api, &owner, "no-images")?;
    s.create(api, &owner, &name)?;
    let mark = s.ai.calls().len();
    let made = super::icons::make(api, &owner, &name, json!({ "hint": "a teapot" }))?;
    let fitted = super::icons::put(api, &owner, &name, &super::icons::photo())?;
    let said = |r: &Reply| r.code() == Some(ErrorCode::HostFailed) && r.message().contains("no Images binding");
    s.ok(
        "with no Images, a making and an upload that needs fitting are refused at once, typed (host_failed), saying why, and nothing is asked of the model",
        said(&made) && said(&fitted) && s.ai.calls().len() == mark,
        json!({ "make": made.to_string(), "upload": fitted.to_string(), "calls": s.ai.calls().len() - mark }),
    );
    Ok(())
}

/// No containers: the container host reads none running, so a person's
/// wipe, which ends their computer, finishes.
fn no_containers(s: &mut Suite, api: &Api) -> Result<()> {
    let wiper = s.wiper.clone().context("Need::Operator lends an operator key")?;
    let person = api.person()?;
    let id = api.identity(&person)?;
    let (r, ran) = super::billing::wipe_until_done(api, &wiper, &id)?;
    s.ok("with no containers, a person's wipe finishes", r.status == 200 && r.body["state"] == "wiped", json!({ "last": r.to_string(), "ran": ran }));
    Ok(())
}
