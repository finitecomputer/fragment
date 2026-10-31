//! The ledger's mutations one at a time. For each: the valid case, every
//! refusal (a refusal changes nothing), a replay (the same id again
//! answers as the first did and changes nothing; another body under it is
//! refused), and a restart (the state written and read back answers the
//! rest as the state that never stopped). Then the money rules (decisions
//! 25–27 and 62) and the port's fix (bug 2), each on its own; and what the
//! releases before decision 62 kept (holds, reservations) reads.

use fragment_proto::ledger::{GrantCredit, Plan, SeatState, SetFragmentCap, SetOverdraft, SetPlan, SetSeat, Standing, Why};
use fragment_proto::ErrorCode;

use super::sim::{apply, Op, Outcome, T0};
use super::*;
use crate::price::{KeyPrice, PriceBook, StorageClass, UsageFault, USD};

const FLASH: &str = "@cf/zai-org/glm-5.3-flash";
/// A month after T0: 2026-11-01T00:00:00Z.
const NOVEMBER: i64 = T0 + 31 * DAY_MS;
/// A Flash turn of $0.004 at list, charged $0.0063 (price.rs).
const TURN: i64 = 6_300;

fn flash(input: u64, output: u64) -> Usage {
    Usage::Tokens { model: FLASH.into(), input, cached_input: 0, cache_write: 0, output }
}

fn turn() -> Usage {
    flash(26_000, 200)
}

/// The default book, plus a key that charges one micro-dollar a unit (2
/// per 3 units, × 1.5): `spend` moves the balance by exact amounts.
/// The defaults' prices, at version 1 (the books these tests set are
/// numbered from there), and a key.
fn book() -> PriceBook {
    let mut book = PriceBook::defaults();
    book.version = 1;
    book.keys.push(KeyPrice { key: "micro".into(), micros: 2, per: 3 });
    book
}

fn grant(id: &str, micros: i64) -> Op {
    Op::Grant(GrantCredit { id: id.into(), micros, by: "id:operator".into(), why: "a test".into() })
}

fn plan(id: &str, plan: Plan) -> Op {
    Op::Plan(SetPlan { id: id.into(), plan })
}

fn seat(id: &str, seat: SeatState, seq: u64) -> Op {
    Op::Seat(SetSeat { id: id.into(), seat, seq })
}

fn overdraft(id: &str, micros: i64) -> Op {
    Op::Overdraft(SetOverdraft { id: id.into(), micros })
}

fn cap(id: &str, fragment: &str, micros: Option<i64>) -> Op {
    Op::Cap(SetFragmentCap { id: id.into(), fragment: fragment.into(), micros })
}

/// `book()` at another margin.
fn set_book(margin_bp: u32) -> Op {
    Op::Book(PriceBook { margin_bp, ..book() })
}

fn row(reference: &str, usage: Usage, fragment: Option<&str>, at_ms: i64) -> MeterRow {
    MeterRow { reference: reference.into(), usage, fragment: fragment.map(str::to_string), agent: None, computer: None, at_ms }
}

fn meter(batch: &str, rows: Vec<MeterRow>) -> Op {
    Op::Meter(Meter { batch: batch.into(), rows })
}

struct World {
    ledger: Ledger,
    store: Memory,
    now: i64,
    /// The configured book of the release applying each op.
    book: PriceBook,
}

impl World {
    fn guest() -> World {
        World { ledger: Ledger::new(T0), store: Memory::default(), now: T0, book: book() }
    }

    /// A $100 seat, active, with its $50 for October.
    fn seat() -> World {
        let mut w = World::guest();
        w.ok(plan("plan:seat", Plan::Seat));
        w
    }

    fn apply(&mut self, op: &Op) -> Result<Outcome, Refused> {
        apply(&mut self.ledger, &mut self.store, op, self.now, &mut self.book)
    }

    fn ok(&mut self, op: Op) -> Outcome {
        self.apply(&op).unwrap_or_else(|e| panic!("{op:?} was refused: {e:?}"))
    }

    fn status(&self) -> LedgerStatus {
        self.ledger.status(&self.store, self.now, &self.book)
    }

    fn balance(&self) -> i64 {
        self.status().balance_micros
    }

    /// Meters `micros` of usage through the exact key, in `fragment`.
    fn spend_in(&mut self, id: &str, micros: i64, fragment: Option<&str>) {
        let usage = Usage::Key { key: "micro".into(), units: micros as u64 };
        let answer = self.ok(meter(id, vec![row(id, usage, fragment, self.now)]));
        let Outcome::Metered(m) = answer else { panic!("a batch answers Metered") };
        assert_eq!((m.charged, m.rows_new), (micros, 1), "the exact key charges what it meters");
    }

    fn spend(&mut self, id: &str, micros: i64) {
        self.spend_in(id, micros, None);
    }

    /// What a one-row batch of its own (`id`, the row's reference too)
    /// charged: a paid call's charge, as the cell sends it after the call.
    fn charged(&mut self, id: &str, usage: Usage) -> i64 {
        let Outcome::Metered(m) = self.ok(meter(id, vec![row(id, usage, None, self.now)])) else { panic!("a batch answers Metered") };
        assert_eq!((m.rows_new, m.refused.len()), (1, 0), "{id} is charged");
        m.charged
    }

    /// `op` is refused with `why`, and nothing changes.
    fn refused(&mut self, op: Op, why: Refused) {
        let (ledger, store) = (self.ledger.clone(), self.store.clone());
        assert_eq!(self.apply(&op), Err(why), "{op:?}");
        assert_eq!((&self.ledger, &self.store), (&ledger, &store), "a refusal changes nothing");
    }

    /// `op` applied, then again: the second answers as the first did and
    /// changes nothing.
    fn replays(&mut self, op: Op) -> Outcome {
        let first = self.ok(op.clone());
        let (ledger, store) = (self.ledger.clone(), self.store.clone());
        assert_eq!(self.apply(&op), Ok(first.clone()), "a replay of {op:?} answers as the first did");
        assert_eq!((&self.ledger, &self.store), (&ledger, &store), "a replay of {op:?} changes nothing");
        first
    }

    /// The state written and read back, as the Durable Object's is across
    /// a restart.
    fn restarted(&self) -> World {
        let ledger: Ledger = serde_json::from_str(&serde_json::to_string(&self.ledger).unwrap()).unwrap();
        let store: Memory = serde_json::from_str(&serde_json::to_string(&self.store).unwrap()).unwrap();
        assert_eq!((&ledger, &store), (&self.ledger, &self.store), "the state reads back as it was written");
        World { ledger, store, now: self.now, book: self.book.clone() }
    }
}

#[test]
fn months_are_utc_and_count_from_1970() {
    assert_eq!(Month::of(0), Month(0));
    assert_eq!(Month::of(0).label(), "1970-01");
    assert_eq!(Month::of(T0), Month(681));
    assert_eq!(Month::of(T0).label(), "2026-10");
    assert_eq!(Month::of(T0 - 1).label(), "2026-09");
    assert_eq!(Month::of(NOVEMBER).label(), "2026-11");
    assert_eq!(Month::of(1_709_164_800_000).label(), "2024-02", "2024-02-29");
    assert_eq!(Month::of(1_767_225_599_999).label(), "2025-12");
    assert_eq!(Month::of(1_767_225_600_000).label(), "2026-01");
    // a label reads back as its month, and nothing else is one
    for ms in [0, T0, T0 - 1, NOVEMBER, 1_709_164_800_000] {
        assert_eq!(Month::parse(&Month::of(ms).label()), Some(Month::of(ms)));
    }
    for bad in ["", "2026", "2026-13", "2026-00", "1969-12", "26-10", "2026-1", "2026/10", "+026-10", "2026-10-01"] {
        assert_eq!(Month::parse(bad), None, "{bad}");
    }
}

#[test]
fn the_standing_is_a_pure_function_of_plan_seat_balance_and_latch() {
    use SeatState::*;
    let cases = [
        (Plan::Guest, Active, 10, false, Standing::AgentsStopped { why: Why::Guest }),
        (Plan::Seat, Canceled, 10, false, Standing::AgentsStopped { why: Why::SeatCanceled }),
        (Plan::Seat, Active, 0, false, Standing::AgentsStopped { why: Why::NoCredit }),
        (Plan::SeatAlwaysOn, Active, -1, false, Standing::AgentsStopped { why: Why::NoCredit }),
        (Plan::Seat, Active, 1, false, Standing::Ok),
        (Plan::Seat, PastDue, 1, false, Standing::Ok),
        (Plan::Seat, Active, -5, true, Standing::ReadOnly { why: Why::Overdrawn }),
        (Plan::Seat, Canceled, 0, true, Standing::ReadOnly { why: Why::Overdrawn }),
    ];
    for (plan, seat, balance, read_only, standing) in cases {
        assert_eq!(standing_of(plan, seat, balance, read_only), standing, "{plan:?} {seat:?} {balance} {read_only}");
    }
    assert_eq!(included(Plan::Seat, Active), 50 * USD);
    assert_eq!(included(Plan::SeatAlwaysOn, Active), 100 * USD);
    for (p, s) in [(Plan::Guest, Active), (Plan::Seat, PastDue), (Plan::SeatAlwaysOn, Canceled)] {
        assert_eq!(included(p, s), 0, "{p:?} {s:?}");
    }
}

/// A new person is a guest: no credit, no agents, nothing billed to them.
#[test]
fn a_new_ledger_is_a_guest_who_pays_for_nothing() {
    let mut w = World::guest();
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::Guest });
    for spend in [Spend::AgentTurn, Spend::AiStep, Spend::Wake, Spend::Write] {
        assert_eq!(w.ledger.gate(spend, T0), Err(Refused::GuestPayer), "{spend:?}");
    }
    assert_eq!(w.ledger.gate(Spend::Create, T0), Err(Refused::GuestCreates), "nor makes a fragment");
    w.refused(meter("b1", vec![row("row:1", Usage::Requests { count: 1 }, None, T0)]), Refused::GuestPayer);
    let s = w.status();
    assert_eq!((s.plan, s.seat, s.month.as_str(), s.balance_micros, s.included_micros), (Plan::Guest, SeatState::Active, "2026-10", 0, 0));
    assert_eq!((s.overdraft_micros, s.price_book, s.reserved_micros, s.available_micros), (OVERDRAFT_DEFAULT, 1, 0, OVERDRAFT_DEFAULT));
}

/// Goal: a guest makes no fragment (Paul, 2026-10-03), and a seat does,
/// whatever its credit, until its fragments are read-only. Method: the
/// question is a read, so it is asked twice (a replay answers the same and
/// changes nothing), then again after an operator's plan change.
#[test]
fn a_guest_makes_no_fragments_and_a_seat_does() {
    let mut w = World::guest();
    let may = |w: &World| w.ledger.may_spend(&w.store, Spend::Create, None, true, w.now);
    let (ledger, store) = (w.ledger.clone(), w.store.clone());
    assert_eq!(may(&w), Err(Refused::GuestCreates));
    assert_eq!(may(&w), Err(Refused::GuestCreates), "asked again, the same");
    assert_eq!((&w.ledger, &w.store), (&ledger, &store), "a question changes nothing");
    let refused = Refused::GuestCreates;
    assert_eq!(refused.code(), ErrorCode::Forbidden, "a plan's refusal, as a role's: no credit is short");
    assert!(refused.message().starts_with("guests can't create fragments"), "{}", refused.message());
    w.ok(plan("plan:seat", Plan::Seat));
    assert_eq!(may(&w), Ok(()), "a seat makes fragments");
    w.spend("b1", 50 * USD);
    assert_eq!((w.ledger.standing(T0), may(&w)), (Standing::AgentsStopped { why: Why::NoCredit }, Ok(())), "at zero too, as it writes");
    w.ok(seat("s1", SeatState::Canceled, 1));
    assert_eq!(may(&w), Ok(()), "a canceled seat too, on what is left");
    w.spend("b2", 2 * USD);
    assert_eq!(may(&w), Err(Refused::ReadOnly { why: Why::Overdrawn }), "past the overdraft its fragments are read-only, a new one too");
    w.ok(plan("plan:guest", Plan::Guest));
    assert_eq!(may(&w), Err(Refused::GuestCreates), "made a guest again: the plan first");
    w.ok(seat("s2", SeatState::Active, 2));
    w.ok(plan("plan:seat-again", Plan::Seat));
    w.ok(grant("g1", 5 * USD));
    assert_eq!(may(&w), Ok(()));
    assert_eq!(may(&w.restarted()), Ok(()), "restarted, the same answer");
}

// Grants

#[test]
fn a_grant_adds_purchased_credit_once() {
    let mut w = World::seat();
    w.replays(grant("g1", 10 * USD));
    let s = w.status();
    assert_eq!((s.balance_micros, s.purchased_micros, s.included_micros), (60 * USD, 10 * USD, 50 * USD));
    assert_eq!(w.ledger.totals().purchased, 10 * USD);
    w.refused(grant("g1", 11 * USD), Refused::ConflictingBody);
    w.refused(plan("g1", Plan::Guest), Refused::ConflictingBody);
    // any plan may hold purchased credit; a guest's simply buys nothing yet
    let mut g = World::guest();
    g.ok(grant("g1", USD));
    assert_eq!(g.balance(), USD);
}

#[test]
fn a_grant_outside_its_rules_is_refused() {
    let mut w = World::seat();
    for micros in [0, -1, GRANT_MAX + 1] {
        w.refused(grant("g1", micros), Refused::Invalid { what: Invalid::Amount });
    }
    let long = "x".repeat(ID_MAX_BYTES + 1);
    for id in ["", "a b", long.as_str(), "é"] {
        w.refused(grant(id, USD), Refused::Invalid { what: Invalid::Id });
    }
    w.refused(Op::Grant(GrantCredit { id: "g1".into(), micros: USD, by: String::new(), why: String::new() }), Refused::Invalid { what: Invalid::Id });
    let long = "w".repeat(NOTE_MAX_BYTES + 1);
    w.refused(Op::Grant(GrantCredit { id: "g1".into(), micros: USD, by: "stripe".into(), why: long }), Refused::Invalid { what: Invalid::Note });
    // a refusal is not remembered: the id is still free
    w.ok(grant("g1", GRANT_MAX));
    assert_eq!(w.status().purchased_micros, GRANT_MAX);
}

/// Every command is idempotent by an id of the same shape as a
/// reference's; one that is not is refused before anything else.
#[test]
fn every_command_refuses_a_bad_id() {
    let mut w = World::seat();
    let bad = Refused::Invalid { what: Invalid::Id };
    for id in ["", "two words", "é"] {
        w.refused(grant(id, USD), bad);
        w.refused(plan(id, Plan::SeatAlwaysOn), bad);
        w.refused(seat(id, SeatState::Canceled, 1), bad);
        w.refused(overdraft(id, USD), bad);
        w.refused(cap(id, "todo.ann", Some(USD)), bad);
    }
}

// Plans and included credit

/// Decision 25: a seat's credit arrives each UTC month and expires at its
/// end; purchased credit stays; a charge spends included credit first.
#[test]
fn a_seats_credit_is_spent_first_and_expires_with_its_month() {
    let mut w = World::seat();
    assert_eq!(w.ledger.standing(T0), Standing::Ok);
    w.ok(grant("g1", 20 * USD));
    w.spend("b1", 3 * USD);
    let s = w.status();
    assert_eq!((s.included_micros, s.purchased_micros), (47 * USD, 20 * USD), "included credit first");
    w.now = NOVEMBER;
    w.spend("b2", USD);
    let s = w.status();
    assert_eq!((s.month.as_str(), s.included_micros, s.purchased_micros), ("2026-11", 49 * USD, 20 * USD));
    assert_eq!(w.ledger.totals().expired, 47 * USD, "October's credit expired");
    // a credit that runs out spends purchased credit next
    w.spend("b3", 60 * USD);
    let s = w.status();
    assert_eq!((s.included_micros, s.purchased_micros, s.balance_micros), (0, 9 * USD, 9 * USD));
}

#[test]
fn a_plan_that_grows_mid_month_gets_the_difference_once() {
    let mut w = World::seat();
    w.spend("b1", 10 * USD);
    w.replays(plan("p2", Plan::SeatAlwaysOn));
    let s = w.status();
    assert_eq!((s.included_granted_micros, s.included_micros), (100 * USD, 90 * USD), "the larger seat's credit, less what was spent");
    // a plan that shrinks takes nothing back; growing again grants nothing new
    w.ok(plan("p3", Plan::Seat));
    w.ok(plan("p4", Plan::SeatAlwaysOn));
    assert_eq!((w.status().included_granted_micros, w.status().included_micros), (100 * USD, 90 * USD));
    assert_eq!(w.ledger.totals().included, 100 * USD);
    w.refused(plan("p2", Plan::Guest), Refused::ConflictingBody);
    w.ok(plan("p5", Plan::Seat));
    w.now = NOVEMBER;
    assert_eq!(w.status().included_micros, 50 * USD, "November's credit is the plan's then");
}

// Seats

#[test]
fn a_seat_past_due_keeps_this_month_and_waits_for_the_next() {
    let mut w = World::seat();
    w.replays(seat("s1", SeatState::PastDue, 1));
    assert_eq!((w.status().included_micros, w.ledger.standing(T0)), (50 * USD, Standing::Ok));
    w.now = NOVEMBER;
    w.ok(seat("s0", SeatState::PastDue, 2));
    let s = w.status();
    assert_eq!((s.included_micros, s.standing), (0, Standing::AgentsStopped { why: Why::NoCredit }));
    w.ok(seat("s2", SeatState::Active, 3));
    assert_eq!(w.status().included_micros, 50 * USD, "November's credit when the seat is active again");
}

#[test]
fn a_canceled_seat_stops_agents_and_its_fragments_keep_writing() {
    let mut w = World::seat();
    w.ok(seat("s1", SeatState::Canceled, 1));
    let stopped = Refused::AgentsStopped { why: Why::SeatCanceled };
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::SeatCanceled });
    for spend in [Spend::AgentTurn, Spend::AiStep, Spend::Wake] {
        assert_eq!(w.ledger.gate(spend, T0), Err(stopped), "{spend:?}");
    }
    assert_eq!(w.ledger.gate(Spend::Write, T0), Ok(()));
    w.spend("b1", USD);
    assert_eq!(w.balance(), 49 * USD, "its fragments' hosting is still metered");
}

/// Pushes arrive out of order: a late `active` never undoes a `canceled`,
/// and is refused as stale, naming the order the ledger is at, so its
/// sender (the registry) knows it was not applied and can order its next
/// past it. A stale change is kept by nothing: again, it is refused again;
/// one of its order with another body is stale too, not a conflict.
#[test]
fn a_seat_change_older_than_the_last_is_refused_as_stale() {
    let mut w = World::seat();
    w.replays(seat("s5", SeatState::Canceled, 5));
    let stale = Refused::StaleSeat { current: 5 };
    w.refused(seat("s4", SeatState::Active, 4), stale);
    w.refused(seat("s4", SeatState::Active, 4), stale);
    w.refused(seat("s4", SeatState::PastDue, 4), stale);
    w.refused(seat("s5b", SeatState::Active, 5), stale);
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::SeatCanceled }, "the late change moved nothing");
    assert_eq!((stale.code(), stale.message()), (ErrorCode::InvalidRequest, "the ledger's seat is at order 5; a change to it has a higher one".to_string()));
    w.refused(seat("s9", SeatState::Active, 0), Refused::Invalid { what: Invalid::Order });
    w.refused(seat("s5", SeatState::Active, 5), Refused::ConflictingBody);
    let mut w = w.restarted();
    w.refused(seat("s4", SeatState::Active, 4), stale);
    w.ok(seat("s6", SeatState::Active, 6));
    assert_eq!(w.ledger.standing(T0), Standing::Ok);
    // an order far ahead of any clock (an operator's `seat` command set
    // one, before it went) is passed by the next change past it
    w.ok(seat("legacy", SeatState::Canceled, u64::from(u32::MAX) * 1_000));
    w.refused(seat("s7", SeatState::Active, 7), Refused::StaleSeat { current: u64::from(u32::MAX) * 1_000 });
    w.ok(seat("s8", SeatState::Active, u64::from(u32::MAX) * 1_000 + 1));
    assert_eq!(w.ledger.standing(T0), Standing::Ok);
}

// Zero, the overdraft, read-only

/// Decision 27, end to end.
#[test]
fn at_zero_agents_stop_and_fragments_write_until_the_overdraft() {
    let mut w = World::seat();
    w.spend("b1", 50 * USD);
    let no_credit = Refused::AgentsStopped { why: Why::NoCredit };
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::NoCredit });
    for spend in [Spend::AgentTurn, Spend::AiStep, Spend::Wake] {
        assert_eq!(w.ledger.gate(spend, T0), Err(no_credit), "{spend:?}");
    }
    assert_eq!(w.ledger.gate(Spend::Write, T0), Ok(()));
    assert_eq!(w.status().available_micros, OVERDRAFT_DEFAULT, "what may still be charged before read-only: the overdraft's room");
    // meters always record: what happened is charged
    w.spend("b2", 3 * USD / 2);
    assert_eq!((w.balance(), w.ledger.standing(T0)), (-3 * USD / 2, Standing::AgentsStopped { why: Why::NoCredit }));
    w.spend("b3", USD / 2);
    let read_only = Refused::ReadOnly { why: Why::Overdrawn };
    assert_eq!((w.balance(), w.ledger.standing(T0)), (-2 * USD, Standing::ReadOnly { why: Why::Overdrawn }), "read-only at the overdraft");
    assert_eq!(w.ledger.gate(Spend::Write, T0), Err(read_only));
    assert_eq!(w.ledger.gate(Spend::AgentTurn, T0), Err(read_only));
    assert_eq!(w.status().available_micros, 0, "read-only, nothing more");
    w.spend("b4", USD);
    // a top-up that leaves the balance at or below zero keeps it read-only
    w.ok(grant("g1", 3 * USD));
    assert_eq!((w.balance(), w.ledger.standing(T0)), (0, Standing::ReadOnly { why: Why::Overdrawn }));
    w.ok(grant("g2", 1));
    assert_eq!((w.balance(), w.ledger.standing(T0)), (1, Standing::Ok), "writable and running above zero");
}

#[test]
fn an_operator_sets_the_overdraft_and_read_only_follows() {
    let mut w = World::seat();
    w.spend("b1", 51 * USD);
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::NoCredit });
    w.replays(overdraft("o1", 0));
    assert_eq!(w.ledger.standing(T0), Standing::ReadOnly { why: Why::Overdrawn }, "no overdraft: read-only below zero");
    w.ok(overdraft("o2", 5 * USD));
    assert_eq!(w.ledger.standing(T0), Standing::AgentsStopped { why: Why::NoCredit }, "a larger overdraft lets fragments write again");
    assert_eq!(w.status().overdraft_micros, 5 * USD);
    for micros in [-1, OVERDRAFT_MAX + 1] {
        w.refused(overdraft("o3", micros), Refused::Invalid { what: Invalid::Amount });
    }
    w.refused(overdraft("o1", 3 * USD), Refused::ConflictingBody);
    w.ok(overdraft("o3", OVERDRAFT_MAX));
}

/// A debt is paid from the next month's credit, once.
#[test]
fn a_debt_is_paid_once_from_the_next_months_credit() {
    let mut w = World::seat();
    w.spend("b1", 51 * USD + USD / 2);
    assert_eq!((w.status().included_micros, w.status().purchased_micros), (0, -3 * USD / 2));
    w.now = NOVEMBER;
    let s = w.status();
    assert_eq!((s.included_micros, s.purchased_micros, s.balance_micros), (48 * USD + USD / 2, 0, 48 * USD + USD / 2));
    w.spend("b2", USD);
    w.now = T0 + 61 * DAY_MS;
    let s = w.status();
    assert_eq!((s.month.as_str(), s.included_micros, s.purchased_micros), ("2026-12", 50 * USD, 0), "December owes nothing");
}

// Caps

/// Decision 26: past its cap, a fragment stops spenders other than its owner.
#[test]
fn a_fragment_past_its_cap_stops_others_but_not_its_owner() {
    let mut w = World::seat();
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", T0), Ok(()));
    w.spend_in("b1", CAP_DEFAULT - 1, Some("todo.ann"));
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", T0), Ok(()));
    w.spend_in("b2", 1, Some("todo.ann"));
    let reached = Refused::CapReached { cap: CAP_DEFAULT, spent: CAP_DEFAULT };
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", T0), Err(reached));
    let may = |w: &World, fragment, by_owner| w.ledger.may_spend(&w.store, Spend::AgentTurn, Some(fragment), by_owner, T0);
    assert_eq!(may(&w, "todo.ann", false), Err(reached));
    assert_eq!(may(&w, "todo.ann", true), Ok(()), "the owner spends past the cap");
    assert_eq!(may(&w, "chat.ann", false), Ok(()), "another fragment has its own cap");
    // usage that happened in it is charged past it, by anyone: meters always record
    w.spend_in("b3", 1, Some("todo.ann"));
    let listed = w.status().fragments;
    assert_eq!(listed[0], FragmentSpend { fragment: "todo.ann".into(), spent_micros: CAP_DEFAULT + 1, cap_micros: CAP_DEFAULT });
    w.now = NOVEMBER;
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", NOVEMBER), Ok(()), "a new month, a new cap");
}

/// The one question the platform asks before a spend: for whom, in
/// which fragment.
#[test]
fn may_this_spend_happen_for_whom_in_which_fragment() {
    let mut w = World::seat();
    w.ok(cap("c1", "todo.ann", Some(USD)));
    w.spend_in("b1", USD, Some("todo.ann"));
    let reached = Refused::CapReached { cap: USD, spent: USD };
    let may = |w: &World, spend, fragment, by_owner| w.ledger.may_spend(&w.store, spend, fragment, by_owner, T0);
    assert_eq!(may(&w, Spend::AgentTurn, Some("todo.ann"), false), Err(reached), "a visitor's agent");
    assert_eq!(may(&w, Spend::AiStep, Some("todo.ann"), false), Err(reached), "a visitor's AI step");
    assert_eq!(may(&w, Spend::AgentTurn, Some("todo.ann"), true), Ok(()), "the owner's own agent");
    assert_eq!(may(&w, Spend::Write, Some("todo.ann"), false), Ok(()), "caps never stop writes");
    assert_eq!(may(&w, Spend::Wake, None, false), Ok(()));
    assert_eq!(may(&w, Spend::AgentTurn, Some("chat.ann"), false), Ok(()));
    w.ok(seat("s1", SeatState::Canceled, 1));
    assert_eq!(may(&w, Spend::AgentTurn, Some("chat.ann"), true), Err(Refused::AgentsStopped { why: Why::SeatCanceled }), "the payer's standing first");
    let g = World::guest();
    assert_eq!(may(&g, Spend::Write, None, true), Err(Refused::GuestPayer));
}

#[test]
fn a_cap_is_set_by_the_fragments_owner() {
    let mut w = World::seat();
    w.replays(cap("c1", "todo.ann", Some(USD)));
    w.spend_in("b1", USD, Some("todo.ann"));
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", T0), Err(Refused::CapReached { cap: USD, spent: USD }));
    w.ok(cap("c2", "todo.ann", None));
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", T0), Ok(()), "back to the default");
    w.ok(cap("c3", "site.ann", Some(0)));
    assert_eq!(w.ledger.fragment_open(&w.store, "site.ann", T0), Err(Refused::CapReached { cap: 0, spent: 0 }), "a cap of nothing: only the owner");
    w.refused(cap("c4", "", Some(USD)), Refused::Invalid { what: Invalid::Id });
    for micros in [-1, CAP_MAX + 1] {
        w.refused(cap("c4", "todo.ann", Some(micros)), Refused::Invalid { what: Invalid::Amount });
    }
    w.refused(cap("c1", "todo.ann", Some(2 * USD)), Refused::ConflictingBody);
    assert_eq!(w.ledger.fragment_open(&w.store, "a fragment", T0), Err(Refused::Invalid { what: Invalid::Id }));
}

#[test]
fn a_late_row_from_last_month_does_not_spend_this_months_cap() {
    let mut w = World::seat();
    w.now = NOVEMBER;
    let late = row("row:late", Usage::Key { key: "micro".into(), units: CAP_DEFAULT as u64 }, Some("todo.ann"), NOVEMBER - 1);
    w.ok(meter("b1", vec![late]));
    assert_eq!(w.ledger.fragment_open(&w.store, "todo.ann", NOVEMBER), Ok(()));
    assert_eq!(w.store.spent(Month::of(T0), "todo.ann"), CAP_DEFAULT, "it counts in October");
}

// Price books

/// A ledger keeps no book (decision 62): a batch is priced with the
/// configured book of the release applying it, whatever its version (#451):
/// the books are compared, never ordered, and one used before is used
/// again, as a configuration rolled back is. Each is kept once, under
/// `book:<digest>`, as it first priced a batch, for the audit; a row is
/// charged at the book of its time.
#[test]
fn a_batch_is_priced_with_the_configured_book_of_its_release() {
    let mut w = World::seat();
    let (cost_basis, dearer) = (PriceBook { margin_bp: 0, ..book() }, PriceBook { margin_bp: 10_000, ..book() });
    w.replays(Op::Book(cost_basis.clone()));
    assert_eq!(w.status().price_book, 1, "the status names the configured book's version: it is informational");
    assert_eq!(w.charged("aig:1", turn()), 4_200, "no margin: the cost basis");
    w.now += 1_000;
    w.replays(Op::Book(PriceBook { version: 0, ..dearer.clone() }));
    assert_eq!(w.status().price_book, 0, "and at a lower one");
    w.replays(Op::Book(dearer.clone()));
    assert_eq!(w.store.entry("aig:1").and_then(|e| e.charge()), Some(4_200), "charged at the book of its time");
    assert_eq!(w.charged("aig:2", turn()), 8_400, "the new book's margin");
    // rolled back, then forward again: each taken again
    w.now += 1_000;
    w.replays(Op::Book(cost_basis.clone()));
    assert_eq!(w.charged("aig:3", turn()), 4_200, "the book rolled back to");
    w.replays(Op::Book(dearer.clone()));
    assert_eq!(w.charged("aig:4", turn()), 8_400, "and forward");
    for (b, at_ms) in [(&cost_basis, T0), (&dearer, T0 + 1_000)] {
        let id = format!("book:{}", b.digest());
        let kept = CommandRecord { command: Command::SetBook(SetPriceBook { id: id.clone(), book: b.clone() }), at_ms };
        assert_eq!(w.store.command(&id), Some(kept), "kept once, as first taken");
    }
}

/// #451: operator keys' rows were added to the deployments' configurations
/// with no version raised, and every ledger kept the book it had and
/// refused their calls as unpriced. Goal: a configuration that adds one
/// key's price, and changes nothing else (its version included), prices
/// the next call on that key, across a restart and a replay; the ledger
/// keeps no book of its own. Method: the release's book changed, then its
/// batches, restarted between.
#[test]
fn a_key_the_configuration_adds_is_priced_at_the_next_call() {
    let mut configured = book();
    configured.keys.push(KeyPrice { key: "google-places".into(), micros: 35_000_000, per: 1_000 });
    let mut w = World::seat();
    let call = Usage::Key { key: "google-places".into(), units: 1 };
    let charge_call = meter("key:1", vec![row("key:1", call.clone(), None, T0)]);
    assert_eq!(w.book.price(&call), Err(PriceError::NoPrice), "the book configured before has no price for the key");
    w.ok(Op::Book(configured.clone()));
    // $35 per thousand calls, and the margin
    let charge = 52_500;
    let charged = Outcome::Metered(Metered { charged: charge, rows_new: 1, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(charge_call.clone()), charged);
    let mut w = w.restarted();
    assert!(serde_json::to_value(&w.ledger).unwrap().get("book").is_none(), "the head keeps no book");
    w.replays(Op::Book(configured.clone()));
    assert_eq!(w.ok(charge_call.clone()), charged, "a batch sent again answers as it did");
    let mut w = w.restarted();
    let (ledger, store) = (w.ledger.clone(), w.store.clone());
    w.ok(Op::Book(configured.clone()));
    assert_eq!((&w.ledger, &w.store), (&ledger, &store), "the same configuration again changes nothing");
    let again = w.ok(meter("key:1-again", vec![row("key:1", call, None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "and the row in another batch is charged once");
}

/// Bug 2: a step's charge is its row under the step's key, whatever
/// attempt made the call (decision 62): a retry or a replay that sends it
/// again, in a batch of its own, is charged nothing more.
#[test]
fn a_retried_step_after_a_storage_failure_never_pays_twice() {
    let mut w = World::seat();
    let step = "step:avatar-lab.paul@1/run/7/step/2";
    assert_eq!(w.charged(step, flash(10_000, 1_000)), 3_150);
    // storing the image failed; the job retries the step, which finds what it kept
    let again = w.ok(meter("frag:avatar-lab.paul@1:2", vec![row(step, flash(10_000, 1_000), None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }));
    assert_eq!(w.ledger.totals().charged, 3_150, "one paid call, metered once");
}

// Meters

#[test]
fn a_batch_charges_each_row_once_whatever_batch_carries_it() {
    let mut w = World::seat();
    let rows = vec![
        row("aig:1", turn(), Some("chat.ann"), T0),
        row("aig:2", Usage::Neurons { milli: 1_000_000 }, Some("chat.ann"), T0),
        row("awake:1", Usage::Awake { instance: "2vcpu-6gib".into(), ms: 3_600_000 }, None, T0),
        row("store:1", Usage::Storage { class: StorageClass::Sqlite, byte_hours: 720_000_000_000 }, Some("todo.ann"), T0),
        row("req:1", Usage::Requests { count: 1_000_000 }, Some("todo.ann"), T0),
        row("dw:1", Usage::DynamicWorkers { count: 1 }, Some("todo.ann"), T0),
        row("shot:1", Usage::Browser { ms: 3_000 }, Some("site.ann"), T0),
        row("img:1", Usage::Images { count: 1 }, Some("site.ann"), T0),
    ];
    let charged = TURN + 17_325 + 96_336 + 300_000 + 675_000 + 3_000 + 113 + 750;
    let answer = w.replays(meter("b1", rows.clone()));
    assert_eq!(answer, Outcome::Metered(Metered { charged, rows_new: 8, rows_before: 0, refused: vec![] }));
    assert_eq!(w.balance(), 50 * USD - charged);
    let spends: Vec<(String, i64)> = w.status().fragments.into_iter().map(|f| (f.fragment, f.spent_micros)).collect();
    assert_eq!(spends, [("todo.ann".to_string(), 978_000), ("chat.ann".to_string(), TURN + 17_325), ("site.ann".to_string(), 863)]);
    // the same rows in another batch: charged before, not again
    let again = w.ok(meter("b2", rows.clone()));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 8, refused: vec![] }));
    w.refused(meter("b1", rows[..7].to_vec()), Refused::ConflictingBody);
}

/// A source flushes a batch and crashes before it marks it flushed: it
/// sends the batch again, after its ledger restarted too.
#[test]
fn a_batch_retried_after_a_crash_between_meter_and_flush_is_applied_once() {
    let mut w = World::seat();
    let batch = meter("computer:0a1b:flush:41", vec![row("awake:0a1b:41", Usage::Awake { instance: "2vcpu-6gib".into(), ms: 600_000 }, None, T0)]);
    let first = w.ok(batch.clone());
    let mut w = w.restarted();
    assert_eq!(w.ok(batch), first, "the retried batch answers as it did");
    assert_eq!(w.ledger.totals().charged, 16_056, "ten minutes awake, charged once");
}

#[test]
fn every_refusal_of_a_batch() {
    let mut w = World::seat();
    let one = || vec![row("row:1", Usage::Requests { count: 1 }, None, T0)];
    w.refused(meter("", one()), Refused::Invalid { what: Invalid::Id });
    w.refused(meter("b1", vec![]), Refused::Invalid { what: Invalid::Rows });
    let many = (0..=BATCH_ROWS_MAX).map(|i| row(&format!("row:{i}"), Usage::Requests { count: 1 }, None, T0)).collect();
    w.refused(meter("b1", many), Refused::Invalid { what: Invalid::Rows });
    let mut g = World::guest();
    g.refused(meter("b1", one()), Refused::GuestPayer);
    let full = (0..BATCH_ROWS_MAX).map(|i| row(&format!("row:{i}"), Usage::Requests { count: 1 }, None, T0)).collect();
    assert!(matches!(w.ok(meter("b1", full)), Outcome::Metered(Metered { rows_new: 1_000, .. })), "BATCH_ROWS_MAX rows fit");
}

/// A row is refused alone: the batch's other rows are charged, and the
/// refused one may come again in a later batch.
#[test]
fn every_refusal_of_a_row() {
    let mut w = World::seat();
    // a reservation the releases before decision 62 kept
    w.store.put_entry("r1", serde_json::from_value(old_reservation("r1", T0)).expect("a record from before reads"));
    w.now = T0 + REF_KEEP_MS + DAY_MS;
    let now = w.now;
    let req = || Usage::Requests { count: 1 };
    let rows = vec![
        row("", req(), None, now),
        row("row:1", req(), Some(""), now),
        row("row:2", flash(price::QUANTITY_MAX + 1, 0), None, now),
        row("row:3", req(), None, now + CLOCK_SKEW_MS + 1),
        row("row:4", req(), None, -1),
        row("row:5", req(), None, now - REF_KEEP_MS - 1),
        row("row:6", Usage::Awake { instance: "8vcpu".into(), ms: 1 }, None, now),
        row("row:7", Usage::DynamicWorkers { count: price::QUANTITY_MAX }, None, now),
        row("r1", req(), None, now),
        row("row:8", req(), None, now),
        row("row:8", req(), None, now),
        row("row:8", Usage::Requests { count: 2 }, None, now),
        row("row:9", req(), None, now + CLOCK_SKEW_MS),
    ];
    let Outcome::Metered(m) = w.replays(meter("b1", rows)) else { panic!("a batch answers Metered") };
    let why: Vec<(u32, Refused)> = m.refused.iter().map(|r| (r.index, r.why)).collect();
    assert_eq!(
        why,
        [
            (0, Refused::Invalid { what: Invalid::Id }),
            (1, Refused::Invalid { what: Invalid::Id }),
            (2, Refused::Invalid { what: Invalid::Usage(UsageFault::Quantity) }),
            (3, Refused::Invalid { what: Invalid::Time }),
            (4, Refused::Invalid { what: Invalid::Time }),
            (5, Refused::Stale),
            (6, Refused::NoPrice),
            (7, Refused::TooLarge),
            (8, Refused::ConflictingBody),
            (11, Refused::ConflictingBody),
        ]
    );
    assert_eq!((m.rows_new, m.rows_before, m.charged), (2, 1, 2), "row:8 once, its twin as before, and row:9");
    // once the book prices it, the refused row comes again
    let mut priced = book();
    priced.instances.push(price::InstancePrice { instance: "8vcpu".into(), hour: 3_600_000 });
    w.ok(Op::Book(priced));
    let again = w.ok(meter("b2", vec![row("row:6", Usage::Awake { instance: "8vcpu".into(), ms: 1 }, None, now)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 2, rows_new: 1, rows_before: 0, refused: vec![] }));
}

/// Decision 25: a $200 seat's always-on computer is not metered for its
/// awake time; the row is still kept.
#[test]
fn an_always_on_seats_awake_time_is_not_charged() {
    let mut w = World::seat();
    w.ok(plan("p2", Plan::SeatAlwaysOn));
    let hour = || Usage::Awake { instance: "2vcpu-6gib".into(), ms: 3_600_000 };
    let answer = w.ok(meter("b1", vec![row("awake:1", hour(), None, T0)]));
    assert_eq!(answer, Outcome::Metered(Metered { charged: 0, rows_new: 1, rows_before: 0, refused: vec![] }));
    let Some(Entry::Row { priced, charge, .. }) = w.store.entry("awake:1") else { panic!("the row is kept") };
    assert_eq!((priced.charge, charge), (96_336, 0));
    w.ok(plan("p3", Plan::Seat));
    let answer = w.ok(meter("b2", vec![row("awake:2", hour(), None, T0)]));
    assert_eq!(answer, Outcome::Metered(Metered { charged: 96_336, rows_new: 1, rows_before: 0, refused: vec![] }));
}

// Sweeps

#[test]
fn a_sweep_forgets_what_is_past_the_ledgers_memory() {
    let mut w = World::seat();
    w.ok(meter("b1", vec![row("row:1", Usage::Requests { count: 1 }, Some("todo.ann"), T0)]));
    w.now = T0 + REF_KEEP_MS;
    assert_eq!(w.ok(Op::Sweep), Outcome::Swept(Swept { forgotten: 0 }), "kept for REF_KEEP_MS");
    assert_eq!(w.ledger.next_sweep_ms(w.now), w.now + SWEEP_EVERY_MS, "a sweep a day");
    w.now += 1;
    assert_eq!(w.ok(Op::Sweep), Outcome::Swept(Swept { forgotten: 2 }), "a reference and a batch");
    assert_eq!(w.store.spent(Month::of(T0), "todo.ann"), 1, "October's spend is kept while part of October is remembered");
    // a row from then, sent again, is refused as stale: it is never charged twice
    let Outcome::Metered(m) = w.ok(meter("b1", vec![row("row:1", Usage::Requests { count: 1 }, Some("todo.ann"), T0)])) else { panic!() };
    assert_eq!(m.refused, [RowRefused { index: 0, why: Refused::Stale }]);
    assert_eq!(w.ledger.totals().charged, 1);
    w.now = NOVEMBER + REF_KEEP_MS;
    w.ok(Op::Sweep);
    assert_eq!(w.store.spent(Month::of(T0), "todo.ann"), 0, "and forgotten once all of October is");
}

// Reads

#[test]
fn a_read_sees_its_own_month_and_changes_nothing() {
    let mut w = World::seat();
    w.spend("b1", 10 * USD);
    let ledger = w.ledger.clone();
    let s = w.ledger.status(&w.store, NOVEMBER, &w.book);
    assert_eq!((s.month.as_str(), s.included_micros, s.balance_micros), ("2026-11", 50 * USD, 50 * USD));
    assert_eq!(w.ledger.standing(NOVEMBER), Standing::Ok);
    assert_eq!(w.ledger, ledger, "a read rolls a copy, never the ledger");
    assert_eq!(w.status().included_micros, 40 * USD);
}

// Restarts

/// Every mutation, written and read back at every point of a script that
/// crosses a month: the restarted state answers the rest as the one that
/// never stopped, and ends the same.
#[test]
fn every_mutation_answers_the_same_after_a_restart() {
    let script: Vec<(i64, Op)> = vec![
        (0, plan("p1", Plan::Seat)),
        (0, grant("g1", 5 * USD)),
        (0, seat("s1", SeatState::PastDue, 1)),
        (0, overdraft("o1", USD)),
        (0, cap("c1", "todo.ann", Some(USD))),
        (0, set_book(4_000)),
        (60_000, meter("aig:1", vec![row("aig:1", flash(50_000, 2_000), Some("todo.ann"), T0 + 60_000)])),
        (120_000, meter("aig:2", vec![row("aig:2", turn(), None, T0 + 120_000)])),
        (180_000, meter("b1", vec![row("row:1", Usage::Requests { count: 10_000 }, Some("todo.ann"), T0)])),
        (180_000, grant("g1", 5 * USD)),
        (180_000, meter("b1", vec![row("row:1", Usage::Requests { count: 10_000 }, Some("todo.ann"), T0)])),
        (240_000, meter("aig:2", vec![row("aig:2", turn(), None, T0 + 120_000)])),
        (31 * DAY_MS, seat("s2", SeatState::Active, 2)),
        (31 * DAY_MS, Op::Sweep),
        (31 * DAY_MS + 1, plan("p2", Plan::SeatAlwaysOn)),
        (31 * DAY_MS + 2, meter("b2", vec![row("awake:1", Usage::Awake { instance: "2vcpu-6gib".into(), ms: 60_000 }, None, T0 + 31 * DAY_MS)])),
    ];
    let run = |w: &mut World, part: &[(i64, Op)]| -> Vec<Result<Outcome, Refused>> {
        part.iter()
            .map(|(at, op)| {
                w.now = T0 + at;
                w.apply(op)
            })
            .collect()
    };
    let mut straight = World::guest();
    let expected = run(&mut straight, &script);
    assert!(expected.iter().all(Result::is_ok), "the script is all valid: {expected:?}");
    for k in 0..=script.len() {
        let mut w = World::guest();
        run(&mut w, &script[..k]);
        let mut restarted = w.restarted();
        assert_eq!(run(&mut restarted, &script[k..]), expected[k..], "restarted after {k} steps");
        assert_eq!((&restarted.ledger, &restarted.store), (&straight.ledger, &straight.store));
    }
}

// Refusals as the platform answers them

#[test]
fn money_refusals_are_402_with_a_reason_people_can_read() {
    let capped = Refused::CapReached { cap: CAP_DEFAULT, spent: CAP_DEFAULT };
    assert_eq!(capped.message(), "this fragment spent $5.00 of its $5.00 cap this month: only its owner spends in it until next month");
    for r in [Refused::AgentsStopped { why: Why::NoCredit }, Refused::ReadOnly { why: Why::Overdrawn }, capped] {
        assert_eq!(r.code(), ErrorCode::BudgetUsedUp, "{r:?}");
    }
    assert!(Refused::AgentsStopped { why: Why::NoCredit }.message().contains("credit is used up"));
    assert_eq!(Refused::ConflictingBody.code(), ErrorCode::ConflictingBody);
    assert_eq!(Refused::GuestPayer.code(), ErrorCode::Forbidden);
    assert_eq!(Refused::GuestCreates.code(), ErrorCode::Forbidden);
    assert_eq!(serde_json::to_value(Refused::GuestCreates).unwrap(), serde_json::json!({ "refused": "guest_creates" }));
    // a wiped person's ledger refuses as one that is not there, and says why
    assert_eq!(Refused::Wiped.code(), ErrorCode::NotFound);
    assert_eq!(serde_json::to_value(Refused::Wiped).unwrap(), serde_json::json!({ "refused": "wiped" }));
    assert!(Refused::Wiped.message().contains("wiped"));
    assert_eq!(Refused::TooLarge.code(), ErrorCode::TooLarge);
    assert_eq!(Refused::Invalid { what: Invalid::Rows }.code(), ErrorCode::InvalidRequest);
    assert_eq!(serde_json::to_value(capped).unwrap(), serde_json::json!({ "refused": "cap_reached", "cap": CAP_DEFAULT, "spent": CAP_DEFAULT }));
    assert_eq!(
        serde_json::to_value(Refused::Invalid { what: Invalid::Usage(UsageFault::Name) }).unwrap(),
        serde_json::json!({ "refused": "invalid", "what": { "usage": "name" } })
    );
}

/// The inner routes' bodies are these types; they refuse fields they do
/// not name.
#[test]
fn route_bodies_refuse_unknown_fields() {
    let row = serde_json::json!({ "ref": "aig:1", "usage": { "kind": "requests", "count": 1 }, "fragment": null, "agent": null, "computer": null, "at_ms": T0 });
    let body = serde_json::json!({ "batch": "aig:1", "rows": [row] });
    let m: Meter = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(m.rows[0].reference, "aig:1");
    let mut extra = body;
    extra["rows"][0]["amount"] = serde_json::json!(5);
    assert!(serde_json::from_value::<Meter>(extra).is_err());
    assert!(serde_json::from_value::<Meter>(serde_json::json!({ "batch": "b", "rows": [], "cost": 3 })).is_err());
}

// What the releases before decision 62 kept

/// A reservation's entry as those releases wrote it (settled, here).
fn old_reservation(reference: &str, at_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "entry": "reservation",
        "reserve": { "ref": reference, "spend": "agent_turn", "worst": { "kind": "requests", "count": 1 }, "fragment": "todo.ann", "agent": null, "capped": false },
        "amount": TURN,
        "at_ms": at_ms,
        "end": { "end": "settled", "usage": null, "priced": null, "charge": TURN, "basis": "reservation" }
    })
}

/// Goal: a head those releases kept, holding reservations, loads with its
/// holds let go, never charged (what they held was never drawn), and its
/// balance and every other part as they were; it is written back without
/// them. Valid, replayed (read twice: the same), and restarted. Method: a
/// head written now, the holds of the old shape put back into its JSON.
#[test]
fn a_head_kept_with_holds_loads_without_them_and_charges_nothing() {
    let mut w = World::seat();
    w.spend_in("b1", 3 * USD, Some("todo.ann"));
    let mut old = serde_json::to_value(&w.ledger).unwrap();
    old["holds"] = serde_json::json!({
        "aig:held": { "amount": TURN, "fragment": "todo.ann", "at_ms": T0 },
        "step:x@1/run/1/attempt/1/step/0": { "amount": 25 * USD, "fragment": null, "at_ms": T0 - 7 * 3_600_000 }
    });
    let loaded: Ledger = serde_json::from_value(old.clone()).expect("a head with holds loads");
    loaded.assert_valid();
    assert_eq!(loaded, w.ledger, "the holds are let go; nothing else moves");
    assert_eq!(serde_json::from_value::<Ledger>(old).unwrap(), loaded, "read again, the same");
    let s = loaded.status(&w.store, T0, &w.book);
    assert_eq!((s.balance_micros, s.reserved_micros, s.available_micros), (47 * USD, 0, 47 * USD + OVERDRAFT_DEFAULT), "nothing held, nothing charged");
    assert_eq!(loaded.fragment_open(&w.store, "todo.ann", T0), Ok(()), "a hold no more counts toward a cap: $3 of $5");
    w.ledger = loaded;
    let mut w = w.restarted();
    w.now += 7 * 3_600_000;
    assert_eq!(w.ok(Op::Sweep), Outcome::Swept(Swept { forgotten: 0 }));
    assert_eq!((w.balance(), w.ledger.totals().charged), (47 * USD, 3 * USD), "no sweep charges a hold that was");
    let written = serde_json::to_value(&w.ledger).unwrap();
    assert!(written.get("holds").is_none(), "written back without them: {written}");
}

/// Goal: a head those releases kept names the book it charged with: it
/// loads, the book let go, and its next batch is priced with the
/// configured book of the release applying it, not that one. Method: a
/// head written now, a dearer book of the old shape put back into it.
#[test]
fn a_head_kept_with_its_book_loads_and_charges_with_the_configured_one() {
    let mut w = World::seat();
    let mut old = serde_json::to_value(&w.ledger).unwrap();
    old["book"] = serde_json::to_value(PriceBook { margin_bp: 10_000, version: 7, ..book() }).unwrap();
    let loaded: Ledger = serde_json::from_value(old).expect("a head with its book loads");
    assert_eq!(loaded, w.ledger, "the book is let go; nothing else moves");
    w.ledger = loaded;
    let mut w = w.restarted();
    assert_eq!(w.charged("aig:1", turn()), TURN, "priced with the configured book (a 50% margin), not the head's (100%)");
    assert_eq!(w.status().price_book, 1, "and the status names the configured one");
}

/// Goal: an entry those releases kept (a reservation, held, settled or
/// released) reads as one from before, kept as it was written: never
/// charged again, a row under its reference another body, forgotten by
/// its own time as any is. Valid and invalid shapes. Method: each as it
/// was written.
#[test]
fn an_old_reservation_entry_reads_as_one_from_before() {
    let settled = old_reservation("aig:old", T0);
    let mut held = settled.clone();
    held["end"] = serde_json::Value::Null;
    let mut released = settled.clone();
    released["end"] = serde_json::json!({ "end": "released" });
    for old in [settled.clone(), held.clone(), released] {
        let read = serde_json::from_value::<Entry>(old.clone()).unwrap();
        assert_eq!(read, Entry::Before(old.clone()), "{old}");
        assert_eq!((read.charge(), read.at_ms()), (None, T0));
        assert_eq!(serde_json::to_value(&read).unwrap(), old, "written back as it was");
    }
    assert!(serde_json::from_value::<Entry>(serde_json::json!({ "row": {} })).is_err(), "an entry names its kind");
    let mut timeless = held;
    timeless.as_object_mut().unwrap().remove("at_ms");
    assert!(serde_json::from_value::<Entry>(timeless).is_err(), "a reservation from before has its time");
    let mut w = World::seat();
    w.store.put_entry("aig:old", Entry::Before(settled));
    let Outcome::Metered(m) = w.ok(meter("b1", vec![row("aig:old", turn(), None, T0)])) else { panic!() };
    assert_eq!((m.charged, m.refused.clone()), (0, vec![RowRefused { index: 0, why: Refused::ConflictingBody }]));
    w.now = T0 + REF_KEEP_MS;
    w.ok(Op::Sweep);
    assert!(w.store.entry("aig:old").is_some(), "kept for its time: a replay stays refused");
    w.now += 1;
    w.ok(Op::Sweep);
    assert_eq!(w.store.entry("aig:old"), None, "forgotten as any is");
}
