//! Making a key request (the owner's CLI) and checking one (the pod),
//! in the pod's order: the signature under the named owner first, then
//! the seal under this boot's secret. Both must pass before any volume
//! is touched.

use super::keys::{BootKey, DiskKey};
use super::seal::{self, Kind, SealError, CT_LEN};
use super::sign::{self, OwnerKey, SignError};
use super::wire::KeyRequest;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    Sign(SignError),
    Seal(SealError),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sign(e) => write!(f, "{e}"),
            Self::Seal(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RequestError {}

/// Seal `key` to `boot_pubkey` for `kind`, and sign it as `owner`.
pub fn make(kind: Kind, boot_pubkey: &[u8; 32], owner: &OwnerKey, key: &DiskKey) -> Result<KeyRequest, RequestError> {
    let owner_pub = owner.public();
    let sealed = seal::seal(boot_pubkey, kind, &owner_pub, key).map_err(RequestError::Seal)?;
    let ct: [u8; CT_LEN] = sealed.ct.as_slice().try_into().map_err(|_| RequestError::Seal(SealError::Length(sealed.ct.len())))?;
    let sig = owner.sign(&sign::digest(kind, boot_pubkey, &sealed.enc, &ct)).map_err(RequestError::Sign)?;
    Ok(KeyRequest { owner: owner_pub, enc: sealed.enc, ct, sig })
}

/// The disk key, if `r` is signed by its owner over this boot's key and
/// its seal opens under this boot's secret for `kind` and that owner.
pub fn open(kind: Kind, boot: &BootKey, r: &KeyRequest) -> Result<DiskKey, RequestError> {
    let digest = sign::digest(kind, &boot.public(), &r.enc, &r.ct);
    sign::verify(&r.owner, &digest, &r.sig).map_err(RequestError::Sign)?;
    seal::open(boot, kind, &r.owner, &r.enc, &r.ct).map_err(RequestError::Seal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_request_opens_on_its_boot() {
        let boot = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let key = DiskKey::random().unwrap();
        let r = make(Kind::Init, &boot.public(), &owner, &key).unwrap();
        assert_eq!(r.owner, owner.public());
        assert_eq!(open(Kind::Init, &boot, &r).unwrap().as_bytes(), key.as_bytes());
    }

    #[test]
    fn the_signature_is_checked_before_the_seal() {
        let boot = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let key = DiskKey::random().unwrap();
        let mut r = make(Kind::Unlock, &boot.public(), &owner, &key).unwrap();
        r.ct[0] ^= 1;
        assert_eq!(open(Kind::Unlock, &boot, &r).unwrap_err(), RequestError::Sign(SignError::Signature));
    }

    #[test]
    fn another_owner_cannot_reuse_the_seal_under_its_own_signature() {
        // Mallory re-signs the honest owner's sealed key as herself: the
        // signature now verifies, but the seal's aad names the owner.
        let boot = BootKey::generate().unwrap();
        let honest = OwnerKey::generate().unwrap();
        let key = DiskKey::random().unwrap();
        let r = make(Kind::Unlock, &boot.public(), &honest, &key).unwrap();
        let mallory = OwnerKey::generate().unwrap();
        let d = sign::digest(Kind::Unlock, &boot.public(), &r.enc, &r.ct);
        let forged = KeyRequest { owner: mallory.public(), sig: mallory.sign(&d).unwrap(), ..r };
        assert_eq!(open(Kind::Unlock, &boot, &forged).unwrap_err(), RequestError::Seal(SealError::Open));
    }

    #[test]
    fn a_request_for_a_previous_boot_is_refused() {
        let old = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let r = make(Kind::Unlock, &old.public(), &owner, &DiskKey::random().unwrap()).unwrap();
        let now = BootKey::generate().unwrap();
        assert_eq!(open(Kind::Unlock, &now, &r).unwrap_err(), RequestError::Sign(SignError::Signature));
    }
}
