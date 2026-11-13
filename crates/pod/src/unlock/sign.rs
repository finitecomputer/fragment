//! The owner's signature over a request: BIP-340 Schnorr over
//! SHA-256("pod-request-v1\0" ‖ kind ‖ boot_pubkey ‖ enc ‖ ct). It ties
//! the sealed key to this boot and to whoever holds the owner key; the
//! seal's aad ties it to the owner the request names.

use super::keys::{random, KeyError};
use super::seal::Kind;
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroizing;

pub const DOMAIN: &[u8; 15] = b"pod-request-v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// Not a valid x-only public key.
    Owner,
    /// Not a valid secret key.
    Secret,
    /// The signature does not verify under the owner key.
    Signature,
    Key(KeyError),
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(f, "owner is not a valid x-only secp256k1 public key"),
            Self::Secret => write!(f, "owner key file is not a valid secp256k1 secret (64 hex digits)"),
            Self::Signature => write!(f, "signature does not verify under the owner key"),
            Self::Key(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SignError {}

pub fn digest(kind: Kind, boot_pubkey: &[u8; 32], enc: &[u8; 32], ct: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(kind.as_bytes());
    h.update(boot_pubkey);
    h.update(enc);
    h.update(ct);
    h.finalize().into()
}

/// The owner's secret key. k256's SigningKey zeroizes on drop.
pub struct OwnerKey(SigningKey);

impl OwnerKey {
    pub fn generate() -> Result<Self, SignError> {
        loop {
            let bytes = random::<32>().map_err(SignError::Key)?;
            // A random 32 bytes is a valid scalar but for a 2^-128 chance.
            if let Ok(k) = SigningKey::from_bytes(bytes.as_ref()) {
                return Ok(Self(k));
            }
        }
    }

    pub fn from_hex(text: &str) -> Result<Self, SignError> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        hex::decode_to_slice(text.trim(), bytes.as_mut()).map_err(|_| SignError::Secret)?;
        SigningKey::from_bytes(bytes.as_ref()).map(Self).map_err(|_| SignError::Secret)
    }

    pub fn to_hex(&self) -> Zeroizing<String> {
        let bytes = Zeroizing::new(self.0.to_bytes());
        Zeroizing::new(hex::encode(bytes.as_slice()))
    }

    pub fn public(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes().into()
    }

    pub fn sign(&self, digest: &[u8; 32]) -> Result<[u8; 64], SignError> {
        // Fresh auxiliary randomness, as BIP-340 recommends against
        // side channels on the nonce derivation.
        let aux = random::<32>().map_err(SignError::Key)?;
        let sig = self.0.sign_raw(digest, &aux).map_err(|_| SignError::Signature)?;
        Ok(sig.to_bytes())
    }
}

pub fn verify(owner: &[u8; 32], digest: &[u8; 32], sig: &[u8; 64]) -> Result<(), SignError> {
    let key = VerifyingKey::from_bytes(owner).map_err(|_| SignError::Owner)?;
    // Exactly 64 bytes by type, so k256's unchecked split cannot panic.
    let sig = Signature::try_from(sig.as_slice()).map_err(|_| SignError::Signature)?;
    key.verify_raw(digest, &sig).map_err(|_| SignError::Signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(boot: [u8; 32]) -> [u8; 32] {
        digest(Kind::Unlock, &boot, &[3; 32], &[4; 80])
    }

    #[test]
    fn digest_is_sha256_of_the_tagged_fields() {
        let mut h = Sha256::new();
        h.update(b"pod-request-v1\0unlock");
        h.update([2; 32]);
        h.update([3; 32]);
        h.update([4; 80]);
        assert_eq!(d([2; 32]), <[u8; 32]>::from(h.finalize()));
        assert_ne!(digest(Kind::Init, &[2; 32], &[3; 32], &[4; 80]), d([2; 32]));
    }

    #[test]
    fn verifies_under_the_signer_only() {
        let k = OwnerKey::generate().unwrap();
        let sig = k.sign(&d([2; 32])).unwrap();
        assert_eq!(verify(&k.public(), &d([2; 32]), &sig), Ok(()));
        let other = OwnerKey::generate().unwrap();
        assert_eq!(verify(&other.public(), &d([2; 32]), &sig), Err(SignError::Signature));
    }

    #[test]
    fn fails_when_the_boot_key_differs() {
        let k = OwnerKey::generate().unwrap();
        let sig = k.sign(&d([2; 32])).unwrap();
        assert_eq!(verify(&k.public(), &d([5; 32]), &sig), Err(SignError::Signature));
    }

    #[test]
    fn refuses_bad_keys_and_signatures_without_panicking() {
        let k = OwnerKey::generate().unwrap();
        assert_eq!(verify(&[0; 32], &d([2; 32]), &[0; 64]), Err(SignError::Owner));
        assert_eq!(verify(&k.public(), &d([2; 32]), &[0xff; 64]), Err(SignError::Signature));
        let again = OwnerKey::from_hex(&k.to_hex()).unwrap();
        assert_eq!(again.public(), k.public());
        assert!(OwnerKey::from_hex("00").is_err());
        assert!(OwnerKey::from_hex(&"00".repeat(32)).is_err());
    }
}
