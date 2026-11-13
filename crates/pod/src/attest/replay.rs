//! Replaying the event log: each RTMR starts at zero and every event
//! extends it, `rtmr = SHA-384(rtmr || digest)`, as TDG.MR.RTMR.EXTEND
//! does. If the replay equals the quote's RTMRs, the log is the boot
//! the quote measured; the log's descriptions are then trustworthy only
//! as far as each event's digest covers what its description says (see
//! [`super::describe::coverage`]).

use super::eventlog::EventLog;
use super::report::Mr;
use sha2::{Digest, Sha384};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    /// An event extends an RTMR but carries no SHA-384 digest.
    NoSha384 { event: usize },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSha384 { event } => write!(f, "event {event} has no SHA-384 digest"),
        }
    }
}

impl std::error::Error for ReplayError {}

pub fn extend(mr: &Mr, digest: &[u8; 48]) -> Mr {
    let mut h = Sha384::new();
    h.update(mr);
    h.update(digest);
    h.finalize().into()
}

pub fn replay(log: &EventLog) -> Result<[Mr; 4], ReplayError> {
    let mut rtmr = [[0u8; 48]; 4];
    for event in &log.events {
        let Some(i) = event.rtmr() else { continue };
        let digest = event.sha384.as_ref().ok_or(ReplayError::NoSha384 { event: event.index })?;
        rtmr[i] = extend(&rtmr[i], digest);
    }
    Ok(rtmr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::eventlog::{parse, tests::build};

    #[test]
    fn replays_into_the_right_registers() {
        let d1: [u8; 48] = Sha384::digest(b"one").into();
        let d2: [u8; 48] = Sha384::digest(b"two").into();
        let log = parse(&build(&[(1, 0x4, d1, b""), (3, 0xd, d2, b""), (1, 0x4, d2, b"")])).unwrap();
        let rtmr = replay(&log).unwrap();
        assert_eq!(rtmr[0], extend(&extend(&[0; 48], &d1), &d2));
        assert_eq!(rtmr[2], extend(&[0; 48], &d2));
        assert_eq!(rtmr[1], [0; 48]);
        assert_eq!(rtmr[3], [0; 48]);
    }
}
