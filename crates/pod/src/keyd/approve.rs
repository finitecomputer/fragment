//! An approval: one release the owner lets the key service unlock, from
//! `not_before` and until `expires` (unix seconds, either optional).
//! `POST /approve` carries one, signed by the owner over
//! SHA-256("pod-keyd-approve-v1\0" ‖ key service boot key ‖ owner ‖
//! record ‖ approval), so it applies to one deposit on one boot of the
//! key service: replayed after a new deposit or a restart, it names a
//! record that no longer exists.

use super::encode::Canon;
use super::{APPROVE_DOMAIN, VERSION};
use crate::unlock::release::{Release, ReleaseWire};
use crate::unlock::sign::{self, OwnerKey, SignError};
use crate::unlock::wire::{self, hex32, hex_into, WireError};
use serde::{Deserialize, Serialize};

/// An approval is a release (about 500 bytes) and two numbers.
pub const MAX_APPROVE_BODY: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalWire {
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
    #[serde(default)]
    pub not_before: Option<u64>,
    #[serde(default)]
    pub expires: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub release: Release,
    pub not_before: Option<u64>,
    pub expires: Option<u64>,
}

impl Approval {
    pub fn from_wire(w: ApprovalWire) -> Result<Self, WireError> {
        let ApprovalWire { seq, name, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, not_before, expires } = w;
        let release = Release::from_wire(ReleaseWire { seq, name, mrtd, rtmr0, rtmr1, rtmr2, rtmr3 })?;
        if let (Some(from), Some(until)) = (not_before, expires) {
            if until <= from {
                return Err(WireError::Field { field: "expires", why: format!("{until} is not after not_before {from}") });
            }
        }
        Ok(Self { release, not_before, expires })
    }

    pub fn to_wire(&self) -> ApprovalWire {
        let ReleaseWire { seq, name, mrtd, rtmr0, rtmr1, rtmr2, rtmr3 } = self.release.to_wire();
        ApprovalWire { seq, name, mrtd, rtmr0, rtmr1, rtmr2, rtmr3, not_before: self.not_before, expires: self.expires }
    }

    pub fn encode(&self, c: &mut Canon) {
        c.u64(self.release.seq).str(&self.release.name);
        for r in &self.release.registers {
            c.opt_fixed(r.as_ref().map(|r| r.as_slice()));
        }
        c.opt_u64(self.not_before).opt_u64(self.expires);
    }
}

pub fn digest(keyd_pubkey: &[u8; 32], owner: &[u8; 32], record: &[u8; 32], approval: &Approval) -> [u8; 32] {
    let mut c = Canon::new(APPROVE_DOMAIN);
    c.fixed(keyd_pubkey).fixed(owner).fixed(record);
    approval.encode(&mut c);
    c.digest()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveWire {
    version: u32,
    owner: String,
    record: String,
    approval: ApprovalWire,
    sig: String,
}

/// The body of `POST /approve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approve {
    pub owner: [u8; 32],
    /// The deposit this approval adds to (its id, from `/pods/{owner}`).
    pub record: [u8; 32],
    pub approval: Approval,
    pub sig: [u8; 64],
}

impl Approve {
    pub fn make(owner: &OwnerKey, keyd_pubkey: &[u8; 32], record: [u8; 32], approval: Approval) -> Result<Self, SignError> {
        let me = owner.public();
        let sig = owner.sign(&digest(keyd_pubkey, &me, &record, &approval))?;
        Ok(Self { owner: me, record, approval, sig })
    }

    /// Signed by the owner it names, for this boot of the key service.
    pub fn verify(&self, keyd_pubkey: &[u8; 32]) -> Result<(), SignError> {
        sign::verify(&self.owner, &digest(keyd_pubkey, &self.owner, &self.record, &self.approval), &self.sig)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let w = ApproveWire {
            version: VERSION,
            owner: hex::encode(self.owner),
            record: hex::encode(self.record),
            approval: self.approval.to_wire(),
            sig: hex::encode(self.sig),
        };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_APPROVE_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_APPROVE_BODY });
        }
        let w: ApproveWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        let mut sig = [0u8; 64];
        hex_into("sig", &w.sig, &mut sig)?;
        Ok(Self {
            owner: hex32("owner", &w.owner)?,
            record: hex32("record", &w.record)?,
            approval: Approval::from_wire(w.approval)?,
            sig,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn approval(seq: u64, rtmr1: u8) -> Approval {
        let mut registers = [None; 5];
        registers[2] = Some([rtmr1; 48]);
        Approval { release: Release { seq, name: format!("r{seq}"), registers }, not_before: Some(10), expires: Some(20) }
    }

    #[test]
    fn verifies_for_its_owner_key_service_and_record_only() {
        let owner = OwnerKey::generate().unwrap();
        let a = Approve::make(&owner, &[1; 32], [2; 32], approval(3, 9)).unwrap();
        assert_eq!(a.verify(&[1; 32]), Ok(()));
        assert_eq!(a.verify(&[4; 32]), Err(SignError::Signature), "another boot of the key service");
        let moved = Approve { record: [5; 32], ..a.clone() };
        assert_eq!(moved.verify(&[1; 32]), Err(SignError::Signature), "another deposit");
        let mut later = a.clone();
        later.approval.expires = Some(99);
        assert_eq!(later.verify(&[1; 32]), Err(SignError::Signature), "a changed window");
        let other = OwnerKey::generate().unwrap();
        let claimed = Approve { owner: other.public(), ..a.clone() };
        assert_eq!(claimed.verify(&[1; 32]), Err(SignError::Signature), "another owner's name on it");
        assert_eq!(Approve::from_json(&a.to_json().unwrap()).unwrap(), a);
    }

    #[test]
    fn refuses_an_empty_window_extra_fields_and_large_bodies() {
        let mut w = approval(1, 1).to_wire();
        w.expires = Some(10);
        assert!(matches!(Approval::from_wire(w), Err(WireError::Field { field: "expires", .. })));
        let owner = OwnerKey::generate().unwrap();
        let a = Approve::make(&owner, &[1; 32], [2; 32], approval(1, 1)).unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&a.to_json().unwrap()).unwrap();
        v["approval"]["delegate"] = "x".into();
        assert!(matches!(Approve::from_json(&serde_json::to_vec(&v).unwrap()), Err(WireError::Json(_))));
        let big = vec![b' '; MAX_APPROVE_BODY + 1];
        assert!(matches!(Approve::from_json(&big), Err(WireError::TooLarge { .. })));
    }
}
