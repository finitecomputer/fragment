//! pod-init, in the TD: keeps the owner's LUKS volume locked after every
//! boot, attests its state and a boot key made at start (`/status`),
//! serves the boot's event log (`/eventlog`) and TLS root (`/tls-root`),
//! and opens the volume for a disk key sealed to that boot in a request
//! the owner signed (`/init`, `/unlock`). docs/pod.md, "pod-init".
//!
//! Its parts, as a library so they build and test on any OS; the binary
//! (src/bin/pod-init.rs) serves them only on a Linux TDX guest. Quotes come
//! through configfs-tsm (`tsm`, behind `quoter::Quoter`); HTTP is the
//! small server in `http`; the protocol is fragment-pod's `unlock`.

pub mod config;
pub mod error;
pub mod eventlog;
pub mod hook;
pub mod http;
pub mod keyreq;
pub mod luks;
pub mod mountinfo;
pub mod owner;
pub mod quoter;
pub mod service;
pub mod tsm;
pub mod volume;

#[cfg(test)]
mod scratch;
