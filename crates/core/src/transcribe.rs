//! The model route's transcriptions (decision 9: a voice memo is one the
//! agent transcribes itself; docs/api.md, Models): OpenAI's `POST
//! /v1/audio/transcriptions` shape, a `multipart/form-data` body
//! (crate::multipart) with the audio as `file` and `model` the route's
//! name `whisper`, run on the deployment's transcription slot (`Slot`, its
//! config's `models.transcription`), and metered at the length of audio its
//! model heard, at the slot's own price. Pure: the cell reads the body,
//! makes the call and meters it (cell/src/models.rs).
//!
//! The slot names where its model runs, as a chat slot does (Paul,
//! 2026-11-24: "we just need a way to point at local models as desired by
//! the self-hoster"):
//! - a Workers AI model (`workers_ai`), through the deployment's AI
//!   Gateway: sent Whisper's catalog input (`workers_ai_input`), its
//!   answer's `transcription_info.duration` the length, priced in the
//!   neurons a minute its slot names (`price.neurons_a_minute`, Workers
//!   AI's own unit for speech recognition);
//! - a model on an OpenAI-compatible API (`openai`): the audio sent on in
//!   OpenAI's own form (`openai_form`) to `<base_url>/audio/transcriptions`
//!   with the slot's key, asked for `verbose_json`, its answer's
//!   `usage.seconds` (OpenAI's duration usage) or `duration` the length,
//!   priced at the dollars a minute its slot names (`price.audio_minute`),
//!   by the second, rounded up: as OpenAI's API bills transcription (a
//!   minute's price, "rounded to the nearest second"), so a self-hosted
//!   slot at zero and a vendor's at its list both read as the vendor's
//!   price page does.
//!
//! The code names no model: a deployment that names no slot transcribes
//! nothing (the route answers 404, saying so). A deploy of a config that
//! names none adds Workers AI's Whisper for now (xtask/src/deploy.rs;
//! docs/technical-debt-ledger.md).
//!
//! A call is charged after its answer at the length its model reports
//! (`usage_of`; docs/ledger.md), nothing held before it. A first try that
//! fails fast for want of capacity is made once more (`again`). What
//! reaches the model is the audio, its voice-activity filter on (Workers
//! AI's), and, when the client sent them, its language and prompt
//! (`bound`); the client's answer is OpenAI's shape, `{"text": …}` or the
//! text alone (`answer`).

use base64::Engine;
use serde_json::{json, Value};

use crate::models::Upstream;
use crate::multipart::{Field, Part};
use crate::price::{KeyPrice, Usage};

/// The route's name for transcription: no tier, as `vision` is none. An
/// agent's runtime sends it (Hermes' `stt.openai.model`, the bridge's
/// scripts), whatever model the slot runs: a name, never a model's.
pub const WHISPER: &str = "whisper";
/// The slot's key in a deployment's `models` (its key's binding is
/// `secrets_store::model_key`'s of it).
pub const TRANSCRIPTION: &str = "transcription";
/// What a transcription on an OpenAI-compatible API is metered in: its
/// audio's seconds, under a name with a `/`, which no catalog row's charge
/// can take, priced at its slot's own price (`key_price`).
pub const SECONDS: &str = "transcription/second";
/// The most a Workers AI slot names a minute, in neurons: far past any
/// speech model's in its catalog (Whisper large v3 turbo's 46.63 on
/// 2026-10-07), so one higher is a typo, refused rather than charged.
pub const NEURONS_A_MINUTE_MAX: f64 = 100_000.0;
/// The most an OpenAI-compatible slot names a minute, in dollars: far past
/// any vendor's list (OpenAI's $0.006 a minute for whisper-1).
pub const USD_A_MINUTE_MAX: f64 = 1.0;
/// The largest audio a call takes (Paul, 2026-10-07): an hour of a voice
/// note at 24 kbps, five minutes of 16 kHz WAV. Its base64 (about 13.3
/// MiB) and the answer stay well within a Worker's memory.
pub const AUDIO_MAX_BYTES: usize = 10 * 1024 * 1024;
/// The whole form: the audio and its other fields.
pub const BODY_MAX_BYTES: usize = AUDIO_MAX_BYTES + 64 * 1024;
/// A prompt (Whisper's `initial_prompt`), at most this many characters:
/// OpenAI reads 224 tokens of one.
pub const PROMPT_MAX_CHARS: usize = 1_024;
/// An answer's audio is at most a day long: a duration past it is not a
/// model's, and no usage.
pub const DURATION_MAX_SECONDS: f64 = 86_400.0;
/// What an OpenAI-compatible API is asked to answer in: the one form of
/// OpenAI's that says the audio's `duration` on every Whisper server.
pub const UPSTREAM_FORMAT: &str = "verbose_json";
/// The path the route adds to an OpenAI-compatible slot's `base_url`.
pub const OPENAI_PATH: &str = "/audio/transcriptions";
/// A try that failed within this long of being sent is made once more
/// (`again`): a refusal for capacity comes at once, and a voice memo's
/// client waits 60 s for the whole call (Hermes' `stt.openai.timeout`), so
/// a slow failure (a long recording timing out) is passed on instead.
pub const AGAIN_WITHIN_MS: i64 = 15_000;
/// How long the route waits before that second try.
pub const AGAIN_AFTER_MS: u64 = 1_000;
/// The `retry-after` a 429 passed on carries, in seconds, once the second
/// try was refused too: an OpenAI SDK's own retry waits this long (it reads
/// the header) instead of half a second.
pub const RETRY_AFTER_S: u32 = 5;

/// Where a deployment's transcriptions run, and their price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub upstream: Upstream,
    pub price: Price,
}

/// A transcription slot's price, a minute of audio, in its kind's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Price {
    /// Workers AI's: thousandths of a neuron (the ledger's unit, as FLUX's
    /// tiles and steps are: crate::media), which the book prices.
    Neurons { milli_a_minute: u64 },
    /// An OpenAI-compatible API's: micro-dollars at list, charged by the
    /// second (`SECONDS`) with the margin, as an operator key's use is.
    Usd { micros_a_minute: i64 },
}

/// A deployment's `models.transcription`, taken off `models` (`None` when
/// unnamed: it transcribes nothing): an `Upstream` and its `price`, which a
/// Workers AI slot names as `{"neurons_a_minute": <n>}` and an
/// OpenAI-compatible one as `{"audio_minute": <$>}`. Refused, saying why:
/// no price, a price of the other kind or out of range, a chat slot's or an
/// embeddings slot's field. Where it runs is checked as every slot's is
/// (`Models::parse`).
pub fn slot_of(models: &mut Value) -> Result<Option<Slot>, String> {
    let Some(mut row) = models.as_object_mut().and_then(|m| m.remove(TRANSCRIPTION)) else { return Ok(None) };
    let o = row.as_object_mut().ok_or("models.transcription: a slot is where a model runs, an object")?;
    let price = o.remove("price");
    for key in ["body", "images", "private", "fallback", "label", "vision", "reasoning", "max_tokens", "deep", "dimensions", "version"] {
        if o.contains_key(key) {
            return Err(format!("models.transcription: {key} is a chat or embeddings slot's, not a transcription slot's"));
        }
    }
    // through the one decoder an `Upstream` has, as the other slots'
    let upstream: Upstream = serde_json::from_slice(row.to_string().as_bytes()).map_err(|e| format!("models.transcription: {e}"))?;
    let (field, max, unit) = match upstream {
        Upstream::WorkersAi { .. } => ("neurons_a_minute", NEURONS_A_MINUTE_MAX, "neurons a minute of audio, as Workers AI's pricing lists its speech models"),
        Upstream::Openai { .. } => ("audio_minute", USD_A_MINUTE_MAX, "dollars a minute of audio, as its API lists it (0 for a box's own)"),
    };
    let named = match price {
        Some(Value::Object(p)) if p.len() == 1 => p.get(field).and_then(Value::as_f64).filter(|n| (0.0..=max).contains(n)),
        _ => None,
    };
    let Some(n) = named else { return Err(format!("models.transcription: price is {{\"{field}\": <{unit}>}}, from 0 to {max}")) };
    // never below what was named, past a float's last digit
    let up = |v: f64| (v - 1e-6).ceil().max(0.0);
    let price = match upstream {
        Upstream::WorkersAi { .. } => Price::Neurons { milli_a_minute: up(n * 1e3) as u64 },
        Upstream::Openai { .. } => Price::Usd { micros_a_minute: up(n * 1e6) as i64 },
    };
    Ok(Some(Slot { upstream, price }))
}

/// The book's row for what an OpenAI-compatible slot's calls are metered in
/// (`SECONDS`), at its price; none for a Workers AI slot (the book prices
/// neurons).
pub fn key_price(slot: &Slot) -> Option<KeyPrice> {
    match slot.price {
        Price::Usd { micros_a_minute } => Some(KeyPrice { key: SECONDS.to_string(), micros: micros_a_minute, per: 60 }),
        Price::Neurons { .. } => None,
    }
}

/// Whether a transcription's first try, which answered `status` (`None`:
/// it never answered) `ms` after it was sent, is made once more: its model
/// short of capacity (429, Workers AI's 3040 "Capacity temporarily
/// exceeded, please try again"), its own failure (5xx) or timeout (408), or
/// no answer at all, when that came fast. A refusal of the request itself
/// (any other 4xx) would be the second try's too, and a 200 is the answer.
/// Nothing is charged for a try that failed: a call is charged after its
/// answer, at what its model heard.
pub fn again(status: Option<u16>, ms: i64) -> bool {
    let failed = match status {
        None => true,
        Some(s) => s == 408 || s == 429 || (500..=599).contains(&s),
    };
    failed && ms <= AGAIN_WITHIN_MS
}

/// Why a transcription is refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `model` is not `whisper`.
    Model,
    /// No `file`, or an empty one.
    NoAudio,
    /// The audio is over `AUDIO_MAX_BYTES` (413).
    TooLarge(usize),
    /// A `response_format` other than `json` or `text`.
    Format,
    /// A `language` that is no ISO-639 code.
    Language,
    /// A `prompt` over `PROMPT_MAX_CHARS`, or not UTF-8.
    Prompt,
    /// A field that is not UTF-8 text.
    Field(String),
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::Model => format!("a transcription's model is {WHISPER:?}"),
            Refusal::NoAudio => "a transcription carries its audio as `file`".into(),
            Refusal::TooLarge(n) => format!("the audio is {n} bytes; a transcription takes at most {AUDIO_MAX_BYTES}"),
            Refusal::Format => "response_format is json or text".into(),
            Refusal::Language => "language is an ISO-639-1 code, as en".into(),
            Refusal::Prompt => format!("prompt is at most {PROMPT_MAX_CHARS} characters"),
            Refusal::Field(name) => format!("{name} is text"),
        }
    }
}

/// How the client reads its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `{"text": …}`, OpenAI's default.
    Json,
    /// The text alone, `text/plain`.
    Text,
}

/// A transcription bounded: what reaches its model (the audio, and the
/// language and prompt the client named), and how the client reads the
/// answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded<'a> {
    pub audio: &'a [u8],
    /// The audio's file name as the client sent it, when a form may carry
    /// it as it is (`multipart::header_safe`), else `audio`: an API may
    /// read the audio's kind from its extension (OpenAI's does).
    pub filename: &'a str,
    /// Its type as the client sent it, when a form may carry it.
    pub content_type: Option<&'a str>,
    pub language: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub format: Format,
}

fn text<'a>(p: &'a Part<'_>) -> Result<&'a str, Refusal> {
    std::str::from_utf8(p.data).map_err(|_| Refusal::Field(p.name.clone()))
}

/// A form's fields, bounded: its audio, its language and prompt when it
/// named them, nothing else (OpenAI's `temperature` and
/// `timestamp_granularities` are let go).
pub fn bound<'a>(parts: &'a [Part<'a>]) -> Result<Bounded<'a>, Refusal> {
    let field = |name: &str| parts.iter().find(|p| p.name == name);
    match field("model").map(text).transpose()? {
        Some(m) if m.trim() == WHISPER => {}
        _ => return Err(Refusal::Model),
    }
    let file = field("file").filter(|p| !p.data.is_empty()).ok_or(Refusal::NoAudio)?;
    if file.data.len() > AUDIO_MAX_BYTES {
        return Err(Refusal::TooLarge(file.data.len()));
    }
    let format = match field("response_format").map(text).transpose()?.map(str::trim) {
        None | Some("") | Some("json") => Format::Json,
        Some("text") => Format::Text,
        Some(_) => return Err(Refusal::Format),
    };
    let language = field("language").map(text).transpose()?.map(str::trim).filter(|l| !l.is_empty());
    if language.is_some_and(|l| !(2..=3).contains(&l.len()) || !l.bytes().all(|b| b.is_ascii_lowercase())) {
        return Err(Refusal::Language);
    }
    let prompt = field("prompt").map(text).transpose().map_err(|_| Refusal::Prompt)?.filter(|p| !p.trim().is_empty());
    if prompt.is_some_and(|p| p.chars().count() > PROMPT_MAX_CHARS) {
        return Err(Refusal::Prompt);
    }
    fn safe(s: Option<&str>) -> Option<&str> {
        s.filter(|s| crate::multipart::header_safe(s))
    }
    Ok(Bounded { audio: file.data, filename: safe(file.filename.as_deref()).unwrap_or("audio"), content_type: safe(file.content_type.as_deref()), language, prompt, format })
}

/// What a Workers AI slot's model is sent, as Whisper's catalog takes it
/// (developers.cloudflare.com/workers-ai/models/whisper-large-v3-turbo/,
/// read 2026-10-07): `{audio: <base64>, task, vad_filter, language?,
/// initial_prompt?}`. Heard through its voice-activity filter
/// (`vad_filter`, the catalog's own input): silence and noise are heard as
/// no words, not as a phrase Whisper is known to make up in them ("Thank
/// you."), so an agent asks for the memo again instead of answering what
/// no one said.
pub fn workers_ai_input(b: &Bounded<'_>) -> Value {
    let mut input = json!({ "audio": base64::engine::general_purpose::STANDARD.encode(b.audio), "task": "transcribe", "vad_filter": true });
    if let Some(l) = b.language {
        input["language"] = json!(l);
    }
    if let Some(p) = b.prompt {
        input["initial_prompt"] = json!(p);
    }
    input
}

/// What an OpenAI-compatible slot's API is sent: OpenAI's transcription
/// form, `model` the slot's (`id`), the audio as `file` with its name and
/// type, its language and prompt, and `response_format` `UPSTREAM_FORMAT`;
/// `None` when `boundary` occurs in it (a random one does not).
pub fn openai_form(b: &Bounded<'_>, id: &str, boundary: &str) -> Option<Vec<u8>> {
    let plain = |name, data: &'static str| Field { name, filename: None, content_type: None, data: data.as_bytes() };
    let mut fields = vec![Field { name: "model", filename: None, content_type: None, data: id.as_bytes() }, plain("response_format", UPSTREAM_FORMAT)];
    for (name, value) in [("language", b.language), ("prompt", b.prompt)] {
        if let Some(v) = value {
            fields.push(Field { name, filename: None, content_type: None, data: v.as_bytes() });
        }
    }
    fields.push(Field { name: "file", filename: Some(b.filename), content_type: b.content_type, data: b.audio });
    crate::multipart::form(boundary, &fields)
}

/// What a call cost, from its model's answer: its audio's length, where
/// the slot's kind says it (Whisper's `transcription_info.duration` on
/// Workers AI; OpenAI's `usage` of type `duration`, else `verbose_json`'s
/// `duration`), at the slot's price. An answer that does not say, or says
/// what no audio is, is `None`: charged nothing, said aloud
/// (cell/src/models.rs; decision 62).
pub fn usage_of(slot: &Slot, answer: &Value) -> Option<Usage> {
    let seconds = match &slot.upstream {
        Upstream::WorkersAi { .. } => answer["transcription_info"]["duration"].as_f64(),
        Upstream::Openai { .. } => match &answer["usage"] {
            u if u["type"] == "duration" => u["seconds"].as_f64(),
            _ => answer["duration"].as_f64(),
        },
    }?;
    if !seconds.is_finite() || !(0.0..=DURATION_MAX_SECONDS).contains(&seconds) {
        return None;
    }
    Some(match slot.price {
        // a part of a thousandth of a neuron counted whole; at most a day
        // of audio at the highest price: far under QUANTITY_MAX
        Price::Neurons { milli_a_minute } => Usage::Neurons { milli: ((seconds * 1_000.0).ceil() as u64 * milli_a_minute).div_ceil(60_000) },
        Price::Usd { .. } => Usage::Key { key: SECONDS.to_string(), units: seconds.ceil() as u64 },
    })
}

/// What a transcription's answer says of its model's (the route's
/// `x-fragment-answer-shape`), never its words: its top-level keys, and
/// its `usage` when that is a small object of numbers, booleans and short
/// strings (else `null`), so a hosted run shows what its API meters it by.
pub fn shape(model: &Value) -> Value {
    let keys: Vec<&str> = model.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    let scalar = |v: &Value| v.is_number() || v.is_boolean() || v.as_str().is_some_and(|s| s.len() <= 32);
    let usage = match model.get("usage") {
        Some(Value::Object(u)) if u.len() <= 8 && u.values().all(scalar) => Value::Object(u.clone()),
        _ => Value::Null,
    };
    json!({ "keys": keys, "usage": usage })
}

/// The client's answer from its model's: its content type and body, or
/// `None` when the answer carries no text.
pub fn answer(format: Format, model: &Value) -> Option<(&'static str, Vec<u8>)> {
    let text = model["text"].as_str()?.trim();
    Some(match format {
        Format::Json => ("application/json", json!({ "text": text }).to_string().into_bytes()),
        Format::Text => ("text/plain; charset=utf-8", text.as_bytes().to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::PriceBook;

    fn part<'a>(name: &str, data: &'a [u8]) -> Part<'a> {
        Part { name: name.into(), filename: (name == "file").then(|| "memo.ogg".into()), content_type: (name == "file").then(|| "audio/ogg".into()), data }
    }

    fn slot(row: Value) -> Result<Option<Slot>, String> {
        slot_of(&mut json!({ "transcription": row }))
    }

    /// A Workers AI slot at Whisper large v3 turbo's 46.63 neurons a
    /// minute (Workers AI's pricing page, read 2026-10-07), as preview's
    /// config names it.
    fn workers_ai() -> Slot {
        slot(json!({ "provider": "workers_ai", "model": "@cf/openai/whisper-large-v3-turbo", "price": { "neurons_a_minute": 46.63 } })).unwrap().unwrap()
    }

    /// A self-hoster's own server, at OpenAI's whisper-1 list of $0.006 a
    /// minute.
    fn openai() -> Slot {
        slot(json!({ "provider": "openai", "base_url": "http://127.0.0.1:8000/v1", "model": "local-whisper", "price": { "audio_minute": 0.006 } })).unwrap().unwrap()
    }

    /// Valid: each kind of slot reads as where it runs and its price in
    /// its own unit; unnamed, there is none.
    #[test]
    fn a_slot_is_where_it_runs_at_its_price() {
        assert_eq!(slot_of(&mut json!({})), Ok(None), "unnamed: the deployment transcribes nothing");
        let mut models = json!({ "transcription": { "provider": "workers_ai", "model": "@cf/openai/whisper-large-v3-turbo", "price": { "neurons_a_minute": 46.63 } }, "default": "cheap" });
        let w = slot_of(&mut models).unwrap().unwrap();
        assert_eq!((w.upstream.model(), w.price), ("@cf/openai/whisper-large-v3-turbo", Price::Neurons { milli_a_minute: 46_630 }));
        assert_eq!(models, json!({ "default": "cheap" }), "taken off models");
        assert_eq!(key_price(&w), None, "the book prices neurons");
        let o = openai();
        assert!(matches!(&o.upstream, Upstream::Openai { base_url, model, body: None, price: None } if base_url == "http://127.0.0.1:8000/v1" && model == "local-whisper"));
        assert_eq!(o.price, Price::Usd { micros_a_minute: 6_000 });
        assert_eq!(key_price(&o), Some(KeyPrice { key: SECONDS.into(), micros: 6_000, per: 60 }));
        let free = slot(json!({ "provider": "openai", "base_url": "http://127.0.0.1:8000/v1", "model": "m", "price": { "audio_minute": 0 } })).unwrap().unwrap();
        assert_eq!(free.price, Price::Usd { micros_a_minute: 0 }, "a box's own, at zero");
    }

    /// Invalid: a slot out of shape is refused, saying why.
    #[test]
    fn a_slot_out_of_shape_is_refused() {
        let wai = |price: Value| slot(json!({ "provider": "workers_ai", "model": "@cf/m", "price": price }));
        let oai = |price: Value| slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "m", "price": price }));
        for (got, says) in [
            (slot(json!({ "provider": "workers_ai", "model": "@cf/m" })), "neurons_a_minute"),
            (wai(json!({ "audio_minute": 0.006 })), "neurons_a_minute"),
            (oai(json!({ "neurons_a_minute": 46.63 })), "audio_minute"),
            (oai(json!({ "input": 1, "output": 1 })), "audio_minute"),
            (oai(json!({ "audio_minute": 0.006, "extra": 1 })), "audio_minute"),
            (oai(json!({ "audio_minute": -1 })), "audio_minute"),
            (oai(json!({ "audio_minute": 2 })), "from 0 to 1"),
            (wai(json!({ "neurons_a_minute": 1e6 })), "neurons_a_minute"),
            (oai(json!({ "audio_minute": "0.006" })), "audio_minute"),
            (slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "m", "price": { "audio_minute": 0 }, "body": {} })), "body is a chat"),
            (slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "m", "price": { "audio_minute": 0 }, "fallback": "x" })), "fallback is a chat"),
            (slot(json!({ "provider": "openai", "model": "m", "price": { "audio_minute": 0 } })), "base_url"),
            (slot(json!({ "provider": "openai", "base_url": "https://a.example/v1", "model": "m", "price": { "audio_minute": 0 }, "dims": 3 })), "unknown field"),
            (slot(json!("@cf/m")), "an object"),
        ] {
            let err = got.unwrap_err();
            assert!(err.starts_with("models.transcription") && err.contains(says), "{err} ({says})");
        }
    }

    /// Valid: an SDK's form is bounded to its audio, the language and
    /// prompt it named, and the format it asked for; Whisper's input on
    /// Workers AI, and OpenAI's form on an OpenAI-compatible API.
    #[test]
    fn a_form_is_its_models_input() {
        let audio = b"OggS\x00voice".to_vec();
        let parts = [part("model", b"whisper"), part("file", &audio), part("response_format", b"json")];
        let b = bound(&parts).unwrap();
        assert_eq!(workers_ai_input(&b), json!({ "audio": base64::engine::general_purpose::STANDARD.encode(&audio), "task": "transcribe", "vad_filter": true }), "no language: Whisper detects it, through its voice-activity filter");
        assert_eq!((b.format, b.filename, b.content_type), (Format::Json, "memo.ogg", Some("audio/ogg")));
        let parts = [part("model", b" whisper "), part("file", &audio), part("language", b"de"), part("prompt", b"Paul, fragment"), part("response_format", b"text"), part("temperature", b"0")];
        let b = bound(&parts).unwrap();
        let input = workers_ai_input(&b);
        assert_eq!((input["language"].clone(), input["initial_prompt"].clone(), b.format), (json!("de"), json!("Paul, fragment"), Format::Text));
        assert!(input.get("temperature").is_none(), "what Whisper has no input for is let go");
        let form = openai_form(&b, "local-whisper", "fragment-0123").unwrap();
        let sent = crate::multipart::parts(&form, "fragment-0123").unwrap();
        let fields: Vec<(&str, &[u8])> = sent.iter().map(|p| (p.name.as_str(), p.data)).collect();
        assert_eq!(fields, [("model", &b"local-whisper"[..]), ("response_format", b"verbose_json"), ("language", b"de"), ("prompt", b"Paul, fragment"), ("file", &audio[..])], "the slot's model, never the route's name; the format the route reads its length from");
        assert_eq!((sent[4].filename.as_deref(), sent[4].content_type.as_deref()), (Some("memo.ogg"), Some("audio/ogg")));
        let parts = [part("model", b"whisper"), part("file", &audio), part("language", b""), part("prompt", b"  ")];
        let b = bound(&parts).unwrap();
        assert_eq!(workers_ai_input(&b), json!({ "audio": base64::engine::general_purpose::STANDARD.encode(&audio), "task": "transcribe", "vad_filter": true }), "empty is none");
        assert_eq!(crate::multipart::parts(&openai_form(&b, "m", "b").unwrap(), "b").unwrap().len(), 3, "nor sent on");
        // a file name no header may carry is sent as `audio`, and the
        // boundary found in the audio writes no form
        let odd = [Part { name: "file".into(), filename: Some("a\"; name=\"model".into()), content_type: Some("audio/ogg; codecs=opus".into()), data: b"--fragment-x" }, part("model", b"whisper")];
        let b = bound(&odd).unwrap();
        assert_eq!((b.filename, b.content_type), ("audio", None));
        assert_eq!(openai_form(&b, "m", "fragment-x"), None);
    }

    /// Invalid: what is no transcription the route takes is refused before
    /// anything is sent, saying why.
    #[test]
    fn a_form_out_of_shape_is_refused() {
        let a = b"audio".as_slice();
        assert_eq!(bound(&[part("file", a)]), Err(Refusal::Model));
        assert_eq!(bound(&[part("model", b"whisper-1"), part("file", a)]), Err(Refusal::Model), "OpenAI's own model is no name of ours");
        assert_eq!(bound(&[part("model", b"medium"), part("file", a)]), Err(Refusal::Model), "a tier transcribes nothing");
        assert_eq!(bound(&[part("model", b"whisper")]), Err(Refusal::NoAudio));
        assert_eq!(bound(&[part("model", b"whisper"), part("file", b"")]), Err(Refusal::NoAudio));
        let big = vec![0u8; AUDIO_MAX_BYTES + 1];
        assert_eq!(bound(&[part("model", b"whisper"), part("file", &big)]), Err(Refusal::TooLarge(AUDIO_MAX_BYTES + 1)));
        assert!(bound(&[part("model", b"whisper"), part("file", &big[..AUDIO_MAX_BYTES])]).is_ok(), "the bound itself is taken");
        for f in ["srt", "vtt", "verbose_json", "JSON"] {
            assert_eq!(bound(&[part("model", b"whisper"), part("file", a), part("response_format", f.as_bytes())]), Err(Refusal::Format), "{f}");
        }
        for l in ["english", "EN", "e", "en-US"] {
            assert_eq!(bound(&[part("model", b"whisper"), part("file", a), part("language", l.as_bytes())]), Err(Refusal::Language), "{l}");
        }
        let long = "p".repeat(PROMPT_MAX_CHARS + 1);
        assert_eq!(bound(&[part("model", b"whisper"), part("file", a), part("prompt", long.as_bytes())]), Err(Refusal::Prompt));
        assert_eq!(bound(&[part("model", b"\xff"), part("file", a)]), Err(Refusal::Field("model".into())));
        assert!(Refusal::TooLarge(11).message().contains("at most 10485760"));
    }

    /// Goal: a call is charged at the length its model heard, at its slot's
    /// price: Whisper on Workers AI at 46.63 neurons a minute is its listed
    /// $0.0005 (before the fee and margin); an OpenAI-compatible API's at
    /// its dollars a minute, by the second. Method: the book's price of a
    /// minute on each, and answers that do and do not say how long.
    #[test]
    fn a_minute_is_its_slots_price() {
        let book = PriceBook::defaults();
        let w = workers_ai();
        let minute = usage_of(&w, &json!({ "text": "hi", "transcription_info": { "duration": 60.0 } })).unwrap();
        assert_eq!(minute, Usage::Neurons { milli: 46_630 });
        assert_eq!(book.price(&minute).unwrap().list, 513, "46.63 neurons at $0.011 a thousand: $0.000513 (512.93 µ$, rounded up)");
        assert_eq!(usage_of(&w, &json!({ "transcription_info": { "duration": 1.5 } })), Some(Usage::Neurons { milli: 1_166 }), "a part counted whole");
        assert_eq!(usage_of(&w, &json!({ "transcription_info": { "duration": 0.0 } })), Some(Usage::Neurons { milli: 0 }));
        assert_eq!(usage_of(&w, &json!({ "duration": 60.0, "usage": { "type": "duration", "seconds": 60 } })), None, "Workers AI's length is its catalog's field alone");
        for bad in [json!({}), json!({ "transcription_info": { "duration": "60" } }), json!({ "transcription_info": { "duration": -1.0 } }), json!({ "transcription_info": { "duration": 90_000.0 } })] {
            assert_eq!(usage_of(&w, &bad), None, "{bad}");
        }
        let o = openai();
        let mut keyed = book.clone();
        keyed.keys.push(key_price(&o).unwrap());
        let minute = usage_of(&o, &json!({ "text": "hi", "usage": { "type": "duration", "seconds": 60 } })).unwrap();
        assert_eq!(minute, Usage::Key { key: SECONDS.into(), units: 60 }, "OpenAI's duration usage");
        assert_eq!(keyed.price(&minute).unwrap().list, 6_000, "a minute at its $0.006");
        assert_eq!(keyed.price(&minute).unwrap().charge, 9_000, "with the margin, no credits fee (a vendor billing a card bills none)");
        assert_eq!(usage_of(&o, &json!({ "text": "hi", "duration": 2.04, "segments": [] })), Some(Usage::Key { key: SECONDS.into(), units: 3 }), "verbose_json's duration, a part of a second counted whole");
        assert_eq!(keyed.price(&Usage::Key { key: SECONDS.into(), units: 3 }).unwrap().list, 300);
        assert_eq!(usage_of(&o, &json!({ "duration": 5.0, "usage": { "type": "duration", "seconds": 2 } })), Some(Usage::Key { key: SECONDS.into(), units: 2 }), "its usage first");
        for bad in [json!({ "text": "hi" }), json!({ "usage": { "type": "tokens", "input_tokens": 10 } }), json!({ "duration": f64::NAN }), json!({ "duration": 90_000 }), json!({ "transcription_info": { "duration": 2.0 } })] {
            assert_eq!(usage_of(&o, &bad), None, "{bad}");
        }
    }

    /// Goal: a voice memo survives its model's passing shortage of
    /// capacity, and a try is never made twice past what its client waits.
    /// Method: each kind of failure, fast and slow, and answers that are
    /// final.
    #[test]
    fn a_fast_failure_is_tried_once_more() {
        for status in [Some(429), Some(500), Some(502), Some(503), Some(504), Some(599), Some(408), None] {
            assert!(again(status, 0), "{status:?} at once");
            assert!(again(status, AGAIN_WITHIN_MS), "{status:?} at the bound");
            assert!(!again(status, AGAIN_WITHIN_MS + 1), "{status:?} past it: its client would wait twice");
        }
        for status in [200, 400, 401, 403, 404, 413, 422] {
            assert!(!again(Some(status), 0), "{status} is final: the second try would answer it too");
        }
        // a failure at the bound and the wait leave the second try most of
        // its client's 60 s
        assert!(AGAIN_WITHIN_MS + AGAIN_AFTER_MS as i64 <= 20_000);
        assert!((1..=60).contains(&RETRY_AFTER_S), "an OpenAI SDK honours a retry-after of at most 60 s");
    }

    /// The client's answer: OpenAI's json, or the text alone; an answer
    /// with no text is none.
    #[test]
    fn the_answer_is_openais_shape() {
        let w = json!({ "text": " hello there ", "word_count": 2, "transcription_info": { "duration": 1.2 }, "vtt": "WEBVTT" });
        assert_eq!(answer(Format::Json, &w), Some(("application/json", br#"{"text":"hello there"}"#.to_vec())));
        assert_eq!(answer(Format::Text, &w), Some(("text/plain; charset=utf-8", b"hello there".to_vec())));
        assert_eq!(answer(Format::Json, &json!({ "transcription_info": {} })), None);
        // an answer in another shape (the REST API's wrapping) has no text
        // and no length: answered 502, charged nothing
        let wrapped = json!({ "result": w, "success": true });
        assert_eq!((answer(Format::Text, &wrapped), usage_of(&workers_ai(), &wrapped)), (None, None));
        let verbose = json!({ "task": "transcribe", "language": "english", "duration": 2.0, "text": "hello", "segments": [] });
        assert_eq!(answer(Format::Json, &verbose), Some(("application/json", br#"{"text":"hello"}"#.to_vec())), "verbose_json's text");
    }

    /// The shape a transcription's answer names: the keys, and a `usage` of
    /// scalars as it came, never the words or anything that could carry
    /// them. Method: the hosted answer's keys, with a usage shown and ones
    /// not.
    #[test]
    fn the_shape_shows_keys_and_a_scalar_usage() {
        let hosted = json!({ "text": "Thank you.", "transcription_info": { "duration": 2.0 }, "segments": [], "vtt": "WEBVTT", "word_count": 2, "usage": { "type": "duration", "seconds": 2 } });
        assert_eq!(shape(&hosted), json!({ "keys": ["segments", "text", "transcription_info", "usage", "vtt", "word_count"], "usage": { "type": "duration", "seconds": 2 } }));
        assert!(!shape(&hosted).to_string().contains("Thank you"), "never its words");
        for not_shown in [json!({ "usage": { "said": "x".repeat(33) } }), json!({ "usage": { "nested": { "a": 1 } } }), json!({ "usage": [1, 2] }), json!({})] {
            assert_eq!(shape(&not_shown)["usage"], Value::Null, "{not_shown}");
        }
    }
}
