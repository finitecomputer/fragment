//! WorkOS AuthKit, for the surface the platform calls (phase 4 slice B):
//! the authorize redirect, the code exchange, and the logout redirect
//! (https://workos.com/docs/reference/authkit). Users are made on demand:
//! an authorize with `login_hint` signs in as that email (a new user the
//! first time, authenticated now: the access token's `auth_time`); without
//! one, a browser gets a one-field form (dev), and a test sets who comes
//! next. Levers: change a user's email, fail the next authorize, the codes
//! and sessions it handed out.
//!
//! And the session AuthKit keeps in a browser (its own cookie, on its own
//! domain), which answers an authorize without asking anyone: the fake
//! sees no cookies, so it holds one such session, for the one browser a
//! test drives, only when the test says so (`hold`). While it is held, an
//! authorize without `login_hint` is answered from it (its session id and
//! its `auth_time`, from when it was authenticated); its logout ends it.
//!
//! And Pipes' access tokens (https://workos.com/docs/reference/pipes/access-token):
//! a user's account at a provider is connected by the test (`connect`), or
//! by the user's browser through the consent URL the authorize call gives
//! (`POST /data-integrations/{provider}/authorize`, then `GET
//! /pipes/consent`), and may come to need authorizing again; each token
//! asked for is new, and lasts an hour. Its state is read with no token
//! (`GET /user_management/users/{user}/connected_accounts/{provider}`,
//! https://workos.com/docs/reference/pipes/connected-account): `connected`
//! or `needs_reauthorization`, 404 when there is none. A disconnect
//! deletes it (`DELETE` of the same path, 204), after revoking its grant at
//! the stand-in OAuth provider (`POST /test-oauth/revoke token=`, a token
//! Pipes handed out).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub email: String,
    /// Whether WorkOS verified the email (made verified).
    pub verified: bool,
}

#[derive(Default)]
struct State {
    users: Vec<User>,
    /// code → (user id, session id, `auth_time`), each used once.
    codes: HashMap<String, (String, String, i64)>,
    ended: Vec<String>,
    /// The session the browser holds (`hold`): user id, session id, and
    /// when it was authenticated (seconds).
    held: Option<(String, String, i64)>,
    next_error: Option<String>,
    counter: u64,
    /// (user id, provider) → whether the account is still authorized.
    pipes: HashMap<(String, String), bool>,
    /// Tokens handed out, by provider.
    tokens: HashMap<String, Vec<String>>,
    /// Consents given out and not yet followed: state → (user id, provider).
    consents: HashMap<String, (String, String)>,
    /// Connected accounts' states read (each with no token).
    states_read: u64,
    /// Independent OAuth provider boundary for browser-bound Pipes tests.
    oauth_codes: HashMap<String, (String, String, String, String)>,
    oauth_grants: HashMap<String, (String, String)>,
    oauth_exchanges: usize,
    imports: Vec<(String, String, String)>,
    legacy_authorizations: usize,
    import_attempts: usize,
    fail_import: bool,
    /// Pipes tokens the OAuth provider was asked to revoke, oldest first.
    revoked: Vec<String>,
    fail_revoke: bool,
    /// Connected accounts deleted (user id, provider), each asked once.
    deleted: Vec<(String, String)>,
}

impl State {
    fn next(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}_{:026}", self.counter)
    }

    fn user_for(&mut self, email: &str) -> User {
        if let Some(u) = self.users.iter().find(|u| u.email.eq_ignore_ascii_case(email)) {
            return u.clone();
        }
        let user = User { id: self.next("user"), email: email.to_string(), verified: true };
        self.users.push(user.clone());
        user
    }
}

pub struct WorkOs {
    pub url: String,
    pub client_id: String,
    state: Arc<Mutex<State>>,
    gates: Arc<Gates>,
    _server: Server,
}

/// The stand-in provider's exchange and Pipes' import, each held by a test
/// (`pause`) until it lets them go (`resume`): a request waits outside the
/// state's lock (each has its thread), so the rest of the fake answers
/// meanwhile.
#[derive(Default)]
struct Gates {
    held: Mutex<HashMap<Gate, usize>>,
    changed: std::sync::Condvar,
}

/// What a test may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// `POST /test-oauth/token`, a callback's code exchange.
    Exchange,
    /// `POST|PUT …/connected_accounts/…`, a callback's import.
    Import,
}

impl Gates {
    /// Waits while `gate` is held, counted as waiting.
    fn pass(&self, gate: Gate) {
        let mut held = self.held.lock().expect("gates");
        if !held.contains_key(&gate) {
            return;
        }
        *held.get_mut(&gate).expect("held") += 1;
        self.changed.notify_all();
        while held.contains_key(&gate) {
            held = self.changed.wait(held).expect("gates");
        }
    }
}

fn problem(status: u16, error: &str, description: &str) -> Response {
    Response::json(status, &json!({ "error": error, "error_description": description }))
}

fn redirect(to: &str) -> Response {
    Response::bytes(302, "text/plain", vec![]).with_header("location", to)
}

fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    let q: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect();
    format!("{base}{}{}", if base.contains('?') { "&" } else { "?" }, q.join("&"))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// An access token shaped like AuthKit's (a JWT); the platform reads its
/// `sid` and `auth_time` from the exchange's answer and never trusts them
/// from a browser, so the fake does not sign it.
fn access_token(user: &str, sid: &str, auth_time: i64, client_id: &str) -> String {
    let b64 = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.fake",
        b64(json!({ "alg": "none", "typ": "JWT" })),
        b64(json!({ "iss": "https://api.workos.com", "sub": user, "sid": sid, "auth_time": auth_time, "client_id": client_id }))
    )
}

fn now_s() -> i64 {
    crate::codestorage::now_ms() / 1000
}

impl WorkOs {
    pub fn connection_oauth(&self) -> Value {
        json!({ "authorize": format!("{}/test-oauth/authorize", self.url), "exchange": format!("{}/test-oauth/token", self.url), "client_id": "mock-connection-client", "scopes": ["mail.read", "files.read"], "revoke": format!("{}/test-oauth/revoke", self.url) })
    }

    /// `gate`'s requests wait from now until `resume`.
    pub fn pause(&self, gate: Gate) {
        self.gates.held.lock().expect("gates").entry(gate).or_insert(0);
    }

    /// How many requests wait at `gate` (since it was held).
    pub fn paused(&self, gate: Gate) -> usize {
        self.gates.held.lock().expect("gates").get(&gate).copied().unwrap_or(0)
    }

    /// `gate`'s requests go on, and later ones pass.
    pub fn resume(&self, gate: Gate) {
        self.gates.held.lock().expect("gates").remove(&gate);
        self.gates.changed.notify_all();
    }

    /// The tokens revoked at the OAuth provider, oldest first: Pipes' for
    /// an account, or one the provider gave a callback.
    pub fn revoked(&self) -> Vec<String> { self.state.lock().expect("workos state").revoked.clone() }
    /// The next revocation fails (503), as a provider that does not answer.
    pub fn fail_next_revoke(&self) { self.state.lock().expect("workos state").fail_revoke = true; }
    /// The connected accounts deleted at Pipes: (user id, provider), oldest first.
    pub fn deleted(&self) -> Vec<(String, String)> { self.state.lock().expect("workos state").deleted.clone() }
    /// Whether `email`'s account at `provider` is one Pipes holds.
    pub fn holds(&self, email: &str, provider: &str) -> bool {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        s.pipes.contains_key(&(user.id, provider.to_string()))
    }

    pub fn connection_counts(&self) -> (usize, usize, usize) {
        let s = self.state.lock().expect("workos state");
        (s.oauth_exchanges, s.imports.len(), s.legacy_authorizations)
    }

    /// Attribution only: no token bytes in test diagnostics.
    pub fn imported_accounts(&self) -> Vec<(String, String, String)> {
        self.state.lock().expect("workos state").imports.clone()
    }

    pub fn fail_next_import(&self) { self.state.lock().expect("workos state").fail_import = true; }
    pub fn import_attempts(&self) -> usize { self.state.lock().expect("workos state").import_attempts }
    /// Serves on a free port for one environment: its client id and API key.
    pub fn start(client_id: &str, api_key: &str) -> std::io::Result<WorkOs> {
        Self::start_on(0, client_id, api_key)
    }

    pub fn start_on(port: u16, client_id: &str, api_key: &str) -> std::io::Result<WorkOs> {
        let state: Arc<Mutex<State>> = Arc::default();
        let (st, client, key) = (Arc::clone(&state), client_id.to_string(), api_key.to_string());
        let gates: Arc<Gates> = Arc::default();
        let gated = Arc::clone(&gates);
        // where the fake answers, for the consent URLs it gives (known once it listens)
        let base: Arc<Mutex<String>> = Arc::default();
        let at = Arc::clone(&base);
        let handler: Handler = Arc::new(move |req: &Request| {
            let q = |k: &str| req.query.get(k).cloned().unwrap_or_default();
            match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/test-oauth/token") => gated.pass(Gate::Exchange),
                ("POST" | "PUT", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => gated.pass(Gate::Import),
                _ => {}
            }
            let mut s = st.lock().expect("workos state");
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/test-oauth/authorize") => {
                    if q("client_id") != "mock-connection-client" || q("response_type") != "code" || q("code_challenge_method") != "S256" { return problem(400, "invalid_request", "OAuth parameters"); }
                    let Ok(mut back) = url::Url::parse(&q("redirect_uri")) else { return problem(400, "invalid_request", "callback"); };
                    if q("decline") != "yes" {
                        let code = s.next("oauth_code");
                        s.oauth_codes.insert(code.clone(), (q("code_challenge"), q("redirect_uri"), q("scope"), q("account")));
                        back.query_pairs_mut().append_pair("code", &code);
                    }
                    back.query_pairs_mut().append_pair("state", &q("state"));
                    redirect(back.as_str())
                }
                ("POST", "/test-oauth/token") => {
                    s.oauth_exchanges += 1;
                    let form: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
                    let f = |k: &str| form.get(k).map(String::as_str).unwrap_or("");
                    let Some((challenge, callback, scopes, account)) = s.oauth_codes.remove(f("code")) else { return problem(400, "invalid_grant", "code"); };
                    use sha2::Digest;
                    let actual = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(f("code_verifier").as_bytes()));
                    if challenge != actual || callback != f("redirect_uri") || f("client_id") != "mock-connection-client" || f("client_secret") != "mock-connection-secret" || f("grant_type") != "authorization_code" {
                        return problem(400, "invalid_grant", "binding");
                    }
                    let access = s.next("mock_access");
                    let refresh = s.next("mock_refresh");
                    s.oauth_grants.insert(access.clone(), (refresh.clone(), account));
                    Response::json(200, &json!({ "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 3600, "scope": scopes }))
                }
                // RFC 7009 as Google answers it: a token Pipes or the provider handed out, or 400
                ("POST", "/test-oauth/revoke") => {
                    if std::mem::take(&mut s.fail_revoke) { return problem(503, "unavailable", "test revocation failure"); }
                    let form: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
                    let token = form.get("token").cloned().unwrap_or_default();
                    let ours = s.tokens.values().flatten().any(|t| *t == token) || s.oauth_grants.contains_key(&token);
                    if form.len() != 1 || !ours { return problem(400, "invalid_token", "not a token handed out here"); }
                    s.revoked.push(token);
                    Response::json(200, &json!({}))
                }
                ("POST" | "PUT", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => {
                    s.import_attempts += 1;
                    if std::mem::take(&mut s.fail_import) { return problem(503, "unavailable", "test import failure"); }
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) { return problem(401, "unauthorized", "API key"); }
                    let Some((user, provider)) = p["/user_management/users/".len()..].split_once("/connected_accounts/") else { return problem(404, "not_found", "route"); };
                    if !s.users.iter().any(|u| u.id == user) { return problem(404, "not_found", "user"); }
                    if req.method == "POST" && s.pipes.contains_key(&(user.into(), provider.into())) { return problem(409, "conflict", "already exists"); }
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let Some((refresh, account)) = s.oauth_grants.get(body["access_token"].as_str().unwrap_or("")).cloned() else { return problem(422, "invalid_tokens", "not issued here"); };
                    if body["refresh_token"] != refresh || !body["expires_at"].is_string() || body["state"] != "connected" { return problem(422, "invalid_tokens", "token combination"); }
                    s.pipes.insert((user.into(), provider.into()), true);
                    s.imports.push((user.into(), provider.into(), account));
                    Response::json(if req.method == "POST" { 201 } else { 200 }, &json!({ "id": format!("conn_{user}_{provider}"), "user_id": user, "state": "connected" }))
                }
                ("GET", "/user_management/authorize") | ("GET", "/user_management/authorize/as") => {
                    if q("client_id") != client {
                        return problem(400, "invalid_client", "unknown client_id");
                    }
                    if q("response_type") != "code" || q("redirect_uri").is_empty() {
                        return problem(400, "invalid_request", "response_type=code and a redirect_uri");
                    }
                    let back = q("redirect_uri");
                    if let Some(error) = s.next_error.take() {
                        return redirect(&with_query(&back, &[("error", &error), ("error_description", "a failure the test asked for"), ("state", &q("state"))]));
                    }
                    let email = q("login_hint");
                    // the browser's session answers, asking no one
                    if let Some((user, sid, auth_time)) = s.held.clone().filter(|_| email.is_empty()) {
                        let code = s.next("code");
                        s.codes.insert(code.clone(), (user, sid, auth_time));
                        return redirect(&with_query(&back, &[("code", &code), ("state", &q("state"))]));
                    }
                    if email.is_empty() {
                        // a person at a browser: who are you?
                        let fields: String = ["client_id", "redirect_uri", "response_type", "provider", "state"]
                            .iter()
                            .map(|k| format!(r#"<input type="hidden" name="{k}" value="{}">"#, q(k).replace('"', "&quot;")))
                            .collect();
                        let page = format!(
                            r#"<!doctype html><title>WorkOS (fake)</title><form action="/user_management/authorize/as" method="get">
<p>The WorkOS fake: sign in as</p>{fields}<input name="login_hint" value="dev@fragment.localhost" size="32"> <button>Sign in</button></form>"#
                        );
                        return Response::bytes(200, "text/html; charset=utf-8", page.into_bytes());
                    }
                    let user = s.user_for(&email);
                    let (code, sid) = (s.next("code"), s.next("session"));
                    s.codes.insert(code.clone(), (user.id, sid, now_s()));
                    redirect(&with_query(&back, &[("code", &code), ("state", &q("state"))]))
                }
                ("POST", "/user_management/authenticate") => {
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    if body["client_id"] != client.as_str() || body["client_secret"] != key.as_str() {
                        return problem(401, "invalid_client", "the client id or secret is wrong");
                    }
                    if body["grant_type"] != "authorization_code" {
                        return problem(400, "unsupported_grant_type", "authorization_code only");
                    }
                    let code = body["code"].as_str().unwrap_or("");
                    let Some((uid, sid, auth_time)) = s.codes.remove(code) else {
                        return problem(400, "invalid_grant", "the code is invalid or was used");
                    };
                    let user = s.users.iter().find(|u| u.id == uid).cloned().expect("a code names a user");
                    Response::json(
                        200,
                        &json!({
                            "user": {
                                "object": "user", "id": user.id, "email": user.email, "email_verified": user.verified,
                                "first_name": null, "last_name": null, "profile_picture_url": null,
                                "created_at": "2026-09-24T00:00:00.000Z", "updated_at": "2026-09-24T00:00:00.000Z",
                            },
                            "organization_id": null,
                            "access_token": access_token(&user.id, &sid, auth_time, &client),
                            "refresh_token": format!("refresh_{sid}"),
                            "authentication_method": "Password",
                        }),
                    )
                }
                ("GET", "/user_management/sessions/logout") => {
                    let sid = q("session_id");
                    if sid.is_empty() {
                        return problem(400, "invalid_request", "session_id");
                    }
                    if s.held.as_ref().is_some_and(|(_, held, _)| *held == sid) {
                        s.held = None;
                    }
                    s.ended.push(sid);
                    match q("return_to") {
                        r if r.is_empty() => Response::bytes(200, "text/plain", b"signed out".to_vec()),
                        r => redirect(&r),
                    }
                }
                ("POST", p) if p.starts_with("/data-integrations/") && p.ends_with("/authorize") => {
                    s.legacy_authorizations += 1;
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let provider = p["/data-integrations/".len()..p.len() - "/authorize".len()].to_string();
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let Some(user) = body["user_id"].as_str().filter(|u| s.users.iter().any(|x| x.id == *u)).map(str::to_string) else {
                        return problem(404, "not_found", "no such user");
                    };
                    let state = s.next("pipes_state");
                    s.consents.insert(state.clone(), (user, provider));
                    let url = format!("{}/pipes/consent?state={state}", at.lock().expect("workos base"));
                    Response::json(200, &json!({ "url": url, "state": state }))
                }
                // the user's browser, consenting: the account is connected
                ("GET", "/pipes/consent") => match s.consents.remove(&q("state")) {
                    Some((user, provider)) => {
                        s.pipes.insert((user, provider.clone()), true);
                        Response::bytes(200, "text/html; charset=utf-8", format!("<!doctype html><title>Connected</title><p>{provider} is connected. You can close this window.</p>").into_bytes())
                    }
                    None => problem(400, "invalid_request", "this consent was used or never given"),
                },
                // a connected account's state, with no token in it
                ("GET", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => {
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let rest = &p["/user_management/users/".len()..];
                    let Some((user, provider)) = rest.split_once("/connected_accounts/") else { return problem(404, "not_found", "no such route") };
                    s.states_read += 1;
                    match s.pipes.get(&(user.to_string(), provider.to_string())).copied() {
                        None => problem(404, "not_found", "no connected account"),
                        Some(authorized) => Response::json(
                            200,
                            &json!({
                                "object": "connected_account", "id": format!("conn_{user}_{provider}"), "user_id": user, "organization_id": null,
                                "scopes": [], "state": if authorized { "connected" } else { "needs_reauthorization" },
                                "created_at": "2026-10-01T00:00:00.000Z", "updated_at": "2026-10-01T00:00:00.000Z",
                            }),
                        ),
                    }
                }
                // a connected account deleted: its tokens go; 404 when there is none
                ("DELETE", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => {
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let Some((user, provider)) = p["/user_management/users/".len()..].split_once("/connected_accounts/") else { return problem(404, "not_found", "no such route") };
                    match s.pipes.remove(&(user.to_string(), provider.to_string())) {
                        None => problem(404, "not_found", "no connected account"),
                        Some(_) => {
                            s.deleted.push((user.into(), provider.into()));
                            Response::bytes(204, "text/plain", vec![])
                        }
                    }
                }
                ("POST", p) if p.starts_with("/data-integrations/") && p.ends_with("/token") => {
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let provider = p["/data-integrations/".len()..p.len() - "/token".len()].to_string();
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let Some(user) = body["user_id"].as_str().filter(|u| s.users.iter().any(|x| x.id == *u)).map(str::to_string) else {
                        return problem(404, "not_found", "no such user");
                    };
                    match s.pipes.get(&(user.clone(), provider.clone())).copied() {
                        None => Response::json(200, &json!({ "active": false, "error": "not_installed" })),
                        Some(false) => Response::json(200, &json!({ "active": false, "error": "needs_reauthorization" })),
                        Some(true) => {
                            let token = s.next(&format!("pipes_{provider}"));
                            s.tokens.entry(provider).or_default().push(token.clone());
                            let expires = crate::codestorage::iso(crate::codestorage::now_ms() + 3_600_000);
                            Response::json(
                                200,
                                &json!({ "active": true, "access_token": { "object": "access_token", "access_token": token, "expires_at": expires, "scopes": [], "missing_scopes": [] } }),
                            )
                        }
                    }
                }
                _ => problem(404, "not_found", "no such route"),
            }
        });
        let server = Server::start(port, handler)?;
        *base.lock().expect("workos base") = server.url.clone();
        Ok(WorkOs { url: server.url.clone(), client_id: client_id.to_string(), state, gates, _server: server })
    }

    /// The user an email signs in as (made the first time).
    pub fn user(&self, email: &str) -> User {
        self.state.lock().expect("workos state").user_for(email)
    }

    /// A user's email changes; their id does not.
    pub fn set_email(&self, id: &str, email: &str) {
        let mut s = self.state.lock().expect("workos state");
        if let Some(u) = s.users.iter_mut().find(|u| u.id == id) {
            u.email = email.to_string();
        }
    }

    /// Whether WorkOS has verified a user's email.
    pub fn set_verified(&self, id: &str, verified: bool) {
        let mut s = self.state.lock().expect("workos state");
        if let Some(u) = s.users.iter_mut().find(|u| u.id == id) {
            u.verified = verified;
        }
    }

    /// The next authorize redirects back with this error.
    pub fn fail_next(&self, error: &str) {
        self.state.lock().expect("workos state").next_error = Some(error.to_string());
    }

    /// `email`'s account at `provider` is connected (`true`), or comes to
    /// need authorizing again (`false`).
    pub fn connect(&self, email: &str, provider: &str, authorized: bool) {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        s.pipes.insert((user.id, provider.to_string()), authorized);
    }

    /// How many connected accounts' states were read.
    pub fn states_read(&self) -> u64 {
        self.state.lock().expect("workos state").states_read
    }

    /// The access tokens handed out for `provider`, oldest first.
    pub fn tokens(&self, provider: &str) -> Vec<String> {
        self.state.lock().expect("workos state").tokens.get(provider).cloned().unwrap_or_default()
    }

    /// Sessions ended through the logout URL.
    pub fn ended(&self) -> Vec<String> {
        self.state.lock().expect("workos state").ended.clone()
    }

    /// The browser holds a session as `email` (made the first time),
    /// authenticated `age_s` ago: an authorize without `login_hint` is
    /// answered from it until its logout. Answers its id.
    pub fn hold(&self, email: &str, age_s: i64) -> String {
        let mut s = self.state.lock().expect("workos state");
        let (user, sid) = (s.user_for(email), s.next("session"));
        s.held = Some((user.id, sid.clone(), now_s() - age_s));
        sid
    }

    /// The id of the session the browser holds, while it does.
    pub fn held(&self) -> Option<String> {
        self.state.lock().expect("workos state").held.as_ref().map(|(_, sid, _)| sid.clone())
    }
}
