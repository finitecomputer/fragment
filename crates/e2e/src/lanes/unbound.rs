//! A deployment on a runtime without some of Cloudflare's services, as a
//! self-hosted one runs (docs/self-host.md): the node restarted without
//! those bindings (`Shape::Unbound`, `devstack::Fleet::unbound`), on
//! wrangler as on any runtime, so master's own runs check the cell without
//! them. Valid: with no mailer (no Email Sending, no mail fake), an invite
//! to someone new still waits for them, their first sign-in sees it and
//! makes them a member, and each mail the platform would have sent is said
//! in the node's log, never an error. Then the node as the lanes expect it
//! again.

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::json;

use crate::api::Api;
use crate::Suite;

/// What a node with no mailer says of each mail it does not send
/// (cell/src/mail.rs).
const NO_MAILER: &str = "email not sent: this runtime has no mailer, not yet implemented";

pub fn unbound(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("unbound", &[crate::Need::Node, crate::Need::Deployment, crate::Need::Levers]) {
        return Ok(());
    }
    s.stop()?;
    let api = s.start_unbound()?;
    no_mailer(s, &api)?;
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
