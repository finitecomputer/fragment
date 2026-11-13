//! A refusal: the HTTP status, a stable code a client can match on, and
//! a detail for a person. No detail ever carries key material: the
//! sources below are cryptsetup's stderr, file errors and our own words.

use crate::http::Response;
use fragment_pod::unlock::wire::ErrorBody;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub detail: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, detail: impl Into<String>) -> Self {
        Self { status, code, detail: detail.into() }
    }

    pub fn response(&self) -> Response {
        let body = ErrorBody { error: self.code.to_owned(), detail: self.detail.clone() };
        // Two strings always serialise; the fallback is only for the type.
        let json = serde_json::to_vec(&body).unwrap_or_else(|_| b"{\"error\":\"internal\"}".to_vec());
        Response::json(self.status, json)
    }
}

/// The stable codes, in one place so the README and clients agree.
pub mod code {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const BAD_FIELD: &str = "bad_field";
    pub const TOO_LARGE: &str = "too_large";
    pub const BAD_NONCE: &str = "bad_nonce";
    pub const BAD_SIGNATURE: &str = "bad_signature";
    pub const BAD_SEAL: &str = "bad_seal";
    pub const WRONG_STATE: &str = "wrong_state";
    pub const WRONG_KEY: &str = "wrong_key";
    pub const OWNER_MISMATCH: &str = "owner_mismatch";
    pub const VOLUME_FAILED: &str = "volume_failed";
    pub const OPEN_HOOK_FAILED: &str = "open_hook_failed";
    pub const QUOTE_FAILED: &str = "quote_failed";
    pub const NOT_FOUND: &str = "not_found";
    pub const METHOD: &str = "method_not_allowed";
    pub const INTERNAL: &str = "internal";
}
