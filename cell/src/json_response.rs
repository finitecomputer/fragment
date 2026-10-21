// JSON responses share their construction after serialization to keep the
// cell's Wasm within its budget. Match the pinned worker 0.8.5 SDK's body,
// headers and error; host parity tests supply doubles only at its boundary.

use crate::error::CellResult;
use worker::{Headers, Response, ResponseBuilder};

pub(crate) fn json_answer<T: serde::Serialize>(value: &T) -> CellResult<Response> {
    json_encoded(Response::builder(), serde_json::to_string(value))
}

#[inline(never)]
fn json_encoded(builder: ResponseBuilder, encoded: serde_json::Result<String>) -> CellResult<Response> {
    match encoded {
        Ok(data) => Ok(builder.with_header("content-type", "application/json")?.fixed(data.into_bytes())),
        Err(_) => Err(worker::Error::Json(("Failed to encode data to json".into(), 500)).into()),
    }
}

pub(crate) fn json_response<T: serde::Serialize>(value: &T) -> CellResult<Response> {
    Ok(json_answer(value)?.with_headers(no_store()))
}

fn no_store() -> Headers {
    let headers = Headers::new();
    headers.set("content-type", "application/json").expect("static header");
    headers.set("cache-control", "no-store").expect("static header");
    headers
}
