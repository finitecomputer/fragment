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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub source: String,
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
    pub score: f64,
    pub signals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub results: Vec<Hit>,
    /// `on`, or `off`: this deployment embeds nothing, so words alone.
    pub semantic: String,
}
