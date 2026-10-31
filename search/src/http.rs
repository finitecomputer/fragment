//! The service's HTTP (README.md, The API): every path but `/healthz`
//! needs the deployment's token as a bearer token, compared in constant
//! time. A body is read to `api::BODY_MAX_BYTES` and no further.

use crate::api;
use crate::store::{Error, Store};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct Service {
    pub store: Arc<Store>,
    pub token: String,
}

type Reply = Response<Full<Bytes>>;
/// A refusal on its way out (boxed: a `Response` is large).
type Refusal = Box<Reply>;

fn reply(status: StatusCode, v: &Value) -> Reply {
    Response::builder().status(status).header("content-type", "application/json").body(Full::new(Bytes::from(v.to_string()))).expect("a response")
}

fn refused(status: StatusCode, error: &str, message: impl Into<String>) -> Reply {
    reply(status, &json!({ "error": error, "message": message.into() }))
}

fn failed(e: Error) -> Reply {
    match e {
        Error::Invalid(m) => refused(StatusCode::BAD_REQUEST, "invalid", m),
        Error::Conflict { current } => reply(StatusCode::CONFLICT, &json!({ "error": "conflict", "message": "the source's revision is not the one this replaces", "current": current })),
        Error::SpaceChanged { current } => reply(StatusCode::CONFLICT, &json!({ "error": "space_changed", "message": "the table's vectors are in another space: drop it and write it again", "current": current })),
        Error::MissingVectors(hashes) => reply(StatusCode::CONFLICT, &json!({ "error": "missing_vectors", "message": "chunks name no vector, and the table has none of their hashes", "hashes": hashes })),
        Error::Full => refused(StatusCode::INSUFFICIENT_STORAGE, "full", format!("a table holds at most {} chunks", api::TABLE_ROWS_MAX)),
        Error::Engine(m) => {
            eprintln!("{}", json!({ "event": "search.engine-error", "error": m }));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "engine", "the search engine failed")
        }
    }
}

/// Whether `presented` is the token, in time independent of where they differ.
fn same(presented: &[u8], token: &[u8]) -> bool {
    if presented.len() != token.len() {
        return false;
    }
    presented.iter().zip(token).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn body<T: serde::de::DeserializeOwned>(req: Request<Incoming>) -> Result<T, Refusal> {
    let bytes = match Limited::new(req.into_body(), api::BODY_MAX_BYTES).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return Err(Box::new(refused(StatusCode::PAYLOAD_TOO_LARGE, "too_large", format!("a body is at most {} bytes", api::BODY_MAX_BYTES)))),
    };
    serde_json::from_slice(&bytes).map_err(|e| Box::new(refused(StatusCode::BAD_REQUEST, "invalid", format!("body: {e}"))))
}

fn query(req: &Request<Incoming>, name: &str) -> Option<String> {
    let q = req.uri().query()?;
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| decode(v))
}

/// A query value, percent-decoded (`+` as a space; a `%` not followed by
/// two hex digits is itself).
fn decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(x)) => {
                out.push(x);
                i += 2;
            }
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Service {
    pub async fn handle(self: Arc<Self>, req: Request<Incoming>) -> Result<Reply, std::convert::Infallible> {
        Ok(self.route(req).await.unwrap_or_else(|r| *r))
    }

    async fn route(&self, req: Request<Incoming>) -> Result<Reply, Refusal> {
        let path = req.uri().path().to_string();
        if req.method() == Method::GET && path == "/healthz" {
            return Ok(reply(StatusCode::OK, &json!({ "ok": true })));
        }
        let presented = req.headers().get("authorization").and_then(|a| a.to_str().ok()).and_then(|a| a.strip_prefix("Bearer ")).unwrap_or("");
        if !same(presented.as_bytes(), self.token.as_bytes()) {
            return Err(Box::new(refused(StatusCode::UNAUTHORIZED, "unauthorized", "the deployment's token, as a bearer token")));
        }
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (scope, op) = match parts.as_slice() {
            ["v1", scope] => (scope.to_string(), ""),
            ["v1", scope, op] => (scope.to_string(), *op),
            _ => return Err(Box::new(refused(StatusCode::NOT_FOUND, "not_found", "no such path"))),
        };
        let op = op.to_string();
        if !api::valid_scope(&scope) {
            return Err(Box::new(refused(StatusCode::BAD_REQUEST, "invalid", "a scope is 1 to 128 of a-z, 0-9, - and _")));
        }
        let store = &self.store;
        let method = req.method().clone();
        let done = |r: Result<Value, Error>| r.map(|v| reply(StatusCode::OK, &v)).map_err(|e| Box::new(failed(e)));
        match (method, op.as_str()) {
            (Method::POST, "upsert") => {
                let up: api::Upsert = body(req).await?;
                done(store.upsert(&scope, up).await.map(|w| serde_json::to_value(w).expect("serializes")))
            }
            (Method::POST, "delete") => {
                let d: api::Delete = body(req).await?;
                done(store.delete(&scope, &d.source).await.map(|deleted| json!({ "source": d.source, "deleted": deleted })))
            }
            (Method::POST, "known") => {
                let k: api::Known = body(req).await?;
                done(store.known(&scope, k).await.map(|known| json!({ "known": known })))
            }
            (Method::GET, "sources") => {
                let after = query(&req, "after").unwrap_or_default();
                let limit = match query(&req, "limit") {
                    None => api::SOURCES_PAGE_MAX,
                    Some(l) => l.parse().map_err(|_| Box::new(refused(StatusCode::BAD_REQUEST, "invalid", "limit is a count")))?,
                };
                done(store.sources(&scope, &after, limit).await.map(|(sources, next)| json!({ "sources": sources, "next": next })))
            }
            (Method::GET, "status") => done(store.status(&scope).await),
            (Method::POST, "search") => {
                let s: api::Search = body(req).await?;
                done(store.search(&scope, s).await.map(|results| json!({ "results": results })))
            }
            (Method::DELETE, "") => done(store.drop_table(&scope).await.map(|dropped| json!({ "dropped": dropped }))),
            _ => Err(Box::new(refused(StatusCode::NOT_FOUND, "not_found", "no such operation"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(same(b"secret-token", b"secret-token"));
        assert!(!same(b"secret-tokeN", b"secret-token"));
        assert!(!same(b"secret", b"secret-token"));
        assert!(!same(b"", b"secret-token"));
    }

    #[test]
    fn queries_decode() {
        assert_eq!(decode("sessions%2Fa%20b.json"), "sessions/a b.json");
        assert_eq!(decode("a+b"), "a b");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
    }
}
