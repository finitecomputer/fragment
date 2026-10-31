//! The service on a directory of its own: what is written is searched,
//! once; a replay writes nothing; a stale write is refused; a space is
//! pinned; a restart reads what was written; what is deleted leaves the
//! store's files; and every call but /healthz needs the token.

use base64::Engine;
use fragment_search::api::{Search, Upsert};
use fragment_search::store::{Error, Store, Timing};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const SCOPE: &str = "f0123-1-members";

/// A test's own directory, removed when the test ends, however it ends.
struct Dir(PathBuf);

impl std::ops::Deref for Dir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let d = std::env::temp_dir().join(format!("fragment-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Dir(d)
}

/// Maintenance at once after a write, keeping a version a second after
/// the next is made, for the reads a test makes meanwhile (the service's
/// is a minute).
fn fast() -> Timing {
    Timing { after: Duration::from_millis(10), keep: Duration::from_secs(1) }
}

async fn store(d: &Path) -> Arc<Store> {
    store_with(d, fast()).await
}

/// Its sweep done: a test's own writes race no sweep of its tables.
async fn store_with(d: &Path, timing: Timing) -> Arc<Store> {
    let s = Store::open(d.to_str().unwrap(), vec![], timing).await.unwrap();
    s.swept().await;
    s
}

/// A stand-in for a text's sha256: 64 hex of its bytes' hash.
fn hash(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    (0..4u8)
        .map(|salt| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (salt, text).hash(&mut h);
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn b64(v: &[f32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())
}

fn space() -> Value {
    json!({ "model": "test-embedder", "dimensions": 4, "version": "1" })
}

fn chunk(text: &str, v: Option<[f32; 4]>) -> Value {
    match v {
        Some(v) => json!({ "text": text, "hash": hash(text), "vector": b64(&v) }),
        None => json!({ "text": text, "hash": hash(text) }),
    }
}

fn up(source: &str, revision: &str, sections: Value) -> Upsert {
    serde_json::from_value(json!({ "source": source, "revision": revision, "title": format!("title of {source}"), "at": "2026-10-29T10:00:00.000Z", "fields": { "kind": "chat" }, "space": space(), "sections": sections })).unwrap()
}

fn search(v: Value) -> Search {
    serde_json::from_value(v).unwrap()
}

fn harbour() -> Value {
    json!([
        { "id": "t0", "heading": "lanterns", "context": ["telegram"], "location": { "from": 0, "to": 3 }, "chunks": [chunk("the harbour lantern audit moved to the north annex", Some([1.0, 0.0, 0.0, 0.0]))] },
        { "id": "t1", "chunks": [chunk("taxes are due in april", Some([0.0, 1.0, 0.0, 0.0])), chunk("bring the receipts", Some([0.0, 0.9, 0.1, 0.0]))] },
    ])
}

#[tokio::test]
async fn what_is_written_is_searched_by_words_and_by_meaning() {
    let d = dir("valid");
    let s = store(&d).await;
    let w = s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
    assert!(w.written && w.chunks == 3);
    s.upsert(SCOPE, up("sessions/b.json", "r1", json!([{ "id": "t0", "chunks": [chunk("lunch was tacos on friday", Some([0.0, 0.0, 1.0, 0.0]))] }]))).await.unwrap();

    let hits = s.search(SCOPE, search(json!({ "q": "lantern audit", "mode": "lexical" }))).await.unwrap();
    assert_eq!((hits[0].source.as_str(), hits[0].section.as_str()), ("sessions/a.json", "t0"));
    assert_eq!(hits[0].signals, vec!["lexical"]);
    assert!(hits[0].snippet.contains("lantern"));
    assert_eq!(hits[0].location, Some(json!({ "from": 0, "to": 3 })));
    assert_eq!(hits[0].context, vec!["telegram"]);

    let near_taxes = search(json!({ "vector": b64(&[0.0, 1.0, 0.05, 0.0]), "space": space(), "mode": "semantic" }));
    let hits = s.search(SCOPE, near_taxes).await.unwrap();
    assert_eq!((hits[0].source.as_str(), hits[0].section.as_str()), ("sessions/a.json", "t1"), "a section is found by its chunks' vectors");
    assert_eq!(hits.iter().filter(|h| h.section == "t1" && h.source == "sessions/a.json").count(), 1, "once, however many of its chunks matched");

    let both = search(json!({ "q": "harbour", "vector": b64(&[1.0, 0.0, 0.0, 0.0]), "space": space() }));
    let hits = s.search(SCOPE, both).await.unwrap();
    assert_eq!(hits[0].signals, vec!["lexical", "semantic"]);

    let filtered = s.search(SCOPE, search(json!({ "q": "tacos", "fields": { "kind": "mail" } }))).await.unwrap();
    assert!(filtered.is_empty(), "a field filter is every field named");
    let later = s.search(SCOPE, search(json!({ "q": "tacos", "after": "2026-11-01" }))).await.unwrap();
    assert!(later.is_empty());
    let none = s.search("f-never-written", search(json!({ "q": "anything" }))).await.unwrap();
    assert!(none.is_empty(), "a scope never written is searched and empty");
}

#[tokio::test]
async fn a_replay_writes_nothing_and_a_stale_write_is_refused() {
    let d = dir("replay");
    let s = store(&d).await;
    s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
    let v1 = s.status(SCOPE).await.unwrap()["version"].clone();
    let again = s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
    assert!(!again.written);
    assert_eq!(s.status(SCOPE).await.unwrap()["version"], v1, "a replay commits nothing");

    let mut stale = up("sessions/a.json", "r0", harbour());
    stale.replaces = Some("r-old".into());
    assert!(matches!(s.upsert(SCOPE, stale).await, Err(Error::Conflict { current: Some(c) }) if c == "r1"));
    let mut fresh = up("sessions/a.json", "r1", harbour());
    fresh.replaces = Some(String::new());
    fresh.revision = "r9".into();
    assert!(matches!(s.upsert(SCOPE, fresh).await, Err(Error::Conflict { .. })), "\"\" replaces only a source that has none");
    let mut next = up("sessions/a.json", "r2", json!([{ "id": "t0", "chunks": [chunk("only this now", Some([0.5, 0.5, 0.0, 0.0]))] }]));
    next.replaces = Some("r1".into());
    assert!(s.upsert(SCOPE, next).await.unwrap().written);
    let old = s.search(SCOPE, search(json!({ "q": "receipts", "mode": "lexical" }))).await.unwrap();
    assert!(old.is_empty(), "a source is replaced whole: its old sections are gone");
    let (sources, _) = s.sources(SCOPE, "", 10).await.unwrap();
    assert_eq!(sources[0]["revision"], "r2");
}

#[tokio::test]
async fn vectors_are_reused_by_their_text_and_never_mixed() {
    let d = dir("reuse");
    let s = store(&d).await;
    s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
    let known = s.known(SCOPE, serde_json::from_value(json!({ "hashes": [hash("bring the receipts"), hash("never written")] })).unwrap()).await.unwrap();
    assert_eq!(known, vec![hash("bring the receipts")]);
    let grown = json!([
        { "id": "t0", "chunks": [chunk("the harbour lantern audit moved to the north annex", None)] },
        { "id": "t1", "chunks": [chunk("taxes are due in april", None), chunk("bring the receipts", None)] },
        { "id": "t2", "chunks": [chunk("a new turn", Some([0.0, 0.0, 0.0, 1.0]))] },
    ]);
    assert!(s.upsert(SCOPE, up("sessions/a.json", "r2", grown)).await.unwrap().written, "old chunks' vectors are the table's own");
    let missing = json!([{ "id": "t9", "chunks": [chunk("no vector anywhere", None)] }]);
    assert!(matches!(s.upsert(SCOPE, up("sessions/a.json", "r3", missing)).await, Err(Error::MissingVectors(h)) if h == vec![hash("no vector anywhere")]));

    let mut other = up("sessions/c.json", "r1", json!([{ "id": "t0", "chunks": [chunk("x", Some([1.0, 0.0, 0.0, 0.0]))] }]));
    other.space = Some(serde_json::from_value(json!({ "model": "test-embedder", "dimensions": 4, "version": "2" })).unwrap());
    assert!(matches!(s.upsert(SCOPE, other).await, Err(Error::SpaceChanged { .. })), "another version is another space");
    let wrong = search(json!({ "vector": b64(&[1.0, 0.0, 0.0, 0.0]), "space": { "model": "other", "dimensions": 4 } }));
    assert!(matches!(s.search(SCOPE, wrong).await, Err(Error::SpaceChanged { .. })));
    let mut short = up("sessions/d.json", "r1", json!([{ "id": "t0", "chunks": [{ "text": "x", "hash": hash("x"), "vector": b64(&[1.0, 0.0]) }] }]));
    short.title = None;
    assert!(matches!(s.upsert(SCOPE, short).await, Err(Error::Invalid(m)) if m.contains("4 float32s")));

    let mut words: Upsert = serde_json::from_value(json!({ "source": "notes/a.md", "revision": "r1", "space": null, "sections": [{ "id": "s0", "chunks": [{ "text": "words alone", "hash": hash("words alone") }] }] })).unwrap();
    assert!(s.upsert("f-words-1-members", words.clone()).await.unwrap().written, "a table of words alone, on a deployment that embeds nothing");
    assert_eq!(s.status("f-words-1-members").await.unwrap()["semantic"], "off");
    words.space = Some(serde_json::from_value(space()).unwrap());
    words.revision = "r2".into();
    assert!(matches!(s.upsert("f-words-1-members", words).await, Err(Error::SpaceChanged { current: None })));
}

#[tokio::test]
async fn a_restart_reads_what_was_written_and_sources_page_in_order() {
    let d = dir("restart");
    {
        let s = store(&d).await;
        for name in ["e", "a", "d", "b", "c"] {
            let sections = json!([{ "id": "t0", "chunks": [chunk(&format!("note {name} about lanterns"), Some([1.0, 0.0, 0.0, 0.0]))] }]);
            s.upsert(SCOPE, up(&format!("sessions/{name}.json"), "r1", sections)).await.unwrap();
        }
    }
    let s = store(&d).await;
    let status = s.status(SCOPE).await.unwrap();
    assert_eq!((status["sources"].as_u64(), status["chunks"].as_u64(), &status["space"]), (Some(5), Some(5), &space()));
    let hits = s.search(SCOPE, search(json!({ "q": "lanterns", "limit": 3 }))).await.unwrap();
    assert_eq!(hits.len(), 3);
    let (first, next) = s.sources(SCOPE, "", 2).await.unwrap();
    assert_eq!(first.iter().map(|x| x["source"].as_str().unwrap()).collect::<Vec<_>>(), ["sessions/a.json", "sessions/b.json"]);
    let (second, next) = s.sources(SCOPE, next.as_deref().unwrap(), 2).await.unwrap();
    assert_eq!(second[0]["source"], "sessions/c.json");
    let (last, end) = s.sources(SCOPE, next.as_deref().unwrap(), 2).await.unwrap();
    assert_eq!((last.len(), end), (1, None));
}

/// Every file under `d`, as bytes.
fn files(d: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files(&p));
        } else {
            out.push(std::fs::read(&p).unwrap());
        }
    }
    out
}

fn holds(d: &Path, needle: &[u8]) -> bool {
    files(d).iter().any(|f| f.windows(needle.len()).any(|w| w == needle))
}

#[tokio::test]
async fn what_is_deleted_leaves_the_store_and_a_drop_takes_the_table() {
    let d = dir("purge");
    let s = store(&d).await;
    let secret = "zqv8secretword";
    s.upsert(SCOPE, up("sessions/a.json", "r1", json!([{ "id": "t0", "chunks": [chunk(&format!("the {secret} is here"), Some([1.0, 0.0, 0.0, 0.0]))] }]))).await.unwrap();
    s.upsert(SCOPE, up("sessions/b.json", "r1", json!([{ "id": "t0", "chunks": [chunk("kept", Some([0.0, 1.0, 0.0, 0.0]))] }]))).await.unwrap();
    assert!(holds(&d, secret.as_bytes()));
    assert!(s.delete(SCOPE, "sessions/a.json").await.unwrap());
    assert!(!s.delete(SCOPE, "sessions/a.json").await.unwrap(), "a delete again deletes nothing");
    assert!(s.search(SCOPE, search(json!({ "q": secret }))).await.unwrap().is_empty(), "gone from the next search at once");
    s.purge(SCOPE).await.unwrap();
    assert!(!holds(&d, secret.as_bytes()), "and from the store's files once purged");
    assert_eq!(s.search(SCOPE, search(json!({ "q": "kept" }))).await.unwrap().len(), 1);
    assert!(s.drop_table(SCOPE).await.unwrap());
    assert_eq!(s.status(SCOPE).await.unwrap()["exists"], false);
    assert!(!s.drop_table(SCOPE).await.unwrap());
}

#[tokio::test]
async fn the_api_needs_the_token() {
    use http_body_util::{BodyExt, Full};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;

    let d = dir("http");
    let token = "t".repeat(32);
    let service = Arc::new(fragment_search::http::Service { store: store(&d).await, token: token.clone() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service.clone();
            tokio::spawn(async move {
                let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |r| service.clone().handle(r))).await;
            });
        }
    });
    let call = |method: &str, path: &str, auth: Option<String>, body: Option<Value>| {
        let (method, path) = (method.to_string(), path.to_string());
        async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
            tokio::spawn(conn);
            let mut req = hyper::Request::builder().method(method.as_str()).uri(path).header("host", "search");
            if let Some(a) = auth {
                req = req.header("authorization", format!("Bearer {a}"));
            }
            let body = body.map(|b| b.to_string()).unwrap_or_default();
            let resp = send.send_request(req.body(Full::new(bytes::Bytes::from(body))).unwrap()).await.unwrap();
            let status = resp.status().as_u16();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
        }
    };
    assert_eq!(call("GET", "/healthz", None, None).await.0, 200);
    assert_eq!(call("GET", "/v1/f-a/status", None, None).await.0, 401);
    assert_eq!(call("GET", "/v1/f-a/status", Some("t".repeat(31) + "u"), None).await.0, 401);
    let auth = Some(token);
    let upsert = json!({ "source": "s/a", "revision": "r1", "space": null, "sections": [{ "id": "t0", "chunks": [{ "text": "lantern", "hash": hash("lantern") }] }] });
    assert_eq!(call("POST", "/v1/f-a/upsert", auth.clone(), Some(upsert)).await.0, 200);
    let (status, found) = call("POST", "/v1/f-a/search", auth.clone(), Some(json!({ "q": "lantern" }))).await;
    assert_eq!((status, found["results"][0]["source"].clone()), (200, json!("s/a")));
    let (status, page) = call("GET", "/v1/f-a/sources?limit=10&after=", auth.clone(), None).await;
    assert_eq!((status, page["sources"].as_array().map(Vec::len), page["next"].clone()), (200, Some(1), Value::Null));
    let (status, refused) = call("POST", "/v1/f-a/upsert", auth.clone(), Some(json!({ "source": "s/b", "revision": "r", "space": null, "sections": [], "order": 3 }))).await;
    assert_eq!((status, refused["error"].clone()), (400, json!("invalid")));
    assert_eq!(call("GET", "/v1/BAD/status", auth.clone(), None).await.0, 400);
    assert_eq!(call("GET", "/v1/f-a/nothing", auth.clone(), None).await.0, 404);
    assert_eq!(call("DELETE", "/v1/f-a", auth.clone(), None).await.1["dropped"], true);
    let upsert = json!({ "source": "s/a", "revision": "r1", "space": null, "sections": [{ "id": "t0", "chunks": [{ "text": "lantern", "hash": hash("lantern") }] }] });
    assert_eq!(call("POST", "/v1/f-a/upsert", auth.clone(), Some(upsert.clone())).await.0, 200, "a dropped scope is written again");
    assert_eq!(call("POST", "/v1/f-a/end", auth.clone(), None).await.1, json!({ "ended": true, "dropped": true }));
    let (status, gone) = call("POST", "/v1/f-a/upsert", auth, Some(upsert)).await;
    assert_eq!((status, gone["error"].clone()), (410, json!("ended")), "an ended one never is");
}

/// The same on an S3-compatible store (R2, or MinIO on a machine of one's
/// own): `SEARCH_TEST_STORAGE=s3://<bucket>/<prefix>` and its options as
/// `SEARCH_TEST_STORAGE_<OPTION>`, or a secret's as `<…>_FILE`, its file.
/// A write is visible, a restart reads it, and a delete and a purge take it
/// from the next search; it says how long each took.
#[tokio::test]
#[ignore = "needs an S3-compatible store: SEARCH_TEST_STORAGE"]
async fn on_an_object_store() {
    let uri = std::env::var("SEARCH_TEST_STORAGE").expect("SEARCH_TEST_STORAGE");
    let options = fragment_search::env::options("SEARCH_TEST_STORAGE_").unwrap();
    let open = || async { Store::open(&uri, options.clone(), Timing::default()).await.unwrap() };
    let scope = format!("f-s3-{}", std::process::id());
    let mut took: Vec<(&str, u128)> = Vec::new();
    let mut lap = std::time::Instant::now();
    let mut mark = |what: &'static str, took: &mut Vec<(&str, u128)>| {
        took.push((what, lap.elapsed().as_millis()));
        lap = std::time::Instant::now();
    };
    {
        let s = open().await;
        mark("open", &mut took);
        let w = s.upsert(&scope, up("sessions/a.json", "r1", harbour())).await.unwrap();
        assert!(w.written);
        mark("first write (table made, indexed)", &mut took);
        let hits = s.search(&scope, search(json!({ "q": "lantern", "vector": b64(&[1.0, 0.0, 0.0, 0.0]), "space": space() }))).await.unwrap();
        assert_eq!(hits[0].section, "t0");
        mark("hybrid search", &mut took);
    }
    let s = open().await;
    assert_eq!(s.status(&scope).await.unwrap()["chunks"], 3, "a restart reads what was written");
    mark("reopened, status", &mut took);
    let hits = s.search(&scope, search(json!({ "q": "lantern", "vector": b64(&[1.0, 0.0, 0.0, 0.0]), "space": space() }))).await.unwrap();
    assert_eq!(hits[0].section, "t0");
    mark("first hybrid search after reopen (cold)", &mut took);
    let hits = s.search(&scope, search(json!({ "q": "taxes" }))).await.unwrap();
    assert_eq!(hits[0].section, "t1");
    mark("warm search", &mut took);
    let mut next = up("sessions/a.json", "r2", json!([{ "id": "t0", "chunks": [chunk("only this now", Some([0.5, 0.5, 0.0, 0.0]))] }]));
    next.replaces = Some("r1".into());
    assert!(s.upsert(&scope, next).await.unwrap().written);
    mark("replace (merge_insert)", &mut took);
    let mut stale = up("sessions/a.json", "r3", harbour());
    stale.replaces = Some("r1".into());
    assert!(matches!(s.upsert(&scope, stale).await, Err(Error::Conflict { .. })), "a stale write, refused by the conditional commit's caller");
    assert!(s.delete(&scope, "sessions/a.json").await.unwrap());
    mark("stale refused, delete", &mut took);
    s.purge(&scope).await.unwrap();
    mark("purge (compact, prune)", &mut took);
    assert!(s.search(&scope, search(json!({ "q": "only" }))).await.unwrap().is_empty());
    assert!(s.drop_table(&scope).await.unwrap());
    mark("drop", &mut took);
    eprintln!("{}", json!({ "store": uri.split('/').take(3).collect::<Vec<_>>().join("/"), "ms": took.iter().map(|(k, v)| json!({ k.to_string(): v })).collect::<Vec<_>>() }));
}

#[tokio::test]
async fn an_empty_write_keeps_its_revision() {
    let d = dir("empty");
    {
        let s = store(&d).await;
        s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
        let mut emptied = up("sessions/a.json", "r2", json!([]));
        emptied.replaces = Some("r1".into());
        assert!(s.upsert(SCOPE, emptied.clone()).await.unwrap().written);
        assert!(!s.upsert(SCOPE, emptied.clone()).await.unwrap().written, "its replay, after a lost answer, is a no-op");
        assert!(s.search(SCOPE, search(json!({ "q": "lantern" }))).await.unwrap().is_empty());
        // a source empty from its first write, in a table and in a scope never written
        assert!(s.upsert(SCOPE, up("sessions/b.json", "r1", json!([]))).await.unwrap().written);
        assert!(s.upsert("f-empty-1-members", up("sessions/c.json", "r1", json!([]))).await.unwrap().written);
    }
    let s = store(&d).await;
    let mut emptied = up("sessions/a.json", "r2", json!([]));
    emptied.replaces = Some("r1".into());
    assert!(!s.upsert(SCOPE, emptied).await.unwrap().written, "and after a restart");
    let mut next = up("sessions/a.json", "r3", json!([{ "id": "t0", "chunks": [chunk("back again", Some([1.0, 0.0, 0.0, 0.0]))] }]));
    next.replaces = Some("r2".into());
    assert!(s.upsert(SCOPE, next).await.unwrap().written, "a write replacing the empty revision");
    for (scope, source) in [(SCOPE, "sessions/b.json"), ("f-empty-1-members", "sessions/c.json")] {
        let mut stale = up(source, "r9", json!([]));
        stale.replaces = Some(String::new());
        assert!(matches!(s.upsert(scope, stale).await, Err(Error::Conflict { current: Some(c) }) if c == "r1"), "{source} has r1");
        let mut next = up(source, "r2", json!([{ "id": "t0", "chunks": [chunk("now some words", Some([0.0, 1.0, 0.0, 0.0]))] }]));
        next.replaces = Some("r1".into());
        assert!(s.upsert(scope, next).await.unwrap().written, "{source}");
    }
    let (sources, _) = s.sources(SCOPE, "", 10).await.unwrap();
    let listed: Vec<(&str, &str, u64)> = sources.iter().map(|x| (x["source"].as_str().unwrap(), x["revision"].as_str().unwrap(), x["chunks"].as_u64().unwrap())).collect();
    assert_eq!(listed, vec![("sessions/a.json", "r3", 1), ("sessions/b.json", "r2", 1)]);
    assert_eq!(s.status(SCOPE).await.unwrap()["chunks"], 2, "a source's revision is no chunk");
    assert!(!s.search(SCOPE, search(json!({ "vector": b64(&[0.0, 1.0, 0.0, 0.0]), "space": space(), "mode": "semantic" }))).await.unwrap().is_empty());
}

/// `d`'s directory `rel`, read-only while `f` runs (a store that refuses
/// writes there), then writable again.
async fn read_only<T>(d: &Path, rel: &str, f: impl std::future::Future<Output = T>) -> T {
    use std::os::unix::fs::PermissionsExt;
    let p = d.join(rel);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(std::fs::write(p.join("probe"), b"").is_err(), "a read-only directory refuses this user's writes (not root's)");
    let out = f.await;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    out
}

#[tokio::test]
async fn an_interrupted_first_write_is_finished_before_it_is_served() {
    let d = dir("init");
    let first = || up("sessions/a.json", "r1", harbour());
    {
        let s = store(&d).await;
        // its table is made, then its first text index fails
        let failed = read_only(&d, &format!("{SCOPE}.lance/_indices"), s.upsert(SCOPE, first())).await;
        assert!(failed.is_err(), "nothing is acknowledged before the table is indexed");
        assert!(s.upsert(SCOPE, first()).await.is_ok(), "its retry finishes it");
        assert_eq!(s.search(SCOPE, search(json!({ "q": "lantern" }))).await.unwrap()[0].section, "t0");
    }
    let d2 = dir("init-restart");
    {
        let s = store(&d2).await;
        assert!(read_only(&d2, &format!("{SCOPE}.lance/_indices"), s.upsert(SCOPE, first())).await.is_err());
    }
    let s = store(&d2).await;
    assert_eq!(s.search(SCOPE, search(json!({ "q": "lantern" }))).await.unwrap()[0].section, "t0", "a restart finishes it before serving it");
    assert!(!s.upsert(SCOPE, first()).await.unwrap().written);
}

/// A word only source `i` holds.
fn word(i: usize) -> String {
    format!("zqv{i:02}word")
}

/// Twenty sources in one file (a purge compacts them into it), so one of
/// them is a twentieth of it, under Lance's own threshold of a tenth.
async fn twenty(s: &Arc<Store>) {
    for i in 0..20 {
        let sections = json!([{ "id": "t0", "chunks": [chunk(&format!("note {i} holds {}", word(i)), Some([1.0, 0.0, 0.0, 0.0]))] }]);
        s.upsert(SCOPE, up(&format!("sessions/{i:02}.json"), "r1", sections)).await.unwrap();
    }
    s.purge(SCOPE).await.unwrap();
}

async fn settled(s: &Store) {
    s.maintenance(SCOPE).await.expect("its table is open").wait_for(|m| !m.pending).await.unwrap();
}

#[tokio::test]
async fn a_low_fraction_delete_leaves_the_store_files() {
    let d = dir("low");
    let s = store(&d).await;
    twenty(&s).await;
    assert!(holds(&d, word(7).as_bytes()) && holds(&d, word(8).as_bytes()));
    assert!(s.delete(SCOPE, "sessions/07.json").await.unwrap());
    settled(&s).await;
    assert!(!holds(&d, word(7).as_bytes()), "its rows rewritten away, its words out of the indexes");
    assert!(holds(&d, word(8).as_bytes()));
    let mut next = up("sessions/08.json", "r2", json!([{ "id": "t0", "chunks": [chunk("note 8 rewritten", Some([0.0, 1.0, 0.0, 0.0]))] }]));
    next.replaces = Some("r1".into());
    assert!(s.upsert(SCOPE, next).await.unwrap().written);
    settled(&s).await;
    assert!(!holds(&d, word(8).as_bytes()), "a replaced revision's too");
    assert_eq!(s.search(SCOPE, search(json!({ "q": word(9) }))).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_delete_while_maintenance_runs_gets_a_pass_of_its_own() {
    let d = dir("between");
    // a pass keeps the versions current a second ago, so the first leaves
    // maintenance pending
    let s = store(&d).await;
    twenty(&s).await;
    let mut m = s.maintenance(SCOPE).await.unwrap();
    assert!(s.delete(SCOPE, "sessions/07.json").await.unwrap());
    m.wait_for(|m| m.passes >= 1).await.unwrap();
    assert!(s.delete(SCOPE, "sessions/08.json").await.unwrap());
    m.wait_for(|m| !m.pending).await.unwrap();
    assert!(m.borrow().passes >= 2);
    assert!(!holds(&d, word(7).as_bytes()) && !holds(&d, word(8).as_bytes()));
    assert!(holds(&d, word(9).as_bytes()));
}

#[tokio::test]
async fn maintenance_a_restart_cut_off_is_finished_after_it() {
    let d = dir("restart-purge");
    {
        // maintenance an hour off: the service stops first
        let s = store_with(&d, Timing { after: Duration::from_secs(3600), keep: Duration::ZERO }).await;
        twenty(&s).await;
        assert!(s.delete(SCOPE, "sessions/07.json").await.unwrap());
    }
    assert!(holds(&d, word(7).as_bytes()));
    let s = store(&d).await;
    s.swept().await;
    let mut m = s.maintenance(SCOPE).await.expect("the sweep opened every table");
    m.wait_for(|m| !m.pending).await.unwrap();
    assert!(!holds(&d, word(7).as_bytes()), "no request needed");
    assert!(holds(&d, word(8).as_bytes()));
}

#[tokio::test]
async fn a_failed_pass_is_retried() {
    let d = dir("retry");
    let s = store(&d).await;
    twenty(&s).await;
    let mut m = s.maintenance(SCOPE).await.unwrap();
    read_only(&d, &format!("{SCOPE}.lance/data"), async {
        assert!(s.delete(SCOPE, "sessions/07.json").await.unwrap());
        m.wait_for(|m| m.failures >= 1).await.unwrap();
    })
    .await;
    m.wait_for(|m| !m.pending).await.unwrap();
    assert!(!holds(&d, word(7).as_bytes()));
}

#[tokio::test]
async fn an_ended_scope_is_never_written_again() {
    let d = dir("ended");
    {
        let s = store(&d).await;
        s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await.unwrap();
        assert!(s.end(SCOPE).await.unwrap());
        assert!(!s.end(SCOPE).await.unwrap(), "ending it again ends nothing more");
        // a write dispatched before the end, arriving after it
        assert!(matches!(s.upsert(SCOPE, up("sessions/a.json", "r2", harbour())).await, Err(Error::Ended)));
        assert_eq!(s.status(SCOPE).await.unwrap()["exists"], false);
        assert!(!s.delete(SCOPE, "sessions/a.json").await.unwrap());
        // a scope ended before it was ever written
        assert!(!s.end("f-never-1-members").await.unwrap());
        assert!(matches!(s.upsert("f-never-1-members", up("sessions/a.json", "r1", harbour())).await, Err(Error::Ended)));
    }
    let s = store(&d).await;
    assert!(matches!(s.upsert(SCOPE, up("sessions/a.json", "r1", harbour())).await, Err(Error::Ended)), "after a restart too");
    assert!(!holds(&d, b"lantern"), "its table is gone");
    // a drop (a new space) is not an end: the scope is written again
    s.upsert("f-other-1-members", up("sessions/a.json", "r1", harbour())).await.unwrap();
    assert!(s.drop_table("f-other-1-members").await.unwrap());
    assert!(s.upsert("f-other-1-members", up("sessions/a.json", "r1", harbour())).await.unwrap().written);
}
