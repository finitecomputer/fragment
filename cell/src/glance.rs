//! `GET /api/admin/glance` (docs/api.md, Operators): the operators'
//! glance at who joined and what runs. The registry answers who joined and
//! signed in; the deployment's Workers Logs, read through Cloudflare's
//! Workers Observability with a read-only key (`keys::observability_query`),
//! answer what runs: computers awake, fragments that ran, and computers in
//! trouble (`fragment_core::glance`). No person's object is called. Each
//! read asks afresh: the page asks again every minute, and only operators
//! open it.

use fragment_core::glance::{self as core, Question};
use fragment_proto::org::{AdminGlance, GlanceLogs, GlanceTrouble, TroubleKind};
use worker::{Env, Url};

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::{ask_registry, js};

/// What the logs said, before the registry names whose each computer is.
struct Read {
    awake_now: u64,
    awake_today: u64,
    /// The computers awake in the last week (at most `core::GROUPS_MAX`).
    awake_week: Vec<String>,
    fragments_now: u64,
    fragments_today: u64,
    trouble: Vec<(String, TroubleKind, u64)>,
}

/// The glance now, the day starting at the operator's `dayStart`.
pub(crate) async fn glance(env: &Env, cfg: &Config, url: &Url) -> CellResult<AdminGlance> {
    let now = js::now_ms();
    let named = url
        .query_pairs()
        .find(|(k, _)| k == "dayStart")
        .map(|(_, v)| v.parse::<i64>().map_err(|_| CellError::invalid("dayStart is the start of today, in ms")))
        .transpose()?;
    let day_start = core::day_start(named, now).map_err(CellError::invalid)?;
    let logs = match &cfg.observability {
        None => Err(None),
        Some(o) => read(env, o, now, day_start).await.map_err(|e| Some(e.message)),
    };
    // the computers in trouble, each once, for the registry to say whose
    let mut computers = logs.as_ref().map(|r| r.trouble.iter().map(|(c, _, _)| c.clone()).collect::<Vec<_>>()).unwrap_or_default();
    computers.sort();
    computers.dedup();
    let g = ask_registry(env, &admin::Glance { now_ms: now, day_start, computers }).await?;
    let mut new_people = g.new_people;
    if let Ok(r) = &logs {
        for p in &mut new_people {
            p.computer_up = Some(r.awake_week.contains(&p.computer));
        }
    }
    let logs = match logs {
        Err(None) => GlanceLogs::NotConfigured,
        Err(Some(why)) => GlanceLogs::Failed { why },
        Ok(r) => GlanceLogs::Read {
            computers_awake_now: r.awake_now,
            computers_awake_today: r.awake_today,
            computers_awake_week: r.awake_week.len() as u64,
            fragments_awake_now: r.fragments_now,
            fragments_awake_today: r.fragments_today,
            trouble: r
                .trouble
                .into_iter()
                .map(|(computer, kind, lines)| {
                    let owner = g.owners.iter().find(|(c, _, _)| *c == computer);
                    GlanceTrouble { npub: owner.map(|(_, n, _)| n.clone()), email: owner.and_then(|(_, _, e)| e.clone()), computer, kind, lines }
                })
                .collect(),
        },
    };
    Ok(AdminGlance { at: now, signups: g.signups, signed_in: g.signed_in, new_people, logs })
}

/// Every question, asked together: what the logs say, or the first
/// question's failure.
async fn read(env: &Env, o: &crate::config::ObservabilityConfig, now: i64, day_start: i64) -> CellResult<Read> {
    let asked = Question::ALL.map(|q| async move {
        let answer = crate::keys::observability_query(env, o, &q.body(&o.script, now, day_start)).await?;
        Ok::<_, CellError>((q, answer))
    });
    let mut read = Read { awake_now: 0, awake_today: 0, awake_week: vec![], fragments_now: 0, fragments_today: 0, trouble: vec![] };
    let mut kinds = Vec::with_capacity(3);
    // bounded: one answer per question
    for answered in futures_util::future::join_all(asked).await {
        let (q, answer) = answered?;
        let why = |e: String| CellError::new(fragment_proto::ErrorCode::UpstreamFailed, format!("Workers Observability, {q:?}: {e}"));
        match q {
            Question::AwakeNow => read.awake_now = core::uniq_of(&answer).map_err(why)?,
            Question::AwakeToday => read.awake_today = core::uniq_of(&answer).map_err(why)?,
            Question::AwakeWeek => read.awake_week = core::by_computer_of(&answer).map_err(why)?.into_iter().map(|(c, _)| c).collect(),
            Question::FragmentsNow => read.fragments_now = core::uniq_of(&answer).map_err(why)?,
            Question::FragmentsToday => read.fragments_today = core::uniq_of(&answer).map_err(why)?,
            Question::WontWake => kinds.push((TroubleKind::WontWake, core::by_computer_of(&answer).map_err(why)?)),
            Question::SavesFailing => kinds.push((TroubleKind::SavesFailing, core::by_computer_of(&answer).map_err(why)?)),
            Question::DiskFilling => kinds.push((TroubleKind::DiskFilling, core::by_computer_of(&answer).map_err(why)?)),
        }
    }
    read.trouble = core::troubles(kinds);
    Ok(read)
}
