//! The service on a directory of its own: what is written is searched,
//! once; a replay writes nothing; a stale write is refused; a space is
//! pinned; a restart reads what was written; what is deleted leaves the
//! store's files; and every call but /healthz needs the token.

use base64::Engine;
use fragment_search::api::{Search, Upsert};
use fragment_search::store::{Error, Store};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SCOPE: &str = "f0123-1-members";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fragment-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn store(d: &Path) -> Arc<Store> {
    Arc::new(Store::open(d.to_str().unwrap(), vec![]).await.unwrap())
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
    assert_eq!(call("DELETE", "/v1/f-a", auth, None).await.1["dropped"], true);
}

/// The same on an S3-compatible store (R2, or MinIO on a machine of one's
/// own): `SEARCH_TEST_STORAGE=s3://<bucket>/<prefix>` and its options as
/// `SEARCH_TEST_STORAGE_<OPTION>`. A write is visible, a restart reads it,
/// and a delete and a purge take it from the next search.
#[tokio::test]
#[ignore = "needs an S3-compatible store: SEARCH_TEST_STORAGE"]
async fn on_an_object_store() {
    let uri = std::env::var("SEARCH_TEST_STORAGE").expect("SEARCH_TEST_STORAGE");
    let options: Vec<(String, String)> = std::env::vars().filter_map(|(k, v)| k.strip_prefix("SEARCH_TEST_STORAGE_").map(|o| (o.to_lowercase(), v))).collect();
    let open = || async { Arc::new(Store::open(&uri, options.clone()).await.unwrap()) };
    let scope = format!("f-s3-{}", std::process::id());
    {
        let s = open().await;
        let w = s.upsert(&scope, up("sessions/a.json", "r1", harbour())).await.unwrap();
        assert!(w.written);
        let hits = s.search(&scope, search(json!({ "q": "lantern", "vector": b64(&[1.0, 0.0, 0.0, 0.0]), "space": space() }))).await.unwrap();
        assert_eq!(hits[0].section, "t0");
    }
    let s = open().await;
    assert_eq!(s.status(&scope).await.unwrap()["chunks"], 3, "a restart reads what was written");
    let mut next = up("sessions/a.json", "r2", json!([{ "id": "t0", "chunks": [chunk("only this now", Some([0.5, 0.5, 0.0, 0.0]))] }]));
    next.replaces = Some("r1".into());
    assert!(s.upsert(&scope, next).await.unwrap().written);
    assert!(s.delete(&scope, "sessions/a.json").await.unwrap());
    s.purge(&scope).await.unwrap();
    assert!(s.search(&scope, search(json!({ "q": "only" }))).await.unwrap().is_empty());
    assert!(s.drop_table(&scope).await.unwrap());
}
