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
