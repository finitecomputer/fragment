//! Allowlisted model failure logs. No message, body, URL, header, model
//! input or identity enters this shape; vendor errors can echo all of them.
//! The cell emits it through its existing one-line console JSON logs.

use fragment_proto::ErrorCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Reservation(ErrorCode),
    Configuration(ErrorCode),
    Transport,
    Upstream(u16),
    ResponseBody,
    Settle(ErrorCode),
    Release(ErrorCode),
}

impl Failure {
    /// Transport adapters map only a failed send/binding call to
    /// UpstreamFailed. Key lookup and request construction fail before it.
    /// A 200 emits nothing; every other HTTP answer is a vendor refusal.
    pub fn attempt(answer: Result<u16, ErrorCode>) -> Option<Self> {
        match answer {
            Ok(200) => None,
            Ok(status) => Some(Self::Upstream(status)),
            Err(ErrorCode::UpstreamFailed) => Some(Self::Transport),
            Err(code) => Some(Self::Configuration(code)),
        }
    }

    /// Correlate a call (including its fallback) or a job step without
    /// logging the reference: job references contain fragment names.
    /// The release is deployment metadata, validated as Config validates it.
    /// Invalid metadata is omitted, never copied or repaired into a log.
    pub fn record(self, reference: &str, release: &str, attempt: Option<u32>, ms: i64) -> Value {
        let (event, code, outcome, error, status) = match self {
            Self::Reservation(e) => ("model.reservation-failed", "model_reservation_failed", "blocked_before_provider", Some(e), None),
            Self::Configuration(e) => ("model.attempt-failed", "model_configuration_failed", "blocked_before_provider", Some(e), None),
            Self::Transport => ("model.attempt-failed", "model_transport_failed", "transport_failure", Some(ErrorCode::UpstreamFailed), None),
            Self::Upstream(s) => ("model.attempt-failed", "model_upstream_rejected", "upstream_rejection", None, Some(s)),
            Self::ResponseBody => ("model.response-failed", "model_response_failed", "response_failure", Some(ErrorCode::UpstreamFailed), None),
            Self::Settle(e) => ("model.settle-failed", "model_settle_failed", "ledger_finalization_failure", Some(e), None),
            Self::Release(e) => ("model.release-failed", "model_release_failed", "ledger_finalization_failure", Some(e), None),
        };
        json!({
            "event": event, "code": code, "outcome": outcome,
            "request_id": correlation(reference),
            "release": crate::release::valid(release).then_some(release),
            "error_code": error, "status": status, "attempt": attempt,
            "ms": ms.max(0),
        })
    }
}

pub fn correlation(reference: &str) -> String {
    hex::encode(&Sha256::digest(reference.as_bytes())[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_distinguish_blocked_rejected_and_unreached() {
        let cases = [
            (Failure::Reservation(ErrorCode::BudgetUsedUp), "model_reservation_failed", "blocked_before_provider", None),
            (Failure::attempt(Err(ErrorCode::HostFailed)).unwrap(), "model_configuration_failed", "blocked_before_provider", None),
            (Failure::attempt(Ok(401)).unwrap(), "model_upstream_rejected", "upstream_rejection", Some(401)),
            (Failure::attempt(Ok(429)).unwrap(), "model_upstream_rejected", "upstream_rejection", Some(429)),
            (Failure::attempt(Ok(503)).unwrap(), "model_upstream_rejected", "upstream_rejection", Some(503)),
            (Failure::attempt(Err(ErrorCode::UpstreamFailed)).unwrap(), "model_transport_failed", "transport_failure", None),
        ];
        for (failure, code, outcome, status) in cases {
            let row = failure.record("aig:synthetic", "dev", Some(1), 23);
            assert_eq!(row["code"], code);
            assert_eq!(row["outcome"], outcome);
            assert_eq!(row["status"], json!(status));
            assert_eq!(row["ms"], 23);
        }
        assert_eq!(Failure::attempt(Ok(200)), None, "successful calls emit no failure");
    }

    #[test]
    fn diagnostics_only_serialize_the_allowlist() {
        // Synthetic canaries, never real credentials or customer content.
        let untrusted = "REDACTION_CANARY\nhttps://example.invalid/?token=CANARY";
        for failure in [
            Failure::Reservation(ErrorCode::BudgetUsedUp), Failure::Configuration(ErrorCode::HostFailed),
            Failure::Transport, Failure::Upstream(403), Failure::ResponseBody,
            Failure::Settle(ErrorCode::HostFailed), Failure::Release(ErrorCode::HostFailed),
        ] {
            let row = failure.record(untrusted, untrusted, None, -5);
            assert_eq!(row.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
                ["attempt", "code", "error_code", "event", "ms", "outcome", "release", "request_id", "status"]);
            assert_eq!(row["release"], Value::Null);
            assert_eq!(row["ms"], 0);
            assert_eq!(row["request_id"].as_str().unwrap().len(), 16);
            let serialized = row.to_string();
            for forbidden in ["REDACTION_CANARY", "example.invalid", "token=", "CANARY"] {
                assert!(!serialized.contains(forbidden));
            }
        }
    }

    #[test]
    fn diagnostics_correlate_fallback_and_finalization_without_names() {
        assert_eq!(correlation("abc"), "ba7816bf8f01cfea", "first 16 hex of SHA-256, as documented");
        let reference = "step:synthetic-fragment/run/42/attempt/1/step/0";
        let first = Failure::Upstream(502).record(reference, "0123456789abcdef", Some(1), 12);
        let fallback = Failure::Transport.record(reference, "0123456789abcdef", Some(2), 34);
        let release = Failure::Release(ErrorCode::HostFailed).record(reference, "0123456789abcdef", None, 50);
        assert_eq!(first["request_id"], fallback["request_id"]);
        assert_eq!(first["request_id"], release["request_id"]);
        assert_ne!(first["request_id"], correlation("another call"));
        assert_eq!(fallback["attempt"], 2);
        assert_eq!(release["release"], "0123456789abcdef");
        assert!(!first.to_string().contains("synthetic-fragment"));
    }
}
