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
        _ => None,
    }
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
            ErrorCode::RegistryUnavailable,
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
