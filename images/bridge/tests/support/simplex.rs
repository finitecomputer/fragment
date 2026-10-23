//! The SimpleX lab's harness, shared by tests/simplex.rs (by hand) and
//! tests/docker.rs (the real Hermes image): a local SMP server (and, for
//! files, an XFTP server) and simplex-chat v7.0.3 clients in Docker
//! (`tests/simplex/lab.Dockerfile`), each driven over its bot API.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The SMP server, pinned: v7.0.1 by its multi-arch index's digest (Docker
/// Hub), the version the spike ran (docs/explorations/simplex-connector.md).
pub const SMP_IMAGE: &str = "simplexchat/smp-server:v7.0.1@sha256:7d825822839a9d5ee9e9a99563d77a319816c7a8e0b4b7bf4dcebe448897af54";
pub const LAB_IMAGE: &str = "fragment-simplex-lab:7.0.3";
/// The SMP server's name on the lab's network: a domain, as its image's
/// entrypoint wants one with a dot.
pub const SMP_HOST: &str = "smp.lab.test";
/// The XFTP server (files past what goes inline), pinned the same way.
pub const XFTP_IMAGE: &str = "simplexchat/xftp-server:v7.0.1@sha256:69a7f95746bb3c094ac36154ee8e7a8f552b5d9231587b88e6699d089003d26b";
pub const XFTP_HOST: &str = "xftp.lab.test";

pub fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker").args(args).output().map_err(|e| format!("docker: {e}"))?;
    if !out.status.success() {
        return Err(format!("docker {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn uid_gid() -> String {
    let id = |flag: &str| String::from_utf8_lossy(&Command::new("id").arg(flag).output().expect("id runs").stdout).trim().to_string();
    format!("{}:{}", id("-u"), id("-g"))
}

/// One Docker network, one SMP server, and clients, all named
/// `<prefix>-…`; each client's database in `dir/<name>` on the host.
pub struct Lab {
    pub prefix: String,
    pub dir: PathBuf,
    pub server: String,
    /// `xftp://…`, for a lab that carries files.
    pub xftp: Option<String>,
    pub clients: Vec<String>,
}

impl Lab {
    pub fn up(name: &str) -> Lab {
        Lab::up_with(name, false)
    }

    /// `files`: with an XFTP server too, every client given it, a files
    /// folder (`/data/files`), and a bot that allows files.
    pub fn up_with(name: &str, files: bool) -> Lab {
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

    pub fn container(&self, name: &str) -> String {
        format!("{}-{name}", self.prefix)
    }

    pub fn db_dir(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Starts client `name` (its database kept across starts) and connects
    /// to its bot API. `bot`: `--create-bot-display-name`, else a person's
    /// `--user-display-name`.
    pub async fn start(&mut self, name: &str, bot: bool) -> Api {
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
    pub fn stop(&self, name: &str) {
        let ctr = self.container(name);
        docker(&["stop", "-t", "10", &ctr]).expect("it stops");
        docker(&["rm", "-f", &ctr]).expect("it goes");
    }

    /// A copy of `name`'s database files, to put back later: a disk that
    /// goes back in time.
    pub fn snapshot(&self, name: &str, to: &str) {
        let (from, to) = (self.db_dir(name), self.dir.join(to));
        let _ = std::fs::remove_dir_all(&to);
        std::fs::create_dir_all(&to).expect("the snapshot's directory");
        for f in db_files(&from) {
            std::fs::copy(&f, to.join(f.file_name().expect("a name"))).expect("copied");
        }
    }

    pub fn restore(&self, name: &str, from: &str) {
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

pub fn db_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("sx_"))).collect()).unwrap_or_default();
    v.sort();
    v
}

pub fn logs(container: &str) -> String {
    Command::new("docker").args(["logs", container]).output().map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))).unwrap_or_default()
}

/// A client's bot API: `{"corrId", "cmd"}` out; `{"corrId", "resp"}` its
/// answer, `{"resp"}` what it says on its own. Every frame read is also
/// appended to `<lab>/<name>.frames.jsonl`.
pub struct Api {
    pub ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub next: u64,
    pub events: VecDeque<Value>,
    pub log: PathBuf,
}

impl Api {
    pub async fn connect(url: &str, name: &str, dir: &Path) -> Result<Api, String> {
        let t0 = Instant::now();
        loop {
            match tokio_tungstenite::connect_async(url).await {
                Ok((ws, _)) => return Ok(Api { ws, next: 1, events: VecDeque::new(), log: dir.join(format!("{name}.frames.jsonl")) }),
                Err(e) if t0.elapsed() > Duration::from_secs(60) => return Err(e.to_string()),
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }

    pub fn keep(&self, dir: &str, v: &Value) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.log).expect("the frames' log");
        writeln!(f, "{}", json!({ "dir": dir, "frame": v })).expect("written");
    }

    pub async fn read(&mut self, within: Duration) -> Option<Value> {
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
    pub async fn cmd(&mut self, cmd: &str) -> Value {
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
    pub async fn wait(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Value {
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
    pub async fn drain(&mut self, d: Duration) {
        let t0 = Instant::now();
        while let Some(left) = d.checked_sub(t0.elapsed()) {
            if let Some(v) = self.read(left).await {
                self.events.push_back(v["resp"].clone());
            }
        }
    }
}

pub fn tag(v: &Value) -> String {
    v["type"].as_str().unwrap_or("?").to_string()
}

/// The person's app: what it received, in order, from the agent.
pub async fn received(person: &mut Api, within: Duration, done: impl Fn(&[String]) -> bool) -> Vec<String> {
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
        assert!(t0.elapsed() < within, "the person received only {:?} within {within:?}", got.iter().map(|t| fragment_bridge::simplex::wire::cut(t, 40)).collect::<Vec<_>>());
        person.drain(Duration::from_millis(200)).await;
    }
}

/// The person writes to the agent; the id their app gave the message.
pub async fn say(person: &mut Api, agent: i64, text: &str) -> String {
    let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
    let r = person.cmd(&format!("/_send @{agent} json {body}")).await;
    assert_eq!(tag(&r), "newChatItems", "the person's send: {r}");
    r["chatItems"][0]["chatItem"]["meta"]["itemSharedMsgId"].as_str().expect("their message's id").to_string()
}

/// A voice message as SimpleX's apps record one (both record MPEG-4 AAC,
/// 16 kHz, mono, 32 kbps: `AudioRecPlay.swift`, `RecAndPlay.android.kt`),
/// `secs` long, made by the host's ffmpeg into `to`.
pub fn voice_file(to: &Path, secs: u32, noise: bool) {
    let src = if noise { format!("anoisesrc=d={secs}:c=pink:a=0.3") } else { format!("sine=frequency=440:duration={secs}") };
    let out = Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", &src, "-ac", "1", "-ar", "16000", "-c:a", "aac", "-b:a", "32k"]).arg(to).output().expect("the host has ffmpeg");
    assert!(out.status.success(), "ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
}

pub fn sha256_of(path: &Path) -> String {
    let out = Command::new("sha256sum").arg(path).output().expect("sha256sum runs");
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string()
}
