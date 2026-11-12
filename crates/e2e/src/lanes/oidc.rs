//! Sign-in by an OpenID Connect provider (docs/self-host.md, seam 4),
//! through the provider fake on a node configured with it in WorkOS's
//! place: first a strict provider (`email_verified` asked for), then one
//! shaped as tsidp, the tailnet's default (no `email_verified`: the
//! deployment vouches for its `email`). The round trip binds the browser
//! as WorkOS's does; the code is exchanged with PKCE (S256) and a nonce
//! derived from the host secret and the state; the id_token's signature
//! is checked by the runtime's WebCrypto (RS256 and ES256), then its
//! claims. Every way an id_token goes wrong signs no one in, on a page,
//! and says why in the node's log (`signin.refused`); a sign-in finishes
//! once, after a restart too; the person is master's npub, their email
//! the handle; the CLI's `/cli` approval, invites and logout work as with
//! WorkOS. The node is plain again at the section's end.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use fragment_fakes::oidc::{Alg, Spoil};
use fragment_nip98::Keys;
use fragment_proto::ErrorCode;
use serde_json::json;

use crate::api::{url_enc, Api, Call, Reply};
use crate::{Need, Suite, OIDC_CLIENT};

/// A sign-in begun at `/auth/login`: where it sends the browser, its query,
/// and the cookie that binds its state.
struct Begun {
    to: String,
    query: HashMap<String, String>,
    cookie: String,
    reply: Reply,
}

fn begin(api: &Api, path: &str) -> Result<Begun> {
    let reply = api.unsigned("GET", path, None)?;
    anyhow::ensure!(reply.status == 302, "GET {path}: {reply}");
    let to = reply.header("location");
    let query = reqwest::Url::parse(&to).with_context(|| format!("{path} sent the browser to {to:?}"))?.query_pairs().into_owned().collect();
    let cookie = reply.cookies().into_iter().find(|c| c.starts_with("fragment_login=")).context("a sign-in's cookie")?;
    Ok(Begun { to, query, cookie, reply })
}

/// The provider's answer for `hint` (a person picked as its form would):
/// its redirect back to the platform's callback.
fn at_provider(api: &Api, b: &Begun, hint: &str) -> Result<String> {
    let back = api.external(&format!("{}&login_hint={}", b.to, url_enc(hint)))?;
    anyhow::ensure!(back.status == 302, "the provider's authorize: {back}");
    Ok(back.header("location"))
}

fn callback(api: &Api, back: &str, cookie: Option<&str>) -> Result<Reply> {
    api.call(Call { method: "GET", url: back.to_string(), cookie: cookie.map(str::to_string), ..Call::default() })
}

/// A whole sign-in as `hint`: the callback's answer.
fn sign_in_as(api: &Api, hint: &str) -> Result<Reply> {
    let b = begin(api, "/auth/login?return=/")?;
    let back = at_provider(api, &b, hint)?;
    callback(api, &back, Some(&b.cookie))
}

fn session_of(r: &Reply) -> Option<String> {
    r.cookies().into_iter().find_map(|c| c.strip_prefix("fragment_session=").map(|v| v.split(';').next().unwrap_or("").to_string())).filter(|v| !v.is_empty())
}

/// A sign-in as `hint` that must set a session: its token.
fn signed_in(api: &Api, hint: &str) -> Result<String> {
    let r = sign_in_as(api, hint)?;
    session_of(&r).with_context(|| format!("signing in as {hint}: {r}"))
}

/// A page refusing who signs in: its status, HTML that says `says`, and no session.
fn refused_page(r: &Reply, status: u16, says: &str) -> bool {
    r.status == status && r.header("content-type").starts_with("text/html") && r.text.contains(says) && session_of(r).is_none()
}

/// How many refusals the node's log says (`signin.refused`).
fn refusals(s: &Suite) -> usize {
    s.node_log().unwrap_or_default().matches("\"event\":\"signin.refused\"").count()
}

/// Whether the node's log says one refusal more than `before`: it writes
/// its lines as they come, a moment after the answer (wrangler's first
/// error line takes longest), so waited for, then counted.
fn refused_once(s: &Suite, before: usize) -> bool {
    s.eventually(Duration::from_secs(10), || refusals(s) > before) && refusals(s) == before + 1
}

pub fn oidc(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("oidc", &[Need::Fakes, Need::Node, Need::Deployment, Need::Levers]) {
        return Ok(());
    }
    s.stop()?;
    let api = s.start_signing_in_by_oidc(false)?;
    let strict = strict_cases(s, &api);
    s.stop()?;
    let api = s.start_signing_in_by_oidc(true)?;
    let tsidp = tsidp_cases(s, &api);
    s.stop()?;
    s.start(false)?;
    strict?;
    tsidp
}

fn strict_cases(s: &mut Suite, api: &Api) -> Result<()> {
    let issuer = s.oidc.url.clone();

    // the round trip
    let b = begin(api, "/auth/login?return=/x&screen_hint=sign-up&invitation_token=abc")?;
    let q = &b.query;
    let want = [("response_type", "code"), ("client_id", OIDC_CLIENT), ("scope", "openid email profile"), ("code_challenge_method", "S256")];
    s.ok(
        "sign-in starts at the provider, not WorkOS: its authorize endpoint, this client, the platform's callback, the code flow, openid, a state, a nonce and an S256 challenge",
        b.to.starts_with(&format!("{issuer}/authorize?"))
            && want.iter().all(|(k, v)| q.get(*k).map(String::as_str) == Some(*v))
            && q.get("redirect_uri") == Some(&format!("{}/auth/callback", api.base))
            && q.get("state").is_some_and(|st| fragment_core::roundtrip::valid_state(st))
            && q.get("nonce").is_some_and(|n| n.len() == 43)
            && q.get("code_challenge").is_some_and(|c| fragment_core::own_signin::valid_token(c)),
        &b.to,
    );
    s.ok(
        "and nothing of WorkOS's rides along (its screen, its invitation), nor any verifier",
        !b.to.contains("screen_hint") && !b.to.contains("invitation_token") && !b.to.contains("code_verifier"),
        &b.to,
    );
    let line = b.reply.headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find(|c| c.starts_with("fragment_login=")).unwrap_or("").to_string();
    s.ok("the state is bound to this browser by its cookie (HttpOnly, SameSite=Lax), as WorkOS's", line.contains("HttpOnly") && line.contains("SameSite=Lax"), &line);
    let back = at_provider(api, &b, "Jane.Doe@Corp.Example")?;
    let r = callback(api, &back, None)?;
    s.ok("a callback without the browser's cookie signs no one in (400)", r.status == 400 && session_of(&r).is_none(), &r);
    let issued = s.oidc.issued();
    let r = callback(api, &back, Some(&b.cookie))?;
    let jane = session_of(&r).unwrap_or_default();
    s.ok(
        "the callback exchanges the code (the client's secret, Basic) and sets a session, back where it began",
        r.status == 302 && r.header("location").ends_with("/x") && !jane.is_empty() && s.oidc.issued() == issued + 1 && s.oidc.token_auth().last().map(String::as_str) == Some("client_secret_basic"),
        &r,
    );
    let me = api.who(&jane)?;
    let jane_id = me["id"].as_str().unwrap_or_default().to_string();
    s.ok(
        "a new person: master's npub, their email the provider's, kept lower case",
        me["kind"] == "person" && fragment_core::npub::is_identity(&jane_id) && me["email"] == "jane.doe@corp.example",
        &me,
    );
    let r = callback(api, &back, Some(&b.cookie))?;
    s.ok("the same callback again signs no one in (400: the provider spent the code)", r.status == 400 && r.code() == Some(ErrorCode::InvalidRequest) && session_of(&r).is_none(), &r);
    let again = signed_in(api, "jane.doe@corp.example")?;
    s.ok("signing in again is the same person", api.who(&again)?["id"] == jane_id.as_str(), &again);

    // the CLI's key joins them as with WorkOS: a code typed at /cli
    let keys = Keys::generate();
    let r = api.approve(&jane, &keys)?;
    s.ok("a CLI key joins them by the code they type at /cli", r.status == 200 && r.body["id"] == jane_id.as_str(), &r);
    let me = api.signed(&keys, "GET", "/api/identities/me", None)?;
    s.ok(
        "and their sign-in is the provider's: its issuer URL and their email",
        me.body["subjects"].as_array().is_some_and(|a| a.len() == 1 && a[0]["issuer"] == issuer.as_str() && a[0]["email"] == "jane.doe@corp.example"),
        &me,
    );

    // invites by email are met at a first sign-in
    let owner = Keys::generate();
    let (session, _) = api.e2e_session(&Api::email_of(&owner))?;
    api.approve(&session, &owner)?;
    let name = s.named(api, &owner, "oidc-invite")?;
    let r = api.create(&owner, &name)?;
    anyhow::ensure!(r.status == 200, "create {name}: {r}");
    let r = api.signed(&owner, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": "newcomer@corp.example", "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200, "an invite: {r}");
    let newcomer = signed_in(api, "newcomer@corp.example")?;
    let theirs = Keys::generate();
    api.approve(&newcomer, &theirs)?;
    let r = api.signed(&theirs, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    s.ok("an invite to an email waits for its person's first sign-in through the provider, and their yes makes them a member", r.status == 200 && r.body["role"] == "viewer", &r);

    // the provider's keys rotate: picked up, the JWKS fetched again
    let fetched = s.oidc.jwks_fetches();
    s.oidc.rotate_keys();
    let r = sign_in_as(api, "jane.doe@corp.example")?;
    s.ok("the provider's keys rotated: a sign-in signed by the new key is taken, the JWKS fetched again for it at once", r.status == 302 && session_of(&r).is_some() && s.oidc.jwks_fetches() > fetched, format!("{r} (fetches {fetched} → {})", s.oidc.jwks_fetches()));

    // every way an id_token goes wrong signs no one in, says so on a page, and logs why
    let mut wrong = vec![];
    let cases: [(Spoil, Alg, &str); 11] = [
        (Spoil::BadSignature, Alg::Rs256, "signature does not verify"),
        (Spoil::BadSignature, Alg::Es256, "signature does not verify"),
        (Spoil::ForeignKey, Alg::Rs256, "signature does not verify"),
        (Spoil::ForeignKey, Alg::Es256, "signature does not verify"),
        (Spoil::WrongIssuer, Alg::Rs256, "another issuer"),
        (Spoil::WrongAudience, Alg::Rs256, "another client"),
        (Spoil::Expired, Alg::Rs256, "expired"),
        (Spoil::WrongNonce, Alg::Rs256, "another sign-in"),
        (Spoil::AlgNone, Alg::Rs256, "which sign-in does not take"),
        (Spoil::Hs256, Alg::Rs256, "which sign-in does not take"),
        (Spoil::UnpublishedKey, Alg::Rs256, "does not hold"),
    ];
    for (spoil, alg, says) in cases {
        s.oidc.sign_with(alg);
        let before = refusals(s);
        s.oidc.spoil_next(spoil);
        let r = sign_in_as(api, "jane.doe@corp.example")?;
        if !(refused_page(&r, 401, says) && refused_once(s, before)) {
            wrong.push(format!("{spoil:?} {alg:?}: {} {:?} ({} refusals logged)", r.status, r.text.chars().take(300).collect::<String>(), refusals(s) - before));
        }
    }
    s.oidc.sign_with(Alg::Rs256);
    s.ok(
        "a bad signature (RS256, ES256), a forgery under a published key's id (RS256, ES256), another issuer, another client, an expired token, another sign-in's nonce, alg none, HS256 keyed by the client's secret, and an unpublished key: each 401 on a page, no session, and signin.refused in the node's log",
        wrong.is_empty(),
        format!("{wrong:?}"),
    );
    let before = refusals(s);
    signed_in(api, "jane.doe@corp.example")?;
    s.oidc.spoil_next(Spoil::Replay);
    let r = sign_in_as(api, "jane.doe@corp.example")?;
    s.ok(
        "a replay (the id_token of the sign-in before, sent again for this one) signs no one in: its nonce is the other sign-in's",
        refused_page(&r, 401, "another sign-in") && refused_once(s, before),
        &r,
    );

    // the email is the handle: verified, an address, and one person's
    let twin = s.oidc.user("twin@corp.example");
    s.oidc.set_verified(&twin.id, false);
    let r = sign_in_as(api, "twin@corp.example")?;
    s.ok("an email the provider has not verified signs no one in (403, on a page)", refused_page(&r, 403, "has not verified"), &r);
    s.oidc.set_verified(&twin.id, true);
    let r = sign_in_as(api, "jdoe")?;
    s.ok("nor does a sign-in the provider gives no email for (403, on a page)", refused_page(&r, 403, "gave no email"), &r);
    s.oidc.set_email(&twin.id, "jane.doe@corp.example");
    let r = sign_in_as(api, "twin@corp.example")?;
    s.ok("another of the provider's people with a person's email is refused, never merged into them (409, on a page)", refused_page(&r, 409, "another account"), &r);
    s.oidc.set_email(&twin.id, "twin@corp.example");
    let twin_session = signed_in(api, "twin@corp.example")?;
    s.ok("verified and their own, it signs in: another person", api.who(&twin_session)?["id"] != jane_id.as_str(), &twin_session);
    let jane_user = s.oidc.user("jane.doe@corp.example");
    s.oidc.set_email(&jane_user.id, "jane@corp.example");
    let renamed = signed_in(api, "jane.doe@corp.example")?;
    let me = api.who(&renamed)?;
    s.ok("an email changed at the provider keeps the person (the same subject), and the new one shows", me["id"] == jane_id.as_str() && me["email"] == "jane@corp.example", &me);
    s.oidc.set_email(&jane_user.id, "jane.doe@corp.example");

    // a refusal at the provider
    s.oidc.fail_next("access_denied");
    let b = begin(api, "/auth/login")?;
    let back = api.external(&format!("{}&login_hint=jane.doe%40corp.example", b.to))?;
    let r = callback(api, &back.header("location"), Some(&b.cookie))?;
    s.ok("a refusal at the provider is shown, and signs no one in", r.status == 400 && r.text.contains("access_denied") && session_of(&r).is_none(), &r);

    // "Use another account": the provider asked to ask (prompt=login)
    let r = api.call(Call {
        method: "POST",
        url: format!("{}/auth/switch?return=/", api.base),
        cookie: Some(format!("fragment_session={jane}")),
        extra: vec![("origin", api.base.clone())],
        ..Call::default()
    })?;
    let to = r.header("refresh").strip_prefix("0; url=").unwrap_or("").to_string();
    let asked = if to.is_empty() { None } else { Some(api.external(&to)?) };
    s.ok(
        "Use another account ends this session and asks the provider to ask who signs in (prompt=login): its form, not an answer",
        r.status == 200 && to.starts_with(&format!("{issuer}/authorize?")) && to.contains("&prompt=login") && asked.is_some_and(|a| a.status == 200 && a.text.contains("sign in as")) && api.who(&jane)?.is_null(),
        &r,
    );

    // logout: the session ends here (the provider is not asked)
    let out = api.call(Call {
        method: "POST",
        url: format!("{}/auth/logout", api.base),
        cookie: Some(format!("fragment_session={again}")),
        extra: vec![("origin", api.base.clone())],
        ..Call::default()
    })?;
    s.ok("logout ends the session here and comes home", out.status == 302 && out.header("location") == "/" && api.who(&again)?.is_null(), &out);

    // a sign-in begun before a restart finishes after it, once: its state
    // is the registry's, its verifier and nonce derived again
    let b = begin(api, "/auth/login?return=/after")?;
    let back = at_provider(api, &b, "jane.doe@corp.example")?;
    s.stop()?;
    let api = s.start_signing_in_by_oidc(false)?;
    let r = callback(&api, &back, Some(&b.cookie))?;
    let after = session_of(&r).unwrap_or_default();
    let twice = callback(&api, &back, Some(&b.cookie))?;
    s.ok(
        "a sign-in begun before a restart finishes after it (the node's caches empty), once",
        r.status == 302 && r.header("location").ends_with("/after") && api.who(&after)?["id"] == jane_id.as_str() && twice.status == 400 && session_of(&twice).is_none(),
        format!("{r} / {twice}"),
    );
    Ok(())
}

fn tsidp_cases(s: &mut Suite, api: &Api) -> Result<()> {
    let issuer = s.oidc.url.clone();
    let meta = api.external(&format!("{issuer}/.well-known/openid-configuration"))?;
    s.ok(
        "(the fake shaped as tsidp: RS256 alone, and no email_verified among its claims)",
        meta.body["id_token_signing_alg_values_supported"] == json!(["RS256"]),
        &meta,
    );
    // a tailnet person whose login is GitHub's: tsidp's email is made up from it
    let hint = "alice@github.idp.tail-e2e.ts.net";
    let b = begin(api, "/auth/login")?;
    let back = at_provider(api, &b, hint)?;
    let r = callback(api, &back, Some(&b.cookie))?;
    let alice = session_of(&r).unwrap_or_default();
    let me = api.who(&alice)?;
    s.ok(
        "a provider shaped as tsidp signs its tailnet person in, the deployment vouching for its email (FRAGMENT_OIDC_EMAIL_VOUCHED=on)",
        r.status == 302 && me["kind"] == "person" && me["email"] == hint && s.oidc.user(hint).id.starts_with("userid:"),
        format!("{r} / {me}"),
    );
    let keys = Keys::generate();
    let r = api.approve(&alice, &keys)?;
    s.ok("and a CLI key joins them at /cli", r.status == 200 && r.body["id"] == me["id"], &r);
    s.oidc.spoil_next(Spoil::BadSignature);
    let r = sign_in_as(api, hint)?;
    s.ok("vouched for, its id_token is still checked: a bad signature signs no one in", refused_page(&r, 401, "signature does not verify"), &r);
    Ok(())
}
