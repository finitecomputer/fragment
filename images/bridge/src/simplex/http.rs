//! An agent's SimpleX on the computer's screen port (6080; docs/bridge.md,
//! "SimpleX"), reached as every port is: through the platform, by its
//! owner alone (docs/computers.md, Ports).
//!
//! - `GET /simplex/?agent=<fragment>`: its link page (`page.html`, in the
//!   bridge, so every image serves the same one), which the shell opens in
//!   a pane through a port ticket;
//! - `GET /simplex/state?agent=…`: what its link is (`run::View`);
//! - `GET /simplex/qr.svg?agent=…`: its one-time invitation, while it is
//!   one, as a QR code (its short link when the server made one, which
//!   SimpleX's apps open, else the full one);
//! - `POST /simplex/link?agent=…`: its owner asks to link it (a one-time
//!   invitation, shown in the state while unused);
//! - `POST /simplex/unlink?agent=…`.
//!
//! An image that names no core answers its page, which says SimpleX is not
//! on this computer, and 404 to the rest; an agent the bridge does not run
//! is 404; a bridge not yet past its restore gate is 503.

use std::sync::{Arc, OnceLock};

use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::run::Handle;
use crate::net::{self, Body};

/// The screen's way to SimpleX: whether the image names a core, and the
/// manager once the bridge has started it (past the restore gate).
#[derive(Clone, Default)]
pub struct Simplex {
    pub configured: bool,
    pub handle: Arc<OnceLock<Handle>>,
}

/// Whether `path` is SimpleX's.
pub fn ours(path: &str) -> bool {
    path == "/simplex" || path.starts_with("/simplex/")
}

/// `?agent=` alone, an agent fragment's name.
fn agent_of(query: Option<&str>) -> Result<String, String> {
    let mut found = query.unwrap_or("").split('&').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| *k == "agent").map(|(_, v)| v);
    match (found.next(), found.next()) {
        (Some(a), None) if crate::screen::agent_name_ok(a) => Ok(a.to_string()),
        (Some(_), None) => Err("?agent= names an agent fragment (<label>--<suffix>)".into()),
        (None, _) => Err("?agent= names no one".into()),
        (Some(_), Some(_)) => Err("?agent= is named twice".into()),
    }
}

/// The link page.
const PAGE: &str = include_str!("page.html");

/// `text` as a QR code, an SVG of black modules on white with its quiet
/// zone, at least 240 px.
pub fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    Some(code.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build())
}

pub async fn handle(req: Request<Incoming>, simplex: &Simplex) -> Response<Body> {
    if req.method() == Method::GET && req.uri().path() == "/simplex/" {
        return net::respond(StatusCode::OK, "text/html; charset=utf-8", PAGE);
    }
    if !simplex.configured {
        return net::refusal(StatusCode::NOT_FOUND, "not_found", "SimpleX is not on this computer");
    }
    let Some(h) = simplex.handle.get() else {
        return net::refusal(StatusCode::SERVICE_UNAVAILABLE, "starting", "this computer is starting: try again in a moment");
    };
    let agent = match agent_of(req.uri().query()) {
        Ok(a) => a,
        Err(why) => return net::refusal(StatusCode::BAD_REQUEST, "invalid", &why),
    };
    if req.method() == Method::GET && req.uri().path() == "/simplex/qr.svg" {
        let link = h.view(&agent).await.and_then(|v| v.short.or(v.link));
        return match link.as_deref().and_then(qr_svg) {
            Some(svg) => net::respond(StatusCode::OK, "image/svg+xml", svg),
            None => net::refusal(StatusCode::NOT_FOUND, "not_found", "no invitation to show"),
        };
    }
    let view = match (req.method(), req.uri().path()) {
        (&Method::GET, "/simplex/state") => h.view(&agent).await,
        (&Method::POST, "/simplex/link") => h.want(&agent).await,
        (&Method::POST, "/simplex/unlink") => h.unlink(&agent).await,
        _ => return net::refusal(StatusCode::NOT_FOUND, "not_found", "GET /simplex/, /simplex/state or /simplex/qr.svg; POST /simplex/link or /simplex/unlink"),
    };
    match view {
        Some(v) => net::json_answer(StatusCode::OK, &serde_json::to_value(v).expect("a view serializes")),
        None => net::refusal(StatusCode::NOT_FOUND, "not_found", &format!("no agent {agent} runs on this computer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: only `/simplex…` is SimpleX's, and it names one agent, once.
    #[test]
    fn its_routes_name_one_agent() {
        assert!(ours("/simplex/state") && ours("/simplex") && !ours("/simplexx") && !ours("/"));
        assert_eq!(agent_of(Some("agent=juniper--k3x9")), Ok("juniper--k3x9".into()));
        assert!(agent_of(Some("agent=juniper--k3x9&agent=x--y")).is_err());
        assert!(agent_of(None).is_err());
        assert!(agent_of(Some("agent=../etc")).is_err());
    }

    /// Goal: an invitation is drawn as a QR code of what it says; the page
    /// reaches its routes by relative paths (it is served under a port's
    /// prefix) and promises only what is so.
    #[test]
    fn the_page_and_its_qr_code() {
        let link = "https://smp.example/i#Hhok7UlhSUBW_LdyHYk2ztmo4xoXFxK-/bAPg22IJtCe8PV_FpkmDTPiHguNsp44oKGKn9OJ06cU";
        let svg = qr_svg(link).expect("drawn");
        assert!(svg.starts_with("<?xml") && svg.contains("<svg") && svg.contains("<path"), "{}", &svg[..80.min(svg.len())]);
        let full = format!("simplex:/invitation#/?v=2-7&smp={}", "x".repeat(700));
        assert!(qr_svg(&full).is_some(), "a full invitation fits");
        assert!(qr_svg(&"x".repeat(5_000)).is_none(), "past what a QR code holds");
        for path in ["ask(\"GET\", \"state\")", "q(\"qr.svg\")", "act(\"link\")", "act(\"unlink\""] {
            assert!(PAGE.contains(path), "{path}");
        }
        assert!(!PAGE.contains("fetch(\"/"), "no absolute path: it runs under /p/6080/");
        assert!(PAGE.contains("encrypted between your SimpleX app and your computer"));
    }
}
