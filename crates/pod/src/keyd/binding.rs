//! What the key service's own quote vouches for: REPORTDATA is SHA-512
//! of "pod-keyd-binding-v1\0" ‖ the client's nonce ‖ this boot's X25519
//! key. An owner who recomputes it and finds it equal to the verified
//! quote's REPORTDATA knows a key service running an attested release
//! holds that boot key now, and seals the deposit to it.

use super::{BINDING_DOMAIN, VERSION};
use crate::unlock::binding::NONCE_LEN;
use crate::unlock::wire::{self, hex32, hex_into, WireError, MAX_QUOTE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

pub fn report_data(nonce: &[u8; NONCE_LEN], boot_pubkey: &[u8; 32]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(BINDING_DOMAIN);
    h.update(nonce);
    h.update(boot_pubkey);
    h.finalize().into()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusWire {
    version: u32,
    boot_pubkey: String,
    quote: String,
}

/// `GET /status?nonce=` on the key service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeydStatus {
    pub boot_pubkey: [u8; 32],
    pub quote: Vec<u8>,
}

impl KeydStatus {
    pub fn report_data(&self, nonce: &[u8; NONCE_LEN]) -> [u8; 64] {
        report_data(nonce, &self.boot_pubkey)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let w = StatusWire { version: VERSION, boot_pubkey: hex::encode(self.boot_pubkey), quote: hex::encode(&self.quote) };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        let w: StatusWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        if w.quote.is_empty() || w.quote.len() > MAX_QUOTE * 2 {
            return Err(WireError::Field { field: "quote", why: format!("{} hex digits", w.quote.len()) });
        }
        let mut quote = vec![0u8; w.quote.len() / 2];
        hex_into("quote", &w.quote, &mut quote)?;
        Ok(Self { boot_pubkey: hex32("boot_pubkey", &w.boot_pubkey)?, quote })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sha512_of_domain_nonce_and_boot_key() {
        let mut h = Sha512::new();
        h.update(b"pod-keyd-binding-v1\0");
        h.update([1; 32]);
        h.update([2; 32]);
        assert_eq!(report_data(&[1; 32], &[2; 32]), <[u8; 64]>::from(h.finalize()));
        assert_ne!(report_data(&[1; 32], &[3; 32]), report_data(&[1; 32], &[2; 32]));
        // Never the pod's binding of the same nonce and key.
        let pod = crate::unlock::binding::Binding {
            nonce: [1; 32],
            boot_pubkey: [2; 32],
            state: crate::unlock::binding::State::Blank,
            owner: None,
            tls_root: None,
        };
        assert_ne!(pod.report_data(), report_data(&[1; 32], &[2; 32]));
    }

    #[test]
    fn statuses_round_trip_and_refuse_odd_quotes() {
        let s = KeydStatus { boot_pubkey: [5; 32], quote: vec![7; 10] };
        assert_eq!(KeydStatus::from_json(&s.to_json().unwrap()).unwrap(), s);
        let empty = KeydStatus { quote: vec![], ..s.clone() };
        assert!(KeydStatus::from_json(&empty.to_json().unwrap()).is_err());
        let v2 = String::from_utf8(s.to_json().unwrap()).unwrap().replace("\"version\":1", "\"version\":2");
        assert_eq!(KeydStatus::from_json(v2.as_bytes()), Err(WireError::Version(2)));
    }
}
