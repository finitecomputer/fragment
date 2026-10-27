//! Feedback on the platform's API (docs/api.md, Feedback): a person
//! reports a problem or sends a message to the deployment's operators,
//! from the shell's Settings, with up to three screenshots; the operators
//! read it on the admin's page. No vendor's product: Cloudflare offers
//! none (Turnstile's feedback is its own challenges'), nor do WorkOS or
//! Stripe, and a tracker's widget would put a third party's script in
//! the shell, which holds the person's session.
//!
//!   POST  /api/feedback                                  a person sends one (multipart)
//!   GET   /api/admin/feedback?before=&open=              the operators read a page
//!   GET   /api/admin/feedback/{id}/screenshots/{place}   one's screenshot
//!   PATCH /api/admin/feedback/{id}                       marked done, or open again
//!   DELETE /api/admin/feedback/{id}                      deleted, then its screenshots (the registry's alarm)
//!
//! A send costs two registry turns, each idempotent on its id (the
//! sender's, else made here): the first begins it, a row `sending`, so no
//! one the registry does not know, or who sent their day's, stores bytes,
//! and none is put that no row records; its screenshots go to BLOBS
//! (`feedback/<person>/<id>/<place>`); the second keeps it. Nothing here
//! deletes on a failure, as a call that failed may have acted (its answer
//! lost): a send that stops anywhere is the registry's to finish, asked
//! again with the same id, or to delete with its bytes once it has waited
//! too long (registry/feedback.rs). The operators are mailed each one at
//! the deployment's `feedback_to` (`FRAGMENT_FEEDBACK_TO`) when it names
//! one, by whichever try learns it is kept and its notice has not gone
//! (marked in its row once it has); the feedback is kept whatever becomes
//! of the mail.

use fragment_core::feedback::{self as rules, Notice};
use fragment_core::multipart;
use fragment_proto::feedback::{Feedback, FeedbackSent, Screenshot, SetResolved};
use fragment_proto::ErrorCode;
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls::By;
use crate::registry::feedback::{FeedbackBegin, FeedbackDelete, FeedbackGet, FeedbackKeep, FeedbackList, FeedbackMailed, FeedbackResolve, Kept};
use crate::{acting_for, ask_registry, caller, js, json_answer, read_body, Caller};
use fragment_nip98::Payload;

/// `POST /api/feedback`: the asker's own (a person's session, or a key
/// of theirs; an agent is refused), as `multipart/form-data`
/// (`fragment_core::feedback::read`). Answers `FeedbackSent`.
pub(crate) async fn send(mut req: Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    if acting_for(url)?.is_some() {
        return Err(CellError::invalid(crate::FOR_HONORED));
    }
    let bytes = read_body(&mut req, rules::BODY_MAX_BYTES).await?;
    let asker = caller(env, &req, url, Payload::Read(&bytes))?;
    let by = || match &asker {
        Caller::Key(k) => By::Key(k.clone()),
        Caller::Session(t) => By::Session(t.clone()),
    };
    let content_type = req.headers().get("content-type")?.unwrap_or_default();
    let malformed = |m: multipart::Malformed| CellError::invalid(m.message());
    let parts = multipart::parts(&bytes, &multipart::boundary(&content_type).map_err(malformed)?).map_err(malformed)?;
    let sent = rules::read(&parts).map_err(|r| match r {
        rules::Refused::TooLarge(..) => CellError::new(ErrorCode::TooLarge, r.message()),
        r => CellError::invalid(r.message()),
    })?;
    let user_agent = req.headers().get("user-agent")?.as_deref().and_then(rules::user_agent);
    let id = sent.id.clone().unwrap_or_else(js::random_hex::<16>);
    let screenshots: Vec<Screenshot> = sent.screenshots.iter().map(|s| Screenshot { mime: s.mime.to_string(), size: s.bytes.len() as u64 }).collect();
    let begin = FeedbackBegin { by: by(), id: id.clone(), kind: sent.kind, message: sent.message.clone(), page: sent.page.clone(), user_agent, release: cfg.deploy_id.clone(), screenshots };
    let began = ask_again(env, &begin).await?;
    let Kept { feedback: kept, mailed } = match began.kept {
        // kept already: a send asked again whose answer was lost
        Some(kept) => kept,
        None => {
            for (place, shot) in sent.screenshots.iter().enumerate() {
                js::blob_put_bytes(env, &rules::screenshot_key(&began.person, &id, place), shot.bytes).await?;
            }
            if began.again {
                // a try before may have put more than this one holds
                let past: Vec<String> = (sent.screenshots.len()..rules::SCREENSHOTS_MAX).map(|place| rules::screenshot_key(&began.person, &id, place)).collect();
                if !past.is_empty() {
                    js::blob_delete(env, &past).await?;
                }
            }
            ask_again(env, &FeedbackKeep { by: by(), id: id.clone() }).await?
        }
    };
    assert!(kept.person == began.person && kept.id == id, "the registry keeps a send as its sender's");
    // the notice goes from whichever try learns it is kept and not yet
    // mailed: at least once (a try that mailed and lost its mark mails again)
    if !mailed {
        notify(env, cfg, &kept).await;
    }
    json_answer(&FeedbackSent { id: kept.id, screenshots: kept.screenshots.len() as u64 })
}

/// The operators' notice of `kept`, at the deployment's `feedback_to`, then
/// its mark (`FeedbackMailed`). A failure of either is logged, never the
/// sender's: the feedback is kept, and a send asked again mails it.
async fn notify(env: &Env, cfg: &Config, kept: &Feedback) {
    // an e2e person's (`<name>@e2e.test`) on a preview, whose mail is real,
    // is not mailed: a hosted run sends a dozen, which are the run's own
    let e2e = cfg.mail_url.is_none() && cfg.test_hooks && kept.email.as_deref().is_some_and(fragment_core::levers::valid_e2e_email);
    let Some(to) = cfg.feedback_to.as_ref().filter(|_| !e2e) else { return };
    let admin = format!("{}/admin", cfg.platform());
    let notice = rules::notice(&Notice {
        to,
        from: kept.email.as_deref(),
        person: &kept.person,
        kind: kept.kind,
        message: &kept.message,
        page: kept.page.as_deref(),
        screenshots: kept.screenshots.len(),
        admin: &admin,
    });
    if let Err(e) = crate::mail::send(env, cfg, &notice).await {
        console_error!("{}", serde_json::json!({ "feedback-mail": kept.id, "failed": e.message }));
        return;
    }
    if let Err(e) = ask_again(env, &FeedbackMailed { id: kept.id.clone() }).await {
        console_error!("{}", serde_json::json!({ "feedback-mailed": kept.id, "failed": e.message }));
    }
}

/// A registry call idempotent on its id, asked again once when its answer
/// was lost (`RegistryUnavailable`: it may have acted, and asked again it
/// is the same call); its refusal is answered as it came.
async fn ask_again<C: crate::registry::calls::Call>(env: &Env, call: &C) -> CellResult<C::Answer> {
    match ask_registry(env, call).await {
        Err(e) if e.code == ErrorCode::RegistryUnavailable => ask_registry(env, call).await,
        answer => answer,
    }
}

/// `/api/admin/feedback…`, for the operators (orgs.rs `admin` checked
/// who asks: `by`).
pub(crate) async fn admin(env: &Env, url: &Url, by: String, method: Method, rest: &[&str], bytes: &[u8]) -> CellResult<Response> {
    let param = |k: &str| url.query_pairs().find(|(q, _)| q == k).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty());
    match (method, rest) {
        (Method::Get, []) => {
            let before = param("before").map(|b| b.parse::<u64>().map_err(|_| CellError::invalid("`before` is a feedback's number"))).transpose()?;
            let open = match param("open").as_deref() {
                None | Some("0") => false,
                Some("1") => true,
                Some(_) => return Err(CellError::invalid("`open` is 1 (only those not done) or 0")),
            };
            json_answer(&ask_registry(env, &FeedbackList { before, open }).await?)
        }
        (Method::Get, [id, "screenshots", place]) => {
            let place: usize = place.parse().map_err(|_| CellError::new(ErrorCode::NotFound, format!("no screenshot {place:?}")))?;
            let f = ask_registry(env, &FeedbackGet { id: id.to_string() }).await?;
            let shot = f.screenshots.get(place).ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("feedback {id} has {} screenshots", f.screenshots.len())))?;
            let blob = js::blob_get(env, &rules::screenshot_key(&f.person, &f.id, place), None).await?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("screenshot {place} of feedback {id} is gone")))?;
            let h = Headers::new();
            // its type was read from its bytes as it came: an image, never markup
            h.set("content-type", &shot.mime)?;
            h.set("x-content-type-options", "nosniff")?;
            h.set("content-security-policy", "default-src 'none'; sandbox")?;
            h.set("cache-control", "private, no-store")?;
            Ok(Response::from_body(ResponseBody::Stream(blob.body))?.with_headers(h))
        }
        (Method::Patch, [id]) => {
            let set: SetResolved = serde_json::from_slice(bytes).map_err(|e| CellError::invalid(format!("body: {e}")))?;
            json_answer(&ask_registry(env, &FeedbackResolve { by, id: id.to_string(), resolved: set.resolved }).await?)
        }
        // its row stays, `deleting`, until the registry's alarm has deleted
        // its bytes: their record, whatever stops between the two
        (Method::Delete, [id]) => json_answer(&ask_registry(env, &FeedbackDelete { by, id: id.to_string() }).await?),
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/admin/feedback/{}", m.as_ref(), rest.join("/")))),
    }
}
