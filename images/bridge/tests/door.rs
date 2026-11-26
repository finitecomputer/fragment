//! A SimpleX door against the real core: the door's own image
//! (images/simplex-door/Dockerfile) on the SimpleX lab's network (a local
//! SMP server, a person's simplex-chat), with a fake door on the host in
//! place of its Durable Object: it keeps every save, numbers every effect
//! (its mark), says when a save is behind, gives each runner life a
//! generation of its own and fences saves by it, as the door's protocol
//! says (images/simplex-door/src/door.rs).
//!
//! Lower rung, labeled so: the lab's server, not SimpleX's presets, and a
//! container per start on this machine's Docker, not Cloudflare's.
//!
//! Run by hand, as the lab's own tests are:
//! `cargo test -p fragment-bridge --test door -- --ignored --nocapture`.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use serde_json::{json, Value};
use support::simplex::*;

const DOOR_IMAGE: &str = "fragment-simplex-door:test";

/// How far a person's burst runs ahead of the door's delivery receipts for
/// it. A phone's app takes them back as they come, so its queue never
/// holds many; the lab's client, sending at the API's speed, fell 129
/// behind, and SimpleX's server holds at most 128 messages a queue: past
/// that it refuses the door's sends (QUOTA) until the person has read them
/// all, and its agent's handshake after one can strand them on its
/// five-minute retry (`a_burst_against_the_real_core`). Well under the
/// quota, as a phone's pace is (300 messages 300 ms apart: none waiting).
const BURST_AHEAD: usize = 32;

/// The person's messages the door's core has said it received (its
/// delivery receipts, as the person's app shows them), from the events
/// read so far, none taken.
fn receipted(person: &Api) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    for e in person.events.iter().filter(|e| tag(e) == "chatItemsStatusesUpdated") {
        for ci in e["chatItems"].as_array().into_iter().flatten() {
            let item = &ci["chatItem"];
            if item["meta"]["itemStatus"]["type"] == "sndRcvd" {
                seen.extend(item["content"]["msgContent"]["text"].as_str().map(str::to_string));
            }
        }
    }
    seen
}

/// The person says `text`, as a phone's app would in a burst: never more
/// than `BURST_AHEAD` messages ahead of the door's receipts, `sent` the
/// burst so far.
async fn say_in_burst(person: &mut Api, agent: i64, text: &str, sent: &[String]) {
    say(person, agent, text).await;
    let Some(behind) = sent.len().checked_sub(BURST_AHEAD) else { return };
    let want = &sent[behind];
    let t0 = Instant::now();
    // bounded: 60 s
    while !receipted(person).contains(want) {
        assert!(t0.elapsed() < Duration::from_secs(60), "the door's receipt of {want}, in step with the burst");
        person.drain(Duration::from_millis(100)).await;
    }
}

/// One save the fake door took.
#[derive(Clone)]
struct Kept {
    /// The start it was saved in, and its life within that start.
    start: u64,
    life: u64,
    seq: u64,
    mark: u64,
    bytes: Bytes,
}

#[derive(Default)]
struct DoorState {
    saves: Vec<Kept>,
    /// The start running: another's restore is refused (409).
    started: u64,
    /// The newest life given, and each life's start: a save of any life
    /// but the newest, or of a start since superseded, is refused (409).
    life: u64,
    lives: BTreeMap<u64, u64>,
    /// The newest effect numbered.
    marks: u64,
    /// A save to restore in place of the newest (a rollback).
    pinned: Option<usize>,
    /// Each delivery as it came, `(id, text)`, duplicates included.
    delivered: Vec<(String, String)>,
    /// Each request's head (method, path, and whether it said a length).
    heads: Vec<(String, String, bool, u64)>,
    refused: u64,
    /// Deliveries answered 503 meanwhile (a door that cannot take them).
    closed: bool,
    /// Where each life's newest save is written as it is taken (the lab's
    /// folder: a failure's evidence).
    keep: Option<PathBuf>,
    /// Each delivery whole, as it came.
    deliveries: Vec<Value>,
    /// The files it holds, by their SHA-256 (each checked as it came), and
    /// how many each way: given by its runner, read by it.
    files: BTreeMap<String, Bytes>,
    given: u64,
    read: u64,
    /// Its XFTP relay (`GET /relay`): each host it carries on 443 and the
    /// address it stands for; any other is 403. The bytes carried, up (from
    /// the core) and down.
    relay_to: BTreeMap<String, std::net::SocketAddr>,
    relay_up: u64,
    relay_down: u64,
}

#[derive(Clone)]
struct FakeDoor {
    state: Arc<Mutex<DoorState>>,
    port: u16,
}

impl FakeDoor {
    async fn start() -> FakeDoor {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.expect("the fake door listens");
        let port = listener.local_addr().unwrap().port();
        let door = FakeDoor { state: Arc::default(), port };
        let d = door.clone();
        let (_tx, rx) = tokio::sync::watch::channel(false);
        std::mem::forget(_tx);
        tokio::spawn(fragment_bridge::net::serve(listener, move |req, _: SocketAddr| d.clone().answer(req), rx));
        door
    }

    fn with<T>(&self, f: impl FnOnce(&mut DoorState) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }

    /// A new mark, for an ask the test makes of the runner.
    fn mark(&self) -> u64 {
        self.with(|s| {
            s.marks += 1;
            s.marks
        })
    }

    /// The deliveries' texts, each once, in the order first delivered; and
    /// that every repeat repeated its id's text.
    fn texts(&self) -> Vec<String> {
        self.with(|s| {
            let mut by_id: BTreeMap<String, String> = BTreeMap::new();
            let mut order = vec![];
            for (id, text) in &s.delivered {
                match by_id.get(id) {
                    Some(t) => assert_eq!(t, text, "a delivery again is its id's text again"),
                    None => {
                        by_id.insert(id.clone(), text.clone());
                        order.push(text.clone());
                    }
                }
            }
            order
        })
    }

    /// Its XFTP relay, as the cell's (entry.mjs `DoorEgress`, relay.mjs):
    /// a WebSocket piped to the host it names, if it carries it.
    fn relay(&self, mut req: Request<Incoming>) -> Response<fragment_bridge::net::Body> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let q = req.uri().query().unwrap_or("").to_string();
        let param = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
        let to = self.with(|s| s.relay_to.get(&param("host")).copied().filter(|_| param("port") == "443"));
        let refused = |status: u16| Response::builder().status(status).body(Full::new(Bytes::new())).unwrap();
        let Some(to) = to else { return refused(403) };
        let Some((response, socket)) = fragment_bridge::net::accept_ws(&mut req) else { return refused(400) };
        let door = self.clone();
        tokio::spawn(async move {
            let Some(ws) = socket.await else { return };
            let Ok(mut tcp) = tokio::net::TcpStream::connect(to).await else { return };
            let (mut rd, mut wr) = tcp.split();
            let (mut sink, mut stream) = ws.split();
            let mut buf = vec![0u8; 16 * 1024];
            // bounded by either side's end
            loop {
                tokio::select! {
                    n = tokio::io::AsyncReadExt::read(&mut rd, &mut buf) => match n {
                        Ok(0) | Err(_) => { let _ = sink.send(Message::Close(None)).await; break; }
                        Ok(n) => { door.with(|s| s.relay_down += n as u64); if sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() { break; } }
                    },
                    m = stream.next() => match m {
                        Some(Ok(Message::Binary(b))) => { door.with(|s| s.relay_up += b.len() as u64); if tokio::io::AsyncWriteExt::write_all(&mut wr, &b).await.is_err() { break; } }
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        _ => {}
                    },
                }
            }
        });
        response
    }

    async fn answer(self, req: Request<Incoming>) -> Response<fragment_bridge::net::Body> {
        if req.uri().path() == "/relay" {
            return self.relay(req);
        }
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let header = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let generation: u64 = header("x-door-generation").and_then(|g| g.parse().ok()).expect("each call carries its generation");
        let sized = header("content-length").is_some();
        let mark: Option<u64> = header("x-door-mark").and_then(|m| m.parse().ok());
        let content_type = header("content-type");
        let named = header("x-door-sha256");
        let body = req.into_body().collect().await.unwrap().to_bytes();
        // every body its runner sends names its SHA-256, as the cell's door asks
        if !body.is_empty() {
            assert_eq!(named.as_deref(), Some(sha256_hex(&body).as_str()), "{method} {path}: a runner's body names its SHA-256");
        }
        self.with(|s| s.heads.push((method.clone(), path.clone(), sized, generation)));
        // every answer named by its SHA-256, as the cell's door egress names
        // each (a save or a file names its own)
        let answer = |status: u16, headers: &[(&str, String)], body: Bytes| {
            let mut r = Response::builder().status(status);
            for (k, v) in headers {
                r = r.header(*k, v.as_str());
            }
            if !headers.iter().any(|(k, _)| *k == "x-door-sha256") {
                r = r.header("x-door-sha256", sha256_hex(&body));
            }
            r.body(Full::new(body)).unwrap()
        };
        match (method.as_str(), path.as_str()) {
            ("GET", "/save") => self.with(|s| {
                if generation != s.started {
                    return answer(409, &[], Bytes::new());
                }
                s.life += 1;
                let life = s.life;
                s.lives.insert(life, s.started);
                let at = s.pinned.or(s.saves.len().checked_sub(1));
                match at.map(|i| s.saves[i].clone()) {
                    None => answer(204, &[("x-door-life", life.to_string())], Bytes::new()),
                    Some(k) => {
                        let behind = if k.mark < s.marks { "1" } else { "0" };
                        let headers = [("x-door-mark", s.marks.to_string()), ("x-door-behind", behind.into()), ("x-door-life", life.to_string()), ("x-door-save", format!("{}-{}", k.life, k.seq)), ("x-door-sha256", sha256_hex(&k.bytes))];
                        answer(200, &headers, k.bytes)
                    }
                }
            }),
            ("PUT", "/save") => self.with(|s| {
                let seq: u64 = query.strip_prefix("seq=").and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("a save is numbered: {query}"));
                if generation != s.life || s.lives.get(&generation) != Some(&s.started) {
                    s.refused += 1;
                    return answer(409, &[], Bytes::new());
                }
                if let Some(dir) = &s.keep {
                    let _ = std::fs::write(dir.join(format!("save-life{generation}.sxd")), &body);
                }
                s.saves.push(Kept { start: s.started, life: generation, seq, mark: mark.expect("a save carries its mark"), bytes: body });
                answer(200, &[], Bytes::new())
            }),
            ("POST", "/unreadable") => answer(200, &[], Bytes::new()),
            ("PUT", p) if p.starts_with("/file/") => self.with(|s| {
                let sha = &p["/file/".len()..];
                assert_eq!(sha256_hex(&body), sha, "a file given is the one it names");
                assert_eq!(content_type.as_deref(), Some("audio/mp4"), "a voice message's file, typed as SimpleX's apps record one");
                s.files.insert(sha.to_string(), body.clone());
                s.given += 1;
                answer(200, &[], Bytes::new())
            }),
            ("GET", p) if p.starts_with("/file/") => self.with(|s| match s.files.get(&p["/file/".len()..]) {
                Some(b) => {
                    s.read += 1;
                    answer(200, &[("x-door-sha256", p["/file/".len()..].to_string())], b.clone())
                }
                None => answer(404, &[], Bytes::new()),
            }),
            ("POST", "/deliver") => self.with(|s| {
                if s.closed {
                    return answer(503, &[], Bytes::new());
                }
                let v: Value = serde_json::from_slice(&body).expect("a delivery is JSON");
                s.deliveries.push(v.clone());
                s.delivered.push((v["id"].as_str().expect("its id").into(), v["text"].as_str().expect("its text").into()));
                s.marks += 1;
                answer(200, &[("x-door-mark", s.marks.to_string())], Bytes::new())
            }),
            _ => answer(404, &[], Bytes::new()),
        }
    }
}

/// One start of the door's container: its generation, on the lab's
/// network, its door the fake on the host, its API published on loopback.
struct Start {
    ctr: String,
    api: String,
    /// Where its log is kept as it goes, however the test ends.
    keep: PathBuf,
}

impl Drop for Start {
    fn drop(&mut self) {
        let _ = std::fs::write(self.keep.join(format!("{}.log", self.ctr)), logs(&self.ctr));
        let _ = docker(&["rm", "-f", &self.ctr]);
    }
}

fn start(lab: &Lab, door: &FakeDoor, generation: u64) -> Start {
    door.with(|s| s.started = generation);
    let ctr = format!("{}-door-{generation}", lab.prefix);
    let _ = docker(&["rm", "-f", &ctr]);
    let (net, api, gen) = (format!("{}-net", lab.prefix), format!("DOOR_API=http://door.test:{}", door.port), format!("DOOR_GENERATION={generation}"));
    let smp = format!("DOOR_SMP={}", lab.server);
    let xftp = format!("DOOR_XFTP={}", lab.xftp.as_deref().unwrap_or(""));
    let mut args = vec!["run", "-d", "--platform", "linux/amd64", "--name", &ctr, "--network", &net, "--add-host", "door.test:host-gateway", "-e", &api, "-e", &gen, "-e", &smp, "-e", "DOOR_NAME=Juniper", "-p", "127.0.0.1::8080"];
    if lab.xftp.is_some() {
        args.extend(["-e", &xftp]);
    }
    args.push(DOOR_IMAGE);
    docker(&args).expect("the door starts");
    let port = docker(&["port", &ctr, "8080/tcp"]).expect("its port");
    Start { ctr, api: format!("http://{}", port.lines().next().expect("a port")), keep: lab.dir.clone() }
}

impl Start {
    async fn call(&self, method: &str, path: &str, mark: Option<u64>, body: Option<Value>) -> (u16, Value) {
        let mut c = tokio::process::Command::new("curl");
        c.args(["-sS", "-m", "30", "-o", "-", "-w", "\n%{http_code}", "-X", method]);
        if let Some(m) = mark {
            c.args(["-H", &format!("x-door-mark: {m}")]);
        }
        // a body named by its SHA-256, as its door names every one
        if let Some(b) = body {
            let text = b.to_string();
            c.args(["-H", "content-type: application/json", "-H", &format!("x-door-sha256: {}", sha256_hex(text.as_bytes())), "--data-binary", &text]);
        }
        let out = c.arg(format!("{}{path}", self.api)).output().await.expect("curl runs");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
        (code.trim().parse().unwrap_or(0), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    async fn state(&self) -> Value {
        self.call("GET", "/state", None, None).await.1
    }

    /// Its state, once `done` holds (within `within`).
    async fn until(&self, within: Duration, done: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        // bounded by `within`
        loop {
            let s = self.state().await;
            if done(&s) {
                return s;
            }
            assert!(t0.elapsed() < within, "the door is not there within {within:?}: {s}\n{}", logs(&self.ctr));
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// A reply asked, as its door asks it: taken (202, not done), then
    /// asked again under a new mark each time until it is done (200).
    async fn reply(&self, door: &FakeDoor, key: &str, text: &str) {
        let t0 = Instant::now();
        // bounded: 60 s
        loop {
            let (code, v) = self.call("POST", "/reply", Some(door.mark()), Some(json!({ "key": key, "text": text }))).await;
            match code {
                200 => return assert_eq!(v, json!({ "done": true }), "the reply {key}"),
                202 => assert_eq!(v, json!({ "done": false }), "the reply {key}"),
                503 => {}
                _ => panic!("the reply {key}: {code} {v}"),
            }
            assert!(t0.elapsed() < Duration::from_secs(60), "the reply {key} is done");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn kill(&self, signal: &str) {
        docker(&["kill", "-s", signal, &self.ctr]).expect("signalled");
    }

    /// Its exit code, once it has ended.
    fn exited(&self) -> i64 {
        let t0 = Instant::now();
        // bounded: 20 s
        loop {
            let s = docker(&["inspect", "-f", "{{.State.Running}} {{.State.ExitCode}}", &self.ctr]).expect("inspected");
            if let Some(code) = s.strip_prefix("false ") {
                return code.parse().expect("an exit code");
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "it did not end: {}", logs(&self.ctr));
            std::thread::sleep(Duration::from_millis(100));
        }
    }

}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// One file of a save, read as the runner packs it: gzip, `SXD1`, then
/// each file's name and bytes (images/simplex-door/src/save.rs).
fn file_of(packed: &[u8], want: &str) -> Vec<u8> {
    use std::io::Read;
    let mut raw = vec![];
    flate2::read::GzDecoder::new(packed).read_to_end(&mut raw).expect("a save is gzip");
    assert_eq!(&raw[..4], b"SXD1");
    let mut at = 4;
    // bounded: three files
    while at < raw.len() {
        let n = u16::from_be_bytes([raw[at], raw[at + 1]]) as usize;
        let name = std::str::from_utf8(&raw[at + 2..at + 2 + n]).expect("a name").to_string();
        at += 2 + n;
        let len = u64::from_be_bytes(raw[at..at + 8].try_into().unwrap()) as usize;
        at += 8;
        if name == want {
            return raw[at..at + len].to_vec();
        }
        at += len;
    }
    panic!("a save without {want}")
}

/// The runner's own state in a save (`door.json`).
fn kept_of(packed: &[u8]) -> Value {
    serde_json::from_slice(&file_of(packed, "door.json")).expect("the runner's state")
}

/// What the core's chat database in a save holds as received, in order.
fn received_in(packed: &[u8], scratch: &std::path::Path) -> Vec<String> {
    let path = scratch.join("save-chat.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::write(&path, file_of(packed, "sx_chat.db")).expect("written");
    let c = rusqlite::Connection::open(&path).expect("the save's chat database");
    let mut q = c.prepare("SELECT item_text FROM chat_items WHERE item_sent = 0 ORDER BY chat_item_id").expect("its items");
    q.query_map([], |r| r.get::<_, String>(0)).expect("read").map(|t| t.expect("a text")).collect()
}

/// Its senders' ids of what the core's chat database in a save holds as
/// received, as its handover reads them (`save::received_past`).
fn sent_ids_in(packed: &[u8], scratch: &std::path::Path) -> Vec<String> {
    let dir = scratch.join("save-sent");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("made");
    std::fs::write(dir.join("sx_chat.db"), file_of(packed, "sx_chat.db")).expect("written");
    let contact: i64 = rusqlite::Connection::open(dir.join("sx_chat.db")).expect("opened").query_row("SELECT max(contact_id) FROM chat_items WHERE item_sent = 0", [], |r| r.get(0)).expect("a contact");
    fragment_bridge::simplex::save::received_past(&dir, contact, 0, 100).expect("read").into_iter().map(|(_, id)| id).collect()
}

fn build_image() {
    let have = docker(&["image", "inspect", "--format", "{{.Id}}", DOOR_IMAGE]).is_ok();
    if have && std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_ok() {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // loaded into the local images whatever builder builds it (a
    // docker-container builder keeps its result in its cache otherwise)
    let out = Command::new("docker").args(["build", "--load", "--platform", "linux/amd64", "-q", "-f", "images/simplex-door/Dockerfile", "-t", DOOR_IMAGE, "."]).current_dir(&root).output().expect("docker runs");
    assert!(out.status.success(), "the door's image builds: {}", String::from_utf8_lossy(&out.stderr));
}

/// What the person received from the door, each once, in order.
async fn person_got(person: &mut Api, seen: &mut Vec<String>, want: &[&str]) {
    let more = received(person, Duration::from_secs(240), |got| want.iter().all(|w| seen.iter().chain(got.iter()).any(|g| g == w))).await;
    seen.extend(more);
    for w in want {
        assert_eq!(seen.iter().filter(|g| g == w).count(), 1, "the person has {w:?} once: {seen:?}");
    }
}

/// Goal: a door outside any computer holds one link through everything
/// its container meets, nothing lost or doubled either way: a new door
/// linked by its invitation, as its QR code in its state holds it;
/// messages each way; a reply asked again sent
/// once; an ask under a stale mark refused; a crash (SIGKILL) with 300
/// messages saved and not yet delivered, its next start reading them on
/// from its cursor, in order; a rollback to a save from before both, healed
/// by the one forced sync the door's "behind" asks; a planned restart
/// (SIGTERM) as a burst arrives, its core stopped before its last save, so
/// nothing is lost and the next start is in step with no sync; and a start
/// superseded by a newer generation, whose save is refused, ending it;
/// and an agent's question, worded as every door words one, reaching the
/// person whole, their answer coming back once. Method: the door's real image and SimpleX's real core, a person's real
/// client, and a fake door on the host as its Durable Object would be.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_door_against_the_real_core() {
    build_image();
    let mut lab = Lab::up("door");
    let door = FakeDoor::start().await;
    door.with(|s| s.keep = Some(lab.dir.clone()));

    // a new door: no save, a new profile, its first save
    let g1 = start(&lab, &door, 1);
    g1.until(Duration::from_secs(60), |s| s["ready"] == true).await;
    let first = door.with(|s| s.heads.first().cloned()).expect("its first call");
    assert_eq!(first, ("GET".into(), "/save".into(), false, 1), "a restore's GET carries no length");
    let t0 = Instant::now();
    while door.with(|s| s.saves.is_empty()) {
        assert!(t0.elapsed() < Duration::from_secs(10), "the new profile is saved");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // linked by its invitation
    let (code, _) = g1.call("POST", "/link", Some(door.mark()), None).await;
    assert_eq!(code, 202);
    let s = g1.until(Duration::from_secs(60), |s| s["phase"] == "inviting").await;
    // its QR code, as its owner's page draws it (templates/simplex
    // setup.js): of its short link where there is one, whole; the person
    // connects with what it holds
    let drawn = s["short"].as_str().or(s["link"].as_str()).expect("its invitation").to_string();
    let qr = qrcode::QrCode::with_error_correction_level(drawn.as_bytes(), qrcode::EcLevel::M).expect("drawn");
    let dark: String = qr.to_colors().iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect();
    assert_eq!((s["qr"]["size"].as_u64(), s["qr"]["dark"].as_str()), (Some(qr.width() as u64), Some(dark.as_str())), "its state's QR code is its invitation's");
    let mut person = lab.start("person", false).await;
    let r = person.cmd(&format!("/connect {drawn}")).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    g1.until(Duration::from_secs(90), |s| s["phase"] == "linked" && s["name"] == "person").await;
    let agent = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the door, at the person's");
    // the first save that holds the link, as its own state says: the
    // rollback's, later
    let t0 = Instant::now();
    let linked_save = loop {
        let found = door.with(|s| s.saves.iter().position(|k| kept_of(&k.bytes)["link"]["phase"] == "linked"));
        if let Some(i) = found {
            break i;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "the link is saved");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // messages each way, and a reply asked again sent once
    let mut sent: Vec<String> = vec![];
    for n in 1..=10 {
        say(&mut person, agent, &format!("p{n}")).await;
        sent.push(format!("p{n}"));
    }
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "the person's messages reach the door, each once, in order: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut got: Vec<String> = vec![];
    for n in 1..=5 {
        g1.reply(&door, &format!("chat:{n}"), &format!("r{n}")).await;
    }
    // asked again once done, under a new mark: done, and sent once
    let (code, v) = g1.call("POST", "/reply", Some(door.mark()), Some(json!({ "key": "chat:5", "text": "r5" }))).await;
    assert_eq!((code, v), (200, json!({ "done": true })), "a key asked again");
    // an ask under a mark older than one the runner has seen (its request
    // overtaken): refused, for its door to ask again under a new mark
    let (code, v) = g1.call("POST", "/reply", Some(1), Some(json!({ "key": "chat:stale", "text": "never" }))).await;
    assert_eq!(code, 409, "a stale mark: {v}");
    person_got(&mut person, &mut got, &["r1", "r2", "r3", "r4", "r5"]).await;

    // a crash with messages saved and not yet delivered (its door could
    // not take them): its next start delivers them from its cursor; and one
    // sent while it was down arrives then
    // 300 of them, past the link's own catch-up (256 back from the newest:
    // the review of #1430, the runner dropped what passed its queue and read
    // back no further than that): the core's database is the queue. Sent
    // as a phone's app sends a burst, in step with the door's receipts
    // (`BURST_AHEAD`: at the API's speed it outran SimpleX's quota)
    door.with(|s| s.closed = true);
    for n in 11..=310 {
        say_in_burst(&mut person, agent, &format!("p{n}"), &sent[10..]).await;
        sent.push(format!("p{n}"));
    }
    // until the newest save holds all 600 (the core's chat database in it)
    let t0 = Instant::now();
    loop {
        let newest = door.with(|s| s.saves.last().map(|k| k.bytes.clone())).expect("a save");
        let held = received_in(&newest, &lab.dir);
        if held.last().map(String::as_str) == Some("p310") {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(180), "the 300 are saved: {} so far", held.len());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    g1.kill("KILL");
    assert_eq!(g1.exited(), 137);
    drop(g1);
    assert_eq!(door.texts(), sent[..10], "none of the 300 was delivered before the crash");
    door.with(|s| s.closed = false);
    say(&mut person, agent, "p311").await;
    sent.push("p311".into());
    let g2 = start(&lab, &door, 2);
    let s = g2.until(Duration::from_secs(180), |s| s["ready"] == true && s["phase"] == "linked" && s["syncing"] == false).await;
    assert_eq!(s["forcedSyncs"], 0, "nothing moved past its save: no sync: {s}");
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(180), "nothing lost in the crash: {} of {}", door.texts().len(), sent.len());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g2.reply(&door, "chat:6", "r6").await;
    person_got(&mut person, &mut got, &["r6"]).await;

    // a rollback to the save that held the link alone, behind both bursts:
    // one sync, then each way in step, nothing delivered again. The sync is
    // the one the door's "behind" forces, or the core's own when a message
    // of the person's reaches the rolled-back ratchet first (their receipt
    // for r6, sent as the crash came: the link then waits on that one,
    // link.rs `ContactInfo`); either way the link says it is in step
    // (`Notice::Resynced`), which a door that never synced does not
    g2.kill("KILL");
    drop(g2);
    door.with(|s| s.pinned = Some(linked_save));
    let g3 = start(&lab, &door, 3);
    let one_sync = |s: &Value| s["forcedSyncs"].as_u64().is_some_and(|n| n <= 1) && s["notices"].as_array().is_some_and(|n| n.iter().any(|x| x == "Resynced"));
    g3.until(Duration::from_secs(60), |s| s["ready"] == true && one_sync(s)).await;
    door.with(|s| s.pinned = None);
    g3.until(Duration::from_secs(180), |s| s["syncing"] == false).await;
    recovered(&mut person, agent, Duration::from_secs(180), || true).await;
    say(&mut person, agent, "p312").await;
    sent.push("p312".into());
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "a message after the rollback, once: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g3.reply(&door, "chat:7", "r7").await;
    person_got(&mut person, &mut got, &["r7"]).await;
    let s = g3.state().await;
    assert!(one_sync(&s), "one sync, the door's or the core's own, not more: {s}");

    // a planned restart: the core stopped first, so its last save holds
    // what the core took (a burst sent as it stops, the review of #1430)
    // and its newest mark, and the next start is in step with no sync
    g3.reply(&door, "chat:8", "r8").await;
    for n in 313..=322 {
        say(&mut person, agent, &format!("p{n}")).await;
        sent.push(format!("p{n}"));
    }
    g3.kill("TERM");
    assert_eq!(g3.exited(), 0, "a planned stop ends well: {}", logs(&g3.ctr));
    drop(g3);
    let newest = door.with(|s| (s.saves.last().map(|k| (k.start, k.mark)), s.marks));
    assert_eq!(newest.0, Some((3, newest.1)), "the last save is the stop's, at the newest mark");
    let g4 = start(&lab, &door, 4);
    let s = g4.until(Duration::from_secs(60), |s| s["ready"] == true && s["syncing"] == false).await;
    assert_eq!(s["forcedSyncs"], 0, "in step: no sync: {s}");
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "nothing lost in the stop: {:?}", &door.texts()[door.texts().len().saturating_sub(12)..]);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    person_got(&mut person, &mut got, &["r8"]).await;
    g4.reply(&door, "chat:9", "r9").await;
    person_got(&mut person, &mut got, &["r9"]).await;
    // its save that holds r9 done, before it is superseded: a reply sent
    // after the newest save a start restores is sent again (never lost,
    // maybe twice: run 4 of 5 on 11-21 met it here), which is not what
    // this step tries
    let t0 = Instant::now();
    // bounded: 30 s
    while !door.with(|s| s.saves.last().is_some_and(|k| kept_of(&k.bytes)["replied"].as_array().is_some_and(|r| r.iter().any(|x| x == "chat:9")))) {
        assert!(t0.elapsed() < Duration::from_secs(30), "a save that holds r9 done");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // superseded: a newer generation started, this one's save refused, and
    // it ends
    door.with(|s| s.started = 5);
    say(&mut person, agent, "p323").await;
    sent.push("p323".into());
    assert_eq!(g4.exited(), 1, "a start superseded ends: {}", logs(&g4.ctr));
    assert!(door.with(|s| s.refused) >= 1);
    drop(g4);
    let g5 = start(&lab, &door, 5);
    g5.until(Duration::from_secs(60), |s| s["ready"] == true && s["syncing"] == false).await;
    recovered(&mut person, agent, Duration::from_secs(180), || true).await;
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "{:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g5.reply(&door, "chat:10", "r10").await;
    person_got(&mut person, &mut got, &["r10"]).await;
    assert_eq!(got, (1..=10).map(|n| format!("r{n}")).collect::<Vec<_>>(), "every reply once, in order");

    // its agent's question, as every door words one for its owner's app
    // (src/questions.json, FIN-747): their app has it whole, and their
    // answer comes back to the door, once
    let cases: Value = serde_json::from_str(include_str!("../src/questions.json")).expect("the cases parse");
    let question = cases["cases"][0]["said"].as_str().expect("a question").to_string();
    assert!(question.contains("\n\n(Answer with one of: "), "{question}");
    g5.reply(&door, "work:1", &question).await;
    let mut asked = vec![];
    person_got(&mut person, &mut asked, &[question.as_str()]).await;
    say(&mut person, agent, "Mint").await;
    sent.push("Mint".into());
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "its answer, once: {:?}", &door.texts()[door.texts().len().saturating_sub(3)..]);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(door.with(|s| s.heads.iter().filter(|h| h.0 == "GET").all(|h| !h.2)), "no GET carried a length");
    drop(g5);
}

/// Whether a person's burst, or a phone gone offline, stalls a door's
/// sends (the lab's crash step did, 5 of 5, on 11-20). Two ways, by
/// `BURST_REPLIES`:
/// - 0: the person sends `BURST_N` messages `BURST_PACE_MS` apart,
///   reading its own events the whole time when `BURST_READ=1`, or not
///   at all until 15 s after the burst (`BURST_READ=0`); then every
///   delivery receipt is waited for (60 s), and one reply after (120 s);
/// - `n`: the person's app frozen (`docker pause`: a phone offline) while
///   its door sends it `n` replies, then back; every reply waited for (10
///   minutes).
/// Prints what came, when, and what the door's newest save holds of the
/// core's sends still waiting. Lower rung: the lab's server and client.
/// Run by hand, one at a time, e.g. `BURST_REPLIES=150 cargo test -p
/// fragment-bridge --test door a_burst -- --ignored --nocapture`
/// (`SIMPLEX_LAB_LOG_AGENT=1` keeps the person's SMP agent's log).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "an experiment: run by hand"]
async fn a_burst_against_the_real_core() {
    let var = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (n, pace, read, offline) = (var("BURST_N", 40) as usize, var("BURST_PACE_MS", 300), var("BURST_READ", 1) == 1, var("BURST_REPLIES", 0));
    build_image();
    let mut lab = Lab::up("burst");
    let door = FakeDoor::start().await;
    let g1 = start(&lab, &door, 1);
    g1.until(Duration::from_secs(60), |s| s["ready"] == true).await;
    let (code, _) = g1.call("POST", "/link", Some(door.mark()), None).await;
    assert_eq!(code, 202);
    let s = g1.until(Duration::from_secs(60), |s| s["phase"] == "inviting").await;
    let mut person = lab.start("person", false).await;
    let r = person.cmd(&format!("/connect {}", s["link"].as_str().expect("its invitation"))).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    g1.until(Duration::from_secs(90), |s| s["phase"] == "linked").await;
    let agent = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the door, at the person's");
    let came = |person: &Api, text: &str| person.events.iter().filter(|e| tag(e) == "newChatItems").flat_map(|e| e["chatItems"].as_array().cloned().unwrap_or_default()).any(|ci| ci["chatItem"]["content"]["msgContent"]["text"] == text);
    let waiting = |lab: &Lab, door: &FakeDoor| -> (i64, Option<i64>) {
        let newest = door.with(|s| s.saves.last().map(|k| k.bytes.clone())).expect("a save");
        let path = lab.dir.join("burst-agent.db");
        std::fs::write(&path, file_of(&newest, "sx_agent.db")).expect("written");
        let c = rusqlite::Connection::open(&path).expect("the save's agent database");
        let waiting = c.query_row("SELECT count(*) FROM snd_message_deliveries", [], |r| r.get(0)).expect("its deliveries");
        (waiting, c.query_row("SELECT max(retry_int_slow) FROM snd_messages", [], |r| r.get(0)).expect("its retries"))
    };
    if offline > 0 {
        let ctr = lab.container("person");
        docker(&["pause", &ctr]).expect("paused");
        let t0 = Instant::now();
        for i in 0..offline {
            g1.reply(&door, &format!("chat:o{i}"), &format!("o{i}")).await;
        }
        let asked_in = t0.elapsed();
        docker(&["unpause", &ctr]).expect("unpaused");
        let t1 = Instant::now();
        // bounded: 600 s
        while !(0..offline).all(|i| came(&person, &format!("o{i}"))) && t1.elapsed() < Duration::from_secs(600) {
            person.drain(Duration::from_millis(500)).await;
        }
        let all = (0..offline).all(|i| came(&person, &format!("o{i}")));
        let (sends, slow) = waiting(&lab, &door);
        println!("OFFLINE replies={offline}: asked in {asked_in:?}; every one arrived: {all}, {:?} after the phone was back; the door's newest save: {sends} sends waiting, slowest retry {slow:?} µs", t1.elapsed());
        return;
    }
    let t0 = Instant::now();
    for i in 0..n {
        say(&mut person, agent, &format!("b{i}")).await;
        if read {
            person.drain(Duration::from_millis(pace.max(1))).await;
        } else if pace > 0 {
            tokio::time::sleep(Duration::from_millis(pace)).await;
        }
    }
    let sent_in = t0.elapsed();
    if !read {
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
    let t1 = Instant::now();
    // bounded: 60 s
    while receipted(&person).len() < n && t1.elapsed() < Duration::from_secs(60) {
        person.drain(Duration::from_millis(500)).await;
    }
    let (receipts, receipts_in) = (receipted(&person).len(), t1.elapsed());
    let t2 = Instant::now();
    g1.reply(&door, "chat:after", "after").await;
    // bounded: 120 s
    while !came(&person, "after") && t2.elapsed() < Duration::from_secs(120) {
        person.drain(Duration::from_millis(500)).await;
    }
    let reply = came(&person, "after").then(|| t2.elapsed());
    let (sends, slow) = waiting(&lab, &door);
    println!("BURST n={n} pace={pace}ms read={read}: sent in {sent_in:?}; receipts {receipts}/{n} in {receipts_in:?}; the reply after: {reply:?}; the door's newest save: {sends} sends waiting, slowest retry {slow:?} µs");
}

/// Goal (S1): a door carries files both ways, against the real core: a
/// person's voice message (past what goes inline: over XFTP) is received
/// by the door's core, given whole to its door (its SHA-256, typed as the
/// apps record one) and delivered naming it; a reply's file, which its
/// door holds, reaches the person whole after the reply's text, the reply
/// done once the core took it. The core's XFTP traffic goes only through
/// its door's relay (a container on Cloudflare reaches no XFTP server on
/// 443 itself): the fake door's relay is the lab's XFTP server's one way
/// in from the door, and it carried more than both files. Lower rung,
/// labeled so: the lab's SMP and XFTP servers, a fake door's relay (a
/// byte pipe), not the cell's. Needs the host's ffmpeg.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and ffmpeg; run by hand"]
async fn files_against_the_real_core() {
    build_image();
    let mut lab = Lab::up_with("door-files", true);
    let door = FakeDoor::start().await;
    door.with(|s| s.keep = Some(lab.dir.clone()));
    let ip = docker(&["inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", &lab.container("xftp")]).expect("the XFTP server's address");
    let to: std::net::SocketAddr = format!("{}:443", ip.trim()).parse().expect("an address");
    door.with(|s| s.relay_to.insert(XFTP_HOST.into(), to));

    let g1 = start(&lab, &door, 1);
    g1.until(Duration::from_secs(60), |s| s["ready"] == true && s["files"] == true).await;
    let (code, _) = g1.call("POST", "/link", Some(door.mark()), None).await;
    assert_eq!(code, 202);
    let s = g1.until(Duration::from_secs(60), |s| s["phase"] == "inviting").await;
    let mut person = lab.start("person", false).await;
    let r = person.cmd(&format!("/connect {}", s["link"].as_str().expect("its invitation"))).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    g1.until(Duration::from_secs(90), |s| s["phase"] == "linked").await;
    let agent = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the door, at the person's");

    // the person's voice message, over XFTP (past 255 KB), then a word
    let name = "memo.m4a";
    let voice = lab.db_dir("person").join("files").join(name);
    voice_file(&voice, 120, true);
    let bytes = std::fs::read(&voice).expect("made");
    assert!(bytes.len() > 255 * 1024, "past what goes inline: {}", bytes.len());
    let body = json!([{ "msgContent": { "type": "voice", "text": "", "duration": 120 }, "filePath": format!("/data/files/{name}") }]);
    let r = person.cmd(&format!("/_send @{agent} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "the person's voice message: {r}");
    say(&mut person, agent, "after the memo").await;
    let t0 = Instant::now();
    while door.with(|s| s.deliveries.len()) < 2 {
        assert!(t0.elapsed() < Duration::from_secs(180), "the voice message and the word after it are delivered: {:?}\n{}", door.with(|s| s.deliveries.clone()), logs(&g1.ctr));
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (first, second) = door.with(|s| (s.deliveries[0].clone(), s.deliveries[1].clone()));
    println!("the voice message delivered in {} ms: {first}", t0.elapsed().as_millis());
    assert_eq!(first["kind"], "voice");
    assert_eq!(first["file"]["sha256"], sha256_hex(&bytes), "its file whole, named by its SHA-256");
    assert_eq!((first["file"]["type"].as_str(), first["file"]["size"].as_u64(), first["file"]["name"].as_str()), (Some("audio/mp4"), Some(bytes.len() as u64), Some(name)));
    assert_eq!(door.with(|s| s.files.get(&sha256_hex(&bytes)).cloned()), Some(Bytes::from(bytes.clone())), "its door holds it whole");
    assert_eq!(second["text"], "after the memo", "deliveries stay in order: the word after the memo comes after it");
    let up_in = door.with(|s| s.relay_down);
    assert!(up_in > bytes.len() as u64, "the voice message came through the door's relay: {up_in} bytes down");

    // a reply's file, which its door holds: after its text, whole
    let file: Vec<u8> = (0..400_000u32).map(|n| (n % 251) as u8).collect();
    let sha = sha256_hex(&file);
    door.with(|s| s.files.insert(sha.clone(), Bytes::from(file.clone())));
    let ask = json!({ "key": "chat:9", "text": "here is the file", "files": [{ "sha256": sha, "name": "notes.bin", "type": "application/octet-stream", "size": file.len() }] });
    let t0 = Instant::now();
    // bounded: 120 s
    loop {
        let (code, v) = g1.call("POST", "/reply", Some(door.mark()), Some(ask.clone())).await;
        if code == 200 {
            assert_eq!(v, json!({ "done": true }));
            break;
        }
        assert!(code == 202 || code == 503, "the reply with a file: {code} {v}");
        assert!(t0.elapsed() < Duration::from_secs(120), "the reply with a file is done: {}", logs(&g1.ctr));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(door.with(|s| s.read), 1, "its file read from its door once");
    // its file first (the words' wait would take its message's event too)
    let (content, path) = received_file(&mut person, &lab.db_dir("person").join("files"), "file", Duration::from_secs(120)).await;
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "here is the file")).await;
    assert_eq!(sha256_of(&path), sha, "the reply's file reaches the person whole: {content}");
    let (up, down) = door.with(|s| (s.relay_up, s.relay_down));
    println!("the reply's file whole at the person's in {} ms; the relay carried {up} bytes up, {down} down", t0.elapsed().as_millis());
    assert!(up > file.len() as u64, "the reply's file went through the door's relay: {up} bytes up");
    let s = g1.until(Duration::from_secs(60), |s| s["sending"] == 0).await;
    assert_eq!(s["replies"], 0, "nothing waits");
    let left = docker(&["exec", &g1.ctr, "sh", "-c", "ls -A /data/door/out | wc -l"]).expect("its copies");
    assert_eq!(left.trim(), "0", "its copy let go once uploaded");
}

/// The platform's handover of juniper's link, as the cell's Computer
/// object asks it (its screen port, a handover's ticket): its status, its
/// door's first save and the SHA-256 its answer says.
fn hand_over(c: &Computer, ticket: &Value, into: &std::path::Path) -> (u16, Vec<u8>, String) {
    let url = format!("http://127.0.0.1:{}/simplex/handover?agent=juniper--k3x9", c.port);
    let out = Command::new("curl").args(["-s", "-X", "POST", "-D", "-", "-o"]).arg(into).args(["-H", "content-type: application/json", "--data-binary", &ticket.to_string(), &url]).output().expect("curl runs");
    let head = String::from_utf8_lossy(&out.stdout).to_string();
    let status = head.lines().next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let sha = head.lines().find_map(|l| l.strip_prefix("x-door-sha256: ")).unwrap_or("").trim().to_string();
    (status, std::fs::read(into).unwrap_or_default(), sha)
}

/// The way back, as the cell's Computer object asks it: its ticket, and its
/// door's newest save (`behind` its door's newest effects or not).
fn hand_back(c: &Computer, ticket: &Value, save: &std::path::Path, behind: bool) -> (u16, Value) {
    let url = format!("http://127.0.0.1:{}/simplex/back?agent=juniper--k3x9", c.port);
    // named by its SHA-256, as its door names it
    let sha: String = sha2::Digest::finalize(sha2::Digest::chain_update(<sha2::Sha256 as sha2::Digest>::new(), std::fs::read(save).unwrap_or_default())).iter().map(|b| format!("{b:02x}")).collect();
    let out = Command::new("curl")
        .args(["-s", "-X", "POST", "-w", "\n%{http_code}", "-H", &format!("x-door-ticket: {ticket}"), "-H", &format!("x-door-behind: {}", u8::from(behind)), "-H", &format!("x-door-sha256: {sha}"), "--data-binary"])
        .arg(format!("@{}", save.display()))
        .arg(&url)
        .output()
        .expect("curl runs");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
    (code.trim().parse().unwrap_or(0), serde_json::from_str(body).unwrap_or(Value::Null))
}

fn relayed_on(fake: &support::fake::Fake, chat: &str) -> Vec<Value> {
    fake.with(|w| w.records(chat, "chat").into_iter().filter(|r| r["body"]["relayed"]["via"] == "simplex").collect())
}

/// Goal (D4, Paul's FIN-731: the move carries the data): a computer's
/// SimpleX link moves into a door with nothing for its owner to do, and
/// back. Linked on a computer (the bridge's own core and chat), the
/// owner's message answered; handed over by the platform's ticket: the
/// computer's core stopped for good, its answer the door's first save,
/// whose SHA-256 it says; a second handover's ticket refused. The door
/// restores it: the same contact, linked, with no invitation; the owner's
/// next message arrives there once, and nothing the computer posted comes
/// again; the door's reply reaches the owner's app once. The way back: the
/// door's planned stop saves last, the computer places that save, takes its
/// door key back, and its core runs again on it: the owner's next message
/// is answered in the computer's chat. Method: the real bridge image (the
/// lab's computer) and the door's real image, SimpleX's real core, a
/// person's real client, fakes for the platform and the door's object.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn a_computers_link_moves_into_a_door_and_back() {
    build_image();
    build_bridge_image();
    let mut lab = Lab::up("door-move");
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let data = lab.dir.join("computer-data");
    let smp = format!("BRIDGE_SIMPLEX_SMP={}", lab.server);
    let c = Computer::start_with(&lab, &fake, &data, &["-e", &smp]);
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    c.link(&fake);
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    person.cmd(&format!("/connect {}", v["link"].as_str().expect("its link"))).await;
    c.state_until(Duration::from_secs(90), "linked", |v| v["phase"] == "linked").await;
    let agent = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
    fake.until(30_000, "its SimpleX chat", |w| w.made.len() == 1).await;
    let chat = fake.with(|w| w.made[0].clone());
    say(&mut person, agent, "before the move").await;
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] before the move")).await;
    assert_eq!(relayed_on(&fake, &chat).len(), 1);

    // its chat unasked (its `held` answering 503): nothing handed over,
    // and its core runs again, its link at one end (Codex's rounds on #1541)
    fake.with(|w| w.fail_held = 1);
    let unasked = lab.dir.join("unasked.sxd");
    let t0 = Instant::now();
    loop {
        let (code, body, _) = hand_over(&c, &fake.ticket("juniper", "handover"), &unasked);
        if code == 503 {
            break;
        }
        assert!(code == 409 && t0.elapsed() < Duration::from_secs(30), "refused while its chat answers nothing: {code} {}", String::from_utf8_lossy(&body));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let t0 = Instant::now();
    while c.cores() != 1 {
        assert!(t0.elapsed() < Duration::from_secs(30), "its core runs again");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // handed over: once its posts are acknowledged (busy until then)
    fake.with(|w| w.computer["simplexMoved"] = json!(["juniper--k3x9"]));
    let ticket = fake.ticket("juniper", "handover");
    let first = lab.dir.join("handover.sxd");
    let t0 = Instant::now();
    let (bytes, sha) = loop {
        let (code, bytes, sha) = hand_over(&c, &fake.ticket("juniper", "handover"), &first);
        if code == 200 {
            break (bytes, sha);
        }
        assert!(code == 409 && t0.elapsed() < Duration::from_secs(30), "handed over within 30 s: {code} {}", String::from_utf8_lossy(&bytes));
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(sha256_hex(&bytes), sha, "its answer names its save's SHA-256");
    assert_eq!(kept_of(&bytes)["link"]["phase"], "linked");
    // its chat database names each message's sender as its posts did: the
    // post id a handover asks of (`save::received_past`) is the one posted
    let ids = sent_ids_in(&bytes, &lab.dir);
    let posted: Vec<String> = fake.with(|w| w.fragments[&chat].channels["chat"].ids.keys().cloned().collect());
    assert!(!ids.is_empty() && ids.iter().all(|id| posted.contains(&fragment_bridge::records::relayed_id("juniper--k3x9", id))), "{ids:?} posted as {posted:?}");
    let again = lab.dir.join("again.sxd");
    let (code, _, _) = hand_over(&c, &ticket, &again);
    assert_eq!(code, 409, "another handover's ticket, once moved");
    let t0 = Instant::now();
    while c.cores() != 0 {
        assert!(t0.elapsed() < Duration::from_secs(10), "its core stopped");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // the door restores it as its first save
    let door = FakeDoor::start().await;
    door.with(|s| {
        s.keep = Some(lab.dir.clone());
        s.saves.push(Kept { start: 0, life: 0, seq: 1, mark: 0, bytes: Bytes::from(bytes.clone()) });
    });
    let g1 = start(&lab, &door, 1);
    let s = g1.until(Duration::from_secs(90), |s| s["ready"] == true && s["phase"] == "linked").await;
    assert_eq!(s["name"], "person", "the same contact, with no invitation: {s}");
    say(&mut person, agent, "after the move").await;
    let t0 = Instant::now();
    while door.texts() != ["after the move"] {
        assert!(t0.elapsed() < Duration::from_secs(60), "the owner's next message at the door, and nothing again: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut got = vec![];
    g1.reply(&door, "chat:1", "from the door").await;
    person_got(&mut person, &mut got, &["from the door"]).await;
    assert_eq!(relayed_on(&fake, &chat).len(), 1, "nothing more on the computer's chat");
    assert_eq!(c.cores(), 0, "the computer's core never ran beside the door's");
    assert_eq!(door.with(|s| s.delivered.iter().filter(|(_, t)| t == "after the move").count()), 1, "delivered once");

    // the way back: the door's planned stop saves last; the computer places it
    let saves = door.with(|s| s.saves.len());
    g1.kill("TERM");
    assert_eq!(g1.exited(), 0, "a planned stop");
    let (newest, behind) = door.with(|s| (s.saves.last().expect("its last save").bytes.clone(), s.saves.last().expect("its last").mark < s.marks));
    assert!(door.with(|s| s.saves.len()) > saves, "its last save taken");
    let newest_at = lab.dir.join("newest.sxd");
    std::fs::write(&newest_at, &newest).expect("written");
    fake.with(|w| w.computer["simplexMoved"] = json!([]));
    let (code, v) = hand_back(&c, &fake.ticket("juniper", "back"), &newest_at, behind);
    assert_eq!(code, 200, "{v}");
    assert!(fake.with(|w| w.doors.contains_key("juniper--k3x9")), "its door key taken back");
    c.state_until(Duration::from_secs(90), "running again", |v| v["running"] == true && v["phase"] == "linked").await;
    say(&mut person, agent, "after the way back").await;
    received(&mut person, Duration::from_secs(90), |got| got.iter().any(|t| t == "echo: [paul] after the way back")).await;
    assert_eq!(relayed_on(&fake, &chat).len(), 2, "on the computer's chat again");
    println!("moved and back: the door's save {} bytes, behind {behind}", bytes.len());
}

/// Goal (D4, Paul's FIN-731 and his answer of 11-24: offered, never kept
/// for them): an owner's SimpleX chat with their old finite.computer agent,
/// staged on their computer by that move and never kept there, moves into a
/// door as staged. Its handover is the computer's read of it (as its own
/// adoption reads one, the old agent's core alone on it), its answer a save
/// whose state holds that read and an unbound link; the computer offers it
/// no more. Kept at its owner's word (its connector's connect `keep`), the
/// door adopts it as it restores: their contact bound, no code to scan, and
/// their app told it moved; what the phone sent while it moved arrives
/// once, nothing from before it; and the door's reply reaches the phone.
/// Method: the old core (v7.0.2) as finite.computer ran it, the lab's
/// computer, the door's real image, fakes for the platform and the door's
/// object.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and python3; run by hand"]
async fn a_staged_chat_moves_into_a_door_and_is_kept() {
    use fragment_bridge::simplex::stage;
    build_image();
    build_bridge_image();
    let mut lab = Lab::up("door-staged");
    let mut person = lab.start("phone", false).await;
    let mut friend = lab.start("friend", false).await;
    let mut old = lab.start_old("old").await;
    let a = old.cmd("/address").await;
    let address = a["connLinkContact"]["connShortLink"].as_str().or(a["connLinkContact"]["connFullLink"].as_str()).expect("its address").to_string();
    assert_eq!(tag(&old.cmd("/auto_accept on").await), "userContactLinkUpdated");
    let mut at = vec![];
    for c in [&mut person, &mut friend] {
        c.cmd(&format!("/c {address}")).await;
        at.push(c.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the old agent, at theirs"));
    }
    let at_person = at[0];
    let owner = old.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected" && e["contact"]["profile"]["displayName"] == "phone").await["contact"]["contactId"].as_i64().expect("the owner, at the old agent's");
    say(&mut person, at_person, "before the move").await;
    old.wait(Duration::from_secs(30), |e| tag(e) == "newChatItems" && e.to_string().contains("before the move")).await;
    drop(old);
    lab.stop("old");
    let olddir = lab.db_dir("old");
    say(&mut person, at_person, "while moving").await;
    let owner_json = lab.dir.join("owner.json");
    std::fs::write(&owner_json, format!("{{\"contact\":{owner}}}")).expect("written");
    let zip = stage_zip(&lab.dir, &[(stage::CHAT, &olddir.join("identity_chat.db")), (stage::AGENT, &olddir.join("identity_agent.db")), (stage::OWNER, &owner_json)]);

    // staged on its new agent's computer, as finite.computer's move left it
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.chat_named("juniper-chat--h6j7", &["juniper"]);
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
        w.stages.insert("old1.p1.1".into(), ("juniper--k3x9".into(), zip.clone()));
    });
    let data = lab.dir.join("computer-data");
    let smp = format!("BRIDGE_SIMPLEX_SMP={}", lab.server);
    let c = Computer::start_with(&lab, &fake, &data, &["-e", &smp]);
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    move_routine(&fake, "juniper", "old1.p1.1");
    c.state_until(Duration::from_secs(60), "staged", |v| v["staged"] == true).await;

    // handed over as staged: its read in its save, its link unbound
    fake.with(|w| w.computer["simplexMoved"] = json!(["juniper--k3x9"]));
    let (code, bytes, sha) = hand_over(&c, &fake.ticket("juniper", "handover"), &lab.dir.join("staged.sxd"));
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(sha256_hex(&bytes), sha);
    let kept = kept_of(&bytes);
    assert_eq!((kept["link"]["phase"].as_str(), kept["adopt"]["owner"].as_i64()), (Some("off"), Some(owner)), "{kept}");
    assert!(kept["adopt"]["leftovers"].as_array().is_some_and(|l| l.iter().any(|x| x["contact"].is_i64())), "the friend, to let go: {kept}");
    c.state_until(Duration::from_secs(10), "offered here no more", |v| v["staged"] != true).await;
    assert_eq!(c.cores(), 0);

    // kept at its owner's word: the door adopts it as it restores
    let door = FakeDoor::start().await;
    door.with(|s| {
        s.keep = Some(lab.dir.clone());
        s.saves.push(Kept { start: 0, life: 0, seq: 1, mark: 0, bytes: Bytes::from(bytes.clone()) });
    });
    let g1 = start(&lab, &door, 1);
    let s = g1.until(Duration::from_secs(120), |s| s["ready"] == true && s["phase"] == "linked").await;
    assert_eq!(s["name"], "phone", "its owner's contact, with no code to scan: {s}");
    phone_got(&mut person, "Hi, it's Juniper. I've moved to my new home: write to me here, as before.", Duration::from_secs(90)).await;
    let t0 = Instant::now();
    while !door.texts().contains(&"while moving".to_string()) {
        assert!(t0.elapsed() < Duration::from_secs(60), "what the phone sent while it moved: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    say(&mut person, at_person, "after the move").await;
    let t0 = Instant::now();
    while door.texts() != ["while moving", "after the move"] {
        assert!(t0.elapsed() < Duration::from_secs(60), "each once, nothing from before the move: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut got = vec![];
    g1.reply(&door, "chat:1", "from the door").await;
    person_got(&mut person, &mut got, &["from the door"]).await;
    println!("a staged chat moved and kept: its save {} bytes", bytes.len());
}
