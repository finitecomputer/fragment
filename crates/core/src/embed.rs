//! The model route's embeddings (docs/api.md, Models): OpenAI's `POST
//! /v1/embeddings` shape, run on the deployment's `embeddings` slot
//! (`Slot`, its config's `models.embeddings`): a Workers AI model through
//! the deployment's AI Gateway, or a model on an OpenAI-compatible API (a
//! self-hosted deployment's own, on its own machines). Pure: the cell
//! reads the body, makes the call and meters it (cell/src/models.rs).
//!
//! A slot names its vectors' space (`Space`): its model, its dimensions and
//! the version the deployment pins it at. Vectors of two spaces are never
//! compared: every answer names its space, and a vector of any other
//! length than the slot's is refused (`answer`), never passed on. Each is
//! unit length, so a dot product is a cosine whichever API made it.
//!
//! A slot reads at most `max_input_tokens` of an input (its model's
//! window; the slot unnamed declares 512): an input that might be past it
//! is refused, never cut, so its caller splits a long text (the search
//! capability embeds a section's chunks). Its tokens are bounded without
//! its model's tokenizer (`tokens_max`), a bound that holds for every
//! tokenizer an embeddings model runs, not an average. A deployment whose tiers are private embeds
//! only on a slot marked private (Paul's rule: Finite Private never falls
//! back to a model that is not; `Models::embeddings_refused`).
//!
//! A call is charged after its answer, at its input tokens, at its model's
//! row in the price book (BGE-M3's on Workers AI; a self-hosted slot names
//! its own `price`, as any OpenAI-compatible slot does). An API that
//! reports its tokens is charged them; Workers AI's embedding models report
//! none, so theirs are estimated from the input (`estimated_tokens`).

use base64::Engine;
use icu_normalizer::DecomposingNormalizerBorrowed;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::models::Upstream;
use crate::price::Usage;

/// The route's name for embeddings (no tier, as `whisper` is none), and its
/// slot's key in a deployment's `models`.
pub const EMBEDDINGS: &str = "embeddings";
/// The slot unnamed: BGE-M3 on Workers AI ($0.012 a million input tokens,
/// developers.cloudflare.com/workers-ai/platform/pricing, read 2026-10-30),
/// 1,024 dimensions. The search benchmark's model (bench/search): the same
/// weights run on a self-hosted deployment's own machines.
pub const MODEL_DEFAULT: &str = "@cf/baai/bge-m3";
pub const DIMENSIONS_DEFAULT: u32 = 1024;
/// A slot's dimensions, at most.
pub const DIMENSIONS_MAX: u32 = 4096;
/// The tokens of an input a slot reads unless it names its own
/// (`max_input_tokens`): BGE-M3's on Workers AI (AI Search's model table,
/// read 2026-10-30), the fewest of the models it might run.
pub const MAX_INPUT_TOKENS_DEFAULT: u32 = 512;
/// A slot's `max_input_tokens`, at most, and at least: a window holds a
/// title and any character (`CHAR_TOKENS_MAX`) beside some words.
pub const MAX_INPUT_TOKENS_MAX: u32 = 32_768;
pub const MAX_INPUT_TOKENS_MIN: u32 = 128;
/// A slot's `version`, at most: a tag the deployment pins, not prose.
pub const VERSION_MAX_BYTES: usize = 32;
/// Inputs one call embeds, at most (Workers AI's text embedding models take
/// a batch of 100).
pub const INPUTS_MAX: usize = 100;
/// A call's body, at most: `INPUTS_MAX` inputs of 10 KiB each, about a
/// search upsert's batch. An input is at most `MAX_INPUT_TOKENS_MAX` bytes
/// (`tokens_max` counts a byte a token or more).
pub const BODY_MAX_BYTES: usize = 1024 * 1024;
/// Tokens a window holds beside its text's own: a model's two special
/// tokens (BGE-M3's `<s>` and `</s>`, BERT's `[CLS]` and `[SEP]`) and the
/// word marker SentencePiece puts before the first word.
pub const OVERHEAD_TOKENS: u32 = 3;
/// The most tokens one character is read as (`char_tokens_max`): U+FDFA,
/// whose NFKD is eighteen Arabic letters and spaces, 33 bytes.
pub const CHAR_TOKENS_MAX: u32 = 33;
/// Bytes a token is estimated at, to charge a call whose API reports no
/// tokens: an estimate (BGE-M3 reads about four bytes of English prose a
/// token, fewer of numbers or punctuation), never a bound.
pub const ESTIMATE_BYTES_A_TOKEN: usize = 3;

/// Where a deployment's embeddings run, and the space their vectors are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub upstream: Upstream,
    pub dimensions: u32,
    pub version: Option<String>,
    /// The tokens of an input its model reads.
    pub max_input_tokens: u32,
    /// Its model is private: confidential inference that keeps nothing
    /// (a slot's `"private": true`, as a chat slot's; never Workers AI's).
    pub private: bool,
}

/// The vector space a slot's answers are in: compare only vectors of one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Space<'a> {
    pub model: &'a str,
    pub dimensions: u32,
    pub version: Option<&'a str>,
}

impl Default for Slot {
    fn default() -> Slot {
        Slot { upstream: Upstream::WorkersAi { model: MODEL_DEFAULT.into() }, dimensions: DIMENSIONS_DEFAULT, version: None, max_input_tokens: MAX_INPUT_TOKENS_DEFAULT, private: false }
    }
}

impl Slot {
    pub fn space(&self) -> Space<'_> {
        Space { model: self.upstream.model(), dimensions: self.dimensions, version: self.version.as_deref() }
    }
}

/// The most tokens `c` is read as, by any tokenizer an embeddings model
/// runs: the UTF-8 bytes of the longest of `c`, its canonical decomposition
/// (NFD) and its compatibility decomposition (NFKD).
///
/// Why: a tokenizer first normalizes its text (not at all, NFC or NFD,
/// NFKC or NFKD), then reads it in tokens of at least a character
/// (SentencePiece, WordPiece) or a byte (byte-level BPE, SentencePiece's
/// byte fallback), and a character is a byte or more. Composing never
/// lengthens UTF-8 (every canonical pair checked), so no normalization
/// makes `c` more bytes than this. Checked against BGE-M3's own tokenizer
/// for every code point, in runs, spaced and after a letter (never over,
/// equal at worst); its NFKC turns "ﷺ", 3 bytes, into 18 characters.
pub fn char_tokens_max(c: char) -> u32 {
    if c.is_ascii() {
        return 1;
    }
    let mut buf = [0u8; 4];
    let one = c.encode_utf8(&mut buf);
    let nfd = DecomposingNormalizerBorrowed::new_nfd().normalize(one).len();
    let nfkd = DecomposingNormalizerBorrowed::new_nfkd().normalize(one).len();
    one.len().max(nfd).max(nfkd) as u32
}

/// The most tokens `text` is read as, a model's own included: its
/// characters' `char_tokens_max` and `OVERHEAD_TOKENS`. At least its bytes.
pub fn tokens_max(text: &str) -> u64 {
    text.chars().map(|c| u64::from(char_tokens_max(c))).sum::<u64>() + u64::from(OVERHEAD_TOKENS)
}

/// A deployment's `models.embeddings`, taken off `models` (the default when
/// unnamed): an `Upstream` with its `dimensions` (required: the space is
/// the deployment's to say, never guessed), an optional `version`, its
/// window (`max_input_tokens`, `MAX_INPUT_TOKENS_DEFAULT` unnamed) and
/// whether it is private.
/// Where it runs is checked as every slot's is (`Models::parse`).
pub fn slot_of(models: &mut Value) -> Result<Slot, String> {
    let Some(mut row) = models.as_object_mut().and_then(|m| m.remove(EMBEDDINGS)) else { return Ok(Slot::default()) };
    let o = row.as_object_mut().ok_or("models.embeddings: a slot is where a model runs, an object")?;
    let dimensions = match o.remove("dimensions") {
        Some(Value::Number(n)) => n.as_u64().filter(|d| (1..=u64::from(DIMENSIONS_MAX)).contains(d)).map(|d| d as u32),
        _ => None,
    }
    .ok_or_else(|| format!("models.embeddings: dimensions is the length of its model's vectors, 1 to {DIMENSIONS_MAX}"))?;
    let version = match o.remove("version") {
        None => None,
        Some(Value::String(v)) if crate::price::printable(&v, VERSION_MAX_BYTES) => Some(v),
        Some(other) => return Err(format!("models.embeddings: version is the tag its model is pinned at (1 to {VERSION_MAX_BYTES} printable characters), not {other}")),
    };
    let max_input_tokens = match o.remove("max_input_tokens") {
        None => MAX_INPUT_TOKENS_DEFAULT,
        Some(v) => v
            .as_u64()
            .filter(|n| (u64::from(MAX_INPUT_TOKENS_MIN)..=u64::from(MAX_INPUT_TOKENS_MAX)).contains(n))
            .map(|n| n as u32)
            .ok_or_else(|| format!("models.embeddings: max_input_tokens is the tokens of an input its model reads, {MAX_INPUT_TOKENS_MIN} to {MAX_INPUT_TOKENS_MAX}, not {v}"))?,
    };
    let private = match o.remove("private") {
        None => false,
        Some(Value::Bool(b)) => b,
        Some(other) => return Err(format!("models.embeddings: private is true (its model is private: confidential inference that keeps nothing) or false, not {other}")),
    };
    for key in ["images", "fallback", "label", "vision", "reasoning", "max_tokens", "deep"] {
        if o.contains_key(key) {
            return Err(format!("models.embeddings: {key} is a chat slot's, not an embeddings slot's"));
        }
    }
    let upstream: Upstream = serde_json::from_value(row).map_err(|e| format!("models.embeddings: {e}"))?;
    if let Upstream::Openai { body: Some(_), .. } = &upstream {
        return Err("models.embeddings: body is a chat slot's (an API's own fields for its chat completions)".into());
    }
    Ok(Slot { upstream, dimensions, version, max_input_tokens, private })
}

/// How the client reads its vectors: OpenAI's `encoding_format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// JSON numbers.
    Float,
    /// Each vector's little-endian float32 bytes, base64 (OpenAI's SDKs ask
    /// for it unless told otherwise).
    Base64,
}

/// A call, checked: its inputs and how it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded {
    pub inputs: Vec<String>,
    pub format: Format,
}

/// Why a call is refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Input,
    Empty,
    TooMany(usize),
    TooLong { index: usize, window: u32, tokens: u64 },
    Model(String),
    Dimensions(u64),
    Format(String),
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::Input => "input is a string or an array of strings (token arrays are not taken)".into(),
            Refusal::Empty => "an input is some text: an empty one has no embedding".into(),
            Refusal::TooMany(n) => format!("a call embeds 1 to {INPUTS_MAX} inputs, not {n}"),
            Refusal::TooLong { index, window, tokens } => format!("input {index} may be {tokens} tokens, past the embeddings slot's {window}: split it, as nothing is cut"),
            Refusal::Model(m) => format!("model is {EMBEDDINGS} or the embeddings slot's own model, not {m:?}"),
            Refusal::Dimensions(d) => format!("dimensions is the slot's own when named: this deployment's vectors are not cut to {d}"),
            Refusal::Format(f) => format!("encoding_format is float or base64, not {f:?}"),
        }
    }
}

/// `body` (OpenAI's embeddings request), checked against `slot`: `input`
/// one string or 1 to `INPUTS_MAX` of them, each some text of at most the
/// slot's `max_input_tokens` (`tokens_max`);
/// `model`, when named, `embeddings` or the slot's model; `dimensions`,
/// when named, the slot's; `encoding_format` float (unnamed) or base64.
/// Nothing else of the client's is read, and nothing of it is sent on.
pub fn bound(body: &Value, slot: &Slot) -> Result<Bounded, Refusal> {
    let inputs: Vec<String> = match &body["input"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or(Refusal::Input)).collect::<Result<_, _>>()?,
        _ => return Err(Refusal::Input),
    };
    if inputs.is_empty() || inputs.len() > INPUTS_MAX {
        return Err(Refusal::TooMany(inputs.len()));
    }
    let window = slot.max_input_tokens;
    if let Some((index, tokens)) = inputs.iter().map(|t| tokens_max(t)).enumerate().find(|&(_, n)| n > u64::from(window)) {
        return Err(Refusal::TooLong { index, window, tokens });
    }
    if inputs.iter().any(|t| t.trim().is_empty()) {
        return Err(Refusal::Empty);
    }
    match &body["model"] {
        Value::Null => {}
        Value::String(m) if m == EMBEDDINGS || m == slot.upstream.model() => {}
        other => return Err(Refusal::Model(other.as_str().map_or_else(|| other.to_string(), str::to_string))),
    }
    match &body["dimensions"] {
        Value::Null => {}
        d if d.as_u64() == Some(u64::from(slot.dimensions)) => {}
        d => return Err(Refusal::Dimensions(d.as_u64().unwrap_or(0))),
    }
    let format = match &body["encoding_format"] {
        Value::Null => Format::Float,
        Value::String(f) if f == "float" => Format::Float,
        Value::String(f) if f == "base64" => Format::Base64,
        other => return Err(Refusal::Format(other.as_str().map_or_else(|| other.to_string(), str::to_string))),
    };
    Ok(Bounded { inputs, format })
}

/// What the slot's API is sent: Workers AI's text embedding input
/// (`{text: [...]}`, the BGE family's), or OpenAI's (`{model, input,
/// encoding_format: float}`), the inputs alone.
pub fn request(slot: &Slot, inputs: &[String]) -> Value {
    match &slot.upstream {
        Upstream::WorkersAi { .. } => json!({ "text": inputs }),
        Upstream::Openai { model, .. } => json!({ "model": model, "input": inputs, "encoding_format": "float" }),
    }
}

/// Why an answer is not passed on: it was paid for, so it is charged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    Shape,
    Count { asked: usize, answered: usize },
    Dimensions { slot: u32, answered: usize },
    Value,
}

impl Fault {
    pub fn message(&self) -> String {
        match self {
            Fault::Shape => "the model's answer carried no vectors".into(),
            Fault::Count { asked, answered } => format!("the model answered {answered} vectors for {asked} inputs"),
            Fault::Dimensions { slot, answered } => format!("the model answered a vector of {answered} dimensions, not the slot's {slot}: a vector of another space is never passed on"),
            Fault::Value => "the model's answer carried a value that is no finite number, or a zero vector".into(),
        }
    }
}

/// The answer's vectors, in the inputs' order, each checked to be the
/// slot's length and finite, and made unit length: Workers AI's `{shape,
/// data}`, or OpenAI's `{data: [{index, embedding}]}`.
pub fn answer(slot: &Slot, asked: usize, response: &Value) -> Result<Vec<Vec<f32>>, Fault> {
    let rows: Vec<&Value> = match &slot.upstream {
        Upstream::WorkersAi { .. } => response["data"].as_array().ok_or(Fault::Shape)?.iter().collect(),
        Upstream::Openai { .. } => {
            let items = response["data"].as_array().ok_or(Fault::Shape)?;
            let mut rows: Vec<Option<&Value>> = vec![None; items.len()];
            for item in items {
                let i = item["index"].as_u64().map(|i| i as usize).filter(|&i| i < rows.len()).ok_or(Fault::Shape)?;
                if rows[i].replace(&item["embedding"]).is_some() {
                    return Err(Fault::Shape);
                }
            }
            rows.into_iter().collect::<Option<_>>().ok_or(Fault::Shape)?
        }
    };
    if rows.len() != asked {
        return Err(Fault::Count { asked, answered: rows.len() });
    }
    rows.into_iter()
        .map(|row| {
            let row = row.as_array().ok_or(Fault::Shape)?;
            if row.len() != slot.dimensions as usize {
                return Err(Fault::Dimensions { slot: slot.dimensions, answered: row.len() });
            }
            let v: Vec<f32> = row.iter().map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite()).ok_or(Fault::Value)).collect::<Result<_, _>>()?;
            unit(v).ok_or(Fault::Value)
        })
        .collect()
}

fn unit(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return None;
    }
    v.iter_mut().for_each(|x| *x = (f64::from(*x) / norm) as f32);
    Some(v)
}

/// Tokens estimated from the inputs (`ESTIMATE_BYTES_A_TOKEN`), to charge a
/// call whose API reports none: a price, not the window (`tokens_max`).
pub fn estimated_tokens(inputs: &[String]) -> u64 {
    inputs.iter().map(|t| t.len().div_ceil(ESTIMATE_BYTES_A_TOKEN) as u64).sum()
}

/// The call's usage: the input tokens its API reported (OpenAI's
/// `usage.prompt_tokens`), else estimated (`estimated_tokens`), at the slot's
/// model.
pub fn usage_of(slot: &Slot, inputs: &[String], response: &Value) -> Option<Usage> {
    let tokens = response["usage"]["prompt_tokens"].as_u64().filter(|&n| n > 0).unwrap_or_else(|| estimated_tokens(inputs));
    Some(Usage::Tokens { model: slot.upstream.model().to_string(), input: tokens, cached_input: 0, cache_write: 0, output: 0 }).filter(|u| u.validate().is_ok())
}

/// The client's answer, OpenAI's shape and the slot's space: `{object:
/// "list", data: [{object: "embedding", index, embedding}], model, usage,
/// space}`.
pub fn reply(slot: &Slot, format: Format, vectors: &[Vec<f32>], usage: Option<&Usage>) -> Value {
    let tokens = match usage {
        Some(Usage::Tokens { input, .. }) => *input,
        _ => 0,
    };
    let data: Vec<Value> = vectors
        .iter()
        .enumerate()
        .map(|(index, v)| {
            let embedding = match format {
                Format::Float => json!(v),
                Format::Base64 => json!(base64::engine::general_purpose::STANDARD.encode(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())),
            };
            json!({ "object": "embedding", "index": index, "embedding": embedding })
        })
        .collect();
    let mut out = Map::new();
    out.insert("object".into(), json!("list"));
    out.insert("data".into(), Value::Array(data));
    out.insert("model".into(), json!(slot.upstream.model()));
    out.insert("usage".into(), json!({ "prompt_tokens": tokens, "total_tokens": tokens }));
    out.insert("space".into(), json!(slot.space()));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openai_slot() -> Slot {
        let mut models = json!({ "embeddings": { "provider": "openai", "base_url": "http://127.0.0.1:8080/v1", "model": "BAAI/bge-m3", "dimensions": 3, "version": "2024-01", "max_input_tokens": 8192, "private": true, "price": { "input": 0, "output": 0 } } });
        slot_of(&mut models).unwrap()
    }

    #[test]
    fn a_slot_names_its_space() {
        assert_eq!(slot_of(&mut json!({})).unwrap(), Slot::default());
        let s = openai_slot();
        assert_eq!(s.space(), Space { model: "BAAI/bge-m3", dimensions: 3, version: Some("2024-01") });
        assert!(s.private && s.max_input_tokens == 8192 && Slot::default().max_input_tokens == MAX_INPUT_TOKENS_DEFAULT);
        for (row, why) in [
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3" }), "dimensions"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 0 }), "dimensions"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "version": "a b" }), "version"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "private": "yes" }), "private"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "max_input_tokens": 0 }), "max_input_tokens"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "max_input_tokens": 127 }), "128 to"),
            (json!({ "provider": "workers_ai", "model": "@cf/baai/bge-m3", "dimensions": 1024, "images": true }), "images"),
            (json!({ "provider": "openai", "base_url": "https://x.example/v1", "model": "m", "dimensions": 8, "body": {} }), "body"),
            (json!("bge"), "an object"),
        ] {
            let err = slot_of(&mut json!({ "embeddings": row })).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
    }

    #[test]
    fn a_call_is_bounded_before_it_is_sent() {
        let s = Slot::default();
        let b = bound(&json!({ "input": "hello", "model": "embeddings" }), &s).unwrap();
        assert_eq!((b.inputs, b.format), (vec!["hello".to_string()], Format::Float));
        assert_eq!(bound(&json!({ "input": ["a", "b"], "encoding_format": "base64", "dimensions": 1024, "model": MODEL_DEFAULT }), &s).unwrap().format, Format::Base64);
        let refused = |body: Value| bound(&body, &s).unwrap_err();
        assert_eq!(refused(json!({ "input": [1, 2] })), Refusal::Input);
        assert_eq!(refused(json!({ "input": [] })), Refusal::TooMany(0));
        assert_eq!(refused(json!({ "input": vec!["x"; INPUTS_MAX + 1] })), Refusal::TooMany(INPUTS_MAX + 1));
        assert_eq!(refused(json!({ "input": ["ok", "x".repeat(510)] })), Refusal::TooLong { index: 1, window: 512, tokens: 513 });
        assert!(bound(&json!({ "input": "x".repeat(509) }), &s).is_ok(), "a window's tokens exactly");
        assert_eq!(refused(json!({ "input": " " })), Refusal::Empty);
        assert_eq!(refused(json!({ "input": "x", "model": "cheap" })), Refusal::Model("cheap".into()));
        assert_eq!(refused(json!({ "input": "x", "dimensions": 256 })), Refusal::Dimensions(256));
        assert_eq!(refused(json!({ "input": "x", "encoding_format": "int8" })), Refusal::Format("int8".into()));
        assert!(Refusal::TooMany(0).message().contains("1 to 100"));
    }

    /// Each text's tokens as BGE-M3 reads them, `<s>` and `</s>` included:
    /// counted by its tokenizer.json (BAAI/bge-m3, sha256 21106b6d…dd08)
    /// with Hugging Face tokenizers 0.21, nothing truncated.
    fn bge_m3_read() -> Vec<(String, u64)> {
        let tight = "|a|b|c|\n".repeat(64);
        vec![
            ("1,2,3,4,5,6,7,8,9,0\n".repeat(70), 773),
            ("The release plan keeps every agent running while the cell rolls forward, and a failed check rolls it back before anyone notices. ".repeat(4), 115),
            ("x".repeat(509), 172),
            ("{\"k\":[1,2,3]}".repeat(40), 402),
            (tight[..508].to_string(), 511),
            (tight[..509].to_string(), 512),
            (tight[..510].to_string(), 513),
            ("搜索索引在每次写入后更新，删除的内容不会再出现。".repeat(6), 99),
            ("検索の索引は書き込みのたびに更新されます。".repeat(6), 81),
            ("검색 색인은 쓰기마다 갱신되고 삭제된 내용은 다시 나타나지 않습니다. ".repeat(4), 75),
            ("يتم تحديث فهرس البحث بعد كل كتابة. ".repeat(6), 63),
            ("खोज सूचकांक हर लेखन के बाद अद्यतन होता है। ".repeat(5), 68),
            ("Поисковый индекс обновляется после каждой записи. ".repeat(5), 58),
            ("Ἐν ἀρχῇ ἦν ὁ λόγος. ".repeat(10), 143),
            ("😀🚀👩\u{200d}👩\u{200d}👧\u{200d}👦🇺🇸".repeat(15), 167),
            ("ﷺ".repeat(30), 122),
            ("ﬁ".repeat(120), 122),
            ("e\u{301}".repeat(120), 122),
        ]
    }

    #[test]
    fn the_window_holds_on_real_tokens() {
        for (text, read) in bge_m3_read() {
            assert!(tokens_max(&text) >= read, "{read} tokens read, at most {} bounded: {text:?}", tokens_max(&text));
        }
        // An average bytes a token is no bound: 1,400 bytes of numbers is 773
        // tokens, and 90 bytes of "ﷺ" 122.
        let numbers = "1,2,3,4,5,6,7,8,9,0\n".repeat(70);
        assert!(numbers.len().div_ceil(ESTIMATE_BYTES_A_TOKEN) < 773);
        assert!("ﷺ".repeat(30).len() < 122);
        let s = Slot::default();
        assert_eq!(bound(&json!({ "input": numbers }), &s).unwrap_err(), Refusal::TooLong { index: 0, window: 512, tokens: 1403 });
        assert_eq!(bound(&json!({ "input": "ﷺ".repeat(30) }), &s).unwrap_err(), Refusal::TooLong { index: 0, window: 512, tokens: 993 });
        // At the window exactly, where BGE-M3 reads as many as are bounded.
        let tight = "|a|b|c|\n".repeat(64);
        assert!(bound(&json!({ "input": &tight[..509] }), &s).is_ok());
        assert_eq!(bound(&json!({ "input": &tight[..510] }), &s).unwrap_err(), Refusal::TooLong { index: 0, window: 512, tokens: 513 });
        let multilingual = "Поисковый индекс обновляется после каждой записи. ".repeat(5);
        assert_eq!(tokens_max(&multilingual), 488);
        assert!(bound(&json!({ "input": multilingual }), &Slot { max_input_tokens: 488, ..Slot::default() }).is_ok());
        assert!(bound(&json!({ "input": multilingual }), &Slot { max_input_tokens: 487, ..Slot::default() }).is_err());
    }

    #[test]
    fn a_character_is_read_as_its_longest_form() {
        assert_eq!(char_tokens_max('a'), 1);
        assert_eq!(char_tokens_max('é'), 3, "e and a combining accent");
        assert_eq!(char_tokens_max('ﬁ'), 3, "its own bytes: NFKD's fi is two");
        assert_eq!(char_tokens_max('ﷺ'), CHAR_TOKENS_MAX, "NFKD: eighteen characters");
        assert_eq!(char_tokens_max('한'), 9, "three jamo");
        assert_eq!(tokens_max(""), u64::from(OVERHEAD_TOKENS));
    }

    #[test]
    fn no_character_is_read_as_more_than_char_tokens_max() {
        let most = (0..=0x10FFFF).filter_map(char::from_u32).map(char_tokens_max).max();
        assert_eq!(most, Some(CHAR_TOKENS_MAX));
    }

    #[test]
    fn requests_carry_the_inputs_alone() {
        let inputs = vec!["a".to_string()];
        assert_eq!(request(&Slot::default(), &inputs), json!({ "text": ["a"] }));
        assert_eq!(request(&openai_slot(), &inputs), json!({ "model": "BAAI/bge-m3", "input": ["a"], "encoding_format": "float" }));
    }

    #[test]
    fn answers_are_one_space_and_unit_length() {
        let s = openai_slot();
        let ok = json!({ "data": [{ "index": 1, "embedding": [0.0, 3.0, 4.0] }, { "index": 0, "embedding": [2.0, 0.0, 0.0] }], "usage": { "prompt_tokens": 7 } });
        assert_eq!(answer(&s, 2, &ok).unwrap(), vec![vec![1.0, 0.0, 0.0], vec![0.0, 0.6, 0.8]]);
        assert_eq!(answer(&s, 3, &ok).unwrap_err(), Fault::Count { asked: 3, answered: 2 });
        let long = json!({ "data": [{ "index": 0, "embedding": [1.0, 0.0, 0.0, 0.0] }] });
        assert_eq!(answer(&s, 1, &long).unwrap_err(), Fault::Dimensions { slot: 3, answered: 4 });
        assert_eq!(answer(&s, 1, &json!({ "data": [{ "index": 0, "embedding": [0.0, 0.0, 0.0] }] })).unwrap_err(), Fault::Value);
        assert_eq!(answer(&s, 2, &json!({ "data": [{ "index": 0, "embedding": [1.0, 0.0, 0.0] }, { "index": 0, "embedding": [1.0, 0.0, 0.0] }] })).unwrap_err(), Fault::Shape);
        let w = Slot { dimensions: 2, ..Slot::default() };
        assert_eq!(answer(&w, 1, &json!({ "shape": [1, 2], "data": [[0.0, 2.0]] })).unwrap(), vec![vec![0.0, 1.0]]);
        assert_eq!(answer(&w, 1, &json!({ "result": {} })).unwrap_err(), Fault::Shape);
    }

    #[test]
    fn usage_is_reported_or_estimated() {
        let inputs = vec!["abcdefg".to_string(), "hi".to_string()];
        let reported = usage_of(&openai_slot(), &inputs, &json!({ "usage": { "prompt_tokens": 9 } })).unwrap();
        assert_eq!(reported, Usage::Tokens { model: "BAAI/bge-m3".into(), input: 9, cached_input: 0, cache_write: 0, output: 0 });
        // 7 bytes is 3 tokens estimated, 2 bytes 1
        let counted = usage_of(&Slot::default(), &inputs, &json!({ "data": [] })).unwrap();
        assert_eq!(counted, Usage::Tokens { model: MODEL_DEFAULT.into(), input: 4, cached_input: 0, cache_write: 0, output: 0 });
    }

    #[test]
    fn replies_are_openai_shaped_and_name_their_space() {
        let s = Slot::default();
        let usage = Usage::Tokens { model: MODEL_DEFAULT.into(), input: 5, cached_input: 0, cache_write: 0, output: 0 };
        let r = reply(&s, Format::Float, &[vec![1.0, 0.0]], Some(&usage));
        assert_eq!(r["data"][0], json!({ "object": "embedding", "index": 0, "embedding": [1.0, 0.0] }));
        assert_eq!((&r["model"], &r["usage"]["prompt_tokens"], &r["space"]["dimensions"]), (&json!(MODEL_DEFAULT), &json!(5), &json!(1024)));
        let b = reply(&s, Format::Base64, &[vec![1.0, 0.0]], None);
        let bytes = base64::engine::general_purpose::STANDARD.decode(b["data"][0]["embedding"].as_str().unwrap()).unwrap();
        assert_eq!(bytes, [1.0f32.to_le_bytes(), 0.0f32.to_le_bytes()].concat());
    }
}
