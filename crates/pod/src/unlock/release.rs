//! A release as an owner names it: a sequence number, a name, and the
//! registers it pins (MRTD, RTMR0 to RTMR3). One shape everywhere: a
//! release in the CLI's policy.json, a release file, and an approval
//! sent to a key service. Registers it does not name are not compared,
//! and a release must name at least one, or it would match any TD.

use super::wire::WireError;
use serde::{Deserialize, Serialize};

pub const REGISTERS: [&str; 5] = ["mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"];
pub const REGISTER_LEN: usize = 48;
pub const MAX_NAME: usize = 64;
/// A release file is five registers in hex and a name: under 1 KiB.
pub const MAX_RELEASE_FILE: usize = 4 * 1024;

pub type Register = [u8; REGISTER_LEN];
/// In `REGISTERS` order; None is "not compared".
pub type Registers = [Option<Register>; 5];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub seq: u64,
    pub name: String,
    pub registers: Registers,
}

/// The JSON form. Hex in either case, as policy.json has always taken
/// it; what is signed is the decoded bytes, never this text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseWire {
    pub seq: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mrtd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtmr0: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtmr1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtmr2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtmr3: Option<String>,
}

fn register(field: &'static str, text: &str) -> Result<Register, WireError> {
    let mut out = [0u8; REGISTER_LEN];
    hex::decode_to_slice(text.trim(), &mut out).map_err(|_| WireError::Field {
        field,
        why: format!("{} hex digits; a register is {} bytes of hex", text.trim().len(), REGISTER_LEN),
    })?;
    Ok(out)
}

pub fn check_name(name: &str) -> Result<(), WireError> {
    if name.is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(WireError::Field { field: "name", why: format!("1 to {MAX_NAME} bytes, no control characters") });
    }
    Ok(())
}

impl Release {
    pub fn from_wire(w: ReleaseWire) -> Result<Self, WireError> {
        check_name(&w.name)?;
        let given = [w.mrtd, w.rtmr0, w.rtmr1, w.rtmr2, w.rtmr3];
        let mut registers: Registers = [None; 5];
        for ((slot, name), value) in registers.iter_mut().zip(REGISTERS).zip(given) {
            *slot = value.as_deref().map(|v| register(name, v)).transpose()?;
        }
        if registers.iter().all(Option::is_none) {
            return Err(WireError::Field { field: "release", why: format!("release {:?} names no register", w.name) });
        }
        Ok(Self { seq: w.seq, name: w.name, registers })
    }

    pub fn to_wire(&self) -> ReleaseWire {
        let [mrtd, rtmr0, rtmr1, rtmr2, rtmr3] = self.registers.map(|r| r.map(hex::encode));
        ReleaseWire { seq: self.seq, name: self.name.clone(), mrtd, rtmr0, rtmr1, rtmr2, rtmr3 }
    }

    /// A release file: one release, as policy.json lists them.
    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_RELEASE_FILE {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_RELEASE_FILE });
        }
        Self::from_wire(super::wire::json(body)?)
    }

    /// The registers it names, by name.
    pub fn named(&self) -> impl Iterator<Item = (&'static str, &Register)> {
        REGISTERS.into_iter().zip(&self.registers).filter_map(|(n, r)| r.as_ref().map(|r| (n, r)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> ReleaseWire {
        ReleaseWire {
            seq: 3,
            name: "pod-b".into(),
            mrtd: None,
            rtmr0: None,
            rtmr1: Some("02".repeat(48)),
            rtmr2: Some("03".repeat(48).to_uppercase()),
            rtmr3: None,
        }
    }

    #[test]
    fn round_trips_and_names_only_the_given_registers() {
        let r = Release::from_wire(wire()).unwrap();
        assert_eq!(r.registers[1], None);
        assert_eq!(r.registers[2], Some([2; 48]));
        assert_eq!(r.registers[3], Some([3; 48]));
        assert_eq!(r.named().map(|(n, _)| n).collect::<Vec<_>>(), ["rtmr1", "rtmr2"]);
        let json = serde_json::to_vec(&r.to_wire()).unwrap();
        assert_eq!(Release::from_json(&json).unwrap(), r);
        assert!(!String::from_utf8(json).unwrap().contains("mrtd"), "absent registers are left out");
    }

    #[test]
    fn refuses_no_register_short_registers_odd_names_and_extra_fields() {
        let none = ReleaseWire { rtmr1: None, rtmr2: None, ..wire() };
        assert!(Release::from_wire(none).unwrap_err().to_string().contains("names no register"));
        let short = ReleaseWire { rtmr1: Some("02".into()), ..wire() };
        assert!(matches!(Release::from_wire(short), Err(WireError::Field { field: "rtmr1", .. })));
        for name in ["", "a\nb", &"x".repeat(MAX_NAME + 1)] {
            let odd = ReleaseWire { name: name.into(), ..wire() };
            assert!(matches!(Release::from_wire(odd), Err(WireError::Field { field: "name", .. })), "{name:?}");
        }
        assert!(Release::from_json(br#"{"seq":1,"name":"a","rtmr4":"00"}"#).is_err());
        let big = vec![b' '; MAX_RELEASE_FILE + 1];
        assert!(matches!(Release::from_json(&big), Err(WireError::TooLarge { .. })));
    }
}
