//! Fixtures the tests compile and the source guard reads as source
//! (`include_str!`, so the guard reads the code that runs): the review's
//! five false passes at `8672f486`, each in two versions, and one for each
//! of the guard's other refusals. No macro at item level: the guard
//! refuses one.
#![allow(dead_code)]

/// Review 1: a default stated once beside `oneOf[a, oneOf[b, c]]`, of a
/// field each variant reads its own way; `c` reads `true` before and
/// `false` after.
pub mod nested {
    use std::borrow::Cow;

    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use serde::{Deserialize, Serialize};

    fn yes() -> bool {
        true
    }
    fn no() -> bool {
        false
    }
    fn is_yes(b: &bool) -> bool {
        *b
    }
    fn is_no(b: &bool) -> bool {
        !*b
    }

    fn schema() -> Schema {
        json_schema!({
            "type": "object",
            "properties": { "kind": { "type": "string" }, "dock": { "type": "boolean", "default": true } },
            "required": ["kind"],
            "oneOf": [
                { "properties": { "kind": { "const": "a" } } },
                { "oneOf": [{ "properties": { "kind": { "const": "b" } } }, { "properties": { "kind": { "const": "c" } } }] },
            ],
        })
    }

    pub mod old {
        use super::*;
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        pub enum Nested {
            A {
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                dock: bool,
            },
            B {
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                dock: bool,
            },
            C {
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                dock: bool,
            },
        }
        impl JsonSchema for Nested {
            fn schema_name() -> Cow<'static, str> {
                "Nested".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }

    pub mod new {
        use super::*;
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        pub enum Nested {
            A {
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                dock: bool,
            },
            B {
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                dock: bool,
            },
            C {
                #[serde(default = "no", skip_serializing_if = "is_no")]
                dock: bool,
            },
        }
        impl JsonSchema for Nested {
            fn schema_name() -> Cow<'static, str> {
                "Nested".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }
}

/// Review 2: a field `b` forbids by its schema, which `b` reads as a
/// derived enum of names the schema's `string` never tries.
pub mod mode {
    use std::borrow::Cow;

    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    pub enum Mode {
        Red,
        Blue,
    }

    fn schema() -> Schema {
        json_schema!({
            "type": "object",
            "properties": { "kind": { "type": "string" }, "mode": { "type": "string" } },
            "required": ["kind"],
            "oneOf": [
                { "properties": { "kind": { "const": "a" } }, "required": ["mode"] },
                { "properties": { "kind": { "const": "b" }, "mode": false } },
            ],
        })
    }

    pub mod old {
        use super::*;
        fn default_mode() -> Mode {
            Mode::Red
        }
        fn is_default(m: &Mode) -> bool {
            *m == Mode::Red
        }
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        pub enum Tagged {
            A {
                mode: String,
            },
            B {
                #[serde(default = "default_mode", skip_serializing_if = "is_default")]
                mode: Mode,
            },
        }
        impl JsonSchema for Tagged {
            fn schema_name() -> Cow<'static, str> {
                "Tagged".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }

    pub mod new {
        use super::*;
        fn default_mode() -> Mode {
            Mode::Blue
        }
        fn is_default(m: &Mode) -> bool {
            *m == Mode::Blue
        }
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        pub enum Tagged {
            A {
                mode: String,
            },
            B {
                #[serde(default = "default_mode", skip_serializing_if = "is_default")]
                mode: Mode,
            },
        }
        impl JsonSchema for Tagged {
            fn schema_name() -> Cow<'static, str> {
                "Tagged".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }
}

/// Review 3: a required field added under a dotted name.
pub mod dotted {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            pub id: String,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            pub id: String,
            #[serde(rename = "settings.mode")]
            pub mode: bool,
        }
    }
}

/// Review 4: an untagged enum whose branches the schema tells apart by a
/// constant serde never reads; `b`'s default changes.
pub mod shadow {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    fn yes() -> bool {
        true
    }
    fn is_yes(b: &bool) -> bool {
        *b
    }
    fn no() -> bool {
        false
    }
    fn is_no(b: &bool) -> bool {
        !*b
    }

    pub mod old {
        use super::*;
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        #[serde(untagged)]
        pub enum Shadow {
            A {
                #[schemars(extend("const" = "a"))]
                kind: String,
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                #[schemars(extend("default" = true))]
                dock: bool,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schemars(extend("default" = null))]
                n: Option<u8>,
            },
            B {
                #[schemars(extend("const" = "b"))]
                kind: String,
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                #[schemars(extend("default" = true))]
                dock: bool,
                n: u16,
            },
        }
    }

    pub mod new {
        use super::*;
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        #[serde(untagged)]
        pub enum Shadow {
            A {
                #[schemars(extend("const" = "a"))]
                kind: String,
                #[serde(default = "yes", skip_serializing_if = "is_yes")]
                #[schemars(extend("default" = true))]
                dock: bool,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schemars(extend("default" = null))]
                n: Option<u8>,
            },
            B {
                #[schemars(extend("const" = "b"))]
                kind: String,
                #[serde(default = "no", skip_serializing_if = "is_no")]
                #[schemars(extend("default" = true))]
                dock: bool,
                n: u16,
            },
        }
    }
}

/// Review 5: an input alias, then not.
pub mod alias {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Alias {
            #[serde(alias = "legacy")]
            pub flag: bool,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Alias {
            pub flag: bool,
        }
    }
}

/// A schema written by hand, the same for both versions, while serde's
/// field is renamed under it.
pub mod lying {
    use std::borrow::Cow;

    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use serde::{Deserialize, Serialize};

    fn schema() -> Schema {
        json_schema!({ "type": "object", "properties": { "a": { "type": "integer" } }, "required": ["a"] })
    }

    pub mod old {
        use super::*;
        #[derive(Debug, Serialize, Deserialize)]
        pub struct Lying {
            pub b: u8,
        }
        impl JsonSchema for Lying {
            fn schema_name() -> Cow<'static, str> {
                "Lying".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }
    pub mod new {
        use super::*;
        #[derive(Debug, Serialize, Deserialize)]
        pub struct Lying {
            pub c: u8,
        }
        impl JsonSchema for Lying {
            fn schema_name() -> Cow<'static, str> {
                "Lying".into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schema()
            }
        }
    }
}

/// A reader hook schemars does not see: it read a number as a string too,
/// then not.
pub mod hooked {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Deserializer, Serialize};
        fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
            match serde_json::Value::deserialize(d)? {
                serde_json::Value::String(s) => s.parse().map_err(serde::de::Error::custom),
                v => u8::deserialize(v).map_err(serde::de::Error::custom),
            }
        }
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Hooked {
            #[serde(deserialize_with = "lenient")]
            pub n: u8,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Hooked {
            pub n: u8,
        }
    }
}

/// The guard's other refusals, one shape each.
pub mod refused {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Inner {
        pub a: u8,
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Flat {
        pub id: String,
        #[serde(flatten)]
        pub inner: Inner,
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Skipped {
        #[serde(skip_deserializing)]
        pub n: u8,
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub enum External {
        Docked { dock: bool },
        Other(u8),
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Generic<T> {
        pub t: T,
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    pub struct Timed {
        pub after: std::time::Duration,
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    #[serde(tag = "type")]
    pub enum Dotted {
        #[serde(rename = "a.b")]
        A { n: u8 },
    }
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    #[schemars(rename = "Other")]
    pub struct Renamed {
        pub n: u8,
    }
}

/// Review 6 (`375914c1`, 1): an input fallback, then not.
pub mod other {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "kind")]
        pub enum Row {
            A,
            #[serde(other)]
            Other,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "kind")]
        pub enum Row {
            A,
            Other,
        }
    }
}

/// Review 7 (2): a skip predicate that omits a value other than the default.
pub mod skipped {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        fn skip(_: &bool) -> bool {
            false
        }
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            #[serde(default, skip_serializing_if = "skip")]
            #[schemars(extend("default" = false))]
            pub flag: bool,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        fn skip(b: &bool) -> bool {
            *b
        }
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            #[serde(default, skip_serializing_if = "skip")]
            #[schemars(extend("default" = false))]
            pub flag: bool,
        }
    }
}

/// Review 8 (3a): two definitions of one type, a `cfg` choosing the one
/// compiled; the alias is on the one compiled before.
pub mod twice {
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[cfg(not(any()))]
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            #[serde(alias = "legacy")]
            pub flag: bool,
        }
        #[cfg(any())]
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            pub flag: bool,
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        #[cfg(not(any()))]
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            pub flag: bool,
        }
        #[cfg(any())]
        #[derive(Debug, Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            pub flag: bool,
        }
    }
}

/// Review 9 (4): a `Debug` written by hand that prints no field, so two
/// readings of it look the same.
pub mod debugged {
    fn skip(_: &bool) -> bool {
        true
    }
    pub mod old {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        use super::skip;
        fn selected() -> bool {
            false
        }
        #[derive(Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            #[serde(default = "selected", skip_serializing_if = "skip")]
            #[schemars(extend("default" = false))]
            pub flag: bool,
        }
        impl std::fmt::Debug for Row {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("Row")
            }
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};
        use super::skip;
        fn selected() -> bool {
            true
        }
        #[derive(Serialize, Deserialize, JsonSchema)]
        pub struct Row {
            #[serde(default = "selected", skip_serializing_if = "skip")]
            #[schemars(extend("default" = false))]
            pub flag: bool,
        }
        impl std::fmt::Debug for Row {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("Row")
            }
        }
    }
}

/// The second review at `375914c1`: a reader written by hand under a
/// renamed trait; it read `legacy` too, then not.
pub mod renamed {
    pub mod old {
        use schemars::JsonSchema;
        use serde::Deserialize as De;
        use serde::{Deserializer, Serialize};
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            flag: Option<bool>,
            #[serde(default)]
            legacy: Option<bool>,
        }
        #[derive(Debug, Serialize, JsonSchema)]
        pub struct Row {
            pub flag: bool,
        }
        impl<'de> De<'de> for Row {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Row, D::Error> {
                let raw = Raw::deserialize(d)?;
                raw.flag.or(raw.legacy).map(|flag| Row { flag }).ok_or_else(|| serde::de::Error::missing_field("flag"))
            }
        }
    }
    pub mod new {
        use schemars::JsonSchema;
        use serde::Deserialize as De;
        use serde::{Deserializer, Serialize};
        #[derive(serde::Deserialize)]
        struct Raw {
            flag: bool,
        }
        #[derive(Debug, Serialize, JsonSchema)]
        pub struct Row {
            pub flag: bool,
        }
        impl<'de> De<'de> for Row {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Row, D::Error> {
                Raw::deserialize(d).map(|r| Row { flag: r.flag })
            }
        }
    }
}
