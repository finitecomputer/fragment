//! A Hermes home's scheduled jobs (`cron/jobs.json`), as the routines an
//! import makes of them: what becomes of each (`Suggestion`), and for a
//! routine its schedule in UTC, in the platform's cron. Routines run in
//! UTC, so a job's time zone is worked out here (with its daylight saving,
//! said), not by the agent. Weekdays are written by name: Hermes counts
//! Sunday as 0 and the platform as 1 (`fragment_core::cron`), and a name
//! is the same day in both.

use std::collections::BTreeSet;

use fragment_core::cron::Cron;
use fragment_proto::limits::{RECORD_SCHEDULES_MAX, RECORD_SCHEDULE_BODY_MAX_BYTES, RECORD_SCHEDULE_EVERY_MIN_MINUTES};
use serde::Serialize;
use serde_json::Value;

use super::survey::{attention_lines, Line};

/// A routine's text, at most, with room in its record for the rest of its
/// body (`{"kind":"routine","text":…}`) and JSON's escapes.
pub const TEXT_MAX_BYTES: usize = RECORD_SCHEDULE_BODY_MAX_BYTES * 7 / 8;
/// A job's prompt as the survey shows it, at most (longer: cut, and said).
const PROMPT_SHOWN_MAX_CHARS: usize = 8_000;
/// Jobs one survey reads, at most.
const JOBS_MAX: usize = 1_000;

const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const DAY_MIN: i64 = 24 * 60;

/// What becomes of one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Suggestion {
    /// A routine: `fragment schedule add --routine … --cron <schedule_utc>`.
    Routine,
    /// Past the routines an agent keeps (`RECORD_SCHEDULES_MAX`).
    TooMany,
    /// More often than a routine may fire: an app's cron trigger fits it.
    TooFrequent,
    /// Its script's output was the whole message: an app's cron trigger.
    NoAgent,
    Paused,
    /// Runs once, later: the owner is told when.
    OnceFuture,
    /// Ran once, already.
    OncePast,
    /// Its schedule can't be said in UTC simply: `why` says why.
    ConvertByHand,
}

#[derive(Debug, Serialize)]
pub struct Job {
    pub id: Option<String>,
    pub name: Option<String>,
    pub suggestion: Suggestion,
    pub why: String,
    /// Its schedule as it was (Hermes' cron, its own zone).
    pub schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_utc: Option<String>,
    /// `schedule_utc` in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    /// Its prompt, cut at `PROMPT_SHOWN_MAX_CHARS` (`prompt_bytes` is the
    /// whole one's), and whether it is longer than a routine's text may be.
    pub prompt: Option<String>,
    pub prompt_bytes: usize,
    pub text_too_long: bool,
    pub skills: Vec<String>,
    /// Its script, in the Hermes home's `scripts/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<Line>,
}

#[derive(Debug, Serialize)]
pub struct Routines {
    /// The zone its jobs ran in, and whether it moves its clocks (a UTC
    /// routine then fires an hour apart across the year, in local time).
    pub zone: String,
    pub dst: Option<bool>,
    pub jobs: Vec<Job>,
    /// How many are routines, and the most an agent keeps.
    pub routines: usize,
    pub max: usize,
}

/// The jobs of a `jobs.json`: a list, `{"jobs": [...]}`, or `{id: job}`.
pub fn jobs_of(v: &Value) -> Vec<Value> {
    let jobs = match v.get("jobs") {
        Some(j) => j.clone(),
        None => v.clone(),
    };
    let listed = match jobs {
        Value::Array(a) => a,
        Value::Object(m) => m.into_iter().filter(|(_, j)| j.is_object()).map(|(id, mut j)| {
            j.as_object_mut().map(|o| o.entry("id").or_insert(Value::String(id)));
            j
        }).collect(),
        _ => Vec::new(),
    };
    listed.into_iter().filter(Value::is_object).take(JOBS_MAX).collect()
}

/// Each job's suggestion, in the order listed: the first
/// `RECORD_SCHEDULES_MAX` routines are routines, the rest `TooMany`.
pub fn routines(jobs: &[Value], zone_name: Option<&str>, now_s: i64, words: &[String]) -> Routines {
    let zone = Zone::of(zone_name, now_s);
    let mut made = 0;
    let jobs = jobs
        .iter()
        .map(|j| {
            let mut job = job(j, &zone, now_s, words);
            if job.suggestion == Suggestion::Routine {
                made += 1;
                if made > RECORD_SCHEDULES_MAX {
                    job.suggestion = Suggestion::TooMany;
                    job.why = format!("an agent keeps at most {RECORD_SCHEDULES_MAX} routines");
                }
            }
            job
        })
        .collect();
    Routines { zone: zone.name, dst: zone.dst, jobs, routines: made.min(RECORD_SCHEDULES_MAX), max: RECORD_SCHEDULES_MAX }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn job(j: &Value, zone: &Zone, now_s: i64, words: &[String]) -> Job {
    let (suggestion, utc, why) = rule(j, zone, now_s);
    let prompt = text(&j["prompt"]);
    let prompt_bytes = prompt.as_ref().map_or(0, String::len);
    let mut skills: Vec<String> = j["skills"].as_array().map(|a| a.iter().filter_map(text).collect()).unwrap_or_default();
    skills.extend(text(&j["skill"]));
    Job {
        id: text(&j["id"]),
        name: text(&j["name"]),
        suggestion,
        why,
        schedule: text(&j["schedule"]["expr"]).or_else(|| text(&j["schedule"]["display"])),
        said: utc.as_deref().map(fragment_core::schedule::said),
        schedule_utc: utc,
        attention: prompt.as_deref().map(|p| attention_lines("prompt", p, words)).unwrap_or_default(),
        prompt: prompt.map(|p| if p.chars().count() > PROMPT_SHOWN_MAX_CHARS { format!("{}… [cut]", p.chars().take(PROMPT_SHOWN_MAX_CHARS).collect::<String>()) } else { p }),
        prompt_bytes,
        text_too_long: prompt_bytes > TEXT_MAX_BYTES,
        skills,
        script: text(&j["script"]),
    }
}

/// (suggestion, its UTC schedule, why) for one job.
fn rule(j: &Value, zone: &Zone, now_s: i64) -> (Suggestion, Option<String>, String) {
    let schedule = &j["schedule"];
    if j["enabled"] == Value::Bool(false) || j["state"] == "paused" {
        return (Suggestion::Paused, None, "it was paused".into());
    }
    if j["no_agent"] == Value::Bool(true) {
        return (Suggestion::NoAgent, None, "its script's output was the whole message, with no agent: an app's cron trigger fits it".into());
    }
    match schedule["kind"].as_str() {
        Some("once") => once(&schedule["run_at"], zone, now_s),
        Some("interval") => {
            let minutes = schedule["minutes"].as_i64().unwrap_or(0);
            if minutes < RECORD_SCHEDULE_EVERY_MIN_MINUTES {
                return (Suggestion::TooFrequent, None, format!("every {minutes} minutes: a routine runs at most every {RECORD_SCHEDULE_EVERY_MIN_MINUTES}"));
            }
            match interval_cron(minutes) {
                Some(c) => (Suggestion::Routine, Some(c), format!("every {minutes} minutes")),
                None => (Suggestion::ConvertByHand, None, format!("every {minutes} minutes fits no cron")),
            }
        }
        Some("cron") => {
            let expr = schedule["expr"].as_str().unwrap_or_default();
            let Some(offset) = zone.offset_min else {
                return (Suggestion::ConvertByHand, None, format!("its time zone, {}, is not one this computer knows", zone.name));
            };
            let Some(utc) = to_utc(expr, offset) else {
                return (Suggestion::ConvertByHand, None, format!("{expr} can't be said in UTC simply"));
            };
            match Cron::parse(&utc) {
                Err(e) => (Suggestion::ConvertByHand, None, format!("{expr}, in UTC {utc}: {e}")),
                Ok(c) if c.closest_minutes() < RECORD_SCHEDULE_EVERY_MIN_MINUTES => {
                    (Suggestion::TooFrequent, None, format!("{expr} fires more often than every {RECORD_SCHEDULE_EVERY_MIN_MINUTES} minutes"))
                }
                Ok(_) => (Suggestion::Routine, Some(utc), if offset == 0 { "already in UTC".into() } else { format!("moved from {} to UTC", zone.name) }),
            }
        }
        _ => (Suggestion::ConvertByHand, None, "an unknown kind of schedule".into()),
    }
}

fn once(run_at: &Value, zone: &Zone, now_s: i64) -> (Suggestion, Option<String>, String) {
    let Some(at) = run_at.as_str().and_then(|s| zone.instant(s)) else {
        return (Suggestion::ConvertByHand, None, "its one time to run couldn't be read".into());
    };
    if at > now_s {
        let when = jiff::Timestamp::from_second(at).map(|t| t.to_string()).unwrap_or_default();
        (Suggestion::OnceFuture, None, format!("runs once, at {when}: ask your owner nearer the time"))
    } else {
        (Suggestion::OncePast, None, "ran once, already".into())
    }
}

/// An interval as cron, or `None` when no cron says it.
fn interval_cron(minutes: i64) -> Option<String> {
    match minutes {
        m if m < 60 => (60 % m == 0).then(|| format!("*/{m} * * * *")),
        60 => Some("0 * * * *".into()),
        1440 => Some("0 0 * * *".into()),
        10080 => Some("0 0 * * mon".into()),
        m if m % 60 == 0 && 24 % (m / 60) == 0 => Some(format!("0 */{} * * *", m / 60)),
        _ => None,
    }
}

/// A day-of-week field in Hermes' cron (0 or 7 Sunday, names too) as
/// days, 0 Sunday; `None` for a shape this doesn't read (a step).
fn days_of(field: &str) -> Option<BTreeSet<i64>> {
    let end = |p: &str| DAYS.iter().position(|d| *d == p).map(|i| i as i64).or_else(|| p.parse::<i64>().ok().filter(|n| (0..=7).contains(n)));
    let mut days = BTreeSet::new();
    for item in field.to_ascii_lowercase().split(',') {
        let ends: Vec<i64> = item.split('-').map(end).collect::<Option<_>>()?;
        match ends[..] {
            [d] => {
                days.insert(d % 7);
            }
            // `5-7` is Friday to Sunday
            [a, b] if a <= b => days.extend((a..=b).map(|d| d % 7)),
            _ => return None,
        }
    }
    Some(days)
}

/// Days (0 Sunday) as a field by name: runs of three or more as ranges.
fn day_names(days: &BTreeSet<i64>) -> String {
    let ordered: Vec<i64> = days.iter().copied().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 0..ordered.len() {
        if i + 1 == ordered.len() || ordered[i + 1] != ordered[i] + 1 {
            let (a, b) = (ordered[start], ordered[i]);
            if b - a >= 2 {
                parts.push(format!("{}-{}", DAYS[a as usize], DAYS[b as usize]));
            } else {
                parts.extend((a..=b).map(|d| DAYS[d as usize].to_string()));
            }
            start = i + 1;
        }
    }
    parts.join(",")
}

/// A Hermes cron expression read in a zone `offset` minutes east of UTC,
/// written in UTC in the platform's cron; `None` when it can't be said
/// simply (a time that moves across a date, a step of hours).
fn to_utc(expr: &str, offset: i64) -> Option<String> {
    let f: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = f[..] else { return None };
    let mut days = if dow == "*" { None } else { Some(days_of(dow)?) };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let field = |days: &Option<BTreeSet<i64>>| days.as_ref().map_or("*".to_string(), day_names);
    if hour == "*" {
        let minute = match (offset % 60 == 0, digits(minute)) {
            (true, _) => minute.to_string(),
            (false, true) => (minute.parse::<i64>().ok()? - offset).rem_euclid(60).to_string(),
            (false, false) => return None,
        };
        return Some(format!("{minute} * {dom} {month} {}", field(&days)));
    }
    let hours: Vec<i64> = hour.split(',').map(|h| h.parse::<i64>().ok().filter(|h| (0..24).contains(h))).collect::<Option<_>>()?;
    if !digits(minute) && offset % 60 != 0 {
        return None;
    }
    let base = if digits(minute) { minute.parse::<i64>().ok()? } else { 0 };
    let moved: Vec<i64> = hours.iter().map(|h| h * 60 + base - offset).collect();
    let shifts: BTreeSet<i64> = moved.iter().map(|m| m.div_euclid(DAY_MIN)).collect();
    let [shift] = shifts.into_iter().collect::<Vec<_>>()[..] else { return None };
    if shift != 0 {
        if dom != "*" || month != "*" {
            return None;
        }
        days = days.map(|d| d.iter().map(|d| (d + shift).rem_euclid(7)).collect());
    }
    let new_minute = if digits(minute) { moved[0].rem_euclid(60).to_string() } else { minute.to_string() };
    let new_hours: BTreeSet<i64> = moved.iter().map(|m| m.rem_euclid(DAY_MIN) / 60).collect();
    let new_hours = new_hours.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    Some(format!("{new_minute} {new_hours} {dom} {month} {}", field(&days)))
}

/// A job's time zone: its offset from UTC now, and whether it moves its
/// clocks across the year (unknown: `None`).
struct Zone {
    name: String,
    tz: Option<jiff::tz::TimeZone>,
    offset_min: Option<i64>,
    dst: Option<bool>,
}

impl Zone {
    fn of(name: Option<&str>, now_s: i64) -> Zone {
        let name = name.filter(|n| !n.is_empty()).unwrap_or("UTC").to_string();
        let tz = jiff::tz::TimeZone::get(&name).ok();
        let now = jiff::Timestamp::from_second(now_s).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        let offset_min = tz.as_ref().map(|z| z.to_offset(now).seconds() as i64 / 60);
        let dst = tz.as_ref().and_then(|z| {
            let year = now.to_zoned(z.clone()).year();
            let noon = |month| jiff::civil::date(year, month, 1).at(12, 0, 0, 0).to_zoned(z.clone()).ok().map(|t| t.offset());
            Some(noon(1)? != noon(7)?)
        });
        Zone { name, tz, offset_min, dst }
    }

    /// A time as Hermes writes one (RFC 3339, or without its offset: then
    /// in this zone), in seconds since 1970.
    fn instant(&self, s: &str) -> Option<i64> {
        if let Ok(t) = s.parse::<jiff::Timestamp>() {
            return Some(t.as_second());
        }
        let civil: jiff::civil::DateTime = s.parse().ok()?;
        civil.to_zoned(self.tz.clone()?).ok().map(|z| z.timestamp().as_second())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 2026-11-09T12:00:00Z.
    const NOW: i64 = 1_794_225_600;

    #[test]
    fn hermes_cron_in_utc_by_day_name() {
        // UTC already, but its weekdays are Hermes' numbers: by name
        assert_eq!(to_utc("0 9 * * 1-5", 0).as_deref(), Some("0 9 * * mon-fri"));
        assert_eq!(to_utc("30 8 * * 0", 0).as_deref(), Some("30 8 * * sun"));
        assert_eq!(to_utc("30 8 * * 7", 0).as_deref(), Some("30 8 * * sun"), "7 is Sunday too");
        // New York in November is UTC-5: 9:00 → 14:00
        assert_eq!(to_utc("0 9 * * *", -300).as_deref(), Some("0 14 * * *"));
        // 21:00 in New York is 02:00 UTC the next day: the days move
        assert_eq!(to_utc("0 21 * * 1-5", -300).as_deref(), Some("0 2 * * tue-sat"));
        // Tokyo, UTC+9: 08:00 Monday is 23:00 Sunday
        assert_eq!(to_utc("0 8 * * mon", 540).as_deref(), Some("0 23 * * sun"));
        // a day of the month can't move across a date simply
        assert_eq!(to_utc("0 1 15 * *", 540), None);
        // India, UTC+5:30: a minute moves with it
        assert_eq!(to_utc("15 * * * *", 330).as_deref(), Some("45 * * * *"));
        assert_eq!(to_utc("*/15 * * * *", 330), None);
        assert_eq!(to_utc("0 9,13 * * *", 120).as_deref(), Some("0 7,11 * * *"));
        assert_eq!(to_utc("0 */2 * * *", 60), None, "a step of hours");
        for utc in ["0 9 * * mon-fri", "30 8 * * sun", "0 2 * * tue-sat", "0 23 * * sun", "45 * * * *", "0 7,11 * * *", "0 0 * * mon,wed", "0 9 * * sun,sat"] {
            assert!(Cron::parse(utc).is_ok(), "the platform reads {utc}");
        }
        assert_eq!(day_names(&[1, 3].into()), "mon,wed");
        assert_eq!(day_names(&[0, 6].into()), "sun,sat");
        assert_eq!(days_of("5-7"), Some([5, 6, 0].into()), "Friday to Sunday");
        assert_eq!(days_of("*/2"), None);
    }

    #[test]
    fn each_job_its_suggestion() {
        let jobs = jobs_of(&json!({"jobs": [
            {"id": "a", "name": "Morning", "prompt": "Brief me. Use fsite to post it.", "schedule": {"kind": "cron", "expr": "0 8 * * 1-5"}},
            {"id": "b", "schedule": {"kind": "interval", "minutes": 5}},
            {"id": "c", "schedule": {"kind": "interval", "minutes": 30}},
            {"id": "d", "enabled": false, "schedule": {"kind": "cron", "expr": "0 8 * * *"}},
            {"id": "e", "no_agent": true, "script": "check.sh", "schedule": {"kind": "cron", "expr": "0 8 * * *"}},
            {"id": "f", "schedule": {"kind": "once", "run_at": "2027-01-01T09:00:00"}},
            {"id": "g", "schedule": {"kind": "once", "run_at": "2025-01-01T09:00:00Z"}},
            {"id": "h", "schedule": {"kind": "cron", "expr": "*/5 * * * *"}},
            {"id": "i", "schedule": {"kind": "weird"}},
            {"id": "j", "prompt": "x".repeat(TEXT_MAX_BYTES + 1), "schedule": {"kind": "interval", "minutes": 1440}}
        ]}));
        let r = routines(&jobs, Some("America/New_York"), NOW, &["fsite".to_string()]);
        let got: Vec<(Option<&str>, Suggestion, Option<&str>)> = r.jobs.iter().map(|j| (j.id.as_deref(), j.suggestion, j.schedule_utc.as_deref())).collect();
        assert_eq!(got, vec![
            (Some("a"), Suggestion::Routine, Some("0 13 * * mon-fri")),
            (Some("b"), Suggestion::TooFrequent, None),
            (Some("c"), Suggestion::Routine, Some("*/30 * * * *")),
            (Some("d"), Suggestion::Paused, None),
            (Some("e"), Suggestion::NoAgent, None),
            (Some("f"), Suggestion::OnceFuture, None),
            (Some("g"), Suggestion::OncePast, None),
            (Some("h"), Suggestion::TooFrequent, None),
            (Some("i"), Suggestion::ConvertByHand, None),
            (Some("j"), Suggestion::Routine, Some("0 0 * * *")),
        ]);
        assert_eq!((r.zone.as_str(), r.dst, r.routines), ("America/New_York", Some(true), 3));
        assert_eq!(r.jobs[0].attention.len(), 1, "its prompt names the old platform");
        assert!(r.jobs[9].text_too_long && !r.jobs[0].text_too_long);
        assert!(r.jobs[0].said.as_deref().is_some_and(|s| s.contains("13:00 UTC")), "{:?}", r.jobs[0].said);
        assert_eq!(routines(&jobs, Some("Mars/Olympus"), NOW, &[]).jobs[0].suggestion, Suggestion::ConvertByHand, "an unknown zone");
        assert_eq!(routines(&jobs, None, NOW, &[]).dst, Some(false), "UTC by default");
        // a list keyed by id reads the same
        assert_eq!(jobs_of(&json!({"x": {"schedule": {"kind": "interval", "minutes": 60}}}))[0]["id"], "x");
    }

    #[test]
    fn past_the_most_an_agent_keeps() {
        let jobs: Vec<Value> = (0..RECORD_SCHEDULES_MAX + 3).map(|i| json!({"id": i.to_string(), "schedule": {"kind": "interval", "minutes": 60}})).collect();
        let r = routines(&jobs, None, NOW, &[]);
        assert_eq!(r.routines, RECORD_SCHEDULES_MAX);
        assert_eq!(r.jobs.iter().filter(|j| j.suggestion == Suggestion::TooMany).count(), 3);
        assert_eq!(r.jobs[RECORD_SCHEDULES_MAX].suggestion, Suggestion::TooMany, "the first listed are kept");
    }
}
