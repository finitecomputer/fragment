//! The owner's side of lock and unlock, as library code the CLI is a
//! thin command line around: one round of attestation (`attest`), the
//! five checks (`checks`), pod-init's client (`client`), the state
//! directory (`dir`), policy.json (`policy`), the TLS-root checks
//! (`tlscheck`), and the key request and verification (`flow`). A key
//! leaves this machine only after every check passes on a fresh
//! attestation, sealed to the boot key that attestation vouched for.

pub mod attest;
pub mod checks;
pub mod client;
pub mod flow;
pub mod dir;
pub mod policy;
pub mod tlscheck;
