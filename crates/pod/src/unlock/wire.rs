//! The JSON on the wire. Binary fields are lowercase hex of a fixed
//! length; every length is checked before decoding, and unknown fields
//! are refused, so a parsed value is exactly what was signed or attested.

use super::binding::{Binding, State, NONCE_LEN};
use super::seal::CT_LEN;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const VERSION: u32 = 1;
/// A key request is about 450 bytes of JSON.
pub const MAX_REQUEST_BODY: usize = 4 * 1024;
/// Larger than any DCAP quote (ours are 8,000 bytes).
pub const MAX_QUOTE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    TooLarge { len: usize, max: usize },
    Json(String),
    Field { field: &'static str, why: String },
    Version(u32),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { len, max } => write!(f, "body is {len} bytes; at most {max}"),
            Self::Json(e) => write!(f, "not the expected JSON: {e}"),
            Self::Field { field, why } => write!(f, "{field}: {why}"),
            Self::Version(v) => write!(f, "version {v}; this client speaks {VERSION}"),
        }
    }
}

impl std::error::Error for WireError {}

pub(crate) fn json<T: for<'a> Deserialize<'a>>(body: &[u8]) -> Result<T, WireError> {
    serde_json::from_slice(body).map_err(|e| WireError::Json(e.to_string()))
}

/// Decode lowercase hex of exactly `out.len()` bytes into `out`.
pub(crate) fn hex_into(field: &'static str, text: &str, out: &mut [u8]) -> Result<(), WireError> {
    let bad = |why: String| WireError::Field { field, why };
    if text.len() != out.len() * 2 {
        return Err(bad(format!("{} hex digits; expected {}", text.len(), out.len() * 2)));
    }
    if text.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(bad("hex must be lowercase".into()));
    }
    hex::decode_to_slice(text, out).map_err(|_| bad("not hex".into()))
}

pub fn hex32(field: &'static str, text: &str) -> Result<[u8; 32], WireError> {
    let mut out = [0u8; 32];
    hex_into(field, text, &mut out)?;
    Ok(out)
}

pub fn nonce(text: &str) -> Result<[u8; NONCE_LEN], WireError> {
    hex32("nonce", text)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusWire {
    version: u32,
    state: State,
    boot_pubkey: String,
    owner: Option<String>,
    tls_root: Option<String>,
    quote: String,
}

/// `GET /status`. Nothing in it is trusted until its binding, with the
/// client's nonce, equals the verified quote's REPORTDATA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub boot_pubkey: [u8; 32],
    pub owner: Option<[u8; 32]>,
    pub tls_root: Option<[u8; 32]>,
    pub quote: Vec<u8>,
}

impl Status {
    pub fn binding(&self, nonce: [u8; NONCE_LEN]) -> Binding {
        Binding {
            nonce,
            boot_pubkey: self.boot_pubkey,
            state: self.state,
            owner: self.owner,
            tls_root: self.tls_root,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let wire = StatusWire {
            version: VERSION,
            state: self.state,
            boot_pubkey: hex::encode(self.boot_pubkey),
            owner: self.owner.map(hex::encode),
            tls_root: self.tls_root.map(hex::encode),
            quote: hex::encode(&self.quote),
        };
        serde_json::to_vec(&wire).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        let w: StatusWire = json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        if w.quote.len() > MAX_QUOTE * 2 || w.quote.is_empty() {
            return Err(WireError::Field { field: "quote", why: format!("{} hex digits", w.quote.len()) });
        }
        let mut quote = vec![0u8; w.quote.len() / 2];
        hex_into("quote", &w.quote, &mut quote)?;
        Ok(Self {
            state: w.state,
            boot_pubkey: hex32("boot_pubkey", &w.boot_pubkey)?,
            owner: w.owner.as_deref().map(|o| hex32("owner", o)).transpose()?,
            tls_root: w.tls_root.as_deref().map(|t| hex32("tls_root", t)).transpose()?,
            quote,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    owner: String,
    enc: String,
    ct: String,
    sig: String,
}

/// The body of `POST /init` and `POST /unlock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRequest {
    pub owner: [u8; 32],
    pub enc: [u8; 32],
    pub ct: [u8; CT_LEN],
    pub sig: [u8; 64],
}

impl KeyRequest {
    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let wire = RequestWire {
            owner: hex::encode(self.owner),
            enc: hex::encode(self.enc),
            ct: hex::encode(self.ct),
            sig: hex::encode(self.sig),
        };
        serde_json::to_vec(&wire).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_REQUEST_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_REQUEST_BODY });
        }
        let w: RequestWire = json(body)?;
        let mut r = Self { owner: [0; 32], enc: [0; 32], ct: [0; CT_LEN], sig: [0; 64] };
        hex_into("owner", &w.owner, &mut r.owner)?;
        hex_into("enc", &w.enc, &mut r.enc)?;
        hex_into("ct", &w.ct, &mut r.ct)?;
        hex_into("sig", &w.sig, &mut r.sig)?;
        Ok(r)
    }
}

/// A refusal: a stable code to match on, and words for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub detail: String,
}

/// What a successful `/init` or `/unlock` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub state: State,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> KeyRequest {
        KeyRequest { owner: [1; 32], enc: [2; 32], ct: [3; CT_LEN], sig: [4; 64] }
    }

    fn status() -> Status {
        Status { state: State::Locked, boot_pubkey: [5; 32], owner: Some([6; 32]), tls_root: None, quote: vec![7; 10] }
    }

    #[test]
    fn requests_round_trip() {
        let json = request().to_json().unwrap();
        assert_eq!(KeyRequest::from_json(&json).unwrap(), request());
    }

    #[test]
    fn statuses_round_trip_with_nulls() {
        let json = status().to_json().unwrap();
        let text = String::from_utf8(json.clone()).unwrap();
        assert!(text.contains("\"version\":1") && text.contains("\"tls_root\":null"), "{text}");
        assert_eq!(Status::from_json(&json).unwrap(), status());
    }

    fn with(field: &str, value: serde_json::Value) -> Vec<u8> {
        let mut v: serde_json::Value = serde_json::from_slice(&request().to_json().unwrap()).unwrap();
        v[field] = value;
        serde_json::to_vec(&v).unwrap()
    }

    #[test]
    fn refuses_wrong_lengths_case_non_hex_and_extra_fields() {
        let short = with("owner", "01".repeat(31).into());
        assert!(matches!(KeyRequest::from_json(&short), Err(WireError::Field { field: "owner", .. })));
        let long_ct = with("ct", "03".repeat(CT_LEN + 1).into());
        assert!(matches!(KeyRequest::from_json(&long_ct), Err(WireError::Field { field: "ct", .. })));
        let upper = with("sig", "AB".repeat(64).into());
        assert!(matches!(KeyRequest::from_json(&upper), Err(WireError::Field { field: "sig", .. })));
        let not_hex = with("enc", "zz".repeat(32).into());
        assert!(matches!(KeyRequest::from_json(&not_hex), Err(WireError::Field { field: "enc", .. })));
        let number = with("enc", 5.into());
        assert!(matches!(KeyRequest::from_json(&number), Err(WireError::Json(_))));
        let extra = with("kind", "unlock".into());
        assert!(matches!(KeyRequest::from_json(&extra), Err(WireError::Json(_))));
        assert!(matches!(KeyRequest::from_json(b"{"), Err(WireError::Json(_))));
        let big = vec![b' '; MAX_REQUEST_BODY + 1];
        assert!(matches!(KeyRequest::from_json(&big), Err(WireError::TooLarge { .. })));
    }

    #[test]
    fn refuses_another_version_and_odd_quotes() {
        let mut v: serde_json::Value = serde_json::from_slice(&status().to_json().unwrap()).unwrap();
        v["version"] = 2.into();
        assert_eq!(Status::from_json(&serde_json::to_vec(&v).unwrap()), Err(WireError::Version(2)));
        v["version"] = 1.into();
        v["quote"] = "abc".into();
        assert!(Status::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
        assert!(nonce(&"00".repeat(31)).is_err());
        assert_eq!(nonce(&"0a".repeat(32)).unwrap(), [10; 32]);
    }
}
