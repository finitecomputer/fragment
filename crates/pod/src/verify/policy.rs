//! The checks dcap-qvl leaves to the caller: the TD attributes, the
//! nonce, which TCB statuses are acceptable, the expected registers,
//! and GCE provenance (MR_OWNER and the host registry).

use super::failure::Failure;
use super::provenance::HostRegistry;
use super::tdx::Verified;
use crate::attest::nonce;
use crate::attest::pzid::Instance;
use crate::attest::report::attr;
use serde::Serialize;

pub const UP_TO_DATE: &str = "UpToDate";
/// Statuses a caller may name in --allow-tcb-status. Revoked is never
/// acceptable (dcap-qvl refuses it before we see it).
pub const NAMED_STATUSES: [&str; 7] = [
    "SWHardeningNeeded",
    "ConfigurationNeeded",
    "ConfigurationAndSWHardeningNeeded",
    "OutOfDate",
    "OutOfDateConfigurationNeeded",
    "TDRelaunchAdvised",
    "TDRelaunchAdvisedConfigurationNeeded",
];
pub const REGISTERS: [&str; 5] = ["mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"];

#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// The nonce REPORTDATA must carry; None only when the caller said
    /// --no-nonce (re-checking an old quote).
    pub nonce: Option<Vec<u8>>,
    pub allowed_statuses: Vec<String>,
    pub expect: Vec<(&'static str, Vec<u8>)>,
    pub gce_instance: Option<Instance>,
    pub host_registry: Option<HostRegistry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub check: String,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Skipped,
}

impl Outcome {
    fn from(check: impl Into<String>, r: Result<String, Failure>) -> Self {
        let check = check.into();
        match r {
            Ok(detail) => Self { check, status: Status::Pass, detail, failure: None },
            Err(f) => Self { check, status: Status::Fail, detail: f.to_string(), failure: Some(f) },
        }
    }
    fn skipped(check: &str, why: &str) -> Self {
        Self { check: check.into(), status: Status::Skipped, detail: why.into(), failure: None }
    }
}

pub fn check(v: &Verified, p: &Policy) -> Vec<Outcome> {
    let mut out = vec![Outcome::from("td_attributes", check_attributes(v.report.td_attributes_u64()))];
    out.push(match &p.nonce {
        Some(n) => Outcome::from("nonce", check_nonce(&v.report.report_data, n)),
        None => Outcome::skipped("nonce", "--no-nonce: freshness not checked"),
    });
    out.push(Outcome::from("tcb_status", check_status(v, &p.allowed_statuses)));
    for (name, expected) in &p.expect {
        let check = format!("expect_{name}");
        out.push(Outcome::from(check, check_register(v, name, expected)));
    }
    out.push(match &p.gce_instance {
        Some(i) => Outcome::from("gce_instance", check_instance(&v.report.mr_owner, i)),
        None => Outcome::skipped("gce_instance", "no --expect-gce-instance"),
    });
    out.push(match &p.host_registry {
        Some(h) => Outcome::from("host_registry", h.check(&v.ppid, p.gce_instance.as_ref())),
        None => Outcome::skipped("host_registry", "no --host-registry"),
    });
    out
}

pub fn first_failure(outcomes: &[Outcome]) -> Option<&Failure> {
    outcomes.iter().find_map(|o| o.failure.as_ref())
}

pub fn check_attributes(td_attributes: u64) -> Result<String, Failure> {
    let shown = hex::encode(td_attributes.to_le_bytes());
    if td_attributes & attr::DEBUG != 0 {
        return Err(Failure::Debug { td_attributes: shown });
    }
    if td_attributes & attr::TUD_MASK != 0 {
        return Err(Failure::TdUnderDebug { td_attributes: shown });
    }
    Ok(format!("{shown}: debug off ({})", attribute_names(td_attributes).join(", ")))
}

pub fn attribute_names(a: u64) -> Vec<&'static str> {
    let named = [
        (attr::DEBUG, "DEBUG"),
        (attr::SEPT_VE_DISABLE, "SEPT_VE_DISABLE"),
        (attr::PKS, "PKS"),
        (attr::KL, "KL"),
        (attr::PERFMON, "PERFMON"),
    ];
    let mut out: Vec<&'static str> = named.iter().filter(|(b, _)| a & b != 0).map(|(_, n)| *n).collect();
    if a & attr::TUD_MASK & !attr::DEBUG != 0 {
        out.push("TUD_OTHER");
    }
    out
}

pub fn check_nonce(report_data: &[u8; 64], nonce: &[u8]) -> Result<String, Failure> {
    let expected = nonce::report_data(nonce).map_err(|e| Failure::Nonce {
        expected: format!("invalid nonce: {e}"),
        actual: hex::encode(report_data),
    })?;
    if &expected != report_data {
        return Err(Failure::Nonce { expected: hex::encode(nonce), actual: hex::encode(report_data) });
    }
    Ok(format!("REPORTDATA is the {}-byte nonce", nonce.len()))
}

pub fn check_status(v: &Verified, allowed: &[String]) -> Result<String, Failure> {
    let ok = v.tcb_status == UP_TO_DATE || allowed.contains(&v.tcb_status);
    if !ok {
        let mut set = vec![UP_TO_DATE.to_owned()];
        set.extend(allowed.iter().cloned());
        return Err(Failure::TcbStatus {
            status: v.tcb_status.clone(),
            allowed: set,
            advisory_ids: v.advisory_ids.clone(),
        });
    }
    let advisories = if v.advisory_ids.is_empty() { "none".to_owned() } else { v.advisory_ids.join(", ") };
    Ok(format!("{} (advisories: {advisories})", v.tcb_status))
}

fn check_register(v: &Verified, name: &str, expected: &[u8]) -> Result<String, Failure> {
    let actual = v
        .report
        .registers()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, b)| b)
        .unwrap_or_default();
    if actual != expected {
        return Err(Failure::Register {
            register: name.to_owned(),
            expected: hex::encode(expected),
            actual: hex::encode(actual),
        });
    }
    Ok("matches".into())
}

pub fn check_instance(mr_owner: &[u8; 48], instance: &Instance) -> Result<String, Failure> {
    let expected = instance.mr_owner();
    if &expected != mr_owner {
        return Err(Failure::GceInstance {
            expected: hex::encode(expected),
            actual: hex::encode(mr_owner),
            payload: instance.payload(),
        });
    }
    Ok(format!("MR_OWNER = SHA-384({})", instance.payload()))
}

pub fn parse_status(s: &str) -> Result<String, String> {
    if NAMED_STATUSES.contains(&s) {
        Ok(s.to_owned())
    } else {
        Err(format!("unknown TCB status {s:?}; one of {}", NAMED_STATUSES.join(", ")))
    }
}

pub fn parse_register(hex_text: &str) -> Result<Vec<u8>, String> {
    let b = hex::decode(hex_text.trim()).map_err(|_| "not hex".to_owned())?;
    if b.len() != 48 {
        return Err(format!("{} bytes; a register is 48", b.len()));
    }
    Ok(b)
}
