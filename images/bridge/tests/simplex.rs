//! The SimpleX lab: a local SMP server (and, for files, an XFTP server) and
//! simplex-chat v7.0.3 clients in Docker (`tests/simplex/lab.Dockerfile`),
//! each driven over its bot API,
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

mod support;

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
/// The lab's computer: the bridge with SimpleX's core
/// (`tests/simplex/bridge.Dockerfile`).
const BRIDGE_IMAGE: &str = "fragment-simplex-bridge:test";
/// The SMP server's name on the lab's network: a domain, as its image's
/// entrypoint wants one with a dot.
const SMP_HOST: &str = "smp.lab.test";
/// The XFTP server (files past what goes inline), pinned the same way.
const XFTP_IMAGE: &str = "simplexchat/xftp-server:v7.0.1@sha256:69a7f95746bb3c094ac36154ee8e7a8f552b5d9231587b88e6699d089003d26b";
const XFTP_HOST: &str = "xftp.lab.test";

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
    /// `xftp://…`, for a lab that carries files.
    xftp: Option<String>,
    clients: Vec<String>,
}

impl Lab {
    fn up(name: &str) -> Lab {
        Lab::up_with(name, false)
    }

    /// `files`: with an XFTP server too, every client given it, a files
    /// folder (`/data/files`), and a bot that allows files.
    fn up_with(name: &str, files: bool) -> Lab {
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
        let xftp = files.then(|| {
            let ctr = format!("{prefix}-xftp");
            docker(&["run", "-d", "--name", &ctr, "--network", &net, "--network-alias", XFTP_HOST, "-e", &format!("ADDR={XFTP_HOST}"), "-e", "QUOTA=1gb", "-e", "PASS=", "--tmpfs", "/srv/xftp:rw,size=256m", XFTP_IMAGE]).expect("the XFTP server starts");
            let t0 = Instant::now();
            loop {
                if let Ok(fp) = docker(&["exec", &ctr, "cat", "/etc/opt/simplex-xftp/fingerprint"]) {
                    if !fp.is_empty() && logs(&ctr).contains("Listening on port") {
                        break format!("xftp://{fp}@{XFTP_HOST}");
                    }
                }
                assert!(t0.elapsed() < Duration::from_secs(90), "the XFTP server is not serving after 90 s: {}", logs(&ctr));
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Lab { prefix, dir, server: format!("smp://{fingerprint}@{SMP_HOST}"), xftp, clients: vec![] }
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
        let (net, ids) = (format!("{}-net", self.prefix), uid_gid());
        let mut args = vec![
            "run", "-d", "--name", &ctr, "--network", &net, "--user", &ids, "-e", "HOME=/data", "-e", "LAB_API_PORT=5226", "-p", "127.0.0.1::5226", "-v", &vol,
            LAB_IMAGE, "-d", "/data/sx", "-s", &self.server, "-p", "5225", who, name, "-y",
        ];
        if let Some(xftp) = &self.xftp {
            std::fs::create_dir_all(data.join("files")).expect("its files folder");
            std::fs::create_dir_all(data.join("tmp")).expect("its temp folder");
            // its temp folder on the files' device: a received file is renamed
            // from one into the other ("Invalid cross-device link" otherwise,
            // as /tmp and /data are on a computer too)
            args.extend(["--xftp-server", xftp.as_str(), "--files-folder", "/data/files", "--temp-folder", "/data/tmp"]);
            if bot {
                args.push("--create-bot-allow-files");
            }
        }
        docker(&args).expect("the client starts");
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
        let _ = docker(&["rm", "-f", &self.container("xftp")]);
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
    /// Every event the core said, as it said it.
    seen: Vec<Value>,
}

impl Agent {
    fn new(api: Api) -> Agent {
        Agent { api, link: Link::default(), forwarded: vec![], told: vec![], seen: vec![] }
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
                self.seen.push(resp.clone());
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

/// A voice message as SimpleX's apps record one (both record MPEG-4 AAC,
/// 16 kHz, mono, 32 kbps: `AudioRecPlay.swift`, `RecAndPlay.android.kt`),
/// `secs` long, made by the host's ffmpeg into `to`.
fn voice_file(to: &Path, secs: u32, noise: bool) {
    let src = if noise { format!("anoisesrc=d={secs}:c=pink:a=0.3") } else { format!("sine=frequency=440:duration={secs}") };
    let out = Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", &src, "-ac", "1", "-ar", "16000", "-c:a", "aac", "-b:a", "32k"]).arg(to).output().expect("the host has ffmpeg");
    assert!(out.status.success(), "ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
}

fn sha256_of(path: &Path) -> String {
    let out = Command::new("sha256sum").arg(path).output().expect("sha256sum runs");
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string()
}

/// The voice message `text`'s sender sent with file `name`: its file's id,
/// protocol and status as the receiving core said them (`seen`, newest last).
fn voice_item<'a>(seen: impl Iterator<Item = &'a Value>, name: &str) -> Option<(i64, String, String)> {
    seen.filter(|e| tag(e) == "newChatItems").flat_map(|e| e["chatItems"].as_array().cloned().unwrap_or_default()).find_map(|ci| {
        let item = &ci["chatItem"];
        let file = &item["file"];
        (item["content"]["msgContent"]["type"] == "voice" && file["fileName"] == name).then(|| (file["fileId"].as_i64().expect("its file id"), file["fileProtocol"].as_str().unwrap_or("?").to_string(), file["fileStatus"]["type"].as_str().unwrap_or("?").to_string()))
    })
}

/// Receives file `id` at a core whose events go to `events`: `/freceive`,
/// then its `rcvFileComplete`, within `within`; how long it took.
async fn freceive(api: &mut Api, id: i64, within: Duration) -> Duration {
    let t0 = Instant::now();
    let r = api.cmd(&format!("/freceive {id}")).await;
    assert!(!tag(&r).contains("rror"), "/freceive {id}: {r}");
    api.wait(within, |e| tag(e) == "rcvFileComplete" && e["chatItem"]["chatItem"]["file"]["fileId"] == id).await;
    t0.elapsed()
}

/// Goal: voice both ways against the real core, through the link: a
/// person's voice message reaches the agent as the link's `voice` forward
/// and its file arrives whole, inline (under 255 KB) and over XFTP (past
/// it); the agent's voice reply reaches the person whole. Needs the host's
/// ffmpeg.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and ffmpeg; run by hand"]
async fn voice_against_the_real_core() {
    let mut lab = Lab::up_with("voice", true);
    let mut agent = Agent::new(lab.start("agent", true).await);
    let mut person = lab.start("person", false).await;
    agent.apply(Input::Want).await;
    let Phase::Inviting { link, .. } = agent.link.phase.clone() else { panic!("an invitation") };
    person.cmd(&format!("/connect {link}")).await;
    agent.until(Duration::from_secs(90), |a| a.link.contact().is_some()).await;
    let at_agent = agent.link.contact().expect("linked");
    let at_person = person.wait(Duration::from_secs(90), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");

    for (name, secs, noise) in [("short.m4a", 20, false), ("long.m4a", 150, true)] {
        let file = lab.db_dir("person").join("files").join(name);
        voice_file(&file, secs, noise);
        let bytes = std::fs::metadata(&file).expect("made").len();
        let body = json!([{ "filePath": format!("/data/files/{name}"), "msgContent": { "type": "voice", "text": "", "duration": secs } }]);
        let r = person.cmd(&format!("/_send @{at_person} json {body}")).await;
        assert_eq!(tag(&r), "newChatItems", "the person's voice message: {r}");
        let t0 = Instant::now();
        agent.until(Duration::from_secs(60), |a| a.forwarded.iter().any(|i| i.kind == "voice") && voice_item(a.seen.iter(), name).is_some()).await;
        let voice = agent.forwarded.iter().rev().find(|i| i.kind == "voice").expect("forwarded").clone();
        let (id, protocol, status) = voice_item(agent.seen.iter(), name).expect("its file");
        println!("{name}: {bytes} bytes, {secs} s; the item in {} ms, its file {protocol} {status}, forwarded as {:?} {:?}", t0.elapsed().as_millis(), voice.kind, voice.text);
        agent.forwarded.retain(|i| i.kind != "voice");
        person.drain(Duration::from_secs(5)).await;
        let theirs: Vec<String> = person.events.drain(..).map(|e| tag(&e)).filter(|t| t.contains("ile") || t.contains("rror")).collect();
        println!("  the sender's file events: {theirs:?}");
        // an XFTP file is received once its description is ready
        if protocol == "xftp" {
            let ready = |e: &Value| tag(e) == "rcvFileDescrReady" && e["chatItem"]["chatItem"]["file"]["fileId"] == id;
            if !agent.seen.iter().any(ready) {
                agent.api.wait(Duration::from_secs(60), ready).await;
            }
        }
        let took = freceive(&mut agent.api, id, Duration::from_secs(120)).await;
        let got = lab.db_dir("agent").join("files").join(name);
        assert_eq!(sha256_of(&got), sha256_of(&file), "{name} arrives whole");
        println!("  received whole in {} ms", took.as_millis());
    }

    // the agent's voice reply (a speech route's audio, made AAC as the apps make it)
    let reply = lab.db_dir("agent").join("files").join("reply.m4a");
    voice_file(&reply, 30, false);
    let body = json!([{ "filePath": "/data/files/reply.m4a", "msgContent": { "type": "voice", "text": "", "duration": 30 } }]);
    let r = agent.api.cmd(&format!("/_send @{at_agent} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "the agent's voice reply: {r}");
    let item = person.wait(Duration::from_secs(60), |e| tag(e) == "newChatItems" && voice_item(std::iter::once(e), "reply.m4a").is_some()).await;
    let (id, protocol, _) = voice_item(std::iter::once(&item), "reply.m4a").expect("its file");
    let took = freceive(&mut person, id, Duration::from_secs(120)).await;
    assert_eq!(sha256_of(&lab.db_dir("person").join("files").join("reply.m4a")), sha256_of(&reply), "the reply arrives whole");
    println!("the agent's voice reply ({protocol}) received whole in {} ms", took.as_millis());
}

// ---- the bridge's SimpleX, as a computer runs it ----

/// The lab's computer, a container of `BRIDGE_IMAGE` on the lab's network:
/// its fragment API the fake on this host, its `/data` the directory
/// `data` here, its screen port on this host's loopback.
struct Computer {
    name: String,
    port: u16,
}

impl Computer {
    fn start(lab: &Lab, fake: &support::fake::Fake, data: &Path) -> Computer {
        let name = lab.container("computer");
        let _ = docker(&["rm", "-f", &name]);
        std::fs::create_dir_all(data).expect("its /data");
        let (net, ids, vol) = (format!("{}-net", lab.prefix), uid_gid(), format!("{}:/data", data.display()));
        let api = format!("FRAGMENT_API=http://api.fragment.internal:{}", fake.addr.port());
        let smp = format!("BRIDGE_SIMPLEX_SMP={}", lab.server);
        let xftp = format!("BRIDGE_SIMPLEX_XFTP={}", lab.xftp.as_deref().expect("a lab with files"));
        docker(&[
            "run", "-d", "--name", &name, "--network", &net, "--add-host", "api.fragment.internal:host-gateway", "--user", &ids, "-e", "HOME=/data", "-e", &api, "-e", &smp, "-e", &xftp, "-e", "FRAGMENT_COMPUTER=computer:00aa", "-v", &vol, "-p", "127.0.0.1::6080", BRIDGE_IMAGE,
        ])
        .expect("the computer starts");
        let port = docker(&["port", &name, "6080/tcp"]).expect("its port").lines().next().and_then(|l| l.rsplit_once(':')).and_then(|(_, p)| p.parse().ok()).expect("a port");
        Computer { name, port }
    }

    /// `method` on its screen port: the status and the JSON answered.
    fn http(&self, method: &str, path: &str) -> (u16, Value) {
        let out = Command::new("curl").args(["-s", "-X", method, "-w", "\n%{http_code}", &format!("http://127.0.0.1:{}{path}", self.port)]).output().expect("curl runs");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", "0"));
        (code.trim().parse().unwrap_or(0), serde_json::from_str(body).unwrap_or(Value::Null))
    }

    /// Its SimpleX state, once `done` holds (within `within`).
    async fn state_until(&self, within: Duration, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        // bounded by `within`
        loop {
            let (code, v) = self.http("GET", "/simplex/state?agent=juniper--k3x9");
            if code == 200 && done(&v) {
                return v;
            }
            assert!(t0.elapsed() < within, "{what}: not within {within:?}; last {code} {v}\n{}", wire::cut(&logs(&self.name), 6_000));
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    fn exec(&self, cmd: &str) -> String {
        docker(&["exec", &self.name, "sh", "-c", cmd]).unwrap_or_default()
    }

    /// How many SimpleX cores run in it now.
    fn cores(&self) -> usize {
        self.exec("for c in /proc/[0-9]*/comm; do cat $c 2>/dev/null; done").lines().filter(|l| l.trim() == "simplex-chat").count()
    }

    fn events(&self, kind: &str) -> Vec<Value> {
        logs(&self.name).lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["event"] == kind).collect()
    }

    /// SIGTERM, as a sleep's stop sends it; gone.
    fn stop(&self) {
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{}.log", self.name)), logs(&self.name));
        docker(&["stop", "-t", "10", &self.name]).expect("it stops");
        docker(&["rm", "-f", &self.name]).expect("it goes");
    }
}

impl Drop for Computer {
    fn drop(&mut self) {
        let _ = std::fs::write(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{}.log", self.name)), logs(&self.name));
        let _ = docker(&["rm", "-f", &self.name]);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).expect("a directory");
    for e in std::fs::read_dir(from).expect("a directory").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().expect("its type").is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copied");
        }
    }
}

fn build_bridge_image() {
    if std::env::var("FRAGMENT_DOCKER_SKIP_BUILD").is_ok() {
        return;
    }
    let images = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("images/").to_path_buf();
    let status = Command::new("docker").args(["build", "--platform", "linux/amd64", "-q", "-f", "bridge/tests/simplex/bridge.Dockerfile", "-t", BRIDGE_IMAGE, "."]).current_dir(images).status().expect("docker runs");
    assert!(status.success(), "the lab's computer builds");
}

/// Goal: the bridge runs a linked agent's core as the platform's lifecycle
/// asks, against the real core: linked from its routes (the owner's link
/// asked, the invitation shown, the person's app bound, greeted, a save
/// asked); stopped under a hold before the bridge answers it, its databases
/// whole, and started again after; stopped while the computer is not always
/// on, and started again when it is; through a planned restart, as it was;
/// and after a rollback of its `/data`, its forced sync heals the link, the
/// owner is told a message may be lost, and both directions work.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker; run by hand"]
async fn the_bridge_runs_a_linked_core() {
    build_bridge_image();
    let mut lab = Lab::up_with("bridge", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let data = lab.dir.join("computer-data");
    let c = Computer::start(&lab, &fake, &data);
    let v = c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    assert_eq!(v["alwaysOn"], true);
    assert_eq!(c.cores(), 0, "nothing linked, no core");

    // linked from its routes
    let (code, _) = c.http("POST", "/simplex/link?agent=juniper--k3x9");
    assert_eq!(code, 200);
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    let link = v["link"].as_str().expect("its link").to_string();
    assert!(link.starts_with("simplex:/invitation#"));
    let r = person.cmd(&format!("/connect {link}")).await;
    assert_eq!(tag(&r), "sentConfirmation", "{r}");
    let v = c.state_until(Duration::from_secs(90), "linked", |v| v["phase"] == "linked").await;
    assert_eq!(v["name"], "person");
    let at_person = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    let greeted = received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
    assert!(greeted.iter().any(|t| t.starts_with("Hi, it's Juniper.")), "named by its title: {greeted:?}");
    fake.until(20_000, "a save asked as the link was made", |w| w.keepalive_log.windows(2).any(|p| p == [true, false])).await;
    assert_eq!(c.cores(), 1);

    // the hold: stopped before it is answered, whole, started again after
    c.exec("touch /run/computer/hold");
    let t0 = Instant::now();
    // bounded: 20 s, the platform's wait for an answer
    while c.exec("test -e /run/computer/held && echo yes").trim() != "yes" {
        assert!(t0.elapsed() < Duration::from_secs(20), "the hold answered within 20 s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("the hold answered in {} ms, its core stopped first", t0.elapsed().as_millis());
    assert_eq!(c.cores(), 0, "no core runs under the hold");
    let files: Vec<String> = c.exec("ls /data/simplex/juniper--k3x9").lines().map(str::to_string).collect();
    assert!(files.contains(&"sx_chat.db".into()) && files.contains(&"sx_agent.db".into()) && files.contains(&"link.json".into()), "{files:?}");
    assert!(!files.iter().any(|f| f.ends_with("-wal")), "whole: {files:?}");
    c.exec("rm -f /run/computer/hold /run/computer/held");
    c.state_until(Duration::from_secs(30), "running again", |v| v["running"] == true).await;
    say(&mut person, at_person, "after the hold").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while !c.events("simplex.forward").iter().any(|e| e["kind"] == "text") {
        assert!(t0.elapsed() < Duration::from_secs(30), "the message after the hold reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // not always on: no core; always on again: its core again
    fake.with(|w| w.computer["alwaysOn"] = json!(false));
    let v = c.state_until(Duration::from_secs(75), "stopped, not always on", |v| v["running"] == false && v["alwaysOn"] == false).await;
    assert!(v["why"].as_str().is_some_and(|w| w.contains("always-on")), "{v}");
    assert_eq!(c.cores(), 0);
    fake.with(|w| w.computer["alwaysOn"] = json!(true));
    c.state_until(Duration::from_secs(75), "running, always on again", |v| v["running"] == true).await;

    // a planned restart (SIGTERM, as a sleep's stop), then its save: kept as it was
    c.stop();
    let save = lab.dir.join("computer-save");
    copy_tree(&data, &save);
    fake.with(|w| {
        w.computer["generation"] = json!(2);
        w.computer["restored"] = json!({ "generation": 2, "rollback": false, "from": "backup", "at": 1 });
    });
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "linked and running after a restart", |v| v["phase"] == "linked" && v["running"] == true).await;
    let before = c.events("simplex.forward").len();
    say(&mut person, at_person, "said after the save").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while c.events("simplex.forward").len() <= before {
        assert!(t0.elapsed() < Duration::from_secs(30), "a message after the restart reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(c.events("simplex.core_up").iter().all(|e| e["rollback"] == false), "a restart that saved forces nothing");

    // a rollback: the save before that message, restored, as a crash's next start
    c.stop();
    copy_tree(&save, &data);
    fake.with(|w| {
        w.computer["generation"] = json!(3);
        w.computer["restored"] = json!({ "generation": 3, "rollback": true, "from": "backup", "at": 1 });
    });
    let t0 = Instant::now();
    let c = Computer::start(&lab, &fake, &data);
    let told = received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t.contains("may have missed a message"))).await;
    println!("after a rollback, the owner told {} ms after the start: {told:?}", t0.elapsed().as_millis());
    assert!(c.events("simplex.core_up").iter().any(|e| e["rollback"] == true), "its start forced a sync");
    let before = c.events("simplex.forward").len();
    say(&mut person, at_person, "after the rollback").await;
    let t0 = Instant::now();
    // bounded by 30 s
    while c.events("simplex.forward").len() <= before {
        assert!(t0.elapsed() < Duration::from_secs(30), "a message after the rollback reached the bridge");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // a bridge started again in the same start forces nothing twice
    c.stop();
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "running again", |v| v["running"] == true).await;
    assert!(c.events("simplex.core_up").iter().all(|e| e["rollback"] == false), "the same start's rollback is handled once");

    // unlinked from its routes: its contact let go, at the person's too
    let (code, v) = c.http("POST", "/simplex/unlink?agent=juniper--k3x9");
    assert_eq!((code, v["phase"].as_str()), (200, Some("off")));
    person.wait(Duration::from_secs(30), |e| tag(e) == "contactDeletedByContact").await;
    c.state_until(Duration::from_secs(30), "its core stopped", |v| v["running"] == false).await;
    // an agent this bridge does not run has no link here
    let (code, _) = c.http("GET", "/simplex/state?agent=nobody--k3x9");
    assert_eq!(code, 404);
}

/// The relayed records on `chat` (an agent's, carrying its owner's words).
fn relayed_on(fake: &support::fake::Fake, chat: &str) -> Vec<Value> {
    fake.with(|w| w.records(chat, "chat").into_iter().filter(|r| r["body"]["relayed"]["via"] == "simplex").collect())
}

/// Goal: messages both ways through the agent's own chat with its owner,
/// against the real core: the owner's SimpleX message is a relayed record
/// on its Bot Chat, by the agent, for its owner, by its sender's id, and a
/// turn asked as its owner (the scripted runtime's echo names them); that
/// turn's reply reaches their app; a reply to a message typed in the chat
/// does not; a voice message is the record's attachment, whole, as an
/// `audio/mp4`; and a start again (its catch-up) posts nothing twice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker and ffmpeg; run by hand"]
async fn messages_go_both_ways_through_the_chat() {
    build_bridge_image();
    let mut lab = Lab::up_with("messages", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    let chat = fake.chat_named("juniper-chat--h6j7", &["juniper"]);
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let data = lab.dir.join("computer-data");
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    c.http("POST", "/simplex/link?agent=juniper--k3x9");
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    person.cmd(&format!("/connect {}", v["link"].as_str().expect("its link"))).await;
    c.state_until(Duration::from_secs(90), "linked", |v| v["phase"] == "linked").await;
    let at_person = person.wait(Duration::from_secs(60), |e| tag(e) == "contactConnected").await["contact"]["contactId"].as_i64().expect("the agent, at the person's");
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;

    // the owner's message: a relayed record, a turn as theirs, its reply back
    let t0 = Instant::now();
    let sent_id = say(&mut person, at_person, "hello from my phone").await;
    let got = received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] hello from my phone")).await;
    println!("SimpleX message to the agent's reply on SimpleX: {} ms ({got:?})", t0.elapsed().as_millis());
    let r = relayed_on(&fake, &chat);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["principal"], "npub1juniper", "posted by the agent");
    assert_eq!(r[0]["body"]["relayed"]["for"], "npub1paul", "for its owner");
    assert_eq!(r[0]["body"]["text"], "hello from my phone");
    assert_eq!(r[0]["body"]["to"], json!(["npub1juniper"]));
    let id = fragment_bridge::records::relayed_id("juniper--k3x9", &sent_id);
    assert_eq!(fake.with(|w| w.fragments[&chat].channels["chat"].ids.get(&id).map(|(seq, _)| *seq)), r[0]["seq"].as_u64(), "by its sender's id");

    // a message typed in the chat: answered there, not over SimpleX
    fake.say(&chat, "npub1paul", json!({ "text": "typed on the web" }));
    fake.until(30_000, "the chat's reply", |w| w.bodies(&chat, "chat", "reply").iter().any(|b| b["text"] == "echo: [paul] typed on the web")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    person.drain(Duration::from_millis(500)).await;
    let late: Vec<String> = person
        .events
        .iter()
        .filter(|e| tag(e) == "newChatItems")
        .flat_map(|e| e["chatItems"].as_array().cloned().unwrap_or_default())
        .filter_map(|ci| ci["chatItem"]["content"]["msgContent"]["text"].as_str().map(str::to_string))
        .collect();
    assert!(!late.iter().any(|t| t.contains("typed on the web")), "a reply to the chat stays in the chat: {late:?}");

    // a voice message: its file, whole, the record's attachment
    let voice = lab.db_dir("person").join("files").join("memo.m4a");
    voice_file(&voice, 20, false);
    let body = json!([{ "filePath": "/data/files/memo.m4a", "msgContent": { "type": "voice", "text": "", "duration": 20 } }]);
    let r = person.cmd(&format!("/_send @{at_person} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "{r}");
    let t0 = Instant::now();
    fake.until(60_000, "the voice message's record", |w| w.records(&chat, "chat").iter().any(|r| r["body"]["relayed"]["via"] == "simplex" && r["body"]["attachments"].is_array())).await;
    println!("a voice message on the agent's chat: {} ms after it was sent", t0.elapsed().as_millis());
    let rec = relayed_on(&fake, &chat).into_iter().find(|r| r["body"]["attachments"].is_array()).expect("its record");
    let a = &rec["body"]["attachments"][0];
    assert_eq!(a["type"], "audio/mp4");
    assert_eq!(a["sha256"].as_str(), Some(sha256_of(&voice).as_str()), "whole");
    assert_eq!(a["size"].as_u64(), std::fs::metadata(&voice).ok().map(|m| m.len()));
    assert!(fake.with(|w| w.fragments[&chat].blobs.contains_key(a["sha256"].as_str().unwrap())), "its blob uploaded");
    let got = received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t.starts_with("echo: [paul]") && t.contains("memo.m4a"))).await;
    println!("the voice message answered on SimpleX: {got:?} (the runtime was handed its file)");
    assert!(c.exec("ls /data/simplex/juniper--k3x9/files").trim().is_empty(), "its file let go once carried");

    // a start again (SIGTERM, a save, a start): its catch-up posts nothing twice
    let before = relayed_on(&fake, &chat).len();
    c.stop();
    fake.with(|w| w.computer["generation"] = json!(2));
    let c = Computer::start(&lab, &fake, &data);
    c.state_until(Duration::from_secs(60), "running again", |v| v["running"] == true).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(relayed_on(&fake, &chat).len(), before, "the catch-up's forwards are replays");
    say(&mut person, at_person, "after the restart").await;
    received(&mut person, Duration::from_secs(60), |got| got.iter().any(|t| t == "echo: [paul] after the restart")).await;
}

/// What a QR code `svg` says, as the host's zbar reads it from its picture.
fn decode_qr(svg: &[u8], dir: &Path) -> String {
    let (svg_path, png) = (dir.join("qr.svg"), dir.join("qr.png"));
    std::fs::write(&svg_path, svg).expect("written");
    let made = Command::new("rsvg-convert").arg("-w").arg("480").arg("-o").arg(&png).arg(&svg_path).status().expect("the host has rsvg-convert");
    assert!(made.success(), "drawn as a picture");
    let out = Command::new("zbarimg").args(["-q", "--raw"]).arg(&png).output().expect("the host has zbarimg");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Goal: the link page, as the shell's pane shows it: served under its
/// port's path; its QR code says the one-time invitation (its short link,
/// the server's), and the person's app links by scanning it; then the page
/// says whom it is linked to.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Docker, rsvg-convert and zbarimg; run by hand"]
async fn the_link_pages_code_links() {
    build_bridge_image();
    let mut lab = Lab::up_with("page", true);
    let mut person = lab.start("person", false).await;
    let fake = support::fake::Fake::start("0.0.0.0:0", &["juniper"]).await;
    fake.with(|w| {
        w.computer["alwaysOn"] = json!(true);
        w.computer["generation"] = json!(1);
    });
    let c = Computer::start(&lab, &fake, &lab.dir.join("computer-data"));
    c.state_until(Duration::from_secs(60), "its state", |v| v["phase"] == "off").await;
    let page = Command::new("curl").args(["-s", &format!("http://127.0.0.1:{}/simplex/?agent=juniper--k3x9", c.port)]).output().expect("curl runs");
    let page = String::from_utf8_lossy(&page.stdout);
    assert!(page.contains("<title>SimpleX</title>") && page.contains("Link SimpleX"), "its page");
    let (code, _) = c.http("GET", "/simplex/qr.svg?agent=juniper--k3x9");
    assert_eq!(code, 404, "no code before an invitation");
    c.http("POST", "/simplex/link?agent=juniper--k3x9");
    let v = c.state_until(Duration::from_secs(60), "an invitation", |v| v["phase"] == "inviting").await;
    let svg = Command::new("curl").args(["-s", &format!("http://127.0.0.1:{}/simplex/qr.svg?agent=juniper--k3x9", c.port)]).output().expect("curl runs").stdout;
    let said = decode_qr(&svg, &lab.dir);
    let short = v["short"].as_str().expect("the lab's server makes a short link");
    assert_eq!(said, short, "its code says its short link");
    let r = person.cmd(&format!("/connect {said}")).await;
    println!("the person scanned the page's code: {}", tag(&r));
    let v = c.state_until(Duration::from_secs(90), "linked by its code", |v| v["phase"] == "linked").await;
    assert_eq!(v["name"], "person");
    received(&mut person, Duration::from_secs(30), |got| got.iter().any(|t| t.contains("You're linked"))).await;
}
