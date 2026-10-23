//! Authorization-code connections: only the browser-bound callback exchanges
//! a code, then hands the tokens to Pipes. No provider refresh logic lives here.
//! This small URL/form codec uses existing url/serde/PKCE primitives because
//! the cell uses Workers Fetch, not a native OAuth client's HTTP transport.

use crate::catalog::{self, ConnectionOAuth};
use serde_json::{json, Value};

pub const SCOPES_MAX: usize = 32;
pub const SCOPE_MAX_BYTES: usize = 256;
pub const TOKEN_MAX_BYTES: usize = 8192;
pub const PENDING_PER_SESSION_MAX: u64 = 8;
/// One synchronous SQLite statement: a mismatched callback consumes nothing;
/// the winner consumes the transaction before the caller awaits any vendor.
pub const TAKE_PENDING: &str = "DELETE FROM connection_signins WHERE state = ? AND session = ? AND provider = ? AND expires_at > ? RETURNING issuer, subject, config, sealed, expires_at";
/// A disconnect's end of every attempt at its account still pending, each
/// session's: (issuer, subject, provider). No index has the subject: it
/// reads at most the fleet's pending sign-ins (`signins_pending_max`).
pub const FENCE_PENDING: &str = "DELETE FROM connection_signins WHERE issuer = ? AND subject = ? AND provider = ?";
/// A disconnect's lasting fence of its account (issuer, subject, provider,
/// when): kept until a connect of the person's is known to have imported
/// again (`UNFENCE`), so an import whose outcome was unknown that lands at
/// Pipes after the disconnect is never taken for a connection.
pub const FENCE: &str = "INSERT INTO connection_fences (issuer, subject, provider, fenced_at) VALUES (?, ?, ?, ?) ON CONFLICT (issuer, subject, provider) DO UPDATE SET fenced_at = excluded.fenced_at";
/// A connect of the account known to have imported (Pipes answered it
/// 200 or 201): its fence goes. Nothing else lifts one.
pub const UNFENCE: &str = "DELETE FROM connection_fences WHERE issuer = ? AND subject = ? AND provider = ?";
/// The providers a person's account is fenced at: (issuer, subject).
pub const FENCED: &str = "SELECT provider FROM connection_fences WHERE issuer = ? AND subject = ?";
const RESERVED: &[&str] = &["client_id", "redirect_uri", "response_type", "scope", "state", "code_challenge", "code_challenge_method", "code", "code_verifier", "client_secret"];

fn token(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len()) && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

pub fn check(o: &ConnectionOAuth) -> Result<(), String> {
    catalog::row_url(&o.authorize, catalog::Plain::Fake)?;
    catalog::row_url(&o.exchange, catalog::Plain::Fake)?;
    let a = url::Url::parse(&o.authorize).expect("checked");
    let e = url::Url::parse(&o.exchange).expect("checked");
    if a.query_pairs().any(|(k, _)| RESERVED.contains(&k.as_ref())) || e.query().is_some() {
        return Err("OAuth endpoints must not supply protocol parameters; exchange has no query".into());
    }
    if let Some(r) = &o.revoke {
        catalog::row_url(r, catalog::Plain::Fake)?;
        if url::Url::parse(r).expect("checked").query().is_some() {
            return Err("the revocation endpoint has no query: the token is its form's".into());
        }
    }
    if !token(&o.client_id, 256) || !(1..=SCOPES_MAX).contains(&o.scopes.len()) || o.scopes.iter().any(|s| !token(s, SCOPE_MAX_BYTES) || s.contains(['"', '\\'])) {
        return Err("a connection needs a client id and 1–32 bounded OAuth scopes".into());
    }
    Ok(())
}

pub fn authorize_url(o: &ConnectionOAuth, callback: &str, challenge: &str, state: &str) -> String {
    let mut u = url::Url::parse(&o.authorize).expect("catalog checked");
    u.query_pairs_mut().append_pair("client_id", &o.client_id).append_pair("redirect_uri", callback)
        .append_pair("response_type", "code").append_pair("scope", &o.scopes.join(" "))
        .append_pair("state", state).append_pair("code_challenge", challenge).append_pair("code_challenge_method", "S256");
    u.into()
}

pub fn exchange_form(o: &ConnectionOAuth, callback: &str, code: &str, verifier: &str, secret: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new()).append_pair("grant_type", "authorization_code")
        .append_pair("client_id", &o.client_id).append_pair("client_secret", secret).append_pair("redirect_uri", callback)
        .append_pair("code", code).append_pair("code_verifier", verifier).finish()
}

/// A revocation's form (RFC 7009 §2.1; Google documents `token` alone).
pub fn revoke_form(token: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new()).append_pair("token", token).finish()
}

/// The scopes that only sign a person in: OpenID Connect's `openid`,
/// `email` and `profile`, and Google's long names for the last two.
/// Google grants them beside whatever else the person allows (its granular
/// consent offers no box for them), so a grant of these alone lets an
/// agent do nothing (`tokens` refuses it), and none is ever missing.
pub const IDENTITY_SCOPES: [&str; 5] = ["openid", "email", "profile", "https://www.googleapis.com/auth/userinfo.email", "https://www.googleapis.com/auth/userinfo.profile"];

fn identity(scope: &str) -> bool {
    IDENTITY_SCOPES.contains(&scope)
}

/// A grant's scopes, held as a grant may hold them: 1 to `SCOPES_MAX`
/// bounded tokens.
fn bounded(scopes: Vec<String>) -> Option<Vec<String>> {
    ((1..=SCOPES_MAX).contains(&scopes.len()) && scopes.iter().all(|s| token(s, SCOPE_MAX_BYTES))).then_some(scopes)
}

/// Tokens are deliberately not Debug. Errors contain no vendor response.
/// A refresh token is required: never import a short-lived grant.
pub struct Tokens {
    access: String,
    refresh: String,
    pub expires_in: u32,
    scopes: Vec<String>,
}

/// Why a token endpoint's answer is not imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// No bounded bearer grant with a refresh token.
    Invalid,
    /// A grant of nothing the row asks for beyond signing in: the person
    /// left every other box unticked.
    NoScopes,
}

pub fn tokens(answer: &Value, requested: &[String]) -> Result<Tokens, Refused> {
    use Refused::Invalid;
    let access = answer["access_token"].as_str().filter(|s| token(s, TOKEN_MAX_BYTES)).ok_or(Invalid)?;
    let refresh = answer["refresh_token"].as_str().filter(|s| token(s, TOKEN_MAX_BYTES)).ok_or(Invalid)?;
    if !answer["token_type"].as_str().ok_or(Invalid)?.eq_ignore_ascii_case("bearer") { return Err(Invalid); }
    let expires_in = answer["expires_in"].as_u64().filter(|s| (1..=86_400).contains(s)).ok_or(Invalid)? as u32;
    // RFC 6749: omitted scope means the requested scope. A partial grant is
    // kept: the person may untick what they will not allow (Google's
    // granular consent), and Pipes holds exactly what the provider granted.
    let scopes = match answer.get("scope") {
        None => requested.to_vec(),
        Some(s) => {
            let s = s.as_str().filter(|s| s.len() <= SCOPES_MAX * (SCOPE_MAX_BYTES + 1)).ok_or(Invalid)?;
            bounded(s.split(' ').map(str::to_string).collect()).ok_or(Invalid)?
        }
    };
    if requested.iter().any(|s| !identity(s)) && !scopes.iter().any(|s| !identity(s) && requested.contains(s)) {
        return Err(Refused::NoScopes);
    }
    Ok(Tokens { access: access.into(), refresh: refresh.into(), expires_in, scopes })
}

/// What a connection's row asks for that its grant lacks, in the row's
/// order: each requested scope not granted, never a sign-in one. Settings
/// names them, and offers a reconnect to add them.
pub fn missing(requested: &[String], granted: &[String]) -> Vec<String> {
    requested.iter().filter(|s| !identity(s) && !granted.contains(s)).cloned().collect()
}

/// The scopes a connected account holds, as Pipes says of it (its
/// `scopes`, which the import set: `Tokens::import_body`), held to what a
/// grant may hold. None: it names none, or an unbounded list, so nothing
/// is said to be missing.
pub fn pipes_scopes(account: &Value) -> Option<Vec<String>> {
    let scopes = account["scopes"].as_array()?.iter().map(|s| s.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
    bounded(scopes)
}

impl Tokens {
    /// WorkOS's documented connected-account import/update body. `account`
    /// is its label (`account_label`); `null` clears an earlier account's
    /// on an update.
    pub fn import_body(&self, expires_at: &str, account: Option<&str>) -> Value {
        json!({ "access_token": self.access, "refresh_token": self.refresh, "expires_at": expires_at, "scopes": self.scopes, "state": "connected", "account_display_name": account })
    }
}

/// Whose connection an attempt is: a WorkOS user's (`issuer`, `subject`)
/// account at `provider`, as its pending row names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub issuer: String,
    pub subject: String,
    pub provider: String,
}

/// The callbacks past their pending row (`TAKE_PENDING` spent it) and not
/// yet answered, which a disconnect fences (`fence`) as it ends the rows
/// still pending (`FENCE_PENDING`), so that no attempt begun before it
/// connects the account after it. One still exchanging its code is
/// cancelled: `may_import` says no, and it imports nothing (the grant its
/// exchange made is not revoked: the provider's revocation ends its whole
/// grant to the client, a newer connect's with it, and this one's tokens
/// are kept nowhere). One importing refuses the fence instead, since what
/// it sent Pipes cannot be called back, and a delete after it could as
/// well remove a newer connect's. An import that answered with no known
/// outcome is no longer under way, and the lasting fence (`FENCE`) holds
/// against it should it land later.
/// The Registry keeps it in memory, not a table: each finish runs inside
/// the Registry and ends with it, so the one a restart starts has none
/// under way. It holds one entry per finish under way, each gone at its
/// answer (an exchange and an import, each within its vendor's timeout).
#[derive(Debug, Default)]
pub struct Finishing {
    next: u64,
    under_way: Vec<Finish>,
}

#[derive(Debug)]
struct Finish {
    id: u64,
    account: Account,
    step: Step,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Exchanging,
    /// A fence came while it exchanged: it imports nothing.
    Cancelled,
    Importing,
}

/// A fence refused: a callback is importing into the account now, which
/// the person's next disconnect, once it answers, fences.
#[derive(Debug, PartialEq, Eq)]
pub struct Importing;

impl Finishing {
    /// A callback that just spent its pending row, under way: its id, for
    /// `may_import` and `end`.
    pub fn begin(&mut self, account: Account) -> u64 {
        self.next += 1;
        self.under_way.push(Finish { id: self.next, account, step: Step::Exchanging });
        self.next
    }

    /// Whether callback `id` may import now: no fence came since it began.
    /// From a yes it is importing, and each fence of its account is
    /// refused until it `end`s.
    pub fn may_import(&mut self, id: u64) -> bool {
        let f = self.under_way.iter_mut().find(|f| f.id == id).expect("a callback asks while it is under way");
        assert!(f.step != Step::Importing, "a callback imports once");
        let may = f.step == Step::Exchanging;
        if may {
            f.step = Step::Importing;
        }
        may
    }

    /// Callback `id` answered, whatever it answered.
    pub fn end(&mut self, id: u64) {
        let before = self.under_way.len();
        self.under_way.retain(|f| f.id != id);
        assert_eq!(self.under_way.len() + 1, before, "a callback ends once");
    }

    /// A disconnect of `account`: each of its callbacks still exchanging
    /// is cancelled (how many), unless one is importing, which refuses the
    /// fence and changes nothing. Another account's are left as they are.
    pub fn fence(&mut self, account: &Account) -> Result<usize, Importing> {
        if self.under_way.iter().any(|f| f.account == *account && f.step == Step::Importing) {
            return Err(Importing);
        }
        let mut cancelled = 0;
        for f in self.under_way.iter_mut().filter(|f| f.account == *account && f.step == Step::Exchanging) {
            f.step = Step::Cancelled;
            cancelled += 1;
        }
        Ok(cancelled)
    }
}

/// An account label, at most (an email address's longest).
pub const ACCOUNT_LABEL_MAX_BYTES: usize = 254;

/// Whether `label` is one a person may be shown as their account's: no
/// control character, nor one that reorders the text around it (a bidi
/// override or isolate, which could show another address than it is).
pub fn label_ok(label: &str) -> bool {
    let reorders = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    (1..=ACCOUNT_LABEL_MAX_BYTES).contains(&label.len()) && !label.chars().any(|c| c.is_control() || reorders(c))
}

/// The account a grant is for, as its person knows it: the email of the
/// OpenID Connect ID token the token endpoint answered beside the grant
/// (when the connection asks for `openid` and its provider's email scope,
/// Google's `https://www.googleapis.com/auth/userinfo.email`). Its
/// signature is not checked: it came straight from the provider's token
/// endpoint over TLS (OpenID Connect Core 3.1.3.7, step 6), it must name
/// this client as its audience, and the label is only ever shown to its
/// person, never trusted for access. None: no ID token, or no verified
/// email in it.
pub fn account_label(answer: &Value, client_id: &str) -> Option<String> {
    use base64::Engine;
    let id_token = answer["id_token"].as_str().filter(|t| t.len() <= TOKEN_MAX_BYTES)?;
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    let ours = claims["aud"] == client_id || claims["aud"].as_array().is_some_and(|a| a.iter().any(|x| x == client_id));
    // some providers send the boolean as a string
    if !ours || claims["email_verified"] == false || claims["email_verified"] == "false" {
        return None;
    }
    claims["email"].as_str().filter(|e| label_ok(e)).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionOAuth {
        ConnectionOAuth { authorize: "https://accounts.example/authorize?access_type=offline&prompt=consent".into(), exchange: "https://accounts.example/token".into(), client_id: "client".into(), scopes: vec!["mail.read".into(), "files.read".into()], revoke: None }
    }

    fn id_token(claims: Value) -> Value {
        use base64::Engine;
        let b64 = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        json!({ "id_token": format!("{}.{}.signature", b64(&json!({ "alg": "RS256" })), b64(&claims)) })
    }

    #[test]
    fn an_account_label_is_a_verified_email_of_an_id_token_for_this_client() {
        let good = json!({ "aud": "client", "email": "me@example.com", "email_verified": true });
        assert_eq!(account_label(&id_token(good.clone()), "client").as_deref(), Some("me@example.com"));
        assert_eq!(account_label(&id_token(json!({ "aud": ["other", "client"], "email": "me@example.com" })), "client").as_deref(), Some("me@example.com"), "an audience list naming this client");
        let unlabeled = |claims: Value| account_label(&id_token(claims), "client").is_none();
        assert!(unlabeled(json!({ "aud": "another-client", "email": "me@example.com" })), "another client's token");
        assert!(unlabeled(json!({ "email": "me@example.com" })), "no audience");
        assert!(unlabeled(json!({ "aud": "client", "email": "me@example.com", "email_verified": false })), "an unverified email");
        assert!(unlabeled(json!({ "aud": "client", "email": "me@example.com", "email_verified": "false" })), "an unverified email, as a string");
        assert!(unlabeled(json!({ "aud": "client", "email": "moc.elpmaxe@\u{202e}me" })), "a bidi override");
        assert!(unlabeled(json!({ "aud": "client", "email": "me\u{2066}@example.com" })), "a bidi isolate");
        assert!(unlabeled(json!({ "aud": "client", "email": "" })), "an empty email");
        assert!(unlabeled(json!({ "aud": "client", "email": "me\n@example.com" })), "a control character");
        assert!(unlabeled(json!({ "aud": "client", "email": format!("{}@example.com", "x".repeat(ACCOUNT_LABEL_MAX_BYTES)) })), "too long");
        assert!(unlabeled(json!({ "aud": "client", "email": 7 })), "not a string");
        for answer in [json!({}), json!({ "id_token": "not-a-jwt" }), json!({ "id_token": "a.!!!.c" }), json!({ "id_token": 1 }), json!({ "id_token": format!("a.{}.c", "x".repeat(TOKEN_MAX_BYTES)) })] {
            assert!(account_label(&answer, "client").is_none(), "{answer}");
        }
    }

    #[test]
    fn a_revocation_endpoint_is_checked_as_the_others_and_absent_from_the_config_hash() {
        let mut o = config();
        let without = serde_json::to_string(&o).unwrap();
        assert!(!without.contains("revoke"), "a row without one hashes as it did before revocation was offered");
        o.revoke = Some("https://accounts.example/revoke".into());
        assert!(check(&o).is_ok());
        for bad in ["http://attacker.example/revoke", "https://accounts.example/revoke?token=fixed", "not a url"] {
            o.revoke = Some(bad.into());
            assert!(check(&o).is_err(), "{bad}");
        }
        let form: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(revoke_form("a+b/c=").as_bytes()).into_owned().collect();
        assert_eq!(form, std::collections::BTreeMap::from([("token".to_string(), "a+b/c=".to_string())]));
    }

    #[test]
    fn the_durable_transaction_is_session_provider_bound_and_spent_once() {
        use rusqlite::{Connection, params};
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(crate::migrate::REGISTRY.steps[1].sql).unwrap();
        for (state, session, provider, expiry) in [("a", "session-a", "mail", 100), ("b", "session-b", "drive", 100), ("old", "session-a", "mail", 50)] {
            db.execute("INSERT INTO connection_signins VALUES (?, ?, ?, 'issuer', 'user', 'config', 'sealed', ?)", params![state, session, provider, expiry]).unwrap();
        }
        let take = |state: &str, session: &str, provider: &str, now: i64| {
            let mut q = db.prepare(TAKE_PENDING).unwrap();
            let taken = q.query(params![state, session, provider, now]).unwrap().next().unwrap().is_some();
            taken
        };
        assert!(!take("a", "session-b", "mail", 50), "another user/session cannot take it");
        assert!(!take("a", "another-session-a", "mail", 50), "same identity in another session is insufficient");
        assert!(!take("a", "session-a", "drive", 50), "another provider cannot take it");
        assert!(!take("old", "session-a", "mail", 50), "expiry is exclusive");
        assert!(take("a", "session-a", "mail", 50), "the correct attempt survives mismatches");
        assert!(!take("a", "session-a", "mail", 50), "replay has no transaction");
        assert!(take("b", "session-b", "drive", 50), "another person's transaction was untouched");
    }

    #[test]
    fn a_disconnect_ends_every_pending_attempt_at_its_account_and_no_other() {
        use rusqlite::{Connection, params};
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(crate::migrate::REGISTRY.steps[1].sql).unwrap();
        for (state, session, issuer, subject, provider) in [
            ("a1", "session-a", "issuer", "user-a", "mail"),
            ("a2", "another-session-a", "issuer", "user-a", "mail"),
            ("a-drive", "session-a", "issuer", "user-a", "drive"),
            ("b", "session-b", "issuer", "user-b", "mail"),
            ("a-elsewhere", "session-a", "another-issuer", "user-a", "mail"),
        ] {
            db.execute("INSERT INTO connection_signins VALUES (?, ?, ?, ?, ?, 'config', 'sealed', 100)", params![state, session, provider, issuer, subject]).unwrap();
        }
        let fence = |issuer: &str, subject: &str, provider: &str| db.execute(FENCE_PENDING, params![issuer, subject, provider]).unwrap();
        let take = |state: &str, session: &str, provider: &str| db.prepare(TAKE_PENDING).unwrap().query(params![state, session, provider, 50]).unwrap().next().unwrap().is_some();
        assert_eq!(fence("issuer", "user-a", "mail"), 2, "each of the account's sessions' attempts");
        assert!(!take("a1", "session-a", "mail") && !take("a2", "another-session-a", "mail"), "a callback after the disconnect has no transaction");
        assert_eq!(fence("issuer", "user-a", "mail"), 0, "a second disconnect finds none");
        assert!(take("a-drive", "session-a", "drive"), "the person's attempt at another provider is untouched");
        assert!(take("b", "session-b", "mail"), "another person's attempt at the provider is untouched");
        assert!(take("a-elsewhere", "session-a", "mail"), "another issuer's user is another account");
    }

    #[test]
    fn a_lasting_fence_stands_until_a_known_import_and_only_for_its_account() {
        use rusqlite::{Connection, params};
        let db = Connection::open_in_memory().unwrap();
        for step in crate::migrate::REGISTRY.steps {
            db.execute_batch(step.sql).unwrap();
        }
        let fenced = |issuer: &str, subject: &str| -> Vec<String> {
            let mut q = db.prepare(FENCED).unwrap();
            let mut providers: Vec<String> = q.query_map(params![issuer, subject], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            providers.sort();
            providers
        };
        db.execute(FENCE, params!["issuer", "user-a", "mail", 10]).unwrap();
        db.execute(FENCE, params!["issuer", "user-a", "mail", 20]).unwrap();
        db.execute(FENCE, params!["issuer", "user-a", "drive", 30]).unwrap();
        assert_eq!(fenced("issuer", "user-a"), ["drive", "mail"], "a second disconnect keeps one fence, at its later time");
        assert_eq!(db.query_row("SELECT fenced_at FROM connection_fences WHERE provider = 'mail'", [], |r| r.get::<_, i64>(0)).unwrap(), 20);
        assert!(fenced("issuer", "user-b").is_empty() && fenced("another-issuer", "user-a").is_empty(), "another person's account is not fenced");
        assert_eq!(db.execute(UNFENCE, params!["issuer", "user-a", "mail"]).unwrap(), 1, "a known import lifts its provider's fence");
        assert_eq!(fenced("issuer", "user-a"), ["drive"], "and no other provider's");
        assert_eq!(db.execute(UNFENCE, params!["issuer", "user-a", "mail"]).unwrap(), 0);
    }

    fn account(subject: &str, provider: &str) -> Account {
        Account { issuer: "issuer".into(), subject: subject.into(), provider: provider.into() }
    }

    #[test]
    fn a_fence_cancels_the_accounts_exchanging_callbacks_and_waits_for_an_importing_one() {
        let (a, b, a_drive) = (account("user-a", "mail"), account("user-b", "mail"), account("user-a", "drive"));
        let mut f = Finishing::default();
        let (a1, a2, b1, d1) = (f.begin(a.clone()), f.begin(a.clone()), f.begin(b.clone()), f.begin(a_drive.clone()));
        assert_eq!(f.fence(&a), Ok(2));
        assert!(!f.may_import(a1) && !f.may_import(a2), "a callback exchanging when its account was disconnected imports nothing");
        assert!(f.may_import(b1) && f.may_import(d1), "another person's, and another provider's, import");
        assert_eq!(f.fence(&a), Ok(0), "a cancelled callback holds no fence back");
        for id in [a1, a2, b1, d1] {
            f.end(id);
        }
        assert!(f.under_way.is_empty());

        let newer = f.begin(a.clone());
        assert!(f.may_import(newer), "a connect begun after the fence imports");
        let exchanging = f.begin(a.clone());
        assert_eq!(f.fence(&a), Err(Importing), "an import under way refuses the disconnect");
        assert_eq!(f.fence(&b), Ok(0), "and no other account's");
        assert!(f.may_import(exchanging), "a refused fence cancelled nothing");
        f.end(newer);
        assert_eq!(f.fence(&a), Err(Importing));
        f.end(exchanging);
        assert_eq!(f.fence(&a), Ok(0), "once the imports answered, the disconnect goes");
        assert!(f.under_way.is_empty());
        assert_eq!(Finishing::default().fence(&a), Ok(0), "a restarted Registry has none under way");
    }

    #[test]
    #[should_panic(expected = "a callback imports once")]
    fn a_callback_imports_once() {
        let mut f = Finishing::default();
        let id = f.begin(account("user-a", "mail"));
        assert!(f.may_import(id));
        f.may_import(id);
    }

    #[test]
    #[should_panic(expected = "a callback ends once")]
    fn a_callback_ends_once() {
        let mut f = Finishing::default();
        let id = f.begin(account("user-a", "mail"));
        f.end(id);
        f.end(id);
    }

    #[test]
    fn protocol_parameters_cannot_be_overridden_by_configuration() {
        let o = config();
        assert!(check(&o).is_ok());
        for key in RESERVED {
            let mut o = o.clone();
            o.authorize.push_str(&format!("&{key}=another"));
            assert!(check(&o).is_err(), "{key}");
        }
        let mut bad = o.clone();
        bad.exchange.push_str("?code=wrong");
        assert!(check(&bad).is_err());
        bad = o.clone();
        bad.scopes = vec!["scope with spaces".into()];
        assert!(check(&bad).is_err());
        bad.scopes = vec!["s".into(); SCOPES_MAX + 1];
        assert!(check(&bad).is_err());
        bad = o;
        bad.authorize = "http://attacker.example/authorize".into();
        assert!(check(&bad).is_err());
    }

    #[test]
    fn authorize_and_exchange_bind_the_same_client_callback_and_challenge() {
        let o = config();
        let callback = "https://platform.example/api/connections/mail/callback";
        let u = url::Url::parse(&authorize_url(&o, callback, "challenge", "state")).unwrap();
        let q: std::collections::BTreeMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["state"], "state");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "challenge");
        assert_eq!(q["redirect_uri"], callback);
        assert_eq!(q["client_id"], "client");
        assert_eq!(q["scope"], "mail.read files.read");
        assert_eq!(q["access_type"], "offline");
        let form = exchange_form(&o, callback, "c+/=&", "verifier", "fake-secret");
        let form: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(form.as_bytes()).into_owned().collect();
        assert_eq!(form["code"], "c+/=&");
        assert_eq!(form["code_verifier"], "verifier");
        assert_eq!(form["redirect_uri"], callback);
        assert_eq!(form["client_id"], "client");
        assert_eq!(form["grant_type"], "authorization_code");
    }

    const EMAIL: &str = "https://www.googleapis.com/auth/userinfo.email";

    fn requested() -> Vec<String> {
        ["mail.read", "files.read", "openid", EMAIL].map(String::from).to_vec()
    }

    fn granting(scope: &str) -> Value {
        json!({ "access_token": "mock-access", "refresh_token": "mock-refresh", "expires_in": 3600, "token_type": "Bearer", "scope": scope })
    }

    #[test]
    fn bounded_grants_are_imported_with_exactly_the_scopes_granted() {
        let requested = requested();
        let good = granting(&requested.join(" "));
        let got = tokens(&good, &requested).expect("complete grant");
        let body = got.import_body("2026-10-20T20:00:00Z", None);
        assert_eq!(body["state"], "connected");
        assert_eq!(body["scopes"], json!(requested));
        assert!(body.get("account_display_name").is_some_and(Value::is_null), "an update clears an earlier account's label");
        assert_eq!(got.import_body("2026-10-20T20:00:00Z", Some("me@example.com"))["account_display_name"], "me@example.com");
        let scopes = |scope: &str| tokens(&granting(scope), &requested).map(|t| t.import_body("2026-10-20T20:00:00Z", None)["scopes"].clone());
        assert_eq!(scopes("openid mail.read"), Ok(json!(["openid", "mail.read"])), "a partial grant, recorded as the provider answered it");
        assert_eq!(scopes("files.read"), Ok(json!(["files.read"])), "with no sign-in scope at all");
        for field in ["access_token", "refresh_token", "expires_in", "token_type"] {
            let mut bad = good.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert_eq!(tokens(&bad, &requested).err(), Some(Refused::Invalid), "missing {field}");
        }
        let too_long = format!("mail.read {}", "s".repeat(SCOPE_MAX_BYTES + 1));
        let too_many = vec!["mail.read"; SCOPES_MAX + 1].join(" ");
        for (field, value) in [("scope", json!(null)), ("scope", json!("")), ("scope", json!("mail.read  files.read")), ("scope", json!(too_long)), ("scope", json!(too_many)), ("expires_in", json!(0)), ("expires_in", json!(86401)), ("access_token", json!("x".repeat(TOKEN_MAX_BYTES + 1))), ("token_type", json!("MAC"))] {
            let mut bad = good.clone();
            bad[field] = value;
            assert_eq!(tokens(&bad, &requested).err(), Some(Refused::Invalid), "invalid {field}");
        }
        let mut omitted = good;
        omitted.as_object_mut().unwrap().remove("scope");
        assert_eq!(tokens(&omitted, &requested).map(|t| t.import_body("2026-10-20T20:00:00Z", None)["scopes"].clone()), Ok(json!(requested)), "RFC 6749: an omitted scope is the one requested");
    }

    #[test]
    fn a_grant_of_signing_in_alone_is_refused_as_no_scopes() {
        let requested = requested();
        for scope in ["openid", "openid https://www.googleapis.com/auth/userinfo.email", "openid email profile https://www.googleapis.com/auth/userinfo.profile", "calendar openid"] {
            assert_eq!(tokens(&granting(scope), &requested).err(), Some(Refused::NoScopes), "{scope}");
        }
        let signing_in: Vec<String> = ["openid", "email"].map(String::from).to_vec();
        assert!(tokens(&granting("openid"), &signing_in).is_ok(), "a row that asks for signing in alone takes any grant of it");
        assert_eq!(tokens(&granting("openid  email"), &signing_in).err(), Some(Refused::Invalid), "and still only a bounded one");
    }

    #[test]
    fn what_a_grant_lacks_is_what_its_row_asks_for_beyond_signing_in() {
        let requested = requested();
        let of = |granted: &[&str]| missing(&requested, &granted.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(of(&["openid", "mail.read"]), ["files.read"]);
        assert_eq!(of(&["mail.read", "files.read"]), Vec::<String>::new(), "sign-in's scopes are never missing");
        assert_eq!(of(&["openid"]), ["mail.read", "files.read"], "in the row's order");
        assert_eq!(of(&["calendar", "files.read", "mail.read", EMAIL, "openid"]), Vec::<String>::new());
        // what Pipes holds of a connected account, read back as a grant is
        assert_eq!(pipes_scopes(&json!({ "scopes": ["openid", "mail.read"] })), Some(vec!["openid".to_string(), "mail.read".to_string()]));
        let too_many = vec!["mail.read"; SCOPES_MAX + 1];
        for account in [json!({}), json!({ "scopes": [] }), json!({ "scopes": null }), json!({ "scopes": "mail.read" }), json!({ "scopes": ["mail.read", 7] }), json!({ "scopes": ["mail read"] }), json!({ "scopes": [""] }), json!({ "scopes": ["s".repeat(SCOPE_MAX_BYTES + 1)] }), json!({ "scopes": too_many })] {
            assert_eq!(pipes_scopes(&account), None, "{account}");
        }
    }

    /// Google's row asks it to show its account chooser, then its consent,
    /// and to keep what the person granted before, so a reconnect asks only
    /// for the rest (docs/pipes-browser-binding.md): the authorize URL's own
    /// query goes to Google as written, its space still one `prompt` of two
    /// values.
    #[test]
    fn a_fixed_prompt_of_two_values_reaches_the_provider_whole() {
        let mut o = config();
        o.authorize = "https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&include_granted_scopes=true&prompt=select_account%20consent".into();
        assert!(check(&o).is_ok());
        let u = url::Url::parse(&authorize_url(&o, "https://platform.example/api/connections/google/callback", "challenge", "state")).unwrap();
        let prompts: Vec<String> = u.query_pairs().filter(|(k, _)| k == "prompt").map(|(_, v)| v.into_owned()).collect();
        assert_eq!(prompts, ["select_account consent"]);
        assert_eq!(u.query_pairs().filter(|(k, v)| k == "include_granted_scopes" && v == "true").count(), 1);
    }
}
