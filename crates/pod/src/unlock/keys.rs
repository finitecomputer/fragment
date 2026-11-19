//! The two secrets: the owner's 64-byte disk key, and the pod's boot
//! key pair, made in memory when pod-init starts and never stored. Both
//! zeroize when dropped; neither has a Debug that shows it.

use hpke::kem::X25519HkdfSha256;
use hpke::{Kem, Serializable};
use rand_core::{OsRng, TryRngCore};
use std::fmt;
use zeroize::Zeroizing;

pub const DISK_KEY_LEN: usize = 64;

pub type BootSecret = <X25519HkdfSha256 as Kem>::PrivateKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The OS gave no randomness.
    Random,
    /// A disk key file is not 128 hex digits.
    DiskKeyFormat,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random => write!(f, "no randomness from the OS"),
            Self::DiskKeyFormat => write!(f, "a disk key is {} hex digits", DISK_KEY_LEN * 2),
        }
    }
}

impl std::error::Error for KeyError {}

pub fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, KeyError> {
    let mut out = Zeroizing::new([0u8; N]);
    OsRng.try_fill_bytes(out.as_mut()).map_err(|_| KeyError::Random)?;
    Ok(out)
}

/// The LUKS key. Random, so it needs no KDF (see pod-init's luks.rs).
pub struct DiskKey(Zeroizing<[u8; DISK_KEY_LEN]>);

impl DiskKey {
    pub fn random() -> Result<Self, KeyError> {
        random().map(Self)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        let mut out = Zeroizing::new([0u8; DISK_KEY_LEN]);
        if bytes.len() != DISK_KEY_LEN {
            return Err(KeyError::DiskKeyFormat);
        }
        out.copy_from_slice(bytes);
        Ok(Self(out))
    }

    /// The disk key file's form: hex, one line.
    pub fn from_hex(text: &str) -> Result<Self, KeyError> {
        let mut bytes = Zeroizing::new([0u8; DISK_KEY_LEN]);
        hex::decode_to_slice(text.trim(), bytes.as_mut()).map_err(|_| KeyError::DiskKeyFormat)?;
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(self.0.as_ref()))
    }

    pub fn as_bytes(&self) -> &[u8; DISK_KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for DiskKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DiskKey(..)")
    }
}

/// This boot's X25519 key pair. The secret is hpke's, which wraps a
/// zeroize-on-drop x25519-dalek StaticSecret.
pub struct BootKey {
    secret: BootSecret,
    public: [u8; 32],
}

impl BootKey {
    pub fn generate() -> Result<Self, KeyError> {
        // derive_keypair over 32 fresh bytes rather than gen_keypair, so
        // a failing OS RNG is an error and not a panic inside hpke.
        let ikm = random::<32>()?;
        let (secret, public) = X25519HkdfSha256::derive_keypair(ikm.as_ref());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&public.to_bytes());
        Ok(Self { secret, public: bytes })
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub(crate) fn secret(&self) -> &BootSecret {
        &self.secret
    }
}

impl fmt::Debug for BootKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BootKey({})", hex::encode(self.public))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_keys_are_random_and_round_trip_through_hex() {
        let a = DiskKey::random().unwrap();
        let b = DiskKey::random().unwrap();
        assert_ne!(a.as_bytes(), b.as_bytes());
        let again = DiskKey::from_hex(&a.to_hex()).unwrap();
        assert_eq!(again.as_bytes(), a.as_bytes());
        assert_eq!(DiskKey::from_hex("abcd").unwrap_err(), KeyError::DiskKeyFormat);
        assert_eq!(DiskKey::from_bytes(&[0; 63]).unwrap_err(), KeyError::DiskKeyFormat);
        assert_eq!(format!("{a:?}"), "DiskKey(..)");
    }

    #[test]
    fn boot_keys_differ_per_generation() {
        let a = BootKey::generate().unwrap();
        let b = BootKey::generate().unwrap();
        assert_ne!(a.public(), b.public());
    }
}
