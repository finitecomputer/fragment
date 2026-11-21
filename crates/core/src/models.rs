//! The model route's bounds (docs/cloudflare-v1.md, decisions 23 and 61;
//! "Lessons from cloudflare/agents" 7 and 8; spike S4). Pure:
//! cell/src/models.rs makes the calls and meters them on the payer's ledger.
//!
//! A deployment's `models` (`Models`) are its slots, each where one model
//! runs, and its routes, what a person picks for an agent (Paul,
//! 2026-11-17: "we don't want to hardcode what they all mean"): each route
//! a name, a label, the slot its turns run on, its vision and its Deep, one
//! the default. Nothing here reads a name for what it means: a route is
//! private, sees images, falls back, by what its config says.
//!
//! A call names one of the route's slots (`Named`): a route, by its name,
//! which picks the model and caps what one call may write; `vision`, its
//! caller's route's vision model, for an agent runtime's calls about an
//! image (Hermes' auxiliary vision, which reads its `computer_use`
//! screenshots); or `fallback`, its caller's route's fallback, for a
//! runtime's own switch when its model stalls (Hermes'
//! `fallback_providers`, after its stale detector gives up on a call that
//! streams nothing). Neither of those two is a route: an agent, a job's
//! step or a manifest names none but the routes. An agent's call may name
//! a slot by its model's id instead (`Models::named`; Paul, 2026-10-16: "be
//! able to run `/model` with hermes and see the real model"): its route's
//! slot when it says its route (`TIER_HEADER`) and that slot runs the id,
//! else the first that does. `Models::ids` are what it may name (`GET
//! /v1/models`).
//!
//! Each slot runs where the deployment says: a Workers AI model through
//! its AI Gateway, or a model on an OpenAI-compatible API, its key a
//! Secrets Store secret (`Upstream`). A deployment that names none runs
//! two routes on Workers AI (`default_config`); Finite's own runs Private
//! on Finite Private and Standard through OpenRouter (Paul, 2026-10-15;
//! decision 61). What the
//! platform sends is the client's OpenAI-shaped chat completion with only
//! these changes (`bound`): no `model` (the route's is the call's),
//! `max_tokens` at most the slot's cap, `reasoning_effort` one of
//! `EFFORTS`, `low`, `high` or `max`, and any other `low` (GLM takes a
//! missing or unknown one as `max`, the dearest, which a call buys only by
//! its name; Finite Private's limiter fills in `high` only when none is
//! sent), and usage asked for when it streams. On an OpenAI-compatible API, only the client's chat completion
//! fields of `CLIENT_KEYS` go (`plugins`, `provider` and the like are
//! dropped), and the deployment's own fields for it over them
//! (`Upstream::Openai`'s `body`: OpenRouter's provider routing;
//! `openai_request`). Nothing else of the client's reaches the vendor: no
//! header, no key.
//!
//! A call that its model fails before answering anything (it is not
//! reached, or it answers 429 or a 5xx; on an OpenAI-compatible API, whose
//! key is the platform's, also 401, 402, 403 or 404) is made once more, the
//! same, on its slot's own fallback (`Tries`, `falls_back`,
//! `Models::fallback_of`; Paul, 2026-10-08: "we def need fallback
//! models"): the one its slot names, whether the call named the slot by a
//! route's name, an alias or its model's id. Not a race: the second call is made only after the first has
//! failed. An answer that began is final, and the fallback's failure is
//! the call's.
//!
//! What a call cost is read from its answer's usage (`usage_of`), priced
//! as the model that answered it, and charged after it (decision 62). A streamed answer
//! carries usage on every chunk, each a per-chunk delta, then a line of its
//! own with the whole call's (spike S4): only that last, cumulative one is
//! metered, and the client reads OpenAI's shape, usage once on a last chunk
//! with no choices (`Stream`). An OpenAI-compatible API streams that shape
//! already, and is metered from its last chunk. A stream's answer is whole
//! at its `[DONE]`: a reader stops there (the gateway has held a stream
//! open for minutes after it, 2026-10-07).
//!
//! A job's text step (`text_body`) is such a call, streamed, its tools
//! bounded (`TOOLS_MAX`, `TOOLS_MAX_BYTES`) and its request too
//! (`AI_TEXT_MAX_BYTES`), its answer read into one message as it streams
//! (`Stream::answering`, `Answer`): the text, the tool calls, why it
//! stopped, and never the model's reasoning, of which only its length is
//! counted (a draft's "thinking").

use fragment_proto::{valid_channel_name, valid_op_id, BUILTIN_CHANNELS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::price::{ModelPrice, PriceBook, TokenPrices, Usage};
use crate::steps::AiText;
use crate::gather::Gathered;

/// DeepSeek V4.1 Flash and Inkling 2 Small, as Workers AI's catalog names
/// them: each id once, each role below naming its model. Both take tools,
/// images and `reasoning_effort` `low`, `high` and `max`, with 1,048,576
/// tokens of context (developers.cloudflare.com/workers-ai/models/
/// deepseek-v4.1-flash-1017/ and inkling-2-small/, read 2026-11-16). They
/// replace GLM-5.3 Flash (`@cf/zai-org/glm-5.3-flash`) and DeepSeek V4
/// Flash (`@cf/deepseek-ai/deepseek-v4-flash-0731`), which Workers AI
/// answers `410 Gone` from 2026-11-20 (its changelog, 2026-11-05: "Planned
/// model deprecations on Workers AI"); their rows stay in the price book
/// (crate::price::DEFAULT_MODELS) for the usage stored and queued under them.
pub const DEEPSEEK_FLASH: &str = "@cf/deepseek-ai/deepseek-v4.1-flash-1017";
pub const INKLING_SMALL: &str = "@cf/thinkingmachines/inkling-2-small";
/// Finite Private's GLM-5.3 Flash, as its API names it: Finite's own
/// GLM-5.3 Flash (`zai-org/GLM-5.3-Flash` on SGLang, 393,216 tokens of
/// context) on Tinfoil, behind finite-mono's `finite-private-limiter` at
/// `https://finite-private.finite.containers.tinfoil.dev/v1` (finite-mono
/// `infra/tinfoil/confidential-finite-private`, read 2026-10-14). It calls
/// tools and streams; its images are not qualified (decision 61). Also its
/// row in the price book.
pub const FINITE_PRIVATE_FLASH: &str = "glm-5-3-flash";
/// GLM-5.3 Flash as OpenRouter names it (`https://openrouter.ai/api/v1`):
/// Finite's tiers' since 2026-10-15 (decision 61), on the providers its
/// slot's `body` lets OpenRouter route to (zero data retention, the
/// highest throughput under a price ceiling). 34 provider endpoints on
/// 2026-10-15 (`/api/v1/models/z-ai/glm-5.3-flash/endpoints`), each taking
/// tools, `reasoning_effort` and `max_tokens`. Priced, as any model there,
/// at its slot's ceiling (`Upstream::ceiling`); its row in the price book
/// stays for its usage queued past a switch.
pub const OPENROUTER_FLASH: &str = "z-ai/glm-5.3-flash";
/// OpenRouter's API's host: a slot there names its price ceiling
/// (`Upstream::ceiling`).
pub const OPENROUTER_HOST: &str = "openrouter.ai";
/// The highest ceiling a slot names, in dollars a million tokens: far past
/// any model's list price (Opus 5.5's output is $20), so one higher is a
/// typo, refused rather than charged.
pub const CEILING_MAX_USD: u32 = 1_000;
/// The default config's routes' models (`DEFAULT_CONFIG`; decision 23):
/// what a deployment that names no `models` runs, the dev stack's and the
/// e2e's plain node's. Its first route runs DeepSeek V4.1 Flash, Inkling 2
/// Small its fallback (`FALLBACK_MODEL_DEFAULT`). Until 2026-11-20 it was
/// GLM-5.3 Flash with DeepSeek V4 Flash behind it (Paul, 2026-10-09: "GLM
/// default, DeepSeek fallback"), both retired then. DeepSeek V4.1 Flash
/// is Workers AI's suggested replacement for both, and the cheaper of
/// the two that take the route's requests (tools, `reasoning_effort` up to
/// `max`, `max_tokens` up to `MAX_TOKENS`) per agent turn, whose input is
/// mostly cached: $0.006 a million cached tokens, Inkling's $0.10.
pub const CHEAP_MODEL: &str = DEEPSEEK_FLASH;
pub const MEDIUM_MODEL: &str = "@cf/zai-org/glm-5.3";
/// The route's name for a caller's route's vision model (`Named::Vision`).
pub const VISION: &str = "vision";
/// The route's name for a caller's route's fallback model (`Named::Fallback`).
pub const FALLBACK: &str = "fallback";
/// The default config's vision model (Paul, 2026-10-05, then GLM-5.3
/// Flash): DeepSeek V4.1 Flash, "Vision: Yes" in Workers AI's catalog
/// (read 2026-11-16), and its first route's, as GLM-5.3 Flash was.
/// GLM-5.3, its second route's, takes no images.
pub const VISION_MODEL_DEFAULT: &str = DEEPSEEK_FLASH;
/// The default config's fallback model (Paul, 2026-10-08: "we def need
/// fallback models"): Inkling 2 Small, on Workers AI, a model of its own
/// beside DeepSeek (its own replicas, its own rate limit). A fallback is
/// another model (`Tries::after`), and of Workers AI's that take the
/// route's requests it is the only one neither route runs: Qwen 3.8 27B
/// takes `reasoning_effort` `low`, `medium` or `xhigh` only, and Gemma 4
/// 26B none at all (their catalog entries, read 2026-11-16). It reads
/// images, but the default config does not say so, so its vision model has
/// no fallback.
pub const FALLBACK_MODEL_DEFAULT: &str = INKLING_SMALL;
/// The most slots a deployment's `models` names: one key binding each.
/// Twelve leaves room to add a route by config alone beside production's
/// eight; the deploy bounds the whole config by its Workers variable's
/// 5 KB too (xtask/src/deploy.rs).
pub const SLOTS_MAX: usize = 12;
/// The most routes a deployment offers: a menu's entries.
pub const ROUTES_MAX: usize = 8;
/// A slot's or a route's name's most bytes.
pub const SLOT_NAME_MAX_BYTES: usize = 24;

/// Whether `name` may name a slot of a deployment's `models`, or a route:
/// 1 to `SLOT_NAME_MAX_BYTES` of lower-case letters, digits and `_`, a
/// letter first (a slot's key's binding is its name in capitals:
/// `secrets_store::model_key`; a route's is stored in agents' `agent.json`
/// and said in `TIER_HEADER`).
pub fn valid_slot_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=SLOT_NAME_MAX_BYTES).contains(&b.len()) && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}
/// The header an agent's runtime says its route in (`Models::named`), by
/// the route's name: of two slots running the id a call names, its route's
/// is the call's (Hermes' `model.default_headers`). Optional; it names no
/// model by itself. Named for the tiers the routes were before 2026-11-17.
pub const TIER_HEADER: &str = "x-fragment-tier";
/// The largest image Hermes sends for its vision call after a size
/// refusal: it shrinks one to this many bytes of base64 data URL and tries
/// again once (its `tools/vision_tools.py`, `_RESIZE_TARGET_BYTES`, which
/// its config does not set).
pub const IMAGE_DATA_URL_MAX_BYTES: usize = 5 * 1024 * 1024;
/// A call's request: an image of `IMAGE_DATA_URL_MAX_BYTES` and a MiB for
/// the rest of it (Hermes' prompt about a screenshot carries the screen's
/// element list), so the call Hermes retries after a 413 fits. GLM's
/// million-token window is about 4 MB of text; an agent's window sends a
/// few hundred KiB. Finite Private's limiter takes at most 2 MiB a request
/// (its 413 past that is the call's): a text tier's call there is that
/// size at most, and `vision` stays on Workers AI (decision 61).
pub const MODEL_BODY_MAX_BYTES: usize = IMAGE_DATA_URL_MAX_BYTES + 1024 * 1024;
const _: () = assert!(MODEL_BODY_MAX_BYTES > IMAGE_DATA_URL_MAX_BYTES && MODEL_BODY_MAX_BYTES < 8 * 1024 * 1024, "the cap fits Hermes' shrunk image, and stays near it");
/// The most one call may write, reasoning included: a job's text step may
/// ask for up to this.
/// It bounds what one call can cost: GLM-5.3's is $0.11 of output at
/// list price, before its input. A slot may name its own (`"max_tokens"`,
/// at most `MAX_TOKENS_MAX`): Deep's, which thinks at `max` (Paul,
/// 2026-10-27: 32,768).
pub const MAX_TOKENS: u32 = 16_384;
/// The most a slot's own cap may be (`SlotOpts::max_tokens`): DeepSeek V4.1
/// Flash's 65,536 tokens of output at Tinfoil's $1.45 a million is $0.095
/// at list, before its input.
pub const MAX_TOKENS_MAX: u32 = 65_536;
/// The reasoning efforts a call is sent as it asked: `low`, `high`, and
/// `max`, the dearest, by its name alone (a mode thinks at it: Deep, Paul,
/// 2026-10-27). Anything else asked for is `low`: GLM maps `none`,
/// `minimal`, `medium` and `xhigh` to `max`, its default, so passing them
/// on would buy the most expensive unasked (Hermes' custom provider sends
/// `medium` when its config names none).
pub const EFFORTS: [&str; 3] = ["low", "high", "max"];
/// One line of a streamed answer: Workers AI's chunks are a few hundred
/// bytes. A longer line is passed through unread, and the call reports no
/// usage (its usage cannot be trusted): charged nothing, said aloud.
pub const SSE_LINE_MAX: usize = 1024 * 1024;
/// A text step's tools: at most this many, in at most this many bytes of
/// JSON (what the model reads on every call).
pub const TOOLS_MAX: usize = 64;
pub const TOOLS_MAX_BYTES: usize = 64 * 1024;
/// A function's name, as OpenAI takes one: 1 to 64 of letters, digits,
/// `_` and `-`.
pub const TOOL_NAME_MAX_BYTES: usize = 64;
/// The tool calls one answer is read for: a delta for a later index is
/// out of the stream's contract, and dropped.
pub const TOOL_CALLS_MAX: usize = 128;
/// A text step's arguments (its messages and tools, as JSON): half a
/// Workflow step's result, which carries them to the supervisor
/// (`limits::RESULT_MAX_BYTES`), so a long tool loop is refused in the
/// app's realm (platform.mjs) and here, saying why, never by the Workflow.
pub const AI_TEXT_MAX_BYTES: usize = 512 * 1024;
const _: () = assert!(AI_TEXT_MAX_BYTES * 2 <= fragment_proto::limits::RESULT_MAX_BYTES && AI_TEXT_MAX_BYTES < MODEL_BODY_MAX_BYTES);

/// Where one of the route's models runs (a deployment's `models`, each
/// slot one of these, `provider` its kind). Its `model` is also its row in
/// the price book (the code's, or its slot's ceiling: `ceiling`): a call is
/// priced as the model that answered it. Read as its fields
/// (`UpstreamFields`), not as serde's internally tagged reading, which
/// buffers each slot to find its `provider`: about 10 KB of the cell's Wasm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", try_from = "UpstreamFields")]
pub enum Upstream {
    /// A Workers AI model by its catalog id, through the deployment's AI
    /// Gateway (the Worker's AI binding; dev and the e2e, the fake at
    /// `FRAGMENT_AI_URL`).
    WorkersAi { model: String },
    /// A model on an OpenAI-compatible API (OpenRouter's, Finite
    /// Private's, Tinfoil's): `POST <base_url>/chat/completions`, its
    /// `model` the id there, with the key the deployment binds for the slot
    /// naming it (`secrets_store::model_key`) as a bearer token. `body`,
    /// when named, is fields of the API's own that every call to it
    /// carries, over any of the client's of the same name
    /// (`openai_request`): OpenRouter's `provider` routing, say. It names
    /// none of the keys the route writes (`ROUTE_KEYS`), in at most
    /// `UPSTREAM_BODY_MAX_BYTES` (`check`). `price`, when named, is its
    /// model's list price there (`ListPrice`; `Upstream::own_price`).
    Openai {
        base_url: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "fields")]
        body: Option<Map<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "price_fields")]
        price: Option<Map<String, Value>>,
    },
}

/// An `Upstream` as it is read: both providers' fields, flat, none unknown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamFields {
    provider: Provider,
    model: String,
    base_url: Option<String>,
    #[serde(default, deserialize_with = "fields")]
    body: Option<Map<String, Value>>,
    #[serde(default, deserialize_with = "price_fields")]
    price: Option<Map<String, Value>>,
}

/// A slot's `provider`: read as an identifier, a string alone, as the
/// tagged reading read its tag (a derived enum also takes `{"<name>": null}`).
#[derive(Deserialize)]
#[serde(variant_identifier, rename_all = "snake_case")]
enum Provider {
    WorkersAi,
    Openai,
}

impl TryFrom<UpstreamFields> for Upstream {
    type Error = String;

    /// The provider its `provider` names, with that provider's fields alone.
    fn try_from(f: UpstreamFields) -> Result<Upstream, String> {
        match f.provider {
            Provider::WorkersAi => match [("base_url", f.base_url.is_some()), ("body", f.body.is_some()), ("price", f.price.is_some())].into_iter().find(|(_, named)| *named) {
                Some((field, _)) => Err(format!("unknown field `{field}`: a Workers AI model is its `model` alone")),
                None => Ok(Upstream::WorkersAi { model: f.model }),
            },
            Provider::Openai => Ok(Upstream::Openai { base_url: f.base_url.ok_or("missing field `base_url`")?, model: f.model, body: f.body, price: f.price }),
        }
    }
}

/// The fields a slot's `price` names (`Upstream::own_price`): a model's
/// list price at an API that is not OpenRouter, as its vendor lists it,
/// dollars a million tokens of input (uncached), of cached input (`input`'s
/// when unnamed: never cheaper than it could be), and of output. Its row
/// in the book is that list, with the book's credits fee and margin on it
/// as on any model's (a vendor billing a card bills no such fee: such a
/// model is charged that much above its cost, never below). A cache write,
/// which none of these APIs bills, is at `input`. So a model on any
/// OpenAI-compatible API is the config's alone (Paul, 2026-10-22: "we try
/// a config out on preview, if we don't like it we revert it"). Read as
/// JSON, as a ceiling is, so it costs the cell's wasm no serde of its own.
pub const PRICE_FIELDS: [&str; 3] = ["input", "cached_input", "output"];

/// A slot's `price`: a JSON object, or refused saying so.
fn price_fields<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Map<String, Value>>, D::Error> {
    match Value::deserialize(d)? {
        Value::Object(o) => Ok(Some(o)),
        other => Err(serde::de::Error::custom(format!("price is a JSON object, {{\"input\": <$>, \"cached_input\"?: <$>, \"output\": <$>}} a million tokens, not {other}"))),
    }
}

/// Dollars a million tokens as the book's micro-dollars a million, rounded
/// up past a float's last digit (never below what was named), or `None`
/// when they are not from 0 to `CEILING_MAX_USD`.
fn usd_a_million(v: f64) -> Option<i64> {
    (0.0..=f64::from(CEILING_MAX_USD)).contains(&v).then(|| (v * 1e6 - 1e-6).ceil().max(0.0) as i64)
}

/// A slot's `body`: a JSON object, or refused saying so.
fn fields<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Map<String, Value>>, D::Error> {
    match Value::deserialize(d)? {
        Value::Object(o) => Ok(Some(o)),
        other => Err(serde::de::Error::custom(format!("body is a JSON object of the fields every call to this API carries, not {other}"))),
    }
}

/// The keys the route itself writes into a call's body (`bound`,
/// `text_body`, `openai_request`): a slot's `body` names none of them, so
/// the tier's model, its cap, its effort, the stream and its usage, and
/// the messages and tools stay the route's.
pub const ROUTE_KEYS: [&str; 9] = ["model", "messages", "stream", "stream_options", "tools", "tool_choice", "max_tokens", "max_completion_tokens", "reasoning_effort"];
/// A slot's `body`, as JSON: an API's own fields (OpenRouter's provider
/// routing is about 150 bytes), never a prompt. Two slots this large
/// would not fit FRAGMENT_MODELS, a Workers variable of at most 5 KB: the
/// deploy checks the whole variable (xtask/src/deploy.rs).
pub const UPSTREAM_BODY_MAX_BYTES: usize = 4 * 1024;

/// The client's fields an OpenAI-compatible API is sent (`openai_request`):
/// OpenAI's chat completion fields that the route, our jobs and Hermes use
/// (Hermes v0.21.6 sends a custom provider `messages`, `tools`,
/// `temperature`, `max_tokens` or `max_completion_tokens`, `stream` and
/// `stream_options`: its `agent/transports/chat_completions.py`). Any other
/// (`plugins`, `web_search_options`, `transforms`, `provider`, `models`,
/// `n`, `user`, …) is dropped: a vendor's paid extra the book does not
/// price, a route to another model, or a parameter its endpoints may not
/// take (under OpenRouter's `require_parameters`, one would leave none).
pub const CLIENT_KEYS: [&str; 14] = [
    "messages", "tools", "tool_choice", "temperature", "top_p", "max_tokens", "max_completion_tokens", "stop", "response_format", "seed", "reasoning_effort", "parallel_tool_calls", "stream", "stream_options",
];

/// The body a call to an OpenAI-compatible API sends, as JSON: the bounded
/// `input`'s fields of `CLIENT_KEYS` and no other, the slot's `body` fields
/// over them, and the model's `id`. Written from the input's own values
/// (a prompt is up to `MODEL_BODY_MAX_BYTES`), their keys sorted as a
/// `Map`'s are.
pub fn openai_request(input: &Value, id: &str, body: Option<&Map<String, Value>>) -> String {
    let input = input.as_object().expect("a bounded input is an object");
    let model = Value::from(id);
    let mut request: BTreeMap<&str, &Value> = input.iter().filter(|(k, _)| CLIENT_KEYS.contains(&k.as_str())).map(|(k, v)| (k.as_str(), v)).gathered();
    for (k, v) in body.into_iter().flatten() {
        assert!(!ROUTE_KEYS.contains(&k.as_str()), "a slot's body names none of the route's keys (`check`)");
        request.insert(k, v);
    }
    request.insert("model", &model);
    serde_json::to_string(&request).expect("a JSON value serializes")
}

impl Upstream {
    /// Its model's id, and its row in the price book.
    pub fn model(&self) -> &str {
        match self {
            Upstream::WorkersAi { model } | Upstream::Openai { model, .. } => model,
        }
    }

    /// Its model's row in the price book when its slot sets one: the
    /// ceiling OpenRouter's provider routing in its `body` routes under
    /// (`provider.max_price`, dollars a million tokens of `prompt` and of
    /// `completion`; openrouter.ai/docs/guides/routing/provider-selection).
    /// OpenRouter bills a call its provider's price, at most that ceiling,
    /// so the book charges the ceiling, never less than it could bill
    /// (docs/ledger.md, OpenRouter's price): every input token, cached and
    /// written ones too (no endpoint's cache price is bounded by it), at
    /// `prompt`, and output at `completion`, each grossed up for
    /// OpenRouter's fee (`price::openrouter`). Whatever model a slot runs
    /// there is priced so, by the config alone (Paul, 2026-10-22). None
    /// without a ceiling; refused, saying why: a slot on OpenRouter's own
    /// API (`OPENROUTER_HOST`) without one, and a ceiling that is not two
    /// numbers from 0 to `CEILING_MAX_USD`.
    pub fn ceiling(&self) -> Result<Option<TokenPrices>, String> {
        let Upstream::Openai { base_url, body, .. } = self else { return Ok(None) };
        let Some(max) = body.as_ref().and_then(|b| b.get("provider")).and_then(|p| p.get("max_price")) else {
            if on_openrouter(base_url) {
                return Err("a model on OpenRouter is priced at the ceiling its calls route under: name it in body.provider.max_price, {\"prompt\": <$>, \"completion\": <$>} a million tokens".into());
            }
            return Ok(None);
        };
        let usd = |field: &str| match max.get(field).and_then(Value::as_f64).and_then(usd_a_million) {
            // never below the ceiling, with OpenRouter's fee
            Some(micros) => Ok(crate::price::openrouter(micros)),
            None => Err(format!("body.provider.max_price.{field} is the dollars a million tokens its calls are priced at, from 0 to {CEILING_MAX_USD}, not {}", max.get(field).map_or("nothing".into(), Value::to_string))),
        };
        let (prompt, completion) = (usd("prompt")?, usd("completion")?);
        Ok(Some(TokenPrices { input: prompt, cached_input: prompt, cache_write: prompt, output: completion }))
    }

    /// Its model's row in the price book when its slot sets one: the list
    /// price it names (`ListPrice`), or, on OpenRouter, its ceiling
    /// (`ceiling`). None when it names neither (the book's row of its id
    /// prices it). Refused, saying why: a `price` on OpenRouter (which
    /// bills each call its provider's price, up to the ceiling: the ceiling
    /// is the price), a `price` beside a ceiling, and a `price` that is not
    /// dollars from 0 to `CEILING_MAX_USD`.
    pub fn own_price(&self) -> Result<Option<TokenPrices>, String> {
        let Upstream::Openai { base_url, price: Some(p), .. } = self else { return self.ceiling() };
        if on_openrouter(base_url) {
            return Err("a model on OpenRouter is priced at its ceiling (body.provider.max_price), not a price of its own: OpenRouter bills each call its provider's price, up to the ceiling".into());
        }
        if self.ceiling()?.is_some() {
            return Err("name its price, or a ceiling (body.provider.max_price), not both".into());
        }
        if let Some(other) = p.keys().find(|k| !PRICE_FIELDS.contains(&k.as_str())) {
            return Err(format!("price names {other:?}: it names input, cached_input (optional) and output"));
        }
        let usd = |field: &str| match p.get(field) {
            None => Ok(None),
            Some(v) => v.as_f64().and_then(usd_a_million).map(Some).ok_or_else(|| format!("price.{field} is the dollars a million tokens its API lists, from 0 to {CEILING_MAX_USD}, not {v}")),
        };
        let named = |field: &str| usd(field)?.ok_or_else(|| format!("price.{field} is the dollars a million tokens its API lists, from 0 to {CEILING_MAX_USD}, not nothing"));
        let input = named("input")?;
        Ok(Some(TokenPrices { input, cached_input: usd("cached_input")?.unwrap_or(input), cache_write: input, output: named("output")? }))
    }
}

/// What OpenRouter's answers say each call cost (FIN-616): `usage.cost`,
/// dollars of its credits (on every answer, a stream's on its last usage
/// chunk: openrouter.ai/docs/cookbook/administration/usage-accounting,
/// read 2026-11-21), as the book's units of it, nano-dollars.
pub const OPENROUTER_COST: &str = "openrouter:nanousd";
/// A nano-dollar of OpenRouter's credits at list, as the book prices it:
/// micro-dollars per nano-dollars, grossed up by OpenRouter's fee on the
/// credits (`price::OPENROUTER_FEE_BP`), which marks up no call's cost.
pub const OPENROUTER_COST_PRICE: (i64, u64) = ((1_000 * (crate::price::BP + crate::price::OPENROUTER_FEE_BP) / crate::price::BP) as i64, 1_000_000);
const _: () = assert!(OPENROUTER_COST_PRICE.0 == 1_055, "a dollar of OpenRouter's credits costs $1.055");

impl Upstream {
    /// What one call on this upstream used, as its answer's `usage` says
    /// (FIN-616): on OpenRouter, the cost it names (`OPENROUTER_COST`'s
    /// nano-dollars, rounded to the nearest), charged as it is, never its
    /// slot's ceiling; else, or with none, its tokens (`usage_of`), priced
    /// at its model's row. A cost that is no finite, non-negative number of
    /// dollars under `price::CHARGE_MAX` is none: its tokens price it.
    pub fn used(&self, model: &str, usage: &Value) -> Option<Usage> {
        let reported = match self {
            Upstream::Openai { base_url, .. } if on_openrouter(base_url) => usage.get("cost").and_then(Value::as_f64),
            _ => None,
        };
        let max = crate::price::CHARGE_MAX as f64 / 1e6;
        match reported.filter(|usd| usd.is_finite() && (0.0..=max).contains(usd)) {
            Some(usd) => Some(Usage::Key { key: OPENROUTER_COST.to_string(), units: (usd * 1e9).round() as u64 }),
            None => usage_of(model, usage),
        }
    }
}

/// Whether an OpenAI-compatible API's `base_url` is OpenRouter's or one of
/// its subdomains: by its host as `check` reads it (`catalog::row_url`,
/// parsed and normalized: lower case, https's port 443 dropped), so no
/// spelling of the URL that reaches OpenRouter passes for another API's.
fn on_openrouter(base_url: &str) -> bool {
    crate::catalog::row_url(base_url, true).is_ok_and(|host| host.strip_suffix(OPENROUTER_HOST).is_some_and(|sub| sub.is_empty() || sub.ends_with('.')))
}

/// The deployment's models (`FRAGMENT_MODELS`: the deploy config's
/// `models`, less its keys): its slots, each where one model runs and what
/// it says of itself, and its routes, what a person picks for an agent
/// (Paul, 2026-11-17: "I should easily be able to list out models I want
/// available, and change models. so we have a model list. we can have pet
/// names for them. but we don't want to hardcode what they all mean."). A
/// route does only what its config says: it is private when its slot says
/// so, its images go to the slot it names as its vision, its calls fall
/// back as its slot does. Nothing here reads a route's or a slot's name for
/// what it means. A deployment that names none runs `default_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Models {
    /// Every slot, in its name's order (its config's object's).
    pub slots: Vec<ModelSlot>,
    /// The routes, in the order Settings offers them.
    pub routes: Vec<Route>,
    /// The route that agents, jobs' steps and calls naming none run on: its
    /// place in `routes`.
    pub default: usize,
    /// Where the route's embeddings run, and their space (`models.
    /// embeddings`, crate::embed): no chat slot, so no call names it as
    /// one, and no route falls back to it.
    pub embeddings: crate::embed::Slot,
}

/// A slot (`models.slots.<name>`): where its model runs, whether it says
/// its model reads images (`"images": true`: an image call falls back only
/// to one that does, `fallback_for_input`) and is private (`"private":
/// true`: confidential inference that keeps nothing, Finite Private's or
/// Tinfoil's; never Workers AI or OpenRouter), the slot its calls are made
/// on once more when its model fails before answering (`"fallback"`), and
/// what it says of its calls (`SlotOpts`). What it reads and keeps is read
/// off it before it is parsed as an `Upstream`: what a model reads is not
/// where it runs, and a field of the upstream's own would cost the cell's
/// wasm 10 KB of serde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSlot {
    pub name: String,
    pub upstream: Upstream,
    pub images: bool,
    pub private: bool,
    /// Its fallback's place in `Models::slots`.
    pub fallback: Option<usize>,
    pub opts: SlotOpts,
}

/// A route (`models.routes[]`): its name, which an agent's `agent.json`
/// stores (`tier`) and its calls say (`TIER_HEADER`); what Settings calls
/// it (`label`; its name when it has none); the slot its turns run on (its
/// Fast); the slot its images go to (`vision`: its own slot unless named);
/// and the slot its agents' delegates run on (`deep`: Paul, 2026-10-27).
/// Paul, 2026-10-24: "A fallback on private should fall back to something
/// private, a fallback on normal should fall back to something normal": a
/// route falls back as its slot does, and a private route reaches only
/// private slots (`reached`, `private_closed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub label: Option<String>,
    pub slot: usize,
    pub vision: usize,
    pub deep: Option<usize>,
}

/// What a slot says of the calls run on it, beside where it runs: the
/// effort a mode on it thinks at (`"reasoning"`, one of `EFFORTS`: a
/// route's Fast's, and its Deep's; Paul, 2026-10-27), which the computer's
/// view carries for its runtime to send; and the most one call of it
/// writes (`"max_tokens"`, 1 to `MAX_TOKENS_MAX`; `MAX_TOKENS` unnamed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotOpts {
    pub reasoning: Option<&'static str>,
    pub max_tokens: Option<u32>,
}

/// A slot's `"reasoning"` and `"max_tokens"`, taken off it (`at`, where
/// it is; `o`, its object). Refused, saying why: an effort the route does
/// not pass (`EFFORTS`), a cap that is no count of 1 to `MAX_TOKENS_MAX`.
fn opts_named(at: &str, o: &mut Map<String, Value>) -> Result<SlotOpts, String> {
    let reasoning = match o.remove("reasoning") {
        None => None,
        Some(v) => Some(EFFORTS.into_iter().find(|e| v.as_str() == Some(e)).ok_or_else(|| format!("{at}: reasoning is {}, not {v}", EFFORTS.join(", ")))?),
    };
    let max_tokens = match o.remove("max_tokens") {
        None => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=MAX_TOKENS_MAX).contains(n)).ok_or_else(|| format!("{at}: max_tokens is the most one call of it writes, 1 to {MAX_TOKENS_MAX}, not {v}"))?),
    };
    Ok(SlotOpts { reasoning, max_tokens })
}

/// A label's most characters: a menu entry's words.
pub const LABEL_MAX_CHARS: usize = 40;

/// The models a deployment that names none runs (dev's, a plain e2e
/// node's): two routes on Workers AI, `cheap` (the default) on
/// `CHEAP_MODEL` and `medium` on `MEDIUM_MODEL`, each falling back to
/// `FALLBACK_MODEL_DEFAULT` and sending images to `VISION_MODEL_DEFAULT`,
/// no label (Settings offers each by its name). A config, as a
/// deployment's: nothing here gives the names a meaning.
fn default_config() -> Value {
    let workers_ai = |model: &str| json!({ "provider": "workers_ai", "model": model });
    let mut slots = Map::new();
    for (name, model) in [("cheap", CHEAP_MODEL), ("medium", MEDIUM_MODEL)] {
        let mut slot = workers_ai(model);
        slot[FALLBACK] = json!("fallback");
        slots.insert(name.into(), slot);
    }
    slots.insert("fallback".into(), workers_ai(FALLBACK_MODEL_DEFAULT));
    slots.insert("vision".into(), workers_ai(VISION_MODEL_DEFAULT));
    let route = |name: &str| json!({ "name": name, "slot": name, "vision": "vision" });
    json!({ "default": "cheap", "routes": [route("cheap"), route("medium")], "slots": slots })
}

/// The slots of a deployment's `models` (`models.slots`), each with its
/// flags, options and fallback's name, which is checked once every slot is
/// read. Refused, saying why: none or more than `SLOTS_MAX`, a name that is
/// no slot's (`valid_slot_name`), a slot out of shape (`Upstream`'s
/// refusals).
fn slots_of(slots: Map<String, Value>) -> Result<Vec<(ModelSlot, Option<String>)>, String> {
    if slots.is_empty() || slots.len() > SLOTS_MAX {
        return Err(format!("models.slots: 1 to {SLOTS_MAX} slots, each where a model runs, not {}", slots.len()));
    }
    let mut out = Vec::with_capacity(slots.len());
    for (name, mut row) in slots {
        let at = format!("models.slots.{name}");
        if !valid_slot_name(&name) {
            return Err(format!("models.slots: {name:?} is no slot's name: 1 to {SLOT_NAME_MAX_BYTES} of a-z, 0-9 and _, a letter first"));
        }
        let o = row.as_object_mut().ok_or_else(|| format!("{at}: a slot is where a model runs, an object"))?;
        let fallback = match o.remove(FALLBACK) {
            None => None,
            Some(Value::String(to)) => Some(to),
            Some(other) => return Err(format!("{at}: fallback names the slot its calls fall back to, not {other}")),
        };
        let opts = opts_named(&at, o)?;
        let mut flag = |key: &str, means: &str| match o.remove(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(b),
            Some(other) => Err(format!("{at}: {key} is true ({means}) or false, not {other}")),
        };
        let images = flag("images", "its model reads images")?;
        let private = flag("private", "its model is private: confidential inference that keeps nothing")?;
        let upstream: Upstream = serde_json::from_slice(row.to_string().as_bytes()).map_err(|e| format!("{at}: {e}"))?;
        out.push((ModelSlot { name, upstream, images, private, fallback: None, opts }, fallback));
    }
    Ok(out)
}

/// The routes of a deployment's `models` (`models.routes`), their slots by
/// place (`slot_at`). Refused, saying why: none or more than `ROUTES_MAX`,
/// a route that is no object, a name that is no route's (a slot's rule, and
/// neither `vision` nor `fallback`, which a call names its route's by) or
/// one named twice, a label that is not 1 to `LABEL_MAX_CHARS` characters of
/// plain text, a slot, vision or deep that is no slot, a deep that is its
/// own slot, a key it does not take.
fn routes_of(routes: Vec<Value>, slot_at: impl Fn(&str) -> Option<usize>) -> Result<Vec<Route>, String> {
    if routes.is_empty() || routes.len() > ROUTES_MAX {
        return Err(format!("models.routes: 1 to {ROUTES_MAX} routes, the ones people pick, not {}", routes.len()));
    }
    let mut out: Vec<Route> = Vec::with_capacity(routes.len());
    for row in routes {
        let Value::Object(mut o) = row else { return Err("models.routes: each is {name, label?, slot, vision?, deep?}".into()) };
        let name = match o.remove("name") {
            Some(Value::String(n)) if valid_slot_name(&n) && n != VISION && n != FALLBACK => n,
            other => return Err(format!("models.routes: a route's name is 1 to {SLOT_NAME_MAX_BYTES} of a-z, 0-9 and _, a letter first, and neither vision nor fallback, not {}", other.unwrap_or(Value::Null))),
        };
        let at = format!("models.routes.{name}");
        if out.iter().any(|r| r.name == name) {
            return Err(format!("{at}: a route is named once"));
        }
        let label = match o.remove("label") {
            None => None,
            Some(label) => {
                let text = label.as_str().map(str::trim).filter(|t| (1..=LABEL_MAX_CHARS).contains(&t.chars().count()) && !t.chars().any(char::is_control));
                Some(text.ok_or_else(|| format!("{at}: label is 1 to {LABEL_MAX_CHARS} characters of plain text, not {label}"))?.to_string())
            }
        };
        let mut slot_named = |key: &str| match o.remove(key) {
            None => Ok(None),
            Some(Value::String(s)) => slot_at(&s).map(Some).ok_or_else(|| format!("{at}: its {key} is {s:?}, which is no slot of the deployment's")),
            Some(other) => Err(format!("{at}: {key} names a slot, not {other}")),
        };
        let slot = slot_named("slot")?.ok_or_else(|| format!("{at}: slot names the slot its turns run on"))?;
        let vision = slot_named(VISION)?.unwrap_or(slot);
        let deep = slot_named("deep")?;
        if deep == Some(slot) {
            return Err(format!("{at}: its deep is its own slot: name another, or none"));
        }
        if let Some(key) = o.keys().next() {
            return Err(format!("{at}: {key} is none of name, label, slot, vision, deep"));
        }
        out.push(Route { name, label, slot, vision, deep });
    }
    Ok(out)
}

/// Whether `upstream` keeps what it is sent where a private slot may not:
/// Workers AI's, or OpenRouter's.
fn open(upstream: &Upstream) -> bool {
    matches!(upstream, Upstream::WorkersAi { .. }) || matches!(upstream, Upstream::Openai { base_url, .. } if on_openrouter(base_url))
}

/// Whether each private route is private throughout: every slot its
/// agents' calls reach (`Models::reached`) is private; and no slot on
/// Workers AI or OpenRouter is marked private. Refused, naming the slot to
/// change and why the route's agents reach it.
fn private_closed(models: &Models) -> Result<(), String> {
    for s in &models.slots {
        if s.private && open(&s.upstream) {
            return Err(format!("models.slots.{}: Workers AI and OpenRouter are not private: only a confidential API (Finite Private's, Tinfoil's) is marked private", s.name));
        }
    }
    if models.embeddings.private && open(&models.embeddings.upstream) {
        return Err("models.embeddings: Workers AI and OpenRouter are not private: only a confidential API (Finite Private's, Tinfoil's) is marked private".into());
    }
    for (at, route) in models.routes.iter().enumerate() {
        if !models.slots[route.slot].private {
            continue;
        }
        if let Some((slot, why)) = models.reached(at).into_iter().find(|(slot, _)| !models.slot_private(slot)) {
            return Err(format!("models.slots.{slot}: the route {} is private, and its agents' {why} reach {slot}, so mark a private model there (\"private\": true)", route.name));
        }
    }
    Ok(())
}

/// The names of the two routes the computer's view carried before it
/// carried the list (`ModelIds::cheap`, `medium` and `routes`): an image
/// older than the list decodes them and nothing else, so each is the route
/// of that name, else the default (`Models::ids_by_name`). Debt
/// (docs/technical-debt-ledger.md): gone once every deployment's pinned
/// image reads the list.
pub const VIEW_ROUTES: [&str; 2] = ["cheap", "medium"];

/// A deployment's `models` as configs named them before 2026-11-17 (four
/// slots: `cheap` and `medium` its tiers, each labelled and naming its
/// route's `fallback`, `vision` and `deep`; `fallback` and `vision` the
/// deployment's; and up to four beside them), as a list of routes over its
/// slots, meaning the same: each of the four it does not name, the default
/// config's model; each tier a route of its name on its slot, which falls
/// back to the slot it names (`fallback` unless it names another), its
/// images to `vision` unless it names another; `cheap` the default; and
/// `vision` falling back to `fallback` when it names none and that slot
/// says it reads images. Models already named as a list are passed as
/// they are. xtask reads a deployment's config through this (deploy.rs
/// `models_cell`): debt (docs/technical-debt-ledger.md), gone once
/// fragment-deploy's configs and self-hosted boxes' model files name
/// their routes.
pub fn legacy(models: &Value) -> Result<Value, String> {
    let Some(old) = models.as_object() else { return Err("models is an object: its default, routes and slots".into()) };
    if old.contains_key("routes") {
        return Ok(models.clone());
    }
    let mut old = old.clone();
    let mut out = Map::new();
    if let Some(e) = old.remove(crate::embed::EMBEDDINGS) {
        out.insert(crate::embed::EMBEDDINGS.into(), e);
    }
    let mut slots = Map::new();
    for (name, model) in [("cheap", CHEAP_MODEL), ("medium", MEDIUM_MODEL), (FALLBACK, FALLBACK_MODEL_DEFAULT), (VISION, VISION_MODEL_DEFAULT)] {
        slots.insert(name.into(), old.remove(name).unwrap_or_else(|| json!({ "provider": "workers_ai", "model": model })));
    }
    let mut routes = vec![];
    for tier in VIEW_ROUTES {
        let mut route = json!({ "name": tier, "slot": tier, "vision": VISION });
        if let Some(slot) = slots[tier].as_object_mut() {
            for key in ["label", VISION, "deep"] {
                if let Some(v) = slot.remove(key) {
                    route[key] = v;
                }
            }
            slot.entry(FALLBACK).or_insert_with(|| json!(FALLBACK));
        }
        routes.push(route);
    }
    let fallback_sees = slots[FALLBACK]["images"] == json!(true);
    if let Some(vision) = slots[VISION].as_object_mut().filter(|v| fallback_sees && !v.contains_key(FALLBACK)) {
        vision.insert(FALLBACK.into(), json!(FALLBACK));
    }
    slots.extend(old);
    out.insert("default".into(), json!(VIEW_ROUTES[0]));
    out.insert("routes".into(), Value::Array(routes));
    out.insert("slots".into(), Value::Object(slots));
    Ok(Value::Object(out))
}

impl Models {
    /// The default config's models (`default_config`).
    pub fn defaults() -> Models {
        Models::parse(None, &PriceBook::defaults(), false).expect("the default config is a deployment's")
    }

    /// A deployment's models: `named` (`FRAGMENT_MODELS`: `{default,
    /// routes, slots, embeddings?}`), or `default_config` when it names
    /// none, checked. Refused, saying why: a key it does not take, a
    /// default that is none of its routes, the slots' and routes' own
    /// refusals (`slots_of`, `routes_of`), a fallback that is no other
    /// slot, or one that reads no images behind a slot a route sends its
    /// images to, a `reasoning` on a slot that runs no route's Fast or
    /// Deep; a provider that is neither, a model neither the price book nor
    /// its slot's ceiling prices (a ledger would charge none of its calls;
    /// `Upstream::ceiling`, which refuses a slot on OpenRouter without
    /// one), an OpenAI-compatible API's `base_url` that is not https on a
    /// host name (`loopback`: or a local fleet's fake at
    /// `http://127.0.0.1:<port>`), or names a query or a trailing `/`, and
    /// its `body` when it is no object, is past `UPSTREAM_BODY_MAX_BYTES`,
    /// or names a key the route writes (`ROUTE_KEYS`); a model id two slots
    /// name at two places (`same_place`) or at two ceilings: a call names a
    /// model by its id (`named`), so an id is one model, and one row of the
    /// book (`priced`), wherever slots name it; and a private route whose
    /// agents could reach a slot that is not (`private_closed`).
    pub fn parse(named: Option<&str>, book: &PriceBook, loopback: bool) -> Result<Models, String> {
        let mut v = match named {
            Some(text) => serde_json::from_slice(text.as_bytes()).map_err(|e| format!("models: {e}"))?,
            None => default_config(),
        };
        let embeddings = crate::embed::slot_of(&mut v)?;
        let Value::Object(mut top) = v else { return Err("models is an object: its default, routes and slots".into()) };
        let default = match top.remove("default") {
            Some(Value::String(d)) => d,
            other => return Err(format!("models.default names the route agents run on unless they name another, not {}", other.unwrap_or(Value::Null))),
        };
        let Some(Value::Array(routes)) = top.remove("routes") else { return Err("models.routes is the list of routes people pick: [{name, label?, slot, vision?, deep?}]".into()) };
        let Some(Value::Object(slots)) = top.remove("slots") else { return Err("models.slots is an object of slots by name, each where a model runs".into()) };
        if let Some(key) = top.keys().next() {
            return Err(format!("models: {key} is none of default, routes, slots, embeddings"));
        }
        let read = slots_of(slots)?;
        let slot_at = |name: &str| read.iter().position(|(s, _)| s.name == name);
        let routes = routes_of(routes, slot_at)?;
        let mut fallbacks = Vec::with_capacity(read.len());
        for (s, to) in &read {
            fallbacks.push(match to {
                None => None,
                Some(to) => Some(slot_at(to).filter(|at| read[*at].0.name != s.name).ok_or_else(|| format!("models.slots.{}: its fallback is {to:?}, which is no other slot of the deployment's", s.name))?),
            });
        }
        let slots: Vec<ModelSlot> = read.into_iter().zip(fallbacks).map(|((s, _), fallback)| ModelSlot { fallback, ..s }).collect();
        let default = routes.iter().position(|r| r.name == default).ok_or_else(|| format!("models.default is {default:?}, which is none of its routes"))?;
        let models = Models { slots, routes, default, embeddings };
        for (at, s) in models.slots.iter().enumerate() {
            if s.opts.reasoning.is_some() && !models.routes.iter().any(|r| r.slot == at || r.deep == Some(at)) {
                return Err(format!("models.slots.{}: reasoning is a mode's, a route's own slot's or its deep's", s.name));
            }
            if let Some(to) = s.fallback.filter(|to| models.routes.iter().any(|r| r.vision == at) && !models.slots[*to].images) {
                let to = &models.slots[to].name;
                return Err(format!("models.slots.{}: a route's images go to {}, so its fallback reads them too: {to} says no \"images\": true", s.name, s.name));
            }
        }
        for (i, s) in models.slots.iter().enumerate() {
            let (id, upstream) = (s.upstream.model(), &s.upstream);
            check(upstream, book, loopback).map_err(|e| format!("models.slots.{}: {e}", s.name))?;
            if let Some(first) = models.slots[..i].iter().find(|f| f.upstream.model() == id && !same_place(&f.upstream, upstream)) {
                return Err(format!("models.slots.{}: {id:?} is models.slots.{}'s model too, where it runs elsewhere: a call names a model by its id, so name it where models.slots.{} does, or name another", s.name, first.name, first.name));
            }
            if let Some(first) = models.slots[..i].iter().find(|f| f.upstream.model() == id && f.upstream.own_price() != upstream.own_price()) {
                return Err(format!("models.slots.{}: {id:?} is models.slots.{}'s model too, at another price: a model is one row of the price book, so name the price (or max_price) models.slots.{} does", s.name, first.name, first.name));
            }
        }
        let embeddings = &models.embeddings.upstream;
        check(embeddings, book, loopback).map_err(|e| format!("models.embeddings: {e}"))?;
        if let Some(s) = models.slots.iter().find(|s| s.upstream.model() == embeddings.model()) {
            return Err(format!("models.embeddings: {:?} is models.slots.{}'s model, a chat model: an embeddings slot runs a model of its own", embeddings.model(), s.name));
        }
        private_closed(&models)?;
        Ok(models)
    }

    /// Why this deployment embeds nothing, if it does not: a route is
    /// private and its embeddings slot is not (Paul's rule: Finite Private
    /// never falls back to a model that is not private, so content is not
    /// sent to one to be embedded either). Its deploy is not refused: its
    /// search answers by words alone.
    pub fn embeddings_refused(&self) -> Option<String> {
        let private_route = self.routes.iter().any(|r| self.slots[r.slot].private);
        (private_route && !self.embeddings.private).then(|| "a route of this deployment's is private and its embeddings slot is not: nothing is embedded rather than sent to a model that is not private (name a private embeddings slot)".to_string())
    }

    /// `book` with each slot's own price (`Upstream::own_price`: its list
    /// price, or its ceiling) as its model's row, over any row of that id:
    /// the book the deployment charges with, so a change to its config
    /// re-prices its calls with the deploy.
    pub fn priced(&self, mut book: PriceBook) -> PriceBook {
        for upstream in self.slots.iter().map(|s| &s.upstream).chain([&self.embeddings.upstream]) {
            if let Some(price) = upstream.own_price().expect("a slot's price is checked as it is parsed") {
                book.models.retain(|m| m.model != upstream.model());
                book.models.push(ModelPrice { model: upstream.model().to_string(), price });
            }
        }
        book
    }

    /// The book's rows for what a slot's API says a call cost (FIN-616):
    /// OpenRouter's, when a slot is on it (`Upstream::used`).
    pub fn cost_keys(&self) -> Vec<crate::price::KeyPrice> {
        let on = self.slots.iter().map(|s| &s.upstream).chain([&self.embeddings.upstream]).any(|u| matches!(u, Upstream::Openai { base_url, .. } if on_openrouter(base_url)));
        let (micros, per) = OPENROUTER_COST_PRICE;
        on.then(|| crate::price::KeyPrice { key: OPENROUTER_COST.to_string(), micros, per }).into_iter().collect()
    }

    /// The place of the slot named `name`.
    fn slot_at(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.name == name)
    }

    /// The place of the route named `name` (an agent's `tier`, a job's
    /// step's `model`, a call's `TIER_HEADER`).
    pub fn route_named(&self, name: &str) -> Option<usize> {
        self.routes.iter().position(|r| r.name == name)
    }

    /// The route a job's step names (`ai.text`'s `model`): the default when
    /// it names none, refused naming the routes when it names another.
    pub fn step_route(&self, name: Option<&str>) -> Result<usize, UnknownRoute> {
        match name {
            None => Ok(self.default),
            Some(n) => self.route_named(n).ok_or_else(|| self.unknown_route()),
        }
    }

    /// The refusal of a name that is none of the routes'.
    pub fn unknown_route(&self) -> UnknownRoute {
        UnknownRoute { allowed: self.routes.iter().map(|r| r.name.clone()).collect() }
    }

    /// The slot at `at`, called as `named`.
    fn model_at(&self, at: usize, named: Named) -> Model<'_> {
        let s = &self.slots[at];
        Model { named, upstream: &s.upstream, slot: &s.name }
    }

    /// How a call naming the slot at `at` by its model's id names it: the
    /// first route whose turns run on it, else the slot itself.
    fn named_slot(&self, at: usize) -> Named {
        match self.routes.iter().position(|r| r.slot == at) {
            Some(r) => Named::Route(u8::try_from(r).expect("at most ROUTES_MAX routes")),
            None => Named::Slot(u8::try_from(at).expect("at most SLOTS_MAX slots")),
        }
    }

    /// What `named` is called in a call's log line: its route's name, the
    /// alias it named, or its slot's name.
    pub fn name_of(&self, named: Named) -> &str {
        match named {
            Named::Route(r) => &self.routes[usize::from(r)].name,
            Named::Vision => VISION,
            Named::Fallback => FALLBACK,
            Named::Slot(s) => &self.slots[usize::from(s)].name,
        }
    }

    /// The route's models' ids, each once, its routes' own slots' first:
    /// what a call may name besides the route's names (`named`), and what
    /// `GET /v1/models` lists (`listed`).
    pub fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::with_capacity(SLOTS_MAX);
        for s in self.routes.iter().map(|r| &self.slots[r.slot]).chain(&self.slots) {
            if !ids.contains(&s.upstream.model()) {
                ids.push(s.upstream.model());
            }
        }
        ids
    }

    /// `GET /v1/models`, OpenAI's list (its `created` unknown, 0): each of
    /// `ids`, and nothing of where it runs or with what key. The route's
    /// names are taken (`named`) and not listed.
    pub fn listed(&self) -> Value {
        let data: Vec<Value> = self.ids().into_iter().map(|id| json!({ "id": id, "object": "model", "created": 0, "owned_by": "fragment" })).collect();
        json!({ "object": "list", "data": data })
    }

    /// The slot whose id the computer's view names as the deployment's
    /// fallback (`ModelIds::fallback`, which an image older than routes
    /// names for every stall): the default route's fallback, else its own
    /// slot (which a runtime takes as none: #527).
    fn view_fallback(&self) -> usize {
        let d = &self.routes[self.default];
        self.slots[d.slot].fallback.unwrap_or(d.slot)
    }

    /// Each route's model ids as a computer's guest and its owner's
    /// Settings read them in its view (`GET /api/computer`'s and `GET
    /// /api/computers`' `models`): the two routes it carried before the
    /// list (`VIEW_ROUTES`), each its label, whether it is private, its
    /// fallback's and vision model's ids, the effort its turns think at,
    /// and its Deep (its id, effort and fallback's id); the default route's
    /// fallback and vision ids as the deployment's; and the ids of the
    /// models that read images.
    pub fn ids_by_name(&self) -> fragment_proto::computer::ModelIds {
        use fragment_proto::computer::{ModelIds, RouteMode, TierRoute, TierRoutes};
        let id = |at: usize| self.slots[at].upstream.model().to_string();
        let reasoning = |at: usize| self.slots[at].opts.reasoning.map(str::to_string);
        let route = |at: usize| {
            let r = &self.routes[at];
            let s = &self.slots[r.slot];
            let deep = r.deep.map(|d| RouteMode { id: id(d), reasoning: reasoning(d), fallback: self.slots[d].fallback.map(id) });
            TierRoute { label: r.label.clone(), private: s.private, fallback: id(s.fallback.unwrap_or(r.slot)), vision: id(r.vision), reasoning: reasoning(r.slot), deep }
        };
        let [cheap, medium] = VIEW_ROUTES.map(|name| self.route_named(name).unwrap_or(self.default));
        let mut images: Vec<String> = Vec::new();
        for s in self.slots.iter().filter(|s| s.images) {
            if !images.iter().any(|id| id == s.upstream.model()) {
                images.push(s.upstream.model().to_string());
            }
        }
        ModelIds {
            cheap: id(self.routes[cheap].slot),
            medium: id(self.routes[medium].slot),
            fallback: id(self.view_fallback()),
            vision: id(self.routes[self.default].vision),
            routes: Some(TierRoutes { cheap: route(cheap), medium: route(medium) }),
            images: Some(images),
        }
    }

    /// What a call naming `model` runs, as the route reads its body's
    /// `model` and its caller's `route` (`TIER_HEADER`): none named, the
    /// default route; `vision` or `fallback`, that alias (`route_for` reads
    /// the caller's route's); a route's name, that route; a model's id, the
    /// caller's route's slot when it runs it, else its Deep when that runs
    /// it, else the first route whose slot runs it, else the first slot
    /// that does (`slots`' order). Anything else names none (`UnknownModel`).
    pub fn named(&self, model: Option<&str>, route: Option<usize>) -> Result<Named, UnknownModel> {
        let at = |r: usize| Named::Route(u8::try_from(r).expect("at most ROUTES_MAX routes"));
        let Some(name) = model else { return Ok(at(self.default)) };
        match name {
            VISION => return Ok(Named::Vision),
            FALLBACK => return Ok(Named::Fallback),
            _ => {}
        }
        if let Some(r) = self.route_named(name) {
            return Ok(at(r));
        }
        let runs = |s: usize| self.slots[s].upstream.model() == name;
        let theirs = route.filter(|r| runs(self.routes[*r].slot)).map(at);
        let their_deep = || route.and_then(|r| self.routes[r].deep).filter(|d| runs(*d)).map(|d| self.named_slot(d));
        let first_route = || self.routes.iter().position(|r| runs(r.slot)).map(at);
        let first_slot = || (0..self.slots.len()).find(|s| runs(*s)).map(|s| self.named_slot(s));
        theirs.or_else(their_deep).or_else(first_route).or_else(first_slot).ok_or_else(|| self.unknown())
    }

    /// The refusal of a `model` that names none of the route's models.
    pub fn unknown(&self) -> UnknownModel {
        UnknownModel { allowed: self.ids().into_iter().map(str::to_string).collect() }
    }

    /// The model a call naming `named` runs, whoever calls (`route_for`
    /// with no route: the default's aliases).
    pub fn route(&self, named: Named) -> Model<'_> {
        self.route_for(named, None)
    }

    /// The model a call naming `named` runs for a caller on `route` (its
    /// `TIER_HEADER`; a job's step's route): a route's own slot; `vision`,
    /// its route's vision; `fallback`, its route's slot's fallback, or that
    /// slot when it has none; a slot, that slot. A caller that says no
    /// route is the default's.
    pub fn route_for(&self, named: Named, route: Option<usize>) -> Model<'_> {
        let r = &self.routes[route.unwrap_or(self.default)];
        match named {
            Named::Route(at) => self.model_at(self.routes[usize::from(at)].slot, named),
            Named::Vision => self.model_at(r.vision, named),
            Named::Fallback => self.model_at(self.slots[r.slot].fallback.unwrap_or(r.slot), named),
            Named::Slot(at) => self.model_at(usize::from(at), named),
        }
    }

    /// The model a call that the slot named `slot` fails before answering
    /// is made on once more (`Tries`), whoever called and however it named
    /// the slot (its name, an alias, its model's id: #589's review): the
    /// slot it names as its fallback. None for a slot that names none.
    pub fn fallback_of(&self, slot: &str) -> Option<Model<'_>> {
        let to = self.slots[self.slot_at(slot)?].fallback?;
        Some(self.model_at(to, Named::Fallback))
    }

    /// Native image calls may fall back only to another model that reads
    /// images. Text calls keep their existing chain, including older images.
    pub fn fallback_for_input(&self, slot: &str, input: &Value) -> Option<Model<'_>> {
        let fallback = self.fallback_of(slot)?;
        let images = input["messages"].as_array().is_some_and(|messages| messages.iter().any(|m| m["content"].as_array().is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))));
        if images && !self.slot_at(fallback.slot).is_some_and(|at| self.slots[at].images) {
            return None;
        }
        Some(fallback)
    }

    /// What the slot named `slot` says of the calls run on it.
    pub fn opts(&self, slot: &str) -> SlotOpts {
        self.slot_at(slot).map(|at| self.slots[at].opts).unwrap_or_default()
    }

    /// The route at `route`'s Deep: the slot its agents' delegates run on,
    /// when it names one (`Route::deep`).
    pub fn deep_of(&self, route: usize) -> Option<Model<'_>> {
        let d = self.routes[route].deep?;
        Some(self.model_at(d, self.named_slot(d)))
    }

    /// Whether the slot named `slot` says its model is private.
    pub fn slot_private(&self, slot: &str) -> bool {
        self.slot_at(slot).is_some_and(|at| self.slots[at].private)
    }

    /// The slots an agent on the route at `route` can reach, and why, as
    /// Hermes v0.21.7 and the route send its calls: its slot (its turns;
    /// and Hermes' recovery of an auxiliary call to the main model, a
    /// vision call's included, after that call and its fallback fail:
    /// `agent/auxiliary_client.py`, `_try_main_agent_model_fallback`), its
    /// slot's fallback (Hermes' own switch on a stall, an auxiliary call's
    /// `fallback_providers`), its vision (Hermes' auxiliary vision) and its
    /// Deep (its delegates). A private route's are all private
    /// (`private_closed`).
    ///
    /// And what a stall's call reaches: it names its fallback's id with its
    /// route unsaid (Hermes' `fallback_providers` carries no header of
    /// ours), as an image before its route was read (#590) names the view's
    /// fallback's (`view_fallback`); such a call runs the slot `named`
    /// finds for the id, which is reached too. A call to any slot reached is
    /// made once more on that slot's own fallback (`fallback_of`), which is
    /// reached too, and so on.
    pub fn reached(&self, route: usize) -> Vec<(&str, &'static str)> {
        let r = &self.routes[route];
        let name = |at: usize| self.slots[at].name.as_str();
        let fallback = self.slots[r.slot].fallback;
        let mut reached = vec![(name(r.slot), "turns and recovered calls")];
        fn add<'a>(reached: &mut Vec<(&'a str, &'static str)>, slot: Option<&'a str>, why: &'static str) {
            if let Some(slot) = slot.filter(|s| !reached.iter().any(|(r, _)| r == s)) {
                reached.push((slot, why));
            }
        }
        add(&mut reached, fallback.map(name), "failures and stalls");
        add(&mut reached, Some(name(r.vision)), "images");
        add(&mut reached, r.deep.map(name), "delegates");
        let by_id = |at: usize| self.named(Some(self.slots[at].upstream.model()), None).ok().map(|n| self.route(n).slot);
        add(&mut reached, by_id(self.view_fallback()), "stalls on an older image (its route unsaid)");
        let deep_stall = r.deep.and_then(|d| self.slots[d].fallback);
        for at in [fallback, deep_stall].into_iter().flatten() {
            add(&mut reached, by_id(at), "stalls (by its model's id)");
        }
        let mut at = 0;
        while at < reached.len() {
            let next = self.fallback_of(reached[at].0).map(|m| m.slot);
            add(&mut reached, next, "failures");
            at += 1;
        }
        reached
    }

    /// `route`'s model, and the most one call of it writes.
    pub fn capped(&self, named: Named) -> Capped<'_> {
        self.capped_for(named, None)
    }

    /// `route_for`'s model, and the most one call of it writes.
    pub fn capped_for(&self, named: Named, route: Option<usize>) -> Capped<'_> {
        let model = self.route_for(named, route);
        Capped { model, max_tokens: self.opts(model.slot).max_tokens.unwrap_or(MAX_TOKENS) }
    }
}

/// Whether two slots make one call alike: one model at one place, sent the
/// same body, whatever price each names (a price is the book's, and two
/// slots of one id name one: `parse`). A fallback that runs so is none
/// (`Tries`; #526's review, P3).
fn same_call(a: &Upstream, b: &Upstream) -> bool {
    match (a, b) {
        (Upstream::WorkersAi { model: x }, Upstream::WorkersAi { model: y }) => x == y,
        (Upstream::Openai { base_url: u, model: x, body: bx, .. }, Upstream::Openai { base_url: v, model: y, body: by, .. }) => u == v && x == y && bx == by,
        _ => false,
    }
}

/// Whether two slots naming one id run it at one place: both on Workers AI,
/// or both on one OpenAI-compatible API (their keys and bodies may differ).
fn same_place(a: &Upstream, b: &Upstream) -> bool {
    match (a, b) {
        (Upstream::WorkersAi { .. }, Upstream::WorkersAi { .. }) => true,
        (Upstream::Openai { base_url: x, .. }, Upstream::Openai { base_url: y, .. }) => x == y,
        _ => false,
    }
}

/// A call's `model` that names none of the route's models (`Models::named`):
/// a 400 that lists what it may name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownModel {
    /// `Models::ids`.
    pub allowed: Vec<String>,
}

impl UnknownModel {
    pub fn message(&self) -> String {
        format!("model is one of this deployment's models, as GET /v1/models lists them: {}", self.allowed.join(", "))
    }
}

/// A name that is none of the deployment's routes (`Models::route_named`):
/// a job's step's `model`, a call's `TIER_HEADER`. A 400 that lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRoute {
    /// The routes' names, in their order.
    pub allowed: Vec<String>,
}

impl UnknownRoute {
    /// What the name should have been, after what names it ("model names").
    pub fn message(&self) -> String {
        format!("one of this deployment's routes: {} (a model id is never one)", self.allowed.join(", "))
    }
}

/// One slot's model, checked (`Models::parse`).
fn check(upstream: &Upstream, book: &PriceBook, loopback: bool) -> Result<(), String> {
    if let Upstream::Openai { base_url, model, body, .. } = upstream {
        crate::catalog::row_url(base_url, loopback).map_err(|e| format!("base_url: {e}"))?;
        if base_url.ends_with('/') || base_url.contains('?') {
            return Err("base_url is named without a query or a trailing /: the route adds /chat/completions".into());
        }
        if !crate::catalog::valid_model_id(model) {
            return Err(format!("{model:?} is no model id (1-{} of letters, digits and . _ - : / @)", crate::catalog::MODEL_ID_MAX_BYTES));
        }
        if let Some(body) = body {
            let bytes = serde_json::to_vec(body).expect("a JSON object serializes").len();
            if bytes > UPSTREAM_BODY_MAX_BYTES {
                return Err(format!("body is {bytes} bytes of JSON, past {UPSTREAM_BODY_MAX_BYTES}: an API's own fields, not a prompt"));
            }
            let named: Vec<&str> = body.keys().map(String::as_str).filter(|k| ROUTE_KEYS.contains(k)).collect();
            if !named.is_empty() {
                return Err(format!("body names {}, which the route writes itself ({}): name only the API's own fields", named.join(", "), ROUTE_KEYS.join(", ")));
            }
        }
    }
    let model = upstream.model();
    if upstream.own_price()?.is_none() && !book.models.iter().any(|m| m.model == model) {
        let priced: Vec<&str> = book.models.iter().map(|m| m.model.as_str()).collect();
        return Err(format!(
            "the model {model:?} is not in the price book, which prices {}: name its list price at its API (price: {{\"input\": <$>, \"cached_input\"?: <$>, \"output\": <$>}} a million tokens), or, on OpenRouter, the ceiling it is priced at (body.provider.max_price), or name one of those",
            priced.join(", ")
        ));
    }
    Ok(())
}

/// A model the route calls: the name it is called under, where it runs,
/// and the slot it runs as (whose key an OpenAI-compatible API's call
/// carries: `secrets_store::model_key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model<'a> {
    pub named: Named,
    pub upstream: &'a Upstream,
    pub slot: &'a str,
}

impl<'a> Model<'a> {
    /// Its model's id, and its row in the price book.
    pub fn id(&self) -> &'a str {
        self.upstream.model()
    }
}

/// A call's model and the most one call of it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capped<'a> {
    pub model: Model<'a>,
    pub max_tokens: u32,
}

/// The slot a call to the route runs on: a route's (`Route`, its place in
/// `Models::routes`), named by its name or by its model's id; `vision` or
/// `fallback`, named by those aliases, which follow the caller's route
/// (`Models::route_for`); or a slot named by its model's id (`Slot`, its
/// place in `Models::slots`), which runs that slot exactly. Each call
/// falls back as the slot it runs does (`Models::fallback_of`). Its log
/// line names it by `Models::name_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    Route(u8),
    Vision,
    Fallback,
    Slot(u8),
}

/// Why a model call is refused before anything is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The body is not a JSON object.
    NotAnObject,
    /// No `messages`, or none in it: the route makes chat completions only.
    NoMessages,
    /// A `max_tokens` (or `max_completion_tokens`) that is not a positive integer.
    MaxTokens,
    /// A `stream` that is not true or false.
    Stream,
    /// A step's tools past `TOOLS_MAX` or `TOOLS_MAX_BYTES`, one not shaped
    /// as OpenAI's function (a key it does not take), a name that is not a
    /// function's, or one named twice.
    Tools,
    /// A tool whose `parameters` is no object schema (`{"type": "object", …}`).
    ToolSchema,
    /// A `tool_choice` with no tools, or naming a function not among them.
    ToolChoice,
    /// A draft to a channel that is no app's, or under a turn that is not
    /// `^[A-Za-z0-9._:-]{1,128}$`.
    Draft,
    /// A step's arguments past `AI_TEXT_MAX_BYTES`.
    TextTooLarge,
}

impl Refusal {
    /// The refusal for people (and agents, which show it).
    pub fn message(self) -> String {
        match self {
            Refusal::NotAnObject => "a chat completion is a JSON object".into(),
            Refusal::NoMessages => "a chat completion has messages: at least one".into(),
            Refusal::MaxTokens => format!("max_tokens is a positive integer (at most {MAX_TOKENS} is used)"),
            Refusal::Stream => "stream is true or false".into(),
            Refusal::Tools => format!(
                "ai.text takes at most {TOOLS_MAX} tools in {TOOLS_MAX_BYTES} bytes, each {{\"type\": \"function\", \"function\": {{name, description?, parameters?, strict?}}}} and no other key, named 1 to {TOOL_NAME_MAX_BYTES} letters, digits, '_' or '-', once"
            ),
            Refusal::ToolSchema => "a tool's parameters is a JSON Schema of an object: {\"type\": \"object\", \"properties\": {…}}".into(),
            Refusal::ToolChoice => "tool_choice is none, auto, required, or a function among the tools".into(),
            Refusal::Draft => "a draft names an app's channel and a turn matching ^[A-Za-z0-9._:-]{1,128}$".into(),
            Refusal::TextTooLarge => format!("job.ai.text's arguments are at most {AI_TEXT_MAX_BYTES} bytes of JSON"),
        }
    }
}

fn valid_tool_name(name: &str) -> bool {
    (1..=TOOL_NAME_MAX_BYTES).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The keys a tool's `function` takes (OpenAI's); any other is refused, as
/// a tool the model would read otherwise than the app meant.
const FUNCTION_KEYS: [&str; 4] = ["name", "description", "parameters", "strict"];

/// A step's tools, OpenAI's functions (`{type: "function", function:
/// {name, description?, parameters?, strict?}}`), within their bounds: their
/// names, or why not.
fn tool_names(tools: &[Value]) -> Result<Vec<&str>, Refusal> {
    let bytes = serde_json::to_vec(tools).expect("a JSON value serializes").len();
    if tools.len() > TOOLS_MAX || bytes > TOOLS_MAX_BYTES {
        return Err(Refusal::Tools);
    }
    let mut names = Vec::with_capacity(tools.len());
    for tool in tools {
        let o = tool.as_object().ok_or(Refusal::Tools)?;
        let f = o.get("function").and_then(Value::as_object).ok_or(Refusal::Tools)?;
        let name = f.get("name").and_then(Value::as_str).filter(|n| valid_tool_name(n)).ok_or(Refusal::Tools)?;
        let shaped = o.len() == 2
            && o.get("type") == Some(&json!("function"))
            && f.keys().all(|k| FUNCTION_KEYS.contains(&k.as_str()))
            && f.get("description").is_none_or(Value::is_string)
            && f.get("strict").is_none_or(Value::is_boolean);
        if !shaped || names.contains(&name) {
            return Err(Refusal::Tools);
        }
        let schema = f.get("parameters").is_none_or(|p| p.as_object().is_some_and(|p| p.get("type") == Some(&json!("object"))));
        if !schema {
            return Err(Refusal::ToolSchema);
        }
        names.push(name);
    }
    Ok(names)
}

/// A job's text step as the chat completion it asks for (`bound` bounds it
/// for its tier): its messages (or its prompt, as one user message), and
/// what it named of `max_tokens`, `reasoning_effort`, `tools` and
/// `tool_choice`. Refused, typed, before anything is sent: tools past
/// their bounds or not a function's, a choice of none of them, a draft out
/// of shape (the cell checks the app declares its channel), a request past
/// `AI_TEXT_MAX_BYTES`.
pub fn text_body(t: &AiText) -> Result<Value, Refusal> {
    let messages = match (&t.messages, &t.prompt) {
        (Some(m), _) => Value::Array(m.clone()),
        (None, Some(prompt)) => json!([{ "role": "user", "content": prompt }]),
        (None, None) => return Err(Refusal::NoMessages),
    };
    let mut body = json!({ "messages": messages });
    if let Some(n) = t.max_tokens {
        body["max_tokens"] = json!(n);
    }
    if let Some(effort) = &t.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    let tools = t.tools.as_deref().unwrap_or_default();
    let names = tool_names(tools)?;
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    if let Some(choice) = &t.tool_choice {
        let named = match choice {
            Value::String(mode) => ["none", "auto", "required"].contains(&mode.as_str()),
            Value::Object(o) => {
                let function = o.get("function").and_then(Value::as_object);
                let name = function.and_then(|f| f.get("name")).and_then(Value::as_str);
                o.len() == 2 && o.get("type") == Some(&json!("function")) && function.is_some_and(|f| f.len() == 1) && name.is_some_and(|n| names.contains(&n))
            }
            _ => false,
        };
        if tools.is_empty() || !named {
            return Err(Refusal::ToolChoice);
        }
        body["tool_choice"] = choice.clone();
    }
    if t.draft.is_some() && draft_of(t).is_none() {
        return Err(Refusal::Draft);
    }
    // within what platform.mjs counts (the step's arguments) before the
    // Workflow carries them
    if body.to_string().len() > AI_TEXT_MAX_BYTES {
        return Err(Refusal::TextTooLarge);
    }
    Ok(body)
}

/// A text step's draft, `{channel, turn}` and no other key: an app's
/// channel (never a built-in one) and a turn `^[A-Za-z0-9._:-]{1,128}$`;
/// `None` when it names none, or one out of shape (`text_body` refuses it).
pub fn draft_of(t: &AiText) -> Option<(&str, &str)> {
    let d = t.draft.as_ref()?.as_object()?;
    let (channel, turn) = (d.get("channel")?.as_str()?, d.get("turn")?.as_str()?);
    (d.len() == 2 && valid_channel_name(channel) && !BUILTIN_CHANNELS.contains(&channel) && valid_op_id(turn)).then_some((channel, turn))
}

/// One tool call of an answer: OpenAI's `{id, type: "function", function:
/// {name, arguments}}` (`Answer::message`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text, as the model wrote it (it may not parse).
    pub arguments: String,
}

/// What a text call answered, folded from its stream's deltas
/// (`Stream::answering`): its text, its tool calls, and why it stopped.
/// The model's reasoning is never kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl Answer {
    /// One streamed choice's delta, and its finish reason when it has one.
    fn take(&mut self, choice: &Value) {
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str() {
            self.content.push_str(text);
        }
        for (n, call) in delta["tool_calls"].as_array().into_iter().flatten().enumerate() {
            let index = call["index"].as_u64().map_or(n, |i| usize::try_from(i).unwrap_or(usize::MAX));
            if index < TOOL_CALLS_MAX {
                self.fold_call(index, call);
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
    }

    /// A delta of a tool call: its id and name as given, its arguments
    /// appended.
    fn fold_call(&mut self, index: usize, call: &Value) {
        assert!(index < TOOL_CALLS_MAX, "a tool call's index is bounded before it is folded");
        if self.tool_calls.len() <= index {
            self.tool_calls.resize(index + 1, ToolCall::default());
        }
        let to = &mut self.tool_calls[index];
        if let Some(id) = call["id"].as_str().filter(|s| !s.is_empty()) {
            to.id = id.to_string();
        }
        if let Some(name) = call["function"]["name"].as_str().filter(|s| !s.is_empty()) {
            to.name = name.to_string();
        }
        if let Some(args) = call["function"]["arguments"].as_str() {
            to.arguments.push_str(args);
        }
    }

    /// The assistant's message, as the next call's messages take it back:
    /// `{role, content, tool_calls?}`.
    pub fn message(&self) -> Value {
        let mut m = json!({ "role": "assistant", "content": self.content });
        if !self.tool_calls.is_empty() {
            let calls = self.tool_calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }));
            m["tool_calls"] = Value::Array(calls.collect());
        }
        m
    }
}

/// Whether an answer's status from `upstream`, before any of its body,
/// sends the call on to the fallback model: the vendor's rate limit (429:
/// Finite Private's limiter answers it when its key's window is spent), or
/// its own failure (5xx: tonight's Workers AI answered 502 "could not route
/// request to AI model", then 500, for five minutes; Finite Private's
/// limiter answers 502 when its model fails and 503 when finite.computer,
/// which admits each call, does not answer). On an OpenAI-compatible API,
/// whose key is the platform's, also the platform's own trouble there: its
/// key refused (401) or denied (403), its account out of credit (402), or
/// no endpoint left by its slot's routing (OpenRouter's 404): a call is
/// not the caller's to fail for the platform's account (decision 61, as
/// decided 2026-10-15). A refusal of the request itself (400 and any other
/// 4xx) would be the fallback's too, and a 200 has begun the answer, which
/// is final.
pub fn falls_back(upstream: &Upstream, status: u16) -> bool {
    let platforms = matches!(upstream, Upstream::Openai { .. }) && PLATFORM_KEY_REFUSALS.contains(&status);
    status == 429 || (500..=599).contains(&status) || platforms
}

/// The statuses an OpenAI-compatible API answers about the platform's own
/// key and account there (`falls_back`): refused, out of credit, denied,
/// and nothing left to route to.
pub const PLATFORM_KEY_REFUSALS: [u16; 4] = [401, 402, 403, 404];

/// Why a try that answered `status` from `upstream` says the platform's own
/// key there wants someone (its log line, loud: every call to it falls
/// back, or fails where it has no fallback, until it is fixed): refused
/// (401) or out of credit (402). Said of any try: the fallback's too, which
/// may be on a key of its own (Tinfoil's, behind Finite Private).
pub fn key_alarm(upstream: &Upstream, status: u16) -> Option<&'static str> {
    match (upstream, status) {
        (Upstream::Openai { .. }, 401) => Some("the platform's key for this API is refused: its calls fall back, or fail with no fallback, until it is set again (cargo xtask secret set)"),
        (Upstream::Openai { .. }, 402) => Some("the platform's account at this API is out of credit: its calls fall back, or fail with no fallback, until it is topped up"),
        _ => None,
    }
}

/// What one try of a call came to, before any of its answer's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tried {
    /// The model was not reached: the transport failed.
    Unreached,
    /// It answered this status.
    Answered(u16),
}

/// A call's tries: its model, then, when that one fails before answering
/// anything (`falls_back`, or not reached), the fallback model, once. At
/// most two tries, and the second only after the first has failed: no
/// race. A fallback that runs where the call's own model runs is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tries<'a> {
    first: Model<'a>,
    fallback: Option<Model<'a>>,
    fell: bool,
    /// A try's answer was the call's: what follows (a stream that breaks
    /// after it began) is no try.
    done: bool,
}

impl<'a> Tries<'a> {
    pub fn new(model: Model<'a>, fallback: Option<Model<'a>>) -> Tries<'a> {
        Tries { first: model, fallback: fallback.filter(|f| !same_call(f.upstream, model.upstream)), fell: false, done: false }
    }

    /// The model the next try calls, and, once the tries are done, the one
    /// whose answer is the call's: its usage is priced as that model's.
    pub fn model(&self) -> Model<'a> {
        match (self.fell, self.fallback) {
            (true, Some(f)) => f,
            _ => self.first,
        }
    }

    /// Whether the call fell back to the fallback model.
    pub fn fell(&self) -> bool {
        self.fell
    }

    /// After a try: the model to try next, or `None` when this try's
    /// answer is the call's (it answered, it refused the request, or it
    /// was the fallback's).
    pub fn after(&mut self, tried: Tried) -> Option<Model<'a>> {
        let failed = match tried {
            Tried::Unreached => true,
            Tried::Answered(status) => falls_back(self.model().upstream, status),
        };
        let next = match (self.done, failed, self.fell, self.fallback) {
            (false, true, false, Some(next)) => next,
            _ => {
                self.done = true;
                return None;
            }
        };
        self.fell = true;
        assert!(!same_call(next.upstream, self.first.upstream), "a fallback is another model");
        Some(next)
    }
}

/// What a guest's key says to name its agent: `agent:<label>--<suffix>`.
pub const AGENT_KEY_PREFIX: &str = "agent:";

/// Why a guest's model call names no agent to bill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unnamed {
    /// Neither the header nor the key names one.
    Missing,
    /// The header, or a key that says `agent:`, names no agent fragment.
    Malformed,
    /// The header and the key name two agents.
    Disagree,
}

impl Unnamed {
    pub fn message(self) -> &'static str {
        match self {
            Unnamed::Missing => "name the agent this call is for (x-fragment-agent, or the key agent:<label>--<suffix>): its owner pays for it",
            Unnamed::Malformed => "the agent a call names is an agent fragment's name, <label>--<suffix>",
            Unnamed::Disagree => "x-fragment-agent and the key agent:<…> name two agents",
        }
    }
}

/// Who a guest's model call is for (docs/computers.md, Models), one rule
/// for every model route: the agent its `x-fragment-agent` names, or, from
/// a client that sends no header of its own (OpenAI's SDKs take a base URL
/// and a key, nothing more), its key, `Authorization: Bearer
/// agent:<label>--<suffix>`. A key that does not say `agent:` is the
/// guest's own placeholder (Hermes' `fragment-model`), not a name. Named
/// both ways, the two agree. The computer signs only for an agent that runs
/// on it, so a key names no more than the header could.
pub fn agent_named(header: Option<&str>, authorization: Option<&str>) -> Result<String, Unnamed> {
    let from_key = authorization
        .and_then(|a| a.trim().split_once(' ').filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer")).map(|(_, k)| k.trim()))
        .and_then(|k| k.strip_prefix(AGENT_KEY_PREFIX));
    let valid = |n: &str| fragment_proto::valid_fragment_name(n).then(|| n.to_string()).ok_or(Unnamed::Malformed);
    match (header.map(str::trim), from_key) {
        (None, None) => Err(Unnamed::Missing),
        (Some(h), None) => valid(h),
        (None, Some(k)) => valid(k),
        (Some(h), Some(k)) => {
            let (h, k) = (valid(h)?, valid(k)?);
            if h == k {
                Ok(h)
            } else {
                Err(Unnamed::Disagree)
            }
        }
    }
}

/// The request the platform sends for one call, and what it may cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Bounded<'a> {
    pub model: Model<'a>,
    /// What it asks for, at most the tier's cap.
    pub max_tokens: u32,
    /// The input `env.AI.run` takes: the body less `model`, bounded (an
    /// OpenAI-compatible API's call adds its model's id).
    pub input: Value,
    pub stream: bool,
}

/// A positive token count from a body's field, if it names one.
fn tokens(v: Option<&Value>) -> Result<Option<u64>, Refusal> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n.as_u64().filter(|n| *n >= 1).map(Some).ok_or(Refusal::MaxTokens),
    }
}

/// `body`, an OpenAI-shaped chat completion, bounded for its model `t`
/// (`Models::capped`).
pub fn bound(t: Capped<'_>, body: Value, stream: bool) -> Result<Bounded<'_>, Refusal> {
    let Value::Object(mut input) = body else { return Err(Refusal::NotAnObject) };
    match input.get("messages") {
        Some(Value::Array(m)) if !m.is_empty() => {}
        _ => return Err(Refusal::NoMessages),
    }
    if input.get("stream").is_some_and(|s| !s.is_boolean() && !s.is_null()) {
        return Err(Refusal::Stream);
    }
    // the smallest of what it asked for (in either name) and the cap
    let asked = [tokens(input.get("max_tokens"))?, tokens(input.get("max_completion_tokens"))?];
    let max_tokens = asked.into_iter().flatten().fold(u64::from(t.max_tokens), u64::min);
    let effort = match input.get("reasoning_effort").and_then(Value::as_str) {
        Some(e) if EFFORTS.contains(&e) => e.to_string(),
        _ => EFFORTS[0].to_string(),
    };
    input.remove("model");
    input.remove("max_completion_tokens");
    input.insert("max_tokens".into(), json!(max_tokens));
    input.insert("reasoning_effort".into(), json!(effort));
    input.insert("stream".into(), json!(stream));
    match stream {
        true => input.insert("stream_options".into(), json!({ "include_usage": true })),
        false => input.remove("stream_options"),
    };
    let max_tokens = u32::try_from(max_tokens).expect("at most the tier's cap, a u32");
    assert!(max_tokens >= 1 && max_tokens <= t.max_tokens, "a call writes at least a token and at most its tier's cap");
    Ok(Bounded { model: t.model, max_tokens, input: Value::Object(input), stream })
}

/// A count from a usage object's field.
fn count(v: &Value, k: &str) -> Option<u64> {
    match v.get(k) {
        None | Some(Value::Null) => Some(0),
        Some(n) => n.as_u64(),
    }
}

/// A call's usage, OpenAI-shaped (Workers AI's and the gateway's), as the
/// ledger meters it: `prompt_tokens` counts cached input and cache writes,
/// so the input is what is left of it. A usage that does not read so (a
/// field missing or negative, more cached than prompted) is `None`: the
/// call is charged nothing, said aloud (decision 62).
pub fn usage_of(model: &str, usage: &Value) -> Option<Usage> {
    let prompt = usage.get("prompt_tokens")?.as_u64()?;
    let output = usage.get("completion_tokens")?.as_u64()?;
    let details = usage.get("prompt_tokens_details").cloned().unwrap_or(Value::Null);
    let cached = count(&details, "cached_tokens")?;
    let cache_write = match count(&details, "cache_write_tokens")? {
        0 => count(usage, "cache_creation_input_tokens")?,
        n => n,
    };
    let input = prompt.checked_sub(cached)?.checked_sub(cache_write)?;
    Some(Usage::Tokens { model: model.to_string(), input, cached_input: cached, cache_write, output })
}

/// A streamed answer read line by line (server-sent events): what the
/// client is sent, and the usage to meter. Workers AI puts a per-chunk
/// delta of usage on every chunk and ends with `{"response": "", "usage":
/// {…}}`, no choices, carrying the whole call's: the client gets OpenAI's
/// semantics (usage dropped from chunks with choices, and that last line as
/// a chunk with none), and only the last cumulative usage is metered. A
/// provider that sends OpenAI's own last chunk (`choices: []` with usage)
/// is metered from that, when no line of Workers AI's follows. An
/// OpenAI-compatible API's usage is the whole call's wherever it comes
/// (`on`): OpenRouter's comes on a last chunk that keeps one choice, a
/// content-free delta repeating the finish reason (its "accounting frame",
/// openrouter.ai/docs/api-reference/streaming, read 2026-10-15), which the
/// client gets as that chunk without it and then OpenAI's last chunk.
#[derive(Debug, Default)]
pub struct Stream {
    line: Vec<u8>,
    /// Each usage the stream carries is the whole call's: an
    /// OpenAI-compatible API's (`on`), never Workers AI's per-chunk deltas.
    whole: bool,
    /// The usage of the last line with no `choices` (Workers AI's whole call).
    cumulative: Option<Value>,
    /// The usage of the last chunk with an empty `choices` (OpenAI's last).
    trailing: Option<Value>,
    /// The answer's id, for the rewritten last line.
    id: Option<Value>,
    /// A line past `SSE_LINE_MAX`: the stream is out of its contract.
    overlong: bool,
    lines: u64,
    /// A `data:` line was read: the answer began.
    began: bool,
    /// `[DONE]` was read: the answer and its usage are whole, whether or
    /// not the connection stays open after it (the gateway's may, for
    /// minutes). Nothing after it is read.
    done: bool,
    /// The answer so far, when one is read (`answering`).
    answer: Option<Answer>,
    /// Characters of reasoning read (`reasoning_content`, or `reasoning`),
    /// none of them kept.
    thought: u64,
}

impl Stream {
    /// A stream whose answer is read as it comes (a job's text step), as
    /// well as its usage.
    pub fn answering() -> Stream {
        Stream { answer: Some(Answer::default()), ..Stream::default() }
    }

    /// The stream, read as `upstream` streams: an OpenAI-compatible API's
    /// usage is the whole call's on whichever chunk carries it.
    pub fn on(self, upstream: &Upstream) -> Stream {
        Stream { whole: matches!(upstream, Upstream::Openai { .. }), ..self }
    }

    /// The answer so far (`answering`'s; `None` for a stream only metered).
    pub fn answer(&self) -> Option<&Answer> {
        self.answer.as_ref()
    }

    /// Whether `[DONE]` was read: a reader stops there.
    pub fn done(&self) -> bool {
        self.done
    }

    /// Whether a `data:` line was read: when the model began to answer.
    pub fn began(&self) -> bool {
        self.began
    }

    /// Characters of reasoning read so far (`answering`'s).
    pub fn thought(&self) -> u64 {
        self.thought
    }

    /// Takes the next bytes; the client's lines go to `out` (none when
    /// only metering). Bytes after `[DONE]` are none of the answer's.
    pub fn push(&mut self, chunk: &[u8], mut out: Option<&mut Vec<u8>>) {
        // bounded by the chunk: each pass takes one line out of it
        for piece in chunk.split_inclusive(|b| *b == b'\n') {
            if self.done {
                return;
            }
            self.line.extend_from_slice(piece);
            if piece.last() == Some(&b'\n') {
                let line = std::mem::take(&mut self.line);
                self.take_line(&line, out.as_deref_mut());
            } else if self.line.len() > SSE_LINE_MAX {
                self.overlong = true;
                let line = std::mem::take(&mut self.line);
                if let Some(o) = out.as_deref_mut() {
                    o.extend_from_slice(&line);
                }
            }
        }
    }

    /// The end of the stream: a last line without its newline.
    pub fn finish(&mut self, out: Option<&mut Vec<u8>>) {
        if !self.line.is_empty() && !self.done {
            let line = std::mem::take(&mut self.line);
            self.take_line(&line, out);
        }
    }

    /// The usage to meter, when the stream told it whole.
    pub fn usage(&self) -> Option<&Value> {
        if self.overlong {
            return None;
        }
        self.cumulative.as_ref().or(self.trailing.as_ref())
    }

    /// Lines read so far.
    pub fn lines(&self) -> u64 {
        self.lines
    }

    fn take_line(&mut self, line: &[u8], out: Option<&mut Vec<u8>>) {
        self.lines += 1;
        let rewritten = self.read_line(line);
        if let Some(o) = out {
            o.extend_from_slice(rewritten.as_deref().unwrap_or(line));
        }
    }

    /// One line, read for its usage; the line the client gets instead, when
    /// it is not the line as it came.
    fn read_line(&mut self, line: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(line).ok()?;
        let data = text.strip_prefix("data:")?.trim();
        self.began = true;
        let ending = if text.ends_with("\r\n") { "\r\n" } else { "\n" };
        if data == "[DONE]" {
            // the last event, ended by its blank line: nothing after it is read
            self.done = true;
            return Some(format!("data: [DONE]{ending}{ending}").into_bytes());
        }
        let Ok(Value::Object(mut chunk)) = serde_json::from_slice::<Value>(data.as_bytes()) else { return None };
        if let Some(id) = chunk.get("id").filter(|id| !id.is_null()) {
            self.id = Some(id.clone());
        }
        if let (Some(answer), Some(choice)) = (self.answer.as_mut(), chunk.get("choices").and_then(|c| c.get(0))) {
            answer.take(choice);
            let delta = &choice["delta"];
            let reasoning = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str()).unwrap_or("");
            self.thought += reasoning.chars().count() as u64;
        }
        let newline = ending;
        let ending = if text.ends_with('\n') { ending } else { "" };
        match (chunk.get("choices"), chunk.get("usage").cloned()) {
            (_, None) => None,
            (Some(Value::Array(choices)), Some(usage)) if choices.is_empty() => {
                self.trailing = Some(usage);
                None
            }
            (Some(_), Some(usage)) if self.whole && !usage.is_null() => {
                // OpenRouter's accounting frame: its choice, then OpenAI's last chunk
                chunk.remove("usage");
                let last = json!({ "id": self.id, "object": "chat.completion.chunk", "choices": [], "usage": usage });
                self.trailing = Some(usage);
                Some(format!("data: {}{newline}{newline}data: {last}{ending}", Value::Object(chunk)).into_bytes())
            }
            (Some(_), Some(_)) => {
                chunk.remove("usage");
                Some(format!("data: {}{ending}", Value::Object(chunk)).into_bytes())
            }
            (None, Some(usage)) => {
                let last = json!({ "id": self.id, "object": "chat.completion.chunk", "choices": [], "usage": usage });
                self.cumulative = Some(usage);
                Some(format!("data: {last}{ending}").into_bytes())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static DEFAULTS: std::sync::LazyLock<Models> = std::sync::LazyLock::new(Models::defaults);

    /// The two routes of a config of the shape before routes were a list,
    /// by their place (`legacy`): `cheap`, then `medium`.
    const CHEAP: Named = Named::Route(0);
    const MEDIUM: Named = Named::Route(1);

    fn tier(n: Named) -> Capped<'static> {
        DEFAULTS.capped(n)
    }

    /// A config of the shape before routes were a list, read as xtask
    /// reads it (`legacy`).
    fn old_parse(text: &str, book: &PriceBook, loopback: bool) -> Result<Models, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("models: {e}"))?;
        Models::parse(Some(&legacy(&v)?.to_string()), book, loopback)
    }

    /// Where the slot named `name` runs.
    fn up<'a>(m: &'a Models, name: &str) -> &'a Upstream {
        &m.slots[m.slot_at(name).unwrap_or_else(|| panic!("no slot {name}"))].upstream
    }

    fn workers_ai(model: &str) -> Upstream {
        Upstream::WorkersAi { model: model.to_string() }
    }

    /// The slots that say their model reads images, and those that say it
    /// is private.
    fn images_of(m: &Models) -> Vec<&str> {
        m.slots.iter().filter(|s| s.images).map(|s| s.name.as_str()).collect()
    }
    fn private_(m: &Models) -> Vec<&str> {
        m.slots.iter().filter(|s| s.private).map(|s| s.name.as_str()).collect()
    }

    /// How a call naming the slot `name` by its id names it, when no route
    /// runs on it.
    fn slot(m: &Models, name: &str) -> Named {
        Named::Slot(u8::try_from(m.slot_at(name).unwrap()).unwrap())
    }

    /// Finite's own deployment's models (deploy/example.jsonc; decision
    /// 61): its tiers on Finite Private, its fallback and vision on
    /// Workers AI's DeepSeek V4.1 Flash.
    const FINITE: &str = r#"{
        "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" },
        "medium": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" },
        "fallback": { "provider": "workers_ai", "model": "@cf/deepseek-ai/deepseek-v4.1-flash-1017" },
        "vision": { "provider": "workers_ai", "model": "@cf/deepseek-ai/deepseek-v4.1-flash-1017" }
    }"#;

    fn finite() -> Models {
        old_parse(FINITE, &PriceBook::defaults(), false).unwrap()
    }

    fn openai(base_url: &str, model: &str) -> Upstream {
        Upstream::Openai { base_url: base_url.into(), model: model.into(), body: None, price: None }
    }

    /// OpenRouter's provider routing, as Finite's slots name it
    /// (deploy/example.jsonc; decision 61).
    fn routing() -> Value {
        json!({ "provider": { "zdr": true, "sort": "throughput", "require_parameters": true, "quantizations": ["fp8", "bf16"], "max_price": { "prompt": 0.15, "completion": 0.5 } } })
    }

    /// Finite's own deployment's models since 2026-10-15 (deploy/example.jsonc):
    /// its tiers on GLM-5.3 Flash through OpenRouter, routed by its `body`.
    fn openrouter() -> Models {
        let tier = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() });
        let flash = json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH });
        let text = json!({ "cheap": tier, "medium": tier, "fallback": flash, "vision": flash }).to_string();
        old_parse(&text, &PriceBook::defaults(), false).unwrap()
    }

    /// Goal: the route's `vision` runs the caller's route's vision model
    /// (the default route's when it says none), at the routes' cap, and is
    /// no route (an agent, a job's step and a manifest name only routes).
    /// Method: the route's names, the routes' names, and a call bounded on
    /// it.
    #[test]
    fn vision_is_the_routes_and_no_tier() {
        let named = |n: Option<&str>| DEFAULTS.named(n, None);
        assert_eq!(named(Some("vision")), Ok(Named::Vision));
        assert_eq!(named(Some("medium")), Ok(MEDIUM));
        assert_eq!(named(None), Ok(CHEAP), "none named: the default route");
        assert_eq!(named(Some("Vision")), Err(DEFAULTS.unknown()));
        assert_eq!(named(Some("high")), Err(DEFAULTS.unknown()), "high is no route of the default config's, nor any model's id");
        assert_eq!(DEFAULTS.step_route(Some("vision")), Err(DEFAULTS.unknown_route()), "vision is no route");
        assert_eq!((DEFAULTS.name_of(Named::Vision), DEFAULTS.name_of(CHEAP)), ("vision", "cheap"));
        let mut seeing = Models::defaults();
        let at = seeing.slot_at("vision").unwrap();
        seeing.slots[at].upstream = workers_ai("@cf/example/seeing");
        let v = seeing.capped(Named::Vision);
        assert_eq!(v, Capped { model: Model { named: Named::Vision, upstream: &workers_ai("@cf/example/seeing"), slot: "vision" }, max_tokens: MAX_TOKENS });
        assert_eq!(seeing.capped(MEDIUM).model.id(), MEDIUM_MODEL, "a tier's call is the tier's");
        // an image part reaches the model as it came
        let image = json!([{ "type": "text", "text": "what is on the screen?" }, { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }]);
        let b = bound(v, json!({ "model": "vision", "messages": [{ "role": "user", "content": image }] }), false).unwrap();
        assert_eq!((b.model.id(), b.input["messages"][0]["content"].clone(), b.input.get("model")), ("@cf/example/seeing", image, None));
    }

    /// Goal: the route's `fallback` runs the deployment's fallback model, at
    /// the tiers' cap, for a runtime's own switch when its model stalls
    /// (Hermes' `fallback_providers`), and is no tier. Method: the route's
    /// names, the tiers' names, and the model each is capped to, by
    /// default and on Finite's own deployment.
    #[test]
    fn fallback_is_the_routes_and_no_tier() {
        assert_eq!(DEFAULTS.named(Some("fallback"), None), Ok(Named::Fallback));
        assert_eq!(DEFAULTS.name_of(Named::Fallback), "fallback");
        assert_eq!(DEFAULTS.named(Some("Fallback"), None), Err(DEFAULTS.unknown()));
        assert_eq!(DEFAULTS.step_route(Some("fallback")), Err(DEFAULTS.unknown_route()), "fallback is no route: no agent, step or manifest names it");
        let f = DEFAULTS.capped(Named::Fallback);
        assert_eq!((f.model.named, f.model.id(), f.max_tokens), (Named::Fallback, "@cf/thinkingmachines/inkling-2-small", MAX_TOKENS));
        assert_eq!(DEFAULTS.route(Named::Fallback), f.model);
        // Hermes' stale switch on Finite's deployment: Workers AI's DeepSeek V4.1 Flash
        let finite = finite();
        assert_eq!(finite.route(Named::Fallback).upstream, &workers_ai(DEEPSEEK_FLASH), "the deployment's, whatever it names");
    }

    /// Goal (Paul, 2026-10-16): an agent's call may name its model by the
    /// model's own id, which runs on the slot of the caller's tier when that
    /// slot runs it, else on the first slot that does, so its key, its body,
    /// its fallback and its price are that slot's; the route's names still
    /// name their slots, whatever tier the caller says. Method: every id of
    /// the defaults, of Finite's OpenRouter tiers (both tiers one id) and of
    /// its Finite Private ones, with and without the caller's tier.
    #[test]
    fn a_call_names_a_model_by_its_id() {
        let (cheap, medium) = (CHEAP, MEDIUM);
        let d = &*DEFAULTS;
        assert_eq!(d.named(Some(DEEPSEEK_FLASH), None), Ok(cheap), "cheap's and vision's id: the first slot's, a tier's");
        assert_eq!(d.named(Some(DEEPSEEK_FLASH), Some(1)), Ok(cheap), "a tier whose slot does not run it is passed over");
        assert_eq!(d.named(Some(MEDIUM_MODEL), Some(0)), Ok(medium));
        assert_eq!(d.named(Some(INKLING_SMALL), Some(0)), Ok(slot(d, "fallback")), "the fallback's id, its slot exactly: no fallback of its own");
        let routed = openrouter();
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), None), Ok(cheap));
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), Some(0)), Ok(cheap));
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), Some(1)), Ok(medium), "both tiers run it: the caller's tier's");
        assert_eq!(routed.named(Some(DEEPSEEK_FLASH), Some(0)), Ok(slot(&routed, "fallback")), "fallback's and vision's: the first, fallback");
        assert_eq!((routed.name_of(slot(&routed, "fallback")), routed.name_of(slot(&routed, "vision"))), ("fallback", "vision"));
        let m = routed.capped(medium).model;
        assert_eq!((routed.name_of(m.named), m.id(), m.upstream), ("medium", OPENROUTER_FLASH, up(&routed, "medium")), "the medium slot's model, key and body");
        assert_eq!(finite().named(Some(FINITE_PRIVATE_FLASH), Some(1)), Ok(medium));
        // the route's names are names, whatever the caller's tier
        for (name, slot) in [("cheap", cheap), ("medium", medium), ("vision", Named::Vision), ("fallback", Named::Fallback)] {
            assert_eq!(routed.named(Some(name), Some(1)), Ok(slot), "{name}");
        }
        assert_eq!(routed.named(None, Some(1)), Ok(cheap), "none named is the default tier, as before");
    }

    /// Goal: an id no slot runs is refused before anything is sent, the
    /// refusal listing the ids the route takes (`GET /v1/models`'s), never
    /// the route's names; the list is OpenAI's, the ids alone, nothing of
    /// where each runs; and a guest reads each slot's id by its name.
    /// Invalid: an unknown id, one off by a letter, a name off by its case,
    /// an empty one.
    #[test]
    fn a_model_no_slot_runs_is_refused_naming_those_it_may() {
        let d = &*DEFAULTS;
        let allowed = vec![DEEPSEEK_FLASH.to_string(), MEDIUM_MODEL.to_string(), INKLING_SMALL.to_string()];
        for bad in ["gpt-4o", "@cf/deepseek-ai/deepseek-v4.1-flash-1017x", "Cheap", "", OPENROUTER_FLASH] {
            assert_eq!(d.named(Some(bad), Some(0)), Err(UnknownModel { allowed: allowed.clone() }), "{bad:?}");
        }
        let said = d.unknown().message();
        assert!(allowed.iter().all(|id| said.contains(id.as_str())) && !said.contains("cheap") && said.contains("GET /v1/models"), "{said}");
        let routed = openrouter();
        assert_eq!(routed.ids(), [OPENROUTER_FLASH, DEEPSEEK_FLASH], "each id once");
        assert_eq!(routed.named(Some(MEDIUM_MODEL), None), Err(UnknownModel { allowed: vec![OPENROUTER_FLASH.into(), DEEPSEEK_FLASH.into()] }), "a model the deployment does not run");
        let listed = routed.listed();
        assert_eq!(
            listed,
            json!({ "object": "list", "data": [
                { "id": OPENROUTER_FLASH, "object": "model", "created": 0, "owned_by": "fragment" },
                { "id": DEEPSEEK_FLASH, "object": "model", "created": 0, "owned_by": "fragment" },
            ] })
        );
        let text = listed.to_string();
        assert!(["openrouter.ai", "zdr", "cheap", "fallback", "vision", "workers_ai"].iter().all(|w| !text.contains(w)), "ids alone: {text}");
        let route = |fallback: &str, vision: &str| json!({ "private": false, "fallback": fallback, "vision": vision });
        assert_eq!(
            serde_json::to_value(routed.ids_by_name()).unwrap(),
            json!({ "cheap": OPENROUTER_FLASH, "medium": OPENROUTER_FLASH, "fallback": DEEPSEEK_FLASH, "vision": DEEPSEEK_FLASH, "images": [], "routes": { "cheap": route(DEEPSEEK_FLASH, DEEPSEEK_FLASH), "medium": route(DEEPSEEK_FLASH, DEEPSEEK_FLASH) } })
        );
        assert_eq!(
            serde_json::to_value(d.ids_by_name()).unwrap(),
            json!({ "cheap": DEEPSEEK_FLASH, "medium": MEDIUM_MODEL, "fallback": INKLING_SMALL, "vision": DEEPSEEK_FLASH, "images": [], "routes": { "cheap": route(INKLING_SMALL, DEEPSEEK_FLASH), "medium": route(INKLING_SMALL, DEEPSEEK_FLASH) } })
        );
        // a reader of the view before its routes (a pinned image's) reads it still
        let before: fragment_proto::computer::ModelIds = serde_json::from_value(json!({ "cheap": DEEPSEEK_FLASH, "medium": MEDIUM_MODEL, "fallback": INKLING_SMALL, "vision": DEEPSEEK_FLASH })).unwrap();
        assert_eq!(before.routes, None);
    }

    /// Goal: a deployment's `models` name where each slot runs and which
    /// routes people pick; a config of the shape before routes were a list
    /// means what it did, each of the four slots it does not name as
    /// before; Finite's own (decision 61) puts its tiers on Finite Private
    /// and its fallback and vision on Workers AI's DeepSeek V4.1 Flash.
    /// Method: none named, an empty old config (the default config, to the
    /// letter), Finite's, and each slot alone.
    #[test]
    fn a_deployments_models_name_where_each_runs() {
        let book = PriceBook::defaults();
        assert_eq!(Models::parse(None, &book, false), Ok(Models::defaults()));
        assert_eq!(legacy(&json!({})), Ok(default_config()), "an empty old config names none: the default config");
        assert_eq!(old_parse("{}", &book, false), Ok(Models::defaults()));
        let d = Models::defaults();
        let slots = |m: &Models| m.slots.iter().map(|s| (s.name.clone(), s.upstream.model().to_string(), s.fallback.map(|f| m.slots[f].name.clone()))).collect::<Vec<_>>();
        let named = |n: &str, id: &str, fallback: Option<&str>| (n.to_string(), id.to_string(), fallback.map(str::to_string));
        assert_eq!(
            slots(&d),
            [named("cheap", DEEPSEEK_FLASH, Some("fallback")), named("fallback", INKLING_SMALL, None), named("medium", MEDIUM_MODEL, Some("fallback")), named("vision", DEEPSEEK_FLASH, None)],
            "every slot on Workers AI, as before"
        );
        assert!(d.slots.iter().all(|s| matches!(s.upstream, Upstream::WorkersAi { .. }) && !s.images && !s.private));
        let routes = |m: &Models| m.routes.iter().map(|r| (r.name.clone(), r.label.clone(), m.slots[r.slot].name.clone(), m.slots[r.vision].name.clone(), r.deep.map(|d| m.slots[d].name.clone()))).collect::<Vec<_>>();
        let route = |n: &str| (n.to_string(), None, n.to_string(), "vision".to_string(), None);
        assert_eq!((routes(&d), d.default), (vec![route("cheap"), route("medium")], 0), "cheap and medium, cheap the default, as the tiers were");
        let finite = finite();
        let private = openai("https://finite-private.finite.containers.tinfoil.dev/v1", FINITE_PRIVATE_FLASH);
        assert_eq!((up(&finite, "cheap"), up(&finite, "medium"), up(&finite, "fallback"), up(&finite, "vision")), (&private, &private, &workers_ai(DEEPSEEK_FLASH), &workers_ai(DEEPSEEK_FLASH)));
        assert_eq!(routes(&finite), routes(&d));
        assert_eq!(finite.route(CHEAP), Model { named: CHEAP, upstream: &private, slot: "cheap" });
        assert_eq!(finite.route(MEDIUM).id(), "glm-5-3-flash");
        // one slot alone: the rest as before
        let medium = old_parse(r#"{"medium": {"provider": "workers_ai", "model": "@cf/deepseek-ai/deepseek-v4.1-flash-1017"}}"#, &book, false).unwrap();
        let mut expected = Models::defaults();
        let at = expected.slot_at("medium").unwrap();
        expected.slots[at].upstream = workers_ai(DEEPSEEK_FLASH);
        assert_eq!(medium, expected);
        // its wire form is the config's, less the key (the deploy's)
        assert_eq!(serde_json::to_value(&private).unwrap(), json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" }));
        assert_eq!(serde_json::to_value(workers_ai(DEEPSEEK_FLASH)).unwrap(), json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH }));
        // OpenRouter's (Finite's since 2026-10-15): its tiers routed by their body, the rest on Workers AI
        let routed = Upstream::Openai { base_url: "https://openrouter.ai/api/v1".into(), model: OPENROUTER_FLASH.into(), body: routing().as_object().cloned(), price: None };
        let or = openrouter();
        assert_eq!((up(&or, "cheap"), up(&or, "medium"), up(&or, "fallback"), up(&or, "vision")), (&routed, &routed, &workers_ai(DEEPSEEK_FLASH), &workers_ai(DEEPSEEK_FLASH)));
        assert_eq!(serde_json::to_value(&routed).unwrap(), json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": "z-ai/glm-5.3-flash", "body": routing() }));
        let empty = old_parse(&json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH, "body": {} } }).to_string(), &book, false).unwrap();
        assert_eq!(up(&empty, "cheap"), &Upstream::Openai { base_url: "https://a.example/v1".into(), model: FINITE_PRIVATE_FLASH.into(), body: Some(Map::new()), price: None }, "an empty body adds nothing");
    }

    /// Goal: a deployment's `models` that the route could not serve is
    /// refused, saying which slot and why, before the deploy and at the
    /// cell's first request. Invalid: an unknown slot (`high` among them),
    /// an unknown provider, a missing or extra field (a key is the deploy's,
    /// never the cell's), a model the book does not price, and a base URL
    /// that is no https on a host name, names a query or a trailing /, or is
    /// a local fleet's fake where none may be, and an id two slots run at two
    /// places. Valid: a local fleet's fake, and one id at one place twice.
    #[test]
    fn a_deployments_models_out_of_shape_are_refused() {
        let book = PriceBook::defaults();
        let parse = |text: &str, loopback: bool| old_parse(text, &book, loopback);
        let slot = |row: Value| json!({ "cheap": row }).to_string();
        let refused = |text: &str, says: &str| {
            let e = parse(text, false).unwrap_err();
            assert!(e.contains(says), "{text}: {e}");
        };
        refused(&slot(json!({ "provider": "anthropic", "model": "glm-5-3-flash" })), "unknown variant `anthropic`");
        // a provider is its name, a string, as the tagged reading's tag was
        refused(&slot(json!({ "provider": { "workers_ai": null }, "model": DEEPSEEK_FLASH })), "invalid type: map");
        refused(&slot(json!({ "provider": { "openai": null }, "base_url": "https://a.example/v1", "model": "glm-5-3-flash" })), "invalid type: map");
        refused(&slot(json!({ "model": "glm-5-3-flash" })), "missing field `provider`");
        refused(&slot(json!({ "provider": "openai", "model": "glm-5-3-flash" })), "missing field `base_url`");
        refused(&slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "glm-5-3-flash", "key": "fragment-finite-private-key" })), "unknown field `key`");
        refused(&slot(json!({ "provider": "workers_ai", "base_url": "https://a.example/v1", "model": DEEPSEEK_FLASH })), "unknown field `base_url`");
        refused(&slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "glm-6" })), "models.slots.cheap: the model \"glm-6\" is not in the price book");
        refused(&slot(json!({ "provider": "workers_ai", "model": "@cf/meta/llama-4-scout-17b-16e-instruct" })), "not in the price book");
        refused(r#"{"vision": {"provider": "workers_ai", "model": ""}}"#, "models.slots.vision: the model \"\" is not in the price book");
        for bad in ["http://a.example/v1", "https://a.example:8443/v1", "https://user@a.example/v1", "ftp://a.example/v1", "a.example/v1", "http://127.0.0.1:4810/v1"] {
            refused(&slot(json!({ "provider": "openai", "base_url": bad, "model": "glm-5-3-flash" })), "models.slots.cheap: base_url:");
        }
        for bad in ["https://a.example/v1/", "https://a.example/v1?x=1"] {
            refused(&slot(json!({ "provider": "openai", "base_url": bad, "model": "glm-5-3-flash" })), "without a query or a trailing /");
        }
        refused("[]", "models is an object");
        // a body: an object of the API's own fields, within its bound, naming none of the route's
        let with_body = |body: Value| slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH, "body": body }));
        for bad in [json!("zdr"), json!([routing()]), json!(null), json!(7)] {
            refused(&with_body(bad.clone()), "models.slots.cheap: body is a JSON object of the fields every call to this API carries");
        }
        for key in ROUTE_KEYS {
            refused(&with_body(json!({ key: 1, "provider": { "zdr": true } })), &format!("models.slots.cheap: body names {key}, which the route writes itself"));
        }
        let two = parse(&with_body(json!({ "model": "anthropic/claude-opus-5.5", "messages": [] })), false).unwrap_err();
        assert!(two.contains("messages") && two.contains("model,") && two.contains("which the route writes itself"), "{two}");
        let pad = |n: usize| json!({ "x": "a".repeat(n) });
        let at = UPSTREAM_BODY_MAX_BYTES - serde_json::to_vec(&pad(0)).unwrap().len();
        assert!(parse(&with_body(pad(at)), false).is_ok(), "{UPSTREAM_BODY_MAX_BYTES} bytes of JSON at most");
        refused(&with_body(pad(at + 1)), &format!("models.slots.cheap: body is {} bytes of JSON, past {UPSTREAM_BODY_MAX_BYTES}", UPSTREAM_BODY_MAX_BYTES + 1));
        refused(&slot(json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH, "body": routing() })), "unknown field `body`");
        // a local fleet's fake, where its egress may reach local addresses
        let local = slot(json!({ "provider": "openai", "base_url": "http://127.0.0.1:4810/v1", "model": "glm-5-3-flash" }));
        assert_eq!(up(&parse(&local, true).unwrap(), "cheap"), &openai("http://127.0.0.1:4810/v1", FINITE_PRIVATE_FLASH));
        assert!(parse(&local, false).is_err(), "a deployment never names one");
        // an id is one model: two slots naming it name one place (a call names it by its id)
        let elsewhere = json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": DEEPSEEK_FLASH } }).to_string();
        refused(&elsewhere, "models.slots.vision: \"@cf/deepseek-ai/deepseek-v4.1-flash-1017\" is models.slots.cheap's model too, where it runs elsewhere");
        let two_apis = json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH }, "medium": { "provider": "openai", "base_url": "https://b.example/v1", "model": FINITE_PRIVATE_FLASH } });
        refused(&two_apis.to_string(), "models.slots.medium: \"glm-5-3-flash\" is models.slots.cheap's model too");
        let one_api = json!({ "cheap": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() }, "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": { "provider": { "sort": "price", "max_price": routing()["provider"]["max_price"] } } } });
        assert!(parse(&one_api.to_string(), false).is_ok(), "one id at one place, its bodies its slots' own, at one ceiling");
        // a model the book prices, whichever slot names it
        let mut other = book.clone();
        other.models.retain(|m| m.model != FINITE_PRIVATE_FLASH);
        assert!(old_parse(FINITE, &other, false).unwrap_err().contains("models.slots.cheap: the model \"glm-5-3-flash\" is not in the price book"));
    }

    /// Goal: a model on OpenRouter is priced at the ceiling its slot routes
    /// under (`body.provider.max_price`), from the config alone, whatever
    /// its id (Paul, 2026-10-22): every input token, cached and written
    /// ones too, at `prompt`, output at `completion`, its cost basis that
    /// ceiling with OpenRouter's 5.5% fee, not AI Gateway's 5% (a decision
    /// for Paul). Valid: Finite's GLM-5.3 Flash config charged to the
    /// micro-dollar as its hand-kept row (150,715 / 502,381 µ$ a million),
    /// which stays for its usage queued past a switch; another id at that
    /// ceiling priced the same; a ceiling over a row of the book's; a
    /// ceiling on another API (the e2e's fake). Invalid: a slot on
    /// OpenRouter, however its URL is spelled (case, port 443, a
    /// subdomain), with no ceiling, half a ceiling, or one that is not
    /// dollars from 0 to `CEILING_MAX_USD`; one id at two ceilings.
    #[test]
    fn an_openrouter_call_is_charged_its_own_cost() {
        // FIN-616: an OpenRouter slot's call is charged what OpenRouter said
        // it cost (`usage.cost`), with its credits' fee and the margin, not
        // its ceiling; with no cost, or one that is no dollars, its tokens at
        // the ceiling, as before; another API's `cost` is not read. Method:
        // the Flash config's call S3 measured, a turn mostly cached.
        let models = openrouter();
        let defaults = PriceBook::defaults();
        let mut book = models.priced(defaults.clone());
        book.keys.extend(models.cost_keys());
        assert_eq!(book.validate(), Ok(()));
        let slot = models.route(models.named(Some("cheap"), None).unwrap());
        let usage = json!({ "prompt_tokens": 30_000, "completion_tokens": 600, "prompt_tokens_details": { "cached_tokens": 25_000 }, "cost": 0.001234567 });
        let used = slot.upstream.used(slot.id(), &usage).unwrap();
        assert_eq!(used, Usage::Key { key: OPENROUTER_COST.into(), units: 1_234_567 });
        // $0.001234567 with the 5.5% fee and the 50% margin, rounded up once
        let paid = book.price(&used).unwrap();
        assert_eq!((paid.list, paid.charge), (1_303, 1_954));
        let ceiling = book.price(&usage_of(slot.id(), &usage).unwrap()).unwrap().charge;
        assert!(paid.charge < ceiling, "less than its ceiling: {} < {ceiling}", paid.charge);
        for none in [json!(null), json!(-0.01), json!("0.01"), json!(1e12)] {
            let mut u = usage.clone();
            u["cost"] = none.clone();
            assert_eq!(slot.upstream.used(slot.id(), &u), usage_of(slot.id(), &u), "{none}: its tokens");
        }
        let mut no_cost = usage.clone();
        no_cost.as_object_mut().unwrap().remove("cost");
        assert!(matches!(slot.upstream.used(slot.id(), &no_cost), Some(Usage::Tokens { .. })));
        let workers = Models::defaults();
        let cf = workers.route(workers.named(Some("cheap"), None).unwrap());
        assert!(matches!(cf.upstream.used(cf.id(), &usage), Some(Usage::Tokens { .. })), "Workers AI's answer names no cost of OpenRouter's");
        assert!(workers.cost_keys().is_empty(), "no OpenRouter slot, no row");
    }

    #[test]
    fn an_openrouter_slot_is_priced_at_its_ceiling() {
        use crate::price::{Priced, BP, OPENROUTER_FEE_BP};
        let defaults = PriceBook::defaults();
        let kept = defaults.models.iter().find(|m| m.model == OPENROUTER_FLASH).map(|m| m.price);
        let tokens = |model: &str, input, cached_input, cache_write, output| Usage::Tokens { model: model.into(), input, cached_input, cache_write, output };
        let at = |model: &str, max_price: Value| {
            let tier = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": model, "body": { "provider": { "zdr": true, "max_price": max_price } } });
            old_parse(&json!({ "cheap": tier, "medium": tier }).to_string(), &defaults, false)
        };
        let glm = openrouter().priced(defaults.clone());
        let row = |book: &PriceBook, model: &str| book.models.iter().find(|m| m.model == model).map(|m| m.price);
        assert_eq!(row(&glm, OPENROUTER_FLASH), Some(TokenPrices { input: 150_715, cached_input: 150_715, cache_write: 150_715, output: 502_381 }), "today's GLM-5.3 Flash, unchanged");
        assert_eq!((glm.models.len(), kept), (defaults.models.len(), row(&glm, OPENROUTER_FLASH)), "the config's ceiling over the code's row, the same: one row for the two tiers that run it");
        assert_eq!(glm.validate(), Ok(()));
        let deepseek = at("deepseek/deepseek-v4.1-flash", json!({ "prompt": 0.15, "completion": 0.5 })).unwrap().priced(defaults.clone());
        for (book, model) in [(&glm, OPENROUTER_FLASH), (&deepseek, "deepseek/deepseek-v4.1-flash")] {
            let million = |input, cached_input, cache_write, output| book.price(&tokens(model, input, cached_input, cache_write, output)).unwrap();
            let m = 1_000_000;
            for (p, ceiling) in [(million(m, 0, 0, 0), 150_000), (million(0, m, 0, 0), 150_000), (million(0, 0, m, 0), 150_000), (million(0, 0, 0, m), 500_000)] {
                let with_fee = ceiling * i64::from(BP + OPENROUTER_FEE_BP) / i64::from(BP);
                assert!((with_fee..=with_fee + 1).contains(&p.cost), "{model} {p:?}: OpenRouter bills at most {with_fee} µ$ with its fee");
                assert!(p.charge >= p.cost * 3 / 2, "{model} {p:?}: the margin on top");
            }
            assert_eq!(million(m, 0, 0, 0), Priced { list: 150_715, cost: 158_251, charge: 237_377 }, "{model}");
            assert_eq!(million(0, 0, 0, m), Priced { list: 502_381, cost: 527_501, charge: 791_251 }, "{model}");
            assert_eq!(book.price(&tokens(model, 26_000, 0, 0, 200)).unwrap(), Priced { list: 4_020, cost: 4_221, charge: 6_331 }, "{model}: S3's turn");
            // an agent's call, most of its prompt cached: dearer than on Workers AI, which priced a cached
            // token at a fifth (GLM-5.3 Flash there, retired 2026-11-20, its row kept)
            let call = |model: &str| book.price(&tokens(model, 5_000, 25_000, 0, 600)).unwrap().charge;
            assert_eq!((call(model), call("@cf/zai-org/glm-5.3-flash")), (7_597, 2_835), "{model}");
        }
        // after the switch, GLM's usage still queued is priced as it was: the code keeps its row
        assert_eq!(row(&deepseek, OPENROUTER_FLASH), kept);
        assert_eq!(deepseek.models.len(), defaults.models.len() + 1);
        assert_eq!(deepseek.price(&tokens(OPENROUTER_FLASH, 26_000, 0, 0, 200)).unwrap(), glm.price(&tokens(OPENROUTER_FLASH, 26_000, 0, 0, 200)).unwrap());
        // a ceiling's cents, rounded up to a micro-dollar a million, never down
        let odd = up(&at("deepseek/deepseek-v4.1-flash", json!({ "prompt": 0.1234567, "completion": 0 })).unwrap(), "cheap").ceiling().unwrap().unwrap();
        assert_eq!((odd.input, odd.output), (crate::price::openrouter(123_457), 0));
        // a ceiling over the book's own row of that id
        let opus = at("anthropic/claude-opus-5.5", json!({ "prompt": 5, "completion": 25 })).unwrap().priced(defaults.clone());
        assert_eq!(row(&opus, "anthropic/claude-opus-5.5").map(|p| (p.input, p.cached_input, p.output)), Some((crate::price::openrouter(5_000_000), crate::price::openrouter(5_000_000), crate::price::openrouter(25_000_000))));
        assert_eq!(opus.models.len(), defaults.models.len());
        // on another API (the e2e's fake), a ceiling prices it too; without one, the book must
        let fake = |body: Value| json!({ "medium": { "provider": "openai", "base_url": "http://127.0.0.1:4810/v1", "model": "deepseek/deepseek-v4.1-flash", "body": body } }).to_string();
        let local = old_parse(&fake(routing()), &defaults, true).unwrap().priced(defaults.clone());
        assert_eq!(row(&local, "deepseek/deepseek-v4.1-flash"), row(&glm, OPENROUTER_FLASH));
        assert!(old_parse(&fake(json!({ "provider": { "zdr": true } })), &defaults, true).unwrap_err().contains("models.slots.medium: the model \"deepseek/deepseek-v4.1-flash\" is not in the price book"));
        // refused, saying why
        let refused = |r: Result<Models, String>, says: &str| {
            let e = r.unwrap_err();
            assert!(e.contains(says), "{e}");
        };
        let bare = |body: Option<Value>| {
            let mut tier = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH });
            if let Some(body) = body {
                tier["body"] = body;
            }
            old_parse(&json!({ "cheap": tier }).to_string(), &defaults, false)
        };
        let no_ceiling = "models.slots.cheap: a model on OpenRouter is priced at the ceiling its calls route under: name it in body.provider.max_price";
        refused(bare(None), no_ceiling);
        refused(bare(Some(json!({ "provider": { "zdr": true, "sort": "throughput" } }))), no_ceiling);
        refused(bare(Some(json!({ "transforms": [] }))), no_ceiling);
        // OpenRouter by its host as the URL parses, however it is spelled, even for a model the book prices
        let spelled = |base_url: &str, model: &str| old_parse(&json!({ "cheap": { "provider": "openai", "base_url": base_url, "model": model } }).to_string(), &defaults, false);
        for base_url in ["https://OPENROUTER.AI/api/v1", "https://openrouter.ai:443/api/v1", "https://OpenRouter.ai:443/api/v1", "https://eu.openrouter.ai/api/v1", "https://EU.OpenRouter.AI/api/v1"] {
            for model in ["anthropic/claude-opus-5.5", OPENROUTER_FLASH] {
                refused(spelled(base_url, model), no_ceiling);
            }
        }
        for elsewhere in ["https://notopenrouter.ai/api/v1", "https://openrouter.ai.example/api/v1"] {
            assert!(spelled(elsewhere, "anthropic/claude-opus-5.5").is_ok(), "{elsewhere} is another API, the book's row its price");
        }
        refused(at(OPENROUTER_FLASH, json!({ "prompt": 0.15 })), "models.slots.cheap: body.provider.max_price.completion is the dollars a million tokens its calls are priced at, from 0 to 1000, not nothing");
        refused(at(OPENROUTER_FLASH, json!({ "completion": 0.5 })), "body.provider.max_price.prompt is the dollars a million tokens");
        for bad in [json!(-0.01), json!("0.15"), json!(1000.5), json!(null), json!({ "usd": 0.15 })] {
            refused(at(OPENROUTER_FLASH, json!({ "prompt": bad, "completion": 0.5 })), &format!("body.provider.max_price.prompt is the dollars a million tokens its calls are priced at, from 0 to 1000, not {bad}"));
        }
        refused(at(OPENROUTER_FLASH, json!(0.15)), "body.provider.max_price.prompt is the dollars a million tokens");
        assert!(at(OPENROUTER_FLASH, json!({ "prompt": 1000, "completion": 0 })).is_ok(), "{CEILING_MAX_USD} at most");
        let two = json!({ "cheap": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() }, "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": { "provider": { "max_price": { "prompt": 0.2, "completion": 0.5 } } } } });
        refused(old_parse(&two.to_string(), &defaults, false), "models.slots.medium: \"z-ai/glm-5.3-flash\" is models.slots.cheap's model too, at another price");
    }

    /// Tinfoil's DeepSeek V4.1 Flash as a slot names it: its list on
    /// 2026-10-22 (`GET https://inference.tinfoil.sh/v1/models`), $0.65 in,
    /// $0.13 cached, $1.45 out a million tokens.
    fn tinfoil(price: Value) -> Value {
        json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": price })
    }

    /// Goal: a model on an API that is not OpenRouter is priced at the list
    /// price its slot names (`price`), from the config alone, whatever its
    /// id (Paul, 2026-10-22: a model is a config's choice): input at
    /// `input`, cached input at `cached_input` (else `input`), a cache write
    /// at `input`, output at `output`, then the book's fee and margin as on
    /// any row. Valid: Tinfoil's DeepSeek V4.1 Flash as the fallback,
    /// charged to the micro-dollar; its cached price left out; a price over
    /// a row of the book's (Finite Private's); one id at one price in two
    /// slots; a price of 0. Invalid: a price on OpenRouter (its ceiling is
    /// its price), beside a ceiling, out of 0 to `CEILING_MAX_USD`, a field
    /// it does not take, one missing, one not a number; one id at two prices.
    #[test]
    fn a_slot_names_its_models_list_price() {
        use crate::price::Priced;
        let defaults = PriceBook::defaults();
        let row = |book: &PriceBook, model: &str| book.models.iter().find(|m| m.model == model).map(|m| m.price);
        let parse = |models: Value| old_parse(&models.to_string(), &defaults, false);
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let models = parse(json!({ "fallback": tinfoil(listed.clone()) })).unwrap();
        assert!(matches!(up(&models, "fallback"), Upstream::Openai { price: Some(p), .. } if Value::Object(p.clone()) == listed), "the fallback names its price: {:?}", up(&models, "fallback"));
        assert_eq!(serde_json::to_value(up(&models, "fallback")).unwrap(), tinfoil(listed.clone()), "its wire form is the config's");
        let book = models.priced(defaults.clone());
        assert_eq!(row(&book, "deepseek-v4-1-flash"), Some(TokenPrices { input: 650_000, cached_input: 130_000, cache_write: 650_000, output: 1_450_000 }));
        assert_eq!(book.models.len(), defaults.models.len() + 1);
        assert_eq!(book.validate(), Ok(()));
        let tokens = |input, cached_input, output| Usage::Tokens { model: "deepseek-v4-1-flash".into(), input, cached_input, cache_write: 0, output };
        // the list, the book's 5% fee on it, then its 50% margin
        assert_eq!(book.price(&tokens(1_000_000, 0, 0)).unwrap(), Priced { list: 650_000, cost: 682_500, charge: 1_023_750 });
        assert_eq!(book.price(&tokens(0, 1_000_000, 0)).unwrap(), Priced { list: 130_000, cost: 136_500, charge: 204_750 });
        assert_eq!(book.price(&tokens(0, 0, 1_000_000)).unwrap(), Priced { list: 1_450_000, cost: 1_522_500, charge: 2_283_750 });
        // an agent's call, most of its prompt cached
        assert_eq!(book.price(&tokens(5_000, 25_000, 600)).unwrap().charge, 11_608);
        // its cached price left out: its input's, never cheaper than it could be
        let uncached = parse(json!({ "fallback": tinfoil(json!({ "input": 0.65, "output": 1.45 })) })).unwrap().priced(defaults.clone());
        assert_eq!(row(&uncached, "deepseek-v4-1-flash").map(|p| p.cached_input), Some(650_000));
        // over the book's own row of that id: Finite Private's, named by the config
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "price": { "input": 0.2, "cached_input": 0.04, "output": 0.6 } });
        let book = parse(json!({ "cheap": private, "medium": private, "vision": private })).unwrap().priced(defaults.clone());
        assert_eq!(row(&book, FINITE_PRIVATE_FLASH), Some(TokenPrices { input: 200_000, cached_input: 40_000, cache_write: 200_000, output: 600_000 }));
        assert_eq!(book.models.len(), defaults.models.len(), "one row, the config's over the code's");
        assert!(parse(json!({ "fallback": tinfoil(json!({ "input": 0, "output": 0 })) })).is_ok(), "a free model is priced at nothing");
        assert!(parse(json!({ "fallback": tinfoil(json!({ "input": 1000, "output": 1000 })) })).is_ok(), "{CEILING_MAX_USD} at most");
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert!(e.contains(says), "{models}: {e}");
        };
        let routed = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "price": listed });
        refused(json!({ "cheap": routed }), "models.slots.cheap: a model on OpenRouter is priced at its ceiling (body.provider.max_price), not a price of its own");
        let mut both = tinfoil(listed.clone());
        both["body"] = json!({ "provider": { "max_price": { "prompt": 1, "completion": 2 } } });
        refused(json!({ "fallback": both }), "models.slots.fallback: name its price, or a ceiling (body.provider.max_price), not both");
        for (field, bad) in [("input", json!(-0.01)), ("output", json!(1000.5)), ("cached_input", json!(-1))] {
            let mut p = listed.clone();
            p[field] = bad.clone();
            refused(json!({ "fallback": tinfoil(p) }), &format!("models.slots.fallback: price.{field} is the dollars a million tokens its API lists, from 0 to 1000, not {bad}"));
        }
        refused(json!({ "fallback": tinfoil(json!({ "input": 0.65, "output": 1.45, "cache_write": 0.65 })) }), "models.slots.fallback: price names \"cache_write\": it names input, cached_input (optional) and output");
        refused(json!({ "fallback": tinfoil(json!({ "input": 0.65 })) }), "models.slots.fallback: price.output is the dollars a million tokens its API lists, from 0 to 1000, not nothing");
        refused(json!({ "fallback": tinfoil(json!({ "input": "0.65", "output": 1.45 })) }), "models.slots.fallback: price.input is the dollars a million tokens its API lists, from 0 to 1000, not \"0.65\"");
        refused(json!({ "fallback": tinfoil(json!(0.65)) }), "models.slots.fallback: price is a JSON object, {\"input\": <$>, \"cached_input\"?: <$>, \"output\": <$>} a million tokens, not 0.65");
        refused(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "price": listed } }), "unknown field `price`");
        // one id is one row: two slots running it name one price
        let dearer = tinfoil(json!({ "input": 0.7, "output": 1.45 }));
        refused(json!({ "fallback": tinfoil(listed.clone()), "vision": dearer }), "models.slots.vision: \"deepseek-v4-1-flash\" is models.slots.fallback's model too, at another price");
        assert!(parse(json!({ "fallback": tinfoil(listed.clone()), "vision": tinfoil(listed.clone()) })).is_ok(), "one id at one price, twice");
        // without one, a model the book does not price is refused, saying what to name
        let bare = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash" });
        refused(json!({ "fallback": bare }), "models.slots.fallback: the model \"deepseek-v4-1-flash\" is not in the price book, which prices");
        refused(json!({ "fallback": bare }), "name its list price at its API (price: {\"input\": <$>, \"cached_input\"?: <$>, \"output\": <$>} a million tokens)");
    }

    /// Goal: a `vision` call falls back to the deployment's fallback only
    /// when the fallback's slot says its model reads images (`"images":
    /// true`), on either kind of upstream; a tier's call falls back as
    /// before, whatever its fallback reads; the fallback running the vision
    /// model's own call is none. Invalid: an `images` that is no boolean.
    /// Method: the defaults (DeepSeek reads no images), the private tier's
    /// (Finite Private's vision, Tinfoil's fallback reading images), and
    /// `Tries` over each.
    #[test]
    fn vision_falls_back_only_to_a_model_that_reads_images() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        assert_eq!(DEFAULTS.fallback_of(VISION), None, "Inkling 2 Small, the default fallback, is not said to read images");
        assert!(images_of(&DEFAULTS).is_empty(), "unsaid, no slot reads images");
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH });
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let mut seeing = tinfoil(listed.clone());
        seeing["images"] = json!(true);
        let tier = parse(json!({ "cheap": private, "medium": private, "fallback": seeing, "vision": private })).unwrap();
        assert_eq!(images_of(&tier), ["fallback"]);
        assert_eq!(up(&tier, "fallback"), up(&parse(json!({ "fallback": tinfoil(listed.clone()) })).unwrap(), "fallback"), "where it runs is as before: images is the slot's");
        assert_eq!(tier.fallback_of(VISION), Some(tier.route(Named::Fallback)));
        // a vision call on Finite Private, failed before answering: once more on Tinfoil
        let mut tries = Tries::new(tier.route(Named::Vision), tier.fallback_of(VISION));
        assert_eq!(tries.after(Tried::Answered(503)).map(|m| m.id()), Some("deepseek-v4-1-flash"));
        assert_eq!(tries.after(Tried::Answered(503)), None, "and no third try");
        // a fallback that says nothing: a vision call has none, a tier's call falls back as ever
        let unsaid = parse(json!({ "cheap": private, "medium": private, "fallback": tinfoil(listed.clone()), "vision": private })).unwrap();
        assert_eq!(unsaid.fallback_of(VISION), None);
        assert_eq!(Tries::new(unsaid.route(Named::Vision), unsaid.fallback_of(VISION)).after(Tried::Answered(503)), None, "its failure is the call's");
        assert!(Tries::new(unsaid.route(CHEAP), Some(unsaid.route(Named::Fallback))).after(Tried::Answered(503)).is_some());
        let false_said = parse(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "images": false } })).unwrap();
        assert_eq!(false_said.fallback_of(VISION), None, "false is unsaid");
        // on Workers AI too; and DeepSeek V4.1 Flash behind its own vision call is no fallback
        let both = parse(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "images": true } })).unwrap();
        assert_eq!((up(&both, "fallback").clone(), both.fallback_of(VISION).map(|m| m.id())), (workers_ai(DEEPSEEK_FLASH), Some(DEEPSEEK_FLASH)));
        assert_eq!(Tries::new(both.route(Named::Vision), both.fallback_of(VISION)).after(Tried::Answered(503)), None, "the same model again is no fallback");
        // refused, saying why
        for bad in [json!("yes"), json!(1), json!(null)] {
            let e = parse(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "images": bad } })).unwrap_err();
            assert_eq!(e, format!("models.slots.fallback: images is true (its model reads images) or false, not {bad}"));
        }
        assert!(parse(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "seeing": true } })).unwrap_err().contains("unknown field `seeing`"), "the rest is the upstream's, as before");
        assert!(parse(json!([])).unwrap_err().starts_with("models is an object"), "no object: refused");
    }

    #[test]
    fn native_images_keep_their_capability_and_never_fall_back_to_text_only() {
        let mut models = Models::defaults();
        let at = |m: &Models, name: &str| m.slot_at(name).unwrap();
        let cheap = at(&models, "cheap");
        models.slots[cheap].images = true;
        let user = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,eA=="}}]}]});
        let tool = json!({"messages":[{"role":"tool","content":user["messages"][0]["content"]}]});
        let text = json!({"messages":[{"role":"user","content":"hello"}]});
        assert!(models.fallback_for_input("cheap", &text).is_some(), "existing text fallback");
        assert!(models.fallback_for_input("cheap", &user).is_none());
        assert!(models.fallback_for_input("cheap", &tool).is_none());
        let fallback = at(&models, "fallback");
        models.slots[fallback].images = true;
        assert!(models.fallback_for_input("cheap", &user).is_some());
        assert!(models.fallback_for_input("cheap", &tool).is_some());
        let ids = models.ids_by_name();
        assert_eq!(ids.images.as_ref().unwrap(), &vec![DEEPSEEK_FLASH.to_string(), INKLING_SMALL.to_string()]);
        let replay = serde_json::from_value::<fragment_proto::computer::ModelIds>(serde_json::to_value(&ids).unwrap()).unwrap();
        assert_eq!(ids, replay);
        let old: fragment_proto::computer::ModelIds = serde_json::from_value(json!({"cheap":"a","medium":"b","fallback":"c","vision":"d"})).unwrap();
        assert!(old.images.is_none(), "old cell metadata remains readable");
    }

    /// Goal (Paul, 2026-10-22: "When a user has private selected, it's not
    /// appropriate to fall back to a non-private model"): a deployment whose
    /// tiers are private keeps every slot their calls reach private, their
    /// images' (`vision`) and their fallback's, and a private vision model's
    /// fallback too; Workers AI and OpenRouter are never private. Valid: the
    /// private tier (all four private), a private fallback behind tiers that
    /// are not, nothing private (today's configs, unchanged). Invalid: a
    /// private tier with vision or the fallback on Workers AI, a private
    /// vision model with a fallback that is not, a slot on Workers AI or
    /// OpenRouter marked private, a mark that is no boolean.
    #[test]
    fn a_private_slots_calls_reach_only_private_models() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert_eq!(e, says, "{models}");
        };
        let mark = |mut slot: Value| {
            slot["private"] = json!(true);
            slot
        };
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH });
        let mut seeing = tinfoil(json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 }));
        seeing["images"] = json!(true);
        let flash = json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH });
        let tier = parse(json!({ "cheap": mark(private.clone()), "medium": mark(private.clone()), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!((private_(&tier), images_of(&tier)), (vec!["cheap", "fallback", "medium", "vision"], vec!["fallback"]));
        assert_eq!(up(&tier, "cheap"), up(&parse(json!({ "cheap": private })).unwrap(), "cheap"), "where it runs is as before: private is the slot's");
        assert!(private_(&DEFAULTS).is_empty());
        assert!(parse(json!({ "fallback": mark(seeing.clone()) })).is_ok(), "a private fallback behind tiers that are not");
        assert!(private_(&openrouter()).is_empty(), "today's configs say nothing, and parse as before");
        // a private tier's agents reach only private slots: their turns, failures, stalls, images, and
        // Hermes' recovery of a failed vision call to their main model, which is the tier itself
        for (t, name) in [(0, "cheap"), (1, "medium")] {
            assert_eq!(tier.reached(t).iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), [name, super::FALLBACK, VISION]);
            assert!(tier.reached(t).iter().all(|(slot, _)| tier.slot_private(slot)), "{name}");
        }
        // a private tier beside one that is not (Private and Standard): each route its own
        let routed_slot = || json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() });
        let mixed = parse(json!({ "cheap": mark(private.clone()), "medium": routed_slot(), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!(private_(&mixed), ["cheap", "fallback", "vision"]);
        assert!(mixed.reached(0).iter().all(|(slot, _)| mixed.slot_private(slot)), "Private's chain is private");
        assert!(!mixed.slot_private("medium"), "Standard's agents recover to Standard: theirs, and promised nothing");
        // a private vision and fallback behind tiers that are not (#540's review): parsed, and no route
        // is private, so nothing is promised: a vision call whose fallback fails too may be recovered by
        // Hermes to the agent's own main model, a Workers AI tier, which is that agent's route
        let behind = parse(json!({ "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!(private_(&behind), ["fallback", "vision"]);
        let recovered: Vec<&str> = behind.reached(0).iter().map(|(slot, _)| *slot).collect();
        assert_eq!(recovered, ["cheap", super::FALLBACK, VISION]);
        assert!(!behind.slot_private("cheap") && !behind.slot_private("medium"), "no private route: its agents chose none");
        let mut tries = Tries::new(behind.route(Named::Vision), behind.fallback_of(VISION));
        assert_eq!(tries.after(Tried::Answered(429)).map(|m| m.id()), Some("deepseek-v4-1-flash"), "vision's 429: its fallback");
        assert_eq!(tries.after(Tried::Answered(429)), None, "the fallback's 429 too: the call's, for Hermes to recover to the main model");
        // refused, saying which slot to change and why the private tier reaches it
        let open = |tier: &str, slot: &str, why: &str| format!("models.slots.{slot}: the route {tier} is private, and its agents' {why} reach {slot}, so mark a private model there (\"private\": true)");
        refused(json!({ "cheap": mark(private.clone()), "fallback": mark(seeing.clone()) }), &open("cheap", VISION, "images"));
        refused(json!({ "medium": mark(private.clone()), "fallback": mark(seeing.clone()), "vision": private.clone() }), &open("medium", VISION, "images"));
        refused(json!({ "cheap": mark(private.clone()), "medium": mark(private.clone()), "fallback": seeing.clone(), "vision": mark(private.clone()) }), &open("cheap", super::FALLBACK, "failures and stalls"));
        refused(json!({ "medium": mark(private.clone()), "vision": mark(private.clone()) }), &open("medium", super::FALLBACK, "failures and stalls"));
        let not_private = |slot: &str| format!("models.slots.{slot}: Workers AI and OpenRouter are not private: only a confidential API (Finite Private's, Tinfoil's) is marked private");
        refused(json!({ "cheap": mark(private.clone()), "fallback": mark(flash.clone()), "vision": mark(private.clone()) }), &not_private("fallback"));
        let routed = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() });
        refused(json!({ "cheap": mark(routed), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) }), &not_private("cheap"));
        for bad in [json!("yes"), json!(1)] {
            let mut slot = private.clone();
            slot["private"] = bad.clone();
            refused(json!({ "cheap": slot }), &format!("models.slots.cheap: private is true (its model is private: confidential inference that keeps nothing) or false, not {bad}"));
        }
    }

    /// Goal: the routes Settings offers are the deployment's (Paul,
    /// 2026-10-24: "Private (recommended)" and "Standard"): a tier slot's
    /// `label` names its route, carried in the computer's view with whether
    /// it is private and its fallback's and vision model's ids; where the
    /// slot runs is as before. Invalid: a label on `fallback` or `vision`
    /// (no one picks those), an empty, overlong, multi-line or non-text one.
    #[test]
    fn a_tiers_label_names_the_route_settings_offers() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true });
        let mut labelled = private.clone();
        labelled["label"] = json!("  Private (recommended) ");
        let standard = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "label": "Standard" });
        let mut fallback = tinfoil(json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 }));
        fallback["private"] = json!(true);
        let routes = parse(json!({ "cheap": labelled, "medium": standard, "fallback": fallback, "vision": private })).unwrap();
        let labels = |m: &Models| m.routes.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(&routes), [Some("Private (recommended)".to_string()), Some("Standard".to_string())], "trimmed");
        let bare = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH });
        assert_eq!(up(&routes, "cheap"), up(&parse(json!({ "cheap": bare })).unwrap(), "cheap"), "where it runs is as before: the label is the route's");
        let view = serde_json::to_value(routes.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(labels(&DEFAULTS), [None, None], "unnamed, none: Settings offers the routes by their names, as before");
        assert!(serde_json::to_value(DEFAULTS.ids_by_name()).unwrap()["routes"]["cheap"].get("label").is_none());
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert_eq!(e, says, "{models}");
        };
        let flash = |label: Value| json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH, "label": label });
        for slot in ["fallback", "vision"] {
            let e = parse(json!({ slot: flash(json!("Backup")) })).unwrap_err();
            assert!(e.starts_with(&format!("models.slots.{slot}: unknown field `label`")), "a label is a route's: {e}");
        }
        let long = "x".repeat(LABEL_MAX_CHARS + 1);
        for bad in [json!(""), json!("   "), json!(long), json!("two\nlines"), json!(7), json!(null)] {
            refused(json!({ "cheap": flash(bad.clone()) }), &format!("models.routes.cheap: label is 1 to {LABEL_MAX_CHARS} characters of plain text, not {bad}"));
        }
        assert!(parse(json!({ "cheap": flash(json!("x".repeat(LABEL_MAX_CHARS))) })).is_ok(), "{LABEL_MAX_CHARS} at most");
    }

    /// Private and Standard as a deployment names them per route (Paul,
    /// 2026-10-24): Private the cheap tier on Finite Private, the route's
    /// four's Tinfoil fallback and Finite Private vision behind it; Standard
    /// the medium tier on OpenRouter, a slot of its own (`cloudflare`, Workers
    /// AI's DeepSeek V4.1 Flash, reading images) its fallback and its vision.
    fn per_route() -> Value {
        json!({
            "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true, "label": "Private (recommended)" },
            "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "label": "Standard", "fallback": "cloudflare", "vision": "cloudflare" },
            "fallback": { "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": { "input": 0.65, "cached_input": 0.13, "output": 1.45 }, "images": true, "private": true },
            "vision": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true },
            "cloudflare": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "images": true }
        })
    }

    /// Goal (Paul, 2026-10-24: "A fallback on private should fall back to
    /// something private, a fallback on normal should fall back to something
    /// normal"): each tier names its route, the slot its calls fall back to
    /// and the slot its images go to, among the route's four and slots of
    /// the deployment's own; a caller's `vision` and `fallback` are its
    /// route's, a caller that says no tier the route's four's; a tier's call
    /// falls back within its route; an extra slot is named by its model's
    /// id; the private guard walks each route. Valid: Private and Standard
    /// per route; today's configs (no routes named) as before. Invalid: a
    /// route naming a slot that is not there, an extra no route names, one
    /// past `EXTRA_SLOTS_MAX`, one named as no slot may be, `high`, an extra
    /// with a label or a route of its own, a private tier whose route
    /// reaches a slot that is not private. And a fallback that makes the
    /// call its model makes, whatever price each names, is none (#526's
    /// review, P3).
    #[test]
    fn each_route_falls_back_within_itself() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        let m = parse(per_route()).unwrap();
        let cf = m.slot_at("cloudflare").unwrap();
        assert_eq!(m.slots[cf], ModelSlot { name: "cloudflare".into(), upstream: workers_ai(DEEPSEEK_FLASH), images: true, private: false, fallback: None, opts: SlotOpts::default() });
        assert_eq!(m.routes[1], Route { name: "medium".into(), label: Some("Standard".into()), slot: m.slot_at("medium").unwrap(), vision: cf, deep: None });
        assert_eq!(m.routes[0].vision, m.slot_at("vision").unwrap(), "Private's images to the old config's vision slot");
        let (cheap, medium) = (Some(0), Some(1));
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        // each tier's call falls back within its route
        assert_eq!(m.fallback_of("cheap").map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        assert_eq!(m.fallback_of("medium").map(at), Some(("cloudflare".into(), DEEPSEEK_FLASH.into())));
        assert_eq!(m.fallback_of("high"), None, "no slot is named high");
        let mut private = Tries::new(m.route(CHEAP), m.fallback_of("cheap"));
        assert_eq!(private.after(Tried::Answered(429)).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        let mut standard = Tries::new(m.route(MEDIUM), m.fallback_of("medium"));
        assert_eq!(standard.after(Tried::Answered(502)).map(at), Some(("cloudflare".into(), DEEPSEEK_FLASH.into())), "Standard's to Cloudflare, never Tinfoil");
        // a caller's vision and fallback are its route's; one that says no tier, the four's
        assert_eq!(at(m.route_for(Named::Vision, cheap)), ("vision".into(), FINITE_PRIVATE_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Vision, medium)), ("cloudflare".into(), DEEPSEEK_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Vision, None)), ("vision".into(), FINITE_PRIVATE_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Fallback, medium)), ("cloudflare".into(), DEEPSEEK_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Fallback, None)), ("fallback".into(), "deepseek-v4-1-flash".into()));
        assert_eq!(m.capped_for(Named::Vision, medium).model.slot, "cloudflare");
        // a vision call falls back as its slot does: the four's vision to the fallback that reads images;
        // Standard's, an extra naming none, has none
        assert_eq!(m.fallback_of(m.route_for(Named::Vision, cheap).slot).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        let seen = m.route_for(Named::Vision, medium);
        assert_eq!(m.fallback_of(seen.slot), None, "Standard's vision, cloudflare, names no fallback");
        // an extra is named by its model's id, and listed
        assert_eq!(m.named(Some(DEEPSEEK_FLASH), None), Ok(slot(&m, "cloudflare")));
        assert_eq!(at(m.route(slot(&m, "cloudflare"))), ("cloudflare".into(), DEEPSEEK_FLASH.into()));
        // an id runs its model, whatever the caller's route; only the aliases follow it (#589's review, P2)
        for (id, slot) in [("deepseek-v4-1-flash", "fallback"), (DEEPSEEK_FLASH, "cloudflare")] {
            let named = m.named(Some(id), medium).unwrap();
            assert_eq!(at(m.route_for(named, medium)), (slot.to_string(), id.to_string()), "{id}, asked by a Standard caller");
            assert_eq!(at(m.route_for(m.named(Some(id), cheap).unwrap(), cheap)), (slot.to_string(), id.to_string()), "{id}, by a Private one");
        }
        let mut seeing = per_route();
        seeing["vision"] = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "gemma4-31b", "price": { "input": 0.4, "output": 1 }, "private": true });
        let seeing = parse(seeing).unwrap();
        let named = seeing.named(Some("gemma4-31b"), medium).unwrap();
        assert_eq!((named, at(seeing.route_for(named, medium))), (slot(&seeing, "vision"), ("vision".into(), "gemma4-31b".into())), "the vision model's own id, from a Standard caller: that model, not Standard's vision");
        for (alias, slot, id) in [("fallback", "cloudflare", DEEPSEEK_FLASH), ("vision", "cloudflare", DEEPSEEK_FLASH)] {
            let named = m.named(Some(alias), medium).unwrap();
            assert_eq!(at(m.route_for(named, medium)), (slot.to_string(), id.to_string()), "the alias {alias} follows Standard's route");
        }
        assert_eq!(m.ids(), [FINITE_PRIVATE_FLASH, OPENROUTER_FLASH, DEEPSEEK_FLASH, "deepseek-v4-1-flash"], "the routes' own first, then by slot");
        assert_eq!(m.name_of(slot(&m, "cloudflare")), "cloudflare", "a slot's call is named by its slot in its log line");
        // the view: each tier's route by its slots' ids
        let view = serde_json::to_value(m.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": DEEPSEEK_FLASH, "vision": DEEPSEEK_FLASH }));
        // the guard walks each route: Private's is private, Standard's its own
        assert_eq!(m.reached(1).iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), ["medium", "cloudflare", "fallback"]);
        assert!(m.reached(0).iter().all(|(slot, _)| m.slot_private(slot)));
        assert!(!m.slot_private("cloudflare") && m.slot_private("fallback"));
        // today's configs name no routes, and run as before
        assert_eq!((&openrouter().routes, openrouter().slots.len()), (&DEFAULTS.routes, 4));
        assert_eq!(openrouter().fallback_of("medium"), Some(openrouter().route(Named::Fallback)));
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert!(e.contains(says), "{models}: {e}");
        };
        let with = |edit: &dyn Fn(&mut Value)| {
            let mut v = per_route();
            edit(&mut v);
            v
        };
        refused(with(&|v| v["medium"]["fallback"] = json!("cloudflair")), "models.slots.medium: its fallback is \"cloudflair\", which is no other slot of the deployment's");
        refused(with(&|v| v["medium"]["vision"] = json!("nowhere")), "models.routes.medium: its vision is \"nowhere\", which is no slot of the deployment's");
        refused(with(&|v| v["medium"]["fallback"] = json!(7)), "models.slots.medium: fallback names the slot its calls fall back to, not 7");
        // a slot no route names is the deployment's to keep, run by its id; `high` is a name like any
        for name in ["spare", "high"] {
            let kept = parse(with(&|v| v[name] = json!({ "provider": "workers_ai", "model": INKLING_SMALL }))).unwrap();
            assert_eq!(kept.route(kept.named(Some(INKLING_SMALL), None).unwrap()).slot, name);
        }
        assert!(parse(with(&|v| v["medium"]["vision"] = json!("cheap"))).is_ok(), "a route's vision is any slot, its own route's included");
        refused(with(&|v| v["Cloud"] = json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH })), "models.slots: \"Cloud\" is no slot's name");
        refused(with(&|v| v["cloudflare"]["label"] = json!("Cloudflare")), "models.slots.cloudflare: unknown field `label`");
        refused(with(&|v| v["cloudflare"]["vision"] = json!("fallback")), "models.slots.cloudflare: unknown field `vision`");
        refused(with(&|v| v["cloudflare"]["images"] = json!("yes")), "models.slots.cloudflare: images is true (its model reads images) or false, not \"yes\"");
        refused(with(&|v| v["cloudflare"]["private"] = json!(true)), "models.slots.cloudflare: Workers AI and OpenRouter are not private");
        refused(with(&|v| v["cloudflare"]["provider"] = json!("anthropic")), "models.slots.cloudflare: unknown variant `anthropic`");
        refused(with(&|v| { for n in 1..=8 { v[format!("a{n}")] = json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH }); } }), &format!("models.slots: 1 to {SLOTS_MAX} slots, each where a model runs, not 13"));
        refused(with(&|v| v["cheap"]["fallback"] = json!("cloudflare")), "models.slots.cloudflare: the route cheap is private, and its agents' failures and stalls reach cloudflare");
        refused(with(&|v| v["cheap"]["vision"] = json!("cloudflare")), "models.slots.cloudflare: the route cheap is private, and its agents' images reach cloudflare");
        // P1 (#589's review): a private route of its own slots, the old config's fallback public. The
        // view's fallback is the default route's (`view_fallback`), private here, so an older image's
        // stall, its route unsaid, names a private model's id and runs it
        let own = with(&|v| {
            v["cheap"]["fallback"] = json!("private_fallback");
            v["cheap"]["vision"] = json!("private_vision");
            v["private_fallback"] = v["fallback"].clone();
            v["private_vision"] = v["vision"].clone();
            v["fallback"] = json!({ "provider": "workers_ai", "model": INKLING_SMALL });
            v["vision"] = json!({ "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3" });
        });
        let parsed = parse(own.clone()).unwrap();
        let stall = parsed.named(Some(&parsed.ids_by_name().fallback), None).unwrap();
        assert_eq!(parsed.route_for(stall, None).slot, "private_fallback", "an older image's stall: the default route's private fallback");
        // and a public default route's fallback is refused behind a private route
        let mut public_default = own.clone();
        public_default["cheap"].as_object_mut().unwrap().remove("private");
        public_default["medium"] = own["cheap"].clone();
        for slot in ["private_fallback", "private_vision"] {
            public_default[slot]["private"] = json!(true);
        }
        public_default["cheap"]["fallback"] = json!("fallback");
        refused(public_default, "models.slots.fallback: the route medium is private, and its agents' stalls on an older image (its route unsaid) reach fallback");
        let mut kept = own;
        kept["fallback"] = kept["private_fallback"].clone();
        kept["cheap"]["fallback"] = json!("fallback");
        kept.as_object_mut().unwrap().remove("private_fallback");
        let kept = parse(kept).unwrap();
        let stall = kept.named(Some("deepseek-v4-1-flash"), None).unwrap();
        assert!(kept.slot_private(kept.route_for(stall, None).slot), "an older image's stall, its tier unsaid: the built-in fallback, private");
        // P3: one endpoint, model and body, its price spelled two ways, is one call: no second try
        let tinfoil_slot = |price: Value| json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": price });
        let twice = parse(json!({ "cheap": tinfoil_slot(json!({ "input": 0.65, "output": 1.45 })), "fallback": tinfoil_slot(json!({ "input": 0.65, "cached_input": 0.65, "output": 1.45 })) })).unwrap();
        assert_ne!(up(&twice, "cheap"), up(&twice, "fallback"), "spelled two ways");
        assert_eq!(Tries::new(twice.route(CHEAP), twice.fallback_of("cheap")).after(Tried::Answered(429)), None, "the same call again is no fallback");
    }

    /// Goal (#589's review, P2): a call falls back as the slot it runs does,
    /// whether it named the slot by its name, an alias or its model's id;
    /// and any slot may name its own fallback (`"fallback"`), which the
    /// private guard follows. Valid: the four's `vision` named by its own id
    /// falls back as the alias does, to a fallback that reads images, and
    /// neither does when that fallback reads none; an extra naming its
    /// fallback, by its name and by its id; the four's `fallback` and
    /// `vision` naming theirs. Invalid: a fallback that is the slot itself,
    /// no slot, no name, or reads no images behind a slot images go to; a
    /// private tier whose slots' own fallbacks, or whose stall's id, reach a
    /// slot that is not private.
    #[test]
    fn each_slot_falls_back_as_itself_however_named() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        let gemma = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "gemma4-31b", "price": { "input": 0.4, "output": 1 } });
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let mut seeing = tinfoil(listed.clone());
        seeing["images"] = json!(true);
        // the four's vision on a model of its own, a fallback that reads images behind it
        let m = parse(json!({ "vision": gemma, "fallback": seeing })).unwrap();
        let by_id = m.named(Some("gemma4-31b"), None).unwrap();
        assert_eq!(by_id, slot(&m, "vision"), "its id names its slot");
        for named in [by_id, Named::Vision] {
            let model = m.route(named);
            assert_eq!(m.fallback_of(model.slot).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())), "{named:?}");
            let mut tries = Tries::new(model, m.fallback_of(model.slot));
            assert_eq!(tries.after(Tried::Answered(429)).map(|f| f.id()), Some("deepseek-v4-1-flash"), "{named:?}: a 429, once more on the fallback");
            assert_eq!(tries.after(Tried::Answered(429)), None, "and no third try");
        }
        // control: a fallback that reads no images is neither's
        let blind = parse(json!({ "vision": gemma, "fallback": tinfoil(listed.clone()) })).unwrap();
        for named in [blind.named(Some("gemma4-31b"), None).unwrap(), Named::Vision] {
            assert_eq!(blind.fallback_of(blind.route(named).slot), None, "{named:?}");
        }
        // the four's fallback has none, by its alias or its id; a tier's, its route's
        assert_eq!(m.fallback_of(m.route(Named::Fallback).slot), None);
        assert_eq!(m.fallback_of(m.route(m.named(Some("deepseek-v4-1-flash"), None).unwrap()).slot), None);
        assert_eq!(m.fallback_of("cheap").map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        // an extra names its own: Standard's vision, Cloudflare, behind it the fallback, which reads images
        let mut own = per_route();
        own["cloudflare"]["fallback"] = json!("fallback");
        let own = parse(own).unwrap();
        assert_eq!(own.fallback_of("cloudflare").map(|f| f.slot), Some("fallback"));
        for named in [Named::Vision, own.named(Some(DEEPSEEK_FLASH), None).unwrap()] {
            let model = own.route_for(named, Some(1));
            assert_eq!((model.slot, own.fallback_of(model.slot).map(at)), ("cloudflare", Some(("fallback".into(), "deepseek-v4-1-flash".into()))), "{named:?}");
        }
        assert_eq!(own.fallback_of("medium").map(at), Some(("cloudflare".into(), DEEPSEEK_FLASH.into())), "a tier's call is made once more on its route's fallback, and no third time");
        // the four's fallback and vision name theirs
        let named = parse(json!({ "fallback": { "provider": "workers_ai", "model": INKLING_SMALL, "fallback": "medium" }, "vision": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "fallback": "spare" }, "spare": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3", "images": true } })).unwrap();
        assert_eq!((named.fallback_of(super::FALLBACK).map(at), named.fallback_of(VISION).map(at)), (Some(("medium".into(), MEDIUM_MODEL.into())), Some(("spare".into(), MEDIUM_MODEL.into()))));
        assert_eq!((DEFAULTS.fallback_of(super::FALLBACK), DEFAULTS.fallback_of(VISION)), (None, None), "unnamed, as before");
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert!(e.contains(says), "{models}: {e}");
        };
        let with = |edit: &dyn Fn(&mut Value)| {
            let mut v = per_route();
            edit(&mut v);
            v
        };
        refused(with(&|v| v["cloudflare"]["fallback"] = json!("cloudflare")), "models.slots.cloudflare: its fallback is \"cloudflare\", which is no other slot of the deployment's");
        refused(with(&|v| v["vision"]["fallback"] = json!("nowhere")), "models.slots.vision: its fallback is \"nowhere\", which is no other slot of the deployment's");
        refused(with(&|v| v["fallback"]["fallback"] = json!(3)), "models.slots.fallback: fallback names the slot its calls fall back to, not 3");
        refused(with(&|v| v["cloudflare"]["fallback"] = json!("medium")), "models.slots.cloudflare: a route's images go to cloudflare, so its fallback reads them too: medium says no \"images\": true");
        refused(with(&|v| v["vision"]["fallback"] = json!("cheap")), "models.slots.vision: a route's images go to vision, so its fallback reads them too: cheap says no \"images\": true");
        // the private guard follows a slot's own fallback: Private's fallback, Tinfoil, behind it Cloudflare
        refused(with(&|v| v["fallback"]["fallback"] = json!("cloudflare")), "models.slots.cloudflare: the route cheap is private, and its agents' failures reach cloudflare");
        // and a stall's call, by its fallback's id with its route unsaid, to the first slot running that id
        // (by slots' order, their names'): here `vision`, which is not private, before `z_fallback`
        let stalled = json!({
            "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true, "fallback": "z_fallback", "vision": "private_vision" },
            "fallback": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true },
            "vision": tinfoil(listed.clone()),
            "z_fallback": { "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": listed, "private": true },
            "private_vision": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true }
        });
        refused(stalled, "models.slots.vision: the route cheap is private, and its agents' stalls on an older image (its route unsaid) reach vision");
    }

    /// Private and Standard as Fast and Deep (Paul, 2026-10-27): each tier's
    /// own slot its Fast (GLM-5.3 Flash at `high`), the slot its `"deep"`
    /// names its Deep (DeepSeek V4.1 Flash at `max`, 32,768 tokens a call);
    /// within Private the two fall back to each other, Standard's both to
    /// Cloudflare's DeepSeek V4.1 Flash.
    fn fast_and_deep() -> Value {
        let tinfoil = |extra: Value| {
            let mut slot = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": { "input": 0.65, "cached_input": 0.13, "output": 1.45 }, "images": true, "private": true });
            slot.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            slot
        };
        let deep_routing = json!({ "provider": { "zdr": true, "sort": "throughput", "require_parameters": true, "quantizations": ["fp8", "bf16"], "max_price": { "prompt": 0.3, "completion": 1.2 } } });
        json!({
            "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true, "label": "Private (recommended)",
                       "reasoning": "high", "deep": "private_deep", "fallback": "private_deep" },
            "private_deep": tinfoil(json!({ "reasoning": "max", "max_tokens": 32_768, "fallback": "cheap" })),
            "fallback": tinfoil(json!({})),
            "vision": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true },
            "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "label": "Standard",
                        "reasoning": "high", "deep": "standard_deep", "fallback": "cloudflare", "vision": "cloudflare" },
            "standard_deep": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": "deepseek/deepseek-v4.1-flash", "body": deep_routing,
                               "reasoning": "max", "max_tokens": 32_768, "fallback": "cloudflare" },
            "cloudflare": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "images": true }
        })
    }

    /// Goal (Paul, 2026-10-27: two modes, Fast and Deep, the same models
    /// and thinking in both tiers): a tier's own slot is its Fast and the
    /// slot its `"deep"` names its Deep, each with its effort
    /// (`"reasoning"`), its cap (`"max_tokens"`) and its own fallback; a
    /// caller on a tier naming its Deep's id runs its Deep, a caller that
    /// says none the first slot running it; the view carries each tier's
    /// modes; the private guard walks Deep, its fallback and its stall.
    /// Valid: Private and Standard as Fast and Deep; today's configs, no
    /// modes. Invalid: a deep that is no other slot or no name, an effort
    /// the route does not pass, an effort on a slot that is no mode, a cap
    /// out of 1 to `MAX_TOKENS_MAX`, a private tier whose Deep (or its
    /// fallback) is not private.
    #[test]
    fn each_tier_runs_fast_and_deep() {
        let book = PriceBook::defaults();
        let parse = |models: Value| old_parse(&models.to_string(), &book, false);
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        let m = parse(fast_and_deep()).unwrap();
        let (cheap, medium) = (Some(0), Some(1));
        // each tier's Deep, and the effort each mode thinks at
        assert_eq!((m.deep_of(0).map(at), m.deep_of(1).map(at)), (Some(("private_deep".into(), "deepseek-v4-1-flash".into())), Some(("standard_deep".into(), "deepseek/deepseek-v4.1-flash".into()))));
        assert_eq!((m.opts("cheap"), m.opts("private_deep")), (SlotOpts { reasoning: Some("high"), max_tokens: None }, SlotOpts { reasoning: Some("max"), max_tokens: Some(32_768) }));
        // a delegate on Private names its Deep's id: its Deep, at its cap, falling back to Private's Fast
        let named = m.named(Some("deepseek-v4-1-flash"), cheap).unwrap();
        let capped = m.capped_for(named, cheap);
        assert_eq!((at(capped.model), capped.max_tokens), (("private_deep".into(), "deepseek-v4-1-flash".into()), 32_768));
        assert_eq!(m.fallback_of(capped.model.slot).map(at), Some(("cheap".into(), FINITE_PRIVATE_FLASH.into())), "Deep falls back to Fast");
        assert_eq!(m.fallback_of("cheap").map(at), Some(("private_deep".into(), "deepseek-v4-1-flash".into())), "and Fast to Deep");
        let b = bound(capped, json!({ "model": "deepseek-v4-1-flash", "messages": [{ "role": "user", "content": "build it" }], "reasoning_effort": "max", "max_tokens": 60_000 }), true).unwrap();
        assert_eq!((b.input["reasoning_effort"].clone(), b.max_tokens), (json!("max"), 32_768), "max as sent, at Deep's cap");
        let mut tries = Tries::new(capped.model, m.fallback_of(capped.model.slot));
        assert_eq!(tries.after(Tried::Answered(503)).map(at), Some(("cheap".into(), FINITE_PRIVATE_FLASH.into())));
        // the same id, tier unsaid (an older image's stall): the first slot running it, the four's fallback
        assert_eq!(m.named(Some("deepseek-v4-1-flash"), None), Ok(slot(&m, "fallback")));
        // a delegate on Standard: its Deep on OpenRouter, falling back to Cloudflare
        let named = m.named(Some("deepseek/deepseek-v4.1-flash"), medium).unwrap();
        let capped = m.capped_for(named, medium);
        assert_eq!((at(capped.model), capped.max_tokens, m.fallback_of(capped.model.slot).map(at)), (("standard_deep".into(), "deepseek/deepseek-v4.1-flash".into()), 32_768, Some(("cloudflare".into(), DEEPSEEK_FLASH.into()))));
        // Fast's turns as before: the tier, at the cap of every call
        assert_eq!(m.capped_for(MEDIUM, medium).max_tokens, MAX_TOKENS);
        assert!(m.ids().contains(&"deepseek/deepseek-v4.1-flash"));
        // the view: each tier's modes
        let view = serde_json::to_value(m.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH, "reasoning": "high", "deep": { "id": "deepseek-v4-1-flash", "reasoning": "max", "fallback": FINITE_PRIVATE_FLASH } }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": DEEPSEEK_FLASH, "vision": DEEPSEEK_FLASH, "reasoning": "high", "deep": { "id": "deepseek/deepseek-v4.1-flash", "reasoning": "max", "fallback": DEEPSEEK_FLASH } }));
        // today's configs: no modes, nothing new in the view
        assert_eq!((DEFAULTS.deep_of(0), DEFAULTS.opts("cheap")), (None, SlotOpts::default()));
        let plain = serde_json::to_value(DEFAULTS.ids_by_name()).unwrap();
        assert!(plain["routes"]["cheap"].get("deep").is_none() && plain["routes"]["cheap"].get("reasoning").is_none());
        // the guard walks Private's Deep
        assert!(m.reached(0).iter().any(|(slot, _)| *slot == "private_deep"), "its Deep, which is its fallback too");
        assert!(m.reached(0).iter().all(|(slot, _)| m.slot_private(slot)));
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert!(e.contains(says), "{models}: {e}");
        };
        let with = |edit: &dyn Fn(&mut Value)| {
            let mut v = fast_and_deep();
            edit(&mut v);
            v
        };
        refused(with(&|v| v["medium"]["deep"] = json!("nowhere")), "models.routes.medium: its deep is \"nowhere\", which is no slot of the deployment's");
        refused(with(&|v| v["medium"]["deep"] = json!("medium")), "models.routes.medium: its deep is its own slot");
        refused(with(&|v| v["medium"]["deep"] = json!(true)), "models.routes.medium: deep names a slot, not true");
        refused(with(&|v| v["cheap"]["reasoning"] = json!("medium")), "models.slots.cheap: reasoning is low, high, max, not \"medium\"");
        refused(with(&|v| v["vision"]["reasoning"] = json!("high")), "models.slots.vision: reasoning is a mode's, a route's own slot's or its deep's");
        refused(with(&|v| v["cloudflare"]["reasoning"] = json!("max")), "models.slots.cloudflare: reasoning is a mode's");
        for bad in [json!(0), json!(MAX_TOKENS_MAX + 1), json!("lots"), json!(-1)] {
            refused(with(&|v| v["standard_deep"]["max_tokens"] = bad.clone()), &format!("models.slots.standard_deep: max_tokens is the most one call of it writes, 1 to {MAX_TOKENS_MAX}, not {bad}"));
        }
        refused(with(&|v| v["private_deep"]["private"] = json!(false)), "models.slots.private_deep: the route cheap is private, and its agents' failures and stalls reach private_deep");
        refused(with(&|v| v["private_deep"]["fallback"] = json!("cloudflare")), "the route cheap is private, and its agents' stalls (by its model's id) reach cloudflare");
        refused(
            with(&|v| {
                v["cheap"]["deep"] = json!("standard_deep");
                v["private_deep"].as_object_mut().unwrap().remove("reasoning");
            }),
            "models.slots.standard_deep: the route cheap is private, and its agents' delegates reach standard_deep",
        );
        assert!(parse(with(&|v| v["cheap"]["max_tokens"] = json!(MAX_TOKENS_MAX))).is_ok(), "a cap of {MAX_TOKENS_MAX} at most, on any slot");
    }

    /// Three routes as a deployment lists them (Paul, 2026-11-17): Private
    /// and Standard as production runs them (`fast_and_deep`'s slots), and
    /// American on Workers AI's Inkling 2 Small, its images to itself, its
    /// Deep Inkling at `max`, both falling back to Inkling through
    /// OpenRouter, on its US providers alone, at a ceiling.
    fn three_routes() -> Value {
        let mut slots = legacy(&fast_and_deep()).unwrap()["slots"].clone();
        let or_inkling = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": "thinkingmachines/inkling-2-small", "images": true,
                                 "body": { "provider": { "zdr": true, "only": ["primeintellect", "deepinfra", "modal", "together"], "max_price": { "prompt": 0.3, "completion": 1 } } } });
        slots["inkling"] = json!({ "provider": "workers_ai", "model": INKLING_SMALL, "images": true, "reasoning": "high", "fallback": "inkling_or" });
        slots["inkling_deep"] = json!({ "provider": "workers_ai", "model": INKLING_SMALL, "images": true, "reasoning": "max", "max_tokens": 32_768, "fallback": "inkling_or" });
        slots["inkling_or"] = or_inkling;
        for old in ["vision", "fallback"] {
            slots.as_object_mut().unwrap().remove(old);
        }
        let mut routes = legacy(&fast_and_deep()).unwrap()["routes"].clone();
        routes[0]["vision"] = json!("cheap");
        routes.as_array_mut().unwrap().push(json!({ "name": "american", "label": "American", "slot": "inkling", "deep": "inkling_deep" }));
        json!({ "default": "cheap", "routes": routes, "slots": slots })
    }

    /// Goal: a deployment lists the routes people pick, any number up to
    /// `ROUTES_MAX` under any names, and each does what its config says and
    /// nothing its name says (Paul, 2026-11-17): a third route is offered
    /// in its order, with its own fallback, vision and Deep; an agent on it
    /// runs its model, its calls priced as that model's row (Inkling's on
    /// Workers AI, its fallback's at its ceiling); a call saying it falls
    /// back within it. The view's two routes stay what an older image
    /// reads. Method: `three_routes`, and the route named by its name, by
    /// its model's id with it said, and the aliases.
    #[test]
    fn a_deployment_lists_its_routes() {
        let book = PriceBook::defaults();
        let m = Models::parse(Some(&three_routes().to_string()), &book, false).unwrap();
        let names: Vec<(&str, Option<&str>)> = m.routes.iter().map(|r| (r.name.as_str(), r.label.as_deref())).collect();
        assert_eq!(names, [("cheap", Some("Private (recommended)")), ("medium", Some("Standard")), ("american", Some("American"))], "in the config's order");
        let american = m.route_named("american").unwrap();
        assert_eq!((american, m.default, m.step_route(Some("american"))), (2, 0, Ok(2)));
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        // an agent on it: its turns on Inkling, at the route's cap, falling back to Inkling elsewhere
        let named = m.named(Some("american"), None).unwrap();
        let turn = m.capped_for(named, Some(american));
        assert_eq!((at(turn.model), turn.max_tokens, m.name_of(named)), (("inkling".into(), INKLING_SMALL.into()), MAX_TOKENS, "american"));
        assert_eq!(m.fallback_of(turn.model.slot).map(at), Some(("inkling_or".into(), "thinkingmachines/inkling-2-small".into())));
        // its runtime names its model by its id, saying its route: the route's own slot, not the Deep's
        assert_eq!(m.named(Some(INKLING_SMALL), Some(american)), Ok(Named::Route(2)));
        // its images, its Deep and its aliases are its own
        assert_eq!(at(m.route_for(Named::Vision, Some(american))), ("inkling".into(), INKLING_SMALL.into()), "Inkling reads its own images");
        assert_eq!(at(m.route_for(Named::Fallback, Some(american))), ("inkling_or".into(), "thinkingmachines/inkling-2-small".into()));
        let deep = m.deep_of(american).unwrap();
        assert_eq!((at(deep), m.opts(deep.slot)), (("inkling_deep".into(), INKLING_SMALL.into()), SlotOpts { reasoning: Some("max"), max_tokens: Some(32_768) }));
        assert_eq!(m.fallback_for_input(deep.slot, &json!({ "messages": [{ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:," } }] }] })).map(at), Some(("inkling_or".into(), "thinkingmachines/inkling-2-small".into())), "an image falls back to a model that reads it");
        // its calls priced as Inkling's row; its fallback's at its ceiling, every input token at it
        let priced = m.priced(book.clone());
        let row = |id: &str| priced.models.iter().find(|r| r.model == id).map(|r| r.price);
        assert_eq!(row(INKLING_SMALL), Some(TokenPrices { input: 300_000, cached_input: 100_000, cache_write: 300_000, output: 1_000_000 }), "the book's row: Workers AI's list");
        let ceiling = Some(TokenPrices { input: crate::price::openrouter(300_000), cached_input: crate::price::openrouter(300_000), cache_write: crate::price::openrouter(300_000), output: crate::price::openrouter(1_000_000) });
        assert_eq!(row("thinkingmachines/inkling-2-small"), ceiling, "cached input at the ceiling's prompt price on a fallback");
        // the American route promises no privacy and reaches only its own models
        let reached: Vec<&str> = m.reached(american).into_iter().map(|(s, _)| s).collect();
        assert!(!m.slots[m.routes[american].slot].private);
        assert_eq!(reached, ["inkling", "inkling_or", "inkling_deep", "private_deep", "cheap"], "Inkling, its fallback, its Deep; and an older image's stall, which names the view's fallback (Private's) and falls back as it does");
        // an id two routes run is the caller's route's; a caller on another route runs the first that does
        assert_eq!(m.named(Some(INKLING_SMALL), Some(0)), Ok(Named::Route(2)));
        // the view: the two routes an older image reads, as they were
        let view = serde_json::to_value(m.ids_by_name()).unwrap();
        assert_eq!((view["cheap"].clone(), view["medium"].clone()), (json!(FINITE_PRIVATE_FLASH), json!(OPENROUTER_FLASH)));
        assert!(view["routes"].get("american").is_none(), "the list is the next PR's: the view is unchanged");
        // a list of routes passes `legacy` as it is
        assert_eq!(legacy(&three_routes()), Ok(three_routes()));
    }

    /// Invalid: a deployment's routes out of shape, each refused saying why:
    /// none, past `ROUTES_MAX`, one named twice, named as an alias or as no
    /// route may be, naming a slot that is not there, a Deep that is its own
    /// slot, a key a route does not take, a default that is no route's, a
    /// key `models` does not take, no slots.
    #[test]
    fn a_deployments_routes_out_of_shape_are_refused() {
        let book = PriceBook::defaults();
        let refused = |edit: &dyn Fn(&mut Value), says: &str| {
            let mut v = three_routes();
            edit(&mut v);
            let e = Models::parse(Some(&v.to_string()), &book, false).unwrap_err();
            assert_eq!(e, says, "{v}");
        };
        refused(&|v| v["routes"] = json!([]), "models.routes: 1 to 8 routes, the ones people pick, not 0");
        refused(&|v| { let r = v["routes"][2].clone(); for n in 0..6 { let mut x = r.clone(); x["name"] = json!(format!("more{n}")); v["routes"].as_array_mut().unwrap().push(x); } }, &format!("models.routes: 1 to {ROUTES_MAX} routes, the ones people pick, not 9"));
        refused(&|v| v["routes"][2]["name"] = json!("cheap"), "models.routes.cheap: a route is named once");
        for bad in ["vision", "fallback", "Cheap", "", "two words"] {
            refused(&|v| v["routes"][2]["name"] = json!(bad), &format!("models.routes: a route's name is 1 to 24 of a-z, 0-9 and _, a letter first, and neither vision nor fallback, not \"{bad}\""));
        }
        refused(&|v| v["routes"][2]["slot"] = json!("nowhere"), "models.routes.american: its slot is \"nowhere\", which is no slot of the deployment's");
        refused(&|v| { v["routes"][2].as_object_mut().unwrap().remove("slot"); }, "models.routes.american: slot names the slot its turns run on");
        refused(&|v| v["routes"][2]["deep"] = json!("inkling"), "models.routes.american: its deep is its own slot: name another, or none");
        refused(&|v| v["routes"][2]["private"] = json!(true), "models.routes.american: private is none of name, label, slot, vision, deep");
        refused(&|v| v["routes"][2] = json!("american"), "models.routes: each is {name, label?, slot, vision?, deep?}");
        refused(&|v| v["default"] = json!("american_typo"), "models.default is \"american_typo\", which is none of its routes");
        refused(&|v| { v.as_object_mut().unwrap().remove("default"); }, "models.default names the route agents run on unless they name another, not null");
        refused(&|v| v["tiers"] = json!({}), "models: tiers is none of default, routes, slots, embeddings");
        refused(&|v| v["slots"] = json!({}), &format!("models.slots: 1 to {SLOTS_MAX} slots, each where a model runs, not 0"));
        // a private route reaching the American slots is refused: privacy is a slot's, never a name's
        refused(&|v| v["slots"]["cheap"]["fallback"] = json!("inkling"), "models.slots.inkling: the route cheap is private, and its agents' failures and stalls reach inkling, so mark a private model there (\"private\": true)");
    }

    /// Goal: one rule names the agent a guest's model call is for, chat and
    /// transcription alike: the header, or a key `agent:<name>` from a
    /// client that sends no header of its own (OpenAI's SDKs); any other
    /// key is the guest's placeholder. Invalid: none, a malformed name
    /// either way, and two that disagree are refused.
    #[test]
    fn the_header_or_the_key_names_the_agent() {
        let named = |h: Option<&str>, a: Option<&str>| agent_named(h, a);
        assert_eq!(named(Some("juniper--k3x9"), None), Ok("juniper--k3x9".into()));
        assert_eq!(named(Some("juniper--k3x9"), Some("Bearer fragment-model")), Ok("juniper--k3x9".into()), "Hermes' chat calls: the header, its key a placeholder");
        assert_eq!(named(None, Some("Bearer agent:juniper--k3x9")), Ok("juniper--k3x9".into()), "an OpenAI SDK's call: the key");
        assert_eq!(named(None, Some("bearer   agent:juniper--k3x9 ")), Ok("juniper--k3x9".into()));
        assert_eq!(named(Some("juniper--k3x9"), Some("Bearer agent:juniper--k3x9")), Ok("juniper--k3x9".into()), "both, agreeing");
        assert_eq!(named(None, None), Err(Unnamed::Missing));
        assert_eq!(named(None, Some("Bearer fragment-model")), Err(Unnamed::Missing), "a placeholder names no one");
        assert_eq!(named(None, Some("Basic agent:juniper--k3x9")), Err(Unnamed::Missing), "only a bearer key names");
        for bad in ["Bearer agent:", "Bearer agent:juniper", "Bearer agent:Juniper--K3X9", "Bearer agent:a b--k3x9", "Bearer agent:juniper--k3x9--x"] {
            assert_eq!(named(None, Some(bad)), Err(Unnamed::Malformed), "{bad}");
        }
        assert_eq!(named(Some("not a name"), None), Err(Unnamed::Malformed));
        assert_eq!(named(Some("juniper--k3x9"), Some("Bearer agent:willow--k3x9")), Err(Unnamed::Disagree));
        assert_eq!(named(Some("juniper--k3x9"), Some("Bearer agent:nope")), Err(Unnamed::Malformed), "a malformed key is refused whatever the header says");
        assert!(Unnamed::Missing.message().contains("agent:<label>--<suffix>"));
    }

    /// Goal: the deployment's vision model is DeepSeek V4.1 Flash unless it names
    /// another, and one the price book does not price is refused (a ledger
    /// would charge none of its calls). Method: the default, a priced
    /// model named, and unpriced ones.
    #[test]
    fn the_vision_model_is_one_the_book_prices() {
        let book = PriceBook::defaults();
        let vision = |text: &str| old_parse(text, &book, false).map(|m| up(&m, "vision").model().to_string());
        assert_eq!(up(&Models::defaults(), "vision"), &workers_ai(DEEPSEEK_FLASH));
        assert_eq!(VISION_MODEL_DEFAULT, "@cf/deepseek-ai/deepseek-v4.1-flash-1017");
        assert_ne!(VISION_MODEL_DEFAULT, FALLBACK_MODEL_DEFAULT, "vision and the fallback, two models");
        assert_eq!(vision(&json!({ "vision": { "provider": "workers_ai", "model": MEDIUM_MODEL } }).to_string()).as_deref(), Ok(MEDIUM_MODEL));
        let unpriced = vision(&json!({ "vision": { "provider": "workers_ai", "model": "@cf/meta/llama-4-scout-17b-16e-instruct" } }).to_string()).unwrap_err();
        assert!(unpriced.contains("models.slots.vision") && unpriced.contains("not in the price book") && unpriced.contains(DEEPSEEK_FLASH), "{unpriced}");
        assert_eq!(up(&finite(), "vision"), &workers_ai(DEEPSEEK_FLASH), "Finite's deployment keeps vision on Workers AI (decision 61)");
        // priced at DeepSeek V4.1 Flash's prices, as Workers AI lists them
        let flash = book.models.iter().find(|m| m.model == VISION_MODEL_DEFAULT).unwrap();
        assert_eq!((flash.price.input, flash.price.cached_input, flash.price.output), (300_000, 6_000, 1_200_000));
        let mut other = book.clone();
        other.models.retain(|m| m.model != VISION_MODEL_DEFAULT);
        assert!(Models::parse(None, &other, false).is_err(), "the default too, were it unpriced");
    }

    /// Goal: Hermes' vision call fits the route at the size Hermes shrinks
    /// an image to after a 413 (its `_RESIZE_TARGET_BYTES`), so its one
    /// retry is answered. Method: the body of such a call, with a long
    /// prompt about a screen, against the cap.
    #[test]
    fn hermes_shrunk_image_fits_a_call() {
        let url = format!("data:image/jpeg;base64,{}", "A".repeat(IMAGE_DATA_URL_MAX_BYTES - "data:image/jpeg;base64,".len()));
        assert_eq!(url.len(), IMAGE_DATA_URL_MAX_BYTES);
        let prompt = "  [12] AXButton 'Save' (100, 200, 80, 24)\n".repeat(2_000);
        let body = json!({
            "model": "vision", "max_tokens": 4096, "temperature": 0.1,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": prompt }, { "type": "image_url", "image_url": { "url": url } }] }],
        });
        let bytes = serde_json::to_vec(&body).unwrap().len();
        assert!(bytes > 5 * 1024 * 1024 && bytes <= MODEL_BODY_MAX_BYTES, "{bytes} bytes against {MODEL_BODY_MAX_BYTES}");
    }

    /// Goal: a route picks its model; a job's step naming no route of the
    /// deployment's (`high` among them, a model's id) is refused naming the
    /// routes (Paul, 2026-11-17). Method: every route of the default
    /// config, and names that are not.
    #[test]
    fn routes_pick_models() {
        assert_eq!(tier(CHEAP).model.id(), "@cf/deepseek-ai/deepseek-v4.1-flash-1017", "DeepSeek V4.1 Flash, Inkling its fallback, since GLM-5.3 Flash's retirement on 2026-11-20");
        assert_eq!(tier(CHEAP).model.id(), CHEAP_MODEL);
        assert_eq!(tier(MEDIUM).model.id(), MEDIUM_MODEL);
        assert_eq!((DEFAULTS.step_route(None), DEFAULTS.step_route(Some("medium"))), (Ok(0), Ok(1)), "none named: the default");
        for bad in ["high", "@cf/zai-org/glm-5.3", "z-ai/glm-5.3-flashx", "Cheap", ""] {
            assert_eq!(DEFAULTS.step_route(Some(bad)), Err(UnknownRoute { allowed: vec!["cheap".into(), "medium".into()] }), "{bad:?}");
        }
        assert_eq!(DEFAULTS.unknown_route().message(), "one of this deployment's routes: cheap, medium (a model id is never one)");
        // the book prices every model a route runs
        let book = crate::price::PriceBook::defaults();
        for model in [CHEAP_MODEL, MEDIUM_MODEL] {
            assert!(book.models.iter().any(|m| m.model == model), "{model}");
        }
    }

    /// Goal: what reaches the vendor is the client's request bounded (lesson
    /// 7). Method: a request with a model, a large max_tokens in both names,
    /// GLM's dearest effort, and a client's own stream options.
    #[test]
    fn a_request_is_bounded() {
        let body = json!({
            "model": "medium", "messages": [{ "role": "user", "content": "hi" }], "tools": [{ "type": "function" }],
            "max_tokens": 100_000, "max_completion_tokens": 50_000, "reasoning_effort": "medium", "stream_options": { "x": 1 },
        });
        let b = bound(tier(MEDIUM), body, false).unwrap();
        assert_eq!((b.model.id(), b.max_tokens, b.stream), (MEDIUM_MODEL, MAX_TOKENS, false));
        assert_eq!(
            b.input,
            json!({
                "messages": [{ "role": "user", "content": "hi" }], "tools": [{ "type": "function" }],
                "max_tokens": MAX_TOKENS, "reasoning_effort": "low", "stream": false,
            }),
            "no model, the cap, the effort clamped, no stream options when not streaming"
        );
        let b = bound(tier(CHEAP), json!({ "messages": [{}], "max_tokens": 64, "reasoning_effort": "high" }), true).unwrap();
        assert_eq!((b.model.id(), b.max_tokens), (CHEAP_MODEL, 64), "what it asked for, under the cap");
        assert_eq!(b.input["reasoning_effort"], "high");
        assert_eq!((b.input["stream"].clone(), b.input["stream_options"].clone()), (json!(true), json!({ "include_usage": true })));
        let b = bound(tier(CHEAP), json!({ "messages": [{}], "max_completion_tokens": 32 }), false).unwrap();
        assert_eq!((b.max_tokens, b.input.get("max_completion_tokens")), (32, None), "either name, sent as max_tokens");
        assert_eq!(bound(tier(CHEAP), json!({ "messages": [{}] }), false).unwrap().max_tokens, MAX_TOKENS, "none asked: the cap");
    }

    /// Goal (Paul, 2026-10-22): how hard an agent thinks, as its owner
    /// picks it (agent.json's `reasoning`, which its Hermes sends as each
    /// call's `reasoning_effort`), reaches its model as picked, on Workers
    /// AI and on an OpenAI-compatible API alike; any other effort, Hermes'
    /// own default (`medium`) among them, is GLM's cheapest, `low`.
    #[test]
    fn an_agents_reasoning_reaches_its_model() {
        let routed = openrouter();
        let call = |effort: Option<&str>| {
            let mut body = json!({ "model": "cheap", "messages": [{ "role": "user", "content": "hi" }], "stream": true });
            if let Some(e) = effort {
                body["reasoning_effort"] = json!(e);
            }
            body
        };
        for (asked, sent) in [(Some("low"), "low"), (Some("high"), "high"), (Some("max"), "max"), (None, "low"), (Some("medium"), "low"), (Some("xhigh"), "low"), (Some("none"), "low"), (Some("MAX"), "low")] {
            let workers_ai = bound(tier(CHEAP), call(asked), true).unwrap();
            assert_eq!(workers_ai.input["reasoning_effort"], sent, "Workers AI, asked {asked:?}");
            let capped = routed.capped(CHEAP);
            let Upstream::Openai { model, body, .. } = capped.model.upstream.clone() else { panic!("OpenRouter's tier is an OpenAI-compatible API's") };
            let b = bound(capped, call(asked), true).unwrap();
            assert_eq!(sent_json(&b.input, &model, body.as_ref())["reasoning_effort"], sent, "OpenRouter, asked {asked:?}");
        }
    }

    #[test]
    fn a_request_out_of_shape_is_refused() {
        let msgs = || json!([{ "role": "user", "content": "hi" }]);
        assert_eq!(bound(tier(CHEAP), json!([1]), false), Err(Refusal::NotAnObject));
        assert_eq!(bound(tier(CHEAP), json!({}), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": [] }), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": "hi" }), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": msgs(), "max_tokens": 0 }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": msgs(), "max_tokens": "9" }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": msgs(), "max_completion_tokens": -1 }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(CHEAP), json!({ "messages": msgs(), "stream": "yes" }), false), Err(Refusal::Stream));
    }

    /// Goal: usage reads as the ledger meters it (S4's shapes). Method: an
    /// uncached call, a cached one, an Anthropic-style cache write, and
    /// usages that do not read.
    #[test]
    fn usage_reads_as_the_ledger_meters_it() {
        let u = |v: Value| usage_of(MEDIUM_MODEL, &v);
        let tokens = |input, cached_input, cache_write, output| Some(Usage::Tokens { model: MEDIUM_MODEL.into(), input, cached_input, cache_write, output });
        assert_eq!(
            u(json!({ "prompt_tokens": 187, "completion_tokens": 12, "total_tokens": 199, "prompt_tokens_details": { "cached_tokens": 0 }, "neurons": 3.09 })),
            tokens(187, 0, 0, 12)
        );
        assert_eq!(u(json!({ "prompt_tokens": 187, "completion_tokens": 12, "prompt_tokens_details": { "cached_tokens": 128 } })), tokens(59, 128, 0, 12));
        assert_eq!(u(json!({ "prompt_tokens": 100, "completion_tokens": 5, "cache_creation_input_tokens": 40 })), tokens(60, 0, 40, 5));
        assert_eq!(u(json!({ "prompt_tokens": 100, "completion_tokens": 5, "prompt_tokens_details": { "cache_write_tokens": 30 } })), tokens(70, 0, 30, 5));
        assert_eq!(u(json!({ "prompt_tokens": 10 })), None, "no completion count");
        assert_eq!(u(json!({ "prompt_tokens": -1, "completion_tokens": 1 })), None);
        assert_eq!(u(json!({ "prompt_tokens": 10, "completion_tokens": 1, "prompt_tokens_details": { "cached_tokens": 11 } })), None, "more cached than prompted");
        assert_eq!(u(json!({ "prompt_tokens": 10, "completion_tokens": 1, "prompt_tokens_details": { "cached_tokens": "x" } })), None);
    }

    /// S4's streamed answer (logs/raw-sse-cheap.txt), its per-chunk usage
    /// deltas and Workers AI's last line.
    const S4_STREAM: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"\",\"role\":\"assistant\"},\"finish_reason\":null,\"index\":0}],\"id\":\"153\",\"object\":\"chat.completion.chunk\",\"usage\":{\"prompt_tokens\":23,\"completion_tokens\":0,\"total_tokens\":23}}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"1, 2, 3,\"},\"finish_reason\":null,\"index\":0}],\"id\":\"153\",\"object\":\"chat.completion.chunk\",\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":9,\"total_tokens\":9}}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"index\":0}],\"id\":\"153\",\"object\":\"chat.completion.chunk\",\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":6,\"total_tokens\":6}}\n\n",
        "data: {\"choices\":[],\"id\":\"153\",\"object\":\"chat.completion.chunk\",\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0,\"total_tokens\":0}}\n\n",
        "data: {\"response\":\"\",\"usage\":{\"prompt_tokens\":23,\"completion_tokens\":15,\"total_tokens\":38,\"prompt_tokens_details\":{\"cached_tokens\":0}}}\n\n",
        "data: [DONE]\n\n",
    );

    fn data_lines(out: &[u8]) -> Vec<Value> {
        String::from_utf8(out.to_vec())
            .unwrap()
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|d| *d != "[DONE]")
            .map(|d| serde_json::from_str(d).unwrap())
            .collect()
    }

    /// Goal: only the last, cumulative usage is metered, and the client
    /// reads OpenAI's shape (S4's two quirks). Method: S4's stream, fed in
    /// pieces that split lines, and fed whole: the same either way.
    #[test]
    fn a_stream_meters_its_last_cumulative_usage() {
        for piece in [1, 7, 64, S4_STREAM.len()] {
            let mut s = Stream::default();
            let mut out = Vec::new();
            for chunk in S4_STREAM.as_bytes().chunks(piece) {
                s.push(chunk, Some(&mut out));
            }
            s.finish(Some(&mut out));
            let usage = s.usage().cloned().unwrap();
            assert_eq!(usage_of(CHEAP_MODEL, &usage), Some(Usage::Tokens { model: CHEAP_MODEL.into(), input: 23, cached_input: 0, cache_write: 0, output: 15 }), "pieces of {piece}");
            let lines = data_lines(&out);
            assert_eq!(lines.len(), 5);
            assert!(lines[..3].iter().all(|l| l.get("usage").is_none() && l["choices"].as_array().is_some_and(|c| !c.is_empty())), "no deltas reach the client");
            assert_eq!(lines[4], json!({ "id": "153", "object": "chat.completion.chunk", "choices": [], "usage": usage }), "Workers AI's last line, as OpenAI's");
            assert!(String::from_utf8(out.clone()).unwrap().ends_with("data: [DONE]\n\n"));
        }
        // metering only, with no client: the same usage
        let mut s = Stream::default();
        s.push(S4_STREAM.as_bytes(), None);
        s.finish(None);
        assert_eq!(s.usage().and_then(|u| u["completion_tokens"].as_u64()), Some(15));
    }

    /// Goal: a reader stops at `[DONE]` with the answer and its usage whole
    /// (the gateway held a stream open ~150 s after it, 2026-10-07), and
    /// what follows it is none of the answer's: not metered, not sent.
    /// Method: S4's stream, then bytes after its `[DONE]` that would change
    /// its usage and its text, fed in pieces.
    #[test]
    fn a_stream_is_done_at_its_done_line() {
        let after = "data: {\"response\":\"\",\"usage\":{\"prompt_tokens\":9999,\"completion_tokens\":9999}}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n";
        let whole = format!("{S4_STREAM}{after}");
        for piece in [1, 9, whole.len()] {
            let mut s = Stream::answering();
            let mut out = Vec::new();
            // a reader stops at done; one that feeds on reads nothing more
            for chunk in whole.as_bytes().chunks(piece) {
                s.push(chunk, Some(&mut out));
            }
            s.finish(Some(&mut out));
            assert!(s.done() && s.began(), "pieces of {piece}");
            assert_eq!(s.usage().and_then(|u| u["completion_tokens"].as_u64()), Some(15), "the usage before [DONE]");
            assert_eq!(s.answer().map(|a| (a.content.as_str(), a.finish_reason.as_deref())), Some(("1, 2, 3,", Some("stop"))));
            let text = String::from_utf8(out).unwrap();
            assert!(text.ends_with("data: [DONE]\n\n") && !text.contains("late") && !text.contains("9999"), "the client's stream ends at [DONE]: {text}");
        }
        let (body, _) = S4_STREAM.split_at(S4_STREAM.len() - "data: [DONE]\n\n".len());
        let mut s = Stream::default();
        assert!(!s.began());
        s.push(body.as_bytes(), None);
        assert!(s.began() && !s.done(), "not done before [DONE]");
        // a [DONE] with no blank line after it still ends its event for the client
        let mut out = Vec::new();
        s.push(b"data: [DONE]", Some(&mut out));
        s.finish(Some(&mut out));
        assert_eq!((s.done(), out.as_slice()), (true, b"data: [DONE]\n\n".as_slice()));
    }

    fn text(args: Value) -> AiText {
        match crate::steps::Step::from_parts("ai.text", args) {
            Ok(crate::steps::Step::AiText(t)) => t,
            other => panic!("{other:?}"),
        }
    }

    fn lookup(name: &str) -> Value {
        json!({ "type": "function", "function": { "name": name, "description": "Looks a word up.", "parameters": { "type": "object", "properties": { "word": { "type": "string" } } } } })
    }

    /// Goal: a text step asks for what it named, its tools and tool choice
    /// as OpenAI's, and a conversation carrying an assistant's tool calls
    /// and their results reaches the model as it came. Method: a prompt, a
    /// tool turn's messages, through `bound`.
    #[test]
    fn a_text_step_asks_for_what_it_named() {
        let body = text_body(&text(json!({ "prompt": "hi", "max_tokens": 10 }))).unwrap();
        assert_eq!(body, json!({ "messages": [{ "role": "user", "content": "hi" }], "max_tokens": 10 }));
        let messages = json!([
            { "role": "system", "content": "be brief" },
            { "role": "user", "content": "what is a shard?" },
            { "role": "assistant", "content": "", "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "lookup", "arguments": "{\"word\":\"shard\"}" } }] },
            { "role": "tool", "tool_call_id": "call_1", "content": "a small fragment" },
        ]);
        let t = text(json!({ "messages": messages, "tools": [lookup("lookup")], "tool_choice": "auto", "reasoning_effort": "high", "draft": { "channel": "log", "turn": "turn:t_0123456789abcdef" } }));
        let b = bound(tier(MEDIUM), text_body(&t).unwrap(), true).unwrap();
        assert_eq!(b.input["messages"], messages, "tool calls and their results pass through");
        assert_eq!((b.input["tools"].clone(), b.input["tool_choice"].clone()), (json!([lookup("lookup")]), json!("auto")));
        assert_eq!((b.input["stream"].clone(), b.input["reasoning_effort"].clone()), (json!(true), json!("high")));
        assert!(b.input.get("draft").is_none(), "a draft is the platform's, never the model's");
        let named = text(json!({ "prompt": "p", "tools": [lookup("lookup")], "tool_choice": { "type": "function", "function": { "name": "lookup" } } }));
        assert_eq!(text_body(&named).unwrap()["tool_choice"], json!({ "type": "function", "function": { "name": "lookup" } }));
        assert!(text_body(&text(json!({ "prompt": "p", "tools": [] }))).unwrap().get("tools").is_none(), "no tools is none sent");
        let bare = json!({ "type": "function", "function": { "name": "now" } });
        assert_eq!(text_body(&text(json!({ "prompt": "p", "tools": [bare] }))).unwrap()["tools"][0], bare, "a function of no arguments names no schema");
    }

    /// Goal: a text step's tools, tool choice, draft and size are bounded,
    /// and refused typed past them, before anything is sent. Method:
    /// each bound at and past its edge.
    #[test]
    fn a_text_steps_tools_and_draft_are_bounded() {
        let check = |args: Value| text_body(&text(args));
        assert_eq!(check(json!({})), Err(Refusal::NoMessages));
        let tools = |n: usize| Value::Array((0..n).map(|i| lookup(&format!("t{i}"))).collect());
        assert!(check(json!({ "prompt": "p", "tools": tools(TOOLS_MAX) })).is_ok());
        assert_eq!(check(json!({ "prompt": "p", "tools": tools(TOOLS_MAX + 1) })), Err(Refusal::Tools));
        let long = json!({ "type": "function", "function": { "name": "big", "description": "d".repeat(TOOLS_MAX_BYTES) } });
        assert_eq!(check(json!({ "prompt": "p", "tools": [long] })), Err(Refusal::Tools), "past the bytes");
        assert_eq!(check(json!({ "prompt": "p", "tools": [lookup("a"), lookup("a")] })), Err(Refusal::Tools), "a name twice");
        assert_eq!(check(json!({ "prompt": "p", "tools": [lookup("look up")] })), Err(Refusal::Tools));
        assert_eq!(check(json!({ "prompt": "p", "tools": [lookup(&"x".repeat(TOOL_NAME_MAX_BYTES + 1))] })), Err(Refusal::Tools));
        assert!(check(json!({ "prompt": "p", "tools": [lookup(&"x".repeat(TOOL_NAME_MAX_BYTES))] })).is_ok());
        for schema in [json!("a word"), json!({ "type": "string" }), json!({ "properties": {} }), json!([])] {
            let bad = json!({ "type": "function", "function": { "name": "f", "parameters": schema } });
            assert_eq!(check(json!({ "prompt": "p", "tools": [bad] })), Err(Refusal::ToolSchema), "{schema}");
        }
        // not OpenAI's function: another type, no function, a key it does not take, a field of another type
        for bad in [
            json!({ "type": "retrieval", "function": { "name": "f" } }),
            json!({ "type": "function" }),
            json!({ "type": "function", "function": { "name": "f", "code": "rm -rf /" } }),
            json!({ "type": "function", "function": { "name": "f" }, "cache": true }),
            json!({ "type": "function", "function": { "name": 7 } }),
            json!({ "type": "function", "function": { "name": "f", "description": 7 } }),
            json!({ "type": "function", "function": { "name": "f", "strict": "yes" } }),
            json!("lookup"),
        ] {
            assert_eq!(check(json!({ "prompt": "p", "tools": [bad] })), Err(Refusal::Tools), "{bad}");
        }
        assert_eq!(check(json!({ "prompt": "p", "tool_choice": "auto" })), Err(Refusal::ToolChoice), "a choice with no tools");
        let with = |choice: Value| check(json!({ "prompt": "p", "tools": [lookup("lookup")], "tool_choice": choice }));
        for good in [json!("none"), json!("auto"), json!("required"), json!({ "type": "function", "function": { "name": "lookup" } })] {
            assert_eq!(with(good.clone()).map(|b| b["tool_choice"].clone()), Ok(good));
        }
        for bad in [json!("sometimes"), json!({ "type": "function", "function": { "name": "other" } }), json!({ "type": "function", "function": { "name": "lookup", "x": 1 } }), json!({ "function": { "name": "lookup" } }), json!(true)] {
            assert_eq!(with(bad.clone()), Err(Refusal::ToolChoice), "{bad}");
        }
        assert_eq!(check(json!({ "prompt": "p", "draft": { "channel": "events", "turn": "t" } })), Err(Refusal::Draft), "a built-in channel is no app's");
        assert_eq!(check(json!({ "prompt": "p", "draft": { "channel": "Log", "turn": "t" } })), Err(Refusal::Draft));
        assert_eq!(check(json!({ "prompt": "p", "draft": { "channel": "log", "turn": "a turn" } })), Err(Refusal::Draft));
        assert_eq!(check(json!({ "prompt": "p", "draft": { "channel": "log", "turn": "t".repeat(129) } })), Err(Refusal::Draft));
        assert!(check(json!({ "prompt": "p", "draft": { "channel": "log", "turn": "t".repeat(128) } })).is_ok());
        for bad in [json!({ "channel": "log" }), json!({ "channel": "log", "turn": "t", "text": "x" }), json!("log"), json!({ "channel": 7, "turn": "t" })] {
            assert_eq!(check(json!({ "prompt": "p", "draft": bad })), Err(Refusal::Draft), "{bad}");
        }
        assert_eq!(draft_of(&text(json!({ "prompt": "p", "draft": { "channel": "log", "turn": "t:1" } }))), Some(("log", "t:1")));
        // the request it makes (within what platform.mjs counts: the step's arguments)
        let room = AI_TEXT_MAX_BYTES - json!({ "messages": [{ "role": "user", "content": "" }] }).to_string().len();
        assert!(check(json!({ "prompt": "x".repeat(room) })).is_ok(), "at the bound");
        assert_eq!(check(json!({ "prompt": "x".repeat(room + 1) })), Err(Refusal::TextTooLarge));
        assert!(Refusal::Tools.message().contains("at most 64 tools") && Refusal::TextTooLarge.message().contains("524288 bytes"));
    }

    /// A streamed tool turn as Workers AI streams one: reasoning, text,
    /// then two calls whose arguments come in pieces, one past the bound,
    /// and its usage.
    const TOOL_STREAM: &str = concat!(
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":40,\"completion_tokens\":0}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hidden\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":3}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Let me \"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":2}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"look.\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":2}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"wo\"}}]},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":4}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":null,\"function\":{\"arguments\":\"rd\\\":\\\"a\\\"}\"}}]},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":4}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"type\":\"function\",\"function\":{\"name\":\"zoom\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":2}}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":9999999999,\"id\":\"call_x\",\"function\":{\"name\":\"x\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"9\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0}}\n\n",
        "data: {\"response\":\"\",\"usage\":{\"prompt_tokens\":40,\"completion_tokens\":17,\"prompt_tokens_details\":{\"cached_tokens\":0}}}\n\n",
        "data: [DONE]\n\n",
    );

    /// Goal: a streamed answer is read into one message: its reasoning
    /// counted and dropped, a call's arguments joined from their pieces, a
    /// delta past `TOOL_CALLS_MAX` dropped, its last usage metered, and the
    /// message as the next call's messages take it back. Method: the tool
    /// stream, fed in pieces that split lines; a stream only metered.
    #[test]
    fn a_stream_is_read_into_its_answer() {
        for piece in [1, 5, 64, TOOL_STREAM.len()] {
            let mut s = Stream::answering();
            for chunk in TOOL_STREAM.as_bytes().chunks(piece) {
                s.push(chunk, None);
            }
            s.finish(None);
            let a = s.answer().cloned().unwrap();
            assert_eq!((a.content.as_str(), a.finish_reason.as_deref(), s.thought(), s.done()), ("Let me look.", Some("tool_calls"), 6, true), "pieces of {piece}");
            assert_eq!(a.tool_calls.len(), 2, "the call past the bound is dropped");
            assert_eq!(a.tool_calls[0], ToolCall { id: "call_1".into(), name: "lookup".into(), arguments: "{\"word\":\"a\"}".into() });
            let m = a.message();
            assert_eq!(m["tool_calls"][1], json!({ "id": "call_2", "type": "function", "function": { "name": "zoom", "arguments": "{}" } }));
            assert!(!m.to_string().contains("hidden"), "no reasoning: {m}");
            assert_eq!(s.usage().and_then(|u| usage_of(CHEAP_MODEL, u)), Some(Usage::Tokens { model: CHEAP_MODEL.into(), input: 40, cached_input: 0, cache_write: 0, output: 17 }));
        }
        let mut metered = Stream::default();
        metered.push(TOOL_STREAM.as_bytes(), None);
        assert_eq!((metered.answer(), metered.thought()), (None, 0), "a stream only metered keeps no answer");
        let mut plain = Stream::answering();
        plain.push(S4_STREAM.as_bytes(), None);
        let a = plain.answer().unwrap();
        assert_eq!(a.message(), json!({ "role": "assistant", "content": "1, 2, 3," }), "no calls: no tool_calls key");
    }

    /// Goal: an OpenAI-style stream (usage once, on a last chunk with no
    /// choices) passes as it came and meters that usage; a stream that
    /// broke before its usage, or ran a line past the limit, meters none.
    #[test]
    fn other_streams() {
        let openai = "data: {\"id\":\"a\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"id\":\"a\",\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(openai.as_bytes(), Some(&mut out));
        s.finish(Some(&mut out));
        assert_eq!(out, openai.as_bytes(), "nothing to rewrite");
        assert_eq!(s.usage().and_then(|u| u["prompt_tokens"].as_u64()), Some(3));
        let mut broken = Stream::default();
        broken.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"h", None);
        broken.finish(None);
        assert_eq!(broken.usage(), None, "it broke before its usage: none reported");
        let mut long = Stream::default();
        long.push(b"data: {\"response\":\"\",\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n", None);
        let mut out = Vec::new();
        long.push(&vec![b'x'; SSE_LINE_MAX + 1], Some(&mut out));
        long.finish(None);
        assert_eq!(out.len(), SSE_LINE_MAX + 1, "an overlong line passes through unread");
        assert_eq!(long.usage(), None, "and the call reports none");
    }

    const FALLBACK: &str = FALLBACK_MODEL_DEFAULT;

    /// Goal: the deployment's fallback model is Inkling 2 Small unless it
    /// names another, another model than the cheap tier's, priced at Workers
    /// AI's list price; one the book does not price is refused, as a vision
    /// model is. Method: the default, a priced model named, and unpriced
    /// ones.
    #[test]
    fn the_fallback_model_is_one_the_book_prices() {
        let book = PriceBook::defaults();
        let fallback = |text: &str| old_parse(text, &book, false).map(|m| up(&m, "fallback").model().to_string());
        assert_eq!(up(&Models::defaults(), "fallback"), &workers_ai("@cf/thinkingmachines/inkling-2-small"));
        assert_ne!(FALLBACK, CHEAP_MODEL, "a fallback is another model");
        assert_eq!(fallback(&json!({ "fallback": { "provider": "workers_ai", "model": MEDIUM_MODEL } }).to_string()).as_deref(), Ok(MEDIUM_MODEL));
        let unpriced = fallback(&json!({ "fallback": { "provider": "workers_ai", "model": "anthropic/claude-haiku-4.5" } }).to_string()).unwrap_err();
        assert!(unpriced.contains("models.slots.fallback: the model \"anthropic/claude-haiku-4.5\" is not in the price book") && unpriced.contains(FALLBACK), "{unpriced}");
        // both priced already: the cheap tier's DeepSeek, its Inkling fallback
        let p = |model: &str| book.models.iter().find(|m| m.model == model).unwrap().price;
        let (i, d) = (p(FALLBACK), p(CHEAP_MODEL));
        assert_eq!((i.input, i.cached_input, i.cache_write, i.output), (300_000, 100_000, 300_000, 1_000_000));
        assert_eq!((d.input, d.cached_input, d.cache_write, d.output), (300_000, 6_000, 300_000, 1_200_000));
    }

    /// Goal: what falls back is the vendor failing before it answered
    /// anything: its rate limit (429) or its own failure (5xx), and, on an
    /// OpenAI-compatible API, the platform's own key and account there
    /// (401, 402, 403, 404), each of which a 401 and a 402 also say aloud.
    /// A refusal of the request (400, any other 4xx) would be the
    /// fallback's too, and an answer that began (2xx) is final. Method:
    /// every class of status, on each kind of upstream.
    #[test]
    fn a_rate_limit_or_a_failure_falls_back() {
        let routed = up(&openrouter(), "cheap").clone();
        let flash = workers_ai(DEEPSEEK_FLASH);
        for upstream in [&flash, &routed] {
            for status in [429, 500, 502, 503, 504, 520, 599] {
                assert!(falls_back(upstream, status), "{status}");
            }
            for status in [100, 200, 204, 301, 400, 408, 413, 422, 428, 430, 499, 600, 0] {
                assert!(!falls_back(upstream, status), "{status}");
            }
        }
        for status in PLATFORM_KEY_REFUSALS {
            assert!(falls_back(&routed, status), "the platform's key or account: {status}");
            assert!(!falls_back(&flash, status), "Workers AI's binding has no key of ours: {status}");
        }
        assert_eq!(PLATFORM_KEY_REFUSALS, [401, 402, 403, 404]);
        assert!(key_alarm(&routed, 401).is_some_and(|w| w.contains("refused")) && key_alarm(&routed, 402).is_some_and(|w| w.contains("out of credit")));
        for status in [403, 404, 429, 502] {
            assert_eq!(key_alarm(&routed, status), None, "{status}");
        }
        assert_eq!(key_alarm(&flash, 401), None);
    }

    /// Goal: a call is tried on its model, then, when that one failed
    /// before answering, on the fallback model once (no race, no ladder):
    /// the fallback's answer, or its failure, is the call's, and the model
    /// that answered is the one whose usage is priced. Method: each path
    /// through `Tries`, valid and invalid, on the default models and on
    /// Finite's (its tiers on Finite Private, Workers AI its fallback), and
    /// each again (a replay decides the same).
    #[test]
    fn a_failed_call_falls_back_once() {
        let (finite, routed) = (finite(), openrouter());
        for models in [&*DEFAULTS, &finite, &routed] {
            let cheap = models.route(CHEAP);
            let medium = models.route(MEDIUM);
            let fallback = models.route(Named::Fallback);
            for _replay in 0..2 {
                // the model is not reached: the fallback answers
                let mut t = Tries::new(cheap, Some(fallback));
                assert_eq!((t.model(), t.fell()), (cheap, false));
                assert_eq!(t.after(Tried::Unreached), Some(fallback));
                assert_eq!((t.model(), t.fell()), (fallback, true));
                assert_eq!(t.after(Tried::Answered(200)), None, "its answer is the call's");
                assert_eq!(t.model().id(), fallback.id(), "priced as the fallback's");
                // the model fails (tonight's 502), and so does the fallback: one fallback, never a third try
                let mut t = Tries::new(cheap, Some(fallback));
                assert_eq!(t.after(Tried::Answered(502)), Some(fallback));
                assert_eq!(t.after(Tried::Answered(502)), None, "the fallback's failure is the call's");
                assert_eq!(t.after(Tried::Unreached), None);
                assert_eq!((t.model(), t.fell()), (fallback, true));
                // a rate limit falls back too (Finite Private's key's window spent)
                let mut t = Tries::new(medium, Some(fallback));
                assert_eq!(t.after(Tried::Answered(429)), Some(fallback));
                // an answer that began is final: a stream that breaks after it is no try
                let mut t = Tries::new(cheap, Some(fallback));
                assert_eq!(t.after(Tried::Answered(200)), None);
                assert_eq!(t.after(Tried::Unreached), None, "a stream that began never falls back");
                assert_eq!(t.after(Tried::Answered(502)), None);
                assert_eq!((t.model(), t.fell()), (cheap, false));
                // a refusal of the request is the call's: the fallback would refuse it too
                let mut t = Tries::new(cheap, Some(fallback));
                assert_eq!(t.after(Tried::Answered(400)), None);
                assert_eq!(t.after(Tried::Answered(502)), None, "the tries are over");
                assert_eq!(t.model(), cheap);
                // the platform's key refused, or its credit spent: the call's on Workers AI, a fallback on an OpenAI-compatible API
                for status in [401, 402, 403, 404] {
                    let mut t = Tries::new(cheap, Some(fallback));
                    let on_openai = matches!(cheap.upstream, Upstream::Openai { .. });
                    assert_eq!(t.after(Tried::Answered(status)), on_openai.then_some(fallback), "{status}");
                    assert_eq!(t.after(Tried::Answered(status)), None, "the fallback's {status} is the call's");
                }
                // no fallback, or one that runs where the call's own model does: the failure is the call's
                let same = Model { named: Named::Fallback, upstream: cheap.upstream, slot: "fallback" };
                for fallback in [None, Some(same)] {
                    let mut t = Tries::new(cheap, fallback);
                    assert_eq!(t.after(Tried::Answered(503)), None, "{fallback:?}");
                    assert_eq!((t.model(), t.fell()), (cheap, false));
                }
            }
        }
        // the same model id on another provider is another model
        let private = openai("https://finite-private.finite.containers.tinfoil.dev/v1", FINITE_PRIVATE_FLASH);
        let other = openai("https://other.example/v1", FINITE_PRIVATE_FLASH);
        let mut t = Tries::new(Model { named: CHEAP, upstream: &private, slot: "cheap" }, Some(Model { named: Named::Fallback, upstream: &other, slot: "fallback" }));
        assert_eq!(t.after(Tried::Answered(503)).map(|m| m.upstream), Some(&other));
    }

    /// Goal: a call that fell back is charged from the usage the fallback
    /// reported, priced as the fallback's. Method: the fallback's usage for
    /// a 30,000-token turn, priced with the default book, then Finite's.
    #[test]
    fn a_fallen_back_call_is_charged_as_the_fallbacks() {
        let book = PriceBook::defaults();
        let b = bound(tier(CHEAP), json!({ "messages": [{ "role": "user", "content": "hi" }] }), true).unwrap();
        let mut t = Tries::new(b.model, Some(DEFAULTS.route(Named::Fallback)));
        assert_eq!(t.after(Tried::Answered(502)).map(|m| m.id()), Some(FALLBACK));
        let used = usage_of(t.model().id(), &json!({ "prompt_tokens": 30_000, "completion_tokens": 400, "prompt_tokens_details": { "cached_tokens": 0 } })).unwrap();
        assert_eq!(used, Usage::Tokens { model: FALLBACK.into(), input: 30_000, cached_input: 0, cache_write: 0, output: 400 });
        // 30,000 in and 400 out at $0.30 and $1.00 a million: $0.0094
        assert_eq!(book.price(&used).unwrap().list, 9_400);

        // Finite's: Finite Private's call, fallen back to Workers AI's DeepSeek V4.1 Flash, at the fallback's price
        let finite = finite();
        let b = bound(finite.capped(CHEAP), json!({ "messages": [{ "role": "user", "content": "hi" }] }), true).unwrap();
        let mut t = Tries::new(b.model, Some(finite.route(Named::Fallback)));
        assert_eq!(t.after(Tried::Answered(429)).map(|m| m.id()), Some(DEEPSEEK_FLASH));
        let usage = json!({ "prompt_tokens": 30_000, "completion_tokens": 400 });
        let (on_private, fell) = (usage_of(b.model.id(), &usage).unwrap(), usage_of(t.model().id(), &usage).unwrap());
        // $0.15 and $0.50 a million on Finite Private, $0.30 and $1.20 on DeepSeek: $0.0047 and $0.00948
        assert_eq!((book.price(&on_private).unwrap().list, book.price(&fell).unwrap().list), (4_700, 9_480), "each priced as the model that answered");
    }

    /// Goal: Finite Private's answers meter as Workers AI's do: SGLang's
    /// usage (no cached tokens told, its details null) reads with all its
    /// prompt uncached, and its stream (OpenAI's: each chunk's usage null,
    /// the whole call's on a last chunk with no choices, as its limiter
    /// asks for it) meters that last usage and reaches the client with
    /// no usage on a chunk with choices. Method: both shapes, fed in pieces.
    #[test]
    fn finite_privates_answers_meter_as_any() {
        let u = usage_of(FINITE_PRIVATE_FLASH, &json!({ "prompt_tokens": 1_200, "completion_tokens": 80, "total_tokens": 1_280, "prompt_tokens_details": null })).unwrap();
        assert_eq!(u, Usage::Tokens { model: FINITE_PRIVATE_FLASH.into(), input: 1_200, cached_input: 0, cache_write: 0, output: 80 });
        let sglang = concat!(
            "data: {\"id\":\"fp1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}],\"usage\":null}\n\n",
            "data: {\"id\":\"fp1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"terminal\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}]}}],\"usage\":null}\n\n",
            "data: {\"id\":\"fp1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":null}\n\n",
            "data: {\"id\":\"fp1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":900,\"completion_tokens\":21,\"total_tokens\":921}}\n\n",
            "data: [DONE]\n\n",
        );
        let private = openai("https://finite-private.finite.containers.tinfoil.dev/v1", FINITE_PRIVATE_FLASH);
        for piece in [1, 9, sglang.len()] {
            let mut s = Stream::default().on(&private);
            let mut out = Vec::new();
            for chunk in sglang.as_bytes().chunks(piece) {
                s.push(chunk, Some(&mut out));
            }
            s.finish(Some(&mut out));
            assert_eq!(s.usage().and_then(|v| usage_of(FINITE_PRIVATE_FLASH, v)), Some(Usage::Tokens { model: FINITE_PRIVATE_FLASH.into(), input: 900, cached_input: 0, cache_write: 0, output: 21 }), "pieces of {piece}");
            let lines = data_lines(&out);
            assert_eq!(lines.len(), 4);
            assert!(lines[..3].iter().all(|l| l.get("usage").is_none()), "no chunk with choices carries usage");
            assert_eq!(lines[1]["choices"][0]["delta"]["tool_calls"][0]["function"]["name"], "terminal", "a tool call's delta as it came");
            assert_eq!(lines[3], json!({ "id": "fp1", "object": "chat.completion.chunk", "choices": [], "usage": { "prompt_tokens": 900, "completion_tokens": 21, "total_tokens": 921 } }), "the last chunk as it came");
        }
    }

    /// Goal: a call to an OpenAI-compatible API carries the client's chat
    /// completion fields of `CLIENT_KEYS` and no other (a paid extra, a
    /// route to another model, a client's own `provider` dropped), its
    /// slot's `body` over them, the route's bounded fields as they were,
    /// and its model's id; a slot without a body sends the same fields; a
    /// job's text step the same. Method: an agent's call with every field
    /// it may send and many it may not, bounded on OpenRouter's tier, with
    /// its body and without, and a step's.
    #[test]
    fn a_call_carries_the_clients_chat_fields_and_the_slots_body() {
        let models = openrouter();
        let tier = models.capped(CHEAP);
        let Upstream::Openai { model, body, .. } = tier.model.upstream else { panic!("OpenRouter's tier is an OpenAI-compatible API's") };
        let tools = json!([{ "type": "function", "function": { "name": "terminal", "parameters": { "type": "object" } } }]);
        let kept = json!({
            "messages": [{ "role": "user", "content": "hi" }], "tools": tools, "tool_choice": "auto", "temperature": 0.2, "top_p": 0.9,
            "stop": ["\n\n"], "response_format": { "type": "text" }, "seed": 7, "parallel_tool_calls": false,
        });
        let dropped = json!({
            "provider": { "zdr": false, "order": ["somewhere-dear"] }, "plugins": [{ "id": "web" }], "web_search_options": { "search_context_size": "high" },
            "transforms": ["middle-out"], "models": ["anthropic/claude-opus-5.5"], "route": "fallback", "n": 4, "user": "paul@example.com",
            "reasoning": { "effort": "high" }, "prompt_cache_key": "k", "metadata": { "a": "b" }, "service_tier": "priority", "logprobs": true,
        });
        let mut asked = json!({ "model": "cheap", "max_tokens": 99, "max_completion_tokens": 120, "reasoning_effort": "xhigh" });
        for part in [&kept, &dropped] {
            asked.as_object_mut().unwrap().extend(part.as_object().unwrap().clone());
        }
        let b = bound(tier, asked, true).unwrap();
        let sent = sent_json(&b.input, model, body.as_ref());
        let mut want = kept.as_object().cloned().unwrap();
        want.extend([("max_tokens".into(), json!(99)), ("reasoning_effort".into(), json!("low")), ("stream".into(), json!(true)), ("stream_options".into(), json!({ "include_usage": true }))]);
        want.insert("provider".into(), routing()["provider"].clone());
        want.insert("model".into(), json!(OPENROUTER_FLASH));
        assert_eq!(sent, Value::Object(want.clone()), "the client's chat fields, the route's bounds, the deployment's routing");
        assert_eq!(sent["provider"]["max_price"], json!({ "prompt": 0.15, "completion": 0.5 }));
        // no body: the same fields, no routing, and still none of the client's extras
        let plain = sent_json(&b.input, FINITE_PRIVATE_FLASH, None);
        want.remove("provider");
        want.insert("model".into(), json!(FINITE_PRIVATE_FLASH));
        assert_eq!(plain, Value::Object(want));
        for key in dropped.as_object().unwrap().keys() {
            assert!(b.input.get(key).is_some() && sent.get(key).is_none_or(|_| key == "provider") && plain.get(key).is_none(), "{key}: dropped");
        }
        // what the route writes is among what the client may send (its model the route's own)
        assert!(ROUTE_KEYS.iter().all(|k| *k == "model" || CLIENT_KEYS.contains(k)));
        // a job's text step, through the same bound and request
        let t: AiText = serde_json::from_value(json!({ "prompt": "the notes", "tier": "medium" })).unwrap();
        let step = bound(models.capped(MEDIUM), text_body(&t).unwrap(), true).unwrap();
        let sent = sent_json(&step.input, OPENROUTER_FLASH, body.as_ref());
        assert_eq!((sent["provider"].clone(), sent["messages"][0]["content"].clone(), sent["model"].clone()), (routing()["provider"].clone(), json!("the notes"), json!(OPENROUTER_FLASH)));
        assert_eq!(sent.as_object().unwrap().len(), step.input.as_object().unwrap().len() + 2, "a step sends nothing the allowlist drops");
    }

    /// What `openai_request` sends, read back.
    fn sent_json(input: &Value, id: &str, body: Option<&Map<String, Value>>) -> Value {
        serde_json::from_str(&openai_request(input, id, body)).expect("a request is JSON")
    }

    /// Goal: the bytes a call to an OpenAI-compatible API sends are pinned:
    /// its fields in their order, numbers, escapes and other scripts as
    /// they are written. Method: a call with a picture, a float, a newline
    /// and a quote in its words, bounded on OpenRouter's tier, with its
    /// slot's body and without.
    #[test]
    fn a_calls_request_bytes_are_pinned() {
        let models = openrouter();
        let tier = models.capped(CHEAP);
        let Upstream::Openai { model, body, .. } = tier.model.upstream else { panic!("OpenRouter's tier is an OpenAI-compatible API's") };
        let asked = json!({
            "model": "cheap", "temperature": 0.7, "seed": 7, "provider": { "order": ["dear"] },
            "messages": [
                { "role": "system", "content": "say \"hi\"\nthen stop" },
                { "role": "user", "content": [{ "type": "text", "text": "héllo, 世界 \u{1F600}" }, { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }] },
            ],
            "tools": [{ "type": "function", "function": { "name": "terminal", "parameters": { "type": "object", "properties": { "command": { "type": "string" } } } } }],
        });
        let b = bound(tier, asked, true).unwrap();
        let routed = r#"{"max_tokens":16384,"messages":[{"content":"say \"hi\"\nthen stop","role":"system"},{"content":[{"text":"héllo, 世界 😀","type":"text"},{"image_url":{"url":"data:image/png;base64,iVBORw0KGgo="},"type":"image_url"}],"role":"user"}],"model":"z-ai/glm-5.3-flash","provider":{"max_price":{"completion":0.5,"prompt":0.15},"quantizations":["fp8","bf16"],"require_parameters":true,"sort":"throughput","zdr":true},"reasoning_effort":"low","seed":7,"stream":true,"stream_options":{"include_usage":true},"temperature":0.7,"tools":[{"function":{"name":"terminal","parameters":{"properties":{"command":{"type":"string"}},"type":"object"}},"type":"function"}]}"#;
        assert_eq!(openai_request(&b.input, model, body.as_ref()), routed);
        let plain = openai_request(&b.input, FINITE_PRIVATE_FLASH, None);
        assert!(plain.contains(&format!(r#""model":{}"#, json!(FINITE_PRIVATE_FLASH))) && !plain.contains("provider"), "{plain}");
    }

    /// Goal: OpenRouter's stream meters as any OpenAI-compatible API's: its
    /// usage comes on a last chunk that keeps a content-free choice (its
    /// accounting frame), which is metered, and which the client gets as
    /// that chunk without usage and then OpenAI's last chunk; Workers AI's
    /// per-chunk deltas on chunks with choices are never metered. Method:
    /// OpenRouter's documented stream (a comment line, a tool call, its
    /// frame, `[DONE]`), fed whole and in pieces, read on OpenRouter's
    /// upstream and on Workers AI's.
    #[test]
    fn openrouters_accounting_frame_is_metered() {
        let or = openrouter();
        let routed = up(&or, "cheap");
        let usage = json!({ "prompt_tokens": 1_000, "completion_tokens": 30, "total_tokens": 1_030, "cost": 0.0001, "prompt_tokens_details": { "cached_tokens": 600 }, "completion_tokens_details": { "reasoning_tokens": 10 } });
        let frame = json!({ "id": "gen-1", "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "content": "", "role": "assistant" }, "finish_reason": "tool_calls", "native_finish_reason": "tool_calls" }], "usage": usage });
        let call = json!({ "id": "gen-1", "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "id": "call_1", "type": "function", "function": { "name": "terminal", "arguments": "{}" } }] }, "finish_reason": "tool_calls" }] });
        let text = format!(": OPENROUTER PROCESSING\n\ndata: {call}\n\ndata: {frame}\n\ndata: [DONE]\n\n");
        for piece in [1, 7, text.len()] {
            let mut s = Stream::answering().on(routed);
            let mut out = Vec::new();
            for chunk in text.as_bytes().chunks(piece) {
                s.push(chunk, Some(&mut out));
            }
            s.finish(Some(&mut out));
            assert!(s.done(), "pieces of {piece}");
            assert_eq!(s.usage().and_then(|u| usage_of(OPENROUTER_FLASH, u)), Some(Usage::Tokens { model: OPENROUTER_FLASH.into(), input: 400, cached_input: 600, cache_write: 0, output: 30 }), "pieces of {piece}");
            let mut bare = frame.clone();
            bare.as_object_mut().unwrap().remove("usage");
            assert_eq!(data_lines(&out), [call.clone(), bare, json!({ "id": "gen-1", "object": "chat.completion.chunk", "choices": [], "usage": usage })], "pieces of {piece}");
            let answer = s.answer().unwrap();
            assert_eq!((answer.tool_calls[0].name.as_str(), answer.finish_reason.as_deref()), ("terminal", Some("tool_calls")));
            let sent = String::from_utf8(out).unwrap();
            assert!(sent.starts_with(": OPENROUTER PROCESSING\n\n") && sent.ends_with("data: [DONE]\n\n") && sent.matches("\n\ndata: ").count() == 4, "{sent:?}");
        }
        // on Workers AI, a chunk with choices carries a delta: never the call's
        let mut s = Stream::default().on(&workers_ai(DEEPSEEK_FLASH));
        s.push(text.as_bytes(), None);
        assert_eq!(s.usage(), None, "none reported");
    }

    /// Goal: a deployment's embeddings slot is its own (crate::embed): BGE-M3
    /// on Workers AI unnamed; a self-hosted model on an OpenAI-compatible
    /// API at the price it names, laid over the book; never a chat slot's
    /// model, nor one the book cannot price. Method: parse each.
    #[test]
    fn the_embeddings_slot_is_its_own() {
        let book = PriceBook::defaults();
        let unnamed = old_parse("{}", &book, false).unwrap();
        assert_eq!(unnamed.embeddings.space().model, crate::embed::MODEL_DEFAULT);
        assert!(unnamed.priced(book.clone()).models.iter().any(|m| m.model == crate::embed::MODEL_DEFAULT));
        let local = r#"{"embeddings": {"provider": "openai", "base_url": "http://127.0.0.1:18091/v1", "model": "BAAI/bge-m3", "dimensions": 1024, "version": "1", "price": {"input": 0.01, "output": 0}}}"#;
        let m = old_parse(local, &book, true).unwrap();
        assert_eq!(m.embeddings.space().version, Some("1"));
        let row = m.priced(book.clone()).models.into_iter().find(|r| r.model == "BAAI/bge-m3").expect("its own price is its row");
        assert_eq!(row.price.input, 10_000);
        assert!(m.slots.iter().all(|s| s.name != crate::embed::EMBEDDINGS), "no chat slot");
        for (config, why) in [
            (local, "base_url"),
            (r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/deepseek-ai/deepseek-v4.1-flash-1017", "dimensions": 1024}}"#, "a chat model"),
            (r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/unknown/embedder", "dimensions": 768}}"#, "price book"),
        ] {
            let err = old_parse(config, &book, false).unwrap_err();
            assert!(err.contains("models.embeddings") && err.contains(why), "{err}");
        }
        let err = old_parse(r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "private": true}}"#, &book, false).unwrap_err();
        assert!(err.contains("models.embeddings: Workers AI and OpenRouter are not private"), "{err}");
    }

    /// Goal: a deployment whose tiers are private embeds nothing on a slot
    /// that is not (Paul's rule), and deploys anyway. Method: Private and
    /// Standard's config, with and without a private embeddings slot.
    #[test]
    fn a_private_deployment_embeds_privately_or_not_at_all() {
        let book = PriceBook::defaults();
        assert_eq!(Models::defaults().embeddings_refused(), None);
        let mut v = per_route();
        let finite = old_parse(&v.to_string(), &book, false).unwrap();
        assert!(finite.embeddings_refused().is_some_and(|why| why.contains("not private")), "Private's tier is private");
        v["embeddings"] = json!({ "provider": "openai", "base_url": "https://embed.example/v1", "model": "BAAI/bge-m3", "dimensions": 1024, "private": true, "price": { "input": 0, "output": 0 } });
        let private = old_parse(&v.to_string(), &book, false).unwrap();
        assert_eq!(private.embeddings_refused(), None);
    }
}

/// Goal: today's deployments' agents run as they did before routes were a
/// list (Paul, 2026-11-17): a config of the shape before it (`legacy`)
/// gives the same computer's view, and every call the same slot, model,
/// cap and fallbacks. Method: master's answers at 6fd2f9bd9 for three
/// configs and the default (tests/fixtures/models-golden.json, recorded
/// there from tests/fixtures/models-golden-configs.json: production's
/// shape, the e2e's plain node's, an all-OpenRouter one) for every name a
/// call may give (none, the routes', the aliases, each id, an unknown)
/// with each route said or none, each route's reach and Deep, each step's
/// route, the ids, the book's rows and embeddings; refusals compared as
/// refusals (their words name the routes now).
#[cfg(test)]
mod golden {
    use super::*;

    fn pair(m: Option<Model<'_>>) -> Value {
        m.map_or(Value::Null, |m| json!([m.slot, m.id()]))
    }

    fn record(models: &Models) -> Value {
        let image = json!({ "messages": [{ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,AA" } }] }] });
        let mut names: Vec<String> = vec!["cheap".into(), "medium".into(), "vision".into(), "fallback".into(), "nope".into()];
        names.extend(models.ids().into_iter().map(str::to_string));
        let mut calls = Map::new();
        for model in std::iter::once(None).chain(names.iter().map(|n| Some(n.as_str()))) {
            for said in [None, Some("cheap"), Some("medium")] {
                let route = said.map(|s| models.route_named(s).unwrap());
                let ran = match models.named(model, route) {
                    Err(_) => json!("refused"),
                    Ok(named) => {
                        let c = models.capped_for(named, route);
                        json!({ "slot": c.model.slot, "id": c.model.id(), "max_tokens": c.max_tokens, "fallback": pair(models.fallback_of(c.model.slot)), "image_fallback": pair(models.fallback_for_input(c.model.slot, &image)) })
                    }
                };
                calls.insert(format!("{}|{}", model.unwrap_or("null"), said.unwrap_or("null")), ran);
            }
        }
        let (mut reached, mut deep) = (Map::new(), Map::new());
        for name in ["cheap", "medium"] {
            let at = models.route_named(name).unwrap();
            let mut slots: Vec<&str> = models.reached(at).into_iter().map(|(s, _)| s).collect();
            slots.sort();
            reached.insert(name.into(), json!(slots));
            deep.insert(name.into(), pair(models.deep_of(at)));
        }
        let mut steps = Map::new();
        for step in [None, Some("cheap"), Some("medium"), Some("high"), Some("nope")] {
            let ran = match models.step_route(step) {
                Err(_) => json!("refused"),
                Ok(r) => {
                    let c = models.capped(Named::Route(u8::try_from(r).unwrap()));
                    json!({ "slot": c.model.slot, "id": c.model.id(), "max_tokens": c.max_tokens })
                }
            };
            steps.insert(step.unwrap_or("(none)").into(), ran);
        }
        let mut view = serde_json::to_value(models.ids_by_name()).unwrap();
        if let Some(images) = view["images"].as_array_mut() {
            images.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
        let mut ids: Vec<&str> = models.ids();
        ids.sort();
        let book = models.priced(PriceBook::defaults());
        let mut priced: Vec<Value> = ids.iter().map(|id| json!([id, book.models.iter().find(|m| m.model == *id).map(|m| serde_json::to_value(m.price).unwrap())])).collect();
        priced.sort_by_key(|v| v.to_string());
        json!({ "view": view, "ids": ids, "reached": reached, "deep": deep, "steps": steps, "embeddings_refused": models.embeddings_refused().is_some(), "priced": priced, "calls": calls })
    }

    /// Master's answers, its refusals as refusals.
    fn master() -> Value {
        let mut m: Value = serde_json::from_str(include_str!("../tests/fixtures/models-golden.json")).unwrap();
        for config in m.as_object_mut().unwrap().values_mut() {
            for section in ["calls", "steps"] {
                for ran in config[section].as_object_mut().unwrap().values_mut() {
                    if ran.get("refused").is_some() {
                        *ran = json!("refused");
                    }
                }
            }
            config["embeddings_refused"] = json!(config["embeddings_refused"].is_string());
        }
        m
    }

    #[test]
    fn todays_configs_route_as_they_did() {
        let configs: Value = serde_json::from_str(include_str!("../tests/fixtures/models-golden-configs.json")).unwrap();
        let mut now = Map::new();
        now.insert("defaults".into(), record(&Models::defaults()));
        for (name, config) in configs.as_object().unwrap() {
            let listed = legacy(config).unwrap();
            now.insert(name.clone(), record(&Models::parse(Some(&listed.to_string()), &PriceBook::defaults(), false).unwrap()));
        }
        let was = master();
        let mut differ = vec![];
        for (config, now) in &now {
            for (section, value) in now.as_object().unwrap() {
                let before = &was[config][section];
                match (value.as_object(), section.as_str()) {
                    (Some(map), "calls" | "steps" | "reached" | "deep") => {
                        for (k, v) in map {
                            if &before[k] != v {
                                differ.push(format!("{config}.{section}.{k}: was {} now {v}", before[k]));
                            }
                        }
                    }
                    _ if before != value => differ.push(format!("{config}.{section}: was {before} now {value}")),
                    _ => {}
                }
            }
        }
        // The one change: `fallback` with no route said was the old fixed
        // `fallback` slot, and is the default route's fallback now (its
        // `private_deep`, the same model at the same place). No client of
        // today's sends it: Hermes names its fallback by its id, and the
        // alias only from a platform that names no ids.
        let changed = r#"production.calls.fallback|null: was {"fallback":null,"id":"deepseek-v4-1-flash","image_fallback":null,"max_tokens":16384,"slot":"fallback"} now {"fallback":["cheap","glm-5-3-flash"],"id":"deepseek-v4-1-flash","image_fallback":["cheap","glm-5-3-flash"],"max_tokens":32768,"slot":"private_deep"}"#;
        assert_eq!(differ, [changed], "{}", differ.join("\n"));
    }
}
