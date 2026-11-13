//! Intel's collateral for a quote, from the Provisioning Certification
//! Service (PCS v4, no API key for these routes) or from a directory a
//! previous fetch saved. The directory keeps Intel's responses as
//! served, so anyone can check them again later:
//!
//! - `tcb_info.json`, `tcb_info_issuer_chain.pem` (TDX TCB info for the FMSPC)
//! - `qe_identity.json`, `qe_identity_issuer_chain.pem` (TD QE identity)
//! - `pck_crl.der`, `pck_crl_issuer_chain.pem` (PCK CRL for the CA type)
//! - `root_ca_crl.der` (Intel SGX root CA CRL)
//! - `source.json` (what was fetched, from where, when)
//!
//! Only the FMSPC and the CA type leave this machine, in the URLs.

use super::failure::Failure;
use super::time;
use dcap_qvl::QuoteCollateralV3;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::path::Path;

pub const PCS: &str = "https://api.trustedservices.intel.com";
/// The CRL distribution point in Intel's SGX root CA certificate.
pub const ROOT_CA_CRL_URL: &str = "https://certificates.trustedservices.intel.com/IntelSGXRootCA.der";
const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub fetched_at: i64,
    pub fmspc: String,
    pub ca: String,
    pub urls: Vec<String>,
}

/// Collateral as Intel served it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collateral {
    pub tcb_info: String,
    pub tcb_info_issuer_chain: String,
    pub qe_identity: String,
    pub qe_identity_issuer_chain: String,
    pub pck_crl: Vec<u8>,
    pub pck_crl_issuer_chain: String,
    pub root_ca_crl: Vec<u8>,
    pub source: Source,
}

fn bad(detail: impl Into<String>) -> Failure {
    Failure::Collateral { detail: detail.into() }
}

/// The FMSPC (upper-case hex) and PCK CA ("processor" or "platform")
/// from the PCK certificate in the quote.
pub fn platform_of(quote: &[u8]) -> Result<(String, String), Failure> {
    let parsed = dcap_qvl::quote::Quote::parse(quote)
        .map_err(|e| Failure::Quote { detail: format!("{e:#}") })?;
    let fmspc = dcap_qvl::intel::quote_fmspc(&parsed)
        .map_err(|e| Failure::Quote { detail: format!("FMSPC from PCK certificate: {e:#}") })?;
    let ca = dcap_qvl::intel::quote_ca(&parsed)
        .map_err(|e| Failure::Quote { detail: format!("CA from PCK certificate: {e:#}") })?;
    Ok((hex::encode_upper(fmspc), ca.as_id_str().to_owned()))
}

struct Got {
    body: Vec<u8>,
    chain: Option<String>,
}

fn get(client: &reqwest::blocking::Client, url: &str, chain_header: Option<&str>) -> Result<Got, Failure> {
    let fetch = |detail: String| Failure::Fetch { detail: format!("{url}: {detail}") };
    let resp = client.get(url).send().map_err(|e| fetch(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(fetch(format!("HTTP {}", resp.status())));
    }
    let chain = match chain_header {
        Some(name) => {
            let raw = resp
                .headers()
                .get(name)
                .ok_or_else(|| fetch(format!("no {name} header")))?
                .to_str()
                .map_err(|_| fetch(format!("{name} is not text")))?;
            Some(percent_decode(raw).ok_or_else(|| fetch(format!("{name} is not URL-encoded text")))?)
        }
        None => None,
    };
    let body = resp.bytes().map_err(|e| fetch(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(fetch("response over 1 MiB".into()));
    }
    Ok(Got { body: body.to_vec(), chain })
}

/// Intel URL-encodes the PEM chains it puts in headers.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn fetch(quote: &[u8]) -> Result<Collateral, Failure> {
    let (fmspc, ca) = platform_of(quote)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("fragment-pod")
        .build()
        .map_err(|e| Failure::Fetch { detail: e.to_string() })?;
    let urls = vec![
        format!("{PCS}/tdx/certification/v4/tcb?fmspc={fmspc}"),
        format!("{PCS}/tdx/certification/v4/qe/identity"),
        format!("{PCS}/sgx/certification/v4/pckcrl?ca={ca}&encoding=der"),
        ROOT_CA_CRL_URL.to_owned(),
    ];
    let tcb = get(&client, &urls[0], Some("TCB-Info-Issuer-Chain"))?;
    let qe = get(&client, &urls[1], Some("SGX-Enclave-Identity-Issuer-Chain"))?;
    let crl = get(&client, &urls[2], Some("SGX-PCK-CRL-Issuer-Chain"))?;
    let root = get(&client, &urls[3], None)?;
    let text = |b: Vec<u8>, what: &str| String::from_utf8(b).map_err(|_| bad(format!("{what} is not UTF-8")));
    let collateral = Collateral {
        tcb_info: text(tcb.body, "TCB info")?,
        tcb_info_issuer_chain: tcb.chain.unwrap_or_default(),
        qe_identity: text(qe.body, "QE identity")?,
        qe_identity_issuer_chain: qe.chain.unwrap_or_default(),
        pck_crl: crl.body,
        pck_crl_issuer_chain: crl.chain.unwrap_or_default(),
        root_ca_crl: root.body,
        source: Source { fetched_at: time::now(), fmspc, ca, urls },
    };
    collateral.to_dcap()?;
    Ok(collateral)
}

const FILES: [&str; 8] = [
    "tcb_info.json",
    "tcb_info_issuer_chain.pem",
    "qe_identity.json",
    "qe_identity_issuer_chain.pem",
    "pck_crl.der",
    "pck_crl_issuer_chain.pem",
    "root_ca_crl.der",
    "source.json",
];

impl Collateral {
    pub fn save(&self, dir: &Path) -> Result<(), Failure> {
        let io = |e: std::io::Error| bad(format!("saving to {}: {e}", dir.display()));
        std::fs::create_dir_all(dir).map_err(io)?;
        let source = serde_json::to_vec_pretty(&self.source).expect("serialisable");
        let contents: [&[u8]; 8] = [
            self.tcb_info.as_bytes(),
            self.tcb_info_issuer_chain.as_bytes(),
            self.qe_identity.as_bytes(),
            self.qe_identity_issuer_chain.as_bytes(),
            &self.pck_crl,
            self.pck_crl_issuer_chain.as_bytes(),
            &self.root_ca_crl,
            &source,
        ];
        for (name, bytes) in FILES.iter().zip(contents) {
            std::fs::write(dir.join(name), bytes).map_err(io)?;
        }
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<Self, Failure> {
        let read = |name: &str| {
            std::fs::read(dir.join(name)).map_err(|e| bad(format!("{}: {e}", dir.join(name).display())))
        };
        let text = |name: &str| {
            String::from_utf8(read(name)?).map_err(|_| bad(format!("{name} is not UTF-8")))
        };
        let source: Source = serde_json::from_slice(&read("source.json")?)
            .map_err(|e| bad(format!("source.json: {e}")))?;
        Ok(Self {
            tcb_info: text("tcb_info.json")?,
            tcb_info_issuer_chain: text("tcb_info_issuer_chain.pem")?,
            qe_identity: text("qe_identity.json")?,
            qe_identity_issuer_chain: text("qe_identity_issuer_chain.pem")?,
            pck_crl: read("pck_crl.der")?,
            pck_crl_issuer_chain: text("pck_crl_issuer_chain.pem")?,
            root_ca_crl: read("root_ca_crl.der")?,
            source,
        })
    }

    /// The signed objects exactly as served, for dcap-qvl. The PCK chain
    /// comes from the quote itself (certification data type 5).
    pub fn to_dcap(&self) -> Result<QuoteCollateralV3, Failure> {
        let tcb = signed(&self.tcb_info, "tcbInfo")?;
        let qe = signed(&self.qe_identity, "enclaveIdentity")?;
        Ok(QuoteCollateralV3 {
            pck_crl_issuer_chain: self.pck_crl_issuer_chain.clone(),
            root_ca_crl: self.root_ca_crl.clone(),
            pck_crl: self.pck_crl.clone(),
            tcb_info_issuer_chain: self.tcb_info_issuer_chain.clone(),
            tcb_info: tcb.body,
            tcb_info_signature: tcb.signature,
            qe_identity_issuer_chain: self.qe_identity_issuer_chain.clone(),
            qe_identity: qe.body,
            qe_identity_signature: qe.signature,
            pck_certificate_chain: None,
        })
    }

    /// TCB info and QE identity must be inside [issueDate, nextUpdate]
    /// at `at`. dcap-qvl checks this too, and the CRLs and certificates;
    /// this is here so an expired set fails with its own name.
    pub fn check_fresh(&self, at: i64) -> Result<(), Failure> {
        for (name, json, key) in [
            ("TCB info", &self.tcb_info, "tcbInfo"),
            ("QE identity", &self.qe_identity, "enclaveIdentity"),
        ] {
            let window = window(json, key)?;
            if at < window.0 || at > window.1 {
                return Err(Failure::CollateralExpired {
                    item: name.to_owned(),
                    valid_from: time::rfc3339(window.0),
                    valid_until: time::rfc3339(window.1),
                    at,
                });
            }
        }
        Ok(())
    }

    /// The earliest nextUpdate of TCB info and QE identity.
    pub fn next_update(&self) -> Result<i64, Failure> {
        Ok(window(&self.tcb_info, "tcbInfo")?.1.min(window(&self.qe_identity, "enclaveIdentity")?.1))
    }
}

struct Signed {
    body: String,
    signature: Vec<u8>,
}

/// `{"<key>": {...}, "signature": "<hex>"}`: the object's bytes exactly
/// as served (the signature is over them) and the signature.
fn signed(json: &str, key: &str) -> Result<Signed, Failure> {
    let map: std::collections::BTreeMap<String, Box<RawValue>> =
        serde_json::from_str(json).map_err(|e| bad(format!("{key}: {e}")))?;
    let body = map.get(key).ok_or_else(|| bad(format!("no {key}")))?.get().to_owned();
    let sig: String = map
        .get("signature")
        .and_then(|v| serde_json::from_str(v.get()).ok())
        .ok_or_else(|| bad(format!("{key}: no signature")))?;
    let signature = hex::decode(sig).map_err(|_| bad(format!("{key}: signature is not hex")))?;
    Ok(Signed { body, signature })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dates {
    issue_date: String,
    next_update: String,
}

fn window(json: &str, key: &str) -> Result<(i64, i64), Failure> {
    let body = signed(json, key)?.body;
    let dates: Dates = serde_json::from_str(&body).map_err(|e| bad(format!("{key} dates: {e}")))?;
    let parse = |s: &str| time::parse_rfc3339(s).ok_or_else(|| bad(format!("{key}: date {s:?}")));
    Ok((parse(&dates.issue_date)?, parse(&dates.next_update)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_signed_object_byte_for_byte() {
        let json = r#"{"tcbInfo":{"b":1, "a":[2]},"signature":"0aff"}"#;
        let s = signed(json, "tcbInfo").unwrap();
        assert_eq!(s.body, r#"{"b":1, "a":[2]}"#);
        assert_eq!(s.signature, vec![0x0a, 0xff]);
        assert!(signed(r#"{"tcbInfo":{}}"#, "tcbInfo").is_err());
    }

    #[test]
    fn decodes_intel_header_chains() {
        assert_eq!(percent_decode("-----BEGIN%20CERT%0A").unwrap(), "-----BEGIN CERT\n");
        assert_eq!(percent_decode("%zz"), None);
    }
}
