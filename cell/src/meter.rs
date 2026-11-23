//! How a fragment meets its owner's ledger (docs/ledger.md, "Who pays";
//! docs/cloudflare-v1.md, decisions 24, 26 and 27).
//!
//! A fragment's hosting bills its owner. Three meters, each a row the
//! fragment keeps in its own outbox (`meter_rows`) before anything is sent:
//!
//! - requests: each request the router hands it, counted per minute and
//!   closed into a row once that minute has passed (`req:<f>@<life>:<minute>`).
//!   The open minute's count is written with the storage's
//!   `allowUnconfirmed`, so no answer waits for it to be durable; a confirmed
//!   write folds it into its minute's row (`fold_open`), and a crash can lose
//!   at most that minute's unconfirmed counts, in the owner's favour
//!   (docs/ledger.md, "Requests");
//! - dynamic workers: the first time a code version runs on a UTC day
//!   (`dw:<f>@<life>:<version>:<day>`), the unit Cloudflare bills;
//! - storage: a daily sample of its SQLite and its blobs' bytes, as
//!   byte-hours since the sample before (`store:<f>@<life>:<class>:<at>`).
//!
//! References carry the fragment's life (`@<incarnation>`), so a name made
//! again never meets its earlier life's rows. The alarm flushes the outbox
//! one batch at a time, through the `fragment-ledger` queue, under the
//! batch's own id (`frag:<f>@<life>:<n>`); the consumer here applies it on
//! the owner's ledger and tells the fragment, which forgets the batch. One
//! not acknowledged is sent again, the same id and the same rows in the
//! same order, and the ledger answers it as before: a crash between its
//! apply and the acknowledgement charges nothing twice.
//!
//! A paid call the pipe makes (crate::pipe: an agent's model call, or its
//! call on an operator key) is charged on the same queue after its answer:
//! one row, as a batch of its own under the row's reference, from no
//! fragment (`source` empty), which the consumer applies and acknowledges,
//! telling no one.
//!
//! The owner's standing gates the fragment's writes (decision 27): past
//! the overdraft, the owner's fragments refuse mutations, posts, file
//! writes, deploys, blob uploads and replays with a 402 that says why, and
//! keep serving reads; their cron and triggers start no runs, each one
//! recorded `blocked` with the reason instead (jobs.rs `start_run`). The
//! fragment asks through the gate (ledger.rs `may`): its isolate asks the
//! owner's ledger at most once a minute, rather than on every write.

use fragment_core::ledger::{Meter, MeterBatch, MeterRow, Refused, Spend};
use fragment_core::price::{StorageClass, Usage};
use fragment_proto::ledger::{PutCap, SetFragmentCap, Why};
use fragment_proto::{ErrorCode, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{Acked, Owned};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::ledger::{self, MaySpend};
use crate::js;

/// The queue a fragment's meter batches travel on (wrangler.jsonc; a
/// branch deployment's is named for its branch), and its binding.
pub const QUEUE: &str = "fragment-ledger";
pub const QUEUE_BINDING: &str = "METERS";
/// A row into the outbox, once by its reference: `(ref, row)`.
pub(crate) const OUTBOX_INSERT: &str = "INSERT OR IGNORE INTO meter_rows (ref, row, batch) VALUES (?, ?, NULL)";
/// A batch not acknowledged this long after it was sent is sent again.
const RESEND_AFTER_MS: i64 = 5 * 60_000;
/// Storage is sampled at most this often (from the alarm's daily pass).
pub const STORAGE_EVERY_MS: i64 = 24 * HOUR_MS;
/// Rows one batch carries: a queue message is at most 128 KB, and a row is
/// about 250 bytes of JSON.
pub const BATCH_ROWS: i64 = 200;
/// Messages one consumer batch holds at most (wrangler.jsonc's
/// `max_batch_size` for the ledger's queue is this or less).
const CONSUME_BATCH_MAX: usize = 100;
/// A ledger that did not answer is asked again this much later.
const RETRY_DELAY_S: u32 = 5;
/// A cap change's id, as its owner names it.
const CAP_ID_MAX_BYTES: usize = 64;
const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

const _: () = assert!(BATCH_ROWS as usize <= fragment_core::ledger::BATCH_ROWS_MAX, "a fragment's batch is one the ledger takes");

#[derive(Deserialize)]
struct Minute {
    minute: i64,
    count: i64,
}

/// The open minute's request count, in the storage's key-value API under
/// `OPEN_COUNT_KEY`: written on every request with `allowUnconfirmed`, read
/// back once an activation. It names its life (the fragment's `created_at`),
/// so a life that ends leaves nothing a later life under the name counts.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, Deserialize)]
pub(crate) struct OpenCount {
    pub(crate) life: String,
    pub(crate) minute: i64,
    pub(crate) count: i64,
}

/// Where `OpenCount` is kept (the key-value API's, beside the SQL tables).
const OPEN_COUNT_KEY: &str = "meter:open";

#[derive(Deserialize)]
struct Waiting {
    id: i64,
    sent_at: Option<i64>,
}

impl FragmentCell {
    /// `<name>@<incarnation>`: what the fragment's references start with.
    pub(crate) fn meter_key(&self) -> CellResult<String> {
        Ok(format!("{}@{}", self.name()?, self.must(MetaKey::CreatedAt)?))
    }

    /// Keeps a row in the outbox, once by its reference: the fragment's
    /// own usage, or an AI step's, made for `agent` when an agent's run.
    pub(crate) fn outbox(&self, reference: &str, usage: Usage, at_ms: i64, agent: Option<String>) -> CellResult<()> {
        let text = self.outbox_row(reference, usage, at_ms, agent)?;
        self.exec(OUTBOX_INSERT, vec![reference.into(), text.into()])
    }

    /// An outbox row's JSON, for `OUTBOX_INSERT` (`outbox`, or a statement
    /// of a transaction of its caller's: an AI step's charge, ai.rs).
    pub(crate) fn outbox_row(&self, reference: &str, usage: Usage, at_ms: i64, agent: Option<String>) -> CellResult<String> {
        let row = MeterRow { reference: reference.to_string(), usage, fragment: Some(self.name()?), agent, computer: None, at_ms };
        assert!(fragment_core::price::printable(reference, fragment_core::ledger::ID_MAX_BYTES), "a meter's reference is one the ledger takes: {reference}");
        Ok(serde_json::to_string(&row).expect("a meter row serializes"))
    }

    /// Counts one request the router handed this created fragment: in this
    /// minute's count, or past the last minute closed into a row (a test's
    /// flush closes the minute it is in), so a reference is never sent with
    /// two counts. The count is written with `allowUnconfirmed`: the answer
    /// does not wait for it (every request would otherwise wait for its own
    /// count to be durable). An earlier minute's count is folded into its
    /// row first, by a confirmed write, once a minute at most. A count that
    /// fails is lost, never the request.
    pub(crate) async fn count_request(&self) {
        if let Err(e) = self.count_request_once().await {
            console_error!("{}", json!({ "event": "meter.count-failed", "message": e.message }));
        }
    }

    async fn count_request_once(&self) -> CellResult<()> {
        let Some(life) = self.meta(MetaKey::CreatedAt)? else { return Ok(()) };
        let closed: i64 = self.meta(MetaKey::MeterClosed)?.and_then(|v| v.parse().ok()).unwrap_or(-1);
        let mut open = self.open_count(&life).await?;
        let minute = fragment_core::ledger::request_minute(js::now_ms() / MINUTE_MS, closed, self.folded_minute()?, open.as_ref().map(|o| o.minute));
        if let Some(earlier) = open.as_ref().filter(|o| o.minute != minute) {
            // its key is overwritten below, so it needs no delete
            self.fold_open(earlier)?;
            open = None;
        }
        let now = OpenCount { life, minute, count: open.map_or(0, |o| o.count) + 1 };
        let value = js::to_js(&now);
        *self.meter_open.borrow_mut() = Some(Some(now));
        js::storage_put_unconfirmed(&self.raw, OPEN_COUNT_KEY, &value).await
    }

    /// This life's open count, not yet folded: read from storage once an
    /// activation. A stored one of another life, or of a minute folded
    /// already (its key's delete lost), is none, and its key goes.
    pub(crate) async fn open_count(&self, life: &str) -> CellResult<Option<OpenCount>> {
        let held = self.meter_open.borrow().clone();
        if let Some(open) = held {
            return Ok(open.filter(|o| o.life == life));
        }
        let stored: Option<OpenCount> = self.state.storage().get(OPEN_COUNT_KEY).await.map_err(|e| CellError::host(format!("the open request count: {e}")))?;
        let folded = self.folded_minute()?;
        let kept = stored.is_some();
        let open = stored.filter(|o| o.life == life && o.minute > folded);
        *self.meter_open.borrow_mut() = Some(open.clone());
        if kept && open.is_none() {
            self.forget_open_key().await;
        }
        Ok(open)
    }

    /// The open count into its minute's row, with the minute recorded as
    /// folded, in one transaction (a confirmed write on Cloudflare and on
    /// celld alike): a count is folded once. Its key may outlive it (its
    /// delete is apart, `forget_open_key`); one at or below the minute folded
    /// is never folded again.
    fn fold_open(&self, open: &OpenCount) -> CellResult<()> {
        if open.minute > self.folded_minute()? {
            let (sql, minute, count) = (self.sql(), open.minute, open.count);
            js::transaction_sync(&self.raw, move || {
                sql.exec(
                    "INSERT INTO meter_requests (minute, count) VALUES (?, ?) ON CONFLICT (minute) DO UPDATE SET count = count + excluded.count",
                    vec![SqlStorageValue::Integer(minute), SqlStorageValue::Integer(count)],
                )?;
                sql.exec(
                    "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    vec![MetaKey::MeterFolded.key().into(), minute.to_string().into()],
                )?;
                Ok(())
            })?;
        }
        *self.meter_open.borrow_mut() = Some(None);
        Ok(())
    }

    /// The last minute folded, or -1.
    fn folded_minute(&self) -> CellResult<i64> {
        Ok(self.meta(MetaKey::MeterFolded)?.and_then(|v| v.parse().ok()).unwrap_or(-1))
    }

    /// The open count's key gone, best effort: one left is never folded
    /// twice (`fold_open`), nor counted by another life (`open_count`).
    async fn forget_open_key(&self) {
        let storage = self.state.storage();
        if let Err(e) = storage.delete(OPEN_COUNT_KEY).await {
            console_warn!("{}", json!({ "event": "meter.open-key-kept", "message": e.to_string() }));
        }
    }

    /// The alarm set to close this minute's count once it is over, once a
    /// minute at most (the activation remembers the minute it armed for).
    pub(crate) async fn meter_soon(&self) {
        let minute = js::now_ms() / MINUTE_MS;
        if self.meter_armed.get() >= minute {
            return;
        }
        self.meter_armed.set(minute);
        if let Err(e) = self.schedule_by((minute + 1) * MINUTE_MS).await {
            console_error!("{}", json!({ "event": "meter.arm-failed", "message": e.message }));
        }
    }

    /// The first time `version` (a loader id: one fragment's code, as the
    /// runtime runs it) runs on a UTC day: one dynamic worker. The
    /// activation remembers the last it noted, so a call costs no write.
    pub(crate) fn note_dynamic_worker(&self, version: &str) {
        let now = js::now_ms();
        let day = now / DAY_MS;
        if self.dw_noted.borrow().as_ref().is_some_and(|(v, d)| v == version && *d == day) {
            return;
        }
        let id = &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(version.as_bytes()))[..16];
        let noted = self.meter_key().and_then(|key| self.outbox(&format!("dw:{key}:{id}:{day}"), Usage::DynamicWorkers { count: 1 }, now, None));
        match noted {
            Ok(()) => *self.dw_noted.borrow_mut() = Some((version.to_string(), day)),
            Err(e) => console_error!("{}", json!({ "event": "meter.dw-failed", "message": e.message })),
        }
    }

    /// A storage sample when one is due (`STORAGE_EVERY_MS` since the last,
    /// or since the fragment was made), or now (`force`): its SQLite as
    /// `sqlite`, its blobs as `r2`, each its bytes × the hours since the
    /// last sample. Its app facet's database is not sampled: only the
    /// app's own realm can say its size, and asking would load its code
    /// (a dynamic worker) every day (docs/technical-debt-ledger.md).
    pub(crate) fn sample_storage(&self, force: bool) -> CellResult<()> {
        let [created, sampled] = self.metas([MetaKey::CreatedAt, MetaKey::StorageSampledAt])?;
        let Some(created) = created.and_then(|c| c.parse::<i64>().ok()) else { return Ok(()) };
        let since = sampled.and_then(|s| s.parse::<i64>().ok()).unwrap_or(created);
        let now = js::now_ms();
        if !force && now - since < STORAGE_EVERY_MS {
            return Ok(());
        }
        let elapsed = u128::try_from(now - since).unwrap_or(0);
        let sqlite = self.sql().database_size() as u64;
        let blobs = self.rows("SELECT COALESCE(SUM(size), 0) AS n FROM blobs", vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        let key = self.meter_key()?;
        for (class, name, bytes) in [(StorageClass::Sqlite, "sqlite", sqlite), (StorageClass::R2, "r2", blobs)] {
            let byte_hours = u64::try_from(u128::from(bytes) * elapsed / HOUR_MS as u128).unwrap_or(u64::MAX);
            if byte_hours > 0 {
                self.outbox(&format!("store:{key}:{name}:{now}"), Usage::Storage { class, byte_hours }, now, None)?;
            }
        }
        self.set_meta(MetaKey::StorageSampledAt, &now.to_string())
    }

    /// Closes the minutes that are over into rows (every counted minute,
    /// with `close_all`: a test's flush), then sends the waiting batch, or
    /// a new one: one at a time, each sent again until it is acknowledged.
    pub(crate) async fn flush_meters(&self, close_all: bool) -> CellResult<()> {
        if self.meta(MetaKey::CreatedAt)?.is_none() {
            return Ok(());
        }
        let key = self.meter_key()?;
        let now = js::now_ms();
        let upto = if close_all { i64::MAX } else { now / MINUTE_MS - 1 };
        let life = self.must(MetaKey::CreatedAt)?;
        if let Some(open) = self.open_count(&life).await?.filter(|o| o.minute <= upto) {
            self.fold_open(&open)?;
            self.forget_open_key().await;
        }
        let minutes: Vec<Minute> = self.typed("SELECT minute, count FROM meter_requests WHERE minute <= ? ORDER BY minute", vec![SqlStorageValue::Integer(upto)])?;
        // bounded: one row per minute counted since the last flush
        for m in &minutes {
            let count = u64::try_from(m.count).map_err(|_| CellError::host("a negative request count"))?;
            // a minute's row is at its end, or now for one that is not over (a test's)
            let at = ((m.minute + 1) * MINUTE_MS - 1).min(now);
            self.outbox(&format!("req:{key}:{}", m.minute), Usage::Requests { count }, at, None)?;
        }
        if let Some(last) = minutes.last() {
            self.exec("DELETE FROM meter_requests WHERE minute <= ?", vec![SqlStorageValue::Integer(last.minute)])?;
            self.set_meta(MetaKey::MeterClosed, &last.minute.to_string())?;
        }
        let waiting: Option<Waiting> = self.typed("SELECT id, sent_at FROM meter_batches ORDER BY id LIMIT 1", vec![])?.into_iter().next();
        let batch = match waiting {
            Some(b) => b,
            None => {
                let rows = self.count("SELECT COUNT(*) AS n FROM meter_rows WHERE batch IS NULL")?;
                if rows == 0 {
                    return Ok(());
                }
                let made = self.rows("INSERT INTO meter_batches (sent_at) VALUES (NULL) RETURNING id", vec![])?;
                let id = made.first().and_then(|r| r["id"].as_i64()).ok_or_else(|| CellError::host("a batch made has an id"))?;
                self.exec(
                    "UPDATE meter_rows SET batch = ? WHERE ref IN (SELECT ref FROM meter_rows WHERE batch IS NULL ORDER BY ref LIMIT ?)",
                    vec![SqlStorageValue::Integer(id), SqlStorageValue::Integer(BATCH_ROWS)],
                )?;
                Waiting { id, sent_at: None }
            }
        };
        if batch.sent_at.is_some_and(|at| now - at < RESEND_AFTER_MS) {
            return Ok(());
        }
        #[derive(Deserialize)]
        struct Row {
            row: String,
        }
        // the same rows in the same order each time: the ledger knows a resend by its digest
        let rows: Vec<Row> = self.typed("SELECT row FROM meter_rows WHERE batch = ? ORDER BY ref", vec![SqlStorageValue::Integer(batch.id)])?;
        let rows: Vec<MeterRow> = rows.iter().map(|r| crate::json_text(&r.row)).collect::<Result<_, _>>().map_err(|e| CellError::host(format!("a meter row: {e}")))?;
        assert!(!rows.is_empty() && rows.len() <= BATCH_ROWS as usize, "a batch holds 1 to BATCH_ROWS rows");
        let message = MeterBatch { payer: self.must(MetaKey::Owner)?, source: self.name()?, seq: batch.id, meter: Meter { batch: format!("frag:{key}:{}", batch.id), rows } };
        let body = serde_json::to_value(&message).expect("a batch serializes");
        match js::queue_send(self.env.as_ref(), QUEUE_BINDING, &[body]).await {
            Ok(()) => self.exec("UPDATE meter_batches SET sent_at = ? WHERE id = ?", vec![SqlStorageValue::Integer(now), SqlStorageValue::Integer(batch.id)]),
            // tried again from the next alarm: the batch keeps its rows
            Err(e) => {
                console_error!("{}", json!({ "event": "meter.deferred", "batch": message.meter.batch, "message": e.message }));
                Ok(())
            }
        }
    }

    /// When the outbox next needs the alarm: the end of the oldest minute
    /// counted, now for rows not yet in a batch (when none waits), or a
    /// waiting batch's resend.
    pub(crate) fn meter_due_at(&self) -> CellResult<Option<i64>> {
        let rows = self.rows(
            "SELECT MIN(at) AS at FROM (
               SELECT (MIN(minute) + 1) * ? AS at FROM meter_requests
               UNION ALL SELECT ? AS at WHERE EXISTS (SELECT 1 FROM meter_rows WHERE batch IS NULL) AND NOT EXISTS (SELECT 1 FROM meter_batches)
               UNION ALL SELECT COALESCE(MIN(sent_at) + ?, ?) AS at FROM meter_batches HAVING COUNT(*) > 0)",
            vec![
                SqlStorageValue::Integer(MINUTE_MS),
                SqlStorageValue::Integer(js::now_ms()),
                SqlStorageValue::Integer(RESEND_AFTER_MS),
                SqlStorageValue::Integer(js::now_ms()),
            ],
        )?;
        let at = rows.first().and_then(|r| r["at"].as_i64());
        // the open minute's count, as this activation holds it: its end
        let open = self.meter_open.borrow().clone().flatten().map(|o| (o.minute + 1) * MINUTE_MS);
        Ok(at.into_iter().chain(open).min())
    }

    /// Test fleets: the outbox as it stands (`/test/fragment meter`): the
    /// minutes counted, the open one's among them.
    pub(crate) fn meter_state(&self) -> CellResult<Value> {
        let mut counted = self.rows("SELECT minute, count FROM meter_requests ORDER BY minute", vec![])?;
        if let Some(open) = self.meter_open.borrow().clone().flatten() {
            counted.push(json!({ "minute": open.minute, "count": open.count }));
        }
        #[derive(Deserialize)]
        struct Row {
            r#ref: String,
            row: String,
            batch: Option<i64>,
        }
        let rows: Vec<Row> = self.typed("SELECT ref, row, batch FROM meter_rows ORDER BY ref", vec![])?;
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                let row: MeterRow = crate::json_text(&r.row).expect("a meter row is JSON the fragment wrote");
                json!({ "ref": r.r#ref, "batch": r.batch, "usage": row.usage })
            })
            .collect();
        let batches = self.rows("SELECT id, sent_at FROM meter_batches ORDER BY id", vec![])?;
        Ok(json!({ "counted": counted, "rows": rows, "batches": batches, "key": self.meter_key()? }))
    }

    /// The consumer's word on batch `seq` (`calls::Acked`). Its rows go
    /// (the ledger has them, or never takes them), and the next batch is
    /// due. A batch of another life of this name, or one acknowledged
    /// before, changes nothing.
    pub(crate) async fn meter_acked(&self, acked: &Acked) -> CellResult<()> {
        let key = self.meter_key()?;
        if acked.batch != format!("frag:{key}:{}", acked.seq) {
            return Ok(());
        }
        self.test_countdown(MetaKey::TestFailMeterAcks, "the batch's acknowledgement was lost")?;
        self.exec("DELETE FROM meter_rows WHERE batch = ?", vec![SqlStorageValue::Integer(acked.seq)])?;
        self.exec("DELETE FROM meter_batches WHERE id = ?", vec![SqlStorageValue::Integer(acked.seq)])?;
        match (&acked.metered, &acked.refused) {
            (Some(m), _) if !m.refused.is_empty() => {
                self.event("meter.rows-refused", &format!("the owner's ledger refused {} of batch {}'s rows", m.refused.len(), acked.batch), json!({ "batch": acked.batch, "refused": m.refused }))
            }
            (None, Some(why)) => self.event("meter.refused", &format!("the owner's ledger takes no batch {}: {why}", acked.batch), json!({ "batch": acked.batch })),
            _ => {}
        }
        self.schedule().await?;
        Ok(())
    }

    /// `PUT /api/f/<name>/cap {id, micros}` (the owner): the fragment's
    /// monthly cap on its owner's ledger (decision 26), `null` for the
    /// default. Its `id` makes the change once; the ledger keeps it under
    /// `cap:<fragment>:<id>`, apart from every other command's.
    pub(crate) async fn put_cap(&self, caller: &Caller, body: PutCap) -> CellResult<Response> {
        self.require(caller, false, Role::Owner)?;
        if !fragment_core::price::printable(&body.id, CAP_ID_MAX_BYTES) {
            return Err(CellError::invalid(format!("a cap's id is 1-{CAP_ID_MAX_BYTES} printable ASCII characters, no spaces")));
        }
        let (name, owner) = (self.name()?, self.must(MetaKey::Owner)?);
        let set = SetFragmentCap { id: format!("cap:{name}:{}", body.id), fragment: name.clone(), micros: body.micros };
        ledger::ask(&self.env, &owner, &set).await?;
        self.event("cap.set", &format!("{name}'s monthly cap: {}", body.micros.map_or_else(|| "the default".into(), fragment_core::price::dollars)), json!({ "micros": body.micros }));
        json_response(&json!({ "fragment": name, "capMicros": body.micros.unwrap_or(fragment_core::ledger::CAP_DEFAULT), "default": body.micros.is_none() }))
    }

    /// Whose it is (`calls::Whose`).
    pub(crate) fn whose(&self, principal: &str) -> CellResult<Owned> {
        Ok(Owned { owner: self.must(MetaKey::Owner)?, member: self.member_role(principal)?.is_some() })
    }

    /// Whether the fragment takes writes now (decision 27): refused, 402,
    /// once its owner's balance is past the overdraft, until a top-up
    /// brings it above zero. A guest's fragments are billed nothing and
    /// take writes.
    pub(crate) async fn writable(&self) -> CellResult<()> {
        match self.read_only().await? {
            None => Ok(()),
            Some(why) => Err(read_only_refusal(why)),
        }
    }

    /// Why the owner's fragments are read-only, when they are: the gate's
    /// answer to `may_spend(write)` (ledger.rs `may`: what this isolate
    /// heard within the minute, or the owner's ledger now). A ledger that
    /// does not answer says none: writes are the product, and an outage
    /// costs cents at most. Any other refusal is none too: a guest's
    /// fragments are billed nothing and take writes.
    pub(crate) async fn read_only(&self) -> CellResult<Option<Why>> {
        let owner = self.must(MetaKey::Owner)?;
        match ledger::may(&self.env, &owner, &MaySpend { spend: Spend::Write, fragment: None, by_owner: true }).await {
            Err(e) => match e.refused {
                Some(Refused::ReadOnly { why }) => Ok(Some(why)),
                _ => Ok(None),
            },
            Ok(()) => Ok(None),
        }
    }
}

/// A write refused past the owner's overdraft, saying why.
pub(crate) fn read_only_refusal(why: Why) -> CellError {
    CellError::new(ErrorCode::BudgetUsedUp, format!("this fragment's owner's {}", Refused::ReadOnly { why }.message()))
}

/// The ledger queue's consumer: each batch applied on its payer's ledger,
/// then its fragment told (a paid call's charge tells no one). A ledger
/// that does not answer, or a fragment that does not hear, gets the
/// message again; a refusal is final (a guest's fragments are billed
/// nothing; a batch the ledger refuses as another body is a source's bug,
/// kept in its event log).
pub async fn consume(batch: MessageBatch<Value>, env: Env) -> Result<()> {
    let messages: Vec<RawMessage> = batch.raw_iter().collect();
    assert!(messages.len() <= CONSUME_BATCH_MAX, "a ledger batch holds at most {CONSUME_BATCH_MAX} messages, not {}", messages.len());
    futures_util::future::join_all(messages.into_iter().map(|m| consume_one(m, &env))).await;
    Ok(())
}

async fn consume_one(raw: RawMessage, env: &Env) {
    let b = match js::queue_text(&raw).and_then(|text| MeterBatch::from_queue(text.as_bytes())) {
        Ok(b) => b,
        Err(e) => {
            // not one a fragment sent: nothing can take it
            console_error!("{}", json!({ "event": "meter.unreadable", "message": e }));
            return;
        }
    };
    let (metered, refused) = match ledger::ask(env, &b.payer, &b.meter).await {
        Ok(metered) => (Some(metered), None),
        Err(e) if e.refused.is_some() => (None, Some(e.message)),
        Err(_) => {
            raw.retry_with_options(&QueueRetryOptionsBuilder::new().with_delay_seconds(RETRY_DELAY_S).build());
            return;
        }
    };
    if b.source.is_empty() {
        // a paid call's charge (`pipe::settle`): no outbox to tell, and nothing to send again
        let rows_refused = metered.as_ref().map_or(0, |m| m.refused.len());
        if refused.is_some() || rows_refused > 0 {
            console_error!("{}", json!({ "event": "ledger.charge-refused", "batch": b.meter.batch, "refused": refused, "rows": metered.map(|m| m.refused) }));
        }
        raw.ack();
        return;
    }
    let acked = Acked { seq: b.seq, batch: b.meter.batch.clone(), metered, refused };
    match crate::fragment::pass(env, &b.source, &acked).await.map(|resp| resp.status_code()) {
        // a fragment deleted since has nothing to forget
        Ok(200 | 404) => raw.ack(),
        _ => raw.retry_with_options(&QueueRetryOptionsBuilder::new().with_delay_seconds(RETRY_DELAY_S).build()),
    }
}
