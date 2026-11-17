//! What a person's ledger tells them (FIN-458), in words: their standing
//! short of `Ok` for want of credit, as Settings said it, with Buy credit
//! where they may (`Ledger::said`). A seat canceled is the seat's to tell
//! (its holder's registry), and a guest has no agents to stop: neither is
//! told here.

use fragment_proto::ledger::{Standing, Why};
use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel};
use serde_json::{Map, Value};

use crate::notice::usd;
use crate::stripe::PACK_MICROS;

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
