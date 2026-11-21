//! `GET /api/admin/glance` (docs/api.md, Operators): the operators'
//! glance at who joined and what runs. The registry answers who joined and
//! signed in; the deployment's Workers Logs, read through Cloudflare's
//! Workers Observability with a read-only key (`keys::observability_query`),
//! answer what runs: computers awake, fragments that ran, and computers in
//! trouble, with why a save failed (`fragment_core::glance`). No person's
//! object is called. The admin Worker's alone (admin.rs): every
//! fragment's isolate holds neither this nor the key it reads with.
//!
//! The page asks again every minute, from each tab and operator open, and
//! the query API is limited per account, so the logs' numbers are kept in
//! the Cache API (`caches.default`, this data center's) for a short while
//! (`core::kept`): its source is the logs, it is let go on its own
//! (`core::LOGS_KEPT_MS`, its `max-age`), and a read past
//! `core::LOGS_FRESH_MS` is read again. A kept read is always answered
//! with when it was read (`at`), and, standing in for a newer read that
//! failed, with why (`whyKept`), so no age hides a problem. The registry's
//! numbers are read every load. A cache that keeps nothing (a node
//! without one) makes every load read, as before.

use fragment_core::glance::{self as core, Question};
use fragment_proto::org::{AdminGlance, GlanceLogs, GlanceTrouble, TroubleKind, TroubleWhy};
use futures_util::StreamExt;
use worker::{console_error, Cache, Env, Response, Url};

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::{ask_registry, js};

/// What the logs said, before the registry names whose each computer is:
/// kept in the cache as it is, with when it was read (`keep`).
#[derive(serde::Serialize, serde::Deserialize)]
struct Read {
    awake_now: u64,
    awake_today: u64,
    /// The computers awake in the last week (at most `core::GROUPS_MAX`).
    awake_week: Vec<String>,
    fragments_now: u64,
    fragments_today: u64,
    trouble: Vec<(String, TroubleKind, u64)>,
    /// Each computer's newest failed save's reason, and when, of the lines
    /// read (`core::whys_of`), or why they were not read.
    whys: Result<Vec<(String, i64, String)>, String>,
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
        Some(o) => logs(env, o, now, day_start).await.map_err(Some),
    };
    // the computers in trouble, each once, for the registry to say whose
    let mut computers = logs.as_ref().map(|(r, _, _)| r.trouble.iter().map(|(c, _, _)| c.clone()).collect::<Vec<_>>()).unwrap_or_default();
    computers.sort();
    computers.dedup();
    let g = ask_registry(env, &admin::Glance { now_ms: now, day_start, computers }).await?;
    let mut new_people = g.new_people;
    if let Ok((r, _, _)) = &logs {
        for p in &mut new_people {
            p.computer_up = Some(r.awake_week.contains(&p.computer));
        }
    }
    let logs = match logs {
        Err(None) => GlanceLogs::NotConfigured,
        Err(Some(why)) => GlanceLogs::Failed { why },
        Ok((r, at, why_kept)) => GlanceLogs::Read {
            at,
            why_kept,
            why_unread: r.whys.as_ref().err().cloned(),
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
                    let why = match kind {
                        TroubleKind::SavesFailing => r.whys.iter().flatten().find(|(c, _, _)| *c == computer).map(|(_, at, text)| TroubleWhy { at: *at, text: text.clone() }),
                        TroubleKind::WontWake | TroubleKind::DiskFilling => None,
                    };
                    GlanceTrouble { npub: owner.map(|(_, n, _)| n.clone()), email: owner.and_then(|(_, _, e)| e.clone()), computer, kind, lines, why }
                })
                .collect(),
        },
    };
    Ok(AdminGlance { at: now, signups: g.signups, signed_in: g.signed_in, new_people, logs })
}

/// The logs' numbers for the day from `day_start`, when they were read,
/// and why a newer read failed when they stand in for it: the cache's
/// while fresh (`core::kept`); else read now, and kept; a read that fails
/// stands on the cache's older one, if it is still under `LOGS_KEPT_MS`
/// when the read has failed (a read takes seconds), saying why, or is
/// that failure. Loads at once each read: the Cache API makes no load
/// wait for another's (a lock would be a Durable Object's), and each is
/// still at most `QUERIES_AT_ONCE_MAX` queries at once.
async fn logs(env: &Env, o: &crate::config::ObservabilityConfig, now: i64, day_start: i64) -> Result<(Read, i64, Option<String>), String> {
    let cache = Cache::default();
    // its shape's version: a shape this release cannot read is no kept read
    let key = format!("https://glance.admin.internal/v1/{}/{day_start}", o.script);
    let stale = match kept(&cache, &key).await {
        Some((at, read)) => match core::kept(at, now) {
            core::Kept::Fresh => return Ok((read, at, None)),
            core::Kept::Stale => Some((at, read)),
            core::Kept::Gone => None,
        },
        None => None,
    };
    match self::read(env, o, now, day_start).await {
        Ok(read) => {
            keep(&cache, &key, now, &read).await;
            Ok((read, now, None))
        }
        // judged again now: the read took its seconds
        Err(e) => match stale.filter(|(at, _)| core::kept(*at, js::now_ms()) != core::Kept::Gone) {
            Some((at, read)) => Ok((read, at, Some(e.message))),
            None => Err(e.message),
        },
    }
}

/// The read the cache keeps under `key`, and when it was read; none when
/// it keeps none, or one this release cannot read.
async fn kept(cache: &Cache, key: &str) -> Option<(i64, Read)> {
    let mut resp = match cache.get(key, true).await {
        Ok(found) => found?,
        Err(e) => {
            console_error!("{}", serde_json::json!({ "event": "glance.cache-unread", "error": e.to_string() }));
            return None;
        }
    };
    serde_json::from_slice(&resp.bytes().await.ok()?).ok()
}

/// `read`, kept under `key` from `at`, let go by the cache after
/// `core::LOGS_KEPT_MS`, unless the cache holds a newer read by then (a
/// load begun later that finished first): an older read never replaces a
/// newer one. One not kept is logged: the next load reads again.
async fn keep(cache: &Cache, key: &str, at: i64, read: &Read) {
    if kept(cache, key).await.is_some_and(|(newer, _)| newer > at) {
        return;
    }
    let kept = async {
        let body = serde_json::to_string(&(at, read)).map_err(|e| worker::Error::RustError(e.to_string()))?;
        let mut resp = Response::ok(body)?;
        resp.headers_mut().set("cache-control", &format!("max-age={}", core::LOGS_KEPT_MS / 1000))?;
        cache.put(key, resp).await
    };
    if let Err(e) = kept.await {
        console_error!("{}", serde_json::json!({ "event": "glance.cache-unkept", "error": e.to_string() }));
    }
}

/// Every question, `core::QUERIES_AT_ONCE_MAX` at once: what the logs say,
/// or the first counting question's failure, met as its answer comes, so
/// no question is asked after it. Why saves failed is the lines
/// themselves, which may run past the cell's bound on an answer: its read
/// failing leaves the rest, and says why (`why_unread`).
async fn read(env: &Env, o: &crate::config::ObservabilityConfig, now: i64, day_start: i64) -> CellResult<Read> {
    let asked = futures_util::stream::iter(Question::ALL).map(|q| async move { (q, crate::keys::observability_query(env, o, &q.body(&o.script, now, day_start)).await) });
    let mut read = Read { awake_now: 0, awake_today: 0, awake_week: vec![], fragments_now: 0, fragments_today: 0, trouble: vec![], whys: Ok(vec![]) };
    let mut kinds = Vec::with_capacity(3);
    let mut answers = asked.buffered(core::QUERIES_AT_ONCE_MAX);
    // bounded: one answer per question, in the questions' order; a `?`
    // below drops the rest, unasked
    while let Some((q, answered)) = answers.next().await {
        let why = |e: String| CellError::new(fragment_proto::ErrorCode::UpstreamFailed, format!("Workers Observability, {q:?}: {e}"));
        match q {
            Question::AwakeNow => read.awake_now = core::uniq_of(&answered?).map_err(why)?,
            Question::AwakeToday => read.awake_today = core::uniq_of(&answered?).map_err(why)?,
            Question::AwakeWeek => read.awake_week = core::by_computer_of(&answered?).map_err(why)?.into_iter().map(|(c, _)| c).collect(),
            Question::FragmentsNow => read.fragments_now = core::uniq_of(&answered?).map_err(why)?,
            Question::FragmentsToday => read.fragments_today = core::uniq_of(&answered?).map_err(why)?,
            Question::WontWake => kinds.push((TroubleKind::WontWake, core::by_computer_of(&answered?).map_err(why)?)),
            Question::SavesFailing => kinds.push((TroubleKind::SavesFailing, core::by_computer_of(&answered?).map_err(why)?)),
            Question::DiskFilling => kinds.push((TroubleKind::DiskFilling, core::by_computer_of(&answered?).map_err(why)?)),
            Question::SaveWhys => read.whys = answered.and_then(|a| core::whys_of(&a).map_err(why)).map_err(|e| e.message),
        }
    }
    read.trouble = core::troubles(kinds);
    Ok(read)
}
