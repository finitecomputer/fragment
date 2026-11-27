//! The security headers every answer of the cell carries, by the host it
//! answers on (docs/security/http-headers.md, for the CASA assessment):
//! HSTS on every host reached over HTTPS, and a `frame-ancestors` floor
//! on the platform's answers, on its own host and on every fragment's
//! (`floor`); a fragment app's own answers are left to the app. Pure: the
//! cell sets them (cell/src/lib.rs `fetch` and `route`'s `framing_floor`).
//!
//! The floor goes only on an answer that names no `frame-ancestors` of
//! its own, and into that answer's own policy (`with_frame_floor`): a
//! page that says who may frame it (the shell's pages, sign-in's, the
//! updating notice, a frame's answer: `frames::ancestors`, a page any
//! page may frame) keeps its policy as it came, and no answer gets a
//! second policy beside its own.

use crate::frames::{self, Framed};

/// `Strict-Transport-Security`: a year, the value the HSTS preload list
/// asks as its minimum and scanners expect. Without `includeSubDomains`:
/// the platform's domain has names the cell does not answer (a
/// preview's, `www`'s, mail's), and HSTS that covered them would bind
/// them to HTTPS from here, which only their own answers may decide.
/// Each fragment's host is answered here and so carries its own. No
/// `preload`: the list requires `includeSubDomains`.
pub const HSTS: &str = "max-age=31536000";

/// The HSTS value for an answer that arrived on `scheme`: only over
/// HTTPS, since a browser ignores the header on plain HTTP and a local
/// fleet (`http://127.0.0.1:8790`) must stay reachable as it is.
pub fn hsts(scheme: &str) -> Option<&'static str> {
    scheme.eq_ignore_ascii_case("https").then_some(HSTS)
}

/// The platform's host: only its own pages may frame its answers (the
/// shell frames `/share` and `/auth/frame` there). `X-Frame-Options`
/// says the same to a browser without CSP.
pub const PLATFORM_FRAME_ANCESTORS: &str = "frame-ancestors 'self'";
pub const PLATFORM_FRAME_OPTIONS: &str = "SAMEORIGIN";

/// The platform's answers on a fragment's host: the fragment's own pages
/// and the platform's (the shell shows every fragment in a frame), the
/// policy a stranger's answer to a frame already gets (`frames::ancestors`),
/// so a browser without Fetch Metadata, or a scan, sees it too. No `X-Frame-Options`:
/// it cannot name a second origin, and `SAMEORIGIN` would break the shell.
pub fn fragment_frame_ancestors(platform: &str) -> String {
    format!("frame-ancestors {}", frames::ancestors(Framed::Stranger, platform))
}

/// Who made an answer, by its host and, on a fragment's, by its maker:
/// the floor is chosen for each (`floor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    /// The platform's host, or any host that is no fragment's or computer's.
    Platform,
    /// A fragment's host (its own, a claimed address), the platform's own
    /// answer there: a sign-in hop, a refusal, an error.
    FragmentPlatform,
    /// A fragment's host, its app's own answer: a page, a file.
    FragmentApp,
    /// A computer's host (`computer::framed_by` frames its every answer).
    Computer,
}

/// The `frame-ancestors` floor an answer gets when it names none, and the
/// `X-Frame-Options` beside it. `None` for an app's own answer: who may
/// frame a published page is its app's to say, so a publisher's page may
/// embed it wherever it could before this floor; a frame's
/// navigation, with Fetch Metadata, still gets the cell's own policy
/// beside the app's (`bound`). `None` for a computer's, framed already.
pub fn floor(answered: Answered, platform: &str) -> Option<(String, Option<&'static str>)> {
    match answered {
        Answered::Platform => Some((PLATFORM_FRAME_ANCESTORS.to_string(), Some(PLATFORM_FRAME_OPTIONS))),
        Answered::FragmentPlatform => Some((fragment_frame_ancestors(platform), None)),
        Answered::FragmentApp | Answered::Computer => None,
    }
}

/// The `Content-Security-Policy` value an answer leaves with, given the
/// one it carries (`existing`: its policies joined by commas, as a
/// browser and `Headers` join them) and its host's `frame-ancestors`
/// floor. `None` when one of its policies already names
/// `frame-ancestors`: who may frame it is that answer's own decision (a
/// page meant to be framed by any page says `*`; a frame's answer says
/// `frames::ancestors`), kept as it came. Otherwise the floor goes into
/// its last policy, so the answer still carries one header and as many
/// policies as it had, never a second policy beside its own; with no
/// policy at all, the floor alone.
pub fn with_frame_floor(existing: Option<&str>, floor: &str) -> Option<String> {
    assert!(
        floor.split_ascii_whitespace().next() == Some("frame-ancestors") && !floor.contains([';', ',']),
        "a floor is one frame-ancestors directive: {floor:?}"
    );
    let existing = existing.unwrap_or_default();
    if names_directive(existing, "frame-ancestors") {
        return None;
    }
    let own = existing.trim_end_matches(|c: char| c.is_ascii_whitespace() || c == ';' || c == ',');
    Some(match own.is_empty() {
        true => floor.to_string(),
        false => format!("{own}; {floor}"),
    })
}

/// Whether some policy in a `Content-Security-Policy` header value names
/// the directive `name` at all.
fn names_directive(header: &str, name: &str) -> bool {
    header.split(',').any(|policy| first_directive(policy, name).is_some())
}

/// A policy's first directive called `name` (directive names are ASCII
/// case-insensitive), the one a browser enforces.
fn first_directive<'a>(policy: &'a str, name: &str) -> Option<&'a str> {
    policy.split(';').find(|d| d.split_ascii_whitespace().next().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: an answer that says who may frame it keeps its policy as it
    /// came, so a page meant to be framed by any page stays framable and
    /// no answer carries the directive twice. Method: `frame-ancestors`
    /// named in each place a policy can name it, `*` among them.
    #[test]
    fn an_answers_own_frame_ancestors_is_kept() {
        let floor = "frame-ancestors 'self' https://finite.chat";
        for own in [
            "frame-ancestors *",
            "frame-ancestors 'self' https://finite.chat",
            "default-src 'none'; style-src 'self'; frame-ancestors 'self'",
            "sandbox, FRAME-ANCESTORS 'none'",
            "frame-ancestors 'self', frame-ancestors 'self' https://finite.chat",
        ] {
            assert_eq!(with_frame_floor(Some(own), floor), None, "{own:?}");
        }
    }

    /// Goal: an answer that names no `frame-ancestors` gets the floor in
    /// its own policy, never as a second one: one header, as many
    /// policies as it had, its other directives untouched. Method: no
    /// policy, an empty one, one policy, one with a trailing `;`, two.
    #[test]
    fn the_floor_joins_an_answers_own_policy() {
        let floor = "frame-ancestors 'self'";
        for (existing, after) in [
            (None, "frame-ancestors 'self'"),
            (Some(""), "frame-ancestors 'self'"),
            (Some("sandbox"), "sandbox; frame-ancestors 'self'"),
            (Some("default-src 'none'; style-src 'self';"), "default-src 'none'; style-src 'self'; frame-ancestors 'self'"),
            (Some("sandbox, default-src 'self' "), "sandbox, default-src 'self'; frame-ancestors 'self'"),
            (Some("report-uri https://x.example/frame-ancestors"), "report-uri https://x.example/frame-ancestors; frame-ancestors 'self'"),
        ] {
            assert_eq!(with_frame_floor(existing, floor).as_deref(), Some(after), "{existing:?}");
            let policies = |v: &str| v.split(',').count();
            assert_eq!(policies(after), existing.map_or(1, policies), "{existing:?}");
        }
    }

    /// Goal: a floor is one `frame-ancestors` directive alone, so joined
    /// into a policy it adds nothing else. Method: a second directive.
    #[test]
    #[should_panic(expected = "a floor is one frame-ancestors directive")]
    fn a_floor_is_one_directive() {
        with_frame_floor(None, "frame-ancestors 'self'; script-src *");
    }

    /// Goal: the platform keeps an answer's own `frame-ancestors`, and
    /// adds its floor to any answer that names none, a mention elsewhere
    /// in a policy included. Method: present, absent and near misses.
    #[test]
    fn a_policy_names_a_directive_only_as_one() {
        for named in ["frame-ancestors 'none'", "default-src 'self'; FRAME-ANCESTORS 'self'", "sandbox, frame-ancestors 'self'"] {
            assert!(names_directive(named, "frame-ancestors"), "{named:?}");
        }
        for unnamed in ["", "default-src 'self'", "report-uri https://x.example/frame-ancestors", "default-src 'self' frame-ancestors"] {
            assert!(!names_directive(unnamed, "frame-ancestors"), "{unnamed:?}");
        }
    }

    /// Goal: HSTS goes on every answer reached over HTTPS and none over
    /// HTTP. Method: the schemes a request arrives on, in either case.
    #[test]
    fn hsts_only_over_https() {
        assert_eq!(hsts("https"), Some("max-age=31536000"));
        assert_eq!(hsts("HTTPS"), Some(HSTS));
        for plain in ["http", "ws", ""] {
            assert_eq!(hsts(plain), None, "{plain:?}");
        }
    }

    /// Goal: the HSTS value lasts at least a year and binds no other
    /// name. Method: parse its directives.
    #[test]
    fn hsts_binds_this_host_for_a_year() {
        let directives: Vec<&str> = HSTS.split(';').map(str::trim).collect();
        let age: u64 = directives[0].strip_prefix("max-age=").expect("max-age first").parse().expect("a number");
        assert!(age >= 31_536_000, "{age}");
        assert!(!HSTS.to_ascii_lowercase().contains("includesubdomains"));
        assert!(!HSTS.to_ascii_lowercase().contains("preload"));
    }

    /// Goal: a fragment may be framed by its own pages and the
    /// platform's, and no one else. Method: the policy for a platform
    /// origin, and the one for a local fleet.
    #[test]
    fn fragment_frames_only_self_and_platform() {
        assert_eq!(fragment_frame_ancestors("https://finite.chat"), "frame-ancestors 'self' https://finite.chat");
        assert_eq!(fragment_frame_ancestors("http://127.0.0.1:8790"), "frame-ancestors 'self' http://127.0.0.1:8790");
    }

    /// Goal: a platform origin that is not one cannot widen the policy.
    /// Method: a wildcard where the origin goes.
    #[test]
    #[should_panic(expected = "the platform is named by its origin")]
    fn fragment_policy_refuses_a_non_origin() {
        fragment_frame_ancestors("https://*.finite.place");
    }

    /// Goal: a published fragment's own page keeps its framing to its app,
    /// so a publisher's embed of it works as it did before the floor, while
    /// the platform's answers, on its host or a fragment's, get theirs.
    /// Method: each kind of answer.
    #[test]
    fn an_apps_own_answer_gets_no_floor() {
        let platform = "https://finite.chat";
        assert_eq!(floor(Answered::FragmentApp, platform), None);
        assert_eq!(floor(Answered::Computer, platform), None);
        assert_eq!(floor(Answered::FragmentPlatform, platform), Some((fragment_frame_ancestors(platform), None)));
        assert_eq!(floor(Answered::Platform, platform), Some((PLATFORM_FRAME_ANCESTORS.to_string(), Some(PLATFORM_FRAME_OPTIONS))));
    }

    /// Goal: the platform's floor lets only its own pages frame it.
    #[test]
    fn platform_frames_only_itself() {
        assert_eq!(PLATFORM_FRAME_ANCESTORS, "frame-ancestors 'self'");
        assert_eq!(PLATFORM_FRAME_OPTIONS, "SAMEORIGIN");
    }
}
