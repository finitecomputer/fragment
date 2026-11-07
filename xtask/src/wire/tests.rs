//! The wire lock's rules, cell by cell (lock.rs's table), its retirements,
//! and the files: valid, invalid, replay and restart.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use super::lock::{breaches, parse, project, Breach, Facts};
use super::{generate, retired, verdict, write_files, Base, OnDisk, LOCK_PATH, RETIRED_HEADER, RETIRED_PATH};

/// Master's facts: a shape each way, enums each way, an open union, a
/// closed shape each way, bounds and defaults each way, and a flattened
/// union each way.
const BASE: &str = "\
route a.get GET /api/a
route a.get accepts session
route a.get path a.get.path
route a.get query a.get.query
route a.get response A
type a.get.path in object
field a.get.path.name required string
type a.get.query in object
route a.put PUT /api/a
route a.put request B
type A out object
field A.x required string
field A.o optional string
type B in object
field B.x required string
field B.o optional string
type C both object
field C.x required string
field C.o optional string
type EO out enum
value EO one
type EI in enum
value EI one
type EB both enum
value EB one
type UO out union(type) open
value UO one
type UO.one out object
field UO.one.type required \"one\"
type K out object closed
field K.x required string
type KI in object closed
field KI.x required string
type KO in object
field KO.x required string
field A.z optional string|null
field B.z optional string|null
field A.r required X
type O out object
field O.* optional string
type BO out object
field BO.s optional string
bound BO.s length 0..=64
bound BO.s pattern ^[a-z]+$
bound BO.s format date-time
field BO.t optional string
field BO.l optional array<string>
bound BO.l items 0..=10
bound BO.l unique
field BO.n required integer
bound BO.n range u32
field BO.k optional integer
bound BO.k multiple 2
field BO.d optional string
bound BO.d default \"x\"
field BO.m optional map<string>
bound BO.m props 0..=5
type BI in object
field BI.s optional string
bound BI.s length 0..=64
bound BI.s pattern ^[a-z]+$
bound BI.s format date-time
field BI.t optional string
field BI.l optional array<string>
bound BI.l items 0..=10
bound BI.l unique
field BI.n required integer
bound BI.n range u32
field BI.k optional integer
bound BI.k multiple 2
field BI.d optional string
bound BI.d default \"x\"
field BI.r required string
bound BI.r default \"y\"
field BI.m optional map<string>
bound BI.m props 0..=5
type BB both object
field BB.n required integer
bound BB.n range u32
type F out object
field F.id required string
branches F anyOf
value F #0
value F #1
type F.#0 out object
field F.#0.extra required string
type F.#1 out object
field F.#1.extra required boolean
type FI in object
branches FI anyOf
value FI #0
type FI.#0 in object
field FI.#0.extra required string
";

/// What a change to master's facts breaks: `edits` are (master's line,
/// its replacement; `None` drops it), `added` new lines.
fn check(edits: &[(&str, Option<&str>)], added: &[&str], retired: &[&str]) -> Vec<Breach> {
    let mut head: Vec<String> = BASE.lines().map(str::to_string).collect();
    for (from, to) in edits {
        let at = head.iter().position(|l| l == from).unwrap_or_else(|| panic!("{from} is master's"));
        match to {
            Some(to) => head[at] = to.to_string(),
            None => {
                head.remove(at);
            }
        }
    }
    head.extend(added.iter().map(|l| l.to_string()));
    let retired: BTreeSet<String> = retired.iter().map(|l| l.to_string()).collect();
    breaches(&parse(BASE).unwrap(), &parse(&head.join("\n")).unwrap(), &retired)
}

fn passes(edits: &[(&str, Option<&str>)], added: &[&str]) {
    let found = check(edits, added, &[]);
    assert!(found.is_empty(), "{edits:?} {added:?} should pass: {found:?}");
}

/// It fails, naming exactly this fact.
fn fails(edits: &[(&str, Option<&str>)], added: &[&str], fact: &str) {
    let found = check(edits, added, &[]);
    assert_eq!(found.iter().map(|b| b.fact.as_str()).collect::<Vec<_>>(), [fact], "{edits:?} {added:?}: {found:?}");
    assert!(!found[0].why.is_empty());
}

#[test]
fn what_an_older_client_does_not_feel_passes() {
    passes(&[], &[]);
    passes(&[], &["route b.get GET /api/b", "route b.get response A"]);
    for t in ["A", "B", "C"] {
        passes(&[], &[&format!("field {t}.n optional string")]);
    }
    passes(&[], &["field A.n required string"]);
    passes(&[("field A.o optional string", Some("field A.o required string"))], &[]);
    passes(&[("field B.x required string", Some("field B.x optional string"))], &[]);
    passes(&[], &["value UO two", "type UO.two out object"]);
    passes(&[], &["value EI two"]);
    passes(&[], &["field KI.n optional string"]);
    // the cell takes what it refused
    passes(&[("type KI in object closed", Some("type KI in object"))], &[]);
    passes(&[("type EI in enum", Some("type EI in enum open"))], &[]);
    // another credential
    passes(&[], &["route a.get accepts nip98"]);
    // a route's first query parameter, left out by every older client
    passes(&[], &["field a.get.query.q optional string"]);
}

#[test]
fn what_an_older_client_feels_fails_naming_the_fact() {
    // a new required field, and optional → required, where a client writes it
    fails(&[], &["field B.n required string"], "field B.n required string");
    fails(&[], &["field C.n required string"], "field C.n required string");
    fails(&[("field B.o optional string", Some("field B.o required string"))], &[], "field B.o optional string");
    fails(&[("field C.o optional string", Some("field C.o required string"))], &[], "field C.o optional string");
    // required → optional where a client reads it
    fails(&[("field A.x required string", Some("field A.x optional string"))], &[], "field A.x required string");
    fails(&[("field C.x required string", Some("field C.x optional string"))], &[], "field C.x required string");
    // a field gone or renamed, or of another type, whoever writes it
    for t in ["A", "B", "C"] {
        fails(&[(&format!("field {t}.x required string"), None)], &[], &format!("field {t}.x required string"));
        fails(&[(&format!("field {t}.o optional string"), Some(&format!("field {t}.p optional string")))], &[], &format!("field {t}.o optional string"));
        fails(&[(&format!("field {t}.o optional string"), Some(&format!("field {t}.o optional integer")))], &[], &format!("field {t}.o optional string"));
    }
    // a new value of a closed enum a client reads
    fails(&[], &["value EO two"], "value EO two");
    fails(&[], &["value EB two"], "value EB two");
    // a value or variant gone
    for t in ["EO", "EI", "EB", "UO"] {
        fails(&[(&format!("value {t} one"), None)], &[], &format!("value {t} one"));
    }
    // a route gone, moved, or answering another shape
    fails(&[("route a.get GET /api/a", None)], &[], "route a.get GET /api/a");
    fails(&[("route a.get GET /api/a", Some("route a.get GET /api/b"))], &[], "route a.get GET /api/a");
    fails(&[("route a.get response A", Some("route a.get response C"))], &[], "route a.get response A");
    fails(&[], &["route a.get request B"], "route a.get request B");
    // a new field of a closed shape a client reads
    fails(&[], &["field K.n optional string"], "field K.n optional string");
    // a credential an older client sends
    fails(&[("route a.get accepts session", None)], &[], "route a.get accepts session");
    // another component
    fails(&[("field A.r required X", Some("field A.r required Y"))], &[], "field A.r required X");
    // a path's parameter of another type, its route's template the same
    fails(&[("field a.get.path.name required string", Some("field a.get.path.name required integer"))], &[], "field a.get.path.name required string");
    // a query parameter an older client does not send
    fails(&[], &["field a.get.query.q required string"], "field a.get.query.q required string");
    // a shape that refuses what it took, or opens where a client reads it closed
    fails(&[("type KO in object", Some("type KO in object closed"))], &[], "type KO in object closed");
    fails(&[("type EO out enum", Some("type EO out enum open"))], &[], "type EO out enum open");
    // a shape of another kind
    fails(&[("type EO out enum", Some("type EO out object"))], &[], "type EO out enum");
    // a union's tag its readers may no longer find
    fails(&[("field UO.one.type required \"one\"", Some("field UO.one.type optional \"one\""))], &[], "field UO.one.type required \"one\"");
}

/// A bound's pass and fail cells: narrower where the cell reads it fails,
/// wider where a client reads it fails, either way where both do.
#[test]
fn bounds_hold_by_who_reads_them() {
    let (bo, bi) = ("bound BO.n range u32", "bound BI.n range u32");
    passes(&[(bo, Some("bound BO.n range u16"))], &[]);
    fails(&[(bo, Some("bound BO.n range u64"))], &[], bo);
    fails(&[(bo, Some("bound BO.n range i32"))], &[], bo);
    passes(&[(bi, Some("bound BI.n range u64"))], &[]);
    passes(&[(bi, Some("bound BI.n range i64"))], &[]);
    fails(&[(bi, Some("bound BI.n range u16"))], &[], bi);
    fails(&[(bi, Some("bound BI.n range 1..=4294967295"))], &[], bi);
    passes(&[(bo, Some("bound BO.n range 1..=100"))], &[]);
    passes(&[(bi, Some("bound BI.n range ..=4294967296"))], &[]);
    // a 128-bit integer's whole range, exactly
    passes(&[(bi, Some("bound BI.n range i128"))], &[]);
    fails(&[(bo, Some("bound BO.n range i128"))], &[], bo);
    fails(&[("bound BB.n range u32", Some("bound BB.n range i128"))], &[], "bound BB.n range u32");
    fails(&[("bound BB.n range u32", Some("bound BB.n range u16"))], &[], "bound BB.n range u32");
    fails(&[("bound BB.n range u32", Some("bound BB.n range u64"))], &[], "bound BB.n range u32");
    // lengths, list sizes and map sizes alike; none is 0..
    for (field, kind, was, narrower, wider) in [("s", "length", "0..=64", "0..=32", "0..=128"), ("l", "items", "0..=10", "1..=10", "0.."), ("m", "props", "0..=5", "0..=4", "0..=9")] {
        let line = |t: &str, v: &str| format!("bound {t}.{field} {kind} {v}");
        passes(&[(&line("BO", was), Some(&line("BO", narrower)))], &[]);
        fails(&[(&line("BO", was), Some(&line("BO", wider)))], &[], &line("BO", was));
        fails(&[(&line("BO", was), None)], &[], &line("BO", was));
        passes(&[(&line("BI", was), Some(&line("BI", wider)))], &[]);
        passes(&[(&line("BI", was), None)], &[]);
        fails(&[(&line("BI", was), Some(&line("BI", narrower)))], &[], &line("BI", was));
    }
    fails(&[], &["bound BI.t length 0..=8"], "bound BI.t length 0..=8");
    passes(&[], &["bound BO.t length 0..=8"]);
    // a pattern, a format, a multiple, uniqueness: none is the widest
    for (field, kind, was, other) in [("s", "pattern", " ^[a-z]+$", " ^[a-z0-9]+$"), ("s", "format", " date-time", " uuid"), ("k", "multiple", " 2", " 4"), ("l", "unique", "", "")] {
        let line = |t: &str, v: &str| format!("bound {t}.{field} {kind}{v}");
        fails(&[(&line("BO", was), None)], &[], &line("BO", was));
        passes(&[(&line("BI", was), None)], &[]);
        if !other.is_empty() {
            fails(&[(&line("BO", was), Some(&line("BO", other)))], &[], &line("BO", was));
            fails(&[(&line("BI", was), Some(&line("BI", other)))], &[], &line("BI", was));
        }
        let new = |t: &str| format!("bound {t}.t {kind}{was}");
        passes(&[], &[&new("BO")]);
        fails(&[], &[&new("BI")], &new("BI"));
    }
    // a bound of a field no older client knows
    passes(&[], &["field BI.new optional integer", "bound BI.new range i8", "bound BI.new default 1"]);
}

/// A default holds where a field may be left out, whoever writes it.
#[test]
fn a_default_holds_where_a_field_may_be_left_out() {
    fails(&[("bound BI.d default \"x\"", Some("bound BI.d default \"z\""))], &[], "bound BI.d default \"x\"");
    fails(&[("bound BO.d default \"x\"", Some("bound BO.d default \"z\""))], &[], "bound BO.d default \"x\"");
    fails(&[("bound BI.d default \"x\"", None)], &[], "bound BI.d default \"x\"");
    fails(&[], &["bound BI.t default \"q\""], "bound BI.t default \"q\"");
    // a field every older client sends
    passes(&[("bound BI.r default \"y\"", Some("bound BI.r default \"z\""))], &[]);
}

#[test]
fn null_is_wider() {
    fails(&[("field A.o optional string", Some("field A.o optional string|null"))], &[], "field A.o optional string");
    passes(&[("field B.o optional string", Some("field B.o optional string|null"))], &[]);
    passes(&[("field A.z optional string|null", Some("field A.z optional string"))], &[]);
    fails(&[("field B.z optional string|null", Some("field B.z optional string"))], &[], "field B.z optional string|null");
    fails(&[("field C.o optional string", Some("field C.o optional string|null"))], &[], "field C.o optional string");
}

#[test]
fn the_fields_beyond_those_named_and_flattened_branches_are_facts() {
    fails(&[("field O.* optional string", Some("field O.* optional integer"))], &[], "field O.* optional string");
    passes(&[], &["field A.* optional string"]);
    fails(&[], &["field K.* optional string"], "field K.* optional string");
    let extra = "field F.#0.extra required string";
    fails(&[(extra, Some("field F.#0.extra required integer"))], &[], extra);
    fails(&[("value F #1", None)], &[], "value F #1");
    fails(&[], &["value F #2", "type F.#2 out object"], "value F #2");
    passes(&[], &["value FI #1", "type FI.#1 in object"]);
    fails(&[("branches F anyOf", Some("branches F type"))], &[], "branches F anyOf");
}

/// An older reader of a closed shape refuses what it does not know until
/// its line is retired, whatever this change says of it.
#[test]
fn a_closed_reader_holds_until_retired() {
    let closed = "type K out object closed";
    fails(&[(closed, Some("type K out object"))], &[], closed);
    let found = check(&[(closed, Some("type K out object"))], &["field K.n optional string"], &[]);
    assert_eq!(found.iter().map(|b| b.fact.as_str()).collect::<Vec<_>>(), [closed, "field K.n optional string"], "{found:?}");
    assert!(check(&[(closed, Some("type K out object"))], &["field K.n optional string"], &[closed]).is_empty());
}

#[test]
fn a_retired_fact_is_waived_and_only_it() {
    let gone = "field A.o optional string";
    assert!(check(&[(gone, None)], &[], &[gone]).is_empty());
    let other = check(&[(gone, None), ("value EO one", None)], &[], &[gone]);
    assert_eq!(other.iter().map(|b| b.fact.as_str()).collect::<Vec<_>>(), ["value EO one"]);
}

#[test]
fn retirements_are_dated_facts_with_their_why() {
    let line = "retire 2026-12-15 field A.o optional string # gone from iOS before 0.5; floor ios=0.5 since 2026-12-01";
    let facts = retired(&format!("{RETIRED_HEADER}\n{line}\n")).unwrap();
    assert_eq!(facts.into_iter().collect::<Vec<_>>(), ["field A.o optional string"]);
    for bad in [
        "retire 2026-12-15 field A.o optional string",
        "retire 2026-12-15 field A.o optional string # ",
        "retire 12-15-2026 field A.o optional string # why",
        "retire 2026-12-15 # why",
        "retire 2026-12-15 fields A.o optional string # why",
        "remove 2026-12-15 field A.o optional string # why",
    ] {
        assert!(retired(bad).is_err(), "{bad}");
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fragment-wire-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_files_are_what_proto_makes_written_again_unchanged_and_read_back_whole() {
    let generated = generate().unwrap();
    // replay: the same proto makes the same text
    let again = generate().unwrap();
    assert!(generated.openapi == again.openapi && generated.lock == again.lock && generated.reference == again.reference);
    let root = scratch("files");
    let wrote = write_files(&root, &generated).unwrap();
    assert_eq!(wrote.len(), 4, "{wrote:?}");
    assert!(write_files(&root, &generated).unwrap().is_empty(), "a second write writes nothing");
    // restart: what was written reads back as what proto makes, master's or not
    let base = Base { lock: generated.lock.clone(), retired: RETIRED_HEADER.to_string(), at: "master".into() };
    verdict(&generated, &OnDisk::read(&root), Some(&base), true).unwrap();
    verdict(&generated, &OnDisk::read(&root), None, false).unwrap();
    let refused = verdict(&generated, &OnDisk::read(&root), None, true).unwrap_err().to_string();
    assert!(refused.contains("FRAGMENT_SCHEMA_BASE"), "{refused}");
    // a hand's edit, or a file gone, is refused
    std::fs::write(root.join(LOCK_PATH), generated.lock.replace("ListedFragment.title", "ListedFragment.titled")).unwrap();
    let refused = verdict(&generated, &OnDisk::read(&root), Some(&base), true).unwrap_err().to_string();
    assert!(refused.contains(LOCK_PATH) && refused.contains("cargo xtask wire"), "{refused}");
    std::fs::remove_file(root.join(LOCK_PATH)).unwrap();
    assert!(verdict(&generated, &OnDisk::read(&root), Some(&base), true).unwrap_err().to_string().contains("missing"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn master_holds_the_lock_and_its_retirements() {
    let generated = generate().unwrap();
    let root = scratch("master");
    write_files(&root, &generated).unwrap();
    let disk = OnDisk::read(&root);
    // master held a field this tree renamed: refused, naming it
    let title = "field ListedFragment.title optional string|null";
    assert!(generated.lock.contains(&format!("\n{title}\n")));
    let held = Base { lock: generated.lock.replace(title, "field ListedFragment.titled optional string|null"), retired: RETIRED_HEADER.into(), at: "master".into() };
    let refused = verdict(&generated, &disk, Some(&held), true).unwrap_err().to_string();
    assert!(refused.contains("field ListedFragment.titled optional string|null") && refused.contains("removed or renamed"), "{refused}");
    // retired on purpose, it passes
    let line = "retire 2026-11-06 field ListedFragment.titled optional string|null # a test's";
    std::fs::write(root.join(RETIRED_PATH), format!("{RETIRED_HEADER}{line}\n")).unwrap();
    verdict(&generated, &OnDisk::read(&root), Some(&held), true).unwrap();
    // master's retirements stay, as they were
    let master = Base { lock: generated.lock.clone(), retired: format!("{RETIRED_HEADER}{line}\n"), at: "master".into() };
    std::fs::write(root.join(RETIRED_PATH), RETIRED_HEADER).unwrap();
    let refused = verdict(&generated, &OnDisk::read(&root), Some(&master), true).unwrap_err().to_string();
    assert!(refused.contains("appended"), "{refused}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Every fact of a route the document names, every `$ref` resolves, each
/// operation is named once, and its only extensions are `x-fragment-*`.
#[test]
fn the_document_is_whole() {
    let generated = generate().unwrap();
    for fact in [
        "route fragments.list GET /api/fragments",
        "route fragments.list response FragmentList",
        "route live.open sends LiveIn",
        "route live.open receives LiveOut",
        "field ListedFragment.title optional string|null",
        "field FragmentStatus.viewToken required string|null",
        "type LiveOut out union(type) open",
        "type ErrorCode out enum open",
        "type Reason out enum open",
        "type PostRecord in object closed",
        "type Role out enum",
        "extends LiveOut.record ChannelRecord",
    ] {
        assert!(generated.lock.lines().any(|l| l == fact), "the lock holds {fact}");
    }
    let doc: Value = serde_json::from_str(&generated.openapi).unwrap();
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    let mut ids = BTreeSet::new();
    for item in doc["paths"].as_object().unwrap().values() {
        for op in item.as_object().unwrap().values() {
            assert!(ids.insert(op["operationId"].as_str().unwrap().to_string()), "{} twice", op["operationId"]);
        }
    }
    assert_eq!(ids.len(), fragment_proto::routes::ROUTES.len());
    fn walk(v: &Value, schemas: &serde_json::Map<String, Value>) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    if k.starts_with("x-") {
                        assert!(k.starts_with("x-fragment-"), "{k}: only x-fragment-* extensions");
                    }
                    if k == "$ref" {
                        let name = v.as_str().unwrap().strip_prefix(super::openapi::COMPONENTS).expect("a ref names a component");
                        assert!(schemas.contains_key(name), "{name} resolves");
                    }
                    walk(v, schemas);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, schemas)),
            _ => {}
        }
    }
    walk(&doc, schemas);
}

/// The lock of one type as a route answers it (`answers`) or takes it, as
/// `cargo xtask wire` makes it: schemars' schema, projected.
fn lock_of<T: schemars::JsonSchema>(answers: bool) -> Facts {
    let mut g = super::openapi::generator(answers);
    let root = g.subschema_for::<T>().to_value();
    let defs = g.take_definitions(true);
    let schemas = if answers { super::openapi::components(defs, Map::new()) } else { super::openapi::components(Map::new(), defs) }.unwrap();
    let mut op = if answers {
        json!({ "responses": { "200": { "content": { "application/json": { "schema": root } } } } })
    } else {
        json!({ "requestBody": { "content": { "application/json": { "schema": root } } } })
    };
    op["operationId"] = json!("t.call");
    parse(&project(&json!({ "paths": { "/t": { "post": op } }, "components": { "schemas": schemas } })).unwrap()).unwrap()
}

fn facts_of(found: &[Breach]) -> Vec<&str> {
    found.iter().map(|b| b.fact.as_str()).collect()
}

/// Each of these was a break the lock once missed, shown with types as
/// proto derives them: a flattened untagged union's branch, an integer's
/// width, a default, and a reader that closes a shape.
#[allow(dead_code)]
mod derived {
    use schemars::JsonSchema;
    use serde_json::Value;

    pub mod flat {
        use super::JsonSchema;
        #[derive(JsonSchema)]
        pub struct Flat {
            pub id: String,
            #[serde(flatten)]
            pub value: Union,
        }
        #[derive(JsonSchema)]
        #[serde(untagged)]
        pub enum Union {
            S { extra: String },
            B { extra: bool },
        }
    }
    pub mod flat_retyped {
        use super::JsonSchema;
        #[derive(JsonSchema)]
        pub struct Flat {
            pub id: String,
            #[serde(flatten)]
            pub value: Union,
        }
        #[derive(JsonSchema)]
        #[serde(untagged)]
        pub enum Union {
            S { extra: i64 },
            B { extra: bool },
        }
    }
    pub mod flat_fewer {
        use super::JsonSchema;
        #[derive(JsonSchema)]
        pub struct Flat {
            pub id: String,
            #[serde(flatten)]
            pub value: Union,
        }
        #[derive(JsonSchema)]
        #[serde(untagged)]
        pub enum Union {
            S { extra: String },
        }
    }
    macro_rules! sub {
        ($m:ident, $t:ty) => {
            pub mod $m {
                #[derive(schemars::JsonSchema)]
                pub struct Sub {
                    pub after: $t,
                }
            }
        };
    }
    sub!(i64s, i64);
    sub!(i32s, i32);
    sub!(u32s, u32);
    sub!(u64s, u64);
    pub mod cleared {
        use super::{JsonSchema, Value};
        #[derive(JsonSchema)]
        pub struct Presence {
            #[serde(default)]
            pub data: Value,
        }
    }
    pub mod kept {
        use super::{JsonSchema, Value};
        #[derive(JsonSchema)]
        pub struct Presence {
            #[serde(default = "kept")]
            pub data: Value,
        }
        fn kept() -> Value {
            serde_json::json!({ "kept": true })
        }
    }
    pub mod closed {
        use super::JsonSchema;
        #[derive(JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct Frame {
            pub id: String,
        }
    }
    pub mod opened {
        use super::JsonSchema;
        #[derive(JsonSchema)]
        pub struct Frame {
            pub id: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            #[schemars(extend("default" = null))]
            pub more: Option<String>,
        }
    }
}

#[test]
fn derived_types_break_where_an_older_client_would() {
    use derived::*;
    let none = BTreeSet::new();
    let (was, retyped, fewer) = (lock_of::<flat::Flat>(true), lock_of::<flat_retyped::Flat>(true), lock_of::<flat_fewer::Flat>(true));
    assert_eq!(facts_of(&breaches(&was, &retyped, &none)), ["field Flat.#0.extra required string"]);
    assert_eq!(facts_of(&breaches(&was, &fewer, &none)), ["value Flat #1"]);
    assert!(breaches(&was, &lock_of::<flat::Flat>(true), &none).is_empty());
    // an integer the cell reads, narrowed, and one a client reads, widened
    assert_eq!(facts_of(&breaches(&lock_of::<i64s::Sub>(false), &lock_of::<i32s::Sub>(false), &none)), ["bound Sub.after range i64"]);
    assert!(breaches(&lock_of::<i32s::Sub>(false), &lock_of::<i64s::Sub>(false), &none).is_empty());
    assert_eq!(facts_of(&breaches(&lock_of::<u32s::Sub>(true), &lock_of::<u64s::Sub>(true), &none)), ["bound Sub.after range u32"]);
    assert!(breaches(&lock_of::<u64s::Sub>(true), &lock_of::<u32s::Sub>(true), &none).is_empty());
    // what leaving `data` out asks for
    assert_eq!(facts_of(&breaches(&lock_of::<cleared::Presence>(false), &lock_of::<kept::Presence>(false), &none)), ["bound Presence.data default null"]);
    // a reader built closed meets a new field
    let found = breaches(&lock_of::<closed::Frame>(true), &lock_of::<opened::Frame>(true), &none);
    assert_eq!(facts_of(&found), ["type Frame out object closed", "field Frame.more optional string|null"]);
}

/// A keyword the lock does not know is refused, never dropped; so is an
/// integer as wide as its machine.
#[test]
fn what_the_lock_cannot_read_is_refused() {
    let doc = |schema: Value| json!({ "paths": {}, "components": { "schemas": { "T": schema } } });
    let refused = project(&doc(json!({ "x-fragment-direction": "out", "allOf": [{ "type": "string" }] }))).unwrap_err().to_string();
    assert!(refused.contains("`allOf`") && refused.contains("lock.rs"), "{refused}");
    let refused = project(&doc(json!({ "x-fragment-direction": "in", "type": "object", "properties": { "n": { "type": "integer", "format": "uint" } } })))
        .unwrap_err()
        .to_string();
    assert!(format!("{refused:#}").contains("T.n"), "{refused}");
    let refused = project(&doc(json!({ "x-fragment-direction": "in", "type": "object", "properties": { "n": { "type": "string", "not": { "const": "" } } } }))).unwrap_err().to_string();
    assert!(refused.contains("`not`"), "{refused}");
}

/// A shape written by reference rather than inline, holding the same, is
/// the same shape; one whose new field comes through the reference is not.
#[test]
fn shapes_compare_by_what_they_hold_not_how_they_are_written() {
    let none = BTreeSet::new();
    let base = parse("type S out object\nfield S.a required string\nbound S.n range u16\nfield S.n optional integer\n").unwrap();
    let by_ref = parse("type S out object\nextends S P\ntype P out object\nfield P.a required string\nfield P.n optional integer\nbound P.n range u16\n").unwrap();
    assert!(breaches(&base, &by_ref, &none).is_empty(), "{:?}", breaches(&base, &by_ref, &none));
    let wider = parse("type S out object\nextends S P\ntype P out object\nfield P.a required string\nfield P.n optional integer\nbound P.n range u32\n").unwrap();
    assert_eq!(facts_of(&breaches(&base, &wider, &none)), ["bound S.n range u16"]);
    let closed = parse("type R out object closed\nfield R.id required string\n").unwrap();
    let more = parse("type R out object\nextends R Q\ntype Q out object\nfield Q.id required string\nfield Q.more optional string\n").unwrap();
    assert_eq!(facts_of(&breaches(&closed, &more, &none)), ["type R out object closed", "field R.more optional string"]);
}

#[allow(dead_code)]
mod derived_inline {
    pub mod plain {
        #[derive(schemars::JsonSchema)]
        #[schemars(inline)]
        pub struct Inner {
            pub v: String,
        }
        #[derive(schemars::JsonSchema)]
        pub struct Wrap {
            pub inner: Inner,
        }
    }
    pub mod nullable {
        #[derive(schemars::JsonSchema)]
        #[schemars(inline)]
        pub struct Inner {
            pub v: String,
        }
        #[derive(schemars::JsonSchema)]
        pub struct Wrap {
            #[schemars(extend("default" = null))]
            pub inner: Option<Inner>,
        }
    }
    /// A documented variant beside one that is not.
    pub mod narrowed {
        #[derive(schemars::JsonSchema)]
        pub enum E {
            /// The first.
            #[schemars(extend("not" = { "const": "A" }))]
            A,
            B,
        }
    }
}

/// An inline shape that may be null now is wider; an enum's branch that
/// says more than its value is read as a union's, so a keyword the lock
/// cannot read is refused there too.
#[test]
fn inline_nullability_and_enum_branches_are_read() {
    use derived_inline::*;
    let none = BTreeSet::new();
    let (plain_out, nullable_out) = (lock_of::<plain::Wrap>(true), lock_of::<nullable::Wrap>(true));
    assert_eq!(facts_of(&breaches(&plain_out, &nullable_out, &none)), ["field Wrap.inner required Wrap.inner"]);
    assert!(breaches(&lock_of::<plain::Wrap>(false), &lock_of::<nullable::Wrap>(false), &none).is_empty());
    let mut g = super::openapi::generator(true);
    let root = g.subschema_for::<narrowed::E>().to_value();
    let schemas = super::openapi::components(g.take_definitions(true), Map::new()).unwrap();
    let doc = json!({ "paths": { "/t": { "get": { "operationId": "t.get", "responses": { "200": { "content": { "application/json": { "schema": root } } } } } } }, "components": { "schemas": schemas } });
    let refused = project(&doc).unwrap_err().to_string();
    assert!(refused.contains("`not`"), "{refused}");
    // a bound on a branch is the branch's fact
    let bounded = json!({ "paths": {}, "components": { "schemas": { "T": { "x-fragment-direction": "out", "oneOf": [{ "const": "a", "type": "string", "maxLength": 1 }, { "const": "bb", "type": "string" }] } } } });
    let lock = project(&bounded).unwrap();
    assert!(lock.lines().any(|l| l == "bound T.#0 length 0..=1"), "{lock}");
}

#[allow(dead_code)]
mod derived_absence {
    /// A row on the dock until it is taken off: left out while true.
    pub mod on_unless_off {
        #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
        pub struct Row {
            #[serde(default = "yes", skip_serializing_if = "is_yes")]
            #[schemars(extend("default" = true))]
            pub dock: bool,
        }
        fn yes() -> bool {
            true
        }
        fn is_yes(b: &bool) -> bool {
            *b
        }
    }
    /// The same field, off until it is put on: left out while false.
    pub mod off_unless_on {
        #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
        pub struct Row {
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            #[schemars(extend("default" = false))]
            pub dock: bool,
        }
    }
    /// It, saying nothing of what leaving it out means.
    pub mod unsaid {
        #[derive(schemars::JsonSchema)]
        pub struct Row {
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            pub dock: bool,
        }
    }
    pub mod wide {
        #[derive(schemars::JsonSchema)]
        pub struct Big {
            pub n: u128,
        }
    }
}

/// What leaving a field out means is a fact: an older reader fills it in
/// with the default it was built with. One the schema cannot see is
/// refused; so is a u128, whose range the lock cannot compare exactly.
#[test]
fn absence_is_said_and_held() {
    use derived_absence::*;
    // the change a reader feels: off is written as nothing, and read as on
    let written = serde_json::to_value(off_unless_on::Row { dock: false }).unwrap();
    assert_eq!(written, json!({}));
    assert!(serde_json::from_value::<on_unless_off::Row>(written).unwrap().dock);
    let none = BTreeSet::new();
    let found = breaches(&lock_of::<on_unless_off::Row>(true), &lock_of::<off_unless_on::Row>(true), &none);
    assert_eq!(facts_of(&found), ["bound Row.dock default true"]);
    let refuse = |doc: Value| project(&doc).unwrap_err().to_string();
    let mut g = super::openapi::generator(true);
    let root = g.subschema_for::<unsaid::Row>().to_value();
    let schemas = super::openapi::components(g.take_definitions(true), Map::new()).unwrap();
    let op = json!({ "operationId": "t.get", "responses": { "200": { "content": { "application/json": { "schema": root } } } } });
    let refused = refuse(json!({ "paths": { "/t": { "get": op } }, "components": { "schemas": schemas } }));
    assert!(refused.contains("Row.dock") && refused.contains("leaving it out"), "{refused}");
    let mut g = super::openapi::generator(false);
    g.subschema_for::<wide::Big>();
    let schemas = super::openapi::components(Map::new(), g.take_definitions(true)).unwrap();
    let refused = refuse(json!({ "paths": {}, "components": { "schemas": schemas } }));
    assert!(format!("{refused:#}").contains("Big.n"), "{refused}");
}

/// A union's tag is a field of its branch, read as any other: a keyword
/// the lock cannot read is refused there too.
#[test]
fn a_tag_is_read_as_a_field() {
    let tagged = |tag: Value| {
        json!({ "paths": {}, "components": { "schemas": { "T": { "x-fragment-direction": "in", "oneOf": [
            { "type": "object", "properties": { "type": tag }, "required": ["type"] },
            { "type": "object", "properties": { "type": { "type": "string", "const": "pong" } }, "required": ["type"] },
        ] } } } })
    };
    let lock = project(&tagged(json!({ "type": "string", "const": "ping" }))).unwrap();
    assert!(lock.lines().any(|l| l == "field T.ping.type required \"ping\""), "{lock}");
    let refused = project(&tagged(json!({ "type": "string", "const": "ping", "not": { "const": "ping" } }))).unwrap_err().to_string();
    assert!(refused.contains("`not`"), "{refused}");
}

/// A type's document as a route answers it (`answers`) or takes it, and
/// its root schema.
fn doc_of<T: schemars::JsonSchema>(answers: bool) -> (Value, Value) {
    let mut g = super::openapi::generator(answers);
    let root = g.subschema_for::<T>().to_value();
    let defs = g.take_definitions(true);
    let schemas = if answers { super::openapi::components(defs, Map::new()) } else { super::openapi::components(Map::new(), defs) }.unwrap();
    let op = json!({ "operationId": "t.call", "responses": { "200": { "content": { "application/json": { "schema": root } } } } });
    (json!({ "paths": { "/t": { "post": op } }, "components": { "schemas": schemas } }), root)
}

/// That `T`'s stated defaults are what serde reads of their fields left
/// out: the lock's fields it held.
fn defaults_hold<T: schemars::JsonSchema + serde::Serialize + serde::de::DeserializeOwned>() -> anyhow::Result<BTreeSet<String>> {
    let (doc, root) = doc_of::<T>(true);
    super::defaults::check("t", &root, doc["components"]["schemas"].as_object().unwrap(), fragment_proto::routes::round_trip::<T>)
}

#[allow(dead_code)]
mod derived_hidden {
    /// Absent is `Some(true)`, and written as nothing.
    pub mod on {
        #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
        pub struct Row {
            #[serde(default = "on", skip_serializing_if = "is_on")]
            #[schemars(extend("default" = true))]
            pub flag: Option<bool>,
        }
        fn on() -> Option<bool> {
            Some(true)
        }
        fn is_on(v: &Option<bool>) -> bool {
            *v == Some(true)
        }
    }
    /// Absent is `Some(false)` now.
    pub mod off {
        #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
        pub struct Row {
            #[serde(default = "off", skip_serializing_if = "is_off")]
            #[schemars(extend("default" = false))]
            pub flag: Option<bool>,
        }
        fn off() -> Option<bool> {
            Some(false)
        }
        fn is_off(v: &Option<bool>) -> bool {
            *v == Some(false)
        }
    }
    /// Its absence unsaid: a nullable field's need not be null.
    pub mod unsaid {
        #[derive(schemars::JsonSchema)]
        pub struct Row {
            #[serde(default = "on", skip_serializing_if = "is_on")]
            pub flag: Option<bool>,
        }
        fn on() -> Option<bool> {
            Some(true)
        }
        fn is_on(v: &Option<bool>) -> bool {
            *v == Some(true)
        }
    }
    /// Said wrong.
    pub mod lying {
        #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
        pub struct Row {
            #[serde(default = "on", skip_serializing_if = "is_on")]
            #[schemars(extend("default" = null))]
            pub flag: Option<bool>,
        }
        fn on() -> Option<bool> {
            Some(true)
        }
        fn is_on(v: &Option<bool>) -> bool {
            *v == Some(true)
        }
    }
    macro_rules! flattened {
        ($m:ident, $default:literal, $fn:literal, $value:literal) => {
            pub mod $m {
                /// A field of a flattened union's branch that its wrapper does
                /// not describe: data, not a condition on the wrapper's.
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(untagged)]
                pub enum Choice {
                    Docked {
                        #[serde(default = $fn, skip_serializing_if = "is_it")]
                        #[schemars(extend("default" = $default))]
                        dock: bool,
                        kind: String,
                    },
                    Other {
                        other: u8,
                    },
                }
                fn yes() -> bool {
                    true
                }
                fn no() -> bool {
                    false
                }
                fn is_it(b: &bool) -> bool {
                    *b == $value
                }
            }
        };
    }
    flattened!(flat_on, true, "yes", true);
    flattened!(flat_off, false, "no", false);
    flattened!(flat_lying, false, "yes", true);
}

/// The two ways a hidden default once passed: a nullable field whose
/// absence is not null, and a field of a flattened union's branch. Each is
/// refused unsaid, held to serde said, and a change of it fails.
#[test]
fn hidden_defaults_are_said_held_and_locked() {
    use derived_hidden::*;
    let none = BTreeSet::new();
    // the change a reader feels: Some(false) written as nothing, read as Some(true)
    let written = serde_json::to_value(off::Row { flag: Some(false) }).unwrap();
    assert_eq!(written, json!({}));
    assert_eq!(serde_json::from_value::<on::Row>(written).unwrap().flag, Some(true));
    assert_eq!(facts_of(&breaches(&lock_of::<on::Row>(true), &lock_of::<off::Row>(true), &none)), ["bound Row.flag default true"]);
    let refused = project(&doc_of::<unsaid::Row>(true).0).unwrap_err().to_string();
    assert!(refused.contains("Row.flag") && refused.contains("leaving it out"), "{refused}");
    assert_eq!(defaults_hold::<on::Row>().unwrap().into_iter().collect::<Vec<_>>(), ["Row.flag"]);
    let lie = defaults_hold::<lying::Row>().unwrap_err().to_string();
    assert!(lie.contains("`flag` left out is not its stated default null"), "{lie}");
    // a flattened branch's own field
    let written = serde_json::to_value(flat_off::Wrap { id: "a".into(), value: flat_off::Choice::Docked { dock: false, kind: "k".into() } }).unwrap();
    let read: flat_on::Wrap = serde_json::from_value(written).unwrap();
    assert!(matches!(read.value, flat_on::Choice::Docked { dock: true, .. }), "an older reader reads it docked");
    assert_eq!(facts_of(&breaches(&lock_of::<flat_on::Wrap>(true), &lock_of::<flat_off::Wrap>(true), &none)), ["bound Wrap.#0.dock default true"]);
    assert_eq!(defaults_hold::<flat_on::Wrap>().unwrap().into_iter().collect::<Vec<_>>(), ["Wrap.#0.dock"]);
    let lie = defaults_hold::<flat_lying::Wrap>().unwrap_err().to_string();
    assert!(lie.contains("`#0.dock` left out is not its stated default false"), "{lie}");
}

/// The wire's own defaults hold, and every optional field's default the
/// lock records is reached from a route, so none goes unread.
#[test]
fn the_wires_defaults_hold_every_one() {
    let doc = super::openapi::document().unwrap();
    let lock = project(&doc).unwrap();
    let held = super::defaults::hold(&doc, &lock).unwrap();
    for field in ["ListedFragment.dock", "ListedFragment.unread", "OpDecl.ephemeral", "PageError.source", "ErrorBody.reason", "LiveIn.presence.data", "LiveOut.draft.thinking"] {
        assert!(held.contains(field), "{field} held: {held:?}");
    }
    // one no sample reaches is named
    let ops = || ["a.get"].into_iter();
    let lock = "field X.f optional boolean\nbound X.f default true\nfield a.get.query.q optional string\nbound a.get.query.q default \"x\"\n";
    assert_eq!(super::defaults::unheld(lock, &BTreeSet::new(), ops()), ["X.f"]);
    assert!(super::defaults::unheld(lock, &BTreeSet::from(["X.f".to_string()]), ops()).is_empty());
}

#[allow(dead_code)]
mod derived_nested {
    macro_rules! nested {
        ($m:ident, $flag:literal) => {
            pub mod $m {
                /// A nested struct inside a flattened untagged branch, its
                /// stated default `true` whatever serde does.
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(untagged)]
                pub enum Choice {
                    Row { inner: Inner },
                    Other { other: u8 },
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Inner {
                    #[serde(default = "flag", skip_serializing_if = "skip_flag")]
                    #[schemars(extend("default" = true))]
                    pub dock: bool,
                }
                fn flag() -> bool {
                    $flag
                }
                fn skip_flag(b: &bool) -> bool {
                    *b == $flag
                }
            }
        };
    }
    nested!(said, true);
    nested!(stale, false);
}

/// A stale default inside a nested struct of a flattened branch: the lock
/// cannot see it (the annotation did not change), so serde's reading of
/// it is what fails, naming the nested field.
#[test]
fn a_nested_default_in_a_flattened_branch_is_held() {
    use derived_nested::*;
    let written = serde_json::to_value(stale::Wrap { id: "x".into(), value: stale::Choice::Row { inner: stale::Inner { dock: false } } }).unwrap();
    assert_eq!(written, json!({ "id": "x", "inner": {} }));
    assert!(matches!(serde_json::from_value::<said::Wrap>(written).unwrap().value, said::Choice::Row { inner: said::Inner { dock: true } }));
    assert!(breaches(&lock_of::<said::Wrap>(true), &lock_of::<stale::Wrap>(true), &BTreeSet::new()).is_empty(), "the lock is the same");
    assert_eq!(defaults_hold::<said::Wrap>().unwrap().into_iter().collect::<Vec<_>>(), ["Inner.dock"]);
    let stale = defaults_hold::<stale::Wrap>().unwrap_err().to_string();
    assert!(stale.contains("`#0.inner.dock` left out is not its stated default true"), "{stale}");
}

#[allow(dead_code)]
mod derived_reach {
    /// A list of children inside a flattened tagged variant.
    macro_rules! listed {
        ($m:ident, $flag:literal) => {
            pub mod $m {
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(tag = "kind")]
                pub enum Choice {
                    Row { children: Vec<Child> },
                    Other { other: u8 },
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Child {
                    #[serde(default = "flag", skip_serializing_if = "skip_flag")]
                    #[schemars(extend("default" = true))]
                    pub dock: bool,
                }
                fn flag() -> bool {
                    $flag
                }
                fn skip_flag(b: &bool) -> bool {
                    *b == $flag
                }
            }
        };
    }
    listed!(listed_said, true);
    listed!(listed_stale, false);

    /// A field one branch requires and the other lets be left out.
    macro_rules! either {
        ($m:ident, $flag:literal, $($said:tt)*) => {
            pub mod $m {
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[schemars(extend("anyOf" = [{ "required": ["dock"] }, { "required": ["id"] }]))]
                pub struct Wrap {
                    pub id: String,
                    #[serde(default = "flag", skip_serializing_if = "skip_flag")]
                    $($said)*
                    pub dock: bool,
                }
                fn flag() -> bool {
                    $flag
                }
                fn skip_flag(b: &bool) -> bool {
                    *b == $flag
                }
            }
        };
    }
    either!(either_unsaid, true,);
    either!(either_stale, false, #[schemars(extend("default" = true))]);

    #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
    #[serde(bound = "T: serde::Serialize + serde::de::DeserializeOwned")]
    pub struct Layer<T> {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("default" = null))]
        pub next: Option<Box<T>>,
    }
    #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
    pub struct Leaf {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[schemars(extend("default" = true))]
        pub dock: bool,
    }
    pub type Ten = Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Leaf>>>>>>>>>>;
    pub type TwentyFive = Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Leaf>>>>>>>>>>>>>>>>>>>>>>>>>;

    /// A recursive type: its walk stops where it meets itself.
    #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
    pub struct Tree {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("default" = null))]
        pub child: Option<Box<Tree>>,
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        #[schemars(extend("default" = true))]
        pub flag: bool,
    }
    fn yes() -> bool {
        true
    }
    fn is_yes(b: &bool) -> bool {
        *b
    }
}

/// Every default a type reaches is held, however it is reached: through a
/// list in a flattened tagged branch, where one branch lets its field be
/// left out though another requires it, ten layers down, or in a type
/// that holds itself. A walk deeper than the checker reads is an error,
/// never a pass.
#[test]
fn every_reachable_default_is_held() {
    use derived_reach::*;
    let stale = defaults_hold::<listed_stale::Wrap>().unwrap_err().to_string();
    assert!(stale.contains("`Row.children.[].dock` left out is not its stated default true"), "{stale}");
    assert_eq!(defaults_hold::<listed_said::Wrap>().unwrap().into_iter().collect::<Vec<_>>(), ["Child.dock"]);
    let refused = project(&doc_of::<either_unsaid::Wrap>(true).0).unwrap_err().to_string();
    assert!(refused.contains("Wrap.dock") && refused.contains("leaving it out"), "{refused}");
    let stale = defaults_hold::<either_stale::Wrap>().unwrap_err().to_string();
    assert!(stale.contains("`dock` left out is not its stated default true"), "{stale}");
    let deep = defaults_hold::<Ten>().unwrap_err().to_string();
    assert!(deep.contains("dock` left out is not its stated default true"), "{deep}");
    let held = defaults_hold::<Tree>().unwrap();
    assert_eq!(held.into_iter().collect::<Vec<_>>(), ["Tree.child", "Tree.flag"]);
    let too_deep = defaults_hold::<TwentyFive>().unwrap_err().to_string();
    assert!(too_deep.contains("deeper than the default checker reads"), "{too_deep}");
}

#[allow(dead_code)]
mod derived_conditions {
    /// A field one branch requires and the other forbids, though serde
    /// reads it in both: left out there, its reader fills in its default.
    macro_rules! forbidding {
        ($m:ident, $flag:literal) => {
            pub mod $m {
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[schemars(extend("anyOf" = [{ "required": ["dock"] }, { "properties": { "dock": false } }]))]
                pub struct Wrap {
                    pub id: String,
                    #[serde(default = "flag", skip_serializing_if = "skip_flag")]
                    pub dock: bool,
                }
                fn flag() -> bool {
                    $flag
                }
                fn skip_flag(b: &bool) -> bool {
                    *b == $flag
                }
            }
        };
    }
    forbidding!(forbid_said, true);
    forbidding!(forbid_stale, false);

    /// A default its object states once, while its branches tell instances
    /// apart by another field: each variant reads it its own way.
    macro_rules! shared {
        ($m:ident, $b:literal) => {
            pub mod $m {
                #[derive(serde::Serialize, serde::Deserialize)]
                #[serde(tag = "kind")]
                pub enum SharedDefault {
                    #[serde(rename = "a")]
                    A {
                        #[serde(default = "yes", skip_serializing_if = "is_yes")]
                        dock: bool,
                    },
                    #[serde(rename = "b")]
                    B {
                        #[serde(default = "b", skip_serializing_if = "is_b")]
                        dock: bool,
                    },
                }
                fn yes() -> bool {
                    true
                }
                fn is_yes(d: &bool) -> bool {
                    *d
                }
                fn b() -> bool {
                    $b
                }
                fn is_b(d: &bool) -> bool {
                    *d == $b
                }
                impl schemars::JsonSchema for SharedDefault {
                    fn schema_name() -> std::borrow::Cow<'static, str> {
                        "SharedDefault".into()
                    }
                    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                        schemars::json_schema!({
                            "type": "object",
                            "properties": { "kind": { "type": "string" }, "dock": { "type": "boolean", "default": true } },
                            "required": ["kind"],
                            "oneOf": [{ "properties": { "kind": { "const": "a" } } }, { "properties": { "kind": { "const": "b" } } }],
                        })
                    }
                }
            }
        };
    }
    shared!(shared_said, true);
    shared!(shared_stale, false);
}

/// The two ways a condition hid a default: a branch forbidding a field
/// serde reads there anyway, and a default stated once beside branches
/// that tell instances apart by another field. The lock is the same either
/// way; serde's reading in each branch is what fails, or refuses.
#[test]
fn conditions_hide_no_default() {
    use derived_conditions::*;
    let none = BTreeSet::new();
    // a forbidding branch serde does not refuse: refused, said or stale
    let written = serde_json::to_value(forbid_stale::Wrap { id: "q".into(), dock: false }).unwrap();
    assert_eq!(written, json!({ "id": "q" }));
    assert!(serde_json::from_value::<forbid_said::Wrap>(written).unwrap().dock, "an older reader fills it in");
    assert!(breaches(&lock_of::<forbid_said::Wrap>(true), &lock_of::<forbid_stale::Wrap>(true), &none).is_empty(), "the lock is the same");
    for refused in [defaults_hold::<forbid_said::Wrap>().unwrap_err().to_string(), defaults_hold::<forbid_stale::Wrap>().unwrap_err().to_string()] {
        assert!(refused.contains("forbids `#1.dock`") && refused.contains("serde reads it there"), "{refused}");
    }
    // a shared default, read in every branch
    let written = serde_json::to_value(shared_stale::SharedDefault::B { dock: false }).unwrap();
    assert_eq!(written, json!({ "kind": "b" }));
    assert!(matches!(serde_json::from_value::<shared_said::SharedDefault>(written).unwrap(), shared_said::SharedDefault::B { dock: true }));
    assert_eq!(defaults_hold::<shared_said::SharedDefault>().unwrap().into_iter().collect::<Vec<_>>(), ["SharedDefault.dock"]);
    let stale = defaults_hold::<shared_stale::SharedDefault>().unwrap_err().to_string();
    assert!(stale.contains("`dock` left out is not its stated default true") && stale.contains("\"kind\":\"b\""), "{stale}");
    // `false` anywhere but a condition's forbidding of a described field
    let doc = json!({ "paths": {}, "components": { "schemas": { "T": { "x-fragment-direction": "out", "type": "object", "properties": { "x": false } } } } });
    let refused = project(&doc).unwrap_err().to_string();
    assert!(refused.contains("T.x") && refused.contains("admits no value"), "{refused}");
    // a forbidding no sample reaches is named
    let lock = "type X out object\nfield X.#1.f optional never\n";
    assert_eq!(super::defaults::unheld(lock, &BTreeSet::new(), [].into_iter()), ["X.#1.f"]);
}

#[allow(dead_code)]
mod derived_baseline {
    /// A name serde refuses under two characters: the least sample of a
    /// string (`"x"`) does not read, unless its schema gives an example.
    macro_rules! validated {
        ($m:ident, $($example:tt)*) => {
            pub mod $m {
                #[derive(Clone, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(try_from = "String", into = "String")]
                $($example)*
                pub struct Name(String);
                impl TryFrom<String> for Name {
                    type Error = String;
                    fn try_from(s: String) -> Result<Name, String> {
                        if s.len() < 2 { Err(format!("{s:?} is too short")) } else { Ok(Name(s)) }
                    }
                }
                impl From<Name> for String {
                    fn from(n: Name) -> String {
                        n.0
                    }
                }
                #[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[schemars(extend("anyOf" = [{ "required": ["dock"] }, { "properties": { "dock": false } }]))]
                pub struct ValidatedForbidden {
                    pub id: Name,
                    #[serde(default = "yes", skip_serializing_if = "is_yes")]
                    pub dock: bool,
                }
                fn yes() -> bool {
                    true
                }
                fn is_yes(b: &bool) -> bool {
                    *b
                }
            }
        };
    }
    validated!(unread,);
    validated!(read, #[schemars(example = &"ok")]);
}

/// A forbidding branch's refusal counts only beside a sample of the
/// branch that reads: one serde refuses for another reason (a name too
/// short) is refused, naming the branch; with an example that reads, the
/// field serde reads there is refused as before.
#[test]
fn a_forbidding_needs_a_sample_that_reads() {
    use derived_baseline::*;
    let written = serde_json::to_value(read::ValidatedForbidden { id: serde_json::from_value(json!("ok")).unwrap(), dock: false }).unwrap();
    assert!(serde_json::from_value::<read::ValidatedForbidden>(written).is_ok(), "dock is no forbidden field: serde reads it");
    let unread = defaults_hold::<unread::ValidatedForbidden>().unwrap_err().to_string();
    assert!(unread.contains("a sample of the branch that forbids `#1.dock` does not read") && unread.contains("examples"), "{unread}");
    let refused = defaults_hold::<read::ValidatedForbidden>().unwrap_err().to_string();
    assert!(refused.contains("forbids `#1.dock`") && refused.contains("serde reads it there"), "{refused}");
}
