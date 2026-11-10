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

use std::cell::Cell;

use fragment_core::vault::{self as rules, Placing};
use fragment_proto::vault::{Place, PutSecret, Secret, SecretRemoved};
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

/// The secrets each of `agents` (fragments of `person`'s) may use, toward
/// which hosts, one `Usable` each: what a computer's view lists and its
/// egress catches.
pub(crate) async fn names(env: &Env, person: &str, agents: &[String]) -> CellResult<Vec<Usable>> {
    crate::routed::ask_object(env, "VAULT", person, "vault/names", &NamesAsk { agents: agents.to_vec() }).await
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

/// Puts `put` (a `PutSecret`'s JSON) into `person`'s Vault under `name`,
/// sealed there, under the rules of the person's own `PUT
/// /api/vault/{name}`; set again, it is replaced whole. The seam for the
/// platform's own code (FIN-463's hand-off): its answer is the `Secret`
/// set, as JSON, never its value.
pub(crate) async fn put(env: &Env, person: &str, name: &str, put: &RawValue) -> CellResult<Vec<u8>> {
    ask(env, person, &format!("vault/put/{name}"), Some(put)).await
}

/// What a move reads of a kept `Secret`: where it goes.
#[derive(serde::Deserialize)]
struct Placed {
    place: Place,
}

/// `vault/take`'s answer: a secret's value, for its move, and which value
/// it is (its revision: its sealed text's SHA-256). A tuple on the wire.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Taken(pub String, pub String);

/// `name`'s value in `person`'s Vault, for its move into a fragment of
/// theirs as that fragment's secret (members.rs, `fromVault`): the mover
/// keeps it, then removes that revision here (`delete_taken`), so it has
/// one home at the end and is never lost between. A Nostr key is refused:
/// it never leaves.
pub(crate) async fn take(env: &Env, person: &str, name: &str) -> CellResult<Taken> {
    crate::routed::ask_object(env, "VAULT", person, &format!("vault/take/{name}"), &None::<&RawValue>).await
}

/// Removes `name` from `person`'s Vault only while it holds the value a
/// `take` answered as `revision`: one its person set since stays.
pub(crate) async fn delete_taken(env: &Env, person: &str, name: &str, revision: &str) -> CellResult<()> {
    crate::routed::ask_bytes(env, "VAULT", person, &format!("vault/delete/{name}"), &Some(revision)).await.map(drop)
}

/// Removes `name` from `person`'s Vault: its answer, a `SecretRemoved`.
pub(crate) async fn delete(env: &Env, person: &str, name: &str) -> CellResult<Vec<u8>> {
    ask(env, person, &format!("vault/delete/{name}"), None).await
}

/// `/api/vault[/{name}]`, signed by `signed`: the person's own, never an
/// agent's (their agents use its secrets, never set or list them). The
/// router finds their Vault, hands it the body as it came, and passes its
/// answer on.
pub(crate) async fn route(env: &Env, signed: &Signed, method: Method, rest: &[&str], body: &[u8]) -> CellResult<Response> {
    if signed.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "a vault is its person's alone: their agents use its secrets, and never set, list or remove one"));
    }
    let person = signed.identity.id.as_str();
    let answer = match (method, rest) {
        (Method::Get, []) => ask(env, person, "vault/list", None).await?,
        (Method::Put, [name]) => {
            let raw: Box<RawValue> = crate::json_body(body)?;
            put(env, person, name, &raw).await?
        }
        (Method::Delete, [name]) => delete(env, person, name).await?,
        (m, _) => return Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/vault/{}", m.as_ref(), rest.join("/")))),
    };
    json_bytes(answer)
}

#[durable_object]
pub struct VaultCell {
    state: State,
    env: Env,
    cfg: &'static Config,
    schema: crate::schema::Schema,
    wiped: Cell<bool>,
}

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
            ["vault", "put", name] => self.put(name, &crate::json_body(&req.bytes().await?)?).await?,
            ["vault", "list"] => self.list()?,
            ["vault", "delete", name] => self.delete(name, crate::json_body::<Option<String>>(&req.bytes().await?)?.as_deref())?,
            ["vault", "take", name] => self.take(name).await?,
            ["vault", "names"] => self.names(&crate::json_body(&req.bytes().await?)?)?,
            ["vault", "fill"] => self.fill(&crate::json_body(&req.bytes().await?)?).await?,
            _ => return Err(CellError::new(ErrorCode::NotFound, format!("no route {path}"))),
        };
        Ok(Response::from_bytes(answer.into_bytes())?)
    }

    /// `put` checked, sealed and kept under `name`: its `Secret` as JSON.
    async fn put(&self, name: &str, put: &PutSecret) -> CellResult<String> {
        let wiped = || CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing");
        if self.wiped.get() {
            return Err(wiped());
        }
        let checked = rules::check(name, put, &self.cfg.providers).map_err(|e| CellError::invalid(e.to_string()))?;
        if checked.placing == Placing::Nip98 && fragment_nip98::Keys::from_secret_hex(&put.value).is_none() {
            return Err(CellError::invalid(format!("a secret's value: {}", rules::NOSTR_KEY)));
        }
        // sealed first (reading the host secret may yield): the checks and
        // the write below share one turn, and a wipe that ended meanwhile
        // stops it
        let sealed = crate::keys::seal(&self.env, &self.scope(), put.value.as_bytes()).await?;
        if self.wiped.get() {
            return Err(wiped());
        }
        let exists = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![name.into()])?.is_empty();
        let held = self.rows("SELECT COUNT(*) AS n FROM secrets", vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        if !exists && held >= rules::SECRETS_MAX as u64 {
            return Err(CellError::invalid(format!("a vault keeps at most {} secrets: remove one first", rules::SECRETS_MAX)));
        }
        let secret = Secret { name: name.to_string(), hosts: checked.hosts, place: checked.place, agents: checked.agents, set_at: js::now_ms() };
        let text = serde_json::to_string(&secret).map_err(|e| CellError::host(format!("a secret's JSON: {e}")))?;
        self.sql().exec(
            "INSERT INTO secrets (name, sealed, secret) VALUES (?, ?, ?) ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, secret = excluded.secret",
            vec![name.into(), sealed.into(), text.as_str().into()],
        )?;
        Ok(text)
    }

    /// Every secret held, by name, as `SecretList`'s JSON: the texts kept,
    /// never a value.
    fn list(&self) -> CellResult<String> {
        let rows = self.rows("SELECT secret FROM secrets ORDER BY name", vec![])?;
        assert!(rows.len() <= rules::SECRETS_MAX, "a vault keeps at most SECRETS_MAX secrets");
        let texts: Vec<&str> = rows.iter().filter_map(|r| r["secret"].as_str()).collect();
        Ok(format!("{{\"secrets\":[{}]}}", texts.join(",")))
    }

    /// Which secrets each of `b.agents` may use, toward which hosts: a
    /// `Usable` each, as JSON.
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
        serde_json::to_string(&usable).map_err(|e| CellError::host(e.to_string()))
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

    /// `name`'s value, opened for its move, and its revision: `Taken`'s
    /// JSON. None, or a Nostr key, refused.
    async fn take(&self, name: &str) -> CellResult<String> {
        let rows = self.rows("SELECT sealed, secret FROM secrets WHERE name = ?", vec![name.into()])?;
        let row = rows.first().ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no secret {name} in your vault (GET /api/vault lists them)")))?;
        let kept: Placed = crate::json_text(row["secret"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored secret: {e}")))?;
        if kept.place.sign.is_some() {
            return Err(CellError::invalid(format!("{name} is a key that signs: it never leaves your vault")));
        }
        let sealed = row["sealed"].as_str().unwrap_or_default().to_string();
        let opened = crate::keys::open(&self.env, &self.scope(), &sealed).await?;
        let value = String::from_utf8(opened.plaintext).map_err(|_| CellError::host("a secret that is not text"))?;
        serde_json::to_string(&Taken(value, revision_of(&sealed))).map_err(|e| CellError::host(e.to_string()))
    }

    /// `name` removed (a move's: only while it holds `revision`):
    /// `SecretRemoved`'s JSON.
    fn delete(&self, name: &str, revision: Option<&str>) -> CellResult<String> {
        let rows = self.rows("SELECT sealed FROM secrets WHERE name = ?", vec![name.into()])?;
        let held = rows.first().and_then(|r| r["sealed"].as_str()).filter(|sealed| revision.is_none_or(|rev| revision_of(sealed) == rev));
        let removed = held.is_some();
        if let Some(sealed) = held {
            self.sql().exec("DELETE FROM secrets WHERE name = ? AND sealed = ?", vec![name.into(), sealed.into()])?;
        }
        serde_json::to_string(&SecretRemoved { name: name.to_string(), removed }).map_err(|e| CellError::host(e.to_string()))
    }

    /// A wipe's calls (docs/api.md, Operators; wipe.rs): `view` →
    /// `{wiped, held}`; `end` → `{wiped, held}`: every table made again
    /// empty, with the row that says it was wiped, in one step. Again, it
    /// changes nothing.
    fn wipe(&self, route: &str) -> CellResult<String> {
        let held = || -> CellResult<bool> { Ok(!self.rows("SELECT name FROM secrets LIMIT 1", vec![])?.is_empty()) };
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

/// Which value a stored secret holds: its sealed text's SHA-256, which a
/// new seal (its random nonce) always changes.
fn revision_of(sealed: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(sealed.as_bytes()))
}

/// What a row's kept `Secret` says, read as `T`.
fn stored<T: serde::de::DeserializeOwned>(row: &Value) -> CellResult<T> {
    crate::json_text(row["secret"].as_str().unwrap_or_default()).map_err(|e| CellError::host(format!("a stored secret: {e}")))
}
