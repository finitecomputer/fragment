//! Every way a verification refuses, each naming the check that failed.

use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum Failure {
    /// The bytes are not a TDX quote we can read.
    Quote { detail: String },
    /// The collateral is missing, unreadable or not for this platform.
    Collateral { detail: String },
    /// TCB info or QE identity is past its nextUpdate (or before its
    /// issueDate) at the verification time.
    CollateralExpired { item: String, valid_from: String, valid_until: String, at: i64 },
    /// dcap-qvl refused: quote signature, attestation key, QE report,
    /// PCK chain to Intel's root, CRLs, TCB info or QE identity
    /// signatures, a Revoked TCB, or a TD attribute it forbids.
    Chain { detail: String },
    /// TD attribute DEBUG is set: the host can read and change the TD.
    Debug { td_attributes: String },
    /// Another "TD under debug" bit (0..7) is set, such as profiling.
    TdUnderDebug { td_attributes: String },
    /// REPORTDATA is not the nonce: a replayed or someone else's quote.
    Nonce { expected: String, actual: String },
    /// The TCB status is not UpToDate nor one the caller allowed.
    TcbStatus { status: String, allowed: Vec<String>, advisory_ids: Vec<String> },
    /// A register is not the value the caller expects.
    Register { register: String, expected: String, actual: String },
    /// MR_OWNER is not the SHA-384 of the named instance's PZID.
    GceInstance { expected: String, actual: String, payload: String },
    /// The host registry document is missing, for another PPID, or
    /// names another region.
    HostRegistry { detail: String },
    /// Fetching the quote, the log or the collateral failed.
    Fetch { detail: String },
}

impl Failure {
    pub fn check(&self) -> &'static str {
        match self {
            Self::Quote { .. } => "quote",
            Self::Collateral { .. } => "collateral",
            Self::CollateralExpired { .. } => "collateral_expired",
            Self::Chain { .. } => "chain",
            Self::Debug { .. } => "debug",
            Self::TdUnderDebug { .. } => "td_under_debug",
            Self::Nonce { .. } => "nonce",
            Self::TcbStatus { .. } => "tcb_status",
            Self::Register { .. } => "register",
            Self::GceInstance { .. } => "gce_instance",
            Self::HostRegistry { .. } => "host_registry",
            Self::Fetch { .. } => "fetch",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} check failed: ", self.check())?;
        match self {
            Self::Quote { detail }
            | Self::Collateral { detail }
            | Self::Chain { detail }
            | Self::HostRegistry { detail }
            | Self::Fetch { detail } => write!(f, "{detail}"),
            Self::CollateralExpired { item, valid_from, valid_until, at } => write!(
                f,
                "{item} is valid from {valid_from} until {valid_until}; verification time is {}",
                super::time::rfc3339(*at)
            ),
            Self::Debug { td_attributes } => {
                write!(f, "TD attributes {td_attributes} have DEBUG set; refusing a debug TD")
            }
            Self::TdUnderDebug { td_attributes } => {
                write!(f, "TD attributes {td_attributes} set a 'TD under debug' bit")
            }
            Self::Nonce { expected, actual } => {
                write!(f, "REPORTDATA {actual} is not the nonce {expected} (zero-padded)")
            }
            Self::TcbStatus { status, allowed, advisory_ids } => write!(
                f,
                "TCB status {status} (advisories: {}) is not in the accepted set [{}]",
                list(advisory_ids),
                allowed.join(", ")
            ),
            Self::Register { register, expected, actual } => {
                write!(f, "{register} is {actual}; expected {expected}")
            }
            Self::GceInstance { expected, actual, payload } => {
                write!(f, "MR_OWNER is {actual}; SHA-384 of {payload} is {expected}")
            }
        }
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

impl std::error::Error for Failure {}
