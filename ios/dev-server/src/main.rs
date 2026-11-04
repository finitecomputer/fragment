//! Local wire fixture for the native app, never a production authentication service.
mod brain;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;

const PERSON: &str = "npub1localperson";
const AGENT: &str = "npub1localagent";
const MAX_RECORDS: usize = 2_000;
type Shared = Arc<Mutex<Fixture>>;
type Failure = (StatusCode, Json<Value>);

struct Room {
    summary: Value,
    records: Vec<Value>,
    posts: HashMap<String, (Value, Value)>,
    tx: broadcast::Sender<Value>,
    turns: HashMap<String, bool>,
    seq: HashMap<String, u64>,
    blobs: HashMap<String, (Vec<u8>, String)>,
    staged_title: Option<String>,
    owner: String,
    visibility: String,
    members: Vec<Value>,
    invites: Vec<Value>,
    icon: Option<Vec<u8>>,
    card: Option<Vec<u8>>,
    card_requested: Option<u64>,
    card_at: Option<u64>,
    token_version: u64,
}

impl Room {
    fn new(name: &str, kind: &str, title: &str) -> Self {
        Self {
            summary: json!({"name": name, "kind": kind, "title": title, "role": "owner", "agents": [AGENT]}),
            records: Vec::new(),
            posts: HashMap::new(),
            tx: broadcast::channel(256).0,
            turns: HashMap::new(),
            seq: HashMap::new(),
            blobs: HashMap::new(),
            staged_title: None,
            owner: PERSON.into(),
            visibility: if ["chat", "agent", "sessions"].contains(&kind) {
                "members"
            } else {
                "link"
            }
            .into(),
            members: vec![
                json!({"principal":PERSON,"role":"owner","kind":"person","email":"preview@finite.local"}),
            ],
            invites: Vec::new(),
            icon: None,
            card: None,
            card_requested: None,
            card_at: None,
            token_version: 1,
        }
    }
    fn sync_sharing(&mut self) {
        if self.summary["role"] == "owner" {
            let guests = self
                .members
                .iter()
                .filter(|m| m["role"] != "owner" && m["kind"] == "person")
                .count();
            self.summary["sharing"] = json!({"visibility":self.visibility,"members":self.members.len(),"guests":guests,"invites":self.invites.len()});
        }
    }
    fn append(&mut self, channel: &str, principal: &str, body: Value) -> Value {
        let seq = self.seq.entry(channel.into()).or_default();
        *seq += 1;
        let record = json!({"channel": channel, "seq": *seq, "at": now(), "principal": principal, "kind": "message", "body": body});
        self.records.push(record.clone());
        if self.records.len() > MAX_RECORDS {
            self.records.remove(0);
        }
        let mut frame = record.clone();
        frame["type"] = json!("record");
        let _ = self.tx.send(frame);
        if channel == "chat" {
            if let Some(text) = record["body"]["text"].as_str() {
                self.summary["preview"] = json!(text.chars().take(100).collect::<String>());
                self.summary["previewAt"] = json!(now());
            }
        }
        record
    }
}

struct Fixture {
    rooms: HashMap<String, Room>,
    counter: u64,
    base: String,
    generation: u64,
    exported: bool,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as u64
}
fn failure(code: StatusCode, message: &str) -> Failure {
    (code, Json(json!({"error": "fixture", "message": message})))
}

#[tokio::main]
async fn main() {
    let address = std::env::var("FRAGMENT_IOS_LISTEN").unwrap_or_else(|_| "127.0.0.1:8910".into());
    let base =
        std::env::var("FRAGMENT_IOS_BASE_URL").unwrap_or_else(|_| "http://localhost:8910".into());
    let state = Arc::new(Mutex::new(seed(base)));
    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"ok": true, "fixture": true})) }),
        )
        .route("/api/fragments", get(list).post(create))
        .route("/api/search", get(search))
        .route("/api/fragments/{name}/archived", put(archive))
        .route("/api/fragments/{name}/dock", put(dock))
        .route("/api/f/{name}/manifest", get(manifest))
        .route("/api/f/{name}/file", get(archive_index))
        .route("/api/f/{name}/search", post(archive_search))
        .route("/api/f/{name}/files", post(files))
        .route("/api/f/{name}/deploy", post(deploy))
        .route("/api/f/{name}/blobs/{hash}", put(upload).get(blob))
        .route("/api/identities/me", get(|| async { Json(json!({"id":PERSON,"email":"preview@finite.local"})) }))
        .route("/api/seat", get(|| async { Json(json!({"seat":{"kind":"Local preview","org":{"name":"Personal"}},"guest":false,"offer":null})) }))
        .route("/api/computers", get(computers))
        .route("/api/computers/{id}/restart", post(restart))
        .route("/api/connections", get(|| async { Json(json!({"providers":[{"provider":"Sample","state":"not_connected","note":"Local stand-in. No external account is connected."}]})) }))
        .route("/api/export", get(export_status).post(export))
        .route("/export.json", get(export_download))
        .route("/api/f/{name}/status", get(status))
        .route("/api/f/{name}/icon", get(app_icon).put(put_icon).delete(remove_icon))
        .route("/api/f/{name}/icon/make", post(make_icons))
        .route("/api/f/{name}/card", get(app_card).post(make_card))
        .route("/api/f/{name}/members", get(members))
        .route("/api/f/{name}/members/{identity}", put(member).delete(remove_member))
        .route("/api/f/{name}/invites", get(invites).post(invite))
        .route("/api/f/{name}/invites/{email}", axum::routing::delete(revoke_invite))
        .route("/api/f/{name}/visibility", put(visibility))
        .route("/api/f/{name}/rotate", post(rotate))
        .route(
            "/api/f/{name}/channels/chat",
            get(history).post(post_message),
        )
        .route("/f/{name}/__live", get(live))
        .route("/f/{name}", get(page))
        .merge(brain::routes())
        .route("/test/reset", post(reset))
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("fixture port available");
    println!("Finite iOS fixture listening at {address} (no real accounts or models)");
    axum::serve(listener, app).await.expect("fixture server");
}

fn seed(base: String) -> Fixture {
    let mut rooms = HashMap::new();
    rooms.insert("juniper".into(), Room::new("juniper", "agent", "Juniper"));
    for (name, title, question, answer) in [
        ("morning", "A calmer morning", "Help me make a little more room in my mornings.",
         "Start with one small thing.\n\n**Ten quiet minutes**, before your phone. Make coffee, open a window, and let the day arrive.\n\nYou don’t need a perfect routine. Just a little space that belongs to you."),
        ("idea", "A small idea", "I want to keep track of the things I read.",
         "A reading list sounds like a good place to start.\n\nI’ve put an example in your apps. Open **Your Apps → Reading list** to try it without leaving the conversation."),
        ("week", "This week, in focus", "Let’s make this week feel less busy.",
         "Let’s choose three things that matter, and give the rest a little less room.\n\n- One thing to finish\n- One thing to explore\n- One thing just for you\n\nWhat’s the first one?"),
    ] {
        let mut room = Room::new(name, "chat", title);
        room.append("chat", PERSON, json!({"text": question}));
        room.append("chat", AGENT, json!({"text": answer, "turn": format!("seed-{name}")}));
        rooms.insert(name.into(), room);
    }
    rooms.insert(
        "reading".into(),
        Room::new("reading", "app", "Reading list"),
    );
    rooms
        .get_mut("reading")
        .expect("seeded reading app")
        .summary["picture"] = json!("/api/f/reading/icon?v=fixture-reading-v1");
    let mut brain = Room::new("test-brain", "brain", "Sample Brain");
    brain.visibility = "members".into();
    brain.icon = Some(include_bytes!("../assets/reading-icon.png").to_vec());
    brain.summary["picture"] = json!("/api/f/test-brain/icon?v=fixture-brain-v1");
    brain.sync_sharing();
    rooms.insert("test-brain".into(), brain);
    let mut moss = Room::new("moss", "agent", "Moss");
    moss.summary["agents"] = json!(["npub1localmoss"]);
    rooms.insert("moss".into(), moss);
    let mut planning = Room::new("planning", "chat", "A weekend outside");
    planning.summary["agents"] = json!(["npub1localmoss", AGENT]);
    planning.append(
        "chat",
        PERSON,
        json!({"text":"Help me plan a quiet weekend outside."}),
    );
    planning.append("chat", "npub1localmoss", json!({"text":"Let’s find a trail with room to wander. Juniper can help make space in your schedule."}));
    planning.append(
        "chat",
        AGENT,
        json!({"text":"Saturday is a good place to start. Keep the morning free."}),
    );
    rooms.insert("planning".into(), planning);
    let mut archived = Room::new("archived-chat", "chat", "Last summer");
    archived.summary["archived"] = json!(true);
    archived.append(
        "chat",
        AGENT,
        json!({"text":"A few ideas to keep for next summer."}),
    );
    rooms.insert("archived-chat".into(), archived);
    rooms.insert(
        "session-archive".into(),
        Room::new("session-archive", "sessions", "Session Archive"),
    );
    rooms.insert("skills".into(), Room::new("skills", "skills", "Skills"));
    let reading = rooms.get_mut("reading").expect("seeded reading app");
    reading.icon = Some(include_bytes!("../assets/reading-icon.png").to_vec());
    reading.members.push(json!({"principal":"npub1localguest","role":"viewer","kind":"person","email":"reader@finite.local"}));
    reading.sync_sharing();
    let mut shared = Room::new("shared-reading", "app", "Shared reading list");
    shared.owner = "npub1otherperson".into();
    shared.summary["role"] = json!("viewer");
    shared.members = vec![
        json!({"principal":"npub1otherperson","role":"owner","kind":"person"}),
        json!({"principal":PERSON,"role":"viewer","kind":"person","email":"preview@finite.local"}),
    ];
    rooms.insert("shared-reading".into(), shared);
    let mut hidden = Room::new("hidden-notes", "app", "A notebook put away");
    hidden.summary["dock"] = json!(false);
    hidden.sync_sharing();
    rooms.insert("hidden-notes".into(), hidden);
    // A full first page followed by an earlier page exercises real backwards paging.
    let mut archive = Room::new("long-chat", "chat", "A longer conversation");
    for n in 1..=80 {
        archive.append("chat", if n % 2 == 0 { AGENT } else { PERSON }, json!({"text": format!("Earlier thought {n}. A little room to think, one message at a time.")}));
    }
    archive.summary["previewAt"] = json!(1);
    rooms.insert("long-chat".into(), archive);
    Fixture {
        rooms,
        counter: 0,
        base,
        generation: 1,
        exported: false,
    }
}

async fn reset(State(state): State<Shared>) -> Json<Value> {
    let mut fixture = state.lock().expect("fixture lock");
    *fixture = seed(fixture.base.clone());
    Json(json!({"ok": true}))
}
async fn list(State(state): State<Shared>) -> Json<Value> {
    let fixture = state.lock().expect("fixture lock");
    let mut rows: Vec<_> = fixture.rooms.values().map(|r| r.summary.clone()).collect();
    rows.sort_by_key(|r| r["name"].as_str().unwrap_or_default().to_owned());
    Json(json!({"fragments": rows}))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

// This fixture uses literal word matching. Production search remains the
// person's permission-filtered FTS projection, behind the same API shape.
async fn search(
    Query(query): Query<SearchQuery>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let words: Vec<_> = query.q.split_whitespace().map(str::to_lowercase).collect();
    if query.q.len() > 256 || words.len() > 8 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Search accepts at most eight words and 256 bytes.",
        ));
    }
    if words.is_empty() {
        return Ok(Json(json!({"fragments": [], "messages": []})));
    }
    let fixture = state.lock().expect("fixture lock");
    let matches = |text: &str| {
        let lower = text.to_lowercase();
        words.iter().all(|word| lower.contains(word))
    };
    let mut fragments: Vec<_> = fixture
        .rooms
        .values()
        .filter(|room| {
            matches(&format!(
                "{} {}",
                room.summary["name"].as_str().unwrap_or(""),
                room.summary["title"].as_str().unwrap_or("")
            ))
        })
        .map(|room| room.summary.clone())
        .collect();
    fragments.sort_by_key(|fragment| fragment["name"].as_str().unwrap_or("").to_owned());
    fragments.truncate(20);
    let mut messages = Vec::new();
    for (name, room) in &fixture.rooms {
        for record in &room.records {
            if record["channel"] != "chat" {
                continue;
            }
            let kind = record["body"]["kind"].as_str();
            if kind.is_some() && kind != Some("message") {
                continue;
            }
            let Some(text) = record["body"]["text"].as_str() else {
                continue;
            };
            if !matches(text) {
                continue;
            }
            let mut end = text.len().min(300);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            messages.push(json!({"fragment": name, "channel": "chat", "seq": record["seq"], "at": record["at"], "snippet": &text[..end]}));
        }
    }
    messages.sort_by_key(|hit| {
        std::cmp::Reverse((
            hit["at"].as_u64().unwrap_or(0),
            hit["seq"].as_u64().unwrap_or(0),
        ))
    });
    messages.truncate(50);
    Ok(Json(json!({"fragments": fragments, "messages": messages})))
}
async fn create(
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let (kind, prefix, default_title) = match body["template"].as_str() {
        Some("chat") => ("chat", "mobile-chat", "New chat"),
        Some("blank") => ("app", "mobile-app", "New app"),
        _ => {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "Only chat and blank templates are supported by the fixture.",
            ))
        }
    };
    let mut fixture = state.lock().expect("fixture lock");
    if fixture.rooms.len() >= 100 {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Reset the fixture to create more chats.",
        ));
    }
    fixture.counter += 1;
    let name = format!("{prefix}-{}", fixture.counter);
    let title = body["title"].as_str().unwrap_or(default_title);
    let mut room = Room::new(&name, kind, title);
    room.summary["agents"] = json!([]);
    fixture.rooms.insert(name.clone(), room);
    Ok(Json(json!({"name": name})))
}
async fn member(
    Path((name, identity)): Path<(String, String)>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let role = body["role"]
        .as_str()
        .filter(|r| ["viewer", "editor"].contains(r))
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid member role."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    if identity == room.owner {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "The owner keeps ownership.",
        ));
    }
    if room.summary["kind"] == "chat" && room.summary["agents"][0] == identity && role != "editor" {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "The lead agent stays an editor.",
        ));
    }
    if let Some(member) = room.members.iter_mut().find(|m| m["principal"] == identity) {
        member["role"] = json!(role);
    } else {
        if ![AGENT, "npub1localmoss"].contains(&identity.as_str()) {
            return Err(failure(StatusCode::FORBIDDEN, "Invite people by email."));
        }
        if room.members.len() >= 64 {
            return Err(failure(StatusCode::TOO_MANY_REQUESTS, "Too many members."));
        }
        room.members.push(json!({"principal":identity,"role":role,"kind":"agent","owner":PERSON,"addedAt":now(),"addedBy":PERSON}));
    }
    if [AGENT, "npub1localmoss"].contains(&identity.as_str()) {
        let agents = room.summary["agents"].as_array_mut().expect("agent list");
        if !agents.iter().any(|id| id == &identity) {
            agents.push(json!(identity));
        }
    }
    room.sync_sharing();
    Ok(Json(json!({"ok":true,"principal":identity,"role":role})))
}
async fn status(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let fixture = state.lock().expect("fixture lock");
    if name == "add-app" {
        return Ok(Json(
            json!({"npub":AGENT,"urls":{"canonical":format!("{}/f/add-app",fixture.base)},"owner":PERSON,"role":"owner","visibility":"link"}),
        ));
    }
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    let mut value = json!({"name":name,"kind":room.summary["kind"],"title":room.summary["title"],"npub":if name == "moss" {"npub1localmoss"} else {AGENT},"owner":room.owner,"role":room.summary["role"],"visibility":room.visibility,"pins":{"live":"fixture-live"},"urls":{"canonical":format!("{}/f/{name}",fixture.base)}});
    if room.summary["kind"] == "brain" {
        value["urls"]["canonical"] = json!(format!("{}/f/{name}/", fixture.base));
    }
    if room.summary["role"] == "owner" {
        value["viewToken"] = json!(format!("fixture-view-{}", room.token_version));
    }
    if let Some(at) = room.card_at {
        value["page"] = json!({"at":at,"live":"fixture-live","errors":[],"dropped":0});
    }
    Ok(Json(value))
}
#[derive(Deserialize)]
struct History {
    before: Option<u64>,
    after: Option<u64>,
    limit: Option<usize>,
}
async fn history(
    Path(name): Path<String>,
    Query(query): Query<History>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such chat."))?;
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 1_000 || (query.before.is_some() && query.after.is_some()) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Invalid history cursor or limit.",
        ));
    }
    let mut records: Vec<_> = room
        .records
        .iter()
        .filter(|r| {
            r["channel"] == "chat"
                && query
                    .before
                    .is_none_or(|n| r["seq"].as_u64().unwrap_or(0) < n)
                && query
                    .after
                    .is_none_or(|n| r["seq"].as_u64().unwrap_or(0) > n)
        })
        .cloned()
        .collect();
    if query.before.is_some() {
        records = records.into_iter().rev().take(limit).collect();
        records.reverse();
    } else {
        records.truncate(limit);
    }
    let next = if query.before.is_some() {
        records.first()
    } else {
        records.last()
    }
    .and_then(|r| r["seq"].as_u64())
    .unwrap_or(0);
    Ok(Json(
        json!({"channel": "chat", "records": records, "next": next}),
    ))
}

#[derive(Deserialize)]
struct Post {
    id: String,
    body: Value,
}
async fn post_message(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(post): Json<Post>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such chat."))?;
    if post.id.is_empty() || post.id.len() > 128 {
        return Err(failure(StatusCode::BAD_REQUEST, "Invalid post id."));
    }
    if let Some((body, record)) = room.posts.get(&post.id) {
        if body != &post.body {
            return Err(failure(
                StatusCode::CONFLICT,
                "Post id already used for another body.",
            ));
        }
        return Ok(Json(json!({"record": record, "replayed": true})));
    }
    if room.posts.len() >= 1_000 {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Reset the fixture to send more messages.",
        ));
    }
    if post.body["text"]
        .as_str()
        .is_some_and(|text| text.len() > 32_768)
    {
        return Err(failure(StatusCode::PAYLOAD_TOO_LARGE, "Message too large."));
    }
    let record = room.append("chat", PERSON, post.body.clone());
    room.posts
        .insert(post.id.clone(), (post.body.clone(), record.clone()));
    if post.body["kind"] == "stop" {
        if let Some(turn) = post.body["turn"].as_str() {
            room.turns.insert(turn.into(), false);
        }
    } else if let Some(text) = post.body["text"].as_str() {
        let text = text.to_owned();
        let available = room.summary["agents"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let targets: Vec<String> = if let Some(to) = post.body["to"].as_array() {
            to.iter()
                .filter(|id| available.contains(id))
                .filter_map(|id| id.as_str().map(str::to_owned))
                .take(16)
                .collect()
        } else {
            available
                .iter()
                .take(1)
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        };
        if room.summary["title"] == "New chat" {
            room.summary["title"] = json!(text.chars().take(48).collect::<String>());
        }
        for agent in targets {
            let turn = format!("local-{}-{agent}", post.id);
            room.turns.insert(turn.clone(), true);
            room.append(
                "work",
                &agent,
                json!({"kind":"turn.start","turn":turn,"asker":PERSON,"agent":agent}),
            );
            tokio::spawn(reply(
                state.clone(),
                name.clone(),
                turn,
                text.clone(),
                agent,
            ));
        }
        // Deliberately lose the acknowledgement after commit. Retrying must
        // return that same record and never start a second turn.
        if text.contains("/fail-once") {
            return Err(failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Fixture: acknowledgement lost. Tap retry.",
            ));
        }
    }
    Ok(Json(json!({"record": record, "replayed": false})))
}

async fn reply(state: Shared, name: String, turn: String, prompt: String, agent: String) {
    tokio::time::sleep(Duration::from_millis(450)).await;
    let answer = if prompt.contains("code") || prompt.contains("markdown") {
        "Here’s a small starting point.\n\n## Keep it simple\n\nOne clear idea is enough to begin.\n\n```swift\nstruct Idea: Identifiable {\n    let id: UUID\n    var title: String\n}\n```\n\n- Make the first version small\n- Try it with real people\n- Keep what feels useful\n\n**What would you like to try first?**"
    } else {
        "Let’s make a little room for that.\n\n**Start small.** Pick the part that matters most to you, and we can work through it together. There’s no need to have it all figured out yet.\n\nIf it helps, we can turn the idea into a simple app — something that fits the way you think and grows with you.\n\nWhat would a useful first version look like?"
    };
    let mut text = String::new();
    let mut disconnected = false;
    for word in answer.split_inclusive(' ') {
        tokio::time::sleep(Duration::from_millis(65)).await;
        let mut fixture = state.lock().expect("fixture lock");
        let Some(room) = fixture.rooms.get_mut(&name) else {
            return;
        };
        if room.turns.get(&turn) != Some(&true) {
            room.append(
                "work",
                &agent,
                json!({"kind": "turn.end", "turn": turn, "status": "stopped"}),
            );
            let _ = room.tx.send(json!({"type": "draft", "channel": "chat", "principal": &agent, "turn": turn, "text": null, "at": now()}));
            room.turns.remove(&turn);
            return;
        }
        text.push_str(word);
        let _ = room.tx.send(json!({"type": "draft", "channel": "chat", "principal": &agent, "turn": turn, "text": text, "at": now()}));
        if prompt.contains("/disconnect") && !disconnected && text.len() > 40 {
            let _ = room.tx.send(json!({"type": "fixture_disconnect"}));
            disconnected = true;
        }
    }
    let mut fixture = state.lock().expect("fixture lock");
    if let Some(room) = fixture.rooms.get_mut(&name) {
        room.append("chat", &agent, json!({"text": text, "turn": turn}));
        room.append(
            "work",
            &agent,
            json!({"kind": "turn.end", "turn": turn, "status": "ok"}),
        );
        room.turns.remove(&turn);
    }
}

async fn live(
    Path(name): Path<String>,
    State(state): State<Shared>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| serve_socket(socket, state, name))
}
async fn send(socket: &mut WebSocket, value: Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}
async fn serve_socket(mut socket: WebSocket, state: Shared, name: String) {
    let mut receiver = {
        let fixture = state.lock().expect("fixture lock");
        let Some(room) = fixture.rooms.get(&name) else {
            return;
        };
        room.tx.subscribe()
    };
    if !send(&mut socket, json!({"type": "hello", "id": "local", "principal": PERSON, "role": "owner", "platform": "http://localhost:8910", "presence": []})).await { return; }
    let mut subscriptions: HashMap<String, u64> = HashMap::new();
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { return };
                let bytes = match message { Message::Text(text) => text.as_bytes().to_vec(), Message::Close(_) => return, _ => continue };
                let Ok(frame) = serde_json::from_slice::<Value>(&bytes) else { return };
                if frame["type"] == "ping" { if !send(&mut socket, json!({"type": "pong"})).await { return; } }
                else if frame["type"] == "subscribe" {
                    let channel = frame["channel"].as_str().unwrap_or("").to_owned();
                    if !["chat", "work"].contains(&channel.as_str()) { return; }
                    let records = {
                        let fixture = state.lock().expect("fixture lock");
                        let Some(room) = fixture.rooms.get(&name) else { return };
                        let mut records: Vec<_> = room.records.iter().filter(|r| r["channel"] == channel).cloned().collect();
                        if let Some(after) = frame["after"].as_u64() { records.retain(|r| r["seq"].as_u64().unwrap_or(0) > after); }
                        else if let Some(last) = frame["last"].as_u64() { records = records.into_iter().rev().take(last.min(1_000) as usize).collect(); records.reverse(); }
                        else { return; }
                        records
                    };
                    let mut next = frame["after"].as_u64().unwrap_or(0);
                    for mut record in records {
                        next = record["seq"].as_u64().unwrap_or(next);
                        record["type"] = json!("record");
                        if !send(&mut socket, record).await { return; }
                    }
                    subscriptions.insert(channel.clone(), next);
                    if !send(&mut socket, json!({"type": "subscribed", "channel": channel, "next": next, "more": false})).await { return; }
                }
            }
            outgoing = receiver.recv() => {
                let Ok(frame) = outgoing else { return };
                if frame["type"] == "fixture_disconnect" { let _ = socket.send(Message::Close(None)).await; return; }
                let channel = frame["channel"].as_str().unwrap_or("");
                let Some(cursor) = subscriptions.get_mut(channel) else { continue };
                if frame["type"] == "record" {
                    let seq = frame["seq"].as_u64().unwrap_or(0);
                    if seq <= *cursor { continue; }
                    *cursor = seq;
                }
                if !send(&mut socket, frame).await { return; }
            }
        }
    }
}

async fn page(Path(name): Path<String>) -> Result<Html<&'static str>, StatusCode> {
    if name == "add-app" {
        return Ok(Html(
            r#"<!doctype html><html><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><style>body{font:17px/1.5 system-ui;margin:28px;background:light-dark(#f7f6f3,#181818);color:light-dark(#171717,#eee)}h1{font-size:28px}a{display:block;color:inherit;text-decoration:none;padding:18px;margin:14px 0;border:1px solid #8885;border-radius:18px}</style><h1>Ask for an app</h1><p>Tell your agent who it’s for and what it should do. Pick an idea and make it yours.</p><a href="finite-new-chat://start?prompt=Make%20a%20shared%20grocery%20list%20my%20family%20can%20use">A grocery list for the family</a><a href="finite-new-chat://start?prompt=Make%20a%20reading%20list%20for%20things%20I%20want%20to%20come%20back%20to">A reading list worth returning to</a><a href="finite-new-chat://start?prompt=Every%20morning%20send%20me%20a%20summary%20of%20the%20sites%20I%20follow">A little morning briefing</a></html>"#,
        ));
    }
    if name == "session-archive" {
        return Ok(Html(
            r#"<!doctype html><html><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><style>body{font:17px/1.6 system-ui;margin:28px}</style><small>IMPORTED · READ ONLY</small><h1>Ideas brought along</h1><p>A few thoughts from an earlier conversation.</p><h2>You</h2><p>How could I make more room for reading?</p><h2>Juniper</h2><p>Start with ten quiet minutes and something that makes you curious.</p></html>"#,
        ));
    }
    if name == "skills" {
        return Ok(Html(
            r#"<!doctype html><html><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><style>body{font:17px/1.6 system-ui;margin:28px}</style><h1>Skills</h1><p>What your agents know how to do, kept in one place.</p><h2>Getting started</h2><p>Ask Juniper or Moss to help build an app, plan your week, or make room for an idea.</p><small>Local preview · Example skills library</small></html>"#,
        ));
    }
    if !["reading", "shared-reading", "hidden-notes"].contains(&name.as_str()) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Html(
        r#"<!doctype html><html><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><style>
    :root{font:17px/1.6 system-ui;color-scheme:light dark}body{margin:32px 24px;background:light-dark(#f7f6f3,#181818);color:light-dark(#171717,#ececec)}h1{font:36px Georgia}label{display:flex;gap:14px;padding:18px 0;border-bottom:1px solid #8884}input{width:20px;accent-color:#a88bea}small{color:#888}p{max-width:30em}
    </style><small>YOUR READING LIST</small><h1>A few good things.</h1><p>A place for the things you want to come back to.</p><label><input type="checkbox">A book that changes your mind</label><label><input type="checkbox">An essay worth sitting with</label><label><input type="checkbox">Something a friend sent you</label><p><small>Local preview · Example fragment</small></p></html>"#,
    ))
}

async fn archive(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let archived = body["archived"]
        .as_bool()
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "archived must be a boolean."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    room.summary["archived"] = json!(archived);
    Ok(Json(json!({"ok":true})))
}
async fn manifest(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    Ok(Json(json!({"meta":{"title":room.summary["title"]}})))
}
async fn files(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let text = body["files"][0]["text"]
        .as_str()
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Missing manifest."))?;
    let manifest: Value = serde_json::from_str(text)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Invalid manifest."))?;
    let title = manifest["meta"]["title"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= 120)
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid title."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    room.staged_title = Some(title.into());
    Ok(Json(json!({"ok":true})))
}
async fn deploy(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    if let Some(title) = room.staged_title.take() {
        room.summary["title"] = json!(title);
    }
    Ok(Json(json!({"ok":true})))
}
async fn upload(
    Path((name, hash)): Path<(String, String)>,
    State(state): State<Shared>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>, Failure> {
    if bytes.is_empty()
        || bytes.len() > 25 * 1024 * 1024
        || format!("{:x}", Sha256::digest(&bytes)) != hash
    {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Invalid blob size or hash.",
        ));
    }
    let mut fixture = state.lock().expect("fixture lock");
    let total: usize = fixture
        .rooms
        .values()
        .flat_map(|r| r.blobs.values())
        .map(|(bytes, _)| bytes.len())
        .sum();
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such chat."))?;
    let stored = !room.blobs.contains_key(&hash);
    if stored && total + bytes.len() > 128 * 1024 * 1024 {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Reset fixture to upload more files.",
        ));
    }
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    room.blobs
        .entry(hash.clone())
        .or_insert((bytes.to_vec(), content_type));
    Ok(Json(
        json!({"ok":true,"sha":hash,"size":bytes.len(),"stored":stored}),
    ))
}
async fn blob(
    Path((name, hash)): Path<(String, String)>,
    State(state): State<Shared>,
) -> Result<impl IntoResponse, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let (bytes, content_type) = fixture
        .rooms
        .get(&name)
        .and_then(|r| r.blobs.get(&hash))
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such file."))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_str(content_type)
            .map_err(|_| failure(StatusCode::BAD_REQUEST, "Invalid content type."))?,
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((headers, bytes.clone()))
}
fn computer(generation: u64) -> Value {
    json!({"computer":"local","state":"awake","version":"local-preview","generation":generation})
}
async fn computers(State(state): State<Shared>) -> Json<Value> {
    Json(json!({"computers":[computer(state.lock().expect("fixture lock").generation)]}))
}
async fn restart(
    Path(id): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    if id != "local" {
        return Err(failure(StatusCode::NOT_FOUND, "No such computer."));
    }
    let generation = body["generation"]
        .as_u64()
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Missing generation."))?;
    if generation > fixture.generation {
        return Err(failure(StatusCode::CONFLICT, "Unknown generation."));
    }
    if generation == fixture.generation {
        fixture.generation += 1;
    }
    Ok(Json(computer(fixture.generation)))
}
async fn export_status(State(state): State<Shared>) -> Json<Value> {
    let fixture = state.lock().expect("fixture lock");
    Json(
        json!({"export": if fixture.exported { json!({"state":"ready","url":format!("{}/export.json",fixture.base)}) } else { Value::Null }}),
    )
}
async fn export(State(state): State<Shared>) -> Json<Value> {
    let mut fixture = state.lock().expect("fixture lock");
    fixture.exported = true;
    Json(json!({"state":"ready","url":format!("{}/export.json",fixture.base)}))
}
async fn export_download(State(state): State<Shared>) -> Json<Value> {
    let fixture = state.lock().expect("fixture lock");
    Json(
        json!({"fixture":true,"fragments":fixture.rooms.values().map(|r| json!({"fragment":r.summary,"records":r.records})).collect::<Vec<_>>()}),
    )
}

async fn archive_index(
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, Failure> {
    if name != "session-archive" || query.get("path").map(String::as_str) != Some("index/chat.json")
    {
        return Err(failure(StatusCode::NOT_FOUND, "No such file."));
    }
    Ok(Json(
        json!({"sessions":[{"key":"imported-chat","title":"Ideas brought along","preview":"A few thoughts from an earlier conversation.","started_at":"2026-10-01T12:00:00Z","ended_at":"2026-10-01T13:00:00Z"}]}),
    ))
}
async fn archive_search(
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    if name != "session-archive" {
        return Err(failure(StatusCode::NOT_FOUND, "No such archive."));
    }
    let query = body["q"].as_str().unwrap_or("").to_lowercase();
    let text = "Ideas brought along. A few thoughts from an earlier conversation. How could I make more room for reading? Start with ten quiet minutes and something that makes you curious.";
    let matched = !query.is_empty()
        && query
            .split_whitespace()
            .all(|word| text.to_lowercase().contains(word));
    Ok(Json(
        json!({"results":if matched { vec![json!({"source":"sessions/imported-chat.json","snippet":text})] } else { vec![] }}),
    ))
}

fn require_owner(room: &Room) -> Result<(), Failure> {
    if room.summary["role"] != "owner" {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "Only the owner can change sharing.",
        ));
    }
    Ok(())
}
fn require_editor(room: &Room) -> Result<(), Failure> {
    if room.summary["role"] != "owner" && room.summary["role"] != "editor" {
        return Err(failure(StatusCode::FORBIDDEN, "An editor is required."));
    }
    Ok(())
}
async fn dock(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let shown = body["dock"]
        .as_bool()
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "dock must be a boolean."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    room.summary["dock"] = json!(shown);
    Ok(Json(json!({"name":name,"dock":shown})))
}
async fn members(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    Ok(Json(json!({"members":room.members})))
}
async fn remove_member(
    Path((name, identity)): Path<(String, String)>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    if identity == room.owner {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "The owner keeps ownership.",
        ));
    }
    room.members.retain(|m| m["principal"] != identity);
    room.sync_sharing();
    Ok(Json(json!({"ok":true,"removed":identity})))
}
async fn invites(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    Ok(Json(json!({"invites":room.invites})))
}
async fn invite(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let email = body["email"]
        .as_str()
        .filter(|s| s.contains('@') && s.len() <= 254 && !s.chars().any(char::is_whitespace))
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid email."))?
        .to_lowercase();
    let role = body["role"]
        .as_str()
        .filter(|r| ["viewer", "editor"].contains(r))
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid invitation role."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    if room.members.iter().any(|m| m["email"] == email) {
        return Err(failure(StatusCode::BAD_REQUEST, "Already a member."));
    }
    if room.invites.len() >= 64 && !room.invites.iter().any(|i| i["email"] == email) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many invitations.",
        ));
    }
    let invitation =
        json!({"email":email,"role":role,"expiresAt":now()+7*24*60*60*1000,"createdBy":PERSON});
    room.invites.retain(|i| i["email"] != email);
    room.invites.push(invitation.clone());
    room.sync_sharing();
    Ok(Json(json!({"invited":invitation})))
}
async fn revoke_invite(
    Path((name, email)): Path<(String, String)>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    let before = room.invites.len();
    room.invites.retain(|i| i["email"] != email);
    if before == room.invites.len() {
        return Err(failure(StatusCode::NOT_FOUND, "No such invitation."));
    }
    room.sync_sharing();
    Ok(Json(json!({"ok":true,"revoked":email})))
}
async fn visibility(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    let visibility = body["visibility"]
        .as_str()
        .filter(|v| ["members", "link", "public"].contains(v))
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Invalid visibility."))?;
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    if room.visibility == "members" && visibility != "members" {
        room.token_version += 1;
    }
    room.visibility = visibility.into();
    room.sync_sharing();
    Ok(Json(json!({"ok":true,"visibility":visibility})))
}
async fn rotate(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    if body["scopes"] != json!(["view"]) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Only the view link is rotated in this fixture.",
        ));
    }
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_owner(room)?;
    room.token_version += 1;
    Ok(Json(
        json!({"viewToken":format!("fixture-view-{}",room.token_version),"rotated":["view"]}),
    ))
}
async fn app_icon(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<impl IntoResponse, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    let data = room
        .icon
        .clone()
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No icon yet."))?;
    Ok((
        [
            ("content-type", "image/png"),
            ("cache-control", "private, no-cache"),
        ],
        data,
    ))
}
async fn make_icons(
    Path(name): Path<String>,
    State(state): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Failure> {
    if body["hint"]
        .as_str()
        .is_some_and(|s| s.chars().count() > 200)
        || (!body["hint"].is_null() && !body["hint"].is_string())
    {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Icon hints are at most 200 characters.",
        ));
    }
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_editor(room)?;
    let png = base64::engine::general_purpose::STANDARD
        .encode(include_bytes!("../assets/reading-icon.png"));
    Ok(Json(
        json!({"subject":"A notebook (local fixture)","candidates":[{"png":png,"score":1.0}],"flaws":{}}),
    ))
}
fn fit_icon(bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    if bytes.is_empty() || bytes.len() > 2 * 1024 * 1024 {
        return Err(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Choose an image smaller than 2 MB.",
        ));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Unreadable image."))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Unreadable image."))?
        .thumbnail(256, 256)
        .to_rgba8();
    let mut canvas = image::RgbaImage::new(256, 256);
    image::imageops::overlay(
        &mut canvas,
        &image,
        ((256 - image.width()) / 2).into(),
        ((256 - image.height()) / 2).into(),
    );
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Icon could not be fitted."))?;
    let png = out.into_inner();
    if png.len() > 256 * 1024 {
        return Err(failure(StatusCode::PAYLOAD_TOO_LARGE, "Icon is too large."));
    }
    Ok(png)
}
async fn put_icon(
    Path(name): Path<String>,
    State(state): State<Shared>,
    bytes: Bytes,
) -> Result<Json<Value>, Failure> {
    {
        let fixture = state.lock().expect("fixture lock");
        let room = fixture
            .rooms
            .get(&name)
            .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
        require_editor(room)?;
    }
    let png = fit_icon(&bytes)?;
    let hash = format!("{:x}", Sha256::digest(&png));
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    let address = format!("/api/f/{name}/icon?v={}", &hash[..12]);
    room.summary["picture"] = json!(address);
    room.icon = Some(png.clone());
    Ok(Json(json!({"icon":address,"sha256":hash,"size":png.len()})))
}
async fn remove_icon(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_editor(room)?;
    room.icon = None;
    room.summary
        .as_object_mut()
        .expect("summary object")
        .remove("picture");
    Ok(Json(json!({"icon":null})))
}
async fn app_card(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<impl IntoResponse, Failure> {
    let fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    let bytes = room
        .card
        .clone()
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No preview yet."))?;
    Ok((
        [
            ("content-type", "image/jpeg"),
            ("cache-control", "private, no-cache"),
        ],
        bytes,
    ))
}
async fn make_card(
    Path(name): Path<String>,
    State(state): State<Shared>,
) -> Result<Json<Value>, Failure> {
    let mut fixture = state.lock().expect("fixture lock");
    let room = fixture
        .rooms
        .get_mut(&name)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "No such fragment."))?;
    require_editor(room)?;
    if room.visibility == "members" {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Restricted apps are not photographed.",
        ));
    }
    if let Some(since) = room
        .card_requested
        .filter(|since| room.card_at.is_none_or(|at| at < *since))
    {
        return Ok(Json(
            json!({"ok":true,"since":since,"live":"fixture-live","joined":true}),
        ));
    }
    let since = now();
    room.card_requested = Some(since);
    let pending = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut fixture = pending.lock().expect("fixture lock");
        if let Some(room) = fixture.rooms.get_mut(&name) {
            if room.card_requested == Some(since) {
                room.card = Some(include_bytes!("../assets/reading-preview.jpg").to_vec());
                room.card_at = Some(now());
            }
        }
    });
    Ok(Json(
        json!({"ok":true,"since":since,"live":"fixture-live","joined":false}),
    ))
}
