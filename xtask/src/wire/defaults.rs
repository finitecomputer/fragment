//! What the wire's schema says of fields left out, held to serde: each
//! route's own types read samples of the shapes the schema gives them
//! (`routes::Ty`), and every conclusion drawn from a reading stands on a
//! control that rules out another cause. A control that cannot be met
//! refuses generation, naming the type, branch and field, and asks for an
//! `examples` value serde reads (or a tag). The inferences:
//!
//! | inference | evidence | control (`Controls`) |
//! |-----------|----------|----------------------|
//! | D. omitted `F` reads as its stated default `D`, in context `C` | `C` without `F` and `C` with `F = D` read the same | D1 both read (`samples_read`); D2 the same by `Debug` as well as by JSON, which hides what `skip_serializing_if` drops (`by_debug`); D3 `C` reads `F`: a value of a kind `F`'s schema does not admit is refused there, or, `F` being any JSON, two kinds read apart (`field_read`); D4 `C` is each branch that lets `F` be left out (`every_branch`) |
//! | B. a sample of a union's branch is read as that branch | its tag, its key, or its conditions on one object's fields | B1 a union whose branches bring fields of their own that neither a tag nor a key of each branch's own tells apart (an untagged enum's), which serde reads by trying each branch in turn, holds no default or forbidding (`branch_is_its_own`) |
//! | F. branch `B` forbids `F` | `B` with `F = X` is refused, for each `X` tried | F1 `B` as it is reads (`branch_reads`); F2 some `X` reads in a branch `A` that allows `F` (`value_reads`); F3 `A` reads `F`, as D3 (`context_reads_field`); F4 the `X` tried: one of each kind of JSON, `F`'s sample, and every value its schema lists, bounds to a few, or gives as `examples` (`every_value`); no branch allows `F`: refused |
//! | H. every default and forbidding the lock records is held | the sites of the probes that passed | each such fact is a site; one no probe reaches fails `hold` (`unheld`), a route parameter's default among them (no type reads one) |
//!
//! A sample is the least the schema asks (its required fields, constants,
//! a union's first branch, an `examples` value first). `Debug` shows a
//! read value faithfully as `#[derive(Debug)]` writes it: crates/proto
//! has no `Debug` written by hand.
//!
//! The evidence is samples, so one assumption stands under every row: a
//! reader treats the instances it is not shown as it treats those it is,
//! within a branch and a kind. serde's derived readers do (a default is its
//! variant's and its field's; a field is read by its type), as do the
//! readers written by hand behind the wire's own types (`live.rs`'s two
//! frames, which agreement tests hold to their schemas; `ErrorBody.reason`,
//! read as a string or none).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use fragment_proto::routes::{ty, Answer, RoundTrip, ROUTES};
use fragment_proto::ErrorBody;
use serde_json::{json, Map, Value};

use super::openapi::COMPONENTS;

/// The deepest a sample or a probe goes into nested shapes; deeper is an
/// error, never a subtree counted as held. A component met again on its
/// own path (a recursive type) is not entered twice: its defaults are
/// held where it was first met.
const DEPTH_MAX: usize = 64;

/// The controls each inference stands on (the module's table): all on,
/// but in the mutation tests, which turn one off to show its fixture then
/// passes, so each is what fails it.
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    pub samples_read: bool,
    pub by_debug: bool,
    pub field_read: bool,
    pub every_branch: bool,
    pub branch_is_its_own: bool,
    pub branch_reads: bool,
    pub value_reads: bool,
    pub context_reads_field: bool,
    pub every_value: bool,
}

impl Controls {
    pub const ALL: Controls = Controls {
        samples_read: true,
        by_debug: true,
        field_read: true,
        every_branch: true,
        branch_is_its_own: true,
        branch_reads: true,
        value_reads: true,
        context_reads_field: true,
        every_value: true,
    };
}

/// Every route's types, and the refusal every route may answer; then that
/// every default and forbidding `lock` records was held, so no shape a
/// route reaches goes unread: the lock's fields that were held.
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
    let missed = unheld(lock, &held);
    if !missed.is_empty() {
        bail!("defaults or forbiddings no route's sample reaches, so serde never read them (a route parameter's default among them: no type reads one) (xtask/src/wire/defaults.rs): {}", missed.join(", "));
    }
    Ok(held)
}

/// The fields `lock` records an optional field's default of, or a
/// condition's forbidding of (`never`), that `held` does not name: a
/// route parameter's default too, which no type reads (H).
pub fn unheld(lock: &str, held: &BTreeSet<String>) -> Vec<String> {
    let optional: BTreeSet<&str> = lock.lines().filter_map(|l| l.strip_prefix("field ")).filter_map(|l| l.split_once(" optional ").map(|(p, _)| p)).collect();
    let defaulted = lock.lines().filter_map(|l| l.strip_prefix("bound ")).filter_map(|l| l.split_once(" default ").map(|(p, _)| p)).filter(|p| optional.contains(p));
    let forbidden = lock.lines().filter_map(|l| l.strip_prefix("field ")).filter_map(|l| l.strip_suffix(" optional never"));
    let mut missed: Vec<String> = defaulted.chain(forbidden).filter(|p| !held.contains(*p)).map(str::to_string).collect();
    missed.sort();
    missed.dedup();
    missed
}

/// What a probe concludes, if its samples read as it says.
#[derive(Debug)]
enum Claim {
    /// The field left out reads as this default.
    Default(Value),
    /// A condition branch forbids the field.
    Forbidden,
}

/// Whether a field is read where a sample sets it.
#[derive(Debug)]
enum Reach {
    /// Set to a value of a kind its schema does not admit: must be refused.
    Refused(Value),
    /// Any JSON: set to one value of each kind; two must read apart.
    Apart(Vec<Value>),
}

impl Reach {
    fn map(self, f: &dyn Fn(Value) -> Value) -> Reach {
        match self {
            Reach::Refused(v) => Reach::Refused(f(v)),
            Reach::Apart(vs) => Reach::Apart(vs.into_iter().map(f).collect()),
        }
    }
}

/// A conclusion about one field, and the samples of the root its evidence
/// and its controls read.
#[derive(Debug)]
struct Probe {
    claim: Claim,
    /// Where the field is in the root's sample.
    path: String,
    /// The lock's name of the field (`<shape>.<field>`).
    site: String,
    /// A default's: its context without the field. A forbidding's: the
    /// forbidding branch's sample as it is.
    without: Value,
    /// A default's: its context with the field its default.
    with: Value,
    /// A default's: whether its context reads the field (D3). A
    /// forbidding's: whether the allowing context does (F3).
    reach: Reach,
    /// A forbidding's: for each value tried, the forbidding branch with it,
    /// and the allowing context with it.
    trials: Vec<(Value, Value)>,
}

impl Probe {
    /// The probe of a sample nested in a larger one: `f` puts each of its
    /// samples where it stands in the larger.
    fn lift(self, path: String, f: &dyn Fn(Value) -> Value) -> Probe {
        Probe {
            claim: self.claim,
            path,
            site: self.site,
            without: f(self.without),
            with: f(self.with),
            reach: self.reach.map(f),
            trials: self.trials.into_iter().map(|(b, a)| (f(b), f(a))).collect(),
        }
    }
}

/// That each default under `schema` is what `round_trip` reads of its
/// field left out, and each forbidding holds: the lock's fields held.
pub fn check(label: &str, schema: &Value, defs: &Map<String, Value>, round_trip: RoundTrip) -> Result<BTreeSet<String>> {
    check_with(label, schema, defs, round_trip, Controls::ALL)
}

/// `check`, with the controls given: all on, but in the mutation tests.
pub fn check_with(label: &str, schema: &Value, defs: &Map<String, Value>, round_trip: RoundTrip, controls: Controls) -> Result<BTreeSet<String>> {
    let probes = probes(schema, "", defs, &mut Walk { depth: 0, components: vec![], controls })?;
    for p in &probes {
        // where it is: the lock's field (type, branch, field), and this file
        let at = format!("the lock's {}; xtask/src/wire/defaults.rs", p.site);
        let path = &p.path;
        match &p.claim {
            Claim::Default(default) => {
                match (round_trip(p.without.clone()), round_trip(p.with.clone())) {
                    (Ok(without), Ok(with)) => {
                        let same = without.0 == with.0 && (!controls.by_debug || without.1 == with.1);
                        if !same {
                            bail!("{label}: `{path}` left out is not its stated default {default}, beside {}: serde reads {} of it left out and {} of it as its default ({at})", p.without, without.1, with.1);
                        }
                    }
                    (Err(_), Err(_)) if !controls.samples_read => {}
                    (Err(e), _) => bail!("{label}: a sample of it with `{path}` left out does not read ({}): {e}; give the type serde refuses there an `examples` value it reads ({at})", p.without),
                    (_, Err(e)) => bail!("{label}: a sample of it with `{path}` its stated default does not read ({}): {e}; give the type serde refuses there an `examples` value it reads ({at})", p.with),
                }
                if controls.field_read {
                    reached(label, path, &p.reach, round_trip, "where its default is held", &at)?;
                }
            }
            Claim::Forbidden => {
                if controls.branch_reads {
                    if let Err(e) = round_trip(p.without.clone()) {
                        bail!("{label}: a sample of the branch that forbids `{path}` does not read ({}): {e}; give the type serde refuses there an `examples` value it reads, or the forbidding cannot be held ({at})", p.without);
                    }
                }
                let mut shown = false;
                for (forbidding, allowing) in &p.trials {
                    if round_trip(forbidding.clone()).is_ok() {
                        bail!("{label}: a condition branch forbids `{path}`, but serde reads it there ({forbidding}): leaving it out there means a value its reader fills in, which no fact holds; give each branch its own fields ({at})");
                    }
                    shown |= !controls.value_reads || round_trip(allowing.clone()).is_ok();
                }
                if !shown {
                    bail!("{label}: none of the values tried for `{path}` reads where a branch allows it, so its refusal in the branch that forbids it may be the value's own: give its schema `examples` values serde reads ({at})");
                }
                if controls.context_reads_field {
                    reached(label, path, &p.reach, round_trip, "where a branch allows it", &at)?;
                }
            }
        }
    }
    Ok(probes.into_iter().map(|p| p.site).collect())
}

/// That a sample reads the field where it sets it (D3, F3).
fn reached(label: &str, path: &str, reach: &Reach, round_trip: RoundTrip, there: &str, at: &str) -> Result<()> {
    match reach {
        Reach::Refused(v) => {
            if round_trip(v.clone()).is_ok() {
                bail!("{label}: `{path}` is not read {there}: a value of a kind its schema does not admit reads ({v}), so what it is there is no evidence; the schema and the type disagree ({at})");
            }
        }
        Reach::Apart(samples) => {
            let read: BTreeSet<String> = samples.iter().filter_map(|v| round_trip(v.clone()).ok()).map(|(_, shown)| shown).collect();
            if read.len() < 2 {
                bail!("{label}: `{path}` is not shown to be read {there}: no two kinds of value read apart; give its schema `examples` values serde reads ({at})");
            }
        }
    }
    Ok(())
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

/// The kinds of JSON value a schema admits: none for any JSON.
fn admitted(s: &Value, defs: &Map<String, Value>, depth: usize) -> Result<Option<BTreeSet<&'static str>>> {
    anyhow::ensure!(depth <= DEPTH_MAX, "a schema nested past {DEPTH_MAX}");
    let Some(m) = s.as_object() else { return Ok(if *s == Value::Bool(false) { Some(BTreeSet::new()) } else { None }) };
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        if !m.contains_key("properties") {
            return admitted(target(r, defs)?, defs, depth + 1);
        }
    }
    let values: Vec<&Value> = m.get("const").into_iter().chain(m.get("enum").and_then(Value::as_array).into_iter().flatten()).collect();
    if !values.is_empty() {
        return Ok(Some(values.into_iter().map(kind_of).collect()));
    }
    if let Some(bs) = branches(m) {
        if !m.contains_key("properties") && !m.contains_key("type") {
            let mut all = BTreeSet::new();
            for b in bs {
                match admitted(b, defs, depth + 1)? {
                    Some(k) => all.extend(k),
                    None => return Ok(None),
                }
            }
            return Ok(Some(all));
        }
    }
    let mut kinds: BTreeSet<&'static str> = types(m)
        .into_iter()
        .flat_map(|t| match t {
            "number" => vec!["number", "integer"],
            "null" => vec!["null"],
            "boolean" => vec!["boolean"],
            "integer" => vec!["integer"],
            "string" => vec!["string"],
            "array" => vec!["array"],
            "object" => vec!["object"],
            _ => vec![],
        })
        .collect();
    if m.contains_key("properties") || m.contains_key("required") || m.contains_key("additionalProperties") {
        kinds.insert("object");
    }
    if m.contains_key("items") {
        kinds.insert("array");
    }
    Ok(if kinds.is_empty() || kinds.len() >= 7 { None } else { Some(kinds) })
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// One value of each kind of JSON.
fn every_kind() -> Vec<Value> {
    vec![Value::Null, json!(true), json!(0), json!("x"), json!([]), json!({})]
}

/// Whether a sample reads a field it sets to these: a value of a kind the
/// field's schema does not admit, else, any JSON, one of each kind.
fn reach_of(ctx: &Map<String, Value>, name: &str, field: &Value, defs: &Map<String, Value>) -> Result<Reach> {
    let set = |v: Value| {
        let mut o = ctx.clone();
        o.insert(name.to_string(), v);
        Value::Object(o)
    };
    Ok(match admitted(field, defs, 0)? {
        None => Reach::Apart(every_kind().into_iter().map(set).collect()),
        Some(kinds) => {
            let wrong = [json!(1.5), json!("~a value of another kind~"), json!(true), json!([]), json!({}), Value::Null].into_iter().find(|v| !kinds.contains(kind_of(v)));
            match wrong {
                Some(v) => Reach::Refused(set(v)),
                None => Reach::Apart(every_kind().into_iter().map(set).collect()),
            }
        }
    })
}

/// The values a forbidding is tried with (F4): one of each kind of JSON,
/// the field's least sample, and every value its schema lists or bounds
/// to a few (constants, enums, booleans, null, a small integer range) or
/// gives as `examples`, through its references and unions.
fn tried(field: &Value, defs: &Map<String, Value>, controls: Controls) -> Result<Vec<Value>> {
    if !controls.every_value {
        return Ok(vec![sample(field, defs, 0)?]);
    }
    let mut all = every_kind();
    all.push(sample(field, defs, 0)?);
    listed(field, defs, 0, &mut all)?;
    let mut seen = BTreeSet::new();
    all.retain(|v| seen.insert(v.to_string()));
    Ok(all)
}

fn listed(s: &Value, defs: &Map<String, Value>, depth: usize, out: &mut Vec<Value>) -> Result<()> {
    anyhow::ensure!(depth <= DEPTH_MAX, "a schema nested past {DEPTH_MAX}");
    let Some(m) = s.as_object() else { return Ok(()) };
    out.extend(m.get("examples").and_then(Value::as_array).into_iter().flatten().cloned());
    out.extend(m.get("const").cloned());
    out.extend(m.get("enum").and_then(Value::as_array).into_iter().flatten().cloned());
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        listed(target(r, defs)?, defs, depth + 1, out)?;
    }
    for b in branches(m).into_iter().flatten() {
        listed(b, defs, depth + 1, out)?;
    }
    for t in types(m) {
        match t {
            "boolean" => out.extend([json!(false), json!(true)]),
            "integer" => {
                if let (Some(lo), Some(hi)) = (m.get("minimum").and_then(Value::as_i64), m.get("maximum").and_then(Value::as_i64)) {
                    if hi >= lo && hi - lo < 16 {
                        out.extend((lo..=hi).map(|n| json!(n)));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether a union is one serde reads by trying each branch in turn: its
/// branches bring fields of their own (or a type by reference), and
/// neither a tag nor a key of each branch's own tells them apart (an
/// untagged enum's `anyOf`), so a sample of one branch may be read as
/// another and no reading of it is evidence of its own branch's.
fn tried_in_turn(m: &Map<String, Value>, bs: &[Value]) -> bool {
    let parent = m.get("properties").and_then(Value::as_object);
    let own = |b: &&Value| b.get("$ref").is_some() || b["properties"].as_object().is_some_and(|p| p.keys().any(|k| !parent.is_some_and(|pp| pp.contains_key(k))));
    let data: Vec<&Value> = bs.iter().filter(|b| !is_null(b)).filter(own).collect();
    if data.is_empty() || bs.iter().filter(|b| !is_null(b)).count() < 2 || super::lock::tag_of(bs).is_some() {
        return false;
    }
    // an externally tagged enum's: each branch one key of its own, and closed
    fn key(b: &Value) -> Option<&str> {
        match (b["required"].as_array().map(Vec::as_slice), b["properties"].as_object()) {
            (Some([k]), Some(p)) if p.len() == 1 && b["additionalProperties"] == false => k.as_str(),
            _ => None,
        }
    }
    let keys: Option<BTreeSet<&str>> = data.iter().map(|b| key(b)).collect();
    !keys.is_some_and(|k| k.len() == data.len())
}

/// That a union's branch holds nothing to check, unless serde reads each
/// branch as its own (B1).
fn own_branch(m: &Map<String, Value>, bs: &[Value], found: &[Probe], site: &str, controls: Controls) -> Result<()> {
    if controls.branch_is_its_own && !found.is_empty() && tried_in_turn(m, bs) {
        let names: Vec<&str> = found.iter().map(|p| p.site.as_str()).collect();
        bail!("{site}: an untagged union (no tag or key tells its branches apart) holds {}: serde reads the first branch that reads, so a sample of one branch may be read as another and no default or forbidding in it can be held; tag it (`#[serde(tag = ...)]`) (xtask/src/wire/defaults.rs)", names.join(", "));
    }
    Ok(())
}

/// Each optional field with a default, and each a condition forbids,
/// under `s`, as a probe of a sample of `s`; `site` is the lock's name of
/// the shape `s` is (a component, a branch `X.<tag>` or `X.#<i>`, a
/// field's inline shape `X.f`), so each probe names the lock's field.
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

/// Where a walk of probes is: how deep, the components on its path, and
/// the controls it makes probes for.
struct Walk {
    depth: usize,
    components: Vec<String>,
    controls: Controls,
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
/// require it (one that forbids it is held as a forbidding).
fn permits_absence(b: &Value, field: &str) -> bool {
    !b["required"].as_array().is_some_and(|r| r.iter().any(|x| x == field))
}

fn probes_of(m: &Map<String, Value>, site: &str, defs: &Map<String, Value>, walk: &mut Walk) -> Result<Vec<Probe>> {
    let depth = walk.depth;
    let controls = walk.controls;
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
                // left out wherever a branch lets it be (D4), whether the branch
                // names it or tells instances apart by another field: each such
                // branch's sample, for each may read it its own way
                let mut contexts: Vec<Map<String, Value>> = match branches(m) {
                    Some(bs) => bs.iter().filter(|b| permits_absence(b, name)).map(&in_branch).collect::<Result<_>>()?,
                    None => vec![base.clone()],
                };
                if !controls.every_branch {
                    contexts.truncate(1);
                }
                for ctx in contexts {
                    let (mut with, mut without) = (ctx.clone(), ctx.clone());
                    with.insert(name.clone(), d.clone());
                    without.remove(name);
                    out.push(Probe {
                        claim: Claim::Default(d.clone()),
                        path: name.clone(),
                        site: format!("{site}.{name}"),
                        without: Value::Object(without),
                        with: Value::Object(with),
                        reach: reach_of(&ctx, name, ps, defs)?,
                        trials: vec![],
                    });
                }
            }
            for p in probes(ps, &format!("{site}.{name}"), defs, walk)? {
                let path = format!("{name}.{}", p.path);
                let at = |v: Value| {
                    let mut o = base.clone();
                    o.insert(name.clone(), v);
                    Value::Object(o)
                };
                out.push(p.lift(path, &at));
            }
        }
        // what it extends, its own fields beside each probe of it
        if let Some(r) = m.get("$ref").and_then(Value::as_str) {
            let own: Map<String, Value> = base.iter().filter(|(k, _)| props.is_some_and(|p| p.contains_key(*k))).map(|(k, v)| (k.clone(), v.clone())).collect();
            for p in probes_ref(r, defs, walk)? {
                let path = p.path.clone();
                let merge = |v: Value| {
                    let mut o = v.as_object().cloned().unwrap_or_default();
                    o.extend(own.clone());
                    Value::Object(o)
                };
                out.push(p.lift(path, &merge));
            }
        }
        // each condition branch's own data, nested shapes and all: each probe
        // of the branch, read beside the fields of the object it stands in
        if let Some(bs) = branches(m) {
            let tag = super::lock::tag_of(bs);
            let before = out.len();
            for (i, b) in bs.iter().enumerate() {
                let key = branch_key(b, tag.as_deref(), i);
                let base_b = in_branch(b)?;
                for p in probes(b, &format!("{site}.{key}"), defs, walk)? {
                    let field = p.path.split('.').next().unwrap_or(&p.path).to_string();
                    // a predicate on a field its object describes is no data of its own
                    if props.is_some_and(|pp| pp.contains_key(&field)) {
                        continue;
                    }
                    let path = format!("{key}.{}", p.path);
                    let lift = |v: Value| {
                        let o = v.as_object().cloned().unwrap_or_default();
                        let mut lifted = base_b.clone();
                        if !o.contains_key(&field) {
                            lifted.remove(&field);
                        }
                        lifted.extend(o);
                        Value::Object(lifted)
                    };
                    out.push(p.lift(path, &lift));
                }
                // each field it forbids of those its object describes: refused
                // there with each value tried, each of which reads where a branch
                // allows the field, a branch that reads it
                for (name, _) in b["properties"].as_object().into_iter().flatten().filter(|(_, ps)| **ps == Value::Bool(false)) {
                    let at = format!("{site}.{key}.{name}");
                    let described = props.and_then(|pp| pp.get(name)).with_context(|| format!("{at}: forbidden, though its object does not describe it"))?;
                    let allowing = bs
                        .iter()
                        .find(|other| other["properties"][name.as_str()] != Value::Bool(false))
                        .with_context(|| format!("{at}: every branch forbids it, so no value of it can be shown to read: give each branch its own fields (xtask/src/wire/defaults.rs)"))?;
                    let base_a = in_branch(allowing)?;
                    let set = |base: &Map<String, Value>, v: &Value| {
                        let mut o = base.clone();
                        o.insert(name.clone(), v.clone());
                        Value::Object(o)
                    };
                    let trials = tried(described, defs, controls)?.iter().map(|x| (set(&base_b, x), set(&base_a, x))).collect();
                    out.push(Probe {
                        claim: Claim::Forbidden,
                        path: format!("{key}.{name}"),
                        site: at,
                        without: Value::Object(base_b.clone()),
                        with: Value::Null,
                        reach: reach_of(&base_a, name, described, defs)?,
                        trials,
                    });
                }
            }
            own_branch(m, bs, &out[before..], site, controls)?;
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
        own_branch(m, bs, &out, site, controls)?;
        return Ok(out);
    }
    if let Some(items) = m.get("items") {
        for p in probes(items, &format!("{site}.item"), defs, walk)? {
            let path = format!("[].{}", p.path);
            out.push(p.lift(path, &|v| json!([v])));
        }
    }
    if let Some(value) = m.get("additionalProperties").filter(|v| v.is_object()) {
        for p in probes(value, &format!("{site}.value"), defs, walk)? {
            let path = format!("{{}}.{}", p.path);
            out.push(p.lift(path, &|v| json!({ "k": v })));
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
