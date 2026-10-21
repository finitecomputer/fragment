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
const RESERVED: &[&str] = &["client_id", "redirect_uri", "response_type", "scope", "state", "code_challenge", "code_challenge_method", "code", "code_verifier", "client_secret"];

fn token(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len()) && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

pub fn check(o: &ConnectionOAuth) -> Result<(), String> {
    catalog::row_url(&o.authorize, true)?;
    catalog::row_url(&o.exchange, true)?;
    let a = url::Url::parse(&o.authorize).expect("checked");
    let e = url::Url::parse(&o.exchange).expect("checked");
    if a.query_pairs().any(|(k, _)| RESERVED.contains(&k.as_ref())) || e.query().is_some() {
        return Err("OAuth endpoints must not supply protocol parameters; exchange has no query".into());
    }
    if let Some(r) = &o.revoke {
        catalog::row_url(r, true)?;
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

/// Tokens are deliberately not Debug. Errors contain no vendor response.
/// A refresh token is required: never import a short-lived or partial grant.
pub struct Tokens {
    access: String,
    refresh: String,
    pub expires_in: u32,
    scopes: Vec<String>,
}

pub fn tokens(answer: &Value, requested: &[String]) -> Option<Tokens> {
    let access = answer["access_token"].as_str().filter(|s| token(s, TOKEN_MAX_BYTES))?;
    let refresh = answer["refresh_token"].as_str().filter(|s| token(s, TOKEN_MAX_BYTES))?;
    if !answer["token_type"].as_str()?.eq_ignore_ascii_case("bearer") { return None; }
    let expires_in = answer["expires_in"].as_u64().filter(|s| (1..=86_400).contains(s))? as u32;
    // RFC 6749: omitted scope means the requested scope. An explicit partial
    // grant is refused; do not report scopes the provider did not grant.
    let scopes = match answer.get("scope") {
        None => requested.to_vec(),
        Some(s) => {
            let s = s.as_str().filter(|s| s.len() <= SCOPES_MAX * (SCOPE_MAX_BYTES + 1))?;
            let granted: Vec<String> = s.split(' ').map(str::to_string).collect();
            if granted.len() > SCOPES_MAX || granted.iter().any(|s| !token(s, SCOPE_MAX_BYTES)) || requested.iter().any(|s| !granted.contains(s)) { return None; }
            granted
        }
    };
    Some(Tokens { access: access.into(), refresh: refresh.into(), expires_in, scopes })
}

impl Tokens {
    /// WorkOS's documented connected-account import/update body.
    pub fn import_body(&self, expires_at: &str) -> Value {
        json!({ "access_token": self.access, "refresh_token": self.refresh, "expires_at": expires_at, "scopes": self.scopes, "state": "connected" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionOAuth {
        ConnectionOAuth { authorize: "https://accounts.example/authorize?access_type=offline&prompt=consent".into(), exchange: "https://accounts.example/token".into(), client_id: "client".into(), scopes: vec!["mail.read".into(), "files.read".into()], revoke: None }
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

    #[test]
    fn only_complete_bounded_grants_can_be_imported() {
        let requested = config().scopes;
        let good = json!({ "access_token": "mock-access", "refresh_token": "mock-refresh", "expires_in": 3600, "token_type": "Bearer", "scope": "mail.read files.read" });
        let got = tokens(&good, &requested).expect("complete grant");
        let body = got.import_body("2026-10-20T20:00:00Z");
        assert_eq!(body["state"], "connected");
        assert_eq!(body["scopes"], json!(requested));
        for field in ["access_token", "refresh_token", "expires_in", "token_type"] {
            let mut bad = good.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(tokens(&bad, &requested).is_none(), "missing {field}");
        }
        for (field, value) in [("scope", json!("mail.read")), ("scope", json!(null)), ("expires_in", json!(0)), ("expires_in", json!(86401)), ("access_token", json!("x".repeat(TOKEN_MAX_BYTES + 1))), ("token_type", json!("MAC"))] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(tokens(&bad, &requested).is_none(), "invalid {field}");
        }
        let mut omitted = good;
        omitted.as_object_mut().unwrap().remove("scope");
        assert!(tokens(&omitted, &requested).is_some(), "RFC 6749 omitted scope is unchanged");
    }

    /// Google's row asks it to show its account chooser, then its consent
    /// (docs/pipes-browser-binding.md): the authorize URL's own query goes to
    /// Google as written, its space still one `prompt` of two values.
    #[test]
    fn a_fixed_prompt_of_two_values_reaches_the_provider_whole() {
        let mut o = config();
        o.authorize = "https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&prompt=select_account%20consent".into();
        assert!(check(&o).is_ok());
        let u = url::Url::parse(&authorize_url(&o, "https://platform.example/api/connections/google/callback", "challenge", "state")).unwrap();
        let prompts: Vec<String> = u.query_pairs().filter(|(k, _)| k == "prompt").map(|(_, v)| v.into_owned()).collect();
        assert_eq!(prompts, ["select_account consent"]);
    }
}
