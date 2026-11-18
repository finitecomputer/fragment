//! SPIKE (branch spike/simplex-door, never merged): the SimpleX door
//! spike's driver. Each step is a subcommand, its state in
//! `~/.cache/sxdoor/state.json` (0600). "The person" is a simplex-chat CLI
//! this machine runs in Docker (never a real person's account), driven over
//! its bot API on loopback.
//!
//!   sxdoor person-up <name> <port>           a person's client, in Docker
//!   sxdoor person-connect <port> <link>      it accepts an invitation
//!   sxdoor person-send <port> <text>         it says something
//!   sxdoor person-items <port> [count]       its chat with the door
//!   (the hosted steps: see `main`)

#[path = "../api.rs"]
#[allow(dead_code)]
mod api;

use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

fn cache() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME")).join(".cache/sxdoor")
}

const PERSON_IMAGE: &str = "fragment-sxdoor:spike";

/// A person's client: simplex-chat's bot API on 127.0.0.1:<port>.
struct Person {
    ws: WebSocket<TcpStream>,
    next: u64,
    pub events: Vec<Value>,
}

impl Person {
    fn open(port: u16) -> Result<Person> {
        let t0 = Instant::now();
        loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(tcp) => {
                    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
                    let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}"), tcp).map_err(|e| anyhow::anyhow!("{e}"))?;
                    return Ok(Person { ws, next: 1, events: vec![] });
                }
                Err(e) if t0.elapsed() < Duration::from_secs(30) => {
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => bail!("the person's client on {port}: {e}"),
            }
        }
    }

    fn ask(&mut self, cmd: &str) -> Result<Value> {
        let n = self.next;
        self.next += 1;
        self.ws.send(Message::Text(json!({ "corrId": n.to_string(), "cmd": cmd }).to_string().into()))?;
        loop {
            let m = self.ws.read()?;
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(&t)?;
            if v["corrId"] == n.to_string() {
                return Ok(v);
            }
            self.events.push(v);
        }
    }

    /// Events until `until` holds of one, or `wait` passes: whether it held.
    fn until(&mut self, wait: Duration, until: impl Fn(&Value) -> bool) -> Result<Option<Value>> {
        if let Some(v) = self.events.iter().find(|v| until(v)).cloned() {
            return Ok(Some(v));
        }
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            let left = wait.saturating_sub(t0.elapsed()).max(Duration::from_millis(10));
            set_timeout(&self.ws, left)?;
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    let v: Value = serde_json::from_str(&t)?;
                    let hit = until(&v);
                    self.events.push(v.clone());
                    if hit {
                        set_timeout(&self.ws, Duration::from_secs(60))?;
                        return Ok(Some(v));
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
                Err(e) => return Err(e.into()),
            }
        }
        set_timeout(&self.ws, Duration::from_secs(60))?;
        Ok(None)
    }
}

fn set_timeout(ws: &WebSocket<TcpStream>, d: Duration) -> Result<()> {
    ws.get_ref().set_read_timeout(Some(d))?;
    Ok(())
}

/// The texts of a chat item list (`/_get chat`'s), oldest first:
/// `(item id, received, text, shared msg id, ts)`.
fn items_of(v: &Value) -> Vec<(i64, bool, String, String, String)> {
    let list = v["resp"]["chat"]["chatItems"].as_array().cloned().unwrap_or_default();
    list.iter()
        .map(|ci| {
            let meta = &ci["meta"];
            let rcv = ci["chatDir"]["type"] == "directRcv";
            let text = ci["content"]["msgContent"]["text"].as_str().or_else(|| meta["itemText"].as_str()).unwrap_or("").to_string();
            (meta["itemId"].as_i64().unwrap_or(0), rcv, text, meta["itemSharedMsgId"].as_str().unwrap_or("").to_string(), meta["itemTs"].as_str().unwrap_or("").to_string())
        })
        .collect()
}

/// The person's one contact (the door), by its id.
fn person_contact(p: &mut Person) -> Result<i64> {
    let v = p.ask("/_contacts 1")?;
    let c = v["resp"]["contacts"].as_array().and_then(|l| l.last()).and_then(|c| c["contactId"].as_i64()).context(format!("a contact: {v}"))?;
    Ok(c)
}

fn person_up(name: &str, port: u16) -> Result<()> {
    let dir = cache().join(format!("person-{name}"));
    std::fs::create_dir_all(&dir)?;
    let status = std::process::Command::new("docker")
        .args(["run", "-d", "--name", &format!("sxdoor-person-{name}"), "--network", "host", "--entrypoint", "/opt/door/simplex-chat"])
        .args(["-v", &format!("{}:/data", dir.display()), PERSON_IMAGE])
        .args(["-d", "/data/sx", "-p", &port.to_string(), "-y", "--mute", "--create-bot-display-name", name, "--smp-web-port-servers", "off"])
        .status()?;
    anyhow::ensure!(status.success(), "docker run");
    let mut p = Person::open(port)?;
    println!("{}", p.ask("/u")?);
    Ok(())
}

fn person_cmd(args: &[String]) -> Result<()> {
    let port: u16 = args.get(1).context("a port")?.parse()?;
    let mut p = Person::open(port)?;
    match args[0].as_str() {
        "person-connect" => {
            let link = args.get(2).context("a link")?;
            println!("{}", p.ask(&format!("/connect {link}"))?);
            let ok = p.until(Duration::from_secs(60), |v| v["resp"]["type"] == "contactConnected")?;
            println!("connected: {}", ok.is_some());
        }
        "person-send" => {
            let c = person_contact(&mut p)?;
            let text = args.get(2).context("a text")?;
            let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
            let v = p.ask(&format!("/_send @{c} json {body}"))?;
            println!("{}", v["resp"]["type"]);
        }
        "person-items" => {
            let c = person_contact(&mut p)?;
            let n: u32 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(20);
            let v = p.ask(&format!("/_get chat @{c} count={n}"))?;
            for (id, rcv, text, shared, at) in items_of(&v) {
                println!("{id}\t{}\t{at}\t{shared}\t{text}", if rcv { "<-" } else { "->" });
            }
        }
        "person-cmd" => {
            let cmd = args.get(2).context("a command")?;
            println!("{}", serde_json::to_string_pretty(&p.ask(cmd)?)?);
        }
        other => bail!("no step {other}"),
    }
    Ok(())
}


// ---- the hosted steps, on the branch deployment ----

const ZONE: &str = "finite.build";
const BRANCH: &str = "sxdoor";

fn state_path() -> PathBuf {
    cache().join("state.json")
}

fn state() -> Value {
    std::fs::read(state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}))
}

fn save_state(v: &Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(state_path())?;
    f.write_all(serde_json::to_string_pretty(v)?.as_bytes())?;
    Ok(())
}

fn secret() -> Result<String> {
    let home = std::env::var("HOME")?;
    Ok(std::fs::read_to_string(format!("{home}/.config/fragment/secrets/preview-test-secret"))?.trim().to_string())
}

fn hosted() -> Result<api::Api> {
    let run = api::Run::signing_in_by_levers(secret()?, 0);
    Ok(api::Api::hosted(&api::Preview::new(ZONE, BRANCH), &run).patient(Duration::from_secs(120)))
}

fn owner(st: &Value) -> Result<fragment_nip98::Keys> {
    fragment_nip98::Keys::from_secret_hex(st["ownerSk"].as_str().context("no owner yet: setup")?).context("the owner's key")
}

fn s(st: &Value, k: &str) -> Result<String> {
    Ok(st[k].as_str().with_context(|| format!("no {k} in the state yet"))?.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// An operation, and for a job its run once it ends.
fn op(a: &api::Api, keys: &fragment_nip98::Keys, name: &str, op: &str, input: Value) -> Result<Value> {
    let id = format!("sx-{op}-{}", now_ms());
    let r = a.op(keys, name, op, &id, input)?;
    anyhow::ensure!(r.status == 200, "{op}: {r}");
    let result = r.body["result"].clone();
    let Some(run) = result["run"].as_i64() else { return Ok(result) };
    let t0 = Instant::now();
    loop {
        let r = a.signed(keys, "GET", &format!("/api/f/{name}/runs/{run}"), None)?;
        let status = r.body["status"].as_str().unwrap_or("").to_string();
        if ["succeeded", "failed", "held", "blocked"].contains(&status.as_str()) {
            return Ok(r.body);
        }
        anyhow::ensure!(t0.elapsed() < Duration::from_secs(180), "{op}'s run {run} still {status}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Every record of a channel, oldest first.
fn records(a: &api::Api, keys: &fragment_nip98::Keys, name: &str, channel: &str) -> Result<Vec<Value>> {
    let mut out = vec![];
    let mut after = 0i64;
    loop {
        let r = a.signed(keys, "GET", &format!("/api/f/{name}/channels/{channel}?after={after}&limit=200"), None)?;
        anyhow::ensure!(r.status == 200, "reading {name}'s {channel}: {r}");
        let page = r.body["records"].as_array().cloned().unwrap_or_default();
        if page.is_empty() {
            return Ok(out);
        }
        after = page.last().and_then(|x| x["seq"].as_i64()).unwrap_or(after);
        out.extend(page);
    }
}

/// The door's admin route (door.mjs), with the levers' secret.
fn admin(a: &api::Api, conn: &str, path: &str, method: &str, body: Option<Vec<u8>>) -> Result<api::Reply> {
    a.call(api::Call {
        method,
        url: format!("{}/api/sxdoor/{conn}/admin/{path}", a.base),
        content_type: body.as_ref().map(|_| "application/octet-stream"),
        body,
        extra: vec![("x-fragment-test-secret", secret()?)],
        ..api::Call::default()
    })
}

fn setup() -> Result<()> {
    let a = hosted()?;
    let mut st = state();
    anyhow::ensure!(st["ownerSk"].is_null(), "set up already: {}", state_path().display());
    let keys = a.person_paying(0)?;
    st["ownerSk"] = json!(keys.secret_hex());
    st["ownerId"] = json!(a.identity(&keys)?);
    save_state(&st)?;
    let r = a.signed(&keys, "POST", "/api/computers", Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "a computer: {r}");
    st["computer"] = r.body["computer"].clone();
    println!("computer {} image {}", r.body["computer"], r.body["image"]);
    let r = a.create_with(&keys, json!({ "label": "e2e-sxd-juniper", "template": "agent", "title": "Juniper" }))?;
    anyhow::ensure!(r.status == 200, "an agent fragment: {r}");
    let agent = r.body["name"].as_str().unwrap_or("").to_string();
    st["agent"] = json!(agent);
    save_state(&st)?;
    let computer = s(&st, "computer")?;
    let r = a.signed(&keys, "PUT", &format!("/api/computers/{computer}/agents/{agent}"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "assigned: {r}");
    st["agentId"] = r.body["agents"][0]["identity"].clone();
    save_state(&st)?;
    println!("{}", serde_json::to_string_pretty(&json!({ "owner": st["ownerId"], "computer": st["computer"], "agent": st["agent"], "agentId": st["agentId"] }))?);
    Ok(())
}

fn connect() -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let conn = s(&st, "connector")?;
    let t0 = Instant::now();
    let run = op(&a, &keys, &conn, "connect", json!({ "platform": a.base, "name": "Juniper" }))?;
    println!("connect: {} in {:.1?}: {}", run["status"], t0.elapsed(), run["output"]);
    Ok(())
}

/// A staged link (a door save: door.json and the two databases, packed)
/// adopted by the connector's door: its next start runs on it.
fn adopt(file: &str) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let conn = s(&st, "connector")?;
    let bytes = std::fs::read(file)?;
    let t0 = Instant::now();
    let r = admin(&a, &conn, "adopt", "POST", Some(bytes))?;
    println!("adopt: {} in {:.1?}: {}", r.status, t0.elapsed(), r.body);
    Ok(())
}

fn connector(name_label: &str) -> Result<()> {
    connector_made(name_label, true)
}

fn connector_made(name_label: &str, connecting: bool) -> Result<()> {
    let a = hosted()?;
    let mut st = state();
    let keys = owner(&st)?;
    let r = a.create_with(&keys, json!({ "label": name_label, "template": "simplex" }))?;
    anyhow::ensure!(r.status == 200, "a SimpleX connector: {r}");
    let conn = r.body["name"].as_str().unwrap_or("").to_string();
    let secret: String = (0..32).map(|_| format!("{:02x}", rand_byte())).collect();
    st["connector"] = json!(conn);
    st["doorSecret"] = json!(secret);
    save_state(&st)?;
    let r = a.signed(&keys, "PUT", &format!("/api/f/{conn}/secrets/SIMPLEX_DOOR_SECRET"), Some(&json!({ "value": secret, "hosts": ["door.fragment.internal"] })))?;
    anyhow::ensure!(r.status == 200, "its secret: {r}");
    println!("connector {conn}");
    if !connecting {
        return Ok(());
    }
    let t0 = Instant::now();
    let run = op(&a, &keys, &conn, "connect", json!({ "platform": a.base, "name": "Juniper" }))?;
    println!("connect: {} in {:.1?}: {}", run["status"], t0.elapsed(), run["output"]);
    Ok(())
}

fn rand_byte() -> u8 {
    let mut b = [0u8; 1];
    std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).expect("urandom");
    b[0]
}

fn link(port: u16, key: &str) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let conn = s(&st, key)?;
    let t0 = Instant::now();
    let run = op(&a, &keys, &conn, "link", json!({}))?;
    println!("link: {} in {:.1?}", run["status"], t0.elapsed());
    let link = run["output"]["short"].as_str().or(run["output"]["link"].as_str()).context(format!("an invitation: {run}"))?.to_string();
    let mut p = Person::open(port)?;
    let t1 = Instant::now();
    println!("{}", p.ask(&format!("/connect {link}"))?["resp"]["type"]);
    let ok = p.until(Duration::from_secs(120), |v| v["resp"]["type"] == "contactConnected")?;
    println!("connected: {} in {:.1?}", ok.is_some(), t1.elapsed());
    Ok(())
}

fn bind() -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let (conn, agent) = (s(&st, "connector")?, s(&st, "agent")?);
    let r = a.signed(&keys, "POST", &format!("/api/f/{conn}/door"), Some(&json!({ "act": "link", "agent": agent })))?;
    println!("{r}");
    Ok(())
}

/// The person sends `n` messages, `<prefix> <k>`; then waits until each is
/// in the connector's chat once, and (with `echo`) each is echoed back to
/// the person once. Prints what arrived, what didn't, and what came twice.
fn burst(port: u16, n: u32, prefix: &str, echo: bool, gap_ms: u64) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let conn = s(&st, "connector")?;
    let mut p = Person::open(port)?;
    let c = person_contact(&mut p)?;
    let mut sent_at = std::collections::BTreeMap::new();
    for k in 1..=n {
        let text = format!("{prefix} {k}");
        let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
        let v = p.ask(&format!("/_send @{c} json {body}"))?;
        anyhow::ensure!(v["resp"]["type"] == "newChatItems", "send {k}: {v}");
        sent_at.insert(text, now_ms());
        if gap_ms > 0 {
            std::thread::sleep(Duration::from_millis(gap_ms));
        }
    }
    println!("sent {n} in all");
    let t0 = Instant::now();
    let want: Vec<String> = sent_at.keys().cloned().collect();
    let mut last = (0, 0);
    loop {
        let recs = records(&a, &keys, &conn, "chat")?;
        let inbound: Vec<&Value> = recs.iter().filter(|r| r["body"]["relayed"].is_object()).collect();
        let got = want.iter().filter(|t| inbound.iter().any(|r| r["body"]["text"] == t.as_str())).count();
        let echoed = if echo {
            let v = p.ask(&format!("/_get chat @{c} count={}", (n * 3).max(50)))?;
            let items = items_of(&v);
            want.iter().filter(|t| items.iter().any(|(_, rcv, text, _, _)| *rcv && text.contains(t.as_str()))).count()
        } else {
            0
        };
        if (got, echoed) != last {
            println!("{:>6.1?}  in the chat {got}/{n}  echoed back {echoed}/{n}", t0.elapsed());
            last = (got, echoed);
        }
        if got == want.len() && (!echo || echoed == want.len()) || t0.elapsed() > Duration::from_secs(600) {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    // the tally: each once, in order
    let recs = records(&a, &keys, &conn, "chat")?;
    let inbound: Vec<(i64, String, i64)> = recs.iter().filter(|r| r["body"]["relayed"].is_object()).map(|r| (r["seq"].as_i64().unwrap_or(0), r["body"]["text"].as_str().unwrap_or("").to_string(), r["at"].as_i64().unwrap_or(0))).collect();
    let mut dup = 0;
    let mut missing = vec![];
    let mut lags = vec![];
    let mut order: Vec<i64> = vec![];
    for t in &want {
        let hits: Vec<&(i64, String, i64)> = inbound.iter().filter(|(_, x, _)| x == t).collect();
        if hits.is_empty() {
            missing.push(t.clone());
        }
        if hits.len() > 1 {
            dup += 1;
        }
        if let Some(h) = hits.first() {
            lags.push(h.2 - sent_at[t]);
            order.push(h.0);
        }
    }
    let in_order = want.iter().filter_map(|t| inbound.iter().find(|(_, x, _)| x == t).map(|h| h.0)).collect::<Vec<_>>();
    let sorted = { let mut v = in_order.clone(); v.sort(); v };
    lags.sort();
    let pct = |q: f64| lags.get(((lags.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(-1);
    let mut out = json!({ "prefix": prefix, "n": n, "inbound": { "arrived": n as usize - missing.len(), "missing": missing, "twice": dup, "inOrder": in_order == sorted, "lagMs": { "p50": pct(0.5), "p90": pct(0.9), "max": pct(1.0) } } });
    if echo {
        let v = p.ask(&format!("/_get chat @{c} count={}", (n * 3).max(50)))?;
        let items = items_of(&v);
        let mut odup = 0;
        let mut omiss = vec![];
        for t in &want {
            let k = items.iter().filter(|(_, rcv, text, _, _)| *rcv && text.contains(t.as_str())).count();
            if k == 0 {
                omiss.push(t.clone());
            }
            if k > 1 {
                odup += 1;
            }
        }
        out["outbound"] = json!({ "arrived": n as usize - omiss.len(), "missing": omiss, "twice": odup });
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    let log = cache().join("bursts.jsonl");
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(log)?;
    std::io::Write::write_all(&mut f, format!("{}\n", json!({ "at": now_ms(), "result": out })).as_bytes())?;
    Ok(())
}

fn door(args: &[String]) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let conn = args.get(2).cloned().map(Ok).unwrap_or_else(|| s(&st, "connector"))?;
    let path = args.get(1).context("an admin path: status, stats?…, saves, kill, rollback?to=, pause, start")?;
    let method = if ["status", "saves"].iter().any(|p| path.starts_with(p)) || path.starts_with("stats") || path.starts_with("save?") { "GET" } else { "POST" };
    let r = admin(&a, &conn, path, method, if method == "POST" { Some(b"{}".to_vec()) } else { None })?;
    if path.starts_with("save?") {
        let out = cache().join(format!("save-{}.bin", path.trim_start_matches("save?id=")));
        std::fs::write(&out, &r.bytes)?;
        println!("{} bytes to {}", r.bytes.len(), out.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&r.body)?);
    }
    Ok(())
}

fn lever(a: &api::Api, computer: &str, body: Value) -> Result<Value> {
    let mut b = body;
    b["computer"] = json!(computer);
    let r = a.unsigned("POST", "/api/test/computer", Some(&b))?;
    anyhow::ensure!(r.status == 200, "lever {b}: {r}");
    Ok(r.body)
}

fn cookie_of(r: &api::Reply, name: &str) -> Option<String> {
    r.headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find_map(|c| c.split(';').next().filter(|kv| kv.starts_with(&format!("{name}="))).map(str::to_string))
}

/// The computer's own SimpleX link (today's door): always on, its owner's
/// ticket redeemed on its link page, the person connecting to it.
fn computer_link(port: u16) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let (computer, agent) = (s(&st, "computer")?, s(&st, "agent")?);
    println!("always-on: {}", lever(&a, &computer, json!({ "op": "always-on", "on": true }))?["alwaysOn"]);
    let t0 = Instant::now();
    let r = a.signed(&keys, "POST", &format!("/api/computers/{computer}/wake"), Some(&json!({})))?;
    println!("wake: {} {} in {:.1?}", r.status, r.body["phase"], t0.elapsed());
    let ticket = a.signed(&keys, "POST", &format!("/api/computers/{computer}/agents/{agent}/door"), Some(&json!({ "door": "simplex", "act": "link" })))?;
    anyhow::ensure!(ticket.status == 200, "a link ticket: {ticket}");
    let nonce = ticket.body["nonce"].as_str().context("a ticket's nonce")?.to_string();
    let (origin, cookie) = port_session(&a, &keys, &computer, &format!("/simplex/?agent={agent}"))?;
    let page = |method: &'static str, path: String, body: Option<Vec<u8>>| a.call(api::Call { method, url: format!("{origin}/p/6080{path}"), content_type: body.as_ref().map(|_| "application/json"), body, cookie: Some(cookie.clone()), extra: vec![("origin", origin.clone()), ("sec-fetch-site", "same-origin".into())], ..api::Call::default() });
    // its link page may answer 503 until the bridge is past its restore gate
    let t1 = Instant::now();
    let r = loop {
        let r = page("POST", format!("/simplex/link?agent={agent}"), Some(ticket.body.to_string().into_bytes()))?;
        if !(r.status == 503 || (r.status == 500 && r.text.contains("not listening"))) || t1.elapsed() > Duration::from_secs(300) {
            break r;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    println!("link page: {} {}", r.status, r.text.chars().take(300).collect::<String>());
    let t2 = Instant::now();
    let link = loop {
        let v = page("GET", format!("/simplex/state?agent={agent}&s={nonce}"), None)?;
        if let Some(l) = v.body["short"].as_str().or(v.body["link"].as_str()).filter(|l| !l.is_empty()) {
            break l.to_string();
        }
        anyhow::ensure!(t2.elapsed() < Duration::from_secs(180), "no invitation: {v}");
        std::thread::sleep(Duration::from_secs(2));
    };
    let mut p = Person::open(port)?;
    println!("{}", p.ask(&format!("/connect {link}"))?["resp"]["type"]);
    let ok = p.until(Duration::from_secs(120), |v| v["resp"]["type"] == "contactConnected")?;
    println!("connected to the computer's core: {}", ok.is_some());
    let v = page("GET", format!("/simplex/state?agent={agent}"), None)?;
    println!("state: {}", v.body);
    Ok(())
}

/// A browser's session on the computer's origin, through a port ticket.
fn port_session(a: &api::Api, keys: &fragment_nip98::Keys, computer: &str, path: &str) -> Result<(String, String)> {
    let r = a.signed(keys, "POST", &format!("/api/computers/{computer}/ports/6080/ticket"), Some(&json!({ "path": path })))?;
    anyhow::ensure!(r.status == 200, "a port ticket: {r}");
    let url = r.body["url"].as_str().context("its url")?.to_string();
    let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
    let visit = a.call(api::Call { method: "GET", url, ..api::Call::default() })?;
    let cookie = cookie_of(&visit, "__Host-fragment_computer").or_else(|| cookie_of(&visit, "fragment_computer")).context(format!("the computer's cookie: {visit}"))?;
    Ok((origin, cookie))
}

fn computer_page(path: &str) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let computer = s(&st, "computer")?;
    let (origin, cookie) = port_session(&a, &keys, &computer, "/")?;
    let r = a.call(api::Call { method: "GET", url: format!("{origin}/p/6080{path}"), cookie: Some(cookie), extra: vec![("origin", origin.clone()), ("sec-fetch-site", "same-origin".into())], ..api::Call::default() })?;
    println!("{} {}", r.status, r.text.chars().take(2000).collect::<String>());
    Ok(())
}

/// The owner's fragment labelled `label`, by its listing.
fn fragment_labelled(a: &api::Api, keys: &fragment_nip98::Keys, label: &str) -> Result<Option<String>> {
    let r = a.signed(keys, "GET", "/api/fragments", None)?;
    Ok(r.body["fragments"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str()).find(|n| n.starts_with(&format!("{label}--"))).map(str::to_string))
}

/// The history question: the agent's old SimpleX chat copied into the
/// connector's chat, in order, each once, before its door is bound.
fn copy() -> Result<()> {
    let a = hosted()?;
    let mut st = state();
    let keys = owner(&st)?;
    let conn = s(&st, "connector")?;
    let agent = s(&st, "agent")?;
    let label = agent.rsplit_once("--").map(|(l, _)| l).unwrap_or(&agent).to_string();
    let old = match st["oldChat"].as_str() {
        Some(o) => o.to_string(),
        None => fragment_labelled(&a, &keys, &format!("{label}-simplex"))?.context("the agent's SimpleX chat")?,
    };
    st["oldChat"] = json!(old);
    save_state(&st)?;
    let recs = records(&a, &keys, &old, "chat")?;
    let messages: Vec<Value> = recs.iter().filter(|r| r["kind"] == "message" && (r["body"]["kind"].is_null() || r["body"]["kind"] == "message")).map(|r| json!({ "seq": r["seq"], "at": r["at"], "principal": r["principal"], "body": r["body"] })).collect();
    println!("{old}: {} records, {} messages", recs.len(), messages.len());
    let t0 = Instant::now();
    let (mut copied, mut skipped) = (0, 0);
    for (i, batch) in messages.chunks(40).enumerate() {
        let r = a.op(&keys, &conn, "copy", &format!("copy-{old}-{i}{}", std::env::var("SXDOOR_COPY_RUN").unwrap_or_default()), json!({ "from": old, "records": batch }))?;
        anyhow::ensure!(r.status == 200, "copy batch {i}: {r}");
        copied += r.body["result"]["copied"].as_i64().unwrap_or(0);
        skipped += r.body["result"]["skipped"].as_i64().unwrap_or(0);
    }
    println!("copied {copied}, skipped {skipped}, in {:.1?}", t0.elapsed());
    Ok(())
}

/// Each message of the old chat in the connector's chat, once, in order,
/// marked copied.
fn copy_check() -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let (conn, old) = (s(&st, "connector")?, s(&st, "oldChat")?);
    let olds: Vec<Value> = records(&a, &keys, &old, "chat")?.into_iter().filter(|r| r["kind"] == "message" && (r["body"]["kind"].is_null() || r["body"]["kind"] == "message")).collect();
    let news: Vec<Value> = records(&a, &keys, &conn, "chat")?.into_iter().filter(|r| r["body"]["copied"].is_object()).collect();
    let mut problems = vec![];
    let mut last = 0;
    for o in &olds {
        let hits: Vec<&Value> = news.iter().filter(|n| n["body"]["copied"]["seq"] == o["seq"]).collect();
        if hits.len() != 1 {
            problems.push(format!("old seq {} copied {} times", o["seq"], hits.len()));
            continue;
        }
        let n = hits[0];
        let by = o["body"]["relayed"]["for"].as_str().or(o["principal"].as_str()).unwrap_or("");
        if n["body"]["text"] != o["body"]["text"] || n["body"]["copied"]["at"] != o["at"] || n["body"]["copied"]["by"] != by {
            problems.push(format!("old seq {} copied otherwise: {} vs {}", o["seq"], n["body"], o["body"]));
        }
        let seq = n["seq"].as_i64().unwrap_or(0);
        if seq < last {
            problems.push(format!("old seq {} out of order", o["seq"]));
        }
        last = seq;
    }
    println!("{}", serde_json::to_string_pretty(&json!({ "old": olds.len(), "copies": news.len(), "problems": problems }))?);
    Ok(())
}

/// The hold's evidence: one message each way through connector `key`'s
/// door (the state's `connector`, or `connector0`), and how long each took.
fn ping(port: u16, key: &str) -> Result<()> {
    let a = hosted()?;
    let st = state();
    let keys = owner(&st)?;
    let conn = s(&st, key)?;
    let mut p = Person::open(port)?;
    let c = person_contact(&mut p)?;
    let n = now_ms();
    let text = format!("ping {n}");
    let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
    p.ask(&format!("/_send @{c} json {body}"))?;
    let t0 = Instant::now();
    let mut inbound_ms = None;
    while t0.elapsed() < Duration::from_secs(120) {
        let recs = records(&a, &keys, &conn, "inbox")?;
        if recs.iter().rev().take(20).any(|r| r["body"]["payload"]["text"] == text.as_str()) {
            inbound_ms = Some(t0.elapsed().as_millis() as u64);
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let pong = format!("pong {n}");
    let t1 = Instant::now();
    let r = admin(&a, &conn, "runner", "POST", Some(json!({ "method": "POST", "path": "/send", "body": { "text": pong } }).to_string().into_bytes()))?;
    let got = p.until(Duration::from_secs(120), |v| v["resp"]["type"] == "newChatItems" && v.to_string().contains(&pong))?;
    let out = json!({ "at": n, "connector": conn, "inboundMs": inbound_ms, "sendStatus": r.status, "outboundMs": got.map(|_| t1.elapsed().as_millis() as u64) });
    println!("{out}");
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(cache().join("pings.jsonl"))?;
    std::io::Write::write_all(&mut f, format!("{out}\n").as_bytes())?;
    Ok(())
}

fn computer_call(method: &str, what: &str) -> Result<()> {
    let a = hosted()?.patient(Duration::from_secs(900));
    let st = state();
    let keys = owner(&st)?;
    let computer = s(&st, "computer")?;
    let t0 = Instant::now();
    let body = if what == "image-stub" { Some(json!({ "image": "stub" })) } else { Some(json!({})) };
    let path = match what { "image-stub" => format!("/api/computers/{computer}/image"), w => format!("/api/computers/{computer}/{w}") };
    let r = a.signed(&keys, method, &path, body.as_ref())?;
    println!("{what}: {} in {:.1?}: phase {} image {} {}", r.status, t0.elapsed(), r.body["phase"], r.body["image"], r.body["why"]);
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(step) = args.first() else { bail!("a step: person-up, person-connect, person-send, person-items, person-cmd") };
    match step.as_str() {
        "person-up" => person_up(args.get(1).context("a name")?, args.get(2).context("a port")?.parse()?),
        "setup" => setup(),
        "connector" => connector_made(args.get(1).map(String::as_str).unwrap_or("e2e-sxd-simplex"), !args.get(2).is_some_and(|a| a == "noconnect")),
        "connect" => connect(),
        "adopt" => adopt(args.get(1).context("a staged save")?),
        "link" => link(args.get(1).context("the person's port")?.parse()?, args.get(2).map(String::as_str).unwrap_or("connector")),
        "bind" => bind(),
        "burst" => burst(args.get(1).context("a port")?.parse()?, args.get(2).context("n")?.parse()?, args.get(3).context("a prefix")?, args.get(4).is_some_and(|e| e == "echo"), args.get(5).map(|g| g.parse()).transpose()?.unwrap_or(0)),
        "door" => door(&args),
        "computer-link" => computer_link(args.get(1).context("the person's port")?.parse()?),
        "computer-page" => computer_page(args.get(1).context("a path")?),
        "copy" => copy(),
        "records" => {
            let a = hosted()?;
            let st = state();
            let keys = owner(&st)?;
            let name = match args.get(2) { Some(n) => n.clone(), None => s(&st, "connector")? };
            println!("{}", serde_json::to_string(&records(&a, &keys, &name, args.get(1).map(String::as_str).unwrap_or("chat"))?)?);
            Ok(())
        }
        "computer" => computer_call(if args.get(1).is_some_and(|a| a == "image-stub") { "PUT" } else { "POST" }, args.get(1).context("sleep, wake, restart or image-stub")?),
        "ping" => ping(args.get(1).context("a port")?.parse()?, args.get(2).map(String::as_str).unwrap_or("connector0")),
        "connector0" => {
            // the hold's own connector, never bound: its door alone
            connector("e2e-sxd-hold")?;
            let mut st = state();
            st["connector0"] = st["connector"].clone();
            st["connector"] = Value::Null;
            save_state(&st)
        }
        "copy-check" => copy_check(),
        "state" => {
            let st = state();
            println!("{}", serde_json::to_string_pretty(&json!({ "ownerId": st["ownerId"], "computer": st["computer"], "agent": st["agent"], "agentId": st["agentId"], "connector": st["connector"], "oldChat": st["oldChat"] }))?);
            Ok(())
        }
        s if s.starts_with("person-") => person_cmd(&args),
        other => bail!("no step {other}"),
    }
}
