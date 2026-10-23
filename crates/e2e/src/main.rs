//! The fragment end-to-end suite: the real cell under `wrangler dev`
//! (workerd, from a staged copy under `target/e2e/<run>`, so a running
//! `xtask dev` is never touched), the code.storage fake from
//! `crates/fakes`, the real CLI, and signed HTTP the
//! way the CLI and a browser send it. The fakes stand only at vendor
//! boundaries (code.storage, WorkOS, the model, a push service): this is
//! the lower rung under the hosted lane.
//!
//! `cargo xtask e2e [--only <section>[,<section>...] | --except <section>[,...]]`.
//! Each section makes its own fragments, so any set can run alone. Every check prints `ok` or
//! `FAIL`, and a section that stops early is one FAIL, with the sections
//! after it still run; the process exits non-zero when any check fails.
//! A run's scratch (`target/e2e/<run>`: the staged cell, each node boot's
//! log, the lanes' directories and screenshots) is
//! removed when every check passes, and kept when one fails (or when
//! `FRAGMENT_E2E_KEEP` is set).
//!
//! The hosted lane (`--hosted`, hosted.rs) runs the same sections against
//! a branch deployment on real vendors: each section says what it needs
//! (needs.rs), and one that needs what a preview lacks (a fake, the node,
//! the whole deployment, local Docker) is a skip that says why.
//!
//! `FRAGMENT_E2E_RUNTIME=celld` runs the local node on `celld dev` instead
//! (`CELLD_BIN`; docs/self-host.md, seam 1), on the same fakes, its cards
//! shot by the renderer (seam 7): a section that needs the runtime's own
//! containers is a skip (needs.rs), saying why. Its secrets are OpenBao's,
//! behind the cell's stand-ins (docs/self-host.md, seam 12), unless
//! `FRAGMENT_E2E_SECRETS=vars` puts them in `.dev.vars`.
//!
//! `FRAGMENT_E2E_CODESTORE=macrofiche` (or `external`) keeps the local
//! node's git outside the run (store.rs; docs/self-host.md, seam 5): the
//! fake is not the store, so git moves through the platform's routes as on
//! the hosted lane, and a section that needs the fake's levers
//! (`Need::StoreFake`) is a skip that says so.

mod api;
mod browser;
mod hosted;
mod lanes;
mod needs;
mod store;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use fragment_devstack as devstack;
use fragment_fakes::codestorage::{self as fake, CodeStorage};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use api::Api;
pub use needs::Need;

pub const SUFFIX: &str = "fragment.localhost";
/// The fragments' suffix on a node shaped as fragment.club is since the
/// move to fragment.boats (`Shape::TwoSites`).
pub const BOATS: &str = "boats.localhost";
/// Where the next node puts the platform and the fragments, as a browser
/// tells sites apart: Chrome takes an unknown top-level domain's last label
/// as its suffix, so every `*.fragment.localhost` is one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The platform on 127.0.0.1, the fragments under `fragment.localhost`.
    Plain,
    /// As fragment.club is (docs/api.md, Hosts): the
    /// platform at `fragment.localhost`, cross-site from the fragments
    /// (`<flat>.boats.localhost`, one site with each other).
    TwoSites,
    /// As `Plain`, its model route shaped as Finite's own deployment
    /// (deploy/example.jsonc; decision 61): `cheap` on Finite Private and
    /// `medium` on OpenRouter (each the model fake's OpenAI-compatible face,
    /// one key a test value), the fallback and vision on Workers AI's
    /// GLM-5.3 Flash.
    FinitePrivate,
    /// As `Plain`, with no Stripe (no `stripe` in its config): billing
    /// off, as finite.chat launched, every seat an operator's comp.
    SellsNothing,
    /// As `Plain`, its catalog one operator key longer (`ADDED_KEY`, at a
    /// price of its own) and nothing else changed: a row an operator adds,
    /// with no version to raise (#451).
    OneKeyMore,
    /// As `Plain`, with no mailer: no mail fake and no sender, as a
    /// self-hosted deployment on celld (no Email Sending) runs.
    NoMailer,
    /// As `Plain`, its model route on a model of the deployment's own, at
    /// no price (`self_hosted_models`), as docs/self-host.md, seam 3, runs
    /// Bonsai.
    SelfHosted,
}

/// Finite Private's key on a node shaped as Finite's (`Shape::FinitePrivate`):
/// a test value, in `fpk_live_`'s shape, which the model fake asks for.
pub const FINITE_PRIVATE_KEY: &str = "fpk_live_e2e0c0ffee";

/// `FRAGMENT_MODELS` on a node shaped as Finite's: deploy/example.jsonc's
/// two alternatives at once, less their keys, each API's base URL the model
/// fake's: `cheap` on Finite Private, `medium` on OpenRouter's GLM-5.3
/// Flash with its provider routing (`openrouter_routing`), `fallback` and
/// `vision` on Workers AI's GLM-5.3 Flash.
pub fn finite_private_models(fake: &str) -> Value {
    let private = json!({ "provider": "openai", "base_url": format!("{fake}/v1"), "model": fragment_core::models::FINITE_PRIVATE_FLASH });
    let routed = json!({ "provider": "openai", "base_url": format!("{fake}/v1"), "model": fragment_core::models::OPENROUTER_FLASH, "body": openrouter_routing() });
    let flash = json!({ "provider": "workers_ai", "model": fragment_core::models::GLM_FLASH });
    json!({ "cheap": private, "medium": routed, "fallback": flash, "vision": flash })
}

/// `FRAGMENT_MODELS` on a node shaped as a self-hosted deployment
/// (`Shape::SelfHosted`; docs/self-host.md, seam 3): every tier and the
/// fallback on one model of an OpenAI-compatible server of its own (the
/// model fake's face, answering Finite Private's id), priced at its slots'
/// ceiling, zero; vision on Workers AI, which the server lacks.
pub fn self_hosted_models(fake: &str) -> Value {
    let own = json!({ "provider": "openai", "base_url": format!("{fake}/v1"), "model": fragment_core::models::FINITE_PRIVATE_FLASH, "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } } });
    json!({ "cheap": own, "medium": own, "fallback": own })
}

/// A local run's own model for its agents (docs/self-host.md, seam 3):
/// `FRAGMENT_E2E_MODELS_FILE` names a file holding a deployment's `models`
/// as its config writes them (JSON, `//` comments allowed: dev's
/// `FRAGMENT_MODELS_FILE`'s shape), and `FRAGMENT_E2E_MODEL_KEY_FILE` the
/// file holding the value of the key its OpenAI-compatible slots name (one,
/// as dev binds one). Its agents are then real ones (`Rung`'s `real_agent`).
#[derive(Debug, Clone)]
struct OwnModels {
    /// The cell's `FRAGMENT_MODELS`: each slot less its key.
    cell: String,
    /// The slots on an OpenAI-compatible API, which name the key.
    slots: Vec<String>,
    key: Option<String>,
    /// What the run says of them.
    said: String,
}

fn own_models() -> Result<Option<OwnModels>> {
    let Some(file) = std::env::var_os("FRAGMENT_E2E_MODELS_FILE") else { return Ok(None) };
    let shown = Path::new(&file).display().to_string();
    let text = std::fs::read_to_string(&file).with_context(|| format!("read {shown} (FRAGMENT_E2E_MODELS_FILE)"))?;
    let models: Value = serde_json::from_str(&devstack::strip_comments(&text)).with_context(|| format!("{shown} is a deployment's `models`, as JSON"))?;
    let (cell, slots) = models_less_keys(&models).with_context(|| format!("{shown} (FRAGMENT_E2E_MODELS_FILE)"))?;
    let key = match slots.is_empty() {
        true => None,
        false => {
            let path = std::env::var_os("FRAGMENT_E2E_MODEL_KEY_FILE").with_context(|| format!("{shown}'s slots on an OpenAI-compatible API name a key: name the file holding its value FRAGMENT_E2E_MODEL_KEY_FILE (any word, for a server that takes none)"))?;
            let key = std::fs::read_to_string(&path).with_context(|| format!("read {} (FRAGMENT_E2E_MODEL_KEY_FILE)", Path::new(&path).display()))?.trim().to_string();
            anyhow::ensure!(!key.is_empty(), "FRAGMENT_E2E_MODEL_KEY_FILE's file is empty: a key is a value (any word, for a server that takes none)");
            Some(key)
        }
    };
    let rows: Value = serde_json::from_str(&cell)?;
    let said = rows.as_object().into_iter().flatten().map(|(slot, u)| format!("{slot}: {} at {}", u["model"].as_str().unwrap_or("?"), u["base_url"].as_str().unwrap_or("Workers AI (the fake)"))).collect::<Vec<_>>().join(", ");
    Ok(Some(OwnModels { cell, slots, key, said: format!("{said} ({shown})") }))
}

/// A deployment's `models` as the cell takes them, each slot's `key` (its
/// secret's name) taken out, checked as the cell reads them (a loopback
/// URL allowed: the run's egress is local), and the slots that named a key.
fn models_less_keys(models: &Value) -> Result<(String, Vec<String>)> {
    let mut rows = models.as_object().context("models is an object: each slot, where it runs")?.clone();
    let mut slots = vec![];
    for (slot, row) in rows.iter_mut() {
        let key = row.as_object_mut().with_context(|| format!("models.{slot} is an object"))?.remove("key");
        match (row["provider"].as_str(), key) {
            (Some("openai"), Some(Value::String(_))) => slots.push(slot.clone()),
            (Some("openai"), _) => anyhow::bail!("models.{slot}: an OpenAI-compatible API names its key, a string"),
            (_, Some(_)) => anyhow::bail!("models.{slot}: only an OpenAI-compatible API (provider openai) names a key"),
            (_, None) => {}
        }
    }
    let cell = Value::Object(rows).to_string();
    fragment_core::models::Models::parse(Some(&cell), &fragment_core::price::PriceBook::defaults(), true).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((cell, slots))
}

/// The `body` of OpenRouter's slot on a node shaped as Finite's:
/// deploy/example.jsonc's provider routing.
pub fn openrouter_routing() -> Value {
    json!({ "provider": { "zdr": true, "sort": "throughput", "require_parameters": true, "quantizations": ["fp8", "bf16"], "max_price": { "prompt": 0.15, "completion": 0.5 } } })
}
const ORG: &str = "fragment-e2e";
/// The poll backstop runs this often here (5 minutes in production).
pub const POLL_S: u32 = 2;
/// Blobs no branch names are kept this long here (7 days in production).
pub const BLOB_GRACE_S: u32 = 4;
/// How long a deleted fragment's cleanup may take (`Suite::ended_cleaned`):
/// at `MEMBERS_MAX` members, its alarm tells the lists a batch a pass (a
/// few seconds here; on a preview, each list a Principal made on the spot).
const ENDED_CLEANUP_IN: Duration = Duration::from_secs(120);
/// The pending sign-ins this fleet keeps: small, so the signin lane fills
/// the table and proves the oldest goes first in a few hundred requests.
pub const SIGNINS_PENDING_MAX: u64 = 200;
/// The local node's CLI codes an address may ask for a minute
/// (`FRAGMENT_CLI_BEGINS_PER_IP_MINUTE`): above what a section's people ask
/// for in a minute, from the run's one address, and small enough for the
/// signin section to pass it from another.
pub const CLI_BEGINS_PER_IP_MINUTE: u32 = 120;
/// A new person's plan here: a seat, so each starts the month with its
/// included credit (production's is `guest`).
pub const DEFAULT_PLAN: &str = "seat";
/// How long a computer whose sleep's save keeps failing stays awake here
/// (`computers.unsaved_max_ms`; thirty minutes in production): short, so
/// the computers lane sees the bound run out and its owner told of it.
pub const UNSAVED_MAX_MS: u64 = 45_000;
/// The WorkOS fake's environment.
const WORKOS_CLIENT: &str = "client_fragment_e2e";
const WORKOS_KEY: &str = "sk_test_fragment_e2e";
/// The OpenID Connect fake's client (`FRAGMENT_E2E_SIGNIN=oidc`:
/// docs/self-host.md, seam 4).
pub const OIDC_CLIENT: &str = "fragment-e2e";
const OIDC_SECRET: &str = "oidc-secret-fragment-e2e";
/// The Stripe fake's account key, the node's webhook endpoint's signing
/// secret, and its portal configuration (docs/billing.md).
pub const STRIPE_KEY: &str = "sk_test_fragment_e2e_stripe";
pub const STRIPE_WEBHOOK: &str = "whsec_fragment_e2e";
pub const STRIPE_PORTAL: &str = "bpc_fragment_e2e";
/// Where the platform's mail says it comes from on the e2e's node.
pub const MAIL_FROM: &str = "fragment <mail@fragment.localhost>";
/// The branch a rehearsal of the hosted lane shapes the local node as.
pub const REHEARSAL_BRANCH: &str = "rh";
/// What the fleet's computers may swap in (docs/computers.md): the
/// platform's own catalog, as the hosted e2e deploys it
/// (`deploy/e2e.jsonc`: Google, and all eight operator keys, at their real
/// hosts, which the swap sends to
/// the upstream fake), each key a test value; and an own key's provider
/// behind basic auth, the e2e's alone.
pub const SWAP_CONNECTION: &str = "google";
pub const SWAP_CONNECTION_HOST: &str = "www.googleapis.com";
pub const SWAP_CONNECTION_ENV: &str = "GOOGLE_OAUTH_ACCESS_TOKEN";
/// The connection's note (its row's `note`, the only one in the e2e's
/// catalog), which settings shows under its name.
pub const SWAP_CONNECTION_NOTE: &str = "Google is still reviewing this app, so Google shows a warning that it isn't verified when you connect.";
/// The operator keys, each its test value and its environment variable.
pub const SWAP_KEYS: [(&str, &str, &str); 8] = [
    ("x", "x-bearer-e2e-51a3", "X_API_BEARER_TOKEN"),
    ("firecrawl", "fc-e2e-631b", "FIRECRAWL_API_KEY"),
    ("fal", "fal-e2e-920a", "FAL_KEY"),
    ("browser-use", "bu-e2e-a278", "BROWSER_USE_API_KEY"),
    ("perplexity", "pplx-e2e-7f3a9c", "PERPLEXITY_API_KEY"),
    ("google-places", "AIza-e2e-places-51b2", "GOOGLE_PLACES_API_KEY"),
    ("xai", "xai-e2e-0c4d22", "XAI_API_KEY"),
    ("elevenlabs", "sk_e2e_eleven_9e1f", "ELEVENLABS_API_KEY"),
];
/// The operator key a node shaped `Shape::OneKeyMore` adds (its name, its
/// test value, its environment variable), and its catalog row: $0.02 a
/// call at list.
pub const ADDED_KEY: (&str, &str, &str) = ("e2e-added", "e2e-added-key-3f9b", "E2E_ADDED_KEY");
pub fn added_key_row() -> Value {
    let (name, _, env) = ADDED_KEY;
    json!({ "name": name, "kind": "operator", "hosts": ["api.added.test"], "placements": [{ "header": "authorization", "format": "Bearer {}" }], "env": [env], "price": { "micros": 20_000, "per": 1 } })
}
pub const SWAP_OWN: &str = "e2e-mail";
pub const SWAP_OWN_HOST: &str = "api.mail.test";
pub const SWAP_OWN_ENV: &str = "E2E_MAIL_KEY";
/// The person's own OpenRouter (deploy/e2e.jsonc's row): connected through
/// its sign-in, which a local run makes at the OpenRouter fake; its models'
/// calls go to the fake through the swap's upstream.
pub const SWAP_ROUTER: &str = "openrouter";
pub const SWAP_ROUTER_HOST: &str = "openrouter.ai";
pub const SWAP_ROUTER_ENV: &str = "OPENROUTER_API_KEY";

/// The fleet's provider catalog: `deploy/e2e.jsonc`'s, its keys' store
/// names dropped (the deploy's), Google with its note, and the own key's
/// provider; OpenRouter's sign-in at `router_oauth` (the fake's) when one
/// is named.
pub fn swap_providers(router_oauth: Option<Value>) -> Result<Value> {
    let text = std::fs::read_to_string(devstack::repo_root().join("deploy/e2e.jsonc"))?;
    let config: Value = serde_json::from_str(&devstack::strip_comments(&text))?;
    let mut rows = config["providers"].as_array().cloned().context("deploy/e2e.jsonc names its providers")?;
    for r in &mut rows {
        r.as_object_mut().context("a provider is an object")?.remove("key");
        if let (Some(o), true) = (&router_oauth, r["name"] == SWAP_ROUTER) {
            r["oauth"] = o.clone();
        }
        if r["name"] == SWAP_CONNECTION {
            r["note"] = json!(SWAP_CONNECTION_NOTE);
        }
    }
    rows.push(json!({ "name": SWAP_OWN, "kind": "own", "hosts": [SWAP_OWN_HOST], "placements": [{ "basic": "password" }], "env": [SWAP_OWN_ENV] }));
    fragment_core::catalog::Catalog::parse(&Value::Array(rows.clone()).to_string()).map_err(|e| anyhow!("the e2e's catalog: {e}"))?;
    Ok(Value::Array(rows))
}

/// A vendor fake, the lanes' on a local run only: a section that touches
/// one declares `Need::Fakes`, and the hosted lane skips it before it
/// could. A rehearsal's node still runs on the fakes (`node`), hidden from
/// the lanes as a preview's vendors are.
pub struct Fake<T> {
    what: &'static str,
    fake: Option<T>,
    /// The lanes may use it (a local run's).
    lanes: bool,
}

impl<T> Fake<T> {
    /// A local run's fake, or (`hidden`) a rehearsal's: the node's vendor,
    /// which no lane reaches.
    fn of(hidden: bool, what: &'static str, fake: T) -> Fake<T> {
        Fake { what, fake: Some(fake), lanes: !hidden }
    }

    fn absent(what: &'static str) -> Fake<T> {
        Fake { what, fake: None, lanes: false }
    }

    /// The fake as the node is configured with it.
    fn node(&self) -> &T {
        self.fake.as_ref().unwrap_or_else(|| panic!("a hosted run starts no node on the {} fake", self.what))
    }
}

impl<T> Deref for Fake<T> {
    type Target = T;

    fn deref(&self) -> &T {
        match (&self.fake, self.lanes) {
            (Some(fake), true) => fake,
            _ => panic!("the {} fake is not this run's (a hosted run's, or code.storage's on a store outside the run): a section that uses it declares Need::Fakes, or Need::StoreFake", self.what),
        }
    }
}

/// Whether the run asks for sign-in on OpenID Connect
/// (`FRAGMENT_E2E_SIGNIN=oidc`).
fn signin_by_oidc() -> bool {
    std::env::var("FRAGMENT_E2E_SIGNIN").as_deref() == Ok("oidc")
}

/// One section as a dry run plans it: what it needs, and why it would be
/// skipped (`None`: it would run).
pub struct Planned {
    pub section: String,
    pub needs: Vec<Need>,
    pub skip: Option<String>,
}

pub struct Suite {
    /// The sections to run (`None`: all of them), and those not to.
    only: Option<Vec<String>>,
    except: Vec<String>,
    /// The table's shard to run, `k` of `--shard k/n` (from 1; `None`: no split).
    shard: Option<u32>,
    /// Where the run is: under `wrangler dev` with the fakes, or a preview.
    rung: needs::Rung,
    /// The hosted lane's rules: on a preview, or its rehearsal on the local
    /// node (shaped as a branch deployment, its fakes hidden from the lanes).
    hosted_rules: bool,
    /// The preview a hosted run targets (`None`: the local node).
    preview: Option<api::Preview>,
    /// A hosted run's candidate image (`--candidate`, the canary: docs/
    /// computers.md, "Stable and canary"): the real-agent sections' new
    /// computers run it from their first start, and agent-restart's restart
    /// upgrades into it and back. `None`: the deployment's default.
    pub candidate: Option<String>,
    /// A dry run's plan (`Some`: nothing runs, each section is planned).
    plan: Option<Vec<Planned>>,
    /// What the run's APIs share: the levers' secret, and (hosted) its
    /// people and the paid calls it may lend.
    shared: Arc<api::Run>,
    /// Who made each fragment `create` made (hosted: commits and deploys
    /// go through the API as its owner, where locally a writer pushes to
    /// the code.storage fake).
    owners: RefCell<BTreeMap<String, Keys>>,
    /// Every section a lane asked for, run or not: a name given to
    /// `--only` or `--except` that is none of them fails the run.
    asked: Vec<String>,
    passed: usize,
    failed: Vec<String>,
    /// Checks this rung cannot make, each with why and where it is made.
    skipped: Vec<String>,
    /// The sections that ran, in order: the last is the one running.
    ran: Vec<String>,
    /// A lane stopped early: the next section to run gets the node back as
    /// the lanes expect it first.
    recovery_due: bool,
    /// Why the node could not be brought back: every later section is a FAIL.
    lost: Option<String>,
    /// The cron fragment, deployed sections before `triggers` checks its
    /// tick (lanes/mod.rs), or why it was not.
    cron: Option<Result<Option<lanes::jobs::Cron>, String>>,
    /// Chrome, started when a lane first asks for it and shared after.
    chrome: browser::Shared,
    /// The local node's tools (`None`: a hosted run, which starts no node).
    tools: Option<devstack::Tools>,
    /// celld's, when the local node runs on it (`FRAGMENT_E2E_RUNTIME=celld`).
    celld: Option<devstack::celld::CelldTools>,
    /// OpenBao, the cell's secrets behind its stand-ins on celld (the
    /// default; `FRAGMENT_E2E_SECRETS=vars` for the stand-ins' variables:
    /// docs/self-host.md, seam 12), its state and audit log in the run's
    /// scratch.
    openbao: Option<devstack::openbao::OpenBao>,
    /// Where a node without the `BROWSER` binding (celld) shoots preview
    /// cards: the renderer over the pinned chrome-headless-shell, its pages
    /// let reach this run's fragments alone (docs/self-host.md, seam 7).
    renderer: Option<devstack::rendering::Rendering>,
    node: Option<devstack::Node>,
    /// The sandcastle nodes the run started, in `FRAGMENT_NODES`' order
    /// (empty: computers run in the runtime's own containers), and the
    /// images they hold, by name (`FRAGMENT_NODES`' `images`).
    nodes: Vec<devstack::sandcastle::SandcastleNode>,
    node_images: Value,
    /// The binaries a node is made of, for a section's own (a person's
    /// node it pairs: lanes/pairing.rs).
    sandcastle: Option<devstack::sandcastle::Tools>,
    /// Whether the run's people may pair nodes of their own
    /// (`FRAGMENT_BYOC`): on wherever the run has nodes, but while a
    /// section turns it off.
    byoc: bool,
    /// The image tags built for the nodes, removed as the run ends.
    image_tags: Vec<String>,
    /// The agents' own model, a local run's (`FRAGMENT_E2E_MODELS_FILE`).
    own_models: Option<OwnModels>,
    port: u16,
    /// Distinguishes this run's fragment names from any earlier state.
    run: String,
    pub fake: Fake<CodeStorage>,
    /// The code store outside the run, when the node's git lives there
    /// (`FRAGMENT_E2E_CODESTORE`, store.rs; `None`: the fake, or a hosted
    /// run's own vendor).
    store: Option<store::External>,
    /// macrofiche, when the run started it: stopped at the run's end.
    macrofiche: Option<devstack::codestore::Macrofiche>,
    /// The model route's vendor boundary, text and images: Workers AI, scripted.
    pub ai: Fake<fragment_fakes::workers_ai::WorkersAi>,
    pub push: Fake<fragment_fakes::push::PushService>,
    /// The platform's mail: Email Sending, faked; what was sent is read here.
    pub mail: Fake<fragment_fakes::mail::Mailer>,
    /// Stripe: seats sold, faked; its events go to the node's webhook.
    pub stripe: Fake<fragment_fakes::stripe::Stripe>,
    org_key: String,
    host_secret: String,
    /// The node's test levers' secret (`FRAGMENT_TEST_SECRET`), made per run.
    test_secret: String,
    /// Sign-in's stand-in: people sign in through it (`Api::person`), and
    /// Pipes', which hands out connections' tokens.
    pub workos: Fake<fragment_fakes::workos::WorkOs>,
    /// An OpenID Connect provider: people sign in through it in WorkOS's
    /// place when the run says so (`oidc_signin`); WorkOS then serves Pipes.
    pub oidc: Fake<fragment_fakes::oidc::Oidc>,
    /// The provider APIs a computer's swap sends to.
    pub upstream: Fake<fragment_fakes::upstream::Upstream>,
    /// OpenRouter: an own key's sign-in (the browser's and the cell's
    /// own calls), and its models (a guest's, through `upstream`).
    pub openrouter: Fake<fragment_fakes::openrouter::OpenRouter>,
    /// The fleet's operator (`FRAGMENT_OPERATORS`): a key a person approves
    /// when a lane needs it.
    pub operator: Keys,
    /// An operator key no person holds (`FRAGMENT_OPERATORS` lists it, and
    /// nothing ever approves it): a wipe's (`Need::Operator`). Local: made
    /// per run; hosted: `--operator-key-file`'s, or none.
    pub wiper: Option<Keys>,
    pub cli: PathBuf,
    pub scratch: PathBuf,
    /// The node's own copy of the cell project (never `cell/`, where `xtask dev` runs).
    project: PathBuf,
    /// The next node's shape (`start_as_browsers_see_it`).
    shape: Shape,
    /// Whether the containers the run's nodes left are removed
    /// (`remove_containers`): at its end, or as it drops.
    containers_removed: bool,
}

impl Drop for Suite {
    /// A run that ends early (an error out of `local`, a panic) leaves
    /// none of its containers either: its node is killed first (its own
    /// drop), so it makes none after they are removed.
    fn drop(&mut self) {
        drop(self.node.take());
        if let Err(e) = self.remove_containers() {
            println!("      could not remove this run's containers: {e:#}");
        }
    }
}

impl Suite {
    /// The deployment's secrets (label, value): the platform Worker holds
    /// them, never an app. A local run's alone: a section that reads them
    /// declares `Need::Deployment`.
    pub fn deployment_secrets(&self) -> Vec<(&'static str, String)> {
        assert!(!self.hosted_rules, "the hosted lane holds none of the deployment's secrets: a section that reads them declares Need::Deployment");
        vec![
            ("host secret", self.host_secret.clone()),
            // a line of the PEM's body: found however the PEM was escaped
            ("code.storage key", self.org_key.lines().find(|l| !l.starts_with("-----") && !l.trim().is_empty()).unwrap_or_default().trim().to_string()),
            ("WorkOS API key", WORKOS_KEY.into()),
            ("OpenID Connect client secret", OIDC_SECRET.into()),
            ("test secret", self.test_secret.clone()),
        ]
        .into_iter()
        // the cell's OpenBao token: secret zero, the one given outside it
        .chain(self.openbao.as_ref().and_then(|b| std::fs::read_to_string(b.layout().cell_token()).ok()).map(|t| ("OpenBao token", t.trim().to_string())))
        .collect()
    }

    /// Whether the cell's secrets are OpenBao's, behind its stand-ins.
    pub fn has_openbao(&self) -> bool {
        self.openbao.is_some()
    }

    /// OpenBao stops: to the cell, its secrets' service is down.
    pub fn openbao_down(&mut self) -> Result<()> {
        Ok(self.openbao.as_mut().context("the run's secrets are OpenBao's")?.down()?)
    }

    /// OpenBao starts again on its state: unsealed by its static seal.
    pub fn openbao_up(&mut self) -> Result<Duration> {
        Ok(self.openbao.as_mut().context("the run's secrets are OpenBao's")?.up()?)
    }

    /// How many times the run's OpenBao initialised its state.
    pub fn openbao_initialisations(&self) -> Result<usize> {
        Ok(devstack::openbao::initialisations(self.openbao.as_ref().context("the run's secrets are OpenBao's")?.layout())?)
    }

    /// Whether the run keeps the hosted lane's rules (on a preview, or its
    /// rehearsal): a section adapts a check, or skips it, where a real
    /// vendor or the deployment's own image answers.
    pub fn hosted(&self) -> bool {
        self.hosted_rules
    }

    /// Whether people sign in through the OpenID Connect fake
    /// (`FRAGMENT_E2E_SIGNIN=oidc`: docs/self-host.md, seam 4), not WorkOS's.
    /// A hosted run's sign in through the levers.
    pub fn oidc_signin(&self) -> bool {
        !self.hosted_rules && signin_by_oidc()
    }

    /// Whether the local node runs on celld (`FRAGMENT_E2E_RUNTIME=celld`).
    pub fn on_celld(&self) -> bool {
        self.celld.is_some()
    }

    /// Whether a job's sleep outlives a crash of the node: celld's
    /// Workflows keep it (each instance a cell, its sleep an alarm); local
    /// workerd's hold it as a timer in the process.
    pub fn durable_workflows(&self) -> bool {
        self.on_celld()
    }

    /// Whether the run's one node fronts the machine's real engine
    /// (`FRAGMENT_E2E_NODES=real`).
    pub fn real_engine(&self) -> bool {
        self.nodes.iter().any(|n| n.is_real())
    }

    /// Whether the run's computers are placed on sandcastle nodes it started.
    pub fn has_nodes(&self) -> bool {
        !self.nodes.is_empty()
    }

    /// Whether the local node makes computers: the runtime's own (wrangler
    /// dev's, in Docker), or, on celld, sandcastle nodes' (seam 2).
    pub fn has_computers(&self) -> bool {
        !self.on_celld() || self.has_nodes()
    }

    /// The nodes' ids, in `FRAGMENT_NODES`' order: the first listens, the
    /// second dials in.
    pub fn node_ids(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.id.clone()).collect()
    }

    /// How the platform reaches node `id`: `"listens"` or `"dials in"`.
    pub fn node_reach(&self, id: &str) -> &'static str {
        match self.nodes.iter().find(|n| n.id == id).map(|n| n.reach) {
            Some(devstack::sandcastle::Reach::Listen(_)) => "listens",
            Some(devstack::sandcastle::Reach::Uplink) => "dials in",
            None => "is not one of the run's",
        }
    }

    /// Node `id`'s process stops: to the platform it is down. Its engine,
    /// and any container on it, stay.
    pub fn node_down(&mut self, id: &str) -> Result<()> {
        self.nodes.iter_mut().find(|n| n.id == id).with_context(|| format!("no node {id}"))?.down()
    }

    /// Node `id`'s process starts again.
    pub fn node_up(&mut self, id: &str) -> Result<()> {
        self.nodes.iter_mut().find(|n| n.id == id).with_context(|| format!("no node {id}"))?.up()
    }

    /// The containers running on node `id`'s engine that the run's nodes
    /// started at or after `since_ms` (devstack's `sandcastle::now_ms`), by
    /// name.
    pub fn node_running_since(&self, id: &str, since_ms: u64) -> Result<Vec<String>> {
        self.nodes.iter().find(|n| n.id == id).with_context(|| format!("no node {id}"))?.running_since(since_ms)
    }

    /// Container `name` on node `id` killed, behind the platform's back.
    pub fn node_kill(&self, id: &str, name: &str) -> Result<()> {
        self.nodes.iter().find(|n| n.id == id).with_context(|| format!("no node {id}"))?.kill(name)
    }

    /// A node of a person's own, for them to pair (lanes/pairing.rs): its
    /// engine double started, its label `label` until it is paired.
    pub fn node_to_pair(&self, label: &str) -> Result<devstack::sandcastle::SandcastleNode> {
        let tools = self.sandcastle.as_ref().context("a run with nodes locates sandcastle's binaries")?;
        let spec = devstack::sandcastle::NodeSpec {
            id: label.into(),
            reach: devstack::sandcastle::Reach::Uplink,
            engine: devstack::sandcastle::Engine::Double,
            // short: its sockets live under it (`sandcastle::socket_path`)
            dir: self.scratch.join("n").join(label),
            platform: self.platform(),
            capacity: 1,
            log_dir: self.scratch.clone(),
            run: self.run.clone(),
        };
        devstack::sandcastle::SandcastleNode::start_to_pair(tools, &spec)
    }

    /// The local node's platform origin.
    pub fn platform(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Whether the next node start lets people pair their own nodes.
    pub fn set_byoc(&mut self, on: bool) {
        self.byoc = on;
    }

    /// A heavy section runs only when `--only` names it (the real-Hermes
    /// lane builds a 3.8 GB image); otherwise it is a skip that says how.
    pub fn section_by_name(&mut self, name: &str, needs: &[Need], why: &str) -> bool {
        if self.only.as_ref().is_some_and(|only| only.iter().any(|o| o == name)) {
            return self.section(name, needs);
        }
        self.asked.push(name.to_string());
        if self.selected(name) {
            // what it needs first: by name it would be skipped all the same
            let why = match (self.unmet(needs), &self.preview) {
                (Some((need, missing)), _) => format!("{missing} ({})", need.name()),
                (None, Some(preview)) => format!("{why}: run it by name, cargo xtask e2e --hosted --config <deploy config> --branch {} --only {name}", preview.branch),
                (None, None) => format!("{why}: run it by name, cargo xtask e2e --only {name}"),
            };
            match &mut self.plan {
                Some(plan) => plan.push(Planned { section: name.into(), needs: needs.to_vec(), skip: Some(why) }),
                None => self.skip(&format!("the {name} section"), &why),
            }
        }
        false
    }

    /// Whether `--only`, `--except` and `--shard` select the section `name`.
    fn selected(&self, name: &str) -> bool {
        let named = self.only.as_ref().is_none_or(|only| only.iter().any(|o| o == name)) && !self.except.iter().any(|e| e == name);
        named && self.shard.is_none_or(|k| lanes::shard_runs(k, name))
    }

    /// Whether the section `name`, needing `needs`, runs in this suite:
    /// selected, on a rung that has what it needs, and not a dry run.
    pub fn runs(&self, name: &str, needs: &[Need]) -> bool {
        self.selected(name) && self.plan.is_none() && self.unmet(needs).is_none()
    }

    /// The first of `needs` this run lacks, with why: its rung's
    /// (`needs::unmet`), or the code.storage fake as the store, on a run
    /// whose git is outside it.
    fn unmet(&self, needs: &[Need]) -> Option<(Need, &'static str)> {
        let outside = self.store.is_some() && needs.contains(&Need::StoreFake);
        needs::unmet(needs, self.rung).or_else(|| outside.then_some((Need::StoreFake, needs::STORE_OUTSIDE)))
    }

    /// Whether the section `name` runs (it prints its header when it does).
    /// It needs `needs`: on a rung without one of them it is a skip that
    /// says which, and a dry run plans it instead.
    pub fn section(&mut self, name: &str, needs: &[Need]) -> bool {
        self.asked.push(name.to_string());
        if !self.selected(name) {
            return false;
        }
        let unmet = self.unmet(needs);
        if let Some(plan) = &mut self.plan {
            let skip = unmet.map(|(need, why)| format!("{why} ({})", need.name()));
            plan.push(Planned { section: name.into(), needs: needs.to_vec(), skip });
            return false;
        }
        if let Some((need, why)) = unmet {
            self.skip(&format!("the {name} section"), &format!("{why} ({})", need.name()));
            return false;
        }
        println!("\n# {name}");
        self.ran.push(name.to_string());
        if let Some(why) = self.lost.clone() {
            self.fail(&format!("{name} did not run: no node"), why);
            return false;
        }
        if std::mem::take(&mut self.recovery_due) {
            if let Err(e) = self.recover() {
                let why = format!("{e:#}");
                self.fail(&format!("{name} did not run: the node would not start again"), &why);
                self.lost = Some(why);
                return false;
            }
        }
        true
    }

    pub fn ok(&mut self, label: &str, cond: bool, detail: impl std::fmt::Display) {
        if cond {
            self.passed += 1;
            println!("ok    {label}");
        } else {
            self.fail(label, detail);
        }
    }

    /// A check this run cannot make (local workerd's limits, the hosted
    /// lane's; a runtime's or a store's): said with why, counted, and never
    /// a pass.
    pub fn skip(&mut self, label: &str, why: &str) {
        self.skipped.push(label.to_string());
        println!("skip  {label}: {why}");
    }

    pub fn fail(&mut self, label: &str, detail: impl std::fmt::Display) {
        self.failed.push(label.to_string());
        println!("FAIL  {label}: {detail}");
    }

    /// A lane that returned an error or panicked after its section began:
    /// one FAIL, and the next section gets the node back first.
    pub fn stopped_early(&mut self, why: &str) {
        let section = self.ran.last().cloned().unwrap_or_default();
        self.fail(&format!("{section} stopped early"), why);
        self.recovery_due = true;
    }

    /// The node as the lanes expect it, after a lane that stopped early:
    /// started again (so no test lever that lane pulled, the registry held
    /// down among them, outlives it), fragments on their own hosts, the
    /// platform on 127.0.0.1, no extra settings, and no model answers left
    /// scripted.
    fn recover(&mut self) -> Result<()> {
        if self.hosted() {
            // the deployment is not the run's to restart; a hosted lane pulls
            // levers on its own fragments and people alone
            return Ok(());
        }
        self.ai.clear_script();
        self.shape = Shape::Plain;
        self.byoc = true;
        // a node a lane left down is up again
        for n in self.nodes.iter_mut().filter(|n| !n.is_up()) {
            n.up()?;
        }
        if let Some(node) = self.node.take() {
            // one that does not stop in time is killed, and starts all the same
            if let Err(e) = node.stop() {
                println!("      {e:#}");
            }
        }
        self.start(false)?;
        Ok(())
    }

    /// The node's API as the lanes use it: fragments on their own hosts
    /// (hosted: the preview's).
    pub fn api(&self) -> Api {
        match (&self.preview, self.hosted_rules) {
            (Some(preview), _) => Api::hosted(preview, &self.shared),
            // a rehearsal's node is shaped as a branch deployment
            (None, true) => Api::new(self.port, SUFFIX, &self.shared).branch(REHEARSAL_BRANCH),
            (None, false) => Api::new(self.port, SUFFIX, &self.shared),
        }
    }

    /// The shared Chrome, in a context of the lane's own (`None`: no Chrome
    /// is installed). Its pages close when the lease is dropped.
    pub fn browser(&self) -> Result<Option<browser::Lease>> {
        self.chrome.lease()
    }

    /// A label for this run (a fragment's full name adds a suffix:
    /// `named`). Hosted, it is `e2e-<run>-<base>`, so the run's sweep
    /// finds it, and leaves every other run's (hosted/sweep.rs).
    pub fn name(&self, base: &str) -> String {
        match self.hosted_rules {
            true => hosted::sweep::label(&self.run, base),
            false => format!("{base}-{}", self.run),
        }
    }

    /// `base`'s full name for this run, for `owner` (`Api::qualified`).
    pub fn named(&self, api: &Api, owner: &Keys, base: &str) -> Result<String> {
        api.qualified(owner, &self.name(base))
    }

    /// Starts the node as fragment.club is shaped (`Shape::TwoSites`): a
    /// browser treats the platform and the fragments as two sites.
    pub fn start_as_browsers_see_it(&mut self) -> Result<Api> {
        self.start_shaped(Shape::TwoSites)
    }

    /// Starts the node with its model route shaped as Finite's own
    /// deployment (`Shape::FinitePrivate`): its tiers on Finite Private.
    pub fn start_as_finite(&mut self) -> Result<Api> {
        self.shape = Shape::FinitePrivate;
        let started = self.start(false);
        self.shape = Shape::Plain;
        started
    }

    /// Starts the node with no Stripe (`Shape::SellsNothing`): billing off.
    pub fn start_selling_nothing(&mut self) -> Result<Api> {
        self.shape = Shape::SellsNothing;
        let started = self.start(false);
        self.shape = Shape::Plain;
        started
    }

    /// Starts the node with one operator key more in its catalog
    /// (`Shape::OneKeyMore`).
    pub fn start_with_a_key_more(&mut self) -> Result<Api> {
        self.shape = Shape::OneKeyMore;
        let started = self.start(false);
        self.shape = Shape::Plain;
        started
    }

    /// Starts the node with its model route on a model of its own, at no
    /// price (`Shape::SelfHosted`).
    pub fn start_self_hosted(&mut self) -> Result<Api> {
        self.shape = Shape::SelfHosted;
        let started = self.start(false);
        self.shape = Shape::Plain;
        started
    }

    /// Starts the node with no mailer (`Shape::NoMailer`).
    pub fn start_without_mail(&mut self) -> Result<Api> {
        self.shape = Shape::NoMailer;
        let started = self.start(false);
        self.shape = Shape::Plain;
        started
    }

    /// This node boot's log, as written so far (`None`: no node runs).
    pub fn node_log(&self) -> Option<String> {
        self.node.as_ref().map(|n| std::fs::read_to_string(&n.log).unwrap_or_default())
    }

    fn start_shaped(&mut self, shape: Shape) -> Result<Api> {
        self.shape = shape;
        let started = self.start(false);
        self.shape = Shape::Plain;
        let mut api = started?;
        api.base = format!("http://{SUFFIX}:{}", self.port);
        Ok(api)
    }

    /// Starts the node, fragments on their own hosts.
    pub fn start(&mut self, clean: bool) -> Result<Api> {
        anyhow::ensure!(self.preview.is_none(), "a hosted run starts no node: a section that does declares Need::Node");
        assert!(self.node.is_none(), "one node at a time");
        let tools = self.tools.as_ref().context("a local run locates its tools")?;
        if clean {
            // before the fleet seeds the local store in it again
            devstack::clear_state(&self.project)?;
        }
        let fleet = devstack::Fleet {
            host_secret: self.host_secret.clone(),
            codestorage_org: self.store.as_ref().map_or(ORG, |x| x.org.as_str()).into(),
            codestorage_key_pem: self.org_key.clone(),
            codestorage_url: self.store_url(),
            host_suffix: self.suffix().to_string(),
            // a rehearsal's node is shaped as a branch deployment, whose
            // levers are scoped as a preview's are
            host_label_suffix: self.hosted_rules.then(|| format!("--{REHEARSAL_BRANCH}")),
            poll_interval_s: POLL_S,
            egress_local: true,
            job_retry_delay_s: 1,
            blob_grace_s: Some(BLOB_GRACE_S),
            // the lower rung: the model route's calls go to the Workers AI fake,
            // and, shaped as Finite's own, its tiers to the fake's
            // OpenAI-compatible face as Finite Private (lanes/upstream.rs)
            ai_url: Some(self.ai.node().url.clone()),
            ai_gateway: None,
            models: match self.shape {
                Shape::FinitePrivate => Some(finite_private_models(&self.ai.node().url).to_string()),
                Shape::SelfHosted => Some(self_hosted_models(&self.ai.node().url).to_string()),
                _ => self.own_models.as_ref().map(|m| m.cell.clone()),
            },
            model_key_slots: match self.shape {
                Shape::FinitePrivate => vec!["cheap".into(), "medium".into()],
                Shape::SelfHosted => vec!["cheap".into(), "medium".into(), "fallback".into()],
                _ => self.own_models.as_ref().map(|m| m.slots.clone()).unwrap_or_default(),
            },
            model_key_value: match self.shape {
                Shape::FinitePrivate | Shape::SelfHosted => Some(FINITE_PRIVATE_KEY.to_string()),
                _ => self.own_models.as_ref().and_then(|m| m.key.clone()),
            },
            mail_url: (self.shape != Shape::NoMailer).then(|| self.mail.node().url.clone()),
            mail_from: (self.shape != Shape::NoMailer).then(|| MAIL_FROM.into()),
            default_plan: Some(DEFAULT_PLAN.into()),
            delivery_retry_s: Some(1),
            workos: Some(devstack::WorkOsVars {
                client_id: self.workos.node().client_id.clone(),
                api_key: WORKOS_KEY.into(),
                api_url: Some(self.workos.node().url.clone()),
            }),
            // who signs people in: the OpenID Connect fake when the run says so
            oidc: self.oidc_signin().then(|| devstack::OidcVars {
                issuer: self.oidc.node().url.clone(),
                client_id: OIDC_CLIENT.into(),
                client_secret: Some(OIDC_SECRET.into()),
                scopes: None,
                claims: None,
                auth: None,
            }),
            stripe: (self.shape != Shape::SellsNothing).then(|| devstack::StripeVars {
                key: STRIPE_KEY.into(),
                webhook_secret: STRIPE_WEBHOOK.into(),
                api_url: Some(self.stripe.node().url.clone()),
                portal: Some(STRIPE_PORTAL.into()),
                tax: true,
            }),
            platform_url: match self.shape {
                Shape::TwoSites => format!("http://{SUFFIX}:{}", self.port),
                Shape::Plain | Shape::FinitePrivate | Shape::SellsNothing | Shape::OneKeyMore | Shape::NoMailer | Shape::SelfHosted => format!("http://127.0.0.1:{}", self.port),
            },
            operators: Some(
                std::iter::once(&self.operator).chain(self.wiper.as_ref()).map(|k| fragment_core::npub::encode(k.pubkey_hex())).collect::<Vec<_>>().join(","),
            ),
            signins_pending_max: Some(SIGNINS_PENDING_MAX),
            cli_begins_per_ip_minute: Some(CLI_BEGINS_PER_IP_MINUTE),
            // the levers, as a preview has them: each request carries the secret
            test_secret: Some(self.test_secret.clone()),
            // celld runs no containers of its own: computers there only on
            // the run's sandcastle nodes (seam 2)
            computer_image: self.has_computers().then(|| "stub".into()),
            computer_snapshots: false,
            computer_unsaved_max_ms: Some(UNSAVED_MAX_MS),
            providers: Some({
                let mut rows = swap_providers(Some(self.openrouter.node().oauth()))?;
                rows.as_array_mut().expect("catalog").push(json!({ "name": "e2e-oauth", "kind": "connection", "hosts": ["oauth.test"], "placements": [{ "header": "authorization", "format": "Bearer {}" }], "env": ["E2E_OAUTH_TOKEN"], "connection_oauth": self.workos.node().connection_oauth() }));
                if self.shape == Shape::OneKeyMore {
                    rows.as_array_mut().expect("catalog").push(added_key_row());
                }
                rows.to_string()
            }),
            operator_key_values: SWAP_KEYS.iter().chain((self.shape == Shape::OneKeyMore).then_some(&ADDED_KEY)).map(|(name, value, _)| (name.to_string(), value.to_string())).collect(),
            connection_secret_values: vec![("e2e-oauth".into(), "mock-connection-secret".into())],
            swap_upstream: Some(self.upstream.node().url.clone()),
            // wrangler's local store in the node's own state (seeded once a
            // state, bound by name as a deploy binds them); celld has none,
            // and the cell's stand-ins read OpenBao or .dev.vars (seam 12)
            secrets: match (&self.celld, &self.openbao) {
                (None, _) => devstack::Secrets::Store,
                (Some(_), None) => devstack::Secrets::Shim,
                (Some(_), Some(bao)) => devstack::Secrets::OpenBao(bao.target()),
            },
            browser_url: self.renderer.as_ref().map(|r| r.url.clone()),
            nodes: match self.nodes.is_empty() {
                true => None,
                false => Some(devstack::sandcastle::vars(&self.nodes.iter().collect::<Vec<_>>(), &self.node_images)?),
            },
            byoc: self.byoc && !self.nodes.is_empty(),
        };
        fleet.configure(tools, &self.project)?;
        let opts = devstack::NodeOptions {
            project: self.project.clone(),
            port: self.port,
            // each boot's log, in this run's scratch: a FAIL comes with the
            // node's side of it, the logs of a node a lane killed included
            log_dir: self.scratch.clone(),
            node_logs: true,
            // a crash kills wrangler and workerd as one
            own_group: true,
        };
        let (node, _) = match &self.celld {
            None => devstack::Node::start(tools, &opts)?,
            Some(celld) => devstack::celld::start(celld, &opts, None)?,
        };
        self.node = Some(node);
        // Stripe's events go to the platform's host, as its endpoint would
        self.stripe.node().set_endpoint(&format!("{}/api/stripe/webhook", fleet.platform_url), STRIPE_WEBHOOK);
        Ok(Api::new(self.port, self.suffix(), &self.shared))
    }

    /// The fragments' suffix in the next node's shape.
    fn suffix(&self) -> &'static str {
        match self.shape {
            Shape::TwoSites => BOATS,
            Shape::Plain | Shape::FinitePrivate | Shape::SellsNothing | Shape::OneKeyMore | Shape::NoMailer | Shape::SelfHosted => SUFFIX,
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        anyhow::ensure!(self.preview.is_none(), "a hosted run stops no node: a section that does declares Need::Node");
        self.node.take().expect("a running node").stop()
    }

    pub fn crash(&mut self) -> Result<()> {
        anyhow::ensure!(!self.hosted_rules, "the hosted lane crashes no node: a section that does declares Need::Node");
        self.node.take().expect("a running node").crash()
    }

    /// Removes the containers the run's nodes left (devstack::containers),
    /// once, its node stopped: none on a hosted run, which starts no node.
    fn remove_containers(&mut self) -> Result<Option<devstack::containers::Removed>> {
        if self.tools.is_none() || self.containers_removed {
            return Ok(None);
        }
        assert!(self.node.is_none(), "the run's node is stopped before its containers go");
        self.containers_removed = true;
        devstack::containers::remove(&self.project).map(Some)
    }

    /// Records who owns a fragment made some other way than `create` (the
    /// CLI, the shell), so `commit` and `deploy` act as them.
    pub fn owned(&self, created: &Value, keys: &Keys) {
        let name = created["name"].as_str().expect("created.name");
        self.owners.borrow_mut().insert(name.to_string(), keys.clone());
    }

    /// Creates a fragment signed by `keys`, its owner.
    pub fn create(&self, api: &Api, keys: &Keys, name: &str) -> Result<Value> {
        let r = api.create(keys, name)?;
        if r.status != 200 {
            bail!("create {name}: {r}");
        }
        self.owned(&r.body, keys);
        Ok(r.body)
    }

    /// A commit on main by another writer: a push through the fake, then
    /// its owner's `refresh`, as the CLI pushes and refreshes. Where the
    /// fake is not the store (hosted, or a store outside the run), its
    /// owner's commit through the files route (`POST /api/f/{name}/files`:
    /// one commit, main moved at once), since the git is real.
    pub fn commit(&self, created: &Value, changes: &[(&str, Option<&[u8]>)]) -> String {
        if self.fake_is_the_store() {
            let sha = self.fake.external_commit(created["repo"].as_str().expect("created.repo"), "main", changes, "e2e commit");
            self.refresh(created);
            return sha;
        }
        let files: Vec<Value> = changes
            .iter()
            .map(|(path, bytes)| match bytes.map(std::str::from_utf8) {
                None => json!({ "path": path, "delete": true }),
                Some(Ok(text)) => json!({ "path": path, "text": text }),
                Some(Err(_)) => json!({ "path": path, "base64": base64::engine::general_purpose::STANDARD.encode(bytes.unwrap_or_default()) }),
            })
            .collect();
        let r = self.as_owner(created, "files", &json!({ "files": files, "message": "e2e commit" }));
        r["commit"].as_str().unwrap_or_else(|| panic!("a commit answers its sha: {r}")).to_string()
    }

    /// Moves live to main's tip through the fake, then refreshes (a ref
    /// move as `fragment rollback` makes one). Where the fake is not the
    /// store, its owner's deploy (`POST /api/f/{name}/deploy`), which
    /// installs the app at once.
    pub fn deploy(&self, created: &Value) -> String {
        if self.fake_is_the_store() {
            let repo = created["repo"].as_str().expect("created.repo");
            let tip = self.fake.branch(repo, "main").expect("main has a commit");
            self.fake.set_branch(repo, "live", &tip);
            self.refresh(created);
            return tip;
        }
        let r = self.as_owner(created, "deploy", &json!({ "note": "e2e deploy" }));
        r["live"].as_str().unwrap_or_else(|| panic!("a deploy answers live: {r}")).to_string()
    }

    /// Whether the node's git is the code.storage fake in this process,
    /// whose levers and state the lanes may reach: a local run's, unless
    /// `FRAGMENT_E2E_CODESTORE` put it outside (store.rs).
    pub fn fake_is_the_store(&self) -> bool {
        !self.hosted() && self.store.is_none()
    }

    /// The store outside the run (`None`: the fake is the store, or a
    /// hosted run's vendor, which the run holds no key to).
    pub fn external_store(&self) -> Option<&store::External> {
        self.store.as_ref()
    }

    /// The code store's API base, as the node and the CLI reach it.
    pub fn store_url(&self) -> String {
        match &self.store {
            Some(x) => x.url.clone(),
            None => self.fake.node().url.clone(),
        }
    }

    /// The url form of the repo named `name`, as the store lists it.
    pub fn repo_url(&self, name: &str) -> Option<String> {
        match &self.store {
            None => self.fake.repo_url(name),
            Some(x) => x.repo_url(name).unwrap_or_else(|e| panic!("{e:#}")),
        }
    }

    /// A branch's head in `repo` (its url form), as the store holds it.
    pub fn head(&self, repo: &str, branch: &str) -> Option<String> {
        match &self.store {
            None => self.fake.branch(repo, branch),
            Some(x) => x.head(repo, branch).unwrap_or_else(|e| panic!("{e:#}")),
        }
    }

    /// `n` empty repos made after every existing one: a busy org, so a
    /// lookup by name must page. On a store outside the run, made as the
    /// cell makes one, named for the run.
    pub fn seed_filler(&self, n: usize) {
        let Some(x) = &self.store else { return self.fake.seed_filler(n) };
        for i in 0..n {
            x.create_repo(&format!("filler-{}-{i}", self.run)).unwrap_or_else(|e| panic!("{e:#}"));
        }
    }

    /// A repo named `name` (its url form the name, as the service's is)
    /// holding `files` on main: one a fragment made before repos were named
    /// for their owners.
    pub fn seed_repo(&self, name: &str, files: &[(&str, Option<&[u8]>)]) {
        let Some(x) = &self.store else {
            let files: Vec<(&str, &[u8])> = files.iter().filter_map(|(p, b)| b.map(|b| (*p, b))).collect();
            return self.fake.seed_repo(name, &files);
        };
        let url = x.create_repo(name).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(url, name, "the store's url form is the repo's name");
        x.commit(&url, "main", files, "seeded").unwrap_or_else(|e| panic!("{e:#}"));
    }

    /// Whether the store says `repo` is deleted: on a store outside the
    /// run, its org no longer lists it, and a delete again reads as the
    /// cell reads one gone before (`core_cs::repo_deleted`; one still
    /// there would be deleted now, and is no pass).
    pub fn repo_deleted(&self, repo: &str) -> bool {
        use fragment_core::codestorage::{repo_deleted, RepoDeleted};
        match &self.store {
            None => self.fake.repo_deleted(repo),
            Some(x) => x.lists(repo).is_ok_and(|listed| !listed) && x.delete_repo(repo).is_ok_and(|a| repo_deleted(a.status, &a.body) == RepoDeleted::Already),
        }
    }

    /// A commit on main by another writer that no one refreshes: only the
    /// owner's `refresh` or the poll finds it.
    pub fn unrefreshed_commit(&self, created: &Value, changes: &[(&str, Option<&[u8]>)]) -> String {
        let repo = created["repo"].as_str().expect("created.repo");
        match &self.store {
            None => self.fake.external_commit(repo, "main", changes, "unannounced"),
            Some(x) => x.commit(repo, "main", changes, "unannounced").unwrap_or_else(|e| panic!("a commit on {repo}: {e:#}")),
        }
    }

    /// A check that pulls the code.storage fake's `lever`, where the fake
    /// is not the store: a skip that says which.
    pub fn skip_lever(&mut self, label: &str, lever: &str) {
        let store = self.store.as_ref().map_or("a preview's".to_string(), |x| x.label.clone());
        self.skip(label, &format!("it pulls the code.storage fake's {lever}, and the node's git is {store} ({}; docs/self-host.md, seam 5)", store::CODESTORE_VAR));
    }

    /// The owner's `refresh` after a move through the fake; one that fails
    /// (a code.storage outage a test makes) is the test's to see, as the
    /// CLI only warns.
    fn refresh(&self, created: &Value) {
        let _ = self.call_as_owner(created, "refresh", &json!({}));
    }

    /// `POST /api/f/{name}/{route}`, signed by the owner `create` or
    /// `owned` recorded.
    fn call_as_owner(&self, created: &Value, route: &str, body: &Value) -> Result<api::Reply> {
        let name = created["name"].as_str().expect("created.name");
        let owner = self.owners.borrow().get(name).cloned().unwrap_or_else(|| panic!("{name}'s owner is recorded by Suite::create or Suite::owned"));
        self.api().signed(&owner, "POST", &format!("/api/f/{name}/{route}"), Some(body))
    }

    /// A hosted run's `POST /api/f/{name}/{route}` as its owner: a git move
    /// as a local run makes it through the fake.
    fn as_owner(&self, created: &Value, route: &str, body: &Value) -> Value {
        let name = created["name"].as_str().expect("created.name");
        let r = self.call_as_owner(created, route, body).unwrap_or_else(|e| panic!("{route} {name}: {e:#}"));
        assert!(r.status == 200, "{route} {name}: {r}");
        r.body
    }

    /// Polls `f` until it holds or `timeout` passes. One that runs out says
    /// so, with its place: a wait whose limit is its usual length (a check
    /// that something never happens aside) costs every run that limit.
    #[track_caller]
    pub fn eventually(&self, timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
        let at = std::panic::Location::caller();
        let t0 = Instant::now();
        loop {
            if f() {
                return true;
            }
            if t0.elapsed() > timeout {
                println!("      (a wait ran out its {timeout:.0?} at {}:{})", at.file(), at.line());
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Waits until the object behind the fragment name `name` has no
    /// ended life left to clean up (the `ended` lever; cell ended.rs: a
    /// delete answers first, and its alarm tells the members' lists and
    /// deletes the app's database and the blobs after): whether it got
    /// there, and what the lever last answered.
    #[track_caller]
    pub fn ended_cleaned(&self, api: &Api, name: &str) -> (bool, String) {
        let mut last = String::new();
        let cleaned = self.eventually(ENDED_CLEANUP_IN, || match api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "ended" }))) {
            Ok(r) => {
                last = r.to_string();
                r.status == 200 && r.body["ended"].as_array().is_some_and(Vec::is_empty)
            }
            Err(e) => {
                last = format!("{e:#}");
                false
            }
        });
        (cleaned, last)
    }

    /// Runs the CLI with its own HOME against the node.
    pub fn cli(&self, api: &Api, home: &Path, args: &[&str]) -> Output {
        self.cli_command(api, home, args).output().expect("run the fragment CLI")
    }

    /// `cli` with variables of its own (a token, a secret's value), which
    /// never go on its command line.
    pub fn cli_with_env(&self, api: &Api, home: &Path, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut c = self.cli_command(api, home, args);
        c.envs(vars.iter().copied());
        c.output().expect("run the fragment CLI")
    }

    /// The CLI as a lane spawns it itself (a watcher, a piped input): its
    /// config under the HOME the lane sets, on every system.
    pub fn bare_cli(&self) -> Command {
        let mut c = Command::new(&self.cli);
        // a Linux runner's XDG_CONFIG_HOME would put every lane's key in one place
        c.env_remove("XDG_CONFIG_HOME");
        c
    }

    fn cli_command(&self, api: &Api, home: &Path, args: &[&str]) -> Command {
        let mut c = self.bare_cli();
        c.args(args).env("HOME", home).env("FRAGMENT_HOST", &api.base).env_remove("FRAGMENT_OUTPUT");
        c
    }

    /// `fragment login` in `home`, with a person approving its key in a
    /// browser: the CLI's pending login, a sign-in through the WorkOS fake,
    /// the approval, then the CLI's login finishing.
    /// A login that fails is a FAIL of its own, so the lane's checks that
    /// fail after it have their cause printed first.
    pub fn login(&mut self, api: &Api, home: &Path) -> Output {
        let args = ["login", "--no-wait", "--json"];
        let pending = self.cli(api, home, &args);
        if let Err(e) = cli_data(&args, &pending).and_then(|p| approve_login(api, &p)) {
            // `fragment login` would wait ten minutes for an approval that is not coming
            self.fail("fragment login: a person approves its key in a browser", format!("{e:#}"));
            return pending;
        }
        let out = self.cli(api, home, &["login", "--no-browser"]);
        if !out.status.success() {
            self.fail("fragment login", String::from_utf8_lossy(&out.stderr).to_string());
        }
        out
    }

    /// The `data` of a `--json` CLI answer.
    pub fn cli_json(&self, api: &Api, home: &Path, args: &[&str]) -> Result<Value> {
        cli_data(args, &self.cli(api, home, args))
    }

    /// The key a CLI home logged in with (its config file, per platform).
    pub fn cli_keys(&self, home: &Path) -> Option<Keys> {
        ["Library/Application Support/fragment/config.json", ".config/fragment/config.json"]
            .iter()
            .find_map(|p| std::fs::read_to_string(home.join(p)).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["secret_key"].as_str().and_then(Keys::from_secret_hex))
    }

    /// A fresh directory in this run's scratch.
    pub fn dir(&self, name: &str) -> PathBuf {
        let d = self.scratch.join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }
}

/// The `data` of a `--json` CLI answer (`args`, its command, for errors).
fn cli_data(args: &[&str], out: &Output) -> Result<Value> {
    let v: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("fragment {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))?;
    if v["ok"] != true {
        bail!("fragment {args:?}: {v}");
    }
    Ok(v["data"].clone())
}

/// Whom `Suite::login` signs in as for a CLI key (its npub): the same
/// person again when a lane signs in with it.
pub fn cli_email(npub: &str) -> Result<String> {
    Ok(format!("cli-{}@e2e.test", npub.get(5..17).context("an npub is longer than 17 characters")?))
}

/// A person signs in through the WorkOS fake and types the code a pending
/// `fragment login` printed, at the page it names (a key already added has
/// nothing pending).
fn approve_login(api: &Api, pending: &Value) -> Result<()> {
    if pending["pending"] != true {
        return Ok(());
    }
    let npub = pending["npub"].as_str().context("a pending login names its key")?;
    let code = pending["code"].as_str().context("a pending login answers its code")?;
    anyhow::ensure!(pending["url"] == format!("{}/cli", api.base), "a pending login names the platform's /cli, plain: {pending}");
    let session = api.sign_in(&cli_email(npub)?)?;
    let typed = api.type_code(&session, code)?;
    anyhow::ensure!(typed.status == 200, "typing the code: {typed}");
    let added = api.add_code(&session, code)?;
    anyhow::ensure!(added.status == 200, "adding its key: {added}");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = hosted::parse(&args)?;
    match args.hosted {
        None => local(args.only, args.except, LocalRun { rehearse: args.rehearse, shard: args.shard }),
        Some(hosted) => hosted::run(args.only, args.except, hosted),
    }
}

/// The CLI the lanes drive (`FRAGMENT_BIN`, else the release build).
fn cli_binary() -> Result<PathBuf> {
    let cli = std::env::var_os("FRAGMENT_BIN").map(PathBuf::from).unwrap_or_else(|| devstack::repo_root().join("target/release/fragment"));
    if !cli.is_file() {
        bail!("no CLI at {} (cargo build --release -p fragment-cli, or set FRAGMENT_BIN)", cli.display());
    }
    Ok(cli)
}

/// A run's name: distinguishes its fragments from any earlier state, and
/// (hosted) names the run its sweep removes.
fn run_name() -> String {
    hosted::sweep::run_id(api::now_s())
}

/// A local run's settings beside its sections (hosted.rs `Args`).
struct LocalRun {
    /// A rehearsal of the hosted lane, lending at most this many paid calls.
    rehearse: Option<u64>,
    shard: Option<u32>,
}

/// The local run: a fresh `wrangler dev` node and the fakes. A rehearsal
/// (`rehearse`: the paid calls it may lend) keeps the hosted lane's rules
/// on it: the node shaped as a branch deployment (its levers scoped as a
/// preview's), people signed in through the levers, git moved through the
/// API, the fakes hidden from the lanes, and each section run or skipped by
/// what it needs, as on a preview; it ends with the sweep. A shard runs the
/// sections the table gives it, in the lanes' order.
fn local(only: Option<Vec<String>>, except: Vec<String>, settings: LocalRun) -> Result<()> {
    let LocalRun { rehearse, shard } = settings;
    let root = devstack::repo_root();
    let cli = cli_binary()?;
    let tools = devstack::Tools::locate()?;
    let run = run_name();
    let scratch = root.join("target/e2e").join(&run);
    std::fs::create_dir_all(&scratch)?;
    let hidden = rehearse.is_some();
    let codestore = codestore(hidden, &scratch)?;
    println!("code store: {}", codestore.said);
    let project = devstack::stage_project(&scratch.join("cell"))?;
    // where computers run: the runtime's containers, or sandcastle nodes
    // (docs/self-host.md, seam 2): two in front of sandcastle's Docker engine
    // double, or one in front of this machine's real engine
    let sandcastle = match std::env::var("FRAGMENT_E2E_NODES").as_deref() {
        Err(_) => None,
        Ok("two") if rehearse.is_none() => Some((devstack::sandcastle::Tools::locate(false)?, None)),
        Ok("real") if rehearse.is_none() => {
            let dir = std::env::var_os("SANDCASTLE_ENGINE_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/lib/sandcastle"));
            Some((devstack::sandcastle::Tools::locate(true)?, Some(dir)))
        }
        Ok("two" | "real") => bail!("FRAGMENT_E2E_NODES is a local run's: a rehearsal keeps a preview's rules"),
        Ok(other) => bail!("FRAGMENT_E2E_NODES is two (one node that listens, one that dials in) or real (one node in front of the machine's engine), not {other}"),
    };
    // Ctrl-C (or SIGTERM, SIGHUP) mid-run, as a run's end: the node runs in
    // a process group of its own, which the terminal's signal does not
    // reach, so the run kills it, then removes the containers it left, stops
    // the rest it started (OpenBao, the sandcastle nodes, whose engine
    // doubles remove theirs, Chrome), and destroys its containers on a real
    // engine (those its node labelled with the run's name)
    let leftovers = project.clone();
    let real_engine = sandcastle.as_ref().and_then(|(_, dir)| dir.as_ref()).map(|dir| (dir.join("engine.sock"), run.clone()));
    devstack::signals::on_termination(move |signal| {
        println!("\nsignal {signal}: the run stops what it started and removes its containers");
        devstack::kill_nodes();
        match devstack::containers::remove(&leftovers) {
            Ok(r) => println!("      (removed {} containers of this run)", r.containers),
            Err(e) => println!("      could not remove this run's containers: {e:#}"),
        }
        devstack::stop_children();
        if let Some((sock, run)) = &real_engine {
            if let Err(e) = devstack::sandcastle::destroy_run(sock, run) {
                println!("      could not destroy this run's containers on the real engine: {e:#}");
            }
        }
    })?;
    // a real model for its agents (docs/self-host.md, seam 3): agent-app alone
    let own_models = own_models()?;
    if let Some(m) = &own_models {
        anyhow::ensure!(rehearse.is_none(), "a rehearsal's agents answer through the scripted model: drop FRAGMENT_E2E_MODELS_FILE");
        anyhow::ensure!(
            only.as_deref() == Some(&[lanes::AGENT_APP.to_string()][..]),
            "a run on a model of its own (FRAGMENT_E2E_MODELS_FILE) runs agent-app alone (--only agent-app): every other section scripts the model"
        );
        println!("models: {}", m.said);
    }
    if only.as_ref().is_some_and(|o| o.iter().any(|n| n == lanes::hermes::SECTION || n == lanes::AGENT_APP)) {
        lanes::hermes::stage_images(&project)?;
    }
    let test_secret = devstack::random_hex(32);
    // the runtime: wrangler dev's workerd, or celld (docs/self-host.md, seam 1)
    let celld = match std::env::var("FRAGMENT_E2E_RUNTIME").as_deref() {
        Err(_) | Ok("wrangler") => None,
        Ok("celld") if rehearse.is_some() => bail!("a rehearsal keeps a preview's rules on wrangler dev: drop FRAGMENT_E2E_RUNTIME=celld"),
        Ok("celld") => Some(devstack::celld::CelldTools::locate()?),
        Ok(other) => bail!("FRAGMENT_E2E_RUNTIME is wrangler or celld, not {other}"),
    };
    if let Some(c) = &celld {
        println!("runtime: celld dev ({})", c.celld.display());
    }
    // who signs people in: the WorkOS fake, or the OpenID Connect fake (docs/self-host.md, seam 4)
    match std::env::var("FRAGMENT_E2E_SIGNIN").as_deref() {
        Err(_) | Ok("workos") => {}
        Ok("oidc") if rehearse.is_some() => bail!("a rehearsal signs people in through the levers: drop FRAGMENT_E2E_SIGNIN=oidc"),
        Ok("oidc") => println!("sign-in: the OpenID Connect fake (WorkOS's serves Pipes alone)"),
        Ok(other) => bail!("FRAGMENT_E2E_SIGNIN is workos or oidc, not {other}"),
    }
    // the cell's secrets on celld: OpenBao behind its stand-ins, or their variables
    let openbao = match (std::env::var("FRAGMENT_E2E_SECRETS").ok().as_deref(), &celld) {
        (None | Some("openbao"), Some(_)) => Some(openbao(&scratch)?),
        (Some("vars"), Some(_)) | (None, None) => None,
        (Some(_), None) => bail!("FRAGMENT_E2E_SECRETS is celld's (FRAGMENT_E2E_RUNTIME=celld): wrangler dev keeps the secrets in its own store"),
        (Some(other), Some(_)) => bail!("FRAGMENT_E2E_SECRETS is openbao or vars, not {other}"),
    };
    let nodes = sandcastle.is_some();
    // a local run's people sign in through the WorkOS fake, and pay the fake
    // model; a rehearsal's, through the levers, lent paid calls as on a preview
    let (rung, shared) = match rehearse {
        None if celld.is_some() => (needs::Rung::Celld { nodes, real_agent: own_models.is_some() }, api::Run::new(test_secret.clone(), 0)),
        None => (needs::Rung::Local { nodes, real_agent: own_models.is_some() }, api::Run::new(test_secret.clone(), 0)),
        Some(paid_calls) => {
            // its computers answer through the scripted model: no real agent
            let offers = needs::Offers { levers: true, computers: true, models: paid_calls > 0, chrome: browser::chrome().is_some(), real_agent: false, operator: true };
            (needs::Rung::Hosted(offers), api::Run::signing_in_by_levers(test_secret.clone(), paid_calls))
        }
    };
    let openrouter = fragment_fakes::openrouter::OpenRouter::start()?;
    let port = devstack::free_port()?;
    let renderer = match &celld {
        Some(_) => Some(renderer(&scratch, port)?),
        None => None,
    };
    let mut s = Suite {
        only,
        except,
        shard,
        rung,
        hosted_rules: rehearse.is_some(),
        preview: None,
        candidate: None,
        plan: None,
        shared,
        owners: RefCell::new(BTreeMap::new()),
        asked: vec![],
        passed: 0,
        failed: vec![],
        skipped: vec![],
        ran: vec![],
        recovery_due: false,
        lost: None,
        cron: None,
        chrome: browser::Shared::new(&scratch),
        tools: Some(tools),
        celld,
        openbao,
        renderer,
        node: None,
        nodes: vec![],
        node_images: Value::Null,
        sandcastle: sandcastle.as_ref().map(|(tools, _)| tools.clone()),
        byoc: true,
        image_tags: vec![],
        own_models,
        port,
        run,
        fake: codestore.fake,
        store: codestore.store,
        macrofiche: codestore.macrofiche,
        ai: Fake::of(hidden, "Workers AI", fragment_fakes::workers_ai::WorkersAi::start(0)?),
        push: Fake::of(hidden, "push service", fragment_fakes::push::PushService::start()?),
        mail: Fake::of(hidden, "mail", fragment_fakes::mail::Mailer::start(0, false)?),
        stripe: Fake::of(hidden, "Stripe", fragment_fakes::stripe::Stripe::start(0, STRIPE_KEY)?),
        org_key: codestore.org_key,
        host_secret: devstack::random_hex(32),
        test_secret,
        workos: Fake::of(hidden, "WorkOS", fragment_fakes::workos::WorkOs::start(WORKOS_CLIENT, WORKOS_KEY)?),
        oidc: Fake::of(hidden, "OpenID Connect", fragment_fakes::oidc::Oidc::start(OIDC_CLIENT, OIDC_SECRET)?),
        upstream: Fake::of(hidden, "upstream", fragment_fakes::upstream::Upstream::start_with(vec![(SWAP_ROUTER_HOST.into(), openrouter.handler())])?),
        openrouter: Fake::of(hidden, "OpenRouter", openrouter),
        operator: Keys::generate(),
        wiper: Some(Keys::generate()),
        cli,
        scratch,
        project,
        shape: Shape::Plain,
        containers_removed: false,
    };
    if let Some(k) = s.shard {
        println!("shard {k}/{}: {}", lanes::SHARDS.len(), lanes::SHARDS[k as usize - 1].join(", "));
    }
    if let Some((tools, real)) = &sandcastle {
        start_nodes(&mut s, tools, real.as_deref())?;
    }
    let t0 = Instant::now();
    s.start(true)?;
    // wrangler builds the computer images as it boots: built ahead (xtask's
    // build), every step is a cache hit, and the boot takes seconds
    let log = s.node.as_ref().map(|n| std::fs::read_to_string(&n.log).unwrap_or_default()).unwrap_or_default();
    let cached = log.lines().filter(|l| l.starts_with('#') && l.ends_with(" CACHED")).count();
    println!("the node is ready in {:.1?} (its image builds: {cached} steps cached)", t0.elapsed());
    lanes::run(&mut s);
    if rehearse.is_some() {
        lanes::run_one(&mut s, |s, _| {
            hosted::rehearse_sweep(s);
            Ok(())
        });
    }
    finish(&mut s)
}

/// The renderer a celld node shoots preview cards with: the pinned
/// chrome-headless-shell (fetched into `target/tools` on first use, or
/// `FRAGMENT_BROWSER_ZIP`), its pages let through to the run's fragments'
/// hosts (both sites' suffixes) on the node's port, and to nothing else.
fn renderer(scratch: &Path, port: u16) -> Result<devstack::rendering::Rendering> {
    let browser = devstack::browser::locate(&devstack::repo_root().join(devstack::TOOLS_DIR))?;
    let upstream = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let origins = devstack::rendering::Origins::new(&[SUFFIX, BOATS], port, upstream);
    let opts = devstack::rendering::Options { browser: browser.bin, state: scratch.join("renderer"), origins, ca_file: None, port: 0 };
    let r = devstack::rendering::Rendering::start(opts)?;
    println!("cards: {} (the renderer at {}; its pages reach this run's fragments alone)", browser.version, r.url);
    Ok(r)
}

/// A local run's code store, as `codestore` started or found it.
struct RunStore {
    fake: Fake<CodeStorage>,
    store: Option<store::External>,
    macrofiche: Option<devstack::codestore::Macrofiche>,
    org_key: String,
    /// What the run's first lines say it is.
    said: String,
}

/// The node's code store (`FRAGMENT_E2E_CODESTORE`, store.rs): the fake in
/// this process (`hidden` from the lanes on a rehearsal), macrofiche
/// started on the run's scratch with an org key made for the run (its log
/// beside the node's), or a store already running.
fn codestore(hidden: bool, scratch: &Path) -> Result<RunStore> {
    let choice = store::Choice::from_env()?;
    anyhow::ensure!(!hidden || choice == store::Choice::Fake, "a rehearsal keeps a preview's rules on the fakes: drop {}", store::CODESTORE_VAR);
    let (x, macrofiche, org_key) = match choice {
        store::Choice::Fake => {
            let org_key = fake::generate_org_key_pem();
            let fake = CodeStorage::start(fake::Options { org: ORG.into(), org_key_pem: Some(org_key.clone()), ..Default::default() })?;
            let said = format!("{} (the code.storage fake, in this process)", fake.url);
            return Ok(RunStore { fake: Fake::of(hidden, "code.storage", fake), store: None, macrofiche: None, org_key, said });
        }
        store::Choice::External => {
            let x = devstack::codestore::ExternalStore::from_env()?;
            let label = format!("the store at {}", x.url);
            (store::External::new(&x.url, &x.org, &x.key_pem, label)?, None, x.key_pem)
        }
        store::Choice::Macrofiche => {
            let bin = devstack::codestore::macrofiche_bin()?;
            let org_key = fake::generate_org_key_pem();
            let opts = devstack::codestore::MacroficheOptions { dir: scratch.join("macrofiche"), port: devstack::free_port()?, org: ORG.into(), key_pem: org_key.clone(), log_dir: scratch.to_path_buf() };
            let m = devstack::codestore::Macrofiche::start(&bin, &opts)?;
            let label = format!("macrofiche ({}; its log {})", bin.display(), m.log.display());
            (store::External::new(&m.url, ORG, &org_key, label)?, Some(m), org_key)
        }
    };
    let said = format!("{}, org {}: {}", x.url, x.org, x.label);
    Ok(RunStore { fake: Fake::absent("code.storage"), store: Some(x), macrofiche, org_key, said })
}

/// The run's two sandcastle nodes (`FRAGMENT_E2E_NODES=two`), in the
/// order `FRAGMENT_NODES` lists them: `direct`, which listens, and
/// `uplink`, which dials the platform; each holds the staged cell's images
/// (the Hermes images among them when its lane runs: `stage_images`),
/// built here for the run and tagged with its name. With a real engine
/// (`FRAGMENT_E2E_NODES=real`), one node, `box`, which listens in front of
/// it, the images loaded into it and named by their digests.
fn start_nodes(s: &mut Suite, tools: &devstack::sandcastle::Tools, real: Option<&Path>) -> Result<()> {
    let t0 = Instant::now();
    let built = devstack::sandcastle::build_cell_images(&s.project, &s.run)?;
    s.image_tags.extend(built.iter().map(|(_, tag)| tag.clone()));
    println!("      (the nodes' images built in {:.1?})", t0.elapsed());
    let platform = s.platform();
    if let Some(dir) = real {
        let pinned = devstack::sandcastle::load_images(dir, &built, &s.scratch.join("images"))?;
        s.node_images = Value::Object(pinned.into_iter().map(|(name, reference)| (name, json!(reference))).collect());
        let spec = devstack::sandcastle::NodeSpec {
            id: "box".into(),
            reach: devstack::sandcastle::Reach::Listen(devstack::free_port()?),
            engine: devstack::sandcastle::Engine::Real(dir.to_path_buf()),
            dir: s.scratch.join("n").join("box"),
            platform,
            capacity: devstack::sandcastle::real_engine_vms(dir)?,
            log_dir: s.scratch.clone(),
            run: s.run.clone(),
        };
        s.nodes.push(devstack::sandcastle::SandcastleNode::start(tools, &spec)?);
        println!("nodes: box at {}, in front of the real engine in {}, its images {} loaded (ready in {:.1?})", s.nodes[0].url().unwrap_or_default(), dir.display(), s.node_images, t0.elapsed());
        return Ok(());
    }
    s.node_images = Value::Object(built.iter().map(|(name, tag)| (name.clone(), json!(format!("docker.io/library/{tag}")))).collect());
    for (id, reach) in [("direct", devstack::sandcastle::Reach::Listen(devstack::free_port()?)), ("uplink", devstack::sandcastle::Reach::Uplink)] {
        let spec = devstack::sandcastle::NodeSpec {
            id: id.into(),
            reach,
            engine: devstack::sandcastle::Engine::Double,
            // short: the engine's sockets live under it (`sandcastle::socket_path`)
            dir: s.scratch.join("n").join(id),
            platform: platform.clone(),
            capacity: NODE_CAPACITY,
            log_dir: s.scratch.clone(),
            run: s.run.clone(),
        };
        s.nodes.push(devstack::sandcastle::SandcastleNode::start(tools, &spec)?);
    }
    println!("nodes: direct at {}, uplink dialing {platform}, their images {} (ready in {:.1?})", s.nodes[0].url().unwrap_or_default(), s.node_images, t0.elapsed());
    Ok(())
}

/// The computers each of the run's nodes holds: room for every section's.
const NODE_CAPACITY: u32 = 64;

/// A run's end: a name `--only` or `--except` gave that no section has
/// fails it; the node stops; the counts, each FAIL, and (hosted) what the
/// run spent are printed. Non-zero when any check failed, the scratch kept.
/// The run's OpenBao (docs/self-host.md, seam 12): the pinned binary, its
/// state in the run's scratch, on a free loopback port, every request in
/// its audit log (read at the run's end: `openbao_reads`).
fn openbao(scratch: &Path) -> Result<devstack::openbao::OpenBao> {
    let bin = devstack::openbao::locate(&devstack::repo_root().join(devstack::TOOLS_DIR))?;
    let opts = devstack::openbao::Options { dir: scratch.join("openbao"), port: devstack::free_port()?, audit: true };
    let (bao, took) = devstack::openbao::OpenBao::start(&bin, opts)?;
    println!("secrets: OpenBao {} at {} (ready in {took:.1?}; every secret behind the cell's stand-ins)", devstack::openbao::OPENBAO_VERSION, bao.target().addr);
    Ok(bao)
}

/// What the run asked of OpenBao, from its audit log: the cell's reads of
/// the secrets, and the log's size.
fn openbao_reads(s: &Suite) {
    let Some(bao) = &s.openbao else { return };
    match devstack::openbao::audited(bao.layout()) {
        Ok(a) => println!("      (OpenBao: {} reads of the secrets by the cell's token, {} requests in all, {} KiB of audit log)", a.cell_reads, a.requests, a.bytes / 1024),
        Err(e) => println!("      (OpenBao's audit log: {e})"),
    }
}

fn finish(s: &mut Suite) -> Result<()> {
    for (flag, name) in s.only.clone().unwrap_or_default().into_iter().map(|n| ("--only", n)).chain(s.except.clone().into_iter().map(|n| ("--except", n))) {
        if !s.asked.contains(&name) {
            s.fail(&format!("{flag} {name}"), "no section has that name");
        }
    }
    // what the run's people spent, from their ledgers, while they answer
    if s.hosted() {
        hosted::spent(s);
    }
    if s.node.is_some() {
        let t0 = Instant::now();
        if let Err(e) = s.stop() {
            s.fail("the node stops at the end of the run", format!("{e:#}"));
        }
        println!("      (the node stopped in {:.1?})", t0.elapsed());
    }
    // the code store the run started, after the node that wrote to it
    if let Some(m) = s.macrofiche.take() {
        if let Err(e) = m.stop() {
            s.fail("macrofiche stops at the end of the run", format!("{e:#}"));
        }
    }
    // its computers' containers and their sidecars, whatever left them
    // (wrangler's teardown removes the containers alone; a crash, neither)
    let t0 = Instant::now();
    match s.remove_containers() {
        Ok(Some(r)) => println!("      (removed {} containers of this run in {:.1?})", r.containers, t0.elapsed()),
        Ok(None) => {}
        Err(e) => s.fail("the run removes the containers its nodes left", format!("{e:#}")),
    }
    // the sandcastle nodes after the platform (each engine double removes
    // its containers; a real engine's the run started are destroyed), then
    // the image tags built for them
    for n in std::mem::take(&mut s.nodes) {
        let id = n.id.clone();
        if let Err(e) = n.stop() {
            s.fail(&format!("the node {id} stops at the end of the run"), format!("{e:#}"));
        }
    }
    for tag in std::mem::take(&mut s.image_tags) {
        let _ = std::process::Command::new("docker").args(["image", "rm", &tag]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let t0 = Instant::now();
    s.chrome.close();
    println!("      (Chrome closed in {:.1?})", t0.elapsed());
    openbao_reads(s);
    if let Some(mut bao) = s.openbao.take() {
        if let Err(e) = bao.down() {
            s.fail("OpenBao stops at the end of the run", format!("{e:#}"));
        }
    }
    let skipped = match s.hosted() || s.on_celld() || s.store.is_some() {
        true => "skipped (each says why)",
        false => "skipped (the hosted lane's)",
    };
    println!("\n{} passed, {} failed, {} {skipped}", s.passed, s.failed.len(), s.skipped.len());
    if !s.failed.is_empty() {
        for f in &s.failed {
            println!("  FAIL {f}");
        }
        match s.hosted() {
            true => println!("kept for a look: {}", s.scratch.display()),
            false => println!("kept for a look: {} (each node boot's log is node-<port>-<boot>.log there)", s.scratch.display()),
        }
        std::process::exit(1);
    }
    if std::env::var_os("FRAGMENT_E2E_KEEP").is_some() {
        println!("kept: {}", s.scratch.display());
    } else if let Err(e) = std::fs::remove_dir_all(&s.scratch) {
        println!("could not remove {}: {e}", s.scratch.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a local run's own models are the cell's as a deploy writes
    /// them, each key's name taken out and its slot named, and a file a
    /// deploy would refuse is refused. Method: the box's Bonsai file's
    /// shape, then each way one goes wrong.
    #[test]
    fn a_runs_own_models_are_read_less_their_keys() {
        let bonsai = json!({ "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "key": "bonsai-key", "model": "bonsai-2-27b",
            "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } }, "stale_timeout_s": 900 });
        let (cell, slots) = models_less_keys(&json!({ "cheap": bonsai, "medium": bonsai, "fallback": bonsai })).expect("Bonsai's file");
        assert_eq!(slots, ["cheap", "fallback", "medium"]);
        assert!(!cell.contains("bonsai-key") && cell.contains("bonsai-2-27b") && cell.contains("stale_timeout_s"), "{cell}");
        let (_, none) = models_less_keys(&json!({ "cheap": { "provider": "workers_ai", "model": fragment_core::models::GLM_FLASH } })).expect("Workers AI names no key");
        assert!(none.is_empty());
        let refused = |m: Value| models_less_keys(&m).map(|_| ()).unwrap_err().to_string();
        assert!(refused(json!([])).contains("an object"));
        assert!(refused(json!({ "cheap": { "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "model": "m" } })).contains("names its key"));
        assert!(refused(json!({ "cheap": { "provider": "workers_ai", "model": "m", "key": "k" } })).contains("only an OpenAI-compatible API"));
        assert!(!refused(json!({ "nonesuch": bonsai })).is_empty(), "a slot the cell has not is refused as the cell refuses it");
    }
}
