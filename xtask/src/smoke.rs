//! Read-only health and deployment identity check. No config, sign-in or vendors.
//! The wire contract is cell/src/lib.rs: HEAD /healthz, x-fragment-deploy.

use std::time::Duration;

use anyhow::{bail, Result};
use fragment_core::release;
use reqwest::{blocking::Client, header::HeaderMap, redirect::Policy, Url};
use serde::Serialize;

const USAGE: &str = "cargo xtask smoke --url <platform-origin> [--expected-release <id>] [--timeout-ms <1..60000>] [--json]
Sends HEAD /healthz once, without credentials or redirects. The URL is required;
use an http(s) origin or its /healthz URL, without userinfo, query or fragment.
Timeout defaults to 10000 ms (connect timeout at most 3000 ms).
Exit 0: HTTP 200 and a valid x-fragment-deploy, matching the optional exact id.
Exit 1: invalid arguments or a failed check. --json emits one JSON object.
Passing checks health and deployment identity only, not a product journey.";

struct Options {
    url: Url,
    expected_release: Option<String>,
    timeout: Duration,
}

impl Options {
    // Never include raw arguments or URL parser errors in diagnostics: a pasted
    // URL can contain credentials. Only validated release ids reach the report.
    fn parse(args: &[String]) -> Result<Self, &'static str> {
        let (mut url, mut expected_release, mut timeout, mut json) = (None, None, None, false);
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--url" if url.is_none() => url = Some(args.next().ok_or("missing_url")?),
                "--expected-release" if expected_release.is_none() => expected_release = Some(args.next().ok_or("invalid_expected_release")?),
                "--timeout-ms" if timeout.is_none() => timeout = Some(args.next().ok_or("invalid_timeout")?),
                "--json" if !json => json = true,
                _ => return Err("invalid_arguments"),
            }
        }
        let raw = url.ok_or("missing_url")?;
        let mut url = Url::parse(raw).map_err(|_| "invalid_url")?;
        if raw.chars().any(char::is_whitespace)
            || raw.chars().any(char::is_control)
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "/" | "/healthz")
        {
            return Err("invalid_url");
        }
        url.set_path("/healthz");
        if expected_release.is_some_and(|id| !release::valid(id)) {
            return Err("invalid_expected_release");
        }
        let timeout = match timeout {
            Some(value) => value.parse::<u64>().ok().filter(|ms| (1..=60_000).contains(ms)).ok_or("invalid_timeout")?,
            None => 10_000,
        };
        Ok(Self { url, expected_release: expected_release.cloned(), timeout: Duration::from_millis(timeout) })
    }
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u8,
    scope: &'static str,
    passed: bool,
    code: &'static str,
    http_status: Option<u16>,
    release: Option<String>,
    expected_release: Option<String>,
}

impl Report {
    fn new(expected_release: Option<String>) -> Self {
        Self { schema_version: 1, scope: "health_and_deployment_identity_only", passed: false, code: "request_failed", http_status: None, release: None, expected_release }
    }

    fn response(mut self, status: u16, headers: &HeaderMap) -> Self {
        self.http_status = Some(status);
        self.code = if status != 200 {
            "http_error"
        } else {
            let mut values = headers.get_all("x-fragment-deploy").iter();
            match values.next() {
                None => "missing_release",
                Some(value) => match value.to_str().ok().filter(|id| release::valid(id) && values.next().is_none()) {
                    None => "invalid_release",
                    Some(id) => {
                        self.release = Some(id.into());
                        if self.expected_release.as_deref().is_some_and(|expected| expected != id) {
                            "release_mismatch"
                        } else {
                            self.passed = true;
                            "passed"
                        }
                    }
                },
            }
        };
        self
    }

    fn text(&self) -> String {
        format!(
            "{}: {} (HTTP {}, release {}, expected {}). Health and deployment identity only; product journeys are untested.",
            if self.passed { "PASS" } else { "FAIL" },
            self.code,
            self.http_status.map_or_else(|| "unavailable".into(), |s| s.to_string()),
            self.release.as_deref().unwrap_or("unavailable"),
            self.expected_release.as_deref().unwrap_or("not specified"),
        )
    }
}

fn check(options: Options) -> Report {
    let mut report = Report::new(options.expected_release);
    // Keep reqwest's TLS verification and managed proxy settings. Do not follow
    // a sign-in/canonical-host redirect and accidentally report another target.
    let client = match Client::builder()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .timeout(options.timeout)
        .connect_timeout(options.timeout.min(Duration::from_secs(3)))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            report.code = "client_failed";
            return report;
        }
    };
    match client.head(options.url).header("cache-control", "no-cache").send() {
        Ok(response) => report.response(response.status().as_u16(), response.headers()),
        Err(error) => {
            // reqwest errors can include URLs and server details. Classify only;
            // never print them, response bodies, or unrelated response headers.
            report.code = if error.is_timeout() { "request_timeout" } else { "request_failed" };
            report
        }
    }
}

pub fn run(args: &[String]) -> Result<()> {
    if args == ["--help"] {
        println!("{USAGE}");
        return Ok(());
    }
    let report = match Options::parse(args) {
        Ok(options) => check(options),
        Err(code) => Report { code, ..Report::new(None) },
    };
    if args.iter().any(|arg| arg == "--json") {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!("{}", report.text());
    }
    if !report.passed {
        bail!("smoke check failed; see the report (usage: cargo xtask smoke --help)");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
