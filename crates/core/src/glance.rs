//! The operators' glance (`GET /api/admin/glance`; docs/api.md,
//! Operators): what it asks the deployment's Workers Logs, through the
//! Workers Observability query API, and how it reads the answers. Each
//! number comes from lines the platform writes anyway, so no person's
//! object is called to count it (Paul, 2026-11-04: "if anything has to
//! route down to the users it's not worth building right now"):
//!
//! - a computer is awake when its lines say so (`"phase": {"phase":
//!   "awake"}`): an awake computer's meter alarm runs at least every
//!   `computer::METER_EVERY_MS`, and each event it applies writes a line;
//! - a fragment ran when its Durable Object (the `Fragment` class) did
//!   anything (`$workers.entrypoint`, `$workers.durableObjectId`);
//! - a computer is in trouble when its lines in the last day say it was
//!   failed (won't wake), that a save failed, or that its disk had less
//!   than `computer::DISK_TOLD_FREE_MAX` free.
//!
//! Every query counts this deployment's Worker's lines alone
//! (`$workers.scriptName`): an account holds other deployments (a
//! preview's account, its branches'), whose computers' ids are hashes of
//! the same people.
//!
//! Nothing is cached: the page asks again every minute.

use serde_json::Value;

use crate::computer::{DISK_TOLD_FREE_MAX, METER_EVERY_MS};

/// "Now": the last ten minutes, twice an awake computer's meter alarm.
pub const NOW_MS: i64 = 10 * 60_000;
/// A day, and a week (the logs keep seven days).
pub const DAY_MS: i64 = 24 * 3_600_000;
pub const WEEK_MS: i64 = 7 * DAY_MS;
/// The operator's midnight is at most this long ago (a day, and an hour
/// for a clock that moved).
pub const DAY_START_MAX_MS: i64 = DAY_MS + 3_600_000;
/// The newest people listed.
pub const NEW_PEOPLE_MAX: u32 = 50;
/// The groups one grouped query answers: computers awake in a week, or in
/// trouble in a day. A week with more is counted as this many, at least.
pub const GROUPS_MAX: u32 = 1_000;
/// Computers in trouble listed, the most lines first.
pub const TROUBLE_MAX: usize = 100;
/// What the queries are named in Cloudflare's own records.
pub const QUERY_ID: &str = "fragment-admin-glance";
/// A Worker's name, at most (Cloudflare's own limit).
pub const SCRIPT_MAX_BYTES: usize = 63;

/// Whether `name` is a Worker's name a query may name: 1 to
/// `SCRIPT_MAX_BYTES` of a-z, 0-9 and dashes inside (`fragment`,
/// `fragment-preview`), so it goes into a query as it is.
pub fn valid_script(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SCRIPT_MAX_BYTES
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

const _: () = assert!(NOW_MS >= 2 * METER_EVERY_MS, "an awake computer writes a line at least twice in a now");

/// Each question the glance asks, one query each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Question {
    /// Distinct computers awake in the last `NOW_MS`, and since the day's start.
    AwakeNow,
    AwakeToday,
    /// The computers awake in the last week, by computer: its count, and
    /// whether a new person's came up.
    AwakeWeek,
    /// Distinct fragments' objects that ran in the last `NOW_MS`, and since the day's start.
    FragmentsNow,
    FragmentsToday,
    /// The computers whose lines in the last day said each trouble, by computer.
    WontWake,
    SavesFailing,
    DiskFilling,
}

impl Question {
    pub const ALL: [Question; 8] = [
        Question::AwakeNow,
        Question::AwakeToday,
        Question::AwakeWeek,
        Question::FragmentsNow,
        Question::FragmentsToday,
        Question::WontWake,
        Question::SavesFailing,
        Question::DiskFilling,
    ];

    /// The query API's body for it, over `script`'s lines (this
    /// deployment's Worker), at `now_ms`, the day starting at
    /// `day_start_ms`: one template for all (a body of `json!` for each
    /// cost the cell's Wasm some 4 KB more).
    pub fn body(self, script: &str, now_ms: i64, day_start_ms: i64) -> Value {
        assert!(valid_script(script), "a Worker's name is checked as the config is read");
        assert!(day_start(Some(day_start_ms), now_ms).is_ok(), "the day's start is checked");
        // its one filter (key, operation, type, value as JSON), what it
        // counts (a distinct key, or none: lines by computer), and from when
        const AWAKE: (&str, &str, &str, &str) = ("phase.phase", "eq", "string", "\"awake\"");
        const FRAGMENT: (&str, &str, &str, &str) = ("$workers.entrypoint", "eq", "string", "\"Fragment\"");
        let disk = DISK_TOLD_FREE_MAX.to_string();
        let (filter, uniq, from) = match self {
            Question::AwakeNow => (AWAKE, Some("computer"), now_ms - NOW_MS),
            Question::AwakeToday => (AWAKE, Some("computer"), day_start_ms),
            Question::AwakeWeek => (AWAKE, None, now_ms - WEEK_MS),
            Question::FragmentsNow => (FRAGMENT, Some("$workers.durableObjectId"), now_ms - NOW_MS),
            Question::FragmentsToday => (FRAGMENT, Some("$workers.durableObjectId"), day_start_ms),
            Question::WontWake => (("phase.phase", "eq", "string", "\"failed\""), None, now_ms - DAY_MS),
            Question::SavesFailing => (("event.event", "eq", "string", "\"save_failed\""), None, now_ms - DAY_MS),
            Question::DiskFilling => (("disk.free", "lt", "number", disk.as_str()), None, now_ms - DAY_MS),
        };
        let (key, op, kind, value) = filter;
        let counted = match uniq {
            Some(k) => format!(r#""calculations":[{{"operator":"uniq","key":"{k}","alias":"n"}}]"#),
            None => r#""calculations":[{"operator":"count","alias":"n"}],"groupBys":[{"type":"string","value":"computer"}]"#.to_string(),
        };
        let body = format!(
            r#"{{"queryId":"{QUERY_ID}","timeframe":{{"from":{from},"to":{now_ms}}},"view":"calculations","limit":{GROUPS_MAX},"parameters":{{"datasets":["cloudflare-workers"],"filters":[{{"key":"$workers.scriptName","operation":"eq","type":"string","value":"{script}"}},{{"key":"{key}","operation":"{op}","type":"{kind}","value":{value}}}],{counted}}}}}"#
        );
        serde_json::from_slice(body.as_bytes()).expect("a glance's query is JSON")
    }
}

/// The aggregates of an answer's one calculation, or why it is none: the
/// API's own error, or a shape it does not have.
fn aggregates(answer: &Value) -> Result<&Vec<Value>, String> {
    if answer["success"] != true {
        let errors = answer["errors"].as_array().map(|e| e.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; ")).unwrap_or_default();
        return Err(if errors.is_empty() { "the logs answered no result".to_string() } else { errors });
    }
    match answer["result"]["calculations"].as_array().and_then(|c| c.first()) {
        // a calculation over no lines answers no aggregate
        None => Ok(const { &Vec::new() }),
        Some(c) => c["aggregates"].as_array().ok_or_else(|| "the logs answered a calculation with no aggregates".to_string()),
    }
}

/// A distinct count's answer: its value (none over no lines: 0).
pub fn uniq_of(answer: &Value) -> Result<u64, String> {
    match aggregates(answer)?.first() {
        None => Ok(0),
        Some(a) => a["value"].as_f64().filter(|v| *v >= 0.0).map(|v| v as u64).ok_or_else(|| format!("a distinct count is a number, not {}", a["value"])),
    }
}

/// A count by computer's answer: each computer and its lines, at most
/// `GROUPS_MAX`.
pub fn by_computer_of(answer: &Value) -> Result<Vec<(String, u64)>, String> {
    let groups = aggregates(answer)?;
    let mut out = Vec::with_capacity(groups.len().min(GROUPS_MAX as usize));
    for a in groups.iter().take(GROUPS_MAX as usize) {
        let computer = a["groups"].as_array().and_then(|g| g.iter().find(|g| g["key"] == "computer")).and_then(|g| g["value"].as_str());
        let Some(computer) = computer.filter(|c| c.starts_with("computer:")) else { return Err(format!("a group names no computer: {a}")) };
        let lines = a["value"].as_f64().filter(|v| *v >= 0.0).ok_or_else(|| format!("a count is a number, not {}", a["value"]))?;
        out.push((computer.to_string(), lines as u64));
    }
    Ok(out)
}

/// The computers in trouble: each kind's, by computer, the most lines
/// first (a kind before another at the same count), at most `TROUBLE_MAX`.
pub fn troubles<K: Copy + Ord>(kinds: Vec<(K, Vec<(String, u64)>)>) -> Vec<(String, K, u64)> {
    let mut all: Vec<(String, K, u64)> = kinds.into_iter().flat_map(|(kind, rows)| rows.into_iter().map(move |(computer, lines)| (computer, kind, lines))).collect();
    all.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));
    all.truncate(TROUBLE_MAX);
    all
}

/// The day's start an operator names (their midnight, `dayStart`), or,
/// unnamed, the UTC day's: refused past `DAY_START_MAX_MS` ago, or ahead.
pub fn day_start(named: Option<i64>, now_ms: i64) -> Result<i64, String> {
    match named {
        None => Ok(now_ms - now_ms.rem_euclid(DAY_MS)),
        // checked: an `at` far below zero would overflow `now_ms - at`
        Some(at) if now_ms.checked_sub(at).is_some_and(|ago| (0..=DAY_START_MAX_MS).contains(&ago)) => Ok(at),
        Some(at) => Err(format!("dayStart is the start of today, at most a day and an hour ago, not {at}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_793_800_000_000;

    /// The answers' shapes as the query API gave them (2026-11-04).
    fn answer(aggregates: Value) -> Value {
        json!({ "success": true, "result": { "run": {}, "statistics": {}, "calculations": [{ "calculation": "uniq", "alias": "n", "aggregates": aggregates }] } })
    }

    const SCRIPT: &str = "fragment";

    /// Goal: each question asks what its number means, over its window,
    /// of this deployment's Worker's lines alone. Method: the bodies, as
    /// the probe of 2026-11-04 asked them.
    #[test]
    fn each_question_asks_its_lines_over_its_window() {
        let day = NOW - 5 * 3_600_000;
        let awake = Question::AwakeNow.body(SCRIPT, NOW, day);
        assert_eq!(awake["timeframe"], json!({ "from": NOW - NOW_MS, "to": NOW }));
        let ours = json!({ "key": "$workers.scriptName", "operation": "eq", "type": "string", "value": SCRIPT });
        assert_eq!(awake["parameters"]["filters"], json!([ours, { "key": "phase.phase", "operation": "eq", "type": "string", "value": "awake" }]));
        assert_eq!(awake["parameters"]["calculations"], json!([{ "operator": "uniq", "key": "computer", "alias": "n" }]));
        assert!(awake["parameters"].get("groupBys").is_none());
        assert_eq!(Question::AwakeToday.body(SCRIPT, NOW, day)["timeframe"]["from"], day);
        let week = Question::AwakeWeek.body(SCRIPT, NOW, day);
        assert_eq!((week["timeframe"]["from"].as_i64(), &week["parameters"]["groupBys"]), (Some(NOW - WEEK_MS), &json!([{ "type": "string", "value": "computer" }])));
        assert_eq!(week["limit"], GROUPS_MAX);
        let fragments = Question::FragmentsNow.body(SCRIPT, NOW, day);
        assert_eq!(fragments["parameters"]["filters"][1]["value"], "Fragment");
        assert_eq!(fragments["parameters"]["calculations"][0]["key"], "$workers.durableObjectId");
        assert_eq!(Question::WontWake.body(SCRIPT, NOW, day)["parameters"]["filters"][1]["value"], "failed");
        assert_eq!(Question::SavesFailing.body(SCRIPT, NOW, day)["parameters"]["filters"][1], json!({ "key": "event.event", "operation": "eq", "type": "string", "value": "save_failed" }));
        assert_eq!(Question::DiskFilling.body(SCRIPT, NOW, day)["parameters"]["filters"][1], json!({ "key": "disk.free", "operation": "lt", "type": "number", "value": DISK_TOLD_FREE_MAX }));
        for q in Question::ALL {
            let b = q.body("fragment-preview", NOW, day);
            assert_eq!((&b["queryId"], &b["parameters"]["filters"][0]["value"]), (&json!(QUERY_ID), &json!("fragment-preview")), "every query is its own deployment's");
        }
    }

    /// Goal: a Worker's name goes into a query only as one: valid,
    /// invalid (empty, too long, a quote, a capital, a dash at an end).
    #[test]
    fn a_workers_name_is_one_before_a_query_names_it() {
        for ok in ["fragment", "fragment-preview", "fragment-rh2", "a"] {
            assert!(valid_script(ok), "{ok}");
        }
        let long = "a".repeat(SCRIPT_MAX_BYTES + 1);
        for bad in ["", "-fragment", "fragment-", "Fragment", "frag\"ment", "frag ment", long.as_str()] {
            assert!(!valid_script(bad), "{bad:?}");
        }
    }

    /// Goal: an answer reads as its number, an empty one as none, and an
    /// error or another shape as why. Valid, empty, invalid.
    #[test]
    fn answers_read_as_numbers_or_say_why() {
        assert_eq!(uniq_of(&answer(json!([{ "groups": [], "value": 37, "count": 412 }]))), Ok(37));
        assert_eq!(uniq_of(&answer(json!([]))), Ok(0), "no lines, none");
        assert_eq!(uniq_of(&json!({ "success": true, "result": { "calculations": [] } })), Ok(0));
        let grouped = answer(json!([
            { "groups": [{ "key": "computer", "value": "computer:17c33cdf88c6d1d8f58b7a0f" }], "value": 4, "groupKey": "computer:17c33cdf88c6d1d8f58b7a0f" },
            { "groups": [{ "key": "computer", "value": "computer:64f44a9e486a752ad97536b2" }], "value": 208 },
        ]));
        assert_eq!(by_computer_of(&grouped), Ok(vec![("computer:17c33cdf88c6d1d8f58b7a0f".into(), 4), ("computer:64f44a9e486a752ad97536b2".into(), 208)]));
        let refused = json!({ "success": false, "errors": [{ "code": 10000, "message": "Authentication error" }] });
        assert_eq!(uniq_of(&refused), Err("Authentication error".into()));
        assert!(uniq_of(&json!({})).is_err());
        assert!(uniq_of(&answer(json!([{ "value": "many" }]))).is_err());
        assert!(by_computer_of(&answer(json!([{ "groups": [{ "key": "computer", "value": "x" }], "value": 1 }]))).is_err(), "a group is a computer's");
        assert!(by_computer_of(&answer(json!([{ "groups": [], "value": 1 }]))).is_err());
    }

    /// Goal: the trouble list is the worst first and bounded. Method: two
    /// kinds, a tie, and more than `TROUBLE_MAX` rows.
    #[test]
    fn trouble_is_the_most_lines_first_and_bounded() {
        let rows = |n: u64, lines: u64| (0..n).map(|i| (format!("computer:{i:024x}"), lines)).collect::<Vec<_>>();
        let listed = troubles(vec![(2, vec![("computer:b".into(), 3)]), (1, vec![("computer:a".into(), 208), ("computer:c".into(), 3)])]);
        assert_eq!(listed, [("computer:a".into(), 1, 208), ("computer:c".into(), 1, 3), ("computer:b".into(), 2, 3)]);
        assert_eq!(troubles(vec![(1, rows(TROUBLE_MAX as u64 + 5, 1))]).len(), TROUBLE_MAX);
    }

    /// Goal: the day's start is the operator's midnight, never more than a
    /// day and an hour ago or ahead; unnamed, UTC's.
    #[test]
    fn the_day_starts_at_the_operators_midnight() {
        assert_eq!(day_start(Some(NOW - 3_600_000), NOW), Ok(NOW - 3_600_000));
        assert_eq!(day_start(Some(NOW - DAY_START_MAX_MS), NOW), Ok(NOW - DAY_START_MAX_MS));
        assert!(day_start(Some(NOW - DAY_START_MAX_MS - 1), NOW).is_err());
        assert!(day_start(Some(NOW + 1), NOW).is_err());
        assert_eq!(day_start(None, NOW), Ok(NOW - NOW % DAY_MS));
        // the integers' ends: refused, never an overflow past the bound
        assert!(day_start(Some(i64::MIN), NOW).is_err(), "now minus the least i64 overflows: refused");
        assert!(day_start(Some(i64::MAX), NOW).is_err());
        assert!(day_start(Some(i64::MIN), 0).is_err());
    }
}
