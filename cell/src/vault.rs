//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): one
//! `Vault` Durable Object a person, named by their identity, holding the
//! secrets their agents use without ever holding them. A value goes in
//! sealed for this object (`keys::seal`) and never comes out: no route,
//! answer, log or event names one.
//!
//! - **One protocol**: every call is `vault/<op>[/<name>]`, a put's body
//!   its `PutSecret` as its person sent it, on the Vault its person's
//!   identity names (only the platform's own code reaches one:
//!   `VAULT_HEADER`). The Vault decides; the router finds it and passes its
//!   answer on as it came.
//! - **Its routes** (the router's `/api/vault`, `route` below): the person
//!   alone, never an agent, lists (names only), puts and removes.
//! - **Its seam** (`put`, `delete`): the same calls, for the platform's own
//!   code (FIN-463's hand-off). The rules are one (`fragment_core::vault`).
//! - **A secret is kept as its value, sealed, and its `Secret` as JSON**
//!   (what every answer names), so a list is those texts, read nowhere.
//! - **Its wipe** (wipe.rs, the ledger's step): emptied in one step, and a
//!   row kept that says so; nothing is put after.
//! - **Its agents' use** (`names`, `fill`): its person's computer asks
//!   which secrets each agent may use, toward which hosts (its guest's
//!   view, and the hosts its egress catches), and, for one request, each
//!   secret it names: filled toward one of its hosts, for an agent bound to
//!   it. A Nostr key is never answered: the Vault signs the request.
//! - **Its person's own keys** (M16: moved from their computer): one per
//!   `own` provider of the catalog, its own kind (Paul, 2026-11-18), used
//!   by each agent of theirs not narrowed from it. Its person sets or
//!   removes one (`own_put`, from `PUT …/key` or a sign-in's finish); their
//!   computer asks for them as a request goes (`own_keys`), beside its ask
//!   of the agent's fragment, so the swap waits on no hop more; `names`
//!   lists them. A sign-in to one is begun and finished here (`sign_in`,
//!   `signed_in`). A computer hands over the keys it kept before the move
//!   (`hand_over`), and one its person set or removed since stands.
//! - **Its agents' asks** (`asks` below; docs/secrets.md, "An agent
//!   asks"): an agent asks its person for a secret, and the Vault keeps
//!   the ask, shown to its person as a notice with a masked field
//!   (`vault/notices`), which they answer from the platform's own page:
//!   their paste comes here alone, never into a chat's records or past an
//!   agent. The answer is a put under the Vault's own rules.

use std::cell::Cell;

use fragment_core::catalog::Kind;
use fragment_core::own_signin;
use fragment_core::vault::{self as rules, Placing};
use fragment_proto::computer::ProviderState;
use fragment_proto::vault::{AnswerAsk, AskSecret, AskState, Place, PutSecret, Secret, SecretAsk, SecretRemoved};
use fragment_proto::{ErrorCode, IdentityKind};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;
use crate::json_response::json_bytes;
use crate::routed::Signed;

/// What a value is sealed for: this class, and the object's id.
const SEAL_CLASS: &str = "Vault";

/// The marker every call into a Vault carries (routed.rs `marker`): only
/// the platform's own code calls one, and the router passes no such header.
pub(crate) const VAULT_HEADER: &str = "x-fragment-vault";

/// `vault/names`' body: which of `agents` may use which secrets.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamesAsk {
    agents: Vec<String>,
}

/// A secret an agent may use, as its computer lists it: the agent, the
/// secret's name and its hosts (never a parked one's: it fills nothing).
/// A tuple on the wire between the two, read in order with no field names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Usable(pub String, pub String, pub Vec<String>);

/// `vault/fill`'s body: `names` for `agent`'s request to `host`, and what
/// a signature covers: the request as it is sent (its method, its https
/// URL, its body's SHA-256 hex; none, an empty body).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct FillAsk {
    pub agent: String,
    pub host: String,
    pub names: Vec<String>,
    pub method: String,
    pub url: String,
    pub body_sha256: Option<String>,
}

/// One secret, filled, toward the host asked (the Vault checked it is one
/// of its own): the value or a signed event, where it goes, and whether it
/// is a signature made for this request alone. A tuple on the wire, read in
/// order with no field names.
#[derive(Serialize, Deserialize)]
pub(crate) struct Filled(pub String, pub Place, pub bool);

/// What a use reads of a kept `Secret`: its hosts, its place and its agents.
#[derive(Deserialize)]
struct Bound {
    hosts: Vec<String>,
    place: Place,
    agents: Vec<String>,
}

/// `vault/fill`'s answer, in the order asked.
#[derive(Serialize, Deserialize)]
struct Fills {
    secrets: Vec<Filled>,
}

/// `vault/names`' answer: what each agent asked about may use (`Usable`),
/// and the catalog's `own` providers its person holds a key for. A tuple
/// on the wire.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Names(pub Vec<Usable>, pub Vec<String>);

/// An own key, as its person gives one (`PUT …/key`'s body, passed on as
/// it came) or a sign-in's exchange made it.
#[derive(Serialize, Deserialize)]
pub(crate) struct OwnPut {
    pub key: String,
}

/// `PUT …/key`'s answer: the own key's provider and its state, `set` or
/// `not_set`.
#[derive(Serialize)]
struct OwnSet<'a> {
    provider: &'a str,
    state: ProviderState,
}

/// An own key a computer kept before the move, handed over (M16): its
/// provider, the key, and when its person set it. A tuple on the wire.
#[derive(Serialize, Deserialize)]
pub(crate) struct Handed(pub String, pub String, pub i64);

/// `vault/sign-in`'s answer: a sign-in's nonce (its round trip's state)
/// and its verifier's S256 challenge. A tuple on the wire.
#[derive(Serialize, Deserialize)]
pub(crate) struct Begun(pub String, pub String);

/// The secrets each of `agents` (fragments of `person`'s) may use, toward
/// which hosts, one `Usable` each: what a computer's view lists and its
/// egress catches; and the own keys `person` holds (none asked: those alone).
pub(crate) async fn names(env: &Env, person: &str, agents: &[String]) -> CellResult<Names> {
    crate::routed::ask_object(env, "VAULT", person, "vault/names", &NamesAsk { agents: agents.to_vec() }).await
}

/// `person`'s own keys for `providers` (a catalog's `own` rows), each
/// opened or none, in the order asked: for their agent's request their
/// computer is swapping, never anyone else.
pub(crate) async fn own_keys(env: &Env, person: &str, providers: &[&str]) -> CellResult<Vec<Option<String>>> {
    crate::routed::ask_object(env, "VAULT", person, "vault/own-keys", &providers).await
}

/// Sets `person`'s own key for `provider` from `put` (an `OwnPut`), or
/// removes it (none): the Vault checks `provider` is an own key's and the
/// key one the swap may send. Its answer, `{provider, state}`, as it wrote
/// it.
pub(crate) async fn own_put(env: &Env, person: &str, provider: &str, put: &impl Serialize) -> CellResult<Vec<u8>> {
    crate::routed::ask_bytes(env, "VAULT", person, &op_path("own", provider)?, put).await
}

/// The own keys `person`'s computer kept before the move, handed to their
/// Vault (M16): the providers it took, each where its person had set or
/// removed none since.
pub(crate) async fn hand_over(env: &Env, person: &str, keys: &[Handed]) -> CellResult<Vec<String>> {
    crate::routed::ask_object(env, "VAULT", person, "vault/own-handover", &keys).await
}

/// A sign-in to `provider`'s own key, begun in `person`'s Vault: its nonce
/// and challenge.
pub(crate) async fn sign_in(env: &Env, person: &str, provider: &str) -> CellResult<Begun> {
    crate::routed::ask_object(env, "VAULT", person, &op_path("sign-in", provider)?, &()).await
}

/// Every own-key sign-in under way in `person`'s Vault put past its time
/// (a test fleet's lever, through their computer's): how many.
pub(crate) async fn expire_sign_ins(env: &Env, person: &str) -> CellResult<u64> {
    crate::routed::ask_object(env, "VAULT", person, "vault/expire-sign-ins", &()).await
}

/// An own-key sign-in coming back to the browser that began it, for its
/// person, whose Vault began it. It is a finished round trip, so only `of`
/// makes one, from its `Bound`: its fields are private.
pub(crate) struct SignedIn {
    person: String,
    provider: String,
    nonce: String,
}

impl SignedIn {
    /// The own-key sign-in `bound` finishes, for `person`, to `provider` (a
    /// catalog's): asked of the person's own Vault, never one a URL names.
    pub(crate) fn of(bound: crate::auth::Bound, person: &fragment_proto::Identity, provider: &str) -> SignedIn {
        assert_eq!(bound.trip(), fragment_core::roundtrip::Trip::OwnKey, "an own key's callback finishes an own key's sign-in");
        assert_eq!(person.kind, IdentityKind::Person, "an own key's sign-in is a person's");
        SignedIn { person: person.id.clone(), provider: provider.to_string(), nonce: bound.state().to_string() }
    }

    /// The person whose sign-in it is.
    pub(crate) fn person(&self) -> &str {
        &self.person
    }
}

/// The sign-in `signed` finishes, taken from its person's Vault (once, and
/// gone whatever comes of it): its verifier. One the Vault holds none of
/// was begun before the move, and is taken from their computer
/// (docs/technical-debt-ledger.md, "A computer keeps its owner's own keys
/// from before the move").
pub(crate) async fn signed_in(env: &Env, signed: &SignedIn) -> CellResult<String> {
    let asked: CellResult<String> = crate::routed::ask_object(env, "VAULT", &signed.person, &op_path("signed-in", &signed.provider)?, &signed.nonce).await;
    let verifier = match asked {
        Err(e) if e.code == ErrorCode::NotFound => {
            let computer = fragment_core::computer::default_computer_of(&signed.person);
            crate::computer::signed_in(env, &computer, &signed.provider, &signed.nonce).await?
        }
        asked => asked?,
    };
    if !own_signin::valid_token(&verifier) {
        return Err(CellError::host("a sign-in was kept with no verifier"));
    }
    Ok(verifier)
}

/// `ask`'s secrets in `person`'s Vault, each filled for its request, or
/// the refusal of the first that may not be.
pub(crate) async fn fill(env: &Env, person: &str, ask: &FillAsk) -> CellResult<Vec<Filled>> {
    let filled: Fills = crate::routed::ask_object(env, "VAULT", person, "vault/fill", ask).await?;
    Ok(filled.secrets)
}

/// `vault/<op>[/<name>]` on `person`'s Vault, `body` its JSON: its answer,
/// as it wrote it.
async fn ask(env: &Env, person: &str, op: &str, body: Option<&RawValue>) -> CellResult<Vec<u8>> {
    crate::routed::ask_bytes(env, "VAULT", person, op, &body).await
}

/// `vault/<op>/<name>`, `name` a secret's name's form
/// (`fragment_core::vault::name_form`), refused (400) before any route is
/// built: a caller's name never carries a dot segment, separator or escape
/// that would choose another op (`../delete/…`).
fn op_path(op: &str, name: &str) -> CellResult<String> {
    rules::name_form(name).map_err(|e| CellError::invalid(e.to_string()))?;
    Ok(format!("vault/{op}/{name}"))
}

/// Puts `put` (a `PutSecret`'s JSON) into `person`'s Vault under `name`,
/// sealed there, under the rules of the person's own `PUT
/// /api/vault/{name}`; set again, it is replaced whole (with no value, all
/// but its value). The seam for the
/// platform's own code (FIN-463's hand-off): its answer is the `Secret`
/// set, as JSON, never its value.
pub(crate) async fn put(env: &Env, person: &str, name: &str, put: &RawValue) -> CellResult<Vec<u8>> {
    ask(env, person, &op_path("put", name)?, Some(put)).await
}

/// `vault/handoff/<name>`'s body: the hand-off's receipt (`<old> <kind>`)
/// and its secret. A tuple on the wire.
#[derive(Serialize, Deserialize)]
struct HandOff<P>(String, P);

/// Puts `put` into `person`'s Vault under `name` for an old agent's
/// hand-off (FIN-463 F11), once for `receipt`: its receipt kept in the
/// turn that puts it, so the same hand-off again (a retry, or a call
/// beside it) puts nothing, even once its person removed, moved or set
/// that secret since. It never takes the name of a secret bound to
/// another agent (409).
pub(crate) async fn hand_off(env: &Env, person: &str, name: &str, receipt: &str, put: &PutSecret) -> CellResult<Vec<u8>> {
    crate::routed::ask_bytes(env, "VAULT", person, &op_path("handoff", name)?, &HandOff(receipt.to_string(), put)).await
}

/// What a move reads of a kept `Secret`: how and where it goes.
#[derive(serde::Deserialize)]
struct Placed {
    place: Place,
    #[serde(default)]
    hosts: Vec<String>,
}

/// `vault/take`'s body: the destination it claims for, and the hosts the
/// move keeps it toward there (`fragment_core::vault::within`). A tuple on
/// the wire.
#[derive(serde::Serialize, serde::Deserialize)]
struct Take(String, Vec<String>);

/// `vault/take`'s answer: a secret's value, for its move, and which value
/// it is (its revision: its sealed text's SHA-256). A tuple on the wire.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Taken(pub String, pub String);

/// `vault/moved`'s body: the destination a take claimed for, and the
/// revision it answered. A tuple on the wire.
#[derive(serde::Serialize, serde::Deserialize)]
struct Moved(String, String);

/// `name`'s value in `person`'s Vault, for its move into `to` (`fragment/
/// KEY`, members.rs `fromVault`), claimed for `to` as it is read
/// (`fragment_core::vault::Claimed`: another destination's take is refused,
/// 409, while it stands). The mover keeps it, then completes (`moved`), so
/// it has one home at the end and is never lost between. A Nostr key is
/// refused: it never leaves; and so is a move toward a host its Vault does
/// not point it at (`hosts`, 400), before anything is claimed.
pub(crate) async fn take(env: &Env, person: &str, name: &str, to: &str, hosts: &[String]) -> CellResult<Taken> {
    crate::routed::ask_object(env, "VAULT", person, &op_path("take", name)?, &Take(to.to_string(), hosts.to_vec())).await
}

/// `to`'s move of `name`, kept there, completed: removed from `person`'s
/// Vault while it still holds the value `take` answered as `revision`,
/// claimed for `to` (one its person set since stays). Again, it removes
/// nothing.
pub(crate) async fn moved(env: &Env, person: &str, name: &str, to: &str, revision: &str) -> CellResult<()> {
    crate::routed::ask_bytes(env, "VAULT", person, &op_path("moved", name)?, &Moved(to.to_string(), revision.to_string())).await.map(drop)
}

/// Removes `name` from `person`'s Vault: its answer, a `SecretRemoved`.
pub(crate) async fn delete(env: &Env, person: &str, name: &str) -> CellResult<Vec<u8>> {
    ask(env, person, &op_path("delete", name)?, None).await
}

/// `/api/vault[/{name}]`, signed by `signed`: the person's own, never an
/// agent's (their agents use its secrets, never set or list them). The
/// router finds their Vault, hands it the body as it came, and passes its
/// answer on.
pub(crate) async fn route(env: &Env, signed: &Signed, method: Method, rest: &[&str], body: &[u8]) -> CellResult<Response> {
    // an ask's routes are a POST, or under an id: `PUT` and `DELETE
    // /api/vault/asks` stay a secret named `asks`'
    if rest.first() == Some(&"asks") && (method == Method::Post || rest.len() > 1) {
        return asks(env, signed, method, &rest[1..], body).await;
    }
    if signed.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "a vault is its person's alone: their agents use its secrets, and never set, list or remove one"));
    }
    let person = signed.identity.id.as_str();
    let answer = match (method, rest) {
        (Method::Get, []) => return json_bytes(ask(env, person, "vault/list", None).await?),
        (Method::Put, [name]) => {
            let raw: Box<RawValue> = crate::json_body(body)?;
            put(env, person, name, &raw).await?
        }
        (Method::Delete, [name]) => delete(env, person, name).await?,
        (m, _) => return Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/vault/{}", m.as_ref(), rest.join("/")))),
    };
    // set or removed: a waiting ask's card may change with it (Save, or Decline alone)
    tell(env, person).await;
    json_bytes(answer)
}

/// What an ask carries into its person's Vault: the agent fragment that
/// asked, what its card calls it (its title, else its label), its
/// picture, and the ask. A tuple on the wire.
#[derive(Serialize, Deserialize)]
struct Asked(String, String, Option<String>, AskSecret);

/// `vault/asked/<id>`'s body: the agent reading its own ask. A tuple.
#[derive(Serialize, Deserialize)]
struct AskedBy(String);

/// An ask's id: 32 lower-case hex, refused (404) before any route is
/// built from it.
fn ask_id(id: &str) -> CellResult<&str> {
    match rules::ask_id_ok(id) {
        true => Ok(id),
        false => Err(CellError::new(ErrorCode::NotFound, format!("{id:?} is no ask"))),
    }
}

/// Who an agent's ask is: the agent `signed` is, its owner, its fragment,
/// and (`named`: as its card shows it) its title, else its label, and its
/// picture. Its reads of an ask name none (it polls).
async fn asker(env: &Env, signed: &Signed, named: bool) -> CellResult<(String, String, String, Option<String>)> {
    if signed.identity.kind != IdentityKind::Agent {
        return Err(CellError::new(ErrorCode::Forbidden, "an ask is an agent's, of its person: you set your own secrets (PUT /api/vault/{secret})"));
    }
    let id = signed.identity.id.as_str();
    let owner = signed.identity.owner.clone().ok_or_else(|| CellError::host(format!("agent {id} has no owner")))?;
    let mut profiles = crate::ask_registry(env, &crate::registry::calls::Profiles { ids: vec![id.to_string()], emails_of: vec![] }).await?.profiles;
    if named {
        crate::share::name_agents(env, &mut profiles).await;
    }
    let p = profiles.remove(id);
    let fragment = p.as_ref().and_then(|p| p.fragment.clone()).ok_or_else(|| CellError::new(ErrorCode::Conflict, format!("{id} is no agent made from an agent fragment: only such an agent asks its person for a secret")))?;
    let label = fragment.split("--").next().unwrap_or(&fragment).to_string();
    let title = p.as_ref().and_then(|p| p.title.clone().or_else(|| p.name.clone())).unwrap_or(label);
    Ok((owner, fragment, title, p.and_then(|p| p.picture)))
}

/// Tells `person`'s open pages that what they are told changed (an ask
/// made or ended): logged, never failed; a page that missed it reads its
/// notices again as it reconnects.
async fn tell(env: &Env, person: &str) {
    if let Err(e) = crate::principal::ask(env, person, &crate::principal::calls::Changed {}).await {
        console_error!("{}", serde_json::json!({ "vault": "told", "error": e.message }));
    }
}

/// `/api/vault/asks[/{id}[/decline]]`: an agent asks its person (`POST`)
/// and reads how its ask stands (`GET {id}`); its person answers (`POST
/// {id}`, their paste) or declines (`POST {id}/decline`) from the
/// card the platform draws. Each answer is a `SecretAsk`, never a value.
async fn asks(env: &Env, signed: &Signed, method: Method, rest: &[&str], body: &[u8]) -> CellResult<Response> {
    let answer = match (method, rest) {
        (Method::Post, []) => {
            let (owner, fragment, title, picture) = asker(env, signed, true).await?;
            let ask: AskSecret = crate::json_body(body)?;
            let made = crate::routed::ask_bytes(env, "VAULT", &owner, "vault/ask", &Asked(fragment, title, picture, ask)).await?;
            tell(env, &owner).await;
            made
        }
        (Method::Get, [id]) => {
            let (owner, fragment, ..) = asker(env, signed, false).await?;
            crate::routed::ask_bytes(env, "VAULT", &owner, &format!("vault/asked/{}", ask_id(id)?), &AskedBy(fragment)).await?
        }
        (Method::Post, [id, verb @ ..]) if verb.is_empty() || verb == ["decline"] => {
            if signed.identity.kind != IdentityKind::Person {
                return Err(CellError::new(ErrorCode::Forbidden, "only the person asked answers an ask, from the platform's own page: never an agent"));
            }
            let person = signed.identity.id.as_str();
            let id = ask_id(id)?;
            let ended = match verb.is_empty() {
                true => {
                    let a: AnswerAsk = crate::json_body(body)?;
                    crate::routed::ask_bytes(env, "VAULT", person, &format!("vault/answer/{id}"), &a).await?
                }
                false => crate::routed::ask_bytes(env, "VAULT", person, &format!("vault/decline/{id}"), &()).await?,
            };
            tell(env, person).await;
            ended
        }
        (m, _) => return Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/vault/asks/{}", m.as_ref(), rest.join("/")))),
    };
    json_bytes(answer)
}

/// `GET /api/notices`, the Vault's part (`notices::route`): `person`'s
/// asks still waiting, each a card with its field, or, for a client that
/// draws none (`fields` false), its link (`rules::linked`). Its bytes, a
/// `NoticeList`.
pub(crate) async fn notices(env: &Env, person: &str, fields: bool) -> CellResult<Vec<u8>> {
    ask(env, person, if fields { "vault/notices" } else { "vault/notices/linked" }, None).await
}

// exported by the cell's build alone (lib.rs, "Two Workers, one crate")
#[cfg_attr(not(feature = "admin"), durable_object)]
pub struct VaultCell {
    state: State,
    env: Env,
    cfg: &'static Config,
    schema: crate::schema::Schema,
    wiped: Cell<bool>,
}

#[cfg(not(feature = "admin"))]
impl DurableObject for VaultCell {
    fn new(state: State, env: Env) -> Self {
        let raw: JsValue = state._inner().into();
        let state = State::from(raw.clone().unchecked_into::<worker_sys::DurableObjectState>());
        let sql = state.storage().sql();
        let schema = crate::schema::Schema::start(&raw, sql.clone(), &fragment_core::migrate::VAULT, &state.id().to_string());
        let wiped = schema.wiped(&sql);
        let cfg = Config::from_env(&env);
        VaultCell { state, env, cfg, schema, wiped: Cell::new(wiped) }
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
}

impl VaultCell {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn rows(&self, query: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<Value>> {
        Ok(self.sql().exec(query, binds)?.to_array()?)
    }

    fn scope(&self) -> String {
        crate::keys::scope(SEAL_CLASS, &self.state)
    }

    async fn route(&self, mut req: Request) -> CellResult<Response> {
        let path = req.path();
        // a wipe ends a vault that refuses its schema too
        if let Some(route) = path.strip_prefix("/wipe/") {
            // only a wipe's orchestrator sets the header (routed.rs `marker`)
            if req.headers().get(crate::wipe::WIPE_HEADER)?.is_none() {
                return Err(CellError::new(ErrorCode::NotFound, format!("no route /wipe/{route}")));
            }
            return Ok(Response::from_bytes(self.wipe(route)?.into_bytes())?);
        }
        if req.headers().get(VAULT_HEADER)?.is_none() {
            return Err(CellError::new(ErrorCode::NotFound, format!("no route {path}")));
        }
        self.schema.ready()?;
        let answer = match path.trim_start_matches('/').split('/').collect::<Vec<_>>()[..] {
            ["vault", "put", name] => self.set(name, crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "handoff", name] => {
                let HandOff(receipt, put) = crate::json_body(&req.bytes().await?)?;
                self.put(name, &put, Some(&receipt), None).await?
            }
            ["vault", "list"] => self.list()?,
            ["vault", "delete", name] => self.delete(name)?,
            ["vault", "take", name] => {
                let Take(to, hosts) = crate::json_body(&req.bytes().await?)?;
                self.take(name, &to, &hosts).await?
            }
            ["vault", "moved", name] => {
                let Moved(to, revision) = crate::json_body(&req.bytes().await?)?;
                self.moved(name, &to, &revision)?
            }
            ["vault", "names"] => self.names(&crate::json_body(&req.bytes().await?)?)?,
            ["vault", "ask"] => self.ask(crate::json_body(&req.bytes().await?)?)?,
            ["vault", "asked", id] => {
                let AskedBy(agent) = crate::json_body(&req.bytes().await?)?;
                self.asked(id, &agent)?
            }
            ["vault", "answer", id] => self.answer(id, crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "decline", id] => self.decline(id)?,
            ["vault", "notices"] => self.notices(true)?,
            ["vault", "notices", "linked"] => self.notices(false)?,
            ["vault", "fill"] => self.fill(&crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "own", provider] => self.own_put(provider, crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "own-keys"] => self.own_keys(crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "own-handover"] => self.hand_over(crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "sign-in", provider] => self.sign_in(provider).await?,
            ["vault", "signed-in", provider] => self.signed_in(provider, &crate::json_body::<String>(&req.bytes().await?)?).await?,
            ["vault", "expire-sign-ins"] if self.cfg.test_hooks => {
                // as ten minutes would leave them: each row stays, refused by its expiry
                self.sql().exec("UPDATE own_signins SET expires_at = ?", vec![SqlStorageValue::Integer(js::now_ms())])?;
                self.count("SELECT COUNT(*) AS n FROM own_signins")?.to_string()
            }
            _ => return Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))),
        };
        Ok(Response::from_bytes(answer.into_bytes())?)
    }

    /// `put` checked, sealed and kept under `name`: its `Secret` as JSON;
    /// with no value, `place`. A hand-off's (`receipt`) is put once, its receipt kept in the same
    /// turn (two writes with no wait between, which the object commits
    /// together); again, it puts nothing and answers `{}`. Nor does it take
    /// the name of a secret bound to other agents.
    async fn put(&self, name: &str, put: &PutSecret, receipt: Option<&str>, still: Option<&dyn Fn() -> CellResult<()>>) -> CellResult<String> {
        let wiped = || CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing");
        if self.wiped.get() {
            return Err(wiped());
        }
        let received = |r: &str| -> CellResult<bool> { Ok(!self.rows("SELECT key FROM receipts WHERE key = ?", vec![r.into()])?.is_empty()) };
        if receipt.map(received).transpose()? == Some(true) {
            return Ok("{}".into());
        }
        let Some(value) = put.value.as_deref() else {
            if receipt.is_some() {
                return Err(CellError::invalid("a hand-off carries its value"));
            }
            return self.place(name, put);
        };
        // with a value, what is held is not read by the rules
        let checked = rules::check(name, put, None, &self.cfg.providers).map_err(|e| CellError::invalid(e.to_string()))?;
        if checked.placing == Placing::Nip98 && fragment_nip98::Keys::from_secret_hex(value).is_none() {
            return Err(CellError::invalid(format!("a secret's value: {}", rules::NOSTR_KEY)));
        }
        // sealed first (reading the host secret may yield): the checks and
        // the write below share one turn, and a wipe that ended meanwhile
        // stops it
        let sealed = crate::keys::seal(&self.env, &self.scope(), value.as_bytes()).await?;
        if self.wiped.get() {
            return Err(wiped());
        }
        // what its caller read before the wait, read again in the turn that writes
        if let Some(still) = still {
            still()?;
        }
        let existing = self.rows("SELECT secret FROM secrets WHERE name = ?", vec![name.into()])?;
        if let Some(r) = receipt {
            // a hand-off beside this one put it while this one sealed
            if received(r)? {
                return Ok("{}".into());
            }
            if let Some(row) = existing.first() {
                let held: Bound = stored(row)?;
                if held.agents != checked.agents {
                    return Err(CellError::new(ErrorCode::Conflict, format!("{name} is a secret of other agents' here: a hand-off never takes its place")));
                }
            }
        }
        let exists = !existing.is_empty();
        let held = self.rows("SELECT COUNT(*) AS n FROM secrets", vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        if !exists && held >= rules::SECRETS_MAX as u64 {
            return Err(CellError::invalid(format!("a vault keeps at most {} secrets: remove one first", rules::SECRETS_MAX)));
        }
        let secret = Secret { name: name.to_string(), hosts: checked.hosts, place: checked.place, agents: checked.agents, set_at: js::now_ms(), revision: None };
        let text = serde_json::to_string(&secret).map_err(|e| CellError::host(format!("a secret's JSON: {e}")))?;
        self.sql().exec(
            "INSERT INTO secrets (name, sealed, secret) VALUES (?, ?, ?) ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, secret = excluded.secret, claim = NULL",
            vec![name.into(), sealed.into(), text.as_str().into()],
        )?;
        if let Some(r) = receipt {
            self.sql().exec("INSERT INTO receipts (key, at) VALUES (?, ?)", vec![r.into(), SqlStorageValue::Integer(js::now_ms())])?;
        }
        Ok(text)
    }

    /// Its person's `PUT /api/vault/{name}`: `put`, and with its `held`,
    /// only onto the secret as they read it (a new one, for `none`): one
    /// changed since is refused (409), nothing written. The seal may yield,
    /// so it is checked again in the turn that writes.
    async fn set(&self, name: &str, put: PutSecret) -> CellResult<String> {
        let Some(read) = put.held.as_deref() else { return self.put(name, &put, None, None).await };
        let still = || -> CellResult<()> {
            match self.held_at(name)?.1 == read {
                true => Ok(()),
                false if read == "none" => Err(CellError::new(ErrorCode::Conflict, format!("you already keep {name}: open it to change it"))),
                false => Err(CellError::new(ErrorCode::Conflict, format!("{name} changed since you read it: look at it again"))),
            }
        };
        still()?;
        self.put(name, &put, None, Some(&still)).await
    }

    /// `name`, set again with no value: the value held kept as it is, never
    /// opened (its seal, so its revision and any claim on it, untouched),
    /// and only where it goes set, its `setAt` its value's own. One turn:
    /// nothing here yields, so nothing is put, moved or removed between
    /// its read and its write. One not held is refused, never made.
    fn place(&self, name: &str, put: &PutSecret) -> CellResult<String> {
        let rows = self.rows("SELECT secret FROM secrets WHERE name = ?", vec![name.into()])?;
        let held: Option<Secret> = rows.first().map(stored).transpose()?;
        let checked = rules::check(name, put, held.as_ref().map(|s| &s.place), &self.cfg.providers).map_err(|e| CellError::invalid(e.to_string()))?;
        let held = held.ok_or_else(|| CellError::host("the rules kept a value for a secret not held"))?;
        let secret = Secret { name: name.to_string(), hosts: checked.hosts, place: checked.place, agents: checked.agents, set_at: held.set_at, revision: None };
        let text = serde_json::to_string(&secret).map_err(|e| CellError::host(format!("a secret's JSON: {e}")))?;
        self.sql().exec("UPDATE secrets SET secret = ? WHERE name = ?", vec![text.as_str().into(), name.into()])?;
        Ok(text)
    }

    /// Every secret held, by name, as `SecretList`'s JSON: what each says
    /// and its revision, never a value.
    fn list(&self) -> CellResult<String> {
        let rows = self.rows("SELECT sealed, secret FROM secrets ORDER BY name", vec![])?;
        assert!(rows.len() <= rules::SECRETS_MAX, "a vault keeps at most SECRETS_MAX secrets");
        let mut secrets = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut secret: Secret = stored(row)?;
            secret.revision = Some(revision_at(row));
            secrets.push(secret);
        }
        serde_json::to_string(&fragment_proto::vault::SecretList { secrets }).map_err(|e| CellError::host(e.to_string()))
    }

    /// `query`'s one count, `n`.
    fn count(&self, query: &str) -> CellResult<u64> {
        Ok(self.rows(query, vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0))
    }

    /// Which secrets each of `b.agents` may use, toward which hosts (a
    /// `Usable` each), and the own keys held: `Names` as JSON.
    fn names(&self, b: &NamesAsk) -> CellResult<String> {
        let mut usable = vec![];
        for r in self.rows("SELECT name, secret FROM secrets ORDER BY name", vec![])? {
            let s: Bound = stored(&r)?;
            if s.hosts.is_empty() {
                continue;
            }
            let name = r["name"].as_str().unwrap_or_default();
            for a in s.agents.iter().filter(|a| b.agents.contains(a)) {
                usable.push(Usable(a.clone(), name.to_string(), s.hosts.clone()));
            }
        }
        let own = self.rows("SELECT provider FROM own_keys WHERE sealed IS NOT NULL ORDER BY provider", vec![])?;
        let own = own.iter().filter_map(|r| r["provider"].as_str().map(str::to_string)).collect();
        serde_json::to_string(&Names(usable, own)).map_err(|e| CellError::host(e.to_string()))
    }

    /// Its person's own key for `provider` set from `put`, or removed (none,
    /// kept as removed: a computer's hand-over from before never puts one
    /// back), with the rules of `PUT …/key`: an own key's provider, and a
    /// key the swap may send. `OwnSet`'s JSON.
    async fn own_put(&self, provider: &str, put: Option<OwnPut>) -> CellResult<String> {
        if self.cfg.providers.get(provider).is_none_or(|p| p.kind != Kind::Own) {
            return Err(CellError::invalid(format!("{provider} takes no key of yours: only an own key's provider does")));
        }
        let key = match put {
            Some(put) => Some(own_signin::given(&put.key).ok_or_else(|| CellError::invalid(format!("a key is 1 to {} printable characters, with no space", own_signin::OWN_KEY_MAX_BYTES)))?),
            None => None,
        };
        let state = if key.is_some() { ProviderState::Set } else { ProviderState::NotSet };
        self.keep_own(vec![(provider.to_string(), key, js::now_ms())], false).await?;
        serde_json::to_string(&OwnSet { provider, state }).map_err(|e| CellError::host(e.to_string()))
    }

    /// The own keys a computer kept before the move (M16), each taken where
    /// its person has set or removed none here since (`keep_own`). The
    /// providers it took, as JSON. Taken again, it takes nothing. One no
    /// swap could send (no provider's name, or no printable key: none such
    /// was ever kept) is not taken, and logged by its provider alone; the
    /// rest are (Codex, #1345: one never stops the others).
    async fn hand_over(&self, keys: Vec<Handed>) -> CellResult<String> {
        if keys.len() > rules::SECRETS_MAX {
            return Err(CellError::invalid(format!("a hand-over is at most {} own keys", rules::SECRETS_MAX)));
        }
        let (keys, unsendable): (Vec<Handed>, Vec<Handed>) = keys.into_iter().partition(|Handed(p, k, _)| fragment_core::catalog::valid_name(p) && own_signin::key_ok(k));
        for Handed(provider, _, _) in &unsendable {
            console_error!("{{\"vault\":\"own-handover\",\"unsendable\":{:?}}}", provider.chars().take(64).collect::<String>());
        }
        let taken = self.keep_own(keys.into_iter().map(|Handed(p, k, at)| (p, Some(k), at)).collect(), true).await?;
        serde_json::to_string(&taken).map_err(|e| CellError::host(e.to_string()))
    }

    /// Own keys kept, each `(provider, key, set_at)`: sealed for this object,
    /// or `None` kept as removed (so a hand-over never puts it back). A
    /// hand-over's (`new_only`) only where nothing of its provider is held,
    /// checked again in the turn that keeps it, after the seals' waits, so
    /// what its person set meanwhile stands; and nothing once a wipe ended.
    /// The providers kept.
    async fn keep_own(&self, keys: Vec<(String, Option<String>, i64)>, new_only: bool) -> CellResult<Vec<String>> {
        let held = |p: &str| -> CellResult<bool> { Ok(new_only && !self.rows("SELECT provider FROM own_keys WHERE provider = ?", vec![p.into()])?.is_empty()) };
        let mut sealed = Vec::with_capacity(keys.len());
        for (provider, key, set_at) in keys {
            if held(&provider)? {
                continue;
            }
            let key = match key {
                Some(k) => Some(crate::keys::seal(&self.env, &self.scope(), k.as_bytes()).await?),
                None => None,
            };
            sealed.push((provider, key, set_at));
        }
        if self.wiped.get() {
            return Err(CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing"));
        }
        let mut kept = vec![];
        for (provider, key, set_at) in sealed {
            if held(&provider)? {
                continue;
            }
            self.sql().exec(
                "INSERT INTO own_keys (provider, sealed, set_at) VALUES (?, ?, ?) ON CONFLICT (provider) DO UPDATE SET sealed = excluded.sealed, set_at = excluded.set_at",
                vec![provider.as_str().into(), key.into(), SqlStorageValue::Integer(set_at)],
            )?;
            kept.push(provider);
        }
        Ok(kept)
    }

    /// Its person's own keys for `providers`, each opened or none, as JSON:
    /// for their computer's swap alone, which asks them as a request goes.
    async fn own_keys(&self, providers: Vec<String>) -> CellResult<String> {
        if providers.len() > fragment_core::swap::PLACEHOLDERS_MAX {
            return Err(CellError::invalid(format!("ask for at most {} own keys", fragment_core::swap::PLACEHOLDERS_MAX)));
        }
        let mut keys: Vec<Option<String>> = Vec::with_capacity(providers.len());
        for provider in &providers {
            let rows = self.rows("SELECT sealed FROM own_keys WHERE provider = ? AND sealed IS NOT NULL", vec![provider.as_str().into()])?;
            let Some(sealed) = rows.first().and_then(|r| r["sealed"].as_str()).map(str::to_string) else {
                keys.push(None);
                continue;
            };
            let opened = crate::keys::open(&self.env, &self.scope(), &sealed).await?;
            if let Some(resealed) = opened.resealed {
                // only over the value opened: its person may have set another meanwhile
                self.sql().exec("UPDATE own_keys SET sealed = ? WHERE provider = ? AND sealed = ?", vec![resealed.into(), provider.as_str().into(), sealed.into()])?;
            }
            keys.push(Some(String::from_utf8(opened.plaintext).map_err(|_| CellError::host("an own key that is not text"))?));
        }
        serde_json::to_string(&keys).map_err(|e| CellError::host(e.to_string()))
    }

    /// A sign-in to `provider`'s own key, begun (`fragment_core::
    /// own_signin`): a nonce and a verifier made, the verifier kept sealed
    /// under the nonce's hash until it expires; `Begun`'s JSON. Expired ones
    /// go first; with `SIGNINS_PENDING_MAX` under way, another is refused.
    async fn sign_in(&self, provider: &str) -> CellResult<String> {
        if self.cfg.providers.get(provider).is_none_or(|p| p.kind != Kind::Own || p.oauth.is_none()) {
            return Err(CellError::invalid(format!("{provider:?} is no own key signed in to here")));
        }
        let nonce = own_signin::nonce(&js::random_bytes::<{ own_signin::NONCE_BYTES }>());
        let verifier = own_signin::b64url(&js::random_bytes::<{ own_signin::VERIFIER_BYTES }>());
        let sealed = crate::keys::seal(&self.env, &self.scope(), verifier.as_bytes()).await?;
        // counted and kept in one turn, after the seal's wait
        if self.wiped.get() {
            return Err(CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing"));
        }
        let now = js::now_ms();
        self.sql().exec("DELETE FROM own_signins WHERE expires_at <= ?", vec![SqlStorageValue::Integer(now)])?;
        let pending = self.count("SELECT COUNT(*) AS n FROM own_signins")?;
        if pending >= own_signin::SIGNINS_PENDING_MAX as u64 {
            return Err(CellError::new(ErrorCode::RateLimited, format!("{pending} sign-ins are under way: finish one, or wait {} minutes", own_signin::SIGNIN_TTL_MS / 60_000)));
        }
        self.sql().exec(
            "INSERT INTO own_signins (nonce, provider, sealed, expires_at) VALUES (?, ?, ?, ?)",
            vec![own_signin::nonce_key(&nonce).into(), provider.into(), sealed.into(), SqlStorageValue::Integer(now + own_signin::SIGNIN_TTL_MS)],
        )?;
        let challenge = own_signin::challenge(&verifier);
        serde_json::to_string(&Begun(nonce, challenge)).map_err(|e| CellError::host(e.to_string()))
    }

    /// The sign-in `nonce` names, finished: gone at once (a replay finds
    /// nothing), and its verifier answered (JSON) if it was `provider`'s
    /// and is unexpired. None here at all is 404: one begun before the move
    /// is its computer's (`signed_in`, the router's).
    async fn signed_in(&self, provider: &str, nonce: &str) -> CellResult<String> {
        let key = own_signin::nonce_key(nonce);
        let rows = self.rows("SELECT provider, sealed, expires_at FROM own_signins WHERE nonce = ?", vec![key.as_str().into()])?;
        self.sql().exec("DELETE FROM own_signins WHERE nonce = ?", vec![key.into()])?;
        let row = rows.first().ok_or_else(|| CellError::new(ErrorCode::NotFound, "no sign-in of this one's is under way here"))?;
        if row["provider"] != provider || row["expires_at"].as_i64().is_none_or(|t| t <= js::now_ms()) {
            return Err(CellError::invalid("this sign-in is not one under way here (finished already, expired, or never made): connect again from settings"));
        }
        let sealed = row["sealed"].as_str().ok_or_else(|| CellError::host("a sign-in kept with no verifier"))?;
        let opened = crate::keys::open(&self.env, &self.scope(), sealed).await?;
        let verifier = String::from_utf8(opened.plaintext).map_err(|_| CellError::host("a sign-in's verifier that is not text"))?;
        serde_json::to_string(&verifier).map_err(|e| CellError::host(e.to_string()))
    }

    /// Each secret `b` names, filled for `b.agent`'s request to `b.host`:
    /// one bound to that agent, toward one of its hosts, as `Fills`' JSON. A
    /// value opens here and goes to the egress alone; a Nostr key signs
    /// here, and never leaves.
    async fn fill(&self, b: &FillAsk) -> CellResult<String> {
        if b.names.is_empty() || b.names.len() > fragment_core::swap::PLACEHOLDERS_MAX {
            return Err(CellError::invalid(format!("fill 1 to {} secrets", fragment_core::swap::PLACEHOLDERS_MAX)));
        }
        let mut out = Vec::with_capacity(b.names.len());
        for name in &b.names {
            let rows = self.rows("SELECT sealed, secret FROM secrets WHERE name = ?", vec![name.as_str().into()])?;
            let not_its = || CellError::new(ErrorCode::NotConnected, format!("{name} is no secret of {}'s: its person keeps it in their vault, for this agent (PUT /api/vault/{name})", b.agent));
            let row = rows.first().ok_or_else(not_its)?;
            let s: Bound = stored(row)?;
            if !s.agents.contains(&b.agent) {
                return Err(not_its());
            }
            if s.hosts.is_empty() {
                return Err(CellError::new(ErrorCode::NotConnected, format!("{name} is parked: it goes to no host until its person sets one")));
            }
            if !s.hosts.contains(&b.host) {
                return Err(CellError::new(ErrorCode::Forbidden, format!("{name} goes to {}, not {}", s.hosts.join(", "), b.host)));
            }
            let (placing, _) = rules::placing(&s.place).map_err(|e| CellError::host(format!("a stored secret's place: {e}")))?;
            let sealed = row["sealed"].as_str().unwrap_or_default().to_string();
            let opened = crate::keys::open(&self.env, &self.scope(), &sealed).await?;
            if let Some(resealed) = opened.resealed {
                // only over the value opened: its person may have set another meanwhile
                self.sql().exec("UPDATE secrets SET sealed = ? WHERE name = ? AND sealed = ?", vec![resealed.into(), name.as_str().into(), sealed.into()])?;
            }
            let value = String::from_utf8(opened.plaintext).map_err(|_| CellError::host("a secret that is not text"))?;
            let (secret, signed) = match placing {
                Placing::Fill(_) => (value, false),
                Placing::Nip98 => (self.sign(&value, b)?, true),
            };
            out.push(Filled(secret, s.place, signed));
        }
        serde_json::to_string(&Fills { secrets: out }).map_err(|e| CellError::host(e.to_string()))
    }

    /// A NIP-98 event over `b`'s request, by the Nostr key `key`, as it goes
    /// in `Authorization: Nostr {}`: made now, its URL the request's to its
    /// host.
    fn sign(&self, key: &str, b: &FillAsk) -> CellResult<String> {
        let url = url::Url::parse(&b.url).map_err(|e| CellError::invalid(format!("a signed request's URL: {e}")))?;
        if url.scheme() != "https" || url.host_str() != Some(b.host.as_str()) {
            return Err(CellError::invalid(format!("a signed request goes over https to {}, not {}", b.host, b.url)));
        }
        let method = b.method.to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS") {
            return Err(CellError::invalid(format!("no method {method:?} to sign")));
        }
        if let Some(sha) = &b.body_sha256 {
            if sha.len() != 64 || !sha.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
                return Err(CellError::invalid("a signed request's body hash is 64 hex"));
            }
        }
        let keys = fragment_nip98::Keys::from_secret_hex(key).ok_or_else(|| CellError::host("a stored Nostr key that is no key"))?;
        let header = keys.header_for_payload(&method, url.as_str(), b.body_sha256.as_deref(), js::now_ms() / 1000);
        let event = header.strip_prefix("Nostr ").ok_or_else(|| CellError::host("a NIP-98 header is `Nostr <event>`"))?;
        Ok(event.to_string())
    }

    /// `name`'s value, opened for its move into `to`, and its revision:
    /// `Taken`'s JSON. Claimed for `to` in the turn that reads it, before
    /// anything yields (`fragment_core::vault::take`). None, a Nostr key,
    /// one toward a host its row does not name, or one claimed for another
    /// destination, refused.
    async fn take(&self, name: &str, to: &str, hosts: &[String]) -> CellResult<String> {
        let rows = self.rows("SELECT sealed, secret, claim FROM secrets WHERE name = ?", vec![name.into()])?;
        let row = rows.first().ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no secret {name} in your vault (GET /api/vault lists them)")))?;
        let kept: Placed = crate::json_text(row["secret"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored secret: {e}")))?;
        if kept.place.sign.is_some() {
            return Err(CellError::invalid(format!("{name} is a key that signs: it never leaves your vault")));
        }
        rules::within(name, &kept.hosts, hosts).map_err(|e| CellError::invalid(e.to_string()))?;
        rules::take(name, row["claim"].as_str(), to).map_err(|e| CellError::new(ErrorCode::Conflict, e.to_string()))?;
        let sealed = row["sealed"].as_str().unwrap_or_default().to_string();
        self.sql().exec("UPDATE secrets SET claim = ? WHERE name = ? AND sealed = ?", vec![to.into(), name.into(), sealed.as_str().into()])?;
        let opened = crate::keys::open(&self.env, &self.scope(), &sealed).await?;
        let value = String::from_utf8(opened.plaintext).map_err(|_| CellError::host("a secret that is not text"))?;
        serde_json::to_string(&Taken(value, revision_of(&sealed))).map_err(|e| CellError::host(e.to_string()))
    }

    /// `name` removed: `SecretRemoved`'s JSON.
    fn delete(&self, name: &str) -> CellResult<String> {
        let removed = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![name.into()])?.is_empty();
        self.sql().exec("DELETE FROM secrets WHERE name = ?", vec![name.into()])?;
        serde_json::to_string(&SecretRemoved { name: name.to_string(), removed }).map_err(|e| CellError::host(e.to_string()))
    }

    /// `to`'s move of `name` completed: removed while it is claimed for
    /// `to` and holds `revision` (`fragment_core::vault::completes`), in one
    /// turn. `SecretRemoved`'s JSON.
    fn moved(&self, name: &str, to: &str, revision: &str) -> CellResult<String> {
        let rows = self.rows("SELECT sealed, claim FROM secrets WHERE name = ?", vec![name.into()])?;
        let held = rows.first().and_then(|r| Some((r["sealed"].as_str()?, r["claim"].as_str())));
        let removed = held.is_some_and(|(sealed, claim)| rules::completes(claim, &revision_of(sealed), to, revision));
        if removed {
            self.sql().exec("DELETE FROM secrets WHERE name = ?", vec![name.into()])?;
        }
        serde_json::to_string(&SecretRemoved { name: name.to_string(), removed }).map_err(|e| CellError::host(e.to_string()))
    }

    /// Asks that waited past their time, ended so; and those ended an
    /// hour, forgotten. Each ask's read and write runs it first.
    fn expire(&self, now: i64) -> CellResult<()> {
        self.sql().exec("UPDATE asks SET state = 'expired' WHERE state = 'waiting' AND expires <= ?", vec![SqlStorageValue::Integer(now)])?;
        self.sql().exec("DELETE FROM asks WHERE state != 'waiting' AND expires <= ?", vec![SqlStorageValue::Integer(now - rules::ASK_KEPT_MS)])?;
        Ok(())
    }

    /// The secret held under `name` and which one it is: its revision, the
    /// SHA-256 of its seal and what it says together (a new value or a
    /// change to its hosts, place or agents changes it), `"none"` for none.
    fn held_at(&self, name: &str) -> CellResult<(Option<Secret>, String)> {
        let rows = self.rows("SELECT sealed, secret FROM secrets WHERE name = ?", vec![name.into()])?;
        let Some(row) = rows.first() else { return Ok((None, "none".into())) };
        Ok((Some(stored(row)?), revision_at(row)))
    }

    /// The ask `id`, if one is kept: its agent, what it asks, its expiry
    /// and its state, read after `expire`.
    fn ask_row(&self, id: &str) -> CellResult<Option<(Asked, i64, AskState)>> {
        let now = js::now_ms();
        self.expire(now)?;
        let Some(row) = self.rows("SELECT ask, expires, state FROM asks WHERE id = ?", vec![id.into()])?.into_iter().next() else { return Ok(None) };
        let asked: Asked = crate::json_text(row["ask"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored ask: {e}")))?;
        let state = row["state"].as_str().and_then(AskState::named).ok_or_else(|| CellError::host(format!("a stored ask's state: {}", row["state"])))?;
        Ok(Some((asked, row["expires"].as_i64().unwrap_or(0), state)))
    }

    /// `SecretAsk`'s JSON, with its platform-origin deep link (`url`).
    fn ask_text(&self, id: &str, name: &str, state: AskState, expires_at: i64) -> CellResult<String> {
        let url = rules::ask_url(&self.cfg.platform(), id);
        serde_json::to_string(&SecretAsk { id: id.to_string(), name: name.to_string(), state, expires_at, url }).map_err(|e| CellError::host(e.to_string()))
    }

    /// An agent's ask, kept to wait on its person's card: checked as its
    /// answer will be (`rules::ask_ok`), the same agent's ask for the same
    /// name before it ended (one card for one secret), and refused past
    /// `ASKS_MAX` waiting. One turn: nothing here yields.
    fn ask(&self, asked: Asked) -> CellResult<String> {
        if self.wiped.get() {
            return Err(CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing"));
        }
        let Asked(agent, _, _, a) = &asked;
        rules::ask_ok(a, &self.cfg.providers).map_err(|e| CellError::invalid(e.to_string()))?;
        let now = js::now_ms();
        self.expire(now)?;
        let kept = self.rows("SELECT COUNT(*) AS n FROM asks", vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        if kept >= rules::ASKS_KEPT_MAX as u64 {
            return Err(CellError::new(ErrorCode::RateLimited, format!("your person's vault keeps {} asks: wait an hour for the ended ones to go", rules::ASKS_KEPT_MAX)));
        }
        let mut waiting = 0;
        for row in self.rows("SELECT id, ask FROM asks WHERE state = 'waiting'", vec![])? {
            let Asked(by, _, _, other) = crate::json_text(row["ask"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored ask: {e}")))?;
            if by == *agent && other.name == a.name {
                self.sql().exec("UPDATE asks SET state = 'expired' WHERE id = ?", vec![row["id"].as_str().unwrap_or_default().into()])?;
            } else {
                waiting += 1;
            }
        }
        if waiting >= rules::ASKS_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, format!("your person has {} asks waiting: wait for one to be answered", rules::ASKS_MAX)));
        }
        let id = hex::encode(js::random_bytes::<16>());
        let expires = now + rules::ASK_TTL_MS;
        let text = serde_json::to_string(&asked).map_err(|e| CellError::host(format!("an ask's JSON: {e}")))?;
        self.sql().exec(
            "INSERT INTO asks (id, ask, at, expires, state) VALUES (?, ?, ?, ?, 'waiting')",
            vec![id.as_str().into(), text.as_str().into(), SqlStorageValue::Integer(now), SqlStorageValue::Integer(expires)],
        )?;
        self.ask_text(&id, &a.name, AskState::Waiting, expires)
    }

    /// The ask `id` as `agent`, who made it, reads it; anyone else's is none.
    fn asked(&self, id: &str, agent: &str) -> CellResult<String> {
        match self.ask_row(id)? {
            Some((Asked(by, _, _, a), expires, state)) if by == agent => self.ask_text(id, &a.name, state, expires),
            _ => Err(CellError::new(ErrorCode::NotFound, format!("no ask {id} of {agent}'s"))),
        }
    }

    /// Its person's answer to the ask `id`: what they pasted (new secret)
    /// or Allow (existing secret, keeping its sealed value), put under the
    /// Vault's own rules (`rules::answered`, then `put`), and the ask saved.
    /// One that ended or whose secret changed since the card showed it is
    /// refused (409), and a put refused leaves it waiting, its card saying
    /// why. The put's seal may yield, so both the ask still waiting and the
    /// held revision are checked again in the turn that writes (a Decline,
    /// an expiry, an ask again, another answer or a secret edit meanwhile:
    /// 409, nothing written).
    async fn answer(&self, id: &str, a: AnswerAsk) -> CellResult<String> {
        let ended = |state: AskState| CellError::new(ErrorCode::Conflict, format!("this ask is {}: ask your agent to ask again", state.name()));
        let (Asked(agent, _, _, ask), expires, state) = self.ask_row(id)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no ask {id}")))?;
        if state != AskState::Waiting {
            return Err(ended(state));
        }
        let changed = || CellError::new(ErrorCode::Conflict, format!("{} changed since its card showed it: look at the card again", ask.name));
        let (held, revision) = self.held_at(&ask.name)?;
        if a.held != revision {
            return Err(changed());
        }
        let put = rules::answered(&ask, &agent, held.as_ref(), &a).map_err(|e| CellError::invalid(e.to_string()))?;
        let still = || -> CellResult<()> {
            match self.ask_row(id)? {
                Some((_, _, AskState::Waiting)) => {}
                Some((_, _, state)) => return Err(ended(state)),
                None => return Err(CellError::new(ErrorCode::NotFound, format!("no ask {id}"))),
            }
            match self.held_at(&ask.name)?.1 == revision {
                true => Ok(()),
                false => Err(changed()),
            }
        };
        self.put(&ask.name, &put, None, Some(&still)).await?;
        self.sql().exec("UPDATE asks SET state = 'saved' WHERE id = ?", vec![id.into()])?;
        self.ask_text(id, &ask.name, AskState::Saved, expires)
    }

    /// Its person's Decline of the ask `id`; again, or
    /// one that ended, it answers as it stands.
    fn decline(&self, id: &str) -> CellResult<String> {
        let (Asked(_, _, _, ask), expires, state) = self.ask_row(id)?.ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no ask {id}")))?;
        if state != AskState::Waiting {
            return self.ask_text(id, &ask.name, state, expires);
        }
        self.sql().exec("UPDATE asks SET state = 'declined' WHERE id = ?", vec![id.into()])?;
        self.ask_text(id, &ask.name, AskState::Declined, expires)
    }

    /// The asks still waiting, oldest first, each its card: `NoticeList`'s
    /// JSON (`rules::ask_notice`), with its field or, not `fields`, its
    /// link (`rules::linked`).
    fn notices(&self, fields: bool) -> CellResult<String> {
        let now = js::now_ms();
        let rows = self.rows("SELECT id, ask, expires FROM asks WHERE state = 'waiting' AND expires > ? ORDER BY at", vec![SqlStorageValue::Integer(now)])?;
        let mut notices = Vec::with_capacity(rows.len());
        for row in rows {
            let Asked(by, title, picture, ask) = crate::json_text(row["ask"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored ask: {e}")))?;
            let (held, revision) = self.held_at(&ask.name)?;
            let id = row["id"].as_str().unwrap_or_default();
            let card = rules::ask_notice(id, &ask, &by, &title, picture, row["expires"].as_i64().unwrap_or(0), (held.as_ref(), &revision));
            notices.push(if fields { card } else { rules::linked(card, &rules::ask_url(&self.cfg.platform(), id)) });
        }
        serde_json::to_string(&fragment_proto::notice::NoticeList { notices }).map_err(|e| CellError::host(e.to_string()))
    }

    /// A wipe's calls (docs/api.md, Operators; wipe.rs): `view` →
    /// `{wiped, held}`; `end` → `{wiped, held}`: every table made again
    /// empty, with the row that says it was wiped, in one step. Again, it
    /// changes nothing.
    fn wipe(&self, route: &str) -> CellResult<String> {
        // a secret, or an own key: a vault that refuses its schema may be
        // one from before its own keys' step, with no such table (Codex, #1345)
        let held = || -> CellResult<bool> {
            let own = self.rows("SELECT provider FROM own_keys WHERE sealed IS NOT NULL LIMIT 1", vec![]).is_ok_and(|r| !r.is_empty());
            Ok(own || !self.rows("SELECT name FROM secrets LIMIT 1", vec![])?.is_empty())
        };
        match route {
            "view" => Ok(format!("{{\"wiped\":{},\"held\":{}}}", self.wiped.get(), held()?)),
            "end" => {
                let had = held()?;
                let sql = self.sql();
                crate::schema::remake(sql.clone(), &fragment_core::migrate::VAULT)?;
                if !self.wiped.get() {
                    sql.exec("INSERT INTO wiped (at) VALUES (?)", vec![SqlStorageValue::Integer(js::now_ms())])?;
                }
                self.wiped.set(true);
                assert!(!held()?, "a wiped vault holds nothing");
                Ok(format!("{{\"wiped\":true,\"held\":{had}}}"))
            }
            other => Err(CellError::new(ErrorCode::NotFound, format!("no wipe route {other}"))),
        }
    }
}

/// `text`'s SHA-256, as hex: a stored secret's seal says which value it
/// holds (a new seal's random nonce always changes it); with what the
/// secret says, which grant (`held_at`).
fn revision_of(sealed: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(sealed.as_bytes()))
}

/// A kept secret's revision: its seal and what it says, together.
fn revision_at(row: &Value) -> String {
    revision_of(&format!("{}\n{}", row["sealed"].as_str().unwrap_or_default(), row["secret"].as_str().unwrap_or_default()))
}

/// What a row's kept `Secret` says, read as `T`.
fn stored<T: serde::de::DeserializeOwned>(row: &Value) -> CellResult<T> {
    crate::json_text(row["secret"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored secret: {e}")))
}
