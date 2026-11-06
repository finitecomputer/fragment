//! The wire lock's rules, cell by cell (lock.rs's table), its retirements,
//! and the files: valid, invalid, replay and restart.

use std::collections::BTreeSet;

use serde_json::Value;

use super::lock::{breaches, parse, Breach};
use super::{generate, retired, verdict, write_files, Base, OnDisk, LOCK_PATH, RETIRED_HEADER, RETIRED_PATH};

/// Master's facts: a shape each way, enums each way, an open union, and
/// a closed shape each way.
const BASE: &str = "\
route a.get GET /api/a
route a.get response A
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
    // a reader of a shape now reads past what it did not know
    passes(&[("type K out object closed", Some("type K out object"))], &[]);
    passes(&[("type EI in enum", Some("type EI in enum open"))], &[]);
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
    // a shape that refuses what it took, or opens where a client reads it closed
    fails(&[("type KO in object", Some("type KO in object closed"))], &[], "type KO in object closed");
    fails(&[("type EO out enum", Some("type EO out enum open"))], &[], "type EO out enum open");
    // a shape of another kind
    fails(&[("type EO out enum", Some("type EO out object"))], &[], "type EO out enum");
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
