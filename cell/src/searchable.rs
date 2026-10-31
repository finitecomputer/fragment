//! A fragment's search (docs/api.md, "A fragment's search"): its sources'
//! sections, indexed by the search service (search/README.md) beside the
//! deployment, `FRAGMENT_SEARCH_URL` with `SEARCH_TOKEN`; none named, these
//! routes answer 404. Opt-in: nothing is held for a fragment until an
//! editor writes a source, and a fragment that never searches costs
//! nothing.
//!
//! The fragment is the authority. Its index is the service's table of its
//! scope (`searchable::scope`: its identity, its incarnation, `members`),
//! which no caller names. A reader (a viewer up) searches it; an editor
//! writes it; the role is checked before anything is read or sent, so a
//! member removed reads nothing more. Writes and questions are embedded
//! here (models.rs `embedded`, on the deployment's embeddings slot, in
//! chunks the slot reads whole), a write paid for by the fragment's owner
//! and a question by its asker (an agent's owner); an anonymous reader's
//! question is searched by its words alone. A deployment that embeds
//! nothing (`Models::embeddings_refused`: a private one without a private
//! slot) indexes and searches by words alone.
//!
//! A source named by one of the fragment's paths is forgotten when a move
//! of main deletes that file (`search_forget`, plane.rs): written to
//! `search_forget` in the move's turn, deleted from the service by the
//! alarm it arms (and before a search), a failure tried again later, and
//! left out of every answer until it is gone.
//!
//! One source is sent one thing at a time (`Sending`): a write of a
//! source waits for a forget of it in flight, and sends any still to send
//! before itself, whenever it is due, or is refused; so no forget ever
//! lands after a later write.
//!
//! A life's index is ended when the life ends (ended.rs), with its
//! database and blobs: the service refuses every write to it after (`end`,
//! search/README.md), one sent before and landing after too. A write
//! checks the life after each wait (`search_life`), so one the end
//! overtook writes nothing, and marks nothing of the next life.

use std::collections::{HashMap, HashSet};

use fragment_core::embed;
use fragment_core::searchable as rules;
use fragment_proto::search as wire;
use fragment_proto::{ErrorCode, IdentityKind, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::models::{embedded, Embedding, InFragment};

/// The service's answer, read at most this far: a page of sources or a
/// search's hits.
const ANSWER_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Forgotten sources a pass deletes from the service, at most.
const FORGETS_PER_PASS: i64 = 100;
/// How long after a failed try a forgotten source is tried again.
const FORGET_RETRY_MS: i64 = 5 * 60 * 1000;

/// The service this deployment names, and its token.
struct Service {
    url: String,
    token: String,
}

impl Service {
    async fn of(env: &Env, url: Option<&str>) -> CellResult<Service> {
        let url = url.ok_or_else(|| CellError::new(ErrorCode::NotFound, "this deployment has no search"))?.to_string();
        Ok(Service { url, token: crate::keys::search_token(env).await? })
    }

    /// `method path` on the service, with `body`: its status and JSON.
    async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> CellResult<(u16, Value)> {
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {}", self.token))?;
        let mut init = RequestInit::new();
        init.with_method(method).with_headers(h);
        if let Some(b) = body {
            init.headers.set("content-type", "application/json")?;
            init.with_body(Some(b.to_string().into()));
        }
        let req = Request::new_with_init(&format!("{}{path}", self.url), &init)?;
        let mut resp = Fetch::Request(req).send().await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the search service did not answer: {e}")))?;
        let status = resp.status_code();
        let (bytes, cut) = crate::cs::read_answer(&mut resp, ANSWER_MAX_BYTES).await?;
        if cut {
            return Err(CellError::new(ErrorCode::UpstreamFailed, format!("the search service's answer is over {ANSWER_MAX_BYTES} bytes")));
        }
        Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }
}

/// A refusal of the service's, as the platform's.
fn refused(status: u16, answer: &Value) -> CellError {
    let message = answer["message"].as_str().unwrap_or("the search service refused").to_string();
    let code = match status {
        400 => ErrorCode::InvalidRequest,
        404 => ErrorCode::NotFound,
        413 => ErrorCode::TooLarge,
        507 => ErrorCode::StorageFull,
        _ => ErrorCode::UpstreamFailed,
    };
    CellError::new(code, message)
}

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

/// One source held: while it is, nothing else sends it to the service.
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
    /// This life's index: its scope on the service.
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
    async fn search_forget_first(&self, svc: &Service, scope: &str, source: &str) -> CellResult<()> {
        if self.rows("SELECT 1 FROM search_forget WHERE source = ?", vec![source.into()])?.is_empty() {
            return Ok(());
        }
        let (status, answer) = svc.call(Method::Post, &format!("/v1/{scope}/delete"), Some(&json!({ "source": source }))).await?;
        if status != 200 {
            let why = answer["message"].as_str().unwrap_or("the search service refused");
            return Err(CellError::new(ErrorCode::UpstreamFailed, format!("its file's deletion is not yet out of the index ({why}): try again")));
        }
        self.exec("DELETE FROM search_forget WHERE source = ?", vec![source.into()])?;
        Ok(())
    }

    /// The space this deployment's vectors are in, or none: it embeds
    /// nothing.
    fn search_space(&self) -> Option<Value> {
        let models = &self.cfg.models;
        models.embeddings_refused().is_none().then(|| json!(models.embeddings.space()))
    }

    /// `inputs`' vectors (base64, by their hashes), embedded on the
    /// deployment's slot, paid by `payer`.
    async fn search_embed(&self, todo: &[&rules::Chunk], payer: &str, agent: Option<&str>, fragment: &InFragment<'_>) -> CellResult<HashMap<String, String>> {
        let mut out = HashMap::new();
        for batch in todo.chunks(embed::INPUTS_MAX) {
            let inputs: Vec<String> = batch.iter().map(|c| c.input.clone()).collect();
            match embedded(&self.env, &inputs, payer, agent, Some(fragment), &self.state).await? {
                Embedding::Vectors { vectors, .. } => {
                    for (c, v) in batch.iter().zip(vectors) {
                        out.insert(c.hash.clone(), rules::vector_b64(&v));
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
        let svc = Service::of(&self.env, self.cfg.search_url.as_deref()).await?;
        let life = self.search_life()?;
        let scope = rules::scope(&self.state.id().to_string(), life);
        // the source, held to its end: a forget of it in flight lands
        // first, and one still to send is sent first
        let _held = self.search_hold(&up.source).await;
        self.search_same_life(life)?;
        self.search_forget_first(&svc, &scope, &up.source).await?;
        let [owner] = self.metas([MetaKey::Owner])?;
        let owner = owner.ok_or_else(|| crate::fragment::missing(MetaKey::Owner))?;
        let who = asker(caller);
        let by_owner = who.payer.as_deref() == Some(owner.as_str()) && caller.signed.as_ref().is_none_or(|s| s.acting_for.as_deref().is_none_or(|f| f == owner));
        let fragment = InFragment { name: &facts.name, owner: &owner, by_owner };
        let space = self.search_space();
        let slot = &self.cfg.models.embeddings;
        let chunks = rules::chunked(&up, slot.max_input_tokens);
        let mut vectors: HashMap<String, String> = HashMap::new();
        // at most twice: once more after the table was dropped for another
        // space, or the service lacked a vector it said it held
        for attempt in 0..2 {
            if space.is_some() {
                let hashes: Vec<&str> = chunks.iter().map(|c| c.hash.as_str()).collect::<HashSet<_>>().into_iter().collect();
                let (status, known) = svc.call(Method::Post, &format!("/v1/{scope}/known"), Some(&json!({ "hashes": hashes }))).await?;
                if status != 200 {
                    return Err(refused(status, &known));
                }
                let held: HashSet<&str> = known["known"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let mut seen = HashSet::new();
                let todo: Vec<&rules::Chunk> = chunks.iter().filter(|c| !held.contains(c.hash.as_str()) && !vectors.contains_key(&c.hash) && seen.insert(c.hash.as_str())).collect();
                vectors.extend(self.search_embed(&todo, &owner, who.agent.as_deref(), &fragment).await?);
            }
            // the life it began in, still: else nothing is sent
            self.search_same_life(life)?;
            let body = rules::service_upsert(&up, &chunks, &vectors, space.as_ref());
            let (status, answer) = svc.call(Method::Post, &format!("/v1/{scope}/upsert"), Some(&body)).await?;
            // and after: an end that overtook it refused it, or dropped it
            self.search_same_life(life)?;
            match (status, answer["error"].as_str()) {
                (200, _) => {
                    self.set_meta(MetaKey::Search, "1")?;
                    let written = json!({ "source": up.source, "revision": up.revision, "written": answer["written"], "chunks": answer["chunks"] });
                    return json_response(&written);
                }
                (409, Some("conflict")) => {
                    let body = json!({ "error": ErrorCode::Conflict, "message": "the source's revision is not the one this replaces", "current": answer["current"] });
                    return Ok(Response::from_json(&body)?.with_status(409));
                }
                (409, Some("space_changed")) if attempt == 0 => {
                    // the deployment's embeddings moved: this index is
                    // written again from its sources, starting with this one
                    console_log!("{}", json!({ "event": "search.space-changed", "fragment": facts.name, "was": answer["current"], "now": space }));
                    let (status, answer) = svc.call(Method::Delete, &format!("/v1/{scope}"), None).await?;
                    if status != 200 {
                        return Err(refused(status, &answer));
                    }
                }
                (409, Some("missing_vectors")) if attempt == 0 => vectors.clear(),
                (410, Some("ended")) => return Err(CellError::new(ErrorCode::NotFound, "this fragment was deleted while the source was written")),
                _ => return Err(refused(status, &answer)),
            }
        }
        Err(CellError::new(ErrorCode::UpstreamFailed, "the search service refused the write twice"))
    }

    /// `POST /api/search/delete` (an editor): a source, gone.
    pub(crate) async fn search_delete(&self, caller: &Caller, d: wire::Delete) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        if d.source.is_empty() || d.source.len() > wire::SOURCE_MAX_BYTES {
            return Err(CellError::invalid(format!("source is 1 to {} bytes", wire::SOURCE_MAX_BYTES)));
        }
        let svc = Service::of(&self.env, self.cfg.search_url.as_deref()).await?;
        let life = self.search_life()?;
        let scope = rules::scope(&self.state.id().to_string(), life);
        let _held = self.search_hold(&d.source).await;
        self.search_same_life(life)?;
        let (status, answer) = svc.call(Method::Post, &format!("/v1/{scope}/delete"), Some(&json!({ "source": d.source }))).await?;
        if status != 200 {
            return Err(refused(status, &answer));
        }
        self.search_same_life(life)?;
        self.exec("DELETE FROM search_forget WHERE source = ?", vec![d.source.as_str().into()])?;
        json_response(&json!({ "source": d.source, "deleted": answer["deleted"] }))
    }

    /// `GET /api/search/sources?after=&limit=` (an editor): its sources,
    /// ascending, a page at a time.
    pub(crate) async fn search_sources(&self, caller: &Caller, after: Option<String>, limit: Option<String>) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let limit: usize = match limit {
            None => wire::SOURCES_PAGE_MAX,
            Some(l) => l.parse().ok().filter(|l| (1..=wire::SOURCES_PAGE_MAX).contains(l)).ok_or_else(|| CellError::invalid(format!("limit is 1 to {}", wire::SOURCES_PAGE_MAX)))?,
        };
        let svc = Service::of(&self.env, self.cfg.search_url.as_deref()).await?;
        let scope = self.search_scope()?;
        self.search_drain(&svc, &scope).await;
        let after: String = url::form_urlencoded::byte_serialize(after.unwrap_or_default().as_bytes()).collect();
        let (status, answer) = svc.call(Method::Get, &format!("/v1/{scope}/sources?limit={limit}&after={after}"), None).await?;
        if status != 200 {
            return Err(refused(status, &answer));
        }
        let pending = self.search_pending()?;
        let sources: Vec<&Value> = answer["sources"].as_array().into_iter().flatten().filter(|s| !s["source"].as_str().is_some_and(|x| pending.contains(x))).collect();
        json_response(&json!({ "sources": sources, "next": answer["next"] }))
    }

    /// `GET /api/search/status` (a reader): what the index holds.
    pub(crate) async fn search_status(&self, caller: &Caller) -> CellResult<Response> {
        self.require(caller, false, Role::Viewer)?;
        let svc = Service::of(&self.env, self.cfg.search_url.as_deref()).await?;
        let scope = self.search_scope()?;
        let (status, answer) = svc.call(Method::Get, &format!("/v1/{scope}/status"), None).await?;
        if status != 200 {
            return Err(refused(status, &answer));
        }
        let semantic = if self.search_space().is_some() { "on" } else { "off" };
        json_response(&json!({
            "exists": answer["exists"],
            "sources": answer["sources"],
            "chunks": answer["chunks"],
            "bytes": answer["bytes"],
            "semantic": semantic,
            "space": answer["space"],
            "forgetting": self.search_pending()?.len(),
        }))
    }

    /// `POST /api/search` (a reader): the sections that answer a question.
    pub(crate) async fn search_query(&self, caller: &Caller, q: wire::Query) -> CellResult<Response> {
        let facts = self.facts()?;
        self.admit(&facts, caller, false, Role::Viewer)?;
        rules::check_query(&q).map_err(CellError::invalid)?;
        let svc = Service::of(&self.env, self.cfg.search_url.as_deref()).await?;
        let scope = self.search_scope()?;
        self.search_drain(&svc, &scope).await;
        let who = asker(caller);
        let space = self.search_space();
        let mut vector = None;
        if let (Some(_), Some(payer), false) = (&space, &who.payer, q.mode == wire::Mode::Lexical) {
            let [owner] = self.metas([MetaKey::Owner])?;
            let owner = owner.ok_or_else(|| crate::fragment::missing(MetaKey::Owner))?;
            let fragment = InFragment { name: &facts.name, owner: &owner, by_owner: payer == &owner };
            match embedded(&self.env, std::slice::from_ref(&q.q), payer, who.agent.as_deref(), Some(&fragment), &self.state).await? {
                Embedding::Vectors { vectors, .. } => vector = vectors.into_iter().next().map(|v| rules::vector_b64(&v)),
                // the question is still searched, by its words
                Embedding::Refused { status, text } => console_warn!("{}", json!({ "event": "search.query-unembedded", "status": status, "why": text })),
            }
        }
        let semantic = vector.is_some();
        let mode = match (q.mode, semantic) {
            (wire::Mode::Semantic, true) => "semantic",
            (wire::Mode::Hybrid, true) => "hybrid",
            _ => "lexical",
        };
        let mut ask = json!({ "q": q.q, "limit": q.limit.unwrap_or(wire::RESULTS_DEFAULT), "mode": mode, "fields": q.fields });
        if let (Some(v), Some(s)) = (&vector, &space) {
            ask["vector"] = json!(v);
            ask["space"] = s.clone();
        }
        for (k, v) in [("after", &q.after), ("before", &q.before)] {
            if let Some(v) = v {
                ask[k] = json!(v);
            }
        }
        let (mut status, mut answer) = svc.call(Method::Post, &format!("/v1/{scope}/search"), Some(&ask)).await?;
        if status == 409 && answer["error"] == "space_changed" {
            // an index of another space, until its sources are written again: words alone
            ask.as_object_mut().expect("an object").retain(|k, _| k != "vector" && k != "space");
            ask["mode"] = json!("lexical");
            (status, answer) = svc.call(Method::Post, &format!("/v1/{scope}/search"), Some(&ask)).await?;
        }
        if status != 200 {
            return Err(refused(status, &answer));
        }
        let pending = self.search_pending()?;
        let results: Vec<&Value> = answer["results"].as_array().into_iter().flatten().filter(|h| !h["source"].as_str().is_some_and(|s| pending.contains(s))).collect();
        json_response(&json!({ "results": results, "semantic": if semantic { "on" } else { "off" } }))
    }

    /// The sources a move of main deleted (`removed`): forgotten by the
    /// index, if this life has one and the deployment searches. Written
    /// in the move's turn, due at once: the alarm the move arms sends
    /// them (`search_forget_due`).
    pub(crate) fn search_forget(&self, removed: &[String]) -> CellResult<()> {
        if removed.is_empty() || self.cfg.search_url.is_none() || self.metas([MetaKey::Search])?[0].is_none() {
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
        match (Service::of(&self.env, self.cfg.search_url.as_deref()).await, self.search_scope()) {
            (Ok(svc), Ok(scope)) => self.search_drain(&svc, &scope).await,
            (Err(e), _) | (_, Err(e)) => {
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

    /// Deletes from the service the sources due to be forgotten, a pass of
    /// at most `FORGETS_PER_PASS`; each gone there goes here. On a failure
    /// the rest wait `FORGET_RETRY_MS`.
    async fn search_drain(&self, svc: &Service, scope: &str) {
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
            match svc.call(Method::Post, &format!("/v1/{scope}/delete"), Some(&json!({ "source": f.source }))).await {
                Ok((200, _)) => {
                    if let Err(e) = self.exec("DELETE FROM search_forget WHERE source = ?", vec![f.source.as_str().into()]) {
                        console_error!("{}", json!({ "event": "search.forget-unwritten", "error": e.message }));
                    }
                }
                Ok((status, answer)) => {
                    console_warn!("{}", json!({ "event": "search.forget-refused", "status": status, "why": answer["message"] }));
                    return self.search_forget_later();
                }
                Err(e) => {
                    console_warn!("{}", json!({ "event": "search.forget-failed", "error": e.message }));
                    return self.search_forget_later();
                }
            }
        }
    }

    /// An ended life's index ended (ended.rs), if the deployment searches:
    /// dropped, and never written again, by a write landing late too.
    pub(crate) async fn search_drop(&self, incarnation: i64) -> CellResult<()> {
        let Some(url) = self.cfg.search_url.as_deref() else { return Ok(()) };
        let svc = Service::of(&self.env, Some(url)).await?;
        let scope = rules::scope(&self.state.id().to_string(), incarnation);
        let (status, answer) = svc.call(Method::Post, &format!("/v1/{scope}/end"), None).await?;
        if status != 200 {
            return Err(refused(status, &answer));
        }
        Ok(())
    }
}
