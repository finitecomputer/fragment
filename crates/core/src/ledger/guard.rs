//! The shared-key guard (docs/ledger.md, "Shared-key guard"; FIN-448,
//! Paul, 2026-11-08): a rolling hour of what a payer was charged on each
//! operator key, so one runaway key pauses at `KEY_HOUR_CAP` while
//! everything else the person does goes on: their agents' turns, their
//! other keys, their fragments' writes. On 2026-11-07 a script left on a
//! person's computer called ElevenLabs every two seconds and spent $300 in
//! about 35 minutes through the shared key; nothing slowed it.
//!
//! What counts: each charge of a `key` row, by the key it is of
//! (`guarded_key`: a part the book prices apart, `elevenlabs:speech`, is
//! its key's), as the ledger charged it, once by its reference (a replay
//! adds nothing). What a call is priced at is the book's affair: the guard
//! sums charged micros.
//!
//! The hour is kept per key (`Store::key_hour`) as sums of `SLOT_MS`
//! slots: a charge counts from its row's time for 60 to 65 minutes (the
//! hour and the rest of its slot), so a paused key resumes as its oldest
//! slots age out (`KeyHour::resumes_in_ms`). Bounded: at most `SLOTS + 1`
//! slots a key, and one record a key, the book's keys at most (a row the
//! book cannot price is refused before it is charged).
//!
//! Best effort, never in an agent's way (decision 62): the cell asks it in
//! the gate's one question (`key_open`), cached a minute in each isolate,
//! and a ledger that does not answer lets the call go. So a key that
//! reaches its cap may be charged past it by the calls that start within
//! the minute, and by those whose rows are still on their way.

use serde::{Deserialize, Serialize};

use super::{invalid, valid_id, Invalid, Ledger, Refused, Spend, Store};
use crate::price::USD;

/// What one operator key may be charged a payer in a rolling hour before
/// its calls pause (FIN-448; a decision for Paul). At a 200% margin, a
/// loop like 2026-11-07's (a 200-character clip every 1.75 s: $0.016 at
/// list, $0.048 charged, $1.65 a minute) reaches it in about three
/// minutes, while an hour of normal use fits under it: about 70 spoken
/// replies of 300 characters, a dozen X searches, ten songs or 45 place
/// searches. $2 would stop the loop a minute sooner but pause a voice
/// chat of 28 replies. One for every key, defined here alone: an override
/// would be a command on the ledger (docs/ledger.md, "Shared-key guard").
pub const KEY_HOUR_CAP: i64 = 5 * USD;
/// The hour, and its slots: each a sum, so a key's record stays small
/// however fast its calls come.
pub const HOUR_MS: i64 = 3_600_000;
pub const SLOT_MS: i64 = 5 * 60_000;
pub const SLOTS: i64 = HOUR_MS / SLOT_MS;

const _: () = assert!(HOUR_MS % SLOT_MS == 0, "the hour is whole slots");
const _: () = assert!(KEY_HOUR_CAP > 0 && KEY_HOUR_CAP <= super::CAP_MAX, "the guard's cap is a cap");

/// The shared key a key row's usage names: a part's name (`price::KeyPart`,
/// `<key>:<part>`) is its key's, so a key's guard sums all its parts. A
/// catalog row's name has no `:` (`catalog`), so a key's own name is itself.
pub fn guarded_key(name: &str) -> &str {
    name.split_once(':').map_or(name, |(key, _)| key)
}

/// Whether slot `slot` counts at `now_ms`: the slot `now_ms` is in and the
/// `SLOTS` before it. A slot ahead of it (a clock that went back) counts
/// not: the guard fails open.
fn counts(slot: i64, now_ms: i64) -> bool {
    let now = now_ms.div_euclid(SLOT_MS);
    (now - SLOTS..=now).contains(&slot)
}

/// One key's charges this last hour, as the store keeps it. A field a
/// later release adds is read past, so a record stays readable across
/// releases.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyHour {
    /// `(slot, micros)`, oldest first, each sum above zero: slot `n` is
    /// `[n × SLOT_MS, (n + 1) × SLOT_MS)`.
    #[serde(default)]
    slots: Vec<(i64, i64)>,
}

impl KeyHour {
    /// A record as the store holds it.
    pub fn read(text: &str) -> Result<KeyHour, String> {
        let hour: KeyHour = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let ordered = hour.slots.windows(2).all(|w| w[0].0 < w[1].0);
        if !ordered || hour.slots.iter().any(|(_, micros)| *micros <= 0) {
            return Err("its slots are not in order, or not each above zero".into());
        }
        Ok(hour)
    }

    pub fn text(&self) -> String {
        serde_json::to_string(self).expect("a key's hour serializes")
    }

    /// What counts at `now_ms`, together.
    pub fn spent(&self, now_ms: i64) -> i64 {
        self.slots.iter().filter(|(slot, _)| counts(*slot, now_ms)).fold(0, |sum, (_, micros)| sum.saturating_add(*micros))
    }

    /// `micros` charged for a row from `at_ms`, at `now_ms`. What no longer
    /// counts goes; a row from before the hour adds nothing, and one from
    /// ahead of the ledger's clock (a source's skew) counts as now.
    pub fn add(&mut self, at_ms: i64, micros: i64, now_ms: i64) {
        assert!(micros > 0, "only a charge adds to a key's hour");
        self.slots.retain(|(slot, _)| counts(*slot, now_ms));
        let slot = at_ms.min(now_ms).div_euclid(SLOT_MS);
        if counts(slot, now_ms) {
            match self.slots.binary_search_by_key(&slot, |(s, _)| *s) {
                Ok(i) => self.slots[i].1 = self.slots[i].1.saturating_add(micros),
                Err(i) => self.slots.insert(i, (slot, micros)),
            }
        }
        assert!(self.slots.len() as i64 <= SLOTS + 1, "an hour holds at most SLOTS + 1 slots");
    }

    /// How long from `now_ms` until what counts is under `cap`, as its
    /// oldest slots age out: 0 when it is already.
    pub fn resumes_in_ms(&self, now_ms: i64, cap: i64) -> i64 {
        let mut left = self.spent(now_ms);
        for (slot, micros) in self.slots.iter().filter(|(slot, _)| counts(*slot, now_ms)) {
            if left < cap {
                break;
            }
            left = left.saturating_sub(*micros);
            if left < cap {
                // it stops counting as the slot `SLOTS` after it ends
                return (slot + SLOTS + 1) * SLOT_MS - now_ms;
            }
        }
        0
    }
}

/// Adds a charge of `micros` on key usage `name`, from a row at `at_ms`,
/// to its key's hour (the ledger's `meter_row`, as it charges the row).
pub(super) fn charged(store: &mut impl Store, name: &str, at_ms: i64, micros: i64, now_ms: i64) {
    let key = guarded_key(name);
    let mut hour = store.key_hour(key);
    hour.add(at_ms, micros, now_ms);
    store.put_key_hour(key, &hour);
}

impl Ledger {
    /// May an agent of this payer's call operator key `key` now? Its
    /// standing lets an agent's turn spend (`may_spend`, as every key's
    /// call asked before the guard), and the key's last hour is under
    /// `KEY_HOUR_CAP`: at it, the key pauses (`KeyPaused`, saying for how
    /// long), and nothing else does.
    pub fn key_open(&self, store: &impl Store, key: &str, now_ms: i64) -> Result<(), Refused> {
        if !valid_id(key) {
            return Err(invalid(Invalid::Id));
        }
        self.may_spend(store, Spend::AgentTurn, None, true, now_ms)?;
        let hour = store.key_hour(key);
        let spent = hour.spent(now_ms);
        if spent < KEY_HOUR_CAP {
            return Ok(());
        }
        Err(Refused::KeyPaused { cap: KEY_HOUR_CAP, spent, resumes_in_ms: hour.resumes_in_ms(now_ms, KEY_HOUR_CAP) })
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::ledger::{GrantCredit, Plan, SetPlan, Why};
    use fragment_proto::ErrorCode;

    use super::super::sim::{apply, Op, Outcome, T0};
    use super::super::{Memory, Meter, MeterRow};
    use super::*;
    use crate::price::{KeyPrice, PriceBook, Usage, KEY_PARTS};

    const MINUTE: i64 = 60_000;
    /// What a call on the tests' keys is charged: one unit of each is
    /// $1.00 at list, $1.50 with the default margin.
    const CALL: i64 = 1_500_000;

    /// The default book and two keys at $1.00 a call, `voice` with a part
    /// priced apart (`voice:speech`, $1.00 a unit too).
    fn book() -> PriceBook {
        let mut book = PriceBook::defaults();
        for key in ["voice", "voice:speech", "search"] {
            book.keys.push(KeyPrice { key: key.into(), micros: USD, per: 1 });
        }
        book
    }

    struct World {
        ledger: Ledger,
        store: Memory,
        now: i64,
        book: PriceBook,
    }

    impl World {
        /// A seat with $100 more than its month's credit.
        fn funded() -> World {
            let mut w = World { ledger: Ledger::new(T0), store: Memory::default(), now: T0, book: book() };
            w.ok(Op::Plan(SetPlan { id: "plan:seat".into(), plan: Plan::Seat }));
            w.ok(Op::Grant(GrantCredit { id: "grant:1".into(), micros: 100 * USD, by: "id:operator".into(), why: "a test".into() }));
            w
        }

        fn ok(&mut self, op: Op) -> Outcome {
            apply(&mut self.ledger, &mut self.store, &op, self.now, &mut self.book).unwrap_or_else(|e| panic!("{op:?} was refused: {e:?}"))
        }

        /// A call on `key` metered as the swap meters it: a batch of its
        /// own, its reference `reference`, at the world's now.
        fn call(&mut self, reference: &str, key: &str) -> Outcome {
            let row = MeterRow { reference: reference.into(), usage: Usage::Key { key: key.into(), units: 1 }, fragment: None, agent: Some("npub1agent".into()), computer: Some("c1".into()), at_ms: self.now };
            self.ok(Op::Meter(Meter { batch: format!("batch:{reference}"), rows: vec![row] }))
        }

        fn open(&self, key: &str) -> Result<(), Refused> {
            self.ledger.key_open(&self.store, key, self.now)
        }

        fn balance(&self) -> i64 {
            self.ledger.status(&self.store, self.now, &self.book).balance_micros
        }

        /// The state written and read back, as the Durable Object's is.
        fn restarted(&self) -> World {
            let ledger: Ledger = serde_json::from_str(&serde_json::to_string(&self.ledger).unwrap()).unwrap();
            let store: Memory = serde_json::from_str(&serde_json::to_string(&self.store).unwrap()).unwrap();
            assert_eq!((&ledger, &store), (&self.ledger, &self.store), "the state reads back as it was written");
            World { ledger, store, now: self.now, book: self.book.clone() }
        }
    }

    /// Goal: a key's calls go on under its cap and pause at it, that key
    /// alone: another key, an agent's turn and the fragments' writes go
    /// on, and the account is neither stopped nor read-only. Method: four
    /// calls on `voice` ($6.00) a minute apart, asking after each.
    #[test]
    fn a_key_pauses_at_its_cap_and_nothing_else_does() {
        let mut w = World::funded();
        assert_eq!(w.open("voice"), Ok(()), "a key never charged is open");
        for n in 1..=3 {
            w.call(&format!("key:c1:{n}"), "voice");
            assert_eq!(w.open("voice"), Ok(()), "under its cap after {n}");
            w.now += MINUTE;
        }
        let before = w.balance();
        w.call("key:c1:4", "voice");
        assert_eq!(w.balance(), before - CALL, "the call that reached the cap was charged as any");
        let paused = w.open("voice");
        // the first call's slot stops counting an hour after its slot ends
        let resumes = (T0.div_euclid(SLOT_MS) + SLOTS + 1) * SLOT_MS - w.now;
        assert_eq!(paused, Err(Refused::KeyPaused { cap: KEY_HOUR_CAP, spent: 4 * CALL, resumes_in_ms: resumes }));
        let why = paused.unwrap_err();
        assert_eq!((why.code(), why.code().status()), (ErrorCode::KeyPaused, 429));
        assert_eq!(why.message(), "this key is paused: $6.00 spent on it in the last hour, its limit $5.00 an hour; its calls resume as the hour passes, in about 62 minutes");
        assert_eq!(w.open("search"), Ok(()), "another key goes on");
        for spend in [Spend::AgentTurn, Spend::AiStep, Spend::Write, Spend::Create, Spend::Wake] {
            assert_eq!(w.ledger.may_spend(&w.store, spend, None, true, w.now), Ok(()), "{spend:?} goes on");
        }
        let status = w.ledger.status(&w.store, w.now, &w.book);
        assert_eq!(status.standing, fragment_proto::ledger::Standing::Ok, "the account is neither stopped nor read-only");
    }

    /// Goal: the hour rolls. Method: $6.00 in one slot; just before that
    /// slot ages out it is paused, as it does it is open; and a charge
    /// spread over the hour resumes as each slot goes, not all at once.
    #[test]
    fn a_paused_key_resumes_as_its_charges_age_out() {
        let mut w = World::funded();
        for n in 1..=4 {
            w.call(&format!("key:c1:{n}"), "voice");
        }
        let gone = (T0.div_euclid(SLOT_MS) + SLOTS + 1) * SLOT_MS;
        w.now = gone - 1;
        assert!(matches!(w.open("voice"), Err(Refused::KeyPaused { resumes_in_ms: 1, .. })), "{:?}", w.open("voice"));
        w.now = gone;
        assert_eq!(w.open("voice"), Ok(()), "open as its charges age out");

        // three calls 20 minutes apart, a fourth at the end: paused until
        // the first ages out, then open (three left is $4.50)
        let mut w = World::funded();
        let start = w.now;
        for n in 0..4 {
            w.now = start + n * 20 * MINUTE;
            w.call(&format!("key:c1:{n}"), "voice");
        }
        let first_gone = (start.div_euclid(SLOT_MS) + SLOTS + 1) * SLOT_MS;
        let Err(Refused::KeyPaused { resumes_in_ms, .. }) = w.open("voice") else { panic!("paused") };
        assert_eq!(w.now + resumes_in_ms, first_gone);
        w.now = first_gone;
        assert_eq!(w.open("voice"), Ok(()));
        assert_eq!(w.store.key_hour("voice").spent(w.now), 3 * CALL);
        // a row heard late, from before the hour, adds nothing
        let late = MeterRow { reference: "key:c1:late".into(), usage: Usage::Key { key: "voice".into(), units: 1 }, fragment: None, agent: None, computer: None, at_ms: start - HOUR_MS };
        w.ok(Op::Meter(Meter { batch: "batch:late".into(), rows: vec![late] }));
        assert_eq!(w.store.key_hour("voice").spent(w.now), 3 * CALL, "charged, but past the hour");
    }

    /// Goal: a part's charges are its key's, and a call refused by its
    /// provider (charged nothing) counts nothing. Method: speech units on
    /// `voice:speech`, then a zero-unit row.
    #[test]
    fn a_keys_parts_count_as_the_key() {
        for p in KEY_PARTS {
            assert_eq!(guarded_key(p.priced_as), p.key, "{p:?}");
            assert_eq!(guarded_key(p.key), p.key);
        }
        let mut w = World::funded();
        let row = |r: &str, units| MeterRow { reference: r.into(), usage: Usage::Key { key: "voice:speech".into(), units }, fragment: None, agent: None, computer: None, at_ms: T0 };
        w.ok(Op::Meter(Meter { batch: "b:1".into(), rows: vec![row("key:c1:s1", 4), row("key:c1:s0", 0)] }));
        assert_eq!(w.store.key_hour("voice").spent(w.now), 4 * CALL);
        assert_eq!(w.store.key_hour("voice:speech"), KeyHour::default(), "kept as its key's alone");
        assert!(matches!(w.open("voice"), Err(Refused::KeyPaused { .. })));
    }

    /// Goal: what the guard is asked of a key it does not know, or a name
    /// no key has. Method: an unknown key is open; an invalid name is
    /// refused, changing nothing; a paused key's standing refusal (out of
    /// credit) is the one said, not the pause.
    #[test]
    fn unknown_and_invalid_keys() {
        let mut w = World::funded();
        assert_eq!(w.open("never-heard-of"), Ok(()));
        for bad in ["", "two words", &"k".repeat(super::super::ID_MAX_BYTES + 1)] {
            assert_eq!(w.open(bad), Err(Refused::Invalid { what: Invalid::Id }), "{bad:?}");
        }
        // a row the book cannot price is charged nothing, and keeps no hour
        let row = MeterRow { reference: "key:c1:x".into(), usage: Usage::Key { key: "unpriced".into(), units: 1 }, fragment: None, agent: None, computer: None, at_ms: T0 };
        let Outcome::Metered(m) = w.ok(Op::Meter(Meter { batch: "b:x".into(), rows: vec![row] })) else { panic!() };
        assert_eq!(m.refused.len(), 1);
        assert_eq!(w.store.key_hour("unpriced"), KeyHour::default());
        // a guest's ledger answers as a guest, before the key's hour
        let guest = World { ledger: Ledger::new(T0), store: Memory::default(), now: T0, book: book() };
        assert_eq!(guest.open("voice"), Err(Refused::GuestPayer));
        // out of credit ($150: the seat's $50 and the grant) and paused:
        // the standing is what is said
        for n in 1..=100 {
            w.call(&format!("key:c1:{n}"), "voice");
        }
        assert_eq!(w.open("voice"), Err(Refused::AgentsStopped { why: Why::NoCredit }));
    }

    /// Goal: a call is counted once by its reference, whatever batch
    /// carries it, and the hour survives a restart. Method: a call, its
    /// batch again, the row in another batch; then the state read back,
    /// and the cap reached across it.
    #[test]
    fn a_replay_counts_nothing_and_a_restart_keeps_the_hour() {
        let mut w = World::funded();
        let first = w.call("key:c1:1", "voice");
        assert_eq!(w.call("key:c1:1", "voice"), first, "the batch again answers as it did");
        let row = MeterRow { reference: "key:c1:1".into(), usage: Usage::Key { key: "voice".into(), units: 1 }, fragment: None, agent: Some("npub1agent".into()), computer: Some("c1".into()), at_ms: w.now };
        let Outcome::Metered(m) = w.ok(Op::Meter(Meter { batch: "another".into(), rows: vec![row] })) else { panic!() };
        assert_eq!((m.rows_new, m.rows_before), (0, 1));
        assert_eq!(w.store.key_hour("voice").spent(w.now), CALL, "counted once");
        let mut w = w.restarted();
        assert_eq!(w.store.key_hour("voice").spent(w.now), CALL, "kept across a restart");
        for n in 2..=4 {
            w.call(&format!("key:c1:{n}"), "voice");
            w = w.restarted();
        }
        assert!(matches!(w.open("voice"), Err(Refused::KeyPaused { spent, .. }) if spent == 4 * CALL));
    }

    /// Goal: a record stays bounded however fast its calls come, and one
    /// that does not read counts nothing (the cell reads it so: the guard
    /// fails open), while a field a later release adds is read past.
    /// Method: a call every 10 s for three hours; records as text.
    #[test]
    fn a_record_is_bounded_and_read_leniently() {
        let mut hour = KeyHour::default();
        for n in 0..(3 * HOUR_MS / 10_000) {
            let at = T0 + n * 10_000;
            hour.add(at, 1, at);
            assert!(hour.slots.len() as i64 <= SLOTS + 1);
        }
        let now = T0 + 3 * HOUR_MS - 10_000;
        assert_eq!(KeyHour::read(&hour.text()), Ok(hour.clone()));
        assert!(hour.spent(now) >= HOUR_MS / 10_000, "at least the hour's calls count");
        assert!(hour.spent(now) <= (HOUR_MS + SLOT_MS) / 10_000, "and at most the hour and a slot's");
        assert_eq!(KeyHour::read(r#"{"slots":[[5,10]],"later":true}"#).map(|h| h.spent(5 * SLOT_MS)), Ok(10), "a later field is read past");
        for bad in ["", "null", r#"{"slots":[[6,1],[5,1]]}"#, r#"{"slots":[[5,0]]}"#, r#"{"slots":"x"}"#] {
            assert!(KeyHour::read(bad).is_err(), "{bad}");
        }
        // a clock that went back: a slot ahead of now counts not
        let mut ahead = KeyHour::default();
        ahead.add(T0, 7, T0);
        assert_eq!((ahead.spent(T0), ahead.spent(T0 - SLOT_MS)), (7, 0));
    }
}
