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

/// The deepest a sample or a probe goes into nested shapes; deeper is an
/// error, never a subtree counted as held. A component met again on its
/// own path (a recursive type) is not entered twice: its defaults are
/// held where it was first met.
const DEPTH_MAX: usize = 64;

type RoundTrip = fn(Value) -> std::result::Result<Value, String>;

/// Every route's types, and the refusal every route may answer; then that
/// every default `lock` records of an optional field was held, so no shape
/// a route reaches goes unread: the lock's fields whose defaults were held.
pub fn hold(doc: &Value, lock: &str) -> Result<BTreeSet<String>> {
    let defs = doc["components"]["schemas"].as_object().context("the document has components")?;
    let mut ops: BTreeMap<&str, &Value> = BTreeMap::new();
    for item in doc["paths"].as_object().context("the document has paths")?.values() {
        for op in item.as_object().into_iter().flat_map(|m| m.values()) {
            ops.insert(op["operationId"].as_str().context("an operation is named")?, op);
        }
    }
    let mut held = BTreeSet::new();
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
            held.extend(check(&label, schema.with_context(|| format!("{label} is in the document"))?, defs, round_trip)?);
        }
    }
    let refusal = json!({ "$ref": format!("{COMPONENTS}ErrorBody") });
    held.extend(check("a refusal", &refusal, defs, ty::<ErrorBody>().round_trip)?);
    let missed = unheld(lock, &held, ROUTES.iter().map(|r| r.operation));
    if !missed.is_empty() {
        bail!("defaults no route's sample reaches, so serde never read them (xtask/src/wire/defaults.rs): {}", missed.join(", "));
    }
    Ok(held)
}

/// The fields `lock` records an optional field's default of, or a
/// condition's forbidding of (`never`), that `held` does not name, but a
/// route's parameters (`<op>.path`, `<op>.query`), which no type reads.
pub fn unheld<'o>(lock: &str, held: &BTreeSet<String>, operations: impl Iterator<Item = &'o str>) -> Vec<String> {
    let params: Vec<String> = operations.flat_map(|op| [format!("{op}.path."), format!("{op}.query.")]).collect();
    let optional: BTreeSet<&str> = lock.lines().filter_map(|l| l.strip_prefix("field ")).filter_map(|l| l.split_once(" optional ").map(|(p, _)| p)).collect();
    let defaulted = lock.lines().filter_map(|l| l.strip_prefix("bound ")).filter_map(|l| l.split_once(" default ").map(|(p, _)| p)).filter(|p| optional.contains(p));
    let forbidden = lock.lines().filter_map(|l| l.strip_prefix("field ")).filter_map(|l| l.strip_suffix(" optional never"));
    let mut missed: Vec<String> =
        defaulted.chain(forbidden).filter(|p| !params.iter().any(|q| p.starts_with(q.as_str())) && !held.contains(*p)).map(str::to_string).collect();
    missed.sort();
    missed.dedup();
    missed
}

/// One field left out, and given its default, in a sample of the root.
#[derive(Debug)]
struct Probe {
    with: Value,
    without: Value,
    /// Where the field is in the root's sample.
    path: String,
    /// The lock's name of the field (`<shape>.<field>`).
    site: String,
    default: Value,
    /// A field a condition branch forbids: `with` holds it in a sample of
    /// that branch, which serde must refuse (else leaving it out there
    /// means a value its reader fills in), while `without`, the branch's
    /// sample as it is, must read: else the refusal could be another's.
    forbidden: bool,
}

/// That each default under `schema` is what `round_trip` reads of its
/// field left out: the lock's fields whose defaults were held.
pub fn check(label: &str, schema: &Value, defs: &Map<String, Value>, round_trip: RoundTrip) -> Result<BTreeSet<String>> {
    let probes = probes(schema, "", defs, &mut Walk::default())?;
    for p in &probes {
        if p.forbidden {
            // a refusal is evidence only beside a sample of the branch that reads
            if let Err(e) = round_trip(p.without.clone()) {
                bail!("{label}: a sample of the branch that forbids `{}` does not read ({}): {e}; give the type serde refuses there an `examples` value it reads, or the forbidding cannot be held (xtask/src/wire/defaults.rs)", p.path, p.without);
            }
            if round_trip(p.with.clone()).is_ok() {
                bail!("{label}: a condition branch forbids `{}`, but serde reads it there ({}): leaving it out there means a value its reader fills in, which no fact holds; give each branch its own fields (xtask/src/wire/defaults.rs)", p.path, p.with);
            }
            continue;
        }
        let read = |v: &Value| round_trip(v.clone()).map_err(|e| anyhow!("{label}: a sample of it does not read ({v}): {e}; give its type an `examples` serde reads"));
        let (without, with) = (read(&p.without)?, read(&p.with)?);
        if without != with {
            bail!("{label}: `{}` left out is not its stated default {}: serde reads {without} of the one and {with} of the other", p.path, p.default);
        }
    }
    Ok(probes.into_iter().map(|p| p.site).collect())
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
/// of `s`; `site` is the lock's name of the shape `s` is (a component, a
/// branch `X.<tag>` or `X.#<i>`, a field's inline shape `X.f`), so each
/// probe names the lock's field it holds.
fn probes(s: &Value, site: &str, defs: &Map<String, Value>, walk: &mut Walk) -> Result<Vec<Probe>> {
    let Some(m) = s.as_object() else { return Ok(vec![]) };
    if walk.depth >= DEPTH_MAX {
        bail!("{site}: nested past {DEPTH_MAX}, deeper than the default checker reads (xtask/src/wire/defaults.rs)");
    }
    walk.depth += 1;
    let found = probes_of(m, site, defs, walk);
    walk.depth -= 1;
    found
}

/// Where a walk of probes is: how deep, and the components on its path.
#[derive(Default)]
struct Walk {
    depth: usize,
    components: Vec<String>,
}

/// `probes` of a component by its reference, unless it is already on the
/// path (a recursive type, held where first met).
fn probes_ref(r: &str, defs: &Map<String, Value>, walk: &mut Walk) -> Result<Vec<Probe>> {
    let name = component(r)?;
    if walk.components.iter().any(|c| c == name) {
        return Ok(vec![]);
    }
    walk.components.push(name.to_string());
    let found = probes(target(r, defs)?, name, defs, walk);
    walk.components.pop();
    found
}

/// Whether a condition branch lets `field` be left out: it does not
/// require it (one that forbids it is refused before: lock.rs).
fn permits_absence(b: &Value, field: &str) -> bool {
    !b["required"].as_array().is_some_and(|r| r.iter().any(|x| x == field))
}

fn probes_of(m: &Map<String, Value>, site: &str, defs: &Map<String, Value>, walk: &mut Walk) -> Result<Vec<Probe>> {
    let depth = walk.depth;
    let mut out = vec![];
    if objectish(m) {
        let base = sample_object(m, defs, depth)?;
        let props = m.get("properties").and_then(Value::as_object);
        let required: BTreeSet<&str> = m.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        // a sample of the object as each condition branch beside it has it
        let in_branch = |b: &Value| -> Result<Map<String, Value>> {
            let mut base_b = base.clone();
            for (k, _) in base.iter().filter(|(k, _)| !required.contains(k.as_str())) {
                base_b.remove(k);
            }
            apply_branch(&mut base_b, b, props, defs, depth + 1)?;
            Ok(base_b)
        };
        for (name, ps) in props.into_iter().flatten() {
            if let (false, Some(d)) = (required.contains(name.as_str()), ps.get("default")) {
                // left out wherever a branch lets it be, whether the branch names
                // it or tells instances apart by another field: each such
                // branch's sample, for each may read it its own way
                let contexts: Vec<Map<String, Value>> = match branches(m) {
                    Some(bs) => bs.iter().filter(|b| permits_absence(b, name)).map(&in_branch).collect::<Result<_>>()?,
                    None => vec![base.clone()],
                };
                for ctx in contexts {
                    let (mut with, mut without) = (ctx.clone(), ctx);
                    with.insert(name.clone(), d.clone());
                    without.remove(name);
                    out.push(Probe { with: Value::Object(with), without: Value::Object(without), path: name.clone(), site: format!("{site}.{name}"), default: d.clone(), forbidden: false });
                }
            }
            for p in probes(ps, &format!("{site}.{name}"), defs, walk)? {
                let (mut with, mut without) = (base.clone(), base.clone());
                with.insert(name.clone(), p.with);
                without.insert(name.clone(), p.without);
                out.push(Probe { with: Value::Object(with), without: Value::Object(without), path: format!("{name}.{}", p.path), site: p.site, default: p.default, forbidden: p.forbidden });
            }
        }
        // what it extends, its own fields beside each probe of it
        if let Some(r) = m.get("$ref").and_then(Value::as_str) {
            let own: Map<String, Value> = base.iter().filter(|(k, _)| props.is_some_and(|p| p.contains_key(*k))).map(|(k, v)| (k.clone(), v.clone())).collect();
            for p in probes_ref(r, defs, walk)? {
                let merge = |v: Value| {
                    let mut o = v.as_object().cloned().unwrap_or_default();
                    o.extend(own.clone());
                    Value::Object(o)
                };
                out.push(Probe { with: merge(p.with), without: merge(p.without), path: p.path, site: p.site, default: p.default, forbidden: p.forbidden });
            }
        }
        // each condition branch's own data, nested shapes and all: each probe
        // of the branch, read beside the fields of the object it stands in
        if let Some(bs) = branches(m) {
            let tag = super::lock::tag_of(bs);
            for (i, b) in bs.iter().enumerate() {
                let key = branch_key(b, tag.as_deref(), i);
                let base_b = in_branch(b)?;
                for p in probes(b, &format!("{site}.{key}"), defs, walk)? {
                    let field = p.path.split('.').next().unwrap_or(&p.path).to_string();
                    // a predicate on a field its object describes is no data of its own
                    if props.is_some_and(|pp| pp.contains_key(&field)) {
                        continue;
                    }
                    let lift = |v: Value| {
                        let o = v.as_object().cloned().unwrap_or_default();
                        let mut lifted = base_b.clone();
                        if !o.contains_key(&field) {
                            lifted.remove(&field);
                        }
                        lifted.extend(o);
                        Value::Object(lifted)
                    };
                    out.push(Probe { with: lift(p.with), without: lift(p.without), path: format!("{key}.{}", p.path), site: p.site, default: p.default, forbidden: p.forbidden });
                }
                // each field it forbids of those its object describes, held
                // there by a sample serde must refuse
                for (name, _) in b["properties"].as_object().into_iter().flatten().filter(|(_, ps)| **ps == Value::Bool(false)) {
                    let described = props.and_then(|pp| pp.get(name)).with_context(|| format!("{site}.{key} forbids `{name}`, which its object does not describe"))?;
                    let mut with = base_b.clone();
                    with.insert(name.clone(), sample(described, defs, depth + 1)?);
                    out.push(Probe {
                        with: Value::Object(with),
                        without: Value::Object(base_b.clone()),
                        path: format!("{key}.{name}"),
                        site: format!("{site}.{key}.{name}"),
                        default: Value::Null,
                        forbidden: true,
                    });
                }
            }
        }
        return Ok(out);
    }
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        return probes_ref(r, defs, walk);
    }
    if let Some(bs) = branches(m) {
        let nullable = bs.len() == 2 && bs.iter().any(is_null);
        let tag = super::lock::tag_of(bs);
        for (i, b) in bs.iter().enumerate().filter(|(_, b)| !is_null(b)) {
            let at = if nullable { site.to_string() } else { format!("{site}.{}", branch_key(b, tag.as_deref(), i)) };
            out.extend(probes(b, &at, defs, walk)?);
        }
        return Ok(out);
    }
    if let Some(items) = m.get("items") {
        for p in probes(items, &format!("{site}.item"), defs, walk)? {
            out.push(Probe { with: json!([p.with]), without: json!([p.without]), path: format!("[].{}", p.path), site: p.site, default: p.default, forbidden: p.forbidden });
        }
    }
    if let Some(value) = m.get("additionalProperties").filter(|v| v.is_object()) {
        for p in probes(value, &format!("{site}.value"), defs, walk)? {
            out.push(Probe { with: json!({ "k": p.with }), without: json!({ "k": p.without }), path: format!("{{}}.{}", p.path), site: p.site, default: p.default, forbidden: p.forbidden });
        }
    }
    Ok(out)
}

/// A component's name, as `$ref` names it.
fn component(r: &str) -> Result<&str> {
    r.strip_prefix(COMPONENTS).with_context(|| format!("{r} names a component"))
}

/// A branch's key in the lock: its tag's value, else its place.
fn branch_key(b: &Value, tag: Option<&str>, i: usize) -> String {
    tag.and_then(|t| super::lock::tag_value(b, t)).unwrap_or_else(|| format!("#{i}"))
}
