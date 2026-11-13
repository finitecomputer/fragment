//! What the pod and its verifier share: the TD report's layout inside a
//! quote, the nonce that becomes REPORTDATA, the GCE instance binding in
//! MR_OWNER, the CCEL boot event log with its replay into RTMRs, and the
//! registers an image's boot must produce (`expect`, from its partition
//! table and its UKI: `gpt`, `pe`).
//!
//! Nothing here checks a signature. A [`report::TdReport`] read from an
//! unverified quote is only as good as the verification that follows.
//! Pure Rust and small on purpose: pod-init links it into the measured
//! image, built for x86_64-unknown-linux-musl.

pub mod describe;
pub mod eventlog;
pub mod expect;
pub mod gpt;
pub mod nonce;
pub mod pe;
pub mod pzid;
pub mod replay;
pub mod report;
