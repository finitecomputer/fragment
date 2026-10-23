//! The model route's speech (Paul, 2026-10-22: "replace ElevenLabs with
//! speech over AI Gateway"): OpenAI's `POST /v1/audio/speech` shape, a JSON
//! body `{model, input, voice?, response_format?, speed?}` with `model` the
//! route's name `speech`, run on OpenAI's tts-1 through the deployment's AI
//! Gateway (Unified Billing, zero data retention), as a transcription runs
//! on Whisper (crate::transcribe), and metered per character at its list
//! price: its characters are its model's input tokens in the price book, as
//! the Clef family's media are (crate::price, `DEFAULT_MODELS`). Pure: the
//! cell reads the body, makes the call and meters it.
//!
//! A call is charged after its answer at the characters it spoke
//! (`usage_of`), nothing held before it. The answer is the audio itself,
//! as OpenAI's API answers it, in the format asked for.

use base64::Engine;
use serde_json::{json, Value};

use crate::price::Usage;

/// The route's name for speech: no tier, as `whisper` is none.
pub const SPEECH: &str = "speech";
/// The model, by its id in the AI model catalog
/// (developers.cloudflare.com/ai/models/openai/tts-1, read 2026-10-22:
/// third-party, zero data retention, $0.000015 a character; its schema
/// `{text, voice, response_format, speed}` in, every field required and no
/// other, `{audio: <the generated file's URL>}` out). It speaks every
/// language Whisper hears, in each of its voices.
pub const SPEECH_MODEL: &str = "openai/tts-1";
/// What a call may say, at most (tts-1's own `maxLength`): Hermes splits a
/// longer reply into parts of at most this (its `openai` provider's limit).
pub const INPUT_MAX_CHARS: usize = 4_096;
/// The whole body: the text, at most 4 bytes a character, and its fields.
pub const BODY_MAX_BYTES: usize = 4 * INPUT_MAX_CHARS + 16 * 1024;
/// tts-1's voices; any of them speaks any language.
pub const VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];
/// The voice when none is named (OpenAI's and tts-1's default).
pub const VOICE_DEFAULT: &str = "alloy";
/// The most audio an answer carries: 4,096 characters are about five
/// minutes of speech, 14 MB as tts-1's 24 kHz WAV, 5 MB as MP3.
pub const AUDIO_MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most an answer that names its audio inline (a `data:` URI in JSON)
/// may be: the audio's base64 and a little more.
pub const ANSWER_MAX_BYTES: usize = AUDIO_MAX_BYTES / 3 * 4 + 64 * 1024;
const _: () = assert!(ANSWER_MAX_BYTES > AUDIO_MAX_BYTES / 3 * 4, "inline audio fits its answer");
/// A `speed`'s bounds (tts-1's).
pub const SPEED_MIN: f64 = 0.25;
pub const SPEED_MAX: f64 = 4.0;

/// Why a speech call is refused before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The body is not a JSON object.
    Body,
    /// `model` is not `speech`.
    Model,
    /// No `input`, an empty one, or one over `INPUT_MAX_CHARS`.
    Input,
    /// A `voice` that is not one of tts-1's.
    Voice,
    /// A `response_format` tts-1 does not write.
    Format,
    /// A `speed` that is not a number from 0.25 to 4.
    Speed,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::Body => "speech is a JSON object: {model, input, voice?, response_format?, speed?}".into(),
            Refusal::Model => format!("speech's model is {SPEECH:?}"),
            Refusal::Input => format!("input is the text to speak, 1 to {INPUT_MAX_CHARS} characters"),
            Refusal::Voice => format!("voice is one of {}", VOICES.join(", ")),
            Refusal::Format => "response_format is mp3, opus, aac, flac or wav".into(),
            // its bounds as text: formatting a float would cost the cell's wasm a float printer
            Refusal::Speed => "speed is a number from 0.25 to 4".into(),
        }
    }
}

/// The audio's container, as asked for (OpenAI's names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Opus,
    Aac,
    Flac,
    Wav,
}

impl Format {
    fn parse(s: &str) -> Option<Format> {
        Some(match s {
            "mp3" => Format::Mp3,
            "opus" => Format::Opus,
            "aac" => Format::Aac,
            "flac" => Format::Flac,
            "wav" => Format::Wav,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::Opus => "opus",
            Format::Aac => "aac",
            Format::Flac => "flac",
            Format::Wav => "wav",
        }
    }

    /// The answer's content type (OpenAI's: its opus is Ogg Opus).
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Mp3 => "audio/mpeg",
            Format::Opus => "audio/ogg",
            Format::Aac => "audio/aac",
            Format::Flac => "audio/flac",
            Format::Wav => "audio/wav",
        }
    }
}

/// A speech call bounded: the model's input, the audio's format, and the
/// characters it speaks, which it is charged at.
#[derive(Debug, Clone, PartialEq)]
pub struct Bounded {
    pub input: Value,
    pub format: Format,
    pub chars: u64,
}

/// A body, bounded: its text, voice, format and speed as tts-1's input,
/// nothing else (OpenAI's `instructions`, `stream_format`, and Hermes'
/// `lang_code` have no tts-1 input, and are let go: tts-1 speaks the text's
/// own language).
pub fn bound(body: &Value) -> Result<Bounded, Refusal> {
    let o = body.as_object().ok_or(Refusal::Body)?;
    match o.get("model").and_then(Value::as_str) {
        Some(m) if m.trim() == SPEECH => {}
        _ => return Err(Refusal::Model),
    }
    let text = o.get("input").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).ok_or(Refusal::Input)?;
    let chars = text.chars().count();
    if chars > INPUT_MAX_CHARS {
        return Err(Refusal::Input);
    }
    let voice = match o.get("voice") {
        None | Some(Value::Null) => VOICE_DEFAULT,
        Some(v) => v.as_str().map(str::trim).filter(|v| VOICES.contains(v)).ok_or(Refusal::Voice)?,
    };
    let format = match o.get("response_format") {
        None | Some(Value::Null) => Format::Mp3,
        Some(f) => f.as_str().and_then(|f| Format::parse(f.trim())).ok_or(Refusal::Format)?,
    };
    let speed = match o.get("speed") {
        None | Some(Value::Null) => 1.0,
        Some(s) => s.as_f64().filter(|s| (SPEED_MIN..=SPEED_MAX).contains(s)).ok_or(Refusal::Speed)?,
    };
    Ok(Bounded { input: json!({ "text": text, "voice": voice, "response_format": format.name(), "speed": speed }), format, chars: chars as u64 })
}

/// What a call cost: the characters it spoke, as its model's input tokens
/// (the book's row for `SPEECH_MODEL` prices them at a character's price).
pub fn usage_of(chars: u64) -> Usage {
    Usage::Tokens { model: SPEECH_MODEL.into(), input: chars, cached_input: 0, cache_write: 0, output: 0 }
}

/// Where an answer's audio is, from its JSON: the catalog's `audio` (the
/// binding's answer), or inside `result` (the REST API's wrapping), a URL
/// of the generated file or the audio inline as a `data:` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audio {
    /// Fetched by the cell (an `https` URL, or the fake's own).
    At(String),
    Inline(Vec<u8>),
}

/// The audio an answer names, or `None` when it names none the route reads.
pub fn audio_of(answer: &Value) -> Option<Audio> {
    let named = answer["audio"].as_str().or_else(|| answer["result"]["audio"].as_str())?;
    if let Some(data) = named.strip_prefix("data:") {
        let (head, b64) = data.split_once(',')?;
        if !head.ends_with(";base64") {
            return None;
        }
        return base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok().map(Audio::Inline);
    }
    Some(Audio::At(named.to_string()))
}

/// What a speech answer says of the model's (the route's
/// `x-fragment-answer-shape`), never its audio or a URL: its content type,
/// and, when it is JSON, its top-level keys and `result`'s, so a hosted run
/// shows how the gateway answers.
pub fn shape(content_type: &str, answer: Option<&Value>) -> Value {
    let keys = |v: &Value| -> Vec<String> { v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() };
    match answer {
        Some(a) => json!({ "type": content_type, "keys": keys(a), "result": keys(&a["result"]) }),
        None => json!({ "type": content_type }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::PriceBook;

    /// Valid: an OpenAI SDK's body (as Hermes' `text_to_speech` sends it) is
    /// tts-1's input, every field it requires set, what it has no input for
    /// let go; the characters counted as a person reads them.
    #[test]
    fn a_body_is_tts_1s_input() {
        let b = bound(&json!({ "model": "speech", "input": "Hello there.", "voice": "nova", "response_format": "mp3", "instructions": "cheerful", "lang_code": "en" })).unwrap();
        assert_eq!(b.input, json!({ "text": "Hello there.", "voice": "nova", "response_format": "mp3", "speed": 1.0 }));
        assert_eq!((b.format, b.chars), (Format::Mp3, 12));
        let b = bound(&json!({ "model": " speech ", "input": "Grüß dich, wie geht's? 你好", "response_format": "opus", "speed": 1.25 })).unwrap();
        assert_eq!(b.input, json!({ "text": "Grüß dich, wie geht's? 你好", "voice": VOICE_DEFAULT, "response_format": "opus", "speed": 1.25 }), "no voice: the default");
        assert_eq!((b.format.content_type(), b.chars), ("audio/ogg", 25), "characters, not bytes");
        let at_most = "a".repeat(INPUT_MAX_CHARS);
        assert!(bound(&json!({ "model": "speech", "input": at_most, "voice": null, "speed": null })).is_ok(), "the bound itself is taken");
    }

    /// Invalid: what is no speech the route takes is refused before
    /// anything is sent, saying why.
    #[test]
    fn a_body_out_of_shape_is_refused() {
        let ok = json!({ "model": "speech", "input": "hi" });
        let with = |k: &str, v: Value| {
            let mut b = ok.clone();
            b[k] = v;
            bound(&b)
        };
        assert_eq!(bound(&json!("hi")), Err(Refusal::Body));
        for m in [json!("tts-1"), json!("gpt-4o-mini-tts"), json!("cheap"), json!(null), json!(1)] {
            assert_eq!(with("model", m.clone()), Err(Refusal::Model), "{m}: OpenAI's own ids are no names of ours");
        }
        for i in [json!(""), json!("   "), json!(null), json!(["hi"]), json!("a".repeat(INPUT_MAX_CHARS + 1))] {
            assert_eq!(with("input", i), Err(Refusal::Input));
        }
        for v in [json!("coral"), json!("Alloy"), json!(""), json!(3)] {
            assert_eq!(with("voice", v.clone()), Err(Refusal::Voice), "{v}: not one of tts-1's");
        }
        for f in [json!("pcm"), json!("MP3"), json!("ogg"), json!(true)] {
            assert_eq!(with("response_format", f.clone()), Err(Refusal::Format), "{f}");
        }
        for s in [json!(0.2), json!(4.5), json!("1"), json!(-1)] {
            assert_eq!(with("speed", s.clone()), Err(Refusal::Speed), "{s}");
        }
        assert!(Refusal::Voice.message().contains("alloy, echo"));
    }

    /// Goal: a call is charged per character at tts-1's list price, $15 a
    /// million, before the fee and margin. Method: the book's price of a
    /// thousand characters.
    #[test]
    fn a_character_is_tts_1s_price() {
        let book = PriceBook::defaults();
        let priced = book.price(&usage_of(1_000)).unwrap();
        assert_eq!(priced.list, 15_000, "1,000 characters at $0.000015: $0.015");
        assert_eq!(book.price(&usage_of(0)).unwrap().list, 0);
    }

    /// The audio an answer names: the catalog's URL, bare or in the REST
    /// API's wrapping, or inline; anything else is none.
    #[test]
    fn an_answer_names_its_audio() {
        let url = "https://example.aig.cloudflare.com/speech.mp3";
        assert_eq!(audio_of(&json!({ "audio": url })), Some(Audio::At(url.into())));
        assert_eq!(audio_of(&json!({ "state": "Completed", "result": { "audio": url } })), Some(Audio::At(url.into())));
        let inline = format!("data:audio/mpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b"ID3audio"));
        assert_eq!(audio_of(&json!({ "audio": inline })), Some(Audio::Inline(b"ID3audio".to_vec())));
        for none in [json!({}), json!({ "result": {} }), json!({ "audio": 3 }), json!({ "audio": "data:audio/mpeg,raw" }), json!({ "audio": "data:audio/mpeg;base64,%%%" })] {
            assert_eq!(audio_of(&none), None, "{none}");
        }
    }

    /// The shape names keys and the content type, never the URL or audio.
    #[test]
    fn the_shape_shows_keys_never_the_audio() {
        let a = json!({ "state": "Completed", "result": { "audio": "https://secret.example/x.mp3" } });
        let s = shape("application/json", Some(&a));
        assert_eq!(s, json!({ "type": "application/json", "keys": ["result", "state"], "result": ["audio"] }));
        assert!(!s.to_string().contains("secret.example"));
        assert_eq!(shape("audio/mpeg", None), json!({ "type": "audio/mpeg" }));
    }
}
