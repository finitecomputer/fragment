//! The verification: dcap-qvl for the chain to Intel's root, then our
//! own checks on what it leaves to the caller (src/policy.rs).

use super::collateral::Collateral;
use super::failure::Failure;
use super::policy::{self, Policy};
use dcap_qvl::configs::RustCryptoConfig;
use dcap_qvl::verify::QuoteVerifier;
use dcap_qvl::{PckCertFlag, QuotePolicy};
use crate::attest::report::{self, RegistersHex, TdReport};
use serde::Serialize;

/// What a verified quote says, for printing and for policy.
#[derive(Debug, Clone, Serialize)]
pub struct Verified {
    pub quote_version: u16,
    pub tcb_status: String,
    pub advisory_ids: Vec<String>,
    pub tcb_eval_data_number: u32,
    pub platform_tcb_date: String,
    pub fmspc: String,
    pub ppid: String,
    pub pck_flags: PckFlags,
    pub collateral_expires: i64,
    pub verified_at: i64,
    pub td_attribute_flags: Vec<&'static str>,
    pub registers: RegistersHex,
    #[serde(skip)]
    pub report: TdReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct PckFlags {
    pub smt_enabled: String,
    pub dynamic_platform: String,
    pub cached_keys: String,
}

fn flag(f: PckCertFlag) -> String {
    match f {
        PckCertFlag::True => "true",
        PckCertFlag::False => "false",
        PckCertFlag::Undefined => "not stated",
    }
    .to_owned()
}

/// Chain verification alone: no policy. A failure here is always fatal.
pub fn chain(quote: &[u8], collateral: &Collateral, at: i64) -> Result<Verified, Failure> {
    let report = report::parse(quote).map_err(|e| Failure::Quote { detail: e.to_string() })?;
    collateral.check_fresh(at)?;
    let now = u64::try_from(at).map_err(|_| Failure::Collateral { detail: "time before 1970".into() })?;
    // allow_debug(true): dcap-qvl would refuse a debug TD with an
    // untyped error; policy::check_attributes refuses it with ours.
    // Its other attribute checks (reserved bits, migratable, SEPT_VE_DISABLE)
    // still apply.
    let claims = QuoteVerifier::new_prod()
        .with_config::<RustCryptoConfig>()
        .allow_debug(true)
        .verify_with_policy(quote, collateral.to_dcap()?, now, &QuotePolicy::claims_only(now))
        .map_err(|e| Failure::Chain { detail: format!("{e:#}") })?;
    let pck = &claims.platform.pck;
    Ok(Verified {
        quote_version: report.quote_version,
        tcb_status: claims.tcb.status.to_string(),
        advisory_ids: claims.tcb.advisory_ids.clone(),
        tcb_eval_data_number: claims.tcb.eval_data_number,
        platform_tcb_date: claims.platform.tcb_level.tcb_date.clone(),
        fmspc: hex::encode_upper(pck.fmspc),
        ppid: hex::encode(&pck.ppid),
        pck_flags: PckFlags {
            smt_enabled: flag(pck.smt_enabled),
            dynamic_platform: flag(pck.dynamic_platform),
            cached_keys: flag(pck.cached_keys),
        },
        collateral_expires: claims.earliest_expiration_date as i64,
        verified_at: at,
        td_attribute_flags: policy::attribute_names(report.td_attributes_u64()),
        registers: RegistersHex::from(&report),
        report,
    })
}

/// The whole verification: chain, then every policy check. Returns the
/// verified quote and the first failure, if any; checks after a policy
/// failure still run, so the output shows all of them.
pub fn verify(
    quote: &[u8],
    collateral: &Collateral,
    at: i64,
    policy: &Policy,
) -> Result<(Verified, Vec<policy::Outcome>), Failure> {
    let verified = chain(quote, collateral, at)?;
    let outcomes = policy::check(&verified, policy);
    Ok((verified, outcomes))
}
