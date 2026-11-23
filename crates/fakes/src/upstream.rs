//! The internet a computer's swap sends to (`FRAGMENT_SWAP_UPSTREAM`): the
//! provider APIs the deployment's catalog names. Every request is answered
//! with what arrived (its host, from `x-fragment-upstream-host`, its
//! method, path, query and auth headers, and any platform header that
//! leaked), so a test reads what the provider would have seen, and each is
//! recorded; with `?e2e_status=<4xx>`, at that status, as a provider
//! refusing it. A vendor faked in full (`start_with`: OpenRouter's) answers
//! its own host's requests, each still recorded here. Each operator
//! vendor's answer says what the call used where its docs say it does
//! (`usage_of`, FIN-616), unless the request asks `?e2e_usage=none`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

/// What a fake Grok call costs, in xAI's ticks (10^10 a dollar).
pub const XAI_TICKS: u64 = 1_234_500;
/// The billing units a fake Klein run says it used.
pub const FAL_UNITS: u64 = 2;
/// The credits a fake Firecrawl scrape says it used, and a v2 search, and
/// a crawl's poll (the credits so far, which no charge reads).
pub const FIRECRAWL_SCRAPE_CREDITS: u64 = 1;
pub const FIRECRAWL_SEARCH_CREDITS: u64 = 2;
/// The posts a fake X search returns.
pub const X_POSTS: u64 = 7;

/// The headers a provider takes a credential in.
const AUTH_HEADERS: [&str; 5] = ["authorization", "x-api-key", "x-goog-api-key", "xi-api-key", "x-browser-use-api-key"];

pub struct Upstream {
    pub url: String,
    seen: Arc<Mutex<Vec<Value>>>,
    _server: Server,
}

fn seen_of(req: &Request) -> Value {
    let mut auth = serde_json::Map::new();
    for h in AUTH_HEADERS {
        if let Some(v) = req.header(h) {
            auth.insert(h.into(), json!(v));
        }
    }
    json!({
        "host": req.header("x-fragment-upstream-host"),
        "method": req.method,
        "path": req.path,
        "target": req.target,
        "query": req.pairs.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "auth": auth,
        "agent": req.header("x-fragment-agent"),
        "body": String::from_utf8_lossy(&req.body),
    })
}

/// An answer as its vendor's says what the call used: xAI's ticks on a
/// POST's JSON, ElevenLabs' characters (its JSON body's `text`) as a
/// header, fal's units as a header on a synchronous Klein run, Firecrawl's
/// credits in a scrape's and a v2 search's JSON (and a crawl poll's), and
/// X's post count on a search's. Any other answer says nothing.
fn usage_of(req: &Request, mut v: Value) -> Response {
    let host = req.header("x-fragment-upstream-host").unwrap_or_default();
    let post = req.method == "POST";
    match host {
        "api.x.ai" if post => v["usage"] = json!({ "cost_in_usd_ticks": XAI_TICKS }),
        "api.elevenlabs.io" if post => {
            let text = serde_json::from_slice::<Value>(&req.body).ok().and_then(|b| b["text"].as_str().map(|t| t.chars().count()));
            if let Some(chars) = text {
                return Response::json(200, &v).with_header("character-cost", &chars.to_string());
            }
        }
        "fal.run" if post && req.path == "/fal-ai/flux-2/klein/9b" => return Response::json(200, &v).with_header("x-fal-billable-units", &FAL_UNITS.to_string()),
        "api.firecrawl.dev" if post && req.path.ends_with("/scrape") => v["data"] = json!({ "metadata": { "creditsUsed": FIRECRAWL_SCRAPE_CREDITS } }),
        "api.firecrawl.dev" if post && req.path == "/v2/search" => v["creditsUsed"] = json!(FIRECRAWL_SEARCH_CREDITS),
        "api.firecrawl.dev" if req.path.contains("/crawl/") => v["creditsUsed"] = json!(99),
        "api.x.com" if req.path.starts_with("/2/tweets/search/") => v["meta"] = json!({ "result_count": X_POSTS }),
        _ => {}
    }
    Response::json(200, &v)
}

impl Upstream {
    pub fn start() -> std::io::Result<Upstream> {
        Upstream::start_with(vec![])
    }

    /// With `vendors`: each a host and the fake that answers for it.
    pub fn start_with(vendors: Vec<(String, Handler)>) -> std::io::Result<Upstream> {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let log = Arc::clone(&seen);
        let handler: Handler = Arc::new(move |req: &Request| {
            let v = seen_of(req);
            log.lock().expect("upstream log").push(v.clone());
            if let Some((_, vendor)) = vendors.iter().find(|(host, _)| req.header("x-fragment-upstream-host") == Some(host.as_str())) {
                return vendor(req);
            }
            // what arrived echoed as an event stream, and among bytes that are
            // no text: the swap names a Vault's value in either (FIN-466)
            if req.path == "/echo-stream" {
                return Response::bytes(200, "text/event-stream", format!("data: {v}\n\n").into_bytes());
            }
            if req.path == "/echo-binary" {
                return Response::bytes(200, "application/octet-stream", [&[0xff, 0xfe][..], v.to_string().as_bytes(), &[0x00]].concat());
            }
            // a redirect, for the test that the swap never follows one
            if req.path == "/redirect" {
                return Response::bytes(302, "text/plain", b"elsewhere".to_vec()).with_header("location", "https://elsewhere.test/");
            }
            // a provider refusing the call (`?e2e_status=429`), for the
            // tests of what a refused call is charged
            if let Some(code) = req.query.get("e2e_status").and_then(|c| c.parse::<u16>().ok()).filter(|c| (400..500).contains(c)) {
                return Response::json(code, &v);
            }
            match req.query.get("e2e_usage").map(String::as_str) {
                Some("none") => Response::json(200, &v),
                _ => usage_of(req, v),
            }
        });
        let server = Server::start(0, handler)?;
        Ok(Upstream { url: server.url.clone(), seen, _server: server })
    }

    /// Every request that arrived, oldest first.
    pub fn seen(&self) -> Vec<Value> {
        self.seen.lock().expect("upstream log").clone()
    }
}
