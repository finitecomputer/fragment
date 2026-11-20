//! `GET /api/admin/glance` (docs/api.md, Operators): the operators'
//! glance at who joined and what runs. The registry answers who joined and
//! signed in; the deployment's Workers Logs, read through Cloudflare's
//! Workers Observability with a read-only key (`keys::observability_query`),
//! answer what runs: computers awake, fragments that ran, and computers in
//! trouble, with why a save failed (`fragment_core::glance`). No person's
//! object is called. What the logs answered is kept by the registry: a
//! load reads them again only when the registry holds them for it, and
//! otherwise shows what was kept, with when it was read. The week's
//! computers are kept apart, the last scan's with every computer seen
//! today since.

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
    let mut asked = admin::Glance { now_ms: now, day_start, computers: vec![], held: None, read: None, week: None, week_at: None };
    let (mut kept, mut fresh, mut week_unread) = (None, None, None);
    // one call site, asked again only once this load read the logs
    let mut g = loop {
        let g = ask_registry(env, &asked).await?;
        if asked.held.is_some() {
            break g;
        }
        // today's numbers kept are shown of their own day's start alone
        let answer = g.kept.as_deref().map(|a| Logs::of_kept(a).ok_or_else(|| "the glance's kept answer does not decode".to_string()));
        kept = g.kept_at.filter(|_| g.kept_day == Some(day_start)).zip(answer);
        let (Some(o), Some(held)) = (&cfg.observability, g.held) else {
            break g;
        };
        let (counts, scanned) = read(env, o, now, day_start, core::week_due(g.week_at, now, day_start)).await;
        // the week to keep: a scan's that read (though the rest failed),
        // else the one kept; either with today's computers since
        let week = match scanned {
            Some(Ok(scan)) => Some((scan, now)),
            Some(Err(why)) => {
                week_unread = Some(why);
                lines(&g.week).zip(g.week_at)
            }
            None => lines(&g.week).zip(g.week_at),
        };
        let week = week.map(|(w, at)| match &counts {
            Ok(c) => (core::week_with(w, &c.computers_today), at),
            Err(_) => (w, at),
        });
        asked = admin::Glance {
            now_ms: now,
            day_start,
            computers: counts.as_ref().map(|c| c.trouble.iter().map(|(c, _, _)| c.clone()).collect()).unwrap_or_default(),
            held: Some(held),
            read: counts.as_ref().ok().map(Logs::kept),
            week: week.as_ref().map(|(w, _)| w.join("\n")),
            week_at: week.map(|(_, at)| at),
        };
        fresh = Some(counts);
    };
    let mut new_people = std::mem::take(&mut g.new_people);
    let logs = match &cfg.observability {
        None => Err(None),
        Some(_) => core::shown_answer(fresh, kept, now).map_err(Some),
    };
    // the week as kept (this load's, when it read), with today's since
    let week = logs.as_ref().ok().and_then(|n| lines(&g.week).map(|w| core::week_with(w, &n.answer.computers_today)));
    if let Ok(n) = &logs {
        // awake this week: as the week says (with today's); with the week
        // never read, only today's says yes
        for p in &mut new_people {
            p.computer_up = match &week {
                Some(week) => Some(week.contains(&p.computer)),
                None => n.answer.computers_today.contains(&p.computer).then_some(true),
            };
        }
    }
    let logs = match logs {
        Err(None) => GlanceLogs::NotConfigured,
        Err(Some(why)) => GlanceLogs::Failed { why },
        Ok(n) => GlanceLogs::Read {
            at: n.at,
            week_at: g.week_at.filter(|_| week.is_some()),
            why_kept: n.why_kept.or(week_unread),
            why_unread: n.answer.whys_unread,
            computers_awake_now: n.answer.awake_now,
            computers_awake_today: n.answer.awake_today,
            computers_awake_week: week.as_ref().map(|w| w.len() as u64),
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

/// The registry's week, one computer a line, as a list.
fn lines(week: &Option<String>) -> Option<Vec<String>> {
    week.as_ref().map(|w| w.split('\n').filter(|c| !c.is_empty()).map(str::to_string).collect())
}

/// The logs read: `Question::ALL` in order, `ASKED_AT_ONCE` at a time, the
/// week's scan first and only when due. The counts stop at a counting
/// question's failure, after which nothing more is asked; the scan, asked
/// first, is answered either way (none: not asked). Why saves failed is
/// the lines themselves, which may run past the cell's bound on an answer:
/// its read failing alone leaves the rest, and says why.
async fn read(env: &Env, o: &ObservabilityConfig, now: i64, day_start: i64, week_due: bool) -> (Result<Logs, String>, Option<Result<Vec<String>, String>>) {
    let asked = if week_due { &Question::ALL[..] } else { &Question::ALL[1..] };
    let (mut logs, mut kinds, mut scanned) = (Logs::default(), Vec::with_capacity(3), None);
    for chunk in asked.chunks(core::ASKED_AT_ONCE) {
        // bounded: one answer per question
        let answers = futures_util::future::join_all(chunk.iter().map(|&q| async move { (q, crate::keys::observability_query(env, o, &q.body(&o.script, now, day_start)).await.map_err(|e| e.message)) })).await;
        for (q, answered) in answers {
            let why = |e: String| format!("Workers Observability, {q:?}: {e}");
            let computers = |a: &serde_json::Value| core::by_computer_of(a).map(|c| c.into_iter().map(|(c, _)| c).collect::<Vec<_>>()).map_err(why);
            let counted = match (q, answered) {
                (Question::AwakeWeek, answered) => {
                    scanned = Some(answered.and_then(|a| computers(&a)));
                    Ok(())
                }
                (Question::SaveWhys, answered) => {
                    match answered.and_then(|a| core::whys_of(&a).map_err(why)) {
                        Ok(whys) => logs.whys = whys,
                        Err(unread) => logs.whys_unread = Some(unread),
                    }
                    Ok(())
                }
                (_, Err(failed)) => Err(failed),
                (Question::AwakeNow, Ok(a)) => core::uniq_of(&a).map(|n| logs.awake_now = n).map_err(why),
                (Question::AwakeToday, Ok(a)) => core::uniq_of(&a).map(|n| logs.awake_today = n).map_err(why),
                (Question::ComputersToday, Ok(a)) => computers(&a).map(|c| logs.computers_today = c),
                (Question::FragmentsNow, Ok(a)) => core::uniq_of(&a).map(|n| logs.fragments_now = n).map_err(why),
                (Question::FragmentsToday, Ok(a)) => core::uniq_of(&a).map(|n| logs.fragments_today = n).map_err(why),
                (Question::WontWake, Ok(a)) => core::by_computer_of(&a).map(|c| kinds.push((TroubleKind::WontWake, c))).map_err(why),
                (Question::SavesFailing, Ok(a)) => core::by_computer_of(&a).map(|c| kinds.push((TroubleKind::SavesFailing, c))).map_err(why),
                (Question::DiskFilling, Ok(a)) => core::by_computer_of(&a).map(|c| kinds.push((TroubleKind::DiskFilling, c))).map_err(why),
            };
            if let Err(failed) = counted {
                return (Err(failed), scanned);
            }
        }
    }
    logs.trouble = core::troubles(kinds);
    (Ok(logs), scanned)
}
