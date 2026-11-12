//! Sign-in by an OpenID Connect provider (docs/self-host.md, seam 4): the
//! router's half, in WorkOS's place where the deployment names a provider
//! (`Config::oidc`), never beside it as a fallback. The rules are
//! `fragment_core::oidc`'s; the id_token's signature is the runtime's
//! WebCrypto's to check (`js::signature_verifies`); keys.rs derives the
//! sign-in's PKCE verifier and nonce from the host secret and exchanges the
//! code with the client's secret. The registry then spends the sign-in and
//! signs the person in as WorkOS's exchange does (`calls::OidcSignIn`).
//!
//! What it shares with WorkOS's sign-in, in auth.rs: the round trip bound
//! to the browser by its cookie (`trip_begun`, `trip_bound`, `Trip::SignIn`
//! and `Trip::Switch`), the registry's pending sign-ins (`calls::Begin`),
//! the person and the session it sets, the CLI's approval, invites met,
//! logout (here alone: tsidp, the tailnet's default, offers no
//! provider logout), and the pages a refusal is said on.
//!
//! The metadata and the JWKS are caches, so their contract:
//! - Source: the provider's `<issuer>/.well-known/openid-configuration` and
//!   the `jwks_uri` it names, over the runtime's TLS (a provider whose
//!   certificate is from a private CA needs the runtime to trust that CA).
//! - Invalidation: each is fetched again once `oidc::METADATA_TTL_MS` old;
//!   the JWKS also when an id_token names a key it lacks (a rotation): at
//!   once, then not again for such a key within `JWKS_REFETCH_MIN_MS`. A
//!   new isolate starts with neither.
//! - Stale reads: a key the provider withdrew is trusted until the hour
//!   ends. Only the provider's own token endpoint hands the cell an id_token
//!   (over TLS, for a code and a verifier only this sign-in can make), so a
//!   withdrawn key would also need that endpoint to sign with it.

use std::cell::RefCell;
use std::rc::Rc;

use fragment_core::oidc::{self, Alg, Authorize, Expect, IdTokenError, Jwks, OidcError, Provider, Settings, Verified};
use fragment_core::roundtrip::Trip;
use fragment_core::site;
use fragment_proto::ErrorCode;
use worker::{console_error, Env, Headers, Method, Request, RequestInit, Response, Url};

use crate::ask_registry;
use crate::auth::{back_to, esc, leave_form, page, query, redirect, secure, set_cookie, trip_begun, trip_bound, trip_spent, LOGIN_HINT_MAX, SESSION_COOKIE};
use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls;
use crate::{js, keys};

/// A fetch of the provider's metadata or keys, answered whole within this.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

thread_local! {
    /// The provider's metadata, and when it was fetched (ms).
    static PROVIDER: RefCell<Option<(Rc<Provider>, i64)>> = const { RefCell::new(None) };
    /// Its signing keys, and when they were fetched (ms).
    static KEYS: RefCell<Option<(Rc<Jwks>, i64)>> = const { RefCell::new(None) };
    /// When they were last fetched again for a key they lacked (ms).
    static REFETCHED: RefCell<Option<i64>> = const { RefCell::new(None) };
}

/// A refusal, said in the node's log for whoever runs the deployment (a
/// page says it to the person): `signin.refused`, the provider, and why,
/// one JSON line (each string escaped as JSON).
fn refused(o: &Settings, why: &str) {
    let text = |s: &str| serde_json::to_string(s).unwrap_or_default();
    console_error!("{{\"event\":\"signin.refused\",\"issuer\":{},\"why\":{}}}", text(&o.issuer), text(why));
}

/// Where the provider sends the browser back: the platform's callback,
/// the redirect URI its client is registered with.
fn callback_url(cfg: &Config) -> String {
    format!("{}/auth/callback", cfg.platform())
}

/// A GET of the provider's JSON (its metadata, its keys): the body, at
/// most `oidc::METADATA_MAX_BYTES`, read whole within `FETCH_TIMEOUT`, no
/// redirect followed.
async fn fetch_json(o: &Settings, url: &str, what: &str) -> CellResult<Vec<u8>> {
    let headers = Headers::new();
    headers.set("accept", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers).with_redirect(worker::RequestRedirect::Manual);
    let a = crate::cs::fetch_all(Request::new_with_init(url, &init)?, FETCH_TIMEOUT, oidc::METADATA_MAX_BYTES).await.map_err(|e| {
        refused(o, &format!("its {what} did not answer ({url}): {}", e.message));
        CellError::new(ErrorCode::UpstreamFailed, format!("the sign-in provider's {what} did not answer: {}", e.message))
    })?;
    if a.status != 200 || a.cut {
        return Err(misbehaved(o, OidcError::Provider(format!("its {what} answered {} ({url}), or more than {} bytes", a.status, oidc::METADATA_MAX_BYTES))));
    }
    Ok(a.body)
}

fn misbehaved(o: &Settings, e: OidcError) -> CellError {
    refused(o, &e.to_string());
    CellError::new(ErrorCode::UpstreamFailed, e.to_string())
}

/// The provider's metadata: the isolate's copy while it is fresh, else
/// fetched (`Provider::parse` checks it).
async fn provider(o: &Settings) -> CellResult<Rc<Provider>> {
    let now = js::now_ms();
    let kept = PROVIDER.with(|c| c.borrow().as_ref().filter(|(_, at)| !oidc::stale(*at, now)).map(|(p, _)| Rc::clone(p)));
    if let Some(p) = kept {
        return Ok(p);
    }
    let bytes = fetch_json(o, &oidc::discovery_url(&o.issuer), "metadata").await?;
    let p = Rc::new(Provider::parse(&o.issuer, &bytes).map_err(|e| misbehaved(o, e))?);
    PROVIDER.with(|c| *c.borrow_mut() = Some((Rc::clone(&p), now)));
    Ok(p)
}

/// The provider's keys, fetched now.
async fn fetch_keys(o: &Settings, provider: &Provider, now: i64) -> CellResult<Rc<Jwks>> {
    let bytes = fetch_json(o, &provider.jwks_uri, "JWKS").await?;
    let keys = Rc::new(Jwks::parse(&bytes).map_err(|e| misbehaved(o, e))?);
    KEYS.with(|c| *c.borrow_mut() = Some((Rc::clone(&keys), now)));
    Ok(keys)
}

/// An id_token checked against `keys`: its header and key by the rules,
/// its signature by the runtime, then its claims.
async fn checked(token: &str, keys: &Jwks, algs: &[Alg], expect: &Expect<'_>) -> Result<Verified, IdTokenError> {
    let signed = oidc::signed(token, keys, algs)?;
    let check = js::signature_verifies(&signed).await;
    signed.verified(check, expect)
}

/// An id_token from the provider's token endpoint, verified for this
/// sign-in (its `nonce`). A key the isolate's JWKS lacks fetches the JWKS
/// again, once (the provider rotated), unless such a fetch was within
/// `JWKS_REFETCH_MIN_MS`; nothing else is tried again. A refusal is 401, logged
/// with why: an id_token that fails is a provider misconfigured (another
/// client's audience, a skewed clock) or one not to trust.
async fn verify(o: &Settings, provider: &Provider, token: &str, nonce: &str) -> CellResult<Verified> {
    let now = js::now_ms();
    let expect = Expect { settings: o, nonce, now_s: now.div_euclid(1000) };
    let kept = KEYS.with(|c| c.borrow().as_ref().filter(|(_, at)| !oidc::stale(*at, now)).map(|(k, at)| (Rc::clone(k), *at)));
    let (keys, fresh) = match kept {
        Some((k, _)) => (k, false),
        None => (fetch_keys(o, provider, now).await?, true),
    };
    let verified = match checked(token, &keys, &provider.algs, &expect).await {
        Err(IdTokenError::UnknownKey(_)) if !fresh && oidc::may_refetch(REFETCHED.with(|r| *r.borrow()), now) => {
            REFETCHED.with(|r| *r.borrow_mut() = Some(now));
            checked(token, &*fetch_keys(o, provider, now).await?, &provider.algs, &expect).await
        }
        verified => verified,
    };
    verified.map_err(|e| {
        refused(o, &e.to_string());
        CellError::new(ErrorCode::Unauthenticated, format!("the sign-in provider's answer was not one to trust: {e}"))
    })
}

/// A sign-in starts at the provider, its state bound to this browser as
/// `trip`'s: a sign-in's, or a switch's, which asks the provider to ask who
/// is signing in even holding a session (`prompt=login`). The provider's
/// metadata first: a provider that cannot answer leaves no pending sign-in.
pub async fn begin(env: &Env, cfg: &Config, o: &Settings, url: &Url, trip: Trip) -> CellResult<Response> {
    let provider = provider(o).await?;
    let return_to = site::return_path(query(url, "return").as_deref());
    let began = ask_registry(env, &calls::Begin { return_to }).await?;
    let secrets = keys::oidc_secrets(env, &began.state).await?;
    let hint = query(url, "login_hint").filter(|h| !h.is_empty() && h.len() <= LOGIN_HINT_MAX);
    let to = oidc::authorize_url(&Authorize {
        endpoint: &provider.authorization_endpoint,
        settings: o,
        redirect_uri: &callback_url(cfg),
        state: &began.state,
        secrets: &secrets,
        login_hint: hint.as_deref(),
        ask_again: trip == Trip::Switch,
    });
    let cookies = [trip_begun(trip, &began.state, url)];
    match trip {
        Trip::Switch => leave_form(&to, "Switching accounts", &cookies),
        _ => redirect(&to, &cookies),
    }
}

/// A refusal of who signs in, said on a page with a way to start again.
fn refusal_page(e: &CellError) -> CellResult<Response> {
    page(e.code.status(), "Sign-in did not finish", &format!("<p>{}</p><p><a href=\"/auth/login\">Try again</a></p>", esc(&e.message)))
}

/// The provider sends the browser back: its code exchanged with this
/// sign-in's verifier and the client's secret, its id_token verified, the
/// email it gives checked as the person's handle, and the registry asked
/// to spend the sign-in and set a session, back where the sign-in began. A
/// refusal for who it is (an id_token not to trust, an email the provider
/// has not verified, or another person's) is said on a page.
pub async fn callback(req: &Request, env: &Env, cfg: &Config, o: &Settings, url: &Url) -> CellResult<Response> {
    if let Some(error) = query(url, "error") {
        let why = query(url, "error_description").unwrap_or_default();
        refused(o, &format!("the provider answered the authorization {error}: {why}"));
        return page(400, "Sign-in did not finish", &format!("<p>The sign-in provider said <code>{}</code>: {}</p><p><a href=\"/auth/login\">Try again</a></p>", esc(&error), esc(&why)));
    }
    let Some(bound) = trip_bound(req, url, &[Trip::SignIn, Trip::Switch])? else {
        return page(400, "Sign-in did not start here", "<p>This sign-in began in another browser, or too long ago.</p><form method=\"get\" action=\"/auth/login\"><button type=\"submit\">Start again</button></form>");
    };
    let spent = trip_spent(bound.trip(), url);
    let code = query(url, "code").filter(|c| !c.is_empty() && c.len() <= oidc::CODE_MAX);
    let code = code.ok_or_else(|| CellError::invalid(format!("the callback carries no code, or one over {} bytes", oidc::CODE_MAX)))?;
    let secrets = keys::oidc_secrets(env, bound.state()).await?;
    let provider = provider(o).await?;
    let (status, answer) = keys::oidc_token(env, &provider, o, &code, &callback_url(cfg), &secrets.verifier).await?;
    let id_token = oidc::id_token_of(status, &answer).map_err(|e| {
        refused(o, &e.to_string());
        match e {
            // a 400 is the code's: used (a callback sent again), expired,
            // or not this sign-in's (its verifier is another's). The browser
            // starts again; the provider answered, so this is no outage.
            OidcError::Refused { status: 400, .. } => CellError::invalid(format!("{e}: start again")),
            e => CellError::new(ErrorCode::UpstreamFailed, e.to_string()),
        }
    })?;
    let verified = match verify(o, &provider, &id_token, &secrets.nonce).await {
        Ok(v) => v,
        Err(e) if e.code == ErrorCode::Unauthenticated => return refusal_page(&e),
        Err(e) => return Err(e),
    };
    let email = match oidc::email_of(&verified, o) {
        Ok(email) => email.to_string(),
        Err(e) => {
            refused(o, &e.to_string());
            let said = match e {
                oidc::EmailRefused::Absent { .. } => "The sign-in provider gave no email for you: ask whoever runs it to share your email with this app.".to_string(),
                oidc::EmailRefused::Unverified { email, .. } => format!("The sign-in provider has not verified the email {email:?}: verify it there, then sign in again."),
            };
            return refusal_page(&CellError::new(ErrorCode::Forbidden, said));
        }
    };
    let done = match ask_registry(env, &calls::OidcSignIn::of(bound, &o.issuer, verified, &email)).await {
        Ok(done) => done,
        // who it is (an email that is no address, or another person's; a
        // person being wiped): said on a page, for a browser
        Err(e) if matches!(e.code, ErrorCode::Forbidden | ErrorCode::AlreadyExists) => {
            refused(o, &e.message);
            return refusal_page(&e);
        }
        Err(e) => return Err(e),
    };
    match done {
        calls::Exchanged::SignedIn { token, return_to } => redirect(
            &back_to(&format!("{}/", cfg.platform()), Some(&return_to))?,
            &[set_cookie(SESSION_COOKIE, &token, "/", crate::registry::SESSION_TTL_MS / 1000, secure(url)), spent],
        ),
        calls::Exchanged::Held { .. } => Err(CellError::host("a provider's sign-in answered as WorkOS's held session")),
    }
}
