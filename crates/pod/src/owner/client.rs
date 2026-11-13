//! Talking to pod-init over plain HTTP. Nothing here is trusted for
//! being on the wire: `/status` is believed only once its binding checks
//! out, and the disk key travels sealed to the attested boot key.

use anyhow::{anyhow, bail, Context, Result};
use crate::unlock::binding::NONCE_LEN;
use crate::unlock::seal::Kind;
use crate::unlock::wire::{self, ErrorBody, KeyRequest, Opened, Status};
use std::io::Read;
use std::time::Duration;

/// A status is a quote (8 KB) in hex and a few fields.
const MAX_RESPONSE: u64 = 1024 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(60);
/// Formatting and mkfs at init, then the open hook (up to 300 s).
const KEY_TIMEOUT: Duration = Duration::from_secs(600);

/// The pod said no: its stable code and detail.
#[derive(Debug)]
pub struct Refused {
    pub route: &'static str,
    pub status: u16,
    pub body: ErrorBody,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the pod refused {} (HTTP {}): {}: {}", self.route, self.status, self.body.error, self.body.detail)
    }
}

impl std::error::Error for Refused {}

pub struct Pod {
    base: String,
    client: reqwest::blocking::Client,
}

fn read_body(resp: reqwest::blocking::Response) -> Result<(u16, Vec<u8>)> {
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    resp.take(MAX_RESPONSE + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_RESPONSE {
        bail!("response over {MAX_RESPONSE} bytes");
    }
    Ok((status, body))
}

fn refused(route: &'static str, status: u16, body: &[u8]) -> anyhow::Error {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(body) => anyhow!(Refused { route, status, body }),
        Err(_) => anyhow!("{route}: HTTP {status}: {}", String::from_utf8_lossy(body).trim()),
    }
}

impl Pod {
    /// `host` is host:port, as `--host` takes it.
    pub fn new(host: &str) -> Result<Self> {
        if host.contains('/') {
            bail!("--host {host:?} is host:port, not a URL");
        }
        let client = reqwest::blocking::Client::builder().user_agent("pod").build()?;
        Ok(Self { base: format!("http://{host}"), client })
    }

    fn get(&self, route: &'static str, query: &str) -> Result<Vec<u8>> {
        let url = format!("{}{route}{query}", self.base);
        let resp = self.client.get(&url).timeout(STATUS_TIMEOUT).send().with_context(|| format!("GET {url}"))?;
        let (status, body) = read_body(resp)?;
        if status != 200 {
            return Err(refused(route, status, &body));
        }
        Ok(body)
    }

    pub fn status(&self, nonce: &[u8; NONCE_LEN]) -> Result<Status> {
        let body = self.get("/status", &format!("?nonce={}", hex::encode(nonce)))?;
        Status::from_json(&body).context("/status")
    }

    pub fn tls_root(&self) -> Result<Vec<u8>> {
        self.get("/tls-root", "")
    }

    pub fn send(&self, kind: Kind, req: &KeyRequest) -> Result<Opened> {
        let route = match kind {
            Kind::Init => "/init",
            Kind::Unlock => "/unlock",
        };
        let url = format!("{}{route}", self.base);
        let resp = self
            .client
            .post(&url)
            .timeout(KEY_TIMEOUT)
            .header("content-type", "application/json")
            .body(req.to_json()?)
            .send()
            .with_context(|| format!("POST {url}"))?;
        let (status, body) = read_body(resp)?;
        if status != 200 {
            return Err(refused(route, status, &body));
        }
        serde_json::from_slice(&body).with_context(|| format!("{route} answered 200 with {:?}", String::from_utf8_lossy(&body)))
    }
}

/// So callers can tell a refusal (exit 1) from a broken connection (2).
pub fn is_refusal(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Refused>().is_some() || e.downcast_ref::<wire::WireError>().is_some()
}
