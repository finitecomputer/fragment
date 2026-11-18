//! SPIKE (branch spike/simplex-door, never merged): the SimpleX door's
//! runner, one per door container (cell/door.mjs runs the container and
//! keeps its saves). It:
//!
//! - restores the door's newest save from its Durable Object
//!   (`GET {api}/save`), or starts with none;
//! - runs SimpleX's own core (`simplex-chat`, pinned in the Dockerfile) with
//!   its bot API on loopback, files through the platform's XFTP relay
//!   (the bridge's `socks`, its relay at the door's own egress);
//! - saves after every change: each database's `data_version` is watched,
//!   and a change is saved about a second later (`save.rs`) to
//!   `PUT {api}/save`, fenced by this start's generation;
//! - posts each message its owner's contact sends to the connector's inbox
//!   (`config.inbox`, with the connector's secret in its header), in order,
//!   reading on from its cursor at every start (events committed before a
//!   kill are never announced again);
//! - sends what the door's Durable Object asks (`POST /send`), and makes a
//!   one-time invitation when asked (`POST /link`);
//! - syncs a ratchet a rollback broke as soon as the core says so.
//!
//! Its HTTP API on `DOOR_HTTP` (8080) is the Durable Object's alone.

mod core;
mod http;
mod save;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fragment_bridge::api::RelayOffer;
use fragment_bridge::net::{self, Body};
use fragment_bridge::simplex::wire::{self, Cmd, Content, Resp, SyncStatus};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Notify};

use crate::core::{Core, Event};

const CORE_PORT: u16 = 5225;
const SOCKS_PORT: u16 = 1080;
const ASK_WAIT: Duration = Duration::from_secs(30);
/// A change is saved this long after it is seen (more changes meanwhile go
/// in the same save).
const DEBOUNCE_MS: u64 = 1000;
/// The databases' versions are read this often.
const WATCH_MS: u64 = 250;
/// A forced sync's answer is waited for this long before the door sends.
const FORCED_SYNC_WAIT: Duration = Duration::from_secs(60);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

macro_rules! log {
    ($($t:tt)*) => { println!("{}", json!({ "at": now_ms(), $($t)* })) };
}

/// The door's state, rolled back with its databases (it is in each save).
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct DoorState {
    /// The owner's contact, once linked or adopted.
    contact: Option<i64>,
    /// The newest of its items posted to the inbox.
    delivered: i64,
    /// A one-time invitation's connection, while one waits.
    invitation: Option<i64>,
}

/// What the door's Durable Object says (`GET {api}/config`, `POST /config`).
#[derive(Deserialize, Default, Clone, Debug)]
struct Config {
    inbox: Option<String>,
    secret: Option<String>,
    header: Option<String>,
    relay: Option<RelayOffer>,
    name: Option<String>,
    ttl: Option<String>,
}

struct Door {
    api: String,
    dir: PathBuf,
    generation: u64,
    started: Instant,
    seq: AtomicU64,
    state: Mutex<DoorState>,
    config: Mutex<Config>,
    core: tokio::sync::OnceCell<Arc<Core>>,
    dirty: Notify,
    deliver: mpsc::UnboundedSender<wire::Item>,
    relay: watch::Sender<Option<RelayOffer>>,
    ready: AtomicBool,
    saves: AtomicU64,
    last_save: Mutex<Value>,
    delivered_n: AtomicU64,
    sent_n: AtomicU64,
    syncs: AtomicU64,
    saving: tokio::sync::Mutex<()>,
}

impl Door {
    async fn to_do(&self, method: &str, path: &str, headers: &[(&str, String)], body: Bytes) -> Result<http::Answer, String> {
        let mut h: Vec<(&str, String)> = vec![("x-door-generation", self.generation.to_string())];
        h.extend(headers.iter().cloned());
        http::call(method, &format!("{}{path}", self.api), &h, body, Duration::from_secs(60)).await
    }

    fn stat(self: &Arc<Self>, kind: &str, data: Value) {
        let d = self.clone();
        let body = json!({ "kind": kind, "data": data }).to_string();
        tokio::spawn(async move {
            let _ = d.to_do("POST", "/stat", &[("content-type", "application/json".into())], Bytes::from(body)).await;
        });
    }

    fn core(&self) -> Option<Arc<Core>> {
        self.core.get().cloned()
    }

    async fn ask(&self, cmd: &Cmd) -> Result<(Resp, String), String> {
        let core = self.core().ok_or("the core is not up")?;
        let text = core.ask(&cmd.line(), ASK_WAIT).await?;
        let frame = wire::parse(&text).map_err(|e| format!("{e:?}"))?;
        Ok((frame.resp, text))
    }

    fn state_json(&self) -> Vec<u8> {
        serde_json::to_vec(&*self.state.lock().expect("state")).expect("state is JSON")
    }

    /// One save, now: the databases copied and packed, and put to the
    /// Durable Object under this start's generation.
    async fn save(self: &Arc<Self>, why: &str) -> Result<Value, String> {
        let _one = self.saving.lock().await;
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let state = self.state_json();
        let (dir, work) = (self.dir.clone(), self.dir.join("save"));
        let t0 = Instant::now();
        let (packed, timing) = tokio::task::spawn_blocking(move || save::make(&dir, &work, &state)).await.map_err(|e| e.to_string())??;
        let t1 = Instant::now();
        let sha = {
            use sha2::Digest;
            let d = sha2::Sha256::digest(&packed);
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let r = self.to_do("PUT", &format!("/save?seq={seq}"), &[("content-type", "application/octet-stream".into()), ("x-save-sha256", sha.clone())], Bytes::from(packed)).await?;
        let put_ms = t1.elapsed().as_millis() as u64;
        if r.status != 200 {
            return Err(format!("the save {seq} was refused: {} {}", r.status, String::from_utf8_lossy(&r.body)));
        }
        let n = self.saves.fetch_add(1, Ordering::SeqCst) + 1;
        let v = json!({ "seq": seq, "why": why, "vacuumMs": timing.vacuum_ms, "packMs": timing.pack_ms, "putMs": put_ms, "totalMs": t0.elapsed().as_millis() as u64, "rawBytes": timing.raw_bytes, "packedBytes": timing.packed_bytes, "saves": n, "sha": sha });
        *self.last_save.lock().expect("last") = v.clone();
        Ok(v)
    }
}

#[tokio::main]
async fn main() {
    let api = std::env::var("DOOR_API").unwrap_or_else(|_| "http://door.fragment.internal".into());
    let dir = PathBuf::from(std::env::var("DOOR_DIR").unwrap_or_else(|_| "/data/door".into()));
    let bin = std::env::var("DOOR_CORE_BIN").unwrap_or_else(|_| "/opt/door/simplex-chat".into());
    let generation: u64 = std::env::var("DOOR_GENERATION").ok().and_then(|g| g.parse().ok()).unwrap_or(0);
    let http_listen = std::env::var("DOOR_HTTP").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let started = Instant::now();
    log!("runner": "start", "generation": generation, "api": api);
    std::fs::create_dir_all(dir.join("files")).expect("the door's folder");
    std::fs::create_dir_all(dir.join("tmp")).expect("the door's folder");

    let (deliver_tx, deliver_rx) = mpsc::unbounded_channel();
    let (relay_tx, relay_rx) = watch::channel(None);
    let door = Arc::new(Door {
        api,
        dir: dir.clone(),
        generation,
        started,
        seq: AtomicU64::new(0),
        state: Mutex::new(DoorState::default()),
        config: Mutex::new(Config::default()),
        core: tokio::sync::OnceCell::new(),
        dirty: Notify::new(),
        deliver: deliver_tx,
        relay: relay_tx,
        ready: AtomicBool::new(false),
        saves: AtomicU64::new(0),
        last_save: Mutex::new(Value::Null),
        delivered_n: AtomicU64::new(0),
        sent_n: AtomicU64::new(0),
        syncs: AtomicU64::new(0),
        saving: tokio::sync::Mutex::new(()),
    });

    // the Durable Object's HTTP API first, so it can see the runner come up
    // held for the runner's life: its sender dropped would end the servers
    let (_stop_tx, stop_rx) = watch::channel(false);
    let listener = tokio::net::TcpListener::bind(&http_listen).await.expect("the runner's port");
    let d = door.clone();
    tokio::spawn(net::serve(listener, move |req, _| handle(d.clone(), req), stop_rx.clone()));

    // 1. the newest save, or none
    let t = Instant::now();
    let restored = loop {
        match door.to_do("GET", "/save", &[], Bytes::new()).await {
            Ok(a) if a.status == 200 => break Some(a),
            Ok(a) if a.status == 204 => break None,
            Ok(a) => log!("restore": "refused", "status": a.status, "body": String::from_utf8_lossy(&a.body)),
            Err(e) => log!("restore": "failed", "error": e),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    let fetch_ms = t.elapsed().as_millis() as u64;
    let mut restore = json!({ "fetchMs": fetch_ms, "save": null });
    if let Some(a) = restored {
        let t = Instant::now();
        let files = save::unpack(&a.body).expect("the save unpacks");
        for (name, bytes) in &files {
            if name == save::STATE {
                *door.state.lock().expect("state") = serde_json::from_slice(bytes).unwrap_or_default();
            } else {
                let tmp = dir.join(format!("{name}.restoring"));
                std::fs::write(&tmp, bytes).expect("a restored database");
                std::fs::rename(&tmp, dir.join(name)).expect("a restored database");
                for s in ["-wal", "-shm"] {
                    let _ = std::fs::remove_file(dir.join(format!("{name}{s}")));
                }
            }
        }
        restore = json!({ "fetchMs": fetch_ms, "unpackMs": t.elapsed().as_millis() as u64, "packedBytes": a.body.len(), "save": a.header("x-save-seq"), "saveGeneration": a.header("x-save-generation"), "behind": a.header("x-save-behind") == Some("1"), "state": *door.state.lock().expect("state") });
    }
    log!("restore": restore);

    // 2. its config
    loop {
        match door.to_do("GET", "/config", &[], Bytes::new()).await {
            Ok(a) if a.status == 200 => {
                let c: Config = serde_json::from_slice(&a.body).unwrap_or_default();
                let _ = door.relay.send(c.relay.clone());
                *door.config.lock().expect("config") = c;
                break;
            }
            other => log!("config": "failed", "answer": other.map(|a| a.status).map_err(|e| e)),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // 3. both databases in WAL mode (save.rs `wal` says why), the XFTP
    // relay's way out, then the core
    let modes = save::wal(&dir).expect("the databases in WAL mode");
    log!("journal": modes);
    let socks = tokio::net::TcpListener::bind(("127.0.0.1", SOCKS_PORT)).await.expect("the SOCKS port");
    let relay_api = fragment_bridge::api::Api::new(&door.api).expect("the relay's base");
    tokio::spawn(fragment_bridge::simplex::socks::serve(socks, relay_api, relay_rx, stop_rx.clone()));
    let name = door.config.lock().expect("config").name.clone().unwrap_or_else(|| "Agent".into());
    let name = wire::valid_name(&name);
    let mut child = tokio::process::Command::new(&bin)
        .args(["-d", &dir.join("sx").to_string_lossy(), "-p", &CORE_PORT.to_string()])
        .args(["--files-folder", &dir.join("files").to_string_lossy(), "--temp-folder", &dir.join("tmp").to_string_lossy()])
        .args(["-y", "--mute", "--create-bot-display-name", &name, "--create-bot-allow-files", "--smp-web-port-servers", "off"])
        .args(["--socks-proxy", &format!("127.0.0.1:{SOCKS_PORT}"), "--host-mode", "public"])
        .kill_on_drop(true)
        .spawn()
        .expect("the core starts");
    let spawned_ms = started.elapsed().as_millis() as u64;
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let core = loop {
        match Core::connect(CORE_PORT, ev_tx.clone()).await {
            Ok(c) => break c,
            Err(_) => {
                if let Ok(Some(status)) = child.try_wait() {
                    log!("core": "exited", "status": status.to_string());
                    std::process::exit(1);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    let _ = door.core.set(core);
    let api_ms = started.elapsed().as_millis() as u64;
    // its message expiry: the chat in Fragment is the record, not the core
    let ttl = door.config.lock().expect("config").ttl.clone().unwrap_or_else(|| "day".into());
    let ttl_answer = door.core().expect("up").ask(&format!("/ttl {ttl}"), ASK_WAIT).await;
    log!("ttl": ttl, "answer": ttl_answer.map(|t| wire::parse(&t).map(|f| format!("{:?}", f.resp)).unwrap_or(t)).unwrap_or_else(|e| e));

    // 4. its contact's ratchet, then what arrived since the cursor
    let contact = door.state.lock().expect("state").contact;
    let mut ratchet = Value::Null;
    let behind = restore["behind"] == true;
    let mut forced = Value::Null;
    if let Some(c) = contact {
        if let Ok((Resp::ContactInfo { status, .. }, _)) = door.ask(&Cmd::ContactInfo { contact: c }).await {
            ratchet = json!(format!("{status:?}"));
            if behind {
                // a save older than what the door last said or delivered:
                // its contact may hold ratchet steps this copy never took,
                // which no unforced sync sees (the core thinks it is in
                // step), so one forced sync before anything is sent
                forced = force_sync(&door, c).await;
            } else if matches!(status, SyncStatus::Required | SyncStatus::Allowed) {
                sync(&door, c).await;
            }
        }
        catch_up(&door, c).await;
    }
    let ready_ms = started.elapsed().as_millis() as u64;
    door.ready.store(true, Ordering::SeqCst);
    let started_stat = json!({ "generation": generation, "restore": restore, "spawnedMs": spawned_ms, "apiMs": api_ms, "readyMs": ready_ms, "ratchet": ratchet, "contact": contact, "forcedSync": forced });
    log!("ready": started_stat);
    door.stat("start", started_stat);

    // 5. the loops: deliveries, saves, events, signals
    tokio::spawn(deliveries(door.clone(), deliver_rx));
    tokio::spawn(saves(door.clone()));
    let d = door.clone();
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
        term.recv().await;
        log!("signal": "SIGTERM");
        match d.save("sigterm").await {
            Ok(v) => log!("save": v),
            Err(e) => log!("save": "failed", "error": e),
        }
        std::process::exit(0);
    });
    loop {
        tokio::select! {
            ev = ev_rx.recv() => match ev {
                Some(Event::Frame(text)) => on_event(&door, &text).await,
                Some(Event::Closed) | None => {
                    log!("core": "closed");
                    std::process::exit(1);
                }
            },
            exited = child.wait() => {
                log!("core": "exited", "status": format!("{exited:?}"));
                std::process::exit(1);
            }
        }
    }
}

async fn sync(door: &Arc<Door>, contact: i64) {
    let t = Instant::now();
    let r = door.ask(&Cmd::Sync { contact, force: false }).await;
    door.syncs.fetch_add(1, Ordering::SeqCst);
    let said = r.as_ref().map(|(resp, _)| format!("{resp:?}")).unwrap_or_else(|e| e.clone());
    log!("sync": said, "ms": t.elapsed().as_millis() as u64);
    door.stat("sync", json!({ "answer": said, "ms": t.elapsed().as_millis() as u64 }));
}

/// One forced sync, then its contact's ratchet read until it is in step
/// (at most `FORCED_SYNC_WAIT`: a phone offline answers later, and sends
/// wait until then).
async fn force_sync(door: &Arc<Door>, contact: i64) -> Value {
    let t = Instant::now();
    let asked = door.ask(&Cmd::Sync { contact, force: true }).await.map(|(r, _)| format!("{r:?}")).unwrap_or_else(|e| e);
    door.syncs.fetch_add(1, Ordering::SeqCst);
    let mut status = String::new();
    while t.elapsed() < FORCED_SYNC_WAIT {
        if let Ok((Resp::ContactInfo { status: s, .. }, _)) = door.ask(&Cmd::ContactInfo { contact }).await {
            status = format!("{s:?}");
            if s == SyncStatus::Ok {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let v = json!({ "asked": asked, "status": status, "ms": t.elapsed().as_millis() as u64 });
    log!("forcedSync": v);
    door.stat("forced_sync", v.clone());
    v
}

/// What the contact sent since the cursor, from the core's own chat (the
/// newest page; a door down longer than that page is a gap the spike
/// reports, not reads back).
async fn catch_up(door: &Arc<Door>, contact: i64) {
    let delivered = door.state.lock().expect("state").delivered;
    match door.ask(&Cmd::Chat { contact, count: 200, before: None }).await {
        Ok((Resp::Page(items), _)) => {
            let mut n = 0;
            for item in items.into_iter().filter(|i| i.received && i.id > delivered) {
                if matches!(item.content, Content::Message { .. }) {
                    n += 1;
                    let _ = door.deliver.send(item);
                }
            }
            log!("catchUp": n, "after": delivered);
            door.stat("catch_up", json!({ "items": n, "after": delivered }));
        }
        other => log!("catchUp": "failed", "answer": format!("{other:?}")),
    }
}

async fn on_event(door: &Arc<Door>, text: &str) {
    let frame = match wire::parse(text) {
        Ok(f) => f,
        Err(e) => {
            log!("event": "unread", "why": format!("{e:?}"));
            return;
        }
    };
    let contact = door.state.lock().expect("state").contact;
    match frame.resp {
        Resp::Items(items) => {
            for item in items {
                if item.received && Some(item.contact) == contact && matches!(item.content, Content::Message { .. }) {
                    let _ = door.deliver.send(item);
                } else if item.received && Some(item.contact) == contact {
                    log!("item": format!("{:?}", item.content), "id": item.id);
                    door.stat("item", json!({ "id": item.id, "content": format!("{:?}", item.content) }));
                }
            }
        }
        Resp::Connected { contact: c, conn, name } => {
            let bound = {
                let mut s = door.state.lock().expect("state");
                if s.invitation == Some(conn) || s.contact.is_none() {
                    s.contact = Some(c);
                    s.invitation = None;
                    true
                } else {
                    false
                }
            };
            log!("connected": c, "conn": conn, "bound": bound);
            if bound {
                door.dirty.notify_one();
                door.stat("linked", json!({ "contact": c, "conn": conn, "name": name }));
            }
        }
        Resp::Sync { contact: c, status } => {
            log!("ratchet": format!("{status:?}"), "contact": c);
            door.stat("ratchet", json!({ "contact": c, "status": format!("{status:?}") }));
            if status == SyncStatus::Required && Some(c) == contact {
                let d = door.clone();
                tokio::spawn(async move { sync(&d, c).await });
            }
        }
        Resp::AnotherClient { contact: c } => {
            log!("anotherClient": c);
            door.stat("another_client", json!({ "contact": c }));
        }
        Resp::SubscriptionEnd { contact: c } => {
            log!("subscriptionEnd": c);
            door.stat("subscription_end", json!({ "contact": c }));
        }
        Resp::Error { what } => log!("coreError": what),
        _ => {}
    }
}

/// Each of its owner's messages to the connector's inbox, in order, each
/// until it is taken; the cursor moves after.
async fn deliveries(door: Arc<Door>, mut rx: mpsc::UnboundedReceiver<wire::Item>) {
    while let Some(item) = rx.recv().await {
        if item.id <= door.state.lock().expect("state").delivered {
            continue;
        }
        let Content::Message { kind, text } = &item.content else { continue };
        let payload = json!({ "source": "simplex", "payload": { "kind": "message", "item": item.id, "shared": item.shared, "type": kind, "text": text, "at": item.at, "atMs": wire::utc_ms(&item.at) } });
        let mut wait = Duration::from_millis(500);
        loop {
            let (inbox, secret, header) = {
                let c = door.config.lock().expect("config");
                (c.inbox.clone(), c.secret.clone(), c.header.clone().unwrap_or_else(|| "x-simplex-door-secret".into()))
            };
            let (Some(inbox), Some(secret)) = (inbox, secret) else {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            };
            let t = Instant::now();
            let r = http::call("POST", &inbox, &[("content-type", "application/json".into()), (header.as_str(), secret)], Bytes::from(payload.to_string()), Duration::from_secs(30)).await;
            match r {
                Ok(a) if (200..300).contains(&a.status) => {
                    let ms = t.elapsed().as_millis() as u64;
                    door.state.lock().expect("state").delivered = item.id;
                    door.delivered_n.fetch_add(1, Ordering::SeqCst);
                    door.dirty.notify_one();
                    let lag = wire::utc_ms(&item.at).map(|at| now_ms().saturating_sub(at));
                    log!("delivered": item.id, "ms": ms, "lagMs": lag);
                    door.stat("delivered", json!({ "item": item.id, "shared": item.shared, "postMs": ms, "lagMs": lag }));
                    break;
                }
                other => {
                    log!("deliver": "failed", "item": item.id, "answer": other.map(|a| format!("{} {}", a.status, String::from_utf8_lossy(&a.body))).unwrap_or_else(|e| e));
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

/// A save about a second after each change the core commits (either
/// database's `data_version` moved), or the door's own state changed.
async fn saves(door: Arc<Door>) {
    let watch = loop {
        match save::Watch::open(&door.dir) {
            Ok(w) => break w,
            Err(e) => {
                log!("watch": "waiting", "why": e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    let mut seen = watch.versions();
    // the first save of each start: what it restored, or the new profile
    let mut due: Option<Instant> = Some(Instant::now());
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(WATCH_MS)) => {}
            _ = door.dirty.notified() => {
                due.get_or_insert_with(|| Instant::now() + Duration::from_millis(DEBOUNCE_MS));
            }
        }
        let now = watch.versions();
        if now != seen {
            seen = now;
            due.get_or_insert_with(|| Instant::now() + Duration::from_millis(DEBOUNCE_MS));
        }
        if due.is_some_and(|d| Instant::now() >= d) {
            due = None;
            // what the save itself sees moving is not a change of the core's
            match door.save("change").await {
                Ok(v) => {
                    log!("save": v);
                    door.stat("save", v);
                }
                Err(e) => {
                    log!("save": "failed", "error": e);
                    door.stat("save_failed", json!({ "error": e }));
                    due = Some(Instant::now() + Duration::from_secs(2));
                }
            }
            seen = watch.versions();
        }
    }
}

async fn read_json(req: Request<Incoming>) -> Value {
    let bytes = http_body_util::Limited::new(req.into_body(), 1024 * 1024).collect().await.map(|b| b.to_bytes()).unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn answer(status: u16, v: Value) -> Response<Body> {
    net::json_answer(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), &v)
}

/// The Durable Object's calls.
async fn handle(door: Arc<Door>, req: Request<Incoming>) -> Response<Body> {
    let path = req.uri().path().to_string();
    let method = req.method().as_str().to_string();
    match (method.as_str(), path.as_str()) {
        ("GET", "/health") => answer(
            200,
            json!({
                "generation": door.generation, "ready": door.ready.load(Ordering::SeqCst), "upMs": door.started.elapsed().as_millis() as u64,
                "state": *door.state.lock().expect("state"), "saves": door.saves.load(Ordering::SeqCst), "lastSave": *door.last_save.lock().expect("last"),
                "delivered": door.delivered_n.load(Ordering::SeqCst), "sent": door.sent_n.load(Ordering::SeqCst), "syncs": door.syncs.load(Ordering::SeqCst),
                "inbox": door.config.lock().expect("config").inbox.is_some(),
            }),
        ),
        ("POST", "/config") => {
            let c: Config = serde_json::from_value(read_json(req).await).unwrap_or_default();
            let _ = door.relay.send(c.relay.clone());
            *door.config.lock().expect("config") = c;
            answer(200, json!({ "ok": true }))
        }
        ("POST", "/link") => {
            if door.state.lock().expect("state").contact.is_some() {
                return answer(409, json!({ "error": "linked already" }));
            }
            match door.ask(&Cmd::Invite).await {
                Ok((Resp::Invitation { conn, link, short }, _)) => {
                    door.state.lock().expect("state").invitation = Some(conn);
                    door.dirty.notify_one();
                    answer(200, json!({ "conn": conn, "link": link, "short": short }))
                }
                other => answer(502, json!({ "error": format!("{other:?}") })),
            }
        }
        ("POST", "/send") => {
            if !door.ready.load(Ordering::SeqCst) {
                return answer(503, json!({ "error": "the door is starting (or syncing its ratchet)" }));
            }
            let v = read_json(req).await;
            let text = v["text"].as_str().unwrap_or("").to_string();
            let Some(contact) = door.state.lock().expect("state").contact else {
                return answer(409, json!({ "error": "not linked" }));
            };
            let mut items = vec![];
            for part in wire::parts(&text) {
                match door.ask(&Cmd::Send { contact, text: part }).await {
                    Ok((Resp::Items(sent), _)) => items.extend(sent.iter().map(|i| i.id)),
                    Ok((resp, _)) => return answer(503, json!({ "error": format!("{resp:?}"), "items": items })),
                    Err(e) => return answer(503, json!({ "error": e, "items": items })),
                }
            }
            door.sent_n.fetch_add(1, Ordering::SeqCst);
            door.dirty.notify_one();
            answer(200, json!({ "items": items }))
        }
        ("POST", "/sync") => {
            let Some(contact) = door.state.lock().expect("state").contact else {
                return answer(409, json!({ "error": "not linked" }));
            };
            let force = read_json(req).await["force"] == true;
            match door.ask(&Cmd::Sync { contact, force }).await {
                Ok((resp, _)) => answer(200, json!({ "answer": format!("{resp:?}") })),
                Err(e) => answer(502, json!({ "error": e })),
            }
        }
        ("POST", "/cmd") => {
            // the spike's own reads of the core (its items, its info)
            let cmd = read_json(req).await["cmd"].as_str().unwrap_or("").to_string();
            let Some(core) = door.core() else { return answer(503, json!({ "error": "not up" })) };
            match core.ask(&cmd, ASK_WAIT).await {
                Ok(t) => answer(200, serde_json::from_str(&t).unwrap_or(Value::String(t))),
                Err(e) => answer(502, json!({ "error": e })),
            }
        }
        ("POST", "/save") => match door.save("asked").await {
            Ok(v) => answer(200, v),
            Err(e) => answer(502, json!({ "error": e })),
        },
        _ => answer(404, json!({ "error": "no such route" })),
    }
}
