//! An AMD SEV-SNP attestation report: verified with virtee's `sev` crate
//! (ARK -> ASK -> VCEK or VLEK -> the report's ECDSA P-384 signature), the
//! ARK pinned to the Milan root the crate carries, the VCEK's chip id and
//! TCB read against the report's, the nonce in REPORT_DATA, and the guest
//! policy's debug bit. What the report names is returned for the caller to
//! read: no field of it names a cloud instance (docs/pod.md, "SEV-SNP").

use super::failure::Failure;
use der::Decode;
use serde::Serialize;
use sev::certs::snp::{builtin, ca, Certificate, Chain, Verifiable};
use sev::firmware::guest::AttestationReport;
use sev::firmware::host::TcbVersion;
use sev::parser::ByteParser;

/// The three certificates, DER: what configfs-tsm's auxblob carries on
/// Google Cloud, or what AMD's KDS serves for the chip and TCB.
pub struct Certs {
    pub vek: Vec<u8>,
    pub ask: Vec<u8>,
    pub ark: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tcb {
    pub bootloader: u8,
    pub tee: u8,
    pub snp: u8,
    pub microcode: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    pub raw: String,
    pub abi: String,
    pub smt_allowed: bool,
    pub migrate_ma_allowed: bool,
    pub debug_allowed: bool,
    pub single_socket_required: bool,
    pub page_swap_disabled: bool,
    pub ciphertext_hiding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub version: u32,
    pub signing_key: String,
    pub product: String,
    pub measurement: String,
    pub report_data: String,
    pub policy: Policy,
    pub vmpl: u32,
    pub guest_svn: u32,
    pub family_id: String,
    pub image_id: String,
    pub host_data: String,
    pub id_key_digest: String,
    pub author_key_digest: String,
    pub author_key_en: bool,
    pub report_id: String,
    pub report_id_ma: String,
    pub chip_id: String,
    pub mask_chip_id: bool,
    pub platform_info: String,
    pub smt_enabled: bool,
    pub current_tcb: Tcb,
    pub reported_tcb: Tcb,
    pub committed_tcb: Tcb,
    pub launch_tcb: Tcb,
    pub firmware: String,
    pub vek_hw_id: Option<String>,
    pub vek_tcb: Option<Tcb>,
    pub vek_subject: String,
}

fn chain_err(detail: impl Into<String>) -> Failure {
    Failure::Chain { detail: detail.into() }
}

pub fn parse(bytes: &[u8]) -> Result<AttestationReport, Failure> {
    AttestationReport::from_bytes(bytes).map_err(|e| Failure::Quote { detail: format!("not an SNP report: {e}") })
}

fn tcb(t: &TcbVersion) -> Tcb {
    Tcb { bootloader: t.bootloader, tee: t.tee, snp: t.snp, microcode: t.microcode }
}

/// Debug off: with it on, the host can read and change the guest's memory.
pub fn check_policy(r: &AttestationReport) -> Result<(), Failure> {
    if r.policy.debug_allowed() {
        return Err(Failure::Debug { td_attributes: format!("SNP guest policy {:#x}", r.policy.0) });
    }
    Ok(())
}

/// The VEK's AMD extensions (VCEK spec 57230, table 8): product name, the
/// TCB it was issued for, and (VCEK only) the chip id.
struct VekFacts {
    subject: String,
    product: String,
    hw_id: Option<Vec<u8>>,
    tcb: Option<Tcb>,
    not_before: i64,
    not_after: i64,
}

fn der_int(v: &[u8]) -> Option<u8> {
    // DER INTEGER 02 01 xx, or 02 02 00 xx
    match v {
        [0x02, 1, x] => Some(*x),
        [0x02, 2, 0, x] => Some(*x),
        _ => None,
    }
}

fn vek_facts(der: &[u8]) -> Result<VekFacts, Failure> {
    let cert = x509_cert::Certificate::from_der(der).map_err(|e| chain_err(format!("VEK certificate: {e}")))?;
    let tbs = cert.tbs_certificate();
    let mut ext = std::collections::BTreeMap::new();
    for e in tbs.extensions().map(|v| v.as_slice()).unwrap_or_default() {
        ext.insert(e.extn_id.to_string(), e.extn_value.as_bytes().to_vec());
    }
    let int = |oid: &str| ext.get(oid).and_then(|v| der_int(v));
    let tcb = match (int("1.3.6.1.4.1.3704.1.3.1"), int("1.3.6.1.4.1.3704.1.3.2"), int("1.3.6.1.4.1.3704.1.3.3"), int("1.3.6.1.4.1.3704.1.3.8")) {
        (Some(bootloader), Some(tee), Some(snp), Some(microcode)) => Some(Tcb { bootloader, tee, snp, microcode }),
        _ => None,
    };
    let hw_id = ext.get("1.3.6.1.4.1.3704.1.4").map(|v| match v.as_slice() {
        [0x04, 0x40, rest @ ..] if rest.len() == 64 => rest.to_vec(),
        other => other.to_vec(),
    });
    let product = ext
        .get("1.3.6.1.4.1.3704.1.2")
        .map(|v| String::from_utf8_lossy(v.get(2..).unwrap_or_default()).into_owned())
        .unwrap_or_default();
    let v = tbs.validity();
    Ok(VekFacts {
        subject: tbs.subject().to_string(),
        product,
        hw_id,
        tcb,
        not_before: v.not_before.to_unix_duration().as_secs() as i64,
        not_after: v.not_after.to_unix_duration().as_secs() as i64,
    })
}

fn cert(der: &[u8], what: &str) -> Result<Certificate, Failure> {
    Certificate::from_der(der).map_err(|e| chain_err(format!("{what} certificate: {e}")))
}

/// Verify `report` against `certs` at time `at`; `nonce`, when given, must
/// be REPORT_DATA (zero-padded to 64 bytes).
pub fn verify(report: &[u8], certs: &Certs, nonce: Option<&[u8]>, at: i64) -> Result<Summary, Failure> {
    let r = parse(report)?;
    let pinned = builtin::milan::ark().map_err(|e| chain_err(format!("pinned ARK: {e}")))?;
    let ark = cert(&certs.ark, "ARK")?;
    let pinned_der = pinned.to_der().map_err(|e| chain_err(e.to_string()))?;
    if ark.to_der().map_err(|e| chain_err(e.to_string()))? != pinned_der {
        return Err(chain_err("the ARK is not AMD's Milan root pinned in the sev crate"));
    }
    let chain = Chain { ca: ca::Chain { ark, ask: cert(&certs.ask, "ASK")? }, vek: cert(&certs.vek, "VEK")? };
    (&chain, &r).verify().map_err(|e| chain_err(format!("ARK -> ASK -> VEK -> report: {e}")))?;
    let facts = vek_facts(&certs.vek)?;
    if at < facts.not_before || at > facts.not_after {
        return Err(chain_err(format!("VEK valid {} to {}", super::time::rfc3339(facts.not_before), super::time::rfc3339(facts.not_after))));
    }
    let signing_key = match r.key_info.signing_key() {
        0 => "VCEK",
        1 => "VLEK",
        7 => "none",
        _ => "reserved",
    };
    if signing_key == "VCEK" && facts.hw_id.as_deref() != Some(&r.chip_id[..]) {
        return Err(chain_err("the VCEK's hwID is not the report's CHIP_ID"));
    }
    if let Some(n) = nonce {
        let mut want = [0u8; 64];
        if n.len() > 64 {
            return Err(Failure::Nonce { expected: hex::encode(n), actual: hex::encode(r.report_data) });
        }
        want[..n.len()].copy_from_slice(n);
        if want != r.report_data {
            return Err(Failure::Nonce { expected: hex::encode(n), actual: hex::encode(r.report_data) });
        }
    }
    check_policy(&r)?;
    let p = &r.policy;
    Ok(Summary {
        version: r.version,
        signing_key: signing_key.into(),
        product: facts.product,
        measurement: hex::encode(r.measurement),
        report_data: hex::encode(r.report_data),
        policy: Policy {
            raw: format!("{:#x}", p.0),
            abi: format!("{}.{}", p.abi_major(), p.abi_minor()),
            smt_allowed: p.smt_allowed(),
            migrate_ma_allowed: p.migrate_ma_allowed(),
            debug_allowed: p.debug_allowed(),
            single_socket_required: p.single_socket_required(),
            page_swap_disabled: p.page_swap_disabled(),
            ciphertext_hiding: p.ciphertext_hiding(),
        },
        vmpl: r.vmpl,
        guest_svn: r.guest_svn,
        family_id: hex::encode(r.family_id),
        image_id: hex::encode(r.image_id),
        host_data: hex::encode(r.host_data),
        id_key_digest: hex::encode(r.id_key_digest),
        author_key_digest: hex::encode(r.author_key_digest),
        author_key_en: r.key_info.author_key_en(),
        report_id: hex::encode(r.report_id),
        report_id_ma: hex::encode(r.report_id_ma),
        chip_id: hex::encode(r.chip_id),
        mask_chip_id: r.key_info.mask_chip_key(),
        platform_info: format!("{:#x}", r.plat_info.0),
        smt_enabled: r.plat_info.smt_enabled(),
        current_tcb: tcb(&r.current_tcb),
        reported_tcb: tcb(&r.reported_tcb),
        committed_tcb: tcb(&r.committed_tcb),
        launch_tcb: tcb(&r.launch_tcb),
        firmware: format!(
            "current {}.{}.{} committed {}.{}.{}",
            r.current.major, r.current.minor, r.current.build, r.committed.major, r.committed.minor, r.committed.build
        ),
        vek_hw_id: facts.hw_id.map(hex::encode),
        vek_tcb: facts.tcb,
        vek_subject: facts.subject,
    })
}
