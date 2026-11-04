//! A native client's approved key uses the existing one-fragment web sessions.
use crate::api::{Api, Call};
use crate::Suite;
use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::json;

pub(super) fn key_site_cookie(api: &Api, key: &Keys, name: &str) -> Result<String> {
    let minted = api.signed(
        key,
        "POST",
        &format!("/api/f/{name}/session"),
        Some(&json!({})),
    )?;
    anyhow::ensure!(
        minted.status == 200 && minted.body["consentRequired"] == false,
        "native session mint: HTTP {}",
        minted.status
    );
    let url = minted.body["url"]
        .as_str()
        .context("native session URL")?
        .to_string();
    let redeemed = api.call(Call {
        method: "GET",
        url,
        ..Call::default()
    })?;
    anyhow::ensure!(
        redeemed.status == 302,
        "native redemption: HTTP {}",
        redeemed.status
    );
    redeemed
        .cookies()
        .into_iter()
        .find(|c| c.starts_with("fragment_site="))
        .context("fragment-only cookie")
}

pub(super) fn key_sessions(s: &mut Suite, api: &Api) -> Result<()> {
    let phone = api.person()?;
    let browser = api.sign_in(&Api::email_of(&phone))?;
    let backup = Keys::generate();
    api.approve(&browser, &backup)?;
    let name = s.named(api, &phone, "native-brain")?;
    let made = api.create_with(
        &phone,
        json!({"name":name,"template":"brain","visibility":"members"}),
    )?;
    anyhow::ensure!(made.status == 200, "native brain setup");
    let path = format!("/api/f/{name}/session");
    let r = api.unsigned("POST", &path, Some(&json!({})))?;
    s.ok(
        "native web sessions require a signed key",
        r.status == 401,
        &r,
    );
    let stray = Keys::generate();
    let r = api.signed(&stray, "POST", &path, Some(&json!({})))?;
    s.ok(
        "a key no person approved cannot mint a web session",
        r.status == 401,
        &r,
    );
    for back in [
        "https://outside.invalid/",
        "//outside.invalid/",
        "/\\outside",
        "/path\n",
    ] {
        let r = api.signed(&phone, "POST", &path, Some(&json!({"returnTo":back})))?;
        s.ok(
            "a native session return path cannot leave its fragment",
            r.status == 400,
            r.status,
        );
    }
    let agent = Keys::generate();
    let r = api.signed(
        &phone,
        "POST",
        "/api/identities",
        Some(&json!({"kind":"agent","proof":api.proof(&agent,"POST","/api/identities",&phone)})),
    )?;
    anyhow::ensure!(r.status == 200, "native agent setup");
    let r = api.signed(&agent, "POST", &path, Some(&json!({})))?;
    s.ok(
        "an agent's key cannot mint a person's web session",
        r.status == 403,
        &r,
    );

    let minted = api.signed(
        &phone,
        "POST",
        &path,
        Some(&json!({"returnTo":"/#/index.md"})),
    )?;
    s.ok(
        "an owner's approved phone key mints a top-level fragment redemption without another login",
        minted.status == 200
            && minted.body["consentRequired"] == false
            && minted.header("cache-control") == "no-store",
        minted.status,
    );
    let url = minted.body["url"]
        .as_str()
        .context("native redemption URL")?
        .to_string();
    let token = reqwest::Url::parse(&url)?
        .query()
        .context("redemption query")?
        .to_string();
    let other = s.named(api, &phone, "native-other")?;
    api.create_with(
        &phone,
        json!({"name":other,"template":"brain","visibility":"members"}),
    )?;
    let wrong = api.page(&other, &format!("__signin?{token}"), None)?;
    s.ok(
        "a native redemption cannot be spent on another fragment",
        wrong.status == 401,
        wrong.status,
    );
    let redeemed = api.call(Call {
        method: "GET",
        url: url.clone(),
        ..Call::default()
    })?;
    s.ok(
        "refusing another fragment left the right fragment's redemption unspent",
        redeemed.status == 302 && redeemed.header("location").ends_with("/#/index.md"),
        redeemed.status,
    );
    let cookie = redeemed
        .cookies()
        .into_iter()
        .find(|c| c.starts_with("fragment_site="))
        .context("native cookie")?;
    let replayed = api.call(Call {
        method: "GET",
        url,
        ..Call::default()
    })?;
    s.ok(
        "native redemptions are one use",
        replayed.status == 401,
        replayed.status,
    );
    let private = api.page(&name, "__files", Some(&cookie))?;
    s.ok(
        "the native web sheet can load the real private Files UI",
        private.status == 200 && private.text.contains("./__files.js"),
        private.status,
    );
    let other_cookie = key_site_cookie(api, &backup, &name)?;

    let stranger = api.person()?;
    let public = s.named(api, &stranger, "native-public")?;
    api.create_with(
        &stranger,
        json!({"name":public,"template":"brain","visibility":"public"}),
    )?;
    let public_path = format!("/api/f/{public}/session");
    let r = api.signed(&phone, "POST", &public_path, Some(&json!({})))?;
    s.ok(
        "an unfamiliar public app asks before learning the person's identity",
        r.status == 200 && r.body["consentRequired"] == true && r.body["url"].is_null(),
        r.status,
    );
    let r = api.signed(&phone, "POST", &public_path, Some(&json!({"consent":true})))?;
    s.ok(
        "explicit native consent mints the same scoped session",
        r.status == 200 && r.body["url"].is_string(),
        r.status,
    );
    let r = api.signed(&phone, "POST", &public_path, Some(&json!({})))?;
    s.ok(
        "the existing consent mechanism remembers that yes",
        r.status == 200 && r.body["consentRequired"] == false,
        r.status,
    );

    let pending = api.signed(&phone, "POST", &path, Some(&json!({})))?;
    let pending_url = pending.body["url"]
        .as_str()
        .context("unspent native redemption")?
        .to_string();
    let r = api.signed(
        &backup,
        "DELETE",
        &format!("/api/identities/me/keys/{}", phone.pubkey_hex()),
        None,
    )?;
    anyhow::ensure!(r.status == 200, "revoke native key");
    let revoked = api.page(&name, "__files", Some(&cookie))?;
    let pending = api.call(Call {
        method: "GET",
        url: pending_url,
        ..Call::default()
    })?;
    let mint = api.signed(&phone, "POST", &path, Some(&json!({})))?;
    s.ok(
        "revoking the phone key invalidates its web cookie, unspent redemption, and future mints",
        revoked.status == 401 && pending.status == 401 && mint.status == 401,
        format!("{} {} {}", revoked.status, pending.status, mint.status),
    );
    let kept = api.page(&name, "__files", Some(&other_cookie))?;
    let browser_kept = api.who(&browser)?;
    s.ok(
        "revoking a phone leaves another device and the browser signed in",
        kept.status == 200 && browser_kept["kind"] == "person",
        kept.status,
    );
    Ok(())
}
