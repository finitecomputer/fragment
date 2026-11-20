//! `GET /api/admin/glance` (docs/api.md, Operators): the operators'
//! glance at who joined and what runs. The registry answers who joined and
//! signed in; the deployment's Workers Logs, read through Cloudflare's
//! Workers Observability with a read-only key (`keys::observability_query`),
//! answer what runs: computers awake, fragments that ran, and computers in
//! trouble, with why a save failed (`fragment_core::glance`). No person's
//! object is called. What the logs answered is kept by the registry, a
//! part at a time (`glance::Part`): a load reads a part again only when
//! the registry says it is due and holds it for that load, and otherwise
//! shows what was kept, with when it was read. The week's failing hides
//! nothing else: its count is then unknown.

use fragment_core::glance::{self as core, Part, Question, Shown};
use fragment_proto::org::{AdminGlance, GlanceLogs, GlanceTrouble, TroubleKind, TroubleWhy};
use serde::{Deserialize, Serialize};
use worker::{Env, Url};

use crate::config::{Config, ObservabilityConfig};
use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::{ask_registry, js};

/// What the logs said of the now part (`Part::Now`), before the registry
/// names whose each computer is: as kept, its JSON.
#[derive(Serialize, Deserialize)]
struct Now {
    awake_now: u64,
    awake_today: u64,
    /// The computers awake since the day's start (at most `core::GROUPS_MAX`).
    computers_today: Vec<String>,
    fragments_now: u64,
    fragments_today: u64,
    trouble: Vec<(String, TroubleKind, u64)>,
    /// Each computer's newest failed save's reason, and when, of the lines
    /// read (`core::whys_of`), or why they were not read.
    whys: Result<Vec<(String, i64, String)>, String>,
}

/// The week's (`Part::Week`): the computers awake in the last week (at
/// most `core::GROUPS_MAX`).
type Week = Vec<String>;

/// The glance now, the day starting at the operator's `dayStart`.
pub(crate) async fn glance(env: &Env, cfg: &Config, url: &Url) -> CellResult<AdminGlance> {
    let now = js::now_ms();
    let named = url
        .query_pairs()
        .find(|(k, _)| k == "dayStart")
        .map(|(_, v)| v.parse::<i64>().map_err(|_| CellError::invalid("dayStart is the start of today, in ms")))
        .transpose()?;
    let day_start = core::day_start(named, now).map_err(CellError::invalid)?;
    let mut read = vec![];
    let logs = match &cfg.observability {
        None => Err(None),
        Some(o) => logs(env, o, now, day_start, &mut read).await?.map_err(Some),
    };
    // the computers in trouble, each once, for the registry to say whose
    let mut computers = logs.as_ref().map(|(n, _)| n.answer.trouble.iter().map(|(c, _, _)| c.clone()).collect::<Vec<_>>()).unwrap_or_default();
    computers.sort();
    computers.dedup();
    let g = ask_registry(env, &admin::Glance { now_ms: now, day_start, computers, read }).await?;
    let mut new_people = g.new_people;
    if let Ok((n, w)) = &logs {
        // awake this week: as the week's says, or since, today; with the
        // week's unread, only today's says yes
        for p in &mut new_people {
            let today = n.answer.computers_today.contains(&p.computer);
            p.computer_up = match w {
                Ok(w) => Some(today || w.answer.contains(&p.computer)),
                Err(_) => today.then_some(true),
            };
        }
    }
    let logs = match logs {
        Err(None) => GlanceLogs::NotConfigured,
        Err(Some(why)) => GlanceLogs::Failed { why },
        Ok((n, w)) => GlanceLogs::Read {
            at: n.at,
            week_at: w.as_ref().ok().map(|w| w.at),
            why_kept: n.why_kept.or(match &w {
                Ok(w) => w.why_kept.clone(),
                Err(why) => Some(why.clone()),
            }),
            why_unread: n.answer.whys.as_ref().err().cloned(),
            computers_awake_now: n.answer.awake_now,
            computers_awake_today: n.answer.awake_today,
            // the week's as kept (an hour), with today's computers since
            // (each list at most `GROUPS_MAX`)
            computers_awake_week: w.as_ref().ok().map(|w| (w.answer.len() + n.answer.computers_today.iter().filter(|c| !w.answer.contains(c)).count()) as u64),
            fragments_awake_now: n.answer.fragments_now,
            fragments_awake_today: n.answer.fragments_today,
            trouble: n
                .answer
                .trouble
                .into_iter()
                .map(|(computer, kind, lines)| {
                    let owner = g.owners.iter().find(|(c, _, _)| *c == computer);
                    let why = match kind {
                        TroubleKind::SavesFailing => n.answer.whys.iter().flatten().find(|(c, _, _)| *c == computer).map(|(_, at, text)| TroubleWhy { at: *at, text: text.clone() }),
                        TroubleKind::WontWake | TroubleKind::DiskFilling => None,
                    };
                    GlanceTrouble { npub: owner.map(|(_, n, _)| n.clone()), email: owner.and_then(|(_, _, e)| e.clone()), computer, kind, lines, why }
                })
                .collect(),
        },
    };
    Ok(AdminGlance { at: now, signups: g.signups, signed_in: g.signed_in, new_people, logs })
}

/// The logs as this load shows them: each part read again when the
/// registry holds it for this load (`admin::GlanceTake`), else as it kept
/// it (`core::shown_part`); or why the now part is not shown. The week's
/// alone may be unknown (and why). What was read again is put in `read`,
/// with its hold, for the registry to keep (`admin::Glance`).
#[allow(clippy::type_complexity)]
async fn logs(env: &Env, o: &ObservabilityConfig, now: i64, day_start: i64, read: &mut Vec<(String, i64, Option<String>)>) -> CellResult<Result<(Shown<Now>, Result<Shown<Week>, String>), String>> {
    let taken = ask_registry(env, &admin::GlanceTake { day_start }).await?;
    let [n, w] = <&[admin::TakenPart; 2]>::try_from(taken.parts.as_slice()).map_err(|_| CellError::host("the registry keeps two parts of the glance"))?;
    // the week's scan beside the now part's queries: `ASKED_AT_ONCE` at most
    let (now_read, week_read) = futures_util::future::join(
        async { if n.held.is_some() { Some(read_now(env, o, now, day_start).await) } else { None } },
        async { if w.held.is_some() { Some(read_week(env, o, now, day_start).await) } else { None } },
    )
    .await;
    read.extend(n.held.zip(now_read.as_ref()).map(|(held, r)| (Part::Now.key().to_string(), held, r.as_ref().ok().map(json))));
    read.extend(w.held.zip(week_read.as_ref()).map(|(held, r)| (Part::Week.key().to_string(), held, r.as_ref().ok().map(json))));
    let n = core::shown_part(now_read, kept(&n.kept, Part::Now), now);
    let w = core::shown_part(week_read, kept(&w.kept, Part::Week), now);
    Ok(n.map(|n| (n, w)))
}

fn json<T: Serialize>(answer: &T) -> String {
    serde_json::to_string(answer).expect("a part's answer is JSON")
}

/// A part's kept answer: when it was read, and it, or why it does not decode.
fn kept<T: serde::de::DeserializeOwned>(kept: &Option<(i64, String)>, part: Part) -> Option<(i64, Result<T, String>)> {
    kept.as_ref().map(|(at, answer)| (*at, crate::json_text(answer).map_err(|e| format!("the glance's kept {} does not decode: {e}", part.key()))))
}

/// One question's answer, or why not: the query API's, or its own error.
async fn ask(env: &Env, o: &ObservabilityConfig, q: Question, now: i64, day_start: i64) -> Result<serde_json::Value, String> {
    crate::keys::observability_query(env, o, &q.body(&o.script, now, day_start)).await.map_err(|e| e.message)
}

/// Why a question's answer read as nothing.
fn why(q: Question) -> impl Fn(String) -> String {
    move |e| format!("Workers Observability, {q:?}: {e}")
}

/// The now part, its questions `ASKED_AT_ONCE - 1` at a time: what the
/// logs say, or the first counting question's failure, after which nothing
/// more is asked. Why saves failed is the lines themselves, which may run
/// past the cell's bound on an answer: its read failing leaves the rest,
/// and says why (`why_unread`).
async fn read_now(env: &Env, o: &ObservabilityConfig, now: i64, day_start: i64) -> Result<Now, String> {
    let mut read = Now { awake_now: 0, awake_today: 0, computers_today: vec![], fragments_now: 0, fragments_today: 0, trouble: vec![], whys: Ok(vec![]) };
    let mut kinds = Vec::with_capacity(3);
    for asked in Part::Now.questions().chunks(core::ASKED_AT_ONCE - 1) {
        // bounded: one answer per question
        let answers = futures_util::future::join_all(asked.iter().map(|&q| async move { (q, ask(env, o, q, now, day_start).await) })).await;
        for (q, answered) in answers {
            let why = why(q);
            match q {
                Question::AwakeNow => read.awake_now = core::uniq_of(&answered?).map_err(why)?,
                Question::AwakeToday => read.awake_today = core::uniq_of(&answered?).map_err(why)?,
                Question::ComputersToday => read.computers_today = core::by_computer_of(&answered?).map_err(why)?.into_iter().map(|(c, _)| c).collect(),
                Question::FragmentsNow => read.fragments_now = core::uniq_of(&answered?).map_err(why)?,
                Question::FragmentsToday => read.fragments_today = core::uniq_of(&answered?).map_err(why)?,
                Question::WontWake => kinds.push((TroubleKind::WontWake, core::by_computer_of(&answered?).map_err(why)?)),
                Question::SavesFailing => kinds.push((TroubleKind::SavesFailing, core::by_computer_of(&answered?).map_err(why)?)),
                Question::DiskFilling => kinds.push((TroubleKind::DiskFilling, core::by_computer_of(&answered?).map_err(why)?)),
                Question::SaveWhys => read.whys = answered.and_then(|a| core::whys_of(&a).map_err(why)),
                Question::AwakeWeek => unreachable!("the week's is its own part"),
            }
        }
    }
    read.trouble = core::troubles(kinds);
    Ok(read)
}

/// The week's part: the computers awake in it, or why not.
async fn read_week(env: &Env, o: &ObservabilityConfig, now: i64, day_start: i64) -> Result<Week, String> {
    let why = why(Question::AwakeWeek);
    let answered = ask(env, o, Question::AwakeWeek, now, day_start).await?;
    Ok(core::by_computer_of(&answered).map_err(why)?.into_iter().map(|(c, _)| c).collect())
}
