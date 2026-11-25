//! The model route (docs/cloudflare-v1.md, decision 23; "Lessons from
//! cloudflare/agents" 7 and 8; spike S4): one OpenAI-shaped chat
//! completion on a route's model, metered on its payer's ledger.
//!
//! Every call goes through the pipe (crate::pipe): gated beside it, then
//! made, bounded (`fragment_core::models`: the route's model, the capped
//! `max_tokens`, its `reasoning_effort` clamped, none of the client's
//! headers), and charged after its answer, never in its way: its last,
//! cumulative usage (input less what was cached) as one meter row under a
//! reference of its own (`aig:<hex>`). A call the model refused or that
//! never reached it used nothing, and is charged nothing; one that
//! reported no usage (its stream broke) is charged nothing too, said aloud.
//! A stream is charged at its `[DONE]`, not at its end: the gateway has held
//! one open ~150 s after its answer. Nothing of the request or its answer
//! is kept: only the usage, on the ledger, and the call's timing on its
//! `model.settled` line (`first_ms`, `ms`, `calls`).
//!
//! A call whose model fails before answering anything (not reached, 429,
//! or a 5xx; on an OpenAI-compatible API, where the key is the platform's,
//! also 401, 402, 403 or 404: `fragment_core::models::falls_back`, a 401 or
//! 402 logged aloud as `model.platform-key`) is made once more, the same,
//! on its slot's own fallback (`fragment_core::models::Models::
//! fallback_of`, `Tries`), however the call named the slot: the one its
//! slot's config names, if any (an image only to one that reads images:
//! `fallback_for_input`). It is logged as `model.fallback`, and
//! charged as the fallback's. Not a race: the fallback is called only
//! after the model failed. An answer that began is final, and the
//! fallback's failure is passed through as any. A transcription has none.
//!
//! Where a model runs is the deployment's (`FRAGMENT_MODELS`, config.rs;
//! decision 61), on one of two kinds of upstream, one input:
//! - Workers AI (`workers_ai`). Production calls the Worker's `AI` binding
//!   through the deployment's AI Gateway (`AI_GATEWAY_ID`), its logs off
//!   and its metadata opaque ids (`opaque`). Dev and the e2e set
//!   `FRAGMENT_AI_URL` instead, and the same input is POSTed to
//!   `<url>/run/<model>`, its answer read the same way: a lower-rung fake
//!   at the vendor boundary (crates/fakes, `workers_ai`), never product
//!   proof. A job's image step calls its model here (`run`), and a
//!   transcription slot on Workers AI its own.
//! - An OpenAI-compatible API (`openai`, OpenRouter's or Finite
//!   Private's): the same input, the slot's own fields over the client's
//!   (its config's `body`: OpenRouter's provider routing) and its model's
//!   id, POSTed to `<base_url>/chat/completions` with the key bound for the
//!   call's slot (keys.rs) as its bearer token, and no metadata (its limit
//!   is its key's). Its answers are OpenAI's shape already, its usage the
//!   whole call's wherever a chunk carries it (`Stream::on`). The e2e's is
//!   the same fake's OpenAI-compatible face.
//!
//! A job's text step makes a route's call the same way, streamed, falling
//! back the same, unmetered here (`streamed`: ai.rs meters it).
//!
//! Who calls: an agent, `POST /api/models/v1/chat/completions` (`route`),
//! signed by the agent (its computer's model intercept signs it), `for`
//! naming whom it acts for and `fragment` the turn's fragment; `complete`
//! is the call itself. The payer is the agent's owner (decision 36).
//! A call names a route (the deployment's `models.routes`, by its name),
//! `vision`: its route's vision model, for a runtime's calls about an
//! image (Hermes' screenshots), or `fallback`: its route's fallback, for a
//! runtime's own switch when its model stalls (Hermes'
//! `fallback_providers`), each metered the same way; or a slot by its
//! model's own id (`Models::named`: the slot of the route its
//! `x-fragment-tier` says, when that slot runs it, else the first that
//! does), which `GET /api/models/v1/models` lists (`list_route`).
//! A fragment someone else owns is asked whether it is still open under its
//! cap first (decision 26).
//!
//! Transcription (decision 9: a voice memo is one the agent transcribes
//! itself): `POST /api/models/v1/audio/transcriptions`, OpenAI's multipart
//! shape, `model` the route's `whisper` (`transcription_route`), runs on
//! the deployment's transcription slot (`models.transcription`): a Workers
//! AI model on the same transport, sent Whisper's input, or an
//! OpenAI-compatible API, sent OpenAI's form at `<base_url>/audio/
//! transcriptions` with the slot's key; charged at the length its model
//! heard, at the slot's price (`fragment_core::transcribe`). A deployment
//! that names none transcribes nothing. It names no fragment: its agent's
//! owner pays, under no fragment's cap, as a call through the model
//! intercept does.
//!
//! Decisions: `POST /api/models/v1/decide` (`decide_route`), a job's
//! `ai.decide` input with its media inline, on the Clef family
//! (fragment_core::decide) over the same transport, from the same caller
//! as `route`, charged at the input tokens it counted; no fallback (a
//! decision's answer is its model's).

use fragment_core::models::{self as bounds, Bounded, Model, Named, Stream, Tried, Tries, Upstream};
use fragment_core::price::Usage;
use fragment_core::{decide, embed, multipart, transcribe};
use fragment_proto::{valid_fragment_name, ErrorCode, IdentityKind};
use futures_util::future::LocalBoxFuture;
use futures_util::StreamExt;
use serde_json::{json, Value};
use worker::*;

use crate::ReadJson;
use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::fragment::calls::{Call, Whose};
use crate::{js, read_body};

/// A call's request (`fragment_core::models`): Hermes' shrunk screenshot
/// and its prompt fit.
pub use fragment_core::models::MODEL_BODY_MAX_BYTES;
/// An unstreamed answer, or a refusal, read whole: at most `MAX_TOKENS`
/// of text and its JSON.
const ANSWER_MAX_BYTES: usize = 8 * 1024 * 1024;
/// A refusal passed on: what a client reads of one (a message, not a
/// document).
const REFUSAL_MAX_CHARS: usize = 4096;

/// The fragment a call is spent in: its name and owner, and whether the
/// spender is that owner, or an agent of theirs acting for them (caps
/// stop everyone else's: decision 26).
pub struct InFragment<'a> {
    pub name: &'a str,
    pub owner: &'a str,
    pub by_owner: bool,
}

/// One model call: who pays, which agent makes it, where, on which route's
/// slot, the client's OpenAI-shaped body, and whether it streams.
pub struct ModelCall<'a> {
    /// The person whose ledger pays: the agent's owner.
    pub payer: &'a str,
    /// The agent making the call (its identity), when one does.
    pub agent: Option<&'a str>,
    pub fragment: Option<InFragment<'a>>,
    pub named: Named,
    /// The route its caller runs on (its `x-fragment-tier`), by its place
    /// in `Models::routes`: whose `vision` and `fallback` it runs
    /// (`Models::route_for`).
    pub route: Option<usize>,
    pub body: Value,
    pub stream: bool,
}

/// Work a call leaves to finish after its answer has started (its
/// charge): the router's `Context`, or a Durable Object's `State`.
pub trait Background {
    fn later(&self, work: LocalBoxFuture<'static, ()>);
}

impl Background for Context {
    fn later(&self, work: LocalBoxFuture<'static, ()>) {
        self.wait_until(work);
    }
}

impl Background for State {
    fn later(&self, work: LocalBoxFuture<'static, ()>) {
        self.wait_until(work);
    }
}

/// The opaque id the AI Gateway's metadata carries for an identity: the
/// first 16 hex of its SHA-256, never a name or an email (spike S4: the
/// gateway keeps a metadata row per call even with its logs off).
pub fn opaque(identity: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(identity.as_bytes());
    hex::encode(&digest[..8])
}

fn refused(why: bounds::Refusal) -> CellError {
    CellError::invalid(why.message())
}

/// A call's charge, after its answer: its bill (crate::pipe), and the
/// call's timing for its log line.
struct Charge {
    env: Env,
    bill: crate::pipe::Bill,
    /// The model whose answer the call's is (the fallback's, when it fell back).
    model: &'static str,
    /// What the call named: a route, an alias, a slot, `whisper` or `decide`.
    name: &'static str,
    /// When its first try was made: its log line says how long it took.
    t0: i64,
    /// Its tries (a second when the first fell back).
    calls: u32,
    /// When the model began to answer, ms after `t0`: a stream's first
    /// data line, or an answer read whole's status.
    first_ms: Option<i64>,
}

impl Charge {
    /// The call's end, charged through the pipe (`pipe::settle`): its
    /// usage, priced by the payer's ledger, or none, charged nothing. Its
    /// log line says its timing (docs/api.md, Models: a latency lane
    /// reads it).
    async fn charge(self, usage: Option<Usage>, log_id: Option<String>) {
        let ms = js::now_ms() - self.t0;
        // one line per event: `wrangler tail` drops lines (lesson 14)
        console_log!(
            "{}",
            json!({ "event": "model.settled", "ref": self.bill.reference, "logId": log_id, "tier": self.name, "model": self.model, "usage": usage, "first_ms": self.first_ms, "ms": ms, "calls": self.calls })
        );
        crate::pipe::settle(&self.env, vec![(self.bill, usage)]).await;
    }

    /// `charge`, as work after the answer.
    fn later(self, after: &dyn Background, usage: Option<Usage>, log_id: Option<String>) {
        after.later(Box::pin(self.charge(usage, log_id)));
    }
}

/// Asks the pipe's gate whether `payer` may make an agent's call
/// (`pipe::gate`, beside it), and the call's charge.
async fn gate(env: &Env, payer: &str, agent: Option<&str>, fragment: Option<&InFragment<'_>>, model: &'static str, name: &'static str, after: &dyn Background) -> CellResult<Charge> {
    let fragment = crate::pipe::gate(env, payer, fragment, after).await?;
    let bill = crate::pipe::Bill { payer: payer.to_string(), reference: format!("aig:{}", js::random_hex::<16>()), agent: agent.map(str::to_string), fragment, computer: None };
    Ok(Charge { env: env.clone(), bill, model, name, t0: js::now_ms(), calls: 1, first_ms: None })
}

/// The bytes `body` encodes to, counted as it is written, not kept: a body
/// `route` read within the cap may write longer (`1e-5` is `0.00001`).
fn encoded_len(body: &Value) -> usize {
    struct Counted(usize);
    impl std::io::Write for Counted {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut n = Counted(0);
    serde_json::to_writer(&mut n, body).expect("a JSON value writes");
    n.0
}

/// A refusal of the model's, passed on as it came: the call used nothing,
/// and is charged nothing.
fn passed(status: u16, text: String) -> CellResult<Response> {
    let mut resp = Response::ok(text)?.with_status(status);
    resp.headers_mut().set("content-type", "application/json")?;
    Ok(resp)
}

/// One model call, metered (the module's doc): its answer as the client
/// reads it, OpenAI's shape. It is charged by `after`, after its answer
/// (a stream's at its `[DONE]`, whether or not the client read it to its
/// end).
pub async fn complete(env: &Env, call: ModelCall<'_>, after: &dyn Background) -> CellResult<Response> {
    let cfg = Config::from_env(env);
    let body_bytes = encoded_len(&call.body);
    if body_bytes > MODEL_BODY_MAX_BYTES {
        return Err(CellError::too_large("a model call", body_bytes, MODEL_BODY_MAX_BYTES));
    }
    let model = cfg.models.capped_for(call.named, call.route);
    let bounded = bounds::bound(model, call.body, call.stream).map_err(refused)?;
    let mut charge = gate(env, call.payer, call.agent, call.fragment.as_ref(), bounded.model.id(), cfg.models.name_of(call.named), after).await?;
    let meta = Metadata { user_id: opaque(call.payer), agent_id: call.agent.map(opaque) };
    // as its slot falls back, however the call named it (#589's review)
    let fallback = cfg.models.fallback_for_input(bounded.model.slot, &bounded.input);
    let (answer, model, calls) = tried(env, cfg, &bounded, fallback, &meta).await;
    charge.model = model.id();
    charge.calls = calls;
    // its stream is read as the API that answered streams
    let api = model.upstream;
    let model = model.id();
    // not reached, or refused: it used nothing, and nothing is charged
    let mut upstream = answer?;
    let status = upstream.status_code();
    let log_id = upstream.headers().get("cf-aig-log-id")?;
    if status != 200 {
        return passed(status, bounded_text(&mut upstream).await);
    }
    if !bounded.stream {
        charge.first_ms = Some(js::now_ms() - charge.t0);
        let bytes = match read_whole(&mut upstream).await {
            Ok(b) => b,
            Err(e) => {
                // what it used is unknown
                charge.later(after, None, log_id);
                return Err(e);
            }
        };
        let usage = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| api.used(model, &v["usage"]));
        charge.later(after, usage, log_id);
        let mut resp = Response::from_bytes(bytes)?;
        resp.headers_mut().set("content-type", "application/json")?;
        return Ok(resp);
    }
    let ResponseBody::Stream(source) = upstream.body() else {
        charge.later(after, None, log_id);
        return Err(CellError::new(ErrorCode::UpstreamFailed, "the model's stream had no body"));
    };
    // One reader, read to its end: each chunk is read once, the client sent
    // its lines in OpenAI's shape (`Stream`) as they come, and the call
    // metered from the same lines, so a client that goes away mid-answer
    // does not leave the call uncharged. The call is charged at its
    // `[DONE]`, where its usage is whole: a gateway has held a stream open
    // ~150 s past it, and a charge at the stream's end came that late, or
    // was cut with the Worker's waitUntil. The stream is read on to its
    // end after `[DONE]` while the charge is made, as it always was, and
    // the client's answer ends with it, never waiting on the charge.
    let source = source.clone();
    let (to_client, client) = futures_channel::mpsc::unbounded::<Result<Vec<u8>>>();
    after.later(Box::pin(async move {
        let mut stream = Stream::default().on(api);
        let mut bytes = ByteStream::from(source);
        let (mut broke, mut ended) = (false, false);
        let mut charge = charge;
        // a client gone away is sent nothing more, and the call read on
        let send = |out: Vec<u8>| {
            if !out.is_empty() {
                let _ = to_client.unbounded_send(Ok(out));
            }
        };
        // bounded by the model's answer: at most the tier's max_tokens,
        // then its `[DONE]` or the stream's end
        while !stream.done() {
            let Some(chunk) = bytes.next().await else {
                ended = true;
                break;
            };
            match chunk {
                Ok(c) => {
                    let mut out = Vec::new();
                    stream.push(&c, Some(&mut out));
                    send(out);
                }
                Err(e) => {
                    let _ = to_client.unbounded_send(Err(e));
                    broke = true;
                    break;
                }
            }
            if charge.first_ms.is_none() && stream.began() {
                charge.first_ms = Some(js::now_ms() - charge.t0);
            }
        }
        if !broke && !stream.done() {
            let mut out = Vec::new();
            stream.finish(Some(&mut out));
            send(out);
        }
        let usage = if broke { None } else { stream.usage().and_then(|u| api.used(model, u)) };
        // bounded by the stream's end: nothing after `[DONE]` is the answer's
        let read_on = async {
            while stream.done() && !ended {
                match bytes.next().await {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let _ = to_client.unbounded_send(Err(e));
                        break;
                    }
                    None => break,
                }
            }
            drop(to_client);
        };
        futures_util::future::join(charge.charge(usage, log_id), read_on).await;
    }));
    let mut resp = Response::from_stream(client)?;
    resp.headers_mut().set("content-type", "text/event-stream")?;
    resp.headers_mut().set("cache-control", "no-cache")?;
    Ok(resp)
}

/// A job's text step's call (ai.rs): one bounded, streamed call of a route,
/// where the deployment runs it, unmetered, falling back as `complete`'s
/// does, the same request (its tools too) to the fallback model.
pub(crate) struct Streamed {
    /// The answer that is the call's, its body unread: a refusal's to read
    /// whole, a 200's to read to its `[DONE]`.
    pub answer: Response,
    pub log_id: Option<String>,
    /// The model whose answer it is: its caller meters it as that model's.
    pub model: &'static str,
    /// Where that model runs: its stream is read as that API streams
    /// (`Stream::on`).
    pub upstream: &'static Upstream,
    /// Its tries (a second when the first fell back).
    pub calls: u32,
}

/// `Streamed`'s call; the caller keeps what it bought before it settles.
pub(crate) async fn streamed(env: &Env, bounded: &Bounded<'static>, payer: &str, agent: Option<&str>) -> CellResult<Streamed> {
    assert!(bounded.stream, "a step's call streams: its answer is read as it comes");
    let cfg = Config::from_env(env);
    let meta = Metadata { user_id: opaque(payer), agent_id: agent.map(opaque) };
    // a step names its route: it falls back as that route's slot does
    let fallback = cfg.models.fallback_for_input(bounded.model.slot, &bounded.input);
    let (answer, model, calls) = tried(env, cfg, bounded, fallback, &meta).await;
    let answer = answer?;
    let log_id = answer.headers().get("cf-aig-log-id")?;
    Ok(Streamed { answer, log_id, model: model.id(), upstream: model.upstream, calls })
}

/// A bounded call made on its model, then, when that one fails before
/// answering anything, once more on `fallback` (`Tries`: at most two
/// tries, the second only after the first failed): the answer that is the
/// call's, its body unread, the model whose answer it is, and the tries.
async fn tried(env: &Env, cfg: &Config, bounded: &Bounded<'static>, fallback: Option<Model<'static>>, meta: &Metadata) -> (CellResult<Response>, Model<'static>, u32) {
    let mut tries = Tries::new(bounded.model, fallback);
    let mut input = std::borrow::Cow::Borrowed(&bounded.input);
    let mut calls = 0;
    // bounded by `Tries`: a second try at most
    loop {
        let model = tries.model();
        let t0 = js::now_ms();
        calls += 1;
        let answer = transport(env, cfg, model, &input, meta).await;
        let status = answer.as_ref().ok().map(Response::status_code);
        // the platform's own key refused or its credit spent, on any try (a
        // fallback's own key too): every call to it falls back, or fails
        // where it has none, until someone fixes it
        if let Some(alarm) = status.and_then(|s| bounds::key_alarm(model.upstream, s)) {
            console_error!("{}", json!({ "event": "model.platform-key", "model": model.id(), "status": status, "alarm": alarm }));
        }
        let Some(next) = tries.after(status.map_or(Tried::Unreached, Tried::Answered)) else { return (answer, model, calls) };
        // what the vendor said (its error, never the request's words)
        let why = match answer {
            Ok(mut failed) => bounded_text(&mut failed).await.chars().take(300).collect::<String>(),
            Err(e) => e.message,
        };
        console_log!("{}", json!({ "event": "model.fallback", "from": model.id(), "to": next.id(), "status": status, "ms": js::now_ms() - t0, "why": why }));
        // Deep may fall back to Fast: its smaller cap still holds. Never
        // raise the caller's count, and copy the prompt only on this path.
        let cap = cfg.models.opts(next.slot).max_tokens.unwrap_or(bounds::MAX_TOKENS);
        if bounded.max_tokens > cap {
            input.to_mut()["max_tokens"] = json!(cap);
        }
    }
}

/// One call of a Workers AI model with its input, unmetered and read whole
/// (a job's image or decision step: ai.rs, whose caller bounds the input and meters
/// it).
pub(crate) async fn run(env: &Env, model: &str, input: &Value, payer: &str, agent: Option<&str>) -> CellResult<(u16, Vec<u8>, Option<String>)> {
    let meta = Metadata { user_id: opaque(payer), agent_id: agent.map(opaque) };
    let mut resp = workers_ai(env, Config::from_env(env), model, input, &meta).await?;
    let status = resp.status_code();
    let log_id = resp.headers().get("cf-aig-log-id")?;
    let bytes = read_whole(&mut resp).await?;
    Ok((status, bytes, log_id))
}

/// One call of a Workers AI model whose input is a multipart form, its
/// `fields` (an icon's drawing: crate::icon), unmetered and read whole,
/// with the neurons Workers AI said it cost (`cf-ai-neurons`). Through
/// the binding without the gateway (`js::ai_run_form`); in dev and the
/// e2e, the fields as JSON to the fake, as `run` sends its input.
pub(crate) async fn run_form(env: &Env, model: &str, fields: &Value, payer: &str) -> CellResult<(u16, Vec<u8>, Option<String>)> {
    let cfg = Config::from_env(env);
    let mut resp = match &cfg.ai_url {
        Some(_) => workers_ai(env, cfg, model, fields, &Metadata { user_id: opaque(payer), agent_id: None }).await?,
        None => {
            let fields = fields.as_object().ok_or_else(|| CellError::host("a form's fields are an object"))?;
            js::ai_run_form(env.as_ref(), model, fields).await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the model did not answer: {}", e.message)))?
        }
    };
    let status = resp.status_code();
    let neurons = resp.headers().get("cf-ai-neurons")?;
    let bytes = read_whole(&mut resp).await?;
    Ok((status, bytes, neurons))
}

/// The AI Gateway's metadata for a call: opaque ids only.
struct Metadata {
    user_id: String,
    agent_id: Option<String>,
}

/// Makes a bounded call on `model`, where the deployment runs it.
async fn transport(env: &Env, cfg: &Config, model: Model<'_>, input: &Value, meta: &Metadata) -> CellResult<Response> {
    match model.upstream {
        Upstream::WorkersAi { model } => workers_ai(env, cfg, model, input, meta).await,
        Upstream::Openai { base_url, model: id, body, .. } => openai(env, model.slot, base_url, "/chat/completions", &bounds::openai_request(input, id, body.as_ref())).await,
    }
}

/// Makes a bounded call on an OpenAI-compatible API: `body` (the input, the
/// slot's own fields and the model's id: `openai_request`; an embeddings
/// call's `embed::request`), POSTed to `<base_url><path>` with the key
/// bound for the call's slot (`keys::model_key`) as its bearer token, and
/// nothing of the client's or of the payer's. A key the deployment does
/// not bind is a model not reached (its call falls back), said in the log
/// without it.
async fn openai(env: &Env, slot: &str, base_url: &str, path: &str, body: &str) -> CellResult<Response> {
    let key = crate::keys::model_key(env, slot).await?;
    keyed(&key, base_url, path, body).await
}

/// `openai`, its key read already.
async fn keyed(key: &str, base_url: &str, path: &str, body: &str) -> CellResult<Response> {
    keyed_as(key, base_url, path, "application/json", body.into()).await
}

/// `keyed`'s call, its body `body` of `content_type` (a transcription's
/// form: `transcribe::openai_form`). A redirect is no answer: the
/// platform's key and the call's body go only to the API its slot names,
/// never on to wherever that API points (a fetch that follows one sends
/// both).
async fn keyed_as(key: &str, base_url: &str, path: &str, content_type: &str, body: worker::wasm_bindgen::JsValue) -> CellResult<Response> {
    let h = Headers::new();
    h.set("content-type", content_type)?;
    h.set("authorization", &format!("Bearer {key}"))?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(h).with_body(Some(body)).with_redirect(RequestRedirect::Error);
    let req = Request::new_with_init(&format!("{base_url}{path}"), &init)?;
    Fetch::Request(req).send().await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the model did not answer: {e}")))
}

/// Makes a bounded call: `input` to Workers AI's `model`, through the AI
/// binding and the deployment's gateway, or, in dev and the e2e, to the
/// fake at `FRAGMENT_AI_URL`.
async fn workers_ai(env: &Env, cfg: &Config, model: &str, input: &Value, meta: &Metadata) -> CellResult<Response> {
    let mut metadata = json!({ "user_id": meta.user_id });
    if let Some(agent) = &meta.agent_id {
        metadata["agent_id"] = json!(agent);
    }
    // GLM caches a prefix per session: an agent's calls share theirs (lesson 8)
    let headers = match &meta.agent_id {
        Some(agent) => json!({ "x-session-affinity": agent }),
        None => json!({}),
    };
    if let Some(url) = &cfg.ai_url {
        // the lower rung: the binding's input, to a fake at the vendor boundary
        let h = Headers::new();
        h.set("content-type", "application/json")?;
        h.set("x-fragment-ai-metadata", &metadata.to_string())?;
        if let Some(agent) = &meta.agent_id {
            h.set("x-session-affinity", agent)?;
        }
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(h).with_body(Some(input.to_string().into()));
        let req = Request::new_with_init(&format!("{url}/run/{model}"), &init)?;
        return Fetch::Request(req).send().await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the model did not answer: {e}")));
    }
    let Some(gateway) = &cfg.ai_gateway_id else {
        return Err(CellError::host("this deployment has no model route: set AI_GATEWAY_ID (its AI Gateway)"));
    };
    let options = json!({ "gateway": { "id": gateway, "metadata": metadata, "collectLog": false }, "extraHeaders": headers });
    js::ai_run(env.as_ref(), model, input, &options).await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the model did not answer: {}", e.message)))
}

/// An answer's body, at most `ANSWER_MAX_BYTES` (read no further).
pub(crate) async fn read_whole(resp: &mut Response) -> CellResult<Vec<u8>> {
    match crate::cs::read_answer(resp, ANSWER_MAX_BYTES).await? {
        (_, true) => Err(CellError::new(ErrorCode::UpstreamFailed, format!("the model's answer is over {ANSWER_MAX_BYTES} bytes"))),
        (bytes, false) => Ok(bytes),
    }
}

/// A refusal's body as text, cut to what a client reads (and read no further).
async fn bounded_text(resp: &mut Response) -> String {
    let (bytes, _) = crate::cs::read_answer(resp, 4 * REFUSAL_MAX_CHARS).await.unwrap_or_default();
    String::from_utf8_lossy(&bytes).chars().take(REFUSAL_MAX_CHARS).collect()
}

/// `POST /api/models/v1/chat/completions[?fragment=<name>]`: an agent's
/// model call, signed by the agent (`for` names whom it acts for), its
/// `model` one of the route's names (a route's, `vision`, `fallback`) or a
/// model's id, read with the route its `x-fragment-tier` says
/// (`Models::named`). Its owner pays; `fragment` (one the agent is in) is
/// where the turn is, for that fragment's cap.
pub(crate) async fn route(mut req: Request, env: &Env, url: &Url, after: &dyn Background) -> CellResult<Response> {
    let body = read_body(&mut req, MODEL_BODY_MAX_BYTES).await?;
    let caller = caller(env, &req, url, &body).await?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("body: {e}")))?;
    let models = &Config::from_env(env).models;
    let route = match req.headers().get(bounds::TIER_HEADER)? {
        None => None,
        Some(t) => Some(models.route_named(t.trim()).ok_or_else(|| CellError::invalid(format!("{} names {}", bounds::TIER_HEADER, models.unknown_route().message())))?),
    };
    let named = match v.get("model") {
        None | Some(Value::Null) => models.named(None, route),
        Some(Value::String(m)) => models.named(Some(m), route),
        Some(_) => Err(models.unknown()),
    };
    let named = named.map_err(|e| CellError::invalid(e.message()))?;
    let stream = v.get("stream") == Some(&Value::Bool(true));
    let call = ModelCall { payer: &caller.owner, agent: Some(&caller.agent), fragment: caller.in_fragment(), named, route, body: v, stream };
    complete(env, call, after).await
}

/// `GET /api/models/v1/models`: the ids an agent's call may name, OpenAI's
/// list (`Models::listed`), nothing of where each runs. Signed by an agent,
/// as the route's other calls are; a computer's guest asks its model
/// intercept (`GET /v1/models`), which answers it itself.
pub(crate) async fn list_route(req: Request, env: &Env, url: &Url) -> CellResult<Response> {
    let agent = crate::signer_for(env, &req, url, &[]).await?;
    if agent.kind != IdentityKind::Agent {
        return Err(CellError::new(ErrorCode::Forbidden, "the model route is an agent's: its owner pays for its calls"));
    }
    Ok(Response::from_json(&Config::from_env(env).models.listed())?)
}

/// Who calls an agent's route (`route`, `decide_route`): the signer, an
/// agent (`for` names whom it acts for); its owner, who pays; and the
/// fragment `fragment` names, one it is in, where the call is spent.
struct Caller {
    agent: String,
    owner: String,
    /// The fragment named: its name, its owner, and whether that owner
    /// spends there (the agent as itself, or for its owner, in its owner's).
    fragment: Option<(String, String, bool)>,
}

impl Caller {
    fn in_fragment(&self) -> Option<InFragment<'_>> {
        self.fragment.as_ref().map(|(name, owner, by_owner)| InFragment { name, owner, by_owner: *by_owner })
    }
}

/// The caller of an agent's route, checked: signed by an agent, naming at
/// most one `fragment`, one it is in.
async fn caller(env: &Env, req: &Request, url: &Url, body: &[u8]) -> CellResult<Caller> {
    let agent = crate::signer_for(env, req, url, body).await?;
    if agent.kind != IdentityKind::Agent {
        return Err(CellError::new(ErrorCode::Forbidden, "the model route is an agent's: its owner pays for its calls"));
    }
    let owner = agent.owner.clone().ok_or_else(|| CellError::host("an agent without an owner"))?;
    let mut fragments = url.query_pairs().filter(|(k, _)| k == "fragment").map(|(_, v)| v.into_owned());
    let named = fragments.next();
    if fragments.next().is_some() {
        return Err(CellError::invalid("`fragment` is named once"));
    }
    let Some(name) = named else { return Ok(Caller { agent: agent.id.clone(), owner, fragment: None }) };
    if !valid_fragment_name(&name) {
        return Err(CellError::invalid("`fragment` is a fragment's name, <label>--<suffix>"));
    }
    let mut answer = crate::fragment::pass(env, &name, &Whose { principal: agent.id.clone() }).await?;
    if answer.status_code() != 200 {
        return Err(CellError::new(ErrorCode::NotFound, format!("no fragment {name}")));
    }
    let whose: <Whose as Call>::Answer = answer.read_json().await?;
    if !whose.member {
        return Err(CellError::new(ErrorCode::Forbidden, format!("a model call names a fragment its agent is in, and it is not in {name}")));
    }
    // the owner's own: the agent acting as itself, or for its owner, in its owner's fragment
    let for_owner = agent.acting_for.as_deref().is_none_or(|asker| asker == owner);
    let by_owner = whose.owner == owner && for_owner;
    Ok(Caller { agent: agent.id.clone(), owner, fragment: Some((name, whose.owner, by_owner)) })
}

/// `POST /api/models/v1/decide[?fragment=<name>]`: an agent's decision
/// (docs/api.md, Models), its body a job's `ai.decide` input with its
/// media inline (fragment_core::decide), checked as a step is before
/// anything is sent; its caller, payer and fragment `route`'s. It is
/// charged, after its answer, at the input tokens the model counted
/// (output is free). Its answer is the step's, `{model, answers, usage}`.
/// A call the model refuses is charged nothing and its refusal passed
/// through; an answer that does not answer what was asked is charged (it
/// was paid for) and answered 502.
pub(crate) async fn decide_route(mut req: Request, env: &Env, url: &Url, after: &dyn Background) -> CellResult<Response> {
    let body = read_body(&mut req, MODEL_BODY_MAX_BYTES).await?;
    let caller = caller(env, &req, url, &body).await?;
    let asked: Value = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("a decision is JSON: {e}")))?;
    drop(body);
    let call = decide::bound(&asked, decide::Asker::Agent).map_err(|why| CellError::invalid(why.message()))?;
    drop(asked);
    let model = call.model;
    let mut charge = gate(env, &caller.owner, Some(&caller.agent), caller.in_fragment().as_ref(), model.id(), DECIDE, after).await?;
    let input = call.input(vec![]);
    let answered = run(env, model.id(), &input, &caller.owner, Some(&caller.agent)).await;
    drop(input);
    // not reached: it used nothing, and nothing is charged
    let (status, bytes, log_id) = answered?;
    charge.first_ms = Some(js::now_ms() - charge.t0);
    if status != 200 {
        return passed(status, String::from_utf8_lossy(&bytes).chars().take(REFUSAL_MAX_CHARS).collect());
    }
    let response: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    // it answered, so it was paid for: its usage
    charge.later(after, call.usage(&response), log_id);
    match call.answer(&response) {
        Ok(answer) => Ok(Response::from_json(&answer)?),
        Err(fault) => Err(CellError::new(ErrorCode::UpstreamFailed, format!("{}: the call is charged", fault.message()))),
    }
}

/// What a decision's log lines name it (`Charge::name`).
const DECIDE: &str = "decide";

/// `POST /api/models/v1/embeddings[?fragment=<name>]`: an agent's
/// embeddings (fragment_core::embed), OpenAI's shape, on the deployment's
/// embeddings slot; its caller, payer and fragment `route`'s. Checked
/// before anything is sent (`embed::bound`), charged after its answer at
/// its input tokens, and answered with its vectors' space.
pub(crate) async fn embeddings_route(mut req: Request, env: &Env, url: &Url, after: &dyn Background) -> CellResult<Response> {
    let body = read_body(&mut req, embed::BODY_MAX_BYTES).await?;
    let caller = caller(env, &req, url, &body).await?;
    let asked: Value = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("body: {e}")))?;
    drop(body);
    let slot = &Config::from_env(env).models.embeddings;
    let bounded = embed::bound(&asked, slot).map_err(|why| CellError::invalid(why.message()))?;
    drop(asked);
    match embedded(env, &bounded.inputs, &caller.owner, Some(&caller.agent), caller.in_fragment().as_ref(), after, None).await? {
        Embedding::Vectors { vectors, usage } => Ok(Response::from_json(&embed::reply(slot, bounded.format, &vectors, usage.as_ref()))?),
        Embedding::Refused { status, text, .. } => passed(status, text),
    }
}

/// What an embeddings call came to: the inputs' vectors (unit length, in
/// the slot's space) and the usage charged for them, or the model's
/// refusal as it came (it used nothing, and nothing is charged).
pub(crate) enum Embedding {
    Vectors { vectors: Vec<Vec<f32>>, usage: Option<Usage> },
    /// The model's refusal; a 429 names how long to wait
    /// (`embed::retry_after_ms`).
    Refused { status: u16, text: String, retry_after_ms: Option<u64> },
}

/// `inputs` (bounded: `embed::bound`) embedded on the deployment's
/// embeddings slot, metered to `payer` as a model call is (`gate`, then
/// charged after its answer at its input tokens); refused, 403, on a
/// deployment that embeds nothing (`Models::embeddings_refused`: its routes
/// are private and its slot is not). An answer that is not
/// the slot's (another count, another length) is charged, as it was paid
/// for, and refused 502: a vector of another space is never passed on.
/// `laps`, if given, takes its waits: `gate`, `key` and `embed`.
pub(crate) async fn embedded(env: &Env, inputs: &[String], payer: &str, agent: Option<&str>, fragment: Option<&InFragment<'_>>, after: &dyn Background, mut laps: Option<&mut js::Laps>) -> CellResult<Embedding> {
    let cfg = Config::from_env(env);
    if let Some(why) = cfg.models.embeddings_refused() {
        return Err(CellError::new(ErrorCode::Forbidden, why));
    }
    let mut took = |step: &'static str, t0: i64| {
        if let Some(laps) = laps.as_deref_mut() {
            laps.took(step, js::now_ms() - t0);
        }
    };
    let slot = &cfg.models.embeddings;
    let t0 = js::now_ms();
    let mut charge = gate(env, payer, agent, fragment, slot.upstream.model(), embed::EMBEDDINGS, after).await?;
    took("gate", t0);
    let request = embed::request(slot, inputs);
    let meta = Metadata { user_id: opaque(payer), agent_id: agent.map(opaque) };
    let mut t0 = js::now_ms();
    let answered = match &slot.upstream {
        Upstream::WorkersAi { model } => workers_ai(env, cfg, model, &request, &meta).await,
        Upstream::Openai { base_url, .. } => {
            let key = crate::keys::model_key(env, embed::EMBEDDINGS).await?;
            took("key", t0);
            t0 = js::now_ms();
            keyed(&key, base_url, "/embeddings", &request.to_string()).await
        }
    };
    drop(request);
    // not reached: it used nothing, and nothing is charged
    let mut upstream = answered?;
    charge.first_ms = Some(js::now_ms() - charge.t0);
    let status = upstream.status_code();
    let log_id = upstream.headers().get("cf-aig-log-id")?;
    if status != 200 {
        let header = upstream.headers().get("retry-after")?;
        let text = bounded_text(&mut upstream).await;
        took("embed", t0);
        let retry_after_ms = (status == 429).then(|| embed::retry_after_ms(header.as_deref(), &text));
        return Ok(Embedding::Refused { status, text, retry_after_ms });
    }
    let bytes = match read_whole(&mut upstream).await {
        Ok(b) => b,
        Err(e) => {
            // what it used is unknown
            charge.later(after, None, log_id);
            return Err(e);
        }
    };
    took("embed", t0);
    let response: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    drop(bytes);
    let usage = embed::usage_of(slot, inputs, &response);
    charge.later(after, usage.clone(), log_id);
    match embed::answer(slot, inputs.len(), &response) {
        Ok(vectors) => Ok(Embedding::Vectors { vectors, usage }),
        Err(fault) => Err(CellError::new(ErrorCode::UpstreamFailed, format!("{}: the call is charged", fault.message()))),
    }
}

/// `POST /api/models/v1/audio/transcriptions`: an agent's transcription
/// (the module's doc), signed by the agent; OpenAI's multipart shape, its
/// `model` `whisper`, run on the deployment's transcription slot. Its owner
/// pays, after its answer. A deployment that names no slot answers 404,
/// and audio past `transcribe::AUDIO_MAX_BYTES` is 413, before anything is
/// sent.
pub(crate) async fn transcription_route(mut req: Request, env: &Env, url: &Url, after: &dyn Background) -> CellResult<Response> {
    let body = read_body(&mut req, transcribe::BODY_MAX_BYTES).await?;
    let agent = crate::signer_for(env, &req, url, &body).await?;
    if agent.kind != IdentityKind::Agent {
        return Err(CellError::new(ErrorCode::Forbidden, "the model route is an agent's: its owner pays for its calls"));
    }
    let owner = agent.owner.clone().ok_or_else(|| CellError::host("an agent without an owner"))?;
    let cfg = Config::from_env(env);
    let Some(slot) = &cfg.models.transcription else {
        return Err(CellError::new(ErrorCode::NotFound, "this deployment transcribes nothing: its models name no transcription slot (models.transcription)"));
    };
    let content_type = req.headers().get("content-type")?.unwrap_or_default();
    let malformed = |m: multipart::Malformed| CellError::invalid(m.message());
    let boundary = multipart::boundary(&content_type).map_err(malformed)?;
    let parts = multipart::parts(&body, &boundary).map_err(malformed)?;
    let bounded = match transcribe::bound(&parts) {
        Ok(b) => b,
        Err(transcribe::Refusal::TooLarge(n)) => return Err(CellError::too_large("the audio", n, transcribe::AUDIO_MAX_BYTES)),
        Err(why) => return Err(CellError::invalid(why.message())),
    };
    let format = bounded.format;
    let sent = match &slot.upstream {
        Upstream::WorkersAi { model } => Sent::WorkersAi { model, input: transcribe::workers_ai_input(&bounded) },
        Upstream::Openai { base_url, model, .. } => {
            // the platform's own: 128 random bits no audio carries
            let boundary = format!("fragment-{}", js::random_hex::<16>());
            let form = transcribe::openai_form(&bounded, model, &boundary).ok_or_else(|| CellError::host("a form's boundary was found in its audio: send it again"))?;
            Sent::Openai { base_url, content_type: format!("multipart/form-data; boundary={boundary}"), form }
        }
    };
    drop(parts);
    drop(body);
    let model = slot.upstream.model();
    let mut charge = gate(env, &owner, Some(&agent.id), None, model, transcribe::WHISPER, after).await?;
    let meta = Metadata { user_id: opaque(&owner), agent_id: Some(opaque(&agent.id)) };
    let (answer, calls) = transcribed(env, cfg, model, &sent, &meta).await;
    drop(sent);
    charge.calls = calls;
    // not reached: it used nothing, and nothing is charged
    let mut upstream = answer?;
    charge.first_ms = Some(js::now_ms() - charge.t0);
    let status = upstream.status_code();
    let log_id = upstream.headers().get("cf-aig-log-id")?;
    if status != 200 {
        let mut refused = passed(status, bounded_text(&mut upstream).await)?;
        if status == 429 {
            // its client's own retry waits for capacity, not half a second
            refused.headers_mut().set("retry-after", &transcribe::RETRY_AFTER_S.to_string())?;
        }
        return Ok(refused);
    }
    let answer = match read_whole(&mut upstream).await {
        Ok(b) => b,
        Err(e) => {
            // what it used is unknown
            charge.later(after, None, log_id);
            return Err(e);
        }
    };
    let heard: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
    // its shape, never its words (`transcribe::shape`): logged, and told
    // the client (`ANSWER_SHAPE_HEADER`)
    let shape = transcribe::shape(&heard);
    console_log!("{}", json!({ "event": "model.transcribed", "ref": charge.bill.reference, "shape": shape, "bytes": answer.len() }));
    charge.later(after, transcribe::usage_of(slot, &heard), log_id);
    let Some((kind, out)) = transcribe::answer(format, &heard) else {
        return Err(CellError::new(ErrorCode::UpstreamFailed, "the model's transcription carried no text"));
    };
    let mut resp = Response::from_bytes(out)?;
    resp.headers_mut().set("content-type", kind)?;
    resp.headers_mut().set(ANSWER_SHAPE_HEADER, &shape.to_string())?;
    Ok(resp)
}

/// A transcription as its slot is sent it: Whisper's input to a Workers
/// AI model, or OpenAI's form to an OpenAI-compatible API.
enum Sent {
    WorkersAi { model: &'static str, input: Value },
    Openai { base_url: &'static str, content_type: String, form: Vec<u8> },
}

/// The slot's model's answer to a transcription, and the tries it took: a
/// first try that failed fast for want of capacity (`transcribe::again`: a
/// 429, a 5xx, a 408 or no answer, within `AGAIN_WITHIN_MS`) is made once
/// more after `AGAIN_AFTER_MS`, and the second's answer is final. A failed
/// try used nothing and is charged nothing. Workers AI's speech recognition
/// takes 720 calls a minute an account (developers.cloudflare.com/workers-ai/
/// platform/limits, read 2026-10-22); what it refuses past that, or when
/// short of capacity, is a passing 429. Not AI Gateway's own retries: its
/// Retry Requests setting is the gateway's, so it would hold back a chat
/// call's fallback too, and the binding's options name no retry, nor its
/// docs which failures it retries. An OpenAI-compatible API's key is read
/// once, before the first try: a deployment that binds none has its call
/// not reached, charged nothing.
async fn transcribed(env: &Env, cfg: &Config, model: &str, sent: &Sent, meta: &Metadata) -> (CellResult<Response>, u32) {
    let key = match sent {
        Sent::Openai { .. } => match crate::keys::model_key(env, transcribe::TRANSCRIPTION).await {
            Ok(k) => k,
            Err(e) => return (Err(e), 0),
        },
        Sent::WorkersAi { .. } => String::new(),
    };
    let t0 = js::now_ms();
    let first = send(env, cfg, sent, &key, meta).await;
    let status = first.as_ref().ok().map(Response::status_code);
    let ms = js::now_ms() - t0;
    if !transcribe::again(status, ms) {
        return (first, 1);
    }
    // where the model's error is (the gateway's log of the call), never
    // its body: an answer whose headers came fast can still stall its body,
    // and a read of it would hold back the second try past what its client
    // waits (Alex's review of #520)
    let why = match first {
        Ok(failed) => failed.headers().get("cf-aig-log-id").ok().flatten().map(|id| format!("cf-aig-log-id {id}")).unwrap_or_default(),
        Err(e) => e.message,
    };
    console_log!("{}", json!({ "event": "model.transcribe-again", "model": model, "status": status, "ms": ms, "why": why }));
    Delay::from(std::time::Duration::from_millis(transcribe::AGAIN_AFTER_MS)).await;
    (send(env, cfg, sent, &key, meta).await, 2)
}

/// One try of a transcription (`transcribed`), where its slot runs: `key`
/// an OpenAI-compatible API's.
async fn send(env: &Env, cfg: &Config, sent: &Sent, key: &str, meta: &Metadata) -> CellResult<Response> {
    match sent {
        Sent::WorkersAi { model, input } => workers_ai(env, cfg, model, input, meta).await,
        Sent::Openai { base_url, content_type, form } => keyed_as(key, base_url, transcribe::OPENAI_PATH, content_type, worker::js_sys::Uint8Array::from(form.as_slice()).into()).await,
    }
}

/// A transcription's answer names the shape of the model's own (its
/// top-level keys, and a `usage` of scalars), never its words: how the
/// hosted lane sees what Workers AI answers (docs/api.md).
pub const ANSWER_SHAPE_HEADER: &str = "x-fragment-answer-shape";
