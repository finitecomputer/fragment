//! The door's HTTP: hyper over tokio, one request a connection, every
//! connection bounded (`CONNECTIONS_MAX` at once, each `CONNECTION_TIMEOUT`,
//! its head within `HEAD_TIMEOUT`, a body `BODY_BYTES_MAX`).
//!
//! It answers the three WorkOS paths the platform calls and the provider's
//! callback (`config::CALLBACK_PATH`), or, a passkey door, its own pages
//! and their ceremonies (passkeys.rs), and 404 to every other (WorkOS
//! Pipes among them: a self-hosted platform has none).

use std::convert::Infallible;
use std::future::Future;
use std::net::{SocketAddr, TcpListener as StdListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::future::{select, Either};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{oneshot, Semaphore};
use url::Url;

use crate::codes::{random_code, Codes};
use crate::config::{Client, Config, SignIn, CALLBACK_PATH};
use crate::passkeys::{pages, Invited, PasskeyError, Passkeys, INVITES_PATH};
use crate::oidc::{self, Authorize, OidcError, Refusal, SignInSecrets, CODE_MAX};
use crate::person::{person_of, EMAIL_BYTES_MAX};
use crate::provider::{ClientError, ProviderClient};
use crate::state::{Begun, StateKey};

/// Connections served at once; one past it is closed unanswered.
pub const CONNECTIONS_MAX: usize = 256;
/// A connection's whole life: a callback's calls to the provider (its
/// metadata, the token, its keys and one refetch of them, each within
/// `oidc::FETCH_TIMEOUT_MS`) fit inside.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(75);
/// A request's head, from the connection's start.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// An exchange's body at most (the platform's is about 200 bytes).
pub const BODY_BYTES_MAX: usize = 16 * 1024;
/// A query at most: a state, a callback, a client id and a hint or two.
pub const QUERY_BYTES_MAX: usize = 8 * 1024;
/// The platform's state at most (its own is 64 hex digits).
pub const STATE_BYTES_MAX: usize = 1024;
const _: () = assert!(CONNECTION_TIMEOUT.as_millis() > 4 * oidc::FETCH_TIMEOUT_MS as u128, "a callback's four calls to the provider fit in its connection");
/// A passkey ceremony's body at most (a credential with its attestation).
pub const PASSKEY_BODY_BYTES_MAX: usize = 64 * 1024;
/// An inviter's ask at most (`{"email": …}`).
pub const ASK_BODY_BYTES_MAX: usize = 4 * 1024;
/// An invite's link token: 64 hex digits.
const INVITE_BYTES: usize = 64;
/// A login hint passed on to the provider at most (an email's).
const LOGIN_HINT_BYTES_MAX: usize = EMAIL_BYTES_MAX;
/// The provider's own error code, passed on, at most.
const PROVIDER_ERROR_BYTES_MAX: usize = 64;

const AUTHORIZE: &str = "/user_management/authorize";
const AUTHENTICATE: &str = "/user_management/authenticate";
const LOGOUT: &str = "/user_management/sessions/logout";

struct Shared {
    config: Config,
    codes: Mutex<Codes>,
    /// The provider's client, or the door's passkeys: one of the two.
    provider: Option<ProviderClient>,
    passkeys: Option<Passkeys>,
    state_key: StateKey,
}

/// A door serving on a thread of its own until dropped (dev, the e2e and
/// tests; the binary serves with `run`). Dropping it stops it and forgets
/// every code it issued, as a restart does.
pub struct Door {
    pub url: String,
    pub addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("bind {addr}: {source}")]
    Bind { addr: SocketAddr, source: std::io::Error },
    #[error("the door's runtime: {0}")]
    Runtime(std::io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the door's passkeys: {0}")]
    Passkeys(#[from] PasskeyError),
}

fn bind(addr: SocketAddr) -> Result<StdListener, StartError> {
    let listener = StdListener::bind(addr).map_err(|source| StartError::Bind { addr, source })?;
    listener.set_nonblocking(true).map_err(|source| StartError::Bind { addr, source })?;
    Ok(listener)
}

fn runtime() -> Result<tokio::runtime::Runtime, StartError> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(StartError::Runtime)
}

impl Door {
    /// Binds `config.listen` (port 0 picks one) and serves until dropped.
    pub fn start(config: Config) -> Result<Door, StartError> {
        let listener = bind(config.listen)?;
        let addr = listener.local_addr().map_err(|source| StartError::Bind { addr: config.listen, source })?;
        let rt = runtime()?;
        let shared = Arc::new(Shared::new(config)?);
        let (stop, stopped) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("identity-door".into())
            .spawn(move || {
                rt.block_on(serve(listener, shared, async {
                    let _ = stopped.await;
                }))
            })
            .map_err(StartError::Runtime)?;
        Ok(Door { url: format!("http://{addr}"), addr, stop: Some(stop), thread: Some(thread) })
    }

    /// Serves on this thread until `until` is done (the binary's: a signal).
    pub fn run(config: Config, until: impl Future<Output = ()>) -> Result<(), StartError> {
        let listener = bind(config.listen)?;
        let addr = listener.local_addr().map_err(|source| StartError::Bind { addr: config.listen, source })?;
        let rt = runtime()?;
        let by = match &config.signin {
            SignIn::Provider(p) => format!("signing people in at {}", p.settings.issuer),
            SignIn::Passkeys(k) => format!("signing people in by their passkeys for {}", k.rp_id),
        };
        eprintln!("identity-door: serving on http://{addr} as {}, {by}", config.public_url);
        let shared = Arc::new(Shared::new(config)?);
        rt.block_on(serve(listener, shared, until));
        Ok(())
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            // its runtime gone, every connection it held is closed and its
            // port free for the next door's
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn new(config: Config) -> Result<Shared, StartError> {
        let (provider, passkeys) = match &config.signin {
            SignIn::Provider(p) => (Some(ProviderClient::new(p.settings.clone(), p.client_secret.clone(), p.ca_pem.as_deref())?), None),
            SignIn::Passkeys(k) => (None, Some(Passkeys::new(k.clone())?)),
        };
        let state_key = StateKey::of(&config.secret);
        Ok(Shared { config, codes: Mutex::new(Codes::new()), provider, passkeys, state_key })
    }
}

async fn serve(listener: StdListener, shared: Arc<Shared>, until: impl Future<Output = ()>) {
    let listener = tokio::net::TcpListener::from_std(listener).expect("a nonblocking listener joins the runtime it is served in");
    let permits = Arc::new(Semaphore::new(CONNECTIONS_MAX));
    let accepting = async move {
        let mut connections = tokio::task::JoinSet::new();
        // unbounded by design: a server's accept loop, each connection
        // bounded by its permit and its timeout
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    // out of descriptors, say: let some close
                    eprintln!("identity-door: accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                drop(stream);
                continue;
            };
            let shared = shared.clone();
            connections.spawn(async move {
                connection(stream, shared).await;
                drop(permit);
            });
            while connections.try_join_next().is_some() {}
            assert!(connections.len() <= CONNECTIONS_MAX, "at most CONNECTIONS_MAX connections are served");
        }
    };
    futures_util::pin_mut!(accepting, until);
    match select(accepting, until).await {
        // dropping the accept loop drops its connections, each aborted
        Either::Left(((), _)) | Either::Right(((), _)) => {}
    }
}

async fn connection(stream: tokio::net::TcpStream, shared: Arc<Shared>) {
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let shared = shared.clone();
        async move { Ok::<_, Infallible>(answer(&shared, req).await) }
    });
    let served = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEAD_TIMEOUT)
        .keep_alive(false)
        .serve_connection(TokioIo::new(stream), service);
    let _ = tokio::time::timeout(CONNECTION_TIMEOUT, served).await;
}

async fn answer(shared: &Shared, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_string();
    if let Some(passkeys) = &shared.passkeys {
        if let Some(answered) = passkey_route(shared, passkeys, req.method(), &path, &req) {
            return answered;
        }
        if path == INVITES_PATH && req.method() == Method::POST {
            return asked_invite(shared, passkeys, req).await;
        }
        if path.starts_with("/passkeys/") && req.method() == Method::POST {
            return ceremony(shared, passkeys, &path, req).await;
        }
    }
    match (req.method(), path.as_str()) {
        (&Method::GET, AUTHORIZE) => authorize(shared, &req).await,
        (&Method::GET, CALLBACK_PATH) => callback(shared, &req).await,
        (&Method::POST, AUTHENTICATE) => authenticate(shared, req).await,
        (&Method::GET, LOGOUT) => logout(shared, &req),
        (_, AUTHORIZE | AUTHENTICATE | LOGOUT | CALLBACK_PATH) => json(StatusCode::METHOD_NOT_ALLOWED, json!({ "error": "method_not_allowed", "message": "not with this method" })),
        _ => json(
            StatusCode::NOT_FOUND,
            json!({ "error": "not_found", "message": "this door answers sign-in alone (authorize, authenticate, logout): no other WorkOS API, Pipes among them, is here" }),
        ),
    }
}

/// A request's query, each name at most once.
fn query(req: &Request<Incoming>) -> Result<Vec<(String, String)>, &'static str> {
    let raw = req.uri().query().unwrap_or("");
    if raw.len() > QUERY_BYTES_MAX {
        return Err("the query is too long");
    }
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(raw.as_bytes()).into_owned().collect();
    for (i, (k, _)) in pairs.iter().enumerate() {
        if pairs[..i].iter().any(|(seen, _)| seen == k) {
            return Err("a parameter is given twice");
        }
    }
    Ok(pairs)
}

fn param<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// A state the platform's callback can be handed back: printable, bounded.
fn state_ok(state: &str) -> bool {
    !state.is_empty() && state.len() <= STATE_BYTES_MAX && state.bytes().all(|b| b.is_ascii_graphic())
}

fn now_s() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("the clock is past 1970");
    i64::try_from(since.as_secs()).expect("seconds since 1970 fit an i64")
}

/// `GET /user_management/authorize`: the platform's sign-in, sent on to
/// the provider with PKCE and a nonce, the door's state sealed (`state.rs`)
/// holding the platform's client, callback and state.
async fn authorize(shared: &Shared, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let pairs = match query(req) {
        Ok(pairs) => pairs,
        Err(why) => return page(StatusCode::BAD_REQUEST, "Sign-in did not start", why),
    };
    // before the client and its callback are known, a refusal is a page
    // here: never a redirect to a URL no client registered
    let Some(client) = param(&pairs, "client_id").and_then(|id| shared.config.client(id)) else {
        return page(StatusCode::BAD_REQUEST, "Sign-in did not start", "This door signs no one in for that client.");
    };
    let Some((index, callback)) = param(&pairs, "redirect_uri").and_then(|u| client.redirect(u)) else {
        return page(StatusCode::BAD_REQUEST, "Sign-in did not start", "That callback is not one its client registered.");
    };
    let state = param(&pairs, "state");
    if !state.is_none_or(state_ok) {
        return back(callback, None, Err(("invalid_request", "the state is empty, unprintable, or past 1024 bytes".into())));
    }
    if param(&pairs, "response_type") != Some("code") {
        return back(callback, state, Err(("unsupported_response_type", "this door answers response_type=code alone".into())));
    }
    let begun = Begun { client_id: client.id.clone(), callback: index, client_state: state.map(str::to_string), at_s: now_s(), salt: hex::encode(&random_code()[..16]) };
    let sealed = match shared.state_key.seal(&begun) {
        Ok(sealed) => sealed,
        Err(refusal) => return back(callback, None, Err(("invalid_request", refusal.to_string()))),
    };
    let hint = param(&pairs, "login_hint").filter(|h| !h.is_empty() && h.len() <= LOGIN_HINT_BYTES_MAX && h.bytes().all(|b| b.is_ascii_graphic()));
    let Some(client_of_provider) = shared.provider.as_ref() else {
        // a passkey door: its own page signs the person in
        return html(StatusCode::OK, pages::signin(&base_path(shared), &sealed, hint));
    };
    let provider = match client_of_provider.provider().await {
        Ok(provider) => provider,
        Err(refusal) => {
            eprintln!("identity-door: authorize for {}: {refusal}", client.id);
            return back(callback, state, Err(("temporarily_unavailable", format!("the door could not reach its sign-in provider: {refusal}"))));
        }
    };
    let secrets = SignInSecrets::of(&shared.config.secret, &sealed);
    let redirect_uri = shared.config.callback_url();
    let to = oidc::authorize_url(&Authorize { endpoint: &provider.authorization_endpoint, settings: &client_of_provider.settings, redirect_uri: &redirect_uri, state: &sealed, secrets: &secrets, login_hint: hint });
    found(&to)
}

/// An OAuth error code from the provider, passed on when it is one (a
/// short word of lower case and `_`), else `access_denied`.
fn provider_error(code: &str) -> &str {
    let word = !code.is_empty() && code.len() <= PROVIDER_ERROR_BYTES_MAX && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    if word {
        code
    } else {
        "access_denied"
    }
}

/// `GET /oidc/callback`: the provider's answer. Its state opened (else a
/// page: the door cannot know where to send the browser), its code redeemed
/// and its id_token verified (`ProviderClient::redeem`), the person read
/// (`person::person_of`), and the browser sent back to the platform with a
/// code of the door's own and the platform's state, or with why not.
async fn callback(shared: &Shared, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let Some(provider) = shared.provider.as_ref() else {
        return page(StatusCode::NOT_FOUND, "Sign-in did not finish", "This door signs people in by their passkeys: it has no provider to come back from.");
    };
    let pairs = match query(req) {
        Ok(pairs) => pairs,
        Err(why) => return page(StatusCode::BAD_REQUEST, "Sign-in did not finish", why),
    };
    let sealed = param(&pairs, "state").unwrap_or("");
    let begun = match shared.state_key.open(sealed, now_s()) {
        Ok(begun) => begun,
        Err(why) => return page(StatusCode::BAD_REQUEST, "Sign-in did not finish", &format!("{why}. Start the sign-in again where you began it.")),
    };
    let found_callback = shared.config.client(&begun.client_id).and_then(|c| c.redirect_uris.get(usize::from(begun.callback)).map(|u| (c, u)));
    let Some((client, to)) = found_callback else {
        return page(StatusCode::BAD_REQUEST, "Sign-in did not finish", "This sign-in's client is no longer the door's. Start again where you began.");
    };
    let state = begun.client_state.as_deref();
    if let Some(error) = param(&pairs, "error") {
        let said = param(&pairs, "error_description").map(oidc::cut).unwrap_or_default();
        eprintln!("identity-door: the provider refused a sign-in for {}: {error}: {said}", client.id);
        return back(to, state, Err((provider_error(error), format!("the sign-in provider said: {said}"))));
    }
    let Some(code) = param(&pairs, "code").filter(|c| !c.is_empty() && c.len() <= CODE_MAX) else {
        return back(to, state, Err(("invalid_request", "the sign-in provider answered no code".into())));
    };
    let secrets = SignInSecrets::of(&shared.config.secret, sealed);
    let verified = match provider.redeem(code, &shared.config.callback_url(), &secrets, now_s()).await {
        Ok(verified) => verified,
        Err(refusal) => {
            eprintln!("identity-door: a sign-in for {} refused: {refusal}", client.id);
            let error = match &refusal {
                Refusal::Code(OidcError::Refused { status, .. }) if (400..500).contains(status) => "access_denied",
                Refusal::IdToken(_) => "access_denied",
                Refusal::Code(_) | Refusal::Provider(_) => "temporarily_unavailable",
            };
            return back(to, state, Err((error, format!("{refusal}; start the sign-in again"))));
        }
    };
    let settings = &provider.settings;
    let person = match person_of(&settings.issuer, &verified, &settings.email_claim, &shared.config.vouched) {
        Ok(person) => person,
        Err(refused) => {
            eprintln!("identity-door: a sign-in for {} refused: {refused}", client.id);
            return back(to, state, Err(("access_denied", refused.to_string())));
        }
    };
    eprintln!("identity-door: {} signed in for {}", person.email, client.id);
    let code = shared.codes.lock().expect("the codes' lock is never poisoned").issue(random_code(), &client.id, person, Instant::now());
    back(to, state, Ok(code))
}

/// The door's root path on its public URL (`/__door`, or empty): where
/// its pages find its script and ceremonies.
fn base_path(shared: &Shared) -> String {
    shared.config.public_url.path().trim_end_matches('/').to_string()
}

/// A passkey door's pages and script: `GET /passkeys.js`, `GET
/// /passkeys` (add one, or recover), `GET /invite/<token>`.
fn passkey_route(shared: &Shared, passkeys: &Passkeys, method: &Method, path: &str, _req: &Request<Incoming>) -> Option<Response<Full<Bytes>>> {
    if method != Method::GET {
        return None;
    }
    let base = base_path(shared);
    match path {
        "/passkeys.js" => {
            let mut resp = bare(StatusCode::OK, Bytes::from_static(pages::SCRIPT.as_bytes()));
            resp.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/javascript; charset=utf-8"));
            Some(resp)
        }
        "/passkeys" => Some(html(StatusCode::OK, pages::manage(&base))),
        _ => {
            let invite = path.strip_prefix("/invite/")?;
            if invite.len() != INVITE_BYTES || !invite.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some(page(StatusCode::NOT_FOUND, "No such invite", "This invite is used, expired, or not this home's."));
            }
            Some(match passkeys.invite_email(invite, now_s()) {
                Ok(email) => html(StatusCode::OK, pages::enroll(&base, invite, &email)),
                Err(_) => page(StatusCode::NOT_FOUND, "No such invite", "This invite is used, expired, or not this home's."),
            })
        }
    }
}

/// What a ceremony's step is sent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    invite: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    flow: Option<String>,
    #[serde(default)]
    credential: Option<serde_json::Value>,
}

fn refused(e: PasskeyError) -> Response<Full<Bytes>> {
    if e.is_refusal() {
        json(StatusCode::BAD_REQUEST, json!({ "error": e.to_string() }))
    } else {
        eprintln!("identity-door: a passkey step failed: {e}");
        json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "the door failed: try again" }))
    }
}

fn need<'a>(v: &'a Option<String>, what: &'static str) -> Result<&'a str, PasskeyError> {
    v.as_deref().filter(|s| !s.is_empty() && s.len() <= STATE_BYTES_MAX * 2).ok_or(PasskeyError::Refused(what))
}

fn credential<T: serde::de::DeserializeOwned>(v: &Option<serde_json::Value>) -> Result<T, PasskeyError> {
    let v = v.clone().ok_or(PasskeyError::Refused("no credential"))?;
    serde_json::from_value(v).map_err(|_| PasskeyError::Refused("that is not a passkey's answer"))
}

/// `POST /passkeys/invites` (`INVITES_PATH`): an invite the door's inviter
/// asks for (passkeys.rs `asked_invite`), signed by NIP-98 for the door's
/// public URL of this path, its body `{"email": …, "nonce"?: …}`. A new email's answer is its invite's link
/// (`invite`), an email with an account's the platform to sign in at
/// (`signIn`); an ask not the inviter's, fresh and first is a 401, and a
/// door with no inviter has no such route.
async fn asked_invite(shared: &Shared, passkeys: &Passkeys, req: Request<Incoming>) -> Response<Full<Bytes>> {
    if passkeys.config.inviter.is_none() {
        return json(StatusCode::NOT_FOUND, json!({ "error": "not_found", "message": "this door takes no asks for invites" }));
    }
    let content_type = req.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !content_type.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")) {
        return json(StatusCode::UNSUPPORTED_MEDIA_TYPE, json!({ "error": "an ask is JSON" }));
    }
    let authorization = req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = match Limited::new(req.into_body(), ASK_BODY_BYTES_MAX).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return json(StatusCode::PAYLOAD_TOO_LARGE, json!({ "error": "an ask is at most 4 KiB" })),
    };
    let public = shared.config.public_url.as_str().trim_end_matches('/');
    match passkeys.asked_invite(authorization.as_deref(), &format!("{public}{INVITES_PATH}"), &body, now_s()) {
        Ok(Invited::New(token)) => {
            eprintln!("identity-door: an invite for a new account, asked by the inviter");
            json(StatusCode::OK, json!({ "invite": format!("{public}/invite/{token}") }))
        }
        Ok(Invited::HasAccount) => json(StatusCode::OK, json!({ "signIn": passkeys.config.origin.as_str() })),
        Err(PasskeyError::NotAsked(why)) => {
            eprintln!("identity-door: an ask for an invite refused: {why}");
            json(StatusCode::UNAUTHORIZED, json!({ "error": why }))
        }
        Err(e) => refused(e),
    }
}

/// `POST /passkeys/<step>`: one step of a ceremony, JSON in and out.
async fn ceremony(shared: &Shared, passkeys: &Passkeys, path: &str, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let content_type = req.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !content_type.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")) {
        return json(StatusCode::UNSUPPORTED_MEDIA_TYPE, json!({ "error": "a step is JSON" }));
    }
    let body = match Limited::new(req.into_body(), PASSKEY_BODY_BYTES_MAX).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return json(StatusCode::PAYLOAD_TOO_LARGE, json!({ "error": "a step is at most 64 KiB" })),
    };
    let Ok(step) = serde_json::from_slice::<Step>(&body) else {
        return json(StatusCode::BAD_REQUEST, json!({ "error": "not a step of this door's" }));
    };
    let now = now_s();
    let answered = (|| -> Result<serde_json::Value, PasskeyError> {
        Ok(match path {
            "/passkeys/signin/begin" => {
                let begun = shared.state_key.open(need(&step.state, "no sign-in to finish: start again where you began")?, now).map_err(|_| PasskeyError::Refused("this sign-in is too old: start again where you began"))?;
                let (flow, options) = passkeys.signin_begin(begun, need(&step.email, "an email")?)?;
                json!({ "flow": flow, "options": options })
            }
            "/passkeys/signin/finish" => {
                let (begun, person) = passkeys.signin_finish(need(&step.flow, "no flow")?, &credential(&step.credential)?)?;
                let to = shared.config.client(&begun.client_id).and_then(|c| c.redirect_uris.get(usize::from(begun.callback)).map(|u| (c, u)));
                let Some((client, to)) = to else {
                    return Err(PasskeyError::Refused("this sign-in's client is no longer the door's: start again"));
                };
                eprintln!("identity-door: {} signed in by a passkey for {}", person.email, client.id);
                let code = shared.codes.lock().expect("the codes' lock is never poisoned").issue(random_code(), &client.id, person, Instant::now());
                let redirect = back(to, begun.client_state.as_deref(), Ok(code));
                let location = redirect.headers().get(LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                json!({ "redirect": location })
            }
            "/passkeys/enroll/begin" => {
                let (flow, options) = passkeys.enroll_begin(need(&step.invite, "no invite")?, now)?;
                json!({ "flow": flow, "options": options })
            }
            "/passkeys/enroll/finish" => {
                let codes = passkeys.enroll_finish(need(&step.flow, "no flow")?, &credential(&step.credential)?, now)?;
                let next = shared.config.clients.first().and_then(|c| c.redirect_uris.first()).map(|u| format!("{}/", u.origin().ascii_serialization())).unwrap_or_default();
                json!({ "recovery_codes": codes, "continue": next })
            }
            "/passkeys/add/begin" => {
                let (flow, options) = passkeys.add_begin(need(&step.email, "an email")?)?;
                json!({ "flow": flow, "options": options })
            }
            "/passkeys/add/prove" => {
                let (flow, options) = passkeys.add_prove(need(&step.flow, "no flow")?, &credential(&step.credential)?)?;
                json!({ "flow": flow, "options": options })
            }
            "/passkeys/add/finish" => {
                passkeys.add_finish(need(&step.flow, "no flow")?, &credential(&step.credential)?)?;
                json!({ "ok": true })
            }
            "/passkeys/recover/begin" => {
                let (flow, options) = passkeys.recover_begin(need(&step.email, "an email")?, need(&step.code, "a recovery code")?)?;
                json!({ "flow": flow, "options": options })
            }
            "/passkeys/recover/finish" => {
                passkeys.recover_finish(need(&step.flow, "no flow")?, &credential(&step.credential)?)?;
                json!({ "ok": true })
            }
            _ => return Err(PasskeyError::Refused("no such step")),
        })
    })();
    match answered {
        Ok(v) => json(StatusCode::OK, v),
        Err(e) => refused(e),
    }
}

/// A redirect to `to`.
fn found(to: &str) -> Response<Full<Bytes>> {
    let mut resp = bare(StatusCode::FOUND, Bytes::new());
    resp.headers_mut().insert(LOCATION, HeaderValue::from_str(to).expect("a URL is a header value"));
    resp
}

/// The client's callback with a code, or an OAuth error, and its state.
fn back(callback: &Url, state: Option<&str>, outcome: Result<String, (&str, String)>) -> Response<Full<Bytes>> {
    let mut to = callback.clone();
    {
        let mut q = to.query_pairs_mut();
        match &outcome {
            Ok(code) => {
                q.append_pair("code", code);
            }
            Err((error, why)) => {
                q.append_pair("error", error).append_pair("error_description", why);
            }
        }
        if let Some(state) = state {
            q.append_pair("state", state);
        }
    }
    found(to.as_str())
}

/// What the platform sends to exchange a code (as WorkOS's
/// `authenticate` takes it). Other fields WorkOS takes are ignored.
#[derive(Deserialize)]
struct Exchange {
    client_id: String,
    client_secret: String,
    grant_type: String,
    code: String,
}

fn oauth_error(status: StatusCode, error: &str, why: &str) -> Response<Full<Bytes>> {
    json(status, json!({ "error": error, "error_description": why }))
}

/// `POST /user_management/authenticate`: a code, with its client's secret,
/// for the person it stands for, once.
async fn authenticate(shared: &Shared, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let content_type = req.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !content_type.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")) {
        return oauth_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid_request", "an exchange is JSON");
    }
    let body = match Limited::new(req.into_body(), BODY_BYTES_MAX).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return oauth_error(StatusCode::PAYLOAD_TOO_LARGE, "invalid_request", "an exchange is at most 16 KiB"),
    };
    let Ok(asked) = serde_json::from_slice::<Exchange>(&body) else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "an exchange names client_id, client_secret, grant_type and code");
    };
    // the client first: a code is never touched by a caller without its
    // secret, so a stolen code cannot be burnt by guessing
    let Some(client) = shared.config.client(&asked.client_id).filter(|c| c.secret_is(&asked.client_secret)) else {
        eprintln!("identity-door: an exchange refused: no client {:?} with that secret", asked.client_id);
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "this client and secret are not the door's");
    };
    if asked.grant_type != "authorization_code" {
        return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "this door exchanges authorization codes alone");
    }
    let taken = shared.codes.lock().expect("the codes' lock is never poisoned").take(&asked.code, &client.id, Instant::now());
    match taken {
        Ok(person) => {
            assert!(!person.subject.is_empty() && !person.email.is_empty(), "a code stands for someone");
            json(StatusCode::OK, json!({ "user": { "object": "user", "id": person.subject, "email": person.email, "email_verified": true } }))
        }
        Err(refusal) => {
            eprintln!("identity-door: an exchange for {} refused: {refusal}", client.id);
            oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", &refusal.to_string())
        }
    }
}

/// `GET /user_management/sessions/logout`: the door keeps no session (its
/// exchange answers no access token, so the platform names none), so this
/// only sends the browser back, to a client's own origin alone.
fn logout(shared: &Shared, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let pairs = match query(req) {
        Ok(pairs) => pairs,
        Err(why) => return page(StatusCode::BAD_REQUEST, "Signed out", why),
    };
    let to = param(&pairs, "return_to").and_then(|r| Url::parse(r).ok());
    let known = |to: &Url| shared.config.clients.iter().any(|c: &Client| c.redirect_uris.iter().any(|r| r.origin() == to.origin()));
    match to {
        Some(to) if known(&to) => found(to.as_str()),
        _ => page(StatusCode::BAD_REQUEST, "Signed out", "This door keeps no session, and that return is not a client's."),
    }
}

/// An answer with the headers every answer carries: never cached, never
/// sniffed, never referred on.
fn bare(status: StatusCode, body: Bytes) -> Response<Full<Bytes>> {
    let mut resp = Response::new(Full::new(body));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    resp
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Full<Bytes>> {
    let mut resp = bare(status, Bytes::from(value.to_string()));
    resp.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp
}

/// A passkey page: the door's own script alone, its ceremonies to the
/// door's own origin alone.
fn html(status: StatusCode, html: String) -> Response<Full<Bytes>> {
    let mut resp = bare(status, Bytes::from(html));
    let h = resp.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    resp
}

fn page(status: StatusCode, title: &str, text: &str) -> Response<Full<Bytes>> {
    let html = format!("<!doctype html><meta charset=\"utf-8\"><title>{}</title><h1>{}</h1><p>{}</p>", esc(title), esc(title), esc(text));
    let mut resp = bare(status, Bytes::from(html));
    let h = resp.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    resp
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
