//! `GET /api/notices` (FIN-458): what a person is told now. The router
//! asks each owner for its own notices, in its own words, at once, and
//! puts them together, the most pressing first: their computer's, then the
//! registry's (the invites waiting on their emails). It keeps none, and
//! reads nothing in them. Each owner tells the person's pages when its own
//! change (`principal::tell_changed`). An owner that does not answer fails
//! the read: a page says it couldn't refresh, never that nothing is told.

use futures_util::future::join;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::ReadJson;
use crate::config::Config;
use crate::registry::calls::AnswerAsked;
use crate::registry::invites::NoticesOf;
use fragment_proto::{limits, ErrorCode, IdentityKind};

pub(crate) async fn route(req: &Request, env: &Env, url: &Url) -> CellResult<Response> {
    let who = crate::signer(env, req, url, &[]).await?;
    if who.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "notices are a person's: an agent's owner is told of it"));
    }
    let person = who.identity.id;
    let (computer, registry) = join(crate::computer::notices_for(env, &person), crate::ask_registry(env, &NoticesOf { person: person.clone() })).await;
    let mut computer = computer?;
    // its refusal (a deploy under way), forwarded as it answered
    if computer.status_code() != 200 {
        return Ok(computer);
    }
    let computer: Value = computer.read_json().await?;
    let notices: Vec<Value> = [computer, registry?].iter().flat_map(|owner| owner["notices"].as_array().cloned().unwrap_or_default()).collect();
    crate::json_answer(&json!({ "notices": notices }))
}

/// `POST /api/notices/asked/{id} {answer}`, a test fleet's alone: the
/// signer's answer to the must-answer notice a test asked them
/// (`TestHook::Ask`, as no owner asks one yet), as the registry takes it:
/// none such 404, the answer it refuses 409 in its words.
pub(crate) async fn answer_asked(mut req: Request, env: &Env, cfg: &Config, url: &Url, id: &str) -> CellResult<Response> {
    if !cfg.test_hooks {
        return Err(CellError::new(ErrorCode::NotFound, format!("no route POST {}", url.path())));
    }
    let body = crate::read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let who = crate::signer(env, &req, url, &body).await?;
    let answer: Value = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("an answer is {{answer}}: {e}")))?;
    let answer = answer["answer"].as_str().ok_or_else(|| CellError::invalid("an answer is {answer}"))?.to_string();
    crate::json_answer(&crate::ask_registry(env, &AnswerAsked { person: who.identity.id, id: id.to_string(), answer }).await?)
}
