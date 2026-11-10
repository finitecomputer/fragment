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

use std::future::Future;
use std::pin::Pin;

use fragment_core::migrations::{self as machine, Col, Effect, Move, Refused, Step};
use fragment_nip98::Payload;
use fragment_proto::migrations::{
    parse_stage, stage_prefix, valid_old, AdminAct, Mailbox, MoveAt, MoveState, Report, Reported, RosterTaken, StageCompleted, StageDone, StagePart, StageParts, MAILBOX_MAX, MANIFEST_MAX_BYTES, PARTS_MAX,
    PART_MAX_BYTES, ROSTER_AGENTS_MAX, ROSTER_PEOPLE_MAX, STAGE_FORMAT, STAGE_MANIFEST, STAGE_ZIP, TEXT_MAX_BYTES,
};
use fragment_proto::{limits, ErrorCode};
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
    pass(env, format!("bind/{subject}"), person.to_string()).await
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
}

impl DurableObject for MigrationsCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let schema = crate::schema::Schema::start(&raw, state.storage().sql(), &fragment_core::migrate::MIGRATIONS, &state.id().to_string());
        let cfg = Config::from_env(&env);
        MigrationsCell { state, env, raw, cfg, schema }
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
        Response::ok("")
    }
}

impl MigrationsCell {
    fn parts(&self) -> Parts {
        Parts { sql: self.state.storage().sql() }
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
        let answer = self.call(&req.path(), body)?;
        self.arm()?;
        Ok(answer)
    }

    fn call(&self, path: &str, body: String) -> CellResult<Response> {
        let parts = self.parts();
        let number = |n: &str| n.parse::<u64>().map_err(|_| CellError::invalid(format!("{n} is no number")));
        let at: Vec<&str> = path.split('/').collect();
        let [_, "migrations", op, args @ ..] = &at[..] else { return Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))) };
        match (*op, args) {
            ("roster", []) => crate::json_answer(&self.roster(&body)?),
            ("bind", [workos_id]) => crate::json_answer(&json!({ "bound": self.bind(workos_id, &body)? })),
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
        let old = v["old"].as_str().unwrap_or_default();
        let m = parts.get(old)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no old agent {old}")))?;
        let person = parts.person_of(&m.workos_id)?.ok_or_else(|| CellError::new(ErrorCode::Conflict, "its person is not known here yet"))?;
        let now = self.now()?;
        let outcome = match (v["op"].as_str(), v["answer"].as_str()) {
            (Some("answer"), Some("later")) => machine::later(&m, now),
            (Some("answer"), Some("never")) => machine::never(&m, now),
            (Some("answer"), Some("ask_again")) => machine::ask_again(&m, now),
            (Some("answer"), Some("yes")) if v["owner"].as_str() == Some(person.as_str()) => machine::yes(&m, v["agent"].as_str().unwrap_or_default(), now),
            (Some("answer"), Some("yes")) => return Err(CellError::new(ErrorCode::Forbidden, "yes binds an agent of the person's own")),
            other => return Err(CellError::invalid(format!("no migrations lever {other:?}"))),
        };
        let (state, attempt) = match outcome.map_err(refused)? {
            None => (m.state, m.attempt),
            Some(s) => {
                let at = (s.to.state, s.to.attempt);
                self.commit(vec![s], now)?;
                at
            }
        };
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

    fn insert(&self, m: &Move, offer: &str) -> CellResult<()> {
        let mut binds = Self::binds(m);
        binds.push(text_value(offer));
        self.exec(INSERT_MOVE, binds)
    }

    /// A change: the move stored, its sweeps and routines queued for after
    /// (the alarm sends them), in the caller's transaction.
    fn apply(&self, s: &Step, now: i64) -> CellResult<()> {
        let mut binds = Self::binds(&s.to);
        binds.push(text_value(&s.to.old));
        self.exec(UPDATE_MOVE, binds)?;
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
