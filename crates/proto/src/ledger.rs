//! The usage ledger's public wire types (docs/ledger.md): what the shell
//! and the CLI read (`LedgerStatus`, an org's `OrgLedgerStatus`) and what
//! operators, Stripe's hooks, fragment owners and an org's billing admins
//! send (credit, plans, seats, overdrafts, caps, an org's budget).
//! Money is integer micro-dollars. The ledger's state machine is
//! `fragment_core::ledger`; the platform's own inner routes (meter, the
//! gate's questions) are its types, not these.

use serde::{Deserialize, Serialize};

/// A person's plan (docs/cloudflare-v1.md, decision 25). A guest signs in
/// free and uses what is shared with them: no agents, no AI, no computer,
/// and nothing billed to them. A seat includes credit each month; the
/// $200 seat's computer is always on, its awake time not metered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    Guest,
    /// $50 of credit included, a computer that sleeps: an operator's comp,
    /// or a deployment's default plan; not sold (Paul, 2026-10-15).
    Seat,
    /// $200 a month, $100 of credit included, an always-on computer: the
    /// one seat sold.
    SeatAlwaysOn,
}

/// A seat's state, as its payment provider reports it (Stripe, later).
/// It means nothing on a guest's ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatState {
    Active,
    /// A payment failed and is being retried: the seat keeps this month's
    /// credit, and next month's waits until it is active again.
    PastDue,
    /// The seat ended: agents stop; fragments keep serving on credit.
    Canceled,
}

/// What a person's money still lets happen (decision 27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "standing", rename_all = "snake_case")]
pub enum Standing {
    /// Agents run, AI steps run, fragments take writes.
    Ok,
    /// No agent turns, no wakes, no AI steps; fragments still take writes.
    AgentsStopped { why: Why },
    /// The person's fragments serve but take no writes, and agents stop.
    ReadOnly { why: Why },
}

/// Why a standing is short of `Ok`, for the shell to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// A guest has no agents.
    Guest,
    /// The seat was canceled.
    SeatCanceled,
    /// The balance is zero or less.
    NoCredit,
    /// The balance reached the overdraft, and has not been above zero since.
    Overdrawn,
}

/// One fragment's spend this month on its owner's ledger, and its cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentSpend {
    pub fragment: String,
    pub spent_micros: i64,
    pub cap_micros: i64,
}

/// A person's ledger now (`GET /api/ledger`). The balance is the included
/// credit left this month plus the purchased credit left (negative while
/// the person owes). Its fields are the wire a shipped CLI parses: none
/// goes, and none changes its type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerStatus {
    pub plan: Plan,
    pub seat: SeatState,
    /// The UTC month, `YYYY-MM`.
    pub month: String,
    pub balance_micros: i64,
    pub included_micros: i64,
    /// This month's included credit, as granted (it expires at the month's end).
    pub included_granted_micros: i64,
    pub purchased_micros: i64,
    /// What calls running now hold: always 0 since nothing is held before
    /// a call (decision 62); kept for the CLIs and pages that read it.
    pub reserved_micros: i64,
    /// What may still be charged before the person's fragments go
    /// read-only: the balance plus the overdraft's room (0 once
    /// read-only). Agents stop at zero, before it.
    pub available_micros: i64,
    pub overdraft_micros: i64,
    pub standing: Standing,
    /// The price book's version this ledger charges with.
    pub price_book: u32,
    /// This month's spend by fragment, the largest first (a bounded list).
    pub fragments: Vec<FragmentSpend>,
}

/// Credit granted: bought (Stripe's hook), or given by an operator. Once
/// per `id` (Stripe's payment id, or the operator's command id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantCredit {
    pub id: String,
    pub micros: i64,
    /// Who granted it: an operator's identity, or `stripe`.
    pub by: String,
    /// Why, for people reading the ledger later.
    pub why: String,
}

/// A person's plan, set by an operator (and by Stripe's hook, later).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetPlan {
    pub id: String,
    pub plan: Plan,
}

/// A seat's state, from its payment provider's hook. Hooks arrive out of
/// order, so each names its place in the seat's own order (`seq`, from 1:
/// Stripe's event time in ms, later), and a change older than the last
/// one applied changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSeat {
    pub id: String,
    pub seat: SeatState,
    pub seq: u64,
}

/// How far below zero a person's fragments keep taking writes (an
/// operator's; $2 until set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetOverdraft {
    pub id: String,
    pub micros: i64,
}

/// A fragment's monthly cap, set by its owner on the owner's ledger
/// (decision 26). `None` goes back to the default ($5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetFragmentCap {
    pub id: String,
    pub fragment: String,
    pub micros: Option<i64>,
}

/// `PUT /api/f/<name>/cap` (the fragment's owner): its monthly cap, in
/// micro-dollars, or `None` for the default. The path names the fragment;
/// `id` makes the change once (the same id again changes nothing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutCap {
    pub id: String,
    pub micros: Option<i64>,
}

/// `POST /api/ledger/org:<org>/budget` (an operator's): what the org's
/// fragments may spend a month before their AI steps stop (decision 68),
/// in micro-dollars; zero until set. `id` makes the change once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgBudget {
    pub id: String,
    pub micros: i64,
}

/// An org's ledger now (`GET /api/orgs/<org>/ledger`; decision 66): the
/// credit operators gave it, with no plan, seat, included credit or
/// overdraft, and the month's budget beside what its fragments spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgLedgerStatus {
    pub org: String,
    /// The UTC month, `YYYY-MM`.
    pub month: String,
    pub balance_micros: i64,
    pub budget_micros: i64,
    /// What its fragments were charged this month, all of them.
    pub spent_micros: i64,
    pub standing: Standing,
    pub price_book: u32,
    /// This month's spend by fragment, the largest first (a bounded list).
    pub fragments: Vec<FragmentSpend>,
}

/// The most items one quote prices, and the most calls one item counts:
/// a plan an agent describes in one sentence, and a loop's whole run. Its
/// total is bounded below `i64::MAX` with the price book's largest row
/// (`fragment_core::price::CHARGE_MAX`; held there).
pub const QUOTE_ITEMS_MAX: usize = 64;
pub const QUOTE_CALLS_MAX: u64 = 1_000_000;

/// `POST /api/ledger/quote` (FIN-448): what a job plans to use, each item
/// priced as the ledger would charge it, and the payer's ledger beside it.
/// It reads and changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteAsk {
    pub items: Vec<Planned>,
}

/// One planned use: `calls` calls (absent, one), each using what it says.
/// Told apart by the field it names: `key`, `model` or `usage`. Read as
/// its fields (`PlannedFields`), not as serde's untagged reading, which
/// buffers each item to try every kind: about 6 KB more of the cell's Wasm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, try_from = "PlannedFields")]
pub enum Planned {
    Key(PlannedKey),
    Model(PlannedModel),
    Usage(PlannedUsage),
}

/// A `Planned` as it is read: every kind's fields, flat, none unknown.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlannedFields {
    key: Option<String>,
    units: Option<u64>,
    model: Option<String>,
    input: Option<u64>,
    cached_input: Option<u64>,
    cache_write: Option<u64>,
    output: Option<u64>,
    usage: Option<serde_json::Value>,
    calls: Option<u64>,
}

impl TryFrom<PlannedFields> for Planned {
    type Error = String;

    /// The kind its one name says, with that kind's fields alone.
    fn try_from(f: PlannedFields) -> Result<Planned, String> {
        let tokens = [f.input, f.cached_input, f.cache_write, f.output];
        let calls = f.calls;
        match (f.key, f.model, f.usage) {
            (Some(key), None, None) if tokens.iter().all(Option::is_none) => Ok(Planned::Key(PlannedKey { key, units: f.units, calls })),
            (None, Some(model), None) if f.units.is_none() => {
                let [input, cached_input, cache_write, output] = tokens.map(|t| t.unwrap_or(0));
                Ok(Planned::Model(PlannedModel { model, input, cached_input, cache_write, output, calls }))
            }
            (None, None, Some(usage)) if f.units.is_none() && tokens.iter().all(Option::is_none) => Ok(Planned::Usage(PlannedUsage { usage, calls })),
            _ => Err("an item names one of `key` (with `units`), `model` (with its tokens) or `usage`, and `calls`".into()),
        }
    }
}

/// Calls on an operator key, or on a part of its calls the book prices
/// apart (`elevenlabs:speech`), by the book's name for it
/// (`fragment_core::price::KEY_PARTS`). `units` are each call's, in the
/// key's own unit: a part's what its vendor counts (ElevenLabs' speech, its
/// text's characters; fal's image, its pixels), which its item must say,
/// unless the part counts at most one a call (xAI's image, a Places
/// photo); any other key's a call (absent, one). A part's are not cut to
/// one call's most, so a job's whole count may be one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedKey {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<u64>,
}

/// Calls on a model, by a name the model route takes (`cheap`, `medium`,
/// `vision`, `fallback`, or a slot's model's id) or the book's id for it
/// (a decision's `@cf/…/clef-flash`), each with these tokens (absent,
/// none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedModel {
    pub model: String,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub cached_input: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<u64>,
}

/// Anything else the book prices, as a meter counts it: one ledger row's
/// usage (`fragment_core::price::Usage`, as the ledger queue carries it:
/// `{"kind": "awake", "instance": "2vcpu-6gib", "ms": 3600000}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedUsage {
    pub usage: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<u64>,
}

/// The answer: each item's charge, in the order asked, their total, and
/// the payer's ledger as `GET /api/ledger` reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub items: Vec<Quoted>,
    /// The priced items' charges together; an unknown item adds nothing.
    pub total_micros: i64,
    /// What may still be spent before the payer's agents stop (decision
    /// 27): the balance (this month's included credit left and the
    /// purchased credit left), never below zero.
    pub left_micros: i64,
    pub ledger: LedgerStatus,
}

/// One item's quote: its charge, or why it has none (never a guess).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quoted {
    /// What the book names it: a key's or a part's name, a model's id (a
    /// route's name resolved), or a usage's kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_as: Option<String>,
    /// One call's charge, as the ledger charges its row: list × (1 + fee)
    /// × (1 + margin), rounded up once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each_micros: Option<i64>,
    pub calls: u64,
    /// `calls` × `eachMicros`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<Unquoted>,
}

/// Why an item has no charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unquoted {
    /// The book prices nothing of its name here (a key the deployment does
    /// not offer, a model no slot runs), or the route refuses it (`high`).
    NoPrice,
    /// A part's calls, whose units (more than one a call) its item does
    /// not say.
    NoUnits,
    /// No usage a meter counts (a misspelt kind, a name not printable, a
    /// count past any one row's).
    Invalid,
    /// One call's charge past any one row's (`CHARGE_MAX`).
    TooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Goal: an item is told apart by the field it names, and a misspelt
    /// field is refused, not read as none. Method: each kind from literal
    /// JSON, then the shapes that are none.
    #[test]
    fn a_quotes_items_are_told_apart_by_what_they_name() {
        let ask: QuoteAsk = serde_json::from_value(json!({ "items": [
            { "key": "elevenlabs:speech", "units": 60000 },
            { "key": "fal", "calls": 40 },
            { "model": "cheap", "input": 2000, "cachedInput": 100, "output": 500, "calls": 3 },
            { "usage": { "kind": "awake", "instance": "2vcpu-6gib", "ms": 3600000 } },
        ] }))
        .unwrap();
        assert_eq!(ask.items[0], Planned::Key(PlannedKey { key: "elevenlabs:speech".into(), units: Some(60_000), calls: None }));
        assert_eq!(ask.items[1], Planned::Key(PlannedKey { key: "fal".into(), units: None, calls: Some(40) }));
        assert_eq!(ask.items[2], Planned::Model(PlannedModel { model: "cheap".into(), input: 2000, cached_input: 100, cache_write: 0, output: 500, calls: Some(3) }));
        assert!(matches!(&ask.items[3], Planned::Usage(u) if u.usage["kind"] == "awake" && u.calls.is_none()));
        for bad in [
            json!({ "key": "fal", "unit": 3 }),
            json!({ "key": "fal", "model": "cheap" }),
            json!({ "model": "cheap", "inputs": 3 }),
            json!({ "calls": 3 }),
            json!({ "key": 3 }),
            json!({ "key": "fal", "calls": -1 }),
            json!({ "key": "fal", "output": 3 }),
            json!({ "model": "cheap", "units": 3 }),
            json!({ "usage": { "kind": "requests", "count": 1 }, "units": 3 }),
            json!({ "usage": null }),
        ] {
            assert!(serde_json::from_value::<Planned>(bad.clone()).is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<QuoteAsk>(json!({ "item": [] })).is_err());
        // an unknown item says why, and has no charge
        let q = Quoted { priced_as: None, each_micros: None, calls: 1, charge_micros: None, unknown: Some(Unquoted::NoPrice) };
        assert_eq!(serde_json::to_value(q).unwrap(), json!({ "calls": 1, "unknown": "no_price" }));
    }

    /// Goal: the shell reads a standing by its tag and its reason; the
    /// names are the documented ones. Method: each against literal JSON.
    #[test]
    fn standings_and_plans_are_the_documented_shape() {
        assert_eq!(serde_json::to_value(Standing::Ok).unwrap(), json!({ "standing": "ok" }));
        assert_eq!(
            serde_json::to_value(Standing::AgentsStopped { why: Why::NoCredit }).unwrap(),
            json!({ "standing": "agents_stopped", "why": "no_credit" })
        );
        assert_eq!(serde_json::to_value(Standing::ReadOnly { why: Why::Overdrawn }).unwrap(), json!({ "standing": "read_only", "why": "overdrawn" }));
        assert_eq!(serde_json::to_value(Plan::SeatAlwaysOn).unwrap(), json!("seat_always_on"));
        assert_eq!(serde_json::to_value(SeatState::PastDue).unwrap(), json!("past_due"));
        for why in [Why::Guest, Why::SeatCanceled, Why::NoCredit, Why::Overdrawn] {
            let s = Standing::AgentsStopped { why };
            assert_eq!(serde_json::from_value::<Standing>(serde_json::to_value(s).unwrap()).unwrap(), s);
        }
    }

    /// Requests are camelCase and refuse fields they do not name, so a
    /// misspelt `micros` is an error, not a grant of nothing.
    #[test]
    fn requests_refuse_unknown_fields() {
        let g: GrantCredit = serde_json::from_value(json!({ "id": "g1", "micros": 5, "by": "stripe", "why": "top-up" })).unwrap();
        assert_eq!(g.micros, 5);
        assert!(serde_json::from_value::<GrantCredit>(json!({ "id": "g1", "micro": 5, "by": "stripe", "why": "" })).is_err());
        let cap: SetFragmentCap = serde_json::from_value(json!({ "id": "c1", "fragment": "todo.ann", "micros": null })).unwrap();
        assert_eq!(cap.micros, None);
    }
}
