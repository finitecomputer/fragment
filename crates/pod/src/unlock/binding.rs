//! What a pod's quote vouches for. REPORTDATA is SHA-512 of a
//! fixed-length record: the client's nonce, this boot's X25519 public
//! key, the volume's state, the owner and the TLS root. A client that
//! recomputes it from `/status` and finds it equal to the verified
//! quote's REPORTDATA knows the TD itself said each of those, now.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

pub const DOMAIN: &[u8; 15] = b"pod-binding-v1\0";
pub const NONCE_LEN: usize = 32;
/// Domain, nonce, boot key, state, owner, TLS root.
pub const BINDING_LEN: usize = DOMAIN.len() + NONCE_LEN + 32 + 1 + 32 + 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// No volume exists.
    Blank,
    /// A volume exists and is not open.
    Locked,
    /// The volume is open and mounted.
    Open,
}

impl State {
    pub fn byte(self) -> u8 {
        match self {
            Self::Blank => 0,
            Self::Locked => 1,
            Self::Open => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Locked => "locked",
            Self::Open => "open",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub nonce: [u8; NONCE_LEN],
    pub boot_pubkey: [u8; 32],
    pub state: State,
    /// x-only secp256k1 key. None is written as 32 zeros: no valid
    /// x-only key is zero, so the two never collide.
    pub owner: Option<[u8; 32]>,
    /// SHA-256 of the root certificate's DER. None is 32 zeros, which no
    /// certificate hashes to in practice.
    pub tls_root: Option<[u8; 32]>,
}

impl Binding {
    pub fn bytes(&self) -> [u8; BINDING_LEN] {
        let mut out = [0u8; BINDING_LEN];
        let parts: [&[u8]; 6] = [
            DOMAIN,
            &self.nonce,
            &self.boot_pubkey,
            &[self.state.byte()],
            &self.owner.unwrap_or_default(),
            &self.tls_root.unwrap_or_default(),
        ];
        let mut at = 0;
        for p in parts {
            out[at..at + p.len()].copy_from_slice(p);
            at += p.len();
        }
        out
    }

    pub fn report_data(&self) -> [u8; 64] {
        Sha512::digest(self.bytes()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Binding {
        Binding {
            nonce: [1; 32],
            boot_pubkey: [2; 32],
            state: State::Locked,
            owner: Some([3; 32]),
            tls_root: None,
        }
    }

    #[test]
    fn canonical_bytes_and_hash() {
        let b = sample().bytes();
        assert_eq!(b.len(), 144);
        assert_eq!(&b[..15], b"pod-binding-v1\0");
        assert_eq!(&b[15..47], &[1; 32]);
        assert_eq!(&b[47..79], &[2; 32]);
        assert_eq!(b[79], 1);
        assert_eq!(&b[80..112], &[3; 32]);
        assert_eq!(&b[112..], &[0; 32]);
        // Computed independently: python3 hashlib.sha512 over the same 144 bytes.
        let expected = "7b673e2b65daa293e7d73228b8b95b455599b08408fa0ab38412dae63546003337bd5d7a67bf47bea68d013f3b89aa71fa7da351390cd873f44ec253daba1959";
        assert_eq!(hex::encode(sample().report_data()), expected);
    }

    #[test]
    fn every_field_changes_the_hash() {
        let base = sample().report_data();
        let changes: [fn(&mut Binding); 6] = [
            |b| b.nonce[31] ^= 1,
            |b| b.boot_pubkey[0] ^= 1,
            |b| b.state = State::Open,
            |b| b.owner = None,
            |b| b.tls_root = Some([9; 32]),
            |b| b.state = State::Blank,
        ];
        for (i, change) in changes.iter().enumerate() {
            let mut b = sample();
            change(&mut b);
            assert_ne!(b.report_data(), base, "change {i}");
        }
    }

    #[test]
    fn states_have_fixed_bytes_and_names() {
        assert_eq!([State::Blank, State::Locked, State::Open].map(State::byte), [0, 1, 2]);
        assert_eq!(serde_json::to_string(&State::Locked).unwrap(), "\"locked\"");
        assert_eq!(State::Open.as_str(), "open");
    }
}
