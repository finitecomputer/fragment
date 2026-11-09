//! `GET /api/notices` (FIN-458): what a person is told now. The router
//! asks each owner for its own notices, in its own words, and forwards
//! them; it keeps none and reads none of them. Each owner tells the
//! person's pages when its own change (`principal::tell_changed`).

use worker::*;

use crate::error::{CellError, CellResult};
use fragment_proto::{ErrorCode, IdentityKind};

pub(crate) async fn route(req: &Request, env: &Env, url: &Url) -> CellResult<Response> {
    let who = crate::signer(env, req, url, &[]).await?;
    if who.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "notices are a person's: an agent's owner is told of it"));
    }
    crate::computer::notices_for(env, &who.identity.id).await
}
