//! The verifier: a TDX quote verified to Intel's root with Intel's
//! collateral (`tdx`, `collateral`), the checks dcap-qvl leaves to the
//! caller (`policy`), Google's provenance for a GCE confidential VM (its
//! signed firmware endorsement, `firmware`; its host registry,
//! `provenance`), AMD SEV-SNP reports (`snp`), and the registers a disk
//! image's boot must produce, lined up against a quote and a log
//! (`disk`, `measure`). Each refusal is a [`failure::Failure`] naming
//! the check.
//!
//! Network: `collateral::fetch` sends only the FMSPC and CA type to
//! Intel's PCS; `firmware::fetch` the MRTD, and `provenance` the PPID,
//! to storage.googleapis.com.

pub mod collateral;
pub mod disk;
pub mod failure;
pub mod firmware;
pub mod measure;
pub mod policy;
pub mod proto;
pub mod provenance;
pub mod snp;
pub mod tdx;
pub mod time;
