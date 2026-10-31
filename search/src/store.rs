//! The tables (README.md): one a scope, LanceDB's, on the store the
//! service was started on. A table is made by its first upsert, its space
//! (`api::Space`, or none: words alone) pinned in its schema for good. Its
//! writes take turns (one service holds a scope: the platform routes a
//! scope to one instance), its reads do not wait. Each upsert replaces its
//! source whole in one commit (`merge_insert`, its old chunks deleted in the
//! same), so a search sees a source's old revision or its new, never part.
//! What a write removed leaves the store within about three minutes: its
//! table is compacted, then its old versions pruned (`maintain`).

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
use lancedb::table::OptimizeAction;
use lancedb::{Connection, DistanceType, Table};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
/// How long after a write that removed rows its table is compacted, then
/// again before its old versions are pruned (each prune keeps what is
/// younger than `KEEP_VERSIONS`, for reads in flight).
const MAINTAIN_AFTER: Duration = Duration::from_secs(75);
const KEEP_VERSIONS: Duration = Duration::from_secs(60);
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

struct Entry {
    table: Table,
    space: Option<Space>,
    /// Held by a write, and by maintenance.
    writes: tokio::sync::Mutex<Writes>,
    used: std::sync::Mutex<Instant>,
}

#[derive(Default)]
struct Writes {
    since_optimize: u32,
    maintaining: bool,
}

pub struct Store {
    db: Connection,
    tables: tokio::sync::Mutex<HashMap<String, Arc<Entry>>>,
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
        fields.push(Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), s.dimensions as i32), false));
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
    pub async fn open(uri: &str, options: Vec<(String, String)>) -> Result<Store> {
        let mut connect = lancedb::connect(uri);
        for (k, v) in options {
            connect = connect.storage_option(k, v);
        }
        Ok(Store { db: connect.execute().await?, tables: Default::default() })
    }

    /// `scope`'s table, if it was made.
    async fn entry(&self, scope: &str) -> Result<Option<Arc<Entry>>> {
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
        let e = Arc::new(Entry { table, space, writes: Default::default(), used: std::sync::Mutex::new(Instant::now()) });
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
        if up.sections.is_empty() {
            table.delete(filter.as_str()).await?;
            drop(writes);
            self.maintain(scope, &entry).await;
            return Ok(Written { source: up.source, revision: up.revision, written: true, chunks: 0, version: table.version().await? });
        }
        let rows = table.count_rows(None).await?;
        let chunks: usize = up.sections.iter().map(|s| s.chunks.len()).sum();
        if rows + chunks > api::TABLE_ROWS_MAX {
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
            table.optimize(OptimizeAction::All).await?;
            writes.since_optimize = 0;
        }
        let version = table.version().await?;
        drop(writes);
        if current.is_some() {
            // what the old revision held leaves the store
            self.maintain(scope, &entry).await;
        }
        Ok(Written { source: up.source, revision: up.revision, written: true, chunks, version })
    }

    /// A table made by its first upsert, its space pinned, its text indexed.
    async fn create(self: &Arc<Self>, scope: &str, up: api::Upsert) -> Result<Written> {
        let mut tables = self.tables.lock().await;
        if tables.contains_key(scope) {
            // made meanwhile: write to it
            drop(tables);
            return Box::pin(self.upsert(scope, up)).await;
        }
        if up.replaces.as_deref().is_some_and(|r| !r.is_empty()) {
            return Err(Error::Conflict { current: None });
        }
        if up.sections.is_empty() {
            return Ok(Written { source: up.source, revision: up.revision, written: false, chunks: 0, version: 0 });
        }
        let batch = batch_of(&up, up.space.as_ref(), &HashMap::new())?;
        let chunks = batch.num_rows();
        let table = self.db.create_table(scope, batch).execute().await?;
        for (column, _) in TEXT {
            table.create_index(&[column], Index::FTS(FtsIndexBuilder::default())).execute().await?;
        }
        let version = table.version().await?;
        let entry = Arc::new(Entry { table, space: up.space.clone(), writes: Default::default(), used: std::sync::Mutex::new(Instant::now()) });
        tables.insert(scope.to_string(), entry);
        Ok(Written { source: up.source, revision: up.revision, written: true, chunks, version })
    }

    pub async fn delete(self: &Arc<Self>, scope: &str, source: &str) -> Result<bool> {
        if source.is_empty() || source.len() > api::SOURCE_MAX_BYTES {
            return Err(Error::Invalid(format!("source is 1 to {} bytes", api::SOURCE_MAX_BYTES)));
        }
        let Some(entry) = self.entry(scope).await? else { return Ok(false) };
        let writes = entry.writes.lock().await;
        let filter = format!("source = {}", quote(source));
        if entry.table.count_rows(Some(filter.clone())).await? == 0 {
            return Ok(false);
        }
        entry.table.delete(filter.as_str()).await?;
        drop(writes);
        self.maintain(scope, &entry).await;
        Ok(true)
    }

    /// Which of `hashes` the table holds a vector of.
    pub async fn known(&self, scope: &str, k: api::Known) -> Result<Vec<String>> {
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
    pub async fn sources(&self, scope: &str, after: &str, limit: usize) -> Result<(Vec<Value>, Option<String>)> {
        if !(1..=api::SOURCES_PAGE_MAX).contains(&limit) {
            return Err(Error::Invalid(format!("limit is 1 to {}", api::SOURCES_PAGE_MAX)));
        }
        let Some(entry) = self.entry(scope).await? else { return Ok((vec![], None)) };
        let mut q = entry.table.query().select(Select::columns(&["source", "revision", "at"]));
        if !after.is_empty() {
            q = q.only_if(format!("source > {}", quote(after)));
        }
        let mut by: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
        for b in q.execute().await?.try_collect::<Vec<_>>().await? {
            let (s, r, a) = (strings(&b, "source"), strings(&b, "revision"), strings(&b, "at"));
            for i in 0..b.num_rows() {
                by.entry(s.value(i).to_string()).or_insert_with(|| (r.value(i).to_string(), a.value(i).to_string(), 0)).2 += 1;
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

    pub async fn status(&self, scope: &str) -> Result<Value> {
        let Some(entry) = self.entry(scope).await? else { return Ok(json!({ "exists": false })) };
        let t = &entry.table;
        let stats = t.stats().await?;
        let mut sources = std::collections::HashSet::new();
        for b in t.query().select(Select::columns(&["source"])).execute().await?.try_collect::<Vec<_>>().await? {
            let s = strings(&b, "source");
            sources.extend((0..b.num_rows()).map(|i| s.value(i).to_string()));
        }
        Ok(json!({
            "exists": true,
            "chunks": t.count_rows(None).await?,
            "sources": sources.len(),
            "bytes": stats.total_bytes,
            "space": entry.space,
            "semantic": if entry.space.is_some() { "on" } else { "off" },
            "version": t.version().await?,
        }))
    }

    pub async fn search(&self, scope: &str, s: api::Search) -> Result<Vec<Hit>> {
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
                let mut q = t.query().nearest_to(v.as_slice())?.distance_type(DistanceType::Cosine).select(Select::columns(&ROW_COLUMNS)).limit(DEPTH);
                if let Some(f) = &filter {
                    q = q.only_if(f.clone());
                }
                legs.push(("semantic", rows(&q.execute().await?.try_collect::<Vec<_>>().await?)));
            }
        }
        Ok(fuse(&legs, &s.q, limit))
    }

    pub async fn drop_table(&self, scope: &str) -> Result<bool> {
        let mut tables = self.tables.lock().await;
        tables.remove(scope);
        match self.db.drop_table(scope, &[]).await {
            Ok(()) => Ok(true),
            Err(lancedb::Error::TableNotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// After a write that removed rows: in `MAINTAIN_AFTER`, compact the
    /// table and fold its rows into its indexes (and index its vectors once
    /// it has `VECTOR_INDEX_MIN_ROWS`), then, as long again after, prune its
    /// versions older than `KEEP_VERSIONS`. One at a time a table.
    async fn maintain(self: &Arc<Self>, scope: &str, entry: &Arc<Entry>) {
        {
            let mut w = entry.writes.lock().await;
            if w.maintaining {
                return;
            }
            w.maintaining = true;
        }
        let (store, entry, scope) = (self.clone(), entry.clone(), scope.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(MAINTAIN_AFTER).await;
            let compacted = store.compact(&entry).await;
            tokio::time::sleep(MAINTAIN_AFTER).await;
            let pruned = store.prune(&entry, KEEP_VERSIONS).await;
            entry.writes.lock().await.maintaining = false;
            if let Err(e) = compacted.and(pruned) {
                eprintln!("{}", json!({ "event": "search.maintain-failed", "scope": scope, "error": format!("{e:?}") }));
            }
        });
    }

    /// The table compacted (its deleted rows' bytes rewritten away), its
    /// newest rows folded into its indexes, and its vectors indexed once it
    /// has `VECTOR_INDEX_MIN_ROWS`.
    async fn compact(&self, entry: &Entry) -> Result<()> {
        let mut w = entry.writes.lock().await;
        entry.table.optimize(OptimizeAction::All).await?;
        w.since_optimize = 0;
        if entry.space.is_some() && entry.table.count_rows(None).await? >= VECTOR_INDEX_MIN_ROWS && !has_vector_index(&entry.table).await? {
            entry.table.create_index(&["vector"], Index::IvfFlat(IvfFlatIndexBuilder::default().distance_type(DistanceType::Cosine))).execute().await?;
        }
        Ok(())
    }

    /// The table's versions older than `keep` deleted, and the files only
    /// they held. No write is in flight while its lock is held, so none is
    /// unverified.
    async fn prune(&self, entry: &Entry, keep: Duration) -> Result<()> {
        let _w = entry.writes.lock().await;
        let older_than = Some(chrono::TimeDelta::seconds(keep.as_secs() as i64));
        entry.table.optimize(OptimizeAction::Prune { older_than, delete_unverified: Some(true), error_if_tagged_old_versions: Some(false) }).await?;
        Ok(())
    }

    /// `scope`'s table compacted and every version but its newest pruned,
    /// now: what was deleted from it leaves the store (`maintain` does this
    /// on its own, minutes after a write).
    pub async fn purge(&self, scope: &str) -> Result<()> {
        let Some(entry) = self.entry(scope).await? else { return Ok(()) };
        self.compact(&entry).await?;
        self.prune(&entry, Duration::ZERO).await
    }
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
    let s = table.query().only_if(format!("source = {}", quote(source))).select(Select::columns(&["revision"])).limit(1).execute().await?;
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
