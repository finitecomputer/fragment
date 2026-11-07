//! What the wire's schema says of fields left out, held to serde: each
//! route's own types read samples of the shapes the schema gives them
//! (`routes::Ty`). It runs after the source guard (source.rs), so every
//! type it reads is one whose schema schemars derived from serde's
//! supported attributes, or one of `HAND_WRITTEN`. Every conclusion stands
//! on a control that rules out another cause; a control that cannot be
//! met refuses generation, naming the lock's field (type, branch, field).
//!
//! Supported:
//! - **a stated default** (`extend("default" = …)`) of an optional field,
//!   held in each condition branch that lets the field be left out;
//! - **a forbidding** (`false` in a condition branch) at a `FORBIDDINGS`
//!   site alone, of a field read as any JSON.
//!
//! | inference | evidence | control (`Controls`) |
//! |-----------|----------|----------------------|
//! | D. omitted `F` reads as its stated default `D`, in context `C` | `C` without `F` and `C` with `F = D` read the same | D1 both read (`samples_read`); D2 the same by `Debug` as well as JSON, which hides what `skip_serializing_if` drops (`by_debug`); D3 `C` reads `F`: a value of a kind `F`'s schema does not admit is refused there, or, `F` any JSON, two kinds read apart (`field_read`); D4 `C` is each branch that lets `F` be left out (`every_branch`); N1 none of those branches has branches of its own, else refused (`flat_conditions`) |
//! | F. branch `B` forbids `F` | `B` with `F` set to one value of each JSON kind is refused, in each of `B`'s nested contexts | F0 the site is one of `FORBIDDINGS` (`forbidding_listed`); F1 each context of `B` reads as it is (`branch_reads`); F2 `F`'s schema admits any JSON (`field_any_json`); F3 a branch that allows `F` reads every kind of it, each apart (`allowing_reads_any`); F4 every nested context of `B`, not its first (`every_leaf`) |
//! | H. every default and forbidding the lock records was read | the sites of the probes that passed | one no probe reaches fails `hold` (`unheld`), a route parameter's default among them |
//!
//! A sample is the least the schema asks (its required fields, constants,
//! a union's first branch, an `examples` value first). `Debug` shows a
//! read value faithfully as `#[derive(Debug)]` writes it: crates/proto has
//! no `Debug` written by hand.

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

/// The forbiddings the wire holds, each with the proto test that holds its
/// reader to its schema: a refusal's `result`, read through a `Value` by
/// `AnswerFrame` (`HAND_WRITTEN`), so any JSON, and refused beside a
/// refusal by `Answer`'s `TryFrom`, whatever the value.
pub const FORBIDDINGS: [(&str, &str); 1] = [("LiveOut.result.#1.result", "a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it")];

/// The controls each inference stands on (the module's table): all on,
/// but in the mutation tests, which turn one off to show its fixture then
/// passes, so each is what fails it.
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    pub samples_read: bool,
    pub by_debug: bool,
    pub field_read: bool,
    pub every_branch: bool,
    pub flat_conditions: bool,
    pub forbidding_listed: bool,
    pub branch_reads: bool,
    pub field_any_json: bool,
    pub allowing_reads_any: bool,
    pub every_leaf: bool,
    /// The sites a forbidding may stand at (`FORBIDDINGS`', or a test's).
    pub forbiddings: &'static [&'static str],
}

impl Controls {
    pub const ALL: Controls = Controls {
        samples_read: true,
        by_debug: true,
        field_read: true,
        every_branch: true,
        flat_conditions: true,
        forbidding_listed: true,
        branch_reads: true,
        field_any_json: true,
        allowing_reads_any: true,
        every_leaf: true,
        forbiddings: &[FORBIDDINGS[0].0],
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

/// Whether a field is read where a sample sets it (D3).
#[derive(Debug)]
enum Reach {
    /// Set to a value of a kind its schema does not admit: must be refused.
    Refused(Value),
    /// Any JSON: set to one value of each kind; two must read apart.
    Apart(Vec<Value>),
}

/// What a probe concludes, and the samples of the root its evidence and
/// its controls read.
#[derive(Debug)]
enum Claim {
    /// The field left out reads as this default.
    Default { default: Value, without: Value, with: Value, reach: Reach },
    /// A condition branch forbids the field: each of its contexts as it
    /// is, then with the field set to each kind; a branch that allows the
    /// field, with it set to each kind.
    Forbidden { contexts: Vec<Value>, trials: Vec<Value>, allowing: Vec<Value> },
}

/// A conclusion about one field.
#[derive(Debug)]
struct Probe {
    claim: Claim,
    /// Where the field is in the root's sample.
    path: String,
    /// The lock's name of the field (`<shape>.<field>`).
    site: String,
}

impl Probe {
    /// The probe of a sample nested in a larger one: `f` puts each of its
    /// samples where it stands in the larger.
    fn lift(self, path: String, f: &dyn Fn(Value) -> Value) -> Probe {
        let all = |vs: Vec<Value>| vs.into_iter().map(f).collect();
        let claim = match self.claim {
            Claim::Default { default, without, with, reach } => Claim::Default {
                default,
                without: f(without),
                with: f(with),
                reach: match reach {
                    Reach::Refused(v) => Reach::Refused(f(v)),
                    Reach::Apart(vs) => Reach::Apart(all(vs)),
                },
            },
            Claim::Forbidden { contexts, trials, allowing } => Claim::Forbidden { contexts: all(contexts), trials: all(trials), allowing: all(allowing) },
        };
        Probe { claim, path, site: self.site }
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
            Claim::Default { default, without: out, with: has, reach } => {
                match (round_trip(out.clone()), round_trip(has.clone())) {
                    (Ok(without), Ok(with)) => {
                        let same = without.0 == with.0 && (!controls.by_debug || without.1 == with.1);
                        if !same {
                            bail!("{label}: `{path}` left out is not its stated default {default}, beside {out}: serde reads {} of it left out and {} of it as its default ({at})", without.1, with.1);
                        }
                    }
                    (Err(_), Err(_)) if !controls.samples_read => {}
                    (Err(e), _) => bail!("{label}: a sample of it with `{path}` left out does not read ({out}): {e}; give the type serde refuses there an `examples` value it reads ({at})"),
                    (_, Err(e)) => bail!("{label}: a sample of it with `{path}` its stated default does not read ({has}): {e}; give the type serde refuses there an `examples` value it reads ({at})"),
                }
                if controls.field_read {
                    reached(label, path, reach, round_trip, &at)?;
                }
            }
            Claim::Forbidden { contexts, trials, allowing } => {
                if controls.branch_reads {
                    for c in contexts {
                        if let Err(e) = round_trip(c.clone()) {
                            bail!("{label}: a sample of the branch that forbids `{path}` does not read ({c}): {e}; give the type serde refuses there an `examples` value it reads, or the forbidding cannot be held ({at})");
                        }
                    }
                }
                for t in trials {
                    if round_trip(t.clone()).is_ok() {
                        bail!("{label}: a condition branch forbids `{path}`, but serde reads it there ({t}): leaving it out there means a value its reader fills in, which no fact holds; give each branch its own fields ({at})");
                    }
                }
                if controls.allowing_reads_any {
                    let mut shown = BTreeSet::new();
                    for a in allowing {
                        match round_trip(a.clone()) {
                            Ok((_, d)) => {
                                shown.insert(d);
                            }
                            Err(e) => bail!("{label}: where a branch allows `{path}`, it is not read as any JSON ({a}: {e}), so its refusal where it is forbidden may be its type's: only a field read as any JSON may be forbidden ({at})"),
                        }
                    }
                    if shown.len() < allowing.len() {
                        bail!("{label}: where a branch allows `{path}`, two kinds of it read the same, so it is not read as any JSON: only a field read as any JSON may be forbidden ({at})");
                    }
                }
            }
        }
    }
    Ok(probes.into_iter().map(|p| p.site).collect())
}

/// That a sample reads the field where it sets it (D3).
fn reached(label: &str, path: &str, reach: &Reach, round_trip: RoundTrip, at: &str) -> Result<()> {
    match reach {
        Reach::Refused(v) => {
            if round_trip(v.clone()).is_ok() {
                bail!("{label}: `{path}` is not read where its default is held: a value of a kind its schema does not admit reads ({v}), so what it is there is no evidence; the schema and the type disagree ({at})");
            }
        }
        Reach::Apart(samples) => {
            let read: BTreeSet<String> = samples.iter().filter_map(|v| round_trip(v.clone()).ok()).map(|(_, shown)| shown).collect();
            if read.len() < 2 {
                bail!("{label}: `{path}` is not shown to be read where its default is held: no two kinds of value read apart; give its schema `examples` values serde reads ({at})");
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
        obj = leaves(&obj, first, props, defs, depth + 1)?.swap_remove(0);
    }
    Ok(obj)
}

/// A condition branch's requirements laid on a sample of its object, once
/// for each branch nested in it, to its leaves: a sample per context.
fn leaves(obj: &Map<String, Value>, b: &Value, parent: Option<&Map<String, Value>>, defs: &Map<String, Value>, depth: usize) -> Result<Vec<Map<String, Value>>> {
    anyhow::ensure!(depth <= DEPTH_MAX, "a sample nested past {DEPTH_MAX}");
    let mut obj = obj.clone();
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
    let Some(inner) = b.as_object().and_then(branches) else { return Ok(vec![obj]) };
    let mut merged = parent.cloned().unwrap_or_default();
    merged.extend(own.cloned().unwrap_or_default());
    let mut all = vec![];
    for i in inner {
        all.extend(leaves(&obj, i, Some(&merged), defs, depth + 1)?);
    }
    anyhow::ensure!(!all.is_empty(), "a union has a branch");
    Ok(all)
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
    vec![Value::Null, json!(true), json!(0), json!(1.5), json!("x"), json!([]), json!({})]
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
        // a sample of the object as each condition branch beside it has
        // it, in each context nested in it
        let in_branch = |b: &Value| -> Result<Vec<Map<String, Value>>> {
            let mut base_b = base.clone();
            for (k, _) in base.iter().filter(|(k, _)| !required.contains(k.as_str())) {
                base_b.remove(k);
            }
            leaves(&base_b, b, props, defs, depth + 1)
        };
        for (name, ps) in props.into_iter().flatten() {
            if let (false, Some(d)) = (required.contains(name.as_str()), ps.get("default")) {
                // left out wherever a branch lets it be (D4), whether the branch
                // names it or tells instances apart by another field: each such
                // branch's sample, for each may read it its own way
                let mut contexts: Vec<Map<String, Value>> = match branches(m) {
                    Some(bs) => {
                        let permitting: Vec<&Value> = bs.iter().filter(|b| permits_absence(b, name)).collect();
                        // N1: a branch with branches of its own has contexts its first
                        // nested one does not stand for
                        if controls.flat_conditions && permitting.iter().any(|b| b.as_object().and_then(branches).is_some()) {
                            bail!("{site}.{name}: a default its object states once, beside a condition branch with branches of its own: the checker reads it in each branch's context, not in each nested one's, so it is refused; state it in each branch's own fields (xtask/src/wire/defaults.rs)");
                        }
                        permitting.into_iter().map(|b| in_branch(b).map(|mut l| l.swap_remove(0))).collect::<Result<_>>()?
                    }
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
                        claim: Claim::Default { default: d.clone(), without: Value::Object(without), with: Value::Object(with), reach: reach_of(&ctx, name, ps, defs)? },
                        path: name.clone(),
                        site: format!("{site}.{name}"),
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
            for (i, b) in bs.iter().enumerate() {
                let key = branch_key(b, tag.as_deref(), i);
                let contexts = in_branch(b)?;
                let base_b = contexts[0].clone();
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
                // each field it forbids of those its object describes
                for (name, _) in b["properties"].as_object().into_iter().flatten().filter(|(_, ps)| **ps == Value::Bool(false)) {
                    let at = format!("{site}.{key}.{name}");
                    out.push(forbidding(&at, name, props, bs, &contexts, &in_branch, defs, controls, format!("{key}.{name}"))?);
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

/// A sample of an object in each context of a condition branch.
type InBranch<'a> = &'a dyn Fn(&Value) -> Result<Vec<Map<String, Value>>>;

/// A forbidding's probe (F): at a listed site, of a field any JSON, its
/// branch's every context with the field set to each kind, and a branch
/// that allows it with the same.
#[allow(clippy::too_many_arguments)]
fn forbidding(
    at: &str,
    name: &str,
    props: Option<&Map<String, Value>>,
    bs: &[Value],
    contexts: &[Map<String, Value>],
    in_branch: InBranch,
    defs: &Map<String, Value>,
    controls: Controls,
    path: String,
) -> Result<Probe> {
    if controls.forbidding_listed && !controls.forbiddings.contains(&at) {
        bail!("{at}: a condition branch forbids it, and the wire holds a forbidding at FORBIDDINGS' sites alone, each a field a reader takes as any JSON and its branch refuses whatever it holds (xtask/src/wire/defaults.rs)");
    }
    let described = props.and_then(|pp| pp.get(name)).with_context(|| format!("{at}: forbidden, though its object does not describe it"))?;
    if controls.field_any_json && admitted(described, defs, 0)?.is_some() {
        bail!("{at}: forbidden, though its field is not any JSON ({described}): a refusal of values tried is no evidence that the branch refuses every value its own reader takes, so only a field any JSON may be forbidden (xtask/src/wire/defaults.rs)");
    }
    let allowing = bs
        .iter()
        .find(|other| other["properties"][name] != Value::Bool(false))
        .with_context(|| format!("{at}: every branch forbids it, so it cannot be shown read anywhere: give each branch its own fields (xtask/src/wire/defaults.rs)"))?;
    let base_a = in_branch(allowing)?.swap_remove(0);
    let set = |base: &Map<String, Value>, v: Value| {
        let mut o = base.clone();
        o.insert(name.to_string(), v);
        Value::Object(o)
    };
    let mut contexts = contexts.to_vec();
    if !controls.every_leaf {
        contexts.truncate(1);
    }
    let trials = contexts.iter().flat_map(|c| every_kind().into_iter().map(|v| set(c, v))).collect();
    let allowing = every_kind().into_iter().map(|v| set(&base_a, v)).collect();
    Ok(Probe { claim: Claim::Forbidden { contexts: contexts.into_iter().map(Value::Object).collect(), trials, allowing }, path, site: at.to_string() })
}

/// A component's name, as `$ref` names it.
fn component(r: &str) -> Result<&str> {
    r.strip_prefix(COMPONENTS).with_context(|| format!("{r} names a component"))
}

/// A branch's key in the lock: its tag's value, else its place.
fn branch_key(b: &Value, tag: Option<&str>, i: usize) -> String {
    tag.and_then(|t| super::lock::tag_value(b, t)).unwrap_or_else(|| format!("#{i}"))
}
