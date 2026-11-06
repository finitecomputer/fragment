//! The wire lock (`crates/proto/wire.lock`): the OpenAPI document as one
//! fact a line, so a change's diff is its list of wire changes, and what
//! a change may do to them by who writes each shape (`Direction`):
//!
//! | change                                  | out  | in   | both |
//! |-----------------------------------------|------|------|------|
//! | add a route, an optional field          | pass | pass | pass |
//! | add a required field; optional→required | pass | fail | fail |
//! | required→optional                       | fail | pass | fail |
//! | remove or rename a field; change a type | fail | fail | fail |
//! | add a value to an open enum or union    | pass | pass | pass |
//! | add a value to a closed one             | fail | pass | fail |
//! | remove a value, a variant or a route    | fail | fail | fail |
//! | add a field to a closed type            | fail | pass | fail |
//!
//! `out` is what the cell writes and clients read (answers, the frames
//! they receive); `in` what clients write and the cell reads (requests,
//! the frames they send, query parameters); `both` both. A closed type
//! refuses fields it does not know (`deny_unknown_fields`); an open enum
//! or union is one its readers read past (`x-fragment-open`). Making a
//! closed `in` type of an open one, and an open enum of a closed one a
//! client reads, fail too. Each failure names the fact it breaks: a line
//! of the lock, master's or this one's. `wire.retired` waives exactly
//! those it names (mod.rs).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use super::openapi::{Direction, COMPONENTS};

pub const HEADER: &str = "\
# The wire's facts (docs/wire/openapi.json), one a line: what clients
# call and the shapes they send and read. `cargo xtask wire` writes it
# from crates/proto; `cargo xtask check` fails when it is not what proto
# makes, or when it breaks a fact master's holds in a way an older
# client would feel (xtask/src/wire/lock.rs). Never edit it by hand: a
# fact gone on purpose is retired in wire.retired.
";

/// Every fact of the document, sorted, after the header.
pub fn project(doc: &Value) -> Result<String> {
    let mut lines = BTreeSet::new();
    let paths = doc["paths"].as_object().context("the document has paths")?;
    for (path, item) in paths {
        for (method, op) in item.as_object().context("a path item is an object")? {
            let name = op["operationId"].as_str().context("an operation is named")?;
            lines.insert(format!("route {name} {} {path}", method.to_ascii_uppercase()));
            let mut query = Map::new();
            let mut required = vec![];
            for p in op["parameters"].as_array().into_iter().flatten() {
                if p["in"] == "query" {
                    let n = p["name"].as_str().context("a parameter is named")?;
                    query.insert(n.to_string(), p["schema"].clone());
                    if p["required"] == true {
                        required.push(Value::String(n.to_string()));
                    }
                }
            }
            if !query.is_empty() {
                let pseudo = format!("{name}.query");
                let schema = serde_json::json!({ "type": "object", "properties": query, "required": required });
                Projector { lines: &mut lines }.object(&pseudo, &schema, Direction::In, false)?;
                lines.insert(format!("route {name} query {pseudo}"));
            }
            let mut p = Projector { lines: &mut lines };
            if let Some(body) = op.pointer("/requestBody/content/application~1json/schema") {
                let t = p.render(body, &format!("{name}.request"), Direction::In)?;
                p.lines.insert(format!("route {name} request {t}"));
            }
            if let Some(body) = op.pointer("/responses/200/content/application~1json/schema") {
                let t = p.render(body, &format!("{name}.response"), Direction::Out)?;
                p.lines.insert(format!("route {name} response {t}"));
            }
            if let Some(socket) = op.get("x-fragment-socket") {
                if !socket["sends"].is_null() {
                    let t = p.render(&socket["sends"], &format!("{name}.sends"), Direction::In)?;
                    p.lines.insert(format!("route {name} sends {t}"));
                }
                let t = p.render(&socket["receives"], &format!("{name}.receives"), Direction::Out)?;
                p.lines.insert(format!("route {name} receives {t}"));
            }
        }
    }
    let schemas = doc["components"]["schemas"].as_object().context("the document has components")?;
    for (name, schema) in schemas {
        let direction = schema["x-fragment-direction"].as_str().and_then(Direction::parse).with_context(|| format!("{name} names its direction"))?;
        Projector { lines: &mut lines }.component(name, schema, direction)?;
    }
    let mut out = HEADER.to_string();
    out.push('\n');
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

struct Projector<'a> {
    lines: &'a mut BTreeSet<String>,
}

/// Keys that say nothing a reader decodes by.
const UNSHAPED: [&str; 12] =
    ["description", "title", "default", "format", "minimum", "maximum", "examples", "deprecated", "readOnly", "writeOnly", "$comment", "$schema"];

fn shaped(schema: &Value) -> Map<String, Value> {
    schema.as_object().map(|m| m.iter().filter(|(k, _)| !UNSHAPED.contains(&k.as_str()) && !k.starts_with("x-")).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

impl Projector<'_> {
    fn component(&mut self, name: &str, schema: &Value, direction: Direction) -> Result<()> {
        let open = schema["x-fragment-open"] == true;
        if let Some(values) = enum_values(schema) {
            self.enumeration(name, &values, direction, open);
        } else if let Some(variants) = schema["oneOf"].as_array().filter(|_| shaped(schema).len() == 1) {
            self.union(name, variants, direction, open)?;
        } else if is_object(schema) {
            self.object(name, schema, direction, open)?;
        } else {
            let t = self.render(schema, name, direction)?;
            self.lines.insert(format!("type {name} {} is {t}", direction.as_str()));
        }
        Ok(())
    }

    fn enumeration(&mut self, name: &str, values: &[String], direction: Direction, open: bool) {
        self.lines.insert(format!("type {name} {} enum{}", direction.as_str(), if open { " open" } else { "" }));
        for v in values {
            self.lines.insert(format!("value {name} {v}"));
        }
    }

    /// An internally tagged union: each variant an object whose tag is a
    /// constant, projected as `<name>.<tag value>` without its tag.
    fn union(&mut self, name: &str, variants: &[Value], direction: Direction, open: bool) -> Result<()> {
        let tags: Vec<&String> = variants.first().and_then(|v| v["properties"].as_object()).map(|p| p.keys().collect()).unwrap_or_default();
        let tag = tags
            .into_iter()
            .find(|t| variants.iter().all(|v| v["properties"][t.as_str()]["const"].is_string()))
            .with_context(|| format!("{name}: a union's variants share a tag whose value is a constant (serde's `tag`)"))?
            .clone();
        self.lines.insert(format!("type {name} {} union({tag}){}", direction.as_str(), if open { " open" } else { "" }));
        for v in variants {
            let value = v["properties"][&tag]["const"].as_str().expect("found above");
            self.lines.insert(format!("value {name} {value}"));
            let mut variant = v.clone();
            variant["properties"].as_object_mut().expect("a variant has properties").remove(&tag);
            if let Some(required) = variant["required"].as_array_mut() {
                required.retain(|r| r != tag.as_str());
            }
            self.object(&format!("{name}.{value}"), &variant, direction, false)?;
        }
        Ok(())
    }

    fn object(&mut self, name: &str, schema: &Value, direction: Direction, open: bool) -> Result<()> {
        let closed = schema["additionalProperties"] == false;
        let kind = if closed { "object closed" } else { "object" };
        self.lines.insert(format!("type {name} {} {kind}{}", direction.as_str(), if open { " open" } else { "" }));
        if let Some(base) = schema["$ref"].as_str() {
            self.lines.insert(format!("extends {name} {}", base.strip_prefix(COMPONENTS).unwrap_or(base)));
        }
        let required: BTreeSet<&str> = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        for (field, s) in schema["properties"].as_object().into_iter().flatten() {
            let t = self.render(s, &format!("{name}.{field}"), direction)?;
            let need = if required.contains(field.as_str()) { "required" } else { "optional" };
            self.lines.insert(format!("field {name}.{field} {need} {t}"));
        }
        Ok(())
    }

    /// A schema as a field's type: a component's name, a scalar, a list
    /// or map of one, a constant, `T|null`, or an inline enum, union or
    /// object projected under `at`.
    fn render(&mut self, schema: &Value, at: &str, direction: Direction) -> Result<String> {
        if schema == &Value::Bool(true) {
            return Ok("any".into());
        }
        let s = shaped(schema);
        if s.is_empty() {
            return Ok("any".into());
        }
        if let Some(r) = s.get("$ref").and_then(Value::as_str).filter(|_| s.len() == 1) {
            return Ok(r.strip_prefix(COMPONENTS).unwrap_or(r).to_string());
        }
        if let Some(c) = s.get("const").filter(|_| s.len() <= 2) {
            return Ok(c.to_string());
        }
        if let Some(any) = s.get("anyOf").and_then(Value::as_array).filter(|_| s.len() == 1) {
            let null = |s: &Value| shaped(s) == shaped(&serde_json::json!({ "type": "null" }));
            if let [a, b] = any.as_slice() {
                if let Some(one) = [(a, b), (b, a)].into_iter().find(|(_, n)| null(n)).map(|(o, _)| o) {
                    return Ok(format!("{}|null", self.render(one, at, direction)?));
                }
            }
        }
        if let Some(values) = enum_values(schema) {
            self.enumeration(at, &values, direction, schema["x-fragment-open"] == true);
            return Ok(at.to_string());
        }
        if s.contains_key("oneOf") && s.len() == 1 {
            self.union(at, s["oneOf"].as_array().expect("oneOf is a list"), direction, schema["x-fragment-open"] == true)?;
            return Ok(at.to_string());
        }
        let types: Vec<&str> = match s.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let nullable = types.contains(&"null");
        let base: Vec<&str> = types.iter().copied().filter(|t| *t != "null").collect();
        let rendered = match base.as_slice() {
            ["array"] => format!("array<{}>", self.render(s.get("items").unwrap_or(&Value::Bool(true)), &format!("{at}.item"), direction)?),
            ["object"] if s.contains_key("properties") || !s.contains_key("additionalProperties") => {
                self.object(at, schema, direction, false)?;
                at.to_string()
            }
            ["object"] => format!("map<{}>", self.render(&s["additionalProperties"], &format!("{at}.value"), direction)?),
            [t @ ("string" | "integer" | "number" | "boolean")] if s.keys().all(|k| k == "type" || k == "pattern") => t.to_string(),
            _ => format!("schema {}", fragment_proto::canonical_json(&Value::Object(s.clone()))),
        };
        Ok(if nullable { format!("{rendered}|null") } else { rendered })
    }
}

fn is_object(schema: &Value) -> bool {
    schema["type"] == "object" || schema.get("properties").is_some()
}

/// A string enum's values: `enum`, or a `oneOf` of string constants and
/// enums (schemars writes a documented variant as its own branch).
fn enum_values(schema: &Value) -> Option<Vec<String>> {
    let strings = |v: &Value| v.as_array().and_then(|a| a.iter().map(|s| s.as_str().map(str::to_string)).collect::<Option<Vec<_>>>());
    if let Some(values) = schema.get("enum").and_then(strings) {
        return Some(values);
    }
    let branches = schema.get("oneOf")?.as_array()?;
    let mut values = vec![];
    for b in branches {
        match (b.get("const").and_then(Value::as_str), b.get("enum").and_then(strings)) {
            (Some(c), None) => values.push(c.to_string()),
            (None, Some(e)) => values.extend(e),
            _ => return None,
        }
    }
    Some(values)
}

/// A route's facts, by what they say of it.
#[derive(Default, Debug, PartialEq)]
struct RouteFacts {
    at: String,
    shapes: BTreeMap<String, String>,
}

#[derive(Default, Debug, PartialEq)]
struct TypeFacts {
    direction: Option<Direction>,
    kind: String,
    open: bool,
    extends: Option<String>,
    fields: BTreeMap<String, (bool, String)>,
    values: BTreeSet<String>,
}

#[derive(Default, Debug)]
pub struct Facts {
    routes: BTreeMap<String, RouteFacts>,
    types: BTreeMap<String, TypeFacts>,
}

/// A lock's facts: what `project` writes, read back.
pub fn parse(text: &str) -> Result<Facts> {
    let mut f = Facts::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("wire.lock line {}: {line:?} is no fact", n + 1);
        let words: Vec<&str> = line.splitn(4, ' ').collect();
        match words.as_slice() {
            ["route", op, slot, rest] if slot.chars().all(|c| c.is_ascii_uppercase()) => {
                f.routes.entry(op.to_string()).or_default().at = format!("{slot} {rest}");
            }
            ["route", op, slot, t] => {
                f.routes.entry(op.to_string()).or_default().shapes.insert(slot.to_string(), t.to_string());
            }
            ["type", name, dir, kind] => {
                let t = f.types.entry(name.to_string()).or_default();
                t.direction = Some(Direction::parse(dir).with_context(bad)?);
                let (kind, open) = match kind.strip_suffix(" open") {
                    Some(k) => (k, true),
                    None => (*kind, false),
                };
                t.kind = kind.to_string();
                t.open = open;
            }
            ["field", path, need, t] => {
                let (name, field) = path.rsplit_once('.').with_context(bad)?;
                let required = match *need {
                    "required" => true,
                    "optional" => false,
                    _ => bail!(bad()),
                };
                f.types.entry(name.to_string()).or_default().fields.insert(field.to_string(), (required, t.to_string()));
            }
            ["value", name, v] => {
                f.types.entry(name.to_string()).or_default().values.insert(v.to_string());
            }
            ["extends", name, base] => {
                f.types.entry(name.to_string()).or_default().extends = Some(base.to_string());
            }
            _ => bail!(bad()),
        }
    }
    Ok(f)
}

/// A fact this change breaks, and why an older client would feel it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breach {
    /// The lock's line it is about (master's, or this change's).
    pub fact: String,
    pub why: String,
}

/// What `head` breaks of `base`, but those `retired` names.
pub fn breaches(base: &Facts, head: &Facts, retired: &BTreeSet<String>) -> Vec<Breach> {
    let mut out = vec![];
    let mut breach = |fact: String, why: &str| {
        if !retired.contains(&fact) {
            out.push(Breach { fact, why: why.to_string() });
        }
    };
    for (op, was) in &base.routes {
        let at = format!("route {op} {}", was.at);
        let Some(now) = head.routes.get(op) else {
            breach(at, "the route is gone: an older client still calls it");
            continue;
        };
        if now.at != was.at {
            breach(at, "the route moved: an older client calls it where it was");
        }
        for slot in ["query", "request", "response", "sends", "receives"] {
            match (was.shapes.get(slot), now.shapes.get(slot)) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), _) => breach(format!("route {op} {slot} {a}"), "the route's shape changed: an older client sends or reads the one it had"),
                (None, Some(b)) => breach(format!("route {op} {slot} {b}"), "the route takes or answers a shape it did not: an older client neither sends nor expects it"),
                (None, None) => {}
            }
        }
    }
    for (name, was) in &base.types {
        let Some(now) = head.types.get(name) else { continue };
        let dir = |t: &TypeFacts| t.direction.unwrap_or(Direction::Both);
        let (d0, d1) = (dir(was), dir(now));
        let dir = if d0 == d1 { d0 } else { Direction::Both };
        let type_line = |t: &TypeFacts| format!("type {name} {} {}{}", dir_of(t), t.kind, if t.open { " open" } else { "" });
        let read = dir.reads_as_out();
        let written = dir.reads_as_in();
        let base_kind = was.kind.trim_end_matches(" closed");
        if base_kind != now.kind.trim_end_matches(" closed") {
            breach(type_line(was), "the shape is another kind now: an older client decodes the kind it was");
            continue;
        }
        let closed_was = was.kind.ends_with(" closed");
        let closed_now = now.kind.ends_with(" closed");
        if written && closed_now && !closed_was {
            breach(type_line(now), "it refuses fields it does not know now: an older client that sends one is refused");
        }
        if read && now.open && !was.open {
            breach(type_line(now), "it is open now, but an older client reads it closed: a value it does not know fails it");
        }
        if was.extends != now.extends {
            let line = match &was.extends {
                Some(b) => format!("extends {name} {b}"),
                None => format!("extends {name} {}", now.extends.as_deref().unwrap_or_default()),
            };
            breach(line, "the shape it is built on changed: an older client reads the one it had");
        }
        let field_line = |f: &str, (req, t): &(bool, String)| format!("field {name}.{f} {} {t}", if *req { "required" } else { "optional" });
        for (f, was_f) in &was.fields {
            let line = field_line(f, was_f);
            match now.fields.get(f) {
                None => breach(line, "the field is gone (removed or renamed): an older client sends or reads it"),
                Some((_, t)) if *t != was_f.1 => breach(line, "the field's type changed: an older client sends or reads the type it had"),
                Some((false, _)) if was_f.0 && read => breach(line, "the field may be absent now: an older client reads it as always there"),
                Some((true, _)) if !was_f.0 && written => breach(line, "the field is required now: an older client that leaves it out is refused"),
                Some(_) => {}
            }
        }
        for (f, now_f) in now.fields.iter().filter(|(f, _)| !was.fields.contains_key(*f)) {
            if now_f.0 && written {
                breach(field_line(f, now_f), "a new required field: an older client does not send it");
            } else if read && closed_now {
                breach(field_line(f, now_f), "a new field of a closed shape: an older client that refuses fields it does not know fails on it");
            }
        }
        for v in was.values.difference(&now.values) {
            breach(format!("value {name} {v}"), "the value is gone: an older client sends or expects it");
        }
        if read && !now.open {
            for v in now.values.difference(&was.values) {
                breach(format!("value {name} {v}"), "a new value of a closed shape: an older client fails on a value it does not know");
            }
        }
    }
    out
}

fn dir_of(t: &TypeFacts) -> &'static str {
    t.direction.map(Direction::as_str).unwrap_or("both")
}

/// A schema as the lock writes a field's type (the reference's).
pub fn type_of(schema: &Value, at: &str) -> String {
    let mut scratch = BTreeSet::new();
    Projector { lines: &mut scratch }.render(schema, at, Direction::Both).unwrap_or_else(|_| "any".into())
}

/// A component's string values, if it is an enum (the reference's).
pub fn values_of(schema: &Value) -> Option<Vec<String>> {
    enum_values(schema)
}
