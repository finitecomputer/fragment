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

    /// The stand-in provider's answer to a grant of its `account` (its code
    /// exchanged with its verifier).
    fn exchange(&self, account: &str) -> Value {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let q = [("client_id", "mock-connection-client"), ("response_type", "code"), ("code_challenge_method", "S256"), ("code_challenge", challenge), ("redirect_uri", "http://127.0.0.1:9/cb"), ("scope", "mail.read openid"), ("state", "s"), ("account", account)];
        let r = self.http.get(format!("{}/test-oauth/authorize", self.fake.url)).query(&q).send().unwrap();
        let back = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        let code = back.query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
        let form = [("code", code.as_str()), ("code_verifier", verifier), ("redirect_uri", "http://127.0.0.1:9/cb"), ("client_id", "mock-connection-client"), ("client_secret", "mock-connection-secret"), ("grant_type", "authorization_code")];
        serde_json::from_slice(&self.http.post(format!("{}/test-oauth/token", self.fake.url)).form(&form).send().unwrap().bytes().unwrap()).unwrap()
    }

    /// A grant of the stand-in provider's `account`: the import body
    /// Fragment sends for it.
    fn grant(&self, account: &str) -> Value {
        let t = self.exchange(account);
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

/// Goal: Pipes keeps an import's metadata as WorkOS documents it: a create
/// takes it, an update that names it is refused (422) and one that does
/// not leaves it; a credential's vend answers it beside the token, a
/// token's vend never. The stand-in provider names its account's host.
#[test]
fn an_imports_metadata_is_a_creates_and_the_credential_answers_it() {
    let p = Pipes::start();
    let user = p.fake.user("a@e2e.test").id;
    let path = format!("/user_management/users/{user}/connected_accounts/mail");
    let kept = json!({ "instance_url": format!("https://first.{ACCOUNT_HOSTS_UNDER}") });
    let mut create = p.grant("first");
    create["metadata"] = kept.clone();
    assert_eq!(p.call("POST", &path, Some(create.clone())).0, 201);
    assert_eq!(p.fake.metadata("a@e2e.test", "mail"), Some(kept.clone()));
    assert_eq!(p.call("PUT", &path, Some(create)).0, 422, "not accepted on update");
    assert_eq!(p.call("PUT", &path, Some(p.grant("again"))).0, 200);
    assert_eq!(p.fake.metadata("a@e2e.test", "mail"), Some(kept.clone()), "an update without it leaves it");
    let (status, vended) = p.call("POST", "/data-integrations/mail/credentials", Some(json!({ "user_id": user })));
    let value = vended["credential"]["value"].as_str().unwrap_or_default().to_string();
    assert_eq!((status, &vended["credential"]["metadata"], p.fake.token_account(&value).is_some()), (200, &kept, true), "{vended}");
    let (_, token) = p.call("POST", "/data-integrations/mail/token", Some(json!({ "user_id": user })));
    assert!(token["access_token"].get("metadata").is_none() && token.get("credential").is_none(), "{token}");
    p.fake.keep_metadata("a@e2e.test", "mail", json!({ "instance_url": "https://moved.my.oauth.test" }));
    let (_, vended) = p.call("POST", "/data-integrations/mail/credentials", Some(json!({ "user_id": user })));
    assert_eq!(vended["credential"]["metadata"]["instance_url"], "https://moved.my.oauth.test", "as a refresh would keep it");
    let (_, none) = p.call("POST", "/data-integrations/mail/credentials", Some(json!({ "user_id": p.fake.user("b@e2e.test").id })));
    assert_eq!((none["active"].as_bool(), none["error"].as_str()), (Some(false), Some("not_installed")));
}

/// Goal: the stand-in provider can answer a grant with no `expires_in`, as
/// Salesforce's token endpoint does, and Pipes refreshes an access token
/// asked for after its import's `expires_at` (once: the refresh gives it a
/// lifetime), a first import's or an update's, and never one still live.
#[test]
fn a_token_past_its_imports_expiry_is_refreshed_once() {
    let p = Pipes::start();
    p.fake.omit_next_expires_in();
    assert!(p.exchange("first").get("expires_in").is_none(), "no lifetime named");
    assert_eq!(p.exchange("first")["expires_in"], 3600, "the lever is one exchange's");
    let mut unnamed = p.grant("first");
    let named = p.grant("first");
    let user = p.fake.user("a@e2e.test").id;
    let path = format!("/user_management/users/{user}/connected_accounts/mail");
    let token = || p.call("POST", "/data-integrations/mail/token", Some(json!({ "user_id": user }))).1["active"] == true;
    let now = fragment_core::export::rfc3339(crate::codestorage::now_ms());
    unnamed["expires_at"] = json!(now);
    assert_eq!(p.call("POST", &path, Some(unnamed.clone())).0, 201);
    assert!(token() && token());
    assert_eq!(p.fake.refreshes(), 1, "refreshed at the first token asked for, then live");
    let mut live = named;
    live["expires_at"] = json!(fragment_core::export::rfc3339(crate::codestorage::now_ms() + 3_600_000));
    assert_eq!(p.call("PUT", &path, Some(live)).0, 200);
    assert!(token());
    assert_eq!(p.fake.refreshes(), 1, "an update with a lifetime: none");
    unnamed["expires_at"] = json!(fragment_core::export::rfc3339(crate::codestorage::now_ms()));
    assert_eq!(p.call("PUT", &path, Some(unnamed)).0, 200);
    assert!(token());
    assert_eq!(p.fake.refreshes(), 2, "an update expired as it lands: refreshed again");
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

/// A browser's authorize as `email` (login_hint): its status, and the code
/// the redirect carries, when it carries one.
fn authorize(fake: &WorkOs, client: &str, email: &str) -> (u16, Option<String>) {
    let http = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let q = [("client_id", client), ("response_type", "code"), ("redirect_uri", "http://127.0.0.1:9/cb"), ("state", "s"), ("login_hint", email)];
    let r = http.get(format!("{}/user_management/authorize", fake.url)).query(&q).send().unwrap();
    let code = r.headers().get("location").and_then(|l| url::Url::parse(l.to_str().ok()?).ok()).and_then(|u| u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()));
    (r.status().as_u16(), code)
}

/// Goal (#910's review): the subjects a registry signs people in by are
/// handed to their emails alone, whatever came first, however many: a new
/// email never gets one. Method: a node whose first person was not the dev
/// form's address and which holds several, adopted by a fake made new; a
/// new email, each adopted one, and a restart over the file.
#[test]
fn adopted_subjects_go_to_their_own_emails_alone() {
    let dir = std::env::temp_dir().join(format!("workos-adopt-{}-{}", std::process::id(), now_s()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("workos.json");
    let registry = [("user_00000000000000000000000003".to_string(), "first@example.test".to_string()), ("user_00000000000000000000000007".to_string(), "second@example.test".to_string())];
    let fake = WorkOs::start("client_adopt", KEY).unwrap();
    fake.keep_users_in(&file).unwrap();
    assert_eq!(fake.adopt(&registry).unwrap(), 2);
    let different = fake.user("different@example.test").id;
    assert!(registry.iter().all(|(s, _)| *s != different) && different.as_str() > "user_00000000000000000000000007", "a new email gets a new subject past them: {different}");
    assert_eq!(fake.user("first@example.test").id, registry[0].0);
    assert_eq!(fake.user("second@example.test").id, registry[1].0);
    assert!(fake.user("dev@fragment.localhost").id.as_str() > different.as_str(), "the dev address is no one's until it signs in");
    let again = WorkOs::start("client_adopt", KEY).unwrap();
    assert_eq!(again.keep_users_in(&file).unwrap(), 4, "kept across a restart");
    assert_eq!(again.adopt(&registry).unwrap(), 0, "adopting the same again adds no one");
    assert_eq!(again.user("second@example.test").id, registry[1].0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Goal (#910's review): a user whose keeping fails is handed out to no
/// one: the authorize answers 503 with no code, a try again once the file
/// can be written makes them and keeps them, and a restart hands their
/// subject to no other email. Method: the users' folder taken away under
/// a fake, an authorize, the folder back, the authorize again, a restart.
#[test]
fn a_user_not_kept_is_handed_out_to_no_one() {
    let dir = std::env::temp_dir().join(format!("workos-unkept-{}-{}", std::process::id(), now_s()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("workos.json");
    let fake = WorkOs::start("client_unkept", KEY).unwrap();
    fake.keep_users_in(&file).unwrap();
    let dev = fake.user("dev@fragment.localhost").id;
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(authorize(&fake, "client_unkept", "new@example.test"), (503, None), "not kept: no code");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, code) = authorize(&fake, "client_unkept", "new@example.test");
    assert_eq!((status, code.is_some()), (302, true), "kept on the try again");
    let new = fake.user("new@example.test").id;
    assert_ne!(new, dev);
    let again = WorkOs::start("client_unkept", KEY).unwrap();
    assert_eq!(again.keep_users_in(&file).unwrap(), 2, "both kept");
    assert_eq!(again.user("new@example.test").id, new);
    let other = again.user("other@example.test").id;
    assert!(other != new && other != dev, "a restart hands neither subject to another email");
    std::fs::remove_dir_all(&dir).unwrap();
}
