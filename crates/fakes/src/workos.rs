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
//!
//! The stand-in provider's consent grants what it was asked for, or, as a
//! person who unticks boxes on Google's consent screen, only those of the
//! asked-for scopes its `grant` names (a test's, on the authorize URL).
//! Pipes keeps the scopes an import names, and answers them in the
//! account's state (`scopes`) and its tokens (`scopes`, and the
//! connection's `missing_scopes`).
//!
//! And Pipes' several connections (https://workos.com/docs/pipes/multiple-connections,
//! as workos-node's `src/pipes/pipes.ts` sends them): a person's accounts
//! at a provider, each with its id (`data_installation_…`). The first an
//! import makes with no intent is the compatibility connection, the one a
//! call naming no account reaches (a `GET`, `PUT` or `DELETE` of the path
//! with no `connected_account_id`, and a token asked with no plural
//! opt-in), as before several were; an import with `connection_intent:
//! "add"` in its body makes another (standard) one, up to its limit (25:
//! 409 `connected_account_limit_reached`), or, while the test says Pipes
//! offers one account a person (`refuse_additional_accounts`), 404
//! `multiple_connections_unavailable` once they have one. A `PUT`, `GET`
//! or `DELETE` naming `connected_account_id` in its query is that
//! account's (a `PUT` may say `connection_intent=reauthorize` there, never
//! `add`); the plural list (`GET /user_management/users/{user}/
//! data_providers?supports_multiple_connections=true`) names each, oldest
//! first; a token asked with `supports_multiple_connections` names one, or
//! is refused (409 `account_selection_required`) while several match.
//! Each token remembers the account it was for (`token_account`).

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
    /// (user id, provider) → its connected accounts, oldest first.
    accounts: HashMap<(String, String), Vec<Account>>,
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
    omit_id_token: bool,
    /// An `add` for a person with an account already is refused, as Pipes
    /// does until WorkOS enables additional connections.
    refuse_additional: bool,
    /// The most accounts a person connects at a provider (None: Pipes' 25).
    account_limit: Option<usize>,
    /// The next import is answered 504 and lands only when the test says
    /// (`land_deferred_import`): one whose outcome its caller cannot know.
    defer_import: bool,
    deferred: Option<Import>,
    /// Pipes' tokens' accounts (user id, provider, account id), for a
    /// revocation and for a test asking whose a token was.
    pipes_tokens: HashMap<String, (String, String, String)>,
}

/// Pipes' limit of accounts a person connects at a provider.
const ACCOUNTS_LIMIT: usize = 25;

/// A connected account at Pipes: the compatibility connection, or a
/// standard one an `add` made; whether it is still authorized, its label
/// (`account_display_name`), scopes and identifier as imported, and the
/// stand-in provider's account its grant is (none: a test's lever made it).
#[derive(Clone, Debug)]
struct Account {
    id: String,
    compatibility: bool,
    authorized: bool,
    label: Option<String>,
    scopes: Option<Vec<String>>,
    identifier: Option<String>,
    created_at: String,
    grant: Option<String>,
}

/// A connected account as a test reads it, with no token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connected {
    pub id: String,
    /// The compatibility connection (the one a call naming none reaches).
    pub compatibility: bool,
    pub authorized: bool,
    pub label: Option<String>,
    /// Empty: none named.
    pub scopes: Vec<String>,
    pub identifier: Option<String>,
}

/// Which account an import lands on.
enum Target {
    /// The compatibility connection: made (a `POST`, none there), or
    /// updated, made when there is none (a `PUT` naming no account).
    Compatibility,
    /// Another account (`connection_intent: "add"`).
    Add,
    /// The account a `PUT` names.
    Named(String),
}

/// An import as it lands: (user id, provider), its label as sent (absent:
/// an update keeps the one there), the scopes it names (none: none kept),
/// its identifier (absent: an update keeps it), the account it lands on,
/// and the provider's account its grant is.
struct Import {
    user: String,
    provider: String,
    label: Option<Value>,
    scopes: Option<Vec<String>>,
    identifier: Option<String>,
    target: Target,
    account: String,
}

impl Account {
    fn json(&self, user: &str, provider: &str) -> Value {
        json!({
            "object": "connected_account", "id": self.id, "provider_slug": provider,
            "connection_role": if self.compatibility { "compatibility" } else { "standard" },
            "account_identifier": self.identifier, "account_display_name": self.label, "user_id": user, "organization_id": null,
            "scopes": self.scopes.clone().unwrap_or_default(), "auth_method": "oauth",
            "state": if self.authorized { "connected" } else { "needs_reauthorization" },
            "created_at": self.created_at, "updated_at": self.created_at, "config": {},
        })
    }

    fn read(&self) -> Connected {
        Connected { id: self.id.clone(), compatibility: self.compatibility, authorized: self.authorized, label: self.label.clone(), scopes: self.scopes.clone().unwrap_or_default(), identifier: self.identifier.clone() }
    }
}

impl State {
    fn next(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}_{:026}", self.counter)
    }

    /// A new account of `user`'s at `provider`, oldest last.
    fn made(&mut self, user: &str, provider: &str, compatibility: bool) -> &mut Account {
        let id = self.next("data_installation");
        let created_at = crate::codestorage::iso(crate::codestorage::now_ms());
        let list = self.accounts.entry((user.to_string(), provider.to_string())).or_default();
        list.push(Account { id, compatibility, authorized: true, label: None, scopes: None, identifier: None, created_at, grant: None });
        list.last_mut().expect("just made")
    }

    /// `user`'s compatibility connection at `provider`, made when there is none.
    fn compatibility(&mut self, user: &str, provider: &str) -> &mut Account {
        let key = (user.to_string(), provider.to_string());
        match self.accounts.get(&key).and_then(|l| l.iter().position(|a| a.compatibility)) {
            Some(at) => &mut self.accounts.get_mut(&key).expect("listed")[at],
            None => self.made(user, provider, true),
        }
    }

    /// The account a call names (`connected_account_id`), else the
    /// compatibility connection.
    fn named(&self, user: &str, provider: &str, id: Option<&str>) -> Option<&Account> {
        let list = self.accounts.get(&(user.to_string(), provider.to_string()))?;
        match id {
            Some(id) => list.iter().find(|a| a.id == id),
            None => list.iter().find(|a| a.compatibility),
        }
    }

    /// An import lands: its account connected, its label set (a string),
    /// cleared (null) or, omitted, kept by an update (a new account has
    /// none), its scopes as named, and its identifier set, or kept. One
    /// naming an account deleted meanwhile lands nowhere. Answers the
    /// account.
    fn land(&mut self, i: Import) -> Option<Value> {
        let key = (i.user.clone(), i.provider.clone());
        let a = match &i.target {
            Target::Compatibility => self.compatibility(&i.user, &i.provider),
            Target::Add => self.made(&i.user, &i.provider, false),
            Target::Named(id) => self.accounts.get_mut(&key)?.iter_mut().find(|a| a.id == *id)?,
        };
        a.authorized = true;
        a.scopes = i.scopes;
        match i.label {
            Some(Value::String(label)) => a.label = Some(label),
            Some(Value::Null) => a.label = None,
            _ => {}
        }
        if let Some(identifier) = i.identifier {
            a.identifier = Some(identifier);
        }
        a.grant = Some(i.account.clone());
        let answer = a.json(&i.user, &i.provider);
        self.imports.push((i.user, i.provider, i.account));
        Some(answer)
    }

    /// A revocation of `token` at the provider, as Google's: it ends the
    /// whole grant of the provider's account it is for, so every Pipes
    /// account last imported from that account (or, a token Pipes minted
    /// for an account no import made, that one) needs authorizing again.
    fn revoke(&mut self, token: &str) {
        let minted = self.pipes_tokens.get(token).cloned();
        let grant = self.oauth_grants.get(token).map(|g| g.1.clone()).or_else(|| {
            let (user, provider, id) = minted.as_ref()?;
            self.accounts.get(&(user.clone(), provider.clone()))?.iter().find(|a| a.id == *id)?.grant.clone()
        });
        for ((user, provider), list) in self.accounts.iter_mut() {
            for a in list.iter_mut() {
                let ended = match &grant {
                    Some(g) => a.grant.as_ref() == Some(g),
                    None => minted.as_ref().is_some_and(|(u, p, id)| u == user && p == provider && *id == a.id),
                };
                if ended {
                    a.authorized = false;
                }
            }
        }
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// `POST /test-oauth/token`, a callback's code exchange.
    Exchange,
    /// The exchange of one code alone (the code a callback brings back).
    ExchangeOf(String),
    /// `POST|PUT …/connected_accounts/…`, a callback's import.
    Import,
}

impl Gates {
    /// Waits while any of `gates` is held, counted as waiting at each.
    fn pass(&self, gates: &[Gate]) {
        let holding = |held: &HashMap<Gate, usize>| gates.iter().any(|g| held.contains_key(g));
        let mut held = self.held.lock().expect("gates");
        if !holding(&held) {
            return;
        }
        for g in gates {
            if let Some(n) = held.get_mut(g) {
                *n += 1;
            }
        }
        self.changed.notify_all();
        while holding(&held) {
            held = self.changed.wait(held).expect("gates");
        }
    }
}

fn problem(status: u16, error: &str, description: &str) -> Response {
    Response::json(status, &json!({ "error": error, "error_description": description }))
}

/// A refusal as WorkOS' API answers one (its `code`, and a `message`).
fn pipes_problem(status: u16, code: &str, message: &str) -> Response {
    Response::json(status, &json!({ "code": code, "message": message }))
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

/// What the stand-in provider's connection asks for (`connection_oauth`).
pub const CONNECTION_SCOPES: [&str; 3] = ["mail.read", "files.read", "openid"];

impl WorkOs {
    pub fn connection_oauth(&self) -> Value {
        json!({ "authorize": format!("{}/test-oauth/authorize", self.url), "exchange": format!("{}/test-oauth/token", self.url), "client_id": "mock-connection-client", "scopes": CONNECTION_SCOPES, "revoke": format!("{}/test-oauth/revoke", self.url) })
    }

    /// The next code exchange answers no ID token, as a provider asked for no `openid`.
    pub fn omit_next_id_token(&self) { self.state.lock().expect("workos state").omit_id_token = true; }

    /// The next import is answered 504, as one whose outcome its caller
    /// cannot know, and lands only at `land_deferred_import`.
    pub fn defer_next_import(&self) { self.state.lock().expect("workos state").defer_import = true; }

    /// The deferred import lands now, late: whether there was one.
    pub fn land_deferred_import(&self) -> bool {
        let mut s = self.state.lock().expect("workos state");
        let Some(import) = s.deferred.take() else { return false };
        s.land(import).is_some()
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
    /// Whether Pipes holds an account of `email`'s at `provider`.
    pub fn holds(&self, email: &str, provider: &str) -> bool {
        !self.accounts(email, provider).is_empty()
    }

    /// `email`'s accounts at `provider`, oldest first.
    pub fn accounts(&self, email: &str, provider: &str) -> Vec<Connected> {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        s.accounts.get(&(user.id, provider.to_string())).map(|l| l.iter().map(Account::read).collect()).unwrap_or_default()
    }

    /// The account a token Pipes minted was for (its id).
    pub fn token_account(&self, token: &str) -> Option<String> {
        self.state.lock().expect("workos state").pipes_tokens.get(token).map(|t| t.2.clone())
    }

    /// Another account of `email`'s at `provider`, connected (a standard
    /// one, as an `add` makes it), named `identifier` and labeled `label`:
    /// its id.
    pub fn add_account(&self, email: &str, provider: &str, identifier: &str, label: &str) -> String {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        let a = s.made(&user.id, provider, false);
        a.identifier = Some(identifier.to_string());
        a.label = Some(label.to_string());
        a.id.clone()
    }

    /// While `refuse` holds, an `add` for a person with an account at the
    /// provider is 404 `multiple_connections_unavailable`, as Pipes answers
    /// until WorkOS enables additional connections.
    pub fn refuse_additional_accounts(&self, refuse: bool) { self.state.lock().expect("workos state").refuse_additional = refuse; }

    /// A person connects at most `n` accounts at a provider (None: Pipes' 25).
    pub fn limit_accounts(&self, n: Option<usize>) { self.state.lock().expect("workos state").account_limit = n; }

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
                ("POST", "/test-oauth/token") => {
                    let code = url::form_urlencoded::parse(&req.body).find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()).unwrap_or_default();
                    gated.pass(&[Gate::Exchange, Gate::ExchangeOf(code)]);
                }
                ("POST" | "PUT", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => gated.pass(&[Gate::Import]),
                _ => {}
            }
            let mut s = st.lock().expect("workos state");
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/test-oauth/authorize") => {
                    if q("client_id") != "mock-connection-client" || q("response_type") != "code" || q("code_challenge_method") != "S256" { return problem(400, "invalid_request", "OAuth parameters"); }
                    let Ok(mut back) = url::Url::parse(&q("redirect_uri")) else { return problem(400, "invalid_request", "callback"); };
                    if q("decline") != "yes" {
                        let code = s.next("oauth_code");
                        // the boxes the person left ticked: all, unless the test says
                        let granted = match req.query.get("grant") {
                            Some(grant) => q("scope").split(' ').filter(|s| grant.split(' ').any(|g| g == *s)).collect::<Vec<_>>().join(" "),
                            None => q("scope"),
                        };
                        s.oauth_codes.insert(code.clone(), (q("code_challenge"), q("redirect_uri"), granted, q("account")));
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
                    let mut grant = json!({ "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 3600, "scope": scopes });
                    // OpenID Connect's ID token beside the grant when `openid` was asked
                    // for, unsigned as the fake's AuthKit tokens are: its account's
                    // email, for this client
                    if scopes.split(' ').any(|s| s == "openid") && !std::mem::take(&mut s.omit_id_token) {
                        let b64 = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
                        let claims = json!({ "iss": "https://oauth.test", "aud": "mock-connection-client", "sub": account, "email": format!("{account}@oauth.test"), "email_verified": true });
                        grant["id_token"] = format!("{}.{}.fake", b64(json!({ "alg": "none", "typ": "JWT" })), b64(claims)).into();
                    }
                    s.oauth_grants.insert(access.clone(), (refresh.clone(), account));
                    Response::json(200, &grant)
                }
                // RFC 7009 as Google answers it: a token Pipes or the provider handed out, or 400
                ("POST", "/test-oauth/revoke") => {
                    if std::mem::take(&mut s.fail_revoke) { return problem(503, "unavailable", "test revocation failure"); }
                    let form: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
                    let token = form.get("token").cloned().unwrap_or_default();
                    let ours = s.tokens.values().flatten().any(|t| *t == token) || s.oauth_grants.contains_key(&token);
                    if form.len() != 1 || !ours { return problem(400, "invalid_token", "not a token handed out here"); }
                    s.revoke(&token);
                    s.revoked.push(token);
                    Response::json(200, &json!({}))
                }
                ("POST" | "PUT", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => {
                    s.import_attempts += 1;
                    if std::mem::take(&mut s.fail_import) { return problem(503, "unavailable", "test import failure"); }
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) { return problem(401, "unauthorized", "API key"); }
                    let Some((user, provider)) = p["/user_management/users/".len()..].split_once("/connected_accounts/") else { return problem(404, "not_found", "route"); };
                    if !s.users.iter().any(|u| u.id == user) { return problem(404, "not_found", "user"); }
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let listed = s.accounts.get(&(user.to_string(), provider.to_string())).cloned().unwrap_or_default();
                    // which account it lands on: the intent a POST's body
                    // names; the account a PUT's query names, as the SDK sends
                    let target = match (req.method.as_str(), body["connection_intent"].as_str(), req.query.get("connected_account_id")) {
                        (_, Some(intent), _) if intent != "add" => return pipes_problem(422, "invalid_connection_intent", "a body's intent is add"),
                        ("POST", _, Some(_)) => return pipes_problem(422, "invalid_request", "an import names no account"),
                        ("POST", None, None) if listed.iter().any(|a| a.compatibility) => return problem(409, "conflict", "already exists"),
                        ("POST", None, None) => Target::Compatibility,
                        ("POST", Some(_), None) if s.refuse_additional && !listed.is_empty() => return pipes_problem(404, "multiple_connections_unavailable", "Additional connections are not available for this environment."),
                        ("POST", Some(_), None) if listed.len() >= s.account_limit.unwrap_or(ACCOUNTS_LIMIT) => return pipes_problem(409, "connected_account_limit_reached", "This user has reached the connected account limit for this provider."),
                        ("POST", Some(_), None) => Target::Add,
                        (_, Some(_), _) => return pipes_problem(422, "invalid_connection_intent", "do not send add intent to PUT"),
                        (_, None, named) => match (named, req.query.get("connection_intent").map(String::as_str)) {
                            (None, Some(_)) => return pipes_problem(422, "invalid_request", "an intent needs the account it names"),
                            (_, Some(intent)) if intent != "reauthorize" => return pipes_problem(422, "invalid_connection_intent", "an update's intent is reauthorize"),
                            (None, None) => Target::Compatibility,
                            (Some(id), _) if listed.iter().any(|a| a.id == *id) => Target::Named(id.clone()),
                            (Some(_), _) => return pipes_problem(404, "not_found", "no such connected account"),
                        },
                    };
                    let Some((refresh, account)) = s.oauth_grants.get(body["access_token"].as_str().unwrap_or("")).cloned() else { return problem(422, "invalid_tokens", "not issued here"); };
                    if body["refresh_token"] != refresh || !body["expires_at"].is_string() || body["state"] != "connected" { return problem(422, "invalid_tokens", "token combination"); }
                    let scopes = serde_json::from_value::<Vec<String>>(body["scopes"].clone()).ok();
                    let identifier = body["account_identifier"].as_str().map(str::to_string);
                    let import = Import { user: user.into(), provider: provider.into(), label: body.get("account_display_name").cloned(), scopes, identifier, target, account };
                    if std::mem::take(&mut s.defer_import) {
                        s.deferred = Some(import);
                        return problem(504, "gateway_timeout", "test import deferred: it lands later");
                    }
                    match s.land(import) {
                        Some(made) => Response::json(if req.method == "POST" { 201 } else { 200 }, &made),
                        None => pipes_problem(404, "not_found", "no such connected account"),
                    }
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
                        s.compatibility(&user, &provider).authorized = true;
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
                    match s.named(user, provider, req.query.get("connected_account_id").map(String::as_str)) {
                        None => problem(404, "not_found", "no connected account"),
                        Some(a) => Response::json(200, &a.json(user, provider)),
                    }
                }
                // a person's providers, each with every account of theirs
                // there (with the plural opt-in), oldest first
                ("GET", p) if p.starts_with("/user_management/users/") && p.ends_with("/data_providers") => {
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let user = &p["/user_management/users/".len()..p.len() - "/data_providers".len()];
                    if !s.users.iter().any(|u| u.id == user) { return pipes_problem(404, "not_found", "user"); }
                    s.states_read += 1;
                    let plural = q("supports_multiple_connections") == "true";
                    let mut data: Vec<Value> = s.accounts.iter().filter(|((u, _), _)| u == user).map(|((_, provider), list)| {
                        let compatibility = list.iter().find(|a| a.compatibility).map(|a| a.json(user, provider));
                        let mut entry = json!({ "object": "data_provider", "slug": provider, "connection_owner": "user", "connected_account": compatibility });
                        if plural {
                            entry["connected_accounts"] = list.iter().map(|a| a.json(user, provider)).collect();
                        }
                        entry
                    }).collect();
                    data.sort_by(|a, b| a["slug"].as_str().cmp(&b["slug"].as_str()));
                    Response::json(200, &json!({ "object": "list", "data": data, "list_metadata": {} }))
                }
                // a connected account deleted: its tokens go; 404 when there is none
                ("DELETE", p) if p.starts_with("/user_management/users/") && p.contains("/connected_accounts/") => {
                    if req.header("authorization") != Some(format!("Bearer {key}").as_str()) {
                        return problem(401, "unauthorized", "the API key is wrong");
                    }
                    let Some((user, provider)) = p["/user_management/users/".len()..].split_once("/connected_accounts/") else { return problem(404, "not_found", "no such route") };
                    let Some(id) = s.named(user, provider, req.query.get("connected_account_id").map(String::as_str)).map(|a| a.id.clone()) else { return problem(404, "not_found", "no connected account") };
                    let key = (user.to_string(), provider.to_string());
                    let list = s.accounts.get_mut(&key).expect("named");
                    list.retain(|a| a.id != id);
                    if list.is_empty() {
                        s.accounts.remove(&key);
                    }
                    s.deleted.push(key);
                    Response::bytes(204, "text/plain", vec![])
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
                    // the account it names; with the plural opt-in and none
                    // named, the one there is, or a refusal while several
                    let named = body["connected_account_id"].as_str();
                    let listed = s.accounts.get(&(user.clone(), provider.clone())).map(Vec::len).unwrap_or(0);
                    if named.is_none() && body["supports_multiple_connections"] == true && listed > 1 {
                        return pipes_problem(409, "account_selection_required", &format!("Several connected accounts match this user for '{provider}'. Name one with a connected account id."));
                    }
                    let account = match (named, body["supports_multiple_connections"] == true) {
                        (None, true) => s.accounts.get(&(user.clone(), provider.clone())).and_then(|l| l.first()).cloned(),
                        (named, _) => s.named(&user, &provider, named).cloned(),
                    };
                    match account.as_ref().map(|a| a.authorized) {
                        None => Response::json(200, &json!({ "active": false, "error": "not_installed" })),
                        Some(false) => Response::json(200, &json!({ "active": false, "error": "needs_reauthorization" })),
                        Some(true) => {
                            let account = account.expect("matched");
                            let token = s.next(&format!("pipes_{provider}"));
                            s.pipes_tokens.insert(token.clone(), (user.clone(), provider.clone(), account.id.clone()));
                            // what the account holds, and what of the connection's asks it lacks
                            let scopes = account.scopes.clone().unwrap_or_default();
                            let missing: Vec<&str> = if scopes.is_empty() { vec![] } else { CONNECTION_SCOPES.into_iter().filter(|c| !scopes.iter().any(|s| s == c)).collect() };
                            s.tokens.entry(provider).or_default().push(token.clone());
                            let expires = crate::codestorage::iso(crate::codestorage::now_ms() + 3_600_000);
                            Response::json(
                                200,
                                &json!({ "active": true, "access_token": { "object": "access_token", "access_token": token, "expires_at": expires, "scopes": scopes, "missing_scopes": missing } }),
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
        s.compatibility(&user.id, provider).authorized = authorized;
    }

    /// `email`'s account at `provider` is labeled `label` (its
    /// `account_display_name`), as a connect through the platform sets it.
    pub fn label(&self, email: &str, provider: &str, label: &str) {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        s.compatibility(&user.id, provider).label = Some(label.to_string());
    }

    /// `email`'s account at `provider` holds `scopes` (its `scopes`), as a
    /// connect through the platform imports what the person allowed.
    pub fn grant(&self, email: &str, provider: &str, scopes: &[&str]) {
        let mut s = self.state.lock().expect("workos state");
        let user = s.user_for(email);
        s.compatibility(&user.id, provider).scopes = Some(scopes.iter().map(|s| s.to_string()).collect());
    }

    /// The scopes `email`'s account at `provider` holds, as imported
    /// (empty: none named): its compatibility connection's, else its oldest's.
    pub fn scopes(&self, email: &str, provider: &str) -> Vec<String> {
        let accounts = self.accounts(email, provider);
        accounts.iter().find(|a| a.compatibility).or(accounts.first()).map(|a| a.scopes.clone()).unwrap_or_default()
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

#[cfg(test)]
mod tests;
