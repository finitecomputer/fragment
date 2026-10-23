//! An agent's SimpleX on the computer's screen port (6080; docs/bridge.md,
//! "SimpleX"), reached as every port is: through the platform, by its
//! owner alone (docs/computers.md, Ports).
//!
//! - `GET /simplex/state?agent=<fragment>`: what its link is (`run::View`);
//! - `POST /simplex/link?agent=…`: its owner asks to link it (a one-time
//!   invitation, shown in the state while unused);
//! - `POST /simplex/unlink?agent=…`.
//!
//! An image that names no core answers 404, saying SimpleX is not on this
//! computer; an agent the bridge does not run is 404; a bridge not yet
//! past its restore gate is 503.

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

pub async fn handle(req: Request<Incoming>, simplex: &Simplex) -> Response<Body> {
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
    let view = match (req.method(), req.uri().path()) {
        (&Method::GET, "/simplex/state") => h.view(&agent).await,
        (&Method::POST, "/simplex/link") => h.want(&agent).await,
        (&Method::POST, "/simplex/unlink") => h.unlink(&agent).await,
        _ => return net::refusal(StatusCode::NOT_FOUND, "not_found", "GET /simplex/state, POST /simplex/link or POST /simplex/unlink"),
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
}
