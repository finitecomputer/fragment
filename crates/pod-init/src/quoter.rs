//! Quotes behind a trait, so a service that attests itself (pod-init,
//! a key service) is tested without a TDX guest.

use std::path::PathBuf;

pub trait Quoter {
    fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, String>;
}

/// A TDX quote through configfs-tsm, as pod-init asks for them.
pub struct Tsm {
    pub report_root: PathBuf,
}

impl Quoter for Tsm {
    fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, String> {
        crate::tsm::quote(&self.report_root, report_data).map_err(|e| e.to_string())
    }
}
