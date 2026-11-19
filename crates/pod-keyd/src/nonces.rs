//! Nonces for `/release`. A pod's status must be bound to one made in
//! the last minute, so a status captured earlier cannot be replayed.
//!
//! `GET /nonce` keeps nothing. A nonce is the time it was made, eight
//! random bytes, and a MAC over both under a key made when the service
//! starts, so anyone may ask for any number and none takes room. (A
//! pool of issued nonces has a limit, and whoever asks fastest fills it
//! and keeps every pod from asking.)
//!
//! That time is the latest this boot has read from its clock, not the
//! clock's own (`Challenges::time`). The host sets the clock: were a
//! nonce's minute measured by it, a nonce forgotten when its minute was
//! over would be good again once the host moved the clock back.
//!
//! What is kept is the nonces releases were made for, by owner, until
//! they expire: a release is made once for a nonce. Only a boot of an
//! owner's pod that passed every check adds one, and only to that
//! owner's own few, so no one fills them for anyone else.

use crate::error::Refusal;
use fragment_pod::unlock::binding::NONCE_LEN;
use fragment_pod::unlock::keys::{self, KeyError};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use zeroize::Zeroizing;

pub const NONCE_TTL: i64 = 60;
/// Releases made for one owner within NONCE_TTL. pod-init asks with
/// backoff from 2 s, so a pod asks six times in a minute at most.
pub const MAX_SPENT: usize = 16;

const DOMAIN: &[u8] = b"pod-keyd-nonce-v1\0";
const TIME_LEN: usize = 8;
/// The time, then the random bytes; the rest of a nonce is the tag.
const HEAD_LEN: usize = 16;
const _: () = assert!(NONCE_LEN == HEAD_LEN + 16);

/// When `nonce` says it was made: believed only after `Challenges::check`.
fn made_at(nonce: &[u8; NONCE_LEN]) -> i64 {
    let mut time = [0u8; TIME_LEN];
    time.copy_from_slice(&nonce[..TIME_LEN]);
    i64::from_be_bytes(time)
}

/// How long ago `at` was; None for a clock so far off the sum overflows.
fn age(at: i64, now: i64) -> Option<i64> {
    now.checked_sub(at)
}

/// The key that makes this boot's nonces, and the latest time it has
/// seen; both live only in this process.
pub struct Challenges {
    key: Zeroizing<[u8; 32]>,
    latest: AtomicI64,
}

impl Challenges {
    pub fn generate() -> Result<Self, KeyError> {
        Ok(Self { key: keys::random()?, latest: AtomicI64::new(i64::MIN) })
    }

    /// The time nonces go by: `now`, or the latest time seen before it
    /// if the clock has gone back. It never goes down.
    fn time(&self, now: i64) -> i64 {
        self.latest.fetch_max(now, Ordering::SeqCst).max(now)
    }

    fn mac(&self, head: &[u8]) -> Result<Hmac<Sha256>, Refusal> {
        // HMAC takes a key of any length; the error is only the type's.
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.as_ref()).map_err(|e| Refusal::Internal(e.to_string()))?;
        mac.update(DOMAIN);
        mac.update(head);
        Ok(mac)
    }

    pub fn issue(&self, now: i64) -> Result<[u8; NONCE_LEN], Refusal> {
        let random = keys::random::<{ HEAD_LEN - TIME_LEN }>().map_err(|e| Refusal::Internal(e.to_string()))?;
        let mut nonce = [0u8; NONCE_LEN];
        nonce[..TIME_LEN].copy_from_slice(&self.time(now).to_be_bytes());
        nonce[TIME_LEN..HEAD_LEN].copy_from_slice(random.as_ref());
        let tag = self.mac(&nonce[..HEAD_LEN])?.finalize().into_bytes();
        nonce[HEAD_LEN..].copy_from_slice(&tag[..NONCE_LEN - HEAD_LEN]);
        Ok(nonce)
    }

    /// One of this boot's (the tag compared in constant time), made
    /// within the last minute of the time nonces go by. The answer is
    /// that time, for `Spent`.
    pub fn check(&self, nonce: &[u8; NONCE_LEN], now: i64) -> Result<i64, Refusal> {
        self.mac(&nonce[..HEAD_LEN])?.verify_truncated_left(&nonce[HEAD_LEN..]).map_err(|_| Refusal::UnknownNonce)?;
        let time = self.time(now);
        match age(made_at(nonce), time) {
            Some(age) if (0..NONCE_TTL).contains(&age) => Ok(time),
            _ => Err(Refusal::UnknownNonce),
        }
    }
}

/// The nonces releases were made for, by owner. Its `now` is the time
/// `Challenges::check` answered, which never goes down: a nonce
/// forgotten here is one `check` refuses from then on.
#[derive(Default)]
pub struct Spent {
    by_owner: HashMap<[u8; 32], Vec<[u8; NONCE_LEN]>>,
}

impl Spent {
    /// One is kept for as long as `check` could pass it, and so also if
    /// `now` were behind it.
    fn forget_expired(&mut self, now: i64) {
        self.by_owner.retain(|_, spent| {
            spent.retain(|n| age(made_at(n), now).is_none_or(|age| age < NONCE_TTL));
            !spent.is_empty()
        });
    }

    pub fn contains(&self, owner: &[u8; 32], nonce: &[u8; NONCE_LEN]) -> bool {
        self.by_owner.get(owner).is_some_and(|spent| spent.contains(nonce))
    }

    /// Spends `nonce` for a release to `owner`'s pod. When the owner's
    /// are full, refuse rather than forget one: a forgotten one could be
    /// used again.
    pub fn spend(&mut self, owner: [u8; 32], nonce: [u8; NONCE_LEN], now: i64) -> Result<(), Refusal> {
        self.forget_expired(now);
        let spent = self.by_owner.entry(owner).or_default();
        if spent.contains(&nonce) {
            return Err(Refusal::UnknownNonce);
        }
        if spent.len() >= MAX_SPENT {
            return Err(Refusal::TooManyReleases);
        }
        spent.push(nonce);
        Ok(())
    }

    pub fn held(&self) -> usize {
        self.by_owner.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonce_is_this_boots_and_good_only_for_a_while() {
        let c = Challenges::generate().unwrap();
        let a = c.issue(100).unwrap();
        assert_ne!(a, c.issue(100).unwrap(), "two in one second differ");
        assert_eq!(c.check(&a, 100), Ok(100));
        assert_eq!(c.check(&a, 100 + NONCE_TTL - 1), Ok(100 + NONCE_TTL - 1));
        assert_eq!(c.check(&a, 100 + NONCE_TTL), Err(Refusal::UnknownNonce), "expired");
        assert_eq!(c.check(&[0; NONCE_LEN], 0), Err(Refusal::UnknownNonce), "never issued");
        let other = Challenges::generate().unwrap();
        assert_eq!(other.check(&a, 100), Err(Refusal::UnknownNonce), "another boot's");
    }

    #[test]
    fn nonce_time_does_not_go_back_with_the_clock() {
        let c = Challenges::generate().unwrap();
        let a = c.issue(100).unwrap();
        assert_eq!(c.check(&a, 99), Ok(100), "a clock a second behind: the time is the latest seen");
        assert_eq!(c.check(&a, 100 + NONCE_TTL), Err(Refusal::UnknownNonce), "expired");
        assert_eq!(c.check(&a, 101), Err(Refusal::UnknownNonce), "and still, the clock back inside its minute");
        assert_eq!(c.check(&a, i64::MIN), Err(Refusal::UnknownNonce));
        // One made while the clock is behind is made at the latest time
        // seen, and its minute runs from there.
        let b = c.issue(101).unwrap();
        assert_eq!(made_at(&b), 100 + NONCE_TTL);
        assert_eq!(c.check(&b, 101), Ok(100 + NONCE_TTL));
        assert_eq!(c.check(&b, 100 + 2 * NONCE_TTL - 1), Ok(100 + 2 * NONCE_TTL - 1));
        assert_eq!(c.check(&b, 100 + 2 * NONCE_TTL), Err(Refusal::UnknownNonce));
        assert_eq!(c.check(&b, i64::MAX), Err(Refusal::UnknownNonce), "no overflow at the far end");
    }

    #[test]
    fn a_nonce_edited_anywhere_is_not_ours() {
        let c = Challenges::generate().unwrap();
        let a = c.issue(100).unwrap();
        for i in 0..NONCE_LEN {
            let mut edited = a;
            edited[i] ^= 1;
            assert_eq!(c.check(&edited, 100), Err(Refusal::UnknownNonce), "byte {i}");
        }
        // The time moved forward by hand, to outlive the minute.
        let mut later = a;
        later[..TIME_LEN].copy_from_slice(&500i64.to_be_bytes());
        assert_eq!(c.check(&later, 500), Err(Refusal::UnknownNonce));
    }

    #[test]
    fn a_nonce_is_spent_once_and_forgotten_when_it_expires() {
        let c = Challenges::generate().unwrap();
        let mut s = Spent::default();
        let a = c.issue(100).unwrap();
        assert!(!s.contains(&[1; 32], &a));
        assert_eq!(s.spend([1; 32], a, 101), Ok(()));
        assert!(s.contains(&[1; 32], &a));
        assert_eq!(s.spend([1; 32], a, 101), Err(Refusal::UnknownNonce), "spent");
        assert!(!s.contains(&[2; 32], &a), "by owner");
        s.forget_expired(50);
        assert_eq!(s.held(), 1, "kept while the time is behind it");
        s.forget_expired(100 + NONCE_TTL);
        assert_eq!(s.held(), 0);
    }

    #[test]
    fn one_owner_filling_theirs_leaves_room_for_another() {
        let c = Challenges::generate().unwrap();
        let mut s = Spent::default();
        for _ in 0..MAX_SPENT {
            s.spend([1; 32], c.issue(100).unwrap(), 100).unwrap();
        }
        assert_eq!(s.spend([1; 32], c.issue(100).unwrap(), 100), Err(Refusal::TooManyReleases));
        assert_eq!(s.spend([2; 32], c.issue(100).unwrap(), 100), Ok(()));
        assert_eq!(s.spend([1; 32], c.issue(100 + NONCE_TTL).unwrap(), 100 + NONCE_TTL), Ok(()), "a minute on");
        assert_eq!(s.held(), 1, "and the old ones are gone");
    }
}
