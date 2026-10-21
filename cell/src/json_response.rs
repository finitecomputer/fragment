//! A JSON answer, made as worker 0.8.5's `Response::from_json` makes one:
//! the same body, `content-type: application/json`, status 200, and the
//! same error when serializing fails. Only the serializing is generic; the
//! rest is one function rather than a copy for each type the cell answers,
//! which keeps the cell's Wasm about 28 KB smaller (its budget:
//! `CELL_WASM_MAX_BYTES`, xtask/src/build.rs). Moving the worker pin means
//! checking `ResponseBuilder::from_json` against this again.

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
