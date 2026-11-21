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

use fragment_core::mail::PRODUCT;
use fragment_proto::org::{CompSeat, Comped, NamePayer, NewContentAdmin, SeatKind, SetSeatKind};
use fragment_proto::{limits, ErrorCode};
use worker::{console_error, Env, Method, Request, Response};

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::orgs::{before, body, param};
use crate::registry::admin;
use crate::registry::content::{ContentOf, NominatePayer, OfferAdmin};
use crate::registry::orgs::{CompKind, CompSeatCall, EndComp, OrgOf};
use crate::registry::trials::{TrialChange, TrialGet, TrialList, TrialNew};
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
/// the cell's classes at its own release. `GET /admin` is the operators'
/// page (shell.rs `admin_page`). Every `/api/admin/…` route is an
/// operator's, checked before the route is read (`orgs::operator`, which
/// the cell's own operators' routes, a wipe's and a ledger command's,
/// hold callers to as well): comps, orgs, people, trial codes and their
/// mail, billing's health, the glance, the move's staff acts, a wake, and
/// the log of what operators did (docs/api.md, Operators).
pub async fn route(mut req: Request, env: &Env) -> CellResult<Response> {
    let cfg = Config::from_env(env);
    let url = fragment_nip98::arrived_url(req.url()?, req.headers().get("x-forwarded-proto")?.as_deref());
    let path = url.path().to_string();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (req.method(), segments.as_slice()) {
        (Method::Get, ["healthz", "admin"]) => return crate::routed::objects(env, cfg).await,
        (Method::Get, ["admin"]) => return crate::shell::admin_page(&req, env, cfg, &url).await,
        _ => {}
    }
    let ["api", "admin", rest @ ..] = segments.as_slice() else { return Err(crate::no_route(&path)) };
    let bytes = read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let by = crate::orgs::operator(env, cfg, &req, &url, &bytes).await?;
    let url = &url;
    match (req.method(), rest) {
        (Method::Get, ["health"]) => json_answer(&ask_registry(env, &admin::Health {}).await?),
        (Method::Get, ["glance"]) => json_answer(&crate::glance::glance(env, cfg, url).await?),
        (Method::Post, ["seats"]) => {
            let comp: CompSeat = body(&bytes)?;
            let mut comped = ask_registry(env, &CompSeatCall { by, comp }).await?;
            if comped.created && cfg.mail_from.is_some() {
                comped.mailed = mail_comp(env, cfg, &comped).await;
            }
            json_answer(&comped)
        }
        (Method::Patch, ["seats", seat]) => {
            let set: SetSeatKind = body(&bytes)?;
            json_answer(&ask_registry(env, &CompKind { by, seat: seat.to_string(), kind: set.kind }).await?)
        }
        (Method::Delete, ["seats", seat]) => json_answer(&ask_registry(env, &EndComp { by, seat: seat.to_string() }).await?),
        (Method::Get, ["orgs"]) => json_answer(&ask_registry(env, &admin::Orgs { after: param(url, "after") }).await?),
        (Method::Get, ["orgs", org]) => json_answer(&ask_registry(env, &OrgOf { by: None, org: Some(org.to_string()) }).await?),
        (Method::Get, ["people"]) => json_answer(&ask_registry(env, &admin::People { q: param(url, "q"), after: param(url, "after") }).await?),
        (Method::Get, ["people", person]) => json_answer(&person_page(env, person).await?),
        // FIN-352's census: reach open by link or public, counted (reach.rs)
        (Method::Get, ["people", person, "reach"]) => json_answer(&crate::reach::census(env, person).await?),
        (Method::Put, ["people", person, "computer", "image"]) => json_answer(&pin(env, &by, person, &bytes).await?),
        (Method::Post, ["people", person, "computer", "start-from"]) => json_answer(&start_from(env, &by, person, &bytes).await?),
        // the finite.computer move's moves, and staff's acts on one
        (m, ["migrations", rest @ ..]) => crate::migrations::admin(env, cfg, &by, m, rest, url, &bytes).await,
        (Method::Post, ["objects", class, "wake"]) => json_answer(&crate::routed::wake(env, class, body(&bytes)?).await?),
        (Method::Post, ["private-agent"]) => json_answer(&private_agent(env, cfg, &req, url, &bytes, by).await?),
        (Method::Get, ["log"]) => {
            let before = param(url, "before").map(|b| b.parse::<u64>().map_err(|_| CellError::invalid("`before` is an entry's number"))).transpose()?;
            json_answer(&ask_registry(env, &admin::Log { before }).await?)
        }
        (Method::Post, ["trials"]) => json_answer(&ask_registry(env, &TrialNew { by, code: body(&bytes)? }).await?),
        (Method::Get, ["trials"]) => json_answer(&ask_registry(env, &TrialList {}).await?),
        (Method::Get, ["trials", id]) => json_answer(&ask_registry(env, &TrialGet { id: id.to_string() }).await?),
        (Method::Patch, ["trials", id]) => json_answer(&ask_registry(env, &TrialChange { by, id: id.to_string(), change: body(&bytes)? }).await?),
        (Method::Post, ["trials", id, "send"]) => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Send {
                email: String,
            }
            let send: Send = body(&bytes)?;
            let code = ask_registry(env, &TrialGet { id: id.to_string() }).await?;
            if !code.active || code.expires_at.is_some_and(|at| at <= crate::js::now_ms()) {
                return Err(CellError::invalid("this trial code has ended: turn it on, or make another"));
            }
            // a code of the $100 seat, made before it stopped being sold, is refused
            fragment_core::stripe::for_sale(code.kind)?;
            let text = format!(
                "You are invited to try {PRODUCT}: {} days of a seat with an always-on computer, free. A card is taken first, and charged once the trial ends unless you cancel.\n\nSign in with this email address, then start your trial: {}/welcome?trial={}\n\nYour code: {}\n",
                code.days,
                cfg.platform(),
                code.code,
                code.code
            );
            let mail = crate::mail::Mail { to: send.email.trim().to_ascii_lowercase(), subject: format!("Try {PRODUCT}"), text };
            crate::mail::send(env, cfg, &mail).await?;
            ask_registry(env, &admin::Note { by, action: "trial-send".into(), target: code.id.clone(), detail: format!("to {}", mail.to), request: None }).await?;
            json_answer(&serde_json::json!({ "sent": true, "to": mail.to }))
        }
        // the recovery of an org with no content admin (a sole admin's
        // wipe; decision 69): its governance log and the operators' name it,
        // in the one change
        (Method::Post, ["orgs", org, "content-admin-offers"]) => {
            let b: NewContentAdmin = body(&bytes)?;
            json_answer(&ask_registry(env, &OfferAdmin { by: None, operator: Some(by), org: org.to_string(), email: b.email, request: b.id }).await?)
        }
        (Method::Get, ["orgs", org, "content"]) => json_answer(&ask_registry(env, &ContentOf { by: None, org: org.to_string(), before: before(url)? }).await?),
        // an org's own credit, as an operator who credits it reads it
        (Method::Get, ["orgs", org, "ledger"]) => {
            ask_registry(env, &OrgOf { by: None, org: Some(org.to_string()) }).await?;
            let ledger = fragment_core::ledger::org::Payer::Org(org.to_string()).name();
            json_answer(&crate::ledger::ask(env, &ledger, &crate::ledger::OrgStatus {}).await?)
        }
        // the person an org's credit is invoiced to, named by an operator
        // (orgs pay out of band: Paul, 2026-10-30), in its governance log
        (Method::Post, ["orgs", org, "payer"]) => {
            let b: NamePayer = body(&bytes)?;
            json_answer(&ask_registry(env, &NominatePayer { operator: by, org: org.to_string(), person: b.person, request: b.id }).await?)
        }
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/admin/{}", m.as_ref(), rest.join("/")))),
    }
}

/// `POST /api/admin/private-agent` (docs/api.md, "A private agent"): the
/// operator's own new account at the deployment's private home, by a
/// one-time invite its door makes when asked (signed with the key the door
/// trusts: keys.rs `private_home_ask`); for an email that has an account
/// there, the home to sign in at, so this deployment never gets into an
/// account at the home. The email is the operator's own, verified at their
/// sign-in here, never the request's (it takes no body). Noted in the
/// operators' log.
async fn private_agent(env: &Env, cfg: &Config, req: &Request, url: &url::Url, bytes: &[u8], by: String) -> CellResult<fragment_proto::PrivateAgent> {
    let door = cfg.private_home_door.as_deref().ok_or_else(|| CellError::new(ErrorCode::NotFound, "this deployment names no private home"))?;
    let view = ask_registry(env, &crate::registry::calls::View { identity: None, by: crate::orgs::signed_by(env, req, url, bytes)? }).await?;
    let email = view.email.ok_or_else(|| CellError::invalid("a private agent is a signed-in person's, by their email: you signed in with none"))?;
    let (status, answer) = crate::keys::private_home_ask(env, door, &email).await?;
    let agent = fragment_core::private_home::answered(door, status, &answer).map_err(|why| CellError::new(ErrorCode::UpstreamFailed, why))?;
    let detail = if agent.invited { "invited to a new account at the private home" } else { "has an account at the private home: to sign in" };
    ask_registry(env, &admin::Note { by, action: "private-agent".into(), target: view.id, detail: detail.into(), request: None }).await?;
    Ok(agent)
}

/// A person's page for operators: as the list shows them, their ledger,
/// their computer, and the whole references of the image it runs and of
/// the one its next start runs (both `null` without a computer).
async fn person_page(env: &Env, person: &str) -> CellResult<serde_json::Value> {
    if !fragment_core::npub::is_identity(person) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no person {person}")));
    }
    let row = ask_registry(env, &admin::Person { person: person.to_string() }).await?;
    let ledger = crate::ledger::ask(env, person, &crate::ledger::Status {}).await.map_err(|e| CellError::new(e.code, e.message))?;
    let id = fragment_core::computer::default_computer_of(person);
    let (computer, references) = match crate::computer::ask(env, &id, "computer/view", &serde_json::json!({})).await {
        Ok(v) => (for_operators(v), crate::computer::ask(env, &id, "computer/references", &serde_json::json!({})).await?),
        Err(e) if e.code == ErrorCode::NotFound => (serde_json::Value::Null, serde_json::Value::Null),
        Err(e) => return Err(e),
    };
    Ok(serde_json::json!({ "person": row, "ledger": ledger, "computer": computer, "references": references }))
}

/// An operator pins a person's computer to another of the deployment's
/// images (docs/computers.md, "Preview and production"), or back to its
/// default. As its owner's pin is, its next start runs it, and awake on
/// another, its owner is told an update is ready. Logged as `pin`.
async fn pin(env: &Env, by: &str, person: &str, bytes: &[u8]) -> CellResult<serde_json::Value> {
    if !fragment_core::npub::is_identity(person) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no person {person}")));
    }
    let image = crate::computer::image_named(bytes)?;
    let id = fragment_core::computer::default_computer_of(person);
    let view = crate::computer::ask(env, &id, "computer/pin", &serde_json::json!({ "image": image })).await.map_err(|e| match e.code {
        ErrorCode::NotFound => CellError::new(ErrorCode::NotFound, format!("{person} has no computer yet")),
        _ => e,
    })?;
    ask_registry(env, &admin::Note { by: by.to_string(), action: "pin".into(), target: person.to_string(), detail: image, request: None }).await?;
    Ok(for_operators(view))
}

/// An operator starts a person's computer from one of the saves it keeps
/// (docs/computers.md, "Saves and what a wake restores"), while it runs no
/// container: its next start restores that save, and the newer are set
/// aside, kept and exportable but never restored by a start. One that
/// won't wake is asleep after it, so its next wake starts it. Logged as
/// `start-from`, with the saves then set aside.
async fn start_from(env: &Env, by: &str, person: &str, bytes: &[u8]) -> CellResult<serde_json::Value> {
    if !fragment_core::npub::is_identity(person) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no person {person}")));
    }
    let save = crate::computer::save_named(bytes)?;
    let id = fragment_core::computer::default_computer_of(person);
    let view = crate::computer::ask(env, &id, "computer/start-from", &serde_json::json!({ "save": save })).await.map_err(|e| match e.code {
        ErrorCode::NotFound => CellError::new(ErrorCode::NotFound, format!("{person} has no computer yet")),
        _ => e,
    })?;
    let aside: Vec<String> = view["saves"].as_array().into_iter().flatten().filter(|s| s["setAside"].is_i64()).filter_map(|s| s["number"].as_u64()).map(|n| n.to_string()).collect();
    let detail = match aside.is_empty() {
        true => format!("save {save}"),
        false => format!("save {save}; set aside {}", aside.join(", ")),
    };
    ask_registry(env, &admin::Note { by: by.to_string(), action: "start-from".into(), target: person.to_string(), detail, request: None }).await?;
    Ok(for_operators(view))
}

/// A person's computer's view as its operators read it: its owner's, but
/// never what takes its disk (`sizes`: the paths of a person's files, its
/// owner's alone; Codex on #1426). Its size and room stay (`disk`).
fn for_operators(mut view: serde_json::Value) -> serde_json::Value {
    if let Some(v) = view.as_object_mut() {
        v.remove("sizes");
    }
    view
}

/// A new comp's mail to its email: where to sign in to use it, or to
/// accept it (an email no one signs in with yet is offered it). The seat
/// is made whatever this answers; a mail that fails is logged and said.
async fn mail_comp(env: &Env, cfg: &Config, comped: &Comped) -> bool {
    let kind = match comped.seat.seat {
        Some(SeatKind::SeatAlwaysOn) => "a seat with an always-on computer",
        _ => "a seat",
    };
    let held = match comped.seat.person {
        Some(_) => "It is yours now",
        None => "Sign in with this email address to accept it",
    };
    let text = format!("You have {kind} on {PRODUCT}, in {}.\n\n{held}: {}/\n", comped.org.name, cfg.platform());
    let mail = crate::mail::Mail { to: comped.seat.email.clone(), subject: format!("You have a seat on {PRODUCT}"), text };
    match crate::mail::send(env, cfg, &mail).await {
        Ok(_) => true,
        Err(e) => {
            console_error!("{}", serde_json::json!({ "comp-mail": comped.seat.id, "failed": e.message }));
            false
        }
    }
}
