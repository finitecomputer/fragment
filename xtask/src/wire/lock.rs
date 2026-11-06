//! The wire lock (`crates/proto/wire.lock`): the OpenAPI document as one
//! fact a line, so a change's diff is its list of wire changes, and what
//! a change may do to them by who writes each shape (`Direction`):
//!
//! | change                                           | out  | in   | both |
//! |--------------------------------------------------|------|------|------|
//! | add a route, an optional field, a credential     | pass | pass | pass |
//! | add a required field; optional→required          | pass | fail | fail |
//! | required→optional                                | fail | pass | fail |
//! | remove or rename a field; change a type          | fail | fail | fail |
//! | make a field nullable                            | fail | pass | fail |
//! | make a nullable field not                        | pass | fail | fail |
//! | add a value to an open enum or union             | pass | pass | pass |
//! | add a value or a branch to a closed one          | fail | pass | fail |
//! | remove a value, a branch, a route, a credential  | fail | fail | fail |
//! | add a field to a type its old readers close      | fail | pass | fail |
//! | stop closing a type an old reader closes         | fail | pass | fail |
//! | narrow a bound                                   | pass | fail | fail |
//! | widen a bound                                    | fail | pass | fail |
//! | change the default of a field one may leave out  | fail | fail | fail |
//!
//! `out` is what the cell writes and clients read (answers, the frames
//! they receive); `in` what clients write and the cell reads (requests,
//! the frames they send, query parameters); `both` both. A closed type
//! refuses fields it does not know (`deny_unknown_fields`); an open enum
//! or union is one its readers read past (`x-fragment-open`). A bound is
//! what a value may be beyond its type: an integer's range (its `format`
//! and `minimum`/`maximum`), a number's, a string's length, `pattern` and
//! `format`, a list's length and `uniqueItems`, a map's size, `multipleOf`.
//! Making a closed `in` type of an open one, and an open enum of a closed
//! one a client reads, fail too. Each failure names the fact it breaks: a
//! line of the lock, master's or this one's. `wire.retired` waives exactly
//! those it names (mod.rs); retiring a closed type's line also lets it
//! gain fields.
//!
//! The projection is closed: every keyword of every schema is a fact
//! (`STRUCTURAL`, `BOUNDS`) or says nothing a reader decodes by
//! (`DOCUMENTARY`), and a schema with any other is refused, so a
//! constraint never goes missing from the lock unseen.

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

/// Keywords that make a shape: its fields, items, branches and values.
const STRUCTURAL: [&str; 10] = ["type", "properties", "required", "items", "additionalProperties", "$ref", "oneOf", "anyOf", "const", "enum"];
/// Keywords that bound a value beyond its type, and its default.
const BOUNDS: [&str; 15] = [
    "format",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "pattern",
    "multipleOf",
    "default",
];
/// Keywords no reader decodes by.
const DOCUMENTARY: [&str; 9] = ["description", "title", "examples", "deprecated", "readOnly", "writeOnly", "$comment", "x-fragment-direction", "x-fragment-open"];

/// Every fact of the document, sorted, after the header.
pub fn project(doc: &Value) -> Result<String> {
    let mut lines = BTreeSet::new();
    let paths = doc["paths"].as_object().context("the document has paths")?;
    for (path, item) in paths {
        for (method, op) in item.as_object().context("a path item is an object")? {
            let name = op["operationId"].as_str().context("an operation is named")?;
            lines.insert(format!("route {name} {} {path}", method.to_ascii_uppercase()));
            for requirement in op["security"].as_array().into_iter().flatten() {
                let scheme = requirement.as_object().and_then(|m| m.keys().next()).map_or("none", String::as_str);
                lines.insert(format!("route {name} accepts {scheme}"));
            }
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
            let mut p = Projector { lines: &mut lines };
            if !query.is_empty() {
                let pseudo = format!("{name}.query");
                let schema = serde_json::json!({ "type": "object", "properties": query, "required": required });
                p.object(&pseudo, &schema, Direction::In, false)?;
                p.lines.insert(format!("route {name} query {pseudo}"));
            }
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
        Projector { lines: &mut lines }.node(name, schema, direction, schema["x-fragment-open"] == true)?;
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

/// A schema's keywords, every one of them known.
fn known<'s>(schema: &'s Value, at: &str) -> Result<&'s Map<String, Value>> {
    let map = schema.as_object().with_context(|| format!("{at}: a schema is an object"))?;
    if let Some(k) = map.keys().find(|k| !STRUCTURAL.contains(&k.as_str()) && !BOUNDS.contains(&k.as_str()) && !DOCUMENTARY.contains(&k.as_str())) {
        bail!("{at}: the wire lock does not know `{k}`, so it cannot say what changing it breaks: teach xtask/src/wire/lock.rs, or describe the type another way");
    }
    Ok(map)
}

/// The keywords that make its shape.
fn structural(map: &Map<String, Value>) -> Map<String, Value> {
    map.iter().filter(|(k, _)| STRUCTURAL.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn types_of(map: &Map<String, Value>) -> Vec<&str> {
    match map.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    }
}

/// `{type: null}`, as `Option` writes its absence.
fn is_null(schema: &Value) -> bool {
    schema.as_object().is_some_and(|m| structural(m) == *serde_json::json!({ "type": "null" }).as_object().expect("an object"))
}

/// A union's branches: `oneOf`, or an `anyOf` that is not `T|null`.
fn union_of(map: &Map<String, Value>) -> Option<(&'static str, &Vec<Value>)> {
    if let Some(branches) = map.get("oneOf").and_then(Value::as_array) {
        return Some(("oneOf", branches));
    }
    let any = map.get("anyOf").and_then(Value::as_array)?;
    let nullable = any.len() == 2 && any.iter().filter(|b| is_null(b)).count() == 1;
    (!nullable).then_some(("anyOf", any))
}

impl Projector<'_> {
    /// A named shape: a component, a union's branch, an inline object.
    fn node(&mut self, name: &str, schema: &Value, direction: Direction, open: bool) -> Result<()> {
        if schema == &Value::Bool(true) {
            self.lines.insert(format!("type {name} {} is any", direction.as_str()));
            return Ok(());
        }
        let map = known(schema, name)?;
        if let Some(values) = enum_values(schema) {
            self.bounds(name, map)?;
            self.enumeration(name, &values, direction, open);
        } else if map.contains_key("properties") || types_of(map) == ["object"] {
            self.object(name, schema, direction, open)?;
        } else if let (Some((keyword, branches)), true) = (union_of(map), !map.contains_key("type")) {
            self.bounds(name, map)?;
            self.union(name, keyword, branches, direction, open)?;
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

    /// A union: `union(<tag>)` when every branch names itself by a
    /// constant of one property (serde's `tag`), each branch projected as
    /// `<name>.<its tag>` without the tag; else `union(oneOf)` or
    /// `union(anyOf)`, each branch `<name>.#<i>` by its place.
    fn union(&mut self, name: &str, keyword: &str, branches: &[Value], direction: Direction, open: bool) -> Result<()> {
        let tag = tag_of(branches);
        let kind = tag.clone().unwrap_or_else(|| keyword.to_string());
        self.lines.insert(format!("type {name} {} union({kind}){}", direction.as_str(), if open { " open" } else { "" }));
        self.branches(name, branches, tag.as_deref(), direction)
    }

    /// Each branch's key as a value of `name`, and its shape under it.
    fn branches(&mut self, name: &str, branches: &[Value], tag: Option<&str>, direction: Direction) -> Result<()> {
        for (i, b) in branches.iter().enumerate() {
            let key = match tag {
                Some(t) => b["properties"][t]["const"].as_str().expect("tag_of found it").to_string(),
                None => format!("#{i}"),
            };
            self.lines.insert(format!("value {name} {key}"));
            let mut branch = b.clone();
            if let Some(t) = tag {
                branch["properties"].as_object_mut().expect("a tagged branch has properties").remove(t);
                if let Some(required) = branch["required"].as_array_mut() {
                    required.retain(|r| r != t);
                }
            }
            self.node(&format!("{name}.{key}"), &branch, direction, false)?;
        }
        Ok(())
    }

    fn object(&mut self, name: &str, schema: &Value, direction: Direction, open: bool) -> Result<()> {
        let map = known(schema, name)?;
        let closed = map.get("additionalProperties") == Some(&Value::Bool(false));
        let kind = if closed { "object closed" } else { "object" };
        self.lines.insert(format!("type {name} {} {kind}{}", direction.as_str(), if open { " open" } else { "" }));
        self.bounds(name, map)?;
        if let Some(base) = map.get("$ref").and_then(Value::as_str) {
            self.lines.insert(format!("extends {name} {}", base.strip_prefix(COMPONENTS).unwrap_or(base)));
        }
        let required: BTreeSet<&str> = map.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        for (field, s) in map.get("properties").and_then(Value::as_object).into_iter().flatten() {
            let t = self.render(s, &format!("{name}.{field}"), direction)?;
            let need = if required.contains(field.as_str()) { "required" } else { "optional" };
            self.lines.insert(format!("field {name}.{field} {need} {t}"));
        }
        match map.get("additionalProperties") {
            None | Some(Value::Bool(_)) => {}
            // the fields it may hold beyond those it names: `*`
            Some(extra) => {
                let t = self.render(extra, &format!("{name}.*"), direction)?;
                self.lines.insert(format!("field {name}.* optional {t}"));
            }
        }
        // a flattened union beside its fields
        if let Some((keyword, branches)) = union_of(map) {
            let tag = tag_of(branches);
            self.lines.insert(format!("branches {name} {}", tag.clone().unwrap_or_else(|| keyword.to_string())));
            self.branches(name, branches, tag.as_deref(), direction)?;
        }
        Ok(())
    }

    /// A schema as a field's type: a component's name, a scalar, a list
    /// or map of one, a constant, `T|null`, or an inline enum, union or
    /// object projected under `at`; its bounds as facts at `at`.
    fn render(&mut self, schema: &Value, at: &str, direction: Direction) -> Result<String> {
        if schema == &Value::Bool(true) {
            return Ok("any".into());
        }
        let map = known(schema, at)?;
        let s = structural(map);
        if s.is_empty() {
            self.bounds(at, map)?;
            return Ok("any".into());
        }
        if let Some(any) = s.get("anyOf").and_then(Value::as_array).filter(|_| s.len() == 1 && union_of(map).is_none()) {
            self.bounds(at, map)?;
            let one = any.iter().find(|b| !is_null(b)).expect("T|null has its T");
            return Ok(format!("{}|null", self.render(one, at, direction)?));
        }
        let objectish = matches!(types_of(map).as_slice(), [] | ["object"]);
        if enum_values(schema).is_some() || s.contains_key("properties") || (union_of(map).is_some() && objectish) {
            self.node(at, schema, direction, schema["x-fragment-open"] == true)?;
            return Ok(at.to_string());
        }
        if let Some(r) = s.get("$ref").and_then(Value::as_str).filter(|_| s.len() == 1) {
            self.bounds(at, map)?;
            return Ok(r.strip_prefix(COMPONENTS).unwrap_or(r).to_string());
        }
        if let Some(c) = s.get("const").filter(|_| s.len() <= 2) {
            self.bounds(at, map)?;
            return Ok(c.to_string());
        }
        let types = types_of(map);
        let nullable = types.contains(&"null");
        let base: Vec<&str> = types.iter().copied().filter(|t| *t != "null").collect();
        let rendered = match base.as_slice() {
            ["array"] if s.keys().all(|k| k == "type" || k == "items") => {
                self.bounds(at, map)?;
                format!("array<{}>", self.render(s.get("items").unwrap_or(&Value::Bool(true)), &format!("{at}.item"), direction)?)
            }
            ["object"] if s.keys().all(|k| k == "type" || k == "additionalProperties") && s.get("additionalProperties").is_some_and(|a| a != &Value::Bool(false)) => {
                self.bounds(at, map)?;
                format!("map<{}>", self.render(&s["additionalProperties"], &format!("{at}.value"), direction)?)
            }
            ["object"] => {
                self.node(at, schema, direction, false)?;
                at.to_string()
            }
            [t @ ("string" | "integer" | "number" | "boolean")] if s.keys().all(|k| k == "type") => {
                self.bounds(at, map)?;
                t.to_string()
            }
            _ => bail!("{at}: a schema the wire lock does not know how to read ({}): teach xtask/src/wire/lock.rs, or describe the type another way", Value::Object(s)),
        };
        Ok(if nullable { format!("{rendered}|null") } else { rendered })
    }

    /// Its bounds and default, as `bound <at> <kind> <value>` facts.
    fn bounds(&mut self, at: &str, map: &Map<String, Value>) -> Result<()> {
        let types = types_of(map);
        let numeric = types.iter().any(|t| *t == "integer" || *t == "number");
        let has = |k: &str| map.contains_key(k);
        if numeric {
            let range = numeric_range(map, types.contains(&"integer")).with_context(|| format!("{at}'s range"))?;
            if let Some(r) = range {
                self.lines.insert(format!("bound {at} range {}", r.render()));
            }
        } else if ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"].into_iter().any(has) {
            bail!("{at}: a numeric bound on a value that is no number");
        }
        for (kind, lo, hi) in [("length", "minLength", "maxLength"), ("items", "minItems", "maxItems"), ("props", "minProperties", "maxProperties")] {
            if has(lo) || has(hi) {
                let n = |k: &str| map.get(k).map(|v| v.as_u64().map(|n| n.to_string()).with_context(|| format!("{at}: {k} is a count")));
                let lo = n(lo).transpose()?.unwrap_or_else(|| "0".into());
                let hi = n(hi).transpose()?.map(|h| format!("={h}")).unwrap_or_default();
                self.lines.insert(format!("bound {at} {kind} {lo}..{hi}"));
            }
        }
        if map.get("uniqueItems") == Some(&Value::Bool(true)) {
            self.lines.insert(format!("bound {at} unique"));
        }
        if let Some(p) = map.get("pattern") {
            self.lines.insert(format!("bound {at} pattern {}", p.as_str().with_context(|| format!("{at}: a pattern is a string"))?));
        }
        if let (false, Some(f)) = (numeric, map.get("format")) {
            self.lines.insert(format!("bound {at} format {}", f.as_str().with_context(|| format!("{at}: a format is a string"))?));
        }
        if let Some(m) = map.get("multipleOf") {
            self.lines.insert(format!("bound {at} multiple {m}"));
        }
        if let Some(d) = map.get("default") {
            self.lines.insert(format!("bound {at} default {}", fragment_proto::canonical_json(d)));
        }
        Ok(())
    }
}

/// The tag every branch names itself by: a property each holds as a
/// constant string, `type` first.
fn tag_of(branches: &[Value]) -> Option<String> {
    let first = branches.first()?["properties"].as_object()?;
    let mut keys: Vec<&String> = first.keys().collect();
    keys.sort_by_key(|k| *k != "type");
    keys.into_iter().find(|t| branches.iter().all(|b| b["properties"][t.as_str()]["const"].is_string())).cloned()
}

/// The sized integer formats schemars writes, and their ranges.
const INTEGER_FORMATS: [(&str, i128, i128); 10] = [
    ("int8", i8::MIN as i128, i8::MAX as i128),
    ("int16", i16::MIN as i128, i16::MAX as i128),
    ("int32", i32::MIN as i128, i32::MAX as i128),
    ("int64", i64::MIN as i128, i64::MAX as i128),
    ("int128", i128::MIN, i128::MAX),
    ("uint8", 0, u8::MAX as i128),
    ("uint16", 0, u16::MAX as i128),
    ("uint32", 0, u32::MAX as i128),
    ("uint64", 0, u64::MAX as i128),
    ("uint128", 0, i128::MAX),
];

/// A range of numbers, either end open (`None`), as the lock writes it:
/// a sized integer by its name (`u32`), else `lo..=hi`, `lo..` or `..=hi`.
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    lo: Option<String>,
    hi: Option<String>,
}

const NAMED: [(&str, &str); 12] = [
    ("i8", "int8"),
    ("i16", "int16"),
    ("i32", "int32"),
    ("i64", "int64"),
    ("i128", "int128"),
    ("u8", "uint8"),
    ("u16", "uint16"),
    ("u32", "uint32"),
    ("u64", "uint64"),
    ("u128", "uint128"),
    ("f32", "float"),
    ("f64", "double"),
];

impl Range {
    fn of_format(format: &str) -> Option<Range> {
        if let Some((_, lo, hi)) = INTEGER_FORMATS.iter().find(|(f, ..)| *f == format) {
            return Some(Range { lo: Some(lo.to_string()), hi: Some(hi.to_string()) });
        }
        match format {
            "float" => Some(Range { lo: Some((-f32::MAX as f64).to_string()), hi: Some((f32::MAX as f64).to_string()) }),
            "double" => Some(Range { lo: None, hi: None }),
            _ => None,
        }
    }

    pub fn parse(text: &str) -> Option<Range> {
        if let Some((_, format)) = NAMED.iter().find(|(n, _)| *n == text) {
            return Range::of_format(format);
        }
        let (lo, hi) = text.split_once("..")?;
        let hi = match hi.strip_prefix('=') {
            Some(h) => Some(h),
            None if hi.is_empty() => None,
            None => return None,
        };
        let num = |s: &str| (s.parse::<i128>().is_ok() || s.parse::<f64>().is_ok_and(f64::is_finite)).then(|| s.to_string());
        let lo = if lo.is_empty() { None } else { Some(num(lo)?) };
        let hi = match hi {
            Some(h) => Some(num(h)?),
            None => None,
        };
        Some(Range { lo, hi })
    }

    fn render(&self) -> String {
        if let Some((name, _)) = NAMED.iter().find(|(_, f)| Range::of_format(f).as_ref() == Some(self)) {
            return name.to_string();
        }
        format!("{}..{}", self.lo.clone().unwrap_or_default(), self.hi.as_ref().map(|h| format!("={h}")).unwrap_or_default())
    }

    /// Whether every number in `inner` is in this one.
    pub fn holds(&self, inner: &Range) -> bool {
        let below = at_most;
        let lo = match (&self.lo, &inner.lo) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => below(a, b),
        };
        let hi = match (&self.hi, &inner.hi) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => below(b, a),
        };
        lo && hi
    }
}

/// A number's range from its `format` and bounds, or none when it names
/// neither. An integer's exclusive bounds are inclusive ones a step in; a
/// number's are refused (no float this wire carries has one yet).
fn numeric_range(map: &Map<String, Value>, integer: bool) -> Result<Option<Range>> {
    let mut range = match map.get("format").map(|f| f.as_str().context("a format is a string")).transpose()? {
        Some(f) => Range::of_format(f).with_context(|| format!("`{f}` is no sized number: use a sized integer (an isize or usize is as wide as its machine)"))?,
        None => Range { lo: None, hi: None },
    };
    let bound = |k: &str| -> Result<Option<String>> {
        map.get(k)
            .map(|v| {
                let text = v.to_string();
                if integer {
                    let n = v.as_i64().map(i128::from).or(v.as_u64().map(i128::from)).with_context(|| format!("an integer's {k} is an integer"))?;
                    Ok(match k {
                        "exclusiveMinimum" => (n + 1).to_string(),
                        "exclusiveMaximum" => (n - 1).to_string(),
                        _ => n.to_string(),
                    })
                } else if k.starts_with("exclusive") {
                    bail!("a number's {k}: the wire lock reads only inclusive bounds of numbers")
                } else {
                    Ok(text)
                }
            })
            .transpose()
    };
    // the higher of two low ends, the lower of two high ends
    let tighter = |a: Option<String>, b: Option<String>, low: bool| -> Option<String> {
        match (a, b) {
            (None, b) => b,
            (a, None) => a,
            (Some(a), Some(b)) => Some(if at_most(&a, &b) == low { b } else { a }),
        }
    };
    for k in ["minimum", "exclusiveMinimum"] {
        range.lo = tighter(range.lo.take(), bound(k)?, true);
    }
    for k in ["maximum", "exclusiveMaximum"] {
        range.hi = tighter(range.hi.take(), bound(k)?, false);
    }
    let named = ["format", "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"].iter().any(|k| map.contains_key(*k));
    Ok(named.then_some(range))
}

/// `a <= b`, as integers where both are, else as numbers.
fn at_most(a: &str, b: &str) -> bool {
    match (a.parse::<i128>(), b.parse::<i128>()) {
        (Ok(a), Ok(b)) => a <= b,
        _ => a.parse::<f64>().unwrap_or(f64::NAN) <= b.parse::<f64>().unwrap_or(f64::NAN),
    }
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
#[derive(Default, Debug, Clone, PartialEq)]
struct RouteFacts {
    at: String,
    shapes: BTreeMap<String, String>,
    accepts: BTreeSet<String>,
}

#[derive(Default, Debug, Clone, PartialEq)]
struct TypeFacts {
    direction: Option<Direction>,
    kind: String,
    open: bool,
    extends: Option<String>,
    branches: Option<String>,
    fields: BTreeMap<String, (bool, String)>,
    values: BTreeSet<String>,
}

#[derive(Default, Debug, Clone)]
pub struct Facts {
    routes: BTreeMap<String, RouteFacts>,
    types: BTreeMap<String, TypeFacts>,
    /// `(path, kind)` → its value (`""` for `unique`).
    bounds: BTreeMap<(String, String), String>,
}

const BOUND_KINDS: [&str; 9] = ["range", "length", "items", "props", "unique", "pattern", "format", "multiple", "default"];

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
            ["route", op, "accepts", scheme] => {
                f.routes.entry(op.to_string()).or_default().accepts.insert(scheme.to_string());
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
            ["branches", name, kind] => {
                f.types.entry(name.to_string()).or_default().branches = Some(kind.to_string());
            }
            ["bound", path, kind, value @ ..] if BOUND_KINDS.contains(kind) => {
                let value = value.first().copied().unwrap_or_default().to_string();
                if matches!(*kind, "range" | "length" | "items" | "props") && Range::parse(&value).is_none() {
                    bail!(bad());
                }
                f.bounds.insert((path.to_string(), kind.to_string()), value);
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

fn type_line(name: &str, t: &TypeFacts) -> String {
    format!("type {name} {} {}{}", t.direction.map(Direction::as_str).unwrap_or("both"), t.kind, if t.open { " open" } else { "" })
}

fn split_null(t: &str) -> (&str, bool) {
    match t.strip_suffix("|null") {
        Some(core) => (core, true),
        None => (t, false),
    }
}

/// Each shape as a reader reads it: one that extends another (a `$ref`
/// beside its own fields, as schemars writes a newtype variant) holds the
/// other's fields, branches, bounds and inner shapes too, its own first.
/// So a shape written inline in one release and by reference in the next
/// compares by what it holds, not by how it is written.
fn resolved(facts: &Facts) -> Facts {
    let mut f = facts.clone();
    let names: Vec<String> = f.types.keys().cloned().collect();
    for name in names {
        let mut seen = BTreeSet::new();
        let mut next = f.types[&name].extends.clone();
        while let Some(from) = next {
            if !seen.insert(from.clone()) {
                break;
            }
            let direction = f.types[&name].direction;
            let inner = |n: &str| n == from || n.strip_prefix(from.as_str()).is_some_and(|r| r.starts_with('.'));
            let copies: Vec<(String, TypeFacts)> = f.types.iter().filter(|(n, _)| inner(n)).map(|(n, t)| (format!("{name}{}", &n[from.len()..]), t.clone())).collect();
            for (to, t) in copies {
                let target = f.types.entry(to.clone()).or_insert_with(|| TypeFacts { direction, kind: t.kind.clone(), open: t.open, ..TypeFacts::default() });
                for (field, held) in &t.fields {
                    target.fields.entry(field.clone()).or_insert_with(|| held.clone());
                }
                target.values.extend(t.values.iter().cloned());
                if target.branches.is_none() {
                    target.branches = t.branches.clone();
                }
            }
            let bounds: Vec<((String, String), String)> =
                f.bounds.iter().filter(|((path, _), _)| inner(path)).map(|((path, kind), v)| ((format!("{name}{}", &path[from.len()..]), kind.clone()), v.clone())).collect();
            for (key, v) in bounds {
                f.bounds.entry(key).or_insert(v);
            }
            next = f.types.get(&from).and_then(|t| t.extends.clone());
        }
    }
    f
}

/// What `head` breaks of `base`, but those `retired` names: each shape as
/// its readers read it (`resolved`).
pub fn breaches(base: &Facts, head: &Facts, retired: &BTreeSet<String>) -> Vec<Breach> {
    let (base, head) = (&resolved(base), &resolved(head));
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
        for scheme in was.accepts.difference(&now.accepts) {
            breach(format!("route {op} accepts {scheme}"), "the route no longer takes this credential: an older client still sends it");
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
        let dir = direction(was, now);
        let read = dir.reads_as_out();
        let written = dir.reads_as_in();
        if was.kind.trim_end_matches(" closed") != now.kind.trim_end_matches(" closed") {
            breach(type_line(name, was), "the shape is another kind now: an older client decodes the kind it was");
            continue;
        }
        let closed_was = was.kind.ends_with(" closed");
        let closed_now = now.kind.ends_with(" closed");
        // an old reader refuses what it does not know until its line is retired
        let old_reader_closed = closed_was && !retired.contains(&type_line(name, was));
        if written && closed_now && !closed_was {
            breach(type_line(name, now), "it refuses fields it does not know now: an older client that sends one is refused");
        }
        if read && closed_was && !closed_now {
            breach(type_line(name, was), "it no longer refuses fields it does not know, but its older readers still do: retire this line once they are gone");
        }
        if read && now.open && !was.open {
            breach(type_line(name, now), "it is open now, but an older client reads it closed: a value it does not know fails it");
        }
        if was.branches != now.branches {
            let line = format!("branches {name} {}", was.branches.as_deref().or(now.branches.as_deref()).unwrap_or_default());
            breach(line, "its branches are told apart another way now: an older client tells them apart the way it did");
        }
        let field_line = |f: &str, (req, t): &(bool, String)| format!("field {name}.{f} {} {t}", if *req { "required" } else { "optional" });
        for (f, was_f) in &was.fields {
            let line = field_line(f, was_f);
            let Some(now_f) = now.fields.get(f) else {
                breach(line, "the field is gone (removed or renamed): an older client sends or reads it");
                continue;
            };
            if now_f.1 != was_f.1 {
                let ((core_was, null_was), (core_now, null_now)) = (split_null(&was_f.1), split_null(&now_f.1));
                if core_was != core_now {
                    breach(line, "the field's type changed: an older client sends or reads the type it had");
                    continue;
                }
                if null_now && read {
                    breach(line, "the field may be null now: an older client reads it as never null");
                    continue;
                }
                if null_was && written {
                    breach(line, "the field refuses null now: an older client that sends null is refused");
                    continue;
                }
            }
            if was_f.0 && !now_f.0 && read {
                breach(line, "the field may be absent now: an older client reads it as always there");
            } else if !was_f.0 && now_f.0 && written {
                breach(line, "the field is required now: an older client that leaves it out is refused");
            }
        }
        for (f, now_f) in now.fields.iter().filter(|(f, _)| !was.fields.contains_key(*f)) {
            if now_f.0 && written {
                breach(field_line(f, now_f), "a new required field: an older client does not send it");
            } else if read && (old_reader_closed || closed_now) {
                breach(field_line(f, now_f), "a new field of a closed shape: an older client that refuses fields it does not know fails on it");
            }
        }
        for v in was.values.difference(&now.values) {
            breach(format!("value {name} {v}"), "the value or branch is gone: an older client sends or expects it");
        }
        if read && !now.open {
            for v in now.values.difference(&was.values) {
                breach(format!("value {name} {v}"), "a new value or branch of a closed shape: an older client fails on one it does not know");
            }
        }
    }
    let keys: BTreeSet<&(String, String)> = base.bounds.keys().chain(head.bounds.keys()).collect();
    for key in keys {
        let (path, kind) = key;
        let (was, now) = (base.bounds.get(key), head.bounds.get(key));
        if was == now {
            continue;
        }
        let line = |v: &String| format!("bound {path} {kind}{}", if v.is_empty() { String::new() } else { format!(" {v}") });
        let fact = was.map(line).or(now.map(line)).expect("one of them holds it");
        let Some((owner_name, dir)) = owner(path, base, head) else {
            breach(fact, "a bound of a shape the lock does not name changed");
            continue;
        };
        // a shape or field new in this change has no older client
        let field = path.strip_prefix(owner_name.as_str()).and_then(|rest| rest.strip_prefix('.')).map(|rest| rest.split('.').next().unwrap_or(rest));
        let existed = base.types.get(owner_name.as_str()).is_some_and(|t| field.is_none_or(|f| t.fields.contains_key(f)));
        if !existed {
            continue;
        }
        let (read, written) = (dir.reads_as_out(), dir.reads_as_in());
        match kind.as_str() {
            "range" | "length" | "items" | "props" => {
                let open = Range::parse(if kind == "range" { ".." } else { "0.." }).expect("an open range parses");
                let r = |v: Option<&String>| v.and_then(|v| Range::parse(v)).unwrap_or_else(|| open.clone());
                let (was_r, now_r) = (r(was), r(now));
                if read && !was_r.holds(&now_r) {
                    breach(fact, "the bound is wider now: an older client reads a value it does not take");
                } else if written && !now_r.holds(&was_r) {
                    breach(fact, "the bound is narrower now: an older client sends a value the cell no longer takes");
                }
            }
            "default" => {
                // it changes what leaving the field out means, where it may be left out
                // only a field may be left out; one master holds as required never was
                let optional = field_of(path, base).is_some_and(|(required, _)| !required);
                if optional {
                    breach(fact, "the default changed: an older client that leaves the field out, or reads it left out, means the one it had");
                }
            }
            _ => {
                // pattern, format, multiple, unique: none is the widest; others compare only as equal
                match (was, now) {
                    (Some(_), None) if read => breach(fact, "the bound is gone: an older client reads a value it does not take"),
                    (None, Some(_)) if written => breach(fact, "a new bound: an older client sends a value the cell no longer takes"),
                    (Some(_), Some(_)) => breach(fact, "the bound changed: an older client sends or reads values of the one it had"),
                    _ => {}
                }
            }
        }
    }
    out
}

/// Who writes a shape, as master and this change say: both, where they
/// differ, so the stricter rules hold.
fn direction(was: &TypeFacts, now: &TypeFacts) -> Direction {
    let d = |t: &TypeFacts| t.direction.unwrap_or(Direction::Both);
    if d(was) == d(now) {
        d(was)
    } else {
        Direction::Both
    }
}

/// The shape a bound's path is in, and who writes it: its longest prefix
/// either lock names as a type.
fn owner(path: &str, base: &Facts, head: &Facts) -> Option<(String, Direction)> {
    let mut at = path;
    loop {
        match (base.types.get(at), head.types.get(at)) {
            (Some(a), Some(b)) => return Some((at.to_string(), direction(a, b))),
            (Some(t), None) | (None, Some(t)) => return Some((at.to_string(), t.direction.unwrap_or(Direction::Both))),
            (None, None) => at = at.rsplit_once('.')?.0,
        }
    }
}

/// The field a bound's path is, as `facts` holds it: `<type>.<field>`.
fn field_of<'f>(path: &str, facts: &'f Facts) -> Option<&'f (bool, String)> {
    let (name, field) = path.rsplit_once('.')?;
    facts.types.get(name)?.fields.get(field)
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
