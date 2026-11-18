//! An app's act in another fragment (the `FRAGMENTS` capability; the
//! cell's acting.rs, docs/api.md "Acting in another fragment"): where it
//! arrives (`<act>/<fragment>/<channel>`) and its body, read into the
//! request the target is asked, which checks who is asking itself.

use std::borrow::Cow;

use fragment_proto::{valid_channel_name, valid_fragment_name};
use serde::Deserialize;

/// The request one act asks of its target.
#[derive(Debug, PartialEq, Eq)]
pub struct Act<'a> {
    pub fragment: &'a str,
    pub method: Method,
    /// Its route there, after `/api`, with its query.
    pub route: String,
    /// JSON: as the app sent it, or a follow's own.
    pub body: Option<Cow<'a, str>>,
    /// The depth its record's runs run at (`x-fragment-hops`): a post's.
    pub hops: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

/// `records`' page, as a channel's reader takes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<u64>,
    before: Option<u64>,
    limit: Option<u32>,
}

/// One act, from the app's call at `depth`: `post` (a `PostRecord`),
/// `draft` (a `PutDraft`), `records` (`{after?, before?, limit?}`) or
/// `follow` (its body unread: the channel's subscription with `follow`).
/// `Err` says why it is no act: the caller's (400), or an act there is
/// not (`Unknown`, 404).
pub fn read<'a>(at: &'a str, body: &'a [u8], depth: u32) -> Result<Act<'a>, Refused> {
    let parts: Vec<&str> = at.split('/').collect();
    let [act, fragment, channel] = parts[..] else { return Err(Refused::Invalid("an act names a fragment and a channel".into())) };
    if !valid_fragment_name(fragment) || !valid_channel_name(channel) {
        return Err(Refused::Invalid("an act names a fragment (<label>--<suffix>) and a channel there".into()));
    }
    let text = std::str::from_utf8(body).map_err(|_| Refused::Invalid("an act's body is JSON".into()))?;
    let at = format!("/channels/{channel}");
    let (method, route, body, hops) = match act {
        "post" => (Method::Post, at, Some(Cow::Borrowed(text)), Some(depth.saturating_add(1))),
        "draft" => (Method::Put, format!("{at}/draft"), Some(Cow::Borrowed(text)), None),
        "follow" => (Method::Post, "/subscriptions".into(), Some(Cow::Owned(format!(r#"{{"channel":"{channel}","follow":true}}"#))), None),
        "records" => {
            let page: Page = serde_json::from_slice(body).map_err(|e| Refused::Invalid(format!("a page is {{after?, before?, limit?}}: {e}")))?;
            let query: Vec<String> = [("after", page.after), ("before", page.before), ("limit", page.limit.map(u64::from))]
                .into_iter()
                .filter_map(|(k, v)| v.map(|n| format!("{k}={n}")))
                .collect();
            let route = if query.is_empty() { at } else { format!("{at}?{}", query.join("&")) };
            (Method::Get, route, None, None)
        }
        _ => return Err(Refused::Unknown(format!("no act {act:?}: post, draft, records, follow"))),
    };
    Ok(Act { fragment, method, route, body, hops })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Invalid(String),
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "room--k3x9";

    #[test]
    fn a_post_goes_to_the_channel_as_sent_a_hop_deeper_than_its_call() {
        let body = br#"{"id":"a","body":{"text":"hi"}}"#;
        let at = format!("post/{ROOM}/chat");
        let act = read(&at, body, 3).unwrap();
        assert_eq!(act, Act { fragment: ROOM, method: Method::Post, route: "/channels/chat".into(), body: Some(r#"{"id":"a","body":{"text":"hi"}}"#.into()), hops: Some(4) });
        // a page's call, or a person's, is at depth 0: as a post from outside
        assert_eq!(read(&format!("post/{ROOM}/chat"), body, 0).unwrap().hops, Some(1));
        assert_eq!(read(&format!("post/{ROOM}/chat"), body, u32::MAX).unwrap().hops, Some(u32::MAX));
    }

    #[test]
    fn a_draft_and_a_read_carry_no_depth() {
        let (drafts, reads) = (format!("draft/{ROOM}/chat"), format!("records/{ROOM}/chat"));
        let draft = read(&drafts, br#"{"turn":"t","text":"ty"}"#, 5).unwrap();
        assert_eq!((draft.method, draft.route.as_str(), draft.hops), (Method::Put, "/channels/chat/draft", None));
        let page = read(&reads, br#"{"after":7,"limit":20}"#, 5).unwrap();
        assert_eq!((page.method, page.route.as_str(), page.body, page.hops), (Method::Get, "/channels/chat?after=7&limit=20", None, None));
        assert_eq!(read(&format!("records/{ROOM}/chat"), b"{}", 0).unwrap().route, "/channels/chat");
        assert_eq!(read(&format!("records/{ROOM}/chat"), br#"{"before":9}"#, 0).unwrap().route, "/channels/chat?before=9");
    }

    #[test]
    fn a_follow_asks_for_the_channel_and_names_nothing_else() {
        let at = format!("follow/{ROOM}/chat");
        let act = read(&at, br#"{"url":"https://elsewhere.example/hook"}"#, 2).unwrap();
        assert_eq!((act.method, act.route.as_str(), act.hops), (Method::Post, "/subscriptions", None));
        assert_eq!(act.body.as_deref(), Some(r#"{"channel":"chat","follow":true}"#), "its body is its own: the app's is not read");
    }

    #[test]
    fn what_is_no_act_is_refused_before_anything_is_asked() {
        let invalid = |at: &str, body: &[u8]| matches!(read(at, body, 0), Err(Refused::Invalid(_)));
        assert!(invalid("post/no such name/chat", b"{}"));
        assert!(invalid(&format!("post/{ROOM}/Chat"), b"{}"));
        assert!(invalid(&format!("post/{ROOM}"), b"{}"));
        assert!(invalid(&format!("post/{ROOM}/chat/more"), b"{}"));
        assert!(invalid(&format!("post/{ROOM}/chat"), &[0xff, 0xfe]));
        // a page names nothing else: no `for`, no other query
        assert!(invalid(&format!("records/{ROOM}/chat"), br#"{"for":"npub1x"}"#));
        assert!(invalid(&format!("records/{ROOM}/chat"), br#"{"after":-1}"#));
        assert!(matches!(read(&format!("delete/{ROOM}/chat"), b"{}", 0), Err(Refused::Unknown(_))));
        assert!(invalid(&format!("follow/{ROOM}/no-such Channel"), b"{}"));
    }
}
