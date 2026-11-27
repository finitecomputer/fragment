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

use fragment_proto::{ErrorBody, ErrorCode};

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

/// Marks a refusal one of the platform's objects answered (the cell's
/// `CellError::response`), its value the refusal's code (`mark`), so the
/// edge says it from the mark alone (`said`), whatever its body holds or
/// however long it is. Never a foreign answer's: an app's, a computer
/// image's or a vendor's is stripped of it where it enters the platform
/// (`forged`), and every answer leaving the Worker is stripped of it (the
/// cell's `error::public_answer`), so no one outside sends or reads it.
pub const MARK: &str = "x-fragment-platform-refusal";

/// `MARK`'s value on a refusal of `code`: the code's wire name.
pub fn mark(code: ErrorCode) -> String {
    let quoted = serde_json::to_string(&code).expect("an ErrorCode serializes");
    quoted.trim_matches('"').to_string()
}

/// The code a marked answer's `MARK` names. Only the platform sets one,
/// in this release (an object refuses another release's call,
/// docs/MODEL.md), so an unknown one is no answer to trust: `host_failed`,
/// said as the platform's own failure is, none of its body.
pub fn marked(mark: &str) -> ErrorCode {
    // as a JSON string: a mark with a quote or a backslash in it is none
    serde_json::from_slice(format!("\"{mark}\"").as_bytes()).unwrap_or(ErrorCode::HostFailed)
}

/// What a marked answer (`mark`, its `MARK`) says past the platform's
/// edge, or to an app's own code (its `env.FILES`, `env.SEARCH`: it may
/// pass it on to anyone): a redacted code's fixed sentence and
/// `reference`, at its code's status (`ErrorCode::status`); `None` where
/// its own message is the asker's (`schema_refused`, a 500 too), and it
/// passes as it came. The mark decides, never the body: `read` is its
/// body when it could be read (bounded) as a refusal, and one too large,
/// not JSON or of another code says the sentence all the same, never
/// itself.
pub fn said(mark: &str, read: Option<&ErrorBody>, reference: &str) -> Option<ErrorBody> {
    let code = marked(mark);
    redacted(code)?;
    let read = read.filter(|b| b.error == code);
    Some(ErrorBody {
        error: code,
        message: public_message(code, "", reference),
        reason: read.and_then(|b| b.reason),
        retry_after_ms: read.and_then(|b| b.retry_after_ms),
    })
}

/// The names among a foreign answer's headers that it is stripped of
/// where it enters the platform (an app's facet, a computer's port, a
/// vendor's answer passed through): `MARK`, in any case, so nothing from
/// outside passes for the platform's own refusal, nor has its message
/// logged as one under a request's reference.
pub fn forged<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    names.into_iter().filter(|n| n.eq_ignore_ascii_case(MARK)).collect()
}

/// The codes `redacted` says words for.
pub const REDACTED: [ErrorCode; 2] = [ErrorCode::HostFailed, ErrorCode::RegistryUnavailable];

/// The most bytes of an OAuth error code a sign-in's page names.
pub const OAUTH_ERROR_MAX_BYTES: usize = 64;

/// The OAuth error code (RFC 6749 §4.1.2.1, `access_denied`) a sign-in's
/// callback names on its page, when its redirect's `error` is shaped like
/// one: a link anyone made may bring it, so it is checked, not trusted.
/// Its `error_description` is said only as `oauth_description` allows.
pub fn oauth_error(error: &str) -> Option<&str> {
    let shaped = !error.is_empty() && error.len() <= OAUTH_ERROR_MAX_BYTES && error.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    shaped.then_some(error)
}

/// The most characters of a sign-in provider's own description a
/// sign-in's page says (the identity door cuts its own at 200).
pub const OAUTH_DESCRIPTION_MAX_CHARS: usize = 300;

/// The `error_description` a sign-in's callback says on its page, beside
/// its code (`oauth_error`): why the sign-in did not finish, in the words
/// of the provider of that sign-in (the identity door's own explanations,
/// docs/self-host.md seam 4; WorkOS's). Only when the redirect's `state` is
/// the one this browser's own sign-in holds (`bound`, the cell's
/// `auth::trip_bound`): no one who made a link knows it, so an unbound
/// redirect's words are whoever made it, and go to the logs alone. Then
/// without control characters, and at most `OAUTH_DESCRIPTION_MAX_CHARS`
/// of it: the provider's words to the person, as OAuth means them (RFC
/// 6749 §4.1.2.1), never an answer's body. The page escapes it.
pub fn oauth_description(description: &str, bound: bool) -> Option<String> {
    if !bound {
        return None;
    }
    let said: String = description.chars().take(OAUTH_DESCRIPTION_MAX_CHARS).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let said = said.trim();
    (!said.is_empty()).then(|| said.to_string())
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

    /// Goal: a refusal the platform marked as its own failure says only a
    /// sentence and a reference, whatever its body: one too large to read
    /// (a long runtime diagnostic), not JSON, or naming another code, as
    /// much as a whole one. A marked refusal the asker acts on
    /// (`schema_refused`, a 500 too) passes as it came. Method: each
    /// redacted code's mark with each body, then every other code's mark.
    #[test]
    fn a_marked_refusal_is_said_from_its_mark_whatever_its_body() {
        let diagnostic = format!("TypeError: storage.list failed: D1_ERROR {}", "at worker.js:1:2345 ".repeat(5000));
        for code in REDACTED {
            let whole = ErrorBody { error: code, message: diagnostic.clone(), reason: None, retry_after_ms: Some(2000) };
            let other = ErrorBody { error: ErrorCode::InvalidRequest, message: "D1_ERROR".into(), reason: None, retry_after_ms: None };
            for (read, retry) in [(Some(&whole), Some(2000)), (None, None), (Some(&other), None)] {
                let said = said(&mark(code), read, "8c1e2f3a4b5c6d7e-SJC").expect("redacted");
                assert_eq!((said.error, said.error.status(), said.retry_after_ms), (code, code.status(), retry), "{read:?}");
                assert!(said.message.ends_with("(reference 8c1e2f3a4b5c6d7e-SJC)"), "{}", said.message);
                assert!(!said.message.contains("D1_ERROR") && !said.message.contains("worker.js"), "{}", said.message);
            }
        }
        let schema = ErrorBody { error: ErrorCode::SchemaRefused, message: "drop the column first".into(), reason: None, retry_after_ms: None };
        assert_eq!(said(&mark(ErrorCode::SchemaRefused), Some(&schema), "r"), None);
        assert_eq!(said(&mark(ErrorCode::SchemaRefused), None, "r"), None, "an unread schema refusal passes as it came");
        for code in [ErrorCode::Unauthenticated, ErrorCode::Forbidden, ErrorCode::NotFound, ErrorCode::UpstreamFailed, ErrorCode::NodeFull] {
            assert_eq!(said(&mark(code), None, "r"), None, "{code:?}");
        }
    }

    /// Goal: the mark names its refusal's code, and a mark that names none
    /// is said as the platform's own failure. Method: every code there,
    /// then a mark of no code (an older release's `1`, a typo).
    #[test]
    fn a_mark_names_its_code() {
        for code in REDACTED.into_iter().chain([ErrorCode::SchemaRefused, ErrorCode::NotFound, ErrorCode::AppFailed]) {
            assert_eq!(marked(&mark(code)), code);
        }
        assert_eq!(mark(ErrorCode::HostFailed), "host_failed");
        for unknown in ["1", "", "Host_Failed", "host failed"] {
            assert_eq!(marked(unknown), ErrorCode::HostFailed, "{unknown:?}");
            assert!(said(unknown, None, "r").is_some(), "{unknown:?}");
        }
        for code in REDACTED {
            assert!(redacted(code).is_some(), "{code:?}");
        }
    }

    /// Goal: an app (or a computer image, or a vendor) cannot pass its
    /// answer off as the platform's refusal: the mark it sent, in any
    /// case, is dropped where its answer enters, so the edge never says
    /// it as its own nor logs its words under a reference. Method: an app's headers
    /// with the mark forged two ways among headers that stay.
    #[test]
    fn a_foreign_answer_cannot_forge_the_platforms_mark() {
        let sent = ["content-type", "X-Fragment-Platform-Refusal", "x-fragment-refusal", "x-fragment-platform-refusal", "x-fragment-platform-refusals", "set-cookie"];
        assert_eq!(forged(sent), ["X-Fragment-Platform-Refusal", "x-fragment-platform-refusal"]);
        let kept: Vec<&str> = sent.into_iter().filter(|n| !forged([*n]).contains(n)).collect();
        assert!(!kept.iter().any(|n| n.eq_ignore_ascii_case(MARK)), "a forged refusal is said as the platform's: {kept:?}");
        assert!(forged(["content-type", ""]).is_empty());
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

    /// Goal: a sign-in's page says why it did not finish in the words of
    /// the provider of that very sign-in (the identity door's own
    /// explanations, WorkOS's), bounded, and never words a link brought.
    /// Method: the door's explanations bound to this browser's sign-in,
    /// then the same unbound, control characters, an over-long one and
    /// an empty one.
    #[test]
    fn a_sign_in_page_says_its_own_providers_words_alone() {
        for why in [
            "the sign-in provider has not verified the email \"jo@corp.e2e.test\", and this door vouches for no address in its domain",
            "the sign-in provider said: a failure the test asked for",
            "the sign-in provider's answer was not one to trust: the id_token's signature does not verify; start the sign-in again",
        ] {
            assert_eq!(oauth_description(why, true).as_deref(), Some(why));
            assert_eq!(oauth_description(why, false), None, "a link's words: {why:?}");
        }
        assert_eq!(oauth_description("line one\nline\u{7}two", true).as_deref(), Some("line one line two"));
        let long = "a".repeat(OAUTH_DESCRIPTION_MAX_CHARS + 50);
        assert_eq!(oauth_description(&long, true).map(|s| s.chars().count()), Some(OAUTH_DESCRIPTION_MAX_CHARS));
        for nothing in ["", "\n\t", "   "] {
            assert_eq!(oauth_description(nothing, true), None, "{nothing:?}");
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
