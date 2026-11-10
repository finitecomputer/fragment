//! The finite.computer move (FIN-463; docs/migrations.md): one
//! `Migrations` Durable Object, named `migrations`, owns every old agent's
//! move. It holds the roster's rows, each move (`fragment_core::migrations`,
//! a pure machine it asks for every change, stored as plain columns), the
//! mover's mailbox and uploads, and the one clock: its alarm, which wakes
//! at the earliest deadline of any move. Everyone else is asked; nothing
//! else holds a copy.
//!
//! The router's half is here too: the mover's routes
//! (`/api/migrations/mover/*`, signed by the key the deployment's
//! `migrations.mover` names, and no other), the new agent's
//! (`/api/migrations/stages/*`: its own stage, while it imports), and the
//! operators' (`/api/admin/migrations`, the whole admin surface). After a
//! pass of its deadlines, the alarm sends what it queued: each routine that
//! starts an import, and the deletion of each stage done with. A deployment with no
//! `migrations` answers none of them. Removable whole (the debt ledger's
//! "The finite.computer move").
//!
//! Lean by structure (the cell's Wasm budget): the router reads no body of
//! the mover's but a complete's, and passes each on as text to one call of
//! the object's, named by its path (`migrations/<op>/<args>`), with no
//! envelope to encode or decode; the object answers synchronously and asks
//! no one; the router does what waits (the registry, R2 through the
//! export's own multipart shims in js.rs) in boxed steps, one copy each,
//! and passes the object's answers on unread.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use fragment_core::migrations::{self as machine, Col, Effect, Move, Refused, Step};
use fragment_nip98::Payload;
use fragment_proto::migrations::{
    parse_stage, stage_prefix, valid_old, AdminAct, Mailbox, MoveAt, MoveState, OfferYes, Report, Reported, RosterTaken, StageCompleted, StageDone, StagePart, StageParts, MAILBOX_MAX, MANIFEST_MAX_BYTES,
    PARTS_MAX, PART_MAX_BYTES, ROSTER_AGENTS_MAX, ROSTER_PEOPLE_MAX, STAGE_FORMAT, STAGE_MANIFEST, STAGE_ZIP, TEXT_MAX_BYTES,
};
use fragment_proto::notice::NoticeList;
use fragment_proto::{limits, ErrorCode, IdentityKind};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;

/// The one object's name.
pub(crate) const NAME: &str = "migrations";
/// The bucket stages are kept in (docs/migrations.md, "The stage"): the
/// platform's alone, through its binding.
pub(crate) const STAGE: &str = "STAGE";
/// Moves an operator's list answers at once.
const PAGE: usize = 100;
/// Moves one alarm pass looks at: the rest at the next, a second later.
const PASS_MAX: usize = 200;
const AGAIN_MS: i64 = 1_000;
/// The roster's word on one old agent (`RosterAgent`), kept whole for the
/// page that offers it, at most.
const OFFER_MAX_BYTES: usize = 64 * 1024;
/// Staff's acts (`AdminAct`'s `op`).
const ACTS: [&str; 4] = ["retry", "restart-old", "retire", "withdraw"];
/// Stages one alarm pass deletes, and the pages of keys of each: one, so
/// at most 1,000 objects a stage a pass (R2 lists and deletes 1,000 a
/// call; docs/migrations.md, "Deletion").
const SWEEPS_A_PASS: i64 = 4;
const SWEEP_PAGES: usize = 1;
/// Routines one pass sends, the tries each gets, and the wait after a
/// failed one (doubling).
const ROUTINES_A_PASS: i64 = 16;
const ROUTINE_TRIES: i64 = 5;
const ROUTINE_RETRY_MS: i64 = 30_000;
/// The header a move's routine into an agent's fragment carries
/// (routed.rs `marker`): the router never passes it. The routine goes as
/// `stage/routine/<key>`, its text the body: appended once by its key.
pub(crate) const ROUTINE_HEADER: &str = "x-fragment-move-routine";
/// A person's pages told of a change to their moves, at most, by one call
/// (a roster may change many): the rest read theirs as they reconnect.
const TELLS_MAX: usize = 32;
/// An old agent's picture, read for its person (`…/picture`): its bytes at
/// most, and the kinds a page shows (never SVG, which runs script).
const PICTURE_MAX_BYTES: usize = 1024 * 1024;
const PICTURE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
/// A person's notices when the move has none for them.
const NO_NOTICES: &[u8] = br#"{"notices":[]}"#;

/// A move's columns (`fragment_core::migrations::COLUMNS`), as SQL names
/// and binds them, and the queries built of them: whole at compile time.
macro_rules! columns {
    () => {
        "old, workos_id, state, reason, held_from, phase, attempt, agent, command, seq, since_ms, progress_ms, staged_ms, routine_ms, routines, opened, expires_ms, by_staff, generation, deadline_ms"
    };
}
macro_rules! marks {
    () => {
        "?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?"
    };
}
/// A move as staff read it: states and sizes, never a stage's bytes.
macro_rules! admin_row {
    () => {
        "SELECT m.old, m.workos_id, p.person, m.agent, m.state, m.reason, m.phase, m.attempt,
    CASE WHEN m.command IS NULL THEN NULL ELSE m.old || ':' || m.attempt || ':' || m.command END AS command,
    m.since_ms, m.deadline_ms, m.staged_ms FROM moves m LEFT JOIN people p ON p.workos_id = m.workos_id"
    };
}
/// The unasked, whom a roster withdraws and the cutoff archives.
macro_rules! unasked {
    () => {
        "state IN ('listed', 'open', 'later', 'never')"
    };
}
const INSERT_MOVE: &str = concat!("INSERT INTO moves (", columns!(), ", offer) VALUES (", marks!(), ", ?)");
const UPDATE_MOVE: &str = concat!("UPDATE moves SET (", columns!(), ") = (", marks!(), ") WHERE old = ?");
const ADMIN_PAGE: &str = concat!(admin_row!(), " WHERE m.old > ? ORDER BY m.old LIMIT ?");
const ADMIN_ONE: &str = concat!(admin_row!(), " WHERE m.old = ?");
const UNASKED_ALL: &str = concat!("SELECT * FROM moves WHERE ", unasked!());
const UNASKED_FIRST: &str = concat!("SELECT * FROM moves WHERE ", unasked!(), " ORDER BY old LIMIT ?");
const UNASKED_COUNT: &str = concat!("SELECT COUNT(*) AS n FROM moves WHERE ", unasked!());

// ---- the router's half

/// A step of the router's that waits, boxed: awaited by reference, so its
/// code is one copy however many routes take it.
type Waits<'a, T> = Pin<Box<dyn Future<Output = CellResult<T>> + 'a>>;

fn not_mover() -> CellError {
    CellError::new(ErrorCode::Forbidden, "only the deployment's mover (migrations.mover) calls this")
}

/// `/api/migrations/…`: the mover's routes, its signature checked against
/// the deployment's `migrations.mover` before anything is done.
pub(crate) async fn route(mut req: Request, env: &Env, cfg: &Config, url: &Url, rest: &[&str]) -> CellResult<Response> {
    let Some(m) = &cfg.migrations else { return Err(crate::no_route(url.path())) };
    if let (Method::Put, ["mover", "stages", stage, "parts", n]) = (req.method(), rest) {
        return part(req, env, url, &m.mover, stage, n).await;
    }
    if let ["stages", stage, op] = rest {
        return stage_route(req, env, url, stage, op).await;
    }
    if let ["offers", old, op] = rest {
        return offer_route(req, env, cfg, url, old, op).await;
    }
    let ["mover", tail @ ..] = rest else { return Err(crate::no_route(url.path())) };
    let bytes = crate::read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    if crate::authenticate(&req, url, Payload::Read(&bytes))? != m.mover {
        return Err(not_mover());
    }
    let body = text(bytes)?;
    let op = match (req.method(), tail) {
        (Method::Post, ["stages", stage, "complete"]) => return complete(env, stage, body).await,
        (Method::Put, ["roster"]) => "roster".to_string(),
        (Method::Get, ["mailbox"]) => "mailbox".to_string(),
        (Method::Post, ["agents", old, "report"]) => {
            old_ok(old)?;
            format!("report/{old}")
        }
        (Method::Get, ["stages", stage, "parts"]) => format!("parts/{stage}"),
        _ => return Err(crate::no_route(url.path())),
    };
    pass(env, op, body).await
}

/// `/api/admin/migrations…`, for an operator (`by`, checked by the caller):
/// the moves, a page at a time (`GET`), and staff's act on one (`POST
/// …/{old}` `AdminAct {op}`).
pub(crate) async fn admin(env: &Env, cfg: &Config, by: &str, method: Method, rest: &[&str], url: &Url, body: &[u8]) -> CellResult<Response> {
    if cfg.migrations.is_none() {
        return Err(crate::no_route(url.path()));
    }
    let op = match (method, rest) {
        (Method::Get, []) => {
            let after = query(url, "after");
            if !after.is_empty() {
                old_ok(&after)?;
            }
            format!("list/{after}")
        }
        (Method::Post, [old]) => {
            old_ok(old)?;
            let AdminAct { op } = crate::json_body(body)?;
            if !ACTS.contains(&op.as_str()) {
                return Err(CellError::invalid(format!("an act is one of {}", ACTS.join(", "))));
            }
            format!("act/{old}/{op}")
        }
        (m, _) => return Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/admin/migrations/{}", m.as_ref(), rest.join("/")))),
    };
    pass(env, op, by.to_string()).await
}

/// `POST /api/test/migrations`: the levers the `move` lane needs and no
/// route gives: the object's clock (`clock {offsetMs}`, its supervisor
/// run then), a person's answers until their page lands (`answer {old,
/// answer, agent?}`, `visit {who}`: an email or npub), and what `STAGE`
/// holds under a prefix (`objects {prefix}`: its keys, never its bytes).
pub(crate) async fn lever(env: &Env, cfg: &Config, body: &[u8]) -> CellResult<Response> {
    if cfg.migrations.is_none() {
        return Err(crate::no_route("/api/test/migrations"));
    }
    let mut v: Value = crate::json_body(body)?;
    let who = |w: &Value| {
        let who = w.as_str().unwrap_or_default().to_string();
        async move { crate::ask_registry(env, &crate::registry::calls::Lookup { who }).await }
    };
    match v["op"].as_str() {
        Some("objects") => {
            let (keys, _) = js::blob_list(env, STAGE, v["prefix"].as_str().unwrap_or_default()).await?;
            return crate::json_answer(&json!({ "keys": keys }));
        }
        Some("visit") => return visit(env, cfg, &who(&v["who"]).await?.id).await,
        // Yes binds an agent of the person's own: whose it is, the registry's
        Some("answer") if v["answer"] == "yes" => {
            let agent = who(&v["agent"]).await?;
            v["agent"] = json!(agent.id);
            v["owner"] = json!(agent.owner);
        }
        _ => {}
    }
    pass(env, "test".into(), v.to_string()).await
}

/// A person opening the move (F6's page; until it lands, the levers'
/// `visit`): the old agents of the WorkOS user they signed in as, bound to
/// them. Who they are is the registry's, asked then, not copied ahead.
pub(crate) async fn visit(env: &Env, cfg: &Config, person: &str) -> CellResult<Response> {
    let subject = crate::connections::workos_user(env, cfg, person).await?.ok_or_else(|| CellError::new(ErrorCode::Conflict, "this person signed in with no WorkOS user"))?;
    // a WorkOS user's id is of the old ids' alphabet: a path segment as it is
    if !valid_old(&subject) {
        return Err(CellError::host("a WorkOS user's id is not one"));
    }
    let bound: Bound = crate::json_body(&ask(env, format!("bind/{subject}"), person.to_string()).await?)?;
    if let Some(until_ms) = bound.until_ms {
        comp(env, person, &subject, until_ms).await?;
    }
    crate::json_answer(&bound)
}

/// The move's comp (F5), once a person: its credit granted on their ledger
/// by its id (`comp_grant_id`: the same id again grants nothing), then a
/// month's seat comped (one they hold already, theirs, is left as it is),
/// then Migrations told, so no visit gives it again. Cut off anywhere, the
/// next visit does it again, each step answering as it did: the seat asked
/// to end at `until_ms`, fixed at the first attempt (one past it, none).
async fn comp(env: &Env, person: &str, subject: &str, until_ms: i64) -> CellResult<()> {
    let grant = fragment_proto::ledger::GrantCredit {
        id: machine::comp_grant_id(person),
        micros: machine::COMP_GRANT_MICROS,
        by: NAME.into(),
        why: "the finite.computer move's credit".into(),
    };
    crate::ledger::ask(env, person, &grant).await?;
    let profiles = crate::ask_registry(env, &crate::registry::calls::Profiles { ids: vec![person.to_string()], emails_of: vec![person.to_string()] }).await?;
    let email = profiles.profiles.get(person).and_then(|p| p.email.clone()).ok_or_else(|| CellError::host(format!("{person} has no email to comp")))?;
    let comp = fragment_proto::org::CompSeat { email, kind: fragment_proto::org::SeatKind::Seat, org: None, until_ms: Some(until_ms) };
    let asked = match until_ms > js::now_ms() {
        true => crate::ask_registry(env, &crate::registry::orgs::CompSeatCall { by: NAME.into(), comp }).await.map(|_| ()),
        false => Ok(()),
    };
    match asked {
        // a seat of theirs already (paid, or another kind): theirs, as it is
        Err(e) if matches!(e.code, ErrorCode::Conflict | ErrorCode::AlreadyExists) => console_log!("migrations.comp: {person} holds a seat: {}", e.message),
        r => {
            r?;
        }
    }
    ask(env, format!("comped/{subject}"), String::new()).await?;
    Ok(())
}

/// `GET /api/notices`, the move's part (`notices::route`): `person`'s open
/// moves, each an offer that must be answered. A person it does not know
/// yet is bound first, by the WorkOS user they sign in as (asked of the
/// registry once: then they are known, roster or not, and a roster after
/// offers their rows at once). One with no WorkOS user, or whose user is
/// another person's here, is told nothing. Its bytes, a `NoticeList`.
pub(crate) async fn notices(env: &Env, cfg: &Config, person: &str) -> CellResult<Vec<u8>> {
    if cfg.migrations.is_none() {
        return Ok(NO_NOTICES.to_vec());
    }
    let told = ask(env, "notices".into(), person.to_string()).await?;
    if !told.is_empty() {
        return Ok(told);
    }
    match crate::connections::workos_user(env, cfg, person).await? {
        Some(subject) if valid_old(&subject) => ask(env, "notices".into(), format!("{person}\n{subject}")).await,
        _ => Ok(NO_NOTICES.to_vec()),
    }
}

/// `/api/migrations/offers/{old}/…`: the person's own answers to an offer
/// (`later`, `never`, `ask-again`; `yes {agent}`, an agent of their own,
/// from `/move`), and its picture, read for them. The object says whether
/// the move is theirs.
fn offer_route<'a>(mut req: Request, env: &'a Env, cfg: &'a Config, url: &'a Url, old: &'a str, op: &'a str) -> Waits<'a, Response> {
    Box::pin(async move {
        let bytes = crate::read_body(&mut req, limits::BODY_MAX_BYTES).await?;
        let who = crate::signer(env, &req, url, &bytes).await?.identity;
        if who.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "an offer is its person's to answer"));
        }
        old_ok(old)?;
        match (req.method(), op) {
            (Method::Get, "picture") => picture(cfg, text(ask(env, format!("picture/{old}"), who.id).await?)?).await,
            (Method::Post, "later" | "never" | "ask-again") => pass(env, format!("answer/{old}/{op}"), who.id).await,
            (Method::Post, "yes") => {
                let OfferYes { agent } = crate::json_body(&bytes)?;
                let agent = crate::ask_registry(env, &crate::registry::calls::Lookup { who: agent }).await?;
                let owner = agent.owner.filter(|_| agent.kind == IdentityKind::Agent).unwrap_or_default();
                pass(env, format!("answer/{old}/yes"), format!("{}\n{}\n{owner}", who.id, agent.id)).await
            }
            _ => Err(crate::no_route(url.path())),
        }
    })
}

/// An old agent's picture, as the roster names it, read for its person:
/// https alone (http too on a local fleet, whose pictures are its own), an
/// image a page shows, within `PICTURE_MAX_BYTES`.
async fn picture(cfg: &Config, at: String) -> CellResult<Response> {
    let none = || CellError::new(ErrorCode::NotFound, "this old agent has no picture to show");
    let local = cfg.test_hooks && !cfg.levers_scoped;
    if !(at.starts_with("https://") || (local && at.starts_with("http://"))) {
        return Err(none());
    }
    let mut resp = Fetch::Url(Url::parse(&at).map_err(|_| none())?).send().await.map_err(|e| CellError::new(ErrorCode::UpstreamFailed, format!("the picture did not answer: {e}")))?;
    let kind = resp.headers().get("content-type")?.unwrap_or_default();
    let kind = kind.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    if resp.status_code() != 200 || !PICTURE_TYPES.contains(&kind.as_str()) {
        return Err(CellError::new(ErrorCode::UpstreamFailed, format!("the picture answered {} ({kind})", resp.status_code())));
    }
    let (bytes, cut) = crate::cs::read_answer(&mut resp, PICTURE_MAX_BYTES).await?;
    if cut {
        return Err(CellError::too_large("a picture", PICTURE_MAX_BYTES + 1, PICTURE_MAX_BYTES));
    }
    let h = Headers::new();
    h.set("content-type", &kind)?;
    // kept by no cache: its URL is the same for whoever signs in next on
    // this browser, and the shell reads it with its own header anyway
    h.set("cache-control", "no-store")?;
    h.set("x-content-type-options", "nosniff")?;
    Ok(Response::from_bytes(bytes)?.with_headers(h))
}

/// One call into the object (`migrations/<op>`, `body` its text), asked
/// again while it refuses this release (it did nothing then): its answer's
/// bytes, or its refusal.
#[inline(never)]
fn ask(env: &Env, op: String, body: String) -> Waits<'_, Vec<u8>> {
    Box::pin(async move { crate::routed::object_answer(env, "MIGRATIONS", NAME, &format!("migrations/{op}"), body).await })
}

/// `ask`, its answer passed on as it is, unread.
#[inline(never)]
fn pass(env: &Env, op: String, body: String) -> Waits<'_, Response> {
    Box::pin(async move { crate::json_passed(ask(env, op, body).await?) })
}

/// `/api/migrations/stages/{stage}/…`: a new agent's own stage, signed as
/// itself (its computer's egress signs so). The object says whether the
/// stage is that agent's to read now: only the agent its person's Yes
/// bound, while the move imports it. `open` answers its manifest; `zip`
/// its bytes, whole or by a range; `done` ends the import.
fn stage_route<'a>(mut req: Request, env: &'a Env, url: &'a Url, stage: &'a str, op: &'a str) -> Waits<'a, Response> {
    Box::pin(async move {
        let bytes = crate::read_body(&mut req, limits::BODY_MAX_BYTES).await?;
        let agent = crate::signer(env, &req, url, &bytes).await?.identity.id;
        match (req.method(), op) {
            (Method::Post, "open") => pass(env, format!("open/{stage}/{agent}"), String::new()).await,
            (Method::Get, "zip") => {
                let key = text(ask(env, format!("readable/{stage}/{agent}"), String::new()).await?)?;
                let range = req.headers().get("range")?;
                let zip = js::blob_get(env, STAGE, &key, range.as_deref()).await?.ok_or_else(|| CellError::host(format!("stage {stage} imports, and its zip is gone")))?;
                crate::blobs::ranged(zip, "application/zip", None)
            }
            (Method::Post, "done") => pass(env, format!("done/{stage}/{agent}"), text(bytes)?).await,
            _ => Err(crate::no_route(url.path())),
        }
    })
}

/// One part of a stage's zip: the object says whether the mover may upload
/// this stage now, and where (its upload made here once, the first part's);
/// the request's own stream goes to R2 unread; the object takes the part.
/// A refusal after the signature waits for the bytes it did not read.
fn part<'a>(req: Request, env: &'a Env, url: &'a Url, mover: &'a str, stage: &'a str, n: &'a str) -> Waits<'a, Response> {
    Box::pin(async move {
        let sha = query(url, "sha256");
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(CellError::invalid("a part's sha256 is 64 lowercase hex"));
        }
        // its signed URL names its bytes' hash
        if crate::authenticate(&req, url, Payload::Streamed { sha256_hex: &sha })? != mover {
            return Err(not_mover());
        }
        let answered = async {
            let n = n.parse::<u16>().ok().filter(|n| (1..=PARTS_MAX).contains(&u32::from(*n))).ok_or_else(|| CellError::invalid(format!("a part's number is 1 to {PARTS_MAX}")))?;
            let bytes = req.headers().get("content-length")?.and_then(|l| l.parse::<u64>().ok()).unwrap_or(0);
            if bytes == 0 || bytes > PART_MAX_BYTES {
                return Err(CellError::invalid(format!("a part declares its content-length, 1 to {PART_MAX_BYTES} bytes")));
            }
            // its key, then its upload (none yet: this part makes it)
            let at = text(ask(env, format!("upload/{stage}"), String::new()).await?)?;
            let (key, mut upload) = at.split_once('\n').map(|(k, u)| (k.to_string(), u.to_string())).ok_or_else(|| CellError::host("an upload's place"))?;
            if upload.is_empty() {
                let made = js::upload_start(env, STAGE, &key, "application/zip").await?;
                let kept = text(ask(env, format!("upload/{stage}"), made.clone()).await?)?;
                upload = kept.split_once('\n').map(|(_, u)| u.to_string()).unwrap_or_default();
                // another part made one first: this one goes
                if upload != made {
                    js::upload_abort(env, STAGE, &key, &made).await?;
                }
            }
            let body = req.inner().body().ok_or_else(|| CellError::invalid("a part has a body"))?;
            let etag = js::upload_part(env, STAGE, &key, &upload, n, body).await?;
            pass(env, format!("part/{stage}/{n}/{bytes}/{sha}"), etag).await
        }
        .await;
        if answered.is_err() {
            js::drain(&req).await?;
        }
        answered
    })
}

/// The stage made whole from its parts, then its manifest written, last:
/// the object checks the manifest and says which parts (`Migrations::
/// complete`'s lines), R2 makes the zip, and the object is told it is done.
fn complete<'a>(env: &'a Env, stage: &'a str, body: String) -> Waits<'a, Response> {
    Box::pin(async move {
        #[derive(Deserialize)]
        struct Complete<'b> {
            #[serde(borrow)]
            manifest: &'b RawValue,
        }
        let c: Complete = serde_json::from_slice(body.as_bytes()).map_err(|e| CellError::invalid(format!("body: {e}")))?;
        let manifest = c.manifest.get();
        if manifest.len() > MANIFEST_MAX_BYTES {
            return Err(CellError::too_large("a manifest", manifest.len(), MANIFEST_MAX_BYTES));
        }
        let at = text(ask(env, format!("complete/{stage}"), manifest.to_string()).await?)?;
        let mut lines = at.lines();
        let mut line = || lines.next().unwrap_or_default();
        let (prefix, upload, zip_bytes, done) = (line(), line(), line(), line());
        let zip_bytes = zip_bytes.parse::<u64>().map_err(|_| CellError::host("a complete's size"))?;
        if !done.is_empty() {
            return crate::json_answer(&StageCompleted { manifest_sha256: done.into(), zip_bytes });
        }
        let parts = lines.map(|l| l.split_once(' ').and_then(|(n, etag)| Some((n.parse::<u16>().ok()?, etag.to_string())))).collect::<Option<Vec<_>>>().ok_or_else(|| CellError::host("a complete's parts"))?;
        // a complete cut off between the zip and its manifest is not done
        // again: its attempt stalls, and the next stages afresh
        let size = js::upload_complete(env, STAGE, &format!("{prefix}{STAGE_ZIP}"), upload, &parts).await?;
        if size != zip_bytes {
            return Err(CellError::host(format!("the zip made is {size} bytes, its parts {zip_bytes}")));
        }
        // the manifest last: its presence is what staged means
        js::blob_put_bytes(env, STAGE, &format!("{prefix}{STAGE_MANIFEST}"), manifest.as_bytes()).await?;
        let sha = fragment_core::blob::sha256_hex(manifest.as_bytes());
        pass(env, format!("completed/{stage}/{sha}/{size}"), manifest.to_string()).await
    })
}

fn query(url: &Url, key: &str) -> String {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).unwrap_or_default()
}

fn text(bytes: Vec<u8>) -> CellResult<String> {
    String::from_utf8(bytes).map_err(|_| CellError::invalid("body: not UTF-8"))
}

fn old_ok(old: &str) -> CellResult<()> {
    match valid_old(old) {
        true => Ok(()),
        false => Err(CellError::invalid("an old agent's id is 1 to 64 of A-Z, a-z, 0-9, _ and -")),
    }
}

fn text_ok(what: &str, s: &str) -> CellResult<()> {
    match s.len() <= TEXT_MAX_BYTES {
        true => Ok(()),
        false => Err(CellError::too_large(what, s.len(), TEXT_MAX_BYTES)),
    }
}

/// A refusal of the machine's, as the route answers it.
fn refused(r: Refused) -> CellError {
    let code = match r {
        Refused::NotNow(_) | Refused::Stale => ErrorCode::Conflict,
        Refused::NotYours => ErrorCode::Forbidden,
        Refused::NoStage => ErrorCode::NotFound,
        Refused::Unfit(_) => ErrorCode::InvalidRequest,
    };
    CellError::new(code, r.message())
}

// ---- the object

/// A roster as the object reads it: its ids, and each agent's offer kept as
/// it came (the page that offers it reads that).
#[derive(Deserialize)]
struct RosterIn {
    snapshot: String,
    people: Vec<PersonIn>,
}

#[derive(Deserialize)]
struct PersonIn {
    workos_id: String,
    email: String,
    agents: Vec<Box<RawValue>>,
}

#[derive(Deserialize)]
struct AgentIn {
    old: String,
}

/// A bind's answer (`visit`): how many moves it offered, whether its
/// person is owed the comp, and when its seat ends (fixed at its first
/// attempt: `comp_due`).
#[derive(serde::Serialize, Deserialize)]
struct Bound {
    bound: u32,
    comp: bool,
    #[serde(default, rename = "untilMs", skip_serializing_if = "Option::is_none")]
    until_ms: Option<i64>,
}

/// An offer as its notice reads it (`RosterAgent`'s name and picture).
#[derive(Deserialize)]
struct OfferIn {
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture_url: Option<String>,
}

/// A manifest as the object checks it (the importer reads the rest).
#[derive(Deserialize)]
struct ManifestIn {
    format: String,
    old: String,
    phase: String,
    attempt: u32,
    zip_bytes: u64,
}

/// A roster within its bounds, each WorkOS id and old id once: its people,
/// and each one's agents (old id, offer).
type Agents = Vec<Vec<(String, String)>>;

fn read_roster(text: &str) -> CellResult<(RosterIn, Agents)> {
    let r: RosterIn = crate::json_body(text.as_bytes())?;
    text_ok("snapshot", &r.snapshot)?;
    if r.snapshot.is_empty() || r.people.len() > ROSTER_PEOPLE_MAX {
        return Err(CellError::invalid(format!("a roster names its snapshot, and at most {ROSTER_PEOPLE_MAX} people")));
    }
    let mut agents = vec![];
    for p in &r.people {
        text_ok("workos_id", &p.workos_id)?;
        text_ok("email", &p.email)?;
        if p.workos_id.is_empty() || p.agents.len() > ROSTER_AGENTS_MAX {
            return Err(CellError::invalid(format!("a WorkOS id is not empty, with at most {ROSTER_AGENTS_MAX} old agents")));
        }
        let mut theirs = vec![];
        for a in &p.agents {
            let offer = a.get();
            let AgentIn { old } = crate::json_body(offer.as_bytes())?;
            if !valid_old(&old) || offer.len() > OFFER_MAX_BYTES {
                return Err(CellError::invalid(format!("an old agent's id is 1 to 64 of A-Z, a-z, 0-9, _ and -, its offer at most {OFFER_MAX_BYTES} bytes")));
            }
            theirs.push((old, offer.to_string()));
        }
        agents.push(theirs);
    }
    // each person and old agent once: an old agent under two people is
    // whose? (refused, not guessed)
    let people: Vec<&str> = r.people.iter().map(|p| p.workos_id.as_str()).collect();
    let olds: Vec<&str> = agents.iter().flatten().map(|(old, _)| old.as_str()).collect();
    for mut ids in [people, olds] {
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(CellError::invalid("a roster names each WorkOS id and old agent once"));
        }
    }
    Ok((r, agents))
}

#[durable_object]
pub struct MigrationsCell {
    state: State,
    env: Env,
    raw: JsValue,
    cfg: &'static Config,
    schema: crate::schema::Schema,
    /// The WorkOS users whose moves this call changed: their people's
    /// pages are told after it (`tell`).
    changed: Rc<RefCell<Vec<String>>>,
}

impl DurableObject for MigrationsCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let schema = crate::schema::Schema::start(&raw, state.storage().sql(), &fragment_core::migrate::MIGRATIONS, &state.id().to_string());
        let cfg = Config::from_env(&env);
        MigrationsCell { state, env, raw, cfg, schema, changed: Rc::default() }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        if let Some(refused) = crate::routed::refused(&req, &self.cfg.deploy_id)? {
            return Ok(refused);
        }
        if let Some(probed) = crate::routed::probed(&req, &self.schema)? {
            return Ok(probed);
        }
        match self.route(req).await {
            Ok(answer) => Ok(answer),
            Err(e) => e.response(),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if self.schema.refused_alarm(&self.state, None).await? {
            return Response::ok("");
        }
        let ran = match self.supervise() {
            Ok(()) => self.outboxes().await,
            Err(e) => Err(e),
        };
        if let Err(e) = ran {
            console_error!("migrations.alarm-failed: {}", e.message);
            // looked at again in a minute
            let _ = js::set_alarm(&self.raw, js::now_ms() + 60_000);
        }
        self.tell().await;
        Response::ok("")
    }
}

impl MigrationsCell {
    fn parts(&self) -> Parts {
        Parts { sql: self.state.storage().sql(), changed: self.changed.clone() }
    }

    /// The object's clock: real time, moved by the `clock` lever on a fleet
    /// with levers.
    fn now(&self) -> CellResult<i64> {
        let offset = match self.cfg.test_hooks {
            true => self.parts().meta("clock_offset")?.and_then(|o| o.parse::<i64>().ok()).unwrap_or(0),
            false => 0,
        };
        Ok(js::now_ms() + offset)
    }

    /// Changes, each stored with its effects queued, in one transaction.
    fn commit(&self, steps: Vec<Step>, now: i64) -> CellResult<()> {
        if steps.is_empty() {
            return Ok(());
        }
        let parts = self.parts();
        js::transaction_sync(&self.raw, move || steps.iter().try_for_each(|s| parts.apply(s, now)))
    }

    /// Its one route: a call, named by its path, its body as text,
    /// answered at once (it asks no one), and its alarm set to its earliest
    /// deadline after.
    async fn route(&self, mut req: Request) -> CellResult<Response> {
        self.schema.ready()?;
        let body = req.text().await?;
        let answer = self.call(&req.path(), body);
        self.arm()?;
        self.tell().await;
        answer
    }

    /// The people whose moves changed, told (`principal::tell_changed`), so
    /// each open page reads its notices again: at most `TELLS_MAX`, best
    /// effort (a page that missed it reads them as it reconnects).
    async fn tell(&self) {
        let mut workos: Vec<String> = self.changed.borrow_mut().drain(..).collect();
        workos.sort_unstable();
        workos.dedup();
        let parts = self.parts();
        let mut people: Vec<String> = workos.iter().filter_map(|w| parts.person_of(w).ok().flatten()).collect();
        people.sort_unstable();
        people.dedup();
        for person in people.iter().take(TELLS_MAX) {
            if let Err(e) = crate::principal::tell_changed(&self.env, person).await {
                console_error!("migrations.tell-failed: {person}: {}", e.message);
            }
        }
    }

    fn call(&self, path: &str, body: String) -> CellResult<Response> {
        let parts = self.parts();
        let number = |n: &str| n.parse::<u64>().map_err(|_| CellError::invalid(format!("{n} is no number")));
        let at: Vec<&str> = path.split('/').collect();
        let [_, "migrations", op, args @ ..] = &at[..] else { return Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))) };
        match (*op, args) {
            ("roster", []) => crate::json_answer(&self.roster(&body)?),
            ("bind", [workos_id]) => {
                let bound = self.bind(workos_id, &body)?;
                let until_ms = self.comp_due(workos_id)?;
                crate::json_answer(&Bound { bound, comp: until_ms.is_some(), until_ms })
            }
            // the comp given (F5): never again
            ("comped", [workos_id]) => {
                parts.exec("UPDATE people SET comped_ms = COALESCE(comped_ms, ?) WHERE workos_id = ?", vec![SqlStorageValue::Integer(self.now()?), text_value(workos_id)])?;
                Ok(Response::ok("")?)
            }
            ("mailbox", []) => crate::json_answer(&self.mailbox()?),
            ("report", [old]) => crate::json_answer(&self.report(old, &crate::json_body(body.as_bytes())?)?),
            ("list", [after]) => {
                let rows = parts.rows(ADMIN_PAGE, vec![text_value(after), SqlStorageValue::Integer(PAGE as i64 + 1)])?;
                let next = (rows.len() > PAGE).then(|| rows[PAGE - 1]["old"].clone());
                crate::json_answer(&json!({ "moves": &rows[..rows.len().min(PAGE)], "next": next }))
            }
            ("act", [old, act]) => {
                self.act(old, act)?;
                console_log!("migrations.staff: {act} {old} by {body}");
                crate::json_answer(&parts.rows(ADMIN_ONE, vec![text_value(old)])?.pop())
            }
            // where its zip goes, and its upload: the one the router made
            // (the body) held, unless one is already
            ("upload", [stage]) => {
                let m = self.may_upload(stage)?;
                if !body.is_empty() {
                    parts.exec("INSERT INTO uploads (stage, upload_id, created_ms) VALUES (?, ?, ?) ON CONFLICT (stage) DO NOTHING", vec![text_value(stage), text_value(&body), SqlStorageValue::Integer(self.now()?)])?;
                }
                let upload = parts.rows("SELECT upload_id FROM uploads WHERE stage = ?", vec![text_value(stage)])?.first().and_then(|r| r["upload_id"].as_str().map(str::to_string)).unwrap_or_default();
                Ok(Response::ok(format!("{}{STAGE_ZIP}\n{upload}", stage_prefix(m.phase, &m.old, m.attempt)))?)
            }
            ("part", [stage, n, bytes, sha256]) => {
                self.may_upload(stage)?;
                let part = StagePart { n: number(n)? as u32, bytes: number(bytes)?, sha256: sha256.to_string(), etag: body };
                parts.exec(
                    "INSERT INTO parts (stage, n, bytes, sha256, etag) VALUES (?, ?, ?, ?, ?) ON CONFLICT (stage, n) DO UPDATE SET bytes = excluded.bytes, sha256 = excluded.sha256, etag = excluded.etag",
                    vec![text_value(stage), SqlStorageValue::Integer(i64::from(part.n)), SqlStorageValue::Integer(part.bytes as i64), text_value(&part.sha256), text_value(&part.etag)],
                )?;
                crate::json_answer(&part)
            }
            ("parts", [stage]) => {
                self.may_upload(stage)?;
                crate::json_answer(&StageParts { parts: parts.stage_parts(stage)? })
            }
            ("complete", [stage]) => Ok(Response::ok(self.complete(stage, &body)?)?),
            // the manifest written (the body), kept for the new agent's open
            ("completed", [stage, sha, bytes]) => {
                self.may_upload(stage)?;
                let done = StageCompleted { manifest_sha256: sha.to_string(), zip_bytes: number(bytes)? };
                parts.exec("UPDATE uploads SET manifest_sha256 = ?, zip_bytes = ?, manifest = ? WHERE stage = ?", vec![text_value(sha), SqlStorageValue::Integer(done.zip_bytes as i64), text_value(&body), text_value(stage)])?;
                crate::json_answer(&done)
            }
            // the new agent's reads, each its own stage's, while it imports
            // `StageOpened`: the stage's id (of the old ids' alphabet, checked
            // by `of_stage`) and the manifest as its complete read it whole,
            // JSON as it was written: set in place, read again by no one here
            ("open", [stage, agent]) => {
                let m = self.of_stage(stage)?;
                if let Some(s) = machine::open(&m, agent, stage).map_err(refused)? {
                    self.commit(vec![s], self.now()?)?;
                }
                let manifest = parts.rows("SELECT manifest FROM uploads WHERE stage = ?", vec![text_value(stage)])?.first().and_then(|r| r["manifest"].as_str().map(str::to_string)).ok_or_else(|| CellError::host(format!("stage {stage} imports, and its manifest is gone")))?;
                crate::json_passed(format!("{{\"stage\":\"{stage}\",\"manifest\":{manifest}}}").into_bytes())
            }
            ("readable", [stage, agent]) => {
                let m = self.of_stage(stage)?;
                machine::readable(&m, agent, stage).map_err(refused)?;
                Ok(Response::ok(format!("{}{STAGE_ZIP}", stage_prefix(m.phase, &m.old, m.attempt)))?)
            }
            ("done", [stage, agent]) => {
                let done: StageDone = crate::json_body(body.as_bytes())?;
                if let Some(why) = &done.why {
                    text_ok("why", why)?;
                }
                let m = self.of_stage(stage)?;
                let now = self.now()?;
                let state = match machine::done(&m, agent, stage, &done, now).map_err(refused)? {
                    None => m.state,
                    Some(s) => {
                        let state = s.to.state;
                        self.commit(vec![s], now)?;
                        state
                    }
                };
                crate::json_answer(&MoveAt { state })
            }
            // a person's: their open moves' offers (none known: empty, for the
            // router to bind them by their WorkOS user, the body's 2nd line)
            ("notices", []) => self.notices(&body),
            // a person's answer (body: who; a Yes's agent and its owner after)
            ("answer", [old, op]) => {
                let mut lines = body.lines();
                let person = lines.next().unwrap_or_default();
                let agent = lines.next().zip(lines.next());
                let (state, _) = self.answer(old, Some(person), op, agent)?;
                crate::json_answer(&MoveAt { state })
            }
            // where its picture is, for its person alone
            ("picture", [old]) => {
                let (_, offer) = self.offer_of(old, &body)?;
                Ok(Response::ok(offer.picture_url.unwrap_or_default())?)
            }
            ("test", []) if self.cfg.test_hooks => self.test(crate::json_body(body.as_bytes())?),
            _ => Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))),
        }
    }

    /// A roster, whole: new rows listed (or open, their person bound here
    /// already), rows the last listed and this does not withdrawn unless
    /// asked, and rows withdrawn by an earlier roster listed again.
    fn roster(&self, roster: &str) -> CellResult<RosterTaken> {
        let (r, agents) = read_roster(roster)?;
        let parts = self.parts();
        if parts.meta("snapshot")?.as_deref() == Some(r.snapshot.as_str()) {
            return Ok(RosterTaken { bound: 0, withdrawn: 0, replay: true });
        }
        let now = self.now()?;
        js::transaction_sync(&self.raw, move || {
            let (mut bound, mut withdrawn) = (0u32, 0u32);
            let mut listed = std::collections::HashSet::new();
            for (p, theirs) in r.people.iter().zip(agents) {
                let person = parts.person_of(&p.workos_id)?;
                parts.exec("INSERT INTO people (workos_id, email) VALUES (?, ?) ON CONFLICT (workos_id) DO UPDATE SET email = excluded.email", vec![text_value(&p.workos_id), text_value(&p.email)])?;
                for (old, offer) in theirs {
                    let step = match parts.get(&old)? {
                        None => {
                            parts.insert(&Move::listed(&old, &p.workos_id, person.is_some(), now), &offer)?;
                            bound += u32::from(person.is_some());
                            None
                        }
                        Some(m) => {
                            parts.exec("UPDATE moves SET offer = ? WHERE old = ?", vec![text_value(&offer), text_value(&old)])?;
                            machine::relisted(&m, person.is_some(), now).or_else(|| person.as_ref().and_then(|_| machine::bind(&m, now)))
                        }
                    };
                    if let Some(s) = step {
                        bound += u32::from(s.to.state == MoveState::Open);
                        parts.apply(&s, now)?;
                    }
                    listed.insert(old);
                }
            }
            // a move it no longer names keeps its state (an archived one waits
            // for a late Yes); one unasked is withdrawn
            for m in parts.moves(UNASKED_ALL, vec![])? {
                if !listed.contains(&m.old) {
                    if let Some(s) = machine::withdrawn(&m, now) {
                        withdrawn += 1;
                        parts.apply(&s, now)?;
                    }
                }
            }
            parts.set_meta("snapshot", &r.snapshot)?;
            Ok(RosterTaken { bound, withdrawn, replay: false })
        })
    }

    /// A person opening the move: the WorkOS user's rows bound to them (a
    /// user is one person's: another's is refused), each listed move
    /// offered. How many were.
    fn bind(&self, workos_id: &str, person: &str) -> CellResult<u32> {
        let parts = self.parts();
        if parts.person_of(workos_id)?.is_some_and(|p| p != person) {
            return Err(CellError::new(ErrorCode::Conflict, "this WorkOS user is another person's here"));
        }
        let (now, person) = (self.now()?, person.to_string());
        let workos_id = workos_id.to_string();
        js::transaction_sync(&self.raw, move || {
            // before its roster, too: the roster's rows come in bound
            parts.exec("INSERT INTO people (workos_id, email, person) VALUES (?, '', ?) ON CONFLICT (workos_id) DO UPDATE SET person = excluded.person", vec![text_value(&workos_id), text_value(&person)])?;
            let mut bound = 0;
            for m in parts.moves("SELECT * FROM moves WHERE workos_id = ? AND state = 'listed'", vec![text_value(&workos_id)])? {
                if let Some(s) = machine::bind(&m, now) {
                    bound += 1;
                    parts.apply(&s, now)?;
                }
            }
            Ok(bound)
        })
    }

    /// A person's open moves, oldest first, each as the offer that must be
    /// answered; `body` is who (`person`), then the WorkOS user to bind them
    /// by when the router asked. One not known here answers nothing at all.
    fn notices(&self, body: &str) -> CellResult<Response> {
        let (person, subject) = body.split_once('\n').unwrap_or((body, ""));
        let parts = self.parts();
        if !subject.is_empty() {
            match self.bind(subject, person) {
                Ok(_) => {}
                // their WorkOS user is another person's here: nothing to tell them
                Err(e) if e.code == ErrorCode::Conflict => {
                    console_error!("migrations.notices: {person}: {}", e.message);
                    return crate::json_passed(NO_NOTICES.to_vec());
                }
                Err(e) => return Err(e),
            }
            // this read answers what binding them offered
            self.changed.borrow_mut().clear();
        }
        if parts.rows("SELECT 1 AS n FROM people WHERE person = ? LIMIT 1", vec![text_value(person)])?.is_empty() {
            return Ok(Response::ok("")?);
        }
        let rows = parts.rows(
            "SELECT m.old, m.offer FROM moves m JOIN people p ON p.workos_id = m.workos_id WHERE p.person = ? AND m.state = 'open' ORDER BY m.since_ms, m.old",
            vec![text_value(person)],
        )?;
        let page = format!("{}/move", self.cfg.platform());
        let notices = rows
            .iter()
            .map(|r| {
                let o: OfferIn = crate::json_body(r["offer"].as_str().unwrap_or_default().as_bytes())?;
                Ok(machine::offer(r["old"].as_str().unwrap_or_default(), &o.name, o.picture_url.is_some(), &page))
            })
            .collect::<CellResult<Vec<_>>>()?;
        crate::json_answer(&NoticeList { notices })
    }

    /// `old`'s move and its roster offer, if it is `person`'s (404 otherwise:
    /// no one learns of another's).
    fn offer_of(&self, old: &str, person: &str) -> CellResult<(Move, OfferIn)> {
        let parts = self.parts();
        let none = || CellError::new(ErrorCode::NotFound, format!("no offer {old} of yours"));
        let m = parts.get(old)?.ok_or_else(none)?;
        if parts.person_of(&m.workos_id)?.as_deref() != Some(person) {
            return Err(none());
        }
        let offer = parts.rows("SELECT offer FROM moves WHERE old = ?", vec![text_value(old)])?.pop().and_then(|r| r["offer"].as_str().map(str::to_string)).unwrap_or_default();
        Ok((m, crate::json_body(offer.as_bytes())?))
    }

    /// A person's answer to `old`'s offer (`later`, `never`, `ask-again`,
    /// `yes` with an agent and its owner): `person` names who answers, its
    /// move theirs (404 otherwise); `None` is a lever's, for its person.
    /// A Yes binds an agent of the person's own alone (403). The move's
    /// state and attempt after.
    fn answer(&self, old: &str, person: Option<&str>, op: &str, agent: Option<(&str, &str)>) -> CellResult<(MoveState, u32)> {
        let parts = self.parts();
        let m = match person {
            Some(p) => self.offer_of(old, p)?.0,
            None => parts.get(old)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no old agent {old}")))?,
        };
        let theirs = parts.person_of(&m.workos_id)?.ok_or_else(|| CellError::new(ErrorCode::Conflict, "its person is not known here yet"))?;
        let now = self.now()?;
        let outcome = match (op, agent) {
            ("later", _) => machine::later(&m, now),
            ("never", _) => machine::never(&m, now),
            ("ask-again" | "ask_again", _) => machine::ask_again(&m, now),
            ("yes", Some((agent, owner))) if owner == theirs => machine::yes(&m, agent, now),
            ("yes", _) => return Err(CellError::new(ErrorCode::Forbidden, "yes binds an agent of the person's own")),
            (other, _) => return Err(CellError::invalid(format!("no answer {other}"))),
        };
        match outcome.map_err(refused)? {
            None => Ok((m.state, m.attempt)),
            Some(s) => {
                let at = (s.to.state, s.to.attempt);
                self.commit(vec![s], now)?;
                Ok(at)
            }
        }
    }

    /// When the move's comp (F5) the WorkOS user's person is owed ends, if
    /// they are owed it (an old agent the roster offers or moves, and not
    /// comped yet): fixed at its first attempt, so a visit cut off and
    /// tried again asks for the same end, never a later one.
    fn comp_due(&self, workos_id: &str) -> CellResult<Option<i64>> {
        let parts = self.parts();
        parts.exec(
            "UPDATE people SET comp_until_ms = COALESCE(comp_until_ms, ?) WHERE workos_id = ? AND comped_ms IS NULL AND EXISTS (SELECT 1 FROM moves m WHERE m.workos_id = people.workos_id AND m.state <> 'withdrawn')",
            // real time, as the registry that ends it keeps (not the levers' clock)
            vec![SqlStorageValue::Integer(js::now_ms() + machine::COMP_MS), text_value(workos_id)],
        )?;
        let rows = parts.rows("SELECT comp_until_ms FROM people WHERE workos_id = ? AND comped_ms IS NULL", vec![text_value(workos_id)])?;
        Ok(rows.first().and_then(|r| r["comp_until_ms"].as_i64()))
    }

    /// The mover's commands, oldest first.
    fn mailbox(&self) -> CellResult<Mailbox> {
        let moves = self.parts().moves("SELECT * FROM moves WHERE command IS NOT NULL ORDER BY since_ms, old LIMIT ?", vec![SqlStorageValue::Integer(MAILBOX_MAX as i64)])?;
        Ok(Mailbox { commands: moves.iter().filter_map(Move::command).collect() })
    }

    fn report(&self, old: &str, r: &Report) -> CellResult<Reported> {
        let parts = self.parts();
        let m = parts.get(old)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no old agent {old} in the move")))?;
        // staged means its stage is complete: its manifest written, the one reported
        if r.kind == "staged" && m.command_id().as_deref() == Some(r.command.as_str()) && r.seq > m.seq {
            let stage = m.stage().unwrap_or_default();
            let written = parts.rows("SELECT manifest_sha256 FROM uploads WHERE stage = ?", vec![text_value(&stage)])?.first().and_then(|u| u["manifest_sha256"].as_str()).map(str::to_string);
            if written.is_none() || written.as_deref() != r.manifest_sha256.as_deref() {
                return Err(CellError::new(ErrorCode::Conflict, "staged, but its stage is not complete with that manifest (POST …/complete first)"));
            }
        }
        let now = self.now()?;
        match machine::report(&m, r, now).map_err(refused)? {
            None => Ok(Reported { state: m.state, attempt: m.attempt, taken: false }),
            Some(s) => {
                let answer = Reported { state: s.to.state, attempt: s.to.attempt, taken: true };
                self.commit(vec![s], now)?;
                Ok(answer)
            }
        }
    }

    fn act(&self, old: &str, act: &str) -> CellResult<()> {
        let m = self.parts().get(old)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no old agent {old} in the move")))?;
        let now = self.now()?;
        let outcome = match act {
            "retry" => machine::retry(&m, now),
            "restart-old" => machine::restart_old(&m, now),
            "retire" => machine::retire(&m, now),
            "withdraw" => machine::withdraw(&m, now),
            other => return Err(CellError::invalid(format!("no act {other}"))),
        };
        if let Some(s) = outcome.map_err(refused)? {
            self.commit(vec![s], now)?;
        }
        Ok(())
    }

    /// The move whose stage `stage` names, if any (404 otherwise).
    fn of_stage(&self, stage: &str) -> CellResult<Move> {
        let no_stage = || refused(Refused::NoStage);
        let (old, _, _) = parse_stage(stage).ok_or_else(no_stage)?;
        self.parts().get(&old)?.ok_or_else(no_stage)
    }

    /// Whether the mover may upload `stage` now: its move (403 for a stage
    /// that is not its current command's, 409 while it does not stage).
    fn may_upload(&self, stage: &str) -> CellResult<Move> {
        let not_yours = || CellError::new(ErrorCode::Forbidden, "not the stage this old agent's current stop_and_stage names");
        let (old, _, _) = parse_stage(stage).ok_or_else(not_yours)?;
        let m = self.parts().get(&old)?.ok_or_else(not_yours)?;
        machine::uploadable(&m, stage).map_err(|r| match r {
            Refused::NotYours => not_yours(),
            r => refused(r),
        })?;
        Ok(m)
    }

    /// A complete's manifest checked against its move and its parts: what
    /// the router makes the zip of, as lines (its prefix, its upload, the
    /// zip's size, the manifest's SHA-256 when this complete was done
    /// already, then each part's number and etag), none of it the
    /// router's to read but there.
    fn complete(&self, stage: &str, manifest: &str) -> CellResult<String> {
        let m = self.may_upload(stage)?;
        let parts = self.parts();
        let upload = parts.rows("SELECT upload_id, manifest_sha256, zip_bytes FROM uploads WHERE stage = ?", vec![text_value(stage)])?.pop().ok_or_else(|| CellError::invalid("no part of this stage was taken"))?;
        let (prefix, upload_id) = (stage_prefix(m.phase, &m.old, m.attempt), upload["upload_id"].as_str().unwrap_or_default());
        if let Some(done) = upload["manifest_sha256"].as_str() {
            return Ok(format!("{prefix}\n{upload_id}\n{}\n{done}\n", upload["zip_bytes"].as_u64().unwrap_or(0)));
        }
        let taken = parts.stage_parts(stage)?;
        let zip_bytes: u64 = taken.iter().map(|p| p.bytes).sum();
        let f: ManifestIn = crate::json_body(manifest.as_bytes())?;
        // a part missing shows here; one too small, R2 refuses; the zip's
        // own hash is the importer's to check (its manifest's `zip_sha256`)
        if f.format != STAGE_FORMAT || f.old != m.old || f.phase != m.phase.as_str() || f.attempt != m.attempt || taken.is_empty() || zip_bytes != f.zip_bytes {
            return Err(CellError::invalid(format!("the stage is not complete: its manifest is not this attempt's in {STAGE_FORMAT}, or its parts, {zip_bytes} bytes, are not its zip")));
        }
        let mut lines = format!("{prefix}\n{upload_id}\n{zip_bytes}\n\n");
        for p in &taken {
            lines += &format!("{} {}\n", p.n, p.etag);
        }
        Ok(lines)
    }

    /// The supervisor: each move whose deadline passed, then phase 2's
    /// cutoff. A pass is bounded; one that left work runs again soon.
    fn supervise(&self) -> CellResult<()> {
        let now = self.now()?;
        let parts = self.parts();
        let due = parts.moves("SELECT * FROM moves WHERE deadline_ms IS NOT NULL AND deadline_ms <= ? ORDER BY deadline_ms LIMIT ?", vec![SqlStorageValue::Integer(now), SqlStorageValue::Integer(PASS_MAX as i64)])?;
        let mut busy = due.len() == PASS_MAX;
        self.commit(due.iter().filter_map(|m| machine::tick(m, now)).collect(), now)?;
        if let Some(cutoff_ms) = self.cutoff_ms().filter(|c| now >= *c) {
            let in_flight = parts.count("SELECT COUNT(*) AS n FROM moves WHERE state = 'archiving'")? as usize;
            let room = machine::ARCHIVING_MAX.saturating_sub(in_flight);
            if room > 0 {
                let unasked = parts.moves(UNASKED_FIRST, vec![SqlStorageValue::Integer(room as i64)])?;
                busy |= unasked.len() == room;
                self.commit(unasked.iter().filter_map(|m| machine::cutoff(m, cutoff_ms, now)).collect(), now)?;
            }
        }
        match busy {
            true => js::set_alarm(&self.raw, js::now_ms() + AGAIN_MS),
            false => self.arm(),
        }
    }

    /// What the alarm sends after its pass: each stage done with deleted
    /// (its unfinished upload aborted first), a few a pass; and each routine
    /// due appended to its agent's own fragment, keyed by its stage and how
    /// many it had, so a retry appends it once. One that keeps failing goes
    /// after `ROUTINE_TRIES` (the move's own deadline holds it, unopened).
    /// Then the alarm again, at the next.
    async fn outboxes(&self) -> CellResult<()> {
        let parts = self.parts();
        for r in parts.rows("SELECT prefix FROM sweeps ORDER BY at_ms, prefix LIMIT ?", vec![SqlStorageValue::Integer(SWEEPS_A_PASS)])? {
            let prefix = r["prefix"].as_str().unwrap_or_default();
            // the stage its prefix is (`p1/<old>/<attempt>/`)
            let stage = match prefix.trim_end_matches('/').splitn(3, '/').collect::<Vec<_>>()[..] {
                [phase @ ("p1" | "p2"), old, attempt] => format!("{old}.{phase}.{attempt}"),
                _ => return Err(CellError::host(format!("{prefix} is no stage's prefix"))),
            };
            if let Some(u) = parts.rows("SELECT upload_id FROM uploads WHERE stage = ? AND manifest_sha256 IS NULL", vec![text_value(&stage)])?.first() {
                js::upload_abort(&self.env, STAGE, &format!("{prefix}{STAGE_ZIP}"), u["upload_id"].as_str().unwrap_or_default()).await?;
            }
            if !js::blob_delete_under(&self.env, STAGE, prefix, SWEEP_PAGES).await?.1 {
                for q in ["DELETE FROM parts WHERE stage = ?", "DELETE FROM uploads WHERE stage = ?"] {
                    parts.exec(q, vec![text_value(&stage)])?;
                }
                parts.exec("DELETE FROM sweeps WHERE prefix = ?", vec![text_value(prefix)])?;
            }
        }
        let now = self.now()?;
        for r in parts.rows("SELECT stage, agent, tries FROM routines WHERE next_ms <= ? ORDER BY next_ms LIMIT ?", vec![SqlStorageValue::Integer(now), SqlStorageValue::Integer(ROUTINES_A_PASS)])? {
            let (stage, tries) = (r["stage"].as_str().unwrap_or_default(), r["tries"].as_i64().unwrap_or(0));
            match self.routine(stage, r["agent"].as_str().unwrap_or_default()).await {
                Ok(()) => parts.exec("DELETE FROM routines WHERE stage = ?", vec![text_value(stage)])?,
                Err(e) if tries + 1 >= ROUTINE_TRIES => {
                    console_error!("migrations.routine-dropped: {stage}: {}", e.message);
                    parts.exec("DELETE FROM routines WHERE stage = ?", vec![text_value(stage)])?;
                }
                Err(e) => {
                    console_error!("migrations.routine-failed: {stage} (try {}): {}", tries + 1, e.message);
                    let next = now + (ROUTINE_RETRY_MS << tries.clamp(0, 10));
                    parts.exec("UPDATE routines SET tries = ?, next_ms = ? WHERE stage = ?", vec![SqlStorageValue::Integer(tries + 1), SqlStorageValue::Integer(next), text_value(stage)])?;
                }
            }
        }
        self.arm()
    }

    /// One routine into `agent`'s own fragment, for `stage`, while the
    /// move still imports it.
    async fn routine(&self, stage: &str, agent: &str) -> CellResult<()> {
        let m = self.of_stage(stage)?;
        if m.state != MoveState::Importing || m.stage().as_deref() != Some(stage) {
            return Ok(());
        }
        let profiles = crate::ask_registry(&self.env, &crate::registry::calls::Profiles { ids: vec![agent.to_string()], emails_of: vec![] }).await?;
        let fragment = profiles.profiles.get(agent).and_then(|p| p.fragment.clone()).ok_or_else(|| CellError::host(format!("{agent} has no agent fragment")))?;
        let key = m.routine_key().ok_or_else(|| CellError::host(format!("{stage} is no import's")))?;
        crate::routed::object_answer(&self.env, "FRAGMENT", &fragment, &format!("stage/routine/{key}"), machine::routine_text(stage)).await?;
        Ok(())
    }

    fn cutoff_ms(&self) -> Option<i64> {
        self.cfg.migrations.as_ref().and_then(|m| m.cutoff_ms)
    }

    /// The alarm when it next has work: the earliest deadline of any move,
    /// or the cutoff while a move waits for it and there is room to archive
    /// (`machine::next_wake`); a routine's next try; or at once while a
    /// stage waits to be deleted. None when nothing is due.
    fn arm(&self) -> CellResult<()> {
        let parts = self.parts();
        let deadline = parts.rows("SELECT MIN(deadline_ms) AS at FROM moves", vec![])?.first().and_then(|r| r["at"].as_i64());
        let (unasked, archiving) = match self.cutoff_ms() {
            Some(_) => (parts.count(UNASKED_COUNT)?, parts.count("SELECT COUNT(*) AS n FROM moves WHERE state = 'archiving'")? as usize),
            None => (0, 0),
        };
        let routine = parts.rows("SELECT MIN(next_ms) AS at FROM routines", vec![])?.first().and_then(|r| r["at"].as_i64());
        let sweep = match parts.count("SELECT COUNT(*) AS n FROM sweeps")? {
            0 => None,
            _ => Some(self.now()?),
        };
        let moves = machine::next_wake(deadline, self.cutoff_ms(), unasked, archiving, self.now()?);
        match [moves, routine, sweep].into_iter().flatten().min() {
            // the clock lever moves the object's time, not the alarm's
            Some(at) => js::set_alarm(&self.raw, (at - (self.now()? - js::now_ms())).max(js::now_ms() + 1)),
            None => Ok(()),
        }
    }

    /// The levers (`/api/test/migrations`, a fleet with levers alone): the
    /// clock, its supervisor run at once (what that queued, the alarm sends
    /// right after); and a person's answers.
    fn test(&self, v: Value) -> CellResult<Response> {
        let parts = self.parts();
        if v["op"] == "clock" {
            let offset = v["offsetMs"].as_i64().ok_or_else(|| CellError::invalid("clock names its offsetMs"))?;
            parts.set_meta("clock_offset", &offset.to_string())?;
            self.supervise()?;
            return crate::json_answer(&json!({ "offsetMs": offset, "now": self.now()? }));
        }
        let (Some("answer"), Some(answer)) = (v["op"].as_str(), v["answer"].as_str()) else { return Err(CellError::invalid(format!("no migrations lever {}", v["op"]))) };
        let agent = v["agent"].as_str().map(|a| (a, v["owner"].as_str().unwrap_or_default()));
        let (state, attempt) = self.answer(v["old"].as_str().unwrap_or_default(), None, answer, agent)?;
        crate::json_answer(&json!({ "state": state.as_str(), "attempt": attempt }))
    }
}

fn text_value(s: &str) -> SqlStorageValue {
    SqlStorageValue::String(s.to_string())
}

/// Its rows, as a transaction's closure holds them (`'static`). Rows read
/// as `Value`, whose reader every object shares; a typed row reader is a
/// deserializer of its own per row type.
#[derive(Clone)]
struct Parts {
    sql: SqlStorage,
    changed: Rc<RefCell<Vec<String>>>,
}

impl Parts {
    fn rows(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<Value>> {
        Ok(self.sql.exec(q, binds)?.to_array::<Value>()?)
    }

    fn exec(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<()> {
        self.sql.exec(q, binds)?;
        Ok(())
    }

    fn count(&self, q: &str) -> CellResult<u64> {
        self.rows(q, vec![])?.first().and_then(|r| r["n"].as_u64()).ok_or_else(|| CellError::host(format!("a count answered no number: {q}")))
    }

    fn meta(&self, key: &str) -> CellResult<Option<String>> {
        Ok(self.rows("SELECT value FROM meta WHERE key = ?", vec![text_value(key)])?.first().and_then(|r| r["value"].as_str()).map(str::to_string))
    }

    fn set_meta(&self, key: &str, value: &str) -> CellResult<()> {
        self.exec("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value", vec![text_value(key), text_value(value)])
    }

    fn person_of(&self, workos_id: &str) -> CellResult<Option<String>> {
        Ok(self.rows("SELECT person FROM people WHERE workos_id = ?", vec![text_value(workos_id)])?.first().and_then(|r| r["person"].as_str()).map(str::to_string))
    }

    fn moves(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<Move>> {
        self.rows(q, binds)?.iter().map(|r| Move::read(r).map_err(CellError::host)).collect()
    }

    fn get(&self, old: &str) -> CellResult<Option<Move>> {
        Ok(self.moves("SELECT * FROM moves WHERE old = ?", vec![text_value(old)])?.pop())
    }

    fn stage_parts(&self, stage: &str) -> CellResult<Vec<StagePart>> {
        let rows = self.rows("SELECT n, bytes, sha256, etag FROM parts WHERE stage = ? ORDER BY n", vec![text_value(stage)])?;
        Ok(rows.iter().map(|r| StagePart { n: r["n"].as_u64().unwrap_or(0) as u32, bytes: r["bytes"].as_u64().unwrap_or(0), sha256: r["sha256"].as_str().unwrap_or_default().into(), etag: r["etag"].as_str().unwrap_or_default().into() }).collect())
    }

    /// A move's columns as SQLite binds them.
    fn binds(m: &Move) -> Vec<SqlStorageValue> {
        m.values()
            .into_iter()
            .map(|c| match c {
                Col::Null => SqlStorageValue::Null,
                Col::Int(n) => SqlStorageValue::Integer(n),
                Col::Text(t) => SqlStorageValue::String(t),
            })
            .collect()
    }

    /// A new row, its person told when it comes in offered (bound already).
    fn insert(&self, m: &Move, offer: &str) -> CellResult<()> {
        let mut binds = Self::binds(m);
        binds.push(text_value(offer));
        self.exec(INSERT_MOVE, binds)?;
        if m.state == MoveState::Open {
            self.changed.borrow_mut().push(m.workos_id.clone());
        }
        Ok(())
    }

    /// A change: the move stored, its sweeps and routines queued for after
    /// (the alarm sends them), in the caller's transaction.
    fn apply(&self, s: &Step, now: i64) -> CellResult<()> {
        let mut binds = Self::binds(&s.to);
        binds.push(text_value(&s.to.old));
        self.exec(UPDATE_MOVE, binds)?;
        self.changed.borrow_mut().push(s.to.workos_id.clone());
        for e in &s.effects {
            match e {
                Effect::Sweep { prefix } => self.exec("INSERT INTO sweeps (prefix, at_ms) VALUES (?, ?) ON CONFLICT (prefix) DO NOTHING", vec![text_value(prefix), SqlStorageValue::Integer(now)])?,
                Effect::Routine { agent, stage } => self.exec(
                    "INSERT INTO routines (stage, agent, tries, next_ms) VALUES (?, ?, 0, ?) ON CONFLICT (stage) DO UPDATE SET agent = excluded.agent, tries = 0, next_ms = excluded.next_ms",
                    vec![text_value(stage), text_value(agent), SqlStorageValue::Integer(now)],
                )?,
            }
        }
        Ok(())
    }
}
