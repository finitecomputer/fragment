//! Google's host registry: for each Google host's PPID (from the Intel
//! PCK certificate in the quote), a public object in the bucket
//! `confidential-host-registry` naming the host's region and when Google
//! last provisioned its certificates. The object is not signed; it is
//! authenticated only by TLS to storage.googleapis.com. We save it with
//! the PPID and URL it came from, so a saved one can be checked against
//! the right quote.

use super::failure::Failure;
use super::time;
use crate::attest::pzid::Instance;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const BUCKET_URL: &str = "https://storage.googleapis.com/confidential-host-registry";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRegistry {
    pub ppid: String,
    pub url: String,
    pub fetched_at: i64,
    pub document: Document,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub zone: String,
    pub timestamp: String,
}

fn bad(detail: impl Into<String>) -> Failure {
    Failure::HostRegistry { detail: detail.into() }
}

/// The PPID of the quote's PCK certificate, lower-case hex as
/// go-tdx-guest names the registry objects. Read before verification,
/// only to look the document up.
pub fn ppid_of(quote: &[u8]) -> Result<String, Failure> {
    let parsed = dcap_qvl::quote::Quote::parse(quote).map_err(|e| Failure::Quote { detail: format!("{e:#}") })?;
    let chain = dcap_qvl::intel::extract_cert_chain(&parsed)
        .map_err(|e| Failure::Quote { detail: format!("PCK chain: {e:#}") })?;
    let leaf = chain.first().ok_or_else(|| Failure::Quote { detail: "empty PCK chain".into() })?;
    let ext = dcap_qvl::intel::parse_pck_extension(leaf)
        .map_err(|e| Failure::Quote { detail: format!("PCK extension: {e:#}") })?;
    Ok(hex::encode(ext.ppid))
}

impl HostRegistry {
    pub fn fetch(quote: &[u8]) -> Result<Self, Failure> {
        let ppid = ppid_of(quote)?;
        let url = format!("{BUCKET_URL}/{ppid}");
        let resp = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .and_then(|c| c.get(&url).send())
            .map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })?;
        if !resp.status().is_success() {
            return Err(bad(format!("{url}: HTTP {} (not a registered Google host?)", resp.status())));
        }
        let body = resp.bytes().map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })?;
        let document: Document = serde_json::from_slice(&body).map_err(|e| bad(format!("{url}: {e}")))?;
        Ok(Self { ppid, url, fetched_at: time::now(), document })
    }

    pub fn load(path: &Path) -> Result<Self, Failure> {
        let bytes = std::fs::read(path).map_err(|e| bad(format!("{}: {e}", path.display())))?;
        serde_json::from_slice(&bytes).map_err(|e| bad(format!("{}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<(), Failure> {
        let json = serde_json::to_vec_pretty(self).expect("serialisable");
        std::fs::write(path, json).map_err(|e| bad(format!("{}: {e}", path.display())))
    }

    /// The document is for this quote's host, and in the instance's region.
    pub fn check(&self, ppid: &str, instance: Option<&Instance>) -> Result<String, Failure> {
        if !self.ppid.eq_ignore_ascii_case(ppid) {
            return Err(bad(format!("document is for PPID {}, the quote's PPID is {ppid}", self.ppid)));
        }
        if let Some(i) = instance {
            if self.document.zone != i.region() && self.document.zone != i.zone {
                return Err(bad(format!(
                    "host is in {}, the instance is in {}",
                    self.document.zone, i.zone
                )));
            }
        }
        Ok(format!(
            "PPID {ppid} is a Google host in {} (provisioned {}; fetched {})",
            self.document.zone,
            self.document.timestamp,
            time::rfc3339(self.fetched_at)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(zone: &str) -> HostRegistry {
        HostRegistry {
            ppid: "00aa".into(),
            url: format!("{BUCKET_URL}/00aa"),
            fetched_at: 0,
            // The shape Google's codelab shows.
            document: Document { zone: zone.into(), timestamp: "2026-02-17T11:25:12Z".into() },
        }
    }

    #[test]
    fn matches_ppid_and_region() {
        let i = Instance::parse("projects/1/zones/us-central1-b/instances/2").unwrap();
        assert!(registry("us-central1").check("00AA", Some(&i)).is_ok());
        assert!(registry("us-central1").check("00bb", Some(&i)).is_err());
        assert!(registry("europe-west4").check("00aa", Some(&i)).is_err());
    }
}
