//! What the router hands a fragment's supervisor, and how it travels. The
//! router decides a request's fragment, the URL it arrived on, and who is
//! asking; `Routed` carries that in headers
//! only the router sets (`fragment_proto::routed`; a client's are dropped),
//! written once by `to_headers` and read once by `from_headers`, side by
//! side here so the two cannot drift.
//!
//! Who is asking arrives resolved (`Signed`) on the control API, and
//! unresolved (`Credential`) on a site request: most of a site's requests
//! answer alike for everyone who may see the fragment, so the fragment asks
//! the registry only when its answer depends on who is asking.
//!
//! Calls from inside the platform (a run's Workflow, the app's `Files`
//! capability, the delivery consumer, the router's test hooks) are not
//! routed requests: they carry no caller and no URL, and the supervisor
//! answers them before it decodes a `Routed`. `internal_request` builds
//! them, for Rust and for `entry.mjs`.
//!
//! Every call into a platform object goes out through `send` (or
//! `send_again`), here alone (xtask's guard), stamped with this release,
//! and every object's `fetch` checks it first (`refused`): a call from
//! another release is refused before anything is done, so a change to a
//! call's shape ships safely across the seconds a deploy takes
//! (fragment_core::release; docs/MODEL.md, "Calls between releases").

use std::ops::Deref;

use fragment_core::{npub, release};
use fragment_proto::{routed, valid_fragment_name, ErrorBody, ErrorCode, Identity, IdentityKind};
use serde::{Deserialize, Serialize};
// the macro's generated code names `wasm_bindgen`: worker's re-export
use worker::wasm_bindgen::{self, prelude::*, JsCast};
use worker::{console_log, Delay, Env, Headers, Method, Request, RequestInit, Response, Url};

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls;

/// Who is asking: an identity the registry vouched for, and the key it
/// signed with (none for a browser's session). It reads as its identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed {
    #[serde(flatten)]
    pub identity: Identity,
    /// 64 hex.
    pub key: Option<String>,
    /// An agent acting for someone (decision R17): the identity its
    /// signed URL named in `for`, which the router honors on an agent's
    /// request to a fragment only (`crate::acting_for`). The fragment acts
    /// with the lower of that identity's role and the agent's cap
    /// (`fragment_core::access`).
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub acting_for: Option<String>,
}

impl Signed {
    /// Who signed, acting as themselves.
    pub fn new(identity: Identity, key: Option<String>) -> Signed {
        Signed { identity, key, acting_for: None }
    }
}

impl Deref for Signed {
    type Target = Identity;
    fn deref(&self) -> &Identity {
        &self.identity
    }
}

/// Who is asking a site request, as the router found them: not yet
/// resolved, since most of a site's requests answer alike for everyone who
/// may see the fragment (a page, a file). The fragment resolves it when its
/// answer depends on who is asking (`FragmentCell::identified`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Credential {
    /// The key a NIP-98 signature over the request was verified for (64
    /// hex): checking a signature needs no registry, so the router did.
    Key(String),
    /// The token of the fragment origin's session cookie (64 hex).
    Session(String),
    /// The token of its frame cookie (64 hex): a session made in a frame
    /// of a page that framed it through `__frame`.
    Frame(String),
}

impl Credential {
    fn well_formed(&self) -> bool {
        match self {
            Credential::Key(key) => npub::is_hex_key(key),
            Credential::Session(token) | Credential::Frame(token) => token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        }
    }

    /// Who the credential names on `fragment`, asked of the registry: a key
    /// no one holds, or a revoked one, is 401; a session that is not live
    /// is nobody (a stale cookie leaves the request anonymous); a registry
    /// that cannot answer is 503, never anonymous (R15, rule 7).
    pub async fn resolve(self, env: &Env, fragment: &str) -> CellResult<Option<Signed>> {
        match self {
            Credential::Key(key) => {
                let identity = crate::ask_registry(env, &calls::Resolve { key: key.clone() }).await?;
                Ok(Some(Signed::new(identity, Some(key))))
            }
            Credential::Session(token) => Ok(site_session(env, token, fragment, false).await?.map(|live| Signed::new(live.identity, None))),
            Credential::Frame(token) => Ok(site_session(env, token, fragment, true).await?.map(|live| Signed::new(live.identity, None))),
        }
    }
}

/// The live session a fragment origin's cookie names (a frame's when
/// `frame`), or `None`: not live is nobody.
pub(crate) async fn site_session(env: &Env, token: String, fragment: &str, frame: bool) -> CellResult<Option<calls::LiveSession>> {
    match crate::ask_registry(env, &calls::Session { token, fragment: Some(fragment.to_string()), frame }).await {
        Ok(live) => Ok(Some(live)),
        Err(e) if e.code == ErrorCode::Unauthenticated => Ok(None),
        Err(e) => Err(e),
    }
}

/// A request the router hands a fragment's supervisor.
pub struct Routed {
    /// The fragment's name (`<label>--<suffix>`).
    pub name: String,
    /// The URL it arrived on: canonical URLs, cookies, and the query string
    /// all come from here.
    pub url: Url,
    /// Who is asking, resolved (the control API). `None` with no
    /// `credential`: anonymous.
    pub signed: Option<Signed>,
    /// Who is asking a site request, unresolved. Never beside `signed`.
    pub credential: Option<Credential>,
}

impl Routed {
    /// Writes the route into `headers` (the router's side).
    pub fn to_headers(&self, headers: &Headers) -> CellResult<()> {
        assert!(valid_fragment_name(&self.name), "the router routes to a valid name");
        assert!(self.signed.is_none() || self.credential.is_none(), "who is asking is resolved or not, never both");
        assert!(self.credential.as_ref().is_none_or(Credential::well_formed), "the router forwards a credential it checked");
        assert!(self.signed.as_ref().is_none_or(|s| s.acting_for.is_none() || s.kind == IdentityKind::Agent), "only an agent acts for someone");
        headers.set(routed::NAME, &self.name)?;
        headers.set(routed::URL, self.url.as_str())?;
        if let Some(signed) = &self.signed {
            let json = serde_json::to_string(signed).map_err(|e| CellError::host(format!("the signer: {e}")))?;
            headers.set(routed::SIGNED, &json)?;
        }
        if let Some(credential) = &self.credential {
            let json = serde_json::to_string(credential).map_err(|e| CellError::host(format!("the credential: {e}")))?;
            headers.set(routed::CREDENTIAL, &json)?;
        }
        Ok(())
    }

    /// Reads the route back (the supervisor's side). Only the router sets
    /// these headers, so a missing or malformed one is a host fault.
    pub fn from_headers(headers: &Headers) -> CellResult<Routed> {
        let name = headers.get(routed::NAME)?.filter(|n| valid_fragment_name(n)).ok_or_else(|| CellError::host("no fragment name from the router"))?;
        let url = headers.get(routed::URL)?.and_then(|u| Url::parse(&u).ok()).ok_or_else(|| CellError::host("no URL from the router"))?;
        let signed = match headers.get(routed::SIGNED)? {
            Some(json) => {
                let signed: Signed = serde_json::from_str(&json).map_err(|e| CellError::host(format!("the router's signer: {e}")))?;
                if !npub::is_identity(&signed.id) || signed.key.as_deref().is_some_and(|k| !npub::is_hex_key(k)) {
                    return Err(CellError::host("the router named a malformed signer"));
                }
                // the router honors `for` for an agent only, naming an identity
                if signed.acting_for.as_deref().is_some_and(|f| !npub::is_identity(f) || signed.kind != IdentityKind::Agent || signed.owner.is_none()) {
                    return Err(CellError::host("the router named a malformed `for`"));
                }
                Some(signed)
            }
            None => None,
        };
        let credential = match headers.get(routed::CREDENTIAL)? {
            Some(json) => {
                let credential: Credential = serde_json::from_str(&json).map_err(|e| CellError::host(format!("the router's credential: {e}")))?;
                if !credential.well_formed() || signed.is_some() {
                    return Err(CellError::host("the router named a malformed credential"));
                }
                Some(credential)
            }
            None => None,
        };
        Ok(Routed { name, url, signed, credential })
    }
}

/// The marker an internal route's caller carries, and its value: the router
/// never passes these headers, so a request with one came from inside.
/// `None`: the route needs none (the router's test hooks, which fleets
/// without them do not answer).
fn marker(path: &str) -> CellResult<Option<(&'static str, &'static str)>> {
    if path.starts_with("job/") {
        Ok(Some((crate::jobs::JOB_HEADER, "1")))
    } else if path.starts_with("cap/files/") {
        Ok(Some((crate::files::CAP_HEADER, "files")))
    } else if path == "deliver/report" {
        Ok(Some((crate::deliveries::REPORT_HEADER, "1")))
    } else if path.starts_with("computer/") {
        Ok(Some((crate::computer::INTERNAL_HEADER, "1")))
    } else if path == "meter/acked" || path == "meter/whose" {
        Ok(Some((crate::meter::METER_HEADER, "1")))
    } else if path == "invites/claim" || path == "invites/decline" || path == "members/agent" {
        Ok(Some((crate::members::CLAIM_HEADER, "1")))
    } else if path.starts_with("wipe/") {
        Ok(Some((crate::wipe::WIPE_HEADER, "1")))
    } else if path.starts_with("test/") {
        Ok(None)
    } else {
        Err(CellError::host(format!("{path} is not an internal route")))
    }
}

/// A call into a fragment's supervisor from inside the platform: `path`
/// (`job/advance`, `cap/files/read`, `deliver/report`, `test/keys`) with a
/// JSON body, marked as that route expects.
pub(crate) fn internal_request(path: &str, body: &str) -> CellResult<Request> {
    let headers = Headers::new();
    headers.set("content-type", "application/json")?;
    if let Some((header, value)) = marker(path)? {
        headers.set(header, value)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(headers).with_body(Some(body.into()));
    Ok(Request::new_with_init(&format!("https://fragment.internal/{path}"), &init)?)
}

/// Stamps a call into a platform object with this release
/// (`release::HEADER`), and when it was deployed (`release::DEPLOYED`),
/// which no release reads now: an object of a release before this one read
/// it to restart onto a newer caller's, so it stays on the wire until none
/// can be live (docs/technical-debt-ledger.md).
fn stamp(req: &Request, env: &Env) -> CellResult<()> {
    let cfg = Config::from_env(env);
    let headers = req.headers();
    headers.set(release::HEADER, &cfg.deploy_id)?;
    match cfg.deployed_at {
        Some(at) => headers.set(release::DEPLOYED, &at.to_string())?,
        None => headers.delete(release::DEPLOYED)?,
    }
    Ok(())
}

/// The object's side, the first thing every platform object's `fetch`
/// does: a call from a release other than `own` (its deployment's
/// `FRAGMENT_DEPLOY_ID`, or the Registry's test stand-in, `skewFor`), or
/// naming none, is answered here, 503 `deploying` marked as this refusal
/// (`release::REFUSED`), before anything is read or done; `None` admits it.
pub(crate) fn refused(req: &Request, own: &str) -> worker::Result<Option<Response>> {
    let caller = req.headers().get(release::HEADER)?;
    let Err(skew) = release::admit(caller.as_deref(), own) else { return Ok(None) };
    console_log!("{}", serde_json::json!({ "event": "release.refused", "caller": skew.caller, "callee": skew.callee, "path": req.path() }));
    let mut resp = CellError::new(ErrorCode::Deploying, skew.message()).response()?;
    resp.headers_mut().set(release::REFUSED, own)?;
    Ok(Some(resp))
}

/// What one call into an object came to: its answer, or its refusal of
/// this release (which did nothing), and the release it runs.
enum Sent {
    Answered(Response),
    Refused(CellError, String),
}

async fn sent(env: &Env, binding: &str, name: &str, req: Request) -> CellResult<Sent> {
    let stub = env.durable_object(binding)?.get_by_name(name)?;
    sent_to(env, binding, name, stub, req).await
}

/// `sent`, to the object `stub` is (`name` says which, in a refusal's words).
async fn sent_to(env: &Env, binding: &str, name: &str, stub: worker::Stub, req: Request) -> CellResult<Sent> {
    stamp(&req, env)?;
    let mut resp = stub.fetch_with_request(req).await?;
    // an app's answer passed through with the mark would only be its own
    // request's refusal, as `serve::REFUSAL`'s would
    let Some(theirs) = resp.headers().get(release::REFUSED)?.filter(|_| resp.status_code() == 503) else {
        return Ok(Sent::Answered(resp));
    };
    let message = match resp.json::<ErrorBody>().await {
        Ok(b) if b.error == ErrorCode::Deploying => b.message,
        _ => format!("{binding} {name} runs another release: a deploy is going out; try again in a few seconds"),
    };
    let mut error = CellError::new(ErrorCode::Deploying, message);
    error.release_refused = Some(theirs.clone());
    Ok(Sent::Refused(error, theirs))
}

/// One call into the Durable Object `name` of `binding` (the only way the
/// cell calls one), stamped with this release: its answer, whatever it
/// is, or `deploying` when the object runs another release and refused it.
/// Asked once: for a request that cannot be made again (a body streamed
/// through, a socket's upgrade), or whose caller retries on its own.
pub(crate) async fn send(env: &Env, binding: &str, name: &str, req: Request) -> CellResult<Response> {
    match sent(env, binding, name, req).await? {
        Sent::Answered(resp) => Ok(resp),
        Sent::Refused(e, _) => Err(e),
    }
}

/// `send`, asked again while the object refuses this release (it did
/// nothing, so asking again is safe, however the call acts), after
/// `release::again_after_ms`'s waits: at most `release::ASKS` asks, then
/// `deploying`. `make` makes each ask's request. Only the object's own
/// refusal is asked again: a `deploying` it answers from a call of its
/// own is its answer, as it may have acted before that call.
pub(crate) async fn send_again(env: &Env, binding: &str, name: &str, make: impl Fn() -> CellResult<Request>) -> CellResult<Response> {
    // bounded: `again_after_ms` is `None` past `release::ASKS` refusals
    for refused in 1.. {
        let e = match sent(env, binding, name, make()?).await? {
            Sent::Answered(resp) => return Ok(resp),
            Sent::Refused(e, _) => e,
        };
        match release::again_after_ms(refused) {
            Some(ms) => Delay::from(std::time::Duration::from_millis(ms)).await,
            None => return Err(e),
        }
    }
    unreachable!("the asks are bounded")
}

/// The internal call `path` with a JSON `body` on the Durable Object
/// `name` of `binding`: its JSON answer, or its refusal as it named it.
pub(crate) async fn ask_object(env: &Env, binding: &str, name: &str, path: &str, body: &serde_json::Value) -> CellResult<serde_json::Value> {
    let body = body.to_string();
    let mut resp = send_again(env, binding, name, || internal_request(path, &body)).await?;
    let status = resp.status_code();
    let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status == 200 {
        return Ok(v);
    }
    let code = v["error"].as_str().and_then(|c| serde_json::from_value::<ErrorCode>(serde_json::json!(c)).ok()).unwrap_or(ErrorCode::HostFailed);
    let message = v["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("{binding} {name} answered {status}"));
    Err(CellError::new(code, message))
}

/// The internal call `path` on the Durable Object `name` of `binding`, its
/// answer as it is (a picture's bytes, or its refusal).
pub(crate) async fn fetch_object(env: &Env, binding: &str, name: &str, path: &str) -> CellResult<Response> {
    send_again(env, binding, name, || internal_request(path, "{}")).await
}

/// The path a deploy's probe of an object asks (`probed`, `objects`), with
/// `PROBE_HEADER`: the router passes no client's header of that name
/// (lib.rs `PASSED_HEADERS`), so only the Worker's own `/healthz/objects`
/// meets an object's answer to it.
const PROBE_PATH: &str = "/__probe";
const PROBE_HEADER: &str = "x-fragment-probe";

/// The probe object of each class `/healthz/objects` asks: the Registry,
/// the deployment's one (the busiest object, held on its old code longest
/// at a deploy), and an object of each other class named for the probe, a
/// name no fragment, person or computer has (`__`). One that never existed
/// is made by the probe, its schema's steps run and nothing more.
const PROBED: [(&str, &str, &str); 5] = [
    ("Registry", "REGISTRY", crate::registry::NAME),
    ("Principal", "PRINCIPAL", "__probe"),
    ("Ledger", "LEDGER", "__probe"),
    ("Fragment", "FRAGMENT", "__probe"),
    ("Computer", "COMPUTER", "__probe"),
];

/// An object's answer to a deploy's probe, the second thing its `fetch`
/// does, right after its release check (`refused`): its schema ready
/// (200), or the refusal every call of its gets (`schema_refused`).
/// `None` for any other call, which goes on as before.
pub(crate) fn probed(req: &Request, schema: &crate::schema::Schema) -> worker::Result<Option<Response>> {
    if req.path() != PROBE_PATH || !req.headers().has(PROBE_HEADER)? {
        return Ok(None);
    }
    match schema.ready() {
        Ok(()) => Response::from_json(&serde_json::json!({ "ready": true })).map(Some),
        Err(e) => e.response().map(Some),
    }
}

/// The request a deploy's probe of an object asks (`probed`).
fn probe_request() -> CellResult<Request> {
    let headers = Headers::new();
    headers.set(PROBE_HEADER, "1")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    Ok(Request::new_with_init(&format!("https://probe.internal{PROBE_PATH}"), &init)?)
}

/// The object an operator wakes (`wake`), by the id Cloudflare lists it
/// under: 64 lowercase hex digits.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Wake {
    id: String,
}

/// An operator's wake of one platform object (`POST /api/admin/objects/
/// {class}/wake`; docs/api.md, Operators): it starts as any call starts
/// it, its schema made current (an object made before versions adopted at
/// step 1, or refused), and answers as a deploy's probe does (`probed`):
/// `{class, id, ready: true}`, or its refusal (`schema_refused`, naming
/// it and why; `deploying` from another release). Asked again, it answers the
/// same and runs nothing. It exists for the objects no call has started
/// since versions came (docs/technical-debt-ledger.md), and goes with them.
pub(crate) async fn wake(env: &Env, class: &str, Wake { id }: Wake) -> CellResult<serde_json::Value> {
    let Some((class, binding, _)) = PROBED.iter().find(|(c, _, _)| c.eq_ignore_ascii_case(class)).copied() else {
        let classes: Vec<&str> = PROBED.iter().map(|(c, _, _)| *c).collect();
        return Err(CellError::invalid(format!("no class {class:?}: one of {}", classes.join(", "))));
    };
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(CellError::invalid("an object's id is 64 lowercase hex digits"));
    }
    // Cloudflare refuses an id of another namespace (another class, another
    // account's) here, before any object is made
    let ns = env.durable_object(binding)?;
    let id = ns.id_from_string(&id).map_err(|_| CellError::invalid(format!("{id} is no {class} object's id")))?;
    let named = id.to_string();
    match sent_to(env, binding, &named, id.get_stub()?, probe_request()?).await? {
        Sent::Answered(resp) if resp.status_code() == 200 => Ok(serde_json::json!({ "class": class, "id": named, "ready": true })),
        Sent::Answered(mut resp) => {
            let status = resp.status_code();
            match resp.json::<ErrorBody>().await {
                Ok(b) => Err(CellError::new(b.error, b.message)),
                Err(_) => Err(CellError::host(format!("{class} {named} answered its wake {status}"))),
            }
        }
        Sent::Refused(e, _) => Err(e),
    }
}

/// `GET /healthz/objects` (docs/api.md, Health): this Worker's release,
/// and what each class's probe object (`PROBED`) answered it, asked once
/// each, at once: `{"release", "objects": {<class>: {"release"} |
/// {"refusedBy"} | {"error"}}}`, 200 when every one admitted this release
/// with its schema ready, else 503. A deploy asks it until it is 200
/// (xtask/src/deploy.rs). Anyone may: each answer is a release, which
/// every answer of the platform names already (`x-fragment-deploy`), and
/// each ask is answered before anything else an object does, as cheaply as
/// the refusal any call of another release gets.
pub(crate) async fn objects(env: &Env, cfg: &Config) -> CellResult<Response> {
    let ask = |binding: &'static str, name: &'static str| async move { sent(env, binding, name, probe_request()?).await };
    let answers = futures_util::future::join_all(PROBED.iter().map(|(_, binding, name)| ask(binding, name))).await;
    let mut objects = serde_json::Map::new();
    let mut all = true;
    for ((class, _, _), answer) in PROBED.iter().zip(answers) {
        let said = match answer {
            Ok(Sent::Answered(resp)) if resp.status_code() == 200 => serde_json::json!({ "release": cfg.deploy_id }),
            Ok(Sent::Answered(mut resp)) => {
                let status = resp.status_code();
                let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
                serde_json::json!({ "error": format!("{status} {}: {}", v["error"].as_str().unwrap_or("?"), v["message"].as_str().unwrap_or("")) })
            }
            Ok(Sent::Refused(_, theirs)) => serde_json::json!({ "refusedBy": theirs }),
            Err(e) => serde_json::json!({ "error": e.message }),
        };
        all &= said.get("release").is_some();
        objects.insert(class.to_string(), said);
    }
    let body = serde_json::json!({ "release": cfg.deploy_id, "objects": objects });
    Ok(Response::from_json(&body)?.with_status(if all { 200 } else { 503 }))
}

/// `internal_request` for `entry.mjs` (the job driver, the `Files`
/// capability and a computer's reports to itself), stamped with the
/// release of `env` (the caller's), so the JavaScript names no header. A
/// class with a static method, because worker-build exports classes by
/// name and a free function only as a method of the Worker's entrypoint.
#[wasm_bindgen(wasm_bindgen = worker::wasm_bindgen)]
pub struct InternalRoute;

#[wasm_bindgen]
impl InternalRoute {
    pub fn request(env: JsValue, path: &str, body: &str) -> Result<worker::web_sys::Request, JsValue> {
        let env: Env = env.unchecked_into();
        let req = internal_request(path, body).map_err(|e| JsValue::from_str(&e.message))?;
        stamp(&req, &env).map_err(|e| JsValue::from_str(&e.message))?;
        // the handle, not JavaScript's `Request.clone()` (which tees the body)
        Ok(Clone::clone(req.inner()))
    }
}
