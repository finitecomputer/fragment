//! Pipes' several connections, as the fake answers them: a person's first
//! import is the compatibility connection, the one calls naming no account
//! reach; an `add` makes another; each is named by its id in the query of
//! a `GET`, `PUT` or `DELETE`, and in a token's body; a plural token naming
//! none is refused while several match; the plural list names each, oldest
//! first; and the refusals Pipes makes (one account for now, its limit).
use super::*;

const KEY: &str = "sk_test_fake";

struct Pipes {
    fake: WorkOs,
    http: reqwest::blocking::Client,
}

impl Pipes {
    fn start() -> Pipes {
        let fake = WorkOs::start("client_test", KEY).expect("the fake listens");
        let http = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        Pipes { fake, http }
    }

    /// (status, JSON) of a call with the API key.
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let url = format!("{}{path}", self.fake.url);
        let req = self.http.request(method.parse().unwrap(), url).header("authorization", format!("Bearer {KEY}"));
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(b.to_string()),
            None => req,
        };
        let r = req.send().unwrap();
        let status = r.status().as_u16();
        (status, serde_json::from_slice(&r.bytes().unwrap()).unwrap_or(Value::Null))
    }

    /// A grant of the stand-in provider's `account` (its code exchanged
    /// with its verifier): the import body Fragment sends for it.
    fn grant(&self, account: &str) -> Value {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let q = [("client_id", "mock-connection-client"), ("response_type", "code"), ("code_challenge_method", "S256"), ("code_challenge", challenge), ("redirect_uri", "http://127.0.0.1:9/cb"), ("scope", "mail.read openid"), ("state", "s"), ("account", account)];
        let r = self.http.get(format!("{}/test-oauth/authorize", self.fake.url)).query(&q).send().unwrap();
        let back = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        let code = back.query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
        let form = [("code", code.as_str()), ("code_verifier", verifier), ("redirect_uri", "http://127.0.0.1:9/cb"), ("client_id", "mock-connection-client"), ("client_secret", "mock-connection-secret"), ("grant_type", "authorization_code")];
        let t: Value = serde_json::from_slice(&self.http.post(format!("{}/test-oauth/token", self.fake.url)).form(&form).send().unwrap().bytes().unwrap()).unwrap();
        json!({ "access_token": t["access_token"], "refresh_token": t["refresh_token"], "expires_at": "2026-10-23T00:00:00Z", "scopes": ["mail.read", "openid"], "state": "connected", "account_identifier": account, "account_display_name": format!("{account}@oauth.test") })
    }
}

#[test]
fn a_persons_accounts_at_a_provider_are_each_named_by_id() {
    let p = Pipes::start();
    let user = p.fake.user("a@e2e.test").id;
    let path = format!("/user_management/users/{user}/connected_accounts/mail");

    let (status, first) = p.call("POST", &path, Some(p.grant("first")));
    assert_eq!((status, first["connection_role"].as_str()), (201, Some("compatibility")), "a first import with no intent: {first}");
    let first = first["id"].as_str().unwrap().to_string();
    assert_eq!(p.call("POST", &path, Some(p.grant("again"))).0, 409, "the compatibility connection is there");
    let mut add = p.grant("second");
    add["connection_intent"] = json!("add");
    let (status, second) = p.call("POST", &path, Some(add.clone()));
    assert_eq!((status, second["connection_role"].as_str(), second["account_identifier"].as_str()), (201, Some("standard"), Some("second")), "{second}");
    let second = second["id"].as_str().unwrap().to_string();

    let (status, list) = p.call("GET", &format!("/user_management/users/{user}/data_providers?supports_multiple_connections=true"), None);
    let entry = list["data"].as_array().unwrap().iter().find(|e| e["slug"] == "mail").cloned().unwrap();
    let ids: Vec<&str> = entry["connected_accounts"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!((status, ids, entry["connected_account"]["id"].as_str()), (200, vec![first.as_str(), second.as_str()], Some(first.as_str())), "oldest first, the compatibility one beside");
    let (_, legacy) = p.call("GET", &format!("/user_management/users/{user}/data_providers"), None);
    assert!(legacy["data"][0].get("connected_accounts").is_none(), "no plural opt-in, no plural list");

    let token = |body: Value| p.call("POST", "/data-integrations/mail/token", Some(body));
    let whose = |answer: &Value| p.fake.token_account(answer["access_token"]["access_token"].as_str().unwrap_or_default());
    let (_, compat) = token(json!({ "user_id": user }));
    assert_eq!(whose(&compat).as_deref(), Some(first.as_str()), "a legacy token is the compatibility connection's");
    let (status, refused) = token(json!({ "user_id": user, "supports_multiple_connections": true }));
    assert_eq!((status, refused["code"].as_str()), (409, Some("account_selection_required")), "several match: name one");
    let (_, named) = token(json!({ "user_id": user, "supports_multiple_connections": true, "connected_account_id": second }));
    assert_eq!(whose(&named).as_deref(), Some(second.as_str()), "the account named");

    let one = |query: &str| format!("{path}?{query}");
    let (status, got) = p.call("GET", &one(&format!("connected_account_id={second}&supports_multiple_connections=true")), None);
    assert_eq!((status, got["id"].as_str()), (200, Some(second.as_str())));
    let mut again = p.grant("second");
    again["account_display_name"] = json!("renamed@oauth.test");
    let (status, updated) = p.call("PUT", &one(&format!("connected_account_id={second}&connection_intent=reauthorize")), Some(again.clone()));
    assert_eq!((status, updated["id"].as_str(), updated["account_display_name"].as_str()), (200, Some(second.as_str()), Some("renamed@oauth.test")));
    assert_eq!(p.call("PUT", &one(&format!("connected_account_id={second}")), Some(add.clone())).0, 422, "no add intent on a PUT");
    assert_eq!(p.call("PUT", &one("connection_intent=reauthorize"), Some(again.clone())).0, 422, "an intent needs its account");
    assert_eq!(p.call("PUT", &one("connected_account_id=data_installation_none"), Some(again)).0, 404);

    p.fake.limit_accounts(Some(2));
    let (status, full) = p.call("POST", &path, Some(add.clone()));
    assert_eq!((status, full["code"].as_str()), (409, Some("connected_account_limit_reached")));
    p.fake.limit_accounts(None);
    p.fake.refuse_additional_accounts(true);
    let (status, one_only) = p.call("POST", &path, Some(add.clone()));
    assert_eq!((status, one_only["code"].as_str()), (404, Some("multiple_connections_unavailable")), "Pipes before additional connections are enabled");
    let other = p.fake.user("b@e2e.test").id;
    assert_eq!(p.call("POST", &format!("/user_management/users/{other}/connected_accounts/mail"), Some(add)).0, 201, "and an owner's first add still goes");
    p.fake.refuse_additional_accounts(false);

    assert_eq!(p.call("DELETE", &one(&format!("connected_account_id={second}&supports_multiple_connections=true")), None).0, 204);
    let left: Vec<String> = p.fake.accounts("a@e2e.test", "mail").into_iter().map(|a| a.id).collect();
    assert_eq!(left, std::slice::from_ref(&first), "only the account named is deleted");
    let (_, after) = token(json!({ "user_id": user, "supports_multiple_connections": true }));
    assert_eq!(whose(&after).as_deref(), Some(first.as_str()), "one left: a plural token naming none is that one's");
    assert_eq!(p.call("DELETE", &path, None).0, 204, "the compatibility connection, naming none");
    assert!(!p.fake.holds("a@e2e.test", "mail"));
}

/// Goal: a fake started again over the file its users were kept in hands no
/// email another's subject, as a dev stack's registry (which signs a person
/// in by subject) needs across its restarts. Method: one fake makes a user
/// and stops; another, over the same file, is asked for a new email and the
/// first one again. A fake with no file starts its ids over, the first
/// email to come taking the first id.
#[test]
fn users_kept_in_a_file_keep_their_subjects_across_a_restart() {
    let dir = std::env::temp_dir().join(format!("workos-kept-{}-{}", std::process::id(), now_s()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("workos.json");
    let first = {
        let fake = WorkOs::start("client_kept", KEY).unwrap();
        assert_eq!(fake.keep_users_in(&file).unwrap(), 0, "a new file holds no one");
        fake.user("dev@fragment.localhost").id
    };
    let fake = WorkOs::start("client_kept", KEY).unwrap();
    assert_eq!(fake.keep_users_in(&file).unwrap(), 1, "the user kept comes back");
    let other = fake.user("rehearsal@fragment.localhost");
    assert_ne!(other.id, first, "a new email gets a new subject, never the one kept");
    assert_eq!(fake.user("dev@fragment.localhost").id, first, "the kept email signs in as itself");
    fake.set_email(&other.id, "renamed@fragment.localhost");
    let again = WorkOs::start("client_kept", KEY).unwrap();
    assert_eq!(again.keep_users_in(&file).unwrap(), 2);
    assert_eq!(again.user("renamed@fragment.localhost").id, other.id, "a change of email is kept too");
    // what a fake with no file does: its ids start over
    let fresh = WorkOs::start("client_kept", KEY).unwrap();
    assert_eq!(fresh.user("rehearsal@fragment.localhost").id, first, "without a file, the first email to come takes the first id");
    std::fs::remove_dir_all(&dir).unwrap();
}
