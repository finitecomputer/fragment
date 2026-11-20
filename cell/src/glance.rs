//! `GET /api/admin/glance` (docs/api.md, Operators): the operators'
//! glance at who joined and what runs. The registry answers who joined and
//! signed in; the deployment's Workers Logs, read through Cloudflare's
//! Workers Observability with a read-only key (`keys::observability_query`),
//! answer what runs: computers awake, fragments that ran, and computers in
//! trouble, with why a save failed (`fragment_core::glance`). No person's
//! object is called. What the logs answered is kept by the registry: a
//! load reads them again only when the registry holds them for it, and
//! otherwise shows what was kept, with when it was read.

use fragment_core::glance::{self as core, Logs, Question};
use fragment_proto::org::{AdminGlance, GlanceLogs, GlanceTrouble, TroubleKind, TroubleWhy};
use worker::{Env, Url};

use crate::config::{Config, ObservabilityConfig};
use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::{ask_registry, js};

/// The glance now, the day starting at the operator's `dayStart`: the
/// registry asked once, and again when this load read the logs (to keep
/// them, end its hold, and say whose their trouble is).
pub(crate) async fn glance(env: &Env, cfg: &Config, url: &Url) -> CellResult<AdminGlance> {
    let now = js::now_ms();
    let named = url
        .query_pairs()
        .find(|(k, _)| k == "dayStart")
        .map(|(_, v)| v.parse::<i64>().map_err(|_| CellError::invalid("dayStart is the start of today, in ms")))
        .transpose()?;
    let day_start = core::day_start(named, now).map_err(CellError::invalid)?;
    let mut asked = admin::Glance { now_ms: now, day_start, computers: vec![], held: None, read: None };
    let (mut kept, mut fresh) = (None, None);
    // one call site, asked again only once this load read the logs
    let mut g = loop {
        let g = ask_registry(env, &asked).await?;
        if asked.held.is_some() {
            break g;
        }
        let answer = g.kept.as_deref().map(|a| Logs::of_kept(a).ok_or_else(|| "the glance's kept answer does not decode".to_string()));
        let (Some(o), Some(held)) = (&cfg.observability, g.held) else {
            kept = shows(&g, day_start).zip(answer);
            break g;
        };
        // the week kept is carried, of any day's answer, until it is due
        let week = answer.as_ref().and_then(|a| a.as_ref().ok()).map(|a| (a.week.clone(), a.week_at));
        let logs_read = read(env, o, now, day_start, week).await;
        asked = admin::Glance {
            now_ms: now,
            day_start,
            computers: logs_read.as_ref().map(|r| r.trouble.iter().map(|(c, _, _)| c.clone()).collect()).unwrap_or_default(),
            held: Some(held),
            read: logs_read.as_ref().ok().map(Logs::kept),
        };
        kept = shows(&g, day_start).zip(answer);
        fresh = Some(logs_read);
    };
    let mut new_people = std::mem::take(&mut g.new_people);
    let logs = match &cfg.observability {
        None => Err(None),
        Some(_) => core::shown_answer(fresh, kept, now).map_err(Some),
    };
    if let Ok(n) = &logs {
        // awake this week: as the week's says, or since, today; with the
        // week's never read, only today's says yes
        for p in &mut new_people {
            let today = n.answer.computers_today.contains(&p.computer);
            p.computer_up = match &n.answer.week {
                Some(week) => Some(today || week.contains(&p.computer)),
                None => today.then_some(true),
            };
        }
    }
    let logs = match logs {
        Err(None) => GlanceLogs::NotConfigured,
        Err(Some(why)) => GlanceLogs::Failed { why },
        Ok(n) => GlanceLogs::Read {
            at: n.at,
            week_at: n.answer.week_at,
            why_kept: n.why_kept.or(n.answer.week_unread),
            why_unread: n.answer.whys_unread,
            computers_awake_now: n.answer.awake_now,
            computers_awake_today: n.answer.awake_today,
            // the week's as read, with today's computers since (each list
            // at most `GROUPS_MAX`)
            computers_awake_week: n.answer.week.as_ref().map(|w| (w.len() + n.answer.computers_today.iter().filter(|c| !w.contains(c)).count()) as u64),
            fragments_awake_now: n.answer.fragments_now,
            fragments_awake_today: n.answer.fragments_today,
            trouble: n
                .answer
                .trouble
                .into_iter()
                .map(|(computer, kind, lines)| {
                    let owner = g.owners.iter().find(|(c, _, _)| *c == computer);
                    let why = match kind {
                        TroubleKind::SavesFailing => n.answer.whys.iter().find(|(c, _, _)| *c == computer).map(|(_, at, text)| TroubleWhy { at: *at, text: text.clone() }),
                        TroubleKind::WontWake | TroubleKind::DiskFilling => None,
                    };
                    GlanceTrouble { npub: owner.map(|(_, n, _)| n.clone()), email: owner.and_then(|(_, _, e)| e.clone()), computer, kind, lines, why }
                })
                .collect(),
        },
    };
    Ok(AdminGlance { at: now, signups: g.signups, signed_in: g.signed_in, new_people, logs })
}

/// When the answer kept was read, if this load shows it: today's numbers
/// of its own day's start alone.
fn shows(g: &admin::Glanced, day_start: i64) -> Option<i64> {
    g.kept_at.filter(|_| g.kept_day == Some(day_start))
}

/// The logs read: `Question::ALL` in order, `ASKED_AT_ONCE` at a time, the
/// week's first and only when due (else carried, as kept). The read stops
/// at a counting question's failure, after which nothing more is asked.
/// The week's failing alone keeps the week carried, and says why; why
/// saves failed is the lines themselves, which may run past the cell's
/// bound on an answer: its read failing alone leaves the rest, and says why.
async fn read(env: &Env, o: &ObservabilityConfig, now: i64, day_start: i64, week: Option<(Option<Vec<String>>, Option<i64>)>) -> Result<Logs, String> {
    let (week, week_at) = week.unwrap_or_default();
    let asked = if core::week_due(week_at, now, day_start) { &Question::ALL[..] } else { &Question::ALL[1..] };
    let mut logs = Logs { week, week_at, ..Logs::default() };
    let mut kinds = Vec::with_capacity(3);
    for chunk in asked.chunks(core::ASKED_AT_ONCE) {
        // bounded: one answer per question
        let answers = futures_util::future::join_all(chunk.iter().map(|&q| async move { (q, crate::keys::observability_query(env, o, &q.body(&o.script, now, day_start)).await.map_err(|e| e.message)) })).await;
        for (q, answered) in answers {
            let why = |e: String| format!("Workers Observability, {q:?}: {e}");
            let computers = |a: &serde_json::Value| core::by_computer_of(a).map(|c| c.into_iter().map(|(c, _)| c).collect::<Vec<_>>()).map_err(why);
            match (q, answered) {
                (Question::AwakeWeek, answered) => match answered.and_then(|a| computers(&a)) {
                    Ok(c) => (logs.week, logs.week_at) = (Some(c), Some(now)),
                    Err(unread) => logs.week_unread = Some(unread),
                },
                (Question::SaveWhys, answered) => match answered.and_then(|a| core::whys_of(&a).map_err(why)) {
                    Ok(whys) => logs.whys = whys,
                    Err(unread) => logs.whys_unread = Some(unread),
                },
                (_, Err(failed)) => return Err(failed),
                (Question::AwakeNow, Ok(a)) => logs.awake_now = core::uniq_of(&a).map_err(why)?,
                (Question::AwakeToday, Ok(a)) => logs.awake_today = core::uniq_of(&a).map_err(why)?,
                (Question::ComputersToday, Ok(a)) => logs.computers_today = computers(&a)?,
                (Question::FragmentsNow, Ok(a)) => logs.fragments_now = core::uniq_of(&a).map_err(why)?,
                (Question::FragmentsToday, Ok(a)) => logs.fragments_today = core::uniq_of(&a).map_err(why)?,
                (Question::WontWake, Ok(a)) => kinds.push((TroubleKind::WontWake, core::by_computer_of(&a).map_err(why)?)),
                (Question::SavesFailing, Ok(a)) => kinds.push((TroubleKind::SavesFailing, core::by_computer_of(&a).map_err(why)?)),
                (Question::DiskFilling, Ok(a)) => kinds.push((TroubleKind::DiskFilling, core::by_computer_of(&a).map_err(why)?)),
            }
        }
    }
    logs.trouble = core::troubles(kinds);
    Ok(logs)
}
