//! The tables (README.md): one a scope, LanceDB's, on the store the
//! service was started on. A table is made by its first upsert, its space
//! (`api::Space`, or none: words alone) pinned in its schema for good. Its
//! writes take turns (one service holds a scope: the platform routes a
//! scope to one instance), its reads do not wait. Each upsert replaces its
//! source whole in one commit (`merge_insert`, its old chunks deleted in the
//! same), so a search sees a source's old revision or its new, never part.
//! A source's revision is its own row's (`MARKERS`), so a source of no
//! chunks keeps one. A table is served only once its text is indexed
//! (`index_text`): a first write cut off before that is finished by its
//! retry, or when the table is next opened.
//!
//! What a write removed leaves the store within about three minutes: each
//! pass of `maintain` rewrites every file holding a deleted row, rebuilds
//! the indexes that held one, and prunes the versions that held either,
//! until a single version is left; it retries a pass that failed, and a
//! write while it runs gets another pass. The store itself says what is
//! left to do (more than one version, or a deletion not yet rewritten),
//! so a table a restart left so is finished once the service is up again
//! (`sweep`).

use crate::api::{self, Hit, Invalid, Mode, Space};
use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{
    types::Float32Type, Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema};
use lancedb::index::scalar::{CombinedFieldsQuery, FtsIndexBuilder, FullTextSearchQuery};
use lancedb::index::vector::IvfFlatIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::{CompactionOptions, OptimizeAction, OptimizeOptions};
use lancedb::{Connection, DistanceType, Table};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A source's own row, holding its revision, and its chunks' rows.
const MARKERS: &str = "section = ''";
const CHUNKS: &str = "section != ''";
/// The schema's metadata key its space is pinned under.
const SPACE_KEY: &str = "fragment.space";
/// The text columns BM25F reads as one field, with the brain's weights
/// (bench/search: its per-column match scored far worse).
const TEXT: [(&str, f32); 4] = [("title", 4.0), ("heading", 3.0), ("context", 2.0), ("body", 1.0)];
/// Filter fields' columns: a source's `fields`, each `name=value`.
const FIELD_COLUMNS: [&str; api::FIELDS_MAX] = ["field0", "field1", "field2", "field3"];
/// Chunks each leg of a search brings to the fusion.
const DEPTH: usize = 100;
/// Reciprocal-rank fusion's constant (bench/search's, LanceDB's).
const RRF_K: f64 = 60.0;
const SNIPPET_MAX_BYTES: usize = 300;
/// Writes between optimizations, which fold the newest rows into the
/// indexes (a search reads them before: the unindexed tail).
const OPTIMIZE_EVERY: u32 = 32;
/// A failed pass's retry waits twice as long as the last, at most this.
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
/// A table that fails to open in the sweep after a restart is tried this
/// many times, then left to the next request that opens it.
const SWEEP_TRIES: u32 = 3;
/// Table names the sweep lists at a time.
const SWEEP_PAGE: u32 = 1000;
/// An ended scope's tombstone: a table of no rows named for it, which no
/// scope's name can be (`api::valid_scope` has no dot).
const ENDED: &str = ".ended";
/// A table's vectors get an index (IVF_FLAT, by cosine) past this many
/// chunks; below it a search compares them all (bench/search: exact at 17k
/// chunks in milliseconds).
const VECTOR_INDEX_MIN_ROWS: usize = 100_000;
/// Tables held open at once, at most; the least used idle one is let go.
const HANDLES_MAX: usize = 512;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    /// `replaces` was not the source's revision now.
    Conflict { current: Option<String> },
    /// The table's vectors are in another space (or none).
    SpaceChanged { current: Option<Space> },
    /// Chunks named no vector, and the table has none of their hashes.
    MissingVectors(Vec<String>),
    /// The scope was ended (`end`): it is never written again.
    Ended,
    Full,
    Engine(String),
}

impl From<Invalid> for Error {
    fn from(i: Invalid) -> Error {
        Error::Invalid(i.0)
    }
}

impl From<lancedb::Error> for Error {
    fn from(e: lancedb::Error) -> Error {
        Error::Engine(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

/// When maintenance runs: `after` a write that removed rows (and between
/// its passes), keeping each version younger than `keep` for the reads
/// that may be in flight on it.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub after: Duration,
    pub keep: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { after: Duration::from_secs(75), keep: Duration::from_secs(60) }
    }
}

/// A table's maintenance: `pending` from a write that removed rows until a
/// pass leaves the table clean (`clean`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Maintenance {
    pub pending: bool,
    pub passes: u64,
    pub failures: u64,
}

struct Entry {
    table: Table,
    space: Option<Space>,
    /// Held by a write, and by a pass of maintenance. Its `Maintenance`
    /// changes only while it is held.
    writes: tokio::sync::Mutex<Writes>,
    maintenance: tokio::sync::watch::Sender<Maintenance>,
    used: std::sync::Mutex<Instant>,
}

#[derive(Default)]
struct Writes {
    since_optimize: u32,
    /// A write removed rows since the last pass began: the next rebuilds
    /// the text indexes, which keep a deleted row's words otherwise.
    removed: bool,
    /// The table was dropped: its maintenance ends.
    dropped: bool,
}

pub struct Store {
    db: Connection,
    tables: tokio::sync::Mutex<HashMap<String, Arc<Entry>>>,
    timing: Timing,
    /// Whether the sweep after opening has seen every table.
    swept: tokio::sync::watch::Sender<bool>,
    /// Its tables' maintenance, which ends with it, as at a restart.
    tasks: std::sync::Mutex<tokio::task::JoinSet<()>>,
}

#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Written {
    pub source: String,
    pub revision: String,
    pub written: bool,
    pub chunks: usize,
    pub version: u64,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn schema(space: Option<&Space>) -> Arc<Schema> {
    let text = |name: &str| Field::new(name, DataType::Utf8, false);
    let mut fields = vec![
        text("id"),
        text("source"),
        text("section"),
        Field::new("chunk", DataType::UInt32, false),
        text("revision"),
        text("hash"),
        text("title"),
        text("heading"),
        text("context"),
        text("body"),
        text("at"),
        text("location"),
    ];
    fields.extend(FIELD_COLUMNS.iter().map(|c| text(c)));
    if let Some(s) = space {
        // a source's own row (`MARKERS`) has none
        fields.push(Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), s.dimensions as i32), true));
    }
    let meta = [(SPACE_KEY.to_string(), serde_json::to_string(&space).expect("a space serializes"))].into();
    Arc::new(Schema::new_with_metadata(fields, meta))
}

/// The strings of `batch`'s column `name`.
fn strings<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch.column_by_name(name).and_then(|c| c.as_any().downcast_ref::<StringArray>()).unwrap_or_else(|| panic!("{name} is a selected text column"))
}

/// A row of a search's leg: the section it is in, and what is shown of it.
#[derive(Clone)]
struct Row {
    source: String,
    section: String,
    revision: String,
    title: String,
    heading: String,
    context: String,
    body: String,
    at: String,
    location: String,
}

const ROW_COLUMNS: [&str; 9] = ["source", "section", "revision", "title", "heading", "context", "body", "at", "location"];

fn rows(batches: &[RecordBatch]) -> Vec<Row> {
    let mut out = Vec::new();
    for b in batches {
        let c: Vec<&StringArray> = ROW_COLUMNS.iter().map(|n| strings(b, n)).collect();
        for i in 0..b.num_rows() {
            let v = |k: usize| c[k].value(i).to_string();
            out.push(Row { source: v(0), section: v(1), revision: v(2), title: v(3), heading: v(4), context: v(5), body: v(6), at: v(7), location: v(8) });
        }
    }
    out
}

/// At most `SNIPPET_MAX_BYTES` of `body`, around the first of the
/// question's words it holds (else its start), cut at characters.
pub fn snippet(body: &str, q: &str) -> String {
    let lower = body.to_lowercase();
    let at = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        .filter(|&i| i < body.len() && body.is_char_boundary(i))
        .unwrap_or(0);
    let mut start = at.saturating_sub(SNIPPET_MAX_BYTES / 3);
    while !body.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + SNIPPET_MAX_BYTES).min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(body[start..end].trim());
    if end < body.len() {
        s.push('…');
    }
    s
}

/// The filter a search's `fields`, `after` and `before` make, or none.
fn filter_of(search: &api::Search) -> Option<String> {
    let mut parts: Vec<String> = search
        .fields
        .iter()
        .map(|(k, v)| {
            let want = quote(&format!("{k}={v}"));
            format!("({})", FIELD_COLUMNS.iter().map(|c| format!("{c} = {want}")).collect::<Vec<_>>().join(" OR "))
        })
        .collect();
    if let Some(a) = &search.after {
        parts.push(format!("at >= {}", quote(a)));
    }
    if let Some(b) = &search.before {
        parts.push(format!("at < {}", quote(b)));
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

impl Store {
    /// The service's store: `uri` a directory or an object store's
    /// (`s3://bucket/prefix`), `options` its client's (LanceDB's storage
    /// options: an S3-compatible endpoint, its region and keys).
    /// Its tables are swept once it is open (`sweep`): maintenance a
    /// restart cut off is finished.
    pub async fn open(uri: &str, options: Vec<(String, String)>, timing: Timing) -> Result<Arc<Store>> {
        let mut connect = lancedb::connect(uri);
        for (k, v) in options {
            connect = connect.storage_option(k, v);
        }
        let store = Arc::new(Store { db: connect.execute().await?, tables: Default::default(), timing, swept: tokio::sync::watch::channel(false).0, tasks: Default::default() });
        tokio::spawn(store.clone().sweep());
        Ok(store)
    }

    /// Every table opened (`entry`), so each that a restart left with
    /// maintenance to do gets it.
    async fn sweep(self: Arc<Self>) {
        let mut after: Option<String> = None;
        let mut wait = self.timing.after;
        loop {
            let mut list = self.db.table_names().limit(SWEEP_PAGE);
            if let Some(a) = &after {
                list = list.start_after(a.clone());
            }
            let names = match list.execute().await {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("{}", json!({ "event": "search.sweep-failed", "error": e.to_string() }));
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(RETRY_MAX);
                    continue;
                }
            };
            for name in names.iter().filter(|n| !n.ends_with(ENDED)) {
                for tried in 1..=SWEEP_TRIES {
                    match self.entry(name).await {
                        Ok(_) => break,
                        Err(e) if tried == SWEEP_TRIES => eprintln!("{}", json!({ "event": "search.sweep-failed", "scope": name, "error": format!("{e:?}") })),
                        Err(_) => tokio::time::sleep(self.timing.after * tried).await,
                    }
                }
            }
            if names.len() < SWEEP_PAGE as usize {
                break;
            }
            after = names.last().cloned();
        }
        self.swept.send_replace(true);
    }

    /// Once the sweep after opening has seen every table.
    pub async fn swept(&self) {
        let _ = self.swept.subscribe().wait_for(|s| *s).await;
    }

    /// `scope`'s maintenance, if its table is open.
    pub async fn maintenance(&self, scope: &str) -> Option<tokio::sync::watch::Receiver<Maintenance>> {
        self.tables.lock().await.get(scope).map(|e| e.maintenance.subscribe())
    }

    /// `scope`'s table, if it was made: opened with its text indexed, and
    /// its maintenance begun if it has some left (a restart cut it off).
    async fn entry(self: &Arc<Self>, scope: &str) -> Result<Option<Arc<Entry>>> {
        let mut tables = self.tables.lock().await;
        if let Some(e) = tables.get(scope) {
            *e.used.lock().unwrap() = Instant::now();
            return Ok(Some(e.clone()));
        }
        let table = match self.db.open_table(scope).execute().await {
            Ok(t) => t,
            Err(lancedb::Error::TableNotFound { .. }) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let space = space_of(&table).await?;
        index_text(&table, false).await?;
        let unclean = !clean(&table).await?;
        let e = Arc::new(Entry { table, space, writes: Default::default(), maintenance: Default::default(), used: std::sync::Mutex::new(Instant::now()) });
        if unclean {
            // what it removed is unknown: rebuild its indexes too
            e.writes.lock().await.removed = true;
            self.maintain(scope, &e);
        }
        if tables.len() >= HANDLES_MAX {
            // let go of the least used table no request holds
            let idle = tables.iter().filter(|(_, e)| Arc::strong_count(e) == 1).min_by_key(|(_, e)| *e.used.lock().unwrap()).map(|(k, _)| k.clone());
            if let Some(k) = idle {
                tables.remove(&k);
            }
        }
        tables.insert(scope.to_string(), e.clone());
        Ok(Some(e))
    }

    pub async fn upsert(self: &Arc<Self>, scope: &str, up: api::Upsert) -> Result<Written> {
        up.check()?;
        let entry = match self.entry(scope).await? {
            Some(e) => e,
            None => return self.create(scope, up).await,
        };
        if entry.space != up.space {
            return Err(Error::SpaceChanged { current: entry.space.clone() });
        }
        let mut writes = entry.writes.lock().await;
        if writes.dropped {
            // dropped while this waited: made again, unless it was ended
            drop(writes);
            return Box::pin(self.upsert(scope, up)).await;
        }
        let table = &entry.table;
        let current = revision_of(table, &up.source).await?;
        let version = table.version().await?;
        if current.as_deref() == Some(up.revision.as_str()) {
            return Ok(Written { source: up.source, revision: up.revision, written: false, chunks: 0, version });
        }
        if let Some(r) = &up.replaces {
            let matches = if r.is_empty() { current.is_none() } else { current.as_deref() == Some(r.as_str()) };
            if !matches {
                return Err(Error::Conflict { current });
            }
        }
        let filter = format!("source = {}", quote(&up.source));
        let rows = table.count_rows(None).await?;
        let chunks: usize = up.sections.iter().map(|s| s.chunks.len()).sum();
        if rows + chunks + 1 > api::TABLE_ROWS_MAX {
            return Err(Error::Full);
        }
        let reused = reusable(table, entry.space.as_ref(), &up).await?;
        let batch = batch_of(&up, entry.space.as_ref(), &reused)?;
        let schema = batch.schema();
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None).when_not_matched_insert_all().when_not_matched_by_source_delete(Some(filter));
        merge.execute(Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema))).await?;
        writes.since_optimize += 1;
        if writes.since_optimize >= OPTIMIZE_EVERY {
            table.optimize(OptimizeAction::Index(OptimizeOptions::default())).await?;
            writes.since_optimize = 0;
        }
        if current.is_some() {
            // what the old revision held leaves the store
            writes.removed = true;
            self.maintain(scope, &entry);
        }
        Ok(Written { source: up.source, revision: up.revision, written: true, chunks, version: table.version().await? })
    }

    /// A table made by its first upsert, its space pinned, its text indexed.
    async fn create(self: &Arc<Self>, scope: &str, up: api::Upsert) -> Result<Written> {
        let mut tables = self.tables.lock().await;
        if tables.contains_key(scope) {
            // made meanwhile: write to it
            drop(tables);
            return Box::pin(self.upsert(scope, up)).await;
        }
        if self.ended(scope).await? {
            return Err(Error::Ended);
        }
        if up.replaces.as_deref().is_some_and(|r| !r.is_empty()) {
            return Err(Error::Conflict { current: None });
        }
        let batch = batch_of(&up, up.space.as_ref(), &HashMap::new())?;
        let chunks = batch.num_rows() - 1;
        let table = self.db.create_table(scope, batch).execute().await?;
        // Not acknowledged until indexed: cut off here, its retry or the
        // next open (`entry`) indexes it before it is served.
        index_text(&table, false).await?;
        let version = table.version().await?;
        let entry = Arc::new(Entry { table, space: up.space.clone(), writes: Default::default(), maintenance: Default::default(), used: std::sync::Mutex::new(Instant::now()) });
        tables.insert(scope.to_string(), entry);
        Ok(Written { source: up.source, revision: up.revision, written: true, chunks, version })
    }

    pub async fn delete(self: &Arc<Self>, scope: &str, source: &str) -> Result<bool> {
        if source.is_empty() || source.len() > api::SOURCE_MAX_BYTES {
            return Err(Error::Invalid(format!("source is 1 to {} bytes", api::SOURCE_MAX_BYTES)));
        }
        let Some(entry) = self.entry(scope).await? else { return Ok(false) };
        let mut writes = entry.writes.lock().await;
        if writes.dropped {
            return Ok(false);
        }
        let filter = format!("source = {}", quote(source));
        if entry.table.count_rows(Some(filter.clone())).await? == 0 {
            return Ok(false);
        }
        entry.table.delete(filter.as_str()).await?;
        writes.removed = true;
        self.maintain(scope, &entry);
        Ok(true)
    }

    /// Which of `hashes` the table holds a vector of.
    pub async fn known(self: &Arc<Self>, scope: &str, k: api::Known) -> Result<Vec<String>> {
        k.check()?;
        let Some(entry) = self.entry(scope).await? else { return Ok(vec![]) };
        if entry.space.is_none() || k.hashes.is_empty() {
            return Ok(vec![]);
        }
        let mut found = std::collections::BTreeSet::new();
        for part in k.hashes.chunks(500) {
            let list = part.iter().map(|h| quote(h)).collect::<Vec<_>>().join(", ");
            let s = entry.table.query().only_if(format!("hash IN ({list})")).select(Select::columns(&["hash"])).execute().await?;
            for b in s.try_collect::<Vec<_>>().await? {
                let h = strings(&b, "hash");
                found.extend((0..b.num_rows()).map(|i| h.value(i).to_string()));
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Its sources after `after`, ascending, at most `limit`, and the
    /// cursor after them (none at the end).
    pub async fn sources(self: &Arc<Self>, scope: &str, after: &str, limit: usize) -> Result<(Vec<Value>, Option<String>)> {
        if !(1..=api::SOURCES_PAGE_MAX).contains(&limit) {
            return Err(Error::Invalid(format!("limit is 1 to {}", api::SOURCES_PAGE_MAX)));
        }
        let Some(entry) = self.entry(scope).await? else { return Ok((vec![], None)) };
        let mut q = entry.table.query().select(Select::columns(&["source", "section", "revision", "at"]));
        if !after.is_empty() {
            q = q.only_if(format!("source > {}", quote(after)));
        }
        // each source's revision and time are its own row's (`MARKERS`)
        let mut by: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
        for b in q.execute().await?.try_collect::<Vec<_>>().await? {
            let (s, section, r, a) = (strings(&b, "source"), strings(&b, "section"), strings(&b, "revision"), strings(&b, "at"));
            for i in 0..b.num_rows() {
                let e = by.entry(s.value(i).to_string()).or_default();
                if section.value(i).is_empty() {
                    (e.0, e.1) = (r.value(i).to_string(), a.value(i).to_string());
                } else {
                    e.2 += 1;
                }
            }
        }
        let more = by.len() > limit;
        let page: Vec<Value> = by
            .into_iter()
            .take(limit)
            .map(|(source, (revision, at, chunks))| json!({ "source": source, "revision": revision, "at": (!at.is_empty()).then_some(at), "chunks": chunks }))
            .collect();
        let next = more.then(|| page.last().and_then(|p| p["source"].as_str()).map(str::to_string)).flatten();
        Ok((page, next))
    }

    pub async fn status(self: &Arc<Self>, scope: &str) -> Result<Value> {
        let Some(entry) = self.entry(scope).await? else { return Ok(json!({ "exists": false })) };
        let t = &entry.table;
        let stats = t.stats().await?;
        Ok(json!({
            "exists": true,
            "chunks": t.count_rows(Some(CHUNKS.into())).await?,
            "sources": t.count_rows(Some(MARKERS.into())).await?,
            "bytes": stats.total_bytes,
            "space": entry.space,
            "semantic": if entry.space.is_some() { "on" } else { "off" },
            "version": t.version().await?,
        }))
    }

    pub async fn search(self: &Arc<Self>, scope: &str, s: api::Search) -> Result<Vec<Hit>> {
        s.check()?;
        let Some(entry) = self.entry(scope).await? else { return Ok(vec![]) };
        let t = &entry.table;
        let filter = filter_of(&s);
        let limit = s.limit.unwrap_or(api::RESULTS_DEFAULT);
        let mut legs: Vec<(&'static str, Vec<Row>)> = Vec::new();
        if s.mode != Mode::Semantic && !s.q.trim().is_empty() {
            let columns = TEXT.iter().map(|(c, _)| c.to_string()).collect();
            let boosts = TEXT.iter().map(|(_, b)| *b).collect();
            let fts = CombinedFieldsQuery::try_new(s.q.clone(), columns).and_then(|c| c.try_with_boosts(boosts)).map_err(|e| Error::Engine(e.to_string()))?;
            let mut q = t.query().full_text_search(FullTextSearchQuery::new_query(fts.into())).select(Select::columns(&ROW_COLUMNS)).limit(DEPTH);
            if let Some(f) = &filter {
                q = q.only_if(f.clone());
            }
            legs.push(("lexical", rows(&q.execute().await?.try_collect::<Vec<_>>().await?)));
        }
        if s.mode != Mode::Lexical {
            if let (Some(b64), Some(space)) = (&s.vector, &s.space) {
                if entry.space.as_ref() != Some(space) {
                    return Err(Error::SpaceChanged { current: entry.space.clone() });
                }
                let v = api::vector_of(b64, space.dimensions)?;
                // a source's own row has no vector, and is no hit
                let only = filter.as_ref().map_or(CHUNKS.to_string(), |f| format!("{CHUNKS} AND {f}"));
                let q = t.query().nearest_to(v.as_slice())?.distance_type(DistanceType::Cosine).select(Select::columns(&ROW_COLUMNS)).limit(DEPTH).only_if(only);
                legs.push(("semantic", rows(&q.execute().await?.try_collect::<Vec<_>>().await?)));
            }
        }
        Ok(fuse(&legs, &s.q, limit))
    }

    /// `scope`'s table dropped; written again, it is made again (a new
    /// space's).
    pub async fn drop_table(&self, scope: &str) -> Result<bool> {
        let mut tables = self.tables.lock().await;
        self.dropped(&mut tables, scope).await
    }

    /// `scope` ended: its tombstone made, then its table dropped, so no
    /// write makes it again, however late it comes (one sent before the
    /// end, say). Its life is over: a scope is one life of one fragment.
    pub async fn end(&self, scope: &str) -> Result<bool> {
        let mut tables = self.tables.lock().await;
        let tombstone = Arc::new(Schema::new(vec![Field::new("scope", DataType::Utf8, false)]));
        match self.db.create_empty_table(format!("{scope}{ENDED}"), tombstone).execute().await {
            Ok(_) | Err(lancedb::Error::TableAlreadyExists { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        self.dropped(&mut tables, scope).await
    }

    async fn dropped(&self, tables: &mut HashMap<String, Arc<Entry>>, scope: &str) -> Result<bool> {
        if let Some(e) = tables.remove(scope) {
            // a write waiting for it finds it dropped (`upsert`)
            e.writes.lock().await.dropped = true;
            e.maintenance.send_modify(|m| m.pending = false);
        }
        match self.db.drop_table(scope, &[]).await {
            Ok(()) => Ok(true),
            Err(lancedb::Error::TableNotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether `scope` was ended (`end`).
    async fn ended(&self, scope: &str) -> Result<bool> {
        match self.db.open_table(format!("{scope}{ENDED}")).execute().await {
            Ok(_) => Ok(true),
            Err(lancedb::Error::TableNotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Called holding `entry`'s writes, after a write that removed rows:
    /// passes (`pass`) `after` it and after each other until one leaves the
    /// table clean, a failed one retried later and later. A write meanwhile
    /// waits for a pass in progress, and gets one of its own after it.
    fn maintain(self: &Arc<Self>, scope: &str, entry: &Arc<Entry>) {
        if !entry.maintenance.send_if_modified(|m| !std::mem::replace(&mut m.pending, true)) {
            return;
        }
        let (timing, entry, scope) = (self.timing, entry.clone(), scope.to_string());
        let mut tasks = self.tasks.lock().unwrap();
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let mut wait = timing.after;
            loop {
                tokio::time::sleep(wait).await;
                let mut w = entry.writes.lock().await;
                if w.dropped {
                    return;
                }
                let removed = std::mem::take(&mut w.removed);
                match pass(&entry, removed, timing.keep).await {
                    Ok(clean) => {
                        w.since_optimize = 0;
                        entry.maintenance.send_modify(|m| {
                            m.passes += 1;
                            m.pending = !clean;
                        });
                        if clean {
                            return;
                        }
                        wait = timing.after;
                    }
                    Err(e) => {
                        w.removed |= removed;
                        entry.maintenance.send_modify(|m| m.failures += 1);
                        eprintln!("{}", json!({ "event": "search.maintain-failed", "scope": scope, "error": format!("{e:?}") }));
                        wait = (wait * 2).min(RETRY_MAX);
                    }
                }
            }
        });
    }

    /// `scope`'s table maintained now, every version but its newest pruned:
    /// what was deleted from it leaves the store (`maintain` does this on
    /// its own, minutes after a write).
    pub async fn purge(self: &Arc<Self>, scope: &str) -> Result<()> {
        let Some(entry) = self.entry(scope).await? else { return Ok(()) };
        let mut w = entry.writes.lock().await;
        pass(&entry, true, Duration::ZERO).await?;
        w.removed = false;
        w.since_optimize = 0;
        Ok(())
    }
}

/// One pass of maintenance, holding the table's writes: every file that
/// holds a deleted row rewritten without it (Lance's own threshold, 10%
/// of a file, would keep a few deleted rows' bytes for good); the text
/// indexes rebuilt when a write `removed` rows (an index keeps a removed
/// row's words, whatever was rewritten), else the newest rows folded into
/// them; the vectors indexed once there are `VECTOR_INDEX_MIN_ROWS` (that
/// index rebuilt too when rows were removed); and the versions older than
/// `keep` pruned, with every file only they held. Whether it left the
/// table clean (`clean`).
async fn pass(entry: &Entry, removed: bool, keep: Duration) -> Result<bool> {
    let t = &entry.table;
    let every_deletion = CompactionOptions { materialize_deletions: true, materialize_deletions_threshold: 0.0, ..CompactionOptions::default() };
    t.optimize(OptimizeAction::Compact { options: every_deletion, remap_options: None }).await?;
    let vector_index = has_vector_index(t).await?;
    if removed {
        index_text(t, true).await?;
    } else {
        t.optimize(OptimizeAction::Index(OptimizeOptions::default())).await?;
    }
    if entry.space.is_some() && ((removed && vector_index) || (!vector_index && t.count_rows(Some(CHUNKS.into())).await? >= VECTOR_INDEX_MIN_ROWS)) {
        t.create_index(&["vector"], Index::IvfFlat(IvfFlatIndexBuilder::default().distance_type(DistanceType::Cosine))).replace(true).execute().await?;
    }
    let older_than = Some(chrono::TimeDelta::from_std(keep).map_err(|e| Error::Engine(e.to_string()))?);
    t.optimize(OptimizeAction::Prune { older_than, delete_unverified: Some(true), error_if_tagged_old_versions: Some(false) }).await?;
    clean(t).await
}

/// A table with nothing left to maintain: one version, deleting no row.
/// Every write makes a version, and only a pass prunes one.
async fn clean(t: &Table) -> Result<bool> {
    let native = t.as_native().ok_or_else(|| Error::Engine("a table not on this store".into()))?;
    let deletes = native.manifest().await?.fragments.iter().any(|f| f.deletion_file.is_some());
    Ok(!deletes && t.list_versions().await?.len() == 1)
}

/// The table's text indexes, each made that is missing (every one,
/// rebuilt, when `rebuild`).
async fn index_text(t: &Table, rebuild: bool) -> Result<()> {
    let indexed: Vec<String> = t.list_indices().await?.into_iter().flat_map(|i| i.columns).collect();
    for (column, _) in TEXT {
        if rebuild || !indexed.iter().any(|c| c == column) {
            t.create_index(&[column], Index::FTS(FtsIndexBuilder::default())).replace(true).execute().await?;
        }
    }
    Ok(())
}

async fn space_of(table: &Table) -> Result<Option<Space>> {
    let schema = table.schema().await?;
    let pinned = schema.metadata().get(SPACE_KEY).ok_or_else(|| Error::Engine("a table without its space".into()))?;
    serde_json::from_str(pinned).map_err(|e| Error::Engine(format!("its space: {e}")))
}

async fn has_vector_index(table: &Table) -> lancedb::Result<bool> {
    Ok(table.list_indices().await?.iter().any(|i| i.columns.iter().any(|c| c == "vector")))
}

async fn revision_of(table: &Table, source: &str) -> Result<Option<String>> {
    let s = table.query().only_if(format!("source = {} AND {MARKERS}", quote(source))).select(Select::columns(&["revision"])).limit(1).execute().await?;
    let batches = s.try_collect::<Vec<_>>().await?;
    Ok(batches.iter().find(|b| b.num_rows() > 0).map(|b| strings(b, "revision").value(0).to_string()))
}

/// The vectors the table holds of the hashes `up`'s chunks name no vector
/// for; refused, naming them, when it has none of some.
async fn reusable(table: &Table, space: Option<&Space>, up: &api::Upsert) -> Result<HashMap<String, Vec<f32>>> {
    let Some(space) = space else { return Ok(HashMap::new()) };
    let wanted: std::collections::BTreeSet<&str> = up.sections.iter().flat_map(|s| &s.chunks).filter(|c| c.vector.is_none()).map(|c| c.hash.as_str()).collect();
    let mut found = HashMap::new();
    let wanted: Vec<&str> = wanted.into_iter().collect();
    for part in wanted.chunks(500) {
        let list = part.iter().map(|h| quote(h)).collect::<Vec<_>>().join(", ");
        let s = table.query().only_if(format!("hash IN ({list})")).select(Select::columns(&["hash", "vector"])).execute().await?;
        for b in s.try_collect::<Vec<_>>().await? {
            let h = strings(&b, "hash");
            let v = b.column_by_name("vector").and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>()).expect("a vector column");
            for i in 0..b.num_rows() {
                let row = v.value(i);
                let floats = row.as_any().downcast_ref::<Float32Array>().expect("float32s");
                assert_eq!(floats.len(), space.dimensions as usize, "a table's vectors are its space's");
                found.entry(h.value(i).to_string()).or_insert_with(|| floats.values().to_vec());
            }
        }
    }
    let missing: Vec<String> = wanted.iter().filter(|h| !found.contains_key(**h)).map(|h| h.to_string()).collect();
    if !missing.is_empty() {
        return Err(Error::MissingVectors(missing));
    }
    Ok(found)
}

fn batch_of(up: &api::Upsert, space: Option<&Space>, reused: &HashMap<String, Vec<f32>>) -> Result<RecordBatch> {
    let schema = schema(space);
    let mut cols: Vec<Vec<String>> = vec![Vec::new(); 12 + FIELD_COLUMNS.len()];
    let mut chunk_ix: Vec<u32> = Vec::new();
    let mut vectors: Vec<Option<Vec<Option<f32>>>> = Vec::new();
    let fields: Vec<String> = up.fields.iter().map(|(k, v)| format!("{k}={v}")).chain(std::iter::repeat(String::new())).take(FIELD_COLUMNS.len()).collect();
    // The source's own row (`MARKERS`): its revision, whatever its chunks
    // (none, too); no words, no vector, no fields, so no search finds it.
    let marker = [format!("{}\u{1f}", up.source), up.source.clone(), String::new(), String::new(), up.revision.clone(), String::new(), String::new(), String::new(), "[]".into(), String::new(), up.at.clone().unwrap_or_default(), String::new()];
    for (k, v) in marker.into_iter().enumerate() {
        cols[k].push(v);
    }
    for col in &mut cols[12..] {
        col.push(String::new());
    }
    chunk_ix.push(0);
    if space.is_some() {
        vectors.push(None);
    }
    for s in &up.sections {
        for (i, c) in s.chunks.iter().enumerate() {
            let row = [
                format!("{}\u{1f}{}\u{1f}{i}", up.source, s.id),
                up.source.clone(),
                s.id.clone(),
                String::new(), // chunk: its own column
                up.revision.clone(),
                c.hash.clone(),
                up.title.clone().unwrap_or_default(),
                s.heading.clone().unwrap_or_default(),
                serde_json::to_string(&s.context).expect("strings serialize"),
                c.text.clone(),
                s.at.clone().or_else(|| up.at.clone()).unwrap_or_default(),
                s.location.as_ref().map(Value::to_string).unwrap_or_default(),
            ];
            for (k, v) in row.into_iter().enumerate() {
                cols[k].push(v);
            }
            for (k, f) in fields.iter().enumerate() {
                cols[12 + k].push(f.clone());
            }
            chunk_ix.push(i as u32);
            if let Some(space) = space {
                let v = match &c.vector {
                    Some(b64) => api::vector_of(b64, space.dimensions)?,
                    None => reused.get(&c.hash).cloned().ok_or_else(|| Error::MissingVectors(vec![c.hash.clone()]))?,
                };
                vectors.push(Some(v.into_iter().map(Some).collect()));
            }
        }
    }
    let mut arrays: Vec<Arc<dyn Array>> = Vec::new();
    for (k, col) in cols.into_iter().enumerate() {
        if k == 3 {
            arrays.push(Arc::new(UInt32Array::from(chunk_ix.clone())));
        } else {
            arrays.push(Arc::new(StringArray::from(col)));
        }
    }
    if let Some(space) = space {
        arrays.push(Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(vectors, space.dimensions as i32)));
    }
    RecordBatch::try_new(schema, arrays).map_err(|e| Error::Engine(e.to_string()))
}

/// The legs' chunks, each leg's sections ranked by their best chunk, fused
/// by reciprocal rank; each hit shown by its best chunk (a lexical one when
/// the words matched).
fn fuse(legs: &[(&'static str, Vec<Row>)], q: &str, limit: usize) -> Vec<Hit> {
    type Key = (String, String);
    /// A section's fused score, the legs that found it, and its best chunk.
    type Scored = (f64, Vec<&'static str>, Row);
    let mut scored: HashMap<Key, Scored> = HashMap::new();
    for (signal, rows) in legs {
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let key = (row.source.clone(), row.section.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            let rank = seen.len();
            let e = scored.entry(key).or_insert_with(|| (0.0, Vec::new(), row.clone()));
            e.0 += 1.0 / (RRF_K + rank as f64);
            e.1.push(signal);
        }
    }
    let mut all: Vec<(Key, Scored)> = scored.into_iter().collect();
    all.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
    all.into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, ((source, section), (score, signals, row)))| Hit {
            rank: i + 1,
            source,
            section,
            revision: row.revision,
            title: row.title,
            heading: (!row.heading.is_empty()).then_some(row.heading),
            context: serde_json::from_str(&row.context).unwrap_or_default(),
            at: (!row.at.is_empty()).then_some(row.at),
            location: (!row.location.is_empty()).then(|| serde_json::from_str(&row.location).ok()).flatten(),
            snippet: snippet(&row.body, q),
            score,
            signals,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_center_the_words_and_cut_at_characters() {
        let body = format!("{} the harbour lantern audit {}", "é".repeat(200), "x".repeat(400));
        let s = snippet(&body, "lantern");
        assert!(s.contains("lantern") && s.starts_with('…') && s.ends_with('…'));
        assert!(s.len() <= SNIPPET_MAX_BYTES + 2 * '…'.len_utf8());
        assert_eq!(snippet("short body", "nothing"), "short body");
    }

    #[test]
    fn filters_quote_what_they_name() {
        let s: api::Search = serde_json::from_value(json!({ "q": "x", "fields": { "kind": "o'brien" }, "after": "2026-10-01" })).unwrap();
        let f = filter_of(&s).unwrap();
        assert!(f.contains("field0 = 'kind=o''brien'") && f.contains("at >= '2026-10-01'"), "{f}");
    }

    #[test]
    fn fusion_ranks_sections_by_their_best_chunks() {
        let row = |source: &str, section: &str, body: &str| Row {
            source: source.into(),
            section: section.into(),
            revision: "r".into(),
            title: "T".into(),
            heading: String::new(),
            context: "[]".into(),
            body: body.into(),
            at: String::new(),
            location: String::new(),
        };
        let lexical = vec![row("a", "1", "lantern one"), row("a", "1", "lantern two"), row("b", "1", "lantern")];
        let semantic = vec![row("b", "1", "lamp"), row("c", "9", "beacon")];
        let hits = fuse(&[("lexical", lexical), ("semantic", semantic)], "lantern", 10);
        let order: Vec<(&str, &str)> = hits.iter().map(|h| (h.source.as_str(), h.section.as_str())).collect();
        assert_eq!(order, vec![("b", "1"), ("a", "1"), ("c", "9")]);
        assert_eq!(hits[0].signals, vec!["lexical", "semantic"]);
        assert_eq!(hits[1].snippet, "lantern one", "shown by its best chunk");
    }
}
