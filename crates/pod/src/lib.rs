//! A pod: one owner's self-hosted stack inside a confidential VM, plus a
//! measured image, the owner's unlock, and attestation (docs/pod.md).
//! This library is both sides' code, by module:
//!
//! - [`unlock`]: the lock-and-unlock protocol both sides link: the binding
//!   a quote's REPORTDATA carries, the disk key sealed to one boot's key,
//!   the owner's signed request, a release, and the JSON on the wire.
//! - [`attest`]: the TDX quote layout, nonces, the GCE instance binding in
//!   MR_OWNER, the CCEL event log and its replay, and the RTMR1 and RTMR2
//!   a boot of an image must produce, computed from the image's bytes.
//! - [`verify`] (feature `verify`): a quote verified to Intel's root
//!   (dcap-qvl), the policy checks it leaves to the caller, Google's
//!   firmware endorsement and host registry, SEV-SNP reports, and the
//!   expected registers of a disk image lined up against a boot.
//! - [`checks`] (feature `verify`): the checks before a disk key leaves its
//!   holder, behind an [`checks::attestor::Attestor`], so the owner's CLI
//!   and a key service decide them with the same code.
//! - [`owner`] (feature `owner`): the owner's side: the client of
//!   pod-init, the state directory, the TLS checks and the key request.
//!
//! The in-VM service (crates/pod-init) takes `unlock` and `attest` alone.
//! Nothing in `attest` checks a signature: a report read from an
//! unverified quote is only as good as the verification that follows.

pub mod attest;
pub mod unlock;

#[cfg(feature = "verify")]
pub mod checks;
#[cfg(feature = "verify")]
pub mod verify;

#[cfg(feature = "owner")]
pub mod owner;
