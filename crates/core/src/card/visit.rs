//! Who is shot and as whom, and whether a shot landed on the app.

use fragment_proto::{FragmentKind, Visibility};
use serde::{Deserialize, Serialize};

/// As whom the renderer opens the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visitor {
    /// A public fragment: as anyone sees it.
    Anyone,
    /// A link fragment: as anyone holding its share link sees it (what a
    /// member of it can already see; the card goes to members only).
    LinkHolder,
}

/// Why a live is not shot (a deploy's, or one asked for again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skip {
    /// A chat or an agent: not an app (the shell's Apps list has no place
    /// for it).
    NotAnApp,
    /// A members-only fragment: an anonymous visitor gets its refusal.
    MembersOnly,
    /// Its owner pays for nothing now (a guest), or is past the overdraft.
    OwnerPays,
}

impl Skip {
    pub fn message(self) -> &'static str {
        match self {
            Skip::NotAnApp => "a chat or an agent is not an app: it has no card",
            Skip::MembersOnly => "a members-only fragment's page is not shot: a visitor without an account sees only its refusal (an uploaded card is its members' all the same)",
            Skip::OwnerPays => "its owner's ledger takes no shot now (a guest pays for nothing; past the overdraft, nothing new is made)",
        }
    }
}

/// Whether a fragment of `kind` has a card at all: an app or a brain,
/// which the shell's Apps list shows. An upload asks only this: it needs
/// no visitor, so a members-only app takes one.
pub fn is_app(kind: FragmentKind) -> Result<(), Skip> {
    match kind {
        // a kind this build does not know (a newer platform's) has none
        FragmentKind::Chat | FragmentKind::Agent | FragmentKind::Skills | FragmentKind::Sessions | FragmentKind::Other => Err(Skip::NotAnApp),
        FragmentKind::App | FragmentKind::Brain => Ok(()),
    }
}

/// Whether a fragment of `kind` and `visibility` is shot, and as whom.
pub fn visitor(kind: FragmentKind, visibility: Visibility) -> Result<Visitor, Skip> {
    is_app(kind)?;
    match visibility {
        Visibility::Members => Err(Skip::MembersOnly),
        Visibility::Public => Ok(Visitor::Anyone),
        Visibility::Link => Ok(Visitor::LinkHolder),
    }
}

/// What the renderer opens: the fragment's front page on its own origin,
/// and for a link holder the share link's token, which it sets as that
/// origin's `fragview` cookie first (`site::VIEW_COOKIE`). Never
/// `?view=`, which the page's code could read, and report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub url: String,
    pub view_token: Option<String>,
}

/// The visit a shot of `visitor` makes to `origin` (`scheme://host[:port]`).
pub fn visit(origin: &str, visitor: Visitor, view_token: &str) -> Visit {
    assert!(!origin.ends_with('/'), "an origin has no path: {origin}");
    let url = format!("{origin}/");
    match visitor {
        Visitor::Anyone => Visit { url, view_token: None },
        Visitor::LinkHolder => {
            assert!(!view_token.is_empty() && view_token.bytes().all(|b| b.is_ascii_alphanumeric()), "a view token is a cookie's value as it is");
            Visit { url, view_token: Some(view_token.to_string()) }
        }
    }
}

/// Why a shot's page is not the app's (`landed`): it went to another
/// origin. A fragment that went members only while it was shot sends the
/// shot's visitor, who has no account, to sign in on the platform's origin
/// (as any signed-out visit to a page it refuses); that page is no card of
/// it (FIN-359).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elsewhere {
    pub origin: String,
}

impl Elsewhere {
    pub fn message(&self) -> String {
        format!("the page went to {}, not the app's own origin: a sign-in, as for a fragment gone members only", self.origin)
    }
}

/// Whether the page a shot of `url` (a `visit`'s) opened is on the app's
/// own origin, once it loaded at `at` (the page's own URL, as the browser
/// says). Whatever the app answers there is its page, a 404 included.
pub fn landed(url: &str, at: &str) -> Result<(), Elsewhere> {
    let opened = url::Url::parse(url).expect("a visit's URL parses").origin();
    let origin = url::Url::parse(at).map(|u| u.origin());
    match origin {
        Ok(origin) if origin == opened => Ok(()),
        Ok(origin) => Err(Elsewhere { origin: origin.ascii_serialization() }),
        Err(_) => Err(Elsewhere { origin: "a page with no URL".to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a shot is the app's only where it landed on the app's own
    /// origin. Valid: its front page, another of its paths (the app's own
    /// redirect), a port's origin. Invalid: a sign-in on another origin (the
    /// platform's, WorkOS'), an origin that only starts like its own, and
    /// a page with no origin (Chrome's error page).
    #[test]
    fn a_shot_is_the_apps_only_where_it_landed_on_it() {
        let url = visit("https://notes.finite.place", Visitor::Anyone, "").url;
        for at in ["https://notes.finite.place/", "https://notes.finite.place/today?x=1"] {
            assert_eq!(landed(&url, at), Ok(()), "{at}");
        }
        let local = visit("http://notes.fragment.localhost:8790", Visitor::Anyone, "").url;
        assert_eq!(landed(&local, "http://notes.fragment.localhost:8790/"), Ok(()));
        let elsewhere = |origin: &str| Err(Elsewhere { origin: origin.into() });
        assert_eq!(landed(&url, "https://finite.chat/auth/fragment?name=notes&return=%2F"), elsewhere("https://finite.chat"));
        assert_eq!(landed(&url, "https://auth.workos.com/sign-in"), elsewhere("https://auth.workos.com"));
        assert_eq!(landed(&url, "https://notes.finite.place.evil.test/"), elsewhere("https://notes.finite.place.evil.test"));
        assert_eq!(landed(&local, "http://notes.fragment.localhost:8791/"), elsewhere("http://notes.fragment.localhost:8791"));
        assert_eq!(landed(&url, "chrome-error://chromewebdata/"), elsewhere("null"));
        assert!(Elsewhere { origin: "https://finite.chat".into() }.message().contains("https://finite.chat"));
    }

    /// Goal: chats and agents are not shot, nor a kind this build does not
    /// know, members-only fragments are not shot, and the others are shot
    /// as their visitors see them.
    #[test]
    fn who_is_shot_and_as_whom() {
        use FragmentKind::*;
        use Visibility::*;
        for kind in [Chat, Agent, Skills, Sessions, Other] {
            assert_eq!(is_app(kind), Err(Skip::NotAnApp));
            for v in [Public, Link, Members] {
                assert_eq!(visitor(kind, v), Err(Skip::NotAnApp));
            }
        }
        for kind in [App, Brain] {
            assert_eq!(is_app(kind), Ok(()), "an app takes an upload, members-only or not");
            assert_eq!(visitor(kind, Public), Ok(Visitor::Anyone));
            assert_eq!(visitor(kind, Link), Ok(Visitor::LinkHolder));
            assert_eq!(visitor(kind, Members), Err(Skip::MembersOnly));
        }
        assert_eq!(visit("https://todo--ann.fragment.boats", Visitor::Anyone, "t0k"), Visit { url: "https://todo--ann.fragment.boats/".into(), view_token: None });
        // a link holder's token goes in the cookie, never the URL the page reads
        assert_eq!(
            visit("http://todo--ann.fragment.localhost:8790", Visitor::LinkHolder, "t0k3n"),
            Visit { url: "http://todo--ann.fragment.localhost:8790/".into(), view_token: Some("t0k3n".into()) }
        );
    }
}
