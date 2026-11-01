//! An agent's SimpleX on the computer's screen port (6080; docs/bridge.md,
//! "SimpleX"), reached as every port is: through the platform, by its
//! owner alone (docs/computers.md, Ports).
//!
//! - `GET /simplex/?agent=<fragment>`: its link page (`page.html`, in the
//!   bridge, so every image serves the same one), which the shell opens in
//!   a pane through a port ticket;
//! - `GET /simplex/state?agent=…[&s=<session>]`: what its link is
//!   (`run::View`), its invitation only to the page that asked for it;
//! - `GET /simplex/qr.svg?agent=…&s=<session>`: that invitation, while it
//!   is one, as a QR code (its short link when the server made one, which
//!   SimpleX's apps open, else the full one), to that page alone;
//! - `POST /simplex/link?agent=…` with a link's ticket (the body): its
//!   owner links it (a one-time invitation, shown to the page whose
//!   ticket's nonce is its session);
//! - `POST /simplex/unlink?agent=…` with an unlink's ticket.
//!
//! The tickets come from the owner's Settings alone, signed by the
//! platform (`door`): the agent's tools, which reach this port too, can
//! neither link it nor see an invitation. An image that names no core
//! answers its page, which says SimpleX is not on this computer, and 404
//! to the rest; an agent the bridge does not run is 404; a bridge not yet
//! past its restore gate is 503; a link or an unlink without a ticket, or
//! with one that is not good, 403 (409 a link while linked).

use std::sync::{Arc, OnceLock};

use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::door::Ticket;
use super::run::{Handle, Refused};
use crate::net::{self, Body};

/// A ticket's body is at most this.
const TICKET_MAX_BYTES: usize = 2_048;

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

/// `?s=`: the page's session (its link ticket's nonce), when it names one.
fn session_of(query: Option<&str>) -> Option<String> {
    query.unwrap_or("").split('&').filter_map(|kv| kv.split_once('=')).find(|(k, v)| *k == "s" && v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit())).map(|(_, v)| v.to_string())
}

/// `?agent=` alone, an agent fragment's name.
pub(crate) fn agent_of(query: Option<&str>) -> Result<String, String> {
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
    let session = session_of(req.uri().query());
    if req.method() == Method::GET && req.uri().path() == "/simplex/qr.svg" {
        let link = h.view(&agent, session.as_deref()).await.and_then(|v| v.short.or(v.link));
        return match link.as_deref().and_then(qr_svg) {
            Some(svg) => net::respond(StatusCode::OK, "image/svg+xml", svg),
            None => net::refusal(StatusCode::NOT_FOUND, "not_found", "no invitation to show"),
        };
    }
    let path = req.uri().path().to_string();
    let answer = match (req.method().clone(), path.as_str()) {
        (Method::GET, "/simplex/state") => h.view(&agent, session.as_deref()).await.ok_or_else(|| Refused { status: 404, why: format!("no agent {agent} runs on this computer") }),
        (Method::POST, "/simplex/link" | "/simplex/unlink") => match ticket_of(req).await {
            Err(why) => Err(Refused { status: 403, why }),
            Ok(t) if path == "/simplex/link" => h.link(&agent, t).await,
            Ok(t) => h.unlink(&agent, t).await,
        },
        _ => return net::refusal(StatusCode::NOT_FOUND, "not_found", "GET /simplex/, /simplex/state or /simplex/qr.svg; POST /simplex/link or /simplex/unlink"),
    };
    match answer {
        Ok(v) => net::json_answer(StatusCode::OK, &serde_json::to_value(v).expect("a view serializes")),
        Err(r) => {
            let status = StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let code = match r.status {
                403 => "forbidden",
                404 => "not_found",
                409 => "conflict",
                503 => "starting",
                _ => "failed",
            };
            net::refusal(status, code, &r.why)
        }
    }
}

/// A link's or an unlink's ticket: the request's body, as the owner's
/// Settings handed it to the page.
async fn ticket_of(req: Request<Incoming>) -> Result<Ticket, String> {
    let none = || "a link or an unlink starts from your Settings, which hand this page a ticket".to_string();
    let bytes = Limited::new(req.into_body(), TICKET_MAX_BYTES).collect().await.map_err(|_| none())?.to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| none())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: only `/simplex…` is SimpleX's, and it names one agent, once.
    #[test]
    fn its_routes_name_one_agent() {
        assert!(ours("/simplex/state") && ours("/simplex") && !ours("/simplexx") && !ours("/"));
        assert_eq!(agent_of(Some("agent=juniper--k3x9")), Ok("juniper--k3x9".into()));
        assert_eq!(session_of(Some(&format!("agent=juniper--k3x9&s={}", "0f".repeat(16)))), Some("0f".repeat(16)));
        assert_eq!(session_of(Some("agent=juniper--k3x9&s=short")), None);
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
        for path in ["ask(\"GET\", \"state\")", "q(\"qr.svg\")", "act(\"link\"", "act(\"unlink\"", "window.parent.postMessage({ fragment: \"simplex\", agent, want, ...extra }, parentOrigin)"] {
            assert!(PAGE.contains(path), "{path}");
        }
        // Paul, 2026-10-26: as a link starts it asks once whether the agent
        // reaches its person here (its home), yes by default, and says so in
        // the link's ask; a link tried again or an unlink says nothing of it
        assert!(PAGE.contains("let home = true;") && PAGE.contains("act(\"link\", null, { home })") && PAGE.contains("box.checked = home;"));
        assert_eq!(PAGE.matches("{ home }").count(), 1, "only the first link's ask says it");
        // Alex's review of #620: the shell's answer kept whole, and a choice
        // it could not save said in one line, the link going ahead
        assert!(PAGE.contains("if (m.ticket) resolve(m);") && PAGE.contains("const ticket = answer.ticket;"), "the answer kept whole");
        assert!(PAGE.contains("homeNote = answer.homeError ?") && PAGE.contains("[view.why, homeNote].filter(Boolean)"), "its home not saved, said");
        let note_then_link = PAGE.find("homeNote = answer.homeError").zip(PAGE.find("view = await ask(\"POST\", want, ticket)"));
        assert!(note_then_link.is_some_and(|(note, link)| note < link), "and the link goes ahead after it");
        assert!(!PAGE.contains("fetch(\"/"), "no absolute path: it runs under /p/6080/");
        assert!(PAGE.contains("encrypted between your SimpleX app and your computer"));
        // Alex's review: a stuck link has its way on, and a poll begun before a change shows nothing
        assert!(PAGE.contains("button(\"Try again\", \"primary\", () => act(\"link\"))"));
        assert!(PAGE.contains("if (at !== epoch || busy) return;"));
    }
}
