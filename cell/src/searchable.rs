//! A fragment's search (docs/api.md, "A fragment's search"): its sources'
//! sections, indexed in a `SearchIndex` object of the life's own
//! (search_index.rs), named by its scope (`searchable::scope`: its
//! identity, its incarnation, `members`), which no caller names. Opt-in:
//! nothing is held for a fragment until an editor writes a source, and a
//! fragment that never searches costs nothing: a life that never wrote
//! one (`MetaKey::Search` unset) answers a search empty, `indexed: false`,
//! embedding nothing and waking no index.
//!
//! The fragment is the authority. A reader (a viewer up) searches its
//! index; an editor writes it; the role is checked before anything is
//! read or sent, so a member removed reads nothing more. Writes and
//! questions are embedded here (models.rs `embedded`, on the deployment's
//! embeddings slot, in chunks the slot reads whole), a write paid for by
//! the fragment's owner and a question by its asker (an agent's owner);
//! an anonymous reader's question is searched by its words alone. A
//! deployment that embeds nothing (`Models::embeddings_refused`: a private
//! one without a private slot) indexes and searches by words alone.
//!
//! A question the platform embedded already (`search_asked`, the
//! meta-brain's fan-out across fragments) is searched by the vector it
//! carries, never embedded again, so its asker pays once.
//!
//! A source named by one of the fragment's paths is forgotten when a move
//! of main deletes that file (`search_forget`, plane.rs): written to
//! `search_forget` in the move's turn, deleted from the index by the alarm
//! it arms (and before a search), a failure tried again later, and left
//! out of every answer until it is gone.
//!
//! One source is sent one thing at a time (`Sending`): a write of a
//! source waits for a forget of it in flight, and sends any still to send
//! before itself, whenever it is due, or is refused; so no forget ever
//! lands after a later write.
//!
//! A life's index is ended when the life ends (ended.rs), with its
//! database and blobs, if that life may have written it (`MetaKey::Search`,
//! set before its first write is sent, recorded with the ended life): its
//! storage released, and every write refused after, one sent before and
//! landing after too. A write checks the life after each wait
//! (`search_life`), so one the end overtook writes nothing, and marks
//! nothing of the next life.

use std::collections::{HashMap, HashSet};

use fragment_core::embed;
use fragment_core::search_index as engine;
use fragment_core::searchable as rules;
use fragment_proto::search as wire;
use fragment_proto::{ErrorCode, IdentityKind, Role};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::models::{embedded, Embedding, InFragment};
use crate::search_index::{self as index, Wrote};

/// Forgotten sources a pass deletes from the index, at most.
const FORGETS_PER_PASS: i64 = 100;
/// How long after a failed try a forgotten source is tried again.
const FORGET_RETRY_MS: i64 = 5 * 60 * 1000;

/// Who a caller is, as search pays and counts it.
struct Asker {
    /// The person whose ledger pays (an agent's owner), if any.
    payer: Option<String>,
    /// The agent asking, if one is.
    agent: Option<String>,
}

fn asker(caller: &Caller) -> Asker {
    match caller.signed.as_ref() {
        None => Asker { payer: None, agent: None },
        Some(s) if s.identity.kind == IdentityKind::Agent => Asker { payer: s.identity.owner.clone(), agent: Some(s.identity.id.clone()) },
        Some(s) => Asker { payer: Some(s.identity.id.clone()), agent: None },
    }
}

#[derive(Deserialize)]
struct Forget {
    source: String,
}

/// The index's answer to `known`.
#[derive(Deserialize)]
struct KnownHashes {
    known: Vec<String>,
}

/// The index's answer to `delete`.
#[derive(Deserialize)]
struct Deleted {
    deleted: bool,
}

/// A question's vector: embedded here, or given by the platform's route
/// (none: by its words alone).
enum Question {
    Embed,
    Given(Option<String>),
}

/// One source held: while it is, nothing else sends it to the index.
struct Sending<'a> {
    cell: &'a FragmentCell,
    source: String,
    guard: Option<futures_util::lock::OwnedMutexGuard<()>>,
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        // the last holder, with no one waiting, takes the entry out
        let mut sending = self.cell.search_sending.borrow_mut();
        if sending.get(&self.source).is_some_and(|lock| std::sync::Arc::strong_count(lock) == 1) {
            sending.remove(&self.source);
        }
    }
}

impl FragmentCell {
    /// This life's index: its object's name.
    fn search_scope(&self) -> CellResult<String> {
        Ok(rules::scope(&self.state.id().to_string(), self.search_life()?))
    }

    /// This life (its incarnation): a write checks it is still the one it
    /// began in after each wait, as the fragment may have been deleted
    /// (or wiped, or made again) meanwhile.
    fn search_life(&self) -> CellResult<i64> {
        let [created] = self.metas([MetaKey::CreatedAt])?;
        created.and_then(|c| c.parse::<i64>().ok()).ok_or_else(|| crate::fragment::missing(MetaKey::CreatedAt))
    }

    fn search_same_life(&self, life: i64) -> CellResult<()> {
        let [created] = self.metas([MetaKey::CreatedAt])?;
        if created.and_then(|c| c.parse::<i64>().ok()) != Some(life) {
            return Err(CellError::new(ErrorCode::NotFound, "this fragment was deleted while the source was written"));
        }
        Ok(())
    }

    /// Whether this life may have written its index.
    fn search_indexed(&self) -> CellResult<bool> {
        Ok(self.metas([MetaKey::Search])?[0].is_some())
    }

    /// `op` on the index `scope` (search_index.rs), with `body`: its answer.
    async fn search_index<T: DeserializeOwned>(&self, scope: &str, op: &str, body: &impl Serialize) -> CellResult<T> {
        self.test_countdown(MetaKey::TestFailSearchIndex, "the search index is out of reach")?;
        let body = serde_json::to_value(body).map_err(|e| CellError::host(format!("index/{op}'s call: {e}")))?;
        let answer = crate::routed::ask_object(&self.env, "SEARCH_INDEX", scope, &format!("index/{op}"), &body).await?;
        serde_json::from_value(answer).map_err(|e| CellError::host(format!("index/{op}'s answer: {e}")))
    }

    /// Waits for, then holds, `source`.
    async fn search_hold(&self, source: &str) -> Sending<'_> {
        let lock = self.search_sending.borrow_mut().entry(source.to_string()).or_default().clone();
        let guard = lock.lock_owned().await;
        Sending { cell: self, source: source.to_string(), guard: Some(guard) }
    }

    /// Holds `source` if nothing else does.
    fn search_try_hold(&self, source: &str) -> Option<Sending<'_>> {
        let lock = self.search_sending.borrow_mut().entry(source.to_string()).or_default().clone();
        let guard = lock.try_lock_owned()?;
        Some(Sending { cell: self, source: source.to_string(), guard: Some(guard) })
    }

    /// `source`'s forget, if one waits (due or not): sent now, held, before
    /// a write of it. The write is refused if it cannot be.
    async fn search_forget_first(&self, scope: &str, source: &str) -> CellResult<()> {
        if self.rows("SELECT 1 FROM search_forget WHERE source = ?", vec![source.into()])?.is_empty() {
            return Ok(());
        }
        if let Err(e) = self.search_index::<Deleted>(scope, "delete", &json!({ "source": source })).await {
            return Err(CellError::new(ErrorCode::UpstreamFailed, format!("its file's deletion is not yet out of the index ({}): try again", e.message)));
        }
        self.exec("DELETE FROM search_forget WHERE source = ?", vec![source.into()])?;
        Ok(())
    }

    /// The space this deployment's vectors are in, or none: it embeds
    /// nothing.
    fn search_space(&self) -> Option<wire::Space> {
        let models = &self.cfg.models;
        models.embeddings_refused().is_none().then(|| {
            let s = models.embeddings.space();
            wire::Space { model: s.model.to_string(), dimensions: s.dimensions, version: s.version.map(str::to_string) }
        })
    }

    /// `todo`'s vectors, held as an index holds them (by their hashes),
    /// embedded on the deployment's slot, paid by `payer`.
    async fn search_embed(&self, todo: &[&rules::Chunk], payer: &str, agent: Option<&str>, fragment: &InFragment<'_>) -> CellResult<HashMap<String, String>> {
        let mut out = HashMap::new();
        for batch in todo.chunks(embed::INPUTS_MAX) {
            let inputs: Vec<String> = batch.iter().map(|c| c.input.clone()).collect();
            match embedded(&self.env, &inputs, payer, agent, Some(fragment), &self.state).await? {
                Embedding::Vectors { vectors, .. } => {
                    for (c, v) in batch.iter().zip(vectors) {
                        let held = engine::hold(&v).ok_or_else(|| CellError::new(ErrorCode::UpstreamFailed, "the embeddings model answered a vector with no direction"))?;
                        out.insert(c.hash.clone(), index::held_b64(&held));
                    }
                }
                Embedding::Refused { status, text } => {
                    let code = if status == 429 { ErrorCode::RateLimited } else { ErrorCode::UpstreamFailed };
                    return Err(CellError::new(code, format!("the embeddings model refused ({status}): {text}")));
                }
            }
        }
        Ok(out)
    }

    /// `POST /api/search/upsert` (an editor): a source, whole.
    pub(crate) async fn search_upsert(&self, caller: &Caller, up: wire::Upsert) -> CellResult<Response> {
        let facts = self.facts()?;
        self.admit(&facts, caller, false, Role::Editor)?;
        rules::check(&up).map_err(CellError::invalid)?;
        let life = self.search_life()?;
        let scope = rules::scope(&self.state.id().to_string(), life);
        // the source, held to its end: a forget of it in flight lands
        // first, and one still to send is sent first
        let _held = self.search_hold(&up.source).await;
        self.search_same_life(life)?;
        // from here a write may reach the index: the life's end ends it
        self.set_meta(MetaKey::Search, "1")?;
        self.search_forget_first(&scope, &up.source).await?;
        let [owner] = self.metas([MetaKey::Owner])?;
        let owner = owner.ok_or_else(|| crate::fragment::missing(MetaKey::Owner))?;
        let who = asker(caller);
        let by_owner = who.payer.as_deref() == Some(owner.as_str()) && caller.signed.as_ref().is_none_or(|s| s.acting_for.as_deref().is_none_or(|f| f == owner));
        let fragment = InFragment { name: &facts.name, owner: &owner, by_owner };
        let space = self.search_space();
        let chunks = rules::chunked(&up, self.cfg.models.embeddings.max_input_tokens);
        let mut vectors: HashMap<String, String> = HashMap::new();
        // at most twice: once more, embedding all, when the index lost a
        // vector it said it held (a write in between took it)
        for attempt in 0..2 {
            if let Some(space) = &space {
                let hashes: Vec<&str> = chunks.iter().map(|c| c.hash.as_str()).collect::<HashSet<_>>().into_iter().collect();
                let held: KnownHashes = if attempt == 0 { self.search_index(&scope, "known", &json!({ "space": space, "hashes": hashes })).await? } else { KnownHashes { known: vec![] } };
                let held: HashSet<&str> = held.known.iter().map(String::as_str).collect();
                let mut seen = HashSet::new();
                let todo: Vec<&rules::Chunk> = chunks.iter().filter(|c| !held.contains(c.hash.as_str()) && !vectors.contains_key(&c.hash) && seen.insert(c.hash.as_str())).collect();
                vectors.extend(self.search_embed(&todo, &owner, who.agent.as_deref(), &fragment).await?);
            }
            // the life it began in, still: else nothing is sent
            self.search_same_life(life)?;
            let write = index_write(&up, &chunks, &vectors, space.clone());
            let wrote: Wrote = self.search_index(&scope, "upsert", &write).await?;
            // and after: an end that overtook it refused it
            self.search_same_life(life)?;
            match wrote {
                Wrote::Written { written, chunks, cleared } => {
                    if cleared {
                        // the deployment's embeddings moved: the index is
                        // written again from its sources, starting with this one
                        console_log!("{}", json!({ "event": "search.space-changed", "fragment": facts.name, "now": space }));
                    }
                    return json_response(&json!({ "source": up.source, "revision": up.revision, "written": written, "chunks": chunks }));
                }
                Wrote::Conflict { current } => {
                    let body = json!({ "error": ErrorCode::Conflict, "message": "the source's revision is not the one this replaces", "current": current });
                    return Ok(Response::from_json(&body)?.with_status(409));
                }
                Wrote::MissingVectors if attempt == 0 => {}
                Wrote::MissingVectors => break,
                Wrote::Full { chunks, max } => {
                    let message = format!("this fragment's index holds {chunks} chunks, at most {max}: split its sources across fragments");
                    return Err(CellError::new(ErrorCode::StorageFull, message));
                }
                Wrote::Ended => return Err(CellError::new(ErrorCode::NotFound, "this fragment was deleted while the source was written")),
            }
        }
        Err(CellError::new(ErrorCode::UpstreamFailed, "the index lost vectors it held twice: try again"))
    }

    /// `POST /api/search/delete` (an editor): a source, gone.
    pub(crate) async fn search_delete(&self, caller: &Caller, d: wire::Delete) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        if d.source.is_empty() || d.source.len() > wire::SOURCE_MAX_BYTES {
            return Err(CellError::invalid(format!("source is 1 to {} bytes", wire::SOURCE_MAX_BYTES)));
        }
        if !self.search_indexed()? {
            return json_response(&json!({ "source": d.source, "deleted": false }));
        }
        let life = self.search_life()?;
        let scope = rules::scope(&self.state.id().to_string(), life);
        let _held = self.search_hold(&d.source).await;
        self.search_same_life(life)?;
        let gone: Deleted = self.search_index(&scope, "delete", &json!({ "source": d.source })).await?;
        self.search_same_life(life)?;
        self.exec("DELETE FROM search_forget WHERE source = ?", vec![d.source.as_str().into()])?;
        json_response(&json!({ "source": d.source, "deleted": gone.deleted }))
    }

    /// `GET /api/search/sources?after=&limit=` (an editor): its sources,
    /// ascending, a page at a time.
    pub(crate) async fn search_sources(&self, caller: &Caller, after: Option<String>, limit: Option<String>) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let limit: usize = match limit {
            None => wire::SOURCES_PAGE_MAX,
            Some(l) => l.parse().ok().filter(|l| (1..=wire::SOURCES_PAGE_MAX).contains(l)).ok_or_else(|| CellError::invalid(format!("limit is 1 to {}", wire::SOURCES_PAGE_MAX)))?,
        };
        if !self.search_indexed()? {
            return json_response(&json!({ "sources": [], "next": null }));
        }
        let scope = self.search_scope()?;
        self.search_drain(&scope).await;
        let mut page: serde_json::Value = self.search_index(&scope, "sources", &index::Page { after: after.unwrap_or_default(), limit }).await?;
        let pending = self.search_pending()?;
        if let Some(sources) = page["sources"].as_array_mut() {
            sources.retain(|s| !s["source"].as_str().is_some_and(|x| pending.contains(x)));
        }
        json_response(&page)
    }

    /// `GET /api/search/status` (a reader): what the index holds.
    pub(crate) async fn search_status(&self, caller: &Caller) -> CellResult<Response> {
        self.require(caller, false, Role::Viewer)?;
        let semantic = if self.search_space().is_some() { "on" } else { "off" };
        let mut status: serde_json::Value = if self.search_indexed()? {
            let mut s: serde_json::Value = self.search_index(&self.search_scope()?, "status", &json!({})).await?;
            // a live life's index is never ended: the levers' to see
            s.as_object_mut().map(|o| o.remove("ended"));
            s
        } else {
            json!({ "exists": false, "sources": 0, "chunks": 0, "bytes": 0, "chunks_split": index::CHUNKS_SPLIT, "chunks_max": index::CHUNKS_MAX })
        };
        status["semantic"] = json!(semantic);
        status["forgetting"] = json!(self.search_pending()?.len());
        json_response(&status)
    }

    /// `POST /api/search` (a reader): the sections that answer a question.
    pub(crate) async fn search_query(&self, caller: &Caller, q: wire::Query) -> CellResult<Response> {
        self.search_found(caller, q, Question::Embed).await
    }

    /// `POST /search/asked` (a reader, by the platform's own route, which
    /// the router maps no outside path to): a question embedded already.
    pub(crate) async fn search_asked(&self, caller: &Caller, asked: wire::Forwarded) -> CellResult<Response> {
        self.search_found(caller, asked.query, Question::Given(asked.vector)).await
    }

    async fn search_found(&self, caller: &Caller, q: wire::Query, question: Question) -> CellResult<Response> {
        let facts = self.facts()?;
        self.admit(&facts, caller, false, Role::Viewer)?;
        rules::check_query(&q).map_err(CellError::invalid)?;
        let space = self.search_space();
        // `wire::Found`, its hits passed through as the index wrote them
        // (`wire::Hit`): read as JSON, not again as Rust (the cell's Wasm
        // is budgeted)
        let found = |results: Vec<serde_json::Value>, semantic: bool, indexed: bool| {
            let mut found = json!({ "results": results, "semantic": if semantic { "on" } else { "off" }, "indexed": indexed });
            if let Some(s) = &space {
                found["space"] = json!(s);
                found["scheme"] = json!(wire::SCHEME);
            }
            found
        };
        if let (Question::Given(Some(v)), Some(s)) = (&question, &space) {
            engine::vector_of(v, s.dimensions).map_err(CellError::invalid)?;
        }
        if !self.search_indexed()? {
            // nothing written, so nothing embedded and no index woken
            return json_response(&found(vec![], false, false));
        }
        let scope = self.search_scope()?;
        self.search_drain(&scope).await;
        let vector = match question {
            _ if space.is_none() || q.mode == wire::Mode::Lexical => None,
            Question::Given(v) => v,
            Question::Embed => self.search_embed_question(caller, &facts.name, &q.q).await?,
        };
        let mode = match (q.mode, vector.is_some()) {
            (wire::Mode::Semantic, true) => wire::Mode::Semantic,
            (wire::Mode::Hybrid, true) => wire::Mode::Hybrid,
            _ => wire::Mode::Lexical,
        };
        let limit = q.limit.unwrap_or(wire::RESULTS_DEFAULT);
        let ask = index::Ask { q: q.q, limit, mode, fields: q.fields, after: q.after, before: q.before, vector, space: space.clone() };
        let answer: serde_json::Value = self.search_index(&scope, "search", &ask).await?;
        let pending = self.search_pending()?;
        let hits = answer["results"].as_array().cloned().unwrap_or_default();
        let mut results: Vec<serde_json::Value> = hits.into_iter().filter(|h| !h["source"].as_str().is_some_and(|s| pending.contains(s))).collect();
        for (i, h) in results.iter_mut().enumerate() {
            h["rank"] = json!(i + 1);
        }
        json_response(&found(results, answer["semantic"] == true, true))
    }

    /// The question's vector, embedded and paid by its asker (an agent's
    /// owner); none for an anonymous reader, or a refusal (it is searched
    /// by its words).
    async fn search_embed_question(&self, caller: &Caller, name: &str, q: &str) -> CellResult<Option<String>> {
        let who = asker(caller);
        let Some(payer) = &who.payer else { return Ok(None) };
        let [owner] = self.metas([MetaKey::Owner])?;
        let owner = owner.ok_or_else(|| crate::fragment::missing(MetaKey::Owner))?;
        let fragment = InFragment { name, owner: &owner, by_owner: payer == &owner };
        match embedded(&self.env, &[q.to_string()], payer, who.agent.as_deref(), Some(&fragment), &self.state).await? {
            Embedding::Vectors { vectors, .. } => Ok(vectors.into_iter().next().map(|v| rules::vector_b64(&v))),
            Embedding::Refused { status, text } => {
                console_warn!("{}", json!({ "event": "search.query-unembedded", "status": status, "why": text }));
                Ok(None)
            }
        }
    }

    /// The sources a move of main deleted (`removed`): forgotten by the
    /// index, if this life wrote one. Written in the move's turn, due at
    /// once: the alarm the move arms sends them (`search_forget_due`).
    pub(crate) fn search_forget(&self, removed: &[String]) -> CellResult<()> {
        if removed.is_empty() || !self.search_indexed()? {
            return Ok(());
        }
        let now = SqlStorageValue::Integer(crate::js::now_ms());
        for path in removed.iter().filter(|p| p.len() <= wire::SOURCE_MAX_BYTES) {
            self.exec("INSERT INTO search_forget (source, next_at) VALUES (?, ?) ON CONFLICT (source) DO UPDATE SET next_at = excluded.next_at", vec![path.as_str().into(), now.clone()])?;
        }
        Ok(())
    }

    /// When the alarm next forgets, if anything is to be forgotten.
    pub(crate) fn search_forget_due_at(&self) -> CellResult<Option<i64>> {
        Ok(self.rows("SELECT MIN(next_at) AS at FROM search_forget", vec![])?.first().and_then(|r| r["at"].as_i64()))
    }

    /// The alarm's pass: the sources due to be forgotten, sent.
    pub(crate) async fn search_forget_due(&self) {
        if self.search_forget_due_at().ok().flatten().is_none_or(|at| at > crate::js::now_ms()) {
            return;
        }
        match self.search_scope() {
            Ok(scope) => self.search_drain(&scope).await,
            Err(e) => {
                console_error!("{}", json!({ "event": "search.forget-unsent", "error": e.message }));
                self.search_forget_later();
            }
        }
    }

    /// The sources still to forget.
    fn search_pending(&self) -> CellResult<HashSet<String>> {
        Ok(self.typed::<Forget>("SELECT source FROM search_forget", vec![])?.into_iter().map(|f| f.source).collect())
    }

    /// Every due source tried again in `FORGET_RETRY_MS`.
    fn search_forget_later(&self) {
        let now = crate::js::now_ms();
        if let Err(e) = self.exec("UPDATE search_forget SET next_at = ? WHERE next_at <= ?", vec![SqlStorageValue::Integer(now + FORGET_RETRY_MS), SqlStorageValue::Integer(now)]) {
            console_error!("{}", json!({ "event": "search.forget-unwritten", "error": e.message }));
        }
    }

    /// Deletes from the index the sources due to be forgotten, a pass of
    /// at most `FORGETS_PER_PASS`; each gone there goes here. On a failure
    /// the rest wait `FORGET_RETRY_MS`.
    async fn search_drain(&self, scope: &str) {
        let now = SqlStorageValue::Integer(crate::js::now_ms());
        let due: Vec<Forget> = match self.typed("SELECT source FROM search_forget WHERE next_at <= ? ORDER BY next_at LIMIT ?", vec![now, SqlStorageValue::Integer(FORGETS_PER_PASS)]) {
            Ok(d) => d,
            Err(e) => return console_error!("{}", json!({ "event": "search.forget-unread", "error": e.message })),
        };
        for f in due {
            // held, so a write of it waits; held already, it is a write's,
            // which sends it first, or another pass's
            let Some(_held) = self.search_try_hold(&f.source) else { continue };
            match self.rows("SELECT 1 FROM search_forget WHERE source = ?", vec![f.source.as_str().into()]) {
                Ok(rows) if rows.is_empty() => continue,
                Ok(_) => {}
                Err(e) => return console_error!("{}", json!({ "event": "search.forget-unread", "error": e.message })),
            }
            match self.search_index::<Deleted>(scope, "delete", &json!({ "source": f.source })).await {
                Ok(_) => {
                    if let Err(e) = self.exec("DELETE FROM search_forget WHERE source = ?", vec![f.source.as_str().into()]) {
                        console_error!("{}", json!({ "event": "search.forget-unwritten", "error": e.message }));
                    }
                }
                Err(e) => {
                    console_warn!("{}", json!({ "event": "search.forget-failed", "code": e.code, "error": e.message }));
                    return self.search_forget_later();
                }
            }
        }
    }

    /// The lever `search-index` (ops.rs, test fleets): the index of this
    /// life, or of the life `incarnation` (an ended one's too), as it
    /// holds; `hold` true or false holds its next writes before they are
    /// written (each counted, `held`), or lets them go; `chunks_max` lowers
    /// the chunks it takes.
    pub(crate) async fn search_index_lever(&self, body: &serde_json::Value) -> CellResult<serde_json::Value> {
        let incarnation = match body["incarnation"].as_i64() {
            Some(i) => i,
            None => self.search_life()?,
        };
        let scope = rules::scope(&self.state.id().to_string(), incarnation);
        let mut ask = json!({});
        if let Some(hold) = body["hold"].as_bool() {
            ask["hold"] = json!(hold);
        }
        if !body["chunks_max"].is_null() {
            ask["chunks_max"] = body["chunks_max"].clone();
        }
        let index: serde_json::Value = self.search_index(&scope, "test", &ask).await?;
        Ok(json!({ "incarnation": incarnation, "index": index }))
    }

    /// An ended life's index ended (ended.rs; a life that may have written
    /// one): its storage released, and never written again, by a write
    /// landing late too.
    pub(crate) async fn search_drop(&self, incarnation: i64) -> CellResult<()> {
        let scope = rules::scope(&self.state.id().to_string(), incarnation);
        self.search_index::<serde_json::Value>(&scope, "end", &json!({})).await?;
        Ok(())
    }
}

/// `up` as its index takes it: each chunk with its held vector, when this
/// write embedded it (else the index holds its hash's), in `space`.
fn index_write(up: &wire::Upsert, chunks: &[rules::Chunk], vectors: &HashMap<String, String>, space: Option<wire::Space>) -> index::Write {
    let sections = up
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| index::WriteSection {
            id: s.id.clone(),
            heading: s.heading.clone(),
            context: s.context.clone(),
            at: s.at.clone(),
            location: s.location,
            chunks: chunks.iter().filter(|c| c.section == i).map(|c| index::WriteChunk { body: c.text.clone(), hash: c.hash.clone(), vector: vectors.get(&c.hash).cloned() }).collect(),
        })
        .collect();
    index::Write {
        source: up.source.clone(),
        revision: up.revision.clone(),
        replaces: up.replaces.clone(),
        title: up.title.clone().unwrap_or_default(),
        at: up.at.clone(),
        fields: up.fields.clone(),
        space,
        sections,
    }
}
