//! Host parity for the cell's actual JSON helpers against worker 0.8.5's
//! from_json algorithm. Headers and response storage are boundary doubles:
//! these tests do not claim JavaScript, workerd or Cloudflare coverage.

extern crate self as worker;

use std::cell::RefCell;
use std::collections::BTreeMap;
use serde::{Serialize, Serializer};
use serde_json::{json, Value};

// Compile the production helpers and CellError conversion, not copies.
#[path = "../../cell/src/error.rs"]
pub mod error;
mod candidate {
    include!("../../cell/src/json_response.rs");

    pub fn with_builder<T: serde::Serialize>(builder: ResponseBuilder, value: &T) -> CellResult<Response> {
        json_encoded(builder, serde_json::to_string(value))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Json((String, u16)),
    Header,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json((message, status)) => write!(f, "{message} (status: {status})"),
            Self::Header => f.write_str("synthetic header refusal"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headers {
    entries: RefCell<BTreeMap<String, String>>,
    reject: bool,
}

impl Headers {
    pub fn new() -> Self { Self::default() }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        if self.reject { return Err(Error::Header); }
        self.entries.borrow_mut().insert(key.to_ascii_lowercase(), value.into());
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseBuilder {
    headers: Headers,
    status: u16,
}

impl ResponseBuilder {
    pub fn with_header(self, key: &str, value: &str) -> Result<Self> {
        self.headers.set(key, value)?;
        Ok(self)
    }

    pub fn fixed(self, body: Vec<u8>) -> Response { Response { builder: self, body } }

    // Golden algorithm from worker 0.8.5 src/response.rs:461-467. Keep it
    // independent of the candidate; the version guard requires SDK upgrades
    // to revisit this reference. The real SDK was inspected when adding it.
    fn sdk_json<B: Serialize>(self, value: &B) -> Result<Response> {
        if let Ok(data) = serde_json::to_string(value) {
            self.headers.set("content-type", "application/json")?;
            Ok(self.fixed(data.into_bytes()))
        } else {
            Err(Error::Json(("Failed to encode data to json".into(), 500)))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    builder: ResponseBuilder,
    body: Vec<u8>,
}

impl Response {
    pub fn builder() -> ResponseBuilder { ResponseBuilder { headers: Headers::new(), status: 200 } }
    pub fn from_json<T: Serialize>(value: &T) -> Result<Self> { Self::builder().sdk_json(value) }
    pub fn with_headers(mut self, headers: Headers) -> Self { self.builder.headers = headers; self }
    pub fn with_status(mut self, status: u16) -> Self { self.builder.status = status; self }
}

fn comparable(result: error::CellResult<Response>) -> std::result::Result<Response, (fragment_proto::ErrorCode, String)> {
    result.map_err(|e| (e.code, e.message))
}

fn parity<T: Serialize>(value: &T) {
    let legacy = Response::from_json(value).map_err(error::CellError::from);
    assert_eq!(comparable(candidate::json_answer(value)), comparable(legacy));
}

#[test]
fn json_response_reference_is_pinned_to_worker_0_8_5() {
    assert!(include_str!("../../cell/Cargo.toml").lines().any(|line| line.starts_with("worker = { version = \"=0.8.5\"")),
        "review the JSON response reference against the new Worker SDK before updating its pin");
}

#[test]
fn json_response_bodies_match_for_scalars_structs_and_nested_values() {
    #[derive(Serialize)]
    struct Answer<'a> { count: u32, label: &'a str, absent: Option<u8>, items: Vec<Value> }
    for count in 0..128 {
        let value = Answer { count, label: "synthetic \"quoted\"\n\\text café 雪", absent: None,
            items: vec![Value::Null, json!(count % 2 == 0), json!({"nested": [count, "x"]})] };
        parity(&value);
        parity(&Some(&value));
    }
    for value in [Value::Null, json!(true), json!(false), json!(i64::MIN), json!(u64::MAX),
        json!([]), json!({}), json!("\u{0000}\t\r\n\"\\")] {
        parity(&value);
    }
    parity(&None::<String>);
    parity(&f64::NAN);
    parity(&f64::INFINITY);
    let answer = candidate::json_answer(&json!({"ok": true})).unwrap();
    assert_eq!(answer.builder.status, 200);
    assert_eq!(answer.body, br#"{"ok":true}"#);
}

struct Refuses;
impl Serialize for Refuses {
    fn serialize<S: Serializer>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("SYNTHETIC_SERIALIZER_CANARY"))
    }
}

#[test]
fn json_response_serialization_errors_preserve_sdk_error_and_hide_details() {
    parity(&Refuses);
    parity(&BTreeMap::from([(vec![1_u8, 2], "invalid JSON object key")]));
    let error = candidate::json_answer(&Refuses).unwrap_err();
    assert_eq!(error.code, fragment_proto::ErrorCode::HostFailed);
    assert_eq!(error.message, "Failed to encode data to json (status: 500)");
    assert!(!error.message.contains("SYNTHETIC_SERIALIZER_CANARY"));
    let response = error.response().unwrap();
    assert_eq!(response.builder.status, 500);
    assert_eq!(serde_json::from_slice::<Value>(&response.body).unwrap()["error"], "host_failed");
}

#[test]
fn json_response_preserves_builder_status_headers_and_cache_policy() {
    let builder = ResponseBuilder { status: 202, headers: Headers::new() };
    builder.headers.set("content-type", "text/plain").unwrap();
    builder.headers.set("cache-control", "private").unwrap();
    builder.headers.set("x-test", "synthetic").unwrap();
    let value = json!([1, "two", null]);
    let actual = candidate::with_builder(builder.clone(), &value).unwrap();
    assert_eq!(actual, builder.sdk_json(&value).unwrap());
    assert_eq!(actual.builder.status, 202);
    assert_eq!(actual.builder.headers.entries.borrow()["content-type"], "application/json");
    assert_eq!(actual.builder.headers.entries.borrow()["cache-control"], "private");
    let no_store = candidate::json_response(&value).unwrap();
    assert_eq!(no_store.builder.status, 200);
    assert_eq!(no_store.body, actual.body);
    assert_eq!(*no_store.builder.headers.entries.borrow(), BTreeMap::from([
        ("content-type".into(), "application/json".into()), ("cache-control".into(), "no-store".into()),
    ]));
}

#[test]
fn json_response_header_failure_and_error_precedence_match_sdk() {
    let builder = ResponseBuilder { status: 200, headers: Headers { reject: true, ..Headers::new() } };
    let value = json!({"synthetic": true});
    assert_eq!(comparable(candidate::with_builder(builder.clone(), &value)),
        comparable(builder.clone().sdk_json(&value).map_err(error::CellError::from)));
    assert_eq!(comparable(candidate::with_builder(builder.clone(), &Refuses)),
        comparable(builder.sdk_json(&Refuses).map_err(error::CellError::from)));
}
