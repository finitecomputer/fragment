//! Every refusal, typed: the HTTP status, a stable code a client can
//! match on, and a detail for a person. A refused release names the
//! check that failed. No detail carries key material: the sources are
//! our own words, parse errors and pod-verify's check failures.

use fragment_pod_init::http::Response;
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::wire::{ErrorBody, WireError};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    BadRequest(String),
    BadField(String),
    TooLarge(String),
    /// `/status?nonce=` or a release's nonce is not 64 hex digits.
    BadNonce(String),
    /// Not one this boot issued, already used, or expired.
    UnknownNonce,
    /// The owner's pods were given their key as often as a minute allows.
    TooManyReleases,
    BadSignature(String),
    /// The deposit's grant does not name this boot of the key service.
    BadGrant,
    BadSeal(String),
    /// A deposit from an owner the operator did not name at start.
    NotAdmitted,
    UnknownOwner,
    UnknownRecord,
    /// An approval signed by someone other than the record's owner.
    NotOwner,
    /// A deposit that names a record other than the current one.
    StaleRecord,
    TooManyRecords,
    TooManyApprovals,
    DuplicateSeq(u64),
    BelowMinSeq { seq: u64, min_seq: u64 },
    /// The pod's status names no owner, so there is no record to ask.
    NoOwner,
    /// The release checks, by name.
    Quote(String),
    Release(String),
    NotYetValid { name: String, seq: u64, not_before: u64, now: i64 },
    Expired { name: String, seq: u64, expires: u64, now: i64 },
    Rollback { name: String, seq: u64, min_seq: u64 },
    Platform(String),
    GoogleInstance(String),
    WrongState(State),
    /// This service's own quote failed.
    QuoteFailed(String),
    NotFound(String),
    Method(String),
    Internal(String),
}

impl Refusal {
    pub fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) | Self::BadField(_) | Self::BadNonce(_) | Self::NoOwner => 400,
            Self::BadSignature(_) | Self::BadGrant => 401,
            Self::BadSeal(_) | Self::UnknownNonce | Self::NotOwner | Self::NotAdmitted => 403,
            Self::Quote(_) | Self::Release(_) | Self::NotYetValid { .. } | Self::Expired { .. } => 403,
            Self::Rollback { .. } | Self::Platform(_) | Self::GoogleInstance(_) => 403,
            Self::UnknownOwner | Self::UnknownRecord | Self::NotFound(_) => 404,
            Self::Method(_) => 405,
            Self::StaleRecord | Self::DuplicateSeq(_) | Self::BelowMinSeq { .. } | Self::WrongState(_) => 409,
            Self::TooManyApprovals => 409,
            Self::TooLarge(_) => 413,
            Self::TooManyReleases | Self::TooManyRecords => 503,
            Self::QuoteFailed(_) | Self::Internal(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::BadField(_) => "bad_field",
            Self::TooLarge(_) => "too_large",
            Self::BadNonce(_) => "bad_nonce",
            Self::UnknownNonce => "unknown_nonce",
            Self::TooManyReleases => "too_many_releases",
            Self::BadSignature(_) => "bad_signature",
            Self::BadGrant => "bad_grant",
            Self::BadSeal(_) => "bad_seal",
            Self::NotAdmitted => "not_admitted",
            Self::UnknownOwner => "unknown_owner",
            Self::UnknownRecord => "unknown_record",
            Self::NotOwner => "not_owner",
            Self::StaleRecord => "stale_record",
            Self::TooManyRecords => "too_many_records",
            Self::TooManyApprovals => "too_many_approvals",
            Self::DuplicateSeq(_) => "duplicate_seq",
            Self::BelowMinSeq { .. } => "below_min_seq",
            Self::NoOwner => "no_owner",
            Self::Quote(_) => "quote",
            Self::Release(_) => "release",
            Self::NotYetValid { .. } => "not_yet_valid",
            Self::Expired { .. } => "expired",
            Self::Rollback { .. } => "rollback",
            Self::Platform(_) => "platform",
            Self::GoogleInstance(_) => "google_instance",
            Self::WrongState(_) => "wrong_state",
            Self::QuoteFailed(_) => "quote_failed",
            Self::NotFound(_) => "not_found",
            Self::Method(_) => "method_not_allowed",
            Self::Internal(_) => "internal",
        }
    }

    pub fn response(&self) -> Response {
        let body = ErrorBody { error: self.code().to_owned(), detail: self.to_string() };
        // Two strings always serialise; the fallback is only for the type.
        let json = serde_json::to_vec(&body).unwrap_or_else(|_| b"{\"error\":\"internal\"}".to_vec());
        Response::json(self.status(), json)
    }

    pub fn wire(e: WireError) -> Self {
        match e {
            WireError::TooLarge { .. } => Self::TooLarge(e.to_string()),
            WireError::Field { .. } => Self::BadField(e.to_string()),
            WireError::Json(_) | WireError::Version(_) => Self::BadRequest(e.to_string()),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(d) | Self::BadField(d) | Self::TooLarge(d) | Self::BadNonce(d) => f.write_str(d),
            Self::BadSignature(d) | Self::BadSeal(d) | Self::NotFound(d) | Self::Method(d) | Self::Internal(d) => f.write_str(d),
            Self::UnknownNonce => f.write_str("not a nonce this boot of the key service issued, or used, or expired; GET /nonce"),
            Self::TooManyReleases => f.write_str("this owner's pods were given their key as often as a minute allows; try again shortly"),
            Self::NotAdmitted => f.write_str("this key service takes no deposit from this owner: its operator names the owners it serves when it starts"),
            Self::BadGrant => f.write_str("the owner's grant does not name this boot of the key service"),
            Self::UnknownOwner => f.write_str("no record for this owner (none deposited since the key service started)"),
            Self::UnknownRecord => f.write_str("no such record (replaced, or the key service restarted)"),
            Self::NotOwner => f.write_str("signed by a key that is not the record's owner"),
            Self::StaleRecord => f.write_str("the deposit does not name the current record it replaces"),
            Self::TooManyRecords => f.write_str("the key service holds as many records as it takes"),
            Self::TooManyApprovals => f.write_str("the record holds as many approvals as it takes"),
            Self::DuplicateSeq(s) => write!(f, "the record already has an approval with seq {s}"),
            Self::BelowMinSeq { seq, min_seq } => write!(f, "seq {seq} is below the record's min_seq {min_seq}"),
            Self::NoOwner => f.write_str("the pod's status names no owner"),
            Self::Quote(d) => write!(f, "quote check failed: {d}"),
            Self::Release(d) => write!(f, "release check failed: {d}"),
            Self::NotYetValid { name, seq, not_before, now } => {
                write!(f, "release check failed: {name} (seq {seq}) is approved from {not_before}; now is {now}")
            }
            Self::Expired { name, seq, expires, now } => {
                write!(f, "release check failed: {name} (seq {seq}) expired at {expires}; now is {now}")
            }
            Self::Rollback { name, seq, min_seq } => {
                write!(f, "release check failed: {name} (seq {seq}) is older than min_seq {min_seq}")
            }
            Self::Platform(d) => write!(f, "platform check failed: {d}"),
            Self::GoogleInstance(d) => write!(f, "google_instance check failed: {d}"),
            Self::WrongState(s) => write!(f, "the pod is {}; a release needs it locked", s.as_str()),
            Self::QuoteFailed(d) => write!(f, "this key service's quote failed: {d}"),
        }
    }
}

impl std::error::Error for Refusal {}
