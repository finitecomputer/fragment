//! The CCEL boot event log: a TCG crypto-agile log (TCG PC Client
//! Platform Firmware Profile, sections 10.2.1 and 10.2.2) whose index
//! field is the CC measurement register, not a TPM PCR: 0 is MRTD and
//! 1 to 4 are RTMR0 to RTMR3 (UEFI 2.10, EFI_CC_MEASUREMENT_PROTOCOL).
//! On a TDX guest it is `/sys/firmware/acpi/tables/data/CCEL`; the
//! table at `/sys/firmware/acpi/tables/CCEL` only points at it.

use std::fmt;

pub const ALG_SHA384: u16 = 0x000c;
pub const EV_NO_ACTION: u32 = 0x3;
const MAX_EVENTS: usize = 10_000;
const MAX_ALGORITHMS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub index: usize,
    /// The log's register index: 1 to 4 is RTMR0 to RTMR3.
    pub mr_index: u32,
    pub event_type: u32,
    /// The SHA-384 digest, if the event carries one.
    pub sha384: Option<[u8; 48]>,
    pub data: Vec<u8>,
}

impl Event {
    /// Which RTMR the event extends, if any.
    pub fn rtmr(&self) -> Option<usize> {
        match self.mr_index {
            1..=4 if self.event_type != EV_NO_ACTION => Some(self.mr_index as usize - 1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLog {
    /// Digest sizes the Spec ID event declares, by algorithm id.
    pub algorithms: Vec<(u16, u16)>,
    pub events: Vec<Event>,
    /// Bytes of the input the log took up, before padding.
    pub used: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    Truncated { at: usize },
    NotCryptoAgile,
    UnknownAlgorithm { at: usize, alg: u16 },
    TooMany,
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { at } => write!(f, "event log truncated at byte {at}"),
            Self::NotCryptoAgile => write!(f, "event log has no 'Spec ID Event03' header"),
            Self::UnknownAlgorithm { at, alg } => {
                write!(f, "event at byte {at} uses algorithm {alg:#06x} the header does not declare")
            }
            Self::TooMany => write!(f, "event log has more than {MAX_EVENTS} events"),
        }
    }
}

impl std::error::Error for LogError {}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LogError> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len());
        let end = end.ok_or(LogError::Truncated { at: self.pos })?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16, LogError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    fn u32(&mut self) -> Result<u32, LogError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn rest_is_padding(&self) -> bool {
        let rest = &self.buf[self.pos..];
        rest.iter().all(|b| *b == 0) || rest.iter().all(|b| *b == 0xff)
    }
}

pub fn parse(buf: &[u8]) -> Result<EventLog, LogError> {
    let mut r = Reader { buf, pos: 0 };
    let algorithms = parse_header(&mut r)?;
    let mut events = Vec::new();
    while r.pos < buf.len() && !r.rest_is_padding() {
        let start = r.pos;
        let mr_index = r.u32()?;
        let event_type = r.u32()?;
        // Firmware leaves the unused log area as 0xff; a record of all
        // ones is the end.
        if event_type == u32::MAX && mr_index == u32::MAX {
            r.pos = start;
            break;
        }
        if events.len() >= MAX_EVENTS {
            return Err(LogError::TooMany);
        }
        let sha384 = parse_digests(&mut r, &algorithms, start)?;
        let size = r.u32()? as usize;
        let data = r.take(size)?.to_vec();
        events.push(Event { index: events.len(), mr_index, event_type, sha384, data });
    }
    Ok(EventLog { algorithms, events, used: r.pos })
}

fn parse_header(r: &mut Reader<'_>) -> Result<Vec<(u16, u16)>, LogError> {
    let _index = r.u32()?;
    let _type = r.u32()?;
    r.take(20)?;
    let size = r.u32()? as usize;
    let spec = r.take(size)?;
    let mut s = Reader { buf: spec, pos: 0 };
    if s.take(16).map_err(|_| LogError::NotCryptoAgile)? != b"Spec ID Event03\0" {
        return Err(LogError::NotCryptoAgile);
    }
    s.take(8)?; // platform class, version, errata, uintn size
    let count = s.u32()? as usize;
    if count == 0 || count > MAX_ALGORITHMS {
        return Err(LogError::NotCryptoAgile);
    }
    (0..count).map(|_| Ok((s.u16()?, s.u16()?))).collect()
}

fn parse_digests(
    r: &mut Reader<'_>,
    algorithms: &[(u16, u16)],
    at: usize,
) -> Result<Option<[u8; 48]>, LogError> {
    let count = r.u32()? as usize;
    if count > MAX_ALGORITHMS {
        return Err(LogError::Truncated { at });
    }
    let mut sha384 = None;
    for _ in 0..count {
        let alg = r.u16()?;
        let size = algorithms
            .iter()
            .find(|(a, _)| *a == alg)
            .map(|(_, s)| *s as usize)
            .ok_or(LogError::UnknownAlgorithm { at, alg })?;
        let digest = r.take(size)?;
        if alg == ALG_SHA384 && size == 48 {
            sha384 = Some(digest.try_into().expect("48"));
        }
    }
    Ok(sha384)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal log: the Spec ID header, then `events` as
    /// (mr index, type, digest, data), then 0xff padding.
    pub fn build(events: &[(u32, u32, [u8; 48], &[u8])]) -> Vec<u8> {
        let mut spec = b"Spec ID Event03\0".to_vec();
        spec.extend_from_slice(&[0, 0, 0, 0, 0, 2, 0, 2]);
        spec.extend_from_slice(&1u32.to_le_bytes());
        spec.extend_from_slice(&ALG_SHA384.to_le_bytes());
        spec.extend_from_slice(&48u16.to_le_bytes());
        spec.push(0);
        let mut out = Vec::new();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&EV_NO_ACTION.to_le_bytes());
        out.extend_from_slice(&[0; 20]);
        out.extend_from_slice(&(spec.len() as u32).to_le_bytes());
        out.extend_from_slice(&spec);
        for (mr, ty, digest, data) in events {
            out.extend_from_slice(&mr.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            out.extend_from_slice(&ALG_SHA384.to_le_bytes());
            out.extend_from_slice(digest);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out.extend_from_slice(&[0xff; 64]);
        out
    }

    #[test]
    fn parses_events_and_stops_at_padding() {
        let log = build(&[(1, 0x8000_0001, [7; 48], b"abc"), (3, 0xd, [9; 48], b"x\0")]);
        let parsed = parse(&log).unwrap();
        assert_eq!(parsed.algorithms, vec![(ALG_SHA384, 48)]);
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.events[0].rtmr(), Some(0));
        assert_eq!(parsed.events[1].rtmr(), Some(2));
        assert_eq!(parsed.events[1].data, b"x\0");
        assert_eq!(parsed.used, log.len() - 64);
    }

    #[test]
    fn refuses_a_truncated_or_headerless_log() {
        let log = build(&[(1, 0x4, [0; 48], b"\0\0\0\0")]);
        let cut = &log[..log.len() - 64 - 2];
        assert!(matches!(parse(cut), Err(LogError::Truncated { .. })));
        assert_eq!(parse(&[0u8; 64]), Err(LogError::NotCryptoAgile));
    }

    #[test]
    fn no_action_events_extend_nothing() {
        let log = build(&[(1, EV_NO_ACTION, [0; 48], b"")]);
        assert_eq!(parse(&log).unwrap().events[0].rtmr(), None);
    }
}
