//! A fragment life's search index (docs/api.md, "A fragment's search"):
//! one `SearchIndex` Durable Object a life, named by its scope
//! (`fragment_core::searchable::scope`) and called by that fragment's
//! supervisor alone (searchable.rs, through `routed::send`), which decides
//! who reads and writes it, embeds, and forgets. This holds the rows and
//! answers: each source's sections in chunks, their words (FTS5, by
//! field) and vectors (`fragment_core::search_index`), and a search's two
//! legs, fused.
//!
//! An object of its own, not the supervisor's tables (FIN-310): a search
//! is synchronous work (about 5 ms at 10k chunks, 65 ms at 100k), which
//! here holds up only other searches of the same index, never its app;
//! its storage is its own; and its life's end is one `deleteAll`.
//!
//! A write is one transaction, so a search sees one revision of a source
//! or the other. Each index holds the vectors of one space (the
//! deployment's embeddings): a write in another clears it first, and its
//! sources are written again by their apps (the supervisor logs it). The
//! meaning leg reads every chunk's sign code from memory (`Resident`),
//! read again after a write.
//!
//! **Its end** (`end`: the life ended, ended.rs) releases its storage
//! whole, then marks it ended, so a write sent before the end that lands
//! after it writes nothing.
//!
//! **Deleted text** leaves every answer in its write's transaction, and the
//! words index's segments at the next fold (the alarm, `FOLD_AFTER_MS`
//! after a removal: FTS5 `optimize`). SQLite's freed pages keep its bytes
//! until reused (DO SQLite refuses `secure_delete` and `VACUUM`), and
//! Cloudflare's point-in-time recovery keeps 30 days of every object's
//! history: Paul's call, FIN-310 decision 4.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use base64::Engine;
use fragment_core::search_index::{self as engine, Found, Fused};
use fragment_proto::search as wire;
use fragment_proto::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;

/// Chunks an index holds before the status says to split it into child
/// fragments (FIN-310: a search stays under about 15 ms warm).
pub(crate) const CHUNKS_SPLIT: u64 = 25_000;
/// Chunks an index holds at most: a write past it is refused (507).
pub(crate) const CHUNKS_MAX: u64 = 50_000;
/// Sign codes an isolate holds across its indexes, at most: several
/// objects share an isolate's 128 MB (50k chunks of 1024 dimensions are
/// 6.4 MB).
const RESIDENT_MAX_BYTES: usize = 32 << 20;
/// How long after a source's removal the alarm folds the words index.
const FOLD_AFTER_MS: i64 = 60 * 60 * 1000;
/// Bound parameters per statement: DO SQLite's limit is 100.
const BINDS_MAX: usize = 99;
/// Test fleets: how long a held write waits at most (`index/test`).
const HELD_MAX_MS: u64 = 60_000;

/// A source, whole, as the supervisor sends it: its chunks embedded
/// (`vector`, base64 of a held vector's code then its int8, or none:
/// a hash this index holds, or a deployment that embeds nothing).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Write {
    pub source: String,
    pub revision: String,
    pub replaces: Option<String>,
    pub title: String,
    pub at: Option<String>,
    pub fields: BTreeMap<String, String>,
    /// The space its vectors are in; none, the deployment embeds nothing.
    pub space: Option<wire::Space>,
    pub sections: Vec<WriteSection>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteSection {
    pub id: String,
    pub heading: Option<String>,
    pub context: Vec<String>,
    pub at: Option<String>,
    pub location: Option<wire::Location>,
    pub chunks: Vec<WriteChunk>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteChunk {
    pub body: String,
    pub hash: String,
    pub vector: Option<String>,
}

/// A held vector as a write carries it.
pub(crate) fn held_b64(h: &engine::Held) -> String {
    let bytes: Vec<u8> = h.code.iter().copied().chain(h.q.iter().map(|x| *x as u8)).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// What a write came to.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Wrote {
    /// Written (`written` false: that revision was its already), in
    /// `chunks`; `cleared`, the index held another space's and was
    /// emptied first.
    Written { written: bool, chunks: usize, cleared: bool },
    /// `replaces` is not the source's revision now. Here and below,
    /// `cleared` as above: a clear comes first, so a refused write may
    /// follow one, and its writer is told so it writes every source again.
    Conflict {
        current: Option<String>,
        #[serde(default)]
        cleared: bool,
    },
    /// A chunk sent without its vector is one the index no longer holds
    /// (a write in between took it): sent again, embedded.
    MissingVectors {
        #[serde(default)]
        cleared: bool,
    },
    /// Past `max` (`CHUNKS_MAX`, or a test fleet's lower), with `chunks`
    /// held.
    Full {
        chunks: u64,
        max: u64,
        #[serde(default)]
        cleared: bool,
    },
    /// Its life ended: nothing is written.
    Ended,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Known {
    pub space: Option<wire::Space>,
    pub hashes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ask {
    pub q: String,
    pub limit: usize,
    /// `hybrid`, `lexical` or `semantic`, as the supervisor resolved it.
    pub mode: wire::Mode,
    pub fields: BTreeMap<String, String>,
    pub after: Option<String>,
    pub before: Option<String>,
    /// The question's float32 vector, base64, in `space`.
    pub vector: Option<String>,
    pub space: Option<wire::Space>,
}

/// A search's answer: its hits, and whether the meaning leg ran.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Answer {
    pub results: Vec<wire::Hit>,
    pub semantic: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub after: String,
    pub limit: usize,
}

/// An index's sign codes, as the meaning leg reads them: each chunk's id,
/// and its code (`width` bytes) at the same place.
struct Codes {
    ids: Vec<i64>,
    codes: Vec<u8>,
    width: usize,
}

impl Codes {
    fn bytes(&self) -> usize {
        self.ids.len() * 8 + self.codes.len()
    }
}

/// The codes this isolate holds, by object, least recently used first
/// out past `RESIDENT_MAX_BYTES`.
#[derive(Default)]
struct Resident {
    by: HashMap<String, (Rc<Codes>, u64)>,
    bytes: usize,
    tick: u64,
}

impl Resident {
    fn get(&mut self, key: &str) -> Option<Rc<Codes>> {
        self.tick += 1;
        let tick = self.tick;
        self.by.get_mut(key).map(|(c, used)| {
            *used = tick;
            Rc::clone(c)
        })
    }

    fn put(&mut self, key: &str, codes: Rc<Codes>) {
        self.forget(key);
        let size = codes.bytes();
        if size > RESIDENT_MAX_BYTES {
            return; // read again at each question
        }
        // bounded: each pass takes one entry out
        while self.bytes + size > RESIDENT_MAX_BYTES {
            let oldest = self.by.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone()).expect("held bytes are some object's");
            self.forget(&oldest);
        }
        self.tick += 1;
        self.bytes += size;
        self.by.insert(key.to_string(), (codes, self.tick));
    }

    fn forget(&mut self, key: &str) {
        if let Some((c, _)) = self.by.remove(key) {
            self.bytes -= c.bytes();
        }
    }
}

thread_local! {
    static RESIDENT: RefCell<Resident> = RefCell::default();
}

#[durable_object]
pub struct SearchIndexCell {
    state: State,
    raw: JsValue,
    /// The isolate's settings (config.rs: built once per isolate).
    cfg: &'static Config,
    /// Its schema as it started (schema.rs): ready, or refused.
    schema: crate::schema::Schema,
    /// Test fleets: writes wait before they are written while this holds
    /// (`index/test`), each counted in `held`: a write sent before its
    /// life's end that lands after it.
    holding: std::cell::Cell<bool>,
    held: std::cell::Cell<u32>,
}

impl DurableObject for SearchIndexCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let schema = crate::schema::Schema::start(&raw, state.storage().sql(), &fragment_core::migrate::SEARCH_INDEX, &state.id().to_string());
        let cfg = Config::from_env(&env);
        SearchIndexCell { state, raw, cfg, schema, holding: Default::default(), held: Default::default() }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        if let Some(refused) = crate::routed::refused(&req, &self.cfg.deploy_id)? {
            return Ok(refused);
        }
        if let Some(probed) = crate::routed::probed(&req, &self.schema)? {
            return Ok(probed);
        }
        match self.route(req).await {
            Ok(answer) => Response::from_json(&answer),
            Err(e) => e.response(),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if self.schema.refused_alarm(&self.state, None).await? {
            return Response::ok("");
        }
        // the words of chunks removed since leave its segments
        let parts = self.parts();
        let folded = parts.exec("INSERT INTO chunk_words (chunk_words) VALUES ('optimize')", vec![]).and_then(|()| parts.exec("DELETE FROM meta WHERE key = 'removed'", vec![]));
        if let Err(e) = folded {
            console_error!("{}", json!({ "event": "search.fold-failed", "error": e.message }));
            let at = js::now_ms() + FOLD_AFTER_MS;
            self.state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(at as f64)))).await?;
        }
        Response::ok("")
    }
}

impl SearchIndexCell {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn key(&self) -> String {
        self.state.id().to_string()
    }

    async fn route(&self, mut req: Request) -> CellResult<Value> {
        self.schema.ready()?;
        let path = req.path();
        let op = path.strip_prefix("/index/").ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no route {path}")))?.to_string();
        let body: Value = req.json().await.map_err(|e| CellError::invalid(format!("a search index's call is JSON: {e}")))?;
        let parse = |what: &str, e: serde_json::Error| CellError::invalid(format!("index/{what}: {e}"));
        match op.as_str() {
            "upsert" => {
                let w: Write = serde_json::from_value(body).map_err(|e| parse("upsert", e))?;
                if self.cfg.test_hooks && self.holding.get() {
                    self.held.set(self.held.get() + 1);
                    let mut waited = 0;
                    while self.holding.get() && waited < HELD_MAX_MS {
                        Delay::from(std::time::Duration::from_millis(20)).await;
                        waited += 20;
                    }
                }
                let max = self.chunks_max()?;
                let me = self.parts();
                let wrote = js::transaction_sync(&self.raw, move || me.upsert(w, max))?;
                // a refusal may follow a clear (another space's), which wrote
                if !matches!(wrote, Wrote::Ended | Wrote::Written { written: false, cleared: false, .. }) {
                    self.after_write().await?;
                }
                Ok(serde_json::to_value(wrote).expect("an outcome serializes"))
            }
            "delete" => {
                let source = body["source"].as_str().ok_or_else(|| CellError::invalid("index/delete names its source"))?.to_string();
                let me = self.parts();
                let deleted = js::transaction_sync(&self.raw, move || me.remove(&source))? > 0;
                if deleted {
                    self.after_write().await?;
                }
                Ok(json!({ "deleted": deleted }))
            }
            "known" => {
                let k: Known = serde_json::from_value(body).map_err(|e| parse("known", e))?;
                Ok(json!({ "known": self.parts().known(&k)? }))
            }
            "sources" => {
                let p: Page = serde_json::from_value(body).map_err(|e| parse("sources", e))?;
                self.parts().sources(&p)
            }
            "status" => self.parts().status(self.chunks_max()?),
            "search" => {
                let a: Ask = serde_json::from_value(body).map_err(|e| parse("search", e))?;
                Ok(serde_json::to_value(self.parts().search(&a)?).expect("an answer serializes"))
            }
            "end" => self.end().await,
            "test" if self.cfg.test_hooks => {
                if let Some(hold) = body["hold"].as_bool() {
                    self.holding.set(hold);
                }
                match &body["chunks_max"] {
                    Value::Null => {}
                    Value::Number(n) if n.as_u64().is_some_and(|n| (1..=CHUNKS_MAX).contains(&n)) => self.parts().set_meta("test_chunks_max", &n.to_string())?,
                    _ => return Err(CellError::invalid(format!("chunks_max is 1 to {CHUNKS_MAX}"))),
                }
                let mut status = self.parts().status(self.chunks_max()?)?;
                status["held"] = json!(self.held.get());
                Ok(status)
            }
            _ => Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))),
        }
    }

    /// The chunks it holds at most: `CHUNKS_MAX`, or on a test fleet one
    /// its lever lowered (`index/test`), so a run reaches the cap.
    fn chunks_max(&self) -> CellResult<u64> {
        if !self.cfg.test_hooks {
            return Ok(CHUNKS_MAX);
        }
        Ok(self.parts().meta("test_chunks_max")?.and_then(|m| m.parse().ok()).unwrap_or(CHUNKS_MAX))
    }

    /// Its rows, as a transaction's closure holds them (`'static`).
    fn parts(&self) -> Parts {
        Parts { sql: self.sql(), key: self.key() }
    }

    /// A write's codes read again; a removal's words folded later.
    async fn after_write(&self) -> CellResult<()> {
        RESIDENT.with(|r| r.borrow_mut().forget(&self.key()));
        if self.parts().meta("removed")?.is_some() && self.state.storage().get_alarm().await?.is_none() {
            let at = js::now_ms() + FOLD_AFTER_MS;
            self.state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(at as f64)))).await?;
        }
        Ok(())
    }

    /// The life ended: its storage released whole, then marked ended (in
    /// the steps its schema makes again), so nothing writes it after.
    async fn end(&self) -> CellResult<Value> {
        self.state.storage().delete_all().await?;
        self.state.storage().delete_alarm().await?;
        RESIDENT.with(|r| r.borrow_mut().forget(&self.key()));
        self.schema.made_again(&self.raw, self.sql(), &fragment_core::migrate::SEARCH_INDEX, &self.key())?;
        self.parts().set_meta("ended", &js::now_ms().to_string())?;
        Ok(json!({ "ended": true }))
    }
}

/// An index's rows: the synchronous half, which a transaction holds.
struct Parts {
    sql: SqlStorage,
    key: String,
}

fn int(n: usize) -> SqlStorageValue {
    SqlStorageValue::Integer(i64::try_from(n).expect("a count fits i64"))
}

fn text(s: &str) -> SqlStorageValue {
    SqlStorageValue::String(s.to_string())
}

fn opt(s: Option<&str>) -> SqlStorageValue {
    s.map_or(SqlStorageValue::Null, text)
}

impl Parts {
    /// Rows as JSON objects: one reader for every query (each row type
    /// read otherwise is code of its own, and the cell's Wasm is budgeted).
    fn rows(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<Value>> {
        Ok(self.sql.exec(q, binds)?.to_array::<Value>()?)
    }

    /// Each row's first column, an integer.
    fn ids(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<i64>> {
        let mut out = Vec::new();
        for row in self.sql.exec(q, binds)?.raw() {
            if let Some(SqlStorageValue::Integer(id)) = row?.first() {
                out.push(*id);
            }
        }
        Ok(out)
    }

    fn exec(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<()> {
        self.sql.exec(q, binds)?;
        Ok(())
    }

    fn meta(&self, key: &str) -> CellResult<Option<String>> {
        let rows: Vec<Value> = self.rows("SELECT value FROM meta WHERE key = ?", vec![text(key)])?;
        Ok(rows.first().and_then(|r| r["value"].as_str()).map(str::to_string))
    }

    fn set_meta(&self, key: &str, value: &str) -> CellResult<()> {
        self.exec("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value", vec![text(key), text(value)])
    }

    fn count(&self, q: &str) -> CellResult<u64> {
        let rows: Vec<Value> = self.rows(q, vec![])?;
        rows.first().and_then(|r| r["n"].as_u64()).ok_or_else(|| CellError::host(format!("a count answered no number: {q}")))
    }

    /// The space its vectors are in; none while it holds none.
    fn space(&self) -> CellResult<Option<wire::Space>> {
        match self.meta("space")?.filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => serde_json::from_str(&s).map(Some).map_err(|e| CellError::host(format!("an index's space reads: {e}"))),
        }
    }

    fn chunks(&self) -> CellResult<u64> {
        Ok(self.meta("chunks")?.and_then(|c| c.parse().ok()).unwrap_or(0))
    }

    /// Everything it holds, gone (another space's vectors).
    fn clear(&self) -> CellResult<()> {
        for t in ["sources", "sections", "chunks", "vectors"] {
            self.exec(&format!("DELETE FROM {t}"), vec![])?;
        }
        self.exec("INSERT INTO chunk_words (chunk_words) VALUES ('delete-all')", vec![])?;
        self.set_meta("chunks", "0")?;
        self.set_meta("removed", &js::now_ms().to_string())
    }

    /// `source`'s sections, chunks, words and vectors gone, its own row
    /// with them: the chunks it held.
    fn remove(&self, source: &str) -> CellResult<u64> {
        let ids = self.ids("SELECT c.id FROM chunks c JOIN sections s ON s.id = c.section WHERE s.source = ?", vec![text(source)])?;
        let had: Vec<Value> = self.rows("SELECT chunks FROM sources WHERE source = ?", vec![text(source)])?;
        if had.is_empty() {
            return Ok(0);
        }
        for id in &ids {
            self.exec("DELETE FROM chunk_words WHERE rowid = ?", vec![SqlStorageValue::Integer(*id)])?;
        }
        self.exec("DELETE FROM vectors WHERE chunk IN (SELECT c.id FROM chunks c JOIN sections s ON s.id = c.section WHERE s.source = ?)", vec![text(source)])?;
        self.exec("DELETE FROM chunks WHERE section IN (SELECT id FROM sections WHERE source = ?)", vec![text(source)])?;
        self.exec("DELETE FROM sections WHERE source = ?", vec![text(source)])?;
        self.exec("DELETE FROM sources WHERE source = ?", vec![text(source)])?;
        let n = ids.len() as u64;
        self.set_meta("chunks", &self.chunks()?.saturating_sub(n).to_string())?;
        if n > 0 {
            self.set_meta("removed", &js::now_ms().to_string())?;
        }
        // its own row counts too: a source of no chunks was held
        Ok(n.max(1))
    }

    /// The held vectors of `hashes`, by hash, that this index has.
    fn held(&self, hashes: &[&str]) -> CellResult<HashMap<String, Vec<u8>>> {
        let mut out = HashMap::new();
        for part in hashes.chunks(BINDS_MAX) {
            let marks = vec!["?"; part.len()].join(", ");
            let q = format!("SELECT c.hash, v.code, v.q FROM chunks c JOIN vectors v ON v.chunk = c.id WHERE c.hash IN ({marks}) GROUP BY c.hash");
            for row in self.sql.exec(&q, part.iter().map(|h| text(h)).collect::<Vec<_>>())?.raw() {
                if let [SqlStorageValue::String(h), SqlStorageValue::Blob(code), SqlStorageValue::Blob(q)] = row?.as_slice() {
                    out.insert(h.clone(), code.iter().chain(q).copied().collect());
                }
            }
        }
        Ok(out)
    }

    fn upsert(&self, w: Write, max: u64) -> CellResult<Wrote> {
        if self.meta("ended")?.is_some() {
            return Ok(Wrote::Ended);
        }
        let mut cleared = false;
        if self.space()? != w.space {
            if self.count("SELECT COUNT(*) AS n FROM sources")? > 0 {
                self.clear()?;
                cleared = true;
            }
            let space = w.space.as_ref().map(|s| serde_json::to_string(s).expect("a space serializes")).unwrap_or_default();
            self.set_meta("space", &space)?;
        }
        let current: Vec<Value> = self.rows("SELECT revision, chunks FROM sources WHERE source = ?", vec![text(&w.source)])?;
        let current_revision = current.first().and_then(|r| r["revision"].as_str()).map(str::to_string);
        if current_revision.as_deref() == Some(w.revision.as_str()) {
            return Ok(Wrote::Written { written: false, chunks: 0, cleared });
        }
        if let Some(r) = &w.replaces {
            let matches = if r.is_empty() { current_revision.is_none() } else { current_revision.as_deref() == Some(r.as_str()) };
            if !matches {
                return Ok(Wrote::Conflict { current: current_revision, cleared });
            }
        }
        let new: usize = w.sections.iter().map(|s| s.chunks.len()).sum();
        let old = current.first().and_then(|r| r["chunks"].as_u64()).unwrap_or(0);
        let held = self.chunks()?;
        if !engine::fits(held, old, new as u64, max) {
            return Ok(Wrote::Full { chunks: held, max, cleared });
        }
        let total = held.saturating_sub(old) + new as u64;
        // each vector, held: its own, or this index's of its hash (read
        // before the source's old chunks go, which may hold it)
        let dims = w.space.as_ref().map(|s| s.dimensions as usize);
        let code_len = dims.map_or(0, engine::code_bytes);
        let width = dims.map(|d| code_len + d);
        let mut vectors: HashMap<String, Vec<u8>> = HashMap::new();
        if let Some(width) = width {
            let mut missing: Vec<&str> = Vec::new();
            for c in w.sections.iter().flat_map(|s| &s.chunks) {
                match &c.vector {
                    Some(b64) => {
                        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| CellError::invalid(format!("a held vector is base64: {e}")))?;
                        if bytes.len() != width {
                            return Err(CellError::invalid(format!("a held vector is {width} bytes in this space, not {}", bytes.len())));
                        }
                        vectors.insert(c.hash.clone(), bytes);
                    }
                    None if !vectors.contains_key(&c.hash) => missing.push(&c.hash),
                    None => {}
                }
            }
            missing.sort_unstable();
            missing.dedup();
            let found = if cleared { HashMap::new() } else { self.held(&missing)? };
            if missing.iter().any(|h| !found.contains_key(*h) && !vectors.contains_key(*h)) {
                return Ok(Wrote::MissingVectors { cleared });
            }
            vectors.extend(found);
        }
        self.remove(&w.source)?;
        let fields = serde_json::to_string(&w.fields).expect("fields serialize");
        self.exec(
            "INSERT INTO sources (source, revision, title, at, fields, chunks) VALUES (?, ?, ?, ?, ?, ?)",
            vec![text(&w.source), text(&w.revision), text(&w.title), opt(w.at.as_deref()), text(&fields), int(new)],
        )?;
        for s in &w.sections {
            let context = serde_json::to_string(&s.context).expect("context serializes");
            let location = s.location.as_ref().map(|l| serde_json::to_string(l).expect("a location serializes"));
            let at = s.at.as_deref().or(w.at.as_deref());
            let made: Vec<Value> = self.rows(
                "INSERT INTO sections (source, section, heading, context, at, location) VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
                vec![text(&w.source), text(&s.id), opt(s.heading.as_deref()), text(&context), opt(at), opt(location.as_deref())],
            )?;
            let section = made.first().and_then(|r| r["id"].as_i64()).ok_or_else(|| CellError::host("a section's insert answered no id"))?;
            for c in &s.chunks {
                let made: Vec<Value> = self.rows("INSERT INTO chunks (section, hash, body) VALUES (?, ?, ?) RETURNING id", vec![SqlStorageValue::Integer(section), text(&c.hash), text(&c.body)])?;
                let chunk = made.first().and_then(|r| r["id"].as_i64()).ok_or_else(|| CellError::host("a chunk's insert answered no id"))?;
                self.exec(
                    "INSERT INTO chunk_words (rowid, title, heading, context, body) VALUES (?, ?, ?, ?, ?)",
                    vec![SqlStorageValue::Integer(chunk), text(&w.title), opt(s.heading.as_deref()), text(&s.context.join("\n")), text(&c.body)],
                )?;
                if let Some(v) = vectors.get(&c.hash) {
                    let (code, q) = v.split_at(code_len);
                    self.exec("INSERT INTO vectors (chunk, code, q) VALUES (?, ?, ?)", vec![SqlStorageValue::Integer(chunk), SqlStorageValue::Blob(code.to_vec()), SqlStorageValue::Blob(q.to_vec())])?;
                }
            }
        }
        self.set_meta("chunks", &total.to_string())?;
        Ok(Wrote::Written { written: true, chunks: new, cleared })
    }

    /// Which of `hashes` it holds a vector of, in `space`.
    fn known(&self, k: &Known) -> CellResult<Vec<String>> {
        if k.space.is_none() || self.space()? != k.space {
            return Ok(vec![]);
        }
        let hashes: Vec<&str> = k.hashes.iter().map(String::as_str).collect();
        let mut known: Vec<String> = self.held(&hashes)?.into_keys().collect();
        known.sort_unstable();
        Ok(known)
    }

    fn sources(&self, p: &Page) -> CellResult<Value> {
        if !(1..=wire::SOURCES_PAGE_MAX).contains(&p.limit) {
            return Err(CellError::invalid(format!("limit is 1 to {}", wire::SOURCES_PAGE_MAX)));
        }
        let mut page: Vec<Value> = self.rows("SELECT source, revision, at, chunks FROM sources WHERE source > ? ORDER BY source LIMIT ?", vec![text(&p.after), int(p.limit + 1)])?;
        let more = page.len() > p.limit;
        page.truncate(p.limit);
        for s in &mut page {
            if s["at"].is_null() {
                s.as_object_mut().expect("a row").remove("at");
            }
        }
        let next = more.then(|| page.last().and_then(|s| s["source"].as_str()).map(str::to_string)).flatten();
        Ok(json!({ "sources": page, "next": next }))
    }

    fn status(&self, max: u64) -> CellResult<Value> {
        let sources = self.count("SELECT COUNT(*) AS n FROM sources")?;
        let chunks = self.chunks()?;
        Ok(json!({
            "exists": sources > 0,
            "ended": self.meta("ended")?.is_some(),
            "sources": sources,
            "chunks": chunks,
            "bytes": self.sql.database_size(),
            "space": self.space()?,
            "chunks_split": CHUNKS_SPLIT,
            "chunks_max": max,
        }))
    }

    /// The filter a question's fields and times add to a search of
    /// `sections s` and `sources src`, and its binds.
    fn filtered(a: &Ask) -> (String, Vec<SqlStorageValue>) {
        let mut sql = String::new();
        let mut binds = Vec::new();
        for (k, v) in &a.fields {
            // a field's name is a-z, 0-9 and _ (searchable::check_fields)
            sql.push_str(" AND json_extract(src.fields, ?) = ?");
            binds.extend([text(&format!("$.{k}")), text(v)]);
        }
        if let Some(t) = &a.after {
            sql.push_str(" AND s.at >= ?");
            binds.push(text(t));
        }
        if let Some(t) = &a.before {
            sql.push_str(" AND s.at < ?");
            binds.push(text(t));
        }
        (sql, binds)
    }

    /// The words leg: the best `DEPTH` chunks by BM25 over their fields
    /// (title 4, heading 3, context 2, body 1).
    fn lexical(&self, a: &Ask, words: &str) -> CellResult<Vec<Found>> {
        let (filter, mut binds) = Self::filtered(a);
        binds.insert(0, text(words));
        binds.push(int(engine::DEPTH));
        let q = format!(
            "SELECT chunk_words.rowid AS chunk, c.section AS section, -bm25(chunk_words, 4.0, 3.0, 2.0, 1.0) AS score
             FROM chunk_words JOIN chunks c ON c.id = chunk_words.rowid JOIN sections s ON s.id = c.section JOIN sources src ON src.source = s.source
             WHERE chunk_words MATCH ?{filter} ORDER BY score DESC, chunk LIMIT ?"
        );
        let mut found = Vec::new();
        for row in self.sql.exec(&q, binds)?.raw() {
            if let [SqlStorageValue::Integer(chunk), SqlStorageValue::Integer(section), score] = row?.as_slice() {
                let score = match score {
                    SqlStorageValue::Float(f) => *f,
                    SqlStorageValue::Integer(i) => *i as f64,
                    _ => return Err(CellError::host("a chunk's BM25 is a number")),
                };
                found.push(Found { section: *section, chunk: *chunk, score });
            }
        }
        Ok(found)
    }

    /// Its sign codes: held, or read and held.
    fn codes(&self, width: usize) -> CellResult<Rc<Codes>> {
        if let Some(c) = RESIDENT.with(|r| r.borrow_mut().get(&self.key)).filter(|c| c.width == width) {
            return Ok(c);
        }
        let mut codes = Codes { ids: Vec::new(), codes: Vec::new(), width };
        for row in self.sql.exec("SELECT chunk, code FROM vectors ORDER BY chunk", None)?.raw() {
            match row?.as_slice() {
                [SqlStorageValue::Integer(id), SqlStorageValue::Blob(code)] if code.len() == width => {
                    codes.ids.push(*id);
                    codes.codes.extend_from_slice(code);
                }
                _ => return Err(CellError::host(format!("a vector's code is not {width} bytes"))),
            }
        }
        let codes = Rc::new(codes);
        RESIDENT.with(|r| r.borrow_mut().put(&self.key, Rc::clone(&codes)));
        Ok(codes)
    }

    /// The meaning leg: every chunk's sign code against the question, the
    /// best `candidates` reranked by int8, the best `DEPTH` of those.
    fn semantic(&self, a: &Ask, question: &[f32], dims: usize) -> CellResult<Vec<Found>> {
        let Some(unit) = engine::unit(question) else { return Ok(vec![]) };
        let held = engine::hold(&unit).expect("a unit vector is held");
        let codes = self.codes(engine::code_bytes(dims))?;
        let (filter, binds) = Self::filtered(a);
        let allowed: Option<HashSet<i64>> = if filter.is_empty() {
            None
        } else {
            let q = format!("SELECT c.id FROM chunks c JOIN sections s ON s.id = c.section JOIN sources src ON src.source = s.source WHERE 1{filter}");
            Some(self.ids(&q, binds)?.into_iter().collect())
        };
        let table = engine::Table::of(&unit);
        let scored: Vec<(i64, f32)> = codes
            .ids
            .iter()
            .zip(codes.codes.chunks_exact(codes.width))
            .filter(|(id, _)| allowed.as_ref().is_none_or(|a| a.contains(id)))
            .map(|(id, code)| (*id, table.score(code)))
            .collect();
        let first = engine::best(scored, engine::candidates(allowed.as_ref().map_or(codes.ids.len(), HashSet::len)));
        let mut reranked: Vec<(i64, f32)> = Vec::with_capacity(first.len());
        let mut sections: HashMap<i64, i64> = HashMap::new();
        for part in first.chunks(BINDS_MAX) {
            let marks = vec!["?"; part.len()].join(", ");
            let q = format!("SELECT v.chunk, c.section, v.q FROM vectors v JOIN chunks c ON c.id = v.chunk WHERE v.chunk IN ({marks})");
            for row in self.sql.exec(&q, part.iter().map(|(id, _)| SqlStorageValue::Integer(*id)).collect::<Vec<_>>())?.raw() {
                if let [SqlStorageValue::Integer(chunk), SqlStorageValue::Integer(section), SqlStorageValue::Blob(q)] = row?.as_slice() {
                    let q: Vec<i8> = q.iter().map(|b| *b as i8).collect();
                    if q.len() == dims {
                        reranked.push((*chunk, engine::dot(&held.q, &q) as f32));
                        sections.insert(*chunk, *section);
                    }
                }
            }
        }
        Ok(engine::best(reranked, engine::DEPTH)
            .into_iter()
            .map(|(chunk, dot)| Found { section: sections[&chunk], chunk, score: engine::cosine(dot as i32) })
            .collect())
    }

    fn search(&self, a: &Ask) -> CellResult<Answer> {
        if !(1..=wire::RESULTS_MAX).contains(&a.limit) {
            return Err(CellError::invalid(format!("limit is 1 to {}", wire::RESULTS_MAX)));
        }
        let lexical = match (a.mode, engine::match_of(&a.q)) {
            (wire::Mode::Semantic, _) | (_, None) => vec![],
            (_, Some(words)) => self.lexical(a, &words)?,
        };
        // the meaning leg runs on a question of this index's space
        let mut semantic = None;
        if let (false, Some(b64), Some(space)) = (a.mode == wire::Mode::Lexical, &a.vector, &a.space) {
            if self.space()?.as_ref() == Some(space) {
                let question = engine::vector_of(b64, space.dimensions).map_err(CellError::invalid)?;
                semantic = Some(self.semantic(a, &question, space.dimensions as usize)?);
            }
        }
        let fused = engine::fuse(&lexical, semantic.as_deref().unwrap_or_default(), a.limit);
        Ok(Answer { results: self.shown(&fused, &a.q)?, semantic: semantic.is_some() })
    }

    /// Each fused section as a hit, shown by its chunk.
    fn shown(&self, fused: &[Fused], q: &str) -> CellResult<Vec<wire::Hit>> {
        if fused.is_empty() {
            return Ok(vec![]);
        }
        let marks = vec!["?"; fused.len()].join(", ");
        let rows = self.rows(
            &format!(
                "SELECT c.id, c.body, s.section, s.source, s.heading, s.context, s.at, s.location, src.revision, src.title
                 FROM chunks c JOIN sections s ON s.id = c.section JOIN sources src ON src.source = s.source WHERE c.id IN ({marks})"
            ),
            fused.iter().map(|f| SqlStorageValue::Integer(f.chunk)).collect(),
        )?;
        let by: HashMap<i64, Value> = rows.into_iter().filter_map(|r| r["id"].as_i64().map(|id| (id, r))).collect();
        let string = |r: &Value, k: &str| r[k].as_str().unwrap_or_default().to_string();
        let some = |r: &Value, k: &str| r[k].as_str().map(str::to_string);
        Ok(fused
            .iter()
            .filter_map(|f| by.get(&f.chunk).map(|r| (f, r)))
            .enumerate()
            .map(|(i, (f, r))| wire::Hit {
                rank: i + 1,
                source: string(r, "source"),
                section: string(r, "section"),
                revision: string(r, "revision"),
                title: string(r, "title"),
                heading: some(r, "heading"),
                context: serde_json::from_str(r["context"].as_str().unwrap_or("[]")).unwrap_or_default(),
                at: some(r, "at"),
                location: r["location"].as_str().and_then(|l| serde_json::from_str(l).ok()),
                snippet: engine::snippet(r["body"].as_str().unwrap_or_default(), q),
                score: f.score,
                signals: [f.lexical.map(|_| "lexical"), f.semantic.map(|_| "semantic")].into_iter().flatten().map(str::to_string).collect(),
                lexical: f.lexical.map(|p| wire::Lexical { rank: p.rank, bm25: p.score }),
                semantic: f.semantic.map(|p| wire::Semantic { rank: p.rank, cosine: p.score }),
            })
            .collect())
    }
}
