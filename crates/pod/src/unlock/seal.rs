//! The disk key sealed to one boot: HPKE base mode (RFC 9180),
//! DHKEM(X25519, HKDF-SHA256) / HKDF-SHA256 / ChaCha20-Poly1305. The aad
//! names the request kind, the owner and the boot key, so a sealed key
//! cannot be replayed on a later boot, moved from init to unlock, or
//! claimed for another owner.

use super::keys::{BootKey, DiskKey, DISK_KEY_LEN};
use hpke::aead::{AeadTag, ChaCha20Poly1305};
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem, OpModeR, OpModeS, Serializable};
use std::fmt;
use zeroize::Zeroizing;

pub const INFO: &[u8] = b"pod-unlock-v1";
const TAG_LEN: usize = 16;
/// The sealed disk key and its Poly1305 tag.
pub const CT_LEN: usize = DISK_KEY_LEN + TAG_LEN;

type PublicKey = <X25519HkdfSha256 as Kem>::PublicKey;
type EncappedKey = <X25519HkdfSha256 as Kem>::EncappedKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Init,
    Unlock,
}

impl Kind {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Init => b"init",
            Self::Unlock => b"unlock",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The boot key or `enc` is not an X25519 point hpke accepts.
    Key,
    /// `ct` is not 80 bytes.
    Length(usize),
    /// Authentication failed: another boot, kind or owner, or tampering.
    Open,
}

impl fmt::Display for SealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key => write!(f, "not a usable X25519 key"),
            Self::Length(n) => write!(f, "sealed key is {n} bytes; expected {CT_LEN}"),
            Self::Open => write!(f, "sealed key does not open under this boot's key, kind and owner"),
        }
    }
}

impl std::error::Error for SealError {}

pub fn aad(kind: Kind, owner: &[u8; 32], boot_pubkey: &[u8; 32]) -> Vec<u8> {
    [kind.as_bytes(), owner, boot_pubkey].concat()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub enc: [u8; 32],
    pub ct: Vec<u8>,
}

/// Seal `key` to the boot key a verified quote vouched for. In place
/// and detached, so the plaintext only ever sits in a zeroizing buffer.
pub fn seal(boot_pubkey: &[u8; 32], kind: Kind, owner: &[u8; 32], key: &DiskKey) -> Result<Sealed, SealError> {
    let pk = PublicKey::from_bytes(boot_pubkey).map_err(|_| SealError::Key)?;
    let mut buf = Zeroizing::new(*key.as_bytes());
    // hpke refuses a low-order boot key (an all-zero shared secret).
    let (enc, tag) = hpke::single_shot_seal_in_place_detached::<ChaCha20Poly1305, HkdfSha256, X25519HkdfSha256, _>(
        &OpModeS::Base,
        &pk,
        INFO,
        buf.as_mut(),
        &aad(kind, owner, boot_pubkey),
        // UnwrapErr panics only if the OS has no randomness to give.
        &mut rand_core::UnwrapErr(rand_core::OsRng),
    )
    .map_err(|_| SealError::Key)?;
    let mut enc_bytes = [0u8; 32];
    enc_bytes.copy_from_slice(&enc.to_bytes());
    let ct = [buf.as_slice(), &tag.to_bytes()].concat();
    Ok(Sealed { enc: enc_bytes, ct })
}

pub fn open(boot: &BootKey, kind: Kind, owner: &[u8; 32], enc: &[u8; 32], ct: &[u8]) -> Result<DiskKey, SealError> {
    if ct.len() != CT_LEN {
        return Err(SealError::Length(ct.len()));
    }
    let enc = EncappedKey::from_bytes(enc).map_err(|_| SealError::Key)?;
    let tag = AeadTag::<ChaCha20Poly1305>::from_bytes(&ct[DISK_KEY_LEN..]).map_err(|_| SealError::Open)?;
    let mut buf = Zeroizing::new([0u8; DISK_KEY_LEN]);
    buf.copy_from_slice(&ct[..DISK_KEY_LEN]);
    hpke::single_shot_open_in_place_detached::<ChaCha20Poly1305, HkdfSha256, X25519HkdfSha256>(
        &OpModeR::Base,
        boot.secret(),
        &enc,
        INFO,
        buf.as_mut(),
        &aad(kind, owner, &boot.public()),
        &tag,
    )
    .map_err(|_| SealError::Open)?;
    DiskKey::from_bytes(buf.as_ref()).map_err(|_| SealError::Open)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: [u8; 32] = [7; 32];

    fn sealed(boot: &BootKey, kind: Kind) -> (DiskKey, Sealed) {
        let key = DiskKey::random().unwrap();
        let s = seal(&boot.public(), kind, &OWNER, &key).unwrap();
        (key, s)
    }

    #[test]
    fn round_trips() {
        let boot = BootKey::generate().unwrap();
        let (key, s) = sealed(&boot, Kind::Unlock);
        assert_eq!(s.ct.len(), CT_LEN);
        let opened = open(&boot, Kind::Unlock, &OWNER, &s.enc, &s.ct).unwrap();
        assert_eq!(opened.as_bytes(), key.as_bytes());
    }

    #[test]
    fn a_later_boot_cannot_open_it() {
        let first = BootKey::generate().unwrap();
        let (_, s) = sealed(&first, Kind::Unlock);
        let later = BootKey::generate().unwrap();
        assert_eq!(open(&later, Kind::Unlock, &OWNER, &s.enc, &s.ct).unwrap_err(), SealError::Open);
    }

    #[test]
    fn kind_and_owner_are_bound() {
        let boot = BootKey::generate().unwrap();
        let (_, init) = sealed(&boot, Kind::Init);
        assert_eq!(open(&boot, Kind::Unlock, &OWNER, &init.enc, &init.ct).unwrap_err(), SealError::Open);
        let (_, unlock) = sealed(&boot, Kind::Unlock);
        assert_eq!(open(&boot, Kind::Init, &OWNER, &unlock.enc, &unlock.ct).unwrap_err(), SealError::Open);
        assert_eq!(open(&boot, Kind::Unlock, &[8; 32], &unlock.enc, &unlock.ct).unwrap_err(), SealError::Open);
    }

    #[test]
    fn tampering_and_bad_lengths_are_errors() {
        let boot = BootKey::generate().unwrap();
        let (_, mut s) = sealed(&boot, Kind::Init);
        assert_eq!(open(&boot, Kind::Init, &OWNER, &s.enc, &s.ct[..79]).unwrap_err(), SealError::Length(79));
        s.ct[0] ^= 1;
        assert_eq!(open(&boot, Kind::Init, &OWNER, &s.enc, &s.ct).unwrap_err(), SealError::Open);
    }

    #[test]
    fn aad_is_kind_owner_boot_key() {
        let a = aad(Kind::Init, &[1; 32], &[2; 32]);
        assert_eq!(&a[..4], b"init");
        assert_eq!(&a[4..36], &[1; 32]);
        assert_eq!(&a[36..], &[2; 32]);
        assert_eq!(aad(Kind::Unlock, &[1; 32], &[2; 32]).len(), 70);
    }
}
