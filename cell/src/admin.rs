//! The operators' admin Worker (docs/api.md, Operators; decision 59): the
//! first Worker split from the cell by who uses it (FIN-475). Only the
//! deployment's operators call it, and nothing in the system reads from
//! it, so it costs no fragment's isolate anything: this crate built with
//! the `admin` feature (lib.rs, "Two Workers, one crate"), deployed as its
//! own Worker from the same run and release (cell/wrangler.admin.jsonc;
//! xtask/src/deploy.rs). It reaches the cell's objects through bindings to
//! the cell's script, stamped with its release as any caller's are
//! (`routed::send`), so in a deploy's seconds an object of the other
//! release refuses it (`deploying`), as it would any caller.
//!
//! The cell hands it the routes it answers (`forward`), as they came:
//! the body streamed, unread, and who asks unchecked, since the admin
//! checks that itself, as every route of its own did in the cell.

use fragment_proto::{limits, ErrorCode};
use worker::{Env, Method, Request, Response};

use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::{ask_registry, json_answer, read_body};

/// The cell's binding to the admin Worker (cell/wrangler.jsonc `services`).
const BINDING: &str = "ADMIN";

/// The cell's side: a request of the admin's (lib.rs, `platform_answer`),
/// handed to its Worker untouched. Its answer comes back to the cell's
/// router, which marks it as every answer of the platform's host
/// (`platform_origin`).
pub(crate) async fn forward(req: Request, env: &Env) -> CellResult<Response> {
    let Ok(worker) = env.service(BINDING) else {
        return Err(CellError::host(format!("this deployment binds no admin Worker ({BINDING}): the operators' routes are its")));
    };
    Ok(worker.fetch_request(req).await?)
}

/// The admin Worker's routes (its `fetch`, lib.rs): what the cell hands it
/// and nothing more, so anything else is the 404 a missing route is.
/// `GET /healthz/admin` is a deploy's probe of it (docs/api.md, Health):
/// what each object class answers its release (`routed::objects`, the
/// cell's probe asked from here), so a deploy learns it reaches each of
/// the cell's classes at its own release. Every other route is an
/// operator's, checked before the route is read, as in the cell
/// (`orgs::operator`).
pub async fn route(mut req: Request, env: &Env) -> CellResult<Response> {
    let cfg = crate::config::Config::from_env(env);
    let url = fragment_nip98::arrived_url(req.url()?, req.headers().get("x-forwarded-proto")?.as_deref());
    let path = url.path().to_string();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if let (Method::Get, ["healthz", "admin"]) = (req.method(), segments.as_slice()) {
        return crate::routed::objects(env, cfg).await;
    }
    let ["api", "admin", rest @ ..] = segments.as_slice() else { return Err(crate::no_route(&path)) };
    let bytes = read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    crate::orgs::operator(env, cfg, &req, &url, &bytes).await?;
    match (req.method(), rest) {
        (Method::Get, ["health"]) => json_answer(&ask_registry(env, &admin::Health {}).await?),
        (Method::Get, ["glance"]) => json_answer(&crate::glance::glance(env, cfg, &url).await?),
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/admin/{}", m.as_ref(), rest.join("/")))),
    }
}
