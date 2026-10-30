//! Calls between releases (docs/MODEL.md, "Calls between releases"). A
//! deploy does not move the Worker and its Durable Objects at one instant:
//! for seconds (minutes, Cloudflare says, at worst) a Worker of the new
//! release can call an object still running the old one, and the other way
//! about, and one object another. A call whose shape changed between them
//! is then read wrong on one side: on 2026-10-20 a new Worker asked the old
//! Registry to exchange a sign-in's WorkOS code, the old Registry did, and
//! answered a shape the new Worker could not read, so the person saw an
//! error and their next try replayed a spent code.
//!
//! So every call into a platform object carries its caller's release (the
//! deployment's `FRAGMENT_DEPLOY_ID`, a Worker variable, so one value for
//! the Worker and every object of one deploy), and the object checks it
//! first, before it reads or does anything: another release's call, or
//! one that names none, is refused (503 `deploying`), so nothing was done
//! and asking again is safe. Its caller asks again a few times, briefly,
//! then answers the refusal: the deploy finishes, and the next try meets
//! one release on both sides.
//!
//! The refusal itself is the one shape every release must keep, as it is
//! what two releases say to each other: `HEADER` on the call, and a 503
//! carrying `REFUSED` back. Neither is ever renamed; the body (`deploying`
//! and its message) a caller reads only for its message.
//!
//! An object Cloudflare keeps busy keeps the code a deploy replaced: on the
//! preview on 2026-10-21, after 7 of its 17 deploys, a busy object (the
//! Registry, a computer, its agent fragment) ran the release before for
//! 283 to 297 seconds, refusing every call of the new one, and those
//! refusals themselves kept it busy. So a call carries when its
//! release was deployed too (`DEPLOYED`), and an object that refuses a call
//! from a release deployed after its own restarts (`restarts`): its next
//! instance runs the newer code, and the caller's next ask meets it. That
//! is the reset Cloudflare gives it minutes later, brought forward; a call
//! from a release deployed before the object's is refused and nothing
//! more, as its caller is the one a deploy replaced.

/// The header a call into a platform object carries its caller's release
/// in (cell/src/routed.rs stamps every one).
pub const HEADER: &str = "x-fragment-release";

/// The header an object's refusal of another release's call carries, its
/// own release as its value: its caller tells the refusal, which did
/// nothing, from an answer of `deploying` the object passed on from a call
/// of its own (which may have done something before it).
pub const REFUSED: &str = "x-fragment-release-refused";

/// The model intercept alone promises it has not dispatched to a provider.
/// Paired with 503 `deploying` and `REFUSED`, on a direct refusal of its
/// signing preflight only. Images may retry that request; an unmarked
/// error (including a nested object's refusal) makes no such promise.
pub const MODEL_NOT_DISPATCHED: &str = "x-fragment-model-not-dispatched";

/// Whether a model's preflight error has direct guard provenance. Its
/// HTTP status and error code remain the ordinary 503 `deploying`.
pub fn model_not_dispatched(code: fragment_proto::ErrorCode, refused_by: Option<&str>) -> bool {
    code == fragment_proto::ErrorCode::Deploying && refused_by.is_some_and(valid)
}

/// The header a call into a platform object carries when its caller's
/// release was deployed in (`FRAGMENT_DEPLOYED_AT`, whole seconds since
/// 1970, set by `cargo xtask deploy`), beside `HEADER`. It orders two
/// releases for `restarts` alone: whether a call is admitted is `admit`'s,
/// by the release, whatever the time. Never renamed, as `HEADER` is not.
pub const DEPLOYED: &str = "x-fragment-release-deployed";

/// A deploy time's most digits: seconds since 1970 until the year 33658.
pub const DEPLOYED_MAX_DIGITS: usize = 12;

/// A release's longest id: a commit's (`git rev-parse --short=12`), and a
/// deploy of an uncommitted tree's mark after it.
pub const ID_MAX_BYTES: usize = 64;

/// The CLI releases stable major.minor.patch versions only. Numeric order
/// is shared by CI's version selection and the CLI's update notice.
pub fn cli_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.');
    let mut version = [0; 3];
    for n in &mut version {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 20 || !part.bytes().all(|b| b.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0')) { return None; }
        *n = part.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

/// Whether `id` can name a release: 1 to `ID_MAX_BYTES` of ASCII letters,
/// digits, `.`, `_` and `-` (a header value as it is, and a log's).
pub fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= ID_MAX_BYTES && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A call refused for its release: the one its caller named (`None`: it
/// named none, as a caller from before releases were named does), and the
/// object's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skew {
    pub caller: Option<String>,
    pub callee: String,
}

impl Skew {
    /// What the refusal says (503 `deploying`): a person reads its start;
    /// the releases are an operator's, mapped to commits by `/healthz`.
    pub fn message(&self) -> String {
        let caller = self.caller.as_deref().unwrap_or("none named");
        format!("a deploy is going out; try again in a few seconds (a call from release {caller} reached release {})", self.callee)
    }
}

/// A deploy time as a call or a deployment names it (`DEPLOYED`,
/// `FRAGMENT_DEPLOYED_AT`): 1 to `DEPLOYED_MAX_DIGITS` ASCII digits, or
/// `None`, as a call from before deploy times were named carries none.
pub fn deployed(v: &str) -> Option<u64> {
    if v.is_empty() || v.len() > DEPLOYED_MAX_DIGITS || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}

/// Whether an object that refused a call (`admit`) restarts, so its next
/// instance runs the caller's code: only when the caller's release was
/// deployed after its own, both times known. One deployed before it, or at
/// the same second, or one that names no time (from before they were
/// named, or a fleet that sets none), leaves it as it is: restarting would
/// only start the same code again, at every such call.
pub fn restarts(caller_deployed: Option<u64>, own_deployed: Option<u64>) -> bool {
    matches!((caller_deployed, own_deployed), (Some(caller), Some(own)) if caller > own)
}

/// What an object restarting onto a newer release (`restarts`) ends every
/// request it is still answering with (cell/src/routed.rs
/// `restart_soon`), naming its own release: `restart_reason`. A call it
/// ends that was another release's was refused before it did anything
/// (`admit`), so its caller asks again as at a refusal (`restarted_by`); a
/// call of its own release may have acted, and is the error it was. The
/// restart's timer runs late under load, past the caller's next ask
/// (`AGAIN_AFTER_MS`), so the ask it ends is often that one.
pub const RESTARTING: &str = "a call from a release deployed after this one's: restarting onto it";

/// The reason an object running `own` restarts with.
pub fn restart_reason(own: &str) -> String {
    assert!(valid(own), "an object's own release is a valid id, not {own:?}");
    format!("{RESTARTING} (from {own})")
}

/// The release that ended a call by restarting (`restart_reason`), read
/// from the call's error as the runtime gave it (a JS error's text, any
/// prefix): `None` for any other error, and for a restart's that named no
/// valid release (one from before restarts named theirs).
pub fn restarted_by(error: &str) -> Option<&str> {
    let after = &error[error.find(RESTARTING)? + RESTARTING.len()..];
    let own = after.strip_prefix(" (from ")?.split(')').next()?;
    valid(own).then_some(own)
}

/// The object's check of a call, the first thing it does: admitted only
/// from its own release (`callee`). Another release, newer or older, is
/// refused, and so is a call that names none or one malformed (when each
/// was deployed decides only whether the object restarts: `restarts`).
/// `callee` is the object's own, checked as the isolate starts
/// (cell/src/config.rs).
pub fn admit(caller: Option<&str>, callee: &str) -> Result<(), Skew> {
    assert!(valid(callee), "an object's own release is a valid id, not {callee:?}");
    match caller {
        Some(c) if c == callee => Ok(()),
        // kept for the message only as a valid id: a malformed one is named as none
        named => Err(Skew { caller: named.filter(|c| valid(c)).map(str::to_string), callee: callee.to_string() }),
    }
}

/// The waits before each ask again after a refusal, in milliseconds: a
/// deploy's skew is seconds, and the person waits through these.
pub const AGAIN_AFTER_MS: [u64; 3] = [250, 500, 1000];

/// The most times one call is asked: the first ask, and one after each
/// wait. Past them the refusal is the answer (503 `deploying`).
pub const ASKS: usize = AGAIN_AFTER_MS.len() + 1;

/// The wait before asking again once `refused` asks (from 1) were refused,
/// or `None` once `ASKS` were: then the refusal stands.
pub fn again_after_ms(refused: usize) -> Option<u64> {
    refused.checked_sub(1).and_then(|i| AGAIN_AFTER_MS.get(i).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid: a call from the object's own release is admitted, whatever
    /// the id looks like (`dev`, a commit, a dirty tree's).
    #[test]
    fn its_own_release_is_admitted() {
        for own in ["dev", "bc8b87e7a1b2", "bc8b87e7a1b2-dirty-1760918400"] {
            assert_eq!(admit(Some(own), own), Ok(()), "{own}");
        }
    }

    /// Invalid, both ways: a newer caller at an older object and an older
    /// caller at a newer one are refused alike, each naming both releases.
    #[test]
    fn another_release_is_refused_both_ways() {
        let (old, new) = ("33e323cd0a1b", "bc8b87e7a1b2");
        let skew = admit(Some(new), old).expect_err("a new Worker at an old object");
        assert_eq!(skew, Skew { caller: Some(new.into()), callee: old.into() });
        assert!(skew.message().starts_with("a deploy is going out; try again in a few seconds"));
        assert!(skew.message().contains(new) && skew.message().contains(old));
        let skew = admit(Some(old), new).expect_err("an old Worker at a new object");
        assert_eq!(skew, Skew { caller: Some(old.into()), callee: new.into() });
        // a prefix, a case, a dirty tree of the same commit: another release
        for other in ["bc8b87e7", "BC8B87E7A1B2", "bc8b87e7a1b2-dirty-1"] {
            assert!(admit(Some(other), new).is_err(), "{other}");
        }
    }

    /// Invalid: a call that names no release (one from before releases
    /// were named: the first deploy of this check meets such callers), an
    /// empty one, or one malformed is refused as another release's; a
    /// malformed one is never echoed into the message.
    #[test]
    fn a_missing_or_malformed_release_is_refused() {
        let own = "bc8b87e7a1b2";
        assert_eq!(admit(None, own), Err(Skew { caller: None, callee: own.into() }));
        assert!(admit(None, own).unwrap_err().message().contains("from release none named"));
        for bad in ["", "bc8b87e7a1b2\r\nx-evil: 1", "release with spaces", &"a".repeat(ID_MAX_BYTES + 1)] {
            assert_eq!(admit(Some(bad), own), Err(Skew { caller: None, callee: own.into() }), "{bad:?}");
        }
    }

    /// The object's own release is checked as its isolate starts; one that
    /// is not an id is a host fault, never a release that admits.
    #[test]
    #[should_panic(expected = "an object's own release is a valid id")]
    fn an_objects_own_release_is_valid() {
        let _ = admit(Some(""), "");
    }

    /// What two releases say to each other is never renamed: a release that
    /// called it otherwise would be refused by every other as naming none,
    /// and would not know their refusals for what they are.
    #[test]
    fn the_refusal_between_releases_keeps_its_names() {
        assert_eq!(HEADER, "x-fragment-release");
        assert_eq!(REFUSED, "x-fragment-release-refused");
        assert_eq!(DEPLOYED, "x-fragment-release-deployed");
        assert_eq!(fragment_proto::ErrorCode::Deploying.status(), 503);
        assert_eq!(serde_json::to_value(fragment_proto::ErrorCode::Deploying).unwrap(), "deploying");
    }

    #[test]
    fn only_a_direct_preflight_refusal_promises_no_model_dispatch() {
        use fragment_proto::ErrorCode;
        assert_eq!(MODEL_NOT_DISPATCHED, "x-fragment-model-not-dispatched");
        assert!(model_not_dispatched(ErrorCode::Deploying, Some("old-release")));
        // A nested deploying error and a provider's 503 are not proof that
        // the original model request did nothing. Neither is retried.
        assert!(!model_not_dispatched(ErrorCode::Deploying, None));
        assert!(!model_not_dispatched(ErrorCode::UpstreamFailed, Some("old-release")));
        for bad in ["", "release with spaces", "old\r\nx-forged: 1"] {
            assert!(!model_not_dispatched(ErrorCode::Deploying, Some(bad)));
        }
    }

    #[test]
    fn ids_are_header_safe() {
        assert!(valid("dev") && valid(&"f".repeat(ID_MAX_BYTES)) && valid("a.b_c-1"));
        assert!(!valid("") && !valid(&"f".repeat(ID_MAX_BYTES + 1)) && !valid("a b") && !valid("é") && !valid("a\n"));
    }

    /// Replay: a refused call is asked again after each wait, `ASKS` times
    /// in all and about two seconds, and never once more.
    #[test]
    fn a_refused_call_is_asked_again_a_bounded_number_of_times() {
        let waits: Vec<u64> = (1..).map_while(again_after_ms).collect();
        assert_eq!(waits, AGAIN_AFTER_MS);
        assert_eq!(waits.len() + 1, ASKS, "the first ask, and one after each wait");
        assert_eq!(ASKS, 4);
        assert_eq!(waits.iter().sum::<u64>(), 1_750, "a person waits under two seconds before the refusal");
        assert_eq!(again_after_ms(0), None, "nothing refused, nothing to wait for");
        assert_eq!(again_after_ms(ASKS), None);
        assert_eq!(again_after_ms(usize::MAX), None);
    }

    /// Restart: the check keeps no state, so an object that restarts on the
    /// new release admits the call it refused a moment before.
    #[test]
    fn a_call_refused_before_the_deploy_finishes_is_admitted_after() {
        let (old, new) = ("33e323cd0a1b", "bc8b87e7a1b2");
        assert!(admit(Some(new), old).is_err(), "the object still runs the old release");
        assert_eq!(admit(Some(new), new), Ok(()), "and once restarted on the new one, the same call is admitted");
    }

    /// Valid and invalid: a deploy time is seconds as digits, and anything
    /// else names none (never a time read from part of it).
    #[test]
    fn a_deploy_time_is_whole_seconds() {
        assert_eq!(deployed("1792616138"), Some(1_792_616_138));
        assert_eq!(deployed("0"), Some(0));
        assert_eq!(deployed(&"9".repeat(DEPLOYED_MAX_DIGITS)), Some(999_999_999_999));
        for bad in ["", " 1792616138", "1792616138 ", "-1", "+1", "1.5", "17926e3", "0x10", &"9".repeat(DEPLOYED_MAX_DIGITS + 1), "١٢"] {
            assert_eq!(deployed(bad), None, "{bad:?}");
        }
    }

    /// Valid: the object refusing a call from a release deployed after its
    /// own restarts onto it. Invalid: one deployed before it (an old caller
    /// at a new object, whose restart would start the same code again at
    /// every such call), at the same second, or naming no time, leaves the
    /// object as it is.
    #[test]
    fn only_a_newer_release_restarts_the_object() {
        let (old, new) = (1_792_612_609, 1_792_615_843);
        assert!(restarts(Some(new), Some(old)), "a new Worker at an old object");
        assert!(!restarts(Some(old), Some(new)), "an old Worker at a new object");
        assert!(!restarts(Some(new), Some(new)), "one second, two releases: neither is newer");
        assert!(!restarts(None, Some(old)), "a caller from before deploy times were named");
        assert!(!restarts(Some(new), None), "an object of a fleet that sets none (dev)");
        assert!(!restarts(None, None));
    }

    /// Valid: a restart's reason names the release that restarted, read
    /// back from the error a caller sees, the runtime's prefix and all.
    /// Invalid: any other error, a restart's from before reasons named a
    /// release (the deploy that brings this in meets those once), or one
    /// naming no valid release, is no restart's: the caller's error stands.
    #[test]
    fn a_restart_names_the_release_that_restarted() {
        let reason = restart_reason("069479198b79");
        assert_eq!(restarted_by(&reason), Some("069479198b79"));
        assert_eq!(restarted_by(&format!("Error: {reason}")), Some("069479198b79"));
        assert_eq!(restarted_by(&format!("{RESTARTING}")), None, "the reason before it named a release");
        assert_eq!(restarted_by(&format!("Error: {RESTARTING}")), None);
        assert_eq!(restarted_by(&format!("{RESTARTING} (from )")), None);
        assert_eq!(restarted_by(&format!("{RESTARTING} (from a b)")), None);
        assert_eq!(restarted_by("Error: Network connection lost."), None);
        assert_eq!(restarted_by(""), None);
    }

    /// Replay: a newer caller's refusal restarts the old object once; the
    /// restarted object, of the caller's release, admits it, and its
    /// restart asks nothing more of it.
    #[test]
    fn a_restart_ends_the_refusals_it_was_for() {
        let ((old, old_at), (new, new_at)) = (("069479198b79", 1_792_612_609), ("5083ebcaeb1b", 1_792_615_843));
        assert!(admit(Some(new), old).is_err() && restarts(Some(new_at), Some(old_at)), "refused, and the object restarts");
        assert_eq!(admit(Some(new), new), Ok(()), "its next instance, the deploy's, admits the same call");
        // an old Worker still calling it is refused, and restarts nothing
        assert!(admit(Some(old), new).is_err() && !restarts(Some(old_at), Some(new_at)));
    }
}
