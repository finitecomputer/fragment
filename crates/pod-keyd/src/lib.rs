//! pod-keyd's parts, as a library so they build and test on any OS; the
//! binary (src/bin/pod-keyd.rs) serves them only on a Linux TDX guest. See
//! docs/pod.md ("A key service").

pub mod approve;
pub mod args;
pub mod clock;
pub mod decide;
pub mod deposit;
pub mod error;
pub mod nonces;
pub mod release;
pub mod service;
pub mod store;
