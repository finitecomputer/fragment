//! A native app's key opens fragments' pages (docs/api.md, Sign-in: `POST
//! /api/f/{name}/session`): a person's approved key, never an agent's or a
//! stranger's, mints a one-use redemption for one fragment's top-level
//! `__signin`, as `/auth/fragment` decides for a browser (silent on their
//! own, asking first on anyone else's), and revoking that key ends every
//! site session redeemed through it, and its unspent redemptions, leaving
//! the person's other keys and browsers signed in.

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Call, Reply};
use crate::Suite;

fn ask(api: &Api, keys: &Keys, name: &str, body: Value) -> Result<Reply> {
    api.signed(keys, "POST", &format!("/api/f/{name}/session"), Some(&body))
}

/// The site cookie (`fragment_site=…`) a redemption URL sets, opened as a
/// web view opens it.
fn redeemed(api: &Api, url: &str) -> Result<(Reply, Option<String>)> {
    let r = api.call(Call { method: "GET", url: url.to_string(), ..Call::default() })?;
    let cookie = r.cookies().into_iter().find(|c| c.starts_with("fragment_site="));
    Ok((r, cookie))
}

/// `keys`' site cookie on `name`, a fragment of theirs or shared with them.
pub(super) fn key_site_cookie(api: &Api, keys: &Keys, name: &str) -> Result<String> {
    let r = ask(api, keys, name, json!({}))?;
    anyhow::ensure!(r.status == 200 && r.body["consentRequired"] == false, "a native session: {r}");
    let (r, cookie) = redeemed(api, r.body["url"].as_str().context("a redemption URL")?)?;
    cookie.with_context(|| format!("__signin set no site cookie: {r}"))
}

pub fn native(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("native", &[crate::Need::Levers]) {
        return Ok(());
    }
    let phone = api.person()?;
    // the same person's browser, and a second key of theirs
    let browser = api.sign_in(&Api::email_of(&phone))?;
    let laptop = Keys::generate();
    api.approve(&browser, &laptop)?;
    let name = s.named(api, &phone, "native")?;
    let made = api.create_with(&phone, json!({ "name": name, "template": "brain", "visibility": "members" }))?;
    anyhow::ensure!(made.status == 200, "a members-only brain: {made}");

    let r = api.unsigned("POST", &format!("/api/f/{name}/session"), Some(&json!({})))?;
    s.ok("an unsigned ask for a fragment's web session is 401", r.status == 401, &r);
    let r = ask(api, &Keys::generate(), &name, json!({}))?;
    s.ok("a key no one approved is 401", r.status == 401, &r);
    let (agent, _) = api.own_agent(&phone)?;
    let r = ask(api, &agent, &name, json!({}))?;
    s.ok("an agent's key is 403: an agent has no browser to sign in", r.status == 403, &r);
    for back in ["https://outside.invalid/", "//outside.invalid/", "/\\outside", "/path\n"] {
        let r = ask(api, &phone, &name, json!({ "returnTo": back }))?;
        s.ok(&format!("a returnTo off the fragment's origin ({back:?}) is 400"), r.status == 400, &r);
    }
    let r = ask(api, &phone, &name, json!({ "consent": true, "more": 1 }))?;
    s.ok("a field it does not know is 400", r.status == 400, &r);

    // the owner's key: silent, one use, this fragment only
    let r = ask(api, &phone, &name, json!({ "returnTo": "/#/index.md" }))?;
    s.ok(
        "an owner's approved key mints a one-use redemption at once, never stored",
        r.status == 200 && r.body["consentRequired"] == false && r.header("cache-control") == "no-store" && r.body["url"].as_str().is_some_and(|u| u.contains("/__signin?token=")),
        &r,
    );
    let url = r.body["url"].as_str().context("a redemption URL")?.to_string();
    let token = url.split_once('?').map(|(_, q)| q.to_string()).context("a redemption's query")?;
    let other = s.named(api, &phone, "native-other")?;
    api.create_with(&phone, json!({ "name": other, "template": "brain", "visibility": "members" }))?;
    let r = api.page(&other, &format!("__signin?{token}"), None)?;
    s.ok("another fragment refuses the redemption (401)", r.status == 401, &r);
    let (r, cookie) = redeemed(api, &url)?;
    s.ok("and leaves it unspent: its own fragment redeems it, back where it asked", r.status == 302 && r.header("location").ends_with("/#/index.md") && cookie.is_some(), &r);
    let cookie = cookie.context("a site cookie")?;
    let (r, _) = redeemed(api, &url)?;
    s.ok("a redemption is spent once (401)", r.status == 401, &r);
    let anonymous = api.page(&name, "__files", None)?;
    let r = api.page(&name, "__files", Some(&cookie))?;
    s.ok("its site session opens the members-only fragment's pages, which no one else sees", r.status == 200 && anonymous.status != 200, format!("{} (anonymous {})", r.status, anonymous.status));
    let kept = key_site_cookie(api, &laptop, &name)?;

    // a stranger's public fragment: asked first, then remembered
    let stranger = api.person()?;
    let public = s.named(api, &stranger, "native-public")?;
    api.create_with(&stranger, json!({ "name": public, "template": "brain", "visibility": "public" }))?;
    let r = ask(api, &phone, &public, json!({}))?;
    s.ok("a stranger's fragment asks first, with no credential", r.status == 200 && r.body["consentRequired"] == true && r.body["url"].is_null(), &r);
    let r = ask(api, &phone, &public, json!({ "consent": true }))?;
    s.ok("a yes mints its redemption", r.status == 200 && r.body["url"].is_string(), &r);
    let r = ask(api, &phone, &public, json!({}))?;
    s.ok("and the yes is remembered, as a browser's is", r.status == 200 && r.body["consentRequired"] == false && r.body["url"].is_string(), &r);

    // revoking the key ends what it signed in, and nothing else
    let r = ask(api, &phone, &name, json!({}))?;
    let unspent = r.body["url"].as_str().context("an unspent redemption")?.to_string();
    let r = api.signed(&laptop, "DELETE", &format!("/api/identities/me/keys/{}", phone.pubkey_hex()), None)?;
    anyhow::ensure!(r.status == 200, "revoking the phone's key: {r}");
    let page = api.page(&name, "__files", Some(&cookie))?;
    let (spent, _) = redeemed(api, &unspent)?;
    let mint = ask(api, &phone, &name, json!({}))?;
    s.ok(
        "revoking the key ends its site sessions, its unspent redemptions, and its mints (401 each)",
        page.status == 401 && spent.status == 401 && mint.status == 401,
        format!("{} {} {}", page.status, spent.status, mint.status),
    );
    let r = api.page(&name, "__files", Some(&kept))?;
    let me = api.who(&browser)?;
    s.ok("and leaves the person's other key's site session and their browser signed in", r.status == 200 && me["kind"] == "person", format!("{} {me}", r.status));
    Ok(())
}
