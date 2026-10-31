//! Fragment's search service (README.md): a fragment's sections, and their
//! chunks' vectors, in LanceDB, on object storage (R2 on Cloudflare, S3
//! self-hosted) or a disk. The platform calls it; it knows no fragment,
//! person or model: a scope is a table's name, a space its vectors'.
//!
//! Configured by its environment:
//! - `SEARCH_STORAGE`: a directory, or `s3://<bucket>/<prefix>`;
//! - `SEARCH_STORAGE_<OPTION>`: each of the store's options, its name
//!   lowercased (`SEARCH_STORAGE_AWS_ENDPOINT` is `aws_endpoint`);
//! - `SEARCH_TOKEN`: the bearer token every call carries, at least 32 bytes;
//! - any of these with `_FILE` after its name: read from that file instead
//!   (a secret mounted in), never from a command line;
//! - `SEARCH_LISTEN`: the address to listen on, `0.0.0.0:8080` unnamed.

use fragment_search::{env, http, store};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::sync::Arc;

const TOKEN_MIN_BYTES: usize = 32;

fn need(name: &str) -> String {
    match env::value(name) {
        Ok(Some(v)) => v,
        Ok(None) => fail(&format!("{name} (or {name}_FILE) is required (README.md)")),
        Err(e) => fail(&e),
    }
}

fn fail(why: &str) -> ! {
    eprintln!("fragment-search: {why}");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let storage = need("SEARCH_STORAGE");
    let token = need("SEARCH_TOKEN");
    if token.len() < TOKEN_MIN_BYTES {
        eprintln!("fragment-search: SEARCH_TOKEN is at least {TOKEN_MIN_BYTES} bytes");
        std::process::exit(2);
    }
    let listen = std::env::var("SEARCH_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let options = env::options("SEARCH_STORAGE_").unwrap_or_else(|e| fail(&e));
    let store = match store::Store::open(&storage, options).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("fragment-search: {storage}: {e:?}");
            std::process::exit(1);
        }
    };
    let service = Arc::new(http::Service { store, token });
    let listener = tokio::net::TcpListener::bind(&listen).await.unwrap_or_else(|e| {
        eprintln!("fragment-search: {listen}: {e}");
        std::process::exit(1);
    });
    eprintln!("{}", serde_json::json!({ "event": "search.listening", "address": listen }));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("a SIGTERM handler");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let service = service.clone();
                tokio::spawn(async move {
                    let serve = service_fn(move |req| service.clone().handle(req));
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), serve).await;
                });
            }
            _ = term.recv() => break,
        }
    }
}
