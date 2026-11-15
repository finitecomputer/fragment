//! The key service protocol (pod-keyd): the owner deposits a pod's disk
//! key with a key service the owner has attested, approves releases,
//! and the key service gives the key to a boot of that pod that passes
//! the owner's checks. The pod-v1 domains are untouched; every string
//! here is new.
//!
//! Who can say what:
//! - Only the owner's key changes what the key service releases: a
//!   deposit and an approval are BIP-340 signatures by the owner, bound
//!   to one boot of the key service (its boot key, which its quote
//!   vouches for), so neither carries over to a restarted key service.
//! - The pod accepts a key only with the owner's grant naming that boot
//!   of the key service, sealed in HPKE auth mode by the holder of that
//!   boot key's secret, to this boot of the pod. So a key service the
//!   owner did not attest can deliver nothing the pod will use.

pub mod approve;
pub mod binding;
pub mod delivery;
pub mod deposit;
pub mod encode;
pub mod view;

use crate::unlock::seal::SealError;
use crate::unlock::sign::SignError;
use crate::unlock::wire::WireError;
use std::fmt;

pub const VERSION: u32 = 1;

/// Signature domains, NUL-terminated as pod-request-v1 is.
pub const GRANT_DOMAIN: &[u8] = b"pod-keyd-grant-v1\0";
pub const DEPOSIT_DOMAIN: &[u8] = b"pod-keyd-deposit-v1\0";
pub const APPROVE_DOMAIN: &[u8] = b"pod-keyd-approve-v1\0";
/// The key service's own REPORTDATA.
pub const BINDING_DOMAIN: &[u8] = b"pod-keyd-binding-v1\0";
/// HPKE info strings, as pod-unlock-v1 is for the owner's own requests.
pub const DEPOSIT_INFO: &[u8] = b"pod-keyd-deposit-v1";
pub const RELEASE_INFO: &[u8] = b"pod-keyd-release-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeydError {
    Wire(WireError),
    Sign(SignError),
    Seal(SealError),
    /// The grant does not verify under the owner for that key service.
    Grant,
    /// A delivery for another owner than the one this pod names.
    Owner,
}

impl fmt::Display for KeydError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "{e}"),
            Self::Sign(e) => write!(f, "{e}"),
            Self::Seal(e) => write!(f, "{e}"),
            Self::Grant => write!(f, "the owner's grant does not name this key service's boot key"),
            Self::Owner => write!(f, "the delivery is for another owner"),
        }
    }
}

impl std::error::Error for KeydError {}

impl From<WireError> for KeydError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

impl From<SignError> for KeydError {
    fn from(e: SignError) -> Self {
        Self::Sign(e)
    }
}

impl From<SealError> for KeydError {
    fn from(e: SealError) -> Self {
        Self::Seal(e)
    }
}
