//! What a person's agents used of each provider, by month (docs/ledger.md,
//! "Usage"; M16, moved from each computer's `uses`): each call a provider
//! answered through a computer's swap, a connection's and an own key's at
//! $0, an operator key's at its charge as the swap priced it. A display,
//! never money: what charges is the meter (`Ledger::meter`), so a count
//! lost loses no charge (Paul, 2026-11-18: accounting is best effort, and
//! never in a call's way). Kept `MONTHS_KEPT` months in the ledger's own
//! table, apart from its head: counting neither reads nor writes the head,
//! so it never changes what the gate answers.
//!
//! A computer from before the move kept its own rows; the ones it held
//! when its step 7 dropped them it owes, a month at a time (`TakeUses`),
//! each taken once by its id, whole: a hand-over sent again, its answer
//! lost, adds nothing twice.

use std::collections::BTreeMap;

use fragment_proto::ledger::{LedgerUses, ProviderUse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{invalid, valid_id, Invalid, Month, Refused, CLOCK_SKEW_MS};
use crate::catalog::PROVIDERS_MAX;
use crate::price::CHARGE_MAX;

/// Months kept: this one and the twelve before it.
pub const MONTHS_KEPT: u32 = 13;
/// Rows a month counts for a person: each of their agents at each
/// provider. A month holding them takes no new pair (a count lost, said
/// aloud by the cell); a pair it holds counts on.
pub const ROWS_MAX: u64 = fragment_proto::limits::AGENTS_PER_OWNER_MAX * PROVIDERS_MAX as u64;
/// Providers one swap names at most (`swap::PLACEHOLDERS_MAX`).
pub const USES_MAX: usize = crate::swap::PLACEHOLDERS_MAX;
/// Rows one hand-over carries at most: a month of a computer's, as it kept
/// them (four rows for each of its 16 agents at each provider).
pub const OWED_ROWS_MAX: usize = 4 * PROVIDERS_MAX * 16;

/// A swap's calls (`/used`): `agent`'s, one to each provider named, at
/// `at_ms`, each with what the swap priced it (an operator key's charge;
/// zero for any other).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Used {
    pub agent: String,
    pub at_ms: i64,
    pub uses: Vec<Use>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Use {
    pub provider: String,
    pub micros: i64,
}

/// A computer's month of rows from before the move (`/take-uses`), handed
/// over under `id` (`<computer>:<month>`): taken once, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeUses {
    pub id: String,
    pub month: Month,
    pub rows: Vec<UseRow>,
}

/// One agent's calls to one provider in a month, and their charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseRow {
    pub agent: String,
    pub provider: String,
    pub calls: u64,
    pub micros: i64,
}

/// Where a ledger keeps its uses: a table by month, agent and provider, and
/// the hand-overs it took, by id with their body's digest. The Durable
/// Object's SQLite in the cell; `Memory` in tests.
pub trait Tally {
    /// Rows `month` holds.
    fn rows(&self, month: Month) -> u64;
    fn holds(&self, month: Month, agent: &str, provider: &str) -> bool;
    fn add(&mut self, month: Month, agent: &str, provider: &str, calls: u64, micros: i64);
    /// Forgets the months before `month`.
    fn forget_before(&mut self, month: Month);
    /// A hand-over taken: its body's digest.
    fn taken(&self, id: &str) -> Option<String>;
    fn put_taken(&mut self, id: &str, digest: &str);
    /// `month`'s rows, by provider then agent.
    fn month(&self, month: Month) -> Vec<ProviderUse>;
    /// The last hour's spend, slot by slot (`spend`).
    fn add_spend(&mut self, slot: i64, micros: i64);
    fn spent_since(&self, slot: i64) -> i64;
    /// The newest slot with spend in it, if any.
    fn spent_last(&self) -> Option<i64>;
    fn forget_spend_before(&mut self, slot: i64);
}

/// The last hour's spend on shared keys, in slots this long (FIN-616): the
/// hourly key pause's window, kept for the notice that replaced it
/// (`said::spending`), never to stop a call.
pub const SPEND_SLOT_MS: i64 = 300_000;
/// Slots an hour holds.
pub const SPEND_SLOTS: i64 = 12;
const _: () = assert!(SPEND_SLOT_MS * SPEND_SLOTS == 3_600_000, "an hour of slots");

fn spend_slot(ms: i64) -> i64 {
    ms.div_euclid(SPEND_SLOT_MS)
}

/// What a swap's calls cost (`u`, as `count` took it), counted in its slot
/// of the hour, the slots past the hour forgotten: whether the hour's spend
/// passed a step of its notice (`said::spending_step`), so its person's
/// pages are told.
pub fn spend(t: &mut impl Tally, u: &Used, now_ms: i64) -> bool {
    let before = super::said::spending_step(spent_hour(t, now_ms));
    // at most USES_MAX charges, each within CHARGE_MAX (`count`'s check)
    let micros: i64 = u.uses.iter().map(|x| x.micros).sum();
    if micros > 0 {
        t.add_spend(spend_slot(u.at_ms.min(now_ms)), micros);
    }
    t.forget_spend_before(hour_from(now_ms));
    super::said::spending_step(spent_hour(t, now_ms)) > before
}

/// The oldest slot the hour ending at `now_ms` holds: a whole hour of
/// them before now's, so a call an hour old or less always counts (the
/// window is an hour to an hour and five minutes).
fn hour_from(now_ms: i64) -> i64 {
    spend_slot(now_ms) - SPEND_SLOTS
}

/// What the last hour's slots hold, the hour ending at `now_ms`.
pub fn spent_hour(t: &impl Tally, now_ms: i64) -> i64 {
    t.spent_since(hour_from(now_ms))
}

/// When what the last hour's slots hold has all aged out (`spent_hour`
/// none), if they hold any: a spending notice is told until then.
pub fn spent_until(t: &impl Tally, now_ms: i64) -> Option<i64> {
    t.spent_last().filter(|slot| *slot >= hour_from(now_ms)).map(|slot| (slot + SPEND_SLOTS + 1) * SPEND_SLOT_MS)
}

/// The oldest month kept at `now_ms`.
pub fn oldest(now_ms: i64) -> Month {
    Month(Month::of(now_ms).0.saturating_sub(MONTHS_KEPT - 1))
}

fn valid_row(agent: &str, provider: &str, micros: i64) -> bool {
    valid_id(agent) && valid_id(provider) && (0..=CHARGE_MAX).contains(&micros)
}

/// Counts a swap's calls in `at_ms`'s month, one each, and forgets months
/// past `MONTHS_KEPT`. Answers how many it could not keep: a pair new to a
/// month already holding `ROWS_MAX`.
pub fn count(t: &mut impl Tally, u: &Used, now_ms: i64) -> Result<u32, Refused> {
    if u.uses.is_empty() || u.uses.len() > USES_MAX {
        return Err(invalid(Invalid::Rows));
    }
    if !u.uses.iter().all(|x| valid_row(&u.agent, &x.provider, x.micros)) {
        return Err(invalid(Invalid::Id));
    }
    if u.at_ms < 0 || u.at_ms > now_ms + CLOCK_SKEW_MS {
        return Err(invalid(Invalid::Time));
    }
    let (month, oldest) = (Month::of(u.at_ms), oldest(now_ms));
    if month < oldest {
        return Err(Refused::Stale);
    }
    let mut full = 0;
    for x in &u.uses {
        if !t.holds(month, &u.agent, &x.provider) && t.rows(month) >= ROWS_MAX {
            full += 1;
            continue;
        }
        t.add(month, &u.agent, &x.provider, 1, x.micros);
    }
    t.forget_before(oldest);
    Ok(full)
}

/// Takes a computer's month from before the move, once by its id: the same
/// body again adds nothing; another under its id is refused. A month past
/// what is kept is taken and kept nowhere, and months past it forgotten,
/// as a count forgets them. Bounded by the computer's own cap
/// (`OWED_ROWS_MAX`), not `ROWS_MAX`, and a row's charge by nothing but
/// its sign (a month's total, no call's: `CHARGE_MAX` bounds one call):
/// rows owed are never dropped.
pub fn take(t: &mut impl Tally, o: &TakeUses, now_ms: i64) -> Result<(), Refused> {
    if !valid_id(&o.id) {
        return Err(invalid(Invalid::Id));
    }
    if o.rows.is_empty() || o.rows.len() > OWED_ROWS_MAX {
        return Err(invalid(Invalid::Rows));
    }
    if !o.rows.iter().all(|r| r.calls > 0 && r.micros >= 0 && valid_id(&r.agent) && valid_id(&r.provider)) {
        return Err(invalid(Invalid::Id));
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&(o.month, &o.rows)).expect("rows serialize")));
    match t.taken(&o.id) {
        Some(d) if d == digest => return Ok(()),
        Some(_) => return Err(Refused::ConflictingBody),
        None => {}
    }
    let oldest = oldest(now_ms);
    if o.month >= oldest {
        for r in &o.rows {
            t.add(o.month, &r.agent, &r.provider, r.calls, r.micros);
        }
    }
    t.put_taken(&o.id, &digest);
    t.forget_before(oldest);
    Ok(())
}

/// The ledger's sweep (its alarm, at least daily): the months past those
/// kept forgotten, as a count forgets them, for a person whose agents count
/// nothing more.
pub fn sweep(t: &mut impl Tally, now_ms: i64) {
    t.forget_before(oldest(now_ms));
}

/// `month`'s uses at `now_ms`: none for a month past those kept, which a
/// read leaves out (it writes nothing) until a count or a hand-over
/// forgets it.
pub fn read(t: &impl Tally, month: Month, now_ms: i64) -> LedgerUses {
    let uses = if month < oldest(now_ms) { vec![] } else { t.month(month) };
    LedgerUses { month: month.label(), uses }
}

/// A tally in memory, for tests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Memory {
    pub rows: BTreeMap<(Month, String, String), (u64, i64)>,
    pub taken: BTreeMap<String, String>,
    pub spend: BTreeMap<i64, i64>,
}

impl Tally for Memory {
    fn rows(&self, month: Month) -> u64 {
        self.rows.keys().filter(|(m, _, _)| *m == month).count() as u64
    }

    fn holds(&self, month: Month, agent: &str, provider: &str) -> bool {
        self.rows.contains_key(&(month, agent.to_string(), provider.to_string()))
    }

    fn add(&mut self, month: Month, agent: &str, provider: &str, calls: u64, micros: i64) {
        let row = self.rows.entry((month, agent.to_string(), provider.to_string())).or_insert((0, 0));
        *row = (row.0 + calls, row.1 + micros);
    }

    fn forget_before(&mut self, month: Month) {
        self.rows.retain(|(m, _, _), _| *m >= month);
    }

    fn taken(&self, id: &str) -> Option<String> {
        self.taken.get(id).cloned()
    }

    fn put_taken(&mut self, id: &str, digest: &str) {
        self.taken.insert(id.to_string(), digest.to_string());
    }

    fn month(&self, month: Month) -> Vec<ProviderUse> {
        let mut uses: Vec<ProviderUse> = self.rows.iter().filter(|((m, _, _), _)| *m == month).map(|((_, agent, provider), (calls, micros))| ProviderUse { provider: provider.clone(), agent: agent.clone(), calls: *calls, micros: *micros }).collect();
        uses.sort_by(|a, b| (&a.provider, &a.agent).cmp(&(&b.provider, &b.agent)));
        uses
    }

    fn add_spend(&mut self, slot: i64, micros: i64) {
        *self.spend.entry(slot).or_insert(0) += micros;
    }

    fn spent_since(&self, slot: i64) -> i64 {
        self.spend.range(slot..).map(|(_, m)| m).sum()
    }

    fn spent_last(&self) -> Option<i64> {
        self.spend.keys().next_back().copied()
    }

    fn forget_spend_before(&mut self, slot: i64) {
        self.spend.retain(|s, _| *s >= slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 3_600_000;
    /// 2026-11-15, mid-month.
    const NOW: i64 = 20_772 * DAY_MS;

    fn used(agent: &str, at_ms: i64, uses: &[(&str, i64)]) -> Used {
        Used { agent: agent.into(), at_ms, uses: uses.iter().map(|(p, m)| Use { provider: (*p).into(), micros: *m }).collect() }
    }

    fn row(agent: &str, provider: &str, calls: u64, micros: i64) -> UseRow {
        UseRow { agent: agent.into(), provider: provider.into(), calls, micros }
    }

    fn of(t: &Memory, month: Month) -> Vec<(String, String, u64, i64)> {
        t.month(month).into_iter().map(|u| (u.provider, u.agent, u.calls, u.micros)).collect()
    }

    fn line(provider: &str, agent: &str, calls: u64, micros: i64) -> (String, String, u64, i64) {
        (provider.into(), agent.into(), calls, micros)
    }

    /// Goal: a swap's calls count in their month, one each, at their price;
    /// what is refused counts nothing. Method: two swaps and four refusals.
    #[test]
    fn a_swap_counts_each_call_once_in_its_month() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        assert_eq!(count(&mut t, &used("lead--a1", NOW, &[("github", 0), ("perplexity", 7_500)]), NOW), Ok(0));
        assert_eq!(count(&mut t, &used("lead--a1", NOW - DAY_MS, &[("perplexity", 7_500)]), NOW), Ok(0));
        assert_eq!(of(&t, month), [line("github", "lead--a1", 1, 0), line("perplexity", "lead--a1", 2, 15_000)]);
        let before = t.clone();
        for (u, why) in [
            (used("lead--a1", NOW, &[]), invalid(Invalid::Rows)),
            (used("lead--a1", NOW, &[("a", 0); USES_MAX + 1]), invalid(Invalid::Rows)),
            (used("lead--a1", NOW, &[("perplexity", -1)]), invalid(Invalid::Id)),
            (used("", NOW, &[("perplexity", 0)]), invalid(Invalid::Id)),
            (used("lead--a1", NOW + CLOCK_SKEW_MS + 1, &[("perplexity", 0)]), invalid(Invalid::Time)),
            (used("lead--a1", NOW - 400 * DAY_MS, &[("perplexity", 0)]), Refused::Stale),
        ] {
            assert_eq!(count(&mut t, &u, NOW), Err(why), "{u:?}");
        }
        assert_eq!(t, before, "a refusal counts nothing");
    }

    /// FIN-616: the hour's spend on shared keys, which replaced the hourly
    /// key pause with a notice. Goal: a swap's charges count in their
    /// five-minute slot, an hour's slots are summed and older ones
    /// forgotten, a call's time past now counts now, and a count says
    /// whether the hour passed a step of its notice ($5, then each
    /// doubling), once a step. Method: swaps across an hour and past it.
    #[test]
    fn an_hours_spend_is_counted_and_its_steps_said_once() {
        let mut t = Memory::default();
        let step = 5_000_000;
        assert!(!spend(&mut t, &used("lead--a1", NOW, &[("elevenlabs", step - 1), ("github", 0)]), NOW), "under $5: nothing to tell");
        assert_eq!(spent_hour(&t, NOW), step - 1);
        assert!(spend(&mut t, &used("lead--a1", NOW, &[("xai", 1)]), NOW), "at $5: told");
        assert!(!spend(&mut t, &used("lead--b2", NOW, &[("xai", 4_000_000)]), NOW), "$9: the same step, told no more");
        assert!(spend(&mut t, &used("lead--b2", NOW, &[("xai", 1_000_000)]), NOW), "$10: the next step, told");
        assert!(!spend(&mut t, &used("lead--b2", NOW, &[("github", 0)]), NOW), "nothing spent, nothing told");
        // a call an hour old still counts: the window is a whole hour of slots before now's
        assert_eq!(spent_hour(&t, NOW + 3_600_000), 10_000_000, "an hour on, the hour's charges still in it");
        assert_eq!(spent_until(&t, NOW), Some((NOW.div_euclid(SPEND_SLOT_MS) + SPEND_SLOTS + 1) * SPEND_SLOT_MS), "told until they have all aged out");
        // the hour passes: its slots go, and the hour starts again
        let later = NOW + SPEND_SLOT_MS * (SPEND_SLOTS + 1);
        assert!(!spend(&mut t, &used("lead--a1", later, &[("xai", 1)]), later));
        assert_eq!(spent_hour(&t, later), 1, "an hour on, only its own");
        assert_eq!(t.spend.len(), 1, "the older slots forgotten");
        // a call stamped past now counts in now's slot
        spend(&mut t, &used("lead--a1", later + CLOCK_SKEW_MS, &[("xai", 2)]), later);
        assert_eq!(spent_hour(&t, later), 3);
    }

    /// Goal: thirteen months are kept, and a month full of pairs takes no
    /// new one while each it holds counts on. Method: a count a year on,
    /// and a month filled to `ROWS_MAX`.
    #[test]
    fn months_and_pairs_are_bounded() {
        let mut t = Memory::default();
        count(&mut t, &used("lead--a1", NOW, &[("github", 0)]), NOW).expect("counted");
        let later = NOW + 365 * DAY_MS;
        count(&mut t, &used("lead--a1", later, &[("github", 0)]), later).expect("counted a year on");
        assert_eq!(t.rows(Month::of(NOW)), 1, "twelve months on, it is kept");
        let later = NOW + 400 * DAY_MS;
        count(&mut t, &used("lead--a1", later, &[("github", 0)]), later).expect("counted");
        assert_eq!(t.rows(Month::of(NOW)), 0, "thirteen months on, it is forgotten");
        let month = Month::of(NOW);
        let mut t = Memory::default();
        for i in 0..ROWS_MAX {
            t.add(month, &format!("a--{i}"), "github", 1, 0);
        }
        assert_eq!(count(&mut t, &used("new--b2", NOW, &[("github", 0)]), NOW), Ok(1), "a new pair is not kept");
        assert_eq!(count(&mut t, &used("a--0", NOW, &[("github", 0)]), NOW), Ok(0), "a pair held counts on");
        assert_eq!((t.rows(month), t.rows[&(month, "a--0".to_string(), "github".to_string())]), (ROWS_MAX, (2, 0)));
    }

    /// Goal: a computer's month from before the move is taken once, whole,
    /// added to what the ledger counted since: sent again (its answer lost,
    /// a restart) it adds nothing; another body under its id is refused,
    /// changing nothing. Method: one hand-over, replayed, then conflicting.
    #[test]
    fn a_hand_over_is_taken_once() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        count(&mut t, &used("lead--a1", NOW, &[("perplexity", 7_500)]), NOW).expect("counted since the move");
        let owed = TakeUses { id: "c-1:682".into(), month, rows: vec![row("lead--a1", "perplexity", 3, 22_500), row("gone--z9", "github", 2, 0)] };
        assert_eq!(take(&mut t, &owed, NOW), Ok(()));
        let once = vec![line("github", "gone--z9", 2, 0), line("perplexity", "lead--a1", 4, 30_000)];
        assert_eq!(of(&t, month), once, "added to what was counted since");
        // replay, and a restart: the same store, the same body
        let kept = t.clone();
        assert_eq!(take(&mut t, &owed, NOW), Ok(()));
        assert_eq!(take(&mut kept.clone(), &owed, NOW + DAY_MS), Ok(()));
        assert_eq!(of(&t, month), once, "taken again, nothing twice");
        let other = TakeUses { rows: vec![row("lead--a1", "perplexity", 9, 0)], ..owed.clone() };
        assert_eq!(take(&mut t, &other, NOW), Err(Refused::ConflictingBody));
        assert_eq!(t, kept, "another body changes nothing");
        // another computer's month adds
        let theirs = TakeUses { id: "c-2:682".into(), ..owed };
        assert_eq!(take(&mut t, &theirs, NOW), Ok(()));
        assert_eq!(of(&t, month), [line("github", "gone--z9", 4, 0), line("perplexity", "lead--a1", 7, 52_500)]);
    }

    /// Goal: a hand-over is refused whole when malformed, and a month past
    /// what is kept is taken and kept nowhere, so its computer forgets it.
    #[test]
    fn a_hand_over_is_checked() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        let ok = TakeUses { id: "c-1:682".into(), month, rows: vec![row("lead--a1", "github", 1, 0)] };
        for (o, why) in [
            (TakeUses { id: String::new(), ..ok.clone() }, invalid(Invalid::Id)),
            (TakeUses { rows: vec![], ..ok.clone() }, invalid(Invalid::Rows)),
            (TakeUses { rows: vec![row("lead--a1", "github", 1, 0); OWED_ROWS_MAX + 1], ..ok.clone() }, invalid(Invalid::Rows)),
            (TakeUses { rows: vec![row("lead--a1", "github", 0, 0)], ..ok.clone() }, invalid(Invalid::Id)),
            (TakeUses { rows: vec![row("lead--a1", "github", 1, -1)], ..ok.clone() }, invalid(Invalid::Id)),
        ] {
            assert_eq!(take(&mut t, &o, NOW), Err(why), "{o:?}");
        }
        assert_eq!(t, Memory::default(), "nothing taken");
        let old = TakeUses { id: "c-1:600".into(), month: Month(month.0 - MONTHS_KEPT), ..ok };
        assert_eq!(take(&mut t, &old, NOW), Ok(()));
        assert_eq!((t.rows.len(), t.taken.len()), (0, 1), "taken, kept nowhere");
    }


    /// Goal: a month's total is no single call: a computer's month whose
    /// charge for one agent at one provider is past what one call may cost
    /// (`CHARGE_MAX`) is taken whole, never refused, which its computer
    /// would forget (Codex review of #1343). Method: a month at twice it.
    #[test]
    fn a_months_total_past_one_calls_most_is_taken() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        let big = TakeUses { id: "c-1:682".into(), month, rows: vec![row("lead--a1", "elevenlabs", 30_000, 2 * CHARGE_MAX), row("lead--a1", "github", 2, 0)] };
        assert_eq!(take(&mut t, &big, NOW), Ok(()));
        assert_eq!(of(&t, month), [line("elevenlabs", "lead--a1", 30_000, 2 * CHARGE_MAX), line("github", "lead--a1", 2, 0)]);
    }

    /// Goal: what is past the months kept is read as nothing and forgotten,
    /// even for a person whose agents count nothing since: a read writes
    /// nothing, so it leaves it out; a hand-over forgets it as a count does
    /// (Codex review of #1343). Method: a month counted, read thirteen
    /// months on; then a hand-over.
    #[test]
    fn a_month_past_those_kept_is_read_as_none() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        count(&mut t, &used("lead--a1", NOW, &[("github", 0)]), NOW).expect("counted");
        let later = NOW + 400 * DAY_MS;
        assert_eq!(read(&t, month, NOW).uses.len(), 1, "kept while in the window");
        assert_eq!(read(&t, month, later).uses, vec![], "past it, none, with no count since");
        let owed = TakeUses { id: "c-1:695".into(), month: Month::of(later), rows: vec![row("lead--a1", "github", 1, 0)] };
        assert_eq!(take(&mut t, &owed, later), Ok(()));
        assert_eq!(t.rows(month), 0, "a hand-over forgets it");
    }

    /// Goal: a ledger whose agents count nothing more still forgets the
    /// months past those kept, at its sweep (Codex review of #1343, round
    /// 2). Method: a month counted, swept twelve and then thirteen months on.
    #[test]
    fn the_sweep_forgets_months_past_those_kept() {
        let mut t = Memory::default();
        let month = Month::of(NOW);
        count(&mut t, &used("lead--a1", NOW, &[("github", 0)]), NOW).expect("counted");
        sweep(&mut t, NOW + 365 * DAY_MS);
        assert_eq!(t.rows(month), 1, "twelve months on, kept");
        sweep(&mut t, NOW + 400 * DAY_MS);
        assert_eq!(t.rows(month), 0, "thirteen months on, forgotten");
    }

    /// A step-6 computer holding `rows` (month, provider, agent, calls,
    /// micros) in its `uses`, as a release before the move wrote them.
    fn at_step_6(rows: &[(u32, &str, &str, u64, i64)]) -> crate::migrate::host::Lite {
        use crate::migrate::{self, host::Lite, Ran, COMPUTER};
        let db = Lite::new();
        let six = migrate::Class { steps: &COMPUTER.steps[..6], ..COMPUTER };
        assert_eq!(db.transaction(|db| migrate::migrate(db, &six, 0)), Ok(Ran::Made { version: 6 }));
        for (month, provider, agent, calls, micros) in rows {
            db.harness(&format!("INSERT INTO uses (month, provider, agent, calls, micros) VALUES ({month}, '{provider}', '{agent}', {calls}, {micros})"));
        }
        db
    }

    /// What a computer owes, month by month, read as the cell reads it.
    fn owed_of(db: &crate::migrate::host::Lite) -> Vec<(Month, Vec<UseRow>)> {
        use crate::migrate::Db;
        let rows = db.rows("SELECT key, value FROM meta WHERE key LIKE 'uses_owed:%' ORDER BY key").expect("meta reads");
        rows.iter()
            .map(|r| {
                let month = r["key"].as_str().and_then(|k| k.strip_prefix("uses_owed:")).and_then(|m| m.parse().ok()).expect("a month's index");
                // as the ledger reads a hand-over's rows, which the computer sends as kept
                (Month(month), serde_json::from_str::<Vec<UseRow>>(r["value"].as_str().expect("text")).expect("step 7's rows read as `TakeUses.rows`"))
            })
            .collect()
    }

    /// Goal: step 7 drops the computer's `uses` and loses none of its rows:
    /// each month is owed, and the ledger that takes them holds exactly what
    /// the computer held, added to what it counted since the move, once
    /// however often a month is sent again (its answer lost, a restart).
    /// Started again, the step changes nothing; the code of the step before
    /// refuses it (rollback). Method: a step-6 computer holding two agents'
    /// rows over three months, migrated by this code, then by the step
    /// before; its months handed to a ledger, each twice.
    #[test]
    fn step_7_owes_every_row_and_the_ledger_takes_each_once() {
        use crate::migrate::{self, Ran, SchemaError, COMPUTER};
        let newest = COMPUTER.steps.len() as u32;
        assert_eq!(COMPUTER.steps[6].name, "uses_owed", "the drop's step is step 7");
        let (this, last, before) = (Month::of(NOW).0, Month::of(NOW).0 - 1, Month::of(NOW).0 - 12);
        let held = [
            (this, "github", "lead--a1", 3, 0),
            (this, "perplexity", "lead--a1", 2, 15_000),
            (this, "perplexity", "gone--z9", 1, 7_500),
            (last, "elevenlabs", "lead--a1", 4, 90_000),
            (before, "github", "lead--a1", 9, 0),
        ];
        let db = at_step_6(&held);
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 100)), Ok(Ran::Migrated { from: 6, version: newest }));
        assert_eq!(db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'uses'"), 0, "the table is gone");
        let owed = owed_of(&db);
        let months: Vec<Month> = owed.iter().map(|(m, _)| *m).collect();
        assert_eq!(months, [Month(before), Month(last), Month(this)], "each month owed");
        let mut flat: Vec<(u32, String, String, u64, i64)> = owed.iter().flat_map(|(m, rows)| rows.iter().map(|r| (m.0, r.provider.clone(), r.agent.clone(), r.calls, r.micros))).collect();
        flat.sort();
        let mut expect: Vec<(u32, String, String, u64, i64)> = held.iter().map(|(m, p, a, c, x)| (*m, p.to_string(), a.to_string(), *c, *x)).collect();
        expect.sort();
        assert_eq!(flat, expect, "every row owed, as it was held");
        // replay and restart of the step
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 200)), Ok(Ran::Current { version: newest }));
        assert_eq!(owed_of(&db), owed, "started again, it owes the same");
        // rollback
        let six = migrate::Class { steps: &COMPUTER.steps[..6], ..COMPUTER };
        assert_eq!(db.transaction(|db| migrate::migrate(db, &six, 300)), Err(SchemaError::Newer { class: "Computer", held: newest, known: 6 }), "the release before refuses it");
        // the ledger counted a call since the move; each month taken twice
        let mut t = Memory::default();
        count(&mut t, &used("lead--a1", NOW, &[("perplexity", 7_500)]), NOW).expect("counted since the move");
        for _ in 0..2 {
            for (month, rows) in &owed {
                assert_eq!(take(&mut t, &TakeUses { id: format!("c-1:{}", month.0), month: *month, rows: rows.clone() }, NOW), Ok(()));
            }
        }
        assert_eq!(
            of(&t, Month(this)),
            [line("github", "lead--a1", 3, 0), line("perplexity", "gone--z9", 1, 7_500), line("perplexity", "lead--a1", 3, 22_500)],
            "this month: the computer's rows and the call since, once"
        );
        assert_eq!(of(&t, Month(last)), [line("elevenlabs", "lead--a1", 4, 90_000)]);
        assert_eq!(of(&t, Month(before)), [line("github", "lead--a1", 9, 0)], "the oldest month kept");
        // a wipe makes its life again, as step 7 left it
        db.transaction(|db| migrate::remake(db, &COMPUTER)).expect("remade");
        assert_eq!((db.count("SELECT COUNT(*) FROM sqlite_master WHERE name = 'uses'"), db.count("SELECT COUNT(*) FROM meta")), (0, 0));
    }

    /// Goal: a computer that held no uses owes none and keeps no row.
    #[test]
    fn nothing_held_owes_nothing() {
        use crate::migrate::{self, Ran, COMPUTER};
        let db = at_step_6(&[]);
        assert_eq!(db.transaction(|db| migrate::migrate(db, &COMPUTER, 100)), Ok(Ran::Migrated { from: 6, version: COMPUTER.steps.len() as u32 }));
        assert_eq!(db.count("SELECT COUNT(*) FROM meta"), 0);
    }

    /// Goal: a ledger from before the move gains its uses table, empty, and
    /// the code before refuses it. Method: a step-1 ledger, migrated.
    #[test]
    fn ledger_step_2_makes_the_tally() {
        use crate::migrate::{self, host::Lite, Ran, SchemaError, LEDGER};
        let db = Lite::new();
        let one = migrate::Class { steps: &LEDGER.steps[..1], ..LEDGER };
        assert_eq!(db.transaction(|db| migrate::migrate(db, &one, 0)), Ok(Ran::Made { version: 1 }));
        let newest = LEDGER.steps.len() as u32;
        assert_eq!(db.transaction(|db| migrate::migrate(db, &LEDGER, 100)), Ok(Ran::Migrated { from: 1, version: newest }));
        assert_eq!(db.columns("uses"), ["month", "agent", "provider", "calls", "micros"]);
        assert_eq!(db.transaction(|db| migrate::migrate(db, &one, 200)), Err(SchemaError::Newer { class: "Ledger", held: newest, known: 1 }));
    }

    /// FIN-616: a ledger gains the hour's spend (step 3), empty; a computer
    /// drops its key hour and its caps (step 8), the rows in them gone;
    /// each step again changes nothing, and the release before refuses it.
    /// Method: a step-2 ledger and a step-7 computer holding a key hour.
    #[test]
    fn the_hour_moves_from_the_computer_to_the_ledger() {
        use crate::migrate::{self, host::Lite, Ran, SchemaError, COMPUTER, LEDGER};
        let ledger = Lite::new();
        let two = migrate::Class { steps: &LEDGER.steps[..2], ..LEDGER };
        assert_eq!(ledger.transaction(|db| migrate::migrate(db, &two, 0)), Ok(Ran::Made { version: 2 }));
        assert_eq!(ledger.transaction(|db| migrate::migrate(db, &LEDGER, 100)), Ok(Ran::Migrated { from: 2, version: 3 }));
        assert_eq!(ledger.columns("spend_slots"), ["slot", "micros"]);
        assert_eq!(ledger.transaction(|db| migrate::migrate(db, &LEDGER, 200)), Ok(Ran::Current { version: 3 }));
        assert_eq!(ledger.transaction(|db| migrate::migrate(db, &two, 300)), Err(SchemaError::Newer { class: "Ledger", held: 3, known: 2 }));
        let computer = Lite::new();
        let seven = migrate::Class { steps: &COMPUTER.steps[..7], ..COMPUTER };
        assert_eq!(computer.transaction(|db| migrate::migrate(db, &seven, 0)), Ok(Ran::Made { version: 7 }));
        computer.harness("INSERT INTO key_hour (provider, slot, micros) VALUES ('elevenlabs', 1, 5000000)");
        computer.harness("INSERT INTO key_caps (provider, micros) VALUES ('elevenlabs', 20000000)");
        assert_eq!(computer.transaction(|db| migrate::migrate(db, &COMPUTER, 100)), Ok(Ran::Migrated { from: 7, version: 8 }));
        assert_eq!(computer.count("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('key_hour', 'key_caps')"), 0, "both gone");
        assert_eq!(computer.transaction(|db| migrate::migrate(db, &COMPUTER, 200)), Ok(Ran::Current { version: 8 }));
        assert_eq!(computer.transaction(|db| migrate::migrate(db, &seven, 300)), Err(SchemaError::Newer { class: "Computer", held: 8, known: 7 }));
    }
}
