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
use crate::registry::invites::NoticesOf;
use fragment_proto::{ErrorCode, IdentityKind};

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
