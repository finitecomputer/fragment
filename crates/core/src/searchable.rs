//! A fragment's search, as the cell makes it of the search service
//! (search/README.md; docs/api.md, A fragment's search). Pure: the cell checks who asks,
//! embeds and calls.
//!
//! A fragment's index is the service's table of its scope (`scope`): its
//! identity and incarnation, and its read scope (`members`: whatever its
//! readers may read). An app writes sources (fragment_proto::search), each
//! a set of sections; the cell cuts each section into chunks its embedding
//! model reads whole (`chunks`, at most the slot's window: nothing is cut
//! away), and names each chunk's embedding input by its sha256 (`Chunk`),
//! so a vector the table already holds is reused, not bought again.

use crate::embed;
use fragment_proto::search::{self as wire, Upsert};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The read scope every index has today: what a fragment's readers read.
pub const READ_SCOPE: &str = "members";
pub const FIELDS_MAX: usize = 4;
pub const FIELD_KEY_MAX_BYTES: usize = 32;
pub const FIELD_VALUE_MAX_BYTES: usize = 64;
pub const TITLE_MAX_BYTES: usize = 512;
pub const HEADING_MAX_BYTES: usize = 512;
pub const CONTEXT_MAX: usize = 8;
pub const CONTEXT_ITEM_MAX_BYTES: usize = 256;
pub const AT_MAX_BYTES: usize = 40;
/// A chunk's embedding input's header (title, context, heading), at most
/// this share of the window: the chunk's text keeps the rest.
const HEADER_MAX_SHARE: u64 = 4;

/// The service's table for a fragment: `f<id>-<incarnation>-members`.
pub fn scope(fragment_id: &str, incarnation: i64) -> String {
    assert!(fragment_id.bytes().all(|b| b.is_ascii_hexdigit()) && !fragment_id.is_empty(), "a Durable Object's id is hex");
    assert!(incarnation >= 0, "an incarnation is a creation time");
    format!("f{}-{incarnation}-{READ_SCOPE}", fragment_id.to_ascii_lowercase())
}

fn bounded(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.len() > max {
        return Err(format!("{what} is at most {max} bytes"));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(format!("{what} holds a control character"));
    }
    Ok(())
}

fn named(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("{what} is named"));
    }
    if s.chars().any(char::is_control) {
        return Err(format!("{what} holds a control character"));
    }
    bounded(what, s, max)
}

/// An upsert, checked: every limit (fragment_proto::search), before
/// anything is embedded or sent.
pub fn check(up: &Upsert) -> Result<(), String> {
    named("source", &up.source, wire::SOURCE_MAX_BYTES)?;
    named("revision", &up.revision, wire::REVISION_MAX_BYTES)?;
    if let Some(r) = up.replaces.as_deref().filter(|r| !r.is_empty()) {
        named("replaces", r, wire::REVISION_MAX_BYTES)?;
    }
    if let Some(t) = &up.title {
        bounded("title", t, TITLE_MAX_BYTES)?;
    }
    if let Some(a) = &up.at {
        named("at", a, AT_MAX_BYTES)?;
    }
    check_fields(&up.fields)?;
    if up.sections.len() > wire::SECTIONS_MAX {
        return Err(format!("a source is at most {} sections, not {}", wire::SECTIONS_MAX, up.sections.len()));
    }
    let mut ids = std::collections::HashSet::new();
    for s in &up.sections {
        named("a section's id", &s.id, wire::SECTION_ID_MAX_BYTES)?;
        if !ids.insert(s.id.as_str()) {
            return Err(format!("section {:?} is named twice", s.id));
        }
        if s.text.trim().is_empty() {
            return Err(format!("section {:?} has no text", s.id));
        }
        let chars = s.text.chars().count();
        if chars > wire::TEXT_MAX_CHARS {
            return Err(format!("section {:?} is {chars} characters, past {}", s.id, wire::TEXT_MAX_CHARS));
        }
        bounded("a section's text", &s.text, usize::MAX)?;
        if let Some(h) = &s.heading {
            bounded("a heading", h, HEADING_MAX_BYTES)?;
        }
        if s.context.len() > CONTEXT_MAX {
            return Err(format!("a section's context is at most {CONTEXT_MAX}"));
        }
        for c in &s.context {
            bounded("a context", c, CONTEXT_ITEM_MAX_BYTES)?;
        }
        if let Some(a) = &s.at {
            named("a section's at", a, AT_MAX_BYTES)?;
        }
        if let Some(l) = &s.location {
            if l.from > l.to {
                return Err(format!("section {:?}'s location runs from {} back to {}", s.id, l.from, l.to));
            }
        }
    }
    Ok(())
}

pub fn check_fields(fields: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
    if fields.len() > FIELDS_MAX {
        return Err(format!("fields are at most {FIELDS_MAX}"));
    }
    for (k, v) in fields {
        if k.is_empty() || k.len() > FIELD_KEY_MAX_BYTES || !k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(format!("a field's name is 1 to {FIELD_KEY_MAX_BYTES} of a-z, 0-9 and _, not {k:?}"));
        }
        named(&format!("fields.{k}"), v, FIELD_VALUE_MAX_BYTES)?;
        if v.contains('=') || v.contains('\u{1f}') {
            return Err(format!("fields.{k} holds ="));
        }
    }
    Ok(())
}

/// A query, checked.
pub fn check_query(q: &wire::Query) -> Result<(), String> {
    bounded("q", &q.q, wire::QUERY_MAX_BYTES)?;
    if q.q.trim().is_empty() {
        return Err("a search names some words".into());
    }
    if let Some(l) = q.limit {
        if !(1..=wire::RESULTS_MAX).contains(&l) {
            return Err(format!("limit is 1 to {}", wire::RESULTS_MAX));
        }
    }
    check_fields(&q.fields)?;
    for (what, t) in [("after", &q.after), ("before", &q.before)] {
        if let Some(t) = t {
            named(what, t, AT_MAX_BYTES)?;
        }
    }
    Ok(())
}

/// The largest index at or below `i` that is a character boundary.
fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The most tokens `s` is read as, its model's own not counted
/// (`embed::char_tokens_max`).
fn tokens(s: &str) -> u64 {
    s.chars().map(|c| u64::from(embed::char_tokens_max(c))).sum()
}

/// The end of `s`'s longest start of at most `max` tokens (`tokens`).
fn fit(s: &str, max: u64) -> usize {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        used += u64::from(embed::char_tokens_max(c));
        if used > max {
            return i;
        }
    }
    s.len()
}

/// `text` cut into pieces of at most `max` tokens (`tokens`: a bound for
/// any tokenizer, as the embeddings route's own), none of it left out: at
/// paragraphs, else sentences, else words, else characters.
pub fn chunks(text: &str, max: u64) -> Vec<String> {
    assert!(max >= 2 * u64::from(embed::CHAR_TOKENS_MAX), "a window holds any character, and some words");
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let end = fit(rest, max);
        if end == rest.len() {
            out.push(rest.to_string());
            break;
        }
        let window = &rest[..end];
        let cut = [&["\n\n"][..], &[". ", "! ", "? ", ".\n", "\n"][..], &[" ", "\t"][..]]
            .iter()
            .find_map(|seps| seps.iter().filter_map(|sep| window.rfind(sep).map(|i| i + sep.len())).filter(|&i| i >= window.len() / 4).max())
            .unwrap_or(window.len());
        let cut = floor(rest, cut).max(rest.chars().next().map_or(1, char::len_utf8));
        out.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    out.retain(|c| !c.is_empty());
    out
}

/// A chunk, as it is embedded and stored: its text, the input its model
/// reads (the section's title, context and heading, then the text), and
/// that input's sha256, by which its vector is reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub section: usize,
    pub text: String,
    pub input: String,
    pub hash: String,
}

/// The section's header, at most `max` tokens.
fn header(title: &str, context: &[String], heading: Option<&str>, max: u64) -> String {
    let mut trail: Vec<&str> = context.iter().map(String::as_str).collect();
    trail.extend(heading);
    let h = if trail.is_empty() { title.to_string() } else { format!("{title}\n{}", trail.join(" > ")) };
    let h = h.trim();
    h[..fit(h, max)].trim_end().to_string()
}

/// `up`'s sections in chunks, each input at most `window` tokens as the
/// embeddings route bounds them (`embed::tokens_max`), its model's own
/// tokens included: a slot's `max_input_tokens`.
pub fn chunked(up: &Upsert, window: u32) -> Vec<Chunk> {
    assert!(window >= embed::MAX_INPUT_TOKENS_MIN, "a slot's window is at least MAX_INPUT_TOKENS_MIN");
    let room = u64::from(window - embed::OVERHEAD_TOKENS);
    let title = up.title.as_deref().unwrap_or("");
    let mut out = Vec::new();
    for (i, s) in up.sections.iter().enumerate() {
        let head = header(title, &s.context, s.heading.as_deref(), room / HEADER_MAX_SHARE);
        let budget = room - if head.is_empty() { 0 } else { tokens(&head) + 2 };
        for text in chunks(&s.text, budget) {
            let input = if head.is_empty() { text.clone() } else { format!("{head}\n\n{text}") };
            assert!(embed::tokens_max(&input) <= u64::from(window), "a chunk's input fits its window");
            let hash = hex::encode(Sha256::digest(input.as_bytes()));
            out.push(Chunk { section: i, text, input, hash });
        }
    }
    out
}

/// The service's upsert (search/README.md): `up`'s sections and their
/// chunks, each with its vector when it has one (base64 float32s), in
/// `space` (`None`: words alone).
pub fn service_upsert(up: &Upsert, chunks: &[Chunk], vectors: &std::collections::HashMap<String, String>, space: Option<&Value>) -> Value {
    let mut sections: Vec<Value> = Vec::with_capacity(up.sections.len());
    for (i, s) in up.sections.iter().enumerate() {
        let cs: Vec<Value> = chunks
            .iter()
            .filter(|c| c.section == i)
            .map(|c| match vectors.get(&c.hash) {
                Some(v) => json!({ "text": c.text, "hash": c.hash, "vector": v }),
                None => json!({ "text": c.text, "hash": c.hash }),
            })
            .collect();
        let mut o = Map::new();
        o.insert("id".into(), json!(s.id));
        if let Some(h) = &s.heading {
            o.insert("heading".into(), json!(h));
        }
        o.insert("context".into(), json!(s.context));
        if let Some(a) = &s.at {
            o.insert("at".into(), json!(a));
        }
        if let Some(l) = &s.location {
            o.insert("location".into(), json!(l));
        }
        o.insert("chunks".into(), Value::Array(cs));
        sections.push(Value::Object(o));
    }
    let mut o = Map::new();
    o.insert("source".into(), json!(up.source));
    o.insert("revision".into(), json!(up.revision));
    if let Some(r) = &up.replaces {
        o.insert("replaces".into(), json!(r));
    }
    if let Some(t) = &up.title {
        o.insert("title".into(), json!(t));
    }
    if let Some(a) = &up.at {
        o.insert("at".into(), json!(a));
    }
    o.insert("fields".into(), json!(up.fields));
    o.insert("space".into(), space.cloned().unwrap_or(Value::Null));
    o.insert("sections".into(), Value::Array(sections));
    Value::Object(o)
}

/// A vector as the service takes it: little-endian float32s, base64.
pub fn vector_b64(v: &[f32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::search::{Location, Section};

    fn up(sections: Vec<Section>) -> Upsert {
        Upsert { source: "sessions/a.json".into(), revision: "r1".into(), replaces: None, title: Some("Trip".into()), at: None, fields: Default::default(), sections }
    }

    fn section(id: &str, text: &str) -> Section {
        Section { id: id.into(), text: text.into(), heading: None, context: vec![], at: None, location: None }
    }

    #[test]
    fn scopes_are_one_life_of_one_fragment() {
        assert_eq!(scope("ABCdef01", 1730000000000), "fabcdef01-1730000000000-members");
        assert_ne!(scope("ab", 1), scope("ab", 2), "a fragment made again is another index");
    }

    #[test]
    fn upserts_are_checked_whole() {
        assert_eq!(check(&up(vec![section("t0", "hello")])), Ok(()));
        let refused = |u: Upsert| check(&u).unwrap_err();
        assert!(refused(up(vec![section("t0", "x"), section("t0", "y")])).contains("twice"));
        assert!(refused(up(vec![section("t0", " ")])).contains("no text"));
        assert!(refused(up(vec![section("t0", &"é".repeat(8001))])).contains("8001 characters"));
        assert!(check(&up(vec![section("t0", &"é".repeat(8000))])).is_ok(), "characters, not bytes");
        let mut backwards = section("t0", "x");
        backwards.location = Some(Location { from: 5, to: 2 });
        assert!(refused(up(vec![backwards])).contains("back to"));
        let mut u = up(vec![section("t0", "x")]);
        u.fields.insert("Kind".into(), "chat".into());
        assert!(refused(u).contains("a-z"));
        let mut u = up(vec![]);
        u.source = String::new();
        assert!(refused(u).contains("source is named"));
    }

    #[test]
    fn chunks_fit_the_window_and_keep_every_word() {
        let text = format!("{}\n\n{}. {}", "alpha ".repeat(80).trim(), "beta ".repeat(300).trim(), "gamma ".repeat(50).trim());
        let pieces = chunks(&text, 512);
        assert!(pieces.iter().all(|p| tokens(p) <= 512 && !p.is_empty()));
        let words = |s: &str| s.split_whitespace().map(|w| w.trim_end_matches('.')).collect::<Vec<_>>().join(" ");
        assert_eq!(words(&pieces.join(" ")), words(&text), "nothing is left out");
        assert_eq!(pieces[0], "alpha ".repeat(80).trim(), "a paragraph first");
        let word = "x".repeat(2000);
        let hard = chunks(&word, 512);
        assert_eq!(hard.concat(), word, "a word longer than a window is cut at characters");
        let wide = "é".repeat(400);
        assert!(chunks(&wide, 99).iter().all(|c| tokens(c) <= 99 && c.chars().all(|ch| ch == 'é')), "é is read as up to 3 tokens");
    }

    #[test]
    fn a_chunk_is_named_by_what_its_model_reads() {
        let mut s = section("t0", &"word ".repeat(400));
        s.heading = Some("Monday".into());
        s.context = vec!["telegram".into()];
        let cs = chunked(&up(vec![s, section("t1", "short")]), 512);
        assert!(cs.len() >= 2 && cs.iter().all(|c| embed::tokens_max(&c.input) <= 512));
        assert!(cs[0].input.starts_with("Trip\ntelegram > Monday\n\nword"));
        assert_eq!(cs.last().unwrap().section, 1);
        let again = chunked(&up(vec![section("t1", "short")]), 512);
        assert_eq!(again[0].hash, cs.last().unwrap().hash, "the same input is the same hash, wherever it was");
    }

    /// The route refused BGE-M3's 773 tokens of 1,400 bytes of numbers, and
    /// chunks of 1,536 bytes were made for it: each chunk is now one the
    /// route takes, whatever its script.
    #[test]
    fn every_chunk_is_one_the_embeddings_route_takes() {
        let slot = embed::Slot::default();
        let mut numbers = section("t0", &"1,2,3,4,5,6,7,8,9,0\n".repeat(400));
        numbers.heading = Some("Таблица".into());
        let texts = [numbers, section("t1", &"ﷺ ".repeat(500)), section("t2", &"검색 색인은 쓰기마다 갱신됩니다. ".repeat(150)), section("t3", &"word ".repeat(1500))];
        let cs = chunked(&up(texts.to_vec()), slot.max_input_tokens);
        for c in &cs {
            assert_eq!(embed::bound(&json!({ "input": c.input }), &slot).map(|_| ()), Ok(()), "{:?}", &c.input[..40.min(c.input.len())]);
        }
        for (i, t) in texts.iter().enumerate() {
            let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
            let joined: Vec<&str> = cs.iter().filter(|c| c.section == i).map(|c| c.text.as_str()).collect();
            assert_eq!(words(&joined.join(" ")), words(&t.text), "nothing of section {i} is left out");
        }
    }

    #[test]
    fn the_service_is_sent_each_chunk_once() {
        let u = up(vec![section("t0", "hello there")]);
        let cs = chunked(&u, 512);
        let vectors = [(cs[0].hash.clone(), vector_b64(&[1.0, 0.0]))].into();
        let body = service_upsert(&u, &cs, &vectors, Some(&json!({ "model": "m", "dimensions": 2 })));
        assert_eq!(body["sections"][0]["chunks"][0]["text"], "hello there");
        assert!(body["sections"][0]["chunks"][0]["vector"].is_string());
        let words = service_upsert(&u, &cs, &Default::default(), None);
        assert_eq!(words["space"], Value::Null);
        assert!(words["sections"][0]["chunks"][0].get("vector").is_none());
    }
}
