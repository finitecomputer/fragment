//! A fragment's search (docs/api.md, "A fragment's search"): the search
//! service itself (search/, its image in Docker on a disk store), the node
//! started naming it, the Workers AI fake embedding. An editor writes a
//! source whole, chunked and embedded once; a reader finds it by its words
//! and its meaning; a replay and a grown source embed nothing again; a
//! stale write is refused; limits are checked before anything is sent; a
//! viewer only reads, a stranger neither, and a member removed reads
//! nothing more; a push that deletes a source's file forgets it, and a
//! forget the service's outage held back, or one past a pass's hundred,
//! never lands after the source's next write; a write held at its
//! embedding, or at the service's door, while its fragment is deleted and
//! made again writes nothing to either life; the service restarted still
//! holds what was written; a deleted fragment's index goes with it; and a
//! deployment whose tiers are private, with no private embeddings slot,
//! indexes by words alone. `fragment search` and `fragment mcp`'s
//! `fragment-search` tool ask it as the signed-in person.

use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::Api;
use crate::Suite;

const IMAGE: &str = "fragment-search:e2e";
const MODEL: &str = fragment_core::embed::MODEL_DEFAULT;

/// The service's container, removed however the section ends.
struct Service {
    name: String,
    url: String,
    port: String,
}

/// Between the node and the service: every call forwarded, but an upsert
/// held while `hold` is on (counted, `held`), until `release`: a write the
/// cell sent that the service has not taken yet.
struct Door {
    server: fragment_fakes::http::Server,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

#[derive(Default)]
struct Gate {
    holding: bool,
    held: usize,
}

impl Door {
    fn open(to: &str) -> Result<Door> {
        use fragment_fakes::http::{Request, Response, Server};
        let gate: Arc<(Mutex<Gate>, Condvar)> = Arc::default();
        let g = Arc::clone(&gate);
        let (to, client) = (to.to_string(), reqwest::blocking::Client::new());
        let server = Server::start(
            0,
            Arc::new(move |req: &Request| {
                if req.path.ends_with("/upsert") {
                    let (lock, changed) = &*g;
                    let mut gate = lock.lock().expect("the door");
                    if gate.holding {
                        gate.held += 1;
                        while gate.holding {
                            gate = changed.wait(gate).expect("the door");
                        }
                    }
                }
                let Ok(mut url) = reqwest::Url::parse(&format!("{to}{}", req.path)) else { return Response::json(400, &json!({ "error": "path" })) };
                if !req.pairs.is_empty() {
                    url.query_pairs_mut().extend_pairs(&req.pairs);
                }
                let method = reqwest::Method::from_bytes(req.method.as_bytes()).unwrap_or(reqwest::Method::GET);
                let mut call = client.request(method, url).body(req.body.clone());
                for h in ["authorization", "content-type"] {
                    if let Some(v) = req.header(h) {
                        call = call.header(h, v);
                    }
                }
                match call.send() {
                    Ok(r) => {
                        let status = r.status().as_u16();
                        Response::bytes(status, "application/json", r.bytes().map(|b| b.to_vec()).unwrap_or_default())
                    }
                    Err(e) => Response::json(502, &json!({ "error": "unreachable", "message": e.to_string() })),
                }
            }),
        )?;
        Ok(Door { server, gate })
    }

    fn hold(&self) {
        self.gate.0.lock().expect("the door").holding = true;
    }

    fn held(&self) -> usize {
        self.gate.0.lock().expect("the door").held
    }

    fn release(&self) {
        self.gate.0.lock().expect("the door").holding = false;
        self.gate.1.notify_all();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.name]).output();
    }
}

fn docker(args: &[&str]) -> Result<String> {
    let out = Command::new("docker").args(args).output().context("docker")?;
    anyhow::ensure!(out.status.success(), "docker {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The service's image (built from search/ unless CI built it before the
/// run), started on a disk store of its container's own.
fn start(token: &str) -> Result<Service> {
    if docker(&["image", "inspect", IMAGE]).is_err() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../search");
        docker(&["build", "--platform", "linux/amd64", "-t", IMAGE, root.to_str().context("a path")?])?;
    }
    let name = format!("fragment-search-e2e-{}", std::process::id());
    let _ = docker(&["rm", "-f", &name]);
    // a port of its own, named: a restart keeps it (`-p 127.0.0.1:0:…` would move)
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port().to_string();
    docker(&["run", "-d", "--name", &name, "-p", &format!("127.0.0.1:{port}:8080"), "-e", "SEARCH_STORAGE=/tmp/search", "-e", &format!("SEARCH_TOKEN={token}"), IMAGE])?;
    let service = Service { name, url: format!("http://127.0.0.1:{port}"), port: port.clone() };
    for _ in 0..100 {
        if healthy(&port) {
            return Ok(service);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("the search service did not answer its health check")
}

/// Whether the service on `port` answers its health check.
fn healthy(port: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut stream) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut answer = String::new();
    stream.write_all(b"GET /healthz HTTP/1.0\r\nhost: search\r\n\r\n").is_ok() && stream.read_to_string(&mut answer).is_ok() && answer.starts_with("HTTP/1.") && answer.split_whitespace().nth(1) == Some("200")
}

/// The tables the service's store holds now.
fn tables(service: &Service) -> BTreeSet<String> {
    docker(&["exec", &service.name, "ls", "/tmp/search"]).map(|out| out.lines().map(str::to_string).collect()).unwrap_or_default()
}

/// How many texts the embeddings model was asked for since `from`.
fn embedded(s: &Suite, from: usize) -> usize {
    s.ai.calls()[from..].iter().filter(|c| c.model == MODEL).map(|c| c.body["text"].as_array().map_or(0, Vec::len)).sum()
}

fn upsert(source: &str, revision: &str, sections: Value) -> Value {
    json!({ "source": source, "revision": revision, "title": "Trip planning", "fields": { "kind": "chat" }, "sections": sections })
}

fn turns() -> Value {
    json!([
        { "id": "t0", "text": "Person: the harbour lantern audit moved to the north annex\nAgent: noted", "at": "2026-10-29T10:00:00.000Z", "location": { "from": 0, "to": 1 } },
        { "id": "t1", "text": "Person: taxes are due in april\nAgent: I will remind you", "location": { "from": 2, "to": 3 } },
    ])
}

pub fn search(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("search", &[crate::Need::Node, crate::Need::LocalDocker, crate::Need::Levers, crate::Need::Fakes]) {
        return Ok(());
    }
    let token = Keys::generate().pubkey_hex()[..40].to_string();
    let service = start(&token)?;
    let door = Door::open(&service.url)?;
    s.stop()?;
    s.search = Some(fragment_devstack::SearchVars { url: door.server.url.clone(), token: token.clone() });
    let checked = match s.start(false) {
        Ok(api) => checks(s, &api, &service, &door),
        Err(e) => Err(e),
    };
    s.stop()?;
    let private = match s.start_as_private_tier() {
        Ok(api) => private_checks(s, &api),
        Err(e) => Err(e),
    };
    s.stop()?;
    s.search = None;
    drop(service);
    s.start(false)?;
    checked.and(private)
}

/// A fragment labelled `label`, made by `owner`, its index made by a
/// first write: its name, and its table's.
fn life(s: &Suite, api: &Api, owner: &Keys, service: &Service, label: &str) -> Result<(String, String)> {
    let created = s.create(api, owner, label)?;
    let name = created["name"].as_str().context("its name")?;
    let before = tables(service);
    let w = api.signed(owner, "POST", &format!("/api/f/{name}/search/upsert"), Some(&upsert("notes/a.md", "r1", json!([{ "id": "t0", "text": "Person: the first life's note" }]))))?;
    anyhow::ensure!(w.status == 200, "{name}'s first write: {w}");
    let made: Vec<String> = tables(service).difference(&before).cloned().collect();
    anyhow::ensure!(made.len() == 1, "{name}'s table: {made:?}");
    Ok((name.to_string(), made[0].clone()))
}

/// A table's tombstone, as the service names it once its scope ended.
fn tombstone(table: &str) -> String {
    format!("{}.ended.lance", table.trim_end_matches(".lance"))
}

fn checks(s: &mut Suite, api: &Api, service: &Service, door: &Door) -> Result<()> {
    let wait = Duration::from_secs(60);
    let owner = api.person()?;
    let reader = api.person()?;
    let stranger = api.person()?;
    let created = s.create(api, &owner, "searched")?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let path = |route: &str| format!("/api/f/{name}/search{route}");
    let r = api.share(&name, &api.identity(&reader)?, "viewer")?;
    anyhow::ensure!(r.status == 200, "a viewer: {r}");
    s.ai.clear_script();

    // ---- an editor writes a source whole; a reader finds it, by words and by meaning
    let calls = s.ai.calls().len();
    let w = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let wrote = embedded(s, calls);
    let calls = s.ai.calls().len();
    let found = api.signed(&reader, "POST", &path(""), Some(&json!({ "q": "harbour lantern audit" })))?;
    let asked = s.ai.calls()[calls..].iter().any(|c| c.model == MODEL && c.body["text"] == json!(["harbour lantern audit"]));
    let hit = &found.body["results"][0];
    let status = api.signed(&reader, "GET", &path("/status"), None)?;
    s.ok(
        "an editor's source is written whole, its sections embedded on the deployment's slot; a reader finds its section by its words and its meaning, the question embedded as theirs, with its location; its status counts it",
        w.status == 200 && w.body["written"] == true && wrote == 2
            && found.status == 200 && found.body["semantic"] == "on" && hit["source"] == "sessions/a.json" && hit["section"] == "t0"
            && hit["signals"] == json!(["lexical", "semantic"]) && hit["location"] == json!({ "from": 0, "to": 1 }) && asked
            && status.body["sources"] == 1 && status.body["semantic"] == "on",
        json!({ "write": w.body, "embedded": wrote, "found": found.body, "status": status.body }),
    );

    // ---- a replay writes and embeds nothing; a grown source embeds only its new turn
    let calls = s.ai.calls().len();
    let again = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let mut grown = turns();
    grown.as_array_mut().unwrap().push(json!({ "id": "t2", "text": "Person: book the ferry for friday" }));
    let more = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r2", grown)))?;
    s.ok(
        "the same revision again writes nothing and embeds nothing; a grown source embeds its new turn alone, its others' vectors the index's own",
        again.status == 200 && again.body["written"] == false && more.status == 200 && more.body["written"] == true && embedded(s, calls) == 1,
        json!({ "again": again.body, "more": more.body, "embedded": embedded(s, calls) }),
    );

    // ---- the CLI, and an MCP client through it, ask it as the signed-in person, a reader
    let home = s.dir("search-cli");
    s.login(api, &home);
    let cli = s.cli_keys(&home).context("the CLI's key")?;
    let shared = api.share(&name, &api.identity(&cli)?, "viewer")?;
    let asked = s.cli_json(api, &home, &["search", &name, "harbour lantern audit", "--json"])?;
    let printed = String::from_utf8_lossy(&s.cli(api, &home, &["search", &name, "book the ferry for friday", "--limit", "1"]).stdout).into_owned();
    s.ok(
        "fragment search asks a fragment's index as the signed-in person, a reader: each result its section, named by its fragment; printed, a line each and its snippet",
        shared.status == 200 && asked["semantic"] == "on" && asked["results"][0]["fragment"] == name.as_str() && asked["results"][0]["section"] == "t0"
            && printed.starts_with("1. sessions/a.json #t2") && printed.contains("ferry"),
        json!({ "json": asked, "printed": printed }),
    );
    let mut mcp = s.bare_cli().args(["mcp", &name]).env("HOME", &home).env("FRAGMENT_HOST", &api.base).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    {
        let mut stdin = mcp.stdin.take().context("its stdin is piped")?;
        let init = json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } });
        for line in [
            json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": init }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "fragment-search", "arguments": { "q": "taxes due in april" } } }),
        ] {
            writeln!(stdin, "{line}")?;
        }
    }
    let out = mcp.wait_with_output()?;
    let answers: Vec<Value> = String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap_or_else(|_| json!({ "not JSON-RPC": l }))).collect();
    let listed = answers.get(1).and_then(|a| a["result"]["tools"].as_array().cloned()).unwrap_or_default();
    let found = answers.get(2).map(|a| a["result"]["structuredContent"].clone()).unwrap_or(Value::Null);
    s.ok(
        "fragment mcp serves the fragment's index as its fragment-search tool, asked as the signed-in reader",
        listed.iter().any(|t| t["name"] == "fragment-search") && found["results"][0]["section"] == "t1" && found["results"][0]["fragment"] == name.as_str(),
        json!({ "mcp": answers }),
    );

    // ---- a stale write is refused, naming the revision it is not
    let mut stale = upsert("sessions/a.json", "r1", turns());
    stale["replaces"] = json!("r0");
    let r = api.signed(&owner, "POST", &path("/upsert"), Some(&stale))?;
    s.ok("a write that replaces a revision the source has not is 409 conflict, naming the one it has", r.status == 409 && r.body["error"] == "conflict" && r.body["current"] == "r2", &r);

    // ---- limits, before anything is embedded or sent
    let calls = s.ai.calls().len();
    let mut refused = Vec::new();
    let many: Vec<Value> = (0..=fragment_proto::search::SECTIONS_MAX).map(|i| json!({ "id": format!("t{i}"), "text": "x" })).collect();
    for (body, status, why) in [
        (upsert("sessions/b.json", "r1", json!(many)), 400, "2000 sections"),
        (upsert("sessions/b.json", "r1", json!([{ "id": "t0", "text": "x".repeat(8001) }])), 400, "past 8000"),
        (upsert("", "r1", turns()), 400, "source is named"),
        (json!({ "source": "s", "revision": "r", "sections": [], "order": 1 }), 400, "unknown field"),
        (upsert("sessions/b.json", "r1", json!([{ "id": "t0", "text": "x".repeat(4 * 1024 * 1024) }])), 413, ""),
    ] {
        let r = api.signed(&owner, "POST", &path("/upsert"), Some(&body))?;
        refused.push((r.status == status && r.message().contains(why), r.status, r.message().chars().take(120).collect::<String>()));
    }
    s.ok(
        "too many sections, a section past 8,000 characters, no source, a field the API does not know, and a body past 4 MiB are refused, saying why, nothing embedded",
        refused.iter().all(|(ok, _, _)| *ok) && embedded(s, calls) == 0,
        format!("{refused:?}"),
    );

    // ---- who may: a viewer reads only, a stranger nothing, a member removed nothing more
    let viewer_writes = api.signed(&reader, "POST", &path("/upsert"), Some(&upsert("sessions/c.json", "r1", turns())))?;
    let stranger_reads = api.signed(&stranger, "POST", &path(""), Some(&json!({ "q": "lantern" })))?;
    let gone = api.signed(&owner, "DELETE", &format!("/api/f/{name}/members/{}", api.identity(&reader)?), None)?;
    let removed_reads = api.signed(&reader, "POST", &path(""), Some(&json!({ "q": "lantern" })))?;
    s.ok(
        "a viewer may search but not write (403); a stranger may not search (403); a member removed searches no more",
        viewer_writes.status == 403 && stranger_reads.status == 403 && gone.status == 200 && removed_reads.status == 403,
        json!({ "viewer_writes": viewer_writes.status, "stranger_reads": stranger_reads.status, "removed": gone.status, "removed_reads": removed_reads.status }),
    );

    // ---- a push that deletes a source's file forgets the source, there and in its sources
    s.commit(&created, &[("sessions/d.json", Some(b"{}".as_slice()))]);
    let d = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/d.json", "r1", json!([{ "id": "t0", "text": "Person: the zebra crossing permit" }]))))?;
    let held = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "zebra permit" })))?;
    s.commit(&created, &[("sessions/d.json", None)]);
    let forgotten = s.eventually(wait, || {
        let found = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "zebra permit", "mode": "lexical" }))).ok();
        let sources = api.signed(&owner, "GET", &path("/sources?limit=1000"), None).ok();
        let status = api.signed(&owner, "GET", &path("/status"), None).ok();
        found.is_some_and(|f| f.body["results"].as_array().is_some_and(|r| r.iter().all(|h| h["source"] != "sessions/d.json")))
            && sources.is_some_and(|x| x.body["sources"].as_array().is_some_and(|r| r.iter().all(|h| h["source"] != "sessions/d.json")))
            && status.is_some_and(|x| x.body["forgetting"] == 0 && x.body["sources"] == 1)
    });
    s.ok(
        "a push that deletes a source's file forgets the source: gone from search, from its sources and from the service, with no write of the app's",
        d.status == 200 && held.body["results"][0]["source"] == "sessions/d.json" && forgotten,
        json!({ "held": held.body }),
    );

    // ---- a forget the service's outage held back goes before its source's next write
    let e = upsert("sessions/e.json", "r1", json!([{ "id": "t0", "text": "Person: the walrus ledger" }]));
    s.commit(&created, &[("sessions/e.json", Some(b"{}".as_slice()))]);
    let first = api.signed(&owner, "POST", &path("/upsert"), Some(&e))?;
    docker(&["stop", &service.name])?;
    s.commit(&created, &[("sessions/e.json", None)]);
    // a read sends what is due: the service down, the forget waits its retry
    let down = api.signed(&owner, "GET", &path("/sources"), None)?;
    docker(&["start", &service.name])?;
    let up_again = s.eventually(wait, || healthy(&service.port));
    let restored = upsert("sessions/e.json", "r2", json!([{ "id": "t0", "text": "Person: the walrus ledger, restored" }]));
    let back = api.signed(&owner, "POST", &path("/upsert"), Some(&restored))?;
    let found = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "walrus ledger", "mode": "lexical" })))?;
    let status = api.signed(&owner, "GET", &path("/status"), None)?;
    s.ok(
        "a forget the service's outage held back is sent before its source's next write: written again during its retry's wait, the source is found at once, and nothing is left to forget it",
        first.status == 200 && down.status != 200 && up_again && back.status == 200 && back.body["written"] == true
            && found.body["results"][0]["source"] == "sessions/e.json" && found.body["results"][0]["revision"] == "r2" && status.body["forgetting"] == 0,
        json!({ "down": down.status, "back": back.body, "found": found.body, "status": status.body }),
    );

    // ---- more forgets than a pass sends: the last source's next write stays
    let notes: Vec<String> = (0..=100).map(|i| format!("notes/n{i:03}.md")).collect();
    s.commit(&created, &notes.iter().map(|p| (p.as_str(), Some(b"x".as_slice()))).collect::<Vec<_>>());
    for p in &notes {
        let r = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert(p, "r1", json!([{ "id": "t0", "text": "Person: a numbered note" }]))))?;
        anyhow::ensure!(r.status == 200, "{p}: {r}");
    }
    s.commit(&created, &notes.iter().map(|p| (p.as_str(), None)).collect::<Vec<_>>());
    let last = &notes[100];
    let again = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert(last, "r2", json!([{ "id": "t0", "text": "Person: the last note, written again" }]))))?;
    let settled = s.eventually(wait, || api.signed(&owner, "GET", &path("/status"), None).is_ok_and(|r| r.body["forgetting"] == 0));
    let listed = api.signed(&owner, "GET", &path("/sources?limit=1000"), None)?;
    let kept: Vec<&str> = listed.body["sources"].as_array().into_iter().flatten().filter_map(|x| x["source"].as_str()).filter(|x| x.starts_with("notes/")).collect();
    s.ok(
        "101 files deleted in one push, more than a pass forgets: the last one's source, written again after, stays; the other 100 are forgotten",
        again.status == 200 && settled && kept == [last.as_str()],
        json!({ "again": again.body, "kept": kept }),
    );

    // ---- a write held at its embedding while its fragment is deleted and made again
    let late = |text: &str| upsert("notes/b.md", "r1", json!([{ "id": "t0", "text": text }]));
    let (a, table_a) = life(s, api, &owner, service, "search-life-a")?;
    s.ai.hold_embeddings();
    let calls = s.ai.calls().len();
    let (wrote, waiting, deleted, ended, remade) = std::thread::scope(|scope| {
        let wrote = scope.spawn(|| api.signed(&owner, "POST", &format!("/api/f/{a}/search/upsert"), Some(&late("Person: the late wombat"))));
        let waiting = s.eventually(wait, || embedded(s, calls) > 0);
        let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{a}"), None);
        let ended = s.eventually(wait, || tables(service).contains(&tombstone(&table_a)) && !tables(service).contains(&table_a));
        let remade = s.create(api, &owner, &a);
        s.ai.release_embeddings();
        (wrote.join().expect("the write's thread"), waiting, deleted, ended, remade)
    });
    s.ai.release_embeddings();
    let (wrote, deleted, remade) = (wrote?, deleted?, remade?);
    let now = api.signed(&owner, "GET", &format!("/api/f/{a}/search/status"), None)?;
    s.ok(
        "a write held at its embedding while its fragment is deleted and made again under its name writes nothing: refused 404, the ended life's index gone for good, the new life's empty",
        waiting && deleted.status == 200 && ended && remade["name"] == a.as_str() && wrote.status == 404
            && !tables(service).contains(&table_a) && now.status == 200 && now.body["exists"] == false,
        json!({ "wrote": wrote.body, "now": now.body, "tables": tables(service) }),
    );

    // ---- a write held at the service's door while its fragment is deleted and made again
    let (b, table_b) = life(s, api, &owner, service, "search-life-b")?;
    door.hold();
    let (wrote, held, deleted, ended, remade) = std::thread::scope(|scope| {
        let wrote = scope.spawn(|| api.signed(&owner, "POST", &format!("/api/f/{b}/search/upsert"), Some(&late("Person: the held pangolin"))));
        let held = s.eventually(wait, || door.held() >= 1);
        let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{b}"), None);
        let ended = s.eventually(wait, || tables(service).contains(&tombstone(&table_b)) && !tables(service).contains(&table_b));
        let remade = s.create(api, &owner, &b);
        door.release();
        (wrote.join().expect("the write's thread"), held, deleted, ended, remade)
    });
    door.release();
    let (wrote, deleted, remade) = (wrote?, deleted?, remade?);
    let now = api.signed(&owner, "GET", &format!("/api/f/{b}/search/status"), None)?;
    let found = api.signed(&owner, "POST", &format!("/api/f/{b}/search"), Some(&json!({ "q": "pangolin" })))?;
    s.ok(
        "a write the cell sent that the service takes only after its fragment was deleted and made again lands on an ended index: refused, never made again, the new life's index empty",
        held && deleted.status == 200 && ended && remade["name"] == b.as_str() && wrote.status == 404
            && !tables(service).contains(&table_b) && now.body["exists"] == false && found.body["results"] == json!([]),
        json!({ "wrote": wrote.body, "now": now.body, "found": found.body, "tables": tables(service) }),
    );

    // ---- the service restarted holds what was written
    docker(&["restart", &service.name])?;
    let back = s.eventually(wait, || api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "ferry friday" }))).is_ok_and(|r| r.body["results"][0]["section"] == "t2"));
    s.ok("the search service restarted, what was written is searched still", back, json!({}));

    // ---- a deleted fragment's index is dropped with it
    let doomed = s.create(api, &owner, "doomed")?;
    let doomed_name = doomed["name"].as_str().context("its name")?.to_string();
    let before = tables(service);
    let w = api.signed(&owner, "POST", &format!("/api/f/{doomed_name}/search/upsert"), Some(&upsert("notes/a.md", "r1", turns())))?;
    let made: Vec<String> = tables(service).difference(&before).cloned().collect();
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{doomed_name}"), None)?;
    let dropped = made.len() == 1 && s.eventually(wait, || !tables(service).contains(&made[0]));
    s.ok("a deleted fragment's index is dropped from the service's store with its other data", w.status == 200 && deleted.status == 200 && dropped, json!({ "made": made, "now": tables(service) }));
    Ok(())
}

/// On a node whose tiers are private, with no private embeddings slot: by
/// words alone, nothing sent to a model.
fn private_checks(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let created = s.create(api, &owner, "private-search")?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let calls = s.ai.calls().len();
    let w = api.signed(&owner, "POST", &format!("/api/f/{name}/search/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let found = api.signed(&owner, "POST", &format!("/api/f/{name}/search"), Some(&json!({ "q": "harbour lantern" })))?;
    let embeds = s.ai.calls()[calls..].iter().filter(|c| c.model == MODEL).count();
    s.ok(
        "a deployment whose tiers are private and whose embeddings slot is not indexes and searches by words alone: written, found, semantic off, nothing sent to embed",
        w.status == 200 && found.status == 200 && found.body["semantic"] == "off" && found.body["results"][0]["signals"] == json!(["lexical"]) && embeds == 0,
        json!({ "write": w.body, "found": found.body, "embeds": embeds }),
    );
    Ok(())
}
