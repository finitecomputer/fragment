//! An org's ledger (docs/cloudflare-v1.md, decisions 66 to 68): credit
//! an operator gave it (orgs pay out of band: Paul, 2026-10-30), which pays
//! for the fragments it owns: their hosting, storage and AI steps. It has
//! none of a person's plan, seat, included credit or overdraft, and pays
//! for no agent's turn and no computer: a person's agents bill that
//! person, in an org's fragment too (decision 67).
//!
//! Its overdraft is zero: at a balance of zero its fragments are
//! read-only until credit arrives. An operator sets its monthly budget,
//! zero until they do; once its fragments' charges this month
//! reach it, their AI steps stop until a larger one or the next month
//! (hosting is still metered: a budget stops what starts, as a cap does).
//! Every AI step in its fragments meets the fragment's cap as well: an
//! org's fragment has no owner who passes it (decision 68).
//!
//! The machine is a person's: grants, meters, caps and the store are
//! shared. Its terms (`OrgTerms`) are never in the head: that it is an
//! org's is its ledger's name, which never changes, and its budget is a row
//! of its own, so a release rolled back to one that knows no org, which
//! reads and rewrites the head on its sweep, loses neither.

use fragment_proto::ledger::OrgLedgerStatus;

use super::*;

/// An org's ledger is named `org:<id>`; a person's, by their identity.
pub const ORG_LEDGER_PREFIX: &str = "org:";
/// A monthly budget: more is a typo.
pub const BUDGET_MAX: i64 = 100_000 * USD;

/// Why a budget's body is refused, whatever its ledger holds (the router
/// asks first, as for a grant: `grant_invalid`).
pub fn budget_invalid(c: &OrgBudget) -> Option<Refused> {
    (!(0..=BUDGET_MAX).contains(&c.micros)).then(|| invalid(Invalid::Amount))
}

/// Who pays, as their ledger is named (`name`, `parse`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payer {
    /// A person, by their identity (an npub).
    Person(String),
    /// An org, by its id (`org-` and 16 hex).
    Org(String),
}

impl Payer {
    /// Its ledger's name: the person's identity, or `org:<id>`.
    pub fn name(&self) -> String {
        match self {
            Payer::Person(id) => id.clone(),
            Payer::Org(id) => format!("{ORG_LEDGER_PREFIX}{id}"),
        }
    }

    /// The payer a ledger's name names, when it is a valid one.
    pub fn parse(name: &str) -> Option<Payer> {
        match name.strip_prefix(ORG_LEDGER_PREFIX) {
            Some(id) => crate::org::valid_org_id(id).then(|| Payer::Org(id.to_string())),
            None => crate::npub::is_identity(name).then(|| Payer::Person(name.to_string())),
        }
    }
}

/// An org's terms beside its money, kept apart from the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OrgTerms {
    /// What its fragments may be charged a month before their AI steps
    /// stop: zero until an operator sets one.
    budget: i64,
}

impl OrgTerms {
    /// Its terms as kept: the budget's row (none: zero), held to today's
    /// limit (a budget set under a larger one is read as the limit, never
    /// a panic on every call).
    pub fn of(budget: i64) -> OrgTerms {
        OrgTerms { budget: budget.clamp(0, BUDGET_MAX) }
    }

    pub fn budget(&self) -> i64 {
        self.budget
    }

    /// `Ledger::may_spend` on an org's ledger, its gate passed: an AI step
    /// meets the budget, then its fragment's cap, whoever starts it.
    pub(super) fn may_spend(&self, ledger: &Ledger, store: &impl Store, spend: Spend, fragment: Option<&str>, now_ms: i64) -> Result<(), Refused> {
        if spend != Spend::AiStep {
            return Ok(());
        }
        let spent = store.month_spent(ledger.money_at(now_ms).month);
        if spent >= self.budget {
            return Err(Refused::BudgetReached { budget: self.budget, spent });
        }
        match fragment {
            Some(f) => ledger.fragment_open(store, f, now_ms),
            None => Ok(()),
        }
    }
}

/// An org's standing: its money's, as a person's with an active seat (it
/// is never a guest's, and agents are none of its).
pub(super) fn standing(money: &Money) -> Standing {
    if money.read_only {
        Standing::ReadOnly { why: Why::Overdrawn }
    } else if money.balance() > 0 {
        Standing::Ok
    } else {
        Standing::AgentsStopped { why: Why::NoCredit }
    }
}

/// Whether `spend` may start on an org's money: never an agent's turn or
/// a computer's wake (decision 67).
pub(super) fn gate(money: &Money, spend: Spend) -> Result<(), Refused> {
    match spend {
        Spend::AgentTurn | Spend::Wake => Err(Refused::PersonsOnly),
        Spend::AiStep | Spend::Write | Spend::Create => allowed(standing(money), spend),
    }
}

/// Whether a meter row is one an org pays: a fragment's, never a
/// computer's time. A row may name the agent that started its fragment's
/// own AI step (an app's step is its fragment's, whoever starts it); an
/// agent's own turns bill its person and are never sent here.
pub(super) fn orgs_row(row: &MeterRow) -> bool {
    row.fragment.is_some() && row.computer.is_none() && !matches!(row.usage, Usage::Awake { .. })
}

/// An org's money: a guest's plan that never changes, no seat order, no
/// overdraft, and nothing included.
pub(super) fn assert_valid(money: &Money) {
    assert_eq!(money.plan, Plan::Guest, "an org has no plan");
    assert_eq!(money.seat_seq, 0, "an org has no seat");
    assert_eq!(money.overdraft, 0, "an org's overdraft is zero");
    assert_eq!(money.totals.included, 0, "an org's credit is never included");
}

impl Ledger {
    /// A new org's ledger: no credit, so read-only until some arrives, and
    /// a budget of zero.
    pub fn new_org(now_ms: i64) -> Ledger {
        let mut ledger = Ledger::new(now_ms);
        ledger.money.overdraft = 0;
        ledger.money.latch();
        ledger.bind_org(OrgTerms::default());
        ledger
    }

    /// A head read back as an org's (its ledger's name says so), with its
    /// terms as kept: its money must be an org's, never a person's.
    pub fn bind_org(&mut self, terms: OrgTerms) {
        self.org = Some(terms);
        self.assert_valid();
    }

    /// Whether this is an org's ledger.
    pub fn is_org(&self) -> bool {
        self.org.is_some()
    }

    /// Its terms, to keep beside the head (none: a person's).
    pub fn org_terms(&self) -> Option<OrgTerms> {
        self.org
    }

    pub(super) fn persons_only(&self) -> Result<(), Refused> {
        match self.org {
            Some(_) => Err(Refused::PersonsOnly),
            None => Ok(()),
        }
    }

    /// An org's monthly budget, from an operator.
    pub fn set_budget(&mut self, store: &mut impl Store, c: &OrgBudget, now_ms: i64) -> Result<(), Refused> {
        if self.org.is_none() {
            return Err(Refused::OrgsOnly);
        }
        self.roll(now_ms);
        let command = Command::OrgBudget(c.clone());
        if !Self::admit(store, &c.id, &command)? {
            return Ok(());
        }
        if let Some(why) = budget_invalid(c) {
            return Err(why);
        }
        self.org = Some(OrgTerms { budget: c.micros });
        self.done(store, &c.id, command, now_ms);
        Ok(())
    }

    /// An org's ledger now.
    pub fn org_status(&self, org: &str, store: &impl Store, now_ms: i64, book: &PriceBook) -> Result<OrgLedgerStatus, Refused> {
        let terms = self.org.ok_or(Refused::OrgsOnly)?;
        let personal = self.status(store, now_ms, book);
        let money = self.money_at(now_ms);
        Ok(OrgLedgerStatus {
            org: org.to_string(),
            month: personal.month,
            balance_micros: money.balance(),
            budget_micros: terms.budget,
            spent_micros: store.month_spent(money.month),
            standing: standing(&money),
            price_book: book.version,
            fragments: personal.fragments,
        })
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::ledger::GrantCredit;

    use super::*;

    /// 2026-10-01T00:00:00Z.
    const T0: i64 = 1_790_812_800_000;
    const ORG: &str = "org-0123456789abcdef";
    const APP: &str = "board--0a1b";

    fn pack(session: &str) -> GrantCredit {
        GrantCredit { id: format!("pack:{session}"), micros: 25 * USD, by: "stripe".into(), why: "a $25 credit pack".into() }
    }

    fn budget(id: &str, micros: i64) -> OrgBudget {
        OrgBudget { id: id.into(), micros }
    }

    fn step_row(reference: &str, fragment: &str, at_ms: i64) -> MeterRow {
        MeterRow { reference: reference.into(), usage: Usage::Neurons { milli: 100_000_000 }, fragment: Some(fragment.into()), agent: None, computer: None, at_ms }
    }

    /// The head and store written and read back, as the Durable Object's
    /// are across a restart: an org's bound again from its terms as kept.
    fn restarted(ledger: &Ledger, store: &Memory) -> (Ledger, Memory) {
        let mut head: Ledger = serde_json::from_str(&ledger.written(&PriceBook::defaults())).unwrap();
        if let Some(terms) = ledger.org_terms() {
            head.bind_org(OrgTerms::of(terms.budget()));
        }
        let kept: Memory = serde_json::from_str(&serde_json::to_string(store).unwrap()).unwrap();
        assert_eq!((&head, &kept), (ledger, store), "the state reads back as it was written");
        (head, kept)
    }

    /// A rollback (Alex's and the review's M1): a release that knows no
    /// org reads an org's head and rewrites it on its sweep, keeping only
    /// the fields it knows. Rolled forward, the ledger is the org's still,
    /// its budget and money whole: neither was in the head.
    #[test]
    fn org_terms_survive_an_older_release_sweep() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        l.grant(&mut s, &pack("cs_1"), T0).unwrap();
        l.set_budget(&mut s, &budget("b1", 10 * USD), T0).unwrap();
        let book = PriceBook::defaults();
        let kept_budget = l.org_terms().unwrap().budget();
        let written: serde_json::Value = serde_json::from_str(&l.written(&book)).unwrap();
        assert!(written.get("org").is_none(), "the head names nothing of an org's: {written}");
        // the older release's head: the fields it knows, rewritten whole
        let old: serde_json::Value = serde_json::json!({ "money": written["money"], "holds": written["holds"], "book": written["book"] });
        let mut forward: Ledger = serde_json::from_value(old).unwrap();
        forward.sweep(&mut s, T0 + 2 * DAY_MS);
        let mut forward: Ledger = serde_json::from_str(&forward.written(&book)).unwrap();
        forward.bind_org(OrgTerms::of(kept_budget));
        let status = forward.org_status(ORG, &s, T0 + 2 * DAY_MS, &book).unwrap();
        assert_eq!((status.balance_micros, status.budget_micros, status.standing), (25 * USD, 10 * USD, Standing::Ok));
        assert_eq!(forward.gate(Spend::Write, T0 + 2 * DAY_MS), Ok(()));
    }

    #[test]
    fn payers_are_named_as_their_ledgers() {
        let person = crate::npub::encode(&"ab".repeat(32));
        assert_eq!(Payer::parse(&person), Some(Payer::Person(person.clone())));
        assert_eq!(Payer::parse(&format!("org:{ORG}")), Some(Payer::Org(ORG.into())));
        assert_eq!(Payer::Org(ORG.into()).name(), format!("org:{ORG}"));
        for bad in ["org:", "org:org-xyz", "org:npub1", ORG, "", "someone"] {
            assert_eq!(Payer::parse(bad), None, "{bad:?} names no ledger");
        }
    }

    /// Valid: a pack credits the org's ledger, which has no plan, seat,
    /// included credit or overdraft, and is writable only while funded.
    #[test]
    fn org_pack_credits_org() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        assert_eq!(l.standing(T0), Standing::ReadOnly { why: Why::Overdrawn }, "unfunded: read-only, no overdraft");
        assert_eq!(l.gate(Spend::Create, T0), Err(Refused::ReadOnly { why: Why::Overdrawn }));
        l.grant(&mut s, &pack("cs_1"), T0).unwrap();
        let status = l.org_status(ORG, &s, T0, &PriceBook::defaults()).unwrap();
        assert_eq!((status.balance_micros, status.budget_micros, status.spent_micros, status.standing), (25 * USD, 0, 0, Standing::Ok));
        assert_eq!(l.gate(Spend::Write, T0), Ok(()));
        // a month on, nothing is included and nothing expires
        let november = T0 + 31 * DAY_MS;
        assert_eq!(l.org_status(ORG, &s, november, &PriceBook::defaults()).unwrap().balance_micros, 25 * USD);
        assert_eq!(l.totals().included, 0);
        // a person's own status is not an org's
        assert_eq!(Ledger::new(T0).org_status(ORG, &s, T0, &PriceBook::defaults()), Err(Refused::OrgsOnly));
    }

    /// Invalid: an org's credit pays for nothing of a person's, and a
    /// person's ledger takes no budget.
    #[test]
    fn org_credit_pays_nothing_personal() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        l.grant(&mut s, &pack("cs_1"), T0).unwrap();
        let before = (l.clone(), s.clone());
        assert_eq!(l.gate(Spend::AgentTurn, T0), Err(Refused::PersonsOnly));
        assert_eq!(l.gate(Spend::Wake, T0), Err(Refused::PersonsOnly));
        assert_eq!(l.set_plan(&mut s, &SetPlan { id: "p".into(), plan: Plan::SeatAlwaysOn }, T0), Err(Refused::PersonsOnly));
        assert_eq!(l.set_seat(&mut s, &SetSeat { id: "s".into(), seat: SeatState::Active, seq: 1 }, T0), Err(Refused::PersonsOnly));
        assert_eq!(l.set_overdraft(&mut s, &SetOverdraft { id: "o".into(), micros: USD }, T0), Err(Refused::PersonsOnly));
        assert_eq!((&l, &s), (&before.0, &before.1), "a refusal changes nothing");
        let book = PriceBook::defaults();
        let rows = vec![
            MeterRow { computer: Some("cmp-1".into()), usage: Usage::Awake { instance: "standard-1".into(), ms: 60_000 }, fragment: None, ..step_row("awake:1", APP, T0) },
            MeterRow { computer: Some("cmp-1".into()), ..step_row("key:1", APP, T0) },
            MeterRow { fragment: None, ..step_row("aig:1", APP, T0) },
        ];
        let answer = l.meter(&mut s, &Meter { batch: "frag:1".into(), rows }, T0, &book).unwrap();
        assert_eq!((answer.charged, answer.refused.iter().map(|r| r.why).collect::<Vec<_>>()), (0, vec![Refused::PersonsOnly; 3]));
        // an app's own AI step an agent started is the app's: the org pays it
        let started = MeterRow { agent: Some("npub1agent".into()), ..step_row("step:by-agent", APP, T0) };
        let answer = l.meter(&mut s, &Meter { batch: "frag:2".into(), rows: vec![started] }, T0, &book).unwrap();
        assert!(answer.charged > 0 && answer.refused.is_empty(), "{answer:?}");
        let (mut person, mut ps) = (Ledger::new(T0), Memory::default());
        assert_eq!(person.set_budget(&mut ps, &budget("b", USD), T0), Err(Refused::OrgsOnly));
        assert_eq!(l.set_budget(&mut s, &budget("too-much", BUDGET_MAX + 1), T0), Err(Refused::Invalid { what: Invalid::Amount }));
        assert_eq!(l.set_budget(&mut s, &budget("negative", -1), T0), Err(Refused::Invalid { what: Invalid::Amount }));
    }

    /// The router's check of a grant's or budget's body (an org's is in
    /// its log before its ledger takes it) refuses just what the ledger
    /// does: the log never names a command its ledger refused.
    #[test]
    fn the_routers_check_is_the_ledgers() {
        let grants = [
            GrantCredit { micros: USD, ..pack("ok") },
            GrantCredit { micros: 0, ..pack("zero") },
            GrantCredit { micros: GRANT_MAX + 1, ..pack("too-much") },
            GrantCredit { by: "a by with spaces".into(), ..pack("by") },
            GrantCredit { why: "x".repeat(NOTE_MAX_BYTES + 1), ..pack("why") },
        ];
        for g in &grants {
            let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
            assert_eq!(grant_invalid(g), l.grant(&mut s, g, T0).err(), "{}", g.id);
        }
        for c in [budget("ok", USD), budget("none", 0), budget("negative", -1), budget("too-much", BUDGET_MAX + 1)] {
            let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
            assert_eq!(budget_invalid(&c), l.set_budget(&mut s, &c, T0).err(), "{}", c.id);
        }
    }

    /// The budget is explicit, sums every fragment, stops AI steps and not
    /// writes, and the cap meets every caller: no owner passes it.
    #[test]
    fn org_admin_cannot_bypass_budget_or_cap() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        l.grant(&mut s, &GrantCredit { micros: 500 * USD, ..pack("cs_1") }, T0).unwrap();
        assert_eq!(l.may_spend(&s, Spend::AiStep, Some(APP), true, T0), Err(Refused::BudgetReached { budget: 0, spent: 0 }), "zero until set");
        l.set_budget(&mut s, &budget("b1", 10 * USD), T0).unwrap();
        assert_eq!(l.may_spend(&s, Spend::AiStep, Some(APP), true, T0), Ok(()));
        s.add_spent(Month::of(T0), APP, CAP_DEFAULT);
        assert_eq!(l.may_spend(&s, Spend::AiStep, Some(APP), true, T0), Err(Refused::CapReached { cap: CAP_DEFAULT, spent: CAP_DEFAULT }), "by_owner passes no org cap");
        assert_eq!(l.may_spend(&s, Spend::AiStep, Some("other--0a1b"), true, T0), Ok(()));
        for n in 0..3 {
            s.add_spent(Month::of(T0), &format!("f{n}--0a1b"), 2 * USD);
        }
        assert_eq!(
            l.may_spend(&s, Spend::AiStep, Some("fresh--0a1b"), true, T0),
            Err(Refused::BudgetReached { budget: 10 * USD, spent: 11 * USD }),
            "the budget is every fragment's spend, past the status's list"
        );
        assert_eq!(l.may_spend(&s, Spend::Write, Some("fresh--0a1b"), true, T0), Ok(()), "a budget stops AI steps, not writes");
        let november = T0 + 31 * DAY_MS;
        assert_eq!(l.may_spend(&s, Spend::AiStep, Some(APP), true, november), Ok(()), "a new month, a new budget");
    }

    /// Replay: a pack's grant is once by its session, a budget once by its
    /// id; another body under either id is refused.
    #[test]
    fn pack_session_granted_once() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        l.grant(&mut s, &pack("cs_1"), T0).unwrap();
        let once = (l.clone(), s.clone());
        l.grant(&mut s, &pack("cs_1"), T0 + 1).unwrap();
        assert_eq!((&l, &s), (&once.0, &once.1), "the webhook and the return are one grant");
        assert_eq!(l.grant(&mut s, &GrantCredit { micros: 26 * USD, ..pack("cs_1") }, T0), Err(Refused::ConflictingBody));
        l.set_budget(&mut s, &budget("b1", 10 * USD), T0).unwrap();
        l.set_budget(&mut s, &budget("b1", 10 * USD), T0).unwrap();
        assert_eq!(l.set_budget(&mut s, &budget("b1", 20 * USD), T0), Err(Refused::ConflictingBody));
        assert_eq!(l.org_status(ORG, &s, T0, &PriceBook::defaults()).unwrap().budget_micros, 10 * USD);
    }

    /// Restart: the org's head reads back, bound as an org's from what is
    /// kept beside it (a person's head is written as before), and a pack
    /// paid after it is still granted once.
    #[test]
    fn paid_pack_survives_restart() {
        let (mut l, mut s) = (Ledger::new_org(T0), Memory::default());
        l.grant(&mut s, &pack("cs_1"), T0).unwrap();
        l.set_budget(&mut s, &budget("b1", 10 * USD), T0).unwrap();
        let book = PriceBook::defaults();
        let rows = vec![step_row("step:a", APP, T0)];
        l.meter(&mut s, &Meter { batch: "frag:1".into(), rows: rows.clone() }, T0, &book).unwrap();
        let (mut l, mut s) = restarted(&l, &s);
        assert!(l.is_org());
        l.grant(&mut s, &pack("cs_1"), T0 + 2 * DAY_MS).unwrap();
        l.grant(&mut s, &pack("cs_2"), T0 + 2 * DAY_MS).unwrap();
        let replayed = l.meter(&mut s, &Meter { batch: "frag:1".into(), rows }, T0 + 2 * DAY_MS, &book).unwrap();
        assert_eq!(replayed.rows_before + replayed.rows_new, 1, "the batch answers as it did");
        let status = l.org_status(ORG, &s, T0 + 2 * DAY_MS, &book).unwrap();
        assert_eq!((status.budget_micros, status.balance_micros + status.spent_micros), (10 * USD, 50 * USD));
        assert!(!serde_json::to_string(&Ledger::new(T0)).unwrap().contains("org"), "a person's head is written as before");
        assert_eq!((OrgTerms::of(BUDGET_MAX + 1).budget(), OrgTerms::of(-1).budget()), (BUDGET_MAX, 0), "a budget kept past today's limit reads as the limit");
    }
}
