//! What a refusal says to whoever asked, outside the platform
//! (docs/security/error-responses.md, for the CASA assessment). The
//! platform's own failures (`host_failed`, a 500) carry messages for
//! operators: a JavaScript error's text, a store's answer, a binding's
//! name, an internal id. Past the cell's edge they say a fixed sentence
//! and a reference instead, and the detail goes only to the Worker's logs
//! under that reference.
//!
//! Every other code's message is the asker's to act on, written for them
//! (what was wrong, what to do), and stays. That includes
//! `upstream_failed` (a 502): its messages say which service failed and
//! whether a model's call was charged. A vendor's own words (its answer's
//! body or `message`) are kept out of them where they are made, and
//! logged there instead (cell/src/cs.rs, keys.rs, stripe.rs).
//!
//! Inside the platform (an object answering the router) a refusal keeps
//! its whole message: it is cut at the edge (cell/src/lib.rs).

use fragment_proto::ErrorCode;

/// The words a refusal of `code` says in place of its own past the edge,
/// or `None` where its own message is the asker's to read.
pub fn redacted(code: ErrorCode) -> Option<&'static str> {
    match code {
        ErrorCode::HostFailed => Some("something went wrong on our side; try again, and if it keeps happening, tell us this reference"),
        // its message wraps the registry's own refusal, a storage error's
        // words among them (cell/src/lib.rs `registry_round_trip`)
        ErrorCode::RegistryUnavailable => Some("the identity registry did not answer; try again shortly, and if it keeps happening, tell us this reference"),
        _ => None,
    }
}

/// Whether an answer leaving the cell is read as one of the platform's
/// refusals, to say it as `public_message` would.
/// Only one the platform marked as its own (`marked`: the cell's
/// `error::PLATFORM_REFUSAL`, set on every refusal an object of the
/// platform answers): an app's or a computer image's answer is theirs,
/// and passes unread, whatever it holds. And only a JSON one at a
/// redacted code's status.
pub fn rereads(marked: bool, status: u16, content_type: &str) -> bool {
    marked && content_type.starts_with("application/json") && REDACTED.iter().any(|c| c.status() == status)
}

/// The codes `redacted` says words for.
pub const REDACTED: [ErrorCode; 2] = [ErrorCode::HostFailed, ErrorCode::RegistryUnavailable];

/// The most bytes of an OAuth error code a sign-in's page names.
pub const OAUTH_ERROR_MAX_BYTES: usize = 64;

/// The OAuth error code (RFC 6749 §4.1.2.1, `access_denied`) a sign-in's
/// callback names on its page, when its redirect's `error` is shaped like
/// one. Never its `error_description`: whoever made the link wrote it, so
/// a page of the platform's would say an attacker's words (it goes to the
/// logs).
pub fn oauth_error(error: &str) -> Option<&str> {
    let shaped = !error.is_empty() && error.len() <= OAUTH_ERROR_MAX_BYTES && error.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    shaped.then_some(error)
}

/// The most bytes of a reference a request may bring (`Cf-Ray` is 16 hex
/// digits, a dash and an airport code); a longer one is not used.
pub const REFERENCE_MAX_BYTES: usize = 40;

/// A request's reference, as a log line and its refusal name it: its
/// `Cf-Ray` when Cloudflare set a well-formed one (so an operator finds
/// the request in Cloudflare's logs too), else `fallback` (random, made
/// by the cell).
pub fn reference(ray: Option<&str>, fallback: &str) -> String {
    let well_formed = |r: &&str| !r.is_empty() && r.len() <= REFERENCE_MAX_BYTES && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ray.filter(well_formed).unwrap_or(fallback).to_string()
}

/// A refusal's message past the edge: its own, or `redacted`'s with the
/// reference that finds its detail in the logs.
pub fn public_message(code: ErrorCode, message: &str, reference: &str) -> String {
    match redacted(code) {
        Some(words) => format!("{words} (reference {reference})"),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: the platform's own failure names none of its internals past
    /// the edge. Method: messages of the kinds the cell makes today (a
    /// JavaScript error, a D1 error, a vendor's answer, a binding's name,
    /// an internal id).
    #[test]
    fn internal_failures_say_only_a_sentence_and_a_reference() {
        let internals = [
            "facets.get: TypeError: Cannot read properties of undefined (reading 'fetch')\n    at Object.get (worker.js:1:2345)",
            "D1_ERROR: no such table: ledger_rows: SQLITE_ERROR",
            "WorkOS refused github's token (401): Invalid API key sk_live_abc",
            "the secret bound as STRIPE_KEY is missing from the store, or empty",
            "export 0f3a9c1e's archive is missing",
            "code.storage push: 500 {\"error\":\"internal\",\"trace\":\"/srv/app.go:120\"}",
        ];
        for message in internals {
            let said = public_message(ErrorCode::HostFailed, message, "8c1e2f3a4b5c6d7e-SJC");
            assert!(said.ends_with("(reference 8c1e2f3a4b5c6d7e-SJC)"), "{said}");
            for leak in ["TypeError", "worker.js", "D1_ERROR", "SQLITE", "sk_live", "STRIPE_KEY", "0f3a9c1e", "trace", "WorkOS", "code.storage"] {
                assert!(!said.contains(leak), "{message:?} said {said:?}");
            }
        }
    }

    /// Goal: a refusal the asker acts on keeps its words. Method: every
    /// other code, with a message that tells them what to do.
    #[test]
    fn refusals_for_the_asker_keep_their_message() {
        let theirs = [
            ErrorCode::InvalidRequest,
            ErrorCode::Unauthenticated,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::UnknownOperation,
            ErrorCode::NoCode,
            ErrorCode::AlreadyExists,
            ErrorCode::ConflictingBody,
            ErrorCode::Conflict,
            ErrorCode::TooLarge,
            ErrorCode::AppFailed,
            ErrorCode::UpstreamFailed,
            ErrorCode::RateLimited,
            ErrorCode::BudgetUsedUp,
            ErrorCode::StorageFull,
            ErrorCode::NodeFull,
            ErrorCode::WontWake,
            ErrorCode::NotConnected,
            ErrorCode::SchemaRefused,
            ErrorCode::Deploying,
        ];
        for code in theirs {
            assert_eq!(public_message(code, "name it in lower case", "r"), "name it in lower case", "{code:?}");
        }
    }

    /// Goal: a registry outage names none of the registry's internals
    /// past the edge: its own refusal (a storage error) is wrapped in
    /// `registry_unavailable`. Method: the message the cell makes of one.
    #[test]
    fn a_registry_outage_says_only_a_sentence_and_a_reference() {
        let message = "the identity registry did not answer (D1_ERROR: database is locked: SQLITE_BUSY); try again shortly";
        let said = public_message(ErrorCode::RegistryUnavailable, message, "8c1e2f3a4b5c6d7e-SJC");
        assert!(said.ends_with("(reference 8c1e2f3a4b5c6d7e-SJC)"), "{said}");
        assert!(said.contains("try again"), "{said}");
        for leak in ["D1_ERROR", "SQLITE", "locked"] {
            assert!(!said.contains(leak), "{said}");
        }
    }

    /// Goal: only the platform's own refusal is reread on its way out; an
    /// app's or a computer image's answer passes unread, whatever it
    /// holds. Method: the platform's mark, or not, on each redacted
    /// code's status, and the answers that are never refusals.
    #[test]
    fn only_the_platforms_marked_refusals_are_reread() {
        for code in [ErrorCode::HostFailed, ErrorCode::RegistryUnavailable] {
            assert!(rereads(true, code.status(), "application/json"), "{code:?}");
            assert!(!rereads(false, code.status(), "application/json"), "an app's own {code:?}");
        }
        assert!(!rereads(true, 500, "text/html"));
        assert!(!rereads(true, 404, "application/json"));
        assert!(!rereads(true, 200, "application/json"));
        for code in REDACTED {
            assert!(redacted(code).is_some(), "{code:?}");
        }
    }

    /// Goal: a sign-in's page names only an OAuth error code, never words
    /// a link brought. Method: the registered codes, then text, markup and
    /// an over-long one.
    #[test]
    fn a_sign_in_page_names_only_an_oauth_code() {
        for code in ["access_denied", "invalid_request", "server_error", "temporarily_unavailable"] {
            assert_eq!(oauth_error(code), Some(code));
        }
        let long = "a".repeat(OAUTH_ERROR_MAX_BYTES + 1);
        for words in ["", "your account is locked: call +1 555 0100", "<b>x</b>", "Access_Denied", long.as_str()] {
            assert_eq!(oauth_error(words), None, "{words:?}");
        }
    }

    /// Goal: a reference is Cloudflare's ray when it is one, and nothing a
    /// request could use to put words in a refusal. Method: a real ray,
    /// then a missing one and near misses, each falling back.
    #[test]
    fn a_reference_is_the_ray_or_ours() {
        assert_eq!(reference(Some("8c1e2f3a4b5c6d7e-SJC"), "ours"), "8c1e2f3a4b5c6d7e-SJC");
        let long = "a".repeat(REFERENCE_MAX_BYTES + 1);
        for bad in [None, Some(""), Some("x) contact evil.example"), Some("a\nb"), Some(long.as_str())] {
            assert_eq!(reference(bad, "ours"), "ours", "{bad:?}");
        }
    }
}
