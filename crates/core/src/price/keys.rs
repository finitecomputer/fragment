//! An operator key's calls: the parts priced apart, and what one call uses (`key_usage`).

use super::defaults::DEFAULT_KEYS;
use super::usage::Usage;

/// A part of an operator key's calls priced apart from its others, by what
/// its vendor counts (`key_usage` says which calls are the part, and what
/// each uses), as `priced_as`, its price's name in `DEFAULT_KEYS`. Its name
/// has a `:`, which no catalog row's has, so it is never a provider's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPart {
    pub key: &'static str,
    pub priced_as: &'static str,
    /// The most units one call is charged: its vendor's own limit for one
    /// request, so even a call it answered is never charged for more than
    /// it would take.
    pub units_max: u64,
}

/// The parts the book prices apart (FIN-443 and FIN-445, 2026-11-07):
///
/// - ElevenLabs' text to speech, its streaming and `with-timestamps` calls
///   too, was charged as a minute of music, 10 to 20 times what a short
///   clip costs. A call takes at most 40,000 characters (Flash's limit;
///   the multilingual models take 10,000 and v3 5,000:
///   elevenlabs.io/pricing/api, read 2026-11-07), so one is charged at
///   most $3.20 at list.
/// - fal's generations were charged as an image, and so was each of the
///   SDK's queue polls, its status checks and its result: an image cost
///   several images. Only a synchronous Klein run is charged now, at most
///   16 megapixels a call, $0.096 at list: Klein makes at most 4 images a
///   call (`num_images`), and its largest preset is 1 megapixel.
/// - xAI's chat and image calls were charged as an X search, $0.18 a call.
///   An image is charged one a call now (a request's count is what it
///   asks for, not what it made), and a chat nothing (its tokens are not
///   read).
/// - Google Places' photos were charged as an Enterprise text search,
///   $35.00 per 1,000, 5 times their list. Its searches and details,
///   whose tier is their field mask's (the meter reads no header), and its
///   autocompletes, free in a session that ends in a Details call, are
///   charged nothing now.
/// - Firecrawl's calls were charged as a search, 2 credits: a scrape, 1,
///   twice its list, and each crawl or batch's status poll a search. A
///   scrape and a map are charged their credits now, at most 1,000 a call
///   ($5 at list); a search, its returned results not counted, nothing.
pub const KEY_PARTS: [KeyPart; 5] = [
    KeyPart { key: "elevenlabs", priced_as: "elevenlabs:speech", units_max: 40_000 },
    KeyPart { key: "fal", priced_as: "fal:image", units_max: 16 * 1_048_576 },
    KeyPart { key: "xai", priced_as: "xai:image", units_max: 1 },
    KeyPart { key: "google-places", priced_as: "google-places:photo", units_max: 1 },
    KeyPart { key: "firecrawl", priced_as: "firecrawl:credit", units_max: 1_000 },
];

/// The list price of operator key `name` per call, when the price book
/// has one: `(micros, per calls)`.
pub fn default_key_price(name: &str) -> Option<(i64, u64)> {
    DEFAULT_KEYS.iter().find(|(n, _, _)| *n == name).map(|(_, micros, per)| (*micros, *per))
}

/// The largest body whose characters a part's call is read for: ElevenLabs
/// takes at most 40,000 characters a call (Flash's limit), 240,000 bytes
/// even with every one a JSON escape (`\uXXXX`), so a body past this is
/// one its vendor refuses, and is not parsed (the swap's own limit is 32
/// MiB).
pub const KEY_PART_BODY_MAX_BYTES: usize = 1024 * 1024;

/// One call on an operator key as the swap meters it, once answered: its
/// method, host (lowercase), path and body as the guest sent them, and its
/// answer's status (under 500: the swap charges no call its provider
/// failed).
#[derive(Debug, Clone, Copy)]
pub struct KeyCall<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
    pub status: u16,
}

impl KeyCall<'_> {
    /// Its body's JSON object, when it is one to read.
    fn said(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        match (self.body.len() <= KEY_PART_BODY_MAX_BYTES).then(|| serde_json::from_slice(self.body).ok()).flatten() {
            Some(serde_json::Value::Object(said)) => Some(said),
            _ => None,
        }
    }
}

/// FLUX.2 Klein 9B's text to image, run synchronously (`fal.run`): the one
/// fal call known to have made what it asked for, at a rate the book
/// knows. A queued one (`queue.fal.run`) is made after its submit, or
/// fails or is cancelled, so is charged nothing until its result can be
/// settled (the debt ledger); an edit's size is its input image's, which
/// the meter does not see.
const FAL_KLEIN_RUN: (&str, &str) = ("fal.run", "/fal-ai/flux-2/klein/9b");

/// xAI's image models, each priced at least `xai:image`'s $0.02 an image.
const XAI_IMAGE_MODELS: [&str; 2] = ["grok-imagine-image", "grok-imagine-image-2.0"];

/// fal's `image_size` presets in pixels (FLUX.2 Klein's, and its other
/// image models'), the first Klein's default.
const FAL_SIZES: [(&str, u64); 6] = [
    ("landscape_4_3", 1024 * 768),
    ("landscape_16_9", 1024 * 576),
    ("square_hd", 1024 * 1024),
    ("square", 512 * 512),
    ("portrait_4_3", 768 * 1024),
    ("portrait_16_9", 576 * 1024),
];

/// The pixels a Klein run asks for: its `num_images` (absent, one) images
/// of its `image_size`, a preset, `{width, height}`, or absent, Klein's
/// default; none for any other (an unknown preset, a size that is no two
/// whole numbers).
fn fal_pixels(said: &serde_json::Map<String, serde_json::Value>) -> Option<u64> {
    let each = match said.get("image_size") {
        None => FAL_SIZES[0].1,
        Some(serde_json::Value::String(preset)) => FAL_SIZES.iter().find(|(name, _)| name == preset)?.1,
        Some(size) => size.get("width")?.as_u64()?.checked_mul(size.get("height")?.as_u64()?)?,
    };
    let images = said.get("num_images").map_or(Some(1), serde_json::Value::as_u64)?;
    each.checked_mul(images)
}

/// Firecrawl's formats that cost 4 credits a page more than a scrape's 1:
/// an LLM's reading (docs.firecrawl.dev/billing, read 2026-11-07).
/// Each is its kind (its bit), charged once a page however often it is
/// named: JSON and v1's `extract` are one, a question and a query one.
const FIRECRAWL_LLM_FORMATS: [(&str, u8); 7] = [("json", 0), ("extract", 0), ("question", 1), ("query", 1), ("highlights", 2), ("audio", 3), ("video", 4)];

/// The credits a page scraped with `formats` costs (strings, or objects
/// naming their `type`; absent, markdown alone): 1, and 4 for each kind
/// of LLM format it names.
fn firecrawl_page(formats: Option<&serde_json::Value>) -> u64 {
    let kinds = formats.and_then(serde_json::Value::as_array).map_or(0u8, |all| {
        all.iter()
            .filter_map(|f| f.as_str().or_else(|| f.get("type")?.as_str()))
            .filter_map(|name| FIRECRAWL_LLM_FORMATS.iter().find(|(llm, _)| *llm == name))
            .fold(0, |kinds, (_, kind)| kinds | 1 << kind)
    });
    1 + 4 * u64::from(kinds.count_ones())
}

/// What one call on operator key `key` uses, as the swap meters it: a
/// part's call (`KEY_PARTS`) its units, at most its `units_max`, as the
/// part; any other call on a key priced by its parts (fal's, xAI's) none of the
/// key's; any other one call of the key.
///
/// - ElevenLabs' text to speech (`/v1/text-to-speech/…`): the characters
///   of its JSON body's `text`.
/// - fal: a synchronous Klein run (`FAL_KLEIN_RUN`, a `POST`): the pixels
///   it asks for (`fal_pixels`).
/// - xAI: a `POST` to `/v1/responses` or `/v1/chat/completions` whose
///   `tools` name `x_search` is one call of the key (an X search); one to
///   `/v1/images/generations` naming a grok-imagine-image model, made in
///   its answer (not `deferred`), one `xai:image`; any other call (a chat,
///   its tokens not read; a deferred image) none.
/// - Google Places (New): a place's photo (`…/photos/<photo>/media`) one
///   of its part; a search or a place's details (their tier their field
///   mask's, which the meter does not read), an autocomplete (free in a
///   session that ends in a Details call) and any other call none.
/// - X: a search (`/2/tweets/search/recent` or `/all`) is one call of the
///   key, ten posts, as before; a lookup (its returned resources not
///   counted) and any other call none.
/// - Browser Use: a `POST` that starts a new browser session or task
///   (`/api/v<n>/browsers`, `/sessions`, `/tasks` or `/runs`, its JSON
///   body read and naming no `sessionId`) is one call of the key; a task
///   on a session it has, its status, its stop and any other call none.
/// - Firecrawl (v1 and v2): a scrape and a map (their JSON bodies read)
///   their credits, as `firecrawl:credit`; any other call (a search, its
///   returned results not counted; a crawl, a batch scrape or an extract,
///   billed as each page completes; a status poll, free) none.
///
/// A part's call uses none, so is charged nothing, when its vendor
/// refused it (any answer but a 2xx: a bad request, a missing voice, a
/// rate limit, which made nothing) or its units cannot be read (a body
/// that is no JSON object saying them, one past
/// `KEY_PART_BODY_MAX_BYTES`, or none, as a socket's upgrade): billing is
/// best effort, and a gap charges zero rather than a guess above what the
/// call cost (decision 62).
pub fn key_usage(key: &str, call: &KeyCall) -> Usage {
    let (priced_as, units) = match key {
        "elevenlabs" if call.path.starts_with("/v1/text-to-speech/") => ("elevenlabs:speech", call.said().and_then(|said| Some(said.get("text")?.as_str()?.chars().count() as u64))),
        "fal" if call.method == "POST" && (call.host, call.path) == FAL_KLEIN_RUN => ("fal:image", call.said().as_ref().and_then(fal_pixels)),
        "fal" => return Usage::Key { key: key.to_string(), units: 0 },
        "xai" => {
            let said = call.said();
            let searches = said.as_ref().and_then(|s| s.get("tools")?.as_array()).is_some_and(|tools| tools.iter().any(|t| t.get("type").and_then(serde_json::Value::as_str) == Some("x_search")));
            match (call.method, call.path) {
                ("POST", "/v1/responses" | "/v1/chat/completions") if searches => ("xai", Some(1)),
                // made in its answer: a known model, not deferred (a request id
                // alone, the image made later or not)
                ("POST", "/v1/images/generations")
                    if said.as_ref().is_some_and(|s| s.get("model").and_then(serde_json::Value::as_str).is_some_and(|model| XAI_IMAGE_MODELS.contains(&model)) && s.get("deferred").is_none_or(|d| *d == serde_json::Value::Bool(false))) =>
                {
                    ("xai:image", said.and_then(|s| s.get("n").map_or(Some(1), serde_json::Value::as_u64)).map(|n| n.min(1)))
                }
                // a chat (its tokens not read), a video, a stored response read, a model list
                _ => return Usage::Key { key: key.to_string(), units: 0 },
            }
        }
        // the method is the path's (Google refuses any other), so not read;
        // a search's or a place's details' tier is its field mask's, and an
        // autocomplete is free in a session that ends in a Details call: none
        "google-places" => match call.path.strip_prefix("/v1/places/") {
            Some(photo) if photo.contains("/photos/") && photo.ends_with("/media") => ("google-places:photo", Some(1)),
            _ => return Usage::Key { key: key.to_string(), units: 0 },
        },
        "firecrawl" => {
            let said = call.said();
            let what = call.path.strip_prefix("/v1/").or_else(|| call.path.strip_prefix("/v2/"));
            let credits = match (call.method, what) {
                // a page, its formats read; a body unread, none
                ("POST", Some("scrape")) => said.as_ref().map(|s| firecrawl_page(s.get("formats"))),
                ("POST", Some("map")) => said.as_ref().map(|_| 1),
                _ => return Usage::Key { key: key.to_string(), units: 0 },
            };
            ("firecrawl:credit", credits)
        }
        // a search, ten posts, as before; a lookup (its returned resources
        // not counted), a timeline, a write: none
        "x" => match (call.method, call.path) {
            ("GET", "/2/tweets/search/recent" | "/2/tweets/search/all") => ("x", Some(1)),
            _ => return Usage::Key { key: key.to_string(), units: 0 },
        },
        "browser-use" => {
            // `/api/v<n>/<what>`, any version
            let what = call.path.strip_prefix("/api/v").and_then(|p| p.split_once('/')).filter(|(v, _)| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())).map(|(_, what)| what);
            // a new one: its body read, naming no session it continues (v2's
            // and v4's `sessionId`; v3 dispatches to it)
            let new = call.said().is_some_and(|said| ["sessionId", "session_id"].iter().all(|name| said.get(*name).is_none_or(serde_json::Value::is_null)));
            match (call.method, what) {
                ("POST", Some("browsers" | "sessions" | "tasks" | "runs")) if new => ("browser-use", Some(1)),
                _ => return Usage::Key { key: key.to_string(), units: 0 },
            }
        }
        _ => return Usage::Key { key: key.to_string(), units: 1 },
    };
    // a part's, or one of its key's own (xAI's X search), made by a 2xx
    let most = KEY_PARTS.iter().find(|p| p.priced_as == priced_as).map_or(1, |p| p.units_max);
    let made = (200..300).contains(&call.status);
    Usage::Key { key: priced_as.to_string(), units: units.filter(|_| made).map_or(0, |u| u.min(most)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::book::{KeyPrice, PriceBook, PriceError};
    use crate::price::limits::NAME_MAX_BYTES;
    use crate::price::usage::printable;

    /// Goal: each part the book prices apart is priced once, in
    /// `DEFAULT_KEYS`, as is its key, and its name is no provider's.
    #[test]
    fn each_key_part_has_one_price_and_no_providers_name() {
        for p in KEY_PARTS {
            assert!(default_key_price(p.key).is_some(), "{p:?}: its key's other calls are priced");
            assert!(default_key_price(p.priced_as).is_some(), "{p:?}: priced");
            assert_eq!(DEFAULT_KEYS.iter().filter(|(n, _, _)| *n == p.priced_as).count(), 1, "{p:?}: once");
            assert!(p.priced_as.starts_with(&format!("{}:", p.key)) && !crate::catalog::valid_name(p.priced_as), "{p:?}: never a catalog row's name");
            assert!(printable(p.priced_as, NAME_MAX_BYTES), "{p:?}: a name the ledger stores");
            assert!(p.units_max >= 1, "{p:?}: a call it made is charged");
        }
        // speech's most characters fit the body read, each a JSON escape
        let speech = KEY_PARTS.iter().find(|p| p.priced_as == "elevenlabs:speech").expect("speech is a part");
        assert!(speech.units_max as usize * 6 + 1024 <= KEY_PART_BODY_MAX_BYTES);
    }

    /// A JSON `POST` on `key` to `path`, answered `status`, as the swap
    /// meters it.
    fn posted(key: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage(key, &KeyCall { method: "POST", host: "api.test", path, body, status })
    }

    /// Goal (FIN-443): ElevenLabs' text to speech is metered by its text's
    /// characters, each of its endpoints, and its music, as every other
    /// call on the key, as one call (a minute). Method: the calls Hermes'
    /// `text_to_speech` and the music skill make, priced as a deployment's
    /// book prices them.
    #[test]
    fn speech_is_metered_by_its_characters_and_music_by_the_call() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let reply = "a".repeat(200);
        let body = serde_json::to_vec(&serde_json::json!({ "text": reply, "model_id": "eleven_multilingual_v2" })).unwrap();
        let spoken = Usage::Key { key: "elevenlabs:speech".into(), units: 200 };
        for path in ["/v1/text-to-speech/pNInz6obpgDQGcFmaJgB", "/v1/text-to-speech/v/stream", "/v1/text-to-speech/v/with-timestamps", "/v1/text-to-speech/v/stream/with-timestamps"] {
            assert_eq!(posted("elevenlabs", path, &body, 200), spoken, "{path}");
        }
        assert_eq!(book.price(&spoken).unwrap().charge, 24_000, "$0.024, not a minute of music's $0.225");
        let music = serde_json::to_vec(&serde_json::json!({ "prompt": "a jingle", "music_length_ms": 15_000 })).unwrap();
        let minute = Usage::Key { key: "elevenlabs".into(), units: 1 };
        assert_eq!(posted("elevenlabs", "/v1/music", &music, 200), minute);
        assert_eq!(posted("elevenlabs", "/v1/music/plan", &music, 200), minute);
        assert_eq!(book.price(&minute).unwrap().charge, 225_000);
        // characters, not bytes: ElevenLabs bills each character
        let accented = serde_json::to_vec(&serde_json::json!({ "text": "héllo, 世界" })).unwrap();
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &accented, 200), Usage::Key { key: "elevenlabs:speech".into(), units: 9 });
        // the path's own segment: a sibling endpoint is no speech
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech-other", &body, 200), minute);
        assert_eq!(posted("elevenlabs", "/v2/text-to-speech/v", &body, 200), minute);
        // only the key the part names
        assert_eq!(posted("perplexity", "/v1/text-to-speech/v", &body, 200), Usage::Key { key: "perplexity".into(), units: 1 });
    }

    /// Goal (FIN-443's review): a speech call ElevenLabs refused made
    /// nothing, so is charged nothing, whatever its text; one it answered
    /// is charged at most a request's limit; every other call on the key is
    /// a call, as the swap charges any answer under 500. Method: a 10,000
    /// character request at each answer the swap meters.
    #[test]
    fn a_refused_speech_call_is_charged_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let long = serde_json::to_vec(&serde_json::json!({ "text": "a".repeat(10_000) })).unwrap();
        let spoken = |status| posted("elevenlabs", "/v1/text-to-speech/v/stream", &long, status);
        for made in [200, 201, 299] {
            assert_eq!(spoken(made), Usage::Key { key: "elevenlabs:speech".into(), units: 10_000 }, "{made}");
        }
        assert_eq!(book.price(&spoken(200)).unwrap().charge, 1_200_000, "10,000 characters made: $0.80 at list");
        // a bad request, no key, no voice, too long, a rate or concurrency limit, a redirect not followed
        for refused in [400, 401, 402, 403, 404, 409, 413, 422, 429, 302, 304, 100, 0] {
            let u = spoken(refused);
            assert_eq!(u, Usage::Key { key: "elevenlabs:speech".into(), units: 0 }, "{refused}");
            assert_eq!(book.price(&u).unwrap().charge, 0, "{refused}");
        }
        // a 2xx is charged at most a request's limit: 40,000 characters, $4.80
        let huge = serde_json::to_vec(&serde_json::json!({ "text": "a".repeat(1_000_000) })).unwrap();
        let most = posted("elevenlabs", "/v1/text-to-speech/v", &huge, 200);
        assert_eq!(most, Usage::Key { key: "elevenlabs:speech".into(), units: 40_000 });
        assert_eq!(book.price(&most).unwrap().charge, 4_800_000);
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &huge, 413), Usage::Key { key: "elevenlabs:speech".into(), units: 0 }, "refused as too long: nothing");
        // music, and any other call, is one call at every answer the swap meters
        for status in [200, 400, 429] {
            assert_eq!(posted("elevenlabs", "/v1/music", b"{}", status), Usage::Key { key: "elevenlabs".into(), units: 1 }, "{status}");
        }
    }

    /// Goal: a speech call whose characters the meter cannot read is
    /// charged nothing, never a guess above its cost (decision 62).
    /// Method: bodies that are no JSON object holding `text` as a string,
    /// and none (a socket's upgrade, `stream-input`).
    #[test]
    fn a_speech_call_the_meter_cannot_read_is_charged_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let none = Usage::Key { key: "elevenlabs:speech".into(), units: 0 };
        for body in [&b""[..], b"text=hello", b"{\"text\":", b"[\"hello\"]", b"\"hello\"", b"{\"text\":42}", b"{\"text\":null}", b"{\"words\":\"hello\"}", &[0xff, 0xfe][..]] {
            assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v/stream-input", body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        assert_eq!(book.price(&none).unwrap().charge, 0);
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", b"{\"text\":\"\"}", 200), none, "an empty text has no characters");
        // the largest body read, and one past it
        let sized = |n: usize| format!("{{\"text\":\"{}\"}}", "a".repeat(n - 11)).into_bytes();
        assert_eq!(sized(KEY_PART_BODY_MAX_BYTES).len(), KEY_PART_BODY_MAX_BYTES);
        let most = Usage::Key { key: "elevenlabs:speech".into(), units: 40_000 };
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &sized(KEY_PART_BODY_MAX_BYTES), 200), most, "read, at most its vendor's limit");
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &sized(KEY_PART_BODY_MAX_BYTES + 1), 200), none, "past it, not parsed");
        // a release whose book has no part's price refuses its rows, as it
        // does any it cannot price: the ledger charges them nothing
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "elevenlabs:speech").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "elevenlabs:speech".into(), units: 200 }), Err(PriceError::NoPrice));
    }

    /// A call on fal's key, answered `status`, as the swap meters it.
    fn fal(method: &str, host: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("fal", &KeyCall { method, host, path, body, status })
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): only a
    /// call known to have made a known model's image is charged, a
    /// synchronous FLUX.2 Klein 9B run, by the pixels it asks for; a queued
    /// submit (fal makes it later, or fails or cancels it), its polls and
    /// result, an edit, another model and every other call nothing.
    /// Method: the calls Hermes' `image_generate` makes for one image
    /// (`fal_client` 0.13: a submit, a status poll every 0.25 s and up,
    /// the result), and a synchronous run of the same request.
    #[test]
    fn a_fal_image_is_charged_only_when_a_known_model_made_it() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let hermes = br#"{"prompt":"a lighthouse at dusk","image_size":"landscape_16_9","num_inference_steps":4,"output_format":"png","enable_safety_checker":false}"#;
        let made = Usage::Key { key: "fal:image".into(), units: 1024 * 576 };
        assert_eq!(fal("POST", "fal.run", "/fal-ai/flux-2/klein/9b", hermes, 200), made, "a synchronous run: its answer is the image");
        assert_eq!(book.price(&made).unwrap().charge, 5_063, "$0.0051");
        let free = Usage::Key { key: "fal".into(), units: 0 };
        let around = [
            ("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/status"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/status/stream"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e"),
            ("PUT", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/cancel"),
            ("POST", "rest.fal.ai", "/storage/upload/initiate"),
            ("POST", "rest.fal.ai", "/storage/auth/token"),
            ("GET", "fal.run", "/fal-ai/flux-2/klein/9b"),
            ("POST", "fal.run", "/fal-ai/flux-2/klein/9b/edit"),
            ("POST", "fal.run", "/fal-ai/flux-2/klein/4b"),
            ("POST", "fal.run", "/fal-ai/flux-2-pro"),
            ("POST", "fal.run", "/fal-ai/flux-2/klein/9b/"),
        ];
        for (method, host, path) in around {
            assert_eq!(fal(method, host, path, hermes, 200), free, "{method} {host}{path}");
        }
        // Hermes' queued image (a submit, six polls, the result) is charged
        // nothing until its result can be settled, once eight images' $0.072
        assert_eq!(8 * book.price(&Usage::Key { key: "fal".into(), units: 1 }).unwrap().charge, 72_000);
        assert_eq!(book.price(&free).unwrap().charge, 0);
        // the presets, a size of its own, several images, and none named
        let sized = |body: &str| fal("POST", "fal.run", "/fal-ai/flux-2/klein/9b", body.as_bytes(), 200);
        let units = |units| Usage::Key { key: "fal:image".into(), units };
        for (preset, pixels) in [("square_hd", 1_048_576), ("square", 262_144), ("portrait_4_3", 786_432), ("portrait_16_9", 589_824), ("landscape_4_3", 786_432), ("landscape_16_9", 589_824)] {
            assert_eq!(sized(&format!(r#"{{"prompt":"p","image_size":"{preset}"}}"#)), units(pixels), "{preset}");
        }
        assert_eq!(sized(r#"{"prompt":"p","image_size":{"width":1920,"height":1080}}"#), units(1920 * 1080));
        assert_eq!(sized(r#"{"prompt":"p","image_size":"square_hd","num_images":4}"#), units(4 * 1_048_576));
        assert_eq!(sized(r#"{"prompt":"p"}"#), units(1024 * 768), "Klein's default, landscape_4_3");
        // only fal's key
        assert_eq!(key_usage("perplexity", &KeyCall { method: "POST", host: "fal.run", path: "/fal-ai/flux-2/klein/9b", body: hermes, status: 200 }), Usage::Key { key: "perplexity".into(), units: 1 });
    }

    /// Goal: a Klein run the meter cannot read, or fal refused, is charged
    /// nothing, and one it made at most a call's most (decision 62); a
    /// release whose book has no image price charges its rows nothing.
    /// Method: bodies that say no size, every answer the swap meters, and
    /// sizes past the cap.
    #[test]
    fn a_fal_call_the_meter_cannot_read_or_fal_refused_is_charged_nothing() {
        let none = Usage::Key { key: "fal:image".into(), units: 0 };
        let run = |body: &[u8], status| fal("POST", "fal.run", "/fal-ai/flux-2/klein/9b", body, status);
        for body in [&b""[..], b"prompt=p", b"{\"prompt\":", b"[\"p\"]", b"\"p\"", br#"{"image_size":"poster"}"#, br#"{"image_size":{"width":1024}}"#, br#"{"image_size":{"width":-1,"height":1024}}"#, br#"{"image_size":{"width":1.5,"height":1024}}"#, br#"{"image_size":{"width":"1024","height":"1024"}}"#, br#"{"image_size":42}"#, br#"{"num_images":"2"}"#, br#"{"num_images":-1}"#, br#"{"num_images":0}"#, &[0xff, 0xfe][..]] {
            assert_eq!(run(body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        // a bad request, no key, a locked account, a rate limit, a redirect not followed
        for refused in [400, 401, 403, 404, 422, 429, 302, 100, 0] {
            assert_eq!(run(br#"{"prompt":"p"}"#, refused), none, "{refused}");
        }
        // at most 16 megapixels a call, even sizes that overflow
        let most = Usage::Key { key: "fal:image".into(), units: 16 * 1_048_576 };
        assert_eq!(run(br#"{"image_size":{"width":100000,"height":100000}}"#, 200), most);
        assert_eq!(run(br#"{"image_size":{"width":18446744073709551615,"height":2},"num_images":2}"#, 200), none, "an overflow is no size");
        assert_eq!(run(br#"{"image_size":"square_hd","num_images":1000}"#, 200), most);
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.price(&most).unwrap().charge, 144_000, "$0.096 at list");
        // a release before the image price refuses its rows as unpriced
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "fal:image").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "fal:image".into(), units: 589_824 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "fal".into(), units: 0 }).unwrap().charge, 0, "a free call is free in it too");
    }

    /// A call on xAI's key, answered `status`, as the swap meters it.
    fn xai(method: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("xai", &KeyCall { method, host: "api.x.ai", path, body, status })
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): only an
    /// X search on xAI's key is charged as one, and an image of a known
    /// model one image a call; a chat (its tokens not read) and any other
    /// call nothing. Method: the call Hermes' `x_search` makes, and the chat
    /// and image calls an agent's code makes with `XAI_API_KEY`.
    #[test]
    fn an_xai_call_is_charged_as_an_x_search_only_when_it_is_one() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let hermes = br#"{"model":"grok-4.3","input":[{"role":"user","content":"what is new on X about fragments"}],"tools":[{"type":"x_search"}]}"#;
        let search = Usage::Key { key: "xai".into(), units: 1 };
        assert_eq!(xai("POST", "/v1/responses", hermes, 200), search, "Hermes' x_search");
        assert_eq!(xai("POST", "/v1/chat/completions", br#"{"model":"grok-4.3","messages":[],"tools":[{"type":"web_search"},{"type":"x_search","allowed_x_handles":["x"]}]}"#, 200), search);
        assert_eq!(book.price(&search).unwrap().charge, 180_000, "an X search, as before");
        let free = Usage::Key { key: "xai".into(), units: 0 };
        for (path, body) in [
            ("/v1/chat/completions", &br#"{"model":"grok-4.3","messages":[{"role":"user","content":"hi"}]}"#[..]),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi"}"#),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi","tools":[{"type":"web_search"}]}"#),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi","tools":"x_search"}"#),
            ("/v1/messages", br#"{"model":"grok-4.3","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}"#),
            ("/v1/chat/completions", b"a body the meter does not read"),
        ] {
            assert_eq!(xai("POST", path, body, 200), free, "a chat: {path} {}", String::from_utf8_lossy(body));
        }
        let image = Usage::Key { key: "xai:image".into(), units: 1 };
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat"}"#, 200), image);
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"model":"grok-imagine-image-2.0","prompt":"a cat","n":4}"#, 200), image, "one a call: its n is asked, not made");
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat","deferred":false}"#, 200), image, "synchronous, said so");
        // deferred: a request id alone, the image made later or not (FIN-445 review)
        for deferred in ["true", r#""true""#, "1", "null"] {
            let body = format!(r#"{{"model":"grok-imagine-image","prompt":"a cat","deferred":{deferred}}}"#);
            assert_eq!(xai("POST", "/v1/images/generations", body.as_bytes(), 200), Usage::Key { key: "xai".into(), units: 0 }, "{body}");
        }
        assert_eq!(xai("GET", "/v1/images/generations/req-1", b"", 200), Usage::Key { key: "xai".into(), units: 0 }, "its result's read");
        assert_eq!(book.price(&image).unwrap().charge, 30_000);
        for body in [&br#"{"model":"grok-imagine-image-mini","prompt":"a cat"}"#[..], br#"{"prompt":"a cat"}"#, br#"{"model":42,"prompt":"a cat"}"#] {
            assert_eq!(xai("POST", "/v1/images/generations", body, 200), free, "a model the book does not price: {}", String::from_utf8_lossy(body));
        }
        for (method, path) in [("GET", "/v1/models"), ("GET", "/v1/responses/resp_1"), ("DELETE", "/v1/responses/resp_1"), ("POST", "/v1/videos/generations"), ("POST", "/v1/tokenize-text"), ("GET", "/v1/api-key"), ("GET", "/v1/chat/completions")] {
            assert_eq!(xai(method, path, hermes, 200), free, "{method} {path}");
        }
        assert_eq!(book.price(&free).unwrap().charge, 0);
    }

    /// Goal: a call xAI refused made nothing, so is charged nothing, an X
    /// search too; an image call whose count the meter cannot read is
    /// charged nothing; a release whose book has no image price refuses
    /// those rows, and prices an X search as before. Method: every answer
    /// the swap meters, and bodies that say no count.
    #[test]
    fn an_xai_call_refused_or_unread_is_charged_nothing() {
        let hermes = br#"{"model":"grok-4.3","input":"news","tools":[{"type":"x_search"}]}"#;
        let image = br#"{"model":"grok-imagine-image","prompt":"a cat"}"#;
        // a bad request, no key, no credit, no model, a rate limit, a redirect not followed
        for refused in [400, 401, 403, 404, 422, 429, 302, 0] {
            assert_eq!(xai("POST", "/v1/responses", hermes, refused), Usage::Key { key: "xai".into(), units: 0 }, "{refused}");
            assert_eq!(xai("POST", "/v1/images/generations", image, refused), Usage::Key { key: "xai:image".into(), units: 0 }, "{refused}");
        }
        for n in [r#""2""#, "-1", "1.5", "0", "null"] {
            let body = format!(r#"{{"model":"grok-imagine-image","prompt":"a cat","n":{n}}}"#);
            assert_eq!(xai("POST", "/v1/images/generations", body.as_bytes(), 200), Usage::Key { key: "xai:image".into(), units: 0 }, "{body}");
        }
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let older = PriceBook { keys: book.keys.iter().filter(|k| !k.key.starts_with("xai:")).cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "xai:image".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "xai".into(), units: 1 }).unwrap().charge, 180_000, "an X search, as before");
    }

    /// A call on Google Places' key, answered `status`, as the swap meters it.
    fn places(method: &str, path: &str, status: u16) -> Usage {
        key_usage("google-places", &KeyCall { method, host: "places.googleapis.com", path, body: b"", status })
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): a Places
    /// photo is charged its own list, one price; a search or a place's
    /// details, whose tier is its field mask's (IDs only is free), an
    /// autocomplete (free in a session that ends in a Details call) and any
    /// other call nothing. Method: the goplaces
    /// skill's calls (Places API (New)), and the others the API takes.
    #[test]
    fn a_places_call_is_charged_as_what_it_is() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let charge = |u: &Usage| book.price(u).unwrap().charge;
        let one = |key: &str| Usage::Key { key: key.into(), units: 1 };
        assert_eq!(places("GET", "/v1/places/ChIJj61dQgK6j4AR4GeTYWZsKWw/photos/AUc7tXX/media", 200), one("google-places:photo"), "skipHttpRedirect: its JSON");
        assert_eq!(charge(&one("google-places:photo")), 10_500, "$0.0105, not a search's $0.0525");
        let free = Usage::Key { key: "google-places".into(), units: 0 };
        for (method, path) in [("POST", "/v1/places:autocomplete"), ("POST", "/v1/places:searchText"), ("POST", "/v1/places:searchNearby"), ("GET", "/v1/places/ChIJj61dQgK6j4AR4GeTYWZsKWw"), ("GET", "/v1/places/"), ("GET", "/v1/places/ChIJ/photos/AUc7"), ("POST", "/v1/places:searchtext"), ("GET", "/v2/places/ChIJ/photos/AUc7/media"), ("GET", "/maps/api/place/textsearch/json"), ("GET", "/")] {
            assert_eq!(places(method, path, 200), free, "{method} {path}");
        }
        assert_eq!(charge(&free), 0);
    }

    /// Goal: a Places call Google refused or redirected (a photo's default
    /// answer, its image elsewhere) is charged nothing; a release whose
    /// book has no photo price refuses those rows.
    #[test]
    fn a_places_call_google_refused_or_redirected_is_charged_nothing() {
        for status in [400, 401, 403, 404, 429, 301, 302, 303, 307, 308, 304, 0] {
            assert_eq!(places("GET", "/v1/places/ChIJ/photos/AUc7/media", status), Usage::Key { key: "google-places:photo".into(), units: 0 }, "{status}");
        }
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let older = PriceBook { keys: book.keys.iter().filter(|k| !k.key.starts_with("google-places:")).cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "google-places:photo".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "google-places".into(), units: 1 }).unwrap().charge, 52_500, "an older release's row, as before");
    }

    /// A JSON `POST` on Firecrawl's key, answered `status`, in credits.
    fn firecrawl(method: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("firecrawl", &KeyCall { method, host: "api.firecrawl.dev", path, body, status })
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): a
    /// Firecrawl scrape and map are charged the credits they cost; a
    /// search (billed by the results it returns, which are not counted), a
    /// crawl, a batch, their status polls and any other call nothing.
    /// Method: Hermes' `web_extract` and `web_search` bodies (its Firecrawl
    /// provider), and an agent's SDK calls.
    #[test]
    fn a_firecrawl_call_is_charged_its_credits() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let credits = |units| Usage::Key { key: "firecrawl:credit".into(), units };
        let extract = br#"{"url":"https://example.com/","formats":["markdown","html"]}"#;
        assert_eq!(firecrawl("POST", "/v2/scrape", extract, 200), credits(1), "Hermes' web_extract");
        assert_eq!(book.price(&credits(1)).unwrap().charge, 7_500, "$0.0075, not a search's $0.015");
        assert_eq!(firecrawl("POST", "/v1/scrape", extract, 200), credits(1));
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u"}"#, 200), credits(1), "markdown alone");
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u","formats":["markdown",{"type":"json","prompt":"p"}]}"#, 200), credits(5));
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u","formats":["json","highlights",{"type":"screenshot"}]}"#, 200), credits(9), "each LLM kind 4 more");
        // each kind of LLM format once a page, a question and a query one kind
        let scraped = |formats: &str| firecrawl("POST", "/v2/scrape", format!(r#"{{"url":"u","formats":{formats}}}"#).as_bytes(), 200);
        assert_eq!(scraped(r#"[{"type":"json","prompt":"name"},{"type":"json","prompt":"name"}]"#), credits(5));
        assert_eq!(scraped(r#"[{"type":"question","question":"q"},{"type":"query","prompt":"q"}]"#), credits(5));
        assert_eq!(scraped(r#"["json","extract"]"#), credits(5), "v1's extract is JSON");
        assert_eq!(firecrawl("POST", "/v2/map", br#"{"url":"u"}"#, 200), credits(1));
        let free = Usage::Key { key: "firecrawl".into(), units: 0 };
        for (method, path) in [("POST", "/v2/search"), ("POST", "/v1/search"), ("GET", "/v2/crawl/0b1e"), ("GET", "/v2/batch/scrape/0b1e"), ("GET", "/v2/extract/0b1e"), ("POST", "/v2/crawl"), ("POST", "/v2/batch/scrape"), ("DELETE", "/v2/batch/scrape/0b1e"), ("POST", "/v2/extract"), ("POST", "/v2/agent"), ("GET", "/v2/team/credit-usage"), ("GET", "/v2/scrape"), ("POST", "/v3/scrape")] {
            assert_eq!(firecrawl(method, path, br#"{"url":"u","query":"q","limit":5,"urls":["a","b"]}"#, 200), free, "{method} {path}");
        }
    }

    /// Goal: a Firecrawl call it refused (or that returned no document), or
    /// whose body the meter cannot read, is charged nothing; a call at
    /// most 1,000 credits; an older book refuses the rows.
    #[test]
    fn a_firecrawl_call_refused_or_unread_is_charged_nothing() {
        let none = Usage::Key { key: "firecrawl:credit".into(), units: 0 };
        for refused in [400, 401, 402, 403, 404, 408, 429, 302, 0] {
            assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u"}"#, refused), none, "{refused}");
            assert_eq!(firecrawl("POST", "/v2/map", br#"{"url":"u"}"#, refused), none, "{refused}");
        }
        for body in [&b""[..], b"url=u", b"[1]", b"a body the meter does not read"] {
            assert_eq!(firecrawl("POST", "/v2/scrape", body, 200), none, "{}", String::from_utf8_lossy(body));
            assert_eq!(firecrawl("POST", "/v2/map", body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        let past = format!(r#"{{"url":"u","pad":"{}"}}"#, "a".repeat(KEY_PART_BODY_MAX_BYTES));
        assert_eq!(firecrawl("POST", "/v2/scrape", past.as_bytes(), 200), none, "a body past the read");
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.price(&Usage::Key { key: "firecrawl:credit".into(), units: 1_000 }).unwrap().charge, 7_500_000, "the most a call: $5 at list");
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "firecrawl:credit").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "firecrawl:credit".into(), units: 1 }), Err(PriceError::NoPrice));
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): a
    /// Browser Use session is charged once, at its start, as five minutes;
    /// a task on a session it has (`sessionId`), its status and stop
    /// calls, an unread start and one Browser Use refused nothing. Method:
    /// Hermes' browser provider's calls (API v3) and an agent's SDK calls
    /// (v2, v4).
    #[test]
    fn a_browser_use_session_is_charged_at_its_start_only() {
        let used = |method: &str, path: &str, body: &[u8], status| key_usage("browser-use", &KeyCall { method, host: "api.browser-use.com", path, body, status });
        let start = Usage::Key { key: "browser-use".into(), units: 1 };
        let free = Usage::Key { key: "browser-use".into(), units: 0 };
        for path in ["/api/v3/browsers", "/api/v2/browsers", "/api/v4/browsers", "/api/v2/sessions", "/api/v2/tasks", "/api/v4/runs"] {
            assert_eq!(used("POST", path, br#"{"timeout":15}"#, 200), start, "{path}");
            assert_eq!(used("POST", path, b"{}", 201), start, "{path} created");
            assert_eq!(used("POST", path, br#"{"task":"t","sessionId":null}"#, 201), start, "{path}: a null session is a new one");
            // a task on a session it has, as v2's, v3's and v4's take it (FIN-445 review)
            assert_eq!(used("POST", path, br#"{"task":"t","sessionId":"s-1"}"#, 201), free, "{path} on a session it has");
            assert_eq!(used("POST", path, br#"{"task":"t","session_id":"s-1"}"#, 201), free, "{path} on a session it has");
            // unread: no body, not an object, past the read
            for body in [&b""[..], b"task=t", b"[1]"] {
                assert_eq!(used("POST", path, body, 201), free, "{path} {}", String::from_utf8_lossy(body));
            }
        }
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.price(&start).unwrap().charge, 2_500, "five minutes");
        // a session, then two tasks on it: one start
        let session: i64 = [used("POST", "/api/v2/sessions", b"{}", 201), used("POST", "/api/v2/tasks", br#"{"task":"a","sessionId":"s-1"}"#, 201), used("POST", "/api/v2/tasks", br#"{"task":"b","sessionId":"s-1"}"#, 201)].iter().map(|u| book.price(u).unwrap().charge).sum();
        assert_eq!(session, 2_500);
        for (method, path) in [("PATCH", "/api/v3/browsers/b-1"), ("DELETE", "/api/v3/browsers/b-1"), ("GET", "/api/v3/browsers/b-1"), ("GET", "/api/v3/browsers"), ("GET", "/api/v2/tasks/t-1"), ("GET", "/api/v2/billing/account"), ("POST", "/api/v3/browsers/b-1"), ("POST", "/api/vx/browsers"), ("POST", "/api/v/browsers"), ("POST", "/browsers")] {
            assert_eq!(used(method, path, b"{}", 200), free, "{method} {path}");
        }
        for refused in [400, 401, 402, 404, 422, 429, 302, 0] {
            assert_eq!(used("POST", "/api/v3/browsers", b"{}", refused), free, "{refused}");
        }
    }

    /// A call on X's key, answered `status`.
    fn x(method: &str, path: &str, status: u16) -> Usage {
        key_usage("x", &KeyCall { method, host: "api.x.com", path, body: b"", status })
    }

    /// Goal (FIN-445, and its review: when in doubt, charge zero): an X
    /// search is charged ten posts as before; a lookup, billed by the
    /// resources it returns (which the meter does not count: a partial
    /// answer is a 200 too), and any other call nothing. Method: the
    /// x-api-finite helper's calls, and the API's others.
    #[test]
    fn an_x_search_is_charged_and_a_lookup_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let search = Usage::Key { key: "x".into(), units: 1 };
        assert_eq!(x("GET", "/2/tweets/search/recent", 200), search, "the helper's search and conversation");
        assert_eq!(x("GET", "/2/tweets/search/all", 200), search);
        assert_eq!(book.price(&search).unwrap().charge, 75_000, "ten posts, as before");
        let free = Usage::Key { key: "x".into(), units: 0 };
        for (method, path) in [("GET", "/2/tweets"), ("GET", "/2/tweets/1460323737035677698"), ("GET", "/2/users/by/username/jack"), ("GET", "/2/users/12"), ("GET", "/2/users/me"), ("GET", "/2/users"), ("GET", "/2/users/by"), ("GET", "/2/users/12/tweets"), ("GET", "/2/users/search"), ("GET", "/2/tweets/counts/recent"), ("POST", "/2/tweets"), ("POST", "/2/tweets/search/recent"), ("GET", "/2/tweets/search/stream"), ("GET", "/e2e")] {
            assert_eq!(x(method, path, 200), free, "{method} {path}");
        }
        assert_eq!(book.price(&free).unwrap().charge, 0);
        // a search X refused or redirected: nothing
        for refused in [400, 401, 403, 429, 302, 0] {
            assert_eq!(x("GET", "/2/tweets/search/recent", refused), free, "{refused}");
        }
    }
}
