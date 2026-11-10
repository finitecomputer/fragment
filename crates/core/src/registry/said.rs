//! What the registry tells a person, in words (FIN-458): each invite
//! waiting on an email of theirs as the `Notice` every client draws the
//! same way, with its answers' routes. Accepted, its answer names the
//! fragment's page, which the client opens (`InviteAccepted::open`).

use fragment_proto::notice::{Notice, NoticeAction, NoticeLevel};
use serde_json::{Map, Value};

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
        must_answer: false,
    }
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
}
