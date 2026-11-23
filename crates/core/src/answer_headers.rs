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
/// by commas, as a browser and `Headers` join them) holds `directive`
/// exactly, as one of its directives: so a policy already there is not
/// sent twice, and a wider one that merely starts the same is not taken
/// for it (`frame-ancestors 'self' A B` is not `frame-ancestors 'self' A`).
pub fn has_directive(policy: &str, directive: &str) -> bool {
    policy.split([',', ';']).any(|d| d.split_ascii_whitespace().eq(directive.split_ascii_whitespace()))
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
