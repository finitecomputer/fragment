//! Scheduled records (docs/api.md, "Scheduled records"): a fragment's data
//! schedules, which its editors add at run time, each appending one record
//! to one of its own channels, as the fragment itself, at each minute its
//! cron names. Records already drive everything: an app's channel triggers
//! run its jobs from one, its pages show it live, and a computer following
//! the channel wakes for it. An agent's routine is one on its own agent
//! fragment's `tasks` (`{kind: "routine", text, chat}`: docs/chat-records.md),
//! which its agent adds as the fragment's editor (Paul, 2026-10-20: "for
//! basic crons the agent shouldn't need to build a new fragment, it would
//! just add schedule to itself"); nothing here knows that.
//!
//! They are kept in `record_schedules`, beside `schedules`, which holds the
//! installed code's cron triggers and is made again at every install
//! (jobs.rs `sync_schedules`): these are the fragment's own data, written by
//! people and kept across installs. The alarm fires both in one pass
//! (`fire_cron`, whose second half is `fire_records` here), each moving on
//! by the one rule (`Cron::next_fire`). A fire appends under a key of its
//! schedule and the minute it was due (`fragment_core::schedule::fire_key`),
//! so a pass that fires it again appends nothing twice; one the alarm came
//! to late is one fire, the minutes it missed skipped; past the owner's
//! overdraft none fires, each moving on, and one event says so. Viewers
//! list them; editors (an agent for its owner among them, at most an
//! editor) add, pause, resume and remove them.
//!
//! The pure rules (the bounds, the channel's admission, the key, the words)
//! are `fragment_core::schedule`.

use fragment_core::cron::Cron;
use fragment_core::{npub, schedule};
use fragment_proto::ledger::Why;
use fragment_proto::{limits, AddSchedule, ErrorCode, Role, Schedule, ScheduleAdded, ScheduleList, SetSchedule, BUILTIN_CHANNELS};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{json_response, Caller, FragmentCell};
use crate::js;

/// A `record_schedules` row, as the cell wrote it.
#[derive(Deserialize)]
struct Row {
    id: i64,
    cron: String,
    channel: String,
    /// The record's body, as canonical JSON.
    body: String,
    created_by: String,
    created_at: i64,
    /// None while paused.
    next_at: Option<i64>,
    last_at: Option<i64>,
}

const COLUMNS: &str = "id, cron, channel, body, created_by, created_at, next_at, last_at";

impl Row {
    /// Only the cell writes `record_schedules`, from a checked cron.
    fn cron(&self) -> Cron {
        Cron::parse(&self.cron).unwrap_or_else(|e| panic!("record_schedules.cron is a checked schedule: {e}: {:?}", self.cron))
    }

    /// Only the cell writes `record_schedules`, from a JSON value.
    fn record(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("record_schedules.body is the JSON the cell wrote: {e}"))
    }

    fn view(self) -> Schedule {
        Schedule {
            said: schedule::said(&self.cron),
            record: self.record(),
            id: self.id,
            cron: self.cron,
            channel: self.channel,
            created_by: npub::display(&self.created_by),
            created_at: self.created_at,
            paused: self.next_at.is_none(),
            next_at: self.next_at,
            last_at: self.last_at,
        }
    }
}

fn schedule_id(id: &str) -> CellResult<i64> {
    id.parse::<i64>().ok().filter(|n| *n > 0).ok_or_else(|| CellError::invalid(format!("a schedule's id is a number (`fragment schedule list`), not {id:?}")))
}

impl FragmentCell {
    fn schedule_rows(&self) -> CellResult<Vec<Row>> {
        let rows: Vec<Row> = self.typed(&format!("SELECT {COLUMNS} FROM record_schedules ORDER BY id LIMIT ?"), vec![SqlStorageValue::Integer(limits::RECORD_SCHEDULES_MAX as i64 + 1)])?;
        assert!(rows.len() <= limits::RECORD_SCHEDULES_MAX, "a fragment keeps at most {} scheduled records", limits::RECORD_SCHEDULES_MAX);
        Ok(rows)
    }

    fn schedule_row(&self, id: i64) -> CellResult<Row> {
        let rows: Vec<Row> = self.typed(&format!("SELECT {COLUMNS} FROM record_schedules WHERE id = ?"), vec![SqlStorageValue::Integer(id)])?;
        rows.into_iter().next().ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no schedule #{id}")))
    }

    /// The schedule the same as `c` (its cron, channel and record), if one is kept.
    fn same_schedule(&self, c: &schedule::Checked) -> CellResult<Option<Row>> {
        let rows: Vec<Row> = self.typed(
            &format!("SELECT {COLUMNS} FROM record_schedules WHERE cron = ? AND channel = ? AND body = ? ORDER BY id LIMIT 1"),
            vec![c.expr.as_str().into(), c.channel.as_str().into(), c.body.as_str().into()],
        )?;
        Ok(rows.into_iter().next())
    }

    /// Its scheduled records, for its page (`__schedules`) and the API:
    /// viewers and up.
    pub(crate) fn schedule_list(&self, caller: &Caller) -> CellResult<ScheduleList> {
        self.require(caller, false, Role::Viewer)?;
        Ok(ScheduleList { schedules: self.schedule_rows()?.into_iter().map(Row::view).collect() })
    }

    /// `GET /api/f/<name>/schedules`
    pub(crate) fn schedules_api(&self, caller: &Caller) -> CellResult<Response> {
        json_response(&self.schedule_list(caller)?)
    }

    /// `POST /api/f/<name>/schedules` (editor, and a write: past the owner's
    /// overdraft it is refused): a schedule added, or the same one found
    /// (added again, it is that one). Its channel must take posts from
    /// whoever holds its post role, a role the caller holds.
    pub(crate) async fn add_schedule(&self, caller: &Caller, body: AddSchedule) -> CellResult<Response> {
        let role = self.require(caller, false, Role::Editor)?;
        let by = self.caller_id(caller)?.to_string();
        self.writable().await?;
        // after the standing's wait, everything to the insert is one turn
        let checked = schedule::check(&body)?;
        let builtin = BUILTIN_CHANNELS.contains(&checked.channel.as_str());
        let decl = if builtin { None } else { self.declared_channel(&checked.channel)? };
        schedule::admit_channel(&checked.channel, decl.as_ref(), builtin, role)?;
        if let Some(same) = self.same_schedule(&checked)? {
            return json_response(&ScheduleAdded { schedule: same.view(), replayed: true });
        }
        let kept = self.count_of("SELECT COUNT(*) AS n FROM record_schedules", vec![])?;
        if kept >= limits::RECORD_SCHEDULES_MAX as u64 {
            return Err(CellError::invalid(format!("this fragment has {kept} scheduled records, the most it may: remove one first (`fragment schedule rm`)")));
        }
        let now = js::now_ms();
        let next = checked.cron.next_fire(now, now).ok_or_else(|| CellError::invalid("this schedule never fires again"))?;
        let rows: Vec<Row> = self.typed(
            &format!("INSERT INTO record_schedules (cron, channel, body, created_by, created_at, next_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING {COLUMNS}"),
            vec![checked.expr.as_str().into(), checked.channel.as_str().into(), checked.body.as_str().into(), by.as_str().into(), SqlStorageValue::Integer(now), SqlStorageValue::Integer(next)],
        )?;
        let row = rows.into_iter().next().ok_or_else(|| CellError::host("a schedule's insert returned no row"))?;
        let summary = format!("schedule #{} ({}) on {}, by {}", row.id, row.cron, row.channel, npub::display(&by));
        self.event("schedule.added", &summary, json!({ "schedule": row.id, "cron": row.cron, "channel": row.channel, "by": npub::display(&by) }));
        self.schedule().await?;
        json_response(&ScheduleAdded { schedule: row.view(), replayed: false })
    }

    /// `PUT /api/f/<name>/schedules/<id>` `{paused}` (editor): paused, it
    /// fires no more; resumed (a write, as an add is), from its next minute
    /// after now, none it missed. Asked again, the same answer.
    pub(crate) async fn set_schedule(&self, caller: &Caller, id: &str, body: SetSchedule) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let by = self.caller_id(caller)?.to_string();
        let id = schedule_id(id)?;
        if !body.paused {
            self.writable().await?;
        }
        let row = self.schedule_row(id)?;
        if row.next_at.is_none() == body.paused {
            return json_response(&json!({ "schedule": row.view() }));
        }
        let now = js::now_ms();
        let next = match body.paused {
            true => None,
            false => Some(row.cron().next_fire(now, now).ok_or_else(|| CellError::invalid("this schedule never fires again"))?),
        };
        let next_at = next.map_or(SqlStorageValue::Null, SqlStorageValue::Integer);
        let rows: Vec<Row> = self.typed(&format!("UPDATE record_schedules SET next_at = ? WHERE id = ? RETURNING {COLUMNS}"), vec![next_at, SqlStorageValue::Integer(id)])?;
        let row = rows.into_iter().next().ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no schedule #{id}")))?;
        assert_eq!(row.next_at.is_none(), body.paused, "schedule #{id}'s pause was written");
        let kind = if body.paused { "schedule.paused" } else { "schedule.resumed" };
        self.event(kind, &format!("schedule #{id} by {}", npub::display(&by)), json!({ "schedule": id, "by": npub::display(&by) }));
        self.schedule().await?;
        json_response(&json!({ "schedule": row.view() }))
    }

    /// `DELETE /api/f/<name>/schedules/<id>` (editor): it fires no more.
    pub(crate) fn remove_schedule(&self, caller: &Caller, id: &str) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let by = self.caller_id(caller)?.to_string();
        let id = schedule_id(id)?;
        let gone = self.rows("DELETE FROM record_schedules WHERE id = ? RETURNING id", vec![SqlStorageValue::Integer(id)])?;
        if gone.is_empty() {
            return Err(CellError::new(ErrorCode::NotFound, format!("no schedule #{id}")));
        }
        self.event("schedule.removed", &format!("schedule #{id} by {}", npub::display(&by)), json!({ "schedule": id, "by": npub::display(&by) }));
        json_response(&json!({ "ok": true, "removed": id }))
    }

    /// `fire_cron`'s second half (jobs.rs): the scheduled records due at
    /// `now` append their records, each once for the minute it was due, and
    /// move on to their next minute after now. Past the owner's overdraft
    /// (`read_only`, read once for the pass) none appends: each moves on,
    /// and one event says why. One whose channel no longer takes the
    /// fragment's records (redeclared since) moves on, saying so.
    pub(crate) async fn fire_records(&self, read_only: Option<Why>, now: i64) -> CellResult<()> {
        // bounded: a fragment's scheduled records
        let due: Vec<Row> = self.typed(
            &format!("SELECT {COLUMNS} FROM record_schedules WHERE next_at <= ? ORDER BY next_at, id LIMIT ?"),
            vec![SqlStorageValue::Integer(now), SqlStorageValue::Integer(limits::RECORD_SCHEDULES_MAX as i64)],
        )?;
        if due.is_empty() {
            return Ok(());
        }
        if let Some(why) = read_only {
            for r in &due {
                self.record_moves_on(r, now, false)?;
            }
            let ids: Vec<i64> = due.iter().map(|r| r.id).collect();
            let said = crate::meter::read_only_refusal(why).message;
            self.event("schedules.blocked", &format!("{} scheduled record(s) did not fire: {said}", due.len()), json!({ "schedules": ids }));
            return Ok(());
        }
        for r in &due {
            let at = r.next_at.expect("a due schedule names its minute");
            let builtin = BUILTIN_CHANNELS.contains(&r.channel.as_str());
            let decl = if builtin { None } else { self.declared_channel(&r.channel)? };
            // as its owner may: whether the channel takes the fragment's records at all
            match schedule::admit_channel(&r.channel, decl.as_ref(), builtin, Role::Owner) {
                Ok(()) => {
                    self.append_own(&r.channel, &r.record(), &schedule::fire_key(r.id, at)).await?;
                }
                Err(why) => self.event("schedule.unfired", &format!("schedule #{}: {why}", r.id), json!({ "schedule": r.id })),
            }
            self.record_moves_on(r, now, true)?;
        }
        Ok(())
    }

    /// A due schedule's next minute after now (`Cron::next_fire`); one that
    /// never fires again goes, saying so. Paused, resumed or removed
    /// meanwhile (its minute changed), it is left as that made it.
    fn record_moves_on(&self, r: &Row, now: i64, fired: bool) -> CellResult<()> {
        let at = r.next_at.expect("a due schedule names its minute");
        let last = if fired { SqlStorageValue::Integer(now) } else { r.last_at.map_or(SqlStorageValue::Null, SqlStorageValue::Integer) };
        match r.cron().next_fire(at, now) {
            Some(next) => self.exec(
                "UPDATE record_schedules SET next_at = ?, last_at = ? WHERE id = ? AND next_at = ?",
                vec![SqlStorageValue::Integer(next), last, SqlStorageValue::Integer(r.id), SqlStorageValue::Integer(at)],
            ),
            None => {
                self.exec("DELETE FROM record_schedules WHERE id = ? AND next_at = ?", vec![SqlStorageValue::Integer(r.id), SqlStorageValue::Integer(at)])?;
                self.event("schedule.ended", &format!("schedule #{} ({}) never fires again", r.id, r.cron), json!({ "schedule": r.id }));
                Ok(())
            }
        }
    }

    /// Test fleets (`cron-now`): every scheduled record not paused is due now.
    pub(crate) fn records_due_now(&self) -> CellResult<usize> {
        Ok(self.rows("UPDATE record_schedules SET next_at = ? WHERE next_at IS NOT NULL RETURNING id", vec![SqlStorageValue::Integer(js::now_ms())])?.len())
    }
}
