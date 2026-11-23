//! The door's one-time codes: what `authorize` hands the browser, and
//! `authenticate` takes back once, from the client it was issued to.
//!
//! A code is 32 bytes from the OS's generator, shown as hex, and kept here
//! by its SHA-256 alone (a memory dump holds no code that still works). It
//! is good for `CODE_TTL`, once: the first exchange that names it takes it
//! out, whatever that exchange is then told, so a replay (a callback sent
//! again, a code read from a log) finds nothing. It is bound to its client:
//! another client's exchange takes it and is refused, so it is never good
//! after that either.
//!
//! **In memory alone, by design.** A restart forgets every code: a sign-in
//! under way at that moment fails at the platform's callback, which says
//! to start again (the platform answers 400 "start again" to the door's
//! `invalid_grant`), and no code outlives the process that issued it, so
//! none can be replayed after a restart either. A sign-in takes a second;
//! keeping codes on disk would buy nothing but a replay window.
//!
//! At most `CODES_MAX` are held at once (codes issued in the last
//! `CODE_TTL`, taken or not); past it, the oldest goes. A tailnet member
//! asking `authorize` in a loop can push out another's code that way, which
//! the other's callback answers with "start again".

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::person::Person;

/// How long a code is good: the browser's redirect and the platform's
/// exchange take a second; ten minutes is OAuth's ceiling (RFC 6749 §4.1.2).
pub const CODE_TTL: Duration = Duration::from_secs(120);
/// The most codes held at once (each a few hundred bytes).
pub const CODES_MAX: usize = 4096;
/// A code's bytes, from the OS's generator.
pub const CODE_BYTES: usize = 32;
const _: () = assert!(CODE_BYTES >= 16, "a code is unguessable: at least 128 bits");
const _: () = assert!(CODE_TTL.as_secs() <= 600, "a code lives at most OAuth's ten minutes");

/// Why an exchange gets no person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodeRefusal {
    /// Never issued, issued by a door since restarted, already taken, or
    /// pushed out past `CODES_MAX`.
    #[error("this code is unknown or was used (codes live in the door's memory: a door restarted since forgot it); start the sign-in again")]
    Unknown,
    #[error("this code expired (codes are good for {} seconds); start the sign-in again", CODE_TTL.as_secs())]
    Expired,
    /// Issued to another client: taken all the same, so it is good for no one.
    #[error("this code was issued to another client")]
    OtherClient,
}

/// What a code stands for until it is taken.
struct Grant {
    client_id: String,
    person: Person,
    expires_at: Instant,
}

#[derive(Default)]
pub struct Codes {
    held: HashMap<[u8; 32], Grant>,
    /// Every code issued and not yet past its time or pushed out, oldest
    /// first, taken or not: what bounds `held`, and what `issue` sweeps.
    order: VecDeque<([u8; 32], Instant)>,
}

fn key_of(code: &[u8; CODE_BYTES]) -> [u8; 32] {
    Sha256::digest(code).into()
}

impl Codes {
    pub fn new() -> Codes {
        Codes::default()
    }

    /// How many codes wait to be taken.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// A code for `person`, good for `client_id` until `now + CODE_TTL`.
    /// `fresh` is the code's bytes, the OS's (`random_code`).
    pub fn issue(&mut self, fresh: [u8; CODE_BYTES], client_id: &str, person: Person, now: Instant) -> String {
        assert!(!client_id.is_empty(), "a code is issued to a client");
        // the oldest first: those past their time, then, at the cap, the
        // oldest still good
        while let Some((key, expires_at)) = self.order.front().copied() {
            let due = expires_at <= now || self.order.len() >= CODES_MAX;
            if !due {
                break;
            }
            self.order.pop_front();
            self.held.remove(&key);
        }
        let key = key_of(&fresh);
        let expires_at = now + CODE_TTL;
        let replaced = self.held.insert(key, Grant { client_id: client_id.to_string(), person, expires_at });
        assert!(replaced.is_none(), "32 bytes from the OS never repeat");
        self.order.push_back((key, expires_at));
        assert!(self.order.len() <= CODES_MAX, "at most CODES_MAX codes are held");
        assert!(self.held.len() <= self.order.len(), "every held code is in the order");
        hex::encode(fresh)
    }

    /// The person `code` stands for, taken out: once, by `client_id`
    /// (already authenticated by its secret), before its time is up.
    pub fn take(&mut self, code: &str, client_id: &str, now: Instant) -> Result<Person, CodeRefusal> {
        let mut bytes = [0u8; CODE_BYTES];
        if code.len() != 2 * CODE_BYTES || hex::decode_to_slice(code, &mut bytes).is_err() {
            return Err(CodeRefusal::Unknown);
        }
        let Some(grant) = self.held.remove(&key_of(&bytes)) else {
            return Err(CodeRefusal::Unknown);
        };
        if grant.expires_at <= now {
            return Err(CodeRefusal::Expired);
        }
        if grant.client_id != client_id {
            return Err(CodeRefusal::OtherClient);
        }
        Ok(grant.person)
    }
}

/// A code's bytes from the OS's generator. A generator that fails is a
/// broken host: the door stops rather than issue a guessable code.
pub fn random_code() -> [u8; CODE_BYTES] {
    let mut bytes = [0u8; CODE_BYTES];
    getrandom::fill(&mut bytes).expect("the OS's random generator answers");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> Person {
        Person { subject: "user_ada".into(), email: "ada@example.com".into() }
    }

    /// Goal: a code is good once, for its client, before its time.
    /// Method: issue, take (valid); take again (replay); take with a
    /// malformed code (invalid).
    #[test]
    fn a_code_is_taken_once() {
        let mut codes = Codes::new();
        let now = Instant::now();
        let code = codes.issue(random_code(), "cell", ada(), now);
        assert_eq!(code.len(), 2 * CODE_BYTES);
        assert_eq!(codes.take(&code, "cell", now), Ok(ada()));
        assert_eq!(codes.take(&code, "cell", now), Err(CodeRefusal::Unknown), "a replay finds nothing");
        assert_eq!(codes.take("not-a-code", "cell", now), Err(CodeRefusal::Unknown));
        assert_eq!(codes.take(&"z".repeat(64), "cell", now), Err(CodeRefusal::Unknown), "not hex");
        assert_eq!(codes.held(), 0);
    }

    /// Goal: a code past its time is refused, and taken (no second try).
    /// Method: take it one tick before and at its deadline.
    #[test]
    fn a_code_expires() {
        let mut codes = Codes::new();
        let now = Instant::now();
        let early = codes.issue(random_code(), "cell", ada(), now);
        let late = codes.issue(random_code(), "cell", ada(), now);
        assert_eq!(codes.take(&early, "cell", now + CODE_TTL - Duration::from_millis(1)), Ok(ada()));
        assert_eq!(codes.take(&late, "cell", now + CODE_TTL), Err(CodeRefusal::Expired));
        assert_eq!(codes.take(&late, "cell", now), Err(CodeRefusal::Unknown), "an expired code is gone, not waiting");
    }

    /// Goal: a code is its client's: another client's exchange burns it.
    /// Method: take it as another client, then as its own.
    #[test]
    fn a_code_is_bound_to_its_client() {
        let mut codes = Codes::new();
        let now = Instant::now();
        let code = codes.issue(random_code(), "cell", ada(), now);
        assert_eq!(codes.take(&code, "other", now), Err(CodeRefusal::OtherClient));
        assert_eq!(codes.take(&code, "cell", now), Err(CodeRefusal::Unknown), "burnt by the other client's try");
    }

    /// Goal: at most CODES_MAX codes are held; the oldest goes first, and
    /// expired ones are swept as new ones come.
    /// Method: issue CODES_MAX + 1; the first is gone, the second good.
    /// Then, past their time, one more issue sweeps them all.
    #[test]
    fn codes_are_bounded() {
        let mut codes = Codes::new();
        let now = Instant::now();
        let first = codes.issue(random_code(), "cell", ada(), now);
        let second = codes.issue(random_code(), "cell", ada(), now);
        for _ in 2..CODES_MAX {
            codes.issue(random_code(), "cell", ada(), now);
        }
        assert_eq!(codes.held(), CODES_MAX);
        codes.issue(random_code(), "cell", ada(), now);
        assert_eq!(codes.held(), CODES_MAX, "still at the cap");
        assert_eq!(codes.take(&first, "cell", now), Err(CodeRefusal::Unknown), "the oldest was pushed out");
        assert_eq!(codes.take(&second, "cell", now), Ok(ada()));
        let fresh = codes.issue(random_code(), "cell", ada(), now + CODE_TTL);
        assert_eq!(codes.held(), 1, "every older code was past its time");
        assert_eq!(codes.take(&fresh, "cell", now + CODE_TTL), Ok(ada()));
    }
}
