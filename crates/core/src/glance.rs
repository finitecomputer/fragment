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
//!   than `computer::ROOM_TOLD_MAX` of room (`computer::room`);
//! - why a save failed is its line's own words (the lifecycle's
//!   `Event::SaveFailed`'s `why`), the newest of each computer's among the
//!   newest lines read (`WHYS_READ_MAX`), shown short and with a person's
//!   paths left out (`why`).
//!
//! Every query counts this deployment's Worker's lines alone
//! (`$workers.scriptName`): an account holds other deployments (a
//! preview's account, its branches'), whose computers' ids are hashes of
//! the same people.
//!
//! Nothing is cached: the page asks again every minute.

use serde_json::Value;

use crate::computer::{METER_EVERY_MS, ROOM_TOLD_MAX, SAVED_DIRS};
use crate::search::cut;

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
/// Failed saves' lines read for their reasons, at most: the events
/// themselves, each a couple of KB, well within the cell's bound on an
/// answer (`OBSERVABILITY_ANSWER_MAX_BYTES`, 1 MiB). A computer failing
/// all day writes about a hundred (its retries back off to every 15
/// minutes); a computer whose last failure is older than these shows none.
pub const WHYS_READ_MAX: u32 = 100;
/// A save's reason, as the glance shows it, at most (cut at a character).
pub const WHY_MAX_BYTES: usize = 160;
/// A reason read, at most: a longer one is left out whole (`why`).
pub const WHY_READ_MAX_BYTES: usize = 4096;
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
    /// The newest lines that said a save failed (at most `WHYS_READ_MAX`):
    /// the lines themselves, for their reasons and when.
    SaveWhys,
}

impl Question {
    pub const ALL: [Question; 9] = [
        Question::AwakeNow,
        Question::AwakeToday,
        Question::AwakeWeek,
        Question::FragmentsNow,
        Question::FragmentsToday,
        Question::WontWake,
        Question::SavesFailing,
        Question::DiskFilling,
        Question::SaveWhys,
    ];

    /// The query API's body for it, over `script`'s lines (this
    /// deployment's Worker), at `now_ms`, the day starting at
    /// `day_start_ms`: one template for all (a body of `json!` for each
    /// cost the cell's Wasm some 4 KB more).
    pub fn body(self, script: &str, now_ms: i64, day_start_ms: i64) -> Value {
        assert!(valid_script(script), "a Worker's name is checked as the config is read");
        assert!(day_start(Some(day_start_ms), now_ms).is_ok(), "the day's start is checked");
        // its one filter (key, operation, type, value as JSON), what it
        // asks (a distinct count of a key, lines by computer, or the lines
        // themselves), and from when
        enum Asks {
            Uniq(&'static str),
            ByComputer,
            Lines,
        }
        const AWAKE: (&str, &str, &str, &str) = ("phase.phase", "eq", "string", "\"awake\"");
        const FRAGMENT: (&str, &str, &str, &str) = ("$workers.entrypoint", "eq", "string", "\"Fragment\"");
        const SAVE_FAILED: (&str, &str, &str, &str) = ("event.event", "eq", "string", "\"save_failed\"");
        let disk = ROOM_TOLD_MAX.to_string();
        let (filter, asks, from) = match self {
            Question::AwakeNow => (AWAKE, Asks::Uniq("computer"), now_ms - NOW_MS),
            Question::AwakeToday => (AWAKE, Asks::Uniq("computer"), day_start_ms),
            Question::AwakeWeek => (AWAKE, Asks::ByComputer, now_ms - WEEK_MS),
            Question::FragmentsNow => (FRAGMENT, Asks::Uniq("$workers.durableObjectId"), now_ms - NOW_MS),
            Question::FragmentsToday => (FRAGMENT, Asks::Uniq("$workers.durableObjectId"), day_start_ms),
            Question::WontWake => (("phase.phase", "eq", "string", "\"failed\""), Asks::ByComputer, now_ms - DAY_MS),
            Question::SavesFailing => (SAVE_FAILED, Asks::ByComputer, now_ms - DAY_MS),
            Question::DiskFilling => (("disk.room", "lt", "number", disk.as_str()), Asks::ByComputer, now_ms - DAY_MS),
            Question::SaveWhys => (SAVE_FAILED, Asks::Lines, now_ms - DAY_MS),
        };
        let (key, op, kind, value) = filter;
        let (view, limit, asked) = match asks {
            Asks::Uniq(k) => ("calculations", GROUPS_MAX, format!(r#","calculations":[{{"operator":"uniq","key":"{k}","alias":"n"}}]"#)),
            Asks::ByComputer => ("calculations", GROUPS_MAX, r#","calculations":[{"operator":"count","alias":"n"}],"groupBys":[{"type":"string","value":"computer"}]"#.to_string()),
            Asks::Lines => ("events", WHYS_READ_MAX, String::new()),
        };
        let body = format!(
            r#"{{"queryId":"{QUERY_ID}","timeframe":{{"from":{from},"to":{now_ms}}},"view":"{view}","limit":{limit},"parameters":{{"datasets":["cloudflare-workers"],"filters":[{{"key":"$workers.scriptName","operation":"eq","type":"string","value":"{script}"}},{{"key":"{key}","operation":"{op}","type":"{kind}","value":{value}}}]{asked}}}}}"#
        );
        serde_json::from_slice(body.as_bytes()).expect("a glance's query is JSON")
    }
}

/// An answer's result, or the API's own error.
fn result(answer: &Value) -> Result<&Value, String> {
    if answer["success"] != true {
        let errors = answer["errors"].as_array().map(|e| e.iter().filter_map(|e| e["message"].as_str()).collect::<Vec<_>>().join("; ")).unwrap_or_default();
        return Err(if errors.is_empty() { "the logs answered no result".to_string() } else { errors });
    }
    Ok(&answer["result"])
}

/// The aggregates of an answer's one calculation, or why it is none: the
/// API's own error, or a shape it does not have.
fn aggregates(answer: &Value) -> Result<&Vec<Value>, String> {
    match result(answer)?["calculations"].as_array().and_then(|c| c.first()) {
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

/// The lines' answer (`Question::SaveWhys`): each computer's newest
/// failed save's reason, as the glance shows it (`why`), and when (ms), in
/// the order each computer was first read. The lines come in the API's
/// order, which this does not count on for which is newest. A line of no computer or no reason is no row's: passed
/// over. An answer of another shape says why.
pub fn whys_of(answer: &Value) -> Result<Vec<(String, i64, String)>, String> {
    let events = &result(answer)?["events"];
    if !events.is_object() {
        return Err("the logs answered no events".to_string());
    }
    let lines = match &events["events"] {
        // none matched: the list is left out
        Value::Null => return Ok(vec![]),
        Value::Array(lines) => lines,
        other => return Err(format!("the logs' events are a list, not {other}")),
    };
    // a list, not a map: at most `WHYS_READ_MAX` lines, and a map's code
    // costs the cell's Wasm more than the scan does
    let mut newest: Vec<(&str, i64, &str)> = Vec::new();
    for e in lines.iter().take(WHYS_READ_MAX as usize) {
        let at = e["timestamp"].as_i64().ok_or_else(|| format!("a line's timestamp is its ms, not {}", e["timestamp"]))?;
        let line = &e["source"];
        let (Some(computer), Some(said)) = (line["computer"].as_str().filter(|c| c.starts_with("computer:")), line["event"]["why"].as_str()) else { continue };
        match newest.iter_mut().find(|(c, _, _)| *c == computer) {
            Some(kept) if at > kept.1 => *kept = (computer, at, said),
            Some(_) => {}
            None => newest.push((computer, at, said)),
        }
    }
    Ok(newest.into_iter().map(|(computer, at, w)| (computer.to_string(), at, why(w))).collect())
}

/// An OS error's own words, as Rust's `io::Error` writes Linux's
/// (`strerror`): the only words a reason keeps after a path.
const OS_ERRORS: [&str; 24] = [
    "Operation not permitted",
    "No such file or directory",
    "Input/output error",
    "Resource temporarily unavailable",
    "Permission denied",
    "Device or resource busy",
    "File exists",
    "Invalid cross-device link",
    "Not a directory",
    "Is a directory",
    "Invalid argument",
    "Too many open files in system",
    "Too many open files",
    "File too large",
    "No space left on device",
    "Read-only file system",
    "Too many links",
    "Broken pipe",
    "File name too long",
    "Directory not empty",
    "Connection reset by peer",
    "Connection timed out",
    "Stale file handle",
    "Disk quota exceeded",
];

/// An errno's name, as the Sandbox SDK writes a file error's detail when
/// its shim said no more.
const ERRNOS: [&str; 20] = [
    "EPERM", "ENOENT", "EIO", "EAGAIN", "EACCES", "EBUSY", "EEXIST", "EXDEV", "ENOTDIR", "EISDIR", "EINVAL", "ENFILE", "EMFILE", "EFBIG", "ENOSPC", "EROFS", "EPIPE", "ENAMETOOLONG", "ENOTEMPTY", "EDQUOT",
];

/// A failed save's reason as the glance shows it: the platform's words,
/// never a person's file names (a backup's error may name a file it could
/// not read, and a name may hold any mark, so no delimiter can say where
/// it ends: `/data/work Tax Return.pdf` is a file of /data's). Shown whole
/// are a reason with no path, and the platform's own forms: a backup past
/// its bound (`the backup of <dir>: no answer within N ms`), and the
/// Sandbox SDK's `backup '<dir>': <detail>` whose detail is a listed OS
/// error's words, an errno's name or R2's count (`<dir>` one of
/// `SAVED_DIRS`). Any other reason with a path is left out whole (the
/// SDK's keeping its `backup`), but for a listed OS error's words after
/// its last `": "`; one longer than `WHY_READ_MAX_BYTES` is left out whole,
/// as a cut could hide its path or make a platform form. When in doubt, it
/// is cut. Its spaces are one, and it is at most `WHY_MAX_BYTES`, cut at a
/// character.
pub fn why(raw: &str) -> String {
    if raw.len() > WHY_READ_MAX_BYTES {
        return "…".to_string();
    }
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = shown(&text);
    if out.len() > WHY_MAX_BYTES {
        out = format!("{}…", cut(&out, WHY_MAX_BYTES - "…".len()));
    }
    assert!(out.len() <= WHY_MAX_BYTES, "a reason shown is short");
    out
}

/// `why`'s words, before its bound.
fn shown(text: &str) -> String {
    // the platform's: a backup of a saved directory past its bound
    let overdue = SAVED_DIRS.iter().any(|d| {
        text.strip_prefix("the backup of ")
            .and_then(|t| t.strip_prefix(d))
            .and_then(|t| t.strip_prefix(": no answer within "))
            .and_then(|t| t.strip_suffix(" ms"))
            .is_some_and(|ms| (1..=10).contains(&ms.len()) && ms.bytes().all(|b| b.is_ascii_digit()))
    });
    if overdue || !text.contains('/') {
        return text.to_string();
    }
    // the Sandbox SDK's, of a saved directory: whole only when its detail is the platform's own
    let sdk = SAVED_DIRS.iter().find_map(|d| text.strip_prefix("backup '").and_then(|t| t.strip_prefix(d)).and_then(|t| t.strip_prefix("': ")));
    match sdk {
        Some(detail) if os_error(detail) || ERRNOS.contains(&detail) || r2_count(detail) => text.to_string(),
        Some(detail) => format!("backup …{}", os_error_after(detail)),
        None => format!("…{}", os_error_after(text)),
    }
}

/// After a reason left out: `": "` and a listed OS error's own words,
/// when its last `": "` is followed by just those; else nothing.
fn os_error_after(text: &str) -> String {
    match text.rfind(": ").map(|i| &text[i + 2..]) {
        Some(words) if os_error(words) => format!(": {words}"),
        _ => String::new(),
    }
}

/// Whether `words` are an OS error's own: one of `OS_ERRORS` and its code
/// (`No space left on device (os error 28)`).
fn os_error(words: &str) -> bool {
    let Some((said, code)) = words.strip_suffix(')').and_then(|w| w.rsplit_once(" (os error ")) else { return false };
    OS_ERRORS.contains(&said) && (1..=3).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `detail` is the Sandbox SDK's count of a backup R2 kept short
/// (`R2 stored 10 bytes, but the container uploaded 12`).
fn r2_count(detail: &str) -> bool {
    let digits = |n: &str| (1..=20).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit());
    detail.strip_prefix("R2 stored ").and_then(|t| t.split_once(" bytes, but the container uploaded ")).is_some_and(|(a, b)| digits(a) && digits(b))
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
        assert_eq!(Question::DiskFilling.body(SCRIPT, NOW, day)["parameters"]["filters"][1], json!({ "key": "disk.room", "operation": "lt", "type": "number", "value": ROOM_TOLD_MAX }));
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

    /// An events answer as Cloudflare's API types it (cloudflare-typescript,
    /// `workers/observability/telemetry.ts`): `result.events.events`, each
    /// with its `timestamp` (ms) and its `source`, the line as logged.
    fn events(lines: Vec<(i64, Value)>) -> Value {
        let events: Vec<Value> = lines.into_iter().enumerate().map(|(i, (at, source))| json!({ "$metadata": { "id": format!("e{i}") }, "dataset": "cloudflare-workers", "timestamp": at, "source": source })).collect();
        json!({ "success": true, "result": { "run": {}, "statistics": {}, "events": { "count": events.len(), "events": events } } })
    }

    /// A failed save's line, as the Computer DO logs its event (cell/src/computer.rs `drive_events`).
    fn save_failed(computer: &str, why: &str) -> Value {
        json!({ "computer": computer, "event": { "event": "save_failed", "generation": 4, "seq": 2, "why": why }, "phase": { "phase": "awake", "generation": 4, "since_ms": NOW - 9_000_000 }, "actions": [{ "action": "unhold", "generation": 4 }] })
    }

    /// Goal: why a computer's save failed is its newest line's reason, with
    /// when, one per computer, whatever order the lines come in; read from
    /// the lines themselves (the events view), not a count. Valid (two
    /// computers, one with two reasons, oldest first and newest first, one
    /// naming a person's file), empty (no lines; the list left out), lines
    /// of no row (no computer, no reason: passed over), invalid (the API's
    /// error, another shape, a time that is no number).
    #[test]
    fn a_failed_saves_newest_reason_is_read_by_computer() {
        let q = Question::SaveWhys.body(SCRIPT, NOW, NOW - 3_600_000);
        assert_eq!((&q["view"], &q["limit"], &q["timeframe"]), (&json!("events"), &json!(WHYS_READ_MAX), &json!({ "from": NOW - DAY_MS, "to": NOW })));
        let ours = json!({ "key": "$workers.scriptName", "operation": "eq", "type": "string", "value": SCRIPT });
        assert_eq!(q["parameters"]["filters"], json!([ours, { "key": "event.event", "operation": "eq", "type": "string", "value": "save_failed" }]));
        assert!(q["parameters"].get("calculations").is_none() && q["parameters"].get("groupBys").is_none(), "the lines themselves, not a count");
        for counted in Question::ALL.into_iter().filter(|q| *q != Question::SaveWhys) {
            assert_eq!(counted.body(SCRIPT, NOW, NOW - 3_600_000)["view"], "calculations", "{counted:?}");
        }
        let (a, b) = ("computer:17c33cdf88c6d1d8f58b7a0f", "computer:64f44a9e486a752ad97536b2");
        let mut lines = vec![
            (NOW - 7_200_000, save_failed(a, "the backup of /data: no answer within 727000 ms")),
            (NOW - 60_000, save_failed(a, "backup '/data': open /data/home/Tax Return.pdf: Permission denied (os error 13)")),
            (NOW - 3_600_000, save_failed(b, "its container is not running")),
        ];
        let read = vec![(a.to_string(), NOW - 60_000, "backup …: Permission denied (os error 13)".to_string()), (b.to_string(), NOW - 3_600_000, "its container is not running".to_string())];
        let sorted = |w: Result<Vec<(String, i64, String)>, String>| w.map(|mut w| {
            w.sort();
            w
        });
        assert_eq!(sorted(whys_of(&events(lines.clone()))), Ok(read.clone()));
        lines.reverse();
        assert_eq!(sorted(whys_of(&events(lines.clone()))), Ok(read.clone()), "newest first or last: the newest");
        lines.push((NOW, json!({ "computer": null, "event": { "event": "save_failed", "why": "no one's" } })));
        lines.push((NOW, json!({ "computer": a, "event": { "event": "save_failed" } })));
        assert_eq!(sorted(whys_of(&events(lines))), Ok(read), "a line of no computer, or of no reason, is passed over");
        assert_eq!(whys_of(&events(vec![])), Ok(vec![]), "no lines, none");
        assert_eq!(whys_of(&json!({ "success": true, "result": { "events": { "count": 0 } } })), Ok(vec![]), "none matched: the list left out");
        assert_eq!(whys_of(&json!({ "success": false, "errors": [{ "code": 10000, "message": "Authentication error" }] })), Err("Authentication error".into()));
        assert!(whys_of(&json!({ "success": true, "result": { "calculations": [] } })).is_err(), "an answer of no events");
        assert!(whys_of(&json!({ "success": true, "result": { "events": { "events": {} } } })).is_err());
        let mut untimed = events(vec![(NOW, save_failed(a, "x"))]);
        untimed["result"]["events"]["events"][0]["timestamp"] = json!("now");
        assert!(whys_of(&untimed).is_err(), "a line's time is a number");
    }

    /// Goal: a reason shows the platform's words, never a person's file
    /// names (a backup's error may name one it could not read), whatever
    /// marks a name holds and however long the reason. Only the platform's
    /// own forms are shown whole: a backup past its bound (`the backup of
    /// <dir>: no answer within N ms`), and the Sandbox SDK's `backup
    /// '<dir>': <detail>` whose detail is a listed OS error's words, an
    /// errno's name or R2's count. Any other reason with a path, or longer
    /// than is read, is left out whole but for a listed OS error's words
    /// after its last `": "` (no delimiter can say where a name ends:
    /// `/data/work Tax Return.pdf` is a file of /data's). Valid (the
    /// platform's forms, as they are), each delimiter after a
    /// platform-looking path, quoted and not, inside and outside the SDK's
    /// form, each review's leaks (watched leaking before), a reason past the
    /// read bound, too long at each width of character.
    #[test]
    fn a_reason_leaves_out_a_persons_paths_and_is_short() {
        let own = [
            "its container is not running",
            "start 4 is not the running one",
            "the backup of /data: no answer within 727000 ms",
            "the backup of /data/work: no answer within 727000 ms",
            "backup '/data': No space left on device (os error 28)",
            "backup '/data/work': No space left on device (os error 28)",
            "backup '/data': Input/output error (os error 5)",
            "backup '/data/work': R2 stored 10 bytes, but the container uploaded 12",
            "backup '/data': ENOSPC",
        ];
        for w in own {
            assert_eq!(why(w), w);
        }
        let theirs = ["Tax", "Return", "pdf", "home", "notes", "diary", "Ann", "My", "Documents", "secret", "Archive", "Réunion", "médicale", "dossier", "gateway", "disk full", "work", "x'"];
        let shown_alone = |w: &str| assert!(theirs.iter().all(|p| !w.contains(p)), "{w}");
        // a name of /data's that starts as the work does, after each delimiter
        for d in [" ", "'", ":", ",", ";", ")"] {
            let name = format!("/data/work{d}Tax Return.pdf");
            for (raw, shown) in [
                (format!("open {name}: Permission denied (os error 13)"), "…: Permission denied (os error 13)"),
                (format!("open '{name}': Permission denied (os error 13)"), "…: Permission denied (os error 13)"),
                (format!("backup '/data': open {name}: Permission denied (os error 13)"), "backup …: Permission denied (os error 13)"),
                (format!("backup '/data': open '{name}': EACCES"), "backup …"),
                (format!("backup '{name}': EACCES"), "…"),
                (format!("the backup of {name}: no answer within 727000 ms"), "…"),
            ] {
                let w = why(&raw);
                assert_eq!(w, shown, "{raw}");
                shown_alone(&w);
            }
        }
        let leaks = [
            ("backup '/data': open /data/home/Tax Return.pdf: Permission denied (os error 13)", "backup …: Permission denied (os error 13)"),
            ("backup '/data/work': Tax Return.pdf: Permission denied (os error 13)", "backup …: Permission denied (os error 13)"),
            ("backup '/data/work': x': EACCES", "backup …"),
            ("backup '/data/home/Tax Return.pdf': EACCES", "…"),
            ("read \"/data/work/notes/my diary.md\" failed", "…"),
            ("Permission denied: /data/home/Tax Return.pdf", "…"),
            ("home/notes/Tax.pdf: Permission denied (os error 13)", "…: Permission denied (os error 13)"),
            ("open Tax: Return/report.pdf: EACCES", "…"),
            ("open Réunion[/report.pdf: EACCES", "…"),
            ("~/notes/Tax.pdf: gone", "…"),
            ("request failed (http://gateway/parts/1?d=/data/home/Tax.pdf) at last", "…"),
            ("ENOSPC: no space left on device, open '/data/home/Tax.pdf'", "…"),
            ("backup\n  '/data':\tdisk   full", "backup …"),
            ("backup '/data/home/Ann's Tax Return.pdf': EACCES", "…"),
            ("open /data/home/Tax: Return.pdf: Permission denied", "…"),
            ("open /data/home/Archive: Réunion médicale.pdf: Permission denied", "…"),
            ("open 'Réunion: médicale (dossier/Tax.pdf': EACCES", "…"),
            ("failed to read My Documents/Tax.pdf: EACCES", "…"),
            ("backup '/data': read My Documents/Tax.pdf", "backup …"),
            ("(a)b/secret: x", "…"),
            // after a path, only a listed OS error's words: a name ending in
            // other words that look like one shows none of them
            ("open /data/home/Tax (os error 1): Permission denied (os error 13)", "…: Permission denied (os error 13)"),
            ("open /data/home/x: Tax Return (os error 1)", "…"),
        ];
        for (raw, shown) in leaks {
            let w = why(raw);
            assert_eq!(w, shown, "{raw}");
            shown_alone(&w);
        }
        // past what is read, nothing is shown: its cut could hide a path's slash or make a platform form
        let deep = "d".repeat(190);
        for (dirs, shown) in [(3, "…: Permission denied (os error 13)"), (30, "…")] {
            let raw = format!("open /data/{}/Archive: Réunion médicale.pdf{}: Permission denied (os error 13)", vec![deep.as_str(); dirs].join("/"), "x".repeat(30));
            assert_eq!(why(&raw), shown, "{dirs} directories, {} bytes", raw.len());
        }
        for raw in [format!("{}Taxé/return.pdf", " ".repeat(WHY_READ_MAX_BYTES - 4)), format!("{}backup '/data': ENOSPC/Tax.pdf", " ".repeat(WHY_READ_MAX_BYTES - 22)), "z ".repeat(WHY_READ_MAX_BYTES)] {
            assert_eq!(why(&raw), "…", "{} bytes", raw.len());
        }
        for c in ['x', 'é', '€', '𝄞'] {
            for pad in WHY_MAX_BYTES - 8..=WHY_MAX_BYTES {
                let raw = format!("{}{}", "y".repeat(pad), c.to_string().repeat(8));
                let w = why(&raw);
                match raw.len() <= WHY_MAX_BYTES {
                    true => assert_eq!(w, raw, "within its bound, whole"),
                    // the most that fits before the mark, cut at a character
                    false => assert!(w.len() <= WHY_MAX_BYTES && w.len() > WHY_MAX_BYTES - "…".len() - 4 && w.ends_with('…') && raw.starts_with(w.trim_end_matches('…')), "{c:?} after {pad}: {w}"),
                }
            }
        }
        assert_eq!(why(&"z".repeat(WHY_MAX_BYTES)), "z".repeat(WHY_MAX_BYTES), "at its bound, whole");
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
