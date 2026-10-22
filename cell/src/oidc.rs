//! Sign-in on OpenID Connect (docs/self-host.md, seam 4): the router's
//! half, beside WorkOS's in auth.rs and chosen by configuration
//! (`Config::oidc`), and the provider's metadata and keys, fetched and kept
//! per isolate, with an id_token verified by them. The rules are
//! `fragment_core::oidc`'s; the registry exchanges the code with the
//! client's secret from keys.rs (`registry/oidc_signin.rs`).
//!
//! What it shares with WorkOS's sign-in, in auth.rs: the round trip bound
//! to the browser by its cookie (`trip_begun`, `trip_bound`, `Trip::SignIn`
//! and `Trip::Switch`), the platform session it sets, and the pages it
//! says a refusal on.
//!
//! The metadata and the JWKS are caches, so their contract:
//! - Source: the provider's `<issuer>/.well-known/openid-configuration`
//!   and the `jwks_uri` it names, over the runtime's TLS (a provider whose
//!   certificate is from a private CA needs the runtime to trust that CA:
//!   docs/self-host.md, seam 4).
//! - Invalidation: each is fetched again once `METADATA_TTL_MS` old; the
//!   JWKS also when an id_token names a key it lacks (a rotation), at most
//!   once a `JWKS_REFETCH_MIN_MS`. A new isolate starts with neither.
//! - Stale reads: a key the provider withdrew is trusted until the hour
//!   ends. Only the provider's own token endpoint hands the cell an
//!   id_token (over TLS, for a code and verifier only this sign-in holds),
//!   so a withdrawn key would also need that endpoint to sign with it.

use std::cell::RefCell;
use std::rc::Rc;

use fragment_core::oidc::{self, Expect, IdTokenError, Jwks, Provider, Verified};
use fragment_core::roundtrip::Trip;
use fragment_core::site;
use fragment_proto::ErrorCode;
use worker::{console_error, Env, Headers, Method, Request, RequestInit, Response, Url};

use crate::ask_registry;
use crate::auth::{esc, leave_form, page, query, redirect, set_cookie, trip_begun, trip_bound, trip_spent, SESSION_COOKIE};
use crate::config::{Config, OidcConfig};
use crate::error::{CellError, CellResult};
use crate::js;
use crate::registry::calls;

/// A fetch of the provider's metadata or keys, answered whole within this.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

thread_local! {
    /// The provider's metadata, and when it was fetched (ms).
    static PROVIDER: RefCell<Option<(Rc<Provider>, i64)>> = const { RefCell::new(None) };
    /// Its signing keys, and when they were fetched (ms).
    static KEYS: RefCell<Option<(Rc<Jwks>, i64)>> = const { RefCell::new(None) };
}

/// A refusal, said in the node's log for whoever runs the deployment (a
/// page says it to the person): `signin.refused`, the provider, and why.
pub fn refused(cfg: &OidcConfig, why: &str) {
    console_error!("{}", serde_json::json!({ "event": "signin.refused", "issuer": cfg.issuer, "why": why }));
}

/// Where the provider sends the browser back: the platform's callback,
/// the redirect URI its client is registered with.
pub fn callback_url(cfg: &Config) -> String {
    format!("{}/auth/callback", cfg.platform())
}

/// A GET of the provider's JSON (its metadata, its keys): the body, at
/// most `METADATA_MAX_BYTES`, read whole within `FETCH_TIMEOUT`.
async fn get(url: &str, what: &str) -> CellResult<Vec<u8>> {
    let headers = Headers::new();
    headers.set("accept", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let req = Request::new_with_init(url, &init)?;
    let a = crate::cs::fetch_all(req, FETCH_TIMEOUT, oidc::METADATA_MAX_BYTES)
        .await
        .map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the sign-in provider's {what} did not answer ({url}): {}", e.message)))?;
    if a.status != 200 || a.cut {
        return Err(CellError::new(ErrorCode::UpstreamFailed, format!("the sign-in provider's {what} answered {} ({url}), or more than {} bytes", a.status, oidc::METADATA_MAX_BYTES)));
    }
    Ok(a.body)
}

fn misbehaved(cfg: &OidcConfig, e: oidc::OidcError) -> CellError {
    refused(cfg, &e.to_string());
    CellError::new(ErrorCode::UpstreamFailed, e.to_string())
}

/// The provider's metadata: the isolate's copy while it is fresh, else
/// fetched (`Provider::parse` checks it).
pub async fn provider(cfg: &OidcConfig) -> CellResult<Rc<Provider>> {
    let now = js::now_ms();
    let kept = PROVIDER.with(|c| c.borrow().as_ref().filter(|(_, at)| !oidc::stale(*at, now)).map(|(p, _)| Rc::clone(p)));
    if let Some(p) = kept {
        return Ok(p);
    }
    let bytes = get(&oidc::discovery_url(&cfg.issuer), "metadata").await?;
    let p = Rc::new(Provider::parse(&cfg.issuer, &bytes).map_err(|e| misbehaved(cfg, e))?);
    PROVIDER.with(|c| *c.borrow_mut() = Some((Rc::clone(&p), now)));
    Ok(p)
}

/// The provider's keys, fetched now.
async fn fetch_keys(cfg: &OidcConfig, provider: &Provider, now: i64) -> CellResult<Rc<Jwks>> {
    let bytes = get(&provider.jwks_uri, "JWKS").await?;
    let keys = Rc::new(Jwks::parse(&bytes).map_err(|e| misbehaved(cfg, e))?);
    KEYS.with(|c| *c.borrow_mut() = Some((Rc::clone(&keys), now)));
    Ok(keys)
}

/// An id_token from the provider's token endpoint, verified for this
/// sign-in (its `nonce`). A key the isolate's JWKS lacks fetches the JWKS
/// again, once, unless the last fetch was within `JWKS_REFETCH_MIN_MS` (the
/// provider rotated); nothing else is tried again. A refusal is 401,
/// logged with why: an id_token that fails is a provider misconfigured
/// (another client's audience, a skewed clock) or one not to trust.
pub async fn verify(cfg: &OidcConfig, provider: &Provider, token: &str, nonce: &str) -> CellResult<Verified> {
    let now = js::now_ms();
    let expect = Expect { issuer: &cfg.issuer, client_id: &cfg.client_id, nonce, now_s: now.div_euclid(1000), algs: &provider.algs };
    let kept = KEYS.with(|c| c.borrow().as_ref().filter(|(_, at)| !oidc::stale(*at, now)).map(|(k, at)| (Rc::clone(k), *at)));
    let (keys, fetched_at) = match kept {
        Some(k) => k,
        None => (fetch_keys(cfg, provider, now).await?, now),
    };
    let verified = match oidc::verify(token, &keys, &expect) {
        Err(IdTokenError::UnknownKey(_)) if oidc::may_refetch(fetched_at, now) => oidc::verify(token, &*fetch_keys(cfg, provider, now).await?, &expect),
        verified => verified,
    };
    verified.map_err(|e| {
        refused(cfg, &e.to_string());
        CellError::new(ErrorCode::Unauthenticated, format!("sign-in refused the provider's id_token: {e}"))
    })
}

/// A sign-in starts at the provider, its state bound to this browser as
/// `trip`'s: a sign-in's, or a switch's, which asks the provider to ask who
/// is signing in even holding a session (`prompt=login`). The provider's
/// metadata first: a provider that cannot answer leaves no pending sign-in.
pub async fn begin(env: &Env, cfg: &Config, o: &OidcConfig, url: &Url, trip: Trip) -> CellResult<Response> {
    let provider = provider(o).await?;
    let return_to = site::return_path(query(url, "return").as_deref());
    let began = ask_registry(env, &calls::Begin { return_to, oidc: true }).await?;
    let pkce = began.oidc.ok_or_else(|| CellError::host("an OpenID Connect sign-in began without its challenge"))?;
    let hint = query(url, "login_hint").filter(|h| !h.is_empty() && h.len() <= crate::auth::LOGIN_HINT_MAX);
    let to = oidc::authorize_url(&oidc::Authorize {
        provider: &provider,
        client_id: &o.client_id,
        redirect_uri: &callback_url(cfg),
        scopes: &o.scopes,
        state: &began.state,
        nonce: &pkce.nonce,
        challenge: &pkce.challenge,
        login_hint: hint.as_deref(),
        ask_again: trip == Trip::Switch,
    });
    let cookies = [trip_begun(trip, &began.state, url)];
    match trip {
        Trip::Switch => leave_form(&to, "Switching accounts", &cookies),
        _ => redirect(&to, &cookies),
    }
}

/// The provider sends the browser back: its code exchanged in the registry
/// (with the sign-in's verifier and the client's secret), its id_token
/// verified, and a session set, back where the sign-in began. A refusal for
/// who it is (an email the provider has not verified, or another person's;
/// an id_token not to trust) is said on a page.
pub async fn callback(req: &Request, env: &Env, cfg: &Config, o: &OidcConfig, url: &Url) -> CellResult<Response> {
    if let Some(error) = query(url, "error") {
        let why = query(url, "error_description").unwrap_or_default();
        refused(o, &format!("the provider answered the authorization {error}: {why}"));
        return page(400, "Sign-in did not finish", &format!("<p>The sign-in provider said <code>{}</code>: {}</p><p><a href=\"/auth/login\">Try again</a></p>", esc(&error), esc(&why)));
    }
    let Some(bound) = trip_bound(req, url, &[Trip::SignIn, Trip::Switch])? else {
        return page(400, "Sign-in did not start here", "<p>This sign-in began in another browser, or too long ago.</p><p><a href=\"/auth/login\">Start again</a></p>");
    };
    let spent = trip_spent(bound.trip(), url);
    let code = query(url, "code").ok_or_else(|| CellError::invalid("the callback carries no code"))?;
    let done = match ask_registry(env, &calls::OidcExchange::of(bound, code)).await {
        Ok(done) => done,
        Err(e) if matches!(e.code, ErrorCode::Forbidden | ErrorCode::AlreadyExists | ErrorCode::Unauthenticated) => {
            return page(e.code.status(), "Sign-in did not finish", &format!("<p>{}</p><p><a href=\"/auth/login\">Try again</a></p>", esc(&e.message)));
        }
        Err(e) => return Err(e),
    };
    match done {
        calls::Exchanged::SignedIn { token, return_to } => redirect(
            &crate::auth::back_to(&format!("{}/", cfg.platform()), Some(&return_to))?,
            &[set_cookie(SESSION_COOKIE, &token, "/", crate::registry::SESSION_TTL_MS / 1000, crate::auth::secure(url)), spent],
        ),
        calls::Exchanged::Held { .. } => Err(CellError::host("an OpenID Connect sign-in answered as WorkOS's held session")),
    }
}

/// After a platform session ended (`/auth/logout`): RP-initiated logout
/// (RP-Initiated Logout 1.0) where the provider advertises an
/// `end_session_endpoint`, with the session's id_token as the hint (Okta
/// and ADFS require one), the client, and the way back to the platform's
/// root (which the client registers as a post-logout redirect URI); left
/// as a page, as WorkOS's is (a form's CSP holds its redirects). Signed out
/// here alone where the provider offers none (Dex) or cannot be asked.
pub async fn logout(cfg: &Config, o: &OidcConfig, id_token: Option<&str>, clear: String) -> CellResult<Response> {
    match provider(o).await.map(|p| p.end_session_endpoint.clone()) {
        Ok(Some(end)) => leave_form(&oidc::end_session_url(&end, id_token, &o.client_id, &format!("{}/", cfg.platform())), "Signing out", &[clear]),
        Ok(None) => redirect("/", &[clear]),
        Err(e) => {
            refused(o, &format!("logout: the provider's metadata: {}", e.message));
            redirect("/", &[clear])
        }
    }
}
