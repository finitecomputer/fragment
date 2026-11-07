//! The wire lock's rules, cell by cell (lock.rs's table), its retirements,
//! and the files: valid, invalid, replay and restart.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use super::defaults::Controls;
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
    let renamed = generated.lock.replace(title, "field ListedFragment.titled optional string|null").replace("bound ListedFragment.title default null\n", "");
    let held = Base { lock: renamed, retired: RETIRED_HEADER.into(), at: "master".into() };
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
        #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
        #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
fn defaults_hold<T: schemars::JsonSchema + serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug>() -> anyhow::Result<BTreeSet<String>> {
    held_with::<T>(Controls::ALL)
}

/// `defaults_hold`, with the checker's controls given.
fn held_with<T: schemars::JsonSchema + serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug>(controls: Controls) -> anyhow::Result<BTreeSet<String>> {
    let (doc, root) = doc_of::<T>(true);
    super::defaults::check_with("t", &root, doc["components"]["schemas"].as_object().unwrap(), fragment_proto::routes::round_trip::<T>, controls)
}

#[allow(dead_code)]
mod derived_hidden {
    /// Absent is `Some(true)`, and written as nothing.
    pub mod on {
        #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
        #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
        #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(tag = "t")]
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
    assert_eq!(facts_of(&breaches(&lock_of::<flat_on::Wrap>(true), &lock_of::<flat_off::Wrap>(true), &none)), ["bound Wrap.Docked.dock default true"]);
    assert_eq!(defaults_hold::<flat_on::Wrap>().unwrap().into_iter().collect::<Vec<_>>(), ["Wrap.Docked.dock"]);
    let lie = defaults_hold::<flat_lying::Wrap>().unwrap_err().to_string();
    assert!(lie.contains("`Docked.dock` left out is not its stated default false"), "{lie}");
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
    // one no sample reaches is named, as a parameter's default is: no type reads it
    let lock = "field X.f optional boolean\nbound X.f default true\nfield a.get.query.q optional string\nbound a.get.query.q default \"x\"\n";
    assert_eq!(super::defaults::unheld(lock, &BTreeSet::new()), ["X.f", "a.get.query.q"]);
    assert_eq!(super::defaults::unheld(lock, &BTreeSet::from(["X.f".to_string()])), ["a.get.query.q"]);
}

#[allow(dead_code)]
mod derived_nested {
    macro_rules! nested {
        ($m:ident, $flag:literal) => {
            pub mod $m {
                /// A nested struct inside a flattened tagged branch, its
                /// stated default `true` whatever serde does.
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(tag = "t")]
                pub enum Choice {
                    Row { inner: Inner },
                    Other { other: u8 },
                }
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
    assert_eq!(written, json!({ "id": "x", "t": "Row", "inner": {} }));
    assert!(matches!(serde_json::from_value::<said::Wrap>(written).unwrap().value, said::Choice::Row { inner: said::Inner { dock: true } }));
    assert!(breaches(&lock_of::<said::Wrap>(true), &lock_of::<stale::Wrap>(true), &BTreeSet::new()).is_empty(), "the lock is the same");
    assert_eq!(defaults_hold::<said::Wrap>().unwrap().into_iter().collect::<Vec<_>>(), ["Inner.dock"]);
    let stale = defaults_hold::<stale::Wrap>().unwrap_err().to_string();
    assert!(stale.contains("`Row.inner.dock` left out is not its stated default true"), "{stale}");
}

#[allow(dead_code)]
mod derived_reach {
    /// A list of children inside a flattened tagged variant.
    macro_rules! listed {
        ($m:ident, $flag:literal) => {
            pub mod $m {
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                pub struct Wrap {
                    pub id: String,
                    #[serde(flatten)]
                    pub value: Choice,
                }
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
                #[serde(tag = "kind")]
                pub enum Choice {
                    Row { children: Vec<Child> },
                    Other { other: u8 },
                }
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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

    #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
    #[serde(bound = "T: serde::Serialize + serde::de::DeserializeOwned")]
    pub struct Layer<T> {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(extend("default" = null))]
        pub next: Option<Box<T>>,
    }
    #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
    pub struct Leaf {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[schemars(extend("default" = true))]
        pub dock: bool,
    }
    pub type Ten = Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Leaf>>>>>>>>>>;
    pub type TwentyFive = Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Leaf>>>>>>>>>>>>>>>>>>>>>>>>>;

    /// A recursive type: its walk stops where it meets itself.
    #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
                #[derive(Debug, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
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
                #[derive(Debug, serde::Serialize, serde::Deserialize)]
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
        assert!(refused.contains("Wrap.#1.dock") && refused.contains("FORBIDDINGS' sites alone"), "{refused}");
    }
    // a shared default, read in every branch
    let written = serde_json::to_value(shared_stale::SharedDefault::B { dock: false }).unwrap();
    assert_eq!(written, json!({ "kind": "b" }));
    assert!(matches!(serde_json::from_value::<shared_said::SharedDefault>(written).unwrap(), shared_said::SharedDefault::B { dock: true }));
    assert_eq!(defaults_hold::<shared_said::SharedDefault>().unwrap().into_iter().collect::<Vec<_>>(), ["SharedDefault.dock"]);
    let stale = defaults_hold::<shared_stale::SharedDefault>().unwrap_err().to_string();
    assert!(stale.contains("`dock` left out is not its stated default true") && stale.contains("\"kind\":\"b\"") && stale.contains("B { dock: false }"), "{stale}");
    // `false` anywhere but a condition's forbidding of a described field
    let doc = json!({ "paths": {}, "components": { "schemas": { "T": { "x-fragment-direction": "out", "type": "object", "properties": { "x": false } } } } });
    let refused = project(&doc).unwrap_err().to_string();
    assert!(refused.contains("T.x") && refused.contains("admits no value"), "{refused}");
    // a forbidding no sample reaches is named
    let lock = "type X out object\nfield X.#1.f optional never\n";
    assert_eq!(super::defaults::unheld(lock, &BTreeSet::new()), ["X.#1.f"]);
}

/// The checker's controls (defaults.rs's table), each with a fixture whose
/// claim is false and that only it refuses.
#[allow(dead_code)]
mod derived_controls {
    use std::borrow::Cow;

    use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    use super::super::fixtures::mode::Mode;

    /// A name serde refuses under two characters, and its schema's example.
    macro_rules! name {
        ($name:ident, $($example:tt)*) => {
            #[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
            #[serde(try_from = "String", into = "String")]
            $($example)*
            pub struct $name(String);
            impl TryFrom<String> for $name {
                type Error = String;
                fn try_from(s: String) -> Result<$name, String> {
                    if s.len() < 2 { Err(format!("{s:?} is too short")) } else { Ok($name(s)) }
                }
            }
            impl From<$name> for String {
                fn from(n: $name) -> String {
                    n.0
                }
            }
            impl $name {
                pub fn anon() -> $name {
                    $name("anon".into())
                }
                pub fn is_anon(&self) -> bool {
                    self.0 == "anon"
                }
            }
        };
    }
    name!(Name,);
    name!(NameOk, #[schemars(example = &"ok")]);

    fn no() -> bool {
        false
    }
    fn is_no(b: &bool) -> bool {
        !*b
    }
    fn red() -> Mode {
        Mode::Red
    }
    fn is_red(m: &Mode) -> bool {
        *m == Mode::Red
    }

    /// D1: a default stated wrong, beside a name no sample of which reads.
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
    pub struct Unread {
        pub id: Name,
        #[serde(default = "no", skip_serializing_if = "is_no")]
        #[schemars(extend("default" = true))]
        pub dock: bool,
    }

    /// D2: a default stated wrong that its writer leaves out either way.
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
    pub struct Hidden {
        #[serde(default = "seven", skip_serializing_if = "small")]
        #[schemars(extend("default" = 0))]
        pub n: u32,
    }
    fn seven() -> u32 {
        7
    }
    fn small(n: &u32) -> bool {
        *n < 10
    }

    /// D3: a field written but never read: no reading of it is evidence.
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
    pub struct Ignored {
        #[serde(skip_deserializing, skip_serializing_if = "zero")]
        #[schemars(extend("default" = 5))]
        pub n: u32,
    }
    fn zero(n: &u32) -> bool {
        *n == 0
    }

    /// The schema of a union tagged by `kind`: branch `a` requires `field`,
    /// branch `b` forbids it.
    fn a_requires_b_forbids(field: &str, schema: Value) -> Schema {
        json_schema!({
            "type": "object",
            "properties": { "kind": { "type": "string" }, field: schema },
            "required": ["kind"],
            "oneOf": [
                { "properties": { "kind": { "const": "a" } }, "required": [field] },
                { "properties": { "kind": { "const": "b" }, field: false } },
            ],
        })
    }

    /// F0, F2, F3: branch `b` reads `mode` as a `Mode` while its schema
    /// says `b` forbids it; `a` reads it as `$a`; the field's schema is
    /// `$schema`.
    macro_rules! tagged_mode {
        ($name:ident, $a:ty, $schema:tt) => {
            #[derive(Debug, Serialize, Deserialize)]
            #[serde(tag = "kind", rename_all = "lowercase")]
            pub enum $name {
                A {
                    mode: $a,
                },
                B {
                    #[serde(default = "red", skip_serializing_if = "is_red")]
                    mode: Mode,
                },
            }
            impl JsonSchema for $name {
                fn schema_name() -> Cow<'static, str> {
                    stringify!($name).into()
                }
                fn json_schema(_: &mut SchemaGenerator) -> Schema {
                    a_requires_b_forbids("mode", serde_json::json!($schema))
                }
            }
        };
    }
    tagged_mode!(Unlisted, Value, {});
    tagged_mode!(NotAny, Value, { "type": "string" });
    tagged_mode!(Blinkered, Mode, {});

    /// F1: the forbidding branch requires a name `"x"` fails; serde reads
    /// `f` there.
    #[derive(Debug, JsonSchema, Serialize, Deserialize)]
    #[schemars(extend("anyOf" = [{ "required": ["f"] }, { "required": ["g"], "properties": { "f": false } }]))]
    pub struct BranchUnread {
        pub id: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        pub f: Value,
        #[serde(default = "Name::anon", skip_serializing_if = "Name::is_anon")]
        pub g: Name,
    }

    /// F4: the forbidding branch has two contexts: `b` refuses `f`, `c`
    /// reads it.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
    pub enum Leafy {
        A {
            f: Value,
        },
        B {},
        C {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            f: Option<Value>,
        },
    }
    impl JsonSchema for Leafy {
        fn schema_name() -> Cow<'static, str> {
            "Leafy".into()
        }
        fn json_schema(_: &mut SchemaGenerator) -> Schema {
            json_schema!({
                "type": "object",
                "properties": { "kind": { "type": "string" }, "f": {} },
                "required": ["kind"],
                "oneOf": [
                    { "properties": { "kind": { "const": "a" } }, "required": ["f"] },
                    { "oneOf": [{ "properties": { "kind": { "const": "b" } } }, { "properties": { "kind": { "const": "c" } } }], "properties": { "f": false } },
                ],
            })
        }
    }

    /// A validated name inside a nested object of a branch, and inside a
    /// flattened struct, each beside a default that is false.
    macro_rules! nest {
        ($m:ident, $name:ident) => {
            pub mod $m {
                use super::*;
                use super::$name as N;
                #[derive(Debug, JsonSchema, Serialize, Deserialize)]
                #[serde(tag = "kind")]
                pub enum Nest {
                    #[serde(rename = "a")]
                    A { inner: Inner },
                    #[serde(rename = "b")]
                    B { other: u8 },
                }
                #[derive(Debug, JsonSchema, Serialize, Deserialize)]
                pub struct Inner {
                    pub name: N,
                    #[serde(default = "no", skip_serializing_if = "is_no")]
                    #[schemars(extend("default" = true))]
                    pub dock: bool,
                }
                #[derive(Debug, JsonSchema, Serialize, Deserialize)]
                pub struct Flat {
                    pub id: String,
                    #[serde(flatten)]
                    pub inner: Inner,
                }
            }
        };
    }
    nest!(nest_unread, Name);
    nest!(nest_read, NameOk);
}

/// Each control is what refuses its fixture: with it, the fixture is
/// refused; without it, the fixture's false claim passes (so no other
/// control covers for it); and serde shows the claim false.
#[test]
fn each_control_is_what_refuses_its_fixture() {
    use super::fixtures::mode::Mode;
    use derived_conditions::shared_stale::SharedDefault;
    use derived_controls::*;
    use serde_json::from_value;
    /// A control, turned off; the forbidding sites its fixture is held
    /// with; its fixture's run; what refuses it; and serde's showing that
    /// the fixture's claim is false.
    type Mutation = (&'static str, fn(&mut Controls), &'static [&'static str], fn(Controls) -> anyhow::Result<BTreeSet<String>>, &'static str, fn() -> bool);
    let real = Controls::ALL.forbiddings;
    let table: [Mutation; 10] = [
        ("D1 samples_read", |c| c.samples_read = false, real, held_with::<Unread>, "with `dock` left out does not read", || !from_value::<Unread>(json!({ "id": "ok" })).unwrap().dock),
        ("D2 by_debug", |c| c.by_debug = false, real, held_with::<Hidden>, "serde reads Hidden { n: 7 } of it left out and Hidden { n: 0 }", || from_value::<Hidden>(json!({})).unwrap().n == 7),
        ("D3 field_read", |c| c.field_read = false, real, held_with::<Ignored>, "`n` is not read where its default is held", || from_value::<Ignored>(json!({})).unwrap().n == 0),
        ("D4 every_branch", |c| c.every_branch = false, real, held_with::<SharedDefault>, "`dock` left out is not its stated default true", || {
            matches!(from_value::<SharedDefault>(json!({ "kind": "b" })).unwrap(), SharedDefault::B { dock: false })
        }),
        ("N1 flat_conditions", |c| c.flat_conditions = false, real, held_with::<super::fixtures::nested::new::Nested>, "beside a condition branch with branches of its own", || {
            matches!(from_value::<super::fixtures::nested::new::Nested>(json!({ "kind": "c" })).unwrap(), super::fixtures::nested::new::Nested::C { dock: false })
        }),
        ("F0 forbidding_listed", |c| c.forbidding_listed = false, real, held_with::<Unlisted>, "Unlisted.b.mode: a condition branch forbids it, and the wire holds a forbidding at FORBIDDINGS' sites alone", || {
            matches!(from_value::<Unlisted>(json!({ "kind": "b", "mode": "Blue" })), Ok(Unlisted::B { mode: Mode::Blue }))
        }),
        ("F1 branch_reads", |c| c.branch_reads = false, &["BranchUnread.#1.f"], held_with::<BranchUnread>, "a sample of the branch that forbids `#1.f` does not read", || {
            from_value::<BranchUnread>(json!({ "id": "x", "g": "ok", "f": 1 })).is_ok_and(|r| r.f == json!(1))
        }),
        ("F2 field_any_json", |c| c.field_any_json = false, &["NotAny.b.mode"], held_with::<NotAny>, "NotAny.b.mode: forbidden, though its field is not any JSON", || {
            matches!(from_value::<NotAny>(json!({ "kind": "b", "mode": "Blue" })), Ok(NotAny::B { mode: Mode::Blue }))
        }),
        ("F3 allowing_reads_any", |c| c.allowing_reads_any = false, &["Blinkered.b.mode"], held_with::<Blinkered>, "where a branch allows `b.mode`, it is not read as any JSON", || {
            matches!(from_value::<Blinkered>(json!({ "kind": "b", "mode": "Blue" })), Ok(Blinkered::B { mode: Mode::Blue }))
        }),
        ("F4 every_leaf", |c| c.every_leaf = false, &["Leafy.#1.f"], held_with::<Leafy>, "a condition branch forbids `#1.f`, but serde reads it there", || {
            matches!(from_value::<Leafy>(json!({ "kind": "c", "f": 1 })), Ok(Leafy::C { f: Some(_) }))
        }),
    ];
    for (control, off, listed, run, refused, false_claim) in table {
        assert!(false_claim(), "{control}: serde shows the fixture's claim false");
        let with = Controls { forbiddings: listed, ..Controls::ALL };
        let found = run(with).map(|held| format!("{held:?}")).unwrap_err().to_string();
        assert!(found.contains(refused), "{control} refuses it: {found}");
        assert!(found.contains("the lock's ") || found.contains("xtask/src/wire/defaults.rs"), "{control} names where: {found}");
        let mut without = with;
        off(&mut without);
        assert!(run(without).is_ok(), "{control}: without it the false claim passes");
    }
    // H: a default the lock records and no probe reaches fails `hold`
    let held = defaults_hold::<derived_hidden::on::Row>().unwrap();
    let lock = "field Row.flag optional boolean\nbound Row.flag default true\nfield Gone.g optional boolean\nbound Gone.g default false\n";
    assert_eq!(super::defaults::unheld(lock, &held), ["Gone.g"]);
}

/// Attempts at a false pass, each refused: a validated name inside a
/// nested object of a branch, and inside a flattened struct, refused for
/// want of an example, then, given one, for the stale default; a
/// forbidding of anything but the one listed, any JSON.
#[test]
fn false_passes_tried_are_refused() {
    use derived_conditions::forbid_said;
    use derived_controls::*;
    let refused = |r: anyhow::Result<BTreeSet<String>>| r.unwrap_err().to_string();
    for (unread, site) in [(refused(defaults_hold::<nest_unread::Nest>()), "Inner.dock"), (refused(defaults_hold::<nest_unread::Flat>()), "Flat.dock")] {
        assert!(unread.contains("does not read") && unread.contains("too short") && unread.contains("examples") && unread.contains(site), "{unread}");
    }
    for stale in [refused(defaults_hold::<nest_read::Nest>()), refused(defaults_hold::<nest_read::Flat>())] {
        assert!(stale.contains("dock` left out is not its stated default true"), "{stale}");
    }
    // a forbidding at no listed site, though its field is any JSON and serde
    // refuses it there (BranchUnread's, with a name that reads), is refused
    let unlisted = refused(defaults_hold::<BranchUnread>());
    assert!(unlisted.contains("BranchUnread.#1.f") && unlisted.contains("FORBIDDINGS"), "{unlisted}");
    let unlisted = refused(defaults_hold::<forbid_said::Wrap>());
    assert!(unlisted.contains("Wrap.#1.dock") && unlisted.contains("FORBIDDINGS"), "{unlisted}");
}

/// The source guard over fixtures.rs (or other files), from one root type
/// as `module` names it.
fn guarded(files: &[(&str, &str)], root: &str, module: &str, components: &[&str], hand: &[(&str, &str)]) -> anyhow::Result<super::source::Guarded> {
    let files: Vec<(String, String)> = files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
    let src = super::source::Source::read(&files)?;
    let roots = vec![(syn::parse_str::<syn::Type>(root)?, module.to_string())];
    let components: Vec<String> = components.iter().map(|c| c.to_string()).collect();
    // wire types allowed in every module here (`WIRE_MODULES`' own test is apart)
    let modules = src.modules();
    super::source::guard(&src, &roots, &components, &super::source::Allowed { hand_written: hand, open: &[], wire_modules: &modules })
}

const FIXTURES: [(&str, &str); 1] = [("lib.rs", include_str!("fixtures.rs"))];

/// The review's five false passes at `8672f486`, each refused with a
/// clear error, by each layer that sees it; serde shows each a break.
#[test]
fn the_reviews_false_passes_are_refused() {
    use super::fixtures::*;
    use serde_json::from_value;
    let none = BTreeSet::new();
    let guard = |root: &str, module: &str| guarded(&FIXTURES, root, module, &[], &[]).unwrap_err().to_string();
    // 1. a default stated once beside nested branches: the guard refuses the
    // schema written by hand, the checker the default it cannot hold
    assert!(matches!(from_value::<nested::old::Nested>(json!({ "kind": "c" })).unwrap(), nested::old::Nested::C { dock: true }));
    assert!(matches!(from_value::<nested::new::Nested>(json!({ "kind": "c" })).unwrap(), nested::new::Nested::C { dock: false }));
    let refused = guard("Nested", "crate::nested::new");
    assert!(refused.contains("impl JsonSchema for Nested: written by hand"), "{refused}");
    for refused in [defaults_hold::<nested::old::Nested>().unwrap_err().to_string(), defaults_hold::<nested::new::Nested>().unwrap_err().to_string()] {
        assert!(refused.contains("Nested.dock: a default its object states once, beside a condition branch with branches of its own"), "{refused}");
    }
    // 2. a forbidding of a field `b` reads as a derived enum
    assert!(matches!(from_value::<mode::old::Tagged>(json!({ "kind": "b", "mode": "Blue" })), Ok(mode::old::Tagged::B { mode: mode::Mode::Blue })));
    let refused = guard("Tagged", "crate::mode::new");
    assert!(refused.contains("impl JsonSchema for Tagged: written by hand"), "{refused}");
    let refused = defaults_hold::<mode::new::Tagged>().unwrap_err().to_string();
    assert!(refused.contains("Tagged.b.mode") && refused.contains("FORBIDDINGS"), "{refused}");
    let listed = Controls { forbiddings: &["Tagged.b.mode"], ..Controls::ALL };
    let refused = held_with::<mode::new::Tagged>(listed).unwrap_err().to_string();
    assert!(refused.contains("not any JSON"), "{refused}");
    // 3. a dotted wire name: the guard, the projection and the parser each refuse it
    assert!(from_value::<dotted::new::Row>(json!({ "id": "q" })).is_err() && from_value::<dotted::old::Row>(json!({ "id": "q" })).is_ok());
    let refused = guard("Row", "crate::dotted::new");
    assert!(refused.contains("Row.mode: serde(rename = \"settings.mode\")"), "{refused}");
    let refused = project(&doc_of::<dotted::new::Row>(false).0).unwrap_err().to_string();
    assert!(refused.contains("\"settings.mode\""), "{refused}");
    let (was, now) = ("type Row in object\nfield Row.id required string\n", "type Row in object\nfield Row.id required string\nfield Row.settings.mode required boolean\n");
    assert!(breaches(&super::lock::facts(was).unwrap(), &super::lock::facts(now).unwrap(), &none).is_empty(), "read without its owners declared, it passes");
    let refused = parse(now).unwrap_err().to_string();
    assert!(refused.contains("`Row.settings`, which no `type` line declares"), "{refused}");
    // 4. an untagged enum told apart by constants serde does not read: the
    // guard refuses it; without it, both versions hold and nothing breaks
    let written = serde_json::to_value(shadow::new::Shadow::B { kind: "b".into(), dock: false, n: 300 }).unwrap();
    assert!(matches!(from_value::<shadow::old::Shadow>(written).unwrap(), shadow::old::Shadow::B { dock: true, .. }));
    let refused = guard("Shadow", "crate::shadow::new");
    assert!(refused.contains("Shadow: `#[serde(untagged)]`") && refused.contains("extend(\"const\""), "{refused}");
    assert!(defaults_hold::<shadow::old::Shadow>().is_ok() && defaults_hold::<shadow::new::Shadow>().is_ok());
    assert!(breaches(&lock_of::<shadow::old::Shadow>(true), &lock_of::<shadow::new::Shadow>(true), &none).is_empty());
    // 5. an alias dropped: the guard refuses it; without it, nothing breaks
    assert!(from_value::<alias::old::Alias>(json!({ "legacy": true })).is_ok() && from_value::<alias::new::Alias>(json!({ "legacy": true })).is_err());
    let refused = guard("Alias", "crate::alias::old");
    assert!(refused.contains("Alias.flag: `#[serde(alias)]`"), "{refused}");
    assert!(breaches(&lock_of::<alias::old::Alias>(false), &lock_of::<alias::new::Alias>(false), &none).is_empty());
}

/// Each of the source guard's refusals, on its fixture; where the rest of
/// the check would pass the change it hides, that too.
#[test]
fn each_source_rule_refuses_its_fixture() {
    use super::fixtures::*;
    use serde_json::from_value;
    let none = BTreeSet::new();
    let guard = |root: &str, module: &str| guarded(&FIXTURES, root, module, &[], &[]).unwrap_err().to_string();
    let rows: [(&str, &str, &str); 11] = [
        ("Shadow", "crate::shadow::old", "`#[serde(untagged)]`: serde reads the first branch that reads"),
        ("Shadow", "crate::shadow::old", "`#[schemars(extend(\"const\" = …))]`"),
        ("Alias", "crate::alias::old", "`#[serde(alias)]`: the schema does not hold an alias"),
        ("Row", "crate::dotted::new", "serde(rename = \"settings.mode\"): a wire name of letters, digits"),
        ("Dotted", "crate::refused", "Dotted::A: `#[serde(rename)]`: an attribute the wire does not support"),
        ("Lying", "crate::lying::old", "impl JsonSchema for Lying: written by hand"),
        ("Hooked", "crate::hooked::old", "Hooked.n: serde(deserialize_with = \"lenient\"): written by hand"),
        ("Flat", "crate::refused", "Flat.inner: `#[serde(flatten)]`: serde reads a flattened field by buffering"),
        ("Skipped", "crate::refused", "Skipped.n: `#[serde(skip_deserializing)]`"),
        ("External", "crate::refused", "External: an enum with data and no `#[serde(tag = …)]`"),
        ("Timed", "crate::refused", "Timed.after: a type `std::time::Duration` the source guard cannot read"),
    ];
    for (root, module, says) in rows {
        let refused = guard(root, module);
        assert!(refused.contains(says), "{root}: {refused}");
    }
    let refused = guard("Generic<u8>", "crate::refused");
    assert!(refused.contains("Generic: a generic wire type"), "{refused}");
    let refused = guard("Renamed", "crate::refused");
    assert!(refused.contains("Renamed: `#[schemars(rename)]`"), "{refused}");
    // what the rest of the check would pass: a schema written by hand over a
    // renamed field, and a reader hook schemars does not see
    assert!(from_value::<lying::old::Lying>(json!({ "b": 1 })).is_ok() && from_value::<lying::new::Lying>(json!({ "b": 1 })).is_err());
    assert!(breaches(&lock_of::<lying::old::Lying>(false), &lock_of::<lying::new::Lying>(false), &none).is_empty());
    assert!(from_value::<hooked::old::Hooked>(json!({ "n": "5" })).is_ok() && from_value::<hooked::new::Hooked>(json!({ "n": "5" })).is_err());
    assert!(breaches(&lock_of::<hooked::old::Hooked>(false), &lock_of::<hooked::new::Hooked>(false), &none).is_empty());
    // read as source alone: a macro at item level or in a function's body,
    // which could make what the guard cannot see; a derive not serde's,
    // schemars' or the standard library's; another crate's `Value`; an
    // untagged enum under nested `cfg_attr`s; a validator's bound; an impl
    // in a function's body
    let source_only = [
        ("macro_rules! m { () => {} }\npub struct S { pub a: u8 }\n", "lib.rs: macro_rules!: a macro at item level"),
        ("pub struct S { pub a: u8 }\nfn f() { macro_rules! m { () => {} } }\n", "lib.rs: macro_rules!: a macro at item level"),
        ("use serde_repr::Deserialize_repr;\n#[derive(Deserialize_repr)]\npub struct S { pub a: u8 }\n", "S: `#[derive(serde_repr::Deserialize_repr)]`, a derive the source guard does not know"),
        ("pub struct S { pub v: toml::Value }\n", "S.v: a type `toml::Value` the source guard cannot read"),
        ("#[cfg_attr(a, cfg_attr(b, serde(untagged)))]\npub enum S { A { n: u8 } }\n", "S: `#[cfg_attr(a, …)]`: the guard evaluates `feature = \"schema\"` alone"),
        ("#[derive(Debug)]\n#[cfg_attr(feature = \"schema\", serde(rename_all = \"camelCase\"))]\npub struct S { pub a: u8 }\n", "S: `serde` under `cfg_attr(feature = \"schema\", …)`"),
        ("#[derive(Debug)]\npub struct S { pub a: u8 }\n#[cfg(unix)]\nimpl serde::Serialize for S { fn serialize<T: serde::Serializer>(&self, s: T) -> Result<T::Ok, T::Error> { s.serialize_u8(self.a) } }\n", "lib.rs: an impl under `#[cfg(unix)]`"),
        ("#[derive(Debug)]\npub struct S { pub a: u16 }\n#[cfg(unix)]\npub mod m {}\n", "crate::m: a `mod` with `#[cfg(unix)]`"),
        ("#[derive(Debug)]\npub struct S { pub a: u8 }\npub mod gone;\n", "crate::gone: no file for the module"),
        ("#[derive(Debug)]\npub struct S { pub m: std::collections::BTreeMap<u8, u8> }\n", "S.m: a map whose keys are not `String`"),
        ("#[derive(Debug)]\npub struct S { #[cfg(unix)] pub a: u8 }\n", "S.a: a field under a `cfg`"),
        ("#[derive(Debug)]\npub struct S { pub a: &'static str }\n", "S.a: a type the source guard cannot read (a reference"),
        ("#[derive(Debug)]\npub struct S { pub a: u16 }\nimpl Unknown for S {}\n", "impl Unknown for S: a trait the source guard cannot name"),
        ("use serde::Serialize as Ser;\n#[derive(Debug)]\npub struct S { pub a: u16 }\nimpl Ser for S {}\n", "impl Serialize for S: written by hand"),
        ("use schemars::JsonSchema as Shape;\n#[derive(Debug)]\npub struct S { pub a: u16 }\nimpl Shape for S {}\n", "impl JsonSchema for S: written by hand"),
        ("use std::fmt;\n#[derive(Debug)]\npub struct S { pub a: u16 }\nimpl fmt::Debug for S {}\n", "impl Debug for S: written by hand"),
        ("use std::fmt::Debug as Shown;\npub struct S { pub a: u16 }\nimpl Shown for S {}\n", "S: no `#[derive(Debug)]`"),
        ("#[derive(Debug)]\n#[serde(rename_all(serialize = \"camelCase\"))]\npub struct S { pub a: u16 }\n", "S: a `rename_all` of another form"),
        ("#[derive(Debug)]\npub struct S(pub u16);\n", "S: a tuple or unit struct"),
        ("#[derive(Debug)]\npub struct S { pub a: Option<Option<u16>> }\n", "S.a: an `Option` of an `Option`"),
        ("#[derive(Debug)]\n#[serde(tag = \"type\")]\npub enum S { A(String) }\n", "S::A: a tagged variant holding no struct"),
        ("pub struct S { #[validate(length(min = 1))] pub a: String }\n", "S.a: `#[validate(…)]`, which schemars writes as bounds serde never checks"),
        ("use serde::Serialize;\npub struct S { pub a: u8 }\nfn f() { impl Serialize for S { fn serialize<T: serde::Serializer>(&self, s: T) -> Result<T::Ok, T::Error> { s.serialize_u8(self.a) } } }\n", "impl Serialize for S: written by hand"),
    ];
    for (text, says) in source_only {
        let refused = guarded(&[("lib.rs", text)], "S", "crate", &[], &[]).unwrap_err().to_string();
        assert!(refused.contains(says), "{text}: {refused}");
    }
    // a type marked open that OPEN does not list
    let open = "#[derive(schemars::JsonSchema)]\n#[schemars(extend(\"x-fragment-open\" = true))]\npub enum S { A }\n";
    let refused = guarded(&[("lib.rs", open)], "S", "crate", &[], &[]).unwrap_err().to_string();
    assert!(refused.contains("S: marked open, so the lock lets a new value of it pass: list it in OPEN"), "{refused}");
    // a path through `super::super` resolves where it names
    let nested = "pub mod a { pub mod b { #[derive(Debug)] pub struct S { pub t: super::super::T } } }\n#[derive(Debug)] pub struct T { pub n: u16, pub v: serde_json::Value }\n";
    assert_eq!(guarded(&[("lib.rs", nested)], "S", "crate::a::b", &[], &[]).unwrap().reached.into_iter().collect::<Vec<_>>(), ["crate::T", "crate::a::b::S"]);
    // a component of the document no type it read is named for
    let refused = guarded(&FIXTURES, "Row", "crate::dotted::old", &["Ghost"], &[]).unwrap_err().to_string();
    assert!(refused.contains("component Ghost: no type the source guard read"), "{refused}");
    // an allowlisted hook no type has, and one whose test is not there
    let refused = guarded(&FIXTURES, "Row", "crate::dotted::old", &[], &[("impl JsonSchema for Nope", "t")]).unwrap_err().to_string();
    assert!(refused.contains("HAND_WRITTEN lists `impl JsonSchema for Nope`, which no wire type has now"), "{refused}");
    let refused = guarded(&FIXTURES, "Lying", "crate::lying::old", &[], &[("impl JsonSchema for Lying", "no_such_test")]).unwrap_err().to_string();
    assert!(refused.contains("HAND_WRITTEN names its test `no_such_test`, which crates/proto does not have"), "{refused}");
    // what passes: the review's dotted fixture's old version
    assert_eq!(guarded(&FIXTURES, "Row", "crate::dotted::old", &["Row"], &[]).unwrap().reached.into_iter().collect::<Vec<_>>(), ["crate::dotted::old::Row"]);
}

/// The wire's own types pass the guard, read from crates/proto's source:
/// each by its own module's name (`live::Answer`, not `routes::Answer`),
/// the hand-written ones each with its test, every component one of them.
#[test]
fn the_wires_types_are_read_from_their_source() {
    let doc = super::openapi::document().unwrap();
    let src = super::source::Source::read_dir(&super::source::proto_src()).unwrap();
    let mut roots = src.route_types();
    roots.push((syn::parse_str("ErrorBody").unwrap(), "crate".into()));
    let components: Vec<String> = doc["components"]["schemas"].as_object().unwrap().keys().cloned().collect();
    let guarded = super::source::guard(&src, &roots, &components, &super::source::Allowed::wire()).unwrap();
    assert_eq!(guarded.facts, ["other FragmentKind other"], "the wire's one input fallback");
    let reached = guarded.reached;
    for t in ["crate::live::Answer", "crate::live::AnswerFrame", "crate::live::SubscribeFrame", "crate::live::Query", "crate::Reason", "crate::ErrorBody", "crate::routes::ListWatchFrame"] {
        assert!(reached.contains(t), "{t} reached: {reached:?}");
    }
    for t in ["crate::routes::Answer", "crate::search::Query", "crate::live::Cursor"] {
        assert!(!reached.contains(t), "{t} is no wire type: {reached:?}");
    }
}

/// A row's source, with an alias and without (not compiled: read alone).
const PLAIN: &str = "use schemars::JsonSchema;\nuse serde::{Deserialize, Serialize};\n#[derive(Debug, Serialize, Deserialize, JsonSchema)]\npub struct Row { pub flag: bool }\n";
const ALIASED: &str = "use schemars::JsonSchema;\nuse serde::{Deserialize, Serialize};\n#[derive(Debug, Serialize, Deserialize, JsonSchema)]\npub struct Row { #[serde(alias = \"legacy\")] pub flag: bool }\n";

/// The review's five false passes at `375914c1`, and the second review's:
/// serde shows each a break; each is refused, or, `serde(other)`, a lock
/// fact that breaches; and without the rule each would pass.
#[test]
fn the_round_nine_false_passes_are_refused() {
    use super::fixtures::*;
    use serde_json::from_value;
    let none = BTreeSet::new();
    let guard = |root: &str, module: &str| guarded(&FIXTURES, root, module, &[], &[]).unwrap_err().to_string();
    // 1. an input fallback: a fact now, so dropping it breaches, and adding it does not
    assert!(from_value::<other::old::Row>(json!({ "kind": "Future" })).is_ok() && from_value::<other::new::Row>(json!({ "kind": "Future" })).is_err());
    let facts = |module: &str| guarded(&FIXTURES, "Row", module, &["Row"], &[]).unwrap().facts;
    assert_eq!(facts("crate::other::old"), ["other Row Other"]);
    assert!(facts("crate::other::new").is_empty());
    let was = parse(&super::lock::project_with(&doc_of::<other::old::Row>(false).0, &facts("crate::other::old")).unwrap()).unwrap();
    let now = parse(&super::lock::project_with(&doc_of::<other::new::Row>(false).0, &facts("crate::other::new")).unwrap()).unwrap();
    assert_eq!(facts_of(&breaches(&was, &now, &none)), ["other Row Other"]);
    assert!(breaches(&now, &was, &none).is_empty(), "adding it is no break");
    assert!(breaches(&lock_of::<other::old::Row>(false), &lock_of::<other::new::Row>(false), &none).is_empty(), "the schema alone does not hold it");
    let refused = parse("type R in union(kind)\nvalue R A\nother R B\n").unwrap_err().to_string();
    assert!(refused.contains("`other R B`, of a value `R` does not have"), "{refused}");
    // 2. a skip predicate that omits a value not the default: refused; without
    // the rule both versions hold and nothing breaks, though `true` is lost
    let written = serde_json::to_value(skipped::new::Row { flag: true }).unwrap();
    assert_eq!(written, json!({}));
    assert!(!from_value::<skipped::old::Row>(written).unwrap().flag, "true written, false read");
    for m in ["crate::skipped::old", "crate::skipped::new"] {
        let refused = guard("Row", m);
        assert!(refused.contains(&format!("Row.flag: `skip_serializing_if = \"skip\"` ({m}::skip, on `bool`, beside default)")), "{refused}");
    }
    assert!(defaults_hold::<skipped::old::Row>().is_ok() && defaults_hold::<skipped::new::Row>().is_ok());
    assert!(breaches(&lock_of::<skipped::old::Row>(true), &lock_of::<skipped::new::Row>(true), &none).is_empty());
    // 3a. two definitions of one type, a `cfg` choosing the one compiled
    let twice = "#[cfg(not(any()))]\n#[derive(Debug)]\npub struct Row { #[serde(alias = \"legacy\")] pub flag: bool }\n#[cfg(any())]\n#[derive(Debug)]\npub struct Row { pub flag: bool }\n";
    let refused = guarded(&[("lib.rs", twice)], "Row", "crate", &[], &[]).unwrap_err().to_string();
    assert!(refused.contains("crate::Row: two bindings of one name in one scope") && refused.contains("Row: a wire type under a `cfg`"), "{refused}");
    // 3b. a `#[path]` module is refused, and a file no `mod` reaches is not read
    let pathed = [("lib.rs", "#[path = \"actual.rs\"]\npub mod row;\n"), ("actual.rs", ALIASED), ("row.rs", PLAIN)];
    let refused = guarded(&pathed, "row::Row", "crate", &[], &[]).unwrap_err().to_string();
    assert!(refused.contains("crate::row: a `mod` with `#[path]`"), "{refused}");
    let stray = [("lib.rs", "pub mod row;\n"), ("row.rs", PLAIN), ("stray.rs", "macro_rules! m { () => {} }\n")];
    assert_eq!(guarded(&stray, "row::Row", "crate", &["Row"], &[]).unwrap().reached.into_iter().collect::<Vec<_>>(), ["crate::row::Row"]);
    let refused = guarded(&[("lib.rs", "pub mod row;\n"), ("row.rs", ALIASED)], "row::Row", "crate", &[], &[]).unwrap_err().to_string();
    assert!(refused.contains("Row.flag: `#[serde(alias)]`"), "the module's own file is read: {refused}");
    // 4. a `Debug` by hand that prints no field: refused; without the rule
    // both versions hold, though `{}` reads false before and true after
    assert!(!from_value::<debugged::old::Row>(json!({})).unwrap().flag && from_value::<debugged::new::Row>(json!({})).unwrap().flag);
    let refused = guard("Row", "crate::debugged::new");
    assert!(refused.contains("Row: no `#[derive(Debug)]`") && refused.contains("impl Debug for Row: written by hand"), "{refused}");
    assert!(defaults_hold::<debugged::old::Row>().is_ok() && defaults_hold::<debugged::new::Row>().is_ok());
    assert!(breaches(&lock_of::<debugged::old::Row>(true), &lock_of::<debugged::new::Row>(true), &none).is_empty());
    // the second review: a reader by hand under a renamed trait
    assert!(from_value::<renamed::old::Row>(json!({ "legacy": true })).is_ok() && from_value::<renamed::new::Row>(json!({ "legacy": true })).is_err());
    let refused = guard("Row", "crate::renamed::old");
    assert!(refused.contains("impl Deserialize for Row: written by hand"), "{refused}");
    assert!(breaches(&lock_of::<renamed::old::Row>(false), &lock_of::<renamed::new::Row>(false), &none).is_empty());
}

/// The review's six false passes at `4d93eafa`, each refused: the guard
/// accepts only forms it resolves without evaluating Rust.
#[test]
fn the_round_ten_false_passes_are_refused() {
    let refused = |files: &[(&str, &str)], root: &str, module: &str| guarded(files, root, module, &[], &[]).unwrap_err().to_string();
    let row = |attrs: &str| format!("#[derive(Debug)]\npub struct Row {{ {attrs} pub dock: bool }}\n");
    // 1. a predicate of a known name in another module: names resolve to their
    // full paths, and only the pinned ones (at the crate's root) are known
    let nested = format!("fn on_the_dock() -> bool {{ true }}\nfn is_on_the_dock(b: &bool) -> bool {{ *b }}\npub mod nested {{\nfn on_the_dock() -> bool {{ true }}\nfn is_on_the_dock(_: &bool) -> bool {{ true }}\n{}}}\n", row("#[serde(default = \"on_the_dock\", skip_serializing_if = \"is_on_the_dock\")]"));
    let found = refused(&[("lib.rs", &nested)], "Row", "crate::nested");
    assert!(found.contains("`#[serde(default = …)]` naming `crate::nested::on_the_dock`") && found.contains("(crate::nested::is_on_the_dock, on `bool`"), "{found}");
    let shadow = format!("fn default_true() -> bool {{ true }}\nfn is_true(b: &bool) -> bool {{ *b }}\npub mod nested {{\nfn is_true(_: &bool) -> bool {{ true }}\n{}}}\n", row("#[serde(default = \"crate::default_true\", skip_serializing_if = \"is_true\")]"));
    let found = refused(&[("lib.rs", &shadow)], "Row", "crate::nested");
    assert!(found.contains("crate::nested::is_true: a function of a pinned name outside the crate's root") && found.contains("(crate::nested::is_true, on `bool`"), "{found}");
    // 2. a predicate whose body omits more than the default: `is_zero` is no
    // known predicate, and the pinned `is_default` is its text alone
    let zero = "fn is_zero(n: &u32) -> bool { *n == 0 || *n == 42 }\n#[derive(Debug)]\npub struct Row { #[serde(default, skip_serializing_if = \"is_zero\")] pub n: u32 }\n";
    assert!(refused(&[("lib.rs", zero)], "Row", "crate").contains("(crate::is_zero, on `u32`, beside default)"));
    let edited = "fn is_default<T: Default + PartialEq>(v: &T) -> bool { *v == T::default() || true }\n#[derive(Debug)]\npub struct Row { #[serde(default, skip_serializing_if = \"is_default\")] pub n: u32 }\n";
    assert!(refused(&[("lib.rs", edited)], "Row", "crate").contains("crate::is_default: not its pinned text"));
    // 3. imports under `cfg`s, binding one name twice
    let imports = "pub mod actual { #[derive(Debug)] pub struct Row { #[serde(alias = \"legacy\")] pub flag: bool } }\npub mod safe { #[derive(Debug)] pub struct Row { pub flag: bool } }\n#[cfg(not(any()))]\npub use actual::Row;\n#[cfg(any())]\npub use safe::Row;\n";
    let found = refused(&[("lib.rs", imports)], "Row", "crate");
    assert!(found.contains("crate: a `use` with an attribute") && found.contains("crate::Row: two bindings of one name in one scope"), "{found}");
    // 4. a module's file chosen by a `cfg_attr`
    let pathed = [("lib.rs", "#[cfg_attr(feature = \"schema\", path = \"actual.rs\")]\npub mod row;\n"), ("actual.rs", ALIASED), ("row.rs", PLAIN)];
    let found = refused(&pathed, "row::Row", "crate");
    assert!(found.contains("crate::row: a `mod` with `#[cfg_attr(") && found.contains("path = \"actual.rs\""), "{found}");
    // 5. a route constant of a block with its own items: a module and an import,
    // or a macro
    let block = "pub mod routes {\n#[derive(Debug)]\npub struct Row { pub flag: bool }\npub const T: u8 = { mod hidden { #[derive(Debug)] pub struct Row { #[serde(alias = \"legacy\")] pub flag: bool } } use hidden::Row; ty::<Row>() };\n}\n";
    let found = refused(&[("lib.rs", block)], "Row", "crate::routes");
    assert!(found.contains("crate::routes::T: `mod hidden` inside a constant's initializer") && found.contains("crate::routes::T: a `use` inside a constant's initializer") && found.contains("crate::routes::T: a route constant of another form"), "{found}");
    let macro_block = "pub mod routes {\n#[derive(Debug)]\npub struct Row { pub flag: bool }\npub const T: u8 = { macro_rules! row { () => {} } row!(); ty::<Row>() };\n}\n";
    let found = refused(&[("lib.rs", macro_block)], "Row", "crate::routes");
    assert!(found.contains("crate::routes::T: a macro inside a constant's initializer") && found.contains("a route constant of another form"), "{found}");
}

/// Each of round ten's refusals of a form the guard does not resolve, on
/// its fixture; and what the plain forms accept.
#[test]
fn only_plain_forms_are_read() {
    let refused = |text: &str, root: &str, module: &str| guarded(&[("lib.rs", text)], root, module, &[], &[]).unwrap_err().to_string();
    let rows = [
        ("#[allow(dead_code)]\nmod m {}\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::m: a `mod` with `#[allow(dead_code)]`"),
        ("#[cfg(feature = \"schema\")]\nmod m {}\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::m: a `mod` with `#[cfg(feature = \"schema\")]`"),
        ("fn f() { mod inner {} }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "`mod inner` inside a function's body"),
        ("#[allow(unused_imports)]\nuse std::fmt;\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate: a `use` with an attribute"),
        ("pub mod a { #[derive(Debug)] pub struct S { pub a: u16 } }\nuse a::S;\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::S: two bindings of one name in one scope"),
        ("const N: u8 = { struct Hidden; 0 };\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::N: an item inside a constant's initializer"),
        ("pub mod routes {\n#[derive(Debug)]\npub struct S { pub a: u16 }\npub const T: u8 = ty::<Vec<S>>();\n}\n", "crate::routes::T: `ty::<…>` of another type than a plain path"),
        ("pub mod routes {\n#[derive(Debug)]\npub struct S { pub a: u16 }\npub const T: u8 = (|| ty::<S>())();\n}\n", "crate::routes::T: a call of something other than a path"),
        ("pub mod other { pub const T: u8 = 0; }\npub mod routes {\n#[derive(Debug)]\npub struct S { pub a: u16 }\npub const R: u8 = crate::other::T;\n}\n", "crate::routes::R: `crate::other::T`, a value of another form"),
        ("pub mod other { pub const fn hidden() -> u8 { 0 } }\npub mod routes {\nuse crate::other::hidden;\n#[derive(Debug)]\npub struct S { pub a: u16 }\npub const R: u8 = hidden();\n}\n", "crate::routes::R: `hidden`, a value of another form"),
        ("fn default_true() -> bool { false }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::default_true: not its pinned text"),
        ("pub mod m { pub fn is_default<T: Default + PartialEq>(v: &T) -> bool { *v == T::default() } }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::m::is_default: a function of a pinned name outside the crate's root"),
        ("fn yes() -> bool { true }\n#[derive(Debug)]\npub struct S { #[serde(default = \"yes\")] pub a: bool }\n", "`#[serde(default = …)]` naming `crate::yes`"),
        ("pub struct Vec;\n#[derive(Debug)]\npub struct S { #[serde(default, skip_serializing_if = \"Vec::is_empty\")] pub a: Vec<u16> }\n", "`skip_serializing_if = \"Vec::is_empty\"` (a function the guard cannot resolve"),
        ("fn is_default<T: Default + PartialEq>(v: &T) -> bool { *v == T::default() }\n#[derive(Debug, Default, PartialEq)]\npub struct Inner { pub n: u16 }\n#[derive(Debug)]\npub struct S { #[serde(default, skip_serializing_if = \"is_default\")] pub a: Inner }\n", "(crate::is_default, on `?`, beside default)"),
        ("fn is_true(b: &bool) -> bool { *b }\n#[derive(Debug)]\npub struct S { #[serde(default, skip_serializing_if = \"is_true\")] pub a: bool }\n", "(crate::is_true, on `bool`, beside default)"),
        ("use std::collections::*;\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate: a glob import"),
        ("extern crate serde as other;\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate: `extern crate serde`"),
        ("#![cfg(any())]\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "lib.rs: an inner `#![cfg]`"),
        ("fn f() { use std::fmt; }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "a `use` inside a function's body"),
        ("fn f() { make!(); }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "lib.rs: make!, a macro the guard does not know"),
        ("fn f() -> String { format!(\"{}\", make!()) }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "lib.rs: make!, a macro the guard does not know"),
        ("const N: u8 = make!();\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::N: make!, a macro the guard does not know"),
        ("use other::vec;\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate: `use other::vec`, an import of a macro's name"),
        ("#[derive(Debug)]\n#[rewrite]\npub struct S { pub a: u16 }\n", "S: `#[rewrite]`, an attribute the guard does not read"),
        ("#[derive(Debug)]\npub struct S { #[some::rewrite] pub a: u16 }\n", "S.a: `#[some::rewrite]`, an attribute the guard does not read"),
        ("#[derive(Debug)]\npub struct S { pub a: u16 }\n#[rewrite]\nimpl Clone for S { fn clone(&self) -> S { S { a: self.a } } }\n", "an impl for S under `#[rewrite]`"),
        ("#[inline]\nfn is_true(b: &bool) -> bool { *b }\n#[derive(Debug)]\npub struct S { pub a: u16 }\n", "crate::is_true: under `#[inline]`"),
    ];
    for (text, says) in rows {
        let found = refused(text, "S", "crate");
        assert!(found.contains(says), "{text}\n{found}");
    }
    // a manifest that could make `serde::` another crate
    for (manifest, text, says) in [
        ("crates/proto/Cargo.toml", "[dependencies]\nserde = { package = \"other\", version = \"1\" }\n", "`package`"),
        ("Cargo.toml", "[workspace]\n[patch.crates-io]\nserde = { path = \"other\" }\n", "`[patch`"),
    ] {
        let root = scratch(&format!("manifest-{}", says.trim_matches(|c: char| !c.is_alphanumeric())));
        std::fs::create_dir_all(root.join("crates/proto/src")).unwrap();
        std::fs::write(root.join("crates/proto/src/lib.rs"), "#[derive(Debug)]\npub struct S { pub a: u16 }\n").unwrap();
        for m in ["Cargo.toml", "crates/proto/Cargo.toml"] {
            std::fs::write(root.join(m), if m == manifest { text } else { "[package]\nname = \"x\"\n" }).unwrap();
        }
        let src = super::source::Source::read_dir(&root.join("crates/proto/src")).unwrap();
        let roots = vec![(syn::parse_str("S").unwrap(), "crate".to_string())];
        let found = super::source::guard(&src, &roots, &[], &super::source::Allowed { hand_written: &[], open: &[], wire_modules: &["crate"] }).unwrap_err().to_string();
        assert!(found.contains(says), "{manifest}: {found}");
        std::fs::remove_dir_all(&root).unwrap();
    }
    // a wire type outside WIRE_MODULES, or below one's top level
    let src = super::source::Source::read(&[("lib.rs".into(), "pub mod deep { #[derive(Debug)] pub struct S { pub a: u16 } }\n".into())]).unwrap();
    let roots = vec![(syn::parse_str("deep::S").unwrap(), "crate".to_string())];
    let found = super::source::guard(&src, &roots, &[], &super::source::Allowed { hand_written: &[], open: &[], wire_modules: &["crate"] }).unwrap_err().to_string();
    assert!(found.contains("S: a wire type in `crate::deep`: wire types live at the top level of [\"crate\"] alone"), "{found}");
    // the plain forms: the pinned functions, the standard library's predicates,
    // and a route constant of literals
    let plain = "fn is_default<T: Default + PartialEq>(v: &T) -> bool { *v == T::default() }\nfn is_true(b: &bool) -> bool { *b }\nfn default_true() -> bool { true }\npub mod routes {\n#[derive(Debug)]\npub struct S {\n#[serde(default, skip_serializing_if = \"crate::is_default\")] pub n: u32,\n#[serde(default = \"crate::default_true\", skip_serializing_if = \"crate::is_true\")] pub dock: bool,\n#[serde(default, skip_serializing_if = \"Vec::is_empty\")] pub list: Vec<u16>,\n#[serde(default, skip_serializing_if = \"Option::is_none\")] pub maybe: Option<u16>,\n}\npub const T: &[(u8, &str)] = &[(1, \"a\")];\npub const R: u8 = ty::<S>();\n}\n";
    let src = super::source::Source::read(&[("lib.rs".into(), plain.into())]).unwrap();
    let roots = src.route_types();
    assert_eq!(roots.len(), 1, "one route root");
    let modules = src.modules();
    let held = super::source::guard(&src, &roots, &[], &super::source::Allowed { hand_written: &[], open: &[], wire_modules: &modules }).unwrap();
    assert_eq!(held.reached.into_iter().collect::<Vec<_>>(), ["crate::routes::S"]);
}
