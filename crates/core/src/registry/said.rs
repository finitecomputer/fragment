//! What the registry tells a person, in words (FIN-458): each invite
//! waiting on an email of theirs as the `Notice` every client draws the
//! same way, with its answers' routes (accepted, its answer names the
//! fragment's page, which the client opens: `InviteAccepted::open`); and
//! their seat, lapsed or ending by itself soon.

use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel};
use serde_json::{Map, Value};

use crate::notice::date;

/// An invite waiting for their yes: `fragment`, shared to `edit` (else to
/// view), titled `title` as it was sent, by `from` (the sharer's email, if
/// anyone signs in as them now).
pub fn invite(fragment: &str, edit: bool, title: &str, from: Option<&str>) -> Notice {
    let at = |verb: &str| format!("/api/invites/{fragment}/{verb}");
    let empty = || Value::Object(Map::new());
    Notice {
        id: format!("invite:{fragment}"),
        kind: "invite".into(),
        level: NoticeLevel::Info,
        title: title.to_string(),
        text: Some(format!("{} invited you to {} it.", from.unwrap_or("Someone"), if edit { "edit" } else { "view" })),
        detail: None,
        until: None,
        actions: vec![NoticeAction::post("Accept", at("accept"), empty()), NoticeAction::post("Decline", at("decline"), empty())],
    }
}

/// Their seat lapsed: their agents stop, as Settings said it. Its admin
/// may pay again where seats are sold (`pay_again`: a Checkout, its answer
/// opening it); anyone else asks their org's admin.
pub fn seat_lapsed(admin: bool, pay_again: bool) -> Notice {
    let ask = if admin { "" } else { " Ask your org's admin to pay for it again." };
    Notice {
        id: "seat".into(),
        kind: "seat".into(),
        level: NoticeLevel::Warn,
        title: "Your seat lapsed".into(),
        text: Some(format!("Your agents are stopped. Nothing of yours is gone.{ask}")),
        detail: None,
        until: None,
        actions: pay_again.then(|| NoticeAction::post("Pay again", "/api/billing/checkout", Value::Object(Map::new()))).into_iter().collect(),
    }
}

/// How long before a seat ends by itself it is told (Paul, 2026-11-09).
pub const ENDING_TOLD_MS: i64 = 3 * 24 * 60 * 60 * 1000;

/// Why a seat ends by itself on a date. A Stripe trial now; the
/// migration's free month (a comp with an end, FIN-463) comes next, in
/// words of its own: a comp holds no card, so it says to add one.
pub enum Ending {
    /// A Stripe trial: a card on file keeps the seat, and this registry
    /// does not know whether one is, so the words are true either way.
    Trial,
}

/// A seat ending by itself at `at_ms`, told in its last
/// `ENDING_TOLD_MS` (none before, nor once it ended: lapsed, it is
/// `seat_lapsed`'s), with Payment and invoices (Stripe's portal, its
/// answer opening it) for whoever pays for it (`portal`).
pub fn seat_ends(ending: Ending, at_ms: i64, now_ms: i64, portal: bool) -> Option<Notice> {
    if at_ms <= now_ms || at_ms - now_ms > ENDING_TOLD_MS {
        return None;
    }
    let (title, text) = match ending {
        Ending::Trial => (format!("Your trial ends on {}", date(at_ms)), "Your seat continues with a card on file; without one it lapses then. Nothing of yours is deleted."),
    };
    Some(Notice {
        id: "seat-ends".into(),
        kind: "seat_ends".into(),
        level: NoticeLevel::Info,
        title,
        text: Some(text.into()),
        detail: None,
        until: None,
        actions: portal.then(|| NoticeAction::post("Payment and invoices", "/api/billing/portal", Value::Object(Map::new()))).into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Goal: an invite says what it shares, who shared it and as what, with
    /// its two answers' routes, so a client draws it with no code of its own.
    #[test]
    fn an_invite_in_words_with_its_answers() {
        let n = invite("garden--k3x9", true, "Garden plans", Some("ada@example.com"));
        assert_eq!((n.id.as_str(), n.kind.as_str(), n.level, n.title.as_str()), ("invite:garden--k3x9", "invite", NoticeLevel::Info, "Garden plans"));
        assert_eq!(n.text.as_deref(), Some("ada@example.com invited you to edit it."));
        let routes: Vec<(&str, &str, Value)> = n.actions.iter().map(|a| (a.label.as_str(), a.post.as_deref().unwrap_or(""), a.body.clone().unwrap_or_default())).collect();
        assert_eq!(routes, vec![("Accept", "/api/invites/garden--k3x9/accept", json!({})), ("Decline", "/api/invites/garden--k3x9/decline", json!({}))]);
        assert_eq!(invite("notes--a2b4", false, "notes--a2b4", None).text.as_deref(), Some("Someone invited you to view it."));
    }

    /// Goal: a lapsed seat is told in Settings' words, Pay again for its
    /// admin where seats are sold, anyone else asking their admin.
    #[test]
    fn a_lapsed_seat_in_words() {
        let mine = seat_lapsed(true, true);
        assert_eq!((mine.id.as_str(), mine.level, mine.title.as_str()), ("seat", NoticeLevel::Warn, "Your seat lapsed"));
        assert_eq!(mine.text.as_deref(), Some("Your agents are stopped. Nothing of yours is gone."));
        assert_eq!(mine.actions.iter().map(|a| (a.label.as_str(), a.post.as_deref())).collect::<Vec<_>>(), vec![("Pay again", Some("/api/billing/checkout"))]);
        let theirs = seat_lapsed(false, false);
        assert_eq!(theirs.text.as_deref(), Some("Your agents are stopped. Nothing of yours is gone. Ask your org's admin to pay for it again."));
        assert!(theirs.actions.is_empty());
    }

    /// Goal: a trial is told in its last three days alone, its end's date
    /// in words, Payment and invoices for whoever pays for it.
    #[test]
    fn a_trial_ending_is_told_in_its_last_three_days() {
        // 2026-11-16T12:00:00Z
        let end = 1_794_830_400_000;
        let day = 24 * 60 * 60 * 1000;
        assert!(seat_ends(Ending::Trial, end, end - 3 * day - 1, true).is_none(), "not before its last three days");
        let told = seat_ends(Ending::Trial, end, end - 3 * day, true).expect("told three days before");
        assert_eq!((told.id.as_str(), told.kind.as_str(), told.level), ("seat-ends", "seat_ends", NoticeLevel::Info));
        assert_eq!(told.title, "Your trial ends on November 16");
        assert_eq!(told.text.as_deref(), Some("Your seat continues with a card on file; without one it lapses then. Nothing of yours is deleted."));
        assert_eq!(told.actions.iter().map(|a| (a.label.as_str(), a.post.as_deref())).collect::<Vec<_>>(), vec![("Payment and invoices", Some("/api/billing/portal"))]);
        assert!(seat_ends(Ending::Trial, end, end - day, false).is_some_and(|n| n.actions.is_empty()), "no portal for someone who does not pay");
        assert!(seat_ends(Ending::Trial, end, end, true).is_none(), "ended, it is no longer told as ending");
    }
}
