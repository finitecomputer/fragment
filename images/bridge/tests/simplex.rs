//! The SimpleX lab: a local SMP server and simplex-chat v7.0.3 clients in
//! Docker (`tests/simplex/lab.Dockerfile`), each driven over its bot API,
//! with the bridge's `simplex` module (its wire and its link's rules) on the
//! agent's side, against the real core.
//!
//! Lower rung, labeled so: the real simplex-chat and a real SMP server, but
//! one local server on a Docker network, not SimpleX's preset ones on the
//! internet, and the test in place of the image's process.
//!
//! Run by hand (it pulls the SMP server's image and builds the lab's once):
//! `cargo test -p fragment-bridge --test simplex -- --ignored --nocapture`.
//! CI's Docker lane runs docker.rs alone, not this.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fragment_bridge::simplex::link::{Action, Inbound, Input, Link, Notice, Phase};
use fragment_bridge::simplex::wire::{self, Resp};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The SMP server, pinned: v7.0.1 by its multi-arch index's digest (Docker
/// Hub), the version the spike ran (docs/explorations/simplex-connector.md).
const SMP_IMAGE: &str = "simplexchat/smp-server:v7.0.1@sha256:7d825822839a9d5ee9e9a99563d77a319816c7a8e0b4b7bf4dcebe448897af54";
const LAB_IMAGE: &str = "fragment-simplex-lab:7.0.3";
/// The SMP server's name on the lab's network: a domain, as its image's
/// entrypoint wants one with a dot.
const SMP_HOST: &str = "smp.lab.test";

fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker").args(args).output().map_err(|e| format!("docker: {e}"))?;
    if !out.status.success() {
        return Err(format!("docker {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn uid_gid() -> String {
    let id = |flag: &str| String::from_utf8_lossy(&Command::new("id").arg(flag).output().expect("id runs").stdout).trim().to_string();
    format!("{}:{}", id("-u"), id("-g"))
}

/// One Docker network, one SMP server, and clients, all named
/// `<prefix>-…`; each client's database in `dir/<name>` on the host.
struct Lab {
    prefix: String,
    dir: PathBuf,
    server: String,
    clients: Vec<String>,
}

impl Lab {
    fn up(name: &str) -> Lab {
        let context = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/simplex");
        if std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_err() {
            docker(&["build", "--platform", "linux/amd64", "-q", "-f", &context.join("lab.Dockerfile").to_string_lossy(), "-t", LAB_IMAGE, &context.to_string_lossy()]).expect("the lab's image builds");
        }
        let prefix = format!("sxlab-{name}-{}", std::process::id());
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(&prefix);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the lab's directory");
        let net = format!("{prefix}-net");
        docker(&["network", "create", &net]).expect("a network");
        let smp = format!("{prefix}-smp");
        docker(&["run", "-d", "--name", &smp, "--network", &net, "--network-alias", SMP_HOST, "-e", &format!("ADDR={SMP_HOST}"), "-e", "WEB_MANUAL=1", SMP_IMAGE]).expect("the SMP server starts");
        let t0 = Instant::now();
        let fingerprint = loop {
            if let Ok(fp) = docker(&["exec", &smp, "cat", "/etc/opt/simplex/fingerprint"]) {
                if !fp.is_empty() && docker(&["logs", &smp]).is_ok_and(|_| logs(&smp).contains("Serving SMP protocol")) {
                    break fp;
                }
            }
            assert!(t0.elapsed() < Duration::from_secs(90), "the SMP server is not serving after 90 s: {}", logs(&smp));
            std::thread::sleep(Duration::from_millis(250));
        };
        Lab { prefix, dir, server: format!("smp://{fingerprint}@{SMP_HOST}"), clients: vec![] }
    }

    fn container(&self, name: &str) -> String {
        format!("{}-{name}", self.prefix)
    }

    fn db_dir(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Starts client `name` (its database kept across starts) and connects
    /// to its bot API. `bot`: `--create-bot-display-name`, else a person's
    /// `--user-display-name`.
    async fn start(&mut self, name: &str, bot: bool) -> Api {
        let data = self.db_dir(name);
        std::fs::create_dir_all(&data).expect("its directory");
        let ctr = self.container(name);
        let _ = docker(&["rm", "-f", &ctr]);
        let vol = format!("{}:/data", data.display());
        let who = if bot { "--create-bot-display-name" } else { "--user-display-name" };
        docker(&[
            "run", "-d", "--name", &ctr, "--network", &format!("{}-net", self.prefix), "--user", &uid_gid(), "-e", "HOME=/data", "-e", "LAB_API_PORT=5226", "-p", "127.0.0.1::5226", "-v", &vol,
            LAB_IMAGE, "-d", "/data/sx", "-s", &self.server, "-p", "5225", who, name, "-y",
        ])
        .expect("the client starts");
        if !self.clients.iter().any(|c| c == name) {
            self.clients.push(name.into());
        }
        let port = docker(&["port", &ctr, "5226/tcp"]).expect("its port");
        let addr = port.lines().next().expect("a port").to_string();
        Api::connect(&format!("ws://{addr}"), name, &self.dir).await.unwrap_or_else(|e| panic!("{ctr}'s bot API: {e}\n{}", logs(&ctr)))
    }

    /// SIGTERM, then the container goes (its database stays on the host).
    fn stop(&self, name: &str) {
        let ctr = self.container(name);
        docker(&["stop", "-t", "10", &ctr]).expect("it stops");
        docker(&["rm", "-f", &ctr]).expect("it goes");
    }

    /// A copy of `name`'s database files, to put back later: a disk that
    /// goes back in time.
    fn snapshot(&self, name: &str, to: &str) {
        let (from, to) = (self.db_dir(name), self.dir.join(to));
        let _ = std::fs::remove_dir_all(&to);
        std::fs::create_dir_all(&to).expect("the snapshot's directory");
        for f in db_files(&from) {
            std::fs::copy(&f, to.join(f.file_name().expect("a name"))).expect("copied");
        }
    }

    fn restore(&self, name: &str, from: &str) {
        let (to, from) = (self.db_dir(name), self.dir.join(from));
        for f in db_files(&to) {
            std::fs::remove_file(f).expect("removed");
        }
        for f in db_files(&from) {
            std::fs::copy(&f, to.join(f.file_name().expect("a name"))).expect("restored");
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        for c in &self.clients {
            let ctr = self.container(c);
            let _ = std::fs::write(self.dir.join(format!("{c}.log")), logs(&ctr));
            let _ = docker(&["rm", "-f", &ctr]);
        }
        let _ = docker(&["rm", "-f", &self.container("smp")]);
        let _ = docker(&["network", "rm", &format!("{}-net", self.prefix)]);
    }
}

fn db_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("sx_"))).collect()).unwrap_or_default();
    v.sort();
    v
}

fn logs(container: &str) -> String {
    Command::new("docker").args(["logs", container]).output().map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))).unwrap_or_default()
}

/// A client's bot API: `{"corrId", "cmd"}` out; `{"corrId", "resp"}` its
/// answer, `{"resp"}` what it says on its own. Every frame read is also
/// appended to `<lab>/<name>.frames.jsonl`.
struct Api {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next: u64,
    events: VecDeque<Value>,
    log: PathBuf,
}

impl Api {
    async fn connect(url: &str, name: &str, dir: &Path) -> Result<Api, String> {
        let t0 = Instant::now();
        loop {
            match tokio_tungstenite::connect_async(url).await {
                Ok((ws, _)) => return Ok(Api { ws, next: 1, events: VecDeque::new(), log: dir.join(format!("{name}.frames.jsonl")) }),
                Err(e) if t0.elapsed() > Duration::from_secs(60) => return Err(e.to_string()),
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }

    fn keep(&self, dir: &str, v: &Value) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.log).expect("the frames' log");
        writeln!(f, "{}", json!({ "dir": dir, "frame": v })).expect("written");
    }

    async fn read(&mut self, within: Duration) -> Option<Value> {
        loop {
            match tokio::time::timeout(within, self.ws.next()).await {
                Err(_) => return None,
                Ok(None) => panic!("the bot API closed"),
                Ok(Some(Err(e))) => panic!("reading the bot API: {e}"),
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(t.as_str()).expect("a frame is JSON");
                    self.keep("in", &v);
                    return Some(v);
                }
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// Sends `cmd`; its answer (`resp`), events read meanwhile queued.
    async fn cmd(&mut self, cmd: &str) -> Value {
        let id = self.next.to_string();
        self.next += 1;
        let frame = json!({ "corrId": id, "cmd": cmd });
        self.keep("out", &frame);
        self.ws.send(Message::text(frame.to_string())).await.expect("sent");
        let t0 = Instant::now();
        loop {
            let left = Duration::from_secs(60).checked_sub(t0.elapsed()).unwrap_or_else(|| panic!("no answer to `{cmd}` in 60 s"));
            let Some(v) = self.read(left).await else { continue };
            if v["corrId"].as_str() == Some(id.as_str()) {
                return v["resp"].clone();
            }
            self.events.push_back(v["resp"].clone());
        }
    }

    /// The next event `pred` takes, within `within`; others stay queued.
    async fn wait(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self.events.iter().position(&pred) {
            return self.events.remove(i).expect("in range");
        }
        let t0 = Instant::now();
        loop {
            let left = within.checked_sub(t0.elapsed()).unwrap_or_else(|| panic!("no such event in {within:?}; queued: {:?}", self.events.iter().map(tag).collect::<Vec<_>>()));
            if let Some(v) = self.read(left).await {
                let resp = v["resp"].clone();
                if pred(&resp) {
                    return resp;
                }
                self.events.push_back(resp);
            }
        }
    }

    /// Whatever arrives for `d`, queued.
    async fn drain(&mut self, d: Duration) {
        let t0 = Instant::now();
        while let Some(left) = d.checked_sub(t0.elapsed()) {
            if let Some(v) = self.read(left).await {
                self.events.push_back(v["resp"].clone());
            }
        }
    }
}

fn tag(v: &Value) -> String {
    v["type"].as_str().unwrap_or("?").to_string()
}


/// The agent's side as the image will run it: the bridge's `link` rules,
/// fed what the real core says through the bridge's `wire`, its commands
/// sent to the core; what it forwards and tells is kept to check.
struct Agent {
    api: Api,
    link: Link,
    forwarded: Vec<Inbound>,
    told: Vec<Notice>,
}

impl Agent {
    fn new(api: Api) -> Agent {
        Agent { api, link: Link::default(), forwarded: vec![], told: vec![] }
    }

    /// The link's rules on `input`, and everything they ask, until nothing
    /// more is asked.
    async fn apply(&mut self, input: Input<'_>) {
        let actions = self.link.apply(input);
        self.run(actions).await;
    }

    async fn run(&mut self, actions: Vec<Action>) {
        let mut todo: VecDeque<Action> = actions.into();
        // bounded: each answer asks at most a few commands, and the lab's are few
        let mut asked = 0;
        while let Some(a) = todo.pop_front() {
            match a {
                Action::Ask(cmd) => {
                    asked += 1;
                    assert!(asked < 64, "the link asks for a bounded number of commands");
                    let answer = self.api.cmd(&cmd.line()).await;
                    let frame = wire::parse(&json!({ "corrId": "1", "resp": answer }).to_string()).expect("the core's answer reads");
                    let next = match &frame.resp {
                        Resp::Error { what } => self.link.apply(Input::Failed { cmd: &cmd, what }),
                        resp => self.link.apply(Input::Said(resp)),
                    };
                    todo.extend(next);
                }
                Action::Forward(i) => self.forwarded.push(i),
                Action::Tell(n) => self.told.push(n),
                Action::Save => {}
            }
        }
    }

    /// The core's events, through the link, until `done` holds (within `within`).
    async fn until(&mut self, within: Duration, done: impl Fn(&Agent) -> bool) {
        let t0 = Instant::now();
        // bounded by `within`
        while !done(self) {
            assert!(t0.elapsed() < within, "not within {within:?}: link {:?}, told {:?}, forwarded {:?}", self.link, self.told, self.forwarded.iter().map(|i| &i.text).collect::<Vec<_>>());
            if self.api.events.is_empty() {
                self.api.drain(Duration::from_millis(200)).await;
            }
            while let Some(resp) = self.api.events.pop_front() {
                let frame = wire::parse(&json!({ "resp": resp }).to_string()).unwrap_or_else(|e| panic!("an event of the core's reads: {e}: {resp}"));
                let actions = self.link.apply(Input::Said(&frame.resp));
                self.run(actions).await;
            }
        }
    }

    fn texts(&self) -> Vec<&str> {
        self.forwarded.iter().map(|i| i.text.as_str()).collect()
    }
}

/// The person's app: what it received, in order, from the agent.
async fn received(person: &mut Api, within: Duration, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let t0 = Instant::now();
    let mut got: Vec<String> = vec![];
    // bounded by `within`
    loop {
        while let Some(e) = person.events.pop_front() {
            if tag(&e) == "newChatItems" {
                for ci in e["chatItems"].as_array().into_iter().flatten() {
                    let item = &ci["chatItem"];
                    if item["chatDir"]["type"] == "directRcv" && item["content"]["type"] == "rcvMsgContent" {
                        got.push(item["content"]["msgContent"]["text"].as_str().unwrap_or("").to_string());
                    }
                }
            }
        }
        if done(&got) {
            return got;
        }
        assert!(t0.elapsed() < within, "the person received only {:?} within {within:?}", got.iter().map(|t| wire::cut(t, 40)).collect::<Vec<_>>());
        person.drain(Duration::from_millis(200)).await;
    }
}

/// The person writes to the agent; the id their app gave the message.
async fn say(person: &mut Api, agent: i64, text: &str) -> String {
    let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
    let r = person.cmd(&format!("/_send @{agent} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "the person's send: {r}");
    r["chatItems"][0]["chatItem"]["meta"]["itemSharedMsgId"].as_str().expect("their message's id").to_string()
}

/// Goal: the link against the real core (simplex-chat v7.0.3): linked by
/// its one-time invitation to the person who used it and no one else;
/// their messages forwarded by their own ids, once however often read; a
/// long reply in parts that arrive whole; a restart that loses nothing; a
/// rollback of the agent's database healed by the forced sync its start
/// asks, the reply held through it delivered after; a core whose process
/// died (its disk intact) caught up from its own chat; and the person's own
/// delete letting the link go.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_link_against_the_real_core() {
    let mut lab = Lab::up("link");
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    let mut stranger = lab.start("stranger", false).await;

    // linked by the invitation, to whoever used it
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation: {:?}", agent.link) };
    let r = person.cmd(&format!("/connect {link}")).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    agent.until(Duration::from_secs(90), |a| a.link.contact().is_some()).await;
    assert_eq!(agent.told, vec![Notice::Linked { name: "person".into() }]);
    let at_person = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    let t0 = Instant::now();

    // the spent invitation is refused to anyone after
    let r = stranger.cmd(&format!("/connect {link}")).await;
    assert!(tag(&r) == "chatCmdError" && r.to_string().contains("AUTH"), "the spent link: {r}");
    // a contact the link did not make is let go, and nothing of theirs forwarded
    let other = agent.api.cmd("/connect").await;
    let other_link = other["connLinkInvitation"]["connFullLink"].as_str().expect("a link").to_string();
    stranger.cmd(&format!("/connect {other_link}")).await;
    agent.until(Duration::from_secs(90), |a| a.told.contains(&Notice::Refused)).await;
    assert!(agent.forwarded.is_empty());
    println!("linked, and a stranger refused: {} ms", t0.elapsed().as_millis());

    // both ways
    let id = say(&mut person, at_person, "hello agent").await;
    agent.until(Duration::from_secs(30), |a| !a.forwarded.is_empty()).await;
    assert_eq!(agent.forwarded[0].id, id, "forwarded by the id its sender's app gave it");
    assert_eq!(agent.forwarded[0].text, "hello agent");
    let long: String = (0..3_000).map(|i| format!("line {i}\n")).collect();
    assert!(long.len() > 2 * wire::SEND_JSON_MAX_BYTES);
    agent.apply(Input::Reply(&long)).await;
    let parts = received(&mut person, Duration::from_secs(30), |got| got.concat() == long).await;
    assert!(parts.len() >= 3, "{} parts", parts.len());
    assert!(agent.told.iter().all(|n| !matches!(n, Notice::Failed { .. })), "{:?}", agent.told);

    // a restart (SIGTERM, as a sleep's stop after its save): nothing lost, nothing twice
    let saved = agent.link.clone();
    drop(agent);
    lab.stop("agent");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved.to_json()).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    assert_eq!(agent.forwarded.iter().filter(|i| i.id == id).count(), 1, "the catch-up forwards it again, by the same id");
    say(&mut person, at_person, "after the restart").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"after the restart")).await;
    agent.apply(Input::Reply("answered after the restart")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the restart")).await;

    // a rollback: the save before an exchange both ways, restored
    drop(agent);
    lab.stop("agent");
    lab.snapshot("agent", "save");
    let saved = saved.to_json();
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    say(&mut person, at_person, "said after the save").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"said after the save")).await;
    agent.apply(Input::Reply("answered after the save")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the save")).await;
    drop(agent);
    lab.stop("agent");
    lab.restore("agent", "save");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the save's link");
    let t0 = Instant::now();
    agent.apply(Input::Started { rollback: true }).await;
    assert_eq!(agent.told, vec![Notice::MaybeLost]);
    agent.apply(Input::Reply("held through the sync")).await;
    assert_eq!(agent.link.outbox.len(), 1, "held while out of step");
    agent.until(Duration::from_secs(60), |a| a.told.contains(&Notice::Resynced)).await;
    println!("a rollback's forced sync: in step {} ms after the start", t0.elapsed().as_millis());
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "held through the sync")).await;
    say(&mut person, at_person, "after the rollback").await;
    agent.until(Duration::from_secs(30), |a| a.texts().contains(&"after the rollback")).await;
    agent.apply(Input::Reply("answered after the rollback")).await;
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t == "answered after the rollback")).await;

    // the core's process dies mid-burst, its disk intact (the bridge starts it again)
    let ids: Vec<String> = {
        let mut ids = vec![];
        for i in 0..5 {
            ids.push(say(&mut person, at_person, &format!("burst {i}")).await);
        }
        ids
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let saved = agent.link.to_json();
    drop(agent);
    docker(&["kill", "-s", "KILL", &lab.container("agent")]).expect("killed");
    docker(&["rm", "-f", &lab.container("agent")]).expect("gone");
    let mut agent = Agent::new(lab.start("agent", true).await);
    agent.link = Link::from_json(&saved).expect("the link reads back");
    agent.apply(Input::Started { rollback: false }).await;
    agent.until(Duration::from_secs(30), |a| ids.iter().all(|id| a.forwarded.iter().any(|i| &i.id == id))).await;

    // the person deletes the agent in their app: the link lets go
    let r = person.cmd(&format!("/_delete @{at_person} full notify=on")).await;
    assert_eq!(tag(&r), "contactDeleted", "{r}");
    agent.until(Duration::from_secs(30), |a| a.link.phase == Phase::Off).await;
    assert!(agent.told.contains(&Notice::Unlinked { by_them: true }), "{:?}", agent.told);

    // and an invitation withdrawn before anyone used it
    agent.apply(Input::Want).await;
    assert!(matches!(agent.link.phase, Phase::Inviting { .. }));
    agent.apply(Input::Unlink).await;
    assert_eq!(agent.link.phase, Phase::Off);
    println!("frames in {}", lab.dir.display());
}
