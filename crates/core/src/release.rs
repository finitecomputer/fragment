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

/// The header a call into a platform object carries its caller's release
/// in (cell/src/routed.rs stamps every one).
pub const HEADER: &str = "x-fragment-release";

/// The header an object's refusal of another release's call carries, its
/// own release as its value: its caller tells the refusal, which did
/// nothing, from an answer of `deploying` the object passed on from a call
/// of its own (which may have done something before it).
pub const REFUSED: &str = "x-fragment-release-refused";

/// A release's longest id: a commit's (`git rev-parse --short=12`), and a
/// deploy of an uncommitted tree's mark after it.
pub const ID_MAX_BYTES: usize = 64;

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

/// The object's check of a call, the first thing it does: admitted only
/// from its own release (`callee`). Another release, newer or older (the
/// two are not ordered: a release is a commit), is refused, and so is a
/// call that names none or one malformed. `callee` is the object's own,
/// checked as the isolate starts (cell/src/config.rs).
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
        assert_eq!(fragment_proto::ErrorCode::Deploying.status(), 503);
        assert_eq!(serde_json::to_value(fragment_proto::ErrorCode::Deploying).unwrap(), "deploying");
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
}
