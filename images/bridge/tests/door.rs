//! A SimpleX door against the real core: the door's own image
//! (images/simplex-door/Dockerfile) on the SimpleX lab's network (a local
//! SMP server, a person's simplex-chat), with a fake door on the host in
//! place of its Durable Object: it keeps every save, numbers every effect
//! (its mark), says when a save is behind, and fences saves by generation,
//! as the door's protocol says (images/simplex-door/src/door.rs).
//!
//! Lower rung, labeled so: the lab's server, not SimpleX's presets, and a
//! container per start on this machine's Docker, not Cloudflare's.
//!
//! Run by hand, as the lab's own tests are:
//! `cargo test -p fragment-bridge --test door -- --ignored --nocapture`.

mod support;

use std::collections::BTreeMap;
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

/// One save the fake door took.
#[derive(Clone)]
struct Kept {
    generation: u64,
    mark: u64,
    bytes: Bytes,
}

#[derive(Default)]
struct DoorState {
    saves: Vec<Kept>,
    /// The generation started last: older ones' saves are refused (409).
    generation: u64,
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

    async fn answer(self, req: Request<Incoming>) -> Response<fragment_bridge::net::Body> {
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let header = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let generation: u64 = header("x-door-generation").and_then(|g| g.parse().ok()).expect("each call carries its generation");
        let sized = header("content-length").is_some();
        let mark: Option<u64> = header("x-door-mark").and_then(|m| m.parse().ok());
        let body = req.into_body().collect().await.unwrap().to_bytes();
        self.with(|s| s.heads.push((method.clone(), path.clone(), sized, generation)));
        let answer = |status: u16, headers: &[(&str, String)], body: Bytes| {
            let mut r = Response::builder().status(status);
            for (k, v) in headers {
                r = r.header(*k, v.as_str());
            }
            r.body(Full::new(body)).unwrap()
        };
        match (method.as_str(), path.as_str()) {
            ("GET", "/save") => self.with(|s| {
                let at = s.pinned.or(s.saves.len().checked_sub(1));
                match at.map(|i| s.saves[i].clone()) {
                    None => answer(204, &[], Bytes::new()),
                    Some(k) => {
                        let behind = if k.mark < s.marks { "1" } else { "0" };
                        answer(200, &[("x-door-mark", s.marks.to_string()), ("x-door-behind", behind.into())], k.bytes)
                    }
                }
            }),
            ("PUT", "/save") => self.with(|s| {
                assert!(query.starts_with("seq="), "a save is numbered: {query}");
                if generation < s.generation {
                    s.refused += 1;
                    return answer(409, &[], Bytes::new());
                }
                s.saves.push(Kept { generation, mark: mark.expect("a save carries its mark"), bytes: body });
                answer(200, &[], Bytes::new())
            }),
            ("POST", "/deliver") => self.with(|s| {
                if s.closed {
                    return answer(503, &[], Bytes::new());
                }
                let v: Value = serde_json::from_slice(&body).expect("a delivery is JSON");
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
    door.with(|s| s.generation = generation);
    let ctr = format!("{}-door-{generation}", lab.prefix);
    let _ = docker(&["rm", "-f", &ctr]);
    let (net, api, gen) = (format!("{}-net", lab.prefix), format!("DOOR_API=http://door.test:{}", door.port), format!("DOOR_GENERATION={generation}"));
    let smp = format!("DOOR_SMP={}", lab.server);
    docker(&["run", "-d", "--platform", "linux/amd64", "--name", &ctr, "--network", &net, "--add-host", "door.test:host-gateway", "-e", &api, "-e", &gen, "-e", &smp, "-e", "DOOR_NAME=Juniper", "-p", "127.0.0.1::8080", DOOR_IMAGE]).expect("the door starts");
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
        if let Some(b) = body {
            c.args(["-H", "content-type: application/json", "--data-binary", &b.to_string()]);
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

    async fn reply(&self, door: &FakeDoor, key: &str, text: &str) -> Value {
        let (code, v) = self.call("POST", "/reply", Some(door.mark()), Some(json!({ "key": key, "text": text }))).await;
        assert_eq!(code, 200, "the reply {key}: {v}");
        v
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

/// The runner's own state in a save (`door.json`, beside the databases),
/// read as the runner packs it: gzip, `SXD1`, then each file's name and
/// bytes (images/simplex-door/src/save.rs).
fn kept_of(packed: &[u8]) -> Value {
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
        if name == "door.json" {
            return serde_json::from_slice(&raw[at..at + len]).expect("the runner's state");
        }
        at += len;
    }
    panic!("a save without the runner's state")
}

fn build_image() {
    let have = docker(&["image", "inspect", "--format", "{{.Id}}", DOOR_IMAGE]).is_ok();
    if have && std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_ok() {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new("docker").args(["build", "--platform", "linux/amd64", "-q", "-f", "images/simplex-door/Dockerfile", "-t", DOOR_IMAGE, "."]).current_dir(&root).output().expect("docker runs");
    assert!(out.status.success(), "the door's image builds: {}", String::from_utf8_lossy(&out.stderr));
}

/// What the person received from the door, each once, in order.
async fn person_got(person: &mut Api, seen: &mut Vec<String>, want: &[&str]) {
    let more = received(person, Duration::from_secs(90), |got| want.iter().all(|w| seen.iter().chain(got.iter()).any(|g| g == w))).await;
    seen.extend(more);
    for w in want {
        assert_eq!(seen.iter().filter(|g| g == w).count(), 1, "the person has {w:?} once: {seen:?}");
    }
}

/// Goal: a door outside any computer holds one link through everything
/// its container meets, nothing lost or doubled either way: a new door
/// linked by its invitation; messages each way; a reply asked again sent
/// once; a crash (SIGKILL) mid-burst, its next start reading back from its
/// cursor; a rollback to a save from before both bursts, healed by the one
/// forced sync the door's "behind" asks; a planned restart (SIGTERM) whose
/// last save leaves the next start in step, with no sync; and a start
/// superseded by a newer generation, whose save is refused, ending it.
/// Method: the door's real image and SimpleX's real core, a person's real
/// client, and a fake door on the host as its Durable Object would be.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_door_against_the_real_core() {
    build_image();
    let mut lab = Lab::up("door");
    let door = FakeDoor::start().await;

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
    let mut person = lab.start("person", false).await;
    let r = person.cmd(&format!("/connect {}", s["link"].as_str().expect("its invitation"))).await;
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
        assert_eq!(g1.reply(&door, &format!("chat:{n}"), &format!("r{n}")).await, json!({ "replayed": false }));
    }
    // asked again under a new mark: sent once, and its mark taken all the same
    assert_eq!(g1.reply(&door, "chat:5", "r5").await, json!({ "replayed": true }), "a key asked again");
    person_got(&mut person, &mut got, &["r1", "r2", "r3", "r4", "r5"]).await;

    // a crash with messages saved and not yet delivered (its door could
    // not take them): its next start delivers them from its cursor; and one
    // sent while it was down arrives then
    door.with(|s| s.closed = true);
    let saves = door.with(|s| s.saves.len());
    for n in 11..=15 {
        say(&mut person, agent, &format!("p{n}")).await;
        sent.push(format!("p{n}"));
    }
    g1.until(Duration::from_secs(30), |s| s["waiting"].as_u64() >= Some(5)).await;
    let t0 = Instant::now();
    // a save after the five arrived: the debounce, then the gap
    while door.with(|s| s.saves.len()) < saves + 1 || t0.elapsed() < Duration::from_millis(2 * 3_000 + 1_000) {
        assert!(t0.elapsed() < Duration::from_secs(20), "the five are saved");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g1.kill("KILL");
    assert_eq!(g1.exited(), 137);
    drop(g1);
    assert_eq!(door.texts(), sent[..10], "none of the five was delivered before the crash");
    door.with(|s| s.closed = false);
    say(&mut person, agent, "p16").await;
    sent.push("p16".into());
    let g2 = start(&lab, &door, 2);
    let s = g2.until(Duration::from_secs(60), |s| s["ready"] == true && s["phase"] == "linked" && s["syncing"] == false).await;
    assert_eq!(s["forcedSyncs"], 0, "nothing moved past its save: no sync: {s}");
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "nothing lost in the crash: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g2.reply(&door, "chat:6", "r6").await;
    person_got(&mut person, &mut got, &["r6"]).await;

    // a rollback to the save that held the link alone, behind both bursts:
    // one forced sync, then each way in step, nothing delivered again
    g2.kill("KILL");
    drop(g2);
    door.with(|s| s.pinned = Some(linked_save));
    let g3 = start(&lab, &door, 3);
    g3.until(Duration::from_secs(60), |s| s["ready"] == true && s["forcedSyncs"] == 1).await;
    door.with(|s| s.pinned = None);
    g3.until(Duration::from_secs(90), |s| s["syncing"] == false).await;
    recovered(&mut person, agent, Duration::from_secs(90), || true).await;
    say(&mut person, agent, "p17").await;
    sent.push("p17".into());
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "a message after the rollback, once: {:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g3.reply(&door, "chat:7", "r7").await;
    person_got(&mut person, &mut got, &["r7"]).await;
    assert_eq!(g3.state().await["forcedSyncs"], 1, "one sync, not more");

    // a planned restart: its last save holds its newest mark, so the next
    // start is in step and asks no sync
    say(&mut person, agent, "p18").await;
    sent.push("p18".into());
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "{:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g3.reply(&door, "chat:8", "r8").await;
    g3.kill("TERM");
    assert_eq!(g3.exited(), 0, "a planned stop ends well: {}", logs(&g3.ctr));
    drop(g3);
    let newest = door.with(|s| (s.saves.last().map(|k| (k.generation, k.mark)), s.marks));
    assert_eq!(newest.0, Some((3, newest.1)), "the last save is the stop's, at the newest mark");
    let g4 = start(&lab, &door, 4);
    let s = g4.until(Duration::from_secs(60), |s| s["ready"] == true && s["syncing"] == false).await;
    assert_eq!(s["forcedSyncs"], 0, "in step: no sync: {s}");
    person_got(&mut person, &mut got, &["r8"]).await;
    g4.reply(&door, "chat:9", "r9").await;
    person_got(&mut person, &mut got, &["r9"]).await;

    // superseded: a newer generation started, this one's save refused, and
    // it ends
    door.with(|s| s.generation = 5);
    say(&mut person, agent, "p19").await;
    sent.push("p19".into());
    assert_eq!(g4.exited(), 1, "a start superseded ends: {}", logs(&g4.ctr));
    assert!(door.with(|s| s.refused) >= 1);
    drop(g4);
    let g5 = start(&lab, &door, 5);
    g5.until(Duration::from_secs(60), |s| s["ready"] == true && s["syncing"] == false).await;
    recovered(&mut person, agent, Duration::from_secs(90), || true).await;
    let t0 = Instant::now();
    while door.texts() != sent {
        assert!(t0.elapsed() < Duration::from_secs(60), "{:?}", door.texts());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    g5.reply(&door, "chat:10", "r10").await;
    person_got(&mut person, &mut got, &["r10"]).await;
    assert_eq!(got, (1..=10).map(|n| format!("r{n}")).collect::<Vec<_>>(), "every reply once, in order");
    assert!(door.with(|s| s.heads.iter().filter(|h| h.0 == "GET").all(|h| !h.2)), "no GET carried a length");
    drop(g5);
}
