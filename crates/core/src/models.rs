//! The model route's bounds (docs/cloudflare-v1.md, decisions 23 and 61;
//! "Lessons from cloudflare/agents" 7 and 8; spike S4). Pure:
//! cell/src/models.rs makes the calls and meters them on the payer's ledger.
//!
//! A call names one of the route's slots (`Named`): a tier, which picks
//! the model and caps what one call may write; `vision`, the deployment's
//! vision model, for an agent runtime's calls about an image (Hermes'
//! auxiliary vision, which reads its `computer_use` screenshots); or
//! `fallback`, the deployment's fallback model, for a runtime's own switch
//! when its model stalls (Hermes' `fallback_providers`, after its stale
//! detector gives up on a call that streams nothing). Neither of those two
//! is a tier: an agent, a job's step or a manifest names none but the
//! tiers. An agent's call may name a slot by its model's own id instead
//! (`Models::named`; Paul, 2026-10-16: "be able to run `/model` with
//! hermes and see the real model"): the caller's tier's slot when it says
//! its tier (`TIER_HEADER`) and that slot runs the id, else the first slot
//! that does. `Models::ids` are what it may name (`GET /v1/models`).
//!
//! Each of those four names runs where the deployment says (`Models`, its
//! config's `models`): a Workers AI model through its AI Gateway, or a
//! model on an OpenAI-compatible API, its key a Secrets Store secret
//! (`Upstream`). Unnamed, each is today's Workers AI model; Finite's own
//! deployment runs its tiers on GLM-5.3 Flash through OpenRouter (Paul,
//! 2026-10-15; decision 61), or, by config, on Finite Private. What the
//! platform sends is the client's OpenAI-shaped chat completion with only
//! these changes (`bound`): no `model` (the tier's is the call's),
//! `max_tokens` at most the tier's cap, `reasoning_effort` one of
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
//! models"): a tier's its route's fallback, `vision`'s the deployment's
//! fallback when that slot says its model reads images, any slot's the one
//! it names, whether the call named the slot by its name or by its model's
//! id. Not a race: the second call is made only after the first has
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

use fragment_proto::{valid_channel_name, valid_op_id, Tier, BUILTIN_CHANNELS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::price::{ModelPrice, PriceBook, TokenPrices, Usage};
use crate::steps::AiText;
use crate::gather::Gathered;

/// GLM-5.3 Flash and DeepSeek V4 Flash, as Workers AI's catalog names
/// them: each id once, each role below naming its model.
pub const GLM_FLASH: &str = "@cf/zai-org/glm-5.3-flash";
pub const DEEPSEEK_FLASH: &str = "@cf/deepseek-ai/deepseek-v4-flash-0731";
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
/// The tiers' models unless a deployment names others (decision 23). The
/// cheap tier, every agent's default, is GLM-5.3 Flash, DeepSeek V4 Flash
/// its fallback (`FALLBACK_MODEL_DEFAULT`; Paul, 2026-10-09: "GLM default,
/// DeepSeek fallback", after DeepSeek, the default for a few hours, was
/// short of capacity on Workers AI: first bytes of 11 to 61 s, a 429
/// "Capacity temporarily exceeded", a 160 s hang ending in a 500, while
/// GLM's were 3 to 5 s). Both take the route's requests as they are (tools,
/// `reasoning_effort` `low` and `high`, `max_tokens` up to `MAX_TOKENS`;
/// checked 2026-10-09).
pub const CHEAP_MODEL: &str = GLM_FLASH;
pub const MEDIUM_MODEL: &str = "@cf/zai-org/glm-5.3";
/// The route's name for the deployment's vision model.
pub const VISION: &str = "vision";
/// The route's name for the deployment's fallback model.
pub const FALLBACK: &str = "fallback";
/// The vision model unless the deployment names another (Paul,
/// 2026-10-05): GLM-5.3 Flash, "Vision: Yes" in Workers AI's catalog
/// (developers.cloudflare.com/workers-ai/models/glm-5.3-flash/, read
/// 2026-10-05), already priced, and the cheap tier's. Neither DeepSeek V4
/// Flash nor GLM-5.3, the medium tier's, takes images. DeepSeek Flash's
/// vision build (`deepseek-flash`) is only on DeepSeek's own API, and
/// reaching it would take our own key (decision 23: no BYOK).
pub const VISION_MODEL_DEFAULT: &str = GLM_FLASH;
/// The fallback model unless the deployment names another (Paul,
/// 2026-10-08 and 10-09): DeepSeek V4 Flash, on Workers AI, a model of its
/// own beside the cheap tier's GLM (its own replicas, and its own rate
/// limit, 50 calls a minute per account on Unified Billing). It reads no
/// images, so the route's `vision` has no fallback unless a deployment's
/// fallback says its model reads them (`Models::fallback_of`).
pub const FALLBACK_MODEL_DEFAULT: &str = DEEPSEEK_FLASH;
/// The route's four names, as a deployment's `models` names them, each
/// where one of its models runs (`Models`).
pub const SLOTS: [&str; 4] = ["cheap", "medium", FALLBACK, VISION];
/// The slots a deployment may name beyond the route's four (`Models::extra`):
/// a route's own fallback or vision model, or another slot's fallback.
pub const EXTRA_SLOTS_MAX: usize = 4;
/// Every slot a deployment's `models` may name: one key binding each, at most.
pub const SLOTS_MAX: usize = SLOTS.len() + EXTRA_SLOTS_MAX;
/// An extra slot's name's most bytes.
pub const SLOT_NAME_MAX_BYTES: usize = 24;

/// Whether `name` may name a slot of a deployment's `models`: one of the
/// route's four, or an extra's (`EXTRA_SLOTS_MAX`): 1 to
/// `SLOT_NAME_MAX_BYTES` of lower-case letters, digits and `_`, a letter
/// first (its key's binding is its name in capitals: `secrets_store::model_key`).
pub fn valid_slot_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=SLOT_NAME_MAX_BYTES).contains(&b.len()) && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}
/// The header an agent's runtime says its tier in (`Models::named`): of
/// two slots running the id a call names, its tier's is the call's (Hermes'
/// `model.default_headers`). Optional; it names no model by itself.
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
/// priced as the model that answered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
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

/// The body a call to an OpenAI-compatible API sends: the bounded `input`'s
/// fields of `CLIENT_KEYS` and no other, the slot's `body` fields over them,
/// and the model's `id`.
pub fn openai_request(input: &Value, id: &str, body: Option<&Map<String, Value>>) -> Value {
    let input = input.as_object().expect("a bounded input is an object");
    let mut request: Map<String, Value> = input.iter().filter(|(k, _)| CLIENT_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).gathered();
    for (k, v) in body.into_iter().flatten() {
        assert!(!ROUTE_KEYS.contains(&k.as_str()), "a slot's body names none of the route's keys (`check`)");
        request.insert(k.clone(), v.clone());
    }
    request.insert("model".into(), json!(id));
    Value::Object(request)
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

/// Whether an OpenAI-compatible API's `base_url` is OpenRouter's or one of
/// its subdomains: by its host as `check` reads it (`catalog::row_url`,
/// parsed and normalized: lower case, https's port 443 dropped), so no
/// spelling of the URL that reaches OpenRouter passes for another API's.
fn on_openrouter(base_url: &str) -> bool {
    crate::catalog::row_url(base_url, true).is_ok_and(|host| host.strip_suffix(OPENROUTER_HOST).is_some_and(|sub| sub.is_empty() || sub.ends_with('.')))
}

fn workers_ai(model: &str) -> Upstream {
    Upstream::WorkersAi { model: model.to_string() }
}

/// The route's models, one for each of its names (`SLOTS`): the deployment's
/// `models` (`FRAGMENT_MODELS`), each slot it does not name on Workers AI as
/// before (`Models::defaults`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Models {
    pub cheap: Upstream,
    pub medium: Upstream,
    pub fallback: Upstream,
    pub vision: Upstream,
    /// Which slots say their model reads images (a slot's `"images":
    /// true`), in `SLOTS`' order: a `vision` call falls back only to a
    /// fallback that does (`fallback_of`). Unsaid, a slot reads none.
    /// Read off each slot before it is parsed as an `Upstream`
    /// (`images_of`): what a model reads is not where it runs, and a field
    /// of the upstream's own would cost the cell's wasm 10 KB of serde.
    pub images: [bool; 4],
    /// Which slots say their model is private (a slot's `"private": true`:
    /// confidential inference that keeps nothing, Finite Private's or
    /// Tinfoil's; never Workers AI or OpenRouter), in `SLOTS`' order, read
    /// off as `images` is. A private tier is a private route: every slot
    /// its agents' calls reach is private (`reached`, `private_closed`;
    /// Paul, 2026-10-22: "When a user has private selected, it's not
    /// appropriate to fall back to a non-private model"). A slot marked
    /// private promises nothing to the agents of a tier that is not.
    pub private: [bool; 4],
    /// What Settings names each tier's route (a tier slot's `"label"`), in
    /// `Tier::Cheap`, `Tier::Medium` order: the routes a person picks among
    /// for an agent ("Private (recommended)", "Standard"). A tier with none
    /// is offered by its tier's name, and only when no tier has one, as
    /// before (`labels_of`).
    pub labels: [Option<String>; 2],
    /// The slots a deployment names beyond the route's four, in its
    /// config's order: a route's own fallback or vision model (Standard's
    /// Cloudflare), or another slot's fallback, each named by one
    /// (`routes`, `fallbacks`, `Extra::fallback`); at most `EXTRA_SLOTS_MAX`.
    pub extra: Vec<Extra>,
    /// Each tier's route, in `Tier::Cheap`, `Tier::Medium` order: the slot
    /// its calls fall back to (its own fallback: `fallback_of`), and the
    /// slot its images go to (a tier slot's `"fallback"` and `"vision"`, by
    /// slot name; `fallback` and `vision` unless it names others). Paul,
    /// 2026-10-24: "A fallback on private should fall back to something
    /// private, a fallback on normal should fall back to something normal."
    pub routes: [RouteSlots; 2],
    /// The slot the route's four's `fallback` and `vision` each fall back
    /// to when it names one (its `"fallback"`), in that order: unnamed,
    /// `fallback` has none and `vision` falls back to `fallback` when that
    /// slot says its model reads images, as before (`fallback_of`).
    pub fallbacks: [Option<String>; 2],
    /// What each of the route's four says of the calls run on it
    /// (`SlotOpts`), in `SLOTS`' order; an extra's is its own.
    pub opts: [SlotOpts; 4],
    /// Where the route's embeddings run, and their space (`models.
    /// embeddings`, crate::embed): no chat slot, so no call names it as
    /// one, and no route falls back to it.
    pub embeddings: crate::embed::Slot,
}

/// What a slot says of the calls run on it, beside where it runs: the
/// effort a mode on it thinks at (`"reasoning"`, one of `EFFORTS`: a
/// tier's, Fast's, and its Deep's; Paul, 2026-10-27), which the computer's
/// view carries for its runtime to send; and the most one call of it
/// writes (`"max_tokens"`, 1 to `MAX_TOKENS_MAX`; `MAX_TOKENS` unnamed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotOpts {
    pub reasoning: Option<&'static str>,
    pub max_tokens: Option<u32>,
}

/// A slot's `"reasoning"` and `"max_tokens"`, taken off it (`slot`, its
/// name; `o`, its object). Refused, saying why: an effort the route does
/// not pass (`EFFORTS`), a cap that is no count of 1 to `MAX_TOKENS_MAX`.
fn opts_named(slot: &str, o: &mut Map<String, Value>) -> Result<SlotOpts, String> {
    let reasoning = match o.remove("reasoning") {
        None => None,
        Some(v) => Some(EFFORTS.into_iter().find(|e| v.as_str() == Some(e)).ok_or_else(|| format!("models.{slot}: reasoning is {}, not {v}", EFFORTS.join(", ")))?),
    };
    let max_tokens = match o.remove("max_tokens") {
        None => None,
        Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| (1..=MAX_TOKENS_MAX).contains(n)).ok_or_else(|| format!("models.{slot}: max_tokens is the most one call of it writes, 1 to {MAX_TOKENS_MAX}, not {v}"))?),
    };
    Ok(SlotOpts { reasoning, max_tokens })
}

/// The route's four's `SlotOpts`, taken off them (`Models::opts`).
fn opts_of(models: &mut Value) -> Result<[SlotOpts; 4], String> {
    let mut opts = [SlotOpts::default(); 4];
    let Some(slots) = models.as_object_mut() else { return Ok(opts) };
    for (at, slot) in SLOTS.into_iter().enumerate() {
        if let Some(o) = slots.get_mut(slot).and_then(Value::as_object_mut) {
            opts[at] = opts_named(slot, o)?;
        }
    }
    Ok(opts)
}

/// A slot beyond the route's four (`Models::extra`): its name, where it
/// runs, whether it reads images and is private, and the slot its calls
/// fall back to, when it names one (its `"fallback"`; none otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    pub name: String,
    pub upstream: Upstream,
    pub images: bool,
    pub private: bool,
    pub fallback: Option<String>,
    pub opts: SlotOpts,
}

/// A tier's route (`Models::routes`): its fallback's and its vision model's
/// slots, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSlots {
    pub fallback: String,
    pub vision: String,
    /// Its Deep (a tier slot's `"deep"`, a slot's name): the model its
    /// agents' delegates run on (Paul, 2026-10-27), when it names one. Its
    /// turns are its Fast, the tier's own slot.
    pub deep: Option<String>,
}

impl RouteSlots {
    /// The route's four's own: `fallback` and `vision`, and no Deep.
    fn defaults() -> RouteSlots {
        RouteSlots { fallback: FALLBACK.into(), vision: VISION.into(), deep: None }
    }
}

/// Each tier slot's `"fallback"`, `"vision"` and `"deep"`, taken off it
/// (`Models::routes`): a slot's name, `fallback`, `vision` or an extra's
/// (checked once the extras are read: `Models::parse`). Refused, saying
/// why: one that is no name.
fn routes_of(models: &mut Value) -> Result<[RouteSlots; 2], String> {
    let mut routes = [RouteSlots::defaults(), RouteSlots::defaults()];
    let Some(slots) = models.as_object_mut() else { return Ok(routes) };
    for (at, tier) in [Tier::Cheap, Tier::Medium].into_iter().enumerate() {
        let Some(slot) = slots.get_mut(tier.as_str()).and_then(Value::as_object_mut) else { continue };
        for (key, into) in [(FALLBACK, &mut routes[at].fallback), (VISION, &mut routes[at].vision)] {
            match slot.remove(key) {
                None => {}
                Some(Value::String(name)) => *into = name,
                Some(other) => return Err(format!("models.{}: {key} names the slot its route's {key} runs on, not {other}", tier.as_str())),
            }
        }
        routes[at].deep = match slot.remove("deep") {
            None => None,
            Some(Value::String(name)) => Some(name),
            Some(other) => return Err(format!("models.{}: deep names the slot its route's Deep runs on, not {other}", tier.as_str())),
        };
    }
    Ok(routes)
}

/// A slot's own `"fallback"`, taken off it (`slot`, its name; `o`, its
/// object): the name of the slot its calls are made on once more, checked
/// once every slot is read (`Models::parse`). Refused, saying why: one
/// that is no name.
fn fallback_named(slot: &str, o: &mut Map<String, Value>) -> Result<Option<String>, String> {
    match o.remove(FALLBACK) {
        None => Ok(None),
        Some(Value::String(name)) => Ok(Some(name)),
        Some(other) => Err(format!("models.{slot}: fallback names the slot its calls fall back to, not {other}")),
    }
}

/// The route's four's `fallback` and `vision`'s own `"fallback"`, taken off
/// them (`Models::fallbacks`).
fn fallbacks_of(models: &mut Value) -> Result<[Option<String>; 2], String> {
    let mut fallbacks = [None, None];
    let Some(slots) = models.as_object_mut() else { return Ok(fallbacks) };
    for (at, slot) in [FALLBACK, VISION].into_iter().enumerate() {
        if let Some(o) = slots.get_mut(slot).and_then(Value::as_object_mut) {
            fallbacks[at] = fallback_named(slot, o)?;
        }
    }
    Ok(fallbacks)
}

/// The slots a deployment names beyond the route's four (`Models::extra`),
/// taken out of its `models`, each with its flags and its own fallback.
/// Refused, saying why: a name that is no slot's (`valid_slot_name`), more
/// than `EXTRA_SLOTS_MAX`, a label (only a tier is offered), a vision of
/// its own (only a tier's route has one), a slot out of shape (`Upstream`'s
/// refusals).
fn extras_of(models: &mut Value) -> Result<Vec<Extra>, String> {
    let Some(slots) = models.as_object_mut() else { return Ok(vec![]) };
    let names: Vec<String> = slots.keys().filter(|k| !SLOTS.contains(&k.as_str())).cloned().collect();
    if names.len() > EXTRA_SLOTS_MAX {
        return Err(format!("models: at most {EXTRA_SLOTS_MAX} slots beyond cheap, medium, fallback and vision (a route's own fallback or vision model), not {}", names.len()));
    }
    let mut extra = Vec::with_capacity(names.len());
    for name in names {
        let mut row = slots.remove(&name).expect("a key of its own");
        if Tier::parse(&name).is_some() {
            return Err(format!("models.{name}: the {name} tier is off (decision 23): its slot is no deployment's to name"));
        }
        if !valid_slot_name(&name) {
            return Err(format!("models: {name:?} is no slot's name: cheap, medium, fallback, vision, or 1 to {SLOT_NAME_MAX_BYTES} of a-z, 0-9 and _, a letter first"));
        }
        let o = row.as_object_mut().ok_or_else(|| format!("models.{name}: a slot is where a model runs, an object"))?;
        for key in ["label", VISION] {
            if o.contains_key(key) {
                return Err(format!("models.{name}: only a tier (cheap, medium) names its {key}"));
            }
        }
        let fallback = fallback_named(&name, o)?;
        let opts = opts_named(&name, o)?;
        let mut flag = |key: &str, means: &str| match o.remove(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(b),
            Some(other) => Err(format!("models.{name}: {key} is true ({means}) or false, not {other}")),
        };
        let images = flag("images", "its model reads images")?;
        let private = flag("private", "its model is private: confidential inference that keeps nothing")?;
        let upstream: Upstream = serde_json::from_str(&row.to_string()).map_err(|e| format!("models.{name}: {e}"))?;
        extra.push(Extra { name, upstream, images, private, fallback, opts });
    }
    Ok(extra)
}

/// A label's most characters: a menu entry's words.
pub const LABEL_MAX_CHARS: usize = 40;

/// Each tier slot's `label`, taken off the slots of a deployment's
/// `models` (`Models::labels`): 1 to `LABEL_MAX_CHARS` characters of plain
/// text. Refused, saying why: one on `fallback` or `vision` (a label names
/// a route a person picks, which is a tier), one that is no such text.
fn labels_of(models: &mut Value) -> Result<[Option<String>; 2], String> {
    let mut labels = [None, None];
    let Some(slots) = models.as_object_mut() else { return Ok(labels) };
    for slot in SLOTS {
        let Some(label) = slots.get_mut(slot).and_then(Value::as_object_mut).and_then(|o| o.remove("label")) else { continue };
        let at = match Tier::parse(slot) {
            Some(Tier::Cheap) => 0,
            Some(Tier::Medium) => 1,
            _ => return Err(format!("models.{slot}: a label names a route people pick, and those are the tiers (cheap, medium)")),
        };
        let text = label.as_str().map(str::trim).filter(|t| (1..=LABEL_MAX_CHARS).contains(&t.chars().count()) && !t.chars().any(char::is_control));
        let Some(text) = text else { return Err(format!("models.{slot}: label is 1 to {LABEL_MAX_CHARS} characters of plain text, not {label}")) };
        labels[at] = Some(text.to_string());
    }
    Ok(labels)
}

/// A slot's flag `key` (`images`, `private`), taken off each slot of a
/// deployment's `models`, in `SLOTS`' order: true or false, or refused
/// saying so (`means`, what true says). What is no object of slots is left
/// to the slots' own refusal.
fn flags_of(models: &mut Value, key: &str, means: &str) -> Result<[bool; 4], String> {
    let mut flags = [false; 4];
    let Some(slots) = models.as_object_mut() else { return Ok(flags) };
    for (i, slot) in SLOTS.iter().enumerate() {
        match slots.get_mut(*slot).and_then(Value::as_object_mut).and_then(|o| o.remove(key)) {
            None => {}
            Some(Value::Bool(b)) => flags[i] = b,
            Some(other) => return Err(format!("models.{slot}: {key} is true ({means}) or false, not {other}")),
        }
    }
    Ok(flags)
}

/// Whether each private tier is a private route (`Models::private`): every
/// slot its agents' calls reach (`Models::reached`) is private; and no slot
/// on Workers AI or OpenRouter is marked private. Refused, naming the slot
/// to change and why the tier's agents reach it.
/// Whether `upstream` keeps what it is sent where a private slot may not:
/// Workers AI's, or OpenRouter's.
fn open(upstream: &Upstream) -> bool {
    matches!(upstream, Upstream::WorkersAi { .. }) || matches!(upstream, Upstream::Openai { base_url, .. } if on_openrouter(base_url))
}

fn private_closed(models: &Models) -> Result<(), String> {
    for (slot, upstream) in models.all_slots().into_iter().chain([(crate::embed::EMBEDDINGS, &models.embeddings.upstream)]) {
        let private = if slot == crate::embed::EMBEDDINGS { models.embeddings.private } else { models.slot_private(slot) };
        if private && open(upstream) {
            return Err(format!("models.{slot}: Workers AI and OpenRouter are not private: only a confidential API (Finite Private's, Tinfoil's) is marked private"));
        }
    }
    for tier in [Tier::Cheap, Tier::Medium] {
        if !models.slot_private(tier.as_str()) {
            continue;
        }
        if let Some((slot, why)) = models.reached(tier).into_iter().find(|(slot, _)| !models.slot_private(slot)) {
            return Err(format!("models.{slot}: models.{} is private, and its agents' {why} reach {slot}, so mark a private model there (\"private\": true)", tier.as_str()));
        }
    }
    Ok(())
}

/// The slots a deployment names; the rest are the defaults'.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Slots {
    cheap: Option<Upstream>,
    medium: Option<Upstream>,
    fallback: Option<Upstream>,
    vision: Option<Upstream>,
}

impl Models {
    /// Every slot on Workers AI: GLM-5.3 Flash cheap, GLM-5.3 medium,
    /// DeepSeek V4 Flash the fallback, GLM-5.3 Flash vision.
    pub fn defaults() -> Models {
        Models {
            cheap: workers_ai(CHEAP_MODEL),
            medium: workers_ai(MEDIUM_MODEL),
            fallback: workers_ai(FALLBACK_MODEL_DEFAULT),
            vision: workers_ai(VISION_MODEL_DEFAULT),
            images: [false; 4],
            private: [false; 4],
            labels: [None, None],
            extra: vec![],
            routes: [RouteSlots::defaults(), RouteSlots::defaults()],
            fallbacks: [None, None],
            opts: [SlotOpts::default(); 4],
            embeddings: crate::embed::Slot::default(),
        }
    }

    /// A deployment's models: `named` (`FRAGMENT_MODELS`: an object of the
    /// slots it names, each an `Upstream`) over the defaults, checked.
    /// Refused, saying why: a slot that is none of `SLOTS` (`high` among
    /// them: decision 23), a provider that is neither, a model neither the
    /// price book nor its slot's ceiling prices (a ledger would charge none
    /// of its calls; `Upstream::ceiling`, which refuses a slot on OpenRouter
    /// without one), an OpenAI-compatible API's `base_url` that is not
    /// https on a host name (`loopback`: or a local fleet's fake at
    /// `http://127.0.0.1:<port>`), or names a query or a trailing `/`, and
    /// its `body` when it is no object, is past `UPSTREAM_BODY_MAX_BYTES`,
    /// or names a key the route writes (`ROUTE_KEYS`); and a model id two
    /// slots name at two places (`same_place`) or at two ceilings: a call
    /// names a model by its id (`named`), so an id is one model, and one
    /// row of the book (`priced`), wherever slots name it; and a private
    /// tier whose agents could reach a slot that is not (`private_closed`).
    pub fn parse(named: Option<&str>, book: &PriceBook, loopback: bool) -> Result<Models, String> {
        let mut models = Models::defaults();
        if let Some(text) = named {
            let mut v: Value = serde_json::from_str(text).map_err(|e| format!("models: {e}"))?;
            models.images = flags_of(&mut v, "images", "its model reads images")?;
            models.private = flags_of(&mut v, "private", "its model is private: confidential inference that keeps nothing")?;
            models.labels = labels_of(&mut v)?;
            models.routes = routes_of(&mut v)?;
            models.fallbacks = fallbacks_of(&mut v)?;
            models.opts = opts_of(&mut v)?;
            models.embeddings = crate::embed::slot_of(&mut v)?;
            models.extra = extras_of(&mut v)?;
            let n: Slots = serde_json::from_str(&v.to_string()).map_err(|e| format!("models: {e}"))?;
            for (slot, upstream) in [(&mut models.cheap, n.cheap), (&mut models.medium, n.medium), (&mut models.fallback, n.fallback), (&mut models.vision, n.vision)] {
                if let Some(u) = upstream {
                    *slot = u;
                }
            }
        }
        // each route's vision is a slot that is there; each slot's fallback
        // another slot, one that reads images behind a slot that sees; and
        // each extra some route's or some slot's
        for (tier, route) in [Tier::Cheap, Tier::Medium].iter().zip(&models.routes) {
            let theirs = route.vision == FALLBACK || route.vision == VISION || models.extra.iter().any(|e| e.name == route.vision);
            if !theirs {
                return Err(format!("models.{}: its vision is {:?}, which is no slot it may name: fallback, vision, or a slot of its own beside them", tier.as_str(), route.vision));
            }
        }
        for (tier, route) in [Tier::Cheap, Tier::Medium].iter().zip(&models.routes) {
            if let Some(deep) = route.deep.as_deref().filter(|d| *d == tier.as_str() || models.slot(d).is_none()) {
                return Err(format!("models.{}: its deep is {deep:?}, which is no other slot of the deployment's", tier.as_str()));
            }
        }
        for (slot, _) in models.all_slots() {
            if models.opts(slot).reasoning.is_some() && !models.is_mode(slot) {
                return Err(format!("models.{slot}: reasoning is a mode's, its tier's own or the slot a tier names as its deep"));
            }
        }
        for (slot, _) in models.all_slots() {
            let Some(to) = models.fallback_named_by(slot) else { continue };
            if to == slot || models.slot(to).is_none() {
                return Err(format!("models.{slot}: its fallback is {to:?}, which is no other slot of the deployment's"));
            }
            if models.sees(slot) && !models.slot_images(to) {
                return Err(format!("models.{slot}: images go to {slot}, so its fallback reads them too: {to} says no \"images\": true"));
            }
        }
        let named_by = |name: &str| models.routes.iter().any(|r| r.fallback == name || r.vision == name || r.deep.as_deref() == Some(name)) || models.all_slots().iter().any(|(s, _)| models.fallback_named_by(s) == Some(name));
        if let Some(e) = models.extra.iter().find(|e| !named_by(&e.name)) {
            return Err(format!("models.{}: no slot names it, as its fallback or a tier's vision or deep (\"fallback\": {:?} on another slot)", e.name, e.name));
        }
        let slots = models.all_slots();
        for (i, (slot, upstream)) in slots.iter().enumerate() {
            check(upstream, book, loopback).map_err(|e| format!("models.{slot}: {e}"))?;
            if let Some((first, _)) = slots[..i].iter().find(|(_, u)| u.model() == upstream.model() && !same_place(u, upstream)) {
                return Err(format!("models.{slot}: {:?} is models.{first}'s model too, where it runs elsewhere: a call names a model by its id, so name it where models.{first} does, or name another", upstream.model()));
            }
            if let Some((first, _)) = slots[..i].iter().find(|(_, u)| u.model() == upstream.model() && u.own_price() != upstream.own_price()) {
                return Err(format!("models.{slot}: {:?} is models.{first}'s model too, at another price: a model is one row of the price book, so name the price (or max_price) models.{first} does", upstream.model()));
            }
        }
        let embeddings = &models.embeddings.upstream;
        check(embeddings, book, loopback).map_err(|e| format!("models.embeddings: {e}"))?;
        if let Some((slot, _)) = models.all_slots().into_iter().find(|(_, u)| u.model() == embeddings.model()) {
            return Err(format!("models.embeddings: {:?} is models.{slot}'s model, a chat model: an embeddings slot runs a model of its own", embeddings.model()));
        }
        private_closed(&models)?;
        Ok(models)
    }

    /// Why this deployment embeds nothing, if it does not: a tier is
    /// private and its embeddings slot is not (Paul's rule: Finite Private
    /// never falls back to a model that is not private, so content is not
    /// sent to one to be embedded either). Its deploy is not refused: its
    /// search answers by words alone.
    pub fn embeddings_refused(&self) -> Option<String> {
        let private_tier = [Tier::Cheap, Tier::Medium].iter().any(|t| self.slot_private(t.as_str()));
        (private_tier && !self.embeddings.private).then(|| "this deployment's tiers are private and its embeddings slot is not: nothing is embedded rather than sent to a model that is not private (name a private embeddings slot)".to_string())
    }

    /// Why this deployment makes no videos, if it does not: a tier is
    /// private, and the video model (crate::media::VIDEO_MODEL, a third
    /// party's through the gateway) is not: zero data retention is not
    /// confidential inference. As with embeddings, a prompt is not sent to
    /// a model that is not private (Paul, 2026-10-22).
    pub fn video_refused(&self) -> Option<String> {
        let private_tier = [Tier::Cheap, Tier::Medium].iter().any(|t| self.slot_private(t.as_str()));
        private_tier.then(|| "this deployment's tiers are private and its video model is not: a video's prompt is not sent to a model that is not private".to_string())
    }

    /// `book` with each slot's own price (`Upstream::own_price`: its list
    /// price, or its ceiling) as its model's row, over any row of that id:
    /// the book the deployment charges with, so a change to its config
    /// re-prices its calls with the deploy.
    pub fn priced(&self, mut book: PriceBook) -> PriceBook {
        for upstream in self.all_slots().into_iter().map(|(_, u)| u).chain([&self.embeddings.upstream]) {
            if let Some(price) = upstream.own_price().expect("a slot's price is checked as it is parsed") {
                book.models.retain(|m| m.model != upstream.model());
                book.models.push(ModelPrice { model: upstream.model().to_string(), price });
            }
        }
        book
    }

    /// Each slot's name (`SLOTS`) and where it runs.
    pub fn slots(&self) -> [(&'static str, &Upstream); 4] {
        [(SLOTS[0], &self.cheap), (SLOTS[1], &self.medium), (SLOTS[2], &self.fallback), (SLOTS[3], &self.vision)]
    }

    /// Every slot's name and where it runs: the route's four, then the
    /// extras, in the config's order.
    pub fn all_slots(&self) -> Vec<(&str, &Upstream)> {
        let mut all: Vec<(&str, &Upstream)> = self.slots().to_vec();
        all.extend(self.extra.iter().map(|e| (e.name.as_str(), &e.upstream)));
        all
    }

    /// The slot named `name`: where it runs. None for a name no slot has.
    fn slot(&self, name: &str) -> Option<&Upstream> {
        self.all_slots().into_iter().find(|(n, _)| *n == name).map(|(_, u)| u)
    }

    /// The model of the slot named `name` (one `all_slots` names), called
    /// as `named`.
    fn model_of<'a>(&'a self, name: &str, named: Named) -> Model<'a> {
        let (slot, upstream) = self.all_slots().into_iter().find(|(n, _)| *n == name).expect("a route names a slot that is there (`parse`)");
        Model { named, upstream, slot }
    }

    /// Each slot as a call naming its model's id names it, in `all_slots`'
    /// order, and where it runs: a tier's as that tier, any other as itself
    /// (`Named::Slot`, by its place in `all_slots`), never through a
    /// caller's route: an id is a model (#589's review, P2; only the aliases
    /// `fallback` and `vision` follow a route). Either way the call falls
    /// back as its slot does (`fallback_of`).
    fn named_slots(&self) -> Vec<(Named, &Upstream)> {
        let at = |i: usize| Named::Slot(u8::try_from(i).expect("at most SLOTS_MAX slots"));
        let mut all = vec![(Named::Tier(Tier::Cheap), &self.cheap), (Named::Tier(Tier::Medium), &self.medium), (at(2), &self.fallback), (at(3), &self.vision)];
        all.extend(self.extra.iter().enumerate().map(|(i, e)| (at(SLOTS.len() + i), &e.upstream)));
        all
    }

    /// `tier`'s route, when it is one of the two that have one.
    fn route_of(&self, tier: Tier) -> Option<&RouteSlots> {
        match tier {
            Tier::Cheap => Some(&self.routes[0]),
            Tier::Medium => Some(&self.routes[1]),
            Tier::High => None,
        }
    }

    /// The route's models' ids, each once, in `SLOTS`' order: what a call
    /// may name besides the route's names (`named`), and what `GET
    /// /v1/models` lists (`listed`).
    pub fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::with_capacity(SLOTS_MAX);
        for (_, u) in self.all_slots() {
            if !ids.contains(&u.model()) {
                ids.push(u.model());
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

    /// Each slot's model id by its name, and each tier's route (its label,
    /// whether it is private, its fallback's and vision model's ids, the
    /// effort its turns think at, and its Deep: its id, effort and
    /// fallback's id), as a
    /// computer's guest and its owner's Settings read them in its view
    /// (`GET /api/computer`'s and `GET /api/computers`' `models`).
    pub fn ids_by_name(&self) -> fragment_proto::computer::ModelIds {
        use fragment_proto::computer::{ModelIds, RouteMode, TierRoute, TierRoutes};
        let id = |u: &Upstream| u.model().to_string();
        let route = |tier: Tier, label: &Option<String>| {
            let slots = self.route_of(tier).expect("a tier with a route");
            let of = |name: &str| self.slot(name).map(Upstream::model).unwrap_or_default().to_string();
            let reasoning = |slot: &str| self.opts(slot).reasoning.map(str::to_string);
            let deep = self.deep_of(tier).map(|d| RouteMode { id: d.id().to_string(), reasoning: reasoning(d.slot), fallback: self.fallback_of(d.slot).map(|f| f.id().to_string()) });
            TierRoute { label: label.clone(), private: self.slot_private(tier.as_str()), fallback: of(&slots.fallback), vision: of(&slots.vision), reasoning: reasoning(tier.as_str()), deep }
        };
        let routes = TierRoutes { cheap: route(Tier::Cheap, &self.labels[0]), medium: route(Tier::Medium, &self.labels[1]) };
        let mut images = Vec::new();
        for (slot, upstream) in self.all_slots() {
            if self.slot_images(slot) && !images.iter().any(|id| id == upstream.model()) { images.push(upstream.model().to_string()); }
        }
        ModelIds { cheap: id(&self.cheap), medium: id(&self.medium), fallback: id(&self.fallback), vision: id(&self.vision), routes: Some(routes), images: Some(images) }
    }

    /// What a call naming `model` runs, as the route reads its body's
    /// `model` and its caller's `tier` (`TIER_HEADER`): none named, the
    /// default tier; one of the route's names (`route_name`), that slot; a
    /// model's id, the slot of `tier` when that slot runs it, else its Deep
    /// when that runs it (`deep_of`), else the first slot that does
    /// (`all_slots`' order: a tier's before `fallback` and `vision`).
    /// Anything else names none (`UnknownModel`). `high` is a tier here,
    /// refused by `capped`.
    pub fn named(&self, model: Option<&str>, tier: Option<Tier>) -> Result<Named, UnknownModel> {
        let Some(name) = model else { return Ok(Named::Tier(fragment_proto::DEFAULT_TIER)) };
        if let Some(named) = route_name(name) {
            return Ok(named);
        }
        let slots = self.named_slots();
        let theirs = tier.map(Named::Tier).filter(|t| slots.iter().any(|(n, u)| n == t && u.model() == name));
        let their_deep = || tier.and_then(|t| self.deep_of(t)).filter(|d| d.id() == name).map(|d| d.named);
        theirs.or_else(their_deep).or_else(|| slots.iter().find(|(_, u)| u.model() == name).map(|(n, _)| *n)).ok_or_else(|| self.unknown())
    }

    /// The refusal of a `model` that names none of the route's models.
    pub fn unknown(&self) -> UnknownModel {
        UnknownModel { allowed: self.ids().into_iter().map(str::to_string).collect() }
    }

    /// The model a call naming `named` runs, whoever calls: its tier's,
    /// the route's four's vision or fallback model, or an extra's; `high`
    /// is refused. `route_for` runs a caller's route's.
    pub fn route(&self, named: Named) -> Result<Model<'_>, Refusal> {
        self.route_for(named, None)
    }

    /// The model a call naming `named` runs for a caller on `tier` (its
    /// `TIER_HEADER`; a job's step's tier): `vision` and `fallback` are its
    /// route's (`routes`), and the route's four's for a caller that says
    /// none; a tier is that tier's; `high` is refused.
    pub fn route_for(&self, named: Named, tier: Option<Tier>) -> Result<Model<'_>, Refusal> {
        let route = tier.and_then(|t| self.route_of(t));
        Ok(match named {
            Named::Tier(Tier::High) => return Err(Refusal::HighOff),
            Named::Tier(t) => self.model_of(t.as_str(), named),
            Named::Fallback => self.model_of(route.map_or(FALLBACK, |r| r.fallback.as_str()), named),
            Named::Vision => self.model_of(route.map_or(VISION, |r| r.vision.as_str()), named),
            Named::Slot(i) => {
                let (name, _) = self.all_slots()[usize::from(i)];
                self.model_of(name, named)
            }
        })
    }

    /// The deployment's fallback model, the route's four's.
    pub fn fallback(&self) -> Model<'_> {
        self.model_of(FALLBACK, Named::Fallback)
    }

    /// The slot that the slot named `slot` names as its fallback: a tier's,
    /// its route's (`routes`); the route's four's `fallback` and `vision`'s,
    /// and an extra's, its own `"fallback"`, when it names one.
    fn fallback_named_by(&self, slot: &str) -> Option<&str> {
        if let Some(route) = Tier::parse(slot).and_then(|t| self.route_of(t)) {
            return Some(&route.fallback);
        }
        match slot {
            FALLBACK => self.fallbacks[0].as_deref(),
            VISION => self.fallbacks[1].as_deref(),
            _ => self.extra.iter().find(|e| e.name == slot).and_then(|e| e.fallback.as_deref()),
        }
    }

    /// The model a call that the slot named `slot` fails before answering is
    /// made on once more (`Tries`), whoever called and however it named the
    /// slot (its name, an alias, its model's id: #589's review): the slot
    /// it names (`fallback_named_by`), or, for the route's four's `vision`
    /// that names none, `fallback` when that slot says its model reads
    /// images (the default, DeepSeek V4 Flash, reads none, and an image sent
    /// to a model that reads none is its refusal, which no fallback answers
    /// better). None for any other slot: `fallback`'s, an extra's that
    /// names none, `high`'s.
    pub fn fallback_of(&self, slot: &str) -> Option<Model<'_>> {
        let to = self.fallback_named_by(slot).or_else(|| (slot == VISION && self.slot_images(FALLBACK)).then_some(FALLBACK))?;
        Some(self.model_of(to, Named::Fallback))
    }

    /// Native image calls may fall back only to another model that reads
    /// images. Text calls keep their existing chain, including older images.
    pub fn fallback_for_input(&self, slot: &str, input: &Value) -> Option<Model<'_>> {
        let fallback = self.fallback_of(slot)?;
        let images = input["messages"].as_array().is_some_and(|messages| messages.iter().any(|m| m["content"].as_array().is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))));
        if images && !self.slot_images(fallback.slot) { return None; }
        Some(fallback)
    }

    /// Whether images go to the slot named `slot`: the route's four's
    /// `vision`, or a tier's route's vision.
    fn sees(&self, slot: &str) -> bool {
        slot == VISION || self.routes.iter().any(|r| r.vision == slot)
    }

    /// What the slot named `slot` says of the calls run on it.
    pub fn opts(&self, slot: &str) -> SlotOpts {
        match SLOTS.iter().position(|s| *s == slot) {
            Some(at) => self.opts[at],
            None => self.extra.iter().find(|e| e.name == slot).map(|e| e.opts).unwrap_or_default(),
        }
    }

    /// Whether the slot named `slot` is a mode, one an agent's runtime runs
    /// a kind of work on (Paul, 2026-10-27): a tier's own (Fast, its turns),
    /// or one a tier names as its Deep (its delegates).
    fn is_mode(&self, slot: &str) -> bool {
        Tier::parse(slot).is_some_and(|t| self.route_of(t).is_some()) || self.routes.iter().any(|r| r.deep.as_deref() == Some(slot))
    }

    /// `tier`'s Deep: the slot its agents' delegates run on, when its route
    /// names one (`RouteSlots::deep`).
    pub fn deep_of(&self, tier: Tier) -> Option<Model<'_>> {
        let name = self.route_of(tier)?.deep.as_deref()?;
        let at = self.all_slots().iter().position(|(n, _)| *n == name).expect("a route's deep is a slot that is there (`parse`)");
        Some(self.model_of(name, self.named_slots()[at].0))
    }

    /// Whether the slot named `slot` (one `all_slots` names) says its model is private.
    pub fn slot_private(&self, slot: &str) -> bool {
        match SLOTS.iter().position(|s| *s == slot) {
            Some(at) => self.private[at],
            None => self.extra.iter().any(|e| e.name == slot && e.private),
        }
    }

    /// Whether the slot named `slot` (one `all_slots` names) says its model reads images.
    fn slot_images(&self, slot: &str) -> bool {
        match SLOTS.iter().position(|s| *s == slot) {
            Some(at) => self.images[at],
            None => self.extra.iter().any(|e| e.name == slot && e.images),
        }
    }

    /// The slots an agent on `tier` can reach, and why, as Hermes v0.21.7
    /// and the route send its calls: its tier (its turns; and Hermes'
    /// recovery of an auxiliary call to the main model, a vision call's
    /// included, after that call and its fallback fail:
    /// `agent/auxiliary_client.py`, `_try_main_agent_model_fallback`), its
    /// route's fallback (Hermes' own switch on a stall, an auxiliary call's
    /// `fallback_providers`) and its route's vision (Hermes' auxiliary
    /// vision, whatever the tier). A private tier's are all private
    /// (`private_closed`); the high tier, off, reaches none.
    ///
    /// And the route's four's `fallback`, whatever the tier's route names:
    /// an image before its route was read (#590) names the deployment's
    /// fallback by its id for its stall, its tier unsaid (#589's review,
    /// P1), so that slot is a private tier's too until every image reads
    /// its route. A stall's call, its tier unsaid, runs the first slot that
    /// runs the id it names (`named`), which is reached too; and a call to
    /// any slot reached is made once more on that slot's own fallback
    /// (`fallback_of`), which is reached too, and so on.
    pub fn reached(&self, tier: Tier) -> Vec<(&str, &'static str)> {
        let Some(r) = self.route_of(tier) else { return vec![] };
        let mut reached = vec![(tier.as_str(), "turns and recovered calls"), (r.fallback.as_str(), "failures and stalls"), (r.vision.as_str(), "images")];
        if let Some(deep) = r.deep.as_deref() {
            reached.push((deep, "delegates"));
        }
        if r.fallback != FALLBACK {
            reached.push((FALLBACK, "stalls on an older image (its tier unsaid)"));
        }
        let by_id = |slot: &str| self.slot(slot).and_then(|u| self.named(Some(u.model()), None).ok()).and_then(|n| self.route(n).ok()).map(|m| m.slot);
        let deep_stall = r.deep.as_deref().and_then(|d| self.fallback_of(d)).and_then(|f| by_id(f.slot));
        let stalled = [by_id(&r.fallback), by_id(FALLBACK), deep_stall];
        fn add<'a>(reached: &mut Vec<(&'a str, &'static str)>, slot: Option<&'a str>, why: &'static str) {
            if let Some(slot) = slot.filter(|s| !reached.iter().any(|(r, _)| r == s)) {
                reached.push((slot, why));
            }
        }
        for slot in stalled {
            add(&mut reached, slot, "stalls (by its model's id)");
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
    pub fn capped(&self, named: Named) -> Result<Capped<'_>, Refusal> {
        self.capped_for(named, None)
    }

    /// `route_for`'s model, and the most one call of it writes.
    pub fn capped_for(&self, named: Named, tier: Option<Tier>) -> Result<Capped<'_>, Refusal> {
        let model = self.route_for(named, tier)?;
        Ok(Capped { model, max_tokens: self.opts(model.slot).max_tokens.unwrap_or(MAX_TOKENS) })
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

/// The slot a call to the route runs on: a tier's, named by its name or by
/// its model's id; `vision` or `fallback`, named by those aliases, which
/// follow the caller's route (`Models::route_for`); or a slot named by its
/// model's id (`Slot`, its place in `Models::all_slots`: the route's four's
/// `fallback` and `vision`, then the extras), which runs that slot exactly.
/// Each call falls back as the slot it runs does (`Models::fallback_of`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    Tier(Tier),
    Vision,
    Fallback,
    Slot(u8),
}

impl Named {
    pub fn as_str(self) -> &'static str {
        match self {
            Named::Tier(t) => t.as_str(),
            Named::Vision => VISION,
            Named::Fallback => FALLBACK,
            Named::Slot(2) => FALLBACK,
            Named::Slot(3) => VISION,
            Named::Slot(_) => "slot",
        }
    }
}

/// Why a model call is refused before anything is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `high` stays off until Cloudflare raises Unified Billing's Opus
    /// limit, about 2 calls a minute per edge machine (decision 23, S4).
    HighOff,
    /// A job's step names no tier (it names a tier, never a model id).
    UnknownTier,
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
            Refusal::HighOff => "the high tier is off until Cloudflare raises Unified Billing's limit on Opus; use cheap or medium".into(),
            Refusal::UnknownTier => "model names a tier: cheap or medium (a model id is never one)".into(),
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

/// The tier a tier's name names (an agent's setting, a job's step); `None`
/// is the default tier. `vision` is none, and a model's id is none: only
/// the model route takes one (`Models::named`).
pub fn tier_named(name: Option<&str>) -> Result<Tier, Refusal> {
    match name {
        None => Ok(fragment_proto::DEFAULT_TIER),
        Some(n) => Tier::parse(n).ok_or(Refusal::UnknownTier),
    }
}

/// The slot one of the route's own names names: `vision`, `fallback`, or a
/// tier's (`Models::named` reads a model's id after these).
fn route_name(name: &str) -> Option<Named> {
    match name {
        VISION => Some(Named::Vision),
        FALLBACK => Some(Named::Fallback),
        other => Tier::parse(other).map(Named::Tier),
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
        let Ok(Value::Object(mut chunk)) = serde_json::from_str::<Value>(data) else { return None };
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

    fn tier(t: Tier) -> Capped<'static> {
        DEFAULTS.capped(Named::Tier(t)).unwrap()
    }

    /// Finite's own deployment's models (deploy/example.jsonc; decision
    /// 61): its tiers on Finite Private, its fallback and vision on
    /// Workers AI's GLM-5.3 Flash.
    const FINITE: &str = r#"{
        "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" },
        "medium": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" },
        "fallback": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash" },
        "vision": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash" }
    }"#;

    fn finite() -> Models {
        Models::parse(Some(FINITE), &PriceBook::defaults(), false).unwrap()
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
        let flash = json!({ "provider": "workers_ai", "model": GLM_FLASH });
        let text = json!({ "cheap": tier, "medium": tier, "fallback": flash, "vision": flash }).to_string();
        Models::parse(Some(&text), &PriceBook::defaults(), false).unwrap()
    }

    /// Goal: the route's `vision` runs the deployment's vision model, at
    /// the tiers' cap, and is no tier (an agent, a job's step and a
    /// manifest name only tiers). Method: the route's names, the tiers'
    /// names, and a call bounded on it.
    #[test]
    fn vision_is_the_routes_and_no_tier() {
        let named = |n: Option<&str>| DEFAULTS.named(n, None);
        assert_eq!(named(Some("vision")), Ok(Named::Vision));
        assert_eq!(named(Some("medium")), Ok(Named::Tier(Tier::Medium)));
        assert_eq!(named(None), Ok(Named::Tier(fragment_proto::DEFAULT_TIER)), "none named: the default tier");
        assert_eq!(named(Some("Vision")), Err(DEFAULTS.unknown()));
        assert_eq!(named(Some("high")).map(|n| DEFAULTS.capped(n)), Ok(Err(Refusal::HighOff)));
        assert_eq!(tier_named(Some("vision")), Err(Refusal::UnknownTier), "vision is no tier");
        assert_eq!(Tier::parse("vision"), None);
        assert_eq!((Named::Vision.as_str(), Named::Tier(Tier::Cheap).as_str()), ("vision", "cheap"));
        let mut seeing = Models::defaults();
        seeing.vision = workers_ai("@cf/example/seeing");
        let v = seeing.capped(Named::Vision).unwrap();
        assert_eq!(v, Capped { model: Model { named: Named::Vision, upstream: &workers_ai("@cf/example/seeing"), slot: "vision" }, max_tokens: MAX_TOKENS });
        assert_eq!(seeing.capped(Named::Tier(Tier::Medium)).unwrap().model.id(), MEDIUM_MODEL, "a tier's call is the tier's");
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
        assert_eq!(Named::Fallback.as_str(), "fallback");
        assert_eq!(DEFAULTS.named(Some("Fallback"), None), Err(DEFAULTS.unknown()));
        assert_eq!(tier_named(Some("fallback")), Err(Refusal::UnknownTier), "fallback is no tier: no agent, step or manifest names it");
        assert_eq!(Tier::parse("fallback"), None);
        let f = DEFAULTS.capped(Named::Fallback).unwrap();
        assert_eq!((f.model.named, f.model.id(), f.max_tokens), (Named::Fallback, "@cf/deepseek-ai/deepseek-v4-flash-0731", MAX_TOKENS));
        assert_eq!(DEFAULTS.fallback(), f.model);
        // Hermes' stale switch on Finite's deployment: Workers AI's GLM-5.3 Flash
        let finite = finite();
        assert_eq!(finite.route(Named::Fallback).unwrap().upstream, &workers_ai(GLM_FLASH), "the deployment's, whatever it names");
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
        let (cheap, medium) = (Named::Tier(Tier::Cheap), Named::Tier(Tier::Medium));
        let d = &*DEFAULTS;
        assert_eq!(d.named(Some(GLM_FLASH), None), Ok(cheap), "cheap's and vision's id: the first slot's, a tier's");
        assert_eq!(d.named(Some(GLM_FLASH), Some(Tier::Medium)), Ok(cheap), "a tier whose slot does not run it is passed over");
        assert_eq!(d.named(Some(MEDIUM_MODEL), Some(Tier::Cheap)), Ok(medium));
        assert_eq!(d.named(Some(DEEPSEEK_FLASH), Some(Tier::Cheap)), Ok(Named::Slot(2)), "the fallback's id, its slot exactly: no fallback of its own");
        assert_eq!(d.named(Some(MEDIUM_MODEL), Some(Tier::High)), Ok(medium), "high has no slot");
        let routed = openrouter();
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), None), Ok(cheap));
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), Some(Tier::Cheap)), Ok(cheap));
        assert_eq!(routed.named(Some(OPENROUTER_FLASH), Some(Tier::Medium)), Ok(medium), "both tiers run it: the caller's tier's");
        assert_eq!(routed.named(Some(GLM_FLASH), Some(Tier::Cheap)), Ok(Named::Slot(2)), "fallback's and vision's: the first, fallback");
        assert_eq!((Named::Slot(2).as_str(), Named::Slot(3).as_str()), ("fallback", "vision"));
        let m = routed.capped(medium).unwrap().model;
        assert_eq!((m.named.as_str(), m.id(), m.upstream), ("medium", OPENROUTER_FLASH, &routed.medium), "the medium slot's model, key and body");
        assert_eq!(finite().named(Some(FINITE_PRIVATE_FLASH), Some(Tier::Medium)), Ok(medium));
        // the route's names are names, whatever the caller's tier
        for (name, slot) in [("cheap", cheap), ("medium", medium), ("vision", Named::Vision), ("fallback", Named::Fallback)] {
            assert_eq!(routed.named(Some(name), Some(Tier::Medium)), Ok(slot), "{name}");
        }
        assert_eq!(routed.named(None, Some(Tier::Medium)), Ok(cheap), "none named is the default tier, as before");
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
        let allowed = vec![GLM_FLASH.to_string(), MEDIUM_MODEL.to_string(), DEEPSEEK_FLASH.to_string()];
        for bad in ["gpt-4o", "@cf/zai-org/glm-5.3-flashx", "Cheap", "", OPENROUTER_FLASH] {
            assert_eq!(d.named(Some(bad), Some(Tier::Cheap)), Err(UnknownModel { allowed: allowed.clone() }), "{bad:?}");
        }
        let said = d.unknown().message();
        assert!(allowed.iter().all(|id| said.contains(id.as_str())) && !said.contains("cheap") && said.contains("GET /v1/models"), "{said}");
        let routed = openrouter();
        assert_eq!(routed.ids(), [OPENROUTER_FLASH, GLM_FLASH], "each id once");
        assert_eq!(routed.named(Some(MEDIUM_MODEL), None), Err(UnknownModel { allowed: vec![OPENROUTER_FLASH.into(), GLM_FLASH.into()] }), "a model the deployment does not run");
        let listed = routed.listed();
        assert_eq!(
            listed,
            json!({ "object": "list", "data": [
                { "id": OPENROUTER_FLASH, "object": "model", "created": 0, "owned_by": "fragment" },
                { "id": GLM_FLASH, "object": "model", "created": 0, "owned_by": "fragment" },
            ] })
        );
        let text = listed.to_string();
        assert!(["openrouter.ai", "zdr", "cheap", "fallback", "vision", "workers_ai"].iter().all(|w| !text.contains(w)), "ids alone: {text}");
        let route = |fallback: &str, vision: &str| json!({ "private": false, "fallback": fallback, "vision": vision });
        assert_eq!(
            serde_json::to_value(routed.ids_by_name()).unwrap(),
            json!({ "cheap": OPENROUTER_FLASH, "medium": OPENROUTER_FLASH, "fallback": GLM_FLASH, "vision": GLM_FLASH, "images": [], "routes": { "cheap": route(GLM_FLASH, GLM_FLASH), "medium": route(GLM_FLASH, GLM_FLASH) } })
        );
        assert_eq!(
            serde_json::to_value(d.ids_by_name()).unwrap(),
            json!({ "cheap": GLM_FLASH, "medium": MEDIUM_MODEL, "fallback": DEEPSEEK_FLASH, "vision": GLM_FLASH, "images": [], "routes": { "cheap": route(DEEPSEEK_FLASH, GLM_FLASH), "medium": route(DEEPSEEK_FLASH, GLM_FLASH) } })
        );
        // a reader of the view before its routes (a pinned image's) reads it still
        let before: fragment_proto::computer::ModelIds = serde_json::from_value(json!({ "cheap": GLM_FLASH, "medium": MEDIUM_MODEL, "fallback": DEEPSEEK_FLASH, "vision": GLM_FLASH })).unwrap();
        assert_eq!(before.routes, None);
    }

    /// Goal: a deployment's `models` name where each of the route's names
    /// runs, each slot it does not name as before; Finite's own (decision
    /// 61) puts its tiers on Finite Private and its fallback and vision on
    /// Workers AI's GLM-5.3 Flash. Method: none named, Finite's, and each
    /// slot alone.
    #[test]
    fn a_deployments_models_name_where_each_runs() {
        let book = PriceBook::defaults();
        assert_eq!(Models::parse(None, &book, false), Ok(Models::defaults()));
        assert_eq!(Models::parse(Some("{}"), &book, false), Ok(Models::defaults()), "an empty object names none");
        let d = Models::defaults();
        assert_eq!(
            d.slots().map(|(slot, u)| (slot, u.model().to_string())),
            [("cheap", GLM_FLASH.into()), ("medium", MEDIUM_MODEL.into()), ("fallback", DEEPSEEK_FLASH.into()), ("vision", GLM_FLASH.into())],
            "every slot on Workers AI, as before"
        );
        assert!(d.slots().iter().all(|(_, u)| matches!(u, Upstream::WorkersAi { .. })));
        let finite = finite();
        let private = openai("https://finite-private.finite.containers.tinfoil.dev/v1", FINITE_PRIVATE_FLASH);
        assert_eq!(finite, Models { cheap: private.clone(), medium: private.clone(), fallback: workers_ai(GLM_FLASH), vision: workers_ai(GLM_FLASH), images: [false; 4], private: [false; 4], labels: [None, None], extra: vec![], routes: [RouteSlots::defaults(), RouteSlots::defaults()], fallbacks: [None, None], opts: [SlotOpts::default(); 4], embeddings: crate::embed::Slot::default() });
        assert_eq!(finite.route(Named::Tier(Tier::Cheap)).unwrap(), Model { named: Named::Tier(Tier::Cheap), upstream: &private, slot: "cheap" });
        assert_eq!(finite.route(Named::Tier(Tier::Medium)).unwrap().id(), "glm-5-3-flash");
        assert_eq!(finite.route(Named::Tier(Tier::High)), Err(Refusal::HighOff), "high stays off wherever the tiers run");
        // one slot alone: the rest as before
        let medium = Models::parse(Some(r#"{"medium": {"provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash"}}"#), &book, false).unwrap();
        assert_eq!(medium, Models { medium: workers_ai(GLM_FLASH), ..Models::defaults() });
        // its wire form is the config's, less the key (the deploy's)
        assert_eq!(serde_json::to_value(&private).unwrap(), json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": "glm-5-3-flash" }));
        assert_eq!(serde_json::to_value(workers_ai(GLM_FLASH)).unwrap(), json!({ "provider": "workers_ai", "model": GLM_FLASH }));
        // OpenRouter's (Finite's since 2026-10-15): its tiers routed by their body, the rest on Workers AI
        let routed = Upstream::Openai { base_url: "https://openrouter.ai/api/v1".into(), model: OPENROUTER_FLASH.into(), body: routing().as_object().cloned(), price: None };
        assert_eq!(openrouter(), Models { cheap: routed.clone(), medium: routed.clone(), fallback: workers_ai(GLM_FLASH), vision: workers_ai(GLM_FLASH), images: [false; 4], private: [false; 4], labels: [None, None], extra: vec![], routes: [RouteSlots::defaults(), RouteSlots::defaults()], fallbacks: [None, None], opts: [SlotOpts::default(); 4], embeddings: crate::embed::Slot::default() });
        assert_eq!(serde_json::to_value(&routed).unwrap(), json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": "z-ai/glm-5.3-flash", "body": routing() }));
        let empty = Models::parse(Some(&json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH, "body": {} } }).to_string()), &book, false).unwrap();
        assert_eq!(empty.cheap, Upstream::Openai { base_url: "https://a.example/v1".into(), model: FINITE_PRIVATE_FLASH.into(), body: Some(Map::new()), price: None }, "an empty body adds nothing");
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
        let parse = |text: &str, loopback: bool| Models::parse(Some(text), &book, loopback);
        let slot = |row: Value| json!({ "cheap": row }).to_string();
        let refused = |text: &str, says: &str| {
            let e = parse(text, false).unwrap_err();
            assert!(e.contains(says), "{text}: {e}");
        };
        refused(r#"{"high": {"provider": "workers_ai", "model": "@cf/zai-org/glm-5.3"}}"#, "models.high: the high tier is off (decision 23)");
        refused(r#"{"cheapest": {"provider": "workers_ai", "model": "@cf/zai-org/glm-5.3"}}"#, "models.cheapest: no slot names it, as its fallback or a tier's vision");
        refused(&slot(json!({ "provider": "anthropic", "model": "glm-5-3-flash" })), "unknown variant `anthropic`");
        refused(&slot(json!({ "model": "glm-5-3-flash" })), "missing field `provider`");
        refused(&slot(json!({ "provider": "openai", "model": "glm-5-3-flash" })), "missing field `base_url`");
        refused(&slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "glm-5-3-flash", "key": "fragment-finite-private-key" })), "unknown field `key`");
        refused(&slot(json!({ "provider": "workers_ai", "base_url": "https://a.example/v1", "model": GLM_FLASH })), "unknown field `base_url`");
        refused(&slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "glm-6" })), "models.cheap: the model \"glm-6\" is not in the price book");
        refused(&slot(json!({ "provider": "workers_ai", "model": "@cf/meta/llama-4-scout-17b-16e-instruct" })), "not in the price book");
        refused(r#"{"vision": {"provider": "workers_ai", "model": ""}}"#, "models.vision: the model \"\" is not in the price book");
        for bad in ["http://a.example/v1", "https://a.example:8443/v1", "https://user@a.example/v1", "ftp://a.example/v1", "a.example/v1", "http://127.0.0.1:4810/v1"] {
            refused(&slot(json!({ "provider": "openai", "base_url": bad, "model": "glm-5-3-flash" })), "models.cheap: base_url:");
        }
        for bad in ["https://a.example/v1/", "https://a.example/v1?x=1"] {
            refused(&slot(json!({ "provider": "openai", "base_url": bad, "model": "glm-5-3-flash" })), "without a query or a trailing /");
        }
        refused("[]", "models:");
        // a body: an object of the API's own fields, within its bound, naming none of the route's
        let with_body = |body: Value| slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH, "body": body }));
        for bad in [json!("zdr"), json!([routing()]), json!(null), json!(7)] {
            refused(&with_body(bad.clone()), "models: body is a JSON object of the fields every call to this API carries");
        }
        for key in ROUTE_KEYS {
            refused(&with_body(json!({ key: 1, "provider": { "zdr": true } })), &format!("models.cheap: body names {key}, which the route writes itself"));
        }
        let two = parse(&with_body(json!({ "model": "anthropic/claude-opus-5.5", "messages": [] })), false).unwrap_err();
        assert!(two.contains("messages") && two.contains("model,") && two.contains("which the route writes itself"), "{two}");
        let pad = |n: usize| json!({ "x": "a".repeat(n) });
        let at = UPSTREAM_BODY_MAX_BYTES - serde_json::to_vec(&pad(0)).unwrap().len();
        assert!(parse(&with_body(pad(at)), false).is_ok(), "{UPSTREAM_BODY_MAX_BYTES} bytes of JSON at most");
        refused(&with_body(pad(at + 1)), &format!("models.cheap: body is {} bytes of JSON, past {UPSTREAM_BODY_MAX_BYTES}", UPSTREAM_BODY_MAX_BYTES + 1));
        refused(&slot(json!({ "provider": "workers_ai", "model": GLM_FLASH, "body": routing() })), "unknown field `body`");
        // a local fleet's fake, where its egress may reach local addresses
        let local = slot(json!({ "provider": "openai", "base_url": "http://127.0.0.1:4810/v1", "model": "glm-5-3-flash" }));
        assert_eq!(parse(&local, true).unwrap().cheap, openai("http://127.0.0.1:4810/v1", FINITE_PRIVATE_FLASH));
        assert!(parse(&local, false).is_err(), "a deployment never names one");
        // an id is one model: two slots naming it name one place (a call names it by its id)
        let elsewhere = json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": GLM_FLASH } }).to_string();
        refused(&elsewhere, "models.vision: \"@cf/zai-org/glm-5.3-flash\" is models.cheap's model too, where it runs elsewhere");
        let two_apis = json!({ "cheap": { "provider": "openai", "base_url": "https://a.example/v1", "model": FINITE_PRIVATE_FLASH }, "medium": { "provider": "openai", "base_url": "https://b.example/v1", "model": FINITE_PRIVATE_FLASH } });
        refused(&two_apis.to_string(), "models.medium: \"glm-5-3-flash\" is models.cheap's model too");
        let one_api = json!({ "cheap": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() }, "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": { "provider": { "sort": "price", "max_price": routing()["provider"]["max_price"] } } } });
        assert!(parse(&one_api.to_string(), false).is_ok(), "one id at one place, its bodies its slots' own, at one ceiling");
        // a model the book prices, whichever slot names it
        let mut other = book.clone();
        other.models.retain(|m| m.model != FINITE_PRIVATE_FLASH);
        assert!(Models::parse(Some(FINITE), &other, false).unwrap_err().contains("models.cheap: the model \"glm-5-3-flash\" is not in the price book"));
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
    fn an_openrouter_slot_is_priced_at_its_ceiling() {
        use crate::price::{Priced, BP, OPENROUTER_FEE_BP};
        let defaults = PriceBook::defaults();
        let kept = defaults.models.iter().find(|m| m.model == OPENROUTER_FLASH).map(|m| m.price);
        let tokens = |model: &str, input, cached_input, cache_write, output| Usage::Tokens { model: model.into(), input, cached_input, cache_write, output };
        let at = |model: &str, max_price: Value| {
            let tier = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": model, "body": { "provider": { "zdr": true, "max_price": max_price } } });
            Models::parse(Some(&json!({ "cheap": tier, "medium": tier }).to_string()), &defaults, false)
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
            // an agent's call, most of its prompt cached: dearer than on Workers AI, which prices a cached token at a fifth
            let call = |model: &str| book.price(&tokens(model, 5_000, 25_000, 0, 600)).unwrap().charge;
            assert_eq!((call(model), call(GLM_FLASH)), (7_597, 2_835), "{model}");
        }
        // after the switch, GLM's usage still queued is priced as it was: the code keeps its row
        assert_eq!(row(&deepseek, OPENROUTER_FLASH), kept);
        assert_eq!(deepseek.models.len(), defaults.models.len() + 1);
        assert_eq!(deepseek.price(&tokens(OPENROUTER_FLASH, 26_000, 0, 0, 200)).unwrap(), glm.price(&tokens(OPENROUTER_FLASH, 26_000, 0, 0, 200)).unwrap());
        // a ceiling's cents, rounded up to a micro-dollar a million, never down
        let odd = at("deepseek/deepseek-v4.1-flash", json!({ "prompt": 0.1234567, "completion": 0 })).unwrap().cheap.ceiling().unwrap().unwrap();
        assert_eq!((odd.input, odd.output), (crate::price::openrouter(123_457), 0));
        // a ceiling over the book's own row of that id
        let opus = at("anthropic/claude-opus-5.5", json!({ "prompt": 5, "completion": 25 })).unwrap().priced(defaults.clone());
        assert_eq!(row(&opus, "anthropic/claude-opus-5.5").map(|p| (p.input, p.cached_input, p.output)), Some((crate::price::openrouter(5_000_000), crate::price::openrouter(5_000_000), crate::price::openrouter(25_000_000))));
        assert_eq!(opus.models.len(), defaults.models.len());
        // on another API (the e2e's fake), a ceiling prices it too; without one, the book must
        let fake = |body: Value| json!({ "medium": { "provider": "openai", "base_url": "http://127.0.0.1:4810/v1", "model": "deepseek/deepseek-v4.1-flash", "body": body } }).to_string();
        let local = Models::parse(Some(&fake(routing())), &defaults, true).unwrap().priced(defaults.clone());
        assert_eq!(row(&local, "deepseek/deepseek-v4.1-flash"), row(&glm, OPENROUTER_FLASH));
        assert!(Models::parse(Some(&fake(json!({ "provider": { "zdr": true } }))), &defaults, true).unwrap_err().contains("models.medium: the model \"deepseek/deepseek-v4.1-flash\" is not in the price book"));
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
            Models::parse(Some(&json!({ "cheap": tier }).to_string()), &defaults, false)
        };
        let no_ceiling = "models.cheap: a model on OpenRouter is priced at the ceiling its calls route under: name it in body.provider.max_price";
        refused(bare(None), no_ceiling);
        refused(bare(Some(json!({ "provider": { "zdr": true, "sort": "throughput" } }))), no_ceiling);
        refused(bare(Some(json!({ "transforms": [] }))), no_ceiling);
        // OpenRouter by its host as the URL parses, however it is spelled, even for a model the book prices
        let spelled = |base_url: &str, model: &str| Models::parse(Some(&json!({ "cheap": { "provider": "openai", "base_url": base_url, "model": model } }).to_string()), &defaults, false);
        for base_url in ["https://OPENROUTER.AI/api/v1", "https://openrouter.ai:443/api/v1", "https://OpenRouter.ai:443/api/v1", "https://eu.openrouter.ai/api/v1", "https://EU.OpenRouter.AI/api/v1"] {
            for model in ["anthropic/claude-opus-5.5", OPENROUTER_FLASH] {
                refused(spelled(base_url, model), no_ceiling);
            }
        }
        for elsewhere in ["https://notopenrouter.ai/api/v1", "https://openrouter.ai.example/api/v1"] {
            assert!(spelled(elsewhere, "anthropic/claude-opus-5.5").is_ok(), "{elsewhere} is another API, the book's row its price");
        }
        refused(at(OPENROUTER_FLASH, json!({ "prompt": 0.15 })), "models.cheap: body.provider.max_price.completion is the dollars a million tokens its calls are priced at, from 0 to 1000, not nothing");
        refused(at(OPENROUTER_FLASH, json!({ "completion": 0.5 })), "body.provider.max_price.prompt is the dollars a million tokens");
        for bad in [json!(-0.01), json!("0.15"), json!(1000.5), json!(null), json!({ "usd": 0.15 })] {
            refused(at(OPENROUTER_FLASH, json!({ "prompt": bad, "completion": 0.5 })), &format!("body.provider.max_price.prompt is the dollars a million tokens its calls are priced at, from 0 to 1000, not {bad}"));
        }
        refused(at(OPENROUTER_FLASH, json!(0.15)), "body.provider.max_price.prompt is the dollars a million tokens");
        assert!(at(OPENROUTER_FLASH, json!({ "prompt": 1000, "completion": 0 })).is_ok(), "{CEILING_MAX_USD} at most");
        let two = json!({ "cheap": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() }, "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": { "provider": { "max_price": { "prompt": 0.2, "completion": 0.5 } } } } });
        refused(Models::parse(Some(&two.to_string()), &defaults, false), "models.medium: \"z-ai/glm-5.3-flash\" is models.cheap's model too, at another price");
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &defaults, false);
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let models = parse(json!({ "fallback": tinfoil(listed.clone()) })).unwrap();
        assert!(matches!(&models.fallback, Upstream::Openai { price: Some(p), .. } if Value::Object(p.clone()) == listed), "the fallback names its price: {:?}", models.fallback);
        assert_eq!(serde_json::to_value(&models.fallback).unwrap(), tinfoil(listed.clone()), "its wire form is the config's");
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
        refused(json!({ "cheap": routed }), "models.cheap: a model on OpenRouter is priced at its ceiling (body.provider.max_price), not a price of its own");
        let mut both = tinfoil(listed.clone());
        both["body"] = json!({ "provider": { "max_price": { "prompt": 1, "completion": 2 } } });
        refused(json!({ "fallback": both }), "models.fallback: name its price, or a ceiling (body.provider.max_price), not both");
        for (field, bad) in [("input", json!(-0.01)), ("output", json!(1000.5)), ("cached_input", json!(-1))] {
            let mut p = listed.clone();
            p[field] = bad.clone();
            refused(json!({ "fallback": tinfoil(p) }), &format!("models.fallback: price.{field} is the dollars a million tokens its API lists, from 0 to 1000, not {bad}"));
        }
        refused(json!({ "fallback": tinfoil(json!({ "input": 0.65, "output": 1.45, "cache_write": 0.65 })) }), "models.fallback: price names \"cache_write\": it names input, cached_input (optional) and output");
        refused(json!({ "fallback": tinfoil(json!({ "input": 0.65 })) }), "models.fallback: price.output is the dollars a million tokens its API lists, from 0 to 1000, not nothing");
        refused(json!({ "fallback": tinfoil(json!({ "input": "0.65", "output": 1.45 })) }), "models.fallback: price.input is the dollars a million tokens its API lists, from 0 to 1000, not \"0.65\"");
        refused(json!({ "fallback": tinfoil(json!(0.65)) }), "models: price is a JSON object, {\"input\": <$>, \"cached_input\"?: <$>, \"output\": <$>} a million tokens, not 0.65");
        refused(json!({ "fallback": { "provider": "workers_ai", "model": GLM_FLASH, "price": listed } }), "unknown field `price`");
        // one id is one row: two slots running it name one price
        let dearer = tinfoil(json!({ "input": 0.7, "output": 1.45 }));
        refused(json!({ "fallback": tinfoil(listed.clone()), "vision": dearer }), "models.vision: \"deepseek-v4-1-flash\" is models.fallback's model too, at another price");
        assert!(parse(json!({ "fallback": tinfoil(listed.clone()), "vision": tinfoil(listed.clone()) })).is_ok(), "one id at one price, twice");
        // without one, a model the book does not price is refused, saying what to name
        let bare = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash" });
        refused(json!({ "fallback": bare }), "models.fallback: the model \"deepseek-v4-1-flash\" is not in the price book, which prices");
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
        assert_eq!(DEFAULTS.fallback_of(VISION), None, "DeepSeek V4 Flash, the default fallback, reads no images");
        assert_eq!(DEFAULTS.images, [false; 4], "unsaid, no slot reads images");
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH });
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let mut seeing = tinfoil(listed.clone());
        seeing["images"] = json!(true);
        let tier = parse(json!({ "cheap": private, "medium": private, "fallback": seeing, "vision": private })).unwrap();
        assert_eq!(tier.images, [false, false, true, false]);
        assert_eq!(tier.fallback, parse(json!({ "fallback": tinfoil(listed.clone()) })).unwrap().fallback, "where it runs is as before: images is the slot's");
        assert_eq!(tier.fallback_of(VISION), Some(tier.fallback()));
        // a vision call on Finite Private, failed before answering: once more on Tinfoil
        let mut tries = Tries::new(tier.route(Named::Vision).unwrap(), tier.fallback_of(VISION));
        assert_eq!(tries.after(Tried::Answered(503)).map(|m| m.id()), Some("deepseek-v4-1-flash"));
        assert_eq!(tries.after(Tried::Answered(503)), None, "and no third try");
        // a fallback that says nothing: a vision call has none, a tier's call falls back as ever
        let unsaid = parse(json!({ "cheap": private, "medium": private, "fallback": tinfoil(listed.clone()), "vision": private })).unwrap();
        assert_eq!(unsaid.fallback_of(VISION), None);
        assert_eq!(Tries::new(unsaid.route(Named::Vision).unwrap(), unsaid.fallback_of(VISION)).after(Tried::Answered(503)), None, "its failure is the call's");
        assert!(Tries::new(unsaid.route(Named::Tier(Tier::Cheap)).unwrap(), Some(unsaid.fallback())).after(Tried::Answered(503)).is_some());
        let false_said = parse(json!({ "fallback": { "provider": "workers_ai", "model": GLM_FLASH, "images": false } })).unwrap();
        assert_eq!(false_said.fallback_of(VISION), None, "false is unsaid");
        // on Workers AI too; and GLM-5.3 Flash behind GLM-5.3 Flash's own vision call is no fallback
        let both = parse(json!({ "fallback": { "provider": "workers_ai", "model": GLM_FLASH, "images": true } })).unwrap();
        assert_eq!((both.fallback.clone(), both.fallback_of(VISION).map(|m| m.id())), (workers_ai(GLM_FLASH), Some(GLM_FLASH)));
        assert_eq!(Tries::new(both.route(Named::Vision).unwrap(), both.fallback_of(VISION)).after(Tried::Answered(503)), None, "the same model again is no fallback");
        // refused, saying why
        for bad in [json!("yes"), json!(1), json!(null)] {
            let e = parse(json!({ "fallback": { "provider": "workers_ai", "model": GLM_FLASH, "images": bad } })).unwrap_err();
            assert_eq!(e, format!("models.fallback: images is true (its model reads images) or false, not {bad}"));
        }
        assert!(parse(json!({ "fallback": { "provider": "workers_ai", "model": GLM_FLASH, "seeing": true } })).unwrap_err().contains("unknown field `seeing`"), "the rest is the upstream's, as before");
        assert!(parse(json!([])).unwrap_err().starts_with("models:"), "no object: the slots' own refusal");
    }

    #[test]
    fn native_images_keep_their_capability_and_never_fall_back_to_text_only() {
        let mut models = Models::defaults();
        models.images[0] = true;
        let user = json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,eA=="}}]}]});
        let tool = json!({"messages":[{"role":"tool","content":user["messages"][0]["content"]}]});
        let text = json!({"messages":[{"role":"user","content":"hello"}]});
        assert!(models.fallback_for_input("cheap", &text).is_some(), "existing text fallback");
        assert!(models.fallback_for_input("cheap", &user).is_none());
        assert!(models.fallback_for_input("cheap", &tool).is_none());
        models.images[2] = true;
        assert!(models.fallback_for_input("cheap", &user).is_some());
        assert!(models.fallback_for_input("cheap", &tool).is_some());
        let ids = models.ids_by_name();
        assert_eq!(ids.images.as_ref().unwrap(), &vec![GLM_FLASH.to_string(), DEEPSEEK_FLASH.to_string()]);
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
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
        let flash = json!({ "provider": "workers_ai", "model": GLM_FLASH });
        let tier = parse(json!({ "cheap": mark(private.clone()), "medium": mark(private.clone()), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!((tier.private, tier.images), ([true; 4], [false, false, true, false]));
        assert_eq!(tier.cheap, parse(json!({ "cheap": private })).unwrap().cheap, "where it runs is as before: private is the slot's");
        assert_eq!(DEFAULTS.private, [false; 4]);
        assert!(parse(json!({ "fallback": mark(seeing.clone()) })).is_ok(), "a private fallback behind tiers that are not");
        assert_eq!(openrouter().private, [false; 4], "today's configs say nothing, and parse as before");
        // a private tier's agents reach only private slots: their turns, failures, stalls, images, and
        // Hermes' recovery of a failed vision call to their main model, which is the tier itself
        for t in [Tier::Cheap, Tier::Medium] {
            assert_eq!(tier.reached(t).iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), [t.as_str(), super::FALLBACK, VISION]);
            assert!(tier.reached(t).iter().all(|(slot, _)| tier.slot_private(slot)), "{t:?}");
        }
        // a private tier beside one that is not (Private and Standard): each route its own
        let routed_slot = || json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() });
        let mixed = parse(json!({ "cheap": mark(private.clone()), "medium": routed_slot(), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!(mixed.private, [true, false, true, true]);
        assert!(mixed.reached(Tier::Cheap).iter().all(|(slot, _)| mixed.slot_private(slot)), "Private's chain is private");
        assert!(!mixed.slot_private("medium"), "Standard's agents recover to Standard: theirs, and promised nothing");
        // a private vision and fallback behind tiers that are not (#540's review): parsed, and no route
        // is private, so nothing is promised: a vision call whose fallback fails too may be recovered by
        // Hermes to the agent's own main model, a Workers AI tier, which is that agent's route
        let behind = parse(json!({ "fallback": mark(seeing.clone()), "vision": mark(private.clone()) })).unwrap();
        assert_eq!(behind.private, [false, false, true, true]);
        let recovered: Vec<&str> = behind.reached(Tier::Cheap).iter().map(|(slot, _)| *slot).collect();
        assert_eq!(recovered, ["cheap", super::FALLBACK, VISION]);
        assert!(!behind.slot_private("cheap") && !behind.slot_private("medium"), "no private route: its agents chose none");
        let mut tries = Tries::new(behind.route(Named::Vision).unwrap(), behind.fallback_of(VISION));
        assert_eq!(tries.after(Tried::Answered(429)).map(|m| m.id()), Some("deepseek-v4-1-flash"), "vision's 429: its fallback");
        assert_eq!(tries.after(Tried::Answered(429)), None, "the fallback's 429 too: the call's, for Hermes to recover to the main model");
        // refused, saying which slot to change and why the private tier reaches it
        let open = |tier: &str, slot: &str, why: &str| format!("models.{slot}: models.{tier} is private, and its agents' {why} reach {slot}, so mark a private model there (\"private\": true)");
        refused(json!({ "cheap": mark(private.clone()), "fallback": mark(seeing.clone()) }), &open("cheap", VISION, "images"));
        refused(json!({ "medium": mark(private.clone()), "fallback": mark(seeing.clone()), "vision": private.clone() }), &open("medium", VISION, "images"));
        refused(json!({ "cheap": mark(private.clone()), "medium": mark(private.clone()), "fallback": seeing.clone(), "vision": mark(private.clone()) }), &open("cheap", super::FALLBACK, "failures and stalls"));
        refused(json!({ "medium": mark(private.clone()), "vision": mark(private.clone()) }), &open("medium", super::FALLBACK, "failures and stalls"));
        let not_private = |slot: &str| format!("models.{slot}: Workers AI and OpenRouter are not private: only a confidential API (Finite Private's, Tinfoil's) is marked private");
        refused(json!({ "cheap": mark(private.clone()), "fallback": mark(flash.clone()), "vision": mark(private.clone()) }), &not_private("fallback"));
        let routed = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing() });
        refused(json!({ "cheap": mark(routed), "fallback": mark(seeing.clone()), "vision": mark(private.clone()) }), &not_private("cheap"));
        for bad in [json!("yes"), json!(1)] {
            let mut slot = private.clone();
            slot["private"] = bad.clone();
            refused(json!({ "cheap": slot }), &format!("models.cheap: private is true (its model is private: confidential inference that keeps nothing) or false, not {bad}"));
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
        let private = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true });
        let mut labelled = private.clone();
        labelled["label"] = json!("  Private (recommended) ");
        let standard = json!({ "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "label": "Standard" });
        let mut fallback = tinfoil(json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 }));
        fallback["private"] = json!(true);
        let routes = parse(json!({ "cheap": labelled, "medium": standard, "fallback": fallback, "vision": private })).unwrap();
        assert_eq!(routes.labels, [Some("Private (recommended)".to_string()), Some("Standard".to_string())], "trimmed");
        let bare = json!({ "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH });
        assert_eq!(routes.cheap, parse(json!({ "cheap": bare })).unwrap().cheap, "where it runs is as before: the label is the slot's");
        let view = serde_json::to_value(routes.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(DEFAULTS.labels, [None, None], "unnamed, none: Settings offers the tiers by their names, as before");
        assert!(serde_json::to_value(DEFAULTS.ids_by_name()).unwrap()["routes"]["cheap"].get("label").is_none());
        // refused, saying why
        let refused = |models: Value, says: &str| {
            let e = parse(models.clone()).unwrap_err();
            assert_eq!(e, says, "{models}");
        };
        let flash = |label: Value| json!({ "provider": "workers_ai", "model": GLM_FLASH, "label": label });
        for slot in ["fallback", "vision"] {
            refused(json!({ slot: flash(json!("Backup")) }), &format!("models.{slot}: a label names a route people pick, and those are the tiers (cheap, medium)"));
        }
        let long = "x".repeat(LABEL_MAX_CHARS + 1);
        for bad in [json!(""), json!("   "), json!(long), json!("two\nlines"), json!(7), json!(null)] {
            refused(json!({ "cheap": flash(bad.clone()) }), &format!("models.cheap: label is 1 to {LABEL_MAX_CHARS} characters of plain text, not {bad}"));
        }
        assert!(parse(json!({ "cheap": flash(json!("x".repeat(LABEL_MAX_CHARS))) })).is_ok(), "{LABEL_MAX_CHARS} at most");
    }

    /// Private and Standard as a deployment names them per route (Paul,
    /// 2026-10-24): Private the cheap tier on Finite Private, the route's
    /// four's Tinfoil fallback and Finite Private vision behind it; Standard
    /// the medium tier on OpenRouter, a slot of its own (`cloudflare`, Workers
    /// AI's GLM-5.3 Flash, reading images) its fallback and its vision.
    fn per_route() -> Value {
        json!({
            "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true, "label": "Private (recommended)" },
            "medium": { "provider": "openai", "base_url": "https://openrouter.ai/api/v1", "model": OPENROUTER_FLASH, "body": routing(), "label": "Standard", "fallback": "cloudflare", "vision": "cloudflare" },
            "fallback": { "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": { "input": 0.65, "cached_input": 0.13, "output": 1.45 }, "images": true, "private": true },
            "vision": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true },
            "cloudflare": { "provider": "workers_ai", "model": GLM_FLASH, "images": true }
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
        let m = parse(per_route()).unwrap();
        assert_eq!(m.extra, [Extra { name: "cloudflare".into(), upstream: workers_ai(GLM_FLASH), images: true, private: false, fallback: None, opts: SlotOpts::default() }]);
        assert_eq!(m.routes, [RouteSlots::defaults(), RouteSlots { fallback: "cloudflare".into(), vision: "cloudflare".into(), deep: None }]);
        let (cheap, medium) = (Some(Tier::Cheap), Some(Tier::Medium));
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        // each tier's call falls back within its route
        assert_eq!(m.fallback_of("cheap").map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        assert_eq!(m.fallback_of("medium").map(at), Some(("cloudflare".into(), GLM_FLASH.into())));
        assert_eq!(m.fallback_of("high"), None, "high has no route");
        let mut private = Tries::new(m.route(Named::Tier(Tier::Cheap)).unwrap(), m.fallback_of("cheap"));
        assert_eq!(private.after(Tried::Answered(429)).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        let mut standard = Tries::new(m.route(Named::Tier(Tier::Medium)).unwrap(), m.fallback_of("medium"));
        assert_eq!(standard.after(Tried::Answered(502)).map(at), Some(("cloudflare".into(), GLM_FLASH.into())), "Standard's to Cloudflare, never Tinfoil");
        // a caller's vision and fallback are its route's; one that says no tier, the four's
        assert_eq!(at(m.route_for(Named::Vision, cheap).unwrap()), ("vision".into(), FINITE_PRIVATE_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Vision, medium).unwrap()), ("cloudflare".into(), GLM_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Vision, None).unwrap()), ("vision".into(), FINITE_PRIVATE_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Fallback, medium).unwrap()), ("cloudflare".into(), GLM_FLASH.into()));
        assert_eq!(at(m.route_for(Named::Fallback, None).unwrap()), ("fallback".into(), "deepseek-v4-1-flash".into()));
        assert_eq!(m.capped_for(Named::Vision, medium).unwrap().model.slot, "cloudflare");
        // a vision call falls back as its slot does: the four's vision to the fallback that reads images;
        // Standard's, an extra naming none, has none
        assert_eq!(m.fallback_of(m.route_for(Named::Vision, cheap).unwrap().slot).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        let seen = m.route_for(Named::Vision, medium).unwrap();
        assert_eq!(m.fallback_of(seen.slot), None, "Standard's vision, cloudflare, names no fallback");
        // an extra is named by its model's id, and listed
        assert_eq!(m.named(Some(GLM_FLASH), None), Ok(Named::Slot(4)));
        assert_eq!(at(m.route(Named::Slot(4)).unwrap()), ("cloudflare".into(), GLM_FLASH.into()));
        // an id runs its model, whatever the caller's route; only the aliases follow it (#589's review, P2)
        for (id, slot) in [("deepseek-v4-1-flash", "fallback"), (GLM_FLASH, "cloudflare")] {
            let named = m.named(Some(id), medium).unwrap();
            assert_eq!(at(m.route_for(named, medium).unwrap()), (slot.to_string(), id.to_string()), "{id}, asked by a Standard caller");
            assert_eq!(at(m.route_for(m.named(Some(id), cheap).unwrap(), cheap).unwrap()), (slot.to_string(), id.to_string()), "{id}, by a Private one");
        }
        let mut seeing = per_route();
        seeing["vision"] = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "gemma4-31b", "price": { "input": 0.4, "output": 1 }, "private": true });
        let seeing = parse(seeing).unwrap();
        let named = seeing.named(Some("gemma4-31b"), medium).unwrap();
        assert_eq!((named, at(seeing.route_for(named, medium).unwrap())), (Named::Slot(3), ("vision".into(), "gemma4-31b".into())), "the vision model's own id, from a Standard caller: that model, not Standard's vision");
        for (alias, slot, id) in [("fallback", "cloudflare", GLM_FLASH), ("vision", "cloudflare", GLM_FLASH)] {
            let named = m.named(Some(alias), medium).unwrap();
            assert_eq!(at(m.route_for(named, medium).unwrap()), (slot.to_string(), id.to_string()), "the alias {alias} follows Standard's route");
        }
        assert_eq!(m.ids(), [FINITE_PRIVATE_FLASH, OPENROUTER_FLASH, "deepseek-v4-1-flash", GLM_FLASH]);
        assert_eq!(Named::Slot(0).as_str(), "slot");
        // the view: each tier's route by its slots' ids
        let view = serde_json::to_value(m.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": GLM_FLASH, "vision": GLM_FLASH }));
        // the guard walks each route: Private's is private, Standard's its own
        assert_eq!(m.reached(Tier::Medium).iter().map(|(slot, _)| *slot).collect::<Vec<_>>(), ["medium", "cloudflare", "cloudflare", "fallback"]);
        assert!(m.reached(Tier::Cheap).iter().all(|(slot, _)| m.slot_private(slot)));
        assert!(!m.slot_private("cloudflare") && m.slot_private("fallback"));
        // today's configs name no routes, and run as before
        assert_eq!(openrouter().routes, [RouteSlots::defaults(), RouteSlots::defaults()]);
        assert!(openrouter().extra.is_empty());
        assert_eq!(openrouter().fallback_of("medium"), Some(openrouter().fallback()));
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
        refused(with(&|v| v["medium"]["fallback"] = json!("cloudflair")), "models.medium: its fallback is \"cloudflair\", which is no other slot of the deployment's");
        refused(with(&|v| v["medium"]["vision"] = json!("cheap")), "models.medium: its vision is \"cheap\", which is no slot it may name");
        refused(with(&|v| v["medium"]["fallback"] = json!(7)), "models.medium: fallback names the slot its route's fallback runs on, not 7");
        refused(with(&|v| v["spare"] = json!({ "provider": "workers_ai", "model": GLM_FLASH })), "models.spare: no slot names it, as its fallback or a tier's vision");
        refused(with(&|v| v["high"] = json!({ "provider": "workers_ai", "model": GLM_FLASH })), "models.high: the high tier is off");
        refused(with(&|v| v["Cloud"] = json!({ "provider": "workers_ai", "model": GLM_FLASH })), "models: \"Cloud\" is no slot's name");
        refused(with(&|v| v["cloudflare"]["label"] = json!("Cloudflare")), "models.cloudflare: only a tier (cheap, medium) names its label");
        refused(with(&|v| v["cloudflare"]["vision"] = json!("fallback")), "models.cloudflare: only a tier (cheap, medium) names its vision");
        refused(with(&|v| v["cloudflare"]["images"] = json!("yes")), "models.cloudflare: images is true (its model reads images) or false, not \"yes\"");
        refused(with(&|v| v["cloudflare"]["private"] = json!(true)), "models.cloudflare: Workers AI and OpenRouter are not private");
        refused(with(&|v| v["cloudflare"]["provider"] = json!("anthropic")), "models.cloudflare: unknown variant `anthropic`");
        refused(with(&|v| { for n in ["a1", "a2", "a3", "a4"] { v[n] = json!({ "provider": "workers_ai", "model": GLM_FLASH }); } }), &format!("models: at most {EXTRA_SLOTS_MAX} slots beyond"));
        refused(with(&|v| v["cheap"]["fallback"] = json!("cloudflare")), "models.cloudflare: models.cheap is private, and its agents' failures and stalls reach cloudflare");
        refused(with(&|v| v["cheap"]["vision"] = json!("cloudflare")), "models.cloudflare: models.cheap is private, and its agents' images reach cloudflare");
        // P1 (#589's review): a private route of its own slots, the built-in fallback public: an older
        // image's stall names that fallback by its id, its tier unsaid, so it is refused
        let own = with(&|v| {
            v["cheap"]["fallback"] = json!("private_fallback");
            v["cheap"]["vision"] = json!("private_vision");
            v["private_fallback"] = v["fallback"].clone();
            v["private_vision"] = v["vision"].clone();
            v["fallback"] = json!({ "provider": "workers_ai", "model": DEEPSEEK_FLASH });
            v["vision"] = json!({ "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3" });
        });
        refused(own.clone(), "models.fallback: models.cheap is private, and its agents' stalls on an older image (its tier unsaid) reach fallback");
        let mut kept = own;
        kept["fallback"] = kept["private_fallback"].clone();
        kept["cheap"]["fallback"] = json!("fallback");
        kept.as_object_mut().unwrap().remove("private_fallback");
        let kept = parse(kept).unwrap();
        let stall = kept.named(Some("deepseek-v4-1-flash"), None).unwrap();
        assert!(kept.slot_private(kept.route_for(stall, None).unwrap().slot), "an older image's stall, its tier unsaid: the built-in fallback, private");
        // P3: one endpoint, model and body, its price spelled two ways, is one call: no second try
        let tinfoil_slot = |price: Value| json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": price });
        let twice = parse(json!({ "cheap": tinfoil_slot(json!({ "input": 0.65, "output": 1.45 })), "fallback": tinfoil_slot(json!({ "input": 0.65, "cached_input": 0.65, "output": 1.45 })) })).unwrap();
        assert_ne!(twice.cheap, twice.fallback, "spelled two ways");
        assert_eq!(Tries::new(twice.route(Named::Tier(Tier::Cheap)).unwrap(), twice.fallback_of("cheap")).after(Tried::Answered(429)), None, "the same call again is no fallback");
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        let gemma = json!({ "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "gemma4-31b", "price": { "input": 0.4, "output": 1 } });
        let listed = json!({ "input": 0.65, "cached_input": 0.13, "output": 1.45 });
        let mut seeing = tinfoil(listed.clone());
        seeing["images"] = json!(true);
        // the four's vision on a model of its own, a fallback that reads images behind it
        let m = parse(json!({ "vision": gemma, "fallback": seeing })).unwrap();
        let by_id = m.named(Some("gemma4-31b"), None).unwrap();
        assert_eq!(by_id, Named::Slot(3), "its id names its slot");
        for named in [by_id, Named::Vision] {
            let model = m.route(named).unwrap();
            assert_eq!(m.fallback_of(model.slot).map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())), "{named:?}");
            let mut tries = Tries::new(model, m.fallback_of(model.slot));
            assert_eq!(tries.after(Tried::Answered(429)).map(|f| f.id()), Some("deepseek-v4-1-flash"), "{named:?}: a 429, once more on the fallback");
            assert_eq!(tries.after(Tried::Answered(429)), None, "and no third try");
        }
        // control: a fallback that reads no images is neither's
        let blind = parse(json!({ "vision": gemma, "fallback": tinfoil(listed.clone()) })).unwrap();
        for named in [blind.named(Some("gemma4-31b"), None).unwrap(), Named::Vision] {
            assert_eq!(blind.fallback_of(blind.route(named).unwrap().slot), None, "{named:?}");
        }
        // the four's fallback has none, by its alias or its id; a tier's, its route's
        assert_eq!(m.fallback_of(m.route(Named::Fallback).unwrap().slot), None);
        assert_eq!(m.fallback_of(m.route(m.named(Some("deepseek-v4-1-flash"), None).unwrap()).unwrap().slot), None);
        assert_eq!(m.fallback_of("cheap").map(at), Some(("fallback".into(), "deepseek-v4-1-flash".into())));
        // an extra names its own: Standard's vision, Cloudflare, behind it the fallback, which reads images
        let mut own = per_route();
        own["cloudflare"]["fallback"] = json!("fallback");
        let own = parse(own).unwrap();
        assert_eq!(own.extra[0].fallback.as_deref(), Some("fallback"));
        for named in [Named::Vision, own.named(Some(GLM_FLASH), None).unwrap()] {
            let model = own.route_for(named, Some(Tier::Medium)).unwrap();
            assert_eq!((model.slot, own.fallback_of(model.slot).map(at)), ("cloudflare", Some(("fallback".into(), "deepseek-v4-1-flash".into()))), "{named:?}");
        }
        assert_eq!(own.fallback_of("medium").map(at), Some(("cloudflare".into(), GLM_FLASH.into())), "a tier's call is made once more on its route's fallback, and no third time");
        // the four's fallback and vision name theirs
        let named = parse(json!({ "fallback": { "provider": "workers_ai", "model": DEEPSEEK_FLASH, "fallback": "medium" }, "vision": { "provider": "workers_ai", "model": GLM_FLASH, "fallback": "spare" }, "spare": { "provider": "workers_ai", "model": "@cf/zai-org/glm-5.3", "images": true } })).unwrap();
        assert_eq!(named.fallbacks, [Some("medium".to_string()), Some("spare".to_string())]);
        assert_eq!((named.fallback_of(super::FALLBACK).map(at), named.fallback_of(VISION).map(at)), (Some(("medium".into(), MEDIUM_MODEL.into())), Some(("spare".into(), MEDIUM_MODEL.into()))));
        assert_eq!(DEFAULTS.fallbacks, [None, None], "unnamed, as before");
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
        refused(with(&|v| v["cloudflare"]["fallback"] = json!("cloudflare")), "models.cloudflare: its fallback is \"cloudflare\", which is no other slot of the deployment's");
        refused(with(&|v| v["vision"]["fallback"] = json!("nowhere")), "models.vision: its fallback is \"nowhere\", which is no other slot of the deployment's");
        refused(with(&|v| v["fallback"]["fallback"] = json!(3)), "models.fallback: fallback names the slot its calls fall back to, not 3");
        refused(with(&|v| v["cloudflare"]["fallback"] = json!("medium")), "models.cloudflare: images go to cloudflare, so its fallback reads them too: medium says no \"images\": true");
        refused(with(&|v| v["vision"]["fallback"] = json!("cheap")), "models.vision: images go to vision, so its fallback reads them too: cheap says no \"images\": true");
        // the private guard follows a slot's own fallback: Private's fallback, Tinfoil, behind it Cloudflare
        refused(with(&|v| v["fallback"]["fallback"] = json!("cloudflare")), "models.cloudflare: models.cheap is private, and its agents' failures reach cloudflare");
        // and a stall's call, by its fallback's id with its tier unsaid, to the first slot running that id
        let stalled = json!({
            "cheap": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true, "fallback": "private_fallback", "vision": "private_vision" },
            "fallback": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true },
            "vision": tinfoil(listed.clone()),
            "private_fallback": { "provider": "openai", "base_url": "https://inference.tinfoil.sh/v1", "model": "deepseek-v4-1-flash", "price": listed, "private": true },
            "private_vision": { "provider": "openai", "base_url": "https://finite-private.finite.containers.tinfoil.dev/v1", "model": FINITE_PRIVATE_FLASH, "private": true }
        });
        refused(stalled, "models.vision: models.cheap is private, and its agents' stalls (by its model's id) reach vision");
    }

    /// Private and Standard as Fast and Deep (Paul, 2026-10-27): each tier's
    /// own slot its Fast (GLM-5.3 Flash at `high`), the slot its `"deep"`
    /// names its Deep (DeepSeek V4.1 Flash at `max`, 32,768 tokens a call);
    /// within Private the two fall back to each other, Standard's both to
    /// Cloudflare's GLM-5.3 Flash.
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
            "cloudflare": { "provider": "workers_ai", "model": GLM_FLASH, "images": true }
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
        let parse = |models: Value| Models::parse(Some(&models.to_string()), &book, false);
        let at = |model: Model<'_>| (model.slot.to_string(), model.id().to_string());
        let m = parse(fast_and_deep()).unwrap();
        let (cheap, medium) = (Some(Tier::Cheap), Some(Tier::Medium));
        // each tier's Deep, and the effort each mode thinks at
        assert_eq!((m.deep_of(Tier::Cheap).map(at), m.deep_of(Tier::Medium).map(at)), (Some(("private_deep".into(), "deepseek-v4-1-flash".into())), Some(("standard_deep".into(), "deepseek/deepseek-v4.1-flash".into()))));
        assert_eq!(m.deep_of(Tier::High), None);
        assert_eq!((m.opts("cheap"), m.opts("private_deep")), (SlotOpts { reasoning: Some("high"), max_tokens: None }, SlotOpts { reasoning: Some("max"), max_tokens: Some(32_768) }));
        // a delegate on Private names its Deep's id: its Deep, at its cap, falling back to Private's Fast
        let named = m.named(Some("deepseek-v4-1-flash"), cheap).unwrap();
        let capped = m.capped_for(named, cheap).unwrap();
        assert_eq!((at(capped.model), capped.max_tokens), (("private_deep".into(), "deepseek-v4-1-flash".into()), 32_768));
        assert_eq!(m.fallback_of(capped.model.slot).map(at), Some(("cheap".into(), FINITE_PRIVATE_FLASH.into())), "Deep falls back to Fast");
        assert_eq!(m.fallback_of("cheap").map(at), Some(("private_deep".into(), "deepseek-v4-1-flash".into())), "and Fast to Deep");
        let b = bound(capped, json!({ "model": "deepseek-v4-1-flash", "messages": [{ "role": "user", "content": "build it" }], "reasoning_effort": "max", "max_tokens": 60_000 }), true).unwrap();
        assert_eq!((b.input["reasoning_effort"].clone(), b.max_tokens), (json!("max"), 32_768), "max as sent, at Deep's cap");
        let mut tries = Tries::new(capped.model, m.fallback_of(capped.model.slot));
        assert_eq!(tries.after(Tried::Answered(503)).map(at), Some(("cheap".into(), FINITE_PRIVATE_FLASH.into())));
        // the same id, tier unsaid (an older image's stall): the first slot running it, the four's fallback
        assert_eq!(m.named(Some("deepseek-v4-1-flash"), None), Ok(Named::Slot(2)));
        // a delegate on Standard: its Deep on OpenRouter, falling back to Cloudflare
        let named = m.named(Some("deepseek/deepseek-v4.1-flash"), medium).unwrap();
        let capped = m.capped_for(named, medium).unwrap();
        assert_eq!((at(capped.model), capped.max_tokens, m.fallback_of(capped.model.slot).map(at)), (("standard_deep".into(), "deepseek/deepseek-v4.1-flash".into()), 32_768, Some(("cloudflare".into(), GLM_FLASH.into()))));
        // Fast's turns as before: the tier, at the cap of every call
        assert_eq!(m.capped_for(Named::Tier(Tier::Medium), medium).unwrap().max_tokens, MAX_TOKENS);
        assert!(m.ids().contains(&"deepseek/deepseek-v4.1-flash"));
        // the view: each tier's modes
        let view = serde_json::to_value(m.ids_by_name()).unwrap();
        assert_eq!(view["routes"]["cheap"], json!({ "label": "Private (recommended)", "private": true, "fallback": "deepseek-v4-1-flash", "vision": FINITE_PRIVATE_FLASH, "reasoning": "high", "deep": { "id": "deepseek-v4-1-flash", "reasoning": "max", "fallback": FINITE_PRIVATE_FLASH } }));
        assert_eq!(view["routes"]["medium"], json!({ "label": "Standard", "private": false, "fallback": GLM_FLASH, "vision": GLM_FLASH, "reasoning": "high", "deep": { "id": "deepseek/deepseek-v4.1-flash", "reasoning": "max", "fallback": GLM_FLASH } }));
        // today's configs: no modes, nothing new in the view
        assert_eq!((DEFAULTS.deep_of(Tier::Cheap), DEFAULTS.opts("cheap")), (None, SlotOpts::default()));
        let plain = serde_json::to_value(DEFAULTS.ids_by_name()).unwrap();
        assert!(plain["routes"]["cheap"].get("deep").is_none() && plain["routes"]["cheap"].get("reasoning").is_none());
        // the guard walks Private's Deep
        assert!(m.reached(Tier::Cheap).iter().any(|(slot, why)| *slot == "private_deep" && *why == "delegates"));
        assert!(m.reached(Tier::Cheap).iter().all(|(slot, _)| m.slot_private(slot)));
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
        refused(with(&|v| v["medium"]["deep"] = json!("nowhere")), "models.medium: its deep is \"nowhere\", which is no other slot of the deployment's");
        refused(with(&|v| v["medium"]["deep"] = json!("medium")), "models.medium: its deep is \"medium\", which is no other slot");
        refused(with(&|v| v["medium"]["deep"] = json!(true)), "models.medium: deep names the slot its route's Deep runs on, not true");
        refused(with(&|v| v["cheap"]["reasoning"] = json!("medium")), "models.cheap: reasoning is low, high, max, not \"medium\"");
        refused(with(&|v| v["vision"]["reasoning"] = json!("high")), "models.vision: reasoning is a mode's, its tier's own or the slot a tier names as its deep");
        refused(with(&|v| v["cloudflare"]["reasoning"] = json!("max")), "models.cloudflare: reasoning is a mode's");
        for bad in [json!(0), json!(MAX_TOKENS_MAX + 1), json!("lots"), json!(-1)] {
            refused(with(&|v| v["standard_deep"]["max_tokens"] = bad.clone()), &format!("models.standard_deep: max_tokens is the most one call of it writes, 1 to {MAX_TOKENS_MAX}, not {bad}"));
        }
        refused(with(&|v| v["private_deep"]["private"] = json!(false)), "models.private_deep: models.cheap is private, and its agents' failures and stalls reach private_deep");
        refused(with(&|v| v["private_deep"]["fallback"] = json!("cloudflare")), "models.cheap is private, and its agents' stalls (by its model's id) reach cloudflare");
        refused(
            with(&|v| {
                v["cheap"]["deep"] = json!("standard_deep");
                v["private_deep"].as_object_mut().unwrap().remove("reasoning");
            }),
            "models.standard_deep: models.cheap is private, and its agents' delegates reach standard_deep",
        );
        assert!(parse(with(&|v| v["cheap"]["max_tokens"] = json!(MAX_TOKENS_MAX))).is_ok(), "a cap of {MAX_TOKENS_MAX} at most, on any slot");
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

    /// Goal: the deployment's vision model is GLM-5.3 Flash unless it names
    /// another, and one the price book does not price is refused (a ledger
    /// would charge none of its calls). Method: the default, a priced
    /// model named, and unpriced ones.
    #[test]
    fn the_vision_model_is_one_the_book_prices() {
        let book = PriceBook::defaults();
        let vision = |text: &str| Models::parse(Some(text), &book, false).map(|m| m.vision.model().to_string());
        assert_eq!(Models::defaults().vision, workers_ai(GLM_FLASH));
        assert_eq!(VISION_MODEL_DEFAULT, "@cf/zai-org/glm-5.3-flash");
        assert_ne!(VISION_MODEL_DEFAULT, FALLBACK_MODEL_DEFAULT, "the fallback reads no images");
        assert_eq!(vision(&json!({ "vision": { "provider": "workers_ai", "model": MEDIUM_MODEL } }).to_string()).as_deref(), Ok(MEDIUM_MODEL));
        let unpriced = vision(&json!({ "vision": { "provider": "workers_ai", "model": "@cf/meta/llama-4-scout-17b-16e-instruct" } }).to_string()).unwrap_err();
        assert!(unpriced.contains("models.vision") && unpriced.contains("not in the price book") && unpriced.contains(GLM_FLASH), "{unpriced}");
        assert_eq!(finite().vision, workers_ai(GLM_FLASH), "Finite's deployment keeps vision on Workers AI (decision 61)");
        // priced at Flash's prices
        let flash = book.models.iter().find(|m| m.model == VISION_MODEL_DEFAULT).unwrap();
        assert_eq!((flash.price.input, flash.price.cached_input, flash.price.output), (150_000, 30_000, 500_000));
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

    /// Goal: a tier picks its model; `high` and model ids are refused.
    /// Method: every tier name, and names that are not.
    #[test]
    fn tiers_pick_models() {
        assert_eq!(tier(Tier::Cheap).model.id(), "@cf/zai-org/glm-5.3-flash", "GLM-5.3 Flash, DeepSeek its fallback (Paul, 2026-10-09)");
        assert_eq!(tier(Tier::Cheap).model.id(), CHEAP_MODEL);
        assert_eq!(tier(Tier::Medium).model.id(), MEDIUM_MODEL);
        assert_eq!(DEFAULTS.capped(Named::Tier(Tier::High)), Err(Refusal::HighOff));
        assert!(Refusal::HighOff.message().contains("off until Cloudflare raises"));
        assert_eq!(tier_named(None), Ok(Tier::Cheap));
        assert_eq!(tier_named(Some("medium")), Ok(Tier::Medium));
        assert_eq!(tier_named(Some("@cf/zai-org/glm-5.3")), Err(Refusal::UnknownTier));
        assert_eq!(tier_named(Some("z-ai/glm-5.3-flashx")), Err(Refusal::UnknownTier));
        // the book prices every model a tier runs
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
        let b = bound(tier(Tier::Medium), body, false).unwrap();
        assert_eq!((b.model.id(), b.max_tokens, b.stream), (MEDIUM_MODEL, MAX_TOKENS, false));
        assert_eq!(
            b.input,
            json!({
                "messages": [{ "role": "user", "content": "hi" }], "tools": [{ "type": "function" }],
                "max_tokens": MAX_TOKENS, "reasoning_effort": "low", "stream": false,
            }),
            "no model, the cap, the effort clamped, no stream options when not streaming"
        );
        let b = bound(tier(Tier::Cheap), json!({ "messages": [{}], "max_tokens": 64, "reasoning_effort": "high" }), true).unwrap();
        assert_eq!((b.model.id(), b.max_tokens), (CHEAP_MODEL, 64), "what it asked for, under the cap");
        assert_eq!(b.input["reasoning_effort"], "high");
        assert_eq!((b.input["stream"].clone(), b.input["stream_options"].clone()), (json!(true), json!({ "include_usage": true })));
        let b = bound(tier(Tier::Cheap), json!({ "messages": [{}], "max_completion_tokens": 32 }), false).unwrap();
        assert_eq!((b.max_tokens, b.input.get("max_completion_tokens")), (32, None), "either name, sent as max_tokens");
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": [{}] }), false).unwrap().max_tokens, MAX_TOKENS, "none asked: the cap");
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
            let workers_ai = bound(tier(Tier::Cheap), call(asked), true).unwrap();
            assert_eq!(workers_ai.input["reasoning_effort"], sent, "Workers AI, asked {asked:?}");
            let capped = routed.capped(Named::Tier(Tier::Cheap)).unwrap();
            let Upstream::Openai { model, body, .. } = capped.model.upstream.clone() else { panic!("OpenRouter's tier is an OpenAI-compatible API's") };
            let b = bound(capped, call(asked), true).unwrap();
            assert_eq!(openai_request(&b.input, &model, body.as_ref())["reasoning_effort"], sent, "OpenRouter, asked {asked:?}");
        }
    }

    #[test]
    fn a_request_out_of_shape_is_refused() {
        let msgs = || json!([{ "role": "user", "content": "hi" }]);
        assert_eq!(DEFAULTS.capped(Named::Tier(Tier::High)).and_then(|t| bound(t, json!({ "messages": msgs() }), false)), Err(Refusal::HighOff));
        assert_eq!(bound(tier(Tier::Cheap), json!([1]), false), Err(Refusal::NotAnObject));
        assert_eq!(bound(tier(Tier::Cheap), json!({}), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": [] }), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": "hi" }), false), Err(Refusal::NoMessages));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": msgs(), "max_tokens": 0 }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": msgs(), "max_tokens": "9" }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": msgs(), "max_completion_tokens": -1 }), false), Err(Refusal::MaxTokens));
        assert_eq!(bound(tier(Tier::Cheap), json!({ "messages": msgs(), "stream": "yes" }), false), Err(Refusal::Stream));
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
        let b = bound(tier(Tier::Medium), text_body(&t).unwrap(), true).unwrap();
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

    /// Goal: the deployment's fallback model is DeepSeek V4 Flash unless it
    /// names another, another model than the cheap tier's, priced at Workers
    /// AI's list price; one the book does not price is refused, as a vision
    /// model is. Method: the default, a priced model named, and unpriced
    /// ones.
    #[test]
    fn the_fallback_model_is_one_the_book_prices() {
        let book = PriceBook::defaults();
        let fallback = |text: &str| Models::parse(Some(text), &book, false).map(|m| m.fallback.model().to_string());
        assert_eq!(Models::defaults().fallback, workers_ai("@cf/deepseek-ai/deepseek-v4-flash-0731"));
        assert_ne!(FALLBACK, CHEAP_MODEL, "a fallback is another model");
        assert_eq!(fallback(&json!({ "fallback": { "provider": "workers_ai", "model": MEDIUM_MODEL } }).to_string()).as_deref(), Ok(MEDIUM_MODEL));
        let unpriced = fallback(&json!({ "fallback": { "provider": "workers_ai", "model": "anthropic/claude-haiku-4.5" } }).to_string()).unwrap_err();
        assert!(unpriced.contains("models.fallback: the model \"anthropic/claude-haiku-4.5\" is not in the price book") && unpriced.contains(FALLBACK), "{unpriced}");
        // both priced already: the cheap tier's GLM, its DeepSeek fallback
        let p = |model: &str| book.models.iter().find(|m| m.model == model).unwrap().price;
        let (d, g) = (p(FALLBACK), p(CHEAP_MODEL));
        assert_eq!((d.input, d.cached_input, d.cache_write, d.output), (440_000, 14_000, 440_000, 1_320_000));
        assert_eq!((g.input, g.cached_input, g.cache_write, g.output), (150_000, 30_000, 150_000, 500_000));
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
        let routed = openrouter().cheap;
        let flash = workers_ai(GLM_FLASH);
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
            let cheap = models.route(Named::Tier(Tier::Cheap)).unwrap();
            let medium = models.route(Named::Tier(Tier::Medium)).unwrap();
            let fallback = models.fallback();
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
        let mut t = Tries::new(Model { named: Named::Tier(Tier::Cheap), upstream: &private, slot: "cheap" }, Some(Model { named: Named::Fallback, upstream: &other, slot: "fallback" }));
        assert_eq!(t.after(Tried::Answered(503)).map(|m| m.upstream), Some(&other));
    }

    /// Goal: a call that fell back is charged from the usage the fallback
    /// reported, priced as the fallback's. Method: the fallback's usage for
    /// a 30,000-token turn, priced with the default book, then Finite's.
    #[test]
    fn a_fallen_back_call_is_charged_as_the_fallbacks() {
        let book = PriceBook::defaults();
        let b = bound(tier(Tier::Cheap), json!({ "messages": [{ "role": "user", "content": "hi" }] }), true).unwrap();
        let mut t = Tries::new(b.model, Some(DEFAULTS.fallback()));
        assert_eq!(t.after(Tried::Answered(502)).map(|m| m.id()), Some(FALLBACK));
        let used = usage_of(t.model().id(), &json!({ "prompt_tokens": 30_000, "completion_tokens": 400, "prompt_tokens_details": { "cached_tokens": 0 } })).unwrap();
        assert_eq!(used, Usage::Tokens { model: FALLBACK.into(), input: 30_000, cached_input: 0, cache_write: 0, output: 400 });
        // 30,000 in and 400 out at $0.44 and $1.32 a million: $0.013728
        assert_eq!(book.price(&used).unwrap().list, 13_728);

        // Finite's: Finite Private's call, fallen back to Workers AI's GLM-5.3 Flash, at one price
        let finite = finite();
        let b = bound(finite.capped(Named::Tier(Tier::Cheap)).unwrap(), json!({ "messages": [{ "role": "user", "content": "hi" }] }), true).unwrap();
        let mut t = Tries::new(b.model, Some(finite.fallback()));
        assert_eq!(t.after(Tried::Answered(429)).map(|m| m.id()), Some(GLM_FLASH));
        let usage = json!({ "prompt_tokens": 30_000, "completion_tokens": 400 });
        let (on_private, fell) = (usage_of(b.model.id(), &usage).unwrap(), usage_of(t.model().id(), &usage).unwrap());
        assert_eq!(book.price(&on_private).unwrap(), book.price(&fell).unwrap(), "a person pays the same whichever answered (decision 61)");
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
        let tier = models.capped(Named::Tier(Tier::Cheap)).unwrap();
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
        let sent = openai_request(&b.input, model, body.as_ref());
        let mut want = kept.as_object().cloned().unwrap();
        want.extend([("max_tokens".into(), json!(99)), ("reasoning_effort".into(), json!("low")), ("stream".into(), json!(true)), ("stream_options".into(), json!({ "include_usage": true }))]);
        want.insert("provider".into(), routing()["provider"].clone());
        want.insert("model".into(), json!(OPENROUTER_FLASH));
        assert_eq!(sent, Value::Object(want.clone()), "the client's chat fields, the route's bounds, the deployment's routing");
        assert_eq!(sent["provider"]["max_price"], json!({ "prompt": 0.15, "completion": 0.5 }));
        // no body: the same fields, no routing, and still none of the client's extras
        let plain = openai_request(&b.input, FINITE_PRIVATE_FLASH, None);
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
        let step = bound(models.capped(Named::Tier(Tier::Medium)).unwrap(), text_body(&t).unwrap(), true).unwrap();
        let sent = openai_request(&step.input, OPENROUTER_FLASH, body.as_ref());
        assert_eq!((sent["provider"].clone(), sent["messages"][0]["content"].clone(), sent["model"].clone()), (routing()["provider"].clone(), json!("the notes"), json!(OPENROUTER_FLASH)));
        assert_eq!(sent.as_object().unwrap().len(), step.input.as_object().unwrap().len() + 2, "a step sends nothing the allowlist drops");
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
        let routed = &openrouter().cheap;
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
        let mut s = Stream::default().on(&workers_ai(GLM_FLASH));
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
        let unnamed = Models::parse(Some("{}"), &book, false).unwrap();
        assert_eq!(unnamed.embeddings.space().model, crate::embed::MODEL_DEFAULT);
        assert!(unnamed.priced(book.clone()).models.iter().any(|m| m.model == crate::embed::MODEL_DEFAULT));
        let local = r#"{"embeddings": {"provider": "openai", "base_url": "http://127.0.0.1:18091/v1", "model": "BAAI/bge-m3", "dimensions": 1024, "version": "1", "price": {"input": 0.01, "output": 0}}}"#;
        let m = Models::parse(Some(local), &book, true).unwrap();
        assert_eq!(m.embeddings.space().version, Some("1"));
        let row = m.priced(book.clone()).models.into_iter().find(|r| r.model == "BAAI/bge-m3").expect("its own price is its row");
        assert_eq!(row.price.input, 10_000);
        assert!(m.all_slots().iter().all(|(slot, _)| *slot != crate::embed::EMBEDDINGS), "no chat slot");
        for (config, why) in [
            (local, "base_url"),
            (r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/zai-org/glm-5.3-flash", "dimensions": 1024}}"#, "a chat model"),
            (r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/unknown/embedder", "dimensions": 768}}"#, "price book"),
        ] {
            let err = Models::parse(Some(config), &book, false).unwrap_err();
            assert!(err.contains("models.embeddings") && err.contains(why), "{err}");
        }
        let err = Models::parse(Some(r#"{"embeddings": {"provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "private": true}}"#), &book, false).unwrap_err();
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
        let finite = Models::parse(Some(&v.to_string()), &book, false).unwrap();
        assert!(finite.embeddings_refused().is_some_and(|why| why.contains("not private")), "Private's tier is private");
        v["embeddings"] = json!({ "provider": "openai", "base_url": "https://embed.example/v1", "model": "BAAI/bge-m3", "dimensions": 1024, "private": true, "price": { "input": 0, "output": 0 } });
        let private = Models::parse(Some(&v.to_string()), &book, false).unwrap();
        assert_eq!(private.embeddings_refused(), None);
    }

    /// Goal: a deployment whose tiers are private makes no videos, as it
    /// embeds nothing on a slot that is not private: no catalog video model
    /// is. Method: the defaults (no private tier) make them; Finite's shape
    /// (Private's tier private) refuses them, saying why, whatever its
    /// embeddings slot.
    #[test]
    fn a_private_deployment_makes_no_videos() {
        let book = PriceBook::defaults();
        assert_eq!(Models::defaults().video_refused(), None);
        let mut v = per_route();
        let finite = Models::parse(Some(&v.to_string()), &book, false).unwrap();
        assert!(finite.video_refused().is_some_and(|why| why.contains("not private")), "Private's tier is private");
        v["embeddings"] = json!({ "provider": "openai", "base_url": "https://embed.example/v1", "model": "BAAI/bge-m3", "dimensions": 1024, "private": true, "price": { "input": 0, "output": 0 } });
        assert!(Models::parse(Some(&v.to_string()), &book, false).unwrap().video_refused().is_some(), "a private embeddings slot makes no private video model");
    }
}
