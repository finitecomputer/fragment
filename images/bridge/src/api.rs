//! The fragment API, as the guest speaks it (docs/computers.md, "The
//! fragment API"; docs/api.md): `FRAGMENT_API` over plain HTTP, every
//! request acting as one of the computer's agents (`x-fragment-agent`)
//! except the computer's own routes. The intercept signs; the guest holds
//! no credential.
//!
//! Every route the bridge calls is a method here, so the platform side can
//! match them one for one (docs/bridge.md lists them).

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::limits;
use crate::net::{self, Base, ClientWs};
use crate::records::Record;
use crate::runtime::Agent;

/// The header naming the agent a request acts as.
pub const AGENT_HEADER: &str = "x-fragment-agent";
/// Identities one `__people` call names, at most (the platform's limit).
pub const PEOPLE_MAX: usize = 64;

/// Why a call failed, typed so callers match without reading strings.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// The platform answered, and refused: its status and `{error, message}`.
    Refused { status: u16, error: String, message: String },
    /// No answer: the connection, or the time it took.
    Transport(String),
    /// An answer that is not what the route returns.
    Decode(String),
    /// An answer past the bound the bridge reads.
    TooLarge,
}

impl ApiError {
    /// The agent is not (or no longer) in that fragment, or it is gone.
    pub fn gone(&self) -> bool {
        matches!(self, ApiError::Refused { status: 403 | 404, .. })
    }

    /// Worth asking again: no answer, too many asks, or the platform's own
    /// failure (every post carries its id, so a retry is the same record).
    pub fn retryable(&self) -> bool {
        match self {
            ApiError::Transport(_) => true,
            ApiError::Refused { status, .. } => *status == 429 || *status >= 500,
            ApiError::Decode(_) | ApiError::TooLarge => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Refused { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Refused { status, error, message } => write!(f, "{status} {error}: {message}"),
            ApiError::Transport(m) => write!(f, "no answer: {m}"),
            ApiError::Decode(m) => write!(f, "an answer that does not decode: {m}"),
            ApiError::TooLarge => write!(f, "an answer past the bound"),
        }
    }
}

/// `GET /api/computer`.
/// What a fragment is called, from its status: its title, and an agent
/// fragment's role (docs/api.md, "An agent's name and picture").
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Face {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "agentRole")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Computer {
    pub computer: String,
    pub owner: String,
    #[serde(default)]
    pub image: String,
    pub agents: Vec<Agent>,
    /// Every environment variable a credential of the deployment may be
    /// in, whether its agents hold it now or not.
    #[serde(default, rename = "credentialEnv")]
    pub credential_env: Vec<String>,
    /// The model route's models' ids by its names, as the deployment names
    /// them now (docs/computers.md, "A model by its id"); none from a
    /// platform before them.
    #[serde(default)]
    pub models: Option<ModelIds>,
    /// It stays awake whatever is open: its owner's always-on seat
    /// (docs/billing.md). SimpleX runs only while it does.
    #[serde(default, rename = "alwaysOn")]
    pub always_on: bool,
    /// Its newest start (the Computer DO's generation; 0 from a platform
    /// before it).
    #[serde(default)]
    pub generation: u64,
    /// What its last start that came up restored; none before its first.
    #[serde(default)]
    pub restored: Option<Restored>,
    /// What the platform's XFTP relay carries (`GET /api/computer/relay`):
    /// none from a platform without one, whose cores then reach no file
    /// server from a Cloudflare computer.
    #[serde(default)]
    pub relay: Option<RelayOffer>,
}

/// The hosts and the port the platform's XFTP relay carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayOffer {
    pub hosts: Vec<String>,
    pub port: u16,
}

impl RelayOffer {
    /// The host it carries that `host` names (any case), on `port`.
    pub fn carried(&self, host: &str, port: u16) -> Option<&str> {
        self.hosts.iter().find(|h| port == self.port && h.eq_ignore_ascii_case(host)).map(String::as_str)
    }
}

/// What a start restored, as far as the bridge reads it (docs/computers.md,
/// "What a wake restored").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restored {
    /// The start it is of.
    pub generation: u64,
    /// It went back in time: what the life before it did since its save is
    /// in no save.
    pub rollback: bool,
}

impl Computer {
    /// Whether this start went back in time, as the platform recorded it.
    pub fn rolled_back(&self) -> bool {
        self.generation > 0 && self.restored.is_some_and(|r| r.rollback && r.generation == self.generation)
    }
}

/// The model route's models' ids, by its names: what a call may name each
/// slot by besides its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    pub cheap: String,
    pub medium: String,
    pub fallback: String,
    pub vision: String,
    /// Each tier's route (2026-10-24; a platform before it names none):
    /// its fallback's and vision model's ids, which a tier agent's stall
    /// falls back to (Paul: "a fallback on private should fall back to
    /// something private, a fallback on normal should fall back to
    /// something normal").
    /// Boxed: a computer's view is the bridge's largest message without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Box<TierRoutes>>,
}

/// The two tiers' routes (`ModelIds::routes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierRoutes {
    pub cheap: TierRoute,
    pub medium: TierRoute,
}

/// A tier's route as the view names it: what Settings calls it, whether it
/// is private, and its fallback's and vision model's ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierRoute {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub private: bool,
    pub fallback: String,
    pub vision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep: Option<RouteMode>,
}

/// A tier's delegated-work route (decision 65). Absent on an older cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteMode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// The efforts the platform accepts, shared by a tier's modes and an
/// owner's model selection. Unknown efforts are rejected in the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reasoning {
    Low,
    High,
    Max,
}

impl Reasoning {
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    /// An owner's model setting, High when absent or invalid. Platform
    /// tiers use their route's effort instead of this personal setting.
    pub fn of(agent_json: Option<&[u8]>) -> Self {
        agent_json.and_then(|b| serde_json::from_slice::<Value>(b).ok()).and_then(|v| serde_json::from_value(v["reasoning"].clone()).ok()).unwrap_or(Self::High)
    }
}

/// A model's id, at most (`anthropic/claude-sonnet-5.5`; the platform's
/// catalog holds its offer to the same).
pub const MODEL_ID_MAX_BYTES: usize = 128;

/// A model's id: 1 to `MODEL_ID_MAX_BYTES` letters, digits and `.`, `_`,
/// `-`, `:`, `/`, `@` (the platform's `catalog::valid_model_id`).
pub fn model_id_ok(id: &str) -> bool {
    (1..=MODEL_ID_MAX_BYTES).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':' | b'/' | b'@'))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FragmentEntry {
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// What it is (`app`, `chat`, `agent`, `brain`, `skills`): its live
    /// `fragment.json`'s `kind`.
    #[serde(default)]
    pub kind: String,
    /// Listed `for` someone: whether they own it.
    #[serde(default)]
    pub owned: bool,
    /// Its title, as its owner named it (an agent's is its name: the shell
    /// titles an agent fragment so); empty when it has none.
    #[serde(default)]
    pub title: String,
}

/// A fragment name's label: `todo` of `todo--k3x9` (decision 47).
pub fn label(name: &str) -> &str {
    name.rsplit_once("--").map_or(name, |(label, _)| label)
}

/// An agent's Bot Chat: its own chat with its owner, the chat fragment the
/// shell makes with it (cell/shell/shell.js, `makeAgent`: labelled
/// `<label>-chat` beside the agent's `<label>`), which `fragment ask` finds
/// as a person's direct chat with it too (cli/src/ask.rs, `direct_label`),
/// and its owner's home unless they chose its SimpleX chat (`home_chat`). A
/// chat's name has a random suffix of its own, so it is found, never
/// derived: in the agent's list for its owner (`fragments`), the chat its
/// owner owns with that label. None until there is one: an agent made some
/// other way has no Bot Chat (Hermes then makes one of its own for a
/// teammate's message, `chat -c "Bot Chat" --create-if-missing`), and one
/// whose chat is made after it is assigned has it at a later look
/// (hermes-boot's main.rs, `find_bot_chats`).
pub fn bot_chat(agent_fragment: &str, fragments: &[FragmentEntry]) -> Option<String> {
    let want = format!("{}-chat", label(agent_fragment));
    fragments.iter().find(|f| f.kind == "chat" && f.owned && label(&f.name) == want).map(|f| f.name.clone())
}

/// What an agent's SimpleX chat is titled as its bridge makes it: the
/// shell shows it as "<agent> · SimpleX" while it keeps the title (a chat's
/// title is its own, never its agent's name, which a rename changes).
pub const SIMPLEX_CHAT_TITLE: &str = "SimpleX";

/// An agent's `agent.json` is read at most this long (its settings: a few
/// hundred bytes; hermes-boot reads it as large).
pub const AGENT_JSON_MAX_BYTES: usize = 64 * 1024;

/// The label of an agent's SimpleX chat: its own, then `-simplex`.
pub fn simplex_label(agent_fragment: &str) -> String {
    format!("{}-simplex", label(agent_fragment))
}

/// What a fragment's name's suffix is drawn from (docs/api.md, "Names":
/// lowercase letters and digits less `l`, `o`, `0` and `1`).
const SUFFIX_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// An agent's SimpleX chat's name in full (docs/api.md, "An agent's home"):
/// its label (`simplex_label`) and a suffix drawn from `random`, chosen
/// before the chat is made and kept with its link (simplex/run.rs,
/// `Saved::chat`), so the chat is made by that name once, whatever restarts.
pub fn simplex_chat_name(agent_fragment: &str, random: [u8; 4]) -> String {
    let suffix: String = random.iter().map(|b| SUFFIX_ALPHABET[usize::from(*b) % SUFFIX_ALPHABET.len()] as char).collect();
    format!("{}--{suffix}", simplex_label(agent_fragment))
}

/// Where an agent reaches its owner when no one asked (docs/api.md, "An
/// agent's home"): its `agent.json`'s `home`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// Its own chat with its owner (`bot_chat`): absent, or anything but
    /// `"simplex"`.
    Chat,
    /// Its SimpleX chat, while its link is bound.
    Simplex,
}

impl Home {
    /// From `agent.json`'s bytes as the platform answered them (none: there
    /// is no file).
    pub fn of(agent_json: Option<&[u8]>) -> Home {
        let simplex = agent_json.and_then(|b| serde_json::from_slice::<Value>(b).ok()).is_some_and(|v| v["home"] == "simplex");
        if simplex {
            Home::Simplex
        } else {
            Home::Chat
        }
    }
}

/// Where a routine that names no chat runs (`home_chat`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeChat {
    In(String),
    /// Not yet: its home is its SimpleX chat, which its link has named and
    /// not made yet.
    Unmade,
    /// Nowhere: it has no chat with its owner.
    Nowhere,
}

/// The chat an agent's home names: its SimpleX chat (`simplex`, the one its
/// link mirrors, while bound) when its home is SimpleX, else its own chat,
/// one of its owner's (`owners`: the agent's list for them) that the agent
/// is in (`mine`: its own list), so one an earlier agent of its label left,
/// or one it has left, is none; none when it has neither. Its SimpleX chat
/// is named before it is made (simplex/run.rs, `Saved::chat`), so it is the
/// home once it is in `mine`; until then a routine waits for it (Alex's
/// review of #620: its making refused for now, or a restart between its
/// naming and its making), unless it has `waited` its most
/// (`limits::HOME_CHAT_WAIT_MS`): then its own chat, never dropped.
pub fn home_chat(agent_fragment: &str, home: Home, simplex: Option<&str>, owners: &[FragmentEntry], mine: &[FragmentEntry], waited: bool) -> HomeChat {
    if let (Home::Simplex, Some(chat)) = (home, simplex) {
        if mine.iter().any(|m| m.name == chat) {
            return HomeChat::In(chat.to_string());
        }
        if !waited {
            return HomeChat::Unmade;
        }
    }
    let theirs: Vec<FragmentEntry> = owners.iter().filter(|f| mine.iter().any(|m| m.name == f.name)).cloned().collect();
    bot_chat(agent_fragment, &theirs).map_or(HomeChat::Nowhere, HomeChat::In)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub name: String,
    #[serde(default)]
    pub post: Option<String>,
    #[serde(default)]
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub principal: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub added_at: i64,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: Value,
    pub channel: String,
    #[serde(default)]
    pub principal: Option<String>,
    #[serde(default)]
    pub wake: bool,
}

/// The platform's answer to a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Posted {
    /// The id was posted before, with this body.
    pub replayed: bool,
    /// The record's place in its channel (`None` from a platform that does
    /// not say).
    pub seq: Option<u64>,
}

/// A page of records: `{channel, records, next}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub next: u64,
}

/// One file of a fragment at main (`GET /api/f/{name}/files`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub last_commit_sha: String,
}

/// One change of a commit (`POST /api/f/{name}/files`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileChange {
    Text { path: String, text: String },
    Base64 { path: String, base64: String },
    Delete { path: String, delete: bool },
}

/// The fragment API at `FRAGMENT_API`.
#[derive(Clone)]
pub struct Api {
    base: Base,
    client: Client<HttpConnector, Full<Bytes>>,
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A fragment's name in a path: a name's characters only, so no name ever
/// reaches another route.
fn name(fragment: &str) -> Result<&str, ApiError> {
    let ok = !fragment.is_empty() && fragment.len() <= 128 && fragment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if ok {
        Ok(fragment)
    } else {
        Err(ApiError::Decode(format!("{fragment:?} is not a fragment name")))
    }
}

impl Api {
    pub fn new(base_url: &str) -> Result<Api, String> {
        let base = Base::parse(base_url)?;
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        connector.set_connect_timeout(Some(Duration::from_millis(limits::HTTP_TIMEOUT_MS)));
        let client = Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(connector);
        Ok(Api { base, client })
    }

    pub fn base(&self) -> &Base {
        &self.base
    }

    /// One call: its status and body (bounded by `max`), or why not.
    async fn call(&self, method: Method, path: &str, agent: Option<&str>, body: Option<(&str, Bytes)>, max: usize) -> Result<Bytes, ApiError> {
        let mut req = Request::builder().method(method).uri(self.base.url(path)).header("host", self.base.authority());
        if let Some(a) = agent {
            req = req.header(AGENT_HEADER, a);
        }
        let body = match body {
            Some((content_type, bytes)) => {
                req = req.header("content-type", content_type).header("content-length", bytes.len());
                bytes
            }
            None => Bytes::new(),
        };
        let req = req.body(Full::new(body)).map_err(|e| ApiError::Transport(e.to_string()))?;
        let call = async {
            let res = self.client.request(req).await.map_err(|e| ApiError::Transport(e.to_string()))?;
            let status = res.status();
            let bytes = Limited::new(res.into_body(), max).collect().await.map_err(|e| {
                if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
                    ApiError::TooLarge
                } else {
                    ApiError::Transport(e.to_string())
                }
            })?;
            Ok::<_, ApiError>((status, bytes.to_bytes()))
        };
        let (status, bytes) = match tokio::time::timeout(Duration::from_millis(limits::HTTP_TIMEOUT_MS), call).await {
            Ok(r) => r?,
            Err(_) => return Err(ApiError::Transport(format!("{path}: no answer in {} ms", limits::HTTP_TIMEOUT_MS))),
        };
        if status.is_success() {
            return Ok(bytes);
        }
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Err(ApiError::Refused {
            status: status.as_u16(),
            error: v["error"].as_str().unwrap_or("").to_string(),
            message: v["message"].as_str().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).into_owned()),
        })
    }

    async fn json<T: DeserializeOwned>(&self, method: Method, path: &str, agent: Option<&str>, body: Option<Value>) -> Result<T, ApiError> {
        let body = body.map(|v| ("application/json", Bytes::from(v.to_string())));
        let bytes = self.call(method, path, agent, body, limits::ANSWER_MAX_BYTES).await?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(format!("{path}: {e}")))
    }

    // ---- the computer's own routes (no agent) ----

    /// `GET /api/computer`: the agents to run. `simplex`: the image does
    /// SimpleX (it names its core), which the read says
    /// (`?simplex=1`), so its owner's Settings offer it (docs/bridge.md,
    /// "SimpleX").
    pub async fn computer(&self, simplex: bool) -> Result<Computer, ApiError> {
        let path = if simplex { "/api/computer?simplex=1" } else { "/api/computer" };
        let c: Computer = self.json(Method::GET, path, None, None).await?;
        if c.agents.len() > limits::AGENTS_MAX {
            return Err(ApiError::Decode(format!("{} agents; a computer runs at most {}", c.agents.len(), limits::AGENTS_MAX)));
        }
        if c.agents.iter().any(|a| a.credentials.len() > crate::runtime::CREDENTIALS_MAX) || c.credential_env.len() > crate::runtime::CREDENTIALS_MAX * 4 {
            return Err(ApiError::Decode(format!("more credentials than a deployment offers (at most {})", crate::runtime::CREDENTIALS_MAX)));
        }
        if let Some(m) = &c.models {
            let routes = m.routes.iter().flat_map(|r| [&r.cheap, &r.medium]);
            let routed = routes.flat_map(|r| [&r.fallback, &r.vision].into_iter().chain(r.deep.iter().flat_map(|d| [&d.id].into_iter().chain(d.fallback.iter()))));
            if m.images.as_ref().is_some_and(|ids| ids.len() > 8) { return Err(ApiError::Decode("models.images names at most 8 models".into())); }
            if let Some(bad) = [&m.cheap, &m.medium, &m.fallback, &m.vision].into_iter().chain(routed).chain(m.images.iter().flatten()).find(|id| !model_id_ok(id)) {
                return Err(ApiError::Decode(format!("models: {bad:?} is no model id")));
            }
        }
        Ok(c)
    }

    /// `POST /api/computer/door {agent, door, nonce, key}`: a link's ticket
    /// redeemed at the platform, which registers its door's public key for
    /// `agent` (fragment_core::door). Refused (403) for a ticket that is not
    /// the link its owner started last, or past its time.
    pub async fn door(&self, agent: &str, door: &str, nonce: &str, key: &str) -> Result<(), ApiError> {
        let _: Value = self.json(Method::POST, "/api/computer/door", None, Some(json!({ "agent": name(agent)?, "door": door, "nonce": nonce, "key": key }))).await?;
        Ok(())
    }

    /// `GET /api/computer/relay?host=&port=`, a WebSocket that carries raw
    /// bytes to `host`:`port` (an XFTP server) and back, untouched. Refused
    /// for a host or port it does not carry, or past its bounds.
    pub async fn relay(&self, host: &str, port: u16) -> Result<ClientWs, String> {
        net::connect_ws(&self.base, &format!("/api/computer/relay?host={}&port={port}", encode(host)), &[]).await
    }

    /// `GET /api/computer/keepalive`, a WebSocket held while busy.
    pub async fn keepalive(&self) -> Result<ClientWs, String> {
        net::connect_ws(&self.base, "/api/computer/keepalive", &[]).await
    }

    // ---- as an agent ----

    /// `GET /api/fragments`: the fragments the agent holds a role on.
    pub async fn fragments(&self, agent: &str) -> Result<Vec<FragmentEntry>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            fragments: Vec<FragmentEntry>,
        }
        let a: A = self.json(Method::GET, "/api/fragments", Some(agent), None).await?;
        Ok(a.fragments)
    }

    /// `GET /api/f/{name}/status`'s title: what the fragment is called, as
    /// its live `fragment.json` last said (none when it names none). An
    /// agent's own is its name (docs/api.md, "An agent's name and picture").
    pub async fn title(&self, agent: &str, fragment: &str) -> Result<Option<String>, ApiError> {
        Ok(self.face(agent, fragment).await?.title)
    }

    /// `GET /api/f/{name}/status`'s title and, an agent fragment's, its role
    /// (`agentRole`; none from a platform before roles).
    pub async fn face(&self, agent: &str, fragment: &str) -> Result<Face, ApiError> {
        self.json(Method::GET, &format!("/api/f/{}/status", name(fragment)?), Some(agent), None).await
    }

    /// `GET /api/f/{name}/channels`.
    pub async fn channels(&self, agent: &str, fragment: &str) -> Result<Vec<ChannelInfo>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            channels: Vec<ChannelInfo>,
        }
        let a: A = self.json(Method::GET, &format!("/api/f/{}/channels", name(fragment)?), Some(agent), None).await?;
        Ok(a.channels)
    }

    /// `GET /api/f/{name}/members`.
    pub async fn members(&self, agent: &str, fragment: &str) -> Result<Vec<Member>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            members: Vec<Member>,
        }
        let a: A = self.json(Method::GET, &format!("/api/f/{}/members", name(fragment)?), Some(agent), None).await?;
        Ok(a.members)
    }

    /// `GET /f/{name}/__people?id=…`: what to call each identity (at most
    /// `PEOPLE_MAX`): a person by their email, which a member is shown (decision 48);
    /// an agent by its name, its agent fragment's title, else its label
    /// (docs/api.md, "An agent's name and picture").
    pub async fn people(&self, agent: &str, fragment: &str, ids: &[String]) -> Result<BTreeMap<String, String>, ApiError> {
        let ids: Vec<&String> = ids.iter().take(PEOPLE_MAX).collect();
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let query: Vec<String> = ids.iter().map(|i| format!("id={}", encode(i))).collect();
        let v: Value = self.json(Method::GET, &format!("/f/{}/__people?{}", name(fragment)?, query.join("&")), Some(agent), None).await?;
        let mut out = BTreeMap::new();
        if let Some(profiles) = v["profiles"].as_object() {
            for (id, p) in profiles {
                let title = p["title"].as_str().map(str::trim).filter(|t| !t.is_empty());
                let named = match p["kind"].as_str() {
                    Some("agent") => title.or_else(|| p["name"].as_str()),
                    _ => p["email"].as_str(),
                };
                if let Some(n) = named {
                    out.insert(id.clone(), n.to_string());
                }
            }
        }
        Ok(out)
    }

    /// `GET /api/f/{name}/subscriptions`: the agent's own.
    pub async fn subscriptions(&self, agent: &str, fragment: &str) -> Result<Vec<Subscription>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            subscriptions: Vec<Subscription>,
        }
        let a: A = self.json(Method::GET, &format!("/api/f/{}/subscriptions", name(fragment)?), Some(agent), None).await?;
        Ok(a.subscriptions)
    }

    /// `POST /api/f/{name}/subscriptions {channel, wake: true}`: a record on
    /// the channel wakes this computer.
    pub async fn subscribe_wake(&self, agent: &str, fragment: &str, channel: &str) -> Result<Subscription, ApiError> {
        self.json(Method::POST, &format!("/api/f/{}/subscriptions", name(fragment)?), Some(agent), Some(json!({ "channel": channel, "wake": true }))).await
    }

    /// `GET /api/f/{name}/channels/{channel}?after=&limit=`.
    pub async fn records(&self, agent: &str, fragment: &str, channel: &str, after: u64, limit: u32) -> Result<Page, ApiError> {
        self.json(Method::GET, &format!("/api/f/{}/channels/{}?after={after}&limit={limit}", name(fragment)?, encode(channel)), Some(agent), None).await
    }

    /// A recovery page, capped by the lookup's remaining byte budget before
    /// collecting the body. The byte count includes JSON/wire overhead.
    pub async fn history_records(&self, agent: &str, fragment: &str, channel: &str, after: u64, limit: u32, bytes: usize) -> Result<(Page, usize), ApiError> {
        let path = format!("/api/f/{}/channels/{}?after={after}&limit={limit}", name(fragment)?, encode(channel));
        let body = self.call(Method::GET, &path, Some(agent), None, bytes.min(limits::ANSWER_MAX_BYTES)).await?;
        let page = serde_json::from_slice(&body).map_err(|e| ApiError::Decode(format!("{path}: {e}")))?;
        Ok((page, body.len()))
    }

    /// `POST /api/f/{name}/channels/{channel} {id, body}` → `{record,
    /// replayed}`: `replayed` when the id was posted before (with this
    /// body), and the record's seq (the first post's, for a replay).
    pub async fn post(&self, agent: &str, fragment: &str, channel: &str, id: &str, body: &Value) -> Result<Posted, ApiError> {
        let v: Value = self.json(Method::POST, &format!("/api/f/{}/channels/{}", name(fragment)?, encode(channel)), Some(agent), Some(json!({ "id": id, "body": body }))).await?;
        Ok(Posted { replayed: v["replayed"].as_bool().unwrap_or(false), seq: v["record"]["seq"].as_u64().filter(|s| *s > 0) })
    }

    /// `PUT /api/f/{name}/channels/{channel}/draft {turn, text}`.
    pub async fn draft(&self, agent: &str, fragment: &str, channel: &str, turn: &str, text: Option<&str>) -> Result<(), ApiError> {
        let _: Value = self.json(Method::PUT, &format!("/api/f/{}/channels/{}/draft", name(fragment)?, encode(channel)), Some(agent), Some(json!({ "turn": turn, "text": text }))).await?;
        Ok(())
    }

    /// `PUT /api/f/{name}/blobs/{sha256}`, typed `content-type`.
    pub async fn put_blob(&self, agent: &str, fragment: &str, sha256: &str, content_type: &str, bytes: Bytes) -> Result<(), ApiError> {
        assert!(sha256.len() == 64, "a blob is named by its sha256");
        self.call(Method::PUT, &format!("/api/f/{}/blobs/{sha256}", name(fragment)?), Some(agent), Some((content_type, bytes)), limits::ANSWER_MAX_BYTES).await?;
        Ok(())
    }

    /// `GET /api/f/{name}/blobs/{sha256}`, at most `ATTACHMENT_MAX_BYTES`.
    pub async fn get_blob(&self, agent: &str, fragment: &str, sha256: &str) -> Result<Bytes, ApiError> {
        let max = usize::try_from(limits::ATTACHMENT_MAX_BYTES).expect("25 MiB fits");
        self.call(Method::GET, &format!("/api/f/{}/blobs/{}", name(fragment)?, encode(sha256)), Some(agent), None, max).await
    }

    /// `GET /api/f/{name}/files`: the files at main.
    pub async fn files(&self, agent: &str, fragment: &str) -> Result<Vec<FileEntry>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            files: Vec<FileEntry>,
        }
        let a: A = self.json(Method::GET, &format!("/api/f/{}/files", name(fragment)?), Some(agent), None).await?;
        Ok(a.files)
    }

    /// `GET /api/f/{name}/file?path=`: one file's bytes at main.
    pub async fn file(&self, agent: &str, fragment: &str, path: &str, max: usize) -> Result<Bytes, ApiError> {
        self.call(Method::GET, &format!("/api/f/{}/file?path={}", name(fragment)?, encode(path)), Some(agent), None, max).await
    }

    // ---- as an agent acting for a person (`for`: decision 36) ----

    /// `GET /api/fragments?for=`: the fragments the agent reaches acting for
    /// `person`, each with its kind.
    pub async fn fragments_for(&self, agent: &str, person: &str) -> Result<Vec<FragmentEntry>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            fragments: Vec<FragmentEntry>,
        }
        let a: A = self.json(Method::GET, &format!("/api/fragments?for={}", encode(person)), Some(agent), None).await?;
        Ok(a.fragments)
    }

    /// `POST /api/fragments?for= {name, template, title}`: a fragment made
    /// by its name in full for `person` (the agent's owner: theirs, billed to
    /// them, the agent its maker and an editor, which the platform tells it
    /// as a `joined`); a name taken is 409.
    pub async fn create_named_for(&self, agent: &str, person: &str, fragment: &str, template: &str, title: &str) -> Result<(), ApiError> {
        let _: Value = self.json(Method::POST, &format!("/api/fragments?for={}", encode(person)), Some(agent), Some(json!({ "name": name(fragment)?, "template": template, "title": title }))).await?;
        Ok(())
    }

    /// An agent's home (`Home`), as its `agent.json` says now: none when it
    /// could not be read (a 404, no file, is `Home::Chat`).
    pub async fn home(&self, agent: &str) -> Result<Home, ApiError> {
        match self.file(agent, agent, "agent.json", AGENT_JSON_MAX_BYTES).await {
            Ok(b) => Ok(Home::of(Some(&b))),
            Err(e) if e.status() == Some(404) => Ok(Home::of(None)),
            Err(e) => Err(e),
        }
    }

    /// `GET /api/f/{name}/files?for=`: the files at main, a blessed
    /// template's data among them.
    pub async fn files_for(&self, agent: &str, fragment: &str, person: &str) -> Result<Vec<FileEntry>, ApiError> {
        #[derive(Deserialize)]
        struct A {
            files: Vec<FileEntry>,
        }
        let a: A = self.json(Method::GET, &format!("/api/f/{}/files?for={}", name(fragment)?, encode(person)), Some(agent), None).await?;
        Ok(a.files)
    }

    /// `GET /api/f/{name}/file?path=&for=`: one file's bytes at main.
    pub async fn file_for(&self, agent: &str, fragment: &str, person: &str, path: &str, max: usize) -> Result<Bytes, ApiError> {
        self.call(Method::GET, &format!("/api/f/{}/file?path={}&for={}", name(fragment)?, encode(path), encode(person)), Some(agent), None, max).await
    }

    /// `POST /api/f/{name}/files {files, message, key}`: one commit to main.
    pub async fn commit(&self, agent: &str, fragment: &str, files: &[FileChange], message: &str, key: &str) -> Result<String, ApiError> {
        let v: Value = self.json(Method::POST, &format!("/api/f/{}/files", name(fragment)?), Some(agent), Some(json!({ "files": files, "message": message, "key": key }))).await?;
        Ok(v["commit"].as_str().unwrap_or("").to_string())
    }

    /// `GET /api/f/{name}/schedules?for=`: a fragment's scheduled records
    /// (docs/api.md, "Scheduled records"), as the agent acting for its
    /// owner, as `fragment schedule list` asks on a computer. The script
    /// runtime's `schedule` (the stub's).
    pub async fn schedules(&self, agent: &str, owner: &str, fragment: &str) -> Result<Value, ApiError> {
        self.json(Method::GET, &format!("/api/f/{}/schedules?for={}", name(fragment)?, encode(owner)), Some(agent), None).await
    }

    /// `POST /api/f/{name}/schedules?for= {cron, channel, record}`: a
    /// scheduled record, as the agent acting for its owner (`{schedule,
    /// replayed}`).
    pub async fn add_schedule(&self, agent: &str, owner: &str, fragment: &str, cron: &str, channel: &str, record: &Value) -> Result<Value, ApiError> {
        let body = json!({ "cron": cron, "channel": channel, "record": record });
        self.json(Method::POST, &format!("/api/f/{}/schedules?for={}", name(fragment)?, encode(owner)), Some(agent), Some(body)).await
    }

    /// `DELETE /api/f/{name}/schedules/{id}?for=`: one of them removed.
    pub async fn remove_schedule(&self, agent: &str, owner: &str, fragment: &str, id: u64) -> Result<Value, ApiError> {
        self.json(Method::DELETE, &format!("/api/f/{}/schedules/{id}?for={}", name(fragment)?, encode(owner)), Some(agent), None).await
    }

    /// `POST /api/f/{name}/door?for=`: the agent asking, for its owner, to
    /// bind a connector as its own door, which the platform refuses whomever
    /// an agent acts for (docs/api.md, "An agent's doors"): the script's
    /// `door bind` asks it, for the platform's lanes.
    pub async fn bind_door(&self, agent: &str, owner: &str, fragment: &str) -> Result<Value, ApiError> {
        let body = json!({ "act": "link", "agent": agent });
        self.json(Method::POST, &format!("/api/f/{}/door?for={}", name(fragment)?, encode(owner)), Some(agent), Some(body)).await
    }

    /// `GET /f/{name}/__live`, as the agent.
    pub async fn live(&self, agent: &str, fragment: &str) -> Result<ClientWs, String> {
        let fragment = name(fragment).map_err(|e| e.to_string())?;
        net::connect_ws(&self.base, &format!("/f/{fragment}/__live"), &[(AGENT_HEADER, agent.to_string())]).await
    }
}

/// Whether a status is the platform's "too many": a draft past the pace.
pub fn rate_limited(e: &ApiError) -> bool {
    e.status() == Some(StatusCode::TOO_MANY_REQUESTS.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str, kind: &str, owned: bool) -> FragmentEntry {
        FragmentEntry { name: name.into(), role: "editor".into(), kind: kind.into(), owned, title: String::new() }
    }

    /// Goal (Paul, 2026-10-26): an agent's SimpleX chat has one name, its
    /// label's, chosen once (a valid name, its suffix the platform's
    /// alphabet), and its home is that chat only while its agent.json says
    /// `simplex` and its link names it, else its own chat. Valid: either
    /// way. Invalid: agent.json naming anything else or not JSON; `simplex`
    /// with no SimpleX chat (its own chat); its SimpleX chat named and not
    /// made yet (waited for, then its own chat); its own chat one it is not
    /// in, or another agent's, or someone else's (none).
    #[test]
    fn an_agents_home_is_its_simplex_chat_or_its_own() {
        assert_eq!(simplex_label("juniper--k3x9"), "juniper-simplex");
        assert_eq!(simplex_chat_name("juniper--k3x9", [0, 31, 32, 255]), "juniper-simplex--a9a9");
        let made = simplex_chat_name("juniper--k3x9", [7, 100, 200, 13]);
        assert!(made.starts_with("juniper-simplex--") && made.len() == "juniper-simplex--".len() + 4 && name(&made).is_ok(), "{made}");
        assert!(made[17..].bytes().all(|b| SUFFIX_ALPHABET.contains(&b)), "{made}");
        let simplex = br#"{"tier":"cheap","home":"simplex"}"#.as_slice();
        assert_eq!(Home::of(Some(simplex)), Home::Simplex);
        for chat in [None, Some(br#"{"tier":"cheap"}"#.as_slice()), Some(br#"{"home":"chat"}"#), Some(br#"{"home":"telegram"}"#), Some(br#"{"home":true}"#), Some(b"not json")] {
            assert_eq!(Home::of(chat), Home::Chat, "{chat:?}");
        }
        let list = [listed("juniper-chat--p2m4", "app", true), listed("juniper-chat--r7t5", "chat", false), listed("maple-chat--z8w6", "chat", true), listed("juniper-chat--h6j7", "chat", true)];
        let sx = Some("juniper-simplex--w9x2");
        let own = HomeChat::In("juniper-chat--h6j7".into());
        // its SimpleX chat made (in its own list): its home, waited or not
        let made: Vec<FragmentEntry> = list.iter().cloned().chain([listed("juniper-simplex--w9x2", "chat", true)]).collect();
        for waited in [false, true] {
            assert_eq!(home_chat("juniper--k3x9", Home::Simplex, sx, &made, &made, waited), HomeChat::In("juniper-simplex--w9x2".into()));
        }
        // named and not made yet (Alex's review of #620): waited for, then its own chat; none of its own, nowhere
        assert_eq!(home_chat("juniper--k3x9", Home::Simplex, sx, &list, &list, false), HomeChat::Unmade);
        assert_eq!(home_chat("juniper--k3x9", Home::Simplex, sx, &list, &list, true), own, "waited its most: its own chat");
        assert_eq!(home_chat("loner--k3x9", Home::Simplex, sx, &[], &[], true), HomeChat::Nowhere);
        assert_eq!(home_chat("juniper--k3x9", Home::Chat, sx, &list, &list, false), own, "its home its own chat: no wait");
        assert_eq!(home_chat("juniper--k3x9", Home::Simplex, None, &list, &list, false), own, "no SimpleX chat: its own");
        assert_eq!(home_chat("juniper--k3x9", Home::Chat, None, &list, &list[..3], false), HomeChat::Nowhere, "its own chat, left: none");
        assert_eq!(home_chat("juniper--k3x9", Home::Chat, None, &list[..3], &list, false), HomeChat::Nowhere, "only its owner's chat of that label");
        assert_eq!(home_chat("maple-2--q4w5", Home::Chat, None, &list, &list, false), HomeChat::Nowhere);
        assert_eq!(home_chat("loner--k3x9", Home::Simplex, None, &[], &[], false), HomeChat::Nowhere);
    }

    #[test]
    fn names_and_queries_stay_in_their_route() {
        assert!(name("talk--k3x9").is_ok());
        for bad in ["", "../x", "a/b", "a?b", "a b"] {
            assert!(name(bad).is_err(), "{bad}");
        }
        assert_eq!(encode("id:ab cd/"), "id%3Aab%20cd%2F");
        assert!(ApiError::Refused { status: 503, error: String::new(), message: String::new() }.retryable());
        assert!(ApiError::Refused { status: 429, error: String::new(), message: String::new() }.retryable());
        assert!(!ApiError::Refused { status: 409, error: String::new(), message: String::new() }.retryable());
        assert!(ApiError::Refused { status: 403, error: String::new(), message: String::new() }.gone());
        assert!(ApiError::Transport("x".into()).retryable());
    }

    #[test]
    fn file_changes_are_the_routes() {
        let v = serde_json::to_value(vec![
            FileChange::Text { path: "SOUL.md".into(), text: "x".into() },
            FileChange::Base64 { path: "a.png".into(), base64: "AA==".into() },
            FileChange::Delete { path: "old.md".into(), delete: true },
        ])
        .unwrap();
        assert_eq!(v, json!([{ "path": "SOUL.md", "text": "x" }, { "path": "a.png", "base64": "AA==" }, { "path": "old.md", "delete": true }]));
    }
}
