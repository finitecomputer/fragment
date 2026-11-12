//! Sign-in by an OpenID Connect provider (docs/self-host.md, seam 4): the
//! router's half, in WorkOS's place where the deployment names a provider
//! (`Config::oidc`), never beside it as a fallback. Its JavaScript
//! (cell/oidc.mjs, through `js::oidc_discover` and `js::oidc_redeem`)
//! fetches the provider's metadata and keys (bounded, kept an hour per
//! isolate), sends the token request keys.rs makes (the client's secret is
//! read there), parses the answers, and checks the id_token's signature
//! with WebCrypto; the rules (`fragment_core::oidc`) decide on what it
//! answers. keys.rs derives the sign-in's PKCE verifier and nonce from the
//! host secret.
//! The registry then spends the sign-in and signs the person in as
//! WorkOS's exchange does (`calls::Exchange::verified`).
//!
//! What it shares with WorkOS's sign-in, in auth.rs: the round trip bound
//! to the browser by its cookie (`trip_begun`, `trip_bound`, `Trip::SignIn`
//! and `Trip::Switch`), the registry's pending sign-ins (`calls::Begin`),
//! the person and the session it sets, the CLI's approval, invites met,
//! logout (here alone: tsidp, the tailnet's default, offers no
//! provider logout), and the pages a refusal is said on.

use fragment_core::oidc::{self, Authorize, Expect, OidcError, Provider, Refusal, Settings};
use fragment_core::roundtrip::Trip;
use fragment_core::site;
use fragment_proto::ErrorCode;
use worker::{console_error, Env, Request, Response, Url};

use crate::ask_registry;
use crate::auth::{back_to, esc, leave_form, page, query, redirect, secure, set_cookie, trip_begun, trip_bound, trip_spent, LOGIN_HINT_MAX, SESSION_COOKIE};
use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls;
use crate::{js, keys};

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

fn misbehaved(o: &Settings, e: OidcError) -> CellError {
    refused(o, &e.to_string());
    CellError::new(ErrorCode::UpstreamFailed, e.to_string())
}

/// The provider's metadata, as the shim fetched (or kept) and read it,
/// checked by the rules (`Provider::from_fields`).
async fn provider(o: &Settings) -> CellResult<Provider> {
    let (code, detail, fields) = js::oidc_discover(&oidc::discovery_url(&o.issuer)).await;
    if code != "ok" {
        return Err(misbehaved(o, OidcError::Provider(format!("its metadata: {detail}"))));
    }
    Provider::from_fields(&o.issuer, fields.unwrap_or_default()).map_err(|e| misbehaved(o, e))
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
    let request = keys::oidc_token_request(env, &provider, o, &code, &callback_url(cfg), &secrets.verifier).await?;
    let (answer, detail, values) = js::oidc_redeem(&provider.token_endpoint, &request, &provider.jwks_uri, &oidc::alg_names(&provider.algs), &oidc::claim_names(o)).await;
    let expect = Expect { settings: o, nonce: &secrets.nonce, now_s: js::now_ms().div_euclid(1000) };
    let verified = match oidc::redeemed(&answer, detail, values, &provider.algs, &expect) {
        Ok(v) => v,
        Err(r) => {
            refused(o, &r.to_string());
            return match r {
                // a 400 is the code's: used (a callback sent again), expired,
                // or not this sign-in's (its verifier is another's). The
                // browser starts again; the provider answered, so this is no
                // outage.
                Refusal::Code(OidcError::Refused { status: 400, .. }) => Err(CellError::invalid(format!("{r}: start again"))),
                Refusal::IdToken(_) => refusal_page(&CellError::new(ErrorCode::Unauthenticated, r.to_string())),
                _ => Err(CellError::new(ErrorCode::UpstreamFailed, r.to_string())),
            };
        }
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
    let done = match ask_registry(env, &calls::Exchange::verified(bound, &o.issuer, verified, &email)).await {
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
