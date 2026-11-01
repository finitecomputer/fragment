//! `POST /api/search`: one question, asked of several fragments' own
//! indexes as the signer (docs/api.md, "Search across fragments"). The
//! meta-brain's search: a parent and its sources, or a person's brains and
//! their Session Archive, searched as one.
//!
//! Each fragment named is asked as the signer, all at once, exactly as if
//! the signer asked it alone: it checks their role, leaves out what it is
//! forgetting, and answers its own hits (searchable.rs, at the internal
//! `/search/asked`). The question is embedded once, here, paid by its
//! asker (an agent's owner), and each fragment searches with that vector.
//! The hits are merged (fragment_core::across); each fragment's own
//! answer is also said: searched, no index, denied, missing, or failed.
//! So this route returns nothing the signer could not have from each
//! fragment, holds nothing, and adds no index.

use fragment_core::across;
use fragment_core::searchable as rules;
use fragment_proto::search::{self as wire, Across, Asked, Forwarded, Reached};
use fragment_proto::IdentityKind;
use futures_util::future::join_all;
use serde_json::{json, Value};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::models::{embedded, Embedding};
use crate::routed::{Routed, Signed};

/// A question's body, at most: its words, 32 names and its filters.
pub(crate) const BODY_MAX_BYTES: usize = 16 * 1024;

pub(crate) async fn route(mut req: Request, env: &Env, cfg: &Config, url: &Url, ctx: &Context) -> CellResult<Response> {
    let body = crate::read_body(&mut req, BODY_MAX_BYTES).await?;
    let who = crate::signer_for(env, &req, url, &body).await?;
    let asked: Across = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("body: {e}")))?;
    across::check(&asked).map_err(CellError::invalid)?;
    let limit = asked.limit.unwrap_or(wire::RESULTS_DEFAULT);
    let vector = question_vector(env, cfg, &who, &asked, ctx).await?;
    // the space the question is in, which an answer's cosines must name
    let space = vector.as_ref().map(|_| cfg.models.embeddings.search_space());
    let semantic = vector.is_some();
    let inner = json!(Forwarded { query: across::query(&asked, Some(across::asked_limit(limit))), vector }).to_string();
    let answers = join_all(asked.fragments.iter().map(|name| ask(env, url, &who, name, &inner))).await;
    let mut fragments = Vec::with_capacity(answers.len());
    let mut hits: Vec<Vec<Value>> = Vec::with_capacity(answers.len());
    let mut legs: Vec<Vec<across::Legs>> = Vec::with_capacity(answers.len());
    for (name, answer) in asked.fragments.iter().zip(answers) {
        let reply = match answer {
            Ok((status, v)) => across::reached(status, v, space.as_ref()),
            Err(e) => across::Reply { state: Reached::Failed, message: Some(e.message), hits: vec![], cosines: false },
        };
        legs.push(reply.hits.iter().enumerate().map(|(i, h)| across::legs(h, i, reply.cosines)).collect());
        fragments.push(Asked { name: name.clone(), state: reply.state, message: reply.message });
        hits.push(reply.hits);
    }
    let results: Vec<Value> = across::merge(&legs, limit)
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let mut hit = hits[p.fragment][p.hit].clone();
            hit["fragment"] = json!(asked.fragments[p.fragment]);
            hit["rank"] = json!(i + 1);
            hit["score"] = json!(p.score);
            hit
        })
        .collect();
    Ok(Response::from_json(&json!({ "results": results, "fragments": fragments, "semantic": if semantic { "on" } else { "off" } }))?)
}

/// The question's vector (base64, the deployment's space), embedded once
/// and paid by its asker; none for words alone, on a deployment that
/// embeds nothing, or when the model refused it (searched by its words).
async fn question_vector(env: &Env, cfg: &Config, who: &Signed, asked: &Across, ctx: &Context) -> CellResult<Option<String>> {
    if asked.mode == wire::Mode::Lexical || cfg.models.embeddings_refused().is_some() {
        return Ok(None);
    }
    let (payer, agent) = match who.identity.kind {
        IdentityKind::Agent => (who.identity.owner.clone(), Some(who.identity.id.as_str())),
        _ => (Some(who.identity.id.clone()), None),
    };
    let Some(payer) = payer else { return Ok(None) };
    // after the slot's query prefix, as each child's chunks after its
    // document prefix (searchable.rs)
    Ok(match embedded(env, &[cfg.models.embeddings.question(&asked.q)], &payer, agent, None, ctx).await? {
        Embedding::Vectors { vectors, .. } => vectors.into_iter().next().map(|v| rules::vector_b64(&v)),
        Embedding::Refused { status, text, .. } => {
            console_warn!("{}", json!({ "event": "search.across-unembedded", "status": status, "why": text }));
            None
        }
    })
}

/// `name` asked the question as the signer: its status and answer.
async fn ask(env: &Env, url: &Url, who: &Signed, name: &str, inner: &str) -> CellResult<(u16, Value)> {
    let make = || {
        let headers = Headers::new();
        Routed { name: name.to_string(), url: url.clone(), signed: Some(who.clone()), credential: None }.to_headers(&headers)?;
        headers.set("content-type", "application/json")?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(headers).with_body(Some(inner.into()));
        Ok(Request::new_with_init("https://fragment.internal/search/asked", &init)?)
    };
    let mut resp = crate::routed::send_again(env, "FRAGMENT", name, make).await?;
    let status = resp.status_code();
    Ok((status, resp.json().await.unwrap_or(Value::Null)))
}
