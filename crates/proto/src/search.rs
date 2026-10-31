//! A fragment's search (docs/api.md, A fragment's search): what an app's writer sends
//! and a reader asks, at `/api/f/{name}/search*`. Optional fields are left
//! out, never null.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One upsert's limits: what an import sends a conversation as.
pub const SECTIONS_MAX: usize = 2_000;
/// A section's text, in characters (Unicode scalar values): the brain's
/// `SECTION_MAX_CHARS`.
pub const TEXT_MAX_CHARS: usize = 8_000;
pub const SECTION_ID_MAX_BYTES: usize = 64;
pub const SOURCE_MAX_BYTES: usize = 512;
pub const REVISION_MAX_BYTES: usize = 128;
/// An upsert's body, at most, as sent.
pub const UPSERT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const QUERY_MAX_BYTES: usize = 1024;
pub const RESULTS_MAX: usize = 50;
pub const RESULTS_DEFAULT: usize = 10;
pub const SOURCES_PAGE_MAX: usize = 1000;

/// Where in its source a section is: its first and last message (both
/// inclusive), or whatever pair its app counts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub from: u64,
    pub to: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

/// A source, whole: its sections replace whatever it held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upsert {
    /// Its id in the fragment: a file's path, when it is one (a push that
    /// deletes that file deletes it from the index too).
    pub source: String,
    /// Its content's identity (a file's sha256): the same again writes
    /// nothing.
    pub revision: String,
    /// The revision this one replaces (`""`: only if it has none): written
    /// only if it is the source's now, else 409 `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    pub sections: Vec<Section>,
    /// A writer's own generation for the source (0 to 2^53 − 1): taken only
    /// at or above the highest the index took for it, so a writer's stale
    /// work never lands over its newer (409 `conflict` naming `fence`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub source: String,
    /// As an upsert's: a delete keeps it, so a stale write cannot bring the
    /// source back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Hybrid,
    Lexical,
    Semantic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub q: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// One answer: a section, shown by its best chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub rank: usize,
    pub source: String,
    pub section: String,
    pub revision: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Value>,
    pub snippet: String,
    /// Its fused score: reciprocal rank fusion (k = 60) of the legs below.
    pub score: f64,
    pub signals: Vec<String>,
    /// Where the words leg found it, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lexical: Option<Lexical>,
    /// Where the meaning leg found it, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<Semantic>,
}

/// A section's place in the words leg: its rank there (from 1) and its
/// best chunk's BM25, higher better, by this fragment's own statistics
/// (so not comparable across fragments).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lexical {
    pub rank: usize,
    pub bm25: f64,
}

/// A section's place in the meaning leg: its rank there (from 1) and its
/// best chunk's cosine with the question, as `SCHEME` estimates it:
/// comparable across fragments whose answers name the same space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Semantic {
    pub rank: usize,
    pub cosine: f64,
}

/// How every index holds a vector: each component as int8 at the one
/// scale 127/0.25 (so a dot product over 508² is a cosine), and its sign
/// bit, which the first pass reads.
pub const SCHEME: &str = "i8x508+sign";

/// What a deployment's vectors mean: its embeddings model, their
/// dimensions, and the version it names, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Space {
    pub model: String,
    pub dimensions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub results: Vec<Hit>,
    /// `on`, or `off`: words alone answered (this deployment embeds
    /// nothing, the question was not embedded, or the index holds vectors
    /// of another space until its sources are written again).
    pub semantic: String,
    /// Whether this life ever wrote its index: `false`, nothing was asked
    /// of it, and `results` is empty.
    pub indexed: bool,
    /// The space of the cosines, and `SCHEME`: both, whenever this
    /// deployment embeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<Space>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

/// A question the platform embedded already, for one fragment of many
/// (`POST /search/asked`, a supervisor's route the router never maps):
/// searched by `vector` (the deployment's space, little-endian float32,
/// base64), never embedded again, so its asker pays once; with none, by
/// its words alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forwarded {
    pub query: Query,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<String>,
}

/// Fragments one question across fragments asks, at most.
pub const IN_MAX: usize = 32;
/// Hits a question across fragments asks each one for, at least: the
/// merge's depth (fragment_core::across::asked_limit).
pub const ASKED_MIN: usize = 20;

/// `POST /api/search`: one question, asked of several fragments' own
/// indexes as the signer (docs/api.md, "Search across fragments").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Across {
    pub q: String,
    /// The fragments asked, each named in full.
    #[serde(rename = "in")]
    pub fragments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// How a fragment asked answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reached {
    /// Its index was searched (its hits may be none).
    Searched,
    /// It holds no index: no source was ever written.
    NoIndex,
    /// The signer may not read it.
    Denied,
    /// There is no such fragment.
    Missing,
    /// It did not answer, or refused for another reason (`message`).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub name: String,
    pub state: Reached,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A hit, and the fragment it is from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FromFragment {
    pub fragment: String,
    #[serde(flatten)]
    pub hit: Hit,
}

/// The answer across fragments: the hits merged, best first, and how each
/// fragment asked answered, in the order asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FoundAcross {
    pub results: Vec<FromFragment>,
    pub fragments: Vec<Asked>,
    /// `on` when the question was searched by its meaning too.
    pub semantic: String,
}
