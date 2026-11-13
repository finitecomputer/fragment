//! The checks before a disk key leaves its holder, so the owner's CLI
//! and a key service decide them with the same code: `quote` (Intel's
//! chain and the binding), `release` (`release`), `platform`,
//! `google_instance`; and where a verified quote comes from
//! (`attestor`). Each is a pure function of a verified quote and what
//! it is held to. The owner's CLI shows each as a line; a key service
//! refuses at the first that fails.

pub mod attestor;
pub mod release;

use crate::attest::pzid::Instance;
use crate::verify::failure::Failure;
use crate::verify::policy::{check_attributes, check_instance, check_status};
use crate::verify::provenance::HostRegistry;
use crate::verify::tdx::Verified;

/// REPORTDATA must be exactly the binding the caller recomputed from
/// what the service said and the caller's own nonce.
pub fn check_report_data(actual: &[u8; 64], expected: &[u8; 64]) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "REPORTDATA {} is not the binding of what the service said and our nonce ({})",
            hex::encode(actual),
            hex::encode(expected)
        ));
    }
    Ok(())
}

/// `quote`: the chain to Intel's root, then the binding. Until this
/// passes nothing the service said is trusted.
pub fn quote<'a>(chain: Result<&'a Verified, &Failure>, expected: &[u8; 64]) -> Result<&'a Verified, String> {
    let v = chain.map_err(|f| f.to_string())?;
    check_report_data(&v.report.report_data, expected)?;
    Ok(v)
}

/// `platform`: debug off, and the TCB status UpToDate or one allowed.
pub fn platform(v: &Verified, allow_tcb_status: &[String]) -> Result<String, String> {
    let attrs = check_attributes(v.report.td_attributes_u64()).map_err(|f| f.to_string())?;
    let tcb = check_status(v, allow_tcb_status).map_err(|f| f.to_string())?;
    Ok(format!("{attrs}; TCB {tcb}"))
}

/// `google_instance`: MR_OWNER names the instance, and Google's host
/// registry says the quote's PPID is a Google host in its region.
pub fn google_instance(v: &Verified, instance: &Instance, registry: Result<&HostRegistry, &Failure>) -> Result<String, String> {
    let owner = check_instance(&v.report.mr_owner, instance).map_err(|f| f.to_string())?;
    let host = registry.map_err(|f| f.to_string())?.check(&v.ppid, Some(instance)).map_err(|f| f.to_string())?;
    Ok(format!("{owner}; {host}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_data_must_be_equal() {
        assert_eq!(check_report_data(&[1; 64], &[1; 64]), Ok(()));
        assert!(check_report_data(&[1; 64], &[2; 64]).unwrap_err().contains("not the binding"));
    }

    #[test]
    fn a_failed_chain_is_the_quote_check_failing() {
        let f = Failure::Chain { detail: "bad signature".into() };
        let err = quote(Err(&f), &[0; 64]).unwrap_err();
        assert!(err.contains("bad signature"), "{err}");
    }
}
