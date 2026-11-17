//! Cloudflare's Workers Observability query API, faked over HTTP: the
//! operators' glance asks it (`fragment_core::glance`; cell/src/glance.rs)
//! at `CLOUDFLARE_API_URL`. It holds log lines a test plants (`log`), each
//! a JSON object at a time, and answers `POST
//! /accounts/<account>/workers/observability/telemetry/query` as the real
//! one does for the queries the glance makes: the lines in the timeframe
//! that pass every filter (`eq` on a string, `lt`/`gt` on a number, a key
//! dotted into the line's objects), and one calculation over them, `uniq`
//! of a key or `count`, grouped by a key or not. Only its account and its
//! key are let in. Lever: the next query is refused (`fail_next`).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

#[derive(Default)]
struct State {
    /// Each line, and when it was written (ms).
    lines: Vec<(i64, Value)>,
    /// The next query's refusal: its status and message.
    fail_next: Option<(u16, String)>,
    /// The queries asked, as their bodies.
    asked: Vec<Value>,
}

pub struct Observability {
    pub url: String,
    state: Arc<Mutex<State>>,
    _server: Server,
}

/// A line's value at a dotted key (`phase.phase`), or at a key as it is
/// (`$workers.entrypoint` is one key, as the real one names it).
fn at<'a>(line: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(v) = line.get(key) {
        return Some(v);
    }
    key.split('.').try_fold(line, |v, k| v.get(k))
}

fn passes(line: &Value, filter: &Value) -> bool {
    let (Some(key), Some(op)) = (filter["key"].as_str(), filter["operation"].as_str()) else { return false };
    let v = at(line, key);
    match op {
        "eq" => v.is_some_and(|v| *v == filter["value"]),
        "lt" => v.and_then(Value::as_f64).zip(filter["value"].as_f64()).is_some_and(|(v, w)| v < w),
        "gt" => v.and_then(Value::as_f64).zip(filter["value"].as_f64()).is_some_and(|(v, w)| v > w),
        _ => false,
    }
}

/// A query's answer over `lines`, as the real one shapes it.
fn answer(lines: &[(i64, Value)], q: &Value) -> Result<Value, String> {
    let (from, to) = (q["timeframe"]["from"].as_i64().ok_or("no timeframe")?, q["timeframe"]["to"].as_i64().ok_or("no timeframe")?);
    let p = &q["parameters"];
    let filters = p["filters"].as_array().cloned().unwrap_or_default();
    let calc = p["calculations"].as_array().and_then(|c| c.first()).ok_or("no calculation")?;
    let group = p["groupBys"].as_array().and_then(|g| g.first()).and_then(|g| g["value"].as_str());
    let matched: Vec<&Value> = lines.iter().filter(|(t, l)| (from..=to).contains(t) && filters.iter().all(|f| passes(l, f))).map(|(_, l)| l).collect();
    let value = |ls: &[&Value]| -> Result<u64, String> {
        match calc["operator"].as_str() {
            Some("count") => Ok(ls.len() as u64),
            Some("uniq") => {
                let key = calc["key"].as_str().ok_or("uniq names its key")?;
                let mut seen: Vec<&Value> = ls.iter().filter_map(|l| at(l, key)).collect();
                seen.sort_by_key(|v| v.to_string());
                seen.dedup();
                Ok(seen.len() as u64)
            }
            other => Err(format!("no operator {other:?} here")),
        }
    };
    let aggregates = match group {
        None if matched.is_empty() => vec![],
        None => vec![json!({ "groups": [], "value": value(&matched)?, "count": matched.len() })],
        Some(key) => {
            let mut keys: Vec<String> = matched.iter().filter_map(|l| at(l, key).and_then(Value::as_str).map(str::to_string)).collect();
            keys.sort();
            keys.dedup();
            let limit = q["limit"].as_u64().unwrap_or(50) as usize;
            let mut out = vec![];
            for k in keys.into_iter().take(limit) {
                let of: Vec<&Value> = matched.iter().copied().filter(|l| at(l, key).and_then(Value::as_str) == Some(k.as_str())).collect();
                out.push(json!({ "groups": [{ "key": key, "value": k }], "value": value(&of)?, "count": of.len(), "groupKey": k }));
            }
            out
        }
    };
    Ok(json!({ "success": true, "errors": [], "result": { "run": {}, "statistics": {}, "calculations": [{ "calculation": calc["operator"], "alias": calc["alias"], "aggregates": aggregates }] } }))
}

impl Observability {
    /// On any port, letting in `account`'s queries made with `key` alone.
    pub fn start(account: &str, key: &str) -> std::io::Result<Observability> {
        let state: Arc<Mutex<State>> = Arc::default();
        let st = Arc::clone(&state);
        let (path, bearer) = (format!("/accounts/{account}/workers/observability/telemetry/query"), format!("Bearer {key}"));
        let refused = |status: u16, message: &str| Response::json(status, &json!({ "success": false, "errors": [{ "code": 10000, "message": message }], "result": null }));
        let handler: Handler = Arc::new(move |req: &Request| {
            if (req.method.as_str(), req.path.as_str()) != ("POST", path.as_str()) {
                return refused(404, "no such route");
            }
            if req.header("authorization") != Some(bearer.as_str()) {
                return refused(403, "Authentication error");
            }
            let mut s = st.lock().expect("observability state");
            if let Some((status, message)) = s.fail_next.take() {
                return refused(status, &message);
            }
            let q: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            s.asked.push(q.clone());
            match answer(&s.lines, &q) {
                Ok(a) => Response::json(200, &a),
                Err(why) => refused(400, &why),
            }
        });
        let server = Server::start(0, handler)?;
        Ok(Observability { url: server.url.clone(), state, _server: server })
    }

    /// A line written at `at_ms`, as a Worker logs one (its JSON's keys,
    /// and the runtime's own `$workers.*`).
    pub fn log(&self, at_ms: i64, line: Value) {
        self.state.lock().expect("observability state").lines.push((at_ms, line));
    }

    /// The next query is refused with `status` and `message`.
    pub fn fail_next(&self, status: u16, message: &str) {
        self.state.lock().expect("observability state").fail_next = Some((status, message.to_string()));
    }

    /// The queries asked, oldest first.
    pub fn asked(&self) -> Vec<Value> {
        self.state.lock().expect("observability state").asked.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_793_800_000_000;

    /// A Worker's line: its own keys, and the runtime's `$workers.scriptName`.
    fn of(script: &str, mut line: Value) -> Value {
        line["$workers.scriptName"] = json!(script);
        line
    }

    fn lines() -> Vec<(i64, Value)> {
        let ours = |line: Value| of("fragment", line);
        // another deployment's in the same account (a branch's): never counted
        let theirs = |line: Value| of("fragment-rh", line);
        vec![
            (T - 60_000, ours(json!({ "computer": "computer:a", "phase": { "phase": "awake" } }))),
            (T - 30_000, ours(json!({ "computer": "computer:a", "phase": { "phase": "awake" } }))),
            (T - 3_600_000, ours(json!({ "computer": "computer:b", "phase": { "phase": "awake" } }))),
            (T - 1_000, ours(json!({ "computer": "computer:c", "phase": { "phase": "failed" } }))),
            (T - 1_000, ours(json!({ "computer": "computer:d", "disk": { "total": 20, "free": 1_000_000_000_u64 } }))),
            (T - 1_000, ours(json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": "f1" }))),
            (T - 2_000, ours(json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": "f1" }))),
            (T - 3_000, ours(json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": "f2" }))),
            (T - 1_000, theirs(json!({ "computer": "computer:e", "phase": { "phase": "awake" } }))),
            (T - 1_000, theirs(json!({ "computer": "computer:a", "phase": { "phase": "failed" } }))),
            (T - 1_000, theirs(json!({ "computer": "computer:a", "event": { "event": "save_failed" } }))),
            (T - 1_000, theirs(json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": "f9" }))),
        ]
    }

    /// Goal: the glance's own queries answer as the real one would: each
    /// question's body (fragment_core::glance) over planted lines, another
    /// deployment's in the same account never counted (its computers are
    /// hashes of the same people's).
    #[test]
    fn the_glances_queries_answer_over_their_lines() {
        use fragment_core::glance::{by_computer_of, uniq_of, Question};
        let day = T - 5 * 3_600_000;
        let ask = |q: Question| answer(&lines(), &q.body("fragment", T, day)).unwrap();
        assert_eq!(uniq_of(&ask(Question::AwakeNow)), Ok(1), "a, twice; b an hour ago");
        assert_eq!(uniq_of(&ask(Question::AwakeToday)), Ok(2));
        assert_eq!(by_computer_of(&ask(Question::AwakeWeek)), Ok(vec![("computer:a".into(), 2), ("computer:b".into(), 1)]));
        assert_eq!(uniq_of(&ask(Question::FragmentsNow)), Ok(2));
        assert_eq!(by_computer_of(&ask(Question::WontWake)), Ok(vec![("computer:c".into(), 1)]));
        assert_eq!(by_computer_of(&ask(Question::DiskFilling)), Ok(vec![("computer:d".into(), 1)]));
        assert_eq!(by_computer_of(&ask(Question::SavesFailing)), Ok(vec![]), "the other deployment's failed save is none of ours");
    }
}
