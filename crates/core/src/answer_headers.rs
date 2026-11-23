//! The security headers every answer of the cell carries, by the host it
//! answers on (docs/security/http-headers.md, for the CASA assessment):
//! HSTS on every host reached over HTTPS, and a `frame-ancestors` floor
//! on the platform's host and on every fragment's. Pure: the cell sets
//! them (cell/src/lib.rs `fetch`, `platform_origin`, `fragment_origin`).
//!
//! The values here are the widest any answer gets. A page that needs a
//! narrower policy sets its own (the shell's pages, sign-in's, a frame's
//! answer: `frames::ancestors`); a second `frame-ancestors` only narrows,
//! because a browser enforces every policy an answer carries.

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

/// A fragment's host: its own pages and the platform's (the shell shows
/// every fragment in a frame), the policy a stranger's answer to a frame
/// already gets (`frames::ancestors`), now on every answer, so a browser
/// without Fetch Metadata, or a scan, sees it too. No `X-Frame-Options`:
/// it cannot name a second origin, and `SAMEORIGIN` would break the shell.
pub fn fragment_frame_ancestors(platform: &str) -> String {
    format!("frame-ancestors {}", frames::ancestors(Framed::Stranger, platform))
}

/// Whether a `Content-Security-Policy` header value (its policies joined
/// by commas, as a browser and `Headers` join them) enforces `directive`
/// exactly in one of its policies: so a policy already there is not sent
/// twice, and a wider one is not taken for it. Only a directive's first
/// occurrence in a policy counts, as a browser enforces only that one
/// (CSP3 §2.2, a duplicate is ignored): `frame-ancestors *;
/// frame-ancestors 'self' A` enforces `*`. And only exactly:
/// `frame-ancestors 'self' A B` is not `frame-ancestors 'self' A`. A miss
/// costs only a second policy, which can only narrow.
pub fn has_directive(header: &str, directive: &str) -> bool {
    let Some(name) = directive.split_ascii_whitespace().next() else { return false };
    header
        .split(',')
        .filter_map(|policy| first_directive(policy, name))
        .any(|d| d.split_ascii_whitespace().eq(directive.split_ascii_whitespace()))
}

/// Whether some policy in a `Content-Security-Policy` header value names
/// the directive `name` at all (for the platform's own answers, whose
/// every policy is the platform's).
pub fn names_directive(header: &str, name: &str) -> bool {
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

    /// Goal: a policy is taken as present only when one directive is
    /// exactly it, so a fragment's answer never skips the floor because
    /// its app sent a wider one. Method: the floor alone, among others,
    /// in a second policy, and the near misses that would widen it.
    #[test]
    fn a_directive_is_present_only_exactly() {
        let floor = "frame-ancestors 'self' https://finite.chat";
        for present in [floor, "default-src 'self'; frame-ancestors 'self' https://finite.chat", "sandbox, frame-ancestors  'self' https://finite.chat ;"] {
            assert!(has_directive(present, floor), "{present:?}");
        }
        for absent in ["", "sandbox", "frame-ancestors 'self' https://finite.chat https://evil.example", "frame-ancestors *", "frame-ancestors 'self'", "default-src 'self' frame-ancestors 'self' https://finite.chat"] {
            assert!(!has_directive(absent, floor), "{absent:?}");
        }
    }

    /// Goal: an app cannot make the floor be skipped by naming it after
    /// a wider `frame-ancestors` in the same policy: a browser enforces
    /// only a directive's first occurrence in each policy (CSP3 §2.2).
    /// Method: the floor behind a wider one, in either case, in one
    /// policy; the floor in a policy of its own still counts.
    #[test]
    fn only_a_policys_first_directive_counts() {
        let floor = "frame-ancestors 'self' https://finite.chat";
        for widened in [
            "frame-ancestors *; frame-ancestors 'self' https://finite.chat",
            "FRAME-ANCESTORS https://evil.example; frame-ancestors 'self' https://finite.chat",
            "default-src 'self'; frame-ancestors *;frame-ancestors 'self' https://finite.chat",
        ] {
            assert!(!has_directive(widened, floor), "{widened:?}");
        }
        assert!(has_directive("frame-ancestors *, frame-ancestors 'self' https://finite.chat", floor));
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

    /// Goal: the platform's floor lets only its own pages frame it.
    #[test]
    fn platform_frames_only_itself() {
        assert_eq!(PLATFORM_FRAME_ANCESTORS, "frame-ancestors 'self'");
        assert_eq!(PLATFORM_FRAME_OPTIONS, "SAMEORIGIN");
    }
}
