//! While locked, ask a key service for the disk key, in the background
//! and with backoff: `GET /nonce`, this pod's status bound to that
//! nonce, `POST /release`, then open the volume with the answer through
//! the same `open_volume` the owner's `/unlock` uses. The owner's
//! `/init` and `/unlock` keep working the whole time; whichever opens
//! first wins and the other is refused by the state check.
//!
//! The key service's address comes from the host (the config, DNS, the
//! network) and is not trusted. A wrong or hostile key service can only
//! fail the unlock; it cannot learn or change anything:
//! - What it learns is this pod's status for a nonce it chose: the same
//!   document `/status` gives anyone for any nonce. No secret is sent.
//! - What it can make happen is an attempt to open the volume with a
//!   key, and the key is used only if all of these hold: the owner the
//!   pod names signed a grant for the boot key that sealed it; the seal
//!   is HPKE auth mode, so the sealer holds that boot key's secret,
//!   which exists only inside a key service boot the owner attested
//!   before granting; it is sealed to this boot's key; it opens the
//!   LUKS volume; and the owner recorded inside the volume is the one
//!   the pod names (else the volume is closed again). Without the grant
//!   and the auth seal, a host could hand the pod a volume of its own
//!   making with the owner's name inside and the matching key; the pod
//!   would open it, and what the owner then wrote would be readable by
//!   whoever made it.
//! - It cannot hold the pod: answers are bounded in size and time, one
//!   attempt runs at a time, and the owner's routes stay open.

use crate::error::ApiError;
use crate::fetch::{self, FetchError};
use crate::keyreq;
use crate::service::Service;
use fragment_pod::unlock::binding::State;
use fragment_pod::keyd::delivery::{self, Delivery, ReleaseRequest};
use fragment_pod::keyd::KeydError;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::wire::ErrorBody;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// Nothing to ask for: the volume is open (by the owner, perhaps) or blank.
    NotLocked(State),
    /// Locked, but no owner hint names whose key to ask for.
    NoOwner,
    Unreachable(FetchError),
    /// The key service said no: its code, and its detail made printable.
    Refused { status: u16, code: String, detail: String },
    /// The answer is not the protocol's.
    Garbled(String),
    /// The delivery failed its grant or seal: not the owner's key service.
    Delivery(KeydError),
    /// This pod could not make its status, or the volume refused the key.
    Pod(ApiError),
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLocked(s) => write!(f, "the pod is {}; nothing to ask for", s.as_str()),
            Self::NoOwner => write!(f, "no owner hint names whose key to ask for"),
            Self::Unreachable(e) => write!(f, "key service unreachable: {e}"),
            Self::Refused { status, code, detail } => write!(f, "key service refused (HTTP {status}): {code}: {detail}"),
            Self::Garbled(e) => write!(f, "key service answer: {e}"),
            Self::Delivery(e) => write!(f, "key service delivery refused: {e}"),
            Self::Pod(e) => write!(f, "{}: {}", e.code, e.detail),
        }
    }
}

impl std::error::Error for AskError {}

/// The key service's words go to our log: printable ASCII, and short.
fn printable(text: &str) -> String {
    text.chars().take(200).map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' }).collect()
}

fn refused(status: u16, body: &[u8]) -> AskError {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(e) => AskError::Refused { status, code: printable(&e.error), detail: printable(&e.detail) },
        Err(_) => AskError::Refused { status, code: "unknown".into(), detail: printable(&String::from_utf8_lossy(body)) },
    }
}

fn call(addr: &str, method: &str, path: &str, body: &[u8]) -> Result<Vec<u8>, AskError> {
    let (status, answer) = fetch::request(addr, method, path, body).map_err(AskError::Unreachable)?;
    if status != 200 {
        return Err(refused(status, &answer));
    }
    Ok(answer)
}

/// One attempt. Ok means the volume is open.
pub fn attempt(s: &Service, addr: &str) -> Result<(), AskError> {
    let state = s.volume.state().map_err(|e| AskError::Pod(ApiError::new(500, crate::error::code::VOLUME_FAILED, e.to_string())))?;
    if state != State::Locked {
        return Err(AskError::NotLocked(state));
    }
    let owner = s.owner(State::Locked).ok_or(AskError::NoOwner)?;
    let nonce = delivery::nonce_from_json(&call(addr, "GET", "/nonce", b"")?).map_err(|e| AskError::Garbled(e.to_string()))?;
    let status = s.status_doc(nonce).map_err(AskError::Pod)?;
    let body = ReleaseRequest { nonce, status }.to_json().map_err(|e| AskError::Garbled(e.to_string()))?;
    let answer = call(addr, "POST", "/release", &body)?;
    let d = Delivery::from_json(&answer).map_err(|e| AskError::Garbled(e.to_string()))?;
    let key = d.open(&s.boot, &owner).map_err(AskError::Delivery)?;
    keyreq::open_volume(s, Kind::Unlock, &owner, key).map_err(AskError::Pod)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
}

/// A restarted key service needs its owner to deposit again, which may
/// take hours; asking every five minutes costs it nothing.
pub const BACKOFF: Backoff = Backoff { first: Duration::from_secs(2), max: Duration::from_secs(300) };

/// Ask until the volume is open (by the key service, or by anyone: then
/// it ends with NotLocked), or `max_attempts` have failed.
pub fn run(s: &Service, addr: &str, backoff: Backoff, max_attempts: Option<usize>) -> Result<(), AskError> {
    let mut delay = backoff.first;
    let mut attempts = 0usize;
    loop {
        let e = match attempt(s, addr) {
            Ok(()) => {
                eprintln!("pod-init: opened with the key from the key service at {addr}");
                return Ok(());
            }
            Err(e @ AskError::NotLocked(_)) => return Err(e),
            Err(e) => e,
        };
        attempts += 1;
        eprintln!("pod-init: asking the key service (attempt {attempts}): {e}; next in {}s", delay.as_secs());
        if max_attempts.is_some_and(|m| attempts >= m) {
            return Err(e);
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(backoff.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_services_words_are_made_printable_and_short() {
        assert_eq!(printable("ok\x1b[2J\nfake"), "ok?[2J?fake");
        assert_eq!(printable(&"x".repeat(500)).len(), 200);
        assert_eq!(refused(500, b"\xff not json"), AskError::Refused { status: 500, code: "unknown".into(), detail: "? not json".into() });
    }
}
