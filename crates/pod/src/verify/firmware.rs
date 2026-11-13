//! Google's signed launch endorsement for its TDX firmware: a
//! VMLaunchEndorsement (gce-tcb-verifier proto/endorsement.proto) at
//! gs://gce_tcb_integrity/ovmf_x64_csm/tdx/<MRTD hex>.binarypb, signed
//! RSA-PSS SHA-256 by a GCE-uefi-signer key whose certificate chains to
//! Google's GCE-cc-tcb-root (pinned below, from
//! https://pki.goog/cloud_integrity/GCE-cc-tcb-root_1.crt). It lists the
//! MRTDs that firmware build gives. Steps as in
//! https://docs.cloud.google.com/confidential-computing/confidential-vm/docs/verify-firmware

use super::failure::Failure;
use super::proto;
use der::{Decode, Encode};
use ring::signature::{UnparsedPublicKey, RSA_PSS_2048_8192_SHA256};
use serde::Serialize;
use x509_cert::Certificate;

pub const ROOT_DER: &[u8] = include_bytes!("../../roots/GCE-cc-tcb-root_1.crt");
pub const BUCKET_URL: &str = "https://storage.googleapis.com/gce_tcb_integrity/ovmf_x64_csm/tdx";
const RSASSA_PSS: &str = "1.2.840.113549.1.1.10";

#[derive(Debug, Clone, Serialize)]
pub struct Endorsement {
    pub signer: String,
    pub signed_at: i64,
    pub cl_spec: u64,
    pub uefi_digest: String,
    pub svn: u64,
    pub measurements: Vec<Measurement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Measurement {
    /// 0 when the MRTD does not depend on the VM's RAM.
    pub ram_gib: u64,
    pub early_accept: bool,
    pub mrtd: String,
}

fn bad(detail: impl Into<String>) -> Failure {
    Failure::Chain { detail: format!("firmware endorsement: {}", detail.into()) }
}

/// Verify `file` against the pinned root at time `at`, and return what
/// it endorses.
pub fn verify(file: &[u8], at: i64) -> Result<Endorsement, Failure> {
    let (golden, signer) = signed_golden(file, at)?;
    let g = proto::fields(golden).map_err(|e| bad(e.to_string()))?;
    let timestamp = proto::bytes(&g, 1).map(proto::fields).transpose().map_err(|e| bad(e.to_string()))?;
    let tdx = proto::bytes(&g, 8).ok_or_else(|| bad("no TDX section"))?;
    let tdx = proto::fields(tdx).map_err(|e| bad(e.to_string()))?;
    let measurements = proto::all_bytes(&tdx, 2)
        .into_iter()
        .map(|m| {
            let m = proto::fields(m).map_err(|e| bad(e.to_string()))?;
            Ok(Measurement {
                ram_gib: proto::uint(&m, 1),
                early_accept: proto::uint(&m, 2) != 0,
                mrtd: hex::encode(proto::bytes(&m, 3).unwrap_or_default()),
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(Endorsement {
        signer,
        signed_at: timestamp.map_or(0, |t| proto::uint(&t, 1) as i64),
        cl_spec: proto::uint(&g, 2),
        uefi_digest: hex::encode(proto::bytes(&g, 5).unwrap_or_default()),
        svn: proto::uint(&tdx, 1),
        measurements,
    })
}

/// The signed VMGoldenMeasurement inside an endorsement file, after its
/// signer's chain to the pinned root and its signature check out.
fn signed_golden(file: &[u8], at: i64) -> Result<(&[u8], String), Failure> {
    let top = proto::fields(file).map_err(|e| bad(e.to_string()))?;
    let golden = proto::bytes(&top, 1).ok_or_else(|| bad("no serialized_uefi_golden"))?;
    let signature = proto::bytes(&top, 2).ok_or_else(|| bad("no signature"))?;
    let g = proto::fields(golden).map_err(|e| bad(e.to_string()))?;
    let cert_der = proto::bytes(&g, 4).ok_or_else(|| bad("no signer certificate"))?;
    let signer = check_signer(cert_der, at)?;
    let cert = Certificate::from_der(cert_der).map_err(|e| bad(e.to_string()))?;
    UnparsedPublicKey::new(&RSA_PSS_2048_8192_SHA256, spki_bits(&cert))
        .verify(golden, signature)
        .map_err(|_| bad("signature over the golden measurement does not verify"))?;
    Ok((golden, signer))
}

/// The SEV-SNP part of the same endorsement (VMGoldenMeasurement field 7,
/// VMSevSnp): one launch measurement per vCPU count, since each vCPU's
/// initial VMSA page is measured with the firmware.
#[derive(Debug, Clone, Serialize)]
pub struct SnpEndorsement {
    pub signer: String,
    pub signed_at: i64,
    pub cl_spec: u64,
    pub uefi_digest: String,
    pub svn: u64,
    /// VMSevSnp field 5: the guest policy the firmware was measured for.
    pub policy: u64,
    pub measurements: Vec<SnpMeasurement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnpMeasurement {
    pub vcpus: u64,
    pub measurement: String,
}

impl SnpEndorsement {
    pub fn covers(&self, measurement_hex: &str) -> Option<&SnpMeasurement> {
        self.measurements.iter().find(|m| m.measurement == measurement_hex)
    }
}

pub fn verify_snp(file: &[u8], at: i64) -> Result<SnpEndorsement, Failure> {
    let (golden, signer) = signed_golden(file, at)?;
    let g = proto::fields(golden).map_err(|e| bad(e.to_string()))?;
    let timestamp = proto::bytes(&g, 1).map(proto::fields).transpose().map_err(|e| bad(e.to_string()))?;
    let snp = proto::bytes(&g, 7).ok_or_else(|| bad("no SEV-SNP section"))?;
    let snp = proto::fields(snp).map_err(|e| bad(e.to_string()))?;
    let mut measurements = proto::all_bytes(&snp, 2)
        .into_iter()
        .map(|m| {
            let m = proto::fields(m).map_err(|e| bad(e.to_string()))?;
            Ok(SnpMeasurement { vcpus: proto::uint(&m, 1), measurement: hex::encode(proto::bytes(&m, 2).unwrap_or_default()) })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    measurements.sort_by_key(|m| m.vcpus);
    Ok(SnpEndorsement {
        signer,
        signed_at: timestamp.map_or(0, |t| proto::uint(&t, 1) as i64),
        cl_spec: proto::uint(&g, 2),
        uefi_digest: hex::encode(proto::bytes(&g, 5).unwrap_or_default()),
        svn: proto::uint(&snp, 1),
        policy: proto::uint(&snp, 5),
        measurements,
    })
}

pub const SNP_BUCKET_URL: &str = "https://storage.googleapis.com/gce_tcb_integrity/ovmf_x64_csm/sevsnp";

pub fn fetch_snp(measurement: &[u8; 48]) -> Result<Vec<u8>, Failure> {
    let url = format!("{SNP_BUCKET_URL}/{}.binarypb", hex::encode(measurement));
    let resp = reqwest::blocking::get(&url).map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })?;
    if !resp.status().is_success() {
        return Err(bad(format!("{url}: HTTP {} (no endorsement for this measurement)", resp.status())));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })
}

fn spki_bits(cert: &Certificate) -> &[u8] {
    cert.tbs_certificate().subject_public_key_info().subject_public_key.raw_bytes()
}

/// The signer's certificate is issued by the pinned root, signed with
/// RSA-PSS, and valid at `at`.
fn check_signer(cert_der: &[u8], at: i64) -> Result<String, Failure> {
    let root = Certificate::from_der(ROOT_DER).map_err(|e| bad(format!("root: {e}")))?;
    let cert = Certificate::from_der(cert_der).map_err(|e| bad(format!("signer: {e}")))?;
    let tbs = cert.tbs_certificate();
    if tbs.issuer() != root.tbs_certificate().subject() {
        return Err(bad(format!("signer is issued by {}, not the pinned root", tbs.issuer())));
    }
    if cert.signature_algorithm().oid.to_string() != RSASSA_PSS {
        return Err(bad("signer certificate is not signed with RSA-PSS"));
    }
    let tbs_der = tbs.to_der().map_err(|e| bad(e.to_string()))?;
    let sig = cert.signature().raw_bytes();
    UnparsedPublicKey::new(&RSA_PSS_2048_8192_SHA256, spki_bits(&root))
        .verify(&tbs_der, sig)
        .map_err(|_| bad("signer certificate's signature does not verify under the root"))?;
    let v = tbs.validity();
    let from = v.not_before.to_unix_duration().as_secs() as i64;
    let until = v.not_after.to_unix_duration().as_secs() as i64;
    if at < from || at > until {
        return Err(bad(format!(
            "signer certificate valid {} to {}",
            super::time::rfc3339(from),
            super::time::rfc3339(until)
        )));
    }
    Ok(tbs.subject().to_string())
}

impl Endorsement {
    pub fn covers(&self, mrtd: &[u8; 48]) -> Option<&Measurement> {
        let want = hex::encode(mrtd);
        self.measurements.iter().find(|m| m.mrtd == want)
    }
}

pub fn fetch(mrtd: &[u8; 48]) -> Result<Vec<u8>, Failure> {
    let url = format!("{BUCKET_URL}/{}.binarypb", hex::encode(mrtd));
    let resp = reqwest::blocking::get(&url).map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })?;
    if !resp.status().is_success() {
        return Err(bad(format!("{url}: HTTP {} (no endorsement for this MRTD)", resp.status())));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| Failure::Fetch { detail: format!("{url}: {e}") })
}

