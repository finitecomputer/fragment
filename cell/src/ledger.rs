//! The `Ledger` cell (docs/ledger.md): one Durable Object per payer,
//! named by a person's identity (an npub) or `org:<id>` (an org's,
//! decision 66: `fragment_core::ledger::org`), running the usage ledger's
//! state machine (`fragment_core::ledger`) over its own SQLite. The caller
//! decides who pays a row and asks that payer's ledger, which keys on
//! nothing else (docs/cloudflare-v1.md, decision 36).
//!
//! The head (plan, seat, balance) is kept whole as one
//! JSON row, small and bounded; what grows with use (references, batches,
//! commands, each fragment's spend by month, caps) is in tables, read and
//! written through `SqlStore`. Each route is one `transactionSync` over
//! both: the core decides before it writes, so a refusal changes nothing
//! but the month it rolled to, and a store that failed rolls the whole
//! route back. Its alarm runs `sweep` at the head's `next_sweep_ms`.
//!
//! Nothing is held before a paid call (decision 62): a head or an entry
//! the releases before kept (holds, reservations) loads, its holds
//! forgiven uncharged and their entries ended as released in the same
//! transaction, and a head is written with `holds` empty, so a release
//! rolled back to one of those reads a ledger with no hold and no held
//! entry (`fragment_core::ledger`, `retire_holds`). The
//! `entries.held` column stays (a schema changes only by a new step),
//! written 0. A batch is priced with this release's configured book
//! (`Config::book`): every call into a ledger is its own release's, so it
//! keeps none (decision 62); a head is written with that book too, which
//! a release rolled back to one that kept its own reads.
//!
//! Inner routes (platform code only, through `ask`): each is a POST of a
//! `Route`'s request (the core's types, or proto's), answered with its
//! `Route::Answer`; a refusal is `ErrorBody` with the typed `Refused`
//! beside it, so a caller can tell a read-only owner from a guest.
//!
//! **The gate** (`may`; docs/ledger.md, "The gate"): before a spend starts
//! (a paid call, a write), its payer's ledger is asked whether it may, at
//! most once a minute in each isolate (`fragment_core::ledger::gate`), and
//! for at most `GATE_WAIT_MS`. Only a refusal stops it: a ledger that does
//! not answer lets it start (decision 62).
//!
//! **Wiped** (docs/api.md, Operators): a wipe of its person empties it in
//! one step and leaves one row that says so (`wiped`). From then every
//! route is refused as `Refused::Wiped` (404), which a caller reads as a
//! refusal, not an outage: a meter batch still in the queue is acknowledged
//! and kept nowhere, so nothing makes the ledger again.

use std::cell::{Cell, RefCell};

use fragment_core::ledger::gate::Gate;
use fragment_core::price::PriceBook;
use fragment_core::ledger::org::Payer;
use fragment_core::ledger::{self as core, BatchRecord, CommandRecord, Entry, Ledger, Month, Refused, Spend, Store};
use fragment_proto::ledger::{GrantCredit, LedgerStatus, OrgBudget, OrgLedgerStatus, Plan, SeatState, SetFragmentCap, SetOverdraft, SetPlan, SetSeat, Why};
use fragment_proto::ErrorCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;

/// The payer a ledger call is for (`Payer::name`); only platform code sets
/// it, and the ledger keeps the first it is told and refuses another.
pub const PAYER_HEADER: &str = "x-fragment-payer";
/// The configured default plan's command id: applied once, as a new
/// person's ledger is made.
const DEFAULT_PLAN_ID: &str = "plan:default";
/// Test fleets: references a test hook lists at most.
const TEST_ENTRIES_MAX: i64 = 500;
/// Test fleets: the longest a slowed ledger waits before each answer
/// (`TestHook::Slow`).
const TEST_SLOW_MAX_MS: u64 = 30_000;
/// The meta key of a capped ledger's paid calls (`TestHook::PaidCalls`).
const PAID_CALLS_MAX: &str = "test_paid_calls_max";
/// How long a gate waits on a ledger it has no fresh answer from: a few
/// times a Durable Object call's usual round trip (tens of ms within a
/// location, a few hundred across the world, and a cold start), so a slow
/// or far ledger costs a call at most this, once a minute, and one that
/// does not answer lets the call through (decision 62).
const GATE_WAIT_MS: u64 = 1_500;

thread_local! {
    /// What each person's ledger last answered this isolate's gate (`may`).
    static GATE: RefCell<Gate> = const { RefCell::new(Gate::new()) };
}

#[durable_object]
pub struct LedgerCell {
    state: State,
    raw: JsValue,
    /// The isolate's settings (config.rs: built once per isolate).
    cfg: &'static Config,
    /// When this activation last set the alarm for (the ledger's clock):
    /// a call that needs it no later leaves it.
    armed: Cell<Option<i64>>,
    /// Its person was wiped (`wiped`'s row, read as it starts).
    wiped: Cell<bool>,
    /// Test fleets: how long it waits before each answer but a lever's
    /// (`TestHook::Slow`; this activation's alone).
    slow_ms: Cell<u64>,
    /// Its schema as it started (schema.rs): ready, or refused.
    schema: crate::schema::Schema,
}

impl DurableObject for LedgerCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let sql = state.storage().sql();
        let schema = crate::schema::Schema::start(&raw, sql.clone(), &fragment_core::migrate::LEDGER, &state.id().to_string());
        let wiped = schema.wiped(&sql);
        let cfg = Config::from_env(&env);
        LedgerCell { state, raw, cfg, armed: Cell::new(None), wiped: Cell::new(wiped), slow_ms: Cell::new(0), schema }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        if let Some(refused) = crate::routed::refused(&req, crate::routed::Own::of(self.cfg), &self.raw)? {
            return Ok(refused);
        }
        if let Some(probed) = crate::routed::probed(&req, &self.schema)? {
            return Ok(probed);
        }
        match self.route(req).await {
            Ok(answer) => Response::from_json(&answer),
            Err(Failed::Refused(why)) => refusal(why, why.message()),
            Err(Failed::Saying(why, message)) => refusal(why, message),
            Err(Failed::Cell(e)) => e.response(),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        // a wiped ledger has nothing to sweep (its alarm went with it)
        if self.schema.refused_alarm(&self.state, None).await? || self.wiped.get() {
            return Response::ok("");
        }
        if let Err(e) = self.swept().await {
            console_error!("{}", json!({ "event": "ledger.sweep-failed", "message": e.message }));
            // tried again within the minute, never spinning
            let at = js::now_ms() + 60_000;
            self.state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(at as f64)))).await?;
        }
        Response::ok("")
    }
}

/// A refusal as its caller reads it: the platform's error body, and the
/// core's typed reason beside it.
fn refusal(why: Refused, message: String) -> Result<Response> {
    let body = json!({ "error": why.code(), "message": message, "refused": why });
    Ok(Response::from_json(&body)?.with_status(why.code().status()))
}

/// How a route failed: the ledger refused (typed), or the cell did.
enum Failed {
    Refused(Refused),
    /// A refusal that says more than its reason's message (a test fleet's
    /// paid-calls cap).
    Saying(Refused, String),
    Cell(CellError),
}

impl From<CellError> for Failed {
    fn from(e: CellError) -> Failed {
        Failed::Cell(e)
    }
}

impl From<worker::Error> for Failed {
    fn from(e: worker::Error) -> Failed {
        Failed::Cell(e.into())
    }
}

/// One of the ledger's routes: the request a caller sends and the answer
/// it gets back. Both ends compile against the same types.
pub trait Route: Serialize + DeserializeOwned {
    const PATH: &'static str;
    type Answer: Serialize + DeserializeOwned;
}

/// An answer that says only that it was done (or allowed): `{}`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Done {}

/// May this spend start (docs/ledger.md, `may_spend`)? The whole question
/// when the payer owns the fragment (or there is none); when another
/// person owns it, that owner's ledger answers `FragmentOpen` too.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaySpend {
    pub spend: Spend,
    pub fragment: Option<String>,
    pub by_owner: bool,
}

/// Is a fragment this ledger's person owns open to spenders other than
/// its owner (under its cap) this month?
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentOpen {
    pub fragment: String,
}

/// The ledger now.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {}

/// An org's ledger now.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrgStatus {}

/// Fleets with levers only (`FRAGMENT_TEST_SECRET`; cell/src/levers.rs):
/// move this ledger's clock (`clock {offsetMs}`), run its sweep now
/// (`sweep`), list the references under a prefix (`entries {prefix}`),
/// read what moved its balance (`totals`), read the price book its head
/// holds as of its last call (`book`, answering `{book}`), or cap its paid
/// calls/// (`paid-calls {max?}`, answering `{max, used}`; with no `max` the cap
/// stays as it is, 0 where there was none: `used` is every model call,
/// operator key's call and AI step it has charged, by their references,
/// and once it reaches `max` its gate refuses them as a ledger at zero
/// does; the hosted e2e's people are capped so), or
/// set its seat at an order of the lever's (`legacy-seat {seat, seq}`,
/// answering its status), as an operator's `seat` command did before it
/// went (2026-10-16): a ledger such a command left ahead of the
/// registry's order; or make it answer every other route `ms` late
/// (`slow {ms}`, at most `TEST_SLOW_MAX_MS`, 0 to end it: a slow or far
/// ledger, which no paid call waits on past the gate's wait), or make the
/// gate of its isolate (and, through the router's lever, the router's)
/// forget what it heard (`forget-gate`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum TestHook {
    #[serde(rename_all = "camelCase")]
    Clock { offset_ms: i64 },
    Sweep,
    Entries { prefix: String },
    Book,
    Totals,
    PaidCalls { max: Option<u64> },
    LegacySeat { seat: SeatState, seq: u64 },
    /// The steps its schema holds (schema.rs `view`).
    Schema,
    Slow {
        ms: u64,
    },
    ForgetGate,
}

impl Route for core::Meter {
    const PATH: &'static str = "/meter";
    type Answer = core::Metered;
}

impl Route for MaySpend {
    const PATH: &'static str = "/may-spend";
    type Answer = Done;
}

impl Route for FragmentOpen {
    const PATH: &'static str = "/fragment-open";
    type Answer = Done;
}

impl Route for GrantCredit {
    const PATH: &'static str = "/grant";
    type Answer = Done;
}

impl Route for SetPlan {
    const PATH: &'static str = "/plan";
    type Answer = Done;
}

impl Route for SetSeat {
    const PATH: &'static str = "/seat";
    type Answer = Done;
}

impl Route for SetOverdraft {
    const PATH: &'static str = "/overdraft";
    type Answer = Done;
}

impl Route for SetFragmentCap {
    const PATH: &'static str = "/cap";
    type Answer = Done;
}

impl Route for Status {
    const PATH: &'static str = "/status";
    type Answer = LedgerStatus;
}

impl Route for OrgStatus {
    const PATH: &'static str = "/org-status";
    type Answer = OrgLedgerStatus;
}

impl Route for OrgBudget {
    const PATH: &'static str = "/org-budget";
    type Answer = Done;
}

impl Route for TestHook {
    const PATH: &'static str = "/test";
    type Answer = Value;
}

fn decode<R: Route>(body: &[u8]) -> CellResult<R> {
    serde_json::from_slice(body).map_err(|e| CellError::invalid(format!("{}: {e}", R::PATH)))
}

/// What grows with use, in this cell's tables (`fragment_core::ledger::Store`).
/// The trait answers values, not results, so a statement that fails marks
/// the store at fault: the route sees it before anything commits and rolls
/// the whole transaction back. A row that does not read as the core wrote
/// it is corruption, a fault too.
struct SqlStore {
    sql: SqlStorage,
    fault: std::cell::RefCell<Option<String>>,
}

impl SqlStore {
    fn new(sql: SqlStorage) -> SqlStore {
        SqlStore { sql, fault: std::cell::RefCell::new(None) }
    }

    fn fail(&self, why: String) {
        let mut fault = self.fault.borrow_mut();
        if fault.is_none() {
            *fault = Some(why);
        }
    }

    /// The first fault, as the route's failure.
    fn checked(&self) -> CellResult<()> {
        match self.fault.borrow().as_ref() {
            None => Ok(()),
            Some(why) => Err(CellError::host(format!("the ledger's store: {why}"))),
        }
    }

    fn rows<T: DeserializeOwned>(&self, q: &str, binds: Vec<SqlStorageValue>) -> Vec<T> {
        match self.sql.exec(q, binds).and_then(|c| c.to_array::<T>()) {
            Ok(rows) => rows,
            Err(e) => {
                self.fail(format!("{q}: {e}"));
                Vec::new()
            }
        }
    }

    fn exec(&self, q: &str, binds: Vec<SqlStorageValue>) {
        if let Err(e) = self.sql.exec(q, binds) {
            self.fail(format!("{q}: {e}"));
        }
    }

    fn count(&self, q: &str, binds: Vec<SqlStorageValue>) -> u64 {
        #[derive(Deserialize)]
        struct N {
            n: i64,
        }
        let n = self.rows::<N>(q, binds).first().map_or(0, |r| r.n);
        u64::try_from(n).unwrap_or_else(|_| {
            self.fail(format!("{q}: a negative count"));
            0
        })
    }

    /// A JSON column as the core's type: one that does not read is corruption.
    fn read<T: DeserializeOwned>(&self, what: &str, text: &str) -> Option<T> {
        match serde_json::from_str(text) {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(format!("{what} does not read as the ledger wrote it: {e}"));
                None
            }
        }
    }

    fn text<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("the ledger's types serialize")
    }
}

#[derive(Deserialize)]
struct Json {
    value: String,
}

#[derive(Deserialize)]
struct SpendRow {
    fragment: String,
    micros: i64,
}

impl Store for SqlStore {
    fn entry(&self, reference: &str) -> Option<Entry> {
        let rows: Vec<Json> = self.rows("SELECT entry AS value FROM entries WHERE ref = ?", vec![reference.into()]);
        let entry = self.read::<Entry>("an entry", &rows.into_iter().next()?.value)?;
        if let Entry::Row { row, .. } = &entry {
            assert_eq!(row.reference, reference, "an entry is kept under its own reference");
        }
        Some(entry)
    }

    fn put_entry(&mut self, reference: &str, entry: Entry) {
        // `held` is a column of the releases before decision 62: nothing is held now
        self.exec(
            "INSERT INTO entries (ref, at_ms, held, entry) VALUES (?, ?, 0, ?)
             ON CONFLICT (ref) DO UPDATE SET at_ms = excluded.at_ms, held = 0, entry = excluded.entry",
            vec![reference.into(), SqlStorageValue::Integer(entry.at_ms()), SqlStore::text(&entry).into()],
        );
    }

    fn batch(&self, id: &str) -> Option<BatchRecord> {
        #[derive(Deserialize)]
        struct Row {
            digest: String,
            answer: String,
            at_ms: i64,
        }
        let row = self.rows::<Row>("SELECT digest, answer, at_ms FROM batches WHERE id = ?", vec![id.into()]).into_iter().next()?;
        Some(BatchRecord { digest: row.digest, answer: self.read("a batch's answer", &row.answer)?, at_ms: row.at_ms })
    }

    fn put_batch(&mut self, id: &str, record: BatchRecord) {
        self.exec(
            "INSERT INTO batches (id, digest, answer, at_ms) VALUES (?, ?, ?, ?)",
            vec![id.into(), record.digest.as_str().into(), SqlStore::text(&record.answer).into(), SqlStorageValue::Integer(record.at_ms)],
        );
    }

    fn command(&self, id: &str) -> Option<CommandRecord> {
        #[derive(Deserialize)]
        struct Row {
            command: String,
            at_ms: i64,
        }
        let row = self.rows::<Row>("SELECT command, at_ms FROM commands WHERE id = ?", vec![id.into()]).into_iter().next()?;
        Some(CommandRecord { command: self.read("a command", &row.command)?, at_ms: row.at_ms })
    }

    fn put_command(&mut self, id: &str, record: CommandRecord) {
        self.exec(
            "INSERT INTO commands (id, command, at_ms) VALUES (?, ?, ?)",
            vec![id.into(), SqlStore::text(&record.command).into(), SqlStorageValue::Integer(record.at_ms)],
        );
    }

    fn spent(&self, month: Month, fragment: &str) -> i64 {
        let rows: Vec<SpendRow> = self.rows("SELECT fragment, micros FROM spend WHERE month = ? AND fragment = ?", vec![SqlStorageValue::Integer(month.0.into()), fragment.into()]);
        rows.first().map_or(0, |r| r.micros)
    }

    fn add_spent(&mut self, month: Month, fragment: &str, micros: i64) {
        assert!(micros > 0, "only a charge adds to a fragment's spend");
        self.exec(
            "INSERT INTO spend (month, fragment, micros) VALUES (?, ?, ?)
             ON CONFLICT (month, fragment) DO UPDATE SET micros = micros + excluded.micros",
            vec![SqlStorageValue::Integer(month.0.into()), fragment.into(), SqlStorageValue::Integer(micros)],
        );
    }

    fn spends(&self, month: Month, limit: usize) -> Vec<(String, i64)> {
        let rows: Vec<SpendRow> = self.rows(
            "SELECT fragment, micros FROM spend WHERE month = ? ORDER BY micros DESC, fragment LIMIT ?",
            vec![SqlStorageValue::Integer(month.0.into()), SqlStorageValue::Integer(limit as i64)],
        );
        rows.into_iter().map(|r| (r.fragment, r.micros)).collect()
    }

    fn month_spent(&self, month: Month) -> i64 {
        #[derive(Deserialize)]
        struct Row {
            micros: i64,
        }
        // bounded by the payer's fragments: one row each a month
        let rows: Vec<Row> = self.rows("SELECT COALESCE(SUM(micros), 0) AS micros FROM spend WHERE month = ?", vec![SqlStorageValue::Integer(month.0.into())]);
        rows.first().map_or(0, |r| r.micros)
    }

    fn cap(&self, fragment: &str) -> Option<i64> {
        #[derive(Deserialize)]
        struct Row {
            micros: i64,
        }
        self.rows::<Row>("SELECT micros FROM caps WHERE fragment = ?", vec![fragment.into()]).first().map(|r| r.micros)
    }

    fn set_cap(&mut self, fragment: &str, micros: Option<i64>) {
        match micros {
            Some(m) => self.exec(
                "INSERT INTO caps (fragment, micros) VALUES (?, ?) ON CONFLICT (fragment) DO UPDATE SET micros = excluded.micros",
                vec![fragment.into(), SqlStorageValue::Integer(m)],
            ),
            None => self.exec("DELETE FROM caps WHERE fragment = ?", vec![fragment.into()]),
        }
    }

    fn prune(&mut self, before_ms: i64, before_month: Month) -> u64 {
        let before = SqlStorageValue::Integer(before_ms);
        let forgotten = self.count(
            "SELECT (SELECT COUNT(*) FROM entries WHERE at_ms < ?) + (SELECT COUNT(*) FROM batches WHERE at_ms < ?) AS n",
            vec![before.clone(), before.clone()],
        );
        self.exec("DELETE FROM entries WHERE at_ms < ?", vec![before.clone()]);
        self.exec("DELETE FROM batches WHERE at_ms < ?", vec![before]);
        self.exec("DELETE FROM spend WHERE month < ?", vec![SqlStorageValue::Integer(before_month.0.into())]);
        forgotten
    }
}

/// The head as this cell keeps it, or a new one: an org's, with no credit
/// (`Ledger::new_org`), or a person's, a guest with no credit
/// (`Ledger::new`), then the deployment's default plan for new people
/// (decision 25: `FRAGMENT_DEFAULT_PLAN`).
fn head(store: &mut SqlStore, cfg: &Config, now_ms: i64) -> CellResult<Ledger> {
    let rows: Vec<Json> = store.rows("SELECT ledger AS value FROM head WHERE id = 1", vec![]);
    store.checked()?;
    if let Some(row) = rows.into_iter().next() {
        let mut ledger: Ledger = serde_json::from_str(&row.value).map_err(|e| CellError::host(format!("the ledger's head does not read as it was written: {e}")))?;
        // a head of the releases before decision 62: its holds forgiven,
        // their entries ended, in this transaction with the head's rewrite
        let retired = ledger.retire_holds(store);
        if retired > 0 {
            console_log!("{}", json!({ "event": "ledger.holds-retired", "retired": retired }));
        }
        ledger.assert_valid();
        return Ok(ledger);
    }
    let payer: Vec<Json> = store.rows("SELECT value FROM meta WHERE key = 'payer'", vec![]);
    store.checked()?;
    if let Some(Payer::Org(_)) = payer.first().and_then(|p| Payer::parse(&p.value)) {
        return Ok(Ledger::new_org(now_ms));
    }
    let mut ledger = Ledger::new(now_ms);
    if cfg.default_plan != Plan::Guest {
        let set = SetPlan { id: DEFAULT_PLAN_ID.into(), plan: cfg.default_plan };
        ledger.set_plan(store, &set, now_ms).map_err(|why| CellError::host(format!("a new ledger refused its default plan: {}", why.message())))?;
    }
    Ok(ledger)
}

/// Writes the head back, with the book this release charges with
/// (`Ledger::written`: for a rollback to a release that kept its own).
fn save(store: &SqlStore, ledger: &Ledger, book: &PriceBook) {
    store.exec(
        "INSERT INTO head (id, ledger) VALUES (1, ?) ON CONFLICT (id) DO UPDATE SET ledger = excluded.ledger",
        vec![ledger.written(book).into()],
    );
}

impl LedgerCell {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn meta(&self, key: &str) -> CellResult<Option<String>> {
        let rows: Vec<Json> = self.sql().exec("SELECT value FROM meta WHERE key = ?", vec![key.into()])?.to_array()?;
        Ok(rows.into_iter().next().map(|r| r.value))
    }

    fn set_meta(&self, key: &str, value: &str) -> CellResult<()> {
        self.sql().exec("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value", vec![key.into(), value.into()])?;
        Ok(())
    }

    /// The clock's offset a test fleet set (`TestHook::Clock`); none elsewhere.
    fn offset_ms(&self) -> CellResult<i64> {
        if !self.cfg.test_hooks {
            return Ok(0);
        }
        Ok(self.meta("clock_offset")?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    /// Runs `op` on the head and the store in one transaction, at the
    /// ledger's now: the head is written back whatever `op` answers (a
    /// refusal changed nothing but the month it rolled to, and a newer
    /// book), and a store that failed rolls everything back. Answers `op`'s
    /// answer and when the next sweep is due (the ledger's clock).
    fn transact<T: 'static>(&self, op: impl FnOnce(&mut Ledger, &mut SqlStore, i64) -> Result<T, Refused> + 'static) -> CellResult<(Result<T, Refused>, i64)> {
        let now = js::now_ms() + self.offset_ms()?;
        let (sql, cfg) = (self.sql(), self.cfg);
        js::transaction_sync(&self.raw, move || {
            let mut store = SqlStore::new(sql);
            let mut ledger = head(&mut store, cfg, now)?;
            let answer = op(&mut ledger, &mut store, now);
            save(&store, &ledger, &cfg.book);
            store.checked()?;
            Ok((answer, ledger.next_sweep_ms(now)))
        })
    }

    /// `transact`, then the alarm set for the next sweep when it is due
    /// sooner than this activation last set it.
    async fn mutate<T: 'static>(&self, op: impl FnOnce(&mut Ledger, &mut SqlStore, i64) -> Result<T, Refused> + 'static) -> Result<T, Failed> {
        let (answer, next) = self.transact(op)?;
        if self.armed.get().is_none_or(|armed| next < armed) {
            self.arm(next).await?;
        }
        answer.map_err(Failed::Refused)
    }

    /// Sets the alarm for `at` on the ledger's clock.
    async fn arm(&self, at: i64) -> CellResult<()> {
        let real = at - self.offset_ms()?;
        self.state.storage().set_alarm(ScheduledTime::new(js_sys::Date::new(&JsValue::from_f64(real.max(js::now_ms() + 1_000) as f64)))).await?;
        self.armed.set(Some(at));
        Ok(())
    }

    /// The alarm's work: the sweep, then the next.
    async fn swept(&self) -> CellResult<core::Swept> {
        if self.meta("payer")?.is_none() {
            // a ledger no one has called has nothing to sweep
            return Ok(core::Swept { forgotten: 0 });
        }
        let (swept, next) = self.transact(|ledger, store, now| Ok(ledger.sweep(store, now)))?;
        let swept = swept.expect("a sweep refuses nothing");
        self.arm(next).await?;
        Ok(swept)
    }

    async fn route(&self, mut req: Request) -> Result<Value, Failed> {
        // a wipe ends a ledger that refuses its schema too, and a wiped
        // ledger answers as one, refused or not (schema.rs)
        if let Some(route) = req.path().strip_prefix("/wipe/") {
            // only a wipe's orchestrator sets the header (routed.rs `marker`)
            if req.headers().get(crate::wipe::WIPE_HEADER)?.is_none() {
                return Err(CellError::new(ErrorCode::NotFound, format!("no route /wipe/{route}")).into());
            }
            return Ok(self.wipe(route).await?);
        }
        if self.wiped.get() {
            return Err(Failed::Refused(Refused::Wiped));
        }
        self.schema.ready()?;
        let payer = req.headers().get(PAYER_HEADER)?.filter(|p| Payer::parse(p).is_some()).ok_or_else(|| CellError::host("a ledger call names its payer"))?;
        match self.meta("payer")? {
            None => self.set_meta("payer", &payer)?,
            Some(p) if p != payer => return Err(CellError::host(format!("this ledger is {p}'s, not {payer}'s")).into()),
            Some(_) => {}
        }
        if req.method() != Method::Post {
            return Err(CellError::new(ErrorCode::NotFound, format!("no route {} {}", req.method().as_ref(), req.path())).into());
        }
        let path = req.path();
        let slow = self.slow_ms.get();
        if slow > 0 && path != TestHook::PATH {
            Delay::from(std::time::Duration::from_millis(slow)).await;
        }
        let body = req.bytes().await?;
        fn reply<T: Serialize>(answer: T) -> Result<Value, Failed> {
            Ok(serde_json::to_value(answer).expect("the ledger's answers serialize"))
        }
        fn done(answer: ()) -> Done {
            let () = answer;
            Done {}
        }
        match path.as_str() {
            <core::Meter as Route>::PATH => {
                let m: core::Meter = decode(&body)?;
                let book = &self.cfg.book;
                reply(self.mutate(move |l, s, now| l.meter(s, &m, now, book)).await?)
            }
            MaySpend::PATH => {
                let q: MaySpend = decode(&body)?;
                if self.cfg.test_hooks && matches!(q.spend, Spend::AgentTurn | Spend::AiStep) {
                    self.paid_calls()?;
                }
                reply(done(self.mutate(move |l, s, now| l.may_spend(s, q.spend, q.fragment.as_deref(), q.by_owner, now)).await?))
            }
            FragmentOpen::PATH => {
                let q: FragmentOpen = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.fragment_open(s, &q.fragment, now)).await?))
            }
            <GrantCredit as Route>::PATH => {
                let c: GrantCredit = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.grant(s, &c, now)).await?))
            }
            <SetPlan as Route>::PATH => {
                let c: SetPlan = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.set_plan(s, &c, now)).await?))
            }
            <SetSeat as Route>::PATH => {
                let c: SetSeat = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.set_seat(s, &c, now)).await?))
            }
            <SetOverdraft as Route>::PATH => {
                let c: SetOverdraft = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.set_overdraft(s, &c, now)).await?))
            }
            <SetFragmentCap as Route>::PATH => {
                let c: SetFragmentCap = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.set_cap(s, &c, now)).await?))
            }
            Status::PATH => {
                let Status {} = decode(&body)?;
                let book = &self.cfg.book;
                reply(self.mutate(move |l, s, now| if l.is_org() { Err(Refused::PersonsOnly) } else { Ok(l.status(s, now, book)) }).await?)
            }
            OrgStatus::PATH => {
                let OrgStatus {} = decode(&body)?;
                let Some(Payer::Org(org)) = Payer::parse(&payer) else { return Err(Failed::Refused(Refused::OrgsOnly)) };
                let book = &self.cfg.book;
                reply(self.mutate(move |l, s, now| l.org_status(&org, s, now, book)).await?)
            }
            <OrgBudget as Route>::PATH => {
                let c: OrgBudget = decode(&body)?;
                reply(done(self.mutate(move |l, s, now| l.set_budget(s, &c, now)).await?))
            }
            TestHook::PATH if self.cfg.test_hooks => self.test_hook(decode(&body)?).await,
            p => Err(CellError::new(ErrorCode::NotFound, format!("no route {p}")).into()),
        }
    }

    /// A wipe's calls (docs/api.md, Operators; the router's cell/src/wipe.rs):
    /// `view` → `{wiped, held}`, whether it holds anything; `end` →
    /// `{wiped, held}`: every table dropped and made again empty, with the
    /// row that says it was wiped, in one step (no await), and its alarm
    /// deleted. Again, it changes nothing.
    async fn wipe(&self, route: &str) -> CellResult<Value> {
        let held = || -> CellResult<bool> {
            let rows: Vec<Value> = self
                .sql()
                .exec(
                    "SELECT (SELECT COUNT(*) FROM meta) + (SELECT COUNT(*) FROM head) + (SELECT COUNT(*) FROM entries) + (SELECT COUNT(*) FROM batches)
                       + (SELECT COUNT(*) FROM commands) + (SELECT COUNT(*) FROM spend) + (SELECT COUNT(*) FROM caps) AS n",
                    None,
                )?
                .to_array()?;
            Ok(rows.first().and_then(|r| r["n"].as_i64()).ok_or_else(|| CellError::host("COUNT answered no row"))? > 0)
        };
        match route {
            "view" => Ok(json!({ "wiped": self.wiped.get(), "held": held()? })),
            "end" => {
                let had = held()?;
                // one step, no await: emptied and marked, or neither
                let sql = self.sql();
                crate::schema::remake(sql.clone(), &fragment_core::migrate::LEDGER)?;
                if !self.wiped.get() {
                    sql.exec("INSERT INTO wiped (at) VALUES (?)", vec![SqlStorageValue::Integer(js::now_ms())])?;
                }
                self.wiped.set(true);
                assert!(!held()?, "a wiped ledger holds nothing");
                self.state.storage().delete_alarm().await?;
                self.armed.set(None);
                Ok(json!({ "wiped": true, "held": had }))
            }
            other => Err(CellError::new(ErrorCode::NotFound, format!("no wipe route {other}"))),
        }
    }

    /// Fleets with levers: a capped ledger's paid calls used, every model
    /// call, operator key's call and AI step it has charged (an entry under
    /// `aig:`, `key:` or `step:`, a reservation of the releases before
    /// among them).
    fn paid_calls_used(&self) -> CellResult<u64> {
        let rows: Vec<Value> = self
            .sql()
            .exec("SELECT COUNT(*) AS n FROM entries WHERE substr(ref, 1, 4) IN ('aig:', 'key:') OR substr(ref, 1, 5) = 'step:'", None)?
            .to_array()?;
        Ok(rows.first().and_then(|r| r["n"].as_u64()).unwrap_or(0))
    }

    /// Fleets with levers: a ledger whose paid calls are capped
    /// (`TestHook::PaidCalls`) refuses one more at its gate once it has
    /// charged `max`, as a ledger at zero refuses it (an isolate trusts the
    /// gate's answer for up to a minute, so the cap is approximate by what
    /// starts in that window).
    fn paid_calls(&self) -> Result<(), Failed> {
        assert!(self.cfg.test_hooks, "only a fleet with levers caps paid calls");
        let Some(max) = self.meta(PAID_CALLS_MAX)?.and_then(|v| v.parse::<u64>().ok()) else { return Ok(()) };
        let used = self.paid_calls_used()?;
        if used >= max {
            let message = format!("this test person's {max} paid calls are used up (the hosted e2e's cap)");
            return Err(Failed::Saying(Refused::AgentsStopped { why: Why::NoCredit }, message));
        }
        Ok(())
    }

    async fn test_hook(&self, hook: TestHook) -> Result<Value, Failed> {
        assert!(self.cfg.test_hooks, "test hooks answer on test fleets only");
        match hook {
            TestHook::Schema => Ok(crate::schema::view(self.sql(), &fragment_core::migrate::LEDGER)?),
            TestHook::Slow { ms } => {
                self.slow_ms.set(ms.min(TEST_SLOW_MAX_MS));
                Ok(json!({ "slowMs": self.slow_ms.get() }))
            }
            TestHook::ForgetGate => {
                forget_gate();
                Ok(json!({ "forgot": true }))
            }
            TestHook::PaidCalls { max } => {
                let max = match max {
                    Some(max) => max,
                    None => self.meta(PAID_CALLS_MAX)?.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0),
                };
                self.set_meta(PAID_CALLS_MAX, &max.to_string())?;
                Ok(json!({ "max": max, "used": self.paid_calls_used()? }))
            }
            TestHook::Clock { offset_ms } => {
                self.set_meta("clock_offset", &offset_ms.to_string())?;
                let book = &self.cfg.book;
                let status = self.mutate(move |l, s, now| Ok(l.status(s, now, book))).await?;
                // the alarm follows the clock
                self.armed.set(None);
                Ok(json!({ "offsetMs": offset_ms, "month": status.month }))
            }
            TestHook::Sweep => Ok(serde_json::to_value(self.swept().await?).expect("a sweep serializes")),
            TestHook::Entries { prefix } => {
                #[derive(Deserialize)]
                struct Row {
                    r#ref: String,
                    entry: String,
                }
                // a prefix compared whole: a Durable Object's SQLite refuses LIKE patterns past a few dozen bytes
                let binds = vec![SqlStorageValue::Integer(prefix.len() as i64), prefix.as_str().into(), SqlStorageValue::Integer(TEST_ENTRIES_MAX)];
                let rows: Vec<Row> = self.sql().exec("SELECT ref, entry FROM entries WHERE substr(ref, 1, ?) = ? ORDER BY ref LIMIT ?", binds)?.to_array()?;
                let entries: Vec<Value> = rows
                    .into_iter()
                    .map(|r| {
                        let entry: Value = serde_json::from_str(&r.entry).expect("an entry is JSON the ledger wrote");
                        json!({ "ref": r.r#ref, "entry": entry })
                    })
                    .collect();
                Ok(json!({ "entries": entries }))
            }
            TestHook::Totals => Ok(serde_json::to_value(self.mutate(|l, _, _| Ok(l.totals())).await?).expect("totals serialize")),
            // the book it charges with: its release's (it keeps none)
            TestHook::Book => Ok(json!({ "book": self.cfg.book })),
            TestHook::LegacySeat { seat, seq } => {
                let c = SetSeat { id: format!("legacy-seat:{seq}"), seat, seq };
                let book = &self.cfg.book;
                let status = self.mutate(move |l, s, now| l.set_seat(s, &c, now).map(|()| l.status(s, now, book))).await?;
                Ok(serde_json::to_value(status).expect("a status serializes"))
            }
        }
    }
}

/// A ledger's refusal as its caller reads it: the platform's code and
/// message, and the core's typed reason when the ledger gave one (a cell
/// failure has none).
#[derive(Debug)]
pub struct LedgerError {
    pub code: ErrorCode,
    pub message: String,
    pub refused: Option<Refused>,
}

impl From<LedgerError> for CellError {
    fn from(e: LedgerError) -> CellError {
        CellError::new(e.code, e.message)
    }
}

impl From<Refused> for LedgerError {
    fn from(why: Refused) -> LedgerError {
        LedgerError { code: why.code(), message: why.message(), refused: Some(why) }
    }
}

impl From<CellError> for LedgerError {
    fn from(e: CellError) -> LedgerError {
        LedgerError { code: e.code, message: e.message, refused: None }
    }
}

impl From<worker::Error> for LedgerError {
    fn from(e: worker::Error) -> LedgerError {
        CellError::from(e).into()
    }
}

/// Asks `payer`'s ledger one of its routes (from a fragment, the router,
/// or the queue's consumer). Not reaching it is `ledger_unavailable`'s
/// stand-in, a host failure (5xx), which a gate may treat as passing (a
/// write, a wake); a paid call never does (`retried`, then refused).
pub async fn ask<R: Route>(env: &Env, payer: &str, request: &R) -> Result<R::Answer, LedgerError> {
    let bytes = asked(env, payer, R::PATH, serde_json::to_string(request).expect("a ledger request serializes")).await?;
    serde_json::from_slice(&bytes).map_err(|e| CellError::host(format!("the ledger's answer to {}: {e}", R::PATH)).into())
}

/// `ask`'s round trip, one copy for every route (the cell's Wasm is held
/// to a budget, and a copy per route type costs it): route `path`'s
/// answer, as bytes, or its refusal.
async fn asked(env: &Env, payer: &str, path: &str, body: String) -> Result<Vec<u8>, LedgerError> {
    assert!(Payer::parse(payer).is_some(), "a ledger is a payer's, named as `Payer::name` names it");
    let headers = Headers::new();
    headers.set(PAYER_HEADER, payer)?;
    headers.set("content-type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(headers).with_body(Some(body.into()));
    let req = Request::new_with_init(&format!("https://ledger.internal{path}"), &init)?;
    let mut resp = crate::routed::send(env, "LEDGER", payer, req).await?;
    let status = resp.status_code();
    let bytes = resp.bytes().await?;
    if status == 200 {
        return Ok(bytes);
    }
    /// A refusal's body: `ErrorBody` and, from the ledger, its reason.
    #[derive(Deserialize)]
    struct Refusal {
        error: ErrorCode,
        message: String,
        refused: Option<Refused>,
    }
    match serde_json::from_slice::<Refusal>(&bytes) {
        Ok(r) => Err(LedgerError { code: r.error, message: r.message, refused: r.refused }),
        Err(_) => Err(CellError::host(format!("the ledger answered {status}")).into()),
    }
}

/// The gate (docs/ledger.md, "The gate"): whether `person`'s ledger lets a
/// spend start, a `MaySpend` of theirs or a `FragmentOpen` of a fragment
/// they own. Its answer as this isolate heard it within the minute
/// (`fragment_core::ledger::gate`), or asked now, once, for at most
/// `GATE_WAIT_MS`. Only the ledger's refusal stops the spend: one that does
/// not answer in time, or answers otherwise (an outage, a deploy going
/// out), lets it start, says so in the log, and is asked again next time
/// (decision 62: billing never stops an agent).
pub async fn may<R: Route<Answer = Done>>(env: &Env, person: &str, question: &R) -> Result<(), LedgerError> {
    gated(env, person, R::PATH, serde_json::to_string(question).expect("a gate's question serializes")).await
}

/// `may`, one copy for both questions: route `path` with `body`.
async fn gated(env: &Env, person: &str, path: &'static str, body: String) -> Result<(), LedgerError> {
    let key = format!("{person} {path} {body}");
    let now = js::now_ms();
    if let Some(answer) = GATE.with(|g| g.borrow().fresh(&key, now)) {
        return answer.map_err(LedgerError::from);
    }
    let asked = std::pin::pin!(asked(env, person, path, body));
    let waited = std::pin::pin!(Delay::from(std::time::Duration::from_millis(GATE_WAIT_MS)));
    // what the ledger said, and its refusal as it said it (its message)
    let (heard, said) = match futures_util::future::select(asked, waited).await {
        // the routes answer `{}` when they allow it
        futures_util::future::Either::Left((Ok(_), _)) => (Some(Ok(())), None),
        futures_util::future::Either::Left((Err(e), _)) if e.refused.is_some() => (e.refused.map(Err), Some(e)),
        futures_util::future::Either::Left((Err(e), _)) => {
            console_error!("{}", json!({ "event": "ledger.gate-unknown", "path": path, "message": e.message }));
            (None, None)
        }
        futures_util::future::Either::Right(_) => {
            console_error!("{}", json!({ "event": "ledger.gate-unknown", "path": path, "message": format!("no answer in {GATE_WAIT_MS} ms") }));
            (None, None)
        }
    };
    GATE.with(|g| g.borrow_mut().heard(&key, heard, now)).map_err(|why| said.unwrap_or_else(|| why.into()))
}

/// Forgets every answer this isolate's gate heard (a test fleet's
/// `forget-standing` lever).
pub fn forget_gate() {
    GATE.with(|g| g.borrow_mut().forget());
}
