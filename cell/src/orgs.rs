//! Seats and orgs on the platform's API (docs/billing.md; docs/api.md,
//! Seats and orgs): a person's own seat (`/api/seat`), and their org to its
//! admins (`/api/org`); the operators' comps and view of any org are the
//! admin Worker's (admin.rs). The registry keeps them (registry/orgs.rs)
//! and pushes each seat's plan to its holder's ledger and computer; this
//! module only checks who asks, an operator too (`operator`, which the
//! admin's routes, a wipe's and a ledger command's hold callers to).

use fragment_core::mail::PRODUCT;
use fragment_core::governance::Answer;
use fragment_core::permit;
use fragment_proto::org::{NewContentAdmin, OrgMember, SeatKind, SeatStatus, SetSleeps};
use fragment_proto::{limits, ErrorCode};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls::By;
use crate::registry::content::{AnswerOffer, ContentOf, OfferAdmin, OffersToMe, StepDown, WithdrawOffer};
use crate::registry::orgs::{Mine, Mined, OrgOf, Sleeps, SyncSeat};
use crate::registry::offers::{AcceptOffer, DeclineOffer, LeaveOrg};
use crate::registry::seats::{AddAdmin, AddSeat, Added, RemoveAdmin, RemoveSeat};
use crate::{ask_registry, caller, json_answer, read_body, Caller};
use fragment_nip98::Payload;

pub(crate) fn body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> CellResult<T> {
    serde_json::from_slice(bytes).map_err(|e| CellError::invalid(format!("body: {e}")))
}

/// Who asks, for the registry to resolve in the same turn as what it asks.
fn by(env: &Env, req: &Request, url: &Url, bytes: &[u8]) -> CellResult<By> {
    if crate::names_for(req, url)? {
        return Err(CellError::invalid(crate::FOR_HONORED));
    }
    signed_by(env, req, url, bytes)
}

/// Who signs, a `for` aside: the caller's to read (the invites', which an
/// agent answers for its owner: share.rs `invites`).
pub(crate) fn signed_by(env: &Env, req: &Request, url: &Url, bytes: &[u8]) -> CellResult<By> {
    Ok(match caller(env, req, url, Payload::Read(bytes))? {
        Caller::Key(k) => By::Key(k),
        Caller::Session(t) => By::Session(t),
    })
}

/// `/api/seat…`: `GET` the asker's seat, org and what is offered them,
/// whether they are a guest, and what is sold (nothing without Stripe);
/// `PUT {sleeps}` lets their always-on computer sleep, or not. Their own
/// answers: accept or decline an offer (`offers/{id}/accept|decline`), and
/// leave their org (`leave`, a seat's holder who is no admin). Nobody joins
/// an org but by these.
pub(crate) async fn seat(mut req: Request, env: &Env, cfg: &Config, url: &Url, rest: &[&str]) -> CellResult<Response> {
    let bytes = read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let by = by(env, &req, url, &bytes)?;
    let empty = || -> CellResult<()> {
        // a body is `{}` or none: what is answered is in the path
        match bytes.is_empty() || serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).is_ok_and(|m| m.is_empty()) {
            true => Ok(()),
            false => Err(CellError::invalid("this takes no body (or {})")),
        }
    };
    match (req.method(), rest) {
        (Method::Post, ["offers", offer, "accept"]) => {
            empty()?;
            json_answer(&ask_registry(env, &AcceptOffer { by, offer: offer.to_string() }).await?)
        }
        (Method::Post, ["offers", offer, "decline"]) => {
            empty()?;
            json_answer(&ask_registry(env, &DeclineOffer { by, offer: offer.to_string() }).await?)
        }
        (Method::Post, ["leave"]) => {
            empty()?;
            json_answer(&ask_registry(env, &LeaveOrg { by }).await?)
        }
        (_, [_, ..]) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/seat/{}", req.method().as_ref(), rest.join("/")))),
        (m, []) => seat_itself(env, cfg, &bytes, by, m).await,
    }
}

/// `GET` or `PUT /api/seat`.
async fn seat_itself(env: &Env, cfg: &Config, bytes: &[u8], by: By, method: Method) -> CellResult<Response> {
    match method {
        Method::Get => {
            let Mined { person, mine } = ask_registry(env, &Mine { by }).await?;
            // holding none, their ledger says whether a plan of its own gives them one
            let guest = match mine.seat {
                Some(_) => false,
                None => {
                    let ledger = crate::ledger::ask(env, &person, &crate::ledger::Status {}).await?;
                    fragment_core::org::guest(false, ledger.plan, ledger.seat)
                }
            };
            json_answer(&SeatStatus { mine, guest, offer: cfg.stripe().is_ok().then(fragment_core::stripe::offer) })
        }
        Method::Put => {
            let set: SetSleeps = body(bytes)?;
            json_answer(&ask_registry(env, &Sleeps { by, sleeps: set.sleeps }).await?)
        }
        m => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/seat", m.as_ref()))),
    }
}

/// `/api/org…`, for the asker's org's admins: `GET` it; offer a paid seat
/// by email (the one sold, `seat_always_on`), remove one; offer or remove
/// an admin's place. An offer is mailed; it is no one's until they accept
/// it. One of the org's members is given it at once.
pub(crate) async fn org(mut req: Request, env: &Env, cfg: &Config, url: &Url, rest: &[&str]) -> CellResult<Response> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NewSeat {
        email: String,
        #[serde(default = "SeatKind::sold")]
        kind: SeatKind,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NewAdmin {
        email: String,
    }
    let bytes = read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let by = by(env, &req, url, &bytes)?;
    match (req.method(), rest) {
        (Method::Get, []) => json_answer(&ask_registry(env, &OrgOf { by: Some(by), org: None }).await?),
        (Method::Post, ["seats"]) => {
            let b: NewSeat = body(&bytes)?;
            let added = ask_registry(env, &AddSeat { by, email: b.email, kind: b.kind }).await?;
            let mailed = added.created && cfg.mail_from.is_some() && mail_offer(env, cfg, &added).await;
            json_answer(&serde_json::json!({ "seat": added.member, "created": added.created, "mailed": mailed }))
        }
        (Method::Delete, ["seats", seat]) => json_answer(&ask_registry(env, &RemoveSeat { by, seat: seat.to_string() }).await?),
        (Method::Post, ["admins"]) => {
            let b: NewAdmin = body(&bytes)?;
            let added = ask_registry(env, &AddAdmin { by, email: b.email }).await?;
            let mailed = added.created && cfg.mail_from.is_some() && mail_offer(env, cfg, &added).await;
            json_answer(&serde_json::json!({ "admin": added.member, "created": added.created, "mailed": mailed }))
        }
        (Method::Delete, ["admins", member]) => json_answer(&ask_registry(env, &RemoveAdmin { by, member: member.to_string() }).await?),
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/org/{}", m.as_ref(), rest.join("/")))),
    }
}

/// `/api/orgs/<org>/…`, an org named, never found through a person
/// (decision 66). Its money: `GET ledger`, its own credit and this month's
/// spend, for its billing admins; its credit and its budget are an
/// operator's to set (orgs pay out of band: Paul, 2026-10-30), at
/// `/api/ledger/org:<id>/…` and `/api/admin/orgs/<org>/…`. Its content
/// (decisions 69 to 71), apart from its billing: `POST
/// content-admin-offers {id, email}` (its content admins; who pays, for
/// its first), answered by whoever signs in with the email (`POST
/// content-admin-offers/<offer>/accept|decline`; theirs are `GET
/// /api/orgs/offers`); `DELETE content-admin-offers/<offer>`, `DELETE
/// content-admins/<person>` and `GET content?before=` (its content
/// admins). Each is a person's own, never an agent's, and a billing
/// admin's place gives no content power.
pub(crate) async fn named(mut req: Request, env: &Env, url: &Url, org: &str, rest: &[&str]) -> CellResult<Response> {
    let bytes = read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let by = by(env, &req, url, &bytes)?;
    if let (Method::Get, "offers", []) = (req.method(), org, rest) {
        return json_answer(&ask_registry(env, &OffersToMe { by }).await?);
    }
    if !fragment_core::org::valid_org_id(org) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no org {org}")));
    }
    let empty = || match bytes.is_empty() || body::<serde_json::Map<String, serde_json::Value>>(&bytes).is_ok_and(|m| m.is_empty()) {
        true => Ok(()),
        false => Err(CellError::invalid("this takes no body (or {})")),
    };
    let ledger = fragment_core::ledger::org::Payer::Org(org.to_string()).name();
    // its billing admins: the registry sees them, or refuses
    let admins = |by| OrgOf { by: Some(by), org: Some(org.to_string()) };
    match (req.method(), rest) {
        (Method::Get, ["ledger"]) => {
            ask_registry(env, &admins(by)).await?;
            json_answer(&crate::ledger::ask(env, &ledger, &crate::ledger::OrgStatus {}).await?)
        }
        (Method::Post, ["content-admin-offers"]) => {
            let b: NewContentAdmin = body(&bytes)?;
            json_answer(&ask_registry(env, &OfferAdmin { by: Some(by), operator: None, org: org.to_string(), email: b.email, request: b.id }).await?)
        }
        (Method::Post, ["content-admin-offers", offer, verb @ ("accept" | "decline")]) => {
            empty()?;
            let answer = if *verb == "accept" { Answer::Accepted } else { Answer::Declined };
            json_answer(&ask_registry(env, &AnswerOffer { by, org: org.to_string(), offer: offer.to_string(), answer }).await?)
        }
        (Method::Delete, ["content-admin-offers", offer]) => json_answer(&ask_registry(env, &WithdrawOffer { by, org: org.to_string(), offer: offer.to_string() }).await?),
        (Method::Delete, ["content-admins", person]) => json_answer(&ask_registry(env, &StepDown { by, org: org.to_string(), person: person.to_string() }).await?),
        (Method::Get, ["content"]) => json_answer(&ask_registry(env, &ContentOf { by: Some(by), org: org.to_string(), before: before(url)? }).await?),
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/orgs/{org}/{}", m.as_ref(), rest.join("/")))),
    }
}

/// Who an operator's request is from (the admin's routes, operators'
/// ledger commands): a key the deployment lists, held by anyone or no one
/// (no registry is asked, as a wipe's); a signed request or the shell's
/// session (decision 59) of an identity it lists, or of a person who holds
/// a key it lists (the session acts as that key may). Anyone else is 403.
/// Answers the operator's name (their identity, or a key no one holds),
/// for the log and a grant's `by`.
pub(crate) async fn operator(env: &Env, cfg: &Config, req: &Request, url: &Url, bytes: &[u8]) -> CellResult<String> {
    if crate::names_for(req, url)? {
        return Err(CellError::invalid(crate::FOR_HONORED));
    }
    let refused = || CellError::new(ErrorCode::Forbidden, "only the deployment's operators do this");
    match caller(env, req, url, Payload::Read(bytes))? {
        Caller::Key(key) => {
            let listed = cfg.operators()?;
            match ask_registry(env, &crate::registry::calls::Resolve { key: key.clone() }).await {
                // a person's: named by their identity
                Ok(who) => permit::operator(listed, Some(&who.id), [key.as_str()]).then_some(who.id).ok_or_else(refused),
                // a listed key no one holds: named by itself
                Err(e) if e.code == ErrorCode::Unauthenticated && permit::operator(listed, None, [key.as_str()]) => Ok(fragment_core::npub::encode(&key)),
                Err(e) => Err(e),
            }
        }
        Caller::Session(token) => session_operator(env, cfg, token).await?.ok_or_else(refused),
    }
}

/// The operator a platform session is (decision 59): an identity the
/// deployment lists, or a person who holds a key it lists. Answers their
/// identity, or `None` for anyone else; a session that is not live is the
/// registry's 401. `operator` holds the shell's session to it, and
/// `GET /admin` the page's (shell.rs `admin_page`).
pub(crate) async fn session_operator(env: &Env, cfg: &Config, token: String) -> CellResult<Option<String>> {
    let view = ask_registry(env, &crate::registry::calls::View { identity: None, by: By::Session(token) }).await?;
    // bounded: an identity holds at most KEYS_PER_IDENTITY_MAX keys
    let keys: Vec<String> = view.keys.iter().filter(|k| k.revoked_at.is_none()).filter_map(|k| fragment_core::npub::parse(&k.npub)).collect();
    Ok(permit::operator(cfg.operators()?, Some(&view.id), keys.iter().map(String::as_str)).then_some(view.id))
}

/// A log page's `before`, a log entry's number when named.
pub(crate) fn before(url: &Url) -> CellResult<Option<u64>> {
    param(url, "before").map(|b| b.parse().map_err(|_| CellError::invalid("before is a log entry's number"))).transpose()
}

/// A query parameter, once.
pub(crate) fn param(url: &Url, k: &str) -> Option<String> {
    url.query_pairs().find(|(q, _)| q == k).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty())
}


/// What an org's row gives, as a mail says it.
fn what(m: &OrgMember) -> &'static str {
    match (m.seat, m.admin) {
        (Some(SeatKind::SeatAlwaysOn), true) => "a seat with an always-on computer, and a place as its admin",
        (Some(_), true) => "a seat, and a place as its admin",
        (Some(SeatKind::SeatAlwaysOn), false) => "a seat with an always-on computer",
        (Some(_), false) => "a seat",
        (None, _) => "a place as its admin (managing its seats and billing)",
    }
}

/// An offer's mail (or, to one of the org's members, what they were
/// given): from whom, and where to accept or decline it. The offer is
/// made whatever this answers; a mail that fails is logged and said.
async fn mail_offer(env: &Env, cfg: &Config, added: &Added) -> bool {
    let m = &added.member;
    let from = added.from.as_deref().map(|f| format!(" ({f})")).unwrap_or_default();
    let text = match m.person {
        Some(_) => format!("{}{from} gave you {} on {PRODUCT}.\n\nIt is yours now: {}/settings\n", added.org_name, what(m), cfg.platform()),
        None => format!(
            "{}{from} offers you {} on {PRODUCT}.\n\nNothing is yours until you accept it. Sign in with this email address to accept or decline it: {}/settings\n",
            added.org_name,
            what(m),
            cfg.platform()
        ),
    };
    let subject = match m.person {
        Some(_) => format!("{} gave you a place on {PRODUCT}", added.org_name),
        None => format!("{} offers you a place on {PRODUCT}", added.org_name),
    };
    let mail = crate::mail::Mail { to: m.email.clone(), subject, text };
    match crate::mail::send(env, cfg, &mail).await {
        Ok(_) => true,
        Err(e) => {
            console_error!("{}", serde_json::json!({ "offer-mail": m.id, "failed": e.message }));
            false
        }
    }
}

/// A computer was made: its owner's seat is pushed to it again, so it
/// learns whether it stays awake (a push before it was made found none, and
/// counted as done). The computer is made whatever this answers; a push
/// not queued fails the making, so its caller makes it again (the same
/// one: `POST /api/computers` is idempotent), which asks again.
pub(crate) async fn computer_made(env: &Env, person: &str) -> CellResult<()> {
    ask_registry(env, &SyncSeat { person: person.to_string() }).await.inspect_err(|e| {
        console_error!("{}", serde_json::json!({ "seat-sync": person, "failed": e.message }));
    })
}
