//! `GET /api/notices` (FIN-458): what a person is told now. The router
//! asks each owner for its own notices, in its own words, at once, and
//! puts them together, the most pressing first: their computer's, then the
//! registry's (the invites waiting on their emails), then the move's (an
//! offer to bring an old agent over: `migrations::notices`). It keeps none, and
//! reads nothing in them: their lists are joined as bytes
//! (`fragment_core::notice::joined`), each notice as its owner wrote it. Each owner tells the person's pages when its own
//! change (`principal::tell_changed`). An owner that does not answer fails
//! the read: a page says it couldn't refresh, never that nothing is told.

use futures_util::future::join3;
use worker::*;

use crate::error::{CellError, CellResult};
use crate::registry::calls::Call;
use crate::registry::invites::NoticesOf;
use fragment_proto::{ErrorCode, IdentityKind};

pub(crate) async fn route(req: &Request, env: &Env, cfg: &crate::config::Config, url: &Url) -> CellResult<Response> {
    let who = crate::signer(env, req, url, &[]).await?;
    if who.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "notices are a person's: an agent's owner is told of it"));
    }
    let person = who.identity.id;
    let asked = serde_json::to_string(&NoticesOf { person: person.clone() }).map_err(|e| CellError::host(format!("a registry call: {e}")))?;
    let (computer, registry, moves) = join3(crate::computer::notices_for(env, &person), crate::registry_round_trip(env, NoticesOf::PATH, asked), crate::migrations::notices(env, cfg, &person)).await;
    let mut computer = computer?;
    // its refusal (a deploy under way), forwarded as it answered
    if computer.status_code() != 200 {
        return Ok(computer);
    }
    let computer = computer.bytes().await?;
    let joined = fragment_core::notice::joined(&[&computer, &registry?, &moves?]).ok_or_else(|| CellError::host("an owner's notices are not a notice list"))?;
    Ok(Response::builder().with_header("content-type", "application/json")?.fixed(joined))
}
