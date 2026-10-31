//! A payer's usage ledger (docs/ledger.md; docs/cloudflare-v1.md,
//! decisions 24–27, 36, 62 and 66–68): credit, plans, meters, caps, and
//! the standing that says what may still spend.
//!
//! One ledger per payer: a person, or an org (`org`: credit it bought or
//! was given, a budget, and none of a person's plan, seat, overdraft,
//! agents or computer). The caller decides who pays (an agent's model
//! and compute bill the agent's owner; a fragment's hosting bills the
//! fragment's owner) and sends each row to that payer's ledger, which
//! keys on nothing else. Money is integer micro-dollars, months are UTC,
//! and time is an argument, never read here: the ledger is a pure state
//! machine that the cell's Ledger Durable Object runs and the tests
//! simulate.
//!
//! The state has two parts. `Ledger` is the head: small and bounded (the
//! plan, the balance's parts), kept whole. A `Store` holds what grows with use, by id: each reference
//! charged, each batch and command applied, each fragment's spend by
//! month, and caps. The Durable Object keeps those in SQLite tables, and
//! runs each mutation in one transaction with the head; tests keep them
//! in memory (`Memory`). A mutation decides before it writes, so a
//! refusal writes nothing.
//!
//! Every mutation is idempotent by its own id (a command's, a reference,
//! a batch's): a replay changes nothing and answers as the first did, and
//! the same id with another body is refused. A refusal is not remembered,
//! so the same id may be tried again (a step after a top-up). Every
//! mutation first brings the ledger to its time's month.
//!
//! Credit (decision 25): a seat's included credit arrives each UTC month
//! and expires at its end; purchased credit stays until spent. A charge
//! draws included credit first. Included credit that arrives while the
//! person owes pays the debt first, so a debt is paid once.
//!
//! Nothing is held before a paid call (decision 62): a spend asks the
//! cell's cached gate (`may_spend`, `fragment_open`; `gate`), and what it
//! used is metered after it, once by its reference. A head kept before
//! (2026-10-21) may still name holds and a book: both are read past, its
//! holds forgiven (what they held was never drawn).
//!
//! The ledger keeps no price book (decision 62): every call into a
//! person's ledger comes from its own release (the deploy-skew guard), so
//! a batch is priced with the book that release configured, passed in
//! (`meter`), and each book a ledger priced with is kept once as a
//! command, `book:<digest>`, for the audit, beside each row's version.
//!
//! Zero (decision 27): at a balance of zero or less, agents stop (no
//! turns, no wakes, no AI steps). Fragments keep taking writes down to the
//! overdraft; at it they go read-only, and stay read-only until the
//! balance is above zero again or an operator changes the overdraft.
//! Meters always record: what happened is charged, even past zero.
//!
//! Guests (decision 25; Paul, 2026-10-03): a guest pays for nothing, so a
//! guest makes no fragment either, whose hosting would bill them. They
//! edit the fragments shared with them, which bill those fragments'
//! owners.

use std::collections::BTreeMap;

use fragment_proto::ledger::{FragmentSpend, GrantCredit, LedgerStatus, OrgBudget, Plan, SeatState, SetFragmentCap, SetOverdraft, SetPlan, SetSeat, Standing, Why};
use fragment_proto::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::price::{self, dollars, PriceBook, PriceError, Priced, Usage, UsageFault, USD};

pub mod gate;
pub mod org;
#[cfg(test)]
mod sim;
#[cfg(test)]
mod tests;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// A seat's included credit each month, and the always-on seat's
/// (decision 25).
pub const SEAT_INCLUDED: i64 = 50 * USD;
pub const SEAT_ALWAYS_ON_INCLUDED: i64 = 100 * USD;
/// How far below zero a person's fragments keep taking writes, until an
/// operator sets another (decision 27).
pub const OVERDRAFT_DEFAULT: i64 = 2 * USD;
/// A fragment's monthly cap until its owner sets one (decision 26).
pub const CAP_DEFAULT: i64 = 5 * USD;

/// Rows in one batch: a batch is one request and one transaction, and
/// 1,000 rows of about 300 bytes stay far under the request body limit.
pub const BATCH_ROWS_MAX: usize = 1_000;
/// One grant: a purchase or an operator's gift; more is a typo.
pub const GRANT_MAX: i64 = 10_000 * USD;
/// A fragment's cap: more is no cap at all, and a typo.
pub const CAP_MAX: i64 = 10_000 * USD;
/// An overdraft an operator may set: someone trusted with more debt than
/// this is someone to invoice.
pub const OVERDRAFT_MAX: i64 = 1_000 * USD;
/// An id (a reference, a batch, a command, who granted) or a name (a
/// fragment, an agent, a computer): printable ASCII, at most this long.
/// Sources prefix their references (`aig:<log id>`, `step:<fragment>@…`)
/// so two sources never collide.
pub const ID_MAX_BYTES: usize = 256;
/// A grant's note for people.
pub const NOTE_MAX_BYTES: usize = 512;
/// How long references and batches are remembered (the audit records'
/// retention). A row from before this is refused as stale: its reference
/// may have been forgotten, so it could be a replay. Sources retry within
/// minutes or hours.
pub const REF_KEEP_MS: i64 = 90 * DAY_MS;
/// How far ahead of the ledger's clock a row may say it happened: its
/// source is another Durable Object, on a clock of its own.
pub const CLOCK_SKEW_MS: i64 = 5 * 60_000;
/// The sweep runs at least this often, to forget old references.
pub const SWEEP_EVERY_MS: i64 = DAY_MS;
/// Fragments a status lists, the largest spend first.
pub const STATUS_FRAGMENTS: usize = 50;

const _: () = assert!(SEAT_INCLUDED <= SEAT_ALWAYS_ON_INCLUDED, "the larger seat includes more");
const _: () = assert!(OVERDRAFT_DEFAULT <= OVERDRAFT_MAX && CAP_DEFAULT <= CAP_MAX, "the defaults are within their limits");

/// A UTC month, counted from January 1970 (month 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Month(pub u32);

impl Month {
    pub fn of(ms: i64) -> Month {
        assert!(ms >= 0, "the ledger's times are after 1970");
        let (year, month, _) = crate::cron::civil(ms.div_euclid(DAY_MS));
        let index = (year - 1970) * 12 + i64::from(month) - 1;
        Month(u32::try_from(index).expect("a month since 1970 fits in 32 bits"))
    }

    /// `YYYY-MM`, for people.
    pub fn label(self) -> String {
        format!("{:04}-{:02}", 1970 + self.0 / 12, self.0 % 12 + 1)
    }

    /// The month `label` (`YYYY-MM`, `label`'s inverse) names.
    pub fn parse(label: &str) -> Option<Month> {
        let (y, m) = label.split_once('-')?;
        if y.len() != 4 || m.len() != 2 || !y.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (y, m): (u32, u32) = (y.parse().ok()?, m.parse().ok()?);
        ((1970..=9999).contains(&y) && (1..=12).contains(&m)).then(|| Month((y - 1970) * 12 + m - 1))
    }
}

/// What a spend is, for the questions the platform asks before it lets
/// one happen (`Ledger::gate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spend {
    /// An agent's turn: its model calls and operator-key calls (through
    /// the computer's intercept), or the in-fragment agent's.
    AgentTurn,
    /// A fragment's AI step (a job's `ai.*`, voice input).
    AiStep,
    /// A computer's wake (its awake time is metered after).
    Wake,
    /// A write to a fragment, whose hosting its owner pays (metered after).
    Write,
    /// Making a fragment, whose hosting its maker will pay: a write to a
    /// fragment of their own, and a guest makes none.
    Create,
}

/// Rows a source metered since its last flush, under an id of the batch's
/// own: a batch retried after a crash between meter and flush is applied
/// once, and each row is charged once by its reference whatever batch
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meter {
    pub batch: String,
    pub rows: Vec<MeterRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterRow {
    /// The usage's reference (the gateway's log id, a run id, a computer's
    /// awake interval, a storage sample): the ledger's replay key.
    #[serde(rename = "ref")]
    pub reference: String,
    pub usage: Usage,
    pub fragment: Option<String>,
    pub agent: Option<String>,
    pub computer: Option<String>,
    /// When it happened (a sample's time), Unix ms.
    pub at_ms: i64,
}

/// A price book from the deploy's configuration, as a ledger keeps it the
/// first time it prices with it: under `book:<digest>` (`PriceBook::digest`),
/// for the audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPriceBook {
    pub id: String,
    pub book: PriceBook,
}

/// Every command, as its record keeps it (to tell a replay from another
/// body under the same id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Grant(GrantCredit),
    SetPlan(SetPlan),
    SetSeat(SetSeat),
    SetOverdraft(SetOverdraft),
    SetCap(SetFragmentCap),
    SetBook(SetPriceBook),
    OrgBudget(OrgBudget),
}

/// A batch's answer, kept with the batch so a replay answers the same.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Metered {
    /// What the batch's new rows charged, together.
    pub charged: i64,
    pub rows_new: u32,
    /// Rows an earlier batch charged with the same body.
    pub rows_before: u32,
    /// Rows refused, by their index: they changed nothing, and a later
    /// batch may carry them again (once the book prices them, say).
    pub refused: Vec<RowRefused>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRefused {
    pub index: u32,
    pub why: Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swept {
    /// References and batches forgotten (past `REF_KEEP_MS`).
    pub forgotten: u64,
}

/// Why a mutation or a question was refused. A refusal changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refused {
    /// The request breaks a rule of its shape.
    Invalid { what: Invalid },
    /// This id was used with another body.
    ConflictingBody,
    /// A price book no newer than the ledger's: refused no more, since a
    /// ledger keeps no book (decision 62); kept so an answer of the
    /// releases before reads.
    StaleBook { current: u32 },
    /// A seat change no newer (by `seq`) than the last one applied
    /// (`current`): it changes nothing, and is kept by nothing. The seat's
    /// one writer, the registry, takes its next order past `current`.
    StaleSeat { current: u64 },
    /// The book prices no such model, instance type or key.
    NoPrice,
    /// More than one row may cost (`price::CHARGE_MAX`).
    TooLarge,
    /// A row from before what the ledger remembers (`REF_KEEP_MS`).
    Stale,
    /// A guest pays for nothing (decisions 25 and 36).
    GuestPayer,
    /// A guest makes no fragment: its hosting would bill them, and a guest
    /// pays for nothing (Paul, 2026-10-03).
    GuestCreates,
    AgentsStopped { why: Why },
    ReadOnly { why: Why },
    /// The fragment spent its cap this month: only its owner spends in it
    /// until the next month.
    CapReached { cap: i64, spent: i64 },
    /// What only a person's ledger has (a plan, a seat, an overdraft, an
    /// agent's turn or a computer's time), asked of an org's: its credit
    /// pays for its fragments alone (decision 67).
    PersonsOnly,
    /// What only an org's ledger has (a budget), asked of a person's.
    OrgsOnly,
    /// The org's fragments spent its budget this month: AI steps wait for
    /// a larger one or the next month (decision 68).
    BudgetReached { budget: i64, spent: i64 },
    /// The person was wiped (docs/api.md, Operators): their ledger keeps
    /// nothing but that, and takes nothing more. Not the core's to decide:
    /// the cell answers it for a ledger it emptied, so a row still on its
    /// way (a meter batch in the queue) is acknowledged, not retried.
    Wiped,
}

/// The rule a request broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invalid {
    /// An id or a name that is empty, too long, or not printable ASCII.
    Id,
    Usage(UsageFault),
    /// An amount outside its limit (a grant, a cap, an overdraft).
    Amount,
    /// A note longer than `NOTE_MAX_BYTES`.
    Note,
    /// A batch with no rows, or more than `BATCH_ROWS_MAX`.
    Rows,
    /// A row from before 1970, or past `CLOCK_SKEW_MS` ahead.
    Time,
    /// A seat change at order 0 (orders start at 1).
    Order,
}

impl From<PriceError> for Refused {
    fn from(e: PriceError) -> Refused {
        match e {
            PriceError::Invalid(fault) => Refused::Invalid { what: Invalid::Usage(fault) },
            PriceError::NoPrice => Refused::NoPrice,
            PriceError::TooLarge => Refused::TooLarge,
        }
    }
}

fn why_text(why: Why) -> &'static str {
    match why {
        Why::Guest => "a guest has no agents",
        Why::SeatCanceled => "the seat was canceled",
        Why::NoCredit => "the credit is used up; adding credit starts them again",
        Why::Overdrawn => "the balance reached the overdraft; credit that brings it above zero ends it",
    }
}

impl Refused {
    /// The platform's error code for the refusal: every money refusal is
    /// `budget_used_up` (402), with the reason in the message.
    pub fn code(self) -> ErrorCode {
        match self {
            Refused::Invalid { .. } | Refused::StaleBook { .. } | Refused::StaleSeat { .. } | Refused::NoPrice | Refused::Stale => ErrorCode::InvalidRequest,
            Refused::ConflictingBody => ErrorCode::ConflictingBody,
            Refused::Wiped => ErrorCode::NotFound,
            Refused::TooLarge => ErrorCode::TooLarge,
            // what a guest's plan does not allow, as a role does not: no
            // amount of credit is short
            Refused::GuestPayer | Refused::GuestCreates | Refused::PersonsOnly | Refused::OrgsOnly => ErrorCode::Forbidden,
            Refused::AgentsStopped { .. } | Refused::ReadOnly { .. } | Refused::CapReached { .. } | Refused::BudgetReached { .. } => ErrorCode::BudgetUsedUp,
        }
    }

    /// The refusal for people (the shell shows it).
    pub fn message(self) -> String {
        match self {
            Refused::Invalid { what } => format!("the request breaks a rule ({what:?})"),
            Refused::ConflictingBody => "this id was used with a different body".into(),
            Refused::StaleBook { current } => format!("the ledger charges with price book {current}; a new book has a higher version"),
            Refused::StaleSeat { current } => format!("the ledger's seat is at order {current}; a change to it has a higher one"),
            Refused::NoPrice => "the price book has no price for that".into(),
            Refused::TooLarge => "that is more than one row may cost".into(),
            Refused::Stale => "that row is older than the ledger remembers".into(),
            Refused::GuestPayer => "a guest pays for nothing: no agents, no AI, nothing billed".into(),
            Refused::GuestCreates => {
                "guests can't create fragments: a fragment's hosting bills its owner, and a guest pays for nothing; \
                 a guest still edits the fragments shared with them, and a seat makes fragments of its own"
                    .into()
            }
            Refused::AgentsStopped { why } => format!("agents are stopped: {}", why_text(why)),
            Refused::ReadOnly { why } => format!("fragments are read-only: {}", why_text(why)),
            Refused::CapReached { cap, spent } => {
                format!("this fragment spent {} of its {} cap this month: only its owner spends in it until next month", dollars(spent), dollars(cap))
            }
            Refused::PersonsOnly => "an org's credit pays for its fragments, never a person's plan, seat, agents or computer".into(),
            Refused::OrgsOnly => "only an org's ledger has a budget".into(),
            Refused::BudgetReached { budget, spent } => {
                format!("this org's fragments spent {} of its {} budget this month: AI steps wait until its billing admins raise it, or next month", dollars(spent), dollars(budget))
            }
            Refused::Wiped => "this person was wiped: their ledger takes nothing more".into(),
        }
    }
}

fn invalid(what: Invalid) -> Refused {
    Refused::Invalid { what }
}

fn valid_id(s: &str) -> bool {
    price::printable(s, ID_MAX_BYTES)
}

/// Why a grant's body is refused, whatever its ledger holds: the router
/// asks first for an org's, whose log records the grant before its ledger
/// takes it, so the log never names one the ledger would refuse.
pub fn grant_invalid(g: &GrantCredit) -> Option<Refused> {
    if !(1..=GRANT_MAX).contains(&g.micros) {
        return Some(invalid(Invalid::Amount));
    }
    if !valid_id(&g.by) {
        return Some(invalid(Invalid::Id));
    }
    (g.why.len() > NOTE_MAX_BYTES).then(|| invalid(Invalid::Note))
}

fn valid_names(names: [&Option<String>; 3]) -> bool {
    names.iter().all(|n| n.as_deref().is_none_or(valid_id))
}

/// A reference's record in the store, `{"entry": "row", …}` (or, from
/// before, `{"entry": "reservation", …}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A meter row: its price under the book of the time (`book`, its
    /// version; the book itself is kept by its digest, `book:<digest>`), and
    /// its charge (the price's, or nothing for a waived meter).
    Row { row: MeterRow, priced: Priced, charge: i64, book: u32 },
    /// A reservation of the releases before decision 62 (held, settled or
    /// released), kept as it was written: never charged here, a row under
    /// its reference another body, forgotten by its time as any.
    Before(serde_json::Value),
}

impl Serialize for Entry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Row<'a> {
            entry: &'static str,
            row: &'a MeterRow,
            priced: &'a Priced,
            charge: i64,
            book: u32,
        }
        match self {
            Entry::Row { row, priced, charge, book } => Row { entry: "row", row, priced, charge: *charge, book: *book }.serialize(s),
            Entry::Before(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Entry, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        struct Row {
            row: MeterRow,
            priced: Priced,
            charge: i64,
            book: u32,
        }
        let v = serde_json::Value::deserialize(d)?;
        match v.get("entry").and_then(serde_json::Value::as_str) {
            Some("row") => {
                let r: Row = serde_json::from_value(v).map_err(D::Error::custom)?;
                Ok(Entry::Row { row: r.row, priced: r.priced, charge: r.charge, book: r.book })
            }
            Some("reservation") if v.get("at_ms").is_some_and(serde_json::Value::is_i64) => Ok(Entry::Before(v)),
            other => Err(D::Error::custom(format!("an entry is a row or a reservation from before, not {other:?}"))),
        }
    }
}

impl Entry {
    /// When it happened: `Store::prune` forgets by this (a reservation from
    /// before, by its own time, so a replay of it stays refused as long).
    pub fn at_ms(&self) -> i64 {
        match self {
            Entry::Row { row, .. } => row.at_ms,
            Entry::Before(v) => v["at_ms"].as_i64().expect("a reservation from before has its time"),
        }
    }

    /// What it charged: a row's charge; a record from before, none here.
    pub fn charge(&self) -> Option<i64> {
        match self {
            Entry::Row { charge, .. } => Some(*charge),
            Entry::Before(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchRecord {
    /// SHA-256 of the batch's rows, to tell a replay from another body.
    pub digest: String,
    pub answer: Metered,
    /// The latest of its time and its rows': the record outlives its
    /// rows' references, so a replay answers from it while any is kept.
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command: Command,
    pub at_ms: i64,
}

/// What grows with use, by id. The Durable Object keeps each in a SQLite
/// table and runs a mutation and the head's write in one transaction;
/// `Memory` keeps them in maps. Commands are never forgotten (a grant's id
/// is Stripe's, and may come again any time; they are few); references
/// and batches are forgotten after `REF_KEEP_MS`.
pub trait Store {
    fn entry(&self, reference: &str) -> Option<Entry>;
    fn put_entry(&mut self, reference: &str, entry: Entry);
    fn batch(&self, id: &str) -> Option<BatchRecord>;
    fn put_batch(&mut self, id: &str, record: BatchRecord);
    fn command(&self, id: &str) -> Option<CommandRecord>;
    fn put_command(&mut self, id: &str, record: CommandRecord);
    /// A fragment's charges in a month.
    fn spent(&self, month: Month, fragment: &str) -> i64;
    fn add_spent(&mut self, month: Month, fragment: &str, micros: i64);
    /// A month's spend by fragment, the largest first, at most `limit`.
    fn spends(&self, month: Month, limit: usize) -> Vec<(String, i64)>;
    /// A month's spend, every fragment's together.
    fn month_spent(&self, month: Month) -> i64;
    fn cap(&self, fragment: &str) -> Option<i64>;
    fn set_cap(&mut self, fragment: &str, micros: Option<i64>);
    /// Forgets the entries and batches from before `before_ms` and the
    /// months of spend before `before_month`; answers how many entries and
    /// batches it forgot.
    fn prune(&mut self, before_ms: i64, before_month: Month) -> u64;
}

/// A store in memory, for tests and the simulation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Memory {
    entries: BTreeMap<String, Entry>,
    batches: BTreeMap<String, BatchRecord>,
    commands: BTreeMap<String, CommandRecord>,
    spent: BTreeMap<Month, BTreeMap<String, i64>>,
    caps: BTreeMap<String, i64>,
}

impl Store for Memory {
    fn entry(&self, reference: &str) -> Option<Entry> {
        self.entries.get(reference).cloned()
    }

    fn put_entry(&mut self, reference: &str, entry: Entry) {
        self.entries.insert(reference.to_string(), entry);
    }

    fn batch(&self, id: &str) -> Option<BatchRecord> {
        self.batches.get(id).cloned()
    }

    fn put_batch(&mut self, id: &str, record: BatchRecord) {
        self.batches.insert(id.to_string(), record);
    }

    fn command(&self, id: &str) -> Option<CommandRecord> {
        self.commands.get(id).cloned()
    }

    fn put_command(&mut self, id: &str, record: CommandRecord) {
        self.commands.insert(id.to_string(), record);
    }

    fn spent(&self, month: Month, fragment: &str) -> i64 {
        self.spent.get(&month).and_then(|m| m.get(fragment)).copied().unwrap_or(0)
    }

    fn add_spent(&mut self, month: Month, fragment: &str, micros: i64) {
        *self.spent.entry(month).or_default().entry(fragment.to_string()).or_insert(0) += micros;
    }

    fn month_spent(&self, month: Month) -> i64 {
        self.spent.get(&month).map_or(0, |m| m.values().sum())
    }

    fn spends(&self, month: Month, limit: usize) -> Vec<(String, i64)> {
        let mut all: Vec<(String, i64)> = self.spent.get(&month).map(|m| m.iter().map(|(f, s)| (f.clone(), *s)).collect()).unwrap_or_default();
        all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        all.truncate(limit);
        all
    }

    fn cap(&self, fragment: &str) -> Option<i64> {
        self.caps.get(fragment).copied()
    }

    fn set_cap(&mut self, fragment: &str, micros: Option<i64>) {
        match micros {
            Some(m) => self.caps.insert(fragment.to_string(), m),
            None => self.caps.remove(fragment),
        };
    }

    fn prune(&mut self, before_ms: i64, before_month: Month) -> u64 {
        let (entries, batches) = (self.entries.len(), self.batches.len());
        self.entries.retain(|_, e| e.at_ms() >= before_ms);
        self.batches.retain(|_, b| b.at_ms >= before_ms);
        self.spent.retain(|m, _| *m >= before_month);
        ((entries - self.entries.len()) + (batches - self.batches.len())) as u64
    }
}

/// What a seat includes each month: nothing unless it is active (a seat
/// past due keeps this month's credit, and gets next month's when it is
/// active again).
pub fn included(plan: Plan, seat: SeatState) -> i64 {
    match (plan, seat) {
        (Plan::Seat, SeatState::Active) => SEAT_INCLUDED,
        (Plan::SeatAlwaysOn, SeatState::Active) => SEAT_ALWAYS_ON_INCLUDED,
        _ => 0,
    }
}

/// A person's standing, from their plan, seat, balance, and whether the
/// overdraft made their fragments read-only (decision 27).
pub fn standing_of(plan: Plan, seat: SeatState, balance: i64, read_only: bool) -> Standing {
    if read_only {
        return Standing::ReadOnly { why: Why::Overdrawn };
    }
    match (plan, seat) {
        (Plan::Guest, _) => Standing::AgentsStopped { why: Why::Guest },
        (_, SeatState::Canceled) => Standing::AgentsStopped { why: Why::SeatCanceled },
        _ if balance > 0 => Standing::Ok,
        _ => Standing::AgentsStopped { why: Why::NoCredit },
    }
}

/// Everything that ever moved the balance: the balance is always
/// `purchased + included - expired - charged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Totals {
    pub purchased: i64,
    pub included: i64,
    pub expired: i64,
    pub charged: i64,
}

/// The balance's parts and what decides them. Small and `Copy`: a read
/// rolls a copy to its own month, so it sees that month's credit before
/// any mutation has rolled the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Money {
    plan: Plan,
    seat: SeatState,
    /// The last seat change's order (`SetSeat::seq`).
    seat_seq: u64,
    overdraft: i64,
    month: Month,
    /// This month's included credit as granted, and what is left of it.
    included_granted: i64,
    included_left: i64,
    /// Purchased credit left; below zero while the person owes.
    purchased_left: i64,
    /// Set at the overdraft, cleared above zero.
    read_only: bool,
    totals: Totals,
}

impl Money {
    fn balance(&self) -> i64 {
        self.included_left + self.purchased_left
    }

    fn standing(&self) -> Standing {
        standing_of(self.plan, self.seat, self.balance(), self.read_only)
    }

    /// Brings the money to `month`. Months only move forward: a late time
    /// rolls nothing back. Skipped months grant nothing (they are over).
    fn roll(&mut self, month: Month) {
        if month <= self.month {
            return;
        }
        // the old month's included credit expires: it never carries over
        self.totals.expired += self.included_left;
        self.included_left = 0;
        self.included_granted = 0;
        self.month = month;
        self.entitle();
        self.latch();
    }

    /// Grants what is missing of this month's included credit for the plan
    /// and seat now: a seat that becomes active, or a plan that grows,
    /// mid-month gets the difference, once; nothing is taken back when one
    /// shrinks (it expires at the month's end anyway). A debt is paid from
    /// it first.
    fn entitle(&mut self) {
        let target = included(self.plan, self.seat);
        if target <= self.included_granted {
            return;
        }
        let grant = target - self.included_granted;
        let pay = grant.min((-self.purchased_left).max(0));
        self.included_granted = target;
        self.totals.included += grant;
        self.purchased_left += pay;
        self.included_left += grant - pay;
    }

    /// A charge, drawn from included credit first.
    fn draw(&mut self, amount: i64) {
        assert!((0..=price::CHARGE_MAX).contains(&amount), "a charge was priced within CHARGE_MAX");
        let from_included = amount.min(self.included_left);
        self.included_left -= from_included;
        self.purchased_left -= amount - from_included;
        self.totals.charged += amount;
    }

    /// Read-only at the overdraft, writable again above zero.
    fn latch(&mut self) {
        let balance = self.balance();
        if balance <= -self.overdraft {
            self.read_only = true;
        } else if balance > 0 {
            self.read_only = false;
        }
    }

    fn assert_valid(&self) {
        assert!(self.included_left >= 0, "included credit is never overspent");
        assert!(self.included_left <= self.included_granted, "included credit never carries past its month");
        assert!(self.included_granted <= SEAT_ALWAYS_ON_INCLUDED, "a month includes at most the largest seat's credit");
        assert!(self.purchased_left >= 0 || self.included_left == 0, "a debt is paid from included credit as it arrives");
        assert!((0..=OVERDRAFT_MAX).contains(&self.overdraft), "an overdraft is within its limit");
        let balance = self.balance();
        assert!(!self.read_only || balance <= 0, "read-only ends above zero");
        assert!(self.read_only || balance > -self.overdraft, "the overdraft makes fragments read-only");
        let t = self.totals;
        assert_eq!(balance, t.purchased + t.included - t.expired - t.charged, "the balance is what moved it");
    }
}

/// What may still be charged before the person's fragments go read-only
/// (`LedgerStatus::available_micros`): the balance plus the overdraft's
/// room, none once read-only.
fn available(money: &Money) -> i64 {
    if money.read_only {
        0
    } else {
        money.balance() + money.overdraft
    }
}

/// Whether `spend` may start on these means. Making a fragment is a write
/// to one of the payer's own: it goes as a write does, but a guest, whose
/// fragments would be billed nothing, makes none.
fn gate(money: &Money, spend: Spend) -> Result<(), Refused> {
    if money.plan == Plan::Guest {
        return Err(match spend {
            Spend::Create => Refused::GuestCreates,
            Spend::AgentTurn | Spend::AiStep | Spend::Wake | Spend::Write => Refused::GuestPayer,
        });
    }
    allowed(money.standing(), spend)
}

/// Whether `spend` may start at `standing`: writes until read-only, the
/// rest only at `Ok`.
fn allowed(standing: Standing, spend: Spend) -> Result<(), Refused> {
    match (spend, standing) {
        (_, Standing::ReadOnly { why }) => Err(Refused::ReadOnly { why }),
        (Spend::Write | Spend::Create, Standing::AgentsStopped { .. } | Standing::Ok) => Ok(()),
        (_, Standing::AgentsStopped { why }) => Err(Refused::AgentsStopped { why }),
        (_, Standing::Ok) => Ok(()),
    }
}

/// A payer's ledger: the head of its state (see the module's docs). The
/// fields a head kept before names that this one does not (`holds`,
/// `book`) are let go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    money: Money,
    /// An org's terms, never written in the head: an older release that
    /// rewrites a head it reads (its sweep) drops what it does not know.
    /// The cell binds them as it reads one, from its ledger's name and its
    /// own row of the budget (`bind_org`).
    #[serde(skip)]
    org: Option<org::OrgTerms>,
}

impl Ledger {
    /// A new person's ledger: a guest with no credit.
    pub fn new(now_ms: i64) -> Ledger {
        let money = Money {
            plan: Plan::Guest,
            seat: SeatState::Active,
            seat_seq: 0,
            overdraft: OVERDRAFT_DEFAULT,
            month: Month::of(now_ms),
            included_granted: 0,
            included_left: 0,
            purchased_left: 0,
            read_only: false,
            totals: Totals::default(),
        };
        let ledger = Ledger { money, org: None };
        ledger.assert_valid();
        ledger
    }

    // Reads. Each sees the ledger as of `now_ms`, changing nothing.

    fn money_at(&self, now_ms: i64) -> Money {
        let mut money = self.money;
        money.roll(Month::of(now_ms));
        money
    }

    pub fn standing(&self, now_ms: i64) -> Standing {
        let money = self.money_at(now_ms);
        match self.org {
            Some(_) => org::standing(&money),
            None => money.standing(),
        }
    }

    /// Whether this payer's money lets `spend` start (decisions 25, 27
    /// and 67). A spend in a fragment someone else owns also asks that
    /// owner's ledger `fragment_open`.
    pub fn gate(&self, spend: Spend, now_ms: i64) -> Result<(), Refused> {
        let money = self.money_at(now_ms);
        match self.org {
            Some(_) => org::gate(&money, spend),
            None => gate(&money, spend),
        }
    }

    /// Whether `fragment`, which this ledger's person owns, is still open
    /// this month to spenders other than its owner (decision 26): its
    /// charges are under its cap.
    pub fn fragment_open(&self, store: &impl Store, fragment: &str, now_ms: i64) -> Result<(), Refused> {
        if !valid_id(fragment) {
            return Err(invalid(Invalid::Id));
        }
        let month = self.money_at(now_ms).month;
        let spent = store.spent(month, fragment);
        let cap = store.cap(fragment).unwrap_or(CAP_DEFAULT);
        if spent < cap {
            Ok(())
        } else {
            Err(Refused::CapReached { cap, spent })
        }
    }

    /// May this spend start: `spend` by this payer, in `fragment` (when
    /// it is in one), `by_owner` when the spender is the fragment's owner
    /// or their agent acting for them. When this payer owns the fragment
    /// (or there is none), this is the whole answer; when another person
    /// owns it, that owner's ledger answers `fragment_open` too (an agent
    /// working for Skyler in Paul's fragment: Skyler pays, Paul's cap
    /// applies; decisions 26 and 36). Caps stop AI steps and agent turns,
    /// never writes or wakes. An org's fragments have no owner who passes
    /// their cap, and their AI steps stop at its budget too (decision 68).
    pub fn may_spend(&self, store: &impl Store, spend: Spend, fragment: Option<&str>, by_owner: bool, now_ms: i64) -> Result<(), Refused> {
        self.gate(spend, now_ms)?;
        if let Some(terms) = &self.org {
            return terms.may_spend(self, store, spend, fragment, now_ms);
        }
        match (spend, fragment, by_owner) {
            (Spend::AgentTurn | Spend::AiStep, Some(f), false) => self.fragment_open(store, f, now_ms),
            _ => Ok(()),
        }
    }

    /// The ledger now, with the version of `book`, the configured book its
    /// release charges with.
    pub fn status(&self, store: &impl Store, now_ms: i64, book: &PriceBook) -> LedgerStatus {
        let money = self.money_at(now_ms);
        let fragments = store
            .spends(money.month, STATUS_FRAGMENTS)
            .into_iter()
            .map(|(fragment, spent)| {
                let cap = store.cap(&fragment).unwrap_or(CAP_DEFAULT);
                FragmentSpend { fragment, spent_micros: spent, cap_micros: cap }
            })
            .collect();
        LedgerStatus {
            plan: money.plan,
            seat: money.seat,
            month: money.month.label(),
            balance_micros: money.balance(),
            included_micros: money.included_left,
            included_granted_micros: money.included_granted,
            purchased_micros: money.purchased_left,
            reserved_micros: 0,
            available_micros: available(&money),
            overdraft_micros: money.overdraft,
            standing: money.standing(),
            price_book: book.version,
            fragments,
        }
    }

    pub fn totals(&self) -> Totals {
        self.money.totals
    }

    /// When the Durable Object's alarm should run `sweep` next: once a
    /// `SWEEP_EVERY_MS`, to forget what is past `REF_KEEP_MS`.
    pub fn next_sweep_ms(&self, now_ms: i64) -> i64 {
        now_ms + SWEEP_EVERY_MS
    }

    fn roll(&mut self, now_ms: i64) {
        self.money.roll(Month::of(now_ms));
    }

    /// A usage's price, and its charge here: a $200 seat's awake time is
    /// not metered (decision 25; one computer per person, decision 13).
    fn charge_for(&self, usage: &Usage, book: &PriceBook) -> Result<(Priced, i64), PriceError> {
        let priced = book.price(usage)?;
        let waived = matches!(usage, Usage::Awake { .. }) && self.money.plan == Plan::SeatAlwaysOn;
        Ok((priced, if waived { 0 } else { priced.charge }))
    }

    /// A charge to the balance, counted in its fragment's month.
    fn charge(&mut self, store: &mut impl Store, amount: i64, fragment: Option<&str>, month: Month) {
        self.money.draw(amount);
        if let (Some(f), true) = (fragment, amount > 0) {
            store.add_spent(month, f, amount);
        }
    }

    /// The head's invariants. Every mutation checks them as it ends; the
    /// Durable Object checks them again after reading the head back.
    pub fn assert_valid(&self) {
        self.money.assert_valid();
        if self.org.is_some() {
            org::assert_valid(&self.money);
        }
    }

    // Commands: each idempotent by its id.

    /// Whether command `id` is new (`true`) or a replay of the same
    /// command (`false`); the same id with another body is refused.
    fn admit(store: &impl Store, id: &str, command: &Command) -> Result<bool, Refused> {
        if !valid_id(id) {
            return Err(invalid(Invalid::Id));
        }
        match store.command(id) {
            None => Ok(true),
            Some(record) if record.command == *command => Ok(false),
            Some(_) => Err(Refused::ConflictingBody),
        }
    }

    fn done(&mut self, store: &mut impl Store, id: &str, command: Command, now_ms: i64) {
        self.money.latch();
        store.put_command(id, CommandRecord { command, at_ms: now_ms });
        self.assert_valid();
    }

    /// Purchased credit (Stripe's hook) or an operator's gift.
    pub fn grant(&mut self, store: &mut impl Store, g: &GrantCredit, now_ms: i64) -> Result<(), Refused> {
        self.roll(now_ms);
        let command = Command::Grant(g.clone());
        if !Self::admit(store, &g.id, &command)? {
            return Ok(());
        }
        if let Some(why) = grant_invalid(g) {
            return Err(why);
        }
        self.money.purchased_left += g.micros;
        self.money.totals.purchased += g.micros;
        self.done(store, &g.id, command, now_ms);
        Ok(())
    }

    pub fn set_plan(&mut self, store: &mut impl Store, c: &SetPlan, now_ms: i64) -> Result<(), Refused> {
        self.persons_only()?;
        self.roll(now_ms);
        let command = Command::SetPlan(c.clone());
        if !Self::admit(store, &c.id, &command)? {
            return Ok(());
        }
        self.money.plan = c.plan;
        self.money.entitle();
        self.done(store, &c.id, command, now_ms);
        Ok(())
    }

    /// A seat's state, from the registry, its one writer (docs/billing.md).
    /// A change no newer than the last one applied (by `seq`) is refused as
    /// stale, naming the order the ledger is at, so a late `active` never
    /// undoes a `canceled`, and its sender learns it was not applied (a
    /// replay of one that was answers as it did).
    pub fn set_seat(&mut self, store: &mut impl Store, c: &SetSeat, now_ms: i64) -> Result<(), Refused> {
        self.persons_only()?;
        self.roll(now_ms);
        let command = Command::SetSeat(c.clone());
        if !Self::admit(store, &c.id, &command)? {
            return Ok(());
        }
        if c.seq == 0 {
            return Err(invalid(Invalid::Order));
        }
        if c.seq <= self.money.seat_seq {
            return Err(Refused::StaleSeat { current: self.money.seat_seq });
        }
        self.money.seat = c.seat;
        self.money.seat_seq = c.seq;
        self.money.entitle();
        self.done(store, &c.id, command, now_ms);
        Ok(())
    }

    /// An operator's overdraft for this person. It decides read-only
    /// afresh: an operator who raises it lets the fragments write again.
    pub fn set_overdraft(&mut self, store: &mut impl Store, c: &SetOverdraft, now_ms: i64) -> Result<(), Refused> {
        self.persons_only()?;
        self.roll(now_ms);
        let command = Command::SetOverdraft(c.clone());
        if !Self::admit(store, &c.id, &command)? {
            return Ok(());
        }
        if !(0..=OVERDRAFT_MAX).contains(&c.micros) {
            return Err(invalid(Invalid::Amount));
        }
        self.money.overdraft = c.micros;
        self.money.read_only = self.money.balance() <= -c.micros;
        self.done(store, &c.id, command, now_ms);
        Ok(())
    }

    /// A fragment's cap, set by its owner on the owner's ledger.
    pub fn set_cap(&mut self, store: &mut impl Store, c: &SetFragmentCap, now_ms: i64) -> Result<(), Refused> {
        self.roll(now_ms);
        let command = Command::SetCap(c.clone());
        if !Self::admit(store, &c.id, &command)? {
            return Ok(());
        }
        if !valid_id(&c.fragment) {
            return Err(invalid(Invalid::Id));
        }
        if c.micros.is_some_and(|m| !(0..=CAP_MAX).contains(&m)) {
            return Err(invalid(Invalid::Amount));
        }
        store.set_cap(&c.fragment, c.micros);
        self.done(store, &c.id, command, now_ms);
        Ok(())
    }

    // Meters: usage that already happened, charged once by reference.

    /// A batch, its rows priced with `book`: the configured book of the
    /// release applying it (every call into a ledger is its own release's),
    /// kept once as a command (`book:<digest>`) the first time it prices a
    /// batch, for the audit.
    pub fn meter(&mut self, store: &mut impl Store, m: &Meter, now_ms: i64, book: &PriceBook) -> Result<Metered, Refused> {
        assert_eq!(book.validate(), Ok(()), "a ledger charges with a valid book");
        self.roll(now_ms);
        if !valid_id(&m.batch) {
            return Err(invalid(Invalid::Id));
        }
        if m.rows.is_empty() || m.rows.len() > BATCH_ROWS_MAX {
            return Err(invalid(Invalid::Rows));
        }
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&m.rows).expect("meter rows serialize")));
        if let Some(record) = store.batch(&m.batch) {
            return if record.digest == digest { Ok(record.answer) } else { Err(Refused::ConflictingBody) };
        }
        // an org's plan is a guest's, and it pays
        if self.money.plan == Plan::Guest && self.org.is_none() {
            return Err(Refused::GuestPayer);
        }
        let id = format!("book:{}", book.digest());
        if store.command(&id).is_none() {
            store.put_command(&id, CommandRecord { command: Command::SetBook(SetPriceBook { id: id.clone(), book: book.clone() }), at_ms: now_ms });
        }
        let charged_before = self.money.totals.charged;
        let mut answer = Metered::default();
        let mut at_ms = now_ms;
        // bounded by BATCH_ROWS_MAX, checked above
        for (index, row) in m.rows.iter().enumerate() {
            match self.meter_row(store, row, now_ms, book) {
                Ok(Some(charge)) => {
                    answer.charged += charge;
                    answer.rows_new += 1;
                }
                Ok(None) => answer.rows_before += 1,
                Err(why) => answer.refused.push(RowRefused { index: index as u32, why }),
            }
            at_ms = at_ms.max(row.at_ms);
        }
        assert_eq!(self.money.totals.charged - charged_before, answer.charged, "a batch charges what its new rows did");
        assert_eq!(answer.rows_new as usize + answer.rows_before as usize + answer.refused.len(), m.rows.len(), "every row has an outcome");
        store.put_batch(&m.batch, BatchRecord { digest, answer: answer.clone(), at_ms });
        self.money.latch();
        self.assert_valid();
        Ok(answer)
    }

    /// One row: `Some(charge)` when charged now, `None` when an earlier
    /// batch charged it with the same body.
    fn meter_row(&mut self, store: &mut impl Store, row: &MeterRow, now_ms: i64, book: &PriceBook) -> Result<Option<i64>, Refused> {
        if self.org.is_some() && !org::orgs_row(row) {
            return Err(Refused::PersonsOnly);
        }
        if !valid_id(&row.reference) || !valid_names([&row.fragment, &row.agent, &row.computer]) {
            return Err(invalid(Invalid::Id));
        }
        row.usage.validate().map_err(|fault| invalid(Invalid::Usage(fault)))?;
        if row.at_ms < 0 || row.at_ms > now_ms + CLOCK_SKEW_MS {
            return Err(invalid(Invalid::Time));
        }
        if row.at_ms < now_ms - REF_KEEP_MS {
            return Err(Refused::Stale);
        }
        match store.entry(&row.reference) {
            None => {}
            Some(Entry::Row { row: first, .. }) if first == *row => return Ok(None),
            // another body under this reference, or a record from before
            Some(_) => return Err(Refused::ConflictingBody),
        }
        let (priced, charge) = self.charge_for(&row.usage, book)?;
        // a row counts in its own month: a late one from last month does
        // not spend this month's cap
        self.charge(store, charge, row.fragment.as_deref(), Month::of(row.at_ms));
        store.put_entry(&row.reference, Entry::Row { row: row.clone(), priced, charge, book: book.version });
        Ok(Some(charge))
    }

    /// Forgets what is past `REF_KEEP_MS`. The Durable Object's alarm runs
    /// it at `next_sweep_ms`.
    pub fn sweep(&mut self, store: &mut impl Store, now_ms: i64) -> Swept {
        self.roll(now_ms);
        let horizon = now_ms - REF_KEEP_MS;
        let forgotten = if horizon > 0 { store.prune(horizon, Month::of(horizon)) } else { 0 };
        self.money.latch();
        self.assert_valid();
        Swept { forgotten }
    }
}
