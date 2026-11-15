//! The pod's side: `GET /nonce`, then `POST /release` with the pod's
//! status bound to that nonce, and the answer, a delivery: the disk key
//! sealed in HPKE auth mode (info "pod-keyd-release-v1", aad owner ‖ pod
//! boot key ‖ key service boot key) from the key service's boot key to
//! this boot of the pod, with the owner's grant naming that key service.

use super::deposit::check_grant;
use super::{KeydError, RELEASE_INFO, VERSION};
use crate::unlock::binding::NONCE_LEN;
use crate::unlock::keys::{BootKey, DiskKey};
use crate::unlock::seal::{self, CT_LEN};
use crate::unlock::wire::{self, hex32, hex_into, Status, StatusWire, WireError, MAX_QUOTE};
use serde::{Deserialize, Serialize};

/// A status is a quote in hex (at most 128 KiB) and a few fields.
pub const MAX_RELEASE_BODY: usize = 2 * MAX_QUOTE + 4 * 1024;
/// A delivery is about 500 bytes; anything far larger is not one.
pub const MAX_DELIVERY_BODY: usize = 4 * 1024;

fn aad(owner: &[u8; 32], pod_boot: &[u8; 32], keyd_pubkey: &[u8; 32]) -> Vec<u8> {
    [owner.as_slice(), pod_boot, keyd_pubkey].concat()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceWire {
    nonce: String,
}

/// `GET /nonce`: a nonce for one `/release`, good for a short while.
pub fn nonce_to_json(nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, WireError> {
    serde_json::to_vec(&NonceWire { nonce: hex::encode(nonce) }).map_err(|e| WireError::Json(e.to_string()))
}

pub fn nonce_from_json(body: &[u8]) -> Result<[u8; NONCE_LEN], WireError> {
    if body.len() > 1024 {
        return Err(WireError::TooLarge { len: body.len(), max: 1024 });
    }
    let w: NonceWire = wire::json(body)?;
    wire::nonce(&w.nonce)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseWire {
    version: u32,
    nonce: String,
    status: StatusWire,
}

/// The body of `POST /release`: the pod's `/status` document, made for
/// the key service's nonce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRequest {
    pub nonce: [u8; NONCE_LEN],
    pub status: Status,
}

impl ReleaseRequest {
    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let w = ReleaseWire { version: VERSION, nonce: hex::encode(self.nonce), status: self.status.to_wire() };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_RELEASE_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_RELEASE_BODY });
        }
        let w: ReleaseWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        Ok(Self { nonce: wire::nonce(&w.nonce)?, status: Status::from_wire(w.status)? })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryWire {
    version: u32,
    owner: String,
    keyd_pubkey: String,
    grant: String,
    seq: u64,
    enc: String,
    ct: String,
}

/// What a successful `/release` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub owner: [u8; 32],
    pub keyd_pubkey: [u8; 32],
    pub grant: [u8; 64],
    /// The approved release the pod matched. For the record only.
    pub seq: u64,
    pub enc: [u8; 32],
    pub ct: [u8; CT_LEN],
}

impl Delivery {
    pub fn make(keyd: &BootKey, owner: [u8; 32], grant: [u8; 64], seq: u64, pod_boot: &[u8; 32], key: &DiskKey) -> Result<Self, KeydError> {
        let me = keyd.public();
        let sealed = seal::seal_with(pod_boot, Some(keyd), RELEASE_INFO, &aad(&owner, pod_boot, &me), key)?;
        let ct: [u8; CT_LEN] = sealed.ct.as_slice().try_into().map_err(|_| seal::SealError::Length(sealed.ct.len()))?;
        Ok(Self { owner, keyd_pubkey: me, grant, seq, enc: sealed.enc, ct })
    }

    /// The disk key, if this delivery is for `owner` (the one the pod
    /// names), the owner granted the key service that sealed it, and it
    /// was sealed by that key service to this boot of the pod.
    pub fn open(&self, pod_boot: &BootKey, owner: &[u8; 32]) -> Result<DiskKey, KeydError> {
        if &self.owner != owner {
            return Err(KeydError::Owner);
        }
        check_grant(owner, &self.keyd_pubkey, &self.grant)?;
        let aad = aad(owner, &pod_boot.public(), &self.keyd_pubkey);
        Ok(seal::open_with(pod_boot, Some(&self.keyd_pubkey), RELEASE_INFO, &aad, &self.enc, &self.ct)?)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let w = DeliveryWire {
            version: VERSION,
            owner: hex::encode(self.owner),
            keyd_pubkey: hex::encode(self.keyd_pubkey),
            grant: hex::encode(self.grant),
            seq: self.seq,
            enc: hex::encode(self.enc),
            ct: hex::encode(self.ct),
        };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_DELIVERY_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_DELIVERY_BODY });
        }
        let w: DeliveryWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        let mut d = Self {
            owner: hex32("owner", &w.owner)?,
            keyd_pubkey: hex32("keyd_pubkey", &w.keyd_pubkey)?,
            grant: [0; 64],
            seq: w.seq,
            enc: hex32("enc", &w.enc)?,
            ct: [0; CT_LEN],
        };
        hex_into("grant", &w.grant, &mut d.grant)?;
        hex_into("ct", &w.ct, &mut d.ct)?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unlock::binding::State;
    use crate::keyd::deposit::grant_digest;
    use crate::unlock::seal::SealError;
    use crate::unlock::sign::OwnerKey;

    struct Setup {
        keyd: BootKey,
        pod: BootKey,
        owner: OwnerKey,
        grant: [u8; 64],
        key: DiskKey,
    }

    fn setup() -> Setup {
        let keyd = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let grant = owner.sign(&grant_digest(&owner.public(), &keyd.public())).unwrap();
        Setup { keyd, pod: BootKey::generate().unwrap(), owner, grant, key: DiskKey::random().unwrap() }
    }

    fn deliver(s: &Setup) -> Delivery {
        Delivery::make(&s.keyd, s.owner.public(), s.grant, 2, &s.pod.public(), &s.key).unwrap()
    }

    #[test]
    fn opens_on_its_pod_boot_for_its_owner_and_round_trips() {
        let s = setup();
        let d = Delivery::from_json(&deliver(&s).to_json().unwrap()).unwrap();
        assert_eq!(d.open(&s.pod, &s.owner.public()).unwrap().as_bytes(), s.key.as_bytes());
    }

    #[test]
    fn another_pod_boot_or_owner_opens_nothing() {
        let s = setup();
        let d = deliver(&s);
        let later = BootKey::generate().unwrap();
        assert_eq!(d.open(&later, &s.owner.public()).unwrap_err(), KeydError::Seal(SealError::Open));
        assert_eq!(d.open(&s.pod, &[9; 32]).unwrap_err(), KeydError::Owner);
    }

    #[test]
    fn a_key_service_the_owner_did_not_grant_delivers_nothing() {
        // A hostile key service seals a key of its choosing to the pod's
        // boot key (public, from /status) under its own boot key.
        let s = setup();
        let hostile = BootKey::generate().unwrap();
        let mut d = Delivery::make(&hostile, s.owner.public(), s.grant, 2, &s.pod.public(), &DiskKey::random().unwrap()).unwrap();
        assert_eq!(d.open(&s.pod, &s.owner.public()).unwrap_err(), KeydError::Grant, "its own key: no grant");
        d.keyd_pubkey = s.keyd.public();
        assert_eq!(d.open(&s.pod, &s.owner.public()).unwrap_err(), KeydError::Seal(SealError::Open), "the granted key: not its seal");
        // Base mode (no sender) under the granted key's name fails too.
        let base = seal::seal_with(&s.pod.public(), None, RELEASE_INFO, &aad(&s.owner.public(), &s.pod.public(), &s.keyd.public()), &s.key).unwrap();
        let unauth = Delivery { enc: base.enc, ct: base.ct.try_into().unwrap(), ..deliver(&s) };
        assert_eq!(unauth.open(&s.pod, &s.owner.public()).unwrap_err(), KeydError::Seal(SealError::Open));
    }

    #[test]
    fn release_requests_and_nonces_round_trip_within_limits() {
        let status = Status { state: State::Locked, boot_pubkey: [5; 32], owner: Some([6; 32]), tls_root: None, quote: vec![7; 10] };
        let r = ReleaseRequest { nonce: [1; 32], status };
        assert_eq!(ReleaseRequest::from_json(&r.to_json().unwrap()).unwrap(), r);
        let big = vec![b' '; MAX_RELEASE_BODY + 1];
        assert!(matches!(ReleaseRequest::from_json(&big), Err(WireError::TooLarge { .. })));
        assert_eq!(nonce_from_json(&nonce_to_json(&[3; 32]).unwrap()).unwrap(), [3; 32]);
        assert!(nonce_from_json(br#"{"nonce":"00"}"#).is_err());
    }
}
