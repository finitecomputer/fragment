//! An agent's SimpleX chat is bound by its name, and a name is no life
//! (security candidate 10). The poster checked the chat was its owner's
//! once (`ensure_chat`, as it started, and again only after a refusal),
//! then posted each of its owner's SimpleX messages to the chat by name;
//! now it checks before each (`still_theirs`). Its owner
//! deletes the chat (its name free at once: docs/api.md, `DELETE
//! /api/f/{name}`); a stranger makes a fragment of that name in full (a
//! create may name one: docs/api.md, Names), public, its `chat` channel
//! `"read": "public", "post": "public"` (deployable: e2e posts lane). The
//! owner's next message was posted there: a relayed record is checked for
//! its door's signature over the fragment's name (cell/src/channels.rs
//! `door_checked`), never for who owns the fragment.
//!
//! The platform here is in process and mirrors the cell's rules the poster
//! meets: a chat made by name for the owner (409 when taken), its members
//! read by a member (404 to anyone else on a members-only fragment), a post
//! admitted by membership or, on a public fragment, by a public post role
//! (`admit_poster`), and a relayed record taken only from its agent, for its
//! owner, signed by the door key it registered (`door_checked`). SimpleX is
//! mocked away: the poster is fed its owner's messages as the manager feeds
//! them once the core forwards them.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use super::{poster, ChatNot, Msg, Post};
use crate::api::Api;
use crate::net::{self, Body};
use crate::runtime::Agent;
use crate::simplex::door::{self, Key};
use crate::simplex::link::Inbound;

const OWNER: &str = "npub1paul";
const STRANGER: &str = "npub1mallory";
const AGENT: &str = "juniper--k3x9";
const IDENTITY: &str = "npub1juniper";
const CHAT: &str = "juniper-simplex--w9x2";

#[derive(Clone, Debug)]
struct Frag {
    /// (principal, role)
    members: Vec<(String, String)>,
    /// Public visibility, its `chat` channel `"post": "public"`.
    public: bool,
    records: Vec<Value>,
}

#[derive(Default)]
struct World {
    fragments: BTreeMap<String, Frag>,
    /// The door key each agent registered (`POST /api/computer/door`).
    doors: BTreeMap<String, String>,
    /// The next this many reads of members answer 503 (a deploying platform).
    fail_members: u32,
}

impl World {
    fn texts(&self, fragment: &str) -> Vec<String> {
        self.fragments.get(fragment).map(|f| f.records.iter().filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect()).unwrap_or_default()
    }
    fn owner_of(&self, fragment: &str) -> Option<String> {
        self.fragments.get(fragment)?.members.iter().find(|(_, r)| r == "owner").map(|(p, _)| p.clone())
    }
}

fn agent() -> Agent {
    Agent { fragment: AGENT.into(), identity: IDENTITY.into(), name: "juniper".into(), owner: OWNER.into(), credentials: vec![], key: None, runtime: None, claude: Default::default() }
}

fn refuse(status: StatusCode, message: &str) -> Response<Body> {
    net::refusal(status, "refused", message)
}

async fn handle(req: Request<Incoming>, world: Arc<Mutex<World>>) -> Response<Body> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let q = req.uri().query().unwrap_or("").to_string();
    // the intercept's job, faked: the request is this agent's, signed as it
    let Some(as_agent) = req.headers().get("x-fragment-agent").and_then(|v| v.to_str().ok()).map(str::to_string) else {
        return refuse(StatusCode::UNAUTHORIZED, "no agent");
    };
    assert_eq!(as_agent, AGENT, "only juniper runs here");
    let body = req.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let p: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut w = world.lock().expect("the world");
    match (method, p.as_slice()) {
        // a chat made by its name for the agent's owner: theirs, the agent an editor
        (Method::POST, ["api", "fragments"]) => {
            assert!(q.contains("for=npub1paul"), "made for its owner: {q}");
            let name = v["name"].as_str().unwrap_or_default().to_string();
            if w.fragments.contains_key(&name) {
                return refuse(StatusCode::CONFLICT, "that name is taken");
            }
            w.fragments.insert(name.clone(), Frag { members: vec![(OWNER.into(), "owner".into()), (IDENTITY.into(), "editor".into())], public: false, records: vec![] });
            net::json_answer(StatusCode::OK, &json!({ "name": name, "owner": OWNER }))
        }
        (Method::GET, ["api", "f", _, "members"]) if w.fail_members > 0 => {
            w.fail_members -= 1;
            refuse(StatusCode::SERVICE_UNAVAILABLE, "deploying")
        }
        (Method::GET, ["api", "f", name, "members"]) => {
            let Some(f) = w.fragments.get(*name) else { return refuse(StatusCode::NOT_FOUND, "no such fragment") };
            if !f.public && !f.members.iter().any(|(m, _)| m == IDENTITY) {
                return refuse(StatusCode::NOT_FOUND, "no such fragment");
            }
            let list: Vec<Value> = f.members.iter().map(|(p, r)| json!({ "principal": p, "role": r, "kind": "person", "addedAt": 1 })).collect();
            net::json_answer(StatusCode::OK, &json!({ "members": list }))
        }
        (Method::POST, ["api", "f", name, "channels", "chat"]) => {
            let name = name.to_string();
            let key = w.doors.get(AGENT).cloned().unwrap_or_default();
            let Some(f) = w.fragments.get_mut(&name) else { return refuse(StatusCode::NOT_FOUND, "no such fragment") };
            // admit_poster: a member (chat's post role is viewer), or anyone
            // at all on a public fragment whose chat's post role is public
            let member = f.members.iter().any(|(m, _)| m == IDENTITY);
            if !member && !f.public {
                return refuse(StatusCode::NOT_FOUND, "no such fragment");
            }
            let (Some(id), rec) = (v["id"].as_str().map(str::to_string), v["body"].clone()) else { return refuse(StatusCode::BAD_REQUEST, "{id, body}") };
            // door_checked: its agent's, for its owner, signed by its door
            // over this fragment's name; who owns the fragment is not asked
            if let Some(r) = rec.get("relayed") {
                let digest = door::record_digest(door::DOOR, &name, "chat", &id, &rec);
                if r["agent"] != AGENT || r["for"] != OWNER || !door::verify(&key, &digest, r["sig"].as_str().unwrap_or("")) {
                    return refuse(StatusCode::FORBIDDEN, "a relayed record is signed by its agent's door");
                }
            }
            let seq = f.records.len() as u64 + 1;
            let record = json!({ "seq": seq, "principal": IDENTITY, "kind": "message", "body": rec });
            f.records.push(record.clone());
            net::json_answer(StatusCode::OK, &json!({ "record": record, "replayed": false }))
        }
        _ => refuse(StatusCode::NOT_FOUND, "no such route"),
    }
}

struct Platform {
    world: Arc<Mutex<World>>,
    url: String,
    _stop: watch::Sender<bool>,
}

async fn platform(door_key: &Key) -> Platform {
    let world = Arc::new(Mutex::new(World::default()));
    world.lock().expect("the world").doors.insert(AGENT.into(), door_key.public_hex());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("its address"));
    let (stop, stop_rx) = watch::channel(false);
    let w = world.clone();
    let handler = move |req: Request<Incoming>, _peer: SocketAddr| {
        let w = w.clone();
        async move { handle(req, w).await }
    };
    tokio::spawn(net::serve(listener, handler, stop_rx));
    Platform { world, url, _stop: stop }
}

fn said(item: i64, text: &str, door_key: &Key) -> Post {
    let inbound = Inbound { id: format!("m{item}"), kind: "text".into(), text: text.into(), at: "2026-11-19T12:00:00Z".into(), item, file: None };
    Post { inbound, file: None, note: None, door: door_key.clone() }
}

/// The manager's next word of a delivery: (item, ok).
async fn delivered(manager: &mut mpsc::Receiver<Msg>) -> (i64, bool) {
    // bounded: fifteen seconds (two backoffs fit)
    let wait = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match manager.recv().await {
                Some(Msg::Delivered { item, ok, .. }) => return (item, ok),
                Some(_) => continue,
                None => panic!("the poster ended"),
            }
        }
    });
    wait.await.expect("a delivery within 15 s")
}

/// The stranger's chat, made by the name the owner's chat had: public,
/// its `chat` channel public to post to, the stranger its one member.
fn stranger_remakes(world: &Arc<Mutex<World>>) {
    let mut w = world.lock().expect("the world");
    assert!(!w.fragments.contains_key(CHAT), "the name is free");
    w.fragments.insert(CHAT.into(), Frag { members: vec![(STRANGER.into(), "owner".into())], public: true, records: vec![] });
}

/// Goal (security candidate 10): an owner's SimpleX message reaches only a
/// chat its owner owns. The owner's chat is deleted while the computer is
/// awake and a stranger makes a public chat of the same name; the owner's
/// next SimpleX message must not be posted to the stranger's chat: it is
/// undelivered (`ok=false`, read again at the next start's catch-up), and
/// its manager told the name is taken, which names another. Failed before
/// the poster checked before each post.
#[tokio::test]
async fn an_owners_message_never_reaches_a_chat_remade_by_a_stranger() {
    let door_key = Key::generate().expect("a door key");
    let p = platform(&door_key).await;
    let api = Api::new(&p.url).expect("an address");
    let (tx, rx) = mpsc::channel(16);
    let (manager_tx, mut manager) = mpsc::channel(16);
    let (_stop_tx, stop) = watch::channel(false);
    tokio::spawn(poster(api, agent(), CHAT.into(), rx, manager_tx, stop));

    // the link bound: its chat made for its owner, a message posted there
    tx.send(said(1, "hello from my phone", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (1, true));
    {
        let w = p.world.lock().expect("the world");
        assert_eq!(w.owner_of(CHAT).as_deref(), Some(OWNER), "the owner's chat");
        assert_eq!(w.texts(CHAT), ["hello from my phone"]);
    }

    // its owner deletes it (the name free at once); a stranger makes that name
    p.world.lock().expect("the world").fragments.remove(CHAT);
    stranger_remakes(&p.world);

    // the owner's next message from their phone
    tx.send(said(2, "my door code is 4321", &door_key)).await.expect("the poster");
    let (item, ok) = delivered(&mut manager).await;
    {
        let w = p.world.lock().expect("the world");
        let leaked = w.texts(CHAT);
        assert!(
            !leaked.iter().any(|t| t.contains("4321")),
            "the owner's SimpleX message was posted to a chat the stranger {STRANGER} owns ({CHAT}, re-made by name): {leaked:?}"
        );
        assert_eq!(w.owner_of(CHAT).as_deref(), Some(STRANGER), "the stranger's chat, untouched");
    }
    assert_eq!((item, ok), (2, false), "undelivered, said so");
    assert!(taken(&mut manager).await, "its manager told the name is someone else's");
}

/// The manager's next word of a name taken: whether it came for `CHAT`.
async fn taken(manager: &mut mpsc::Receiver<Msg>) -> bool {
    // bounded: five seconds
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match manager.recv().await {
                Some(Msg::ChatTaken { chat, .. }) => return chat == CHAT,
                Some(_) => continue,
                None => return false,
            }
        }
    });
    wait.await.expect("a word within 5 s")
}

/// Goal: a chat its owner deleted (its name free, no one's) takes nothing
/// as it is gone. Valid: the message that met it gone is undelivered
/// (`ok=false`), the chat is made again for its owner, and the next is
/// posted there. Invalid: a post to a name with no chat behind it.
#[tokio::test]
async fn a_message_to_a_deleted_chat_is_undelivered_and_the_chat_made_again() {
    let door_key = Key::generate().expect("a door key");
    let p = platform(&door_key).await;
    let api = Api::new(&p.url).expect("an address");
    let (tx, rx) = mpsc::channel(16);
    let (manager_tx, mut manager) = mpsc::channel(16);
    let (_stop_tx, stop) = watch::channel(false);
    tokio::spawn(poster(api, agent(), CHAT.into(), rx, manager_tx, stop));
    tx.send(said(1, "first", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (1, true));
    p.world.lock().expect("the world").fragments.remove(CHAT);
    tx.send(said(2, "met it gone", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (2, false), "undelivered");
    tx.send(said(3, "after", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (3, true));
    let w = p.world.lock().expect("the world");
    assert_eq!(w.owner_of(CHAT).as_deref(), Some(OWNER), "made again for its owner");
    assert_eq!(w.texts(CHAT), ["after"], "the one that met it gone is posted nowhere");
}

/// Goal: the check before each post is no new way to lose a message. Valid:
/// a platform that fails the read twice (503, deploying) is asked again,
/// and the message posted. Replay: the next message is checked and posted.
#[tokio::test]
async fn a_check_the_platform_fails_for_now_is_tried_again() {
    let door_key = Key::generate().expect("a door key");
    let p = platform(&door_key).await;
    let api = Api::new(&p.url).expect("an address");
    let (tx, rx) = mpsc::channel(16);
    let (manager_tx, mut manager) = mpsc::channel(16);
    let (_stop_tx, stop) = watch::channel(false);
    tokio::spawn(poster(api, agent(), CHAT.into(), rx, manager_tx, stop));
    tx.send(said(1, "first", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (1, true));
    p.world.lock().expect("the world").fail_members = 2;
    tx.send(said(2, "through a deploy", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (2, true), "posted once the platform answered");
    tx.send(said(3, "after", &door_key)).await.expect("the poster");
    assert_eq!(delivered(&mut manager).await, (3, true));
    let w = p.world.lock().expect("the world");
    assert_eq!(w.fail_members, 0, "both failed reads were asked");
    assert_eq!(w.texts(CHAT), ["first", "through a deploy", "after"]);
}

/// Control: a poster that starts (a bridge restart) on a name a stranger
/// re-made finds it taken (its one ownership check), and posts nothing
/// there. Passes today: the hole is only between checks.
#[tokio::test]
async fn a_poster_started_on_a_remade_name_finds_it_taken() {
    let door_key = Key::generate().expect("a door key");
    let p = platform(&door_key).await;
    stranger_remakes(&p.world);
    let api = Api::new(&p.url).expect("an address");
    assert!(matches!(super::ensure_chat(&api, &agent(), CHAT).await, Err(ChatNot::Taken)), "the stranger's chat is no one else's to post in");
    let (tx, rx) = mpsc::channel(16);
    let (manager_tx, mut manager) = mpsc::channel(16);
    let (_stop_tx, stop) = watch::channel(false);
    let posting = tokio::spawn(poster(api, agent(), CHAT.into(), rx, manager_tx, stop));
    let _ = tx.send(said(1, "my door code is 4321", &door_key)).await;
    let told = tokio::time::timeout(Duration::from_secs(5), manager.recv()).await.expect("a word within 5 s");
    assert!(matches!(told, Some(Msg::ChatTaken { ref chat, .. }) if chat == CHAT), "the manager is told its name is taken");
    posting.await.expect("the poster ended");
    assert!(p.world.lock().expect("the world").texts(CHAT).is_empty(), "nothing posted in the stranger's chat");
}
