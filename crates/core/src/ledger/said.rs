//! What a person's ledger tells them (FIN-458), in words: their standing
//! short of `Ok` for want of credit, as Settings said it, with Buy credit
//! where they may (`Ledger::said`). A seat canceled is the seat's to tell
//! (its holder's registry), and a guest has no agents to stop: neither is
//! told here.

use fragment_proto::ledger::{Standing, Why};
use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel, Until};
use serde_json::{Map, Value};

use crate::notice::usd;
use crate::price::USD;
use crate::stripe::PACK_MICROS;

/// What its agents' calls on shared keys cost an hour, past which their
/// person is told (FIN-616; 2026-11-20: the hourly key pause's $5, a notice
/// now instead of a pause), and again at each doubling.
pub const SPENDING_MICROS: i64 = 5 * USD;

/// The step an hour's spend of `micros` has passed: `SPENDING_MICROS`,
/// then each doubling of it; none below it.
pub fn spending_step(micros: i64) -> Option<i64> {
    if micros < SPENDING_MICROS {
        return None;
    }
    let mut at = SPENDING_MICROS;
    // bounded: `at` doubles each pass and stays at most `micros`, an i64
    while at <= micros / 2 {
        at *= 2;
    }
    Some(at)
}

/// The notice for an hour's spend of `micros` on shared keys, at its step
/// (`spending_step`): the same words until it passes the next, so its
/// person is told once a step, and gone at `until_ms`, when what it counts
/// has aged out of the hour (`uses::spent_until`), so a page open that long
/// drops it with no read.
pub fn spending(micros: i64, until_ms: Option<i64>) -> Option<Notice> {
    let over = spending_step(micros)?;
    Some(Notice {
        id: "spending".into(),
        kind: "spending".into(),
        level: NoticeLevel::Info,
        title: format!("Your agents spent over {} on tools in the last hour", usd(over)),
        text: Some("Their calls on shared keys are charged to you.".into()),
        detail: None,
        until: until_ms.map(|at| Until { at, said: "It clears".into() }),
        actions: vec![],
        must_answer: false,
        image: None,
    })
}

/// The notice for `standing`, if it is one of credit, with a pack to buy
/// (`/api/billing/packs`, whose answer opens its Checkout) when `may_buy`.
pub fn standing(standing: Standing, may_buy: bool) -> Option<Notice> {
    let (title, text) = match standing {
        Standing::AgentsStopped { why: Why::NoCredit } => ("Your agents are stopped", "The credit is used up; adding credit starts them again."),
        Standing::ReadOnly { why: Why::Overdrawn } => ("Your fragments are read-only, and your agents are stopped", "The balance reached the overdraft; credit that brings it above zero ends it."),
        _ => return None,
    };
    let buy = may_buy.then(|| NoticeAction::post(format!("Buy {} of credit", usd(PACK_MICROS)), "/api/billing/packs", Value::Object(Map::new())));
    Some(Notice {
        id: "credit".into(),
        kind: "credit".into(),
        level: NoticeLevel::Warn,
        title: title.into(),
        text: Some(text.into()),
        detail: None,
        until: None,
        actions: buy.into_iter().collect(),
        must_answer: false,
        image: None,
    })
}

/// Whether what a ledger says now (`Ledger::told_key`) is news to its
/// person's pages: anything but what was last told them (`kept`), none
/// told yet too (a ledger from before these notices, whose page may show
/// one already).
pub fn news(kept: Option<&str>, now: &str) -> bool {
    kept != Some(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Goal: a page is told whenever what its ledger says differs from what
    /// it last told, none told yet too: a ledger from before these notices
    /// has none kept, and its page may show "Your agents are stopped"
    /// already, so the credit that lifts it must be told; the same again is
    /// not.
    #[test]
    fn what_is_said_is_news_unless_it_was_told_last() {
        let stopped = serde_json::to_string(&standing(Standing::AgentsStopped { why: Why::NoCredit }, true).into_iter().collect::<Vec<_>>()).expect("serializes");
        let quiet = "[]";
        assert!(news(None, quiet), "none told yet: told");
        assert!(news(Some(&stopped), quiet), "lifted: told");
        assert!(!news(Some(quiet), quiet), "the same again: not told");
    }

    /// FIN-616: an hour's spend on shared keys past $5 is told, at the step
    /// it passed (each doubling), in the same words until the next step;
    /// under it, nothing.
    #[test]
    fn an_hours_spend_is_told_at_its_step() {
        assert_eq!(spending_step(SPENDING_MICROS - 1), None);
        assert_eq!(spending_step(SPENDING_MICROS), Some(5_000_000));
        assert_eq!(spending_step(9_999_999), Some(5_000_000));
        assert_eq!(spending_step(10_000_000), Some(10_000_000));
        assert_eq!(spending_step(45_000_000), Some(40_000_000));
        assert_eq!(spending_step(i64::MAX), Some(SPENDING_MICROS << 40), "an i64's largest, bounded");
        assert!(spending(SPENDING_MICROS - 1, Some(1)).is_none());
        let told = spending(12_340_000, Some(9_000)).expect("past $10");
        assert_eq!(told.until, Some(Until { at: 9_000, said: "It clears".into() }), "gone as its spend ages out");
        assert_eq!((told.id.as_str(), told.kind.as_str(), told.level, told.title.as_str()), ("spending", "spending", NoticeLevel::Info, "Your agents spent over $10 on tools in the last hour"));
        assert!(told.actions.is_empty() && !told.must_answer);
        assert_eq!(spending(12_340_000, Some(9_000)), spending(19_000_000, Some(9_000)), "the same step, the same words: told once");
    }

    #[test]
    fn credit_in_words_with_a_pack_to_buy() {
        let out = standing(Standing::AgentsStopped { why: Why::NoCredit }, true).expect("out of credit is told");
        assert_eq!((out.id.as_str(), out.kind.as_str(), out.level, out.title.as_str()), ("credit", "credit", NoticeLevel::Warn, "Your agents are stopped"));
        assert_eq!(out.text.as_deref(), Some("The credit is used up; adding credit starts them again."));
        let buy: Vec<(&str, Option<&str>, Option<&Value>)> = out.actions.iter().map(|a| (a.label.as_str(), a.post.as_deref(), a.body.as_ref())).collect();
        assert_eq!(buy, vec![("Buy $25 of credit", Some("/api/billing/packs"), Some(&json!({})))]);
        let over = standing(Standing::ReadOnly { why: Why::Overdrawn }, false).expect("past the overdraft is told");
        assert_eq!(over.title, "Your fragments are read-only, and your agents are stopped");
        assert!(over.actions.is_empty(), "no pack where they may not buy one");
        for none in [Standing::Ok, Standing::AgentsStopped { why: Why::Guest }, Standing::AgentsStopped { why: Why::SeatCanceled }] {
            assert!(standing(none, true).is_none(), "{none:?} is no credit notice");
        }
    }
}
