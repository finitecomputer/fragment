//! Scheduled records (docs/api.md, "Scheduled records"): a fragment's data
//! schedules, which its editors add at run time (not its `fragment.json`),
//! each appending one record to one of its own channels, as the fragment
//! itself, at each minute its cron names. A record is what everything
//! already follows: an app's channel triggers run its jobs from it, its
//! pages show it live, and a computer that follows the channel wakes for it
//! (an agent's routine is one on its agent fragment's `tasks`:
//! docs/chat-records.md). The same alarm pass fires them as the installed
//! code's cron triggers (cell/src/jobs.rs `fire_cron`), by the same rule
//! for their next minute (`Cron::next_fire`).
//!
//! Here: what a schedule may be, checked once for every way one is added
//! (`check`, `admit_channel`); the key that makes each fire append once
//! (`fire_key`); and a schedule in words (`said`).

use std::fmt;

use fragment_proto::{canonical_json, limits, AddSchedule, ChannelDecl, ErrorCode, Role};

use crate::cron::Cron;

/// Why a schedule is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Its cron does not parse: the parser's why.
    Cron(String),
    /// It fires closer than `RECORD_SCHEDULE_EVERY_MIN_MINUTES`: its
    /// closest, in minutes.
    TooOften(i64),
    /// Its record's bytes, as JSON, past `RECORD_SCHEDULE_BODY_MAX_BYTES`.
    RecordTooLarge(usize),
    /// The fragment declares no channel of that name.
    NoChannel(String),
    /// The channel takes no posts (or is the platform's own: `events`,
    /// `ops`, `inbox`), so nothing appends to it as its fragment.
    TakesNoPosts(String),
    /// Only its members of one kind post there (`from`), and the fragment
    /// is no member: its record would be a forgery of theirs.
    FromMembers(String),
    /// The one adding it may not post there: the channel's post role.
    BelowPostRole { channel: String, post: Role },
}

impl Refusal {
    /// What the platform answers it with.
    pub fn code(&self) -> ErrorCode {
        match self {
            Refusal::RecordTooLarge(_) => ErrorCode::TooLarge,
            Refusal::BelowPostRole { .. } => ErrorCode::Forbidden,
            Refusal::Cron(_) | Refusal::TooOften(_) | Refusal::NoChannel(_) | Refusal::TakesNoPosts(_) | Refusal::FromMembers(_) => ErrorCode::InvalidRequest,
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Cron(why) => write!(f, "a schedule's cron is five fields in UTC (minute hour day month weekday; weekdays by name: `mon-fri`): {why}"),
            Refusal::TooOften(closest) => write!(
                f,
                "this schedule fires {closest} minute{} apart at its closest; a scheduled record fires at most every {} minutes (an app's cron trigger runs more often)",
                if *closest == 1 { "" } else { "s" },
                limits::RECORD_SCHEDULE_EVERY_MIN_MINUTES
            ),
            Refusal::RecordTooLarge(bytes) => write!(f, "a scheduled record is {bytes} bytes as JSON; at most {}", limits::RECORD_SCHEDULE_BODY_MAX_BYTES),
            Refusal::NoChannel(c) => write!(f, "this fragment declares no channel {c:?}"),
            Refusal::TakesNoPosts(c) => write!(f, "channel {c} takes no posts, so nothing is appended to it on a schedule"),
            Refusal::FromMembers(c) => write!(f, "only its members of one kind post on channel {c}, and a scheduled record is the fragment's own"),
            Refusal::BelowPostRole { channel, post } => write!(f, "posting on channel {channel} needs the {} role, so scheduling a record there does too", post.as_str()),
        }
    }
}

/// A schedule to add, as it is kept: its cron's fields one space apart and
/// its record as canonical JSON, so the same one added again is found.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub cron: Cron,
    pub expr: String,
    pub channel: String,
    pub body: String,
}

/// `add`, checked: its cron parses and fires at most every
/// `RECORD_SCHEDULE_EVERY_MIN_MINUTES`, and its record is within
/// `RECORD_SCHEDULE_BODY_MAX_BYTES`. Its channel is its fragment's to
/// admit (`admit_channel`).
pub fn check(add: &AddSchedule) -> Result<Checked, Refusal> {
    let expr = add.cron.split_whitespace().collect::<Vec<_>>().join(" ");
    let cron = Cron::parse(&expr).map_err(Refusal::Cron)?;
    let closest = cron.closest_minutes();
    if closest < limits::RECORD_SCHEDULE_EVERY_MIN_MINUTES {
        return Err(Refusal::TooOften(closest));
    }
    let body = canonical_json(&add.record);
    if body.len() > limits::RECORD_SCHEDULE_BODY_MAX_BYTES {
        return Err(Refusal::RecordTooLarge(body.len()));
    }
    const { assert!(limits::RECORD_SCHEDULE_BODY_MAX_BYTES < limits::RECORD_BODY_MAX_BYTES, "a scheduled record fits a channel record") };
    Ok(Checked { cron, expr, channel: add.channel.clone(), body })
}

/// Whether a record may be scheduled on `channel` (its fragment's
/// declaration, `None` when it declares none; `builtin`, one of the
/// platform's own) by someone holding `role`: a channel that takes posts,
/// from whoever holds its post role (not only its members of one kind),
/// that role theirs. The fire appends as the fragment, so no schedule
/// writes where its maker could not post.
pub fn admit_channel(channel: &str, decl: Option<&ChannelDecl>, builtin: bool, role: Role) -> Result<(), Refusal> {
    if builtin {
        return Err(Refusal::TakesNoPosts(channel.to_string()));
    }
    let decl = decl.ok_or_else(|| Refusal::NoChannel(channel.to_string()))?;
    let post = decl.post.ok_or_else(|| Refusal::TakesNoPosts(channel.to_string()))?;
    if decl.from.is_some() {
        return Err(Refusal::FromMembers(channel.to_string()));
    }
    if role < post {
        return Err(Refusal::BelowPostRole { channel: channel.to_string(), post });
    }
    Ok(())
}

/// The key a fire appends under: the schedule and the minute it was due,
/// so an alarm that fires it again (a retry, a pass cut short) appends
/// nothing twice (docs/cloudflare-v1.md, lesson 2).
pub fn fire_key(id: i64, due: i64) -> String {
    assert!(id > 0 && due > 0, "a fire names its schedule and its minute");
    format!("schedule:{id}:{due}")
}

const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DAY_SHORT: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTH_NAMES: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTH_SHORT: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// A field that is one number within `lo..=hi`.
fn number(field: &str, lo: u32, hi: u32) -> Option<u32> {
    field.bytes().all(|b| b.is_ascii_digit()).then(|| field.parse().ok()).flatten().filter(|n| (lo..=hi).contains(n))
}

/// A weekday as the parser takes it (1 = Sunday … 7 = Saturday, or a name).
fn day(token: &str) -> Option<&'static str> {
    let i = match number(token, 1, 7) {
        Some(n) => n as usize - 1,
        None => DAY_SHORT.iter().position(|d| d.eq_ignore_ascii_case(token))?,
    };
    Some(DAY_NAMES[i])
}

fn month(token: &str) -> Option<&'static str> {
    let i = match number(token, 1, 12) {
        Some(n) => n as usize - 1,
        None => MONTH_SHORT.iter().position(|m| m.eq_ignore_ascii_case(token))?,
    };
    Some(MONTH_NAMES[i])
}

/// `a, b and c`.
fn listed(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A weekday field in words (`mon-fri`: "Monday to Friday"), or none for
/// one with a step.
fn days(field: &str) -> Option<String> {
    let mut parts = vec![];
    for part in field.split(',') {
        parts.push(match part.split_once('-') {
            Some((a, b)) => format!("{} to {}", day(a)?, day(b)?),
            None => day(part)?.to_string(),
        });
    }
    Some(listed(&parts))
}

/// A schedule in words, as a fragment's page and the CLI list it: the
/// common shapes said plainly (every N minutes, every hour, every day,
/// some weekdays, a day of the month, a day of the year), any other as the
/// cron itself. Every time is UTC, and says so.
pub fn said(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month_field, dow] = fields[..] else { return format!("at the times `{expr}` names (UTC)") };
    let at = |h: u32, m: u32| format!("{h:02}:{m:02} UTC");
    let shown = match (number(minute, 0, 59), number(hour, 0, 23), dom, month_field, dow) {
        (None, None, "*", "*", "*") if hour == "*" => match minute.strip_prefix("*/").and_then(|n| number(n, 1, 59)) {
            Some(n) if 60 % n == 0 => Some(format!("every {n} minutes")),
            _ => None,
        },
        (Some(m), None, "*", "*", "*") if hour == "*" => Some(format!("every hour at :{m:02}")),
        (Some(m), None, "*", "*", "*") => match hour.strip_prefix("*/").and_then(|n| number(n, 1, 23)) {
            Some(n) if 24 % n == 0 => Some(format!("every {n} hours at :{m:02} (from midnight UTC)")),
            _ => None,
        },
        (Some(m), Some(h), "*", "*", "*") => Some(format!("every day at {}", at(h, m))),
        (Some(m), Some(h), "*", "*", d) => days(d).map(|d| format!("every {d} at {}", at(h, m))),
        (Some(m), Some(h), d, "*", "*") => number(d, 1, 31).map(|d| format!("on day {d} of every month at {}", at(h, m))),
        (Some(m), Some(h), d, mo, "*") => match (number(d, 1, 31), month(mo)) {
            (Some(d), Some(mo)) => Some(format!("every year on {mo} {d} at {}", at(h, m))),
            _ => None,
        },
        _ => None,
    };
    shown.unwrap_or_else(|| format!("at the times `{expr}` names (UTC)"))
}

#[cfg(test)]
mod tests {
    use fragment_proto::IdentityKind;
    use serde_json::json;

    use super::*;

    const MINUTE: i64 = 60_000;
    // 2026-09-23 12:34:56 UTC
    const NOW: i64 = 1_790_166_896_000;

    fn add(cron: &str, record: serde_json::Value) -> AddSchedule {
        AddSchedule { cron: cron.into(), channel: "tasks".into(), record }
    }

    fn channel(post: Option<Role>, from: Option<IdentityKind>) -> ChannelDecl {
        ChannelDecl { read: Role::Viewer, post, signed_in: false, from, listed: None }
    }

    /// Valid: an hourly, a weekday and a quarter-hourly schedule, kept with
    /// its cron's fields one space apart and its record as canonical JSON
    /// (its keys in order: the same record however it was written); the
    /// bounds' own values pass.
    #[test]
    fn valid() {
        let c = check(&add(" 0  *  * * * ", json!({ "text": "check the shelf", "kind": "routine" }))).unwrap();
        assert_eq!((c.expr.as_str(), c.channel.as_str(), c.body.as_str()), ("0 * * * *", "tasks", r#"{"kind":"routine","text":"check the shelf"}"#));
        assert!(check(&add("30 9 * * mon-fri", json!({ "kind": "tick" }))).is_ok());
        assert!(check(&add("*/15 * * * *", json!("ping"))).is_ok(), "every 15 minutes is the bound's own; any JSON is a record");
        let pad = limits::RECORD_SCHEDULE_BODY_MAX_BYTES - r#"{"t":""}"#.len();
        assert!(check(&add("0 * * * *", json!({ "t": "w".repeat(pad) }))).is_ok(), "the record's bound is its own");
        let editor = channel(Some(Role::Editor), None);
        assert_eq!(admit_channel("tasks", Some(&editor), false, Role::Editor), Ok(()));
        assert_eq!(admit_channel("tasks", Some(&editor), false, Role::Owner), Ok(()));
    }

    /// Invalid: each bound refused, with its typed refusal and the code the
    /// platform answers it with.
    #[test]
    fn invalid() {
        let refused = |a: AddSchedule| check(&a).unwrap_err();
        assert!(matches!(refused(add("0 * * *", json!({}))), Refusal::Cron(_)));
        assert!(matches!(refused(add("0 9 * * 0", json!({}))), Refusal::Cron(_)), "weekday 0 is the parser's refusal");
        assert_eq!(refused(add("*/5 * * * *", json!({}))), Refusal::TooOften(5));
        assert_eq!(refused(add("* * * * *", json!({}))), Refusal::TooOften(1));
        assert_eq!(refused(add("0,10 * * * *", json!({}))), Refusal::TooOften(10));
        assert_eq!(refused(add("*/25 * * * *", json!({}))), Refusal::TooOften(10), "across the hour");
        let pad = limits::RECORD_SCHEDULE_BODY_MAX_BYTES - r#"{"t":""}"#.len() + 1;
        assert_eq!(refused(add("0 * * * *", json!({ "t": "w".repeat(pad) }))), Refusal::RecordTooLarge(limits::RECORD_SCHEDULE_BODY_MAX_BYTES + 1));
        let editor = channel(Some(Role::Editor), None);
        assert_eq!(admit_channel("events", None, true, Role::Owner), Err(Refusal::TakesNoPosts("events".into())));
        assert_eq!(admit_channel("nope", None, false, Role::Owner), Err(Refusal::NoChannel("nope".into())));
        assert_eq!(admit_channel("log", Some(&channel(None, None)), false, Role::Owner), Err(Refusal::TakesNoPosts("log".into())));
        assert_eq!(admit_channel("work", Some(&channel(Some(Role::Editor), Some(IdentityKind::Agent))), false, Role::Owner), Err(Refusal::FromMembers("work".into())));
        assert_eq!(admit_channel("tasks", Some(&editor), false, Role::Viewer), Err(Refusal::BelowPostRole { channel: "tasks".into(), post: Role::Editor }));
        assert_eq!(Refusal::RecordTooLarge(5000).code(), ErrorCode::TooLarge);
        assert_eq!(Refusal::BelowPostRole { channel: "tasks".into(), post: Role::Editor }.code(), ErrorCode::Forbidden);
        for r in [Refusal::Cron("x".into()), Refusal::TooOften(5), Refusal::NoChannel("x".into()), Refusal::TakesNoPosts("x".into()), Refusal::FromMembers("x".into())] {
            assert_eq!(r.code(), ErrorCode::InvalidRequest, "{r}");
        }
        assert!(Refusal::TooOften(5).to_string().contains("fires 5 minutes apart at its closest; a scheduled record fires at most every 15 minutes"));
    }

    /// Replay: a fire's key names its schedule and the minute it was due, so
    /// the same fire appends under the same key (and appends once), and no
    /// two fires share one.
    #[test]
    fn a_fire_appends_once() {
        let due = NOW / MINUTE * MINUTE;
        assert_eq!(fire_key(3, due), fire_key(3, due));
        assert_ne!(fire_key(3, due), fire_key(3, due + 60 * MINUTE));
        assert_ne!(fire_key(3, due), fire_key(4, due));
    }

    #[test]
    fn said_in_words() {
        for (expr, words) in [
            ("*/15 * * * *", "every 15 minutes"),
            ("*/30 * * * *", "every 30 minutes"),
            ("0 * * * *", "every hour at :00"),
            ("5 * * * *", "every hour at :05"),
            ("0 */6 * * *", "every 6 hours at :00 (from midnight UTC)"),
            ("0 9 * * *", "every day at 09:00 UTC"),
            ("30 9 * * mon-fri", "every Monday to Friday at 09:30 UTC"),
            ("0 18 * * 1,7", "every Sunday and Saturday at 18:00 UTC"),
            ("0 8 * * mon,wed,fri", "every Monday, Wednesday and Friday at 08:00 UTC"),
            ("0 7 1 * *", "on day 1 of every month at 07:00 UTC"),
            ("0 0 1 jan *", "every year on January 1 at 00:00 UTC"),
            ("0 0 25 12 *", "every year on December 25 at 00:00 UTC"),
            ("*/25 * * * *", "at the times `*/25 * * * *` names (UTC)"),
            ("0 9,17 * * *", "at the times `0 9,17 * * *` names (UTC)"),
            ("0 9 * * */2", "at the times `0 9 * * */2` names (UTC)"),
        ] {
            assert_eq!(said(expr), words, "{expr}");
        }
    }
}
