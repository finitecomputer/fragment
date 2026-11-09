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
use crate::price::{KeyCall, KeyPrice, PriceBook, StorageClass, UsageFault, USD};

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

/// FIN-443: every ElevenLabs call was charged as a minute of music, so
/// short speech clips cost people 10 to 20 times what they cost. Goal: a
/// deployment's book (its keys' prices from its catalog) charges a speech
/// call by its text's characters and a music call by the minute, each
/// once, across a replay and a restart, and one ElevenLabs refused
/// nothing; a release whose book has no
/// speech price (one rolled back to) charges its rows nothing. Method:
/// the swap's rows as it meters them (`price::key_usage`), sent again.
#[test]
fn a_speech_call_is_charged_by_its_characters_and_once() {
    let eleven = serde_json::json!([{ "name": "elevenlabs", "kind": "operator", "hosts": ["api.elevenlabs.io"], "placements": [{ "header": "xi-api-key" }], "env": ["ELEVENLABS_API_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&eleven.to_string()).unwrap().key_prices());
    let reply = serde_json::to_vec(&serde_json::json!({ "text": "a".repeat(200), "model_id": "eleven_multilingual_v2" })).unwrap();
    let eleven_call = |path, body, status| crate::price::key_usage("elevenlabs", &KeyCall { method: "POST", host: "api.elevenlabs.io", path, body, status });
    let spoken = eleven_call("/v1/text-to-speech/voice", &reply, 200);
    let music = eleven_call("/v1/music", br#"{"prompt":"a jingle","music_length_ms":15000}"#, 200);
    assert_eq!((&spoken, &music), (&Usage::Key { key: "elevenlabs:speech".into(), units: 200 }, &Usage::Key { key: "elevenlabs".into(), units: 1 }));
    // one ElevenLabs refused (a rate limit): its row kept, charged nothing
    let refused = eleven_call("/v1/text-to-speech/voice", &reply, 429);
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let batch = meter("key:c-1:a1", vec![row("key:c-1:a1", spoken.clone(), None, T0), row("key:c-1:a2", music, None, T0), row("key:c-1:a3", refused, None, T0)]);
    // 200 characters at $0.08 per 1,000, a minute at $0.15, each with the margin, and nothing
    let charged = Outcome::Metered(Metered { charged: 24_000 + 225_000, rows_new: 3, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    assert_eq!(w.balance(), before - 249_000);
    let mut w = w.restarted();
    w.replays(Op::Book(configured.clone()));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:a1-again", vec![row("key:c-1:a1", spoken.clone(), None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 249_000);
    // a release before the speech price: its row refused as unpriced, charging nothing
    let older = PriceBook { keys: configured.keys.iter().filter(|k| k.key != "elevenlabs:speech").cloned().collect(), ..configured };
    w.ok(Op::Book(older));
    let Outcome::Metered(m) = w.ok(meter("key:c-1:b1", vec![row("key:c-1:b1", spoken, None, T0)])) else { panic!("a batch answers Metered") };
    assert_eq!((m.charged, m.rows_new, m.refused.iter().map(|r| r.why).collect::<Vec<_>>()), (0, 0, vec![Refused::NoPrice]));
    assert_eq!(w.balance(), before - 249_000);
}

/// FIN-445: fal's key charged each call as an image, so the SDK's queue
/// polls, status and result made one image cost several. Goal: a
/// deployment's book charges a generation by its pixels and the calls
/// around it nothing, once across a replay and a restart; one fal refused
/// nothing; a row an older release queued (one image) as before; a release
/// whose book has no image price charges this one's rows nothing. Method:
/// the swap's rows as it meters them (`price::key_usage`), sent again.
#[test]
fn a_fal_image_is_charged_by_its_pixels_and_once() {
    let fal = serde_json::json!([{ "name": "fal", "kind": "operator", "hosts": ["queue.fal.run", "fal.run", "rest.fal.ai"], "placements": [{ "header": "authorization", "format": "Key {}" }], "env": ["FAL_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&fal.to_string()).unwrap().key_prices());
    let call = |method, host, path, body, status| crate::price::key_usage("fal", &KeyCall { method, host, path, body, status });
    let ask = br#"{"prompt":"a lighthouse at dusk","image_size":"landscape_16_9"}"#;
    // a synchronous run is charged; a queued submit, its poll and result nothing
    let made = call("POST", "fal.run", "/fal-ai/flux-2/klein/9b", ask, 200);
    let polled = call("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/status", b"", 200);
    let result = call("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e", b"", 200);
    let refused = call("POST", "fal.run", "/fal-ai/flux-2/klein/9b", ask, 422);
    assert_eq!(call("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b", ask, 200), Usage::Key { key: "fal".into(), units: 0 }, "a queued submit");
    assert_eq!(made, Usage::Key { key: "fal:image".into(), units: 1024 * 576 });
    assert_eq!((&polled, &result, &refused), (&Usage::Key { key: "fal".into(), units: 0 }, &Usage::Key { key: "fal".into(), units: 0 }, &Usage::Key { key: "fal:image".into(), units: 0 }));
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let rows = vec![row("key:c-1:f1", made.clone(), None, T0), row("key:c-1:f2", polled, None, T0), row("key:c-1:f3", result, None, T0), row("key:c-1:f4", refused, None, T0)];
    let batch = meter("key:c-1:f1", rows);
    // 589,824 pixels at $0.006 per 1,048,576, with the margin, and nothing for the rest
    let charged = Outcome::Metered(Metered { charged: 5_063, rows_new: 4, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    assert_eq!(w.balance(), before - 5_063);
    let mut w = w.restarted();
    w.replays(Op::Book(configured.clone()));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:f1-again", vec![row("key:c-1:f1", made.clone(), None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    // a row an older release queued: one call, one image, as before
    let queued = w.ok(meter("key:c-1:old", vec![row("key:c-1:old", Usage::Key { key: "fal".into(), units: 1 }, None, T0)]));
    assert_eq!(queued, Outcome::Metered(Metered { charged: 9_000, rows_new: 1, rows_before: 0, refused: vec![] }));
    assert_eq!(w.balance(), before - 5_063 - 9_000);
    // a release before the image price: its row refused as unpriced, charging nothing
    let older = PriceBook { keys: configured.keys.iter().filter(|k| k.key != "fal:image").cloned().collect(), ..configured };
    w.ok(Op::Book(older));
    let Outcome::Metered(m) = w.ok(meter("key:c-1:g1", vec![row("key:c-1:g1", made, None, T0)])) else { panic!("a batch answers Metered") };
    assert_eq!((m.charged, m.rows_new, m.refused.iter().map(|r| r.why).collect::<Vec<_>>()), (0, 0, vec![Refused::NoPrice]));
    assert_eq!(w.balance(), before - 5_063 - 9_000);
}

/// FIN-445: xAI's key charged every call as an X search ($0.18). Goal: a
/// deployment's book charges an X search as before and an image one image,
/// a chat (its tokens not read) nothing, each once across a replay and a
/// restart; one xAI refused nothing; a release whose book has no image
/// price charges its rows nothing. Method: the
/// swap's rows as it meters them (`price::key_usage`), sent again.
#[test]
fn an_xai_call_is_charged_as_what_it_is_and_once() {
    let xai = serde_json::json!([{ "name": "xai", "kind": "operator", "hosts": ["api.x.ai"], "placements": [{ "header": "authorization", "format": "Bearer {}" }], "env": ["XAI_API_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&xai.to_string()).unwrap().key_prices());
    let call = |path, body, status| crate::price::key_usage("xai", &KeyCall { method: "POST", host: "api.x.ai", path, body, status });
    let searched = call("/v1/responses", br#"{"model":"grok-4.3","input":"news","tools":[{"type":"x_search"}]}"#, 200);
    let chatted = call("/v1/chat/completions", br#"{"model":"grok-4.3","messages":[{"role":"user","content":"hi"}]}"#, 200);
    let drawn = call("/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat","n":2}"#, 200);
    let refused = call("/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat"}"#, 429);
    assert_eq!((&searched, &chatted, &drawn), (&Usage::Key { key: "xai".into(), units: 1 }, &Usage::Key { key: "xai".into(), units: 0 }, &Usage::Key { key: "xai:image".into(), units: 1 }));
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let rows = vec![row("key:c-1:x1", searched, None, T0), row("key:c-1:x2", chatted, None, T0), row("key:c-1:x3", drawn.clone(), None, T0), row("key:c-1:x4", refused, None, T0)];
    let batch = meter("key:c-1:x1", rows);
    // an X search at $0.12 and an image at $0.02, each with the margin; a chat and a refusal nothing
    let charged = Outcome::Metered(Metered { charged: 180_000 + 30_000, rows_new: 4, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    assert_eq!(w.balance(), before - 210_000);
    let mut w = w.restarted();
    w.replays(Op::Book(configured.clone()));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:x3-again", vec![row("key:c-1:x3", drawn.clone(), None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 210_000);
    // a release before the image price: its row refused as unpriced, charging nothing
    let older = PriceBook { keys: configured.keys.iter().filter(|k| k.key != "xai:image").cloned().collect(), ..configured };
    w.ok(Op::Book(older));
    let Outcome::Metered(m) = w.ok(meter("key:c-1:y1", vec![row("key:c-1:y1", drawn, None, T0)])) else { panic!("a batch answers Metered") };
    assert_eq!((m.charged, m.rows_new, m.refused.iter().map(|r| r.why).collect::<Vec<_>>()), (0, 0, vec![Refused::NoPrice]));
    assert_eq!(w.balance(), before - 210_000);
}

/// FIN-445: Google Places' key charged every call as an Enterprise text
/// search. Goal: a deployment's book charges a photo its own list and a
/// search or a place's details (their tier their field mask's) nothing,
/// once across a replay and a restart; one Google refused nothing.
/// Method: the swap's rows as it meters them (`price::key_usage`), sent
/// again.
#[test]
fn a_places_photo_is_charged_its_own_price_and_once() {
    let places = serde_json::json!([{ "name": "google-places", "kind": "operator", "hosts": ["places.googleapis.com"], "placements": [{ "header": "x-goog-api-key" }, { "query": "key" }], "env": ["GOOGLE_PLACES_API_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&places.to_string()).unwrap().key_prices());
    let call = |method, path, status| crate::price::key_usage("google-places", &KeyCall { method, host: "places.googleapis.com", path, body: b"", status });
    let searched = call("POST", "/v1/places:searchText", 200);
    let detailed = call("GET", "/v1/places/ChIJ", 200);
    let photo = call("GET", "/v1/places/ChIJ/photos/AUc7/media", 200);
    let redirected = call("GET", "/v1/places/ChIJ/photos/AUc7/media", 302);
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let rows = vec![row("key:c-1:p1", searched, None, T0), row("key:c-1:p2", detailed, None, T0), row("key:c-1:p3", photo.clone(), None, T0), row("key:c-1:p4", redirected, None, T0)];
    let batch = meter("key:c-1:p1", rows);
    // a photo at $0.007 with the margin; a search, a detail and a redirect nothing
    let charged = Outcome::Metered(Metered { charged: 10_500, rows_new: 4, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    let mut w = w.restarted();
    w.replays(Op::Book(configured));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:p3-again", vec![row("key:c-1:p3", photo, None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 10_500);
}

/// FIN-445: Firecrawl's key charged every call as a search (2 credits),
/// its status polls too. Goal: a deployment's book charges a scrape its
/// credit, and a search (its results not counted) and a poll nothing, once
/// across a replay and a restart. Method: the swap's rows as it meters them, sent again.
#[test]
fn a_firecrawl_call_is_charged_its_credits_and_once() {
    let firecrawl = serde_json::json!([{ "name": "firecrawl", "kind": "operator", "hosts": ["api.firecrawl.dev"], "placements": [{ "header": "authorization", "format": "Bearer {}" }], "env": ["FIRECRAWL_API_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&firecrawl.to_string()).unwrap().key_prices());
    let call = |method, path, body, status| crate::price::key_usage("firecrawl", &KeyCall { method, host: "api.firecrawl.dev", path, body, status });
    let scraped = call("POST", "/v2/scrape", br#"{"url":"https://example.com/","formats":["markdown","html"]}"#, 200);
    let searched = call("POST", "/v2/search", br#"{"query":"q","limit":5}"#, 200);
    let polled = call("GET", "/v2/crawl/0b1e", b"", 200);
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let batch = meter("key:c-1:w1", vec![row("key:c-1:w1", scraped.clone(), None, T0), row("key:c-1:w2", searched, None, T0), row("key:c-1:w3", polled, None, T0)]);
    // a scrape's credit at $0.005 with the margin; a search (its results not counted) and a poll nothing
    let charged = Outcome::Metered(Metered { charged: 7_500, rows_new: 3, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    let mut w = w.restarted();
    w.replays(Op::Book(configured));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:w1-again", vec![row("key:c-1:w1", scraped, None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 7_500);
}

/// FIN-445: Browser Use's key charged a session's start, each status read
/// and its stop each as five minutes. Goal: a deployment's book charges
/// the start alone, once across a replay and a restart. Method: Hermes'
/// browser provider's calls as the swap meters them, sent again.
#[test]
fn a_browser_session_is_charged_once_at_its_start() {
    let browser = serde_json::json!([{ "name": "browser-use", "kind": "operator", "hosts": ["api.browser-use.com"], "placements": [{ "header": "x-browser-use-api-key" }], "env": ["BROWSER_USE_API_KEY"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&browser.to_string()).unwrap().key_prices());
    let call = |method, path, status| crate::price::key_usage("browser-use", &KeyCall { method, host: "api.browser-use.com", path, body: b"{}", status });
    let started = call("POST", "/api/v3/browsers", 201);
    let read = call("GET", "/api/v3/browsers/b-1", 200);
    let stopped = call("PATCH", "/api/v3/browsers/b-1", 200);
    let continued = crate::price::key_usage("browser-use", &KeyCall { method: "POST", host: "api.browser-use.com", path: "/api/v4/runs", body: br#"{"task":"t","sessionId":"b-1"}"#, status: 201 });
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let batch = meter("key:c-1:b1", vec![row("key:c-1:b1", started.clone(), None, T0), row("key:c-1:b2", read, None, T0), row("key:c-1:b3", stopped, None, T0), row("key:c-1:b4", continued, None, T0)]);
    // five minutes at $0.02 an hour, with the margin, and nothing for the
    // rest: a read, a stop, a task on the session it has
    let charged = Outcome::Metered(Metered { charged: 2_500, rows_new: 4, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    let mut w = w.restarted();
    w.replays(Op::Book(configured));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:b1-again", vec![row("key:c-1:b1", started, None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 2_500);
}

/// FIN-445: X's key charged every call as a 10-post search. Goal: a
/// deployment's book charges a search ten posts as before, and a lookup
/// (its returned resources not counted) nothing, once across a replay and
/// a restart. Method: the x-api-finite helper's calls as the swap meters
/// them, sent again.
#[test]
fn an_x_search_is_charged_and_once_and_a_lookup_nothing() {
    let x = serde_json::json!([{ "name": "x", "kind": "operator", "hosts": ["api.x.com", "api.twitter.com"], "placements": [{ "header": "authorization", "format": "Bearer {}" }], "env": ["X_API_BEARER_TOKEN"] }]);
    let mut configured = book();
    configured.keys.extend(crate::catalog::Catalog::parse(&x.to_string()).unwrap().key_prices());
    let call = |path| crate::price::key_usage("x", &KeyCall { method: "GET", host: "api.x.com", path, body: b"", status: 200 });
    let looked = call("/2/tweets");
    let user = call("/2/users/by/username/jack");
    let searched = call("/2/tweets/search/recent");
    let mut w = World::seat();
    w.ok(Op::Book(configured.clone()));
    let before = w.balance();
    let batch = meter("key:c-1:t1", vec![row("key:c-1:t1", looked, None, T0), row("key:c-1:t2", user, None, T0), row("key:c-1:t3", searched.clone(), None, T0)]);
    // a search of ten posts with the margin; two lookups nothing
    let charged = Outcome::Metered(Metered { charged: 75_000, rows_new: 3, rows_before: 0, refused: vec![] });
    assert_eq!(w.replays(batch.clone()), charged);
    let mut w = w.restarted();
    w.replays(Op::Book(configured));
    assert_eq!(w.ok(batch), charged, "a batch sent again answers as it did");
    let again = w.ok(meter("key:c-1:t3-again", vec![row("key:c-1:t3", searched, None, T0)]));
    assert_eq!(again, Outcome::Metered(Metered { charged: 0, rows_new: 0, rows_before: 1, refused: vec![] }), "the row in another batch is charged once");
    assert_eq!(w.balance(), before - 75_000);
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
/// released) reads as one from before, by its time: never charged again,
/// a row under its reference another body, forgotten by its own time as
/// any is. Valid and invalid shapes, each read from its stored text.
/// Method: each as it was written.
#[test]
fn an_old_reservation_entry_reads_as_one_from_before() {
    let read = |v: &serde_json::Value| serde_json::from_slice::<Entry>(v.to_string().as_bytes());
    let settled = old_reservation("aig:old", T0);
    let mut held = settled.clone();
    held["end"] = serde_json::Value::Null;
    let mut released = settled.clone();
    released["end"] = serde_json::json!({ "end": "released" });
    for old in [settled.clone(), held.clone(), released] {
        let entry = read(&old).unwrap();
        assert_eq!(entry, Entry::Before { at_ms: T0 }, "{old}");
        assert_eq!((entry.charge(), entry.at_ms()), (None, T0));
    }
    assert!(read(&serde_json::json!({ "row": {} })).is_err(), "an entry names its kind");
    assert!(read(&serde_json::json!({ "entry": "hold", "at_ms": T0 })).is_err(), "a row or a reservation");
    let mut timeless = held;
    timeless.as_object_mut().unwrap().remove("at_ms");
    assert!(read(&timeless).is_err(), "a reservation from before has its time");
    let mut w = World::seat();
    w.store.put_entry("aig:old", read(&settled).unwrap());
    let Outcome::Metered(m) = w.ok(meter("b1", vec![row("aig:old", turn(), None, T0)])) else { panic!() };
    assert_eq!((m.charged, m.refused.clone()), (0, vec![RowRefused { index: 0, why: Refused::ConflictingBody }]));
    w.now = T0 + REF_KEEP_MS;
    w.ok(Op::Sweep);
    assert!(w.store.entry("aig:old").is_some(), "kept for its time: a replay stays refused");
    w.now += 1;
    w.ok(Op::Sweep);
    assert_eq!(w.store.entry("aig:old"), None, "forgotten as any is");
}

/// Goal: an entry a ledger stored reads as its fields, once, from its
/// text, and writes back as it was (FIN-453 M7: no `Value` between).
/// Method: `stored/entries.jsonl`, rows of every kind of usage that a
/// local e2e run's ledgers stored (each identity in them that run's own).
#[test]
fn a_stored_entry_reads_from_its_text_and_writes_back_as_it_was() {
    for line in include_str!("stored/entries.jsonl").lines().filter(|l| !l.is_empty()) {
        let entry: Entry = serde_json::from_slice(line.as_bytes()).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert!(matches!(entry, Entry::Row { .. }), "{line}");
        assert_eq!(serde_json::to_string(&entry).unwrap(), line);
    }
}

/// The two bodies the ledger queue carries, as this release and the one
/// before write them (`serde_json::to_value`, sent as JSON and read back
/// by the consumer from `JSON.stringify` of what arrives), every kind of
/// usage among their rows: the outbox's batch, and a paid call's charge.
fn queued_batches() -> Vec<(MeterBatch, &'static str)> {
    let usages = [
        Usage::Tokens { model: "@cf/zai-org/glm-5.3".into(), input: 1200, cached_input: 300, cache_write: 0, output: 450 },
        Usage::Neurons { milli: 100_000_000 },
        Usage::Awake { instance: "standard-1".into(), ms: 60_000 },
        Usage::Storage { class: StorageClass::R2, byte_hours: 9 },
        Usage::Storage { class: StorageClass::Sqlite, byte_hours: 1 },
        Usage::Storage { class: StorageClass::Git, byte_hours: 2 },
        Usage::Requests { count: 3 },
        Usage::DynamicWorkers { count: 1 },
        Usage::Browser { ms: 1500 },
        Usage::Images { count: 4 },
        Usage::Key { key: "brave".into(), units: 1 },
    ];
    let rows: Vec<MeterRow> = usages.into_iter().enumerate().map(|(i, u)| row(&format!("r:{i}"), u, Some("notes--8x7j"), T0 + i as i64)).collect();
    let outbox = MeterBatch { payer: "npub1owner".into(), source: "notes--8x7j".into(), seq: 7, meter: Meter { batch: "frag:k:7".into(), rows } };
    let charged = MeterRow { agent: Some("npub1agent".into()), computer: Some("cmp-1".into()), fragment: None, ..row("key:cmp-1:00aa", Usage::Key { key: "brave".into(), units: 1 }, None, T0) };
    let charge = MeterBatch { payer: "npub1owner".into(), source: String::new(), seq: 0, meter: Meter { batch: "key:cmp-1:00aa".into(), rows: vec![charged] } };
    vec![(outbox, OUTBOX), (charge, CHARGE)]
}

const OUTBOX: &str = r#"{"meter":{"batch":"frag:k:7","rows":[{"agent":null,"at_ms":1790812800000,"computer":null,"fragment":"notes--8x7j","ref":"r:0","usage":{"cache_write":0,"cached_input":300,"input":1200,"kind":"tokens","model":"@cf/zai-org/glm-5.3","output":450}},{"agent":null,"at_ms":1790812800001,"computer":null,"fragment":"notes--8x7j","ref":"r:1","usage":{"kind":"neurons","milli":100000000}},{"agent":null,"at_ms":1790812800002,"computer":null,"fragment":"notes--8x7j","ref":"r:2","usage":{"instance":"standard-1","kind":"awake","ms":60000}},{"agent":null,"at_ms":1790812800003,"computer":null,"fragment":"notes--8x7j","ref":"r:3","usage":{"byte_hours":9,"class":"r2","kind":"storage"}},{"agent":null,"at_ms":1790812800004,"computer":null,"fragment":"notes--8x7j","ref":"r:4","usage":{"byte_hours":1,"class":"sqlite","kind":"storage"}},{"agent":null,"at_ms":1790812800005,"computer":null,"fragment":"notes--8x7j","ref":"r:5","usage":{"byte_hours":2,"class":"git","kind":"storage"}},{"agent":null,"at_ms":1790812800006,"computer":null,"fragment":"notes--8x7j","ref":"r:6","usage":{"count":3,"kind":"requests"}},{"agent":null,"at_ms":1790812800007,"computer":null,"fragment":"notes--8x7j","ref":"r:7","usage":{"count":1,"kind":"dynamic_workers"}},{"agent":null,"at_ms":1790812800008,"computer":null,"fragment":"notes--8x7j","ref":"r:8","usage":{"kind":"browser","ms":1500}},{"agent":null,"at_ms":1790812800009,"computer":null,"fragment":"notes--8x7j","ref":"r:9","usage":{"count":4,"kind":"images"}},{"agent":null,"at_ms":1790812800010,"computer":null,"fragment":"notes--8x7j","ref":"r:10","usage":{"key":"brave","kind":"key","units":1}}]},"payer":"npub1owner","seq":7,"source":"notes--8x7j"}"#;
const CHARGE: &str = r#"{"meter":{"batch":"key:cmp-1:00aa","rows":[{"agent":"npub1agent","at_ms":1790812800000,"computer":"cmp-1","fragment":null,"ref":"key:cmp-1:00aa","usage":{"key":"brave","kind":"key","units":1}}]},"payer":"npub1owner","seq":0,"source":""}"#;

#[test]
fn a_queued_batch_reads_back_from_its_json_text() {
    for (batch, text) in queued_batches() {
        assert_eq!(serde_json::to_value(&batch).unwrap().to_string(), text, "the form this release writes");
        assert_eq!(MeterBatch::from_queue(text.as_bytes()), Ok(batch), "{text}");
    }
}

/// Each queued form with a field a later release might add (to the batch,
/// its meter, and a row), as a message from it would reach this one, mid-deploy
/// or after a rollback.
fn with_added_fields(text: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(text).unwrap();
    v["region"] = serde_json::json!("weur");
    v["meter"]["sent_at"] = serde_json::json!(T0);
    for row in v["meter"]["rows"].as_array_mut().unwrap() {
        row["model_route"] = serde_json::json!({ "tier": "cheap" });
        row["note"] = serde_json::Value::Null;
    }
    v.to_string()
}

#[test]
fn a_queued_batch_with_fields_a_later_release_added_still_reads_and_bills() {
    for (batch, text) in queued_batches() {
        let added = with_added_fields(text);
        assert_ne!(added, text);
        assert_eq!(MeterBatch::from_queue(added.as_bytes()), Ok(batch.clone()), "{added}");
        // billed exactly as its own form is, on a seat's ledger
        let billed = |text: &str| {
            let mut w = World::seat();
            let mut priced = book();
            priced.keys.push(KeyPrice { key: "brave".into(), micros: 5_000_000, per: 1_000 });
            priced.instances.push(crate::price::InstancePrice { instance: "standard-1".into(), hour: 100_000 });
            w.ok(Op::Book(priced));
            let answer = w.ok(Op::Meter(MeterBatch::from_queue(text.as_bytes()).unwrap().meter));
            (answer, w.balance())
        };
        let (answer, balance) = billed(&added);
        let Outcome::Metered(m) = &answer else { panic!("{added}: {answer:?}") };
        assert_eq!(m.rows_new as usize, batch.meter.rows.len(), "every row is new: {m:?}");
        assert!(m.charged > 0 && m.refused.is_empty(), "{m:?}");
        assert_eq!((answer, balance), billed(text));
    }
    // the route's own decoder keeps its contract: no field it does not know
    let meter = |text: &str| serde_json::to_string(&serde_json::from_str::<serde_json::Value>(text).unwrap()["meter"]).unwrap();
    assert!(serde_json::from_str::<Meter>(&meter(&with_added_fields(OUTBOX))).unwrap_err().to_string().contains("unknown field"));
    assert!(serde_json::from_str::<Meter>(&meter(OUTBOX)).is_ok());
}

#[test]
fn a_queued_batch_missing_or_malformed_is_still_refused() {
    let refused = |from: &str, to: &str| {
        let text = OUTBOX.replacen(from, to, 1);
        assert_ne!(text, OUTBOX, "{from}");
        let e = MeterBatch::from_queue(text.as_bytes()).expect_err(&text);
        (text, e)
    };
    for (from, to, says) in [
        (r#""ref":"r:0","#, "", "missing field `ref`"),
        (r#""at_ms":1790812800000,"#, r#""at_ms":"soon","#, "invalid type"),
        (r#""kind":"neurons","#, r#""kind":"neutrinos","#, "unknown variant"),
        (r#""milli":100000000}"#, r#""milli":100000000,"extra":1}"#, "unknown field `extra`"),
        (r#""payer":"npub1owner","#, "", "missing field `payer`"),
        (r#""batch":"frag:k:7","#, "", "missing field `batch`"),
        (r#""seq":7"#, r#""seq":"7""#, "invalid type"),
    ] {
        let (text, e) = refused(from, to);
        assert!(e.contains(says), "{text}: {e}");
    }
    // a usage's kind an object, not its name, for every kind the outbox carries
    let mut kinds = 0;
    for kind in ["tokens", "neurons", "awake", "storage", "requests", "dynamic_workers", "browser", "images", "key"] {
        let named = format!(r#""kind":"{kind}""#);
        if OUTBOX.contains(&named) {
            kinds += 1;
            let (text, e) = refused(&named, &format!(r#""kind":{{"{kind}":null}}"#));
            assert!(e.contains("invalid type: map"), "{text}: {e}");
        }
    }
    assert_eq!(kinds, 9, "the outbox carries every kind");
    assert!(MeterBatch::from_queue(b"{}").is_err());
    assert!(MeterBatch::from_queue(br#"{"meter":7,"payer":"p","seq":0,"source":""}"#).is_err());
    assert!(MeterBatch::from_queue(b"not json").is_err());
}

/// What a ledger has stored and answered, byte for byte as the releases
/// before reading a usage and a refusal as their fields wrote it (FIN-453
/// M7), in `stored/`: `entries.jsonl`, `entries.entry` rows from a local
/// e2e run (the ledger, billing, computers, keys and icons lanes; each
/// identity in them that run's own); `answers.jsonl`, `batches.answer`
/// from that run and from `Ledger::meter` refusing a row each way it does,
/// then those answers as the ledger's route sends them (`to_value`: keys
/// sorted, a tag no longer first); `refused.jsonl`, every refusal as an
/// answer stores it; and `refusals.jsonl`, each as the ledger's refusal
/// body carries it. Each reads, and writes back, as it was.
#[test]
fn what_a_ledger_stored_and_answered_reads_back_byte_for_byte() {
    fn lines(text: &str) -> impl Iterator<Item = &str> {
        text.lines().filter(|l| !l.is_empty())
    }
    fn read<'a, T: Deserialize<'a>>(line: &'a str) -> T {
        serde_json::from_slice(line.as_bytes()).unwrap_or_else(|e| panic!("{line}: {e}"))
    }
    let entries: Vec<Entry> = lines(include_str!("stored/entries.jsonl")).map(read).collect();
    for (line, entry) in lines(include_str!("stored/entries.jsonl")).zip(&entries) {
        assert!(matches!(entry, Entry::Row { .. }), "{line}");
        assert_eq!(serde_json::to_string(entry).unwrap(), line, "an entry is stored as it was");
    }
    let kinds: std::collections::BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Row { row, .. } => serde_json::to_value(&row.usage).ok().and_then(|u| u["kind"].as_str().map(str::to_string)),
            Entry::Before { .. } => None,
        })
        .collect();
    assert_eq!(kinds.len(), 9, "every kind of usage is among the stored rows: {kinds:?}");
    for line in lines(include_str!("stored/answers.jsonl")) {
        let answer: Metered = read(line);
        let (stored, sent) = (serde_json::to_string(&answer).unwrap(), serde_json::to_value(&answer).unwrap().to_string());
        assert!(line == stored || line == sent, "an answer reads back as it was stored or sent: {line}");
    }
    let refused: Vec<Refused> = lines(include_str!("stored/refused.jsonl")).map(read).collect();
    for (line, why) in lines(include_str!("stored/refused.jsonl")).zip(&refused) {
        assert_eq!(serde_json::to_string(why).unwrap(), line, "a refusal is stored as it was");
    }
    #[derive(Deserialize)]
    struct Body {
        error: ErrorCode,
        message: String,
        refused: Refused,
    }
    for (line, why) in lines(include_str!("stored/refusals.jsonl")).zip(&refused) {
        let body: Body = read(line);
        assert_eq!((body.refused, body.error, body.message.as_str()), (*why, why.code(), why.message().as_str()), "{line}");
        assert_eq!(serde_json::json!({ "error": body.error, "message": body.message, "refused": body.refused }).to_string(), line);
    }
    assert_eq!(refused.len(), 23, "every refusal, each `Invalid` its own");
}

/// A refusal's `refused` is a name, a string, as the tagged reading's tag
/// was: an object (`{"no_price": null}`) is no refusal, for every one
/// (FIN-453 M7, review).
#[test]
fn a_refusal_is_named_by_a_string_alone() {
    for line in include_str!("stored/refused.jsonl").lines().filter(|l| !l.is_empty()) {
        let kind = serde_json::from_str::<serde_json::Value>(line).unwrap()["refused"].as_str().unwrap().to_string();
        let object = line.replacen(&format!("\"refused\":\"{kind}\""), &format!("\"refused\":{{\"{kind}\":null}}"), 1);
        assert_ne!(object, line);
        let e = serde_json::from_slice::<Refused>(object.as_bytes()).map(|r| format!("{r:?}")).expect_err(&object).to_string();
        assert!(e.contains("invalid type: map"), "{object}: {e}");
    }
}
