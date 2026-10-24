//! Cloudflare's API for one account, as a deploy asks it beside wrangler:
//! with wrangler's own token (`CLOUDFLARE_API_TOKEN`), never printed.
//! Image preparations (prepare.rs).

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

const API: &str = "https://api.cloudflare.com/client/v4";

pub(crate) struct Api {
    http: reqwest::blocking::Client,
    pub(crate) account: String,
    token: String,
}

impl Api {
    /// The API for `account`, with wrangler's token (`CLOUDFLARE_API_TOKEN`,
    /// as a deploy's wrangler and the Image workflow's are given it).
    pub(crate) fn from_env(account: &str) -> Result<Api> {
        let token = std::env::var("CLOUDFLARE_API_TOKEN").ok().filter(|t| !t.trim().is_empty()).context(
            "CLOUDFLARE_API_TOKEN is not set: Cloudflare's API is asked with wrangler's token (Containers on the account) beside wrangler",
        )?;
        let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;
        Ok(Api { http, account: account.to_string(), token: token.trim().to_string() })
    }

    /// One call: `path` under the account (`/workers/…`, `/containers/…`),
    /// its status and JSON (`Null` when it sent none), or why it got no
    /// answer at all.
    pub(crate) fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        assert!(path.starts_with('/'), "an account's path, not {path:?}");
        let url = format!("{API}/accounts/{}{path}", self.account);
        let mut req = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().map_err(|e| format!("Cloudflare's API did not answer: {e}"))?;
        let status = resp.status().as_u16();
        Ok((status, resp.json().unwrap_or(Value::Null)))
    }
}

/// An answer's errors, as people read them: `<status>: <message> (<code>); …`.
pub(crate) fn errors(status: u16, body: &Value) -> String {
    let listed: Vec<String> = body["errors"].as_array().into_iter().flatten().map(|e| format!("{} ({})", e["message"].as_str().unwrap_or("?"), e["code"])).collect();
    if listed.is_empty() { format!("{status}") } else { format!("{status}: {}", listed.join("; ")) }
}
