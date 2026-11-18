//! What a meter counted (`Usage`), as it is written and read, and whether a book can price it.

use super::limits::{NAME_MAX_BYTES, QUANTITY_MAX};
use serde::{Deserialize, Serialize};

/// What happened, in a meter's own unit (decision 24). The quantities are
/// exactly what the source counted; the book prices them. Written tagged by
/// `kind`; read as its fields (`UsageFields`), not as serde's internally
/// tagged reading, which buffers each usage to find its kind: about 30 KB
/// more of the cell's Wasm. Both read every form a release has written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "UsageFields")]
pub enum Usage {
    /// A model's tokens through AI Gateway, by its catalog id. `input`
    /// excludes cached tokens (OpenAI-shaped usage counts them in
    /// `prompt_tokens`, so the intercept subtracts them); `cache_write` is
    /// Anthropic's cache creation. Only the stream's last, cumulative
    /// usage is metered (spike S4).
    Tokens { model: String, input: u64, cached_input: u64, cache_write: u64, output: u64 },
    /// Workers AI neurons through the gateway, in thousandths of a neuron
    /// (the fraction Workers AI reports, × 1000, rounded up; an image's,
    /// from its tiles and steps: crate::media).
    Neurons { milli: u64 },
    /// A computer awake, in milliseconds, on its instance type (the
    /// book's name for it).
    Awake { instance: String, ms: u64 },
    /// Bytes held, sampled: the sample's bytes × the hours since the last.
    Storage { class: StorageClass, byte_hours: u64 },
    /// Requests served (each a router Worker request and the Durable
    /// Object request behind it).
    Requests { count: u64 },
    /// Unique dynamic workers (one fragment's code version) loaded on one
    /// UTC day.
    DynamicWorkers { count: u64 },
    /// Browser Rendering time, in milliseconds (a deploy's screenshot).
    Browser { ms: u64 },
    /// Cloudflare Images' unique transformations.
    Images { count: u64 },
    /// An operator key's use (decision 37), in the key's own unit: a call,
    /// unless its price counts something else.
    Key { key: String, units: u64 },
}

/// A `Usage` as it is read: every kind's fields, flat, none unknown, and
/// none `null` (`named`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageFields {
    kind: UsageKind,
    #[serde(default, deserialize_with = "named")]
    model: Option<String>,
    #[serde(default, deserialize_with = "named")]
    input: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    cached_input: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    cache_write: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    output: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    milli: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    instance: Option<String>,
    #[serde(default, deserialize_with = "named")]
    ms: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    class: Option<StorageClass>,
    #[serde(default, deserialize_with = "named")]
    byte_hours: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    count: Option<u64>,
    #[serde(default, deserialize_with = "named")]
    key: Option<String>,
    #[serde(default, deserialize_with = "named")]
    units: Option<u64>,
}

/// A field `UsageFields` names: its value, never `null`. An `Option`'s
/// `null` would read as absent, so a field of another kind's, `null`, would
/// pass where the tagged reading refused it as unknown.
fn named<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// A usage's `kind`: read as an identifier, a string alone, as the tagged
/// reading read its tag (a derived enum also takes `{"<kind>": null}`).
#[derive(Deserialize)]
#[serde(variant_identifier, rename_all = "snake_case")]
enum UsageKind {
    Tokens,
    Neurons,
    Awake,
    Storage,
    Requests,
    DynamicWorkers,
    Browser,
    Images,
    Key,
}

impl TryFrom<UsageFields> for Usage {
    type Error = String;

    /// The kind its `kind` names, with each of that kind's fields and no
    /// other kind's, as the tagged reading refused one.
    fn try_from(f: UsageFields) -> Result<Usage, String> {
        fn need<T>(v: Option<T>, name: &str) -> Result<T, String> {
            v.ok_or_else(|| format!("missing field `{name}`"))
        }
        let named = [
            f.model.is_some(),
            f.input.is_some(),
            f.cached_input.is_some(),
            f.cache_write.is_some(),
            f.output.is_some(),
            f.milli.is_some(),
            f.instance.is_some(),
            f.ms.is_some(),
            f.class.is_some(),
            f.byte_hours.is_some(),
            f.count.is_some(),
            f.key.is_some(),
            f.units.is_some(),
        ];
        let (usage, fields) = match f.kind {
            UsageKind::Tokens => (
                Usage::Tokens {
                    model: need(f.model, "model")?,
                    input: need(f.input, "input")?,
                    cached_input: need(f.cached_input, "cached_input")?,
                    cache_write: need(f.cache_write, "cache_write")?,
                    output: need(f.output, "output")?,
                },
                5,
            ),
            UsageKind::Neurons => (Usage::Neurons { milli: need(f.milli, "milli")? }, 1),
            UsageKind::Awake => (Usage::Awake { instance: need(f.instance, "instance")?, ms: need(f.ms, "ms")? }, 2),
            UsageKind::Storage => (Usage::Storage { class: need(f.class, "class")?, byte_hours: need(f.byte_hours, "byte_hours")? }, 2),
            UsageKind::Requests => (Usage::Requests { count: need(f.count, "count")? }, 1),
            UsageKind::DynamicWorkers => (Usage::DynamicWorkers { count: need(f.count, "count")? }, 1),
            UsageKind::Browser => (Usage::Browser { ms: need(f.ms, "ms")? }, 1),
            UsageKind::Images => (Usage::Images { count: need(f.count, "count")? }, 1),
            UsageKind::Key => (Usage::Key { key: need(f.key, "key")?, units: need(f.units, "units")? }, 2),
        };
        // each of its own fields is there, so any more is another kind's
        match named.iter().filter(|n| **n).count() == fields {
            true => Ok(usage),
            false => Err("unknown field: a usage names its own kind's fields alone".into()),
        }
    }
}

/// Where sampled bytes are held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    /// Blobs, site copies, screenshots and backups.
    R2,
    /// A fragment's SQLite (its Durable Object's).
    Sqlite,
    /// A fragment's git repository at code.storage.
    Git,
}

/// Why a usage is not one the book can price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFault {
    /// A model, instance or key name that is empty, too long, or not
    /// printable ASCII.
    Name,
    /// A quantity over `QUANTITY_MAX`.
    Quantity,
}

/// Whether `s` is 1 to `max` bytes of printable ASCII without spaces: the
/// shape of every id and name the ledger stores.
pub fn printable(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

impl Usage {
    /// Whether the usage is one a book can price: its names well formed,
    /// its quantities within `QUANTITY_MAX`.
    pub fn validate(&self) -> Result<(), UsageFault> {
        let name = match self {
            Usage::Tokens { model, .. } => Some(model),
            Usage::Awake { instance, .. } => Some(instance),
            Usage::Key { key, .. } => Some(key),
            _ => None,
        };
        if let Some(n) = name {
            if !printable(n, NAME_MAX_BYTES) {
                return Err(UsageFault::Name);
            }
        }
        let largest = match self {
            Usage::Tokens { input, cached_input, cache_write, output, .. } => *input.max(cached_input).max(cache_write).max(output),
            Usage::Neurons { milli } => *milli,
            Usage::Awake { ms, .. } | Usage::Browser { ms } => *ms,
            Usage::Storage { byte_hours, .. } => *byte_hours,
            Usage::Requests { count } | Usage::DynamicWorkers { count } | Usage::Images { count } => *count,
            Usage::Key { units, .. } => *units,
        };
        if largest <= QUANTITY_MAX {
            Ok(())
        } else {
            Err(UsageFault::Quantity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::defaults::INSTANCE;
    use crate::price::testing::{tokens, FLASH};

    /// Usages are tagged by kind, in snake_case, and refuse unknown fields.
    #[test]
    fn a_usage_is_tagged_by_kind() {
        let u = tokens(FLASH, 1, 2, 3, 4);
        assert_eq!(
            serde_json::to_value(&u).unwrap(),
            serde_json::json!({ "kind": "tokens", "model": FLASH, "input": 1, "cached_input": 2, "cache_write": 3, "output": 4 })
        );
        let s = Usage::Storage { class: StorageClass::Sqlite, byte_hours: 9 };
        assert_eq!(serde_json::to_value(&s).unwrap(), serde_json::json!({ "kind": "storage", "class": "sqlite", "byte_hours": 9 }));
        assert!(serde_json::from_value::<Usage>(serde_json::json!({ "kind": "requests", "count": 1, "extra": 2 })).is_err());
        assert!(serde_json::from_value::<Usage>(serde_json::json!({ "kind": "requests", "count": -1 })).is_err(), "quantities are unsigned");
        let billed = serde_json::json!({ "kind": "billed", "vendor": "openrouter", "micros": 1 });
        assert!(serde_json::from_value::<Usage>(billed).is_err(), "a vendor's own bill went with OpenRouter: every meter is Cloudflare's");
    }

    /// A usage's `kind` is a name, a string, as the tagged reading's tag
    /// was: serde's other spelling of a variant, an object
    /// (`{"requests": null}`), is no kind, for every kind (FIN-453 M7,
    /// review: it was charged).
    #[test]
    fn a_usage_kind_is_named_by_a_string_alone() {
        let every = [
            tokens(FLASH, 1, 2, 3, 4),
            Usage::Neurons { milli: 1 },
            Usage::Awake { instance: INSTANCE.into(), ms: 1 },
            Usage::Storage { class: StorageClass::R2, byte_hours: 1 },
            Usage::Requests { count: 1_000_000 },
            Usage::DynamicWorkers { count: 1 },
            Usage::Browser { ms: 1 },
            Usage::Images { count: 1 },
            Usage::Key { key: "brave".into(), units: 1 },
        ];
        for usage in every {
            let text = serde_json::to_string(&usage).unwrap();
            assert_eq!(serde_json::from_slice::<Usage>(text.as_bytes()).ok(), Some(usage.clone()), "{text}");
            let kind = serde_json::to_value(&usage).unwrap()["kind"].as_str().unwrap().to_string();
            let object = text.replacen(&format!("\"kind\":\"{kind}\""), &format!("\"kind\":{{\"{kind}\":null}}"), 1);
            assert_ne!(object, text);
            let e = serde_json::from_slice::<Usage>(object.as_bytes()).map(|u| format!("{u:?}")).expect_err(&object).to_string();
            assert!(e.contains("invalid type: map"), "{object}: {e}");
        }
    }

    /// Read as its fields, a usage is as strict as the tagged reading was:
    /// its kind's every field, no other kind's, in any order, from bytes.
    #[test]
    fn a_usage_reads_as_its_kinds_fields_alone() {
        let read = |text: &str| serde_json::from_slice::<Usage>(text.as_bytes()).map_err(|e| e.to_string());
        assert_eq!(read(r#"{"ms":5,"instance":"standard-1","kind":"awake"}"#), Ok(Usage::Awake { instance: "standard-1".into(), ms: 5 }), "the tag anywhere");
        assert_eq!(read(r#"{"kind":"key","key":"brave","units":2}"#), Ok(Usage::Key { key: "brave".into(), units: 2 }));
        for (text, says) in [
            (r#"{"kind":"requests","count":1,"ms":2}"#, "unknown field"),
            (r#"{"kind":"browser","ms":1,"instance":"standard-1"}"#, "unknown field"),
            (r#"{"kind":"tokens","model":"m","input":1,"cached_input":0,"cache_write":0}"#, "missing field `output`"),
            (r#"{"kind":"awake","ms":5}"#, "missing field `instance`"),
            (r#"{"count":1}"#, "missing field `kind`"),
            (r#"{"kind":"storage","class":"disk","byte_hours":1}"#, "unknown variant `disk`"),
            (r#"{"kind":"images","count":null}"#, "invalid type: null"),
            (r#"{"kind":"requests","count":1000000,"ms":null}"#, "invalid type: null"),
            (r#"{"kind":"key","key":"brave","units":1,"model":null}"#, "invalid type: null"),
        ] {
            let e = read(text).expect_err(text);
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
