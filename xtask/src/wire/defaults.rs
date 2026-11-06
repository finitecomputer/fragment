//! Each default the wire states is what serde reads of its field left out
//! (lock.rs asks one of every optional field whose absence nothing else
//! says): held here to each route's own types. For every optional field
//! with a default, anywhere in a route's body, answer or frames, a sample
//! of the route's type is read and written again by serde twice, once
//! without the field and once with its default, and the two must be the
//! same. A sample is the least the schema asks (its required fields,
//! constants, a union's first branch); one serde refuses is an error that
//! names it, as is any default that lies.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail, Context, Result};
use fragment_proto::routes::{ty, Answer, ROUTES};
use fragment_proto::ErrorBody;
use serde_json::{json, Map, Value};

use super::openapi::COMPONENTS;

/// The deepest a sample or a probe goes into nested shapes.
const DEPTH_MAX: usize = 16;

type RoundTrip = fn(Value) -> std::result::Result<Value, String>;

/// Every route's types, and the refusal every route may answer: how many
/// defaults were held.
pub fn hold(doc: &Value) -> Result<usize> {
    let defs = doc["components"]["schemas"].as_object().context("the document has components")?;
    let mut ops: BTreeMap<&str, &Value> = BTreeMap::new();
    for item in doc["paths"].as_object().context("the document has paths")?.values() {
        for op in item.as_object().into_iter().flat_map(|m| m.values()) {
            ops.insert(op["operationId"].as_str().context("an operation is named")?, op);
        }
    }
    let mut held = 0;
    for route in ROUTES {
        let op = ops.get(route.operation).with_context(|| format!("{} is in the document", route.operation))?;
        let mut each: Vec<(&str, RoundTrip, Option<&Value>)> = vec![];
        if let Some(t) = route.request {
            each.push(("its body", t.round_trip, op.pointer("/requestBody/content/application~1json/schema")));
        }
        match route.answer {
            Answer::Json(t) => each.push(("its answer", t.round_trip, op.pointer("/responses/200/content/application~1json/schema"))),
            Answer::Socket { sends, receives } => {
                if let Some(t) = sends {
                    each.push(("the frames it takes", t.round_trip, op.pointer("/x-fragment-socket/sends")));
                }
                each.push(("the frames it sends", receives.round_trip, op.pointer("/x-fragment-socket/receives")));
            }
        }
        for (what, round_trip, schema) in each {
            let label = format!("{} {what}", route.operation);
            held += check(&label, schema.with_context(|| format!("{label} is in the document"))?, defs, round_trip)?;
        }
    }
    let refusal = json!({ "$ref": format!("{COMPONENTS}ErrorBody") });
    held += check("a refusal", &refusal, defs, ty::<ErrorBody>().round_trip)?;
    Ok(held)
}

/// One field left out, and given its default, in a sample of the root.
#[derive(Debug)]
struct Probe {
    with: Value,
    without: Value,
    path: String,
    default: Value,
}

/// That each default under `schema` is what `round_trip` reads of its
/// field left out: how many were held.
pub fn check(label: &str, schema: &Value, defs: &Map<String, Value>, round_trip: RoundTrip) -> Result<usize> {
    let probes = probes(schema, defs, 0)?;
    for p in &probes {
        let read = |v: &Value| round_trip(v.clone()).map_err(|e| anyhow!("{label}: a sample of it does not read ({v}): {e}; give its type an `examples` serde reads"));
        let (without, with) = (read(&p.without)?, read(&p.with)?);
        if without != with {
            bail!("{label}: `{}` left out is not its stated default {}: serde reads {without} of the one and {with} of the other", p.path, p.default);
        }
    }
    Ok(probes.len())
}

fn target<'d>(r: &str, defs: &'d Map<String, Value>) -> Result<&'d Value> {
    let name = r.strip_prefix(COMPONENTS).with_context(|| format!("{r} names a component"))?;
    defs.get(name).with_context(|| format!("{name} is a component"))
}

fn types(m: &Map<String, Value>) -> Vec<&str> {
    match m.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    }
}

fn branches(m: &Map<String, Value>) -> Option<&Vec<Value>> {
    m.get("oneOf").or(m.get("anyOf")).and_then(Value::as_array)
}

fn is_null(s: &Value) -> bool {
    s.get("type") == Some(&json!("null"))
}

/// An object schema: one with fields, or a condition's requirements.
fn objectish(m: &Map<String, Value>) -> bool {
    m.contains_key("properties") || m.contains_key("required") || (types(m).contains(&"object") && !m.get("additionalProperties").is_some_and(Value::is_object))
}

/// The least instance `s` asks for.
fn sample(s: &Value, defs: &Map<String, Value>, depth: usize) -> Result<Value> {
    anyhow::ensure!(depth <= DEPTH_MAX, "a sample nested past {DEPTH_MAX}");
    let Some(m) = s.as_object() else { return Ok(Value::Null) };
    if let Some(example) = m.get("examples").and_then(|e| e.get(0)) {
        return Ok(example.clone());
    }
    if let Some(c) = m.get("const") {
        return Ok(c.clone());
    }
    if let Some(first) = m.get("enum").and_then(Value::as_array).and_then(|e| e.first()) {
        return Ok(first.clone());
    }
    if types(m).contains(&"null") {
        return Ok(Value::Null);
    }
    if objectish(m) {
        return Ok(Value::Object(sample_object(m, defs, depth)?));
    }
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        return sample(target(r, defs)?, defs, depth + 1);
    }
    if let Some(bs) = branches(m) {
        if bs.iter().any(is_null) {
            return Ok(Value::Null);
        }
        return sample(bs.first().context("a union has a branch")?, defs, depth + 1);
    }
    Ok(match types(m).first().copied() {
        Some("array") => json!([]),
        Some("object") => json!({}),
        Some("string") => json!("x"),
        Some("integer") => {
            let lo = m.get("minimum").and_then(Value::as_i64).unwrap_or(0).max(0);
            json!(m.get("maximum").and_then(Value::as_i64).map_or(lo, |hi| lo.min(hi)))
        }
        Some("number") => json!(0.0),
        Some("boolean") => json!(false),
        _ => Value::Null,
    })
}

fn sample_object(m: &Map<String, Value>, defs: &Map<String, Value>, depth: usize) -> Result<Map<String, Value>> {
    let mut obj = match m.get("$ref").and_then(Value::as_str) {
        Some(r) => sample(target(r, defs)?, defs, depth + 1)?.as_object().cloned().unwrap_or_default(),
        None => Map::new(),
    };
    let props = m.get("properties").and_then(Value::as_object);
    for name in m.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        let s = props.and_then(|p| p.get(name)).unwrap_or(&Value::Bool(true));
        obj.insert(name.to_string(), sample(s, defs, depth + 1)?);
    }
    if let Some(first) = branches(m).and_then(|b| b.first()) {
        apply_branch(&mut obj, first, props, defs, depth + 1)?;
    }
    Ok(obj)
}

/// A condition branch's requirements laid on a sample of its object.
fn apply_branch(obj: &mut Map<String, Value>, b: &Value, parent: Option<&Map<String, Value>>, defs: &Map<String, Value>, depth: usize) -> Result<()> {
    anyhow::ensure!(depth <= DEPTH_MAX, "a sample nested past {DEPTH_MAX}");
    let own = b["properties"].as_object();
    for name in b["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let s = own.and_then(|p| p.get(name)).filter(|s| **s != Value::Bool(false)).or(parent.and_then(|p| p.get(name))).unwrap_or(&Value::Bool(true));
        obj.insert(name.to_string(), sample(s, defs, depth + 1)?);
    }
    for (name, s) in own.into_iter().flatten() {
        if s.get("const").is_some() || s.get("enum").is_some() {
            obj.insert(name.clone(), sample(s, defs, depth + 1)?);
        }
    }
    if let Some(inner) = b.as_object().and_then(branches).and_then(|bs| bs.first()) {
        let mut merged = parent.cloned().unwrap_or_default();
        merged.extend(own.cloned().unwrap_or_default());
        apply_branch(obj, inner, Some(&merged), defs, depth + 1)?;
    }
    Ok(())
}

/// Each optional field with a default under `s`, as a probe of a sample
/// of `s`.
fn probes(s: &Value, defs: &Map<String, Value>, depth: usize) -> Result<Vec<Probe>> {
    let Some(m) = s.as_object() else { return Ok(vec![]) };
    if depth > DEPTH_MAX {
        return Ok(vec![]);
    }
    let mut out = vec![];
    if objectish(m) {
        let base = sample_object(m, defs, depth)?;
        let props = m.get("properties").and_then(Value::as_object);
        let required: BTreeSet<&str> = m.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let conditioned: BTreeSet<&str> = branches(m).into_iter().flatten().flat_map(|b| b["required"].as_array().into_iter().flatten().filter_map(Value::as_str)).collect();
        for (name, ps) in props.into_iter().flatten() {
            if let (false, false, Some(d)) = (required.contains(name.as_str()), conditioned.contains(name.as_str()), ps.get("default")) {
                let (mut with, mut without) = (base.clone(), base.clone());
                with.insert(name.clone(), d.clone());
                without.remove(name);
                out.push(Probe { with: Value::Object(with), without: Value::Object(without), path: name.clone(), default: d.clone() });
            }
            for p in probes(ps, defs, depth + 1)? {
                let (mut with, mut without) = (base.clone(), base.clone());
                with.insert(name.clone(), p.with);
                without.insert(name.clone(), p.without);
                out.push(Probe { with: Value::Object(with), without: Value::Object(without), path: format!("{name}.{}", p.path), default: p.default });
            }
        }
        // what it extends, its own fields beside each probe of it
        if let Some(r) = m.get("$ref").and_then(Value::as_str) {
            let own: Map<String, Value> = base.iter().filter(|(k, _)| props.is_some_and(|p| p.contains_key(*k))).map(|(k, v)| (k.clone(), v.clone())).collect();
            for p in probes(target(r, defs)?, defs, depth + 1)? {
                let merge = |v: Value| {
                    let mut o = v.as_object().cloned().unwrap_or_default();
                    o.extend(own.clone());
                    Value::Object(o)
                };
                out.push(Probe { with: merge(p.with), without: merge(p.without), path: p.path, default: p.default });
            }
        }
        // each condition branch's own data: its fields its object does not describe
        for (i, b) in branches(m).into_iter().flatten().enumerate() {
            let mut base_b = base.clone();
            for (k, _) in base.iter().filter(|(k, _)| !required.contains(k.as_str())) {
                base_b.remove(k);
            }
            apply_branch(&mut base_b, b, props, defs, depth + 1)?;
            for (name, ps) in b["properties"].as_object().into_iter().flatten().filter(|(k, _)| !props.is_some_and(|p| p.contains_key(*k))) {
                let required_b = b["required"].as_array().is_some_and(|r| r.iter().any(|x| x == name.as_str()));
                if let (false, Some(d)) = (required_b, ps.get("default")) {
                    let (mut with, mut without) = (base_b.clone(), base_b.clone());
                    with.insert(name.clone(), d.clone());
                    without.remove(name);
                    out.push(Probe { with: Value::Object(with), without: Value::Object(without), path: format!("#{i}.{name}"), default: d.clone() });
                }
            }
        }
        return Ok(out);
    }
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        return probes(target(r, defs)?, defs, depth + 1);
    }
    if let Some(bs) = branches(m) {
        for b in bs.iter().filter(|b| !is_null(b)) {
            out.extend(probes(b, defs, depth + 1)?);
        }
        return Ok(out);
    }
    if let Some(items) = m.get("items") {
        for p in probes(items, defs, depth + 1)? {
            out.push(Probe { with: json!([p.with]), without: json!([p.without]), path: format!("[].{}", p.path), default: p.default });
        }
    }
    if let Some(value) = m.get("additionalProperties").filter(|v| v.is_object()) {
        for p in probes(value, defs, depth + 1)? {
            out.push(Probe { with: json!({ "k": p.with }), without: json!({ "k": p.without }), path: format!("{{}}.{}", p.path), default: p.default });
        }
    }
    Ok(out)
}
