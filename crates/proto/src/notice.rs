//! What a person is told by the platform (`GET /api/notices`, FIN-458):
//! one shape for every kind, its words the server's. Each owner works out
//! its own when asked (the computer's in `fragment_core::computer::said`),
//! the router joins them, and a client draws each the same way: it never
//! reads `kind` to decide what to draw, so a new kind is the server's alone.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `GET /api/notices`: the signer's notices, the most pressing first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoticeList {
    pub notices: Vec<Notice>,
}

/// One thing a person is told, in the server's words, with what they may do
/// about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Notice {
    /// Unique among the person's notices, and the same across reads while it
    /// tells the same thing: what a client keys its row by.
    pub id: String,
    /// What it is about (`key_paused`), for tests and logs. No client reads
    /// it to decide what to draw.
    pub kind: String,
    pub level: NoticeLevel,
    /// One short line.
    pub title: String,
    /// A sentence or two under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub text: Option<String>,
    /// The platform's own words, small, for whoever helps (why a start
    /// failed, as it said it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub detail: Option<String>,
    /// When what it tells ends by the clock alone (a paused key resumes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub until: Option<Until>,
    /// What the person may do about it; the first is the main one.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend("default" = [])))]
    pub actions: Vec<NoticeAction>,
    /// It must be answered (FIN-458): a client shows it alone, over
    /// everything, until one of its actions goes through, and the read
    /// after no longer has it. The first such in the list first: its
    /// owner lists its own oldest first. A client from before it shows it
    /// as any other.
    #[serde(default, rename = "mustAnswer", skip_serializing_if = "crate::is_default")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = false)))]
    pub must_answer: bool,
    /// A picture beside its words (an agent's): a platform path, read with
    /// the person's own credential. A client from before it shows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub image: Option<String>,
}

/// How a notice is shown.
///
/// Open (`x-fragment-open`, docs/wire): a level a reader does not know is
/// shown as `info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-fragment-open" = true)))]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    /// Something to know.
    Info,
    /// Something wrong, or about to be.
    Warn,
}

/// A time a notice counts down to: a client shows `said`, then how long
/// until `at` (" in 23 min"), live, and drops the notice once it passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Until {
    /// ms since the epoch.
    pub at: i64,
    /// The words before the countdown ("Calls resume").
    pub said: String,
}

/// What a person may do about a notice: `post` its `body` to a platform
/// route with their own credential, then read their notices again; or
/// `open` a page. One of the two. A post's answer may name a page to open
/// (`open`, as `InviteAccepted` does), opened once that read has it; a
/// post refused shows the server's own words for it (`ErrorBody.message`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoticeAction {
    pub label: String,
    /// A platform path (`/api/computers/{id}/restart`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub post: Option<String>,
    /// The JSON `post` sends (none: `{}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub body: Option<Value>,
    /// An `https:` page or a `mailto:` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(extend("default" = null)))]
    pub open: Option<String>,
}

impl NoticeAction {
    /// An action that posts `body` to `route`.
    pub fn post(label: impl Into<String>, route: impl Into<String>, body: Value) -> NoticeAction {
        NoticeAction { label: label.into(), post: Some(route.into()), body: Some(body), open: None }
    }

    /// An action that opens `url`.
    pub fn open(label: impl Into<String>, url: impl Into<String>) -> NoticeAction {
        NoticeAction { label: label.into(), post: None, body: None, open: Some(url.into()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the wire: what is unknown left out, each action one of its two.
    #[test]
    fn a_notice_on_the_wire() {
        let n = Notice {
            id: "key_paused:elevenlabs".into(),
            kind: "key_paused".into(),
            level: NoticeLevel::Warn,
            title: "ElevenLabs calls paused".into(),
            text: None,
            detail: None,
            until: Some(Until { at: 60_000, said: "Calls resume".into() }),
            actions: vec![NoticeAction::post("Allow", "/api/computers/computer:aa/key-caps/elevenlabs", serde_json::json!({ "from": 5 })), NoticeAction::open("Get help", "mailto:help@example.com")],
            must_answer: false,
            image: None,
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "id": "key_paused:elevenlabs", "kind": "key_paused", "level": "warn", "title": "ElevenLabs calls paused",
                "until": { "at": 60_000, "said": "Calls resume" },
                "actions": [
                    { "label": "Allow", "post": "/api/computers/computer:aa/key-caps/elevenlabs", "body": { "from": 5 } },
                    { "label": "Get help", "open": "mailto:help@example.com" },
                ],
            })
        );
        assert_eq!(serde_json::from_value::<Notice>(v).unwrap(), n);
        let bare: Notice = serde_json::from_str(r#"{"id":"a","kind":"a","level":"info","title":"A"}"#).unwrap();
        assert!(bare.actions.is_empty() && bare.until.is_none() && bare.text.is_none() && !bare.must_answer);
        let asked = Notice { must_answer: true, ..bare };
        assert_eq!(serde_json::to_value(&asked).unwrap()["mustAnswer"], serde_json::json!(true));
    }
}
