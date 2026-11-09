//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): one
//! `Vault` Durable Object a person, named by their identity, holding the
//! secrets their agents use without ever holding them. A value goes in
//! sealed for this object (`keys::seal`) and never comes out: no route,
//! answer, log or event names one.
//!
//! - **Its routes** (the router's `/api/vault`, `route` below): the person
//!   alone, never an agent, lists (names only), puts and removes.
//! - **Its seam** (`put`, `delete`): what the platform's own code puts for
//!   a person, as FIN-463's migration hands an old agent's keys over. The
//!   rules are one (`fragment_core::vault::check`).
//! - **Its wipe** (wipe.rs, the ledger's step): emptied in one step, and a
//!   row kept that says so; nothing is put after.

use std::cell::Cell;

use fragment_core::vault::{self as rules, Placing};
use fragment_proto::vault::{Place, PutSecret, Secret, SecretList, SecretRemoved};
use fragment_proto::{ErrorCode, IdentityKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;
use crate::json_response::json_response;
use crate::routed::Signed;

/// What a value is sealed for: this class, and the object's id.
const SEAL_CLASS: &str = "Vault";

/// The marker every call into a Vault carries (routed.rs `marker`): only
/// the platform's own code calls one, and the router passes no such header.
pub(crate) const VAULT_HEADER: &str = "x-fragment-vault";

/// `vault/put`'s body: a secret for `person`, under `name`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PutAsk {
    person: String,
    name: String,
    put: PutSecret,
}

/// `vault/list`'s and `vault/delete`'s bodies.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    person: String,
    #[serde(default)]
    name: Option<String>,
}

/// `vault/delete`'s answer.
#[derive(Serialize, Deserialize)]
struct Removed {
    removed: bool,
}

/// Puts `put` into `person`'s Vault under `name`, sealed there: the same
/// rules and answer as the person's own `PUT /api/vault/{name}`. Set
/// again, it is replaced whole. The seam for the platform's own code
/// (FIN-463's hand-off): it never answers the value.
pub(crate) async fn put(env: &Env, person: &str, name: &str, put: &PutSecret) -> CellResult<Secret> {
    crate::routed::ask_object(env, "VAULT", person, "vault/put", &PutAsk { person: person.into(), name: name.into(), put: put.clone() }).await
}

/// Removes `name` from `person`'s Vault: whether there was one.
pub(crate) async fn delete(env: &Env, person: &str, name: &str) -> CellResult<bool> {
    let r: Removed = crate::routed::ask_object(env, "VAULT", person, "vault/delete", &Named { person: person.into(), name: Some(name.into()) }).await?;
    Ok(r.removed)
}

/// `/api/vault[/{name}]`, signed by `signed`: the person's own, never an
/// agent's (their agents use its secrets, never set or list them).
pub(crate) async fn route(env: &Env, signed: &Signed, method: Method, rest: &[&str], body: &[u8]) -> CellResult<Response> {
    if signed.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "a vault is its person's alone: their agents use its secrets, and never set, list or remove one"));
    }
    let person = signed.identity.id.as_str();
    match (method, rest) {
        (Method::Get, []) => {
            let list: SecretList = crate::routed::ask_object(env, "VAULT", person, "vault/list", &Named { person: person.into(), name: None }).await?;
            json_response(&list)
        }
        (Method::Put, [name]) => {
            let p: PutSecret = serde_json::from_slice(body).map_err(|e| CellError::invalid(format!("a secret is {{value, hosts, place?, agents}}: {e}")))?;
            json_response(&put(env, person, name, &p).await?)
        }
        (Method::Delete, [name]) => json_response(&SecretRemoved { name: name.to_string(), removed: delete(env, person, name).await? }),
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/vault/{}", m.as_ref(), rest.join("/")))),
    }
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
        // a wipe ends a vault that refuses its schema too
        if let Some(route) = req.path().strip_prefix("/wipe/") {
            // only a wipe's orchestrator sets the header (routed.rs `marker`)
            if req.headers().get(crate::wipe::WIPE_HEADER)?.is_none() {
                return Err(CellError::new(ErrorCode::NotFound, format!("no route /wipe/{route}")));
            }
            return Ok(Response::from_json(&self.wipe(route).await?)?);
        }
        if req.headers().get(VAULT_HEADER)?.is_none() {
            return Err(CellError::new(ErrorCode::NotFound, format!("no route {}", req.path())));
        }
        self.schema.ready()?;
        let path = req.path();
        // each call reads its own type from the call's bytes, once
        let body = req.bytes().await?;
        fn read<'a, T: Deserialize<'a>>(path: &str, body: &'a [u8]) -> CellResult<T> {
            serde_json::from_slice(body).map_err(|e| CellError::invalid(format!("{path}: {e}")))
        }
        fn answer(a: &impl Serialize) -> CellResult<Response> {
            Ok(Response::from_json(a)?)
        }
        match path.as_str() {
            "/vault/put" => {
                let b: PutAsk = read(&path, &body)?;
                answer(&self.put(&b.person, &b.name, &b.put).await?)
            }
            "/vault/list" => {
                let b: Named = read(&path, &body)?;
                self.person_is(&b.person, false)?;
                answer(&SecretList { secrets: self.secrets()? })
            }
            "/vault/delete" => {
                let b: Named = read(&path, &body)?;
                self.person_is(&b.person, false)?;
                let name = b.name.ok_or_else(|| CellError::invalid("vault/delete names its secret"))?;
                let removed = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![name.as_str().into()])?.is_empty();
                self.sql().exec("DELETE FROM secrets WHERE name = ?", vec![name.as_str().into()])?;
                if removed {
                    console_log!("{}", json!({ "vault": "removed", "name": name }));
                }
                answer(&Removed { removed })
            }
            other => Err(CellError::new(ErrorCode::NotFound, format!("no route {other}"))),
        }
    }

    /// Refuses a call for anyone but the person this vault is: the first
    /// put names them for good (`write`), and a vault no put named holds
    /// nothing of anyone's.
    fn person_is(&self, person: &str, write: bool) -> CellResult<()> {
        if !fragment_core::npub::is_identity(person) {
            return Err(CellError::invalid("a vault's call names its person"));
        }
        let rows = self.rows("SELECT value FROM meta WHERE key = 'person'", vec![])?;
        match rows.first().and_then(|r| r["value"].as_str()) {
            Some(p) if p != person => Err(CellError::host(format!("this vault is {p}'s, not {person}'s"))),
            Some(_) => Ok(()),
            None if write => {
                self.sql().exec("INSERT INTO meta (key, value) VALUES ('person', ?)", vec![person.into()])?;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn put(&self, person: &str, name: &str, put: &PutSecret) -> CellResult<Secret> {
        if self.wiped.get() {
            return Err(CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing"));
        }
        let checked = rules::check(name, put, &self.cfg.providers).map_err(|e| CellError::invalid(e.to_string()))?;
        if checked.placing == Placing::Nip98 && fragment_nip98::Keys::from_secret_hex(&put.value).is_none() {
            return Err(CellError::invalid(format!("a secret's value: {}", rules::NOSTR_KEY)));
        }
        // sealed first (reading the host secret may yield): the checks and
        // the write below share one turn
        let sealed = crate::keys::seal(&self.env, &self.scope(), put.value.as_bytes()).await?;
        // a wipe that ended while it sealed: nothing is put after it
        if self.wiped.get() {
            return Err(CellError::new(ErrorCode::Forbidden, "this person was wiped: their vault keeps nothing"));
        }
        self.person_is(person, true)?;
        let exists = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![name.into()])?.is_empty();
        let held = self.rows("SELECT COUNT(*) AS n FROM secrets", vec![])?.first().and_then(|r| r["n"].as_u64()).unwrap_or(0);
        if !exists && held >= rules::SECRETS_MAX as u64 {
            return Err(CellError::invalid(format!("a vault keeps at most {} secrets: remove one first", rules::SECRETS_MAX)));
        }
        let at = js::now_ms();
        self.sql().exec(
            "INSERT INTO secrets (name, sealed, hosts, place, agents, set_at) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, hosts = excluded.hosts, place = excluded.place, agents = excluded.agents, set_at = excluded.set_at",
            vec![name.into(), sealed.into(), text(&checked.hosts).into(), text(&checked.place).into(), text(&checked.agents).into(), SqlStorageValue::Integer(at)],
        )?;
        // what was set, never the value
        console_log!("{}", json!({ "vault": "set", "name": name, "hosts": checked.hosts.len(), "agents": checked.agents.len() }));
        Ok(Secret { name: name.to_string(), hosts: checked.hosts, place: checked.place, agents: checked.agents, set_at: at })
    }

    /// Every secret held, by name: never a value.
    fn secrets(&self) -> CellResult<Vec<Secret>> {
        let rows = self.rows("SELECT name, hosts, place, agents, set_at FROM secrets ORDER BY name", vec![])?;
        assert!(rows.len() <= rules::SECRETS_MAX, "a vault keeps at most SECRETS_MAX secrets");
        rows.iter()
            .map(|r| {
                let field = |k: &str| r[k].as_str().unwrap_or_default();
                let bad = |k: &str, e: serde_json::Error| CellError::host(format!("a stored secret's {k}: {e}"));
                Ok(Secret {
                    name: field("name").to_string(),
                    hosts: crate::json_text(field("hosts")).map_err(|e| bad("hosts", e))?,
                    place: crate::json_text::<Place>(field("place")).map_err(|e| bad("place", e))?,
                    agents: crate::json_text(field("agents")).map_err(|e| bad("agents", e))?,
                    set_at: r["set_at"].as_i64().unwrap_or_default(),
                })
            })
            .collect()
    }

    /// A wipe's calls (docs/api.md, Operators; wipe.rs): `view` →
    /// `{wiped, held}`; `end` → `{wiped, held}`: every table made again
    /// empty, with the row that says it was wiped, in one step (no await).
    /// Again, it changes nothing.
    async fn wipe(&self, route: &str) -> CellResult<Value> {
        let held = || -> CellResult<bool> { Ok(!self.rows("SELECT name FROM secrets LIMIT 1", vec![])?.is_empty()) };
        match route {
            "view" => Ok(json!({ "wiped": self.wiped.get(), "held": held()? })),
            "end" => {
                let had = held()?;
                let sql = self.sql();
                crate::schema::remake(sql.clone(), &fragment_core::migrate::VAULT)?;
                if !self.wiped.get() {
                    sql.exec("INSERT INTO wiped (at) VALUES (?)", vec![SqlStorageValue::Integer(js::now_ms())])?;
                }
                self.wiped.set(true);
                assert!(!held()?, "a wiped vault holds nothing");
                Ok(json!({ "wiped": true, "held": had }))
            }
            other => Err(CellError::new(ErrorCode::NotFound, format!("no wipe route {other}"))),
        }
    }
}

/// A stored field's JSON text, written once.
fn text(v: &impl Serialize) -> String {
    serde_json::to_string(v).expect("a stored field serializes")
}
