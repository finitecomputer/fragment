//! Two independent browser cookie jars and two people, against real workerd
//! and only local OAuth/Pipes fakes. No provider grants or tokens are real.
use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::json;
use crate::{Suite, Need, api::{Api, Call, Reply}};

const PROVIDER: &str = "e2e-oauth";

fn begin(api: &Api, session: &str, provider: &str) -> Result<Reply> {
    api.call(Call { method: "POST", url: format!("{}/api/connections/{provider}/authorize", api.base), cookie: Some(format!("fragment_session={session}")),
        extra: vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())], ..Call::default() })
}

fn pending(api: &Api, session: &str, account: &str) -> Result<(String, String, String)> {
    let r = begin(api, session, PROVIDER)?;
    anyhow::ensure!(r.status == 200, "begin connection: {r}");
    let cookie = r.cookies().into_iter().find(|c| c.starts_with("fragment_connection=")).context("connection cookie")?;
    let mut to = reqwest::Url::parse(r.body["url"].as_str().context("authorize URL")?)?;
    let state = to.query_pairs().find(|(k, _)| k == "state").context("state")?.1.into_owned();
    to.query_pairs_mut().append_pair("account", account);
    let consent = api.external(to.as_str())?;
    anyhow::ensure!(consent.status == 302, "fake consent: {consent}");
    Ok((consent.header("location"), cookie, state))
}

fn back(api: &Api, url: &str, session: Option<&str>, cookie: Option<&str>) -> Result<Reply> {
    let mut cookies = vec![];
    if let Some(s) = session { cookies.push(format!("fragment_session={s}")); }
    if let Some(c) = cookie { cookies.push(c.to_string()); }
    api.call(Call { method: "GET", url: url.into(), cookie: Some(cookies.join("; ")), ..Call::default() })
}

fn refused(r: &Reply, word: &str) -> bool { r.status == 303 && r.header("location").ends_with(&format!("?refused={word}")) }

pub fn connections(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("connections", &[Need::Fakes, Need::Node, Need::Levers]) { return Ok(()); }
    let a = api.sign_in("connection-a@e2e.test")?;
    let b = api.sign_in("connection-b@e2e.test")?;
    let another_a = api.sign_in("connection-a@e2e.test")?;
    let key = Keys::generate();
    api.approve(&a, &key)?;
    let before = s.workos.connection_counts();
    let signed = api.signed(&key, "POST", &format!("/api/connections/{PROVIDER}/authorize"), None)?;
    let disabled = begin(api, &a, "google")?;
    s.ok("keys and unconfigured connections cannot use the old Pipes authorize bypass", signed.status == 403 && disabled.status >= 400 && s.workos.connection_counts() == before, format!("key {}, unconfigured {}", signed.status, disabled.status));

    let (url, cookie, _) = pending(api, &a, "provider-account-a")?;
    for (label, session, held, why) in [
        ("a copied consent/callback URL in another user's browser", Some(b.as_str()), None, "elsewhere"),
        ("the initiating session without its browser cookie", Some(a.as_str()), None, "elsewhere"),
        ("the cookie without a platform session", None, Some(cookie.as_str()), "signed-out"),
        ("another user even with the copied binding cookie", Some(b.as_str()), Some(cookie.as_str()), "not-under-way"),
        ("the same user in another session", Some(another_a.as_str()), Some(cookie.as_str()), "not-under-way"),
    ] {
        let r = back(api, &url, session, held)?;
        s.ok(label, refused(&r, why) && s.workos.connection_counts() == before, &r);
    }
    let cross = cookie.replace("fragment_connection=", "fragment_own_key=");
    let r = back(api, &url, Some(&a), Some(&cross))?;
    s.ok("an own-key cookie cannot finish a Pipes connection", refused(&r, "elsewhere") && s.workos.connection_counts() == before, &r);
    let forged = url.replace("e2e-oauth/callback", "google/callback");
    let r = back(api, &forged, Some(&a), Some(&cookie))?;
    s.ok("another provider cannot exchange this code", r.status == 400 && s.workos.connection_counts() == before, &r);

    let r = back(api, &url, Some(&a), Some(&cookie))?;
    let user_a = s.workos.user("connection-a@e2e.test").id;
    let counts = s.workos.connection_counts();
    s.ok("only the original browser and live session imports the account into its own WorkOS user", r.status == 200 && counts == (before.0 + 1, before.1 + 1, before.2)
        && s.workos.imported_accounts().last() == Some(&(user_a.clone(), PROVIDER.into(), "provider-account-a".into())), &r);
    let r = back(api, &url, Some(&a), Some(&cookie))?;
    s.ok("replaying the callback performs no exchange or import", refused(&r, "not-under-way") && s.workos.connection_counts() == counts, &r);

    let (expired, held, state) = pending(api, &a, "expired")?;
    let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "expireConnection": state } })))?;
    anyhow::ensure!(hook.status == 200, "expire local transaction: {hook}");
    let r = back(api, &expired, Some(&a), Some(&held))?;
    s.ok("expired transaction exchanges nothing", refused(&r, "not-under-way") && s.workos.connection_counts() == counts, &r);

    let (old, _, _) = pending(api, &a, "superseded")?;
    let (new, newest, _) = pending(api, &a, "newest")?;
    let r = back(api, &old, Some(&a), Some(&newest))?;
    s.ok("a later start replaces the browser's binding", refused(&r, "elsewhere") && s.workos.connection_counts() == counts, &r);
    let mut injected = reqwest::Url::parse(&new)?;
    let old_code = reqwest::Url::parse(&old)?.query_pairs().find(|(k, _)| k == "code").context("old code")?.1.into_owned();
    let new_state = injected.query_pairs().find(|(k, _)| k == "state").context("new state")?.1.into_owned();
    injected.set_query(None);
    injected.query_pairs_mut().append_pair("state", &new_state).append_pair("code", &old_code);
    let r = back(api, injected.as_str(), Some(&a), Some(&newest))?;
    let failed_counts = s.workos.connection_counts();
    s.ok("a code from another attempt fails PKCE and imports nothing", refused(&r, "code-refused") && failed_counts == (counts.0 + 1, counts.1, counts.2), &r);
    let r = back(api, &new, Some(&a), Some(&newest))?;
    s.ok("a failed exchange still consumes its transaction", refused(&r, "not-under-way") && s.workos.connection_counts() == failed_counts, &r);

    let (url_b, cookie_b, _) = pending(api, &b, "provider-account-b")?;
    s.stop()?;
    let api = s.start(false)?;
    let r = back(&api, &url_b, Some(&b), Some(&cookie_b))?;
    s.ok("pending binding survives restart and imports only for the second user", r.status == 200 && s.workos.imported_accounts().last() == Some(&(s.workos.user("connection-b@e2e.test").id, PROVIDER.into(), "provider-account-b".into())), &r);
    let counts = s.workos.connection_counts();
    let r = back(&api, &url, Some(&a), Some(&cookie))?;
    s.ok("a consumed transaction stays consumed after restart", refused(&r, "not-under-way") && s.workos.connection_counts() == counts, &r);

    let (replace, cookie, _) = pending(&api, &a, "replacement-a")?;
    let r = back(&api, &replace, Some(&a), Some(&cookie))?;
    s.ok("reauthorization uses documented update after an explicit import conflict", r.status == 200 && s.workos.imported_accounts().last() == Some(&(user_a, PROVIDER.into(), "replacement-a".into())), &r);

    let (concurrent, cookie, _) = pending(&api, &b, "concurrent-b")?;
    let counts = s.workos.connection_counts();
    let (one, two) = std::thread::scope(|scope| {
        let one = scope.spawn(|| back(&api, &concurrent, Some(&b), Some(&cookie)));
        let two = back(&api, &concurrent, Some(&b), Some(&cookie));
        (one.join().expect("callback thread"), two)
    });
    let (one, two) = (one?, two?);
    s.ok("concurrent callbacks exchange and import once", ((one.status == 200 && refused(&two, "not-under-way")) || (two.status == 200 && refused(&one, "not-under-way")))
        && s.workos.connection_counts() == (counts.0 + 1, counts.1 + 1, counts.2), format!("{} / {}", one.status, two.status));

    let (declined, cookie, _) = pending(&api, &b, "declined")?;
    let mut declined = reqwest::Url::parse(&declined)?;
    declined.query_pairs_mut().append_pair("error", "access_denied");
    let counts = s.workos.connection_counts();
    let r = back(&api, declined.as_str(), Some(&b), Some(&cookie))?;
    s.ok("a provider error consumes the attempt without exchanging even if a code is present", refused(&r, "declined") && s.workos.connection_counts() == counts, &r);

    let ended = api.sign_in("connection-ended@e2e.test")?;
    let (ended_url, cookie, _) = pending(&api, &ended, "ended")?;
    let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "expireSession": ended } })))?;
    anyhow::ensure!(hook.status == 200, "expire local session: {hook}");
    let r = back(&api, &ended_url, Some(&ended), Some(&cookie))?;
    s.ok("an ended platform session exchanges and imports nothing", refused(&r, "signed-out") && s.workos.connection_counts() == counts, &r);

    let (failing, cookie, _) = pending(&api, &b, "failed-import")?;
    s.workos.fail_next_import();
    let attempts = s.workos.import_attempts();
    let r = back(&api, &failing, Some(&b), Some(&cookie))?;
    let replay = back(&api, &failing, Some(&b), Some(&cookie))?;
    s.ok("failed import is not retried and callback replay cannot import", refused(&r, "no-key") && refused(&replay, "not-under-way")
        && s.workos.import_attempts() == attempts + 1 && s.workos.connection_counts() == (counts.0 + 1, counts.1, counts.2), format!("{} / {}", r.status, replay.status));

    let capped = api.sign_in("connection-cap@e2e.test")?;
    let counts = s.workos.connection_counts();
    let statuses: Vec<u16> = (0..=fragment_core::connection_signin::PENDING_PER_SESSION_MAX).map(|_| begin(&api, &capped, PROVIDER).map(|r| r.status)).collect::<Result<_>>()?;
    s.ok("pending attempts are bounded without exchanging or importing", statuses[..8] == [200; 8] && statuses[8] == 429 && s.workos.connection_counts() == counts, format!("{statuses:?}"));
    Ok(())
}
