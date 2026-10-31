//! What the service is asked and answers (README.md, The API), and every
//! limit, checked before anything is read or written.
//!
//! The service holds one table a scope: a name the platform makes from a
//! fragment's identity, incarnation and read scope (never an author's), to
//! the service only a name. A table's rows are its sources' sections'
//! chunks: the platform cuts a section into chunks its embeddings model
//! reads whole, so a section's vectors are its chunks'. Each source is
//! replaced whole, its revision once.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCOPE_MAX_BYTES: usize = 128;
pub const SOURCE_MAX_BYTES: usize = 512;
pub const REVISION_MAX_BYTES: usize = 128;
pub const SECTION_ID_MAX_BYTES: usize = 64;
pub const SECTIONS_MAX: usize = 2_000;
/// Chunks a section is cut into, at most (an 8,000-character section is
/// about six of BGE-M3's 512-token windows).
pub const CHUNKS_MAX: usize = 32;
pub const CHUNK_MAX_BYTES: usize = 32 * 1024;
/// Chunks one upsert writes, at most.
pub const UPSERT_CHUNKS_MAX: usize = 8_000;
pub const TITLE_MAX_BYTES: usize = 512;
pub const HEADING_MAX_BYTES: usize = 512;
pub const CONTEXT_MAX: usize = 8;
pub const CONTEXT_ITEM_MAX_BYTES: usize = 256;
pub const AT_MAX_BYTES: usize = 40;
pub const LOCATION_MAX_BYTES: usize = 256;
pub const FIELDS_MAX: usize = 4;
pub const FIELD_KEY_MAX_BYTES: usize = 32;
pub const FIELD_VALUE_MAX_BYTES: usize = 64;
pub const QUERY_MAX_BYTES: usize = 1024;
pub const RESULTS_MAX: usize = 50;
pub const RESULTS_DEFAULT: usize = 10;
pub const KNOWN_MAX: usize = UPSERT_CHUNKS_MAX;
pub const SOURCES_PAGE_MAX: usize = 1000;
pub const DIMENSIONS_MAX: u32 = 4096;
/// A request's body, at most: an upsert's chunks with a 1,024-dimension
/// vector each, base64, and their text.
pub const BODY_MAX_BYTES: usize = 64 * 1024 * 1024;
/// A table's chunks, at most: an archive of 18,000 conversations, cut.
pub const TABLE_ROWS_MAX: usize = 2_000_000;

/// Why a request is refused (400, unless said).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

fn bounded(what: &str, s: &str, max: usize) -> Result<(), Invalid> {
    if s.len() > max {
        return Err(Invalid(format!("{what} is at most {max} bytes, not {}", s.len())));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(Invalid(format!("{what} holds a control character")));
    }
    Ok(())
}

fn named(what: &str, s: &str, max: usize) -> Result<(), Invalid> {
    if s.is_empty() {
        return Err(Invalid(format!("{what} is named")));
    }
    if s.chars().any(char::is_control) {
        return Err(Invalid(format!("{what} holds a control character")));
    }
    bounded(what, s, max)
}

/// A scope: 1 to `SCOPE_MAX_BYTES` of a-z, 0-9, `-` and `_`, a letter or
/// digit first (a table's name on any store).
pub fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= SCOPE_MAX_BYTES
        && scope.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && scope.as_bytes()[0].is_ascii_alphanumeric()
}

/// A text's sha256, as the platform names a chunk's: 64 lowercase hex.
pub fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The vectors' space: what made them. A table keeps one; `None` is a
/// table of words alone (a deployment that embeds nothing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Space {
    pub model: String,
    pub dimensions: u32,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub text: String,
    /// The text's sha256: a vector made of it is reused by it.
    pub hash: String,
    /// Its vector, little-endian float32s, base64; none: the table's own of
    /// this hash (`known`), refused when it has none.
    #[serde(default)]
    pub vector: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub id: String,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub location: Option<Value>,
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upsert {
    pub source: String,
    pub revision: String,
    /// The revision this one replaces: written only if it is the source's
    /// now (`""`: only if the source has none); unnamed, whatever it is.
    #[serde(default)]
    pub replaces: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    pub space: Option<Space>,
    pub sections: Vec<Section>,
}

fn valid_fields(fields: &BTreeMap<String, String>) -> Result<(), Invalid> {
    if fields.len() > FIELDS_MAX {
        return Err(Invalid(format!("fields are at most {FIELDS_MAX}")));
    }
    for (k, v) in fields {
        if k.is_empty() || k.len() > FIELD_KEY_MAX_BYTES || !k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(Invalid(format!("a field's name is 1 to {FIELD_KEY_MAX_BYTES} of a-z, 0-9 and _, not {k:?}")));
        }
        named(&format!("fields.{k}"), v, FIELD_VALUE_MAX_BYTES)?;
        if v.contains('\u{1f}') || v.contains('=') {
            return Err(Invalid(format!("fields.{k} holds = or a unit separator")));
        }
    }
    Ok(())
}

impl Space {
    pub fn check(&self) -> Result<(), Invalid> {
        named("space.model", &self.model, 128)?;
        if !(1..=DIMENSIONS_MAX).contains(&self.dimensions) {
            return Err(Invalid(format!("space.dimensions is 1 to {DIMENSIONS_MAX}")));
        }
        if let Some(v) = &self.version {
            named("space.version", v, 32)?;
        }
        Ok(())
    }
}

impl Upsert {
    /// Everything but the vectors' bytes, which the store decodes.
    pub fn check(&self) -> Result<(), Invalid> {
        named("source", &self.source, SOURCE_MAX_BYTES)?;
        named("revision", &self.revision, REVISION_MAX_BYTES)?;
        if let Some(r) = &self.replaces {
            if !r.is_empty() {
                named("replaces", r, REVISION_MAX_BYTES)?;
            }
        }
        if let Some(t) = &self.title {
            bounded("title", t, TITLE_MAX_BYTES)?;
        }
        if let Some(at) = &self.at {
            named("at", at, AT_MAX_BYTES)?;
        }
        valid_fields(&self.fields)?;
        if let Some(space) = &self.space {
            space.check()?;
        }
        if self.sections.len() > SECTIONS_MAX {
            return Err(Invalid(format!("a source is at most {SECTIONS_MAX} sections, not {}", self.sections.len())));
        }
        let mut ids = std::collections::HashSet::new();
        let mut chunks = 0;
        for s in &self.sections {
            named("a section's id", &s.id, SECTION_ID_MAX_BYTES)?;
            if !ids.insert(s.id.as_str()) {
                return Err(Invalid(format!("section {:?} is named twice", s.id)));
            }
            if let Some(h) = &s.heading {
                bounded("a heading", h, HEADING_MAX_BYTES)?;
            }
            if s.context.len() > CONTEXT_MAX {
                return Err(Invalid(format!("a section's context is at most {CONTEXT_MAX}")));
            }
            for c in &s.context {
                bounded("a context", c, CONTEXT_ITEM_MAX_BYTES)?;
            }
            if let Some(at) = &s.at {
                named("a section's at", at, AT_MAX_BYTES)?;
            }
            if let Some(l) = &s.location {
                if !l.is_object() || l.to_string().len() > LOCATION_MAX_BYTES {
                    return Err(Invalid(format!("a location is a JSON object of at most {LOCATION_MAX_BYTES} bytes")));
                }
            }
            if s.chunks.is_empty() || s.chunks.len() > CHUNKS_MAX {
                return Err(Invalid(format!("a section is 1 to {CHUNKS_MAX} chunks")));
            }
            for c in &s.chunks {
                bounded("a chunk's text", &c.text, CHUNK_MAX_BYTES)?;
                if !valid_hash(&c.hash) {
                    return Err(Invalid("a chunk's hash is its text's sha256, 64 lowercase hex".into()));
                }
                if c.vector.is_some() && self.space.is_none() {
                    return Err(Invalid("a vector is in a space: name it".into()));
                }
            }
            chunks += s.chunks.len();
        }
        if chunks > UPSERT_CHUNKS_MAX {
            return Err(Invalid(format!("an upsert is at most {UPSERT_CHUNKS_MAX} chunks, not {chunks}")));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub source: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Known {
    pub hashes: Vec<String>,
}

impl Known {
    pub fn check(&self) -> Result<(), Invalid> {
        if self.hashes.len() > KNOWN_MAX {
            return Err(Invalid(format!("at most {KNOWN_MAX} hashes")));
        }
        if !self.hashes.iter().all(|h| valid_hash(h)) {
            return Err(Invalid("a hash is a text's sha256, 64 lowercase hex".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Hybrid,
    Lexical,
    Semantic,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    #[serde(default)]
    pub q: String,
    /// The question's vector, in the table's space, base64.
    #[serde(default)]
    pub vector: Option<String>,
    #[serde(default)]
    pub space: Option<Space>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: Mode,
    /// Every one of these, as the sources named them.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// Sections at or after, and before, these times (as their `at`s
    /// sort: text).
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
}

impl Search {
    pub fn check(&self) -> Result<(), Invalid> {
        bounded("q", &self.q, QUERY_MAX_BYTES)?;
        if let Some(l) = self.limit {
            if !(1..=RESULTS_MAX).contains(&l) {
                return Err(Invalid(format!("limit is 1 to {RESULTS_MAX}")));
            }
        }
        valid_fields(&self.fields)?;
        for (what, t) in [("after", &self.after), ("before", &self.before)] {
            if let Some(t) = t {
                named(what, t, AT_MAX_BYTES)?;
            }
        }
        if self.vector.is_some() != self.space.is_some() {
            return Err(Invalid("a vector is named with its space".into()));
        }
        if let Some(space) = &self.space {
            space.check()?;
        }
        match self.mode {
            Mode::Lexical if self.q.trim().is_empty() => Err(Invalid("a search by words names some".into())),
            Mode::Semantic if self.vector.is_none() => Err(Invalid("a search by meaning names a vector".into())),
            Mode::Hybrid if self.q.trim().is_empty() && self.vector.is_none() => Err(Invalid("a search names words, a vector, or both".into())),
            _ => Ok(()),
        }
    }
}

/// One answer: a section, the chunk that matched best, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub rank: usize,
    pub source: String,
    pub section: String,
    pub revision: String,
    pub title: String,
    pub heading: Option<String>,
    pub context: Vec<String>,
    pub at: Option<String>,
    pub location: Option<Value>,
    pub snippet: String,
    pub score: f64,
    /// `lexical`, `semantic`, or both.
    pub signals: Vec<&'static str>,
}

/// Little-endian float32s from base64, exactly `dimensions` of them.
pub fn vector_of(b64: &str, dimensions: u32) -> Result<Vec<f32>, Invalid> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| Invalid("a vector is base64".into()))?;
    if bytes.len() != dimensions as usize * 4 {
        return Err(Invalid(format!("a vector in this space is {dimensions} float32s, not {} bytes", bytes.len())));
    }
    let v: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    if v.iter().any(|x| !x.is_finite()) {
        return Err(Invalid("a vector's values are finite".into()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upsert(v: Value) -> Upsert {
        serde_json::from_value(v).unwrap()
    }

    fn hash() -> String {
        "a".repeat(64)
    }

    #[test]
    fn scopes_are_names_on_any_store() {
        assert!(valid_scope("f0123abcd-2-members"));
        for bad in ["", "-x", "UPPER", "a/b", "a.b", &"x".repeat(129)] {
            assert!(!valid_scope(bad), "{bad}");
        }
    }

    #[test]
    fn an_upsert_is_bounded() {
        let ok = json!({ "source": "sessions/a.json", "revision": "r1", "space": null, "sections": [{ "id": "t0", "chunks": [{ "text": "hi", "hash": hash() }] }] });
        assert_eq!(upsert(ok.clone()).check(), Ok(()));
        let refused = |patch: Value| {
            let mut v = ok.clone();
            json_patch(&mut v, patch);
            upsert(v).check().unwrap_err().0
        };
        assert!(refused(json!({ "source": "" })).contains("source is named"));
        assert!(refused(json!({ "sections": [{ "id": "t0", "chunks": [] }] })).contains("1 to 32 chunks"));
        assert!(refused(json!({ "sections": [{ "id": "t0", "chunks": [{ "text": "x", "hash": "nope" }] }] })).contains("sha256"));
        assert!(refused(json!({ "sections": [{ "id": "t0", "chunks": [{ "text": "x", "hash": hash(), "vector": "AAAA" }] }] })).contains("name it"));
        let twice = json!([{ "id": "t", "chunks": [{ "text": "x", "hash": hash() }] }, { "id": "t", "chunks": [{ "text": "y", "hash": hash() }] }]);
        assert!(refused(json!({ "sections": twice })).contains("twice"));
        assert!(refused(json!({ "fields": { "Kind": "chat" } })).contains("a-z"));
        assert!(refused(json!({ "fields": { "kind": "a=b" } })).contains("="));
        assert!(refused(json!({ "sections": [{ "id": "t0", "location": [1, 2], "chunks": [{ "text": "x", "hash": hash() }] }] })).contains("JSON object"));
        let many: Vec<Value> = (0..=SECTIONS_MAX).map(|i| json!({ "id": format!("t{i}"), "chunks": [{ "text": "x", "hash": hash() }] })).collect();
        assert!(refused(json!({ "sections": many })).contains("at most 2000 sections"));
        assert!(serde_json::from_value::<Upsert>(json!({ "source": "s", "revision": "r", "space": null, "sections": [], "order": 1 })).is_err(), "no field it does not know");
    }

    fn json_patch(v: &mut Value, patch: Value) {
        for (k, p) in patch.as_object().unwrap() {
            v[k] = p.clone();
        }
    }

    #[test]
    fn a_search_names_what_it_searches_by() {
        let s = |v: Value| serde_json::from_value::<Search>(v).unwrap().check();
        assert_eq!(s(json!({ "q": "harbour" })), Ok(()));
        assert!(s(json!({})).is_err());
        assert!(s(json!({ "q": "x", "mode": "semantic" })).is_err());
        assert!(s(json!({ "q": "x", "vector": "AAAA" })).is_err(), "a vector without its space");
        assert!(s(json!({ "q": "x", "limit": 51 })).is_err());
    }

    #[test]
    fn vectors_are_their_space_s_length() {
        use base64::Engine;
        let b = base64::engine::general_purpose::STANDARD.encode([1.0f32.to_le_bytes(), 0.5f32.to_le_bytes()].concat());
        assert_eq!(vector_of(&b, 2).unwrap(), vec![1.0, 0.5]);
        assert!(vector_of(&b, 3).is_err());
        let nan = base64::engine::general_purpose::STANDARD.encode(f32::NAN.to_le_bytes());
        assert!(vector_of(&nan, 1).is_err());
    }
}
