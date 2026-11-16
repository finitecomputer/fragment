//! The door end to end, over real sockets: its HTTP from a plain TCP
//! client playing the browser and the platform, its provider the fake
//! (crates/fakes/src/oidc.rs: strict, or shaped as tsidp). Each test says
//! its goal and method; together they are the door's valid, invalid,
//! replay and restart cases.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use fragment_fakes::oidc::{Alg, Oidc, Spoil};
use fragment_identity_door::oidc::{Settings, Vars};
use fragment_identity_door::{person, state, Client, Config, Door, ProviderConfig, Vouched};
use serde_json::{json, Value};

/// The door's client at the provider.
const PROVIDER_CLIENT: &str = "door";
const PROVIDER_SECRET: &str = "provider-secret";
/// The platform, as the door's client.
const CLIENT: &str = "client_cell";
const SECRET: &str = "sk_door_test";
const CALLBACK: &str = "http://fragment.example.com/auth/callback";
const DOOR_SECRET: &[u8] = b"the door's own secret, thirty-two bytes or more";

struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
    }

    fn location(&self) -> String {
        self.header("location").unwrap_or_default()
    }

    /// The query of this answer's Location, as pairs.
    fn sent(&self) -> BTreeMap<String, String> {
        url::Url::parse(&self.location()).map(|u| u.query_pairs().into_owned().collect()).unwrap_or_default()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// One request on a connection of its own.
fn send(to: SocketAddr, raw: &str) -> Answer {
    let mut s = TcpStream::connect(to).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Answer { status, head: head.to_string(), body: body.to_string() }
}

/// A browser's GET of `url`, at `to` whatever its host says (the door's
/// public URL is a name the test does not resolve).
fn get_at(to: SocketAddr, url: &str) -> Answer {
    let u = url::Url::parse(url).unwrap();
    let target = u.query().map_or_else(|| u.path().to_string(), |q| format!("{}?{q}", u.path()));
    send(to, &format!("GET {target} HTTP/1.1\r\nhost: {}\r\n\r\n", u.host_str().unwrap()))
}

fn get(url: &str) -> Answer {
    let u = url::Url::parse(url).unwrap();
    let to = format!("{}:{}", u.host_str().unwrap(), u.port().unwrap()).parse().unwrap();
    get_at(to, url)
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn authenticate(door: SocketAddr, body: &Value) -> Answer {
    let body = body.to_string();
    send(door, &format!("POST /user_management/authenticate HTTP/1.1\r\nhost: door\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()))
}

fn exchange(door: SocketAddr, code: &str) -> Answer {
    authenticate(door, &json!({ "client_id": CLIENT, "client_secret": SECRET, "grant_type": "authorization_code", "code": code }))
}

/// A door and its fake provider.
struct Rig {
    provider: Oidc,
    door: Door,
    vouched: Vec<String>,
}

fn config(provider: &Oidc, listen: SocketAddr, vouched: &[String], clients: Vec<Client>) -> Config {
    let vouched = Vouched::new(vouched).unwrap();
    let settings = Settings::parse(Vars { issuer: &provider.url, client_id: PROVIDER_CLIENT, vouches: vouched.any(), ..Vars::default() }).unwrap();
    let p = ProviderConfig { settings, client_secret: PROVIDER_SECRET.into(), ca_pem: None };
    Config::new(listen, "http://door.example.com", DOOR_SECRET.to_vec(), p, vouched, clients).unwrap()
}

fn rig(vouched: &[&str]) -> Rig {
    let provider = Oidc::start(PROVIDER_CLIENT, PROVIDER_SECRET).unwrap();
    let vouched: Vec<String> = vouched.iter().map(|d| d.to_string()).collect();
    let client = Client::with_secret(CLIENT, SECRET, &[CALLBACK]).unwrap();
    let door = Door::start(config(&provider, "127.0.0.1:0".parse().unwrap(), &vouched, vec![client])).unwrap();
    Rig { provider, door, vouched }
}

/// A sign-in's legs: the door's redirect to the provider, the provider's
/// back to the door, and the door's answer to that.
struct Trip {
    to_provider: Answer,
    to_door: Answer,
    back: Answer,
}

impl Rig {
    fn authorize(&self, query: &str) -> Answer {
        send(self.door.addr, &format!("GET /user_management/authorize?{query} HTTP/1.1\r\nhost: door\r\n\r\n"))
    }

    fn begin(&self, hint: &str) -> Answer {
        self.authorize(&format!("client_id={CLIENT}&redirect_uri={}&response_type=code&provider=authkit&state=s1&login_hint={}", enc(CALLBACK), enc(hint)))
    }

    /// The provider's answer to the door's redirect, followed back to the
    /// door: the whole sign-in as `hint`.
    fn sign_in(&self, hint: &str) -> Trip {
        let to_provider = self.begin(hint);
        assert_eq!(to_provider.status, 302, "{}", to_provider.head);
        let to_door = get(&to_provider.location());
        let back = match to_door.status {
            302 => get_at(self.door.addr, &to_door.location()),
            _ => Answer { status: 0, head: String::new(), body: String::new() },
        };
        Trip { to_provider, to_door, back }
    }

    /// A sign-in's code for the platform.
    fn code(&self, hint: &str) -> String {
        let t = self.sign_in(hint);
        t.back.sent().get("code").cloned().unwrap_or_else(|| panic!("no code: {} {:?}", t.back.head, t.back.sent()))
    }

    /// A sign-in the door refuses: its OAuth error and description, handed
    /// back to the platform's callback with its state.
    fn refused(&self, hint: &str) -> (String, String) {
        let t = self.sign_in(hint);
        let q = t.back.sent();
        assert!(t.back.location().starts_with(CALLBACK), "back to the platform: {}", t.back.head);
        assert!(!q.contains_key("code"), "no code: {q:?}");
        assert_eq!(q.get("state").map(String::as_str), Some("s1"));
        (q["error"].clone(), q["error_description"].clone())
    }
}

/// Goal: a person signs in through the provider with nothing of the
/// platform's sent there: the door's own state, PKCE's S256 and a nonce
/// go to the provider; the door's code, with the platform's state, comes
/// back; the code, with the platform's secret, answers who they are, once;
/// and the same provider subject is the same person again.
/// Method: the browser's legs by hand, then the platform's exchange.
#[test]
fn a_person_signs_in_through_the_provider() {
    let r = rig(&[]);
    let t = r.sign_in("Grace@Corp.Example");
    let at_provider = t.to_provider.sent();
    assert!(t.to_provider.location().starts_with(&format!("{}/authorize?", r.provider.url)));
    assert_eq!(at_provider["code_challenge_method"], "S256");
    assert!(at_provider["nonce"].len() == 43 && at_provider["code_challenge"].len() == 43);
    assert_eq!(at_provider["redirect_uri"], "http://door.example.com/oidc/callback");
    assert_ne!(at_provider["state"], "s1", "the door's state, not the platform's");
    assert_eq!(at_provider["login_hint"], "Grace@Corp.Example", "passed on");
    assert!(t.to_door.location().starts_with("http://door.example.com/oidc/callback?code="));
    let back = t.back.sent();
    assert!(t.back.location().starts_with(&format!("{CALLBACK}?code=")), "{}", t.back.head);
    assert_eq!(back["state"], "s1", "the platform's own state, handed back");
    let user = exchange(r.door.addr, &back["code"]);
    assert_eq!(user.status, 200, "{}", user.body);
    let u = &user.json()["user"];
    let sub = r.provider.user("grace@corp.example").id;
    assert_eq!((u["email"].clone(), u["email_verified"].clone()), (json!("grace@corp.example"), json!(true)));
    assert_eq!(u["id"], person::subject_of(&r.provider.url, &sub).as_str());
    assert!(user.json().get("access_token").is_none(), "no session at the door, so no token");
    assert_eq!(r.provider.token_auth().last().map(String::as_str), Some("client_secret_basic"));
    let again = exchange(r.door.addr, &r.code("grace@corp.example"));
    assert_eq!(again.json()["user"]["id"], u["id"].clone(), "the same subject, the same person");
    let renamed = r.provider.user("grace@corp.example");
    r.provider.set_email(&renamed.id, "grace.h@corp.example");
    let after = exchange(r.door.addr, &r.code("grace@corp.example"));
    assert_eq!((after.json()["user"]["id"].clone(), after.json()["user"]["email"].clone()), (u["id"].clone(), json!("grace.h@corp.example")), "an email changed keeps the subject");
}

/// Goal: shaped as tsidp (RS256 alone, no `email_verified`), a person
/// signs in only by a domain the door vouches for; a GitHub login's
/// `user@github` never; and a strict provider's unverified email, or none,
/// signs no one in.
/// Method: the fake shaped so, each login in turn.
#[test]
fn the_email_is_verified_or_vouched_for() {
    let r = rig(&["tailnet.example"]);
    assert_eq!(r.vouched, ["tailnet.example"]);
    r.provider.like_tsidp(true);
    let user = exchange(r.door.addr, &r.code("ada@tailnet.example"));
    assert_eq!(user.json()["user"]["email"], "ada@tailnet.example", "vouched for: {}", user.body);
    let (error, why) = r.refused("bob@other.example");
    assert!(error == "access_denied" && why.contains("has not verified"), "{why}");
    let (error, why) = r.refused("alice@github");
    assert!(error == "access_denied" && why.contains("no mailbox"), "{why}");
    r.provider.like_tsidp(false);
    let jo = r.provider.user("jo@other.example");
    r.provider.set_verified(&jo.id, false);
    let (_, why) = r.refused("jo@other.example");
    assert!(why.contains("has not verified"), "{why}");
    let (_, why) = r.refused("jdoe");
    assert!(why.contains("gave no email"), "a directory that releases none: {why}");
}

/// Goal: an id_token not to trust signs no one in, whatever is wrong with
/// it; keys rotated at the provider are fetched again at once, and a key
/// it lacks is not asked for again within 15 s of that.
/// Method: each spoil the fake makes; ES256; a rotation; then two tokens
/// by a key never published, counting the JWKS's fetches.
#[test]
fn an_id_token_not_to_trust_signs_no_one_in() {
    let r = rig(&[]);
    r.code("grace@corp.example");
    for spoil in [
        Spoil::WrongAudience,
        Spoil::WrongIssuer,
        Spoil::Expired,
        Spoil::WrongNonce,
        Spoil::Replay,
        Spoil::BadSignature,
        Spoil::AlgNone,
        Spoil::Hs256,
        Spoil::ForeignKey,
    ] {
        r.provider.spoil_next(spoil);
        let (error, why) = r.refused("grace@corp.example");
        assert!(error == "access_denied" && why.contains("not one to trust"), "{spoil:?}: {why}");
    }
    r.provider.sign_with(Alg::Es256);
    assert_eq!(exchange(r.door.addr, &r.code("grace@corp.example")).status, 200, "ES256");
    r.provider.sign_with(Alg::Rs256);
    let fetched = r.provider.jwks_fetches();
    r.provider.rotate_keys();
    assert_eq!(exchange(r.door.addr, &r.code("grace@corp.example")).status, 200, "a rotation is picked up at its first token");
    assert_eq!(r.provider.jwks_fetches(), fetched + 1);
    for _ in 0..2 {
        r.provider.spoil_next(Spoil::UnpublishedKey);
        let (_, why) = r.refused("grace@corp.example");
        assert!(why.contains("does not hold"), "{why}");
    }
    assert_eq!(r.provider.jwks_fetches(), fetched + 1, "a key never published, within 15 s of the last fetch for a missing key: no fetch again");
}

/// Goal: the provider's own refusal is handed back to the platform; a
/// provider that cannot be reached is said so, before the browser leaves;
/// a state the door did not seal is a page, never a redirect.
/// Method: the fake's refusal; a door whose provider has stopped; a
/// callback with a forged state and with none.
#[test]
fn refusals_go_back_to_the_platform_or_stay_a_page() {
    let r = rig(&[]);
    r.provider.fail_next("access_denied");
    let (error, why) = r.refused("grace@corp.example");
    assert!(error == "access_denied" && why.contains("the sign-in provider said"), "{why}");
    let t = r.sign_in("grace@corp.example");
    let forged = t.to_door.location().replace("state=", "state=x");
    let page = get_at(r.door.addr, &forged);
    assert_eq!((page.status, page.header("location")), (400, None), "{}", page.body);
    // well formed, for this client and callback, sealed by another secret
    let begun = state::Begun { client_id: CLIENT.into(), callback: 0, client_state: Some("s1".into()), at_s: now_s(), salt: "00".into() };
    let other = state::StateKey::of(b"another door's secret, thirty-two bytes or more").seal(&begun).unwrap();
    let page = get_at(r.door.addr, &format!("http://door.example.com/oidc/callback?code=abc&state={}", enc(&other)));
    assert_eq!((page.status, page.header("location")), (400, None), "another door's state: {}", page.body);
    assert!(page.body.contains("not the door"), "{}", page.body);
    let none = get_at(r.door.addr, "http://door.example.com/oidc/callback?code=abc");
    assert_eq!((none.status, none.header("location")), (400, None));
    // a door whose provider is gone (it never fetched its metadata)
    let gone = Oidc::start(PROVIDER_CLIENT, PROVIDER_SECRET).unwrap();
    let client = Client::with_secret(CLIENT, SECRET, &[CALLBACK]).unwrap();
    let lone = Door::start(config(&gone, "127.0.0.1:0".parse().unwrap(), &[], vec![client])).unwrap();
    drop(gone);
    let a = send(lone.addr, &format!("GET /user_management/authorize?client_id={CLIENT}&redirect_uri={}&response_type=code&state=s1 HTTP/1.1\r\nhost: door\r\n\r\n", enc(CALLBACK)));
    assert!(a.location().starts_with(CALLBACK) && a.sent()["error"] == "temporarily_unavailable", "{}", a.head);
}

/// Goal: nothing is good twice: the provider's callback sent again finds
/// its code spent there; the door's code is good once, for its client's
/// secret alone, and a wrong secret never burns it.
/// Method: each replayed by hand.
#[test]
fn nothing_is_good_twice() {
    let r = rig(&[]);
    let t = r.sign_in("grace@corp.example");
    let code = t.back.sent()["code"].clone();
    let replayed = get_at(r.door.addr, &t.to_door.location());
    let q = replayed.sent();
    assert!(q["error"] == "access_denied" && q["error_description"].contains("refused the code"), "{q:?}");
    let wrong = authenticate(r.door.addr, &json!({ "client_id": CLIENT, "client_secret": "nope", "grant_type": "authorization_code", "code": code }));
    assert_eq!((wrong.status, wrong.json()["error"].clone()), (401, json!("invalid_client")));
    let grant = authenticate(r.door.addr, &json!({ "client_id": CLIENT, "client_secret": SECRET, "grant_type": "refresh_token", "code": code }));
    assert_eq!((grant.status, grant.json()["error"].clone()), (400, json!("unsupported_grant_type")));
    assert_eq!(exchange(r.door.addr, &code).status, 200, "untouched by the refusals before");
    let replay = exchange(r.door.addr, &code);
    assert_eq!((replay.status, replay.json()["error"].clone()), (400, json!("invalid_grant")));
}

/// Goal: the door's state lives in the browser, so a door restarted while
/// its person is at the provider finishes the sign-in; its codes live in
/// its memory, so one issued before a restart is refused after it, saying
/// so; and a restarted door signs people in as before.
/// Method: the door dropped and started again on its port, with its
/// secret, at each point of a sign-in.
#[test]
fn a_restart_keeps_the_trip_and_forgets_the_codes() {
    let Rig { provider, door, vouched } = rig(&[]);
    let restart = |door: Door| {
        let addr = door.addr;
        drop(door);
        let client = Client::with_secret(CLIENT, SECRET, &[CALLBACK]).unwrap();
        let again = Door::start(config(&provider, addr, &vouched, vec![client])).unwrap();
        assert_eq!(again.addr, addr);
        again
    };
    let to_provider = send(door.addr, &format!("GET /user_management/authorize?client_id={CLIENT}&redirect_uri={}&response_type=code&state=s1&login_hint=grace%40corp.example HTTP/1.1\r\nhost: door\r\n\r\n", enc(CALLBACK)));
    let door = restart(door);
    let to_door = get(&to_provider.location());
    let back = get_at(door.addr, &to_door.location());
    let code = back.sent()["code"].clone();
    assert!(!code.is_empty(), "a sign-in begun before the restart finishes after it: {}", back.head);
    let door = restart(door);
    let stale = exchange(door.addr, &code);
    assert_eq!((stale.status, stale.json()["error"].clone()), (400, json!("invalid_grant")));
    assert!(stale.json()["error_description"].as_str().unwrap_or("").contains("restarted"), "said plainly: {}", stale.body);
    let fresh = send(door.addr, &format!("GET /user_management/authorize?client_id={CLIENT}&redirect_uri={}&response_type=code&state=s1&login_hint=grace%40corp.example HTTP/1.1\r\nhost: door\r\n\r\n", enc(CALLBACK)));
    let back = get_at(door.addr, &get(&fresh.location()).location());
    assert_eq!(exchange(door.addr, &back.sent()["code"]).status, 200);
}

/// Goal: an authorize naming no client of the door's, or a callback its
/// client did not register, is a page: never a redirect. A malformed one
/// past that goes back as OAuth's error. Nothing but sign-in is served.
/// Method: each malformed request in turn.
#[test]
fn malformed_requests_are_refused() {
    let r = rig(&[]);
    for (query, what) in [
        (format!("client_id=client_nobody&redirect_uri={}&response_type=code", enc(CALLBACK)), "unknown client"),
        (format!("client_id={CLIENT}&redirect_uri={}&response_type=code", enc("https://evil.example/auth/callback")), "foreign callback"),
        (format!("client_id={CLIENT}&redirect_uri={}&response_type=code", enc(&format!("{CALLBACK}/x"))), "a callback merely like it"),
        (format!("client_id={CLIENT}&client_id={CLIENT}&redirect_uri={}&response_type=code", enc(CALLBACK)), "a parameter twice"),
        (format!("client_id={CLIENT}&response_type=code"), "no callback"),
    ] {
        let a = r.authorize(&query);
        assert_eq!(a.status, 400, "{what}");
        assert!(a.header("location").is_none(), "{what}: never a redirect");
        assert!(a.header("content-security-policy").is_some_and(|c| c.contains("frame-ancestors 'none'")), "{what}");
    }
    let a = r.authorize(&format!("client_id={CLIENT}&redirect_uri={}&response_type=token&state=s1", enc(CALLBACK)));
    assert_eq!(a.sent()["error"], "unsupported_response_type");
    let a = r.authorize(&format!("client_id={CLIENT}&redirect_uri={}&response_type=code&state=%01", enc(CALLBACK)));
    assert!(a.sent()["error"] == "invalid_request" && !a.sent().contains_key("state"), "an unprintable state is not handed back");
    let a = r.authorize(&format!("client_id={CLIENT}&redirect_uri={}&response_type=code&state={}", enc(CALLBACK), "%5C".repeat(1024)));
    assert!(a.sent()["error"] == "invalid_request" && !a.sent().contains_key("state"), "a state too long once sealed is refused, not a panic");
    let a = send(r.door.addr, "POST /user_management/authenticate HTTP/1.1\r\nhost: door\r\ncontent-type: application/x-www-form-urlencoded\r\ncontent-length: 4\r\n\r\ncode");
    assert_eq!(a.status, 415);
    let big = "x".repeat(fragment_identity_door::BODY_BYTES_MAX + 1);
    let a = send(r.door.addr, &format!("POST /user_management/authenticate HTTP/1.1\r\nhost: door\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{big}", big.len()));
    assert_eq!(a.status, 413);
    assert_eq!(authenticate(r.door.addr, &json!({ "code": "x" })).status, 400);
    let a = send(r.door.addr, "POST /data-integrations/google/token HTTP/1.1\r\nhost: door\r\ncontent-length: 0\r\n\r\n");
    assert_eq!(a.status, 404, "no Pipes");
    let a = send(r.door.addr, "POST /user_management/authorize HTTP/1.1\r\nhost: door\r\ncontent-length: 0\r\n\r\n");
    assert_eq!(a.status, 405);
    let a = get_at(r.door.addr, &format!("http://door/user_management/sessions/logout?session_id=x&return_to={}", enc("http://fragment.example.com/")));
    assert_eq!((a.status, a.header("location")), (302, Some("http://fragment.example.com/".into())));
    let a = get_at(r.door.addr, &format!("http://door/user_management/sessions/logout?return_to={}", enc("https://evil.example/")));
    assert_eq!((a.status, a.header("location")), (400, None));
}
