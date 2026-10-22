//! `cargo xtask <command>`: the repo's tooling, in Rust.
//!
//!   build            build cell/ for wasm32 (worker-build 0.8.5: build.rs)
//!   dev [--clean] [--runtime wrangler|celld]
//!                    build, then run the stack in the foreground under
//!                    `wrangler dev`: the cell on :8790 (fragments at
//!                    <name>.fragment.localhost:8790), the
//!                    code.storage fake on :8792 (or macrofiche there:
//!                    MACROFICHE_BIN; or a store already running:
//!                    CODESTORAGE_API_URL; `dev_codestore`), and the Workers AI fake on :8796
//!                    behind the model route (its calls spend their payer's
//!                    ledger; new people are seats with the month's included
//!                    credit; FRAGMENT_MODELS_FILE puts the route's slots
//!                    where a deployment's `models` says, a self-hosted
//!                    model's say, its key in FRAGMENT_MODEL_KEY_FILE:
//!                    `dev_models`). --runtime celld runs it on `celld dev`
//!                    (CELLD_BIN; docs/self-host.md, seam 1): no computers,
//!                    cards through the renderer (seam 7: `dev_renderer`),
//!                    the secrets behind the cell's stand-ins for the
//!                    store's bindings, in .dev.vars or, with
//!                    FRAGMENT_SECRETS_BACKEND=openbao, in OpenBao on :8804
//!                    (seam 12: `dev_openbao`). Sign-in is the
//!                    WorkOS fake on :8794, or, with FRAGMENT_SIGNIN=oidc,
//!                    the OpenID Connect fake on :8802, or a real provider
//!                    (FRAGMENT_OIDC_ISSUER: `dev_oidc`; docs/self-host.md,
//!                    seam 4). FRAGMENT_DEV_PORT moves the stack and its
//!                    fakes together (`dev_shift`), so a second one runs
//!                    beside one on :8790
//!   try <template> [name]
//!                    on the running dev stack: a fragment from a template
//!                    (todo, inbox, notes), scaffolded under target/devstack/try
//!                    so nothing lands in the repo; prints what to open and paste
//!   e2e [args...]    build, then run crates/e2e (args pass through: --only <section>[,...],
//!                    --except <section>[,...], or --shard <k>/<n> (the table's, as CI
//!                    splits it); --rehearse keeps the hosted lane's rules on the
//!                    local node). The build builds the computer images ahead of
//!                    the node beside the Rust. CI splits it in two steps, so the
//!                    cache saves between them: --build-only (builds, runs
//!                    nothing), then --no-build (runs what the build left)
//!   e2e --hosted --config <file> --branch <name> [--only … | --except …]
//!       [--dry-run | --sweep] [--max-paid-calls <n>] [--operator-key-file <file>]
//!       [--candidate <image>]
//!                    the suite against that branch deployment on its real vendors
//!                    (crates/e2e/src/hosted.rs); --dry-run prints its plan;
//!                    --operator-key-file names an operator key the config's
//!                    operators list (its `wipe` section's); --candidate names
//!                    the config's canary, which the real-agent sections run
//!                    and agent-restart upgrades into and back
//!   check            every first-party JavaScript file parses (node --check;
//!                    xtask/src/js_syntax.rs) and none turns a string into
//!                    markup (xtask/src/html_sinks.rs), no runtime code runs
//!                    a string as script (xtask/src/eval_sinks.rs), the
//!                    schema lock is what the Durable Objects' steps make
//!                    (xtask/src/schema_lock.rs), then host tests and
//!                    clippy, warnings denied
//!   schema-lock      appends the Durable Objects' new schema steps to
//!                    crates/core/migrations/schema.lock, refusing to change
//!                    or drop a locked one (crates/core/src/migrate.rs)
//!   secret set <name> --config <file> [--from-file <path>]
//!   secret gen <name> --config <file>
//!   secret list --config <file>
//!                    the deployment's secrets in its account's Cloudflare
//!                    Secrets Store: set one (wrangler's hidden prompt, standard
//!                    input, or a file once), make a host secret, or list names
//!                    and times; --local <state dir> for wrangler's local store
//!                    instead (xtask/src/secret.rs)
//!   deploy --config <file> [--branch <name>] [--pinned-only]
//!                    build and deploy to Cloudflare from a deployment's config
//!                    (deploy/example.jsonc), once its secrets are all in the
//!                    store: a branch gets a complete copy of its own at
//!                    <branch>.<zone> (xtask/src/deploy.rs); --pinned-only
//!                    (CI's) refuses a config that builds a computer image
//!                    rather than keeping it at its digest
//!   image push <image> --repository <name> --tag <tag> [--summary <file>]
//!                    build a computer image (hermes) as a deploy builds it and
//!                    push it to the registry of the account CLOUDFLARE_ACCOUNT_ID
//!                    names, then print the digest a config pins it at
//!                    (xtask/src/image.rs; the Image workflow runs it)
//!   image copy <image> --from <reference> --repository <name> --tag <tag> [--summary <file>]
//!                    copy an image another account's registry holds
//!                    (FROM_CLOUDFLARE_ACCOUNT_ID's) into that one, byte for
//!                    byte (skopeo), its digest checked unchanged: a promotion
//!   teardown --config <file> --branch <name>
//!                    remove a branch deployment (irreversible)
//!   stripe check|setup --config <file> [--branch <name>] [--webhook-secret-file <path>]
//!                    the deployment's Stripe as docs/billing.md wants it, on the
//!                    account its stripe.key_file names: check reads and says what
//!                    is wrong; setup makes what is missing (xtask/src/stripe.rs)
//!
//! dev, e2e, secret, deploy, image and teardown run wrangler and npm on the pinned Node,
//! and check runs `node --check` on it, fetched into target/tools on first
//! use, never a `node` from PATH (crates/devstack/src/node.rs).
//!
//! fragment.club runs on celld from the `celld` branch (the tag celld-final)
//! until the cutover (docs/cloudflare-v1.md, decision 35): `deploy` never
//! reaches it.

use std::net::TcpStream;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;

mod build;
mod deploy;
mod dns;
mod eval_sinks;
mod guards;
mod html_sinks;
mod image;
mod js_syntax;
#[cfg(test)]
mod markdown_dom;
mod schema_lock;
mod secret;
mod stripe;

const DEV_PORT: u16 = 8790;
const DEV_CODESTORAGE_PORT: u16 = 8792;
const DEV_WORKOS_PORT: u16 = 8794;
/// The Workers AI fake behind the model route (`FRAGMENT_AI_URL`): dev
/// never calls real models.
const DEV_AI_PORT: u16 = 8796;
/// The mail fake (`FRAGMENT_MAIL_URL`): dev never sends real mail, and
/// prints each message instead.
const DEV_MAIL_PORT: u16 = 8798;
/// The Stripe fake (docs/billing.md): dev never charges a card; its
/// Checkout is a page with a Pay button.
const DEV_STRIPE_PORT: u16 = 8800;
const DEV_STRIPE_KEY: &str = "sk_test_fragment_dev";
/// The OpenID Connect fake (`FRAGMENT_SIGNIN=oidc`: docs/self-host.md,
/// seam 4), and its client.
const DEV_OIDC_PORT: u16 = 8802;
const DEV_OIDC_CLIENT: &str = "fragment-dev";
const DEV_OIDC_SECRET: &str = "oidc-secret-fragment-dev";
/// OpenBao, on loopback, when the cell's secrets are its (on celld,
/// `FRAGMENT_SECRETS_BACKEND=openbao`: `dev_openbao`).
const DEV_OPENBAO_PORT: u16 = 8804;
const DEV_STRIPE_WEBHOOK: &str = "whsec_fragment_dev";
/// Where dev's mail says it comes from.
const DEV_MAIL_FROM: &str = "fragment <mail@fragment.localhost>";
/// The WorkOS fake's environment in dev.
const DEV_WORKOS_CLIENT: &str = "client_fragment_dev";
const DEV_WORKOS_KEY: &str = "sk_test_fragment_dev";
const DEV_ORG: &str = "fragment-dev";

/// How far `FRAGMENT_DEV_PORT` moves the dev stack's ports from `DEV_PORT`
/// (each fake keeps its place above the cell), so a second stack runs
/// beside one on :8790; none when it is unset.
fn dev_shift() -> Result<u16> {
    // OpenBao's, the highest a stack takes
    let top = DEV_OPENBAO_PORT - DEV_PORT;
    match std::env::var("FRAGMENT_DEV_PORT") {
        Err(_) => Ok(0),
        Ok(p) => match p.parse::<u16>() {
            Ok(port) if (1024..=u16::MAX - top).contains(&port) => Ok(port.wrapping_sub(DEV_PORT)),
            _ => bail!("FRAGMENT_DEV_PORT is a port from 1024 to {}, not {p:?}", u16::MAX - top),
        },
    }
}

/// A command as errors show it: the program and its arguments only
/// (`{cmd:?}` would print the environment set on it, where a deploy may
/// hand wrangler its token).
fn shown(cmd: &Command) -> String {
    std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

fn run(cmd: &mut Command) -> Result<()> {
    let shown = shown(cmd);
    let status = cmd.status().with_context(|| format!("could not start {shown}"))?;
    if !status.success() {
        bail!("{shown} failed: {status}");
    }
    Ok(())
}

/// cell/ for wasm32 (build.rs).
fn build() -> Result<()> {
    build::cell()
}

fn dev(args: &[String]) -> Result<()> {
    // the runtime: wrangler's local workerd, or celld (docs/self-host.md,
    // seam 1), located first: a missing CELLD_BIN stops the run at once
    let celld = match args.iter().position(|a| a == "--runtime").map(|i| args.get(i + 1).map(String::as_str)) {
        None | Some(Some("wrangler")) => None,
        Some(Some("celld")) => Some(devstack::celld::CelldTools::locate()?),
        Some(other) => bail!("--runtime is wrangler or celld, not {}", other.unwrap_or("nothing")),
    };
    // the pinned Node and node_modules first: one that cannot be had stops
    // the run before a build
    let tools = devstack::Tools::locate()?;
    let shift = dev_shift()?;
    let at = |port: u16| port.wrapping_add(shift);
    let dev_port = at(DEV_PORT);
    build()?;
    let clean = args.iter().any(|a| a == "--clean");
    if clean {
        // the node's state, before the fleet seeds its local store again
        devstack::clear_state(&devstack::cell_dir())?;
    }
    let store = dev_codestore(clean, at(DEV_CODESTORAGE_PORT))?;
    // sign-in: a real WorkOS environment when its files are named (its
    // redirect URI must include http://127.0.0.1:8790/auth/callback), else the fake
    let read = |var: &str| -> Result<Option<String>> {
        match std::env::var_os(var) {
            Some(path) => Ok(Some(std::fs::read_to_string(&path).with_context(|| format!("reading {}", Path::new(&path).display()))?.trim().to_string())),
            None => Ok(None),
        }
    };
    let (workos, _workos_fake) = match (read("WORKOS_CLIENT_ID_FILE")?, read("WORKOS_API_KEY_FILE")?) {
        (Some(client_id), Some(api_key)) => (devstack::WorkOsVars { client_id, api_key, api_url: None }, None),
        _ => {
            let fake = fragment_fakes::workos::WorkOs::start_on(at(DEV_WORKOS_PORT), DEV_WORKOS_CLIENT, DEV_WORKOS_KEY)?;
            (devstack::WorkOsVars { client_id: DEV_WORKOS_CLIENT.into(), api_key: DEV_WORKOS_KEY.into(), api_url: Some(fake.url.clone()) }, Some(fake))
        }
    };
    let ai = fragment_fakes::workers_ai::WorkersAi::start(at(DEV_AI_PORT))?;
    let models = dev_models(&read)?;
    // the cell's own secrets: wrangler's local store, or on celld the
    // stand-ins' variables or OpenBao behind them (docs/self-host.md, seam 12)
    let openbao = dev_openbao(celld.is_some(), clean, at(DEV_OPENBAO_PORT))?;
    let mailer = fragment_fakes::mail::Mailer::start(at(DEV_MAIL_PORT), true)?;
    let stripe = fragment_fakes::stripe::Stripe::start(at(DEV_STRIPE_PORT), DEV_STRIPE_KEY)?;
    stripe.set_endpoint(&format!("http://127.0.0.1:{dev_port}/api/stripe/webhook"), DEV_STRIPE_WEBHOOK);
    let (oidc, oidc_fake) = dev_oidc(&read, at(DEV_OIDC_PORT))?;
    let signin_label = match (&oidc, &workos.api_url) {
        (Some(o), _) if oidc_fake.is_some() => format!("{} (the OpenID Connect fake: any email)", o.issuer),
        (Some(o), _) => format!("the OpenID Connect provider {}", o.issuer),
        (None, Some(u)) => format!("{u} (the WorkOS fake)"),
        (None, None) => format!("WorkOS {}", workos.client_id),
    };
    // preview cards: wrangler's workerd has Browser Rendering's local mode;
    // on celld, which has no `browser` binding, the renderer shoots them
    let (suffix, platform_url) = ("fragment.localhost", format!("http://127.0.0.1:{dev_port}"));
    let renderer = match celld {
        Some(_) => Some(dev_renderer(suffix, &platform_url)?),
        None => None,
    };
    let fleet = devstack::Fleet {
        host_secret: devstack::dev_secret("host-secret", || devstack::random_hex(32))?,
        codestorage_org: store.org.clone(),
        codestorage_key_pem: store.key_pem.clone(),
        codestorage_url: store.url.clone(),
        host_suffix: suffix.into(),
        host_label_suffix: None,
        // A dev fragment's pins move by its own moves, the CLI's refresh, and this poll.
        poll_interval_s: 10,
        egress_local: true,
        job_retry_delay_s: 2,
        blob_grace_s: None,
        // text and images: the Workers AI fake (dev never calls a real model),
        // every tier on it
        ai_url: Some(ai.url.clone()),
        ai_gateway: None,
        models: models.as_ref().map(|m| m.cell.to_string()),
        model_key_slots: models.as_ref().map(|m| m.slots.clone()).unwrap_or_default(),
        model_key_value: models.as_ref().and_then(|m| m.key.clone()),
        // the mail fake, which prints each message (dev never sends real mail)
        mail_url: Some(mailer.url.clone()),
        mail_from: Some(DEV_MAIL_FROM.into()),
        // a dev person is a seat, with the month's included credit
        default_plan: Some("seat".into()),
        delivery_retry_s: None,
        workos: Some(workos),
        oidc,
        stripe: Some(devstack::StripeVars {
            key: DEV_STRIPE_KEY.into(),
            webhook_secret: DEV_STRIPE_WEBHOOK.into(),
            api_url: Some(stripe.url.clone()),
            portal: None,
            tax: false,
        }),
        // the CLI's host: sign-in and approvals happen where it points
        platform_url,
        operators: None,
        signins_pending_max: None,
        cli_begins_per_ip_minute: None,
        test_secret: None,
        // celld runs no containers of its own here: a self-hosted
        // deployment's computers are a sandcastle node's (seam 2)
        computer_image: celld.is_none().then(|| "stub".into()),
        computer_snapshots: false,
        computer_unsaved_max_ms: None,
        providers: None,
        operator_key_values: vec![],
        connection_secret_values: vec![],
        swap_upstream: None,
        // wrangler's local store under cell/.wrangler/state; celld has none,
        // and the cell's stand-ins read .dev.vars or OpenBao (seam 12)
        secrets: match (&celld, &openbao) {
            (None, _) => devstack::Secrets::Store,
            (Some(_), None) => devstack::Secrets::Shim,
            (Some(_), Some((bao, _))) => devstack::Secrets::OpenBao(bao.target()),
        },
        browser_url: renderer.as_ref().map(|(r, _)| r.url.clone()),
    };
    fleet.configure(&tools, &devstack::cell_dir())?;
    // the cell's token, renewed while the stack runs (each start renews it too)
    let _renewer = openbao.as_ref().map(|(bao, _)| devstack::openbao::Renewer::start(bao.target(), devstack::openbao::RENEW_EVERY));
    let opts = devstack::NodeOptions {
        project: devstack::cell_dir(),
        port: dev_port,
        log_dir: devstack::repo_root().join("target/devstack"),
        // wrangler's own debug logs, when asked for
        node_logs: std::env::var_os("FRAGMENT_NODE_LOGS").is_some(),
        // in the terminal's group: Ctrl-C stops it with xtask
        own_group: false,
    };
    let (node, took) = match &celld {
        None => devstack::Node::start(&tools, &opts)?,
        Some(celld) => devstack::celld::start(celld, &opts)?,
    };
    println!("fragment dev: {} (ready in {took:.1?}; Ctrl-C stops it)", node.base);
    match &celld {
        None => println!("  runtime:      wrangler dev (workerd)"),
        Some(c) => println!("  runtime:      celld dev ({}): no computers, no mail but the fake's", c.celld.display()),
    }
    match &renderer {
        Some((r, said)) => println!("  cards:        {said} (the renderer at {})", r.url),
        None => println!("  cards:        Browser Rendering's local mode (wrangler's Chrome for Testing)"),
    }
    println!("  node log:     {}", node.log.display());
    println!("  fragments:    http://<name>.fragment.localhost:{dev_port}/");
    println!("  code.storage: {} ({})", store.url, store.said);
    match &models {
        Some(m) => println!("  models:       {}; the rest, and image steps, on the Workers AI fake at {}", m.said, ai.url),
        None => println!("  models:       {} (the Workers AI fake: echoes, never a real model)", ai.url),
    }
    println!("  seats:        {} (the Stripe fake: its Checkout is a Pay button)", stripe.url);
    match &openbao {
        Some((bao, took)) => println!(
            "  secrets:      OpenBao {} at {} on loopback (ready in {took:.1?}; its state {}, its log {})",
            devstack::openbao::OPENBAO_VERSION,
            bao.target().addr,
            bao.layout().dir.display(),
            bao.layout().log().display()
        ),
        None if celld.is_some() => println!("  secrets:      the cell's variables (.dev.vars; FRAGMENT_SECRETS_BACKEND=openbao for OpenBao)"),
        None => println!("  secrets:      wrangler's local Secrets Store"),
    }

    println!("  sign-in:      http://127.0.0.1:{dev_port}/ via {signin_label}");
    println!("  try one:      cargo xtask try todo | inbox   (in another terminal)");
    // Ctrl-C stops the node, and xtask outlives it to remove the containers
    // it left (wrangler's teardown removes its computers, not their sidecars)
    devstack::signals::outlive_interrupt()?;
    let status = node.wait()?;
    println!("the node exited: {status}");
    // the runtime's own containers: celld runs none here
    if celld.is_none() {
        let removed = devstack::containers::remove(&devstack::cell_dir())?;
        println!("removed {} containers the dev node left", removed.containers);
    }
    // the code store started with the stack stops with it
    if let Some(m) = store.macrofiche {
        m.stop()?;
    }
    Ok(())
}

/// Sign-in on OpenID Connect for dev (docs/self-host.md, seam 4), when
/// asked for: a real provider (`FRAGMENT_OIDC_ISSUER`, with
/// `FRAGMENT_OIDC_CLIENT_ID`, the client secret in the file
/// `FRAGMENT_OIDC_CLIENT_SECRET_FILE` (none: a public client), and
/// optionally `FRAGMENT_OIDC_SCOPES`, `FRAGMENT_OIDC_CLAIMS`,
/// `FRAGMENT_OIDC_AUTH`; its client's redirect URI is
/// `http://127.0.0.1:8790/auth/callback`, its post-logout one
/// `http://127.0.0.1:8790/`), else with `FRAGMENT_SIGNIN=oidc` the fake on
/// :8802 (any email), else none: WorkOS signs people in.
fn dev_oidc(read: &dyn Fn(&str) -> Result<Option<String>>, fake_port: u16) -> Result<(Option<devstack::OidcVars>, Option<fragment_fakes::oidc::Oidc>)> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(issuer) = var("FRAGMENT_OIDC_ISSUER") {
        let vars = devstack::OidcVars {
            issuer,
            client_id: var("FRAGMENT_OIDC_CLIENT_ID").context("FRAGMENT_OIDC_ISSUER needs FRAGMENT_OIDC_CLIENT_ID")?,
            client_secret: read("FRAGMENT_OIDC_CLIENT_SECRET_FILE")?,
            scopes: var("FRAGMENT_OIDC_SCOPES"),
            claims: var("FRAGMENT_OIDC_CLAIMS"),
            auth: var("FRAGMENT_OIDC_AUTH"),
        };
        return Ok((Some(vars), None));
    }
    match var("FRAGMENT_SIGNIN").as_deref() {
        None | Some("workos") => Ok((None, None)),
        Some("oidc") => {
            let fake = fragment_fakes::oidc::Oidc::start_on(fake_port, DEV_OIDC_CLIENT, DEV_OIDC_SECRET)?;
            let vars = devstack::OidcVars { issuer: fake.url.clone(), client_id: DEV_OIDC_CLIENT.into(), client_secret: Some(DEV_OIDC_SECRET.into()), scopes: None, claims: None, auth: None };
            Ok((Some(vars), Some(fake)))
        }
        Some(other) => bail!("FRAGMENT_SIGNIN is workos or oidc, not {other:?}"),
    }
}

/// The dev stack's OpenBao (docs/self-host.md, seam 12), when the cell's
/// secrets are its: on celld, `FRAGMENT_SECRETS_BACKEND` is `vars` (the
/// stand-ins' variables, the default) or `openbao`; on wrangler, which has
/// a Secrets Store of its own, unset. The pinned OpenBao (or
/// `FRAGMENT_OPENBAO_TARBALL`'s), on loopback at `port` (`DEV_OPENBAO_PORT`,
/// moved with the stack), its state in `target/devstack/openbao-<port>`
/// (cleared by `--clean`, after which every secret is seeded again). With
/// it, how long its start took.
fn dev_openbao(celld: bool, clean: bool, port: u16) -> Result<Option<(devstack::openbao::OpenBao, std::time::Duration)>> {
    let var = std::env::var("FRAGMENT_SECRETS_BACKEND").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    if !celld {
        if let Some(v) = var {
            bail!("FRAGMENT_SECRETS_BACKEND={v} is celld's: wrangler dev has a Secrets Store of its own (drop it, or add --runtime celld)");
        }
        return Ok(None);
    }
    match var.as_deref().unwrap_or("vars") {
        "vars" => return Ok(None),
        "openbao" => {}
        other => bail!("FRAGMENT_SECRETS_BACKEND is vars or openbao, not {other:?}"),
    }
    let dir = devstack::repo_root().join(format!("target/devstack/openbao-{port}"));
    if clean && dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("clear {}", dir.display()))?;
    }
    let bin = devstack::openbao::locate(&devstack::repo_root().join(devstack::TOOLS_DIR))?;
    let opts = devstack::openbao::Options { dir, port, audit: false };
    Ok(Some(devstack::openbao::OpenBao::start(&bin, opts)?))
}

/// The dev stack's renderer on celld (docs/self-host.md, seam 7): the
/// pinned chrome-headless-shell (`target/tools`, fetched on first use, or
/// `FRAGMENT_BROWSER_ZIP`), its pages let reach the fragments' origins
/// alone, served on this box at the platform URL's port (the node, or an
/// edge before it) unless `FRAGMENT_BROWSER_UPSTREAM` (`ip:port`) names
/// where. `FRAGMENT_BROWSER_CA_FILE` names a private CA's PEM the browser
/// trusts, for fragments on https under it. With it, what the banner says
/// the browser is.
fn dev_renderer(suffix: &str, platform_url: &str) -> Result<(devstack::rendering::Rendering, String)> {
    let upstream = match std::env::var("FRAGMENT_BROWSER_UPSTREAM") {
        Ok(u) => Some(u.parse().with_context(|| format!("FRAGMENT_BROWSER_UPSTREAM is ip:port, not {u:?}"))?),
        Err(_) => None,
    };
    let origins = devstack::rendering::Origins::of_platform(suffix, platform_url, upstream).map_err(anyhow::Error::msg)?;
    let browser = devstack::browser::locate(&devstack::repo_root().join(devstack::TOOLS_DIR))?;
    let opts = devstack::rendering::Options {
        browser: browser.bin,
        state: devstack::repo_root().join(format!("target/devstack/renderer-{}", origins.port)),
        origins,
        ca_file: std::env::var_os("FRAGMENT_BROWSER_CA_FILE").map(std::path::PathBuf::from),
        port: 0,
    };
    Ok((devstack::rendering::Rendering::start(opts)?, browser.version))
}

/// The dev org's key, made on first run (`target/devstack`).
fn dev_org_key() -> Result<String> {
    devstack::dev_secret("codestorage-org-key.pem", fragment_fakes::codestorage::generate_org_key_pem)
}

/// The dev stack's code store, as it runs (docs/self-host.md, seam 5).
struct DevStore {
    url: String,
    org: String,
    key_pem: String,
    /// What the banner says it is.
    said: String,
    /// The fake, serving on this process's threads.
    _fake: Option<fragment_fakes::codestorage::CodeStorage>,
    /// macrofiche, when the stack started it.
    macrofiche: Option<devstack::codestore::Macrofiche>,
}

/// The code store by configuration (`devstack::codestore::CodeStore::from_env`):
/// the fake on `port`, its repos in `target/devstack/codestorage.json`
/// (the default); macrofiche (`MACROFICHE_BIN`) on the same port, its
/// repos in `target/devstack/macrofiche`, given the same org key's public
/// half; or a store already running (`CODESTORAGE_API_URL`, `CODESTORAGE_ORG`,
/// `CODESTORAGE_PRIVATE_KEY_FILE`), which `--clean` never touches. A cell
/// remembers each fragment's repo: moving to another store takes `--clean`.
fn dev_codestore(clean: bool, port: u16) -> Result<DevStore> {
    use devstack::codestore::{CodeStore, Macrofiche, MacroficheOptions};
    let dir = devstack::repo_root().join("target/devstack");
    match CodeStore::from_env()? {
        CodeStore::Fake => {
            let state = dir.join("codestorage.json");
            if clean {
                let _ = std::fs::remove_file(&state);
            }
            let key = dev_org_key()?;
            let fake = fragment_fakes::codestorage::CodeStorage::start(fragment_fakes::codestorage::Options {
                org: DEV_ORG.into(),
                org_key_pem: Some(key.clone()),
                state_file: Some(state),
                port,
                ..Default::default()
            })?;
            Ok(DevStore { url: fake.url.clone(), org: DEV_ORG.into(), key_pem: key, said: "the fake".into(), _fake: Some(fake), macrofiche: None })
        }
        CodeStore::External(x) => Ok(DevStore { url: x.url, org: x.org, key_pem: x.key_pem, said: "a store already running: CODESTORAGE_API_URL".into(), _fake: None, macrofiche: None }),
        CodeStore::Macrofiche(bin) => {
            let data = dir.join("macrofiche");
            if clean {
                let _ = std::fs::remove_dir_all(&data);
            }
            let key = dev_org_key()?;
            let m = Macrofiche::start(&bin, &MacroficheOptions { dir: data.clone(), port, org: DEV_ORG.into(), key_pem: key.clone(), log_dir: dir })?;
            let said = format!("macrofiche {}, its repos in {}, its log {}", bin.display(), data.display(), m.log.display());
            Ok(DevStore { url: m.url.clone(), org: DEV_ORG.into(), key_pem: key, said, _fake: None, macrofiche: Some(m) })
        }
    }
}

/// The dev stack's models, from `FRAGMENT_MODELS_FILE`.
struct DevModels {
    /// The cell's `FRAGMENT_MODELS`: each slot's upstream, less its key.
    cell: serde_json::Value,
    /// The slots on an OpenAI-compatible API, each bound to `key`.
    slots: Vec<String>,
    key: Option<String>,
    /// What the banner says of them.
    said: String,
}

/// Where the dev stack's model route sends its calls, beside the Workers
/// AI fake: `FRAGMENT_MODELS_FILE` names a file holding a deployment's
/// `models` as its config writes them (deploy/example.jsonc; JSON, `//`
/// comments allowed), checked as a deploy checks them but for its URLs,
/// which may be local (dev's egress is). The key the slots on an
/// OpenAI-compatible API name is read from the file
/// `FRAGMENT_MODEL_KEY_FILE` names (dev's store holds one; a server that
/// takes none still gets a value, any word). A model the price book does
/// not price (a self-hosted one) names its price in its slot's `body`, as
/// a model on OpenRouter does (`provider.max_price`; docs/self-host.md,
/// seam 3). Unset, every slot is the fake's.
fn dev_models(read: &dyn Fn(&str) -> Result<Option<String>>) -> Result<Option<DevModels>> {
    let Some(file) = std::env::var_os("FRAGMENT_MODELS_FILE") else { return Ok(None) };
    let shown = Path::new(&file).display().to_string();
    let text = std::fs::read_to_string(&file).with_context(|| format!("read {shown} (FRAGMENT_MODELS_FILE)"))?;
    let models: serde_json::Value = serde_json::from_str(&devstack::strip_comments(&text)).with_context(|| format!("{shown} is a deployment's `models`, as JSON"))?;
    let (cell, keys) = deploy::split_models(&models, true).with_context(|| format!("{shown} (FRAGMENT_MODELS_FILE)"))?;
    let names: std::collections::BTreeSet<&str> = keys.iter().map(|(_, name)| name.as_str()).collect();
    if names.len() > 1 {
        bail!("{shown}: its slots name {} keys ({}); dev binds one, from FRAGMENT_MODEL_KEY_FILE", names.len(), names.into_iter().collect::<Vec<_>>().join(", "));
    }
    let key = match keys.is_empty() {
        true => None,
        false => {
            let key = read("FRAGMENT_MODEL_KEY_FILE")?.with_context(|| format!("{shown}'s slots on an OpenAI-compatible API name a key: put its value in a file (any word, for a server that takes none) and name it FRAGMENT_MODEL_KEY_FILE"))?;
            anyhow::ensure!(!key.is_empty(), "FRAGMENT_MODEL_KEY_FILE's file is empty: a key is a value (any word, for a server that takes none)");
            Some(key)
        }
    };
    let slots: Vec<String> = keys.into_iter().map(|(slot, _)| slot).collect();
    let said = cell
        .as_object()
        .into_iter()
        .flatten()
        .map(|(slot, u)| format!("{slot}: {} at {}", u["model"].as_str().unwrap_or("?"), u["base_url"].as_str().unwrap_or("Workers AI (the fake)")))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(DevModels { cell, slots, key, said: format!("{said} ({shown})") }))
}

/// The templates `try` scaffolds.
const TRY_TEMPLATES: [&str; 4] = ["todo", "inbox", "notes", "doc"];

fn try_template(args: &[String]) -> Result<()> {
    let usage = || format!("usage: cargo xtask try <{}> [name]", TRY_TEMPLATES.join("|"));
    let tpl = args.first().ok_or_else(|| anyhow::anyhow!(usage()))?;
    if !TRY_TEMPLATES.contains(&tpl.as_str()) {
        bail!("{}", usage());
    }
    let dev_port = DEV_PORT.wrapping_add(dev_shift()?);
    if TcpStream::connect(("127.0.0.1", dev_port)).is_err() {
        bail!("nothing answers on :{dev_port}: start `cargo xtask dev` in another terminal, wait for `ready`, then try again");
    }
    let root = devstack::repo_root();
    run(Command::new("cargo").args(["build", "--quiet", "--manifest-path"]).arg(root.join("Cargo.toml")).args(["-p", "fragment-cli"]))?;
    let cli = root.join("target/debug/fragment");
    let host = format!("http://127.0.0.1:{dev_port}");
    let name = args.get(1).cloned().unwrap_or_else(|| format!("{tpl}-{}", devstack::random_hex(2)));
    let dir = root.join("target/devstack/try");
    std::fs::create_dir_all(&dir)?;
    // your CLI key must be a person's on the dev fleet (phase 4: people sign in)
    let who = Command::new(&cli).args(["whoami"]).env("FRAGMENT_HOST", &host).output()?;
    if !who.status.success() {
        bail!(
            "your CLI key is no one's on the dev stack yet. Once:\n  FRAGMENT_HOST={host} {} login\n(the dev stack signs in through the WorkOS fake: any email)",
            cli.display()
        );
    }
    let out = Command::new(&cli).args(["init", &name, "--template", tpl]).env("FRAGMENT_HOST", &host).current_dir(&dir).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    print!("{text}");
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("fragment init failed (a name already taken? pass another: cargo xtask try {tpl} <name>)");
    }
    let line = |prefix: &str| text.lines().find_map(|l| l.strip_prefix(prefix)).map(str::trim).unwrap_or("").to_string();
    let alias = format!("alias fragment='FRAGMENT_HOST={host} {}'", cli.display());
    println!("\nnext:");
    println!("  open it      {}", line("share link:"));
    if tpl == "inbox" {
        let hook = line("webhook URL:");
        println!("  post to it   curl -s -X POST '{hook}' -H 'content-type: application/json' -d '{{\"payload\":{{\"text\":\"hi\"}}}}'");
        println!("  or a page    curl -s -X POST '{hook}' -H 'content-type: application/json' -d '{{\"payload\":{{\"url\":\"https://example.com\"}}}}'");
    }
    println!("  the CLI      {alias}");
    let query = if tpl == "doc" { "notes" } else { "list" };
    println!("               fragment runs {name} | fragment channel {name} | fragment call {name} {query}");
    println!("  change it    edit {}, then: fragment deploy {name} --dir {}", dir.join(&name).display(), dir.join(&name).display());
    Ok(())
}

fn e2e(args: &[String]) -> Result<()> {
    // the hosted lane: a branch deployment, read from its config; it needs
    // the CLI and the suite, and no Worker built here (a dry run, the suite alone)
    if args.iter().any(|a| a == "--hosted") {
        let suite_args = deploy::hosted_e2e_args(args)?;
        match suite_args.iter().any(|a| a == "--dry-run") {
            true => build_suite()?,
            false => build_native()?,
        }
        return run(Command::new(devstack::repo_root().join(E2E_BIN)).args(suite_args));
    }
    // CI's split: `--build-only` builds (and the cache is saved after it,
    // so a red run still saves), then `--no-build` runs what it left
    let build_only = args.iter().any(|a| a == "--build-only");
    let no_build = args.iter().any(|a| a == "--no-build");
    if build_only && no_build {
        bail!("--build-only and --no-build are two steps of one run, not one");
    }
    let suite_args: Vec<&String> = args.iter().filter(|a| *a != "--build-only" && *a != "--no-build").collect();
    if build_only && !suite_args.is_empty() {
        bail!("--build-only builds what every run needs, and takes no suite arguments");
    }
    // the pinned Node and node_modules, before the build; the suite finds
    // them in place
    devstack::Tools::locate()?;
    if !no_build {
        build_e2e()?;
    }
    if build_only {
        return Ok(());
    }
    run(Command::new(devstack::repo_root().join(E2E_BIN)).args(suite_args))
}

/// The suite alone, for this machine.
fn build_suite() -> Result<()> {
    let manifest = devstack::repo_root().join("Cargo.toml");
    run(Command::new("cargo").args(["build", "--quiet", "--release", "--manifest-path"]).arg(&manifest).args(["-p", "fragment-e2e"]))
}

/// The suite, as `build_e2e` leaves it.
const E2E_BIN: &str = "target/release/fragment-e2e";
/// What the e2e runs: the cell, the CLI (the e2e drives it too) and the
/// suite beside it, and the computer images the node's boot builds, built
/// ahead beside them all so its build finds every layer cached. A run on
/// celld (`FRAGMENT_E2E_RUNTIME=celld`) builds no images: celld runs no
/// containers of its own (docs/self-host.md, seam 2).
fn build_e2e() -> Result<()> {
    let t0 = std::time::Instant::now();
    let images = std::env::var("FRAGMENT_E2E_RUNTIME").as_deref() != Ok("celld");
    let built = std::thread::scope(|s| {
        let built_images = images.then(|| s.spawn(build::images));
        let native = s.spawn(build_native);
        let cell = build::cell();
        let built_images = built_images.map_or(Ok(()), |b| b.join().expect("the images' build does not panic"));
        cell.and(native.join().expect("the native build does not panic")).and(built_images)
    });
    println!("built the cell, the CLI, the suite{} in {:.1?}", if images { " and the images" } else { "" }, t0.elapsed());
    built
}

/// The CLI and the suite, for this machine.
fn build_native() -> Result<()> {
    let t0 = std::time::Instant::now();
    let manifest = devstack::repo_root().join("Cargo.toml");
    run(Command::new("cargo").args(["build", "--quiet", "--release", "--manifest-path"]).arg(&manifest).args(["-p", "fragment-cli"]))?;
    build_suite()?;
    println!("built the CLI and the suite in {:.1?}", t0.elapsed());
    Ok(())
}

/// A merge conflict's markers (git's diff3 style too), at the start of a
/// line: none may be committed.
fn conflict_marker(line: &str) -> bool {
    ["<<<<<<< ", "||||||| ", ">>>>>>> "].iter().any(|m| line.starts_with(m)) || line == "======="
}

/// Fails on a tracked text file holding conflict markers: a rebase that
/// committed one compiles when it lands in docs or a lockfile.
fn no_conflict_markers(root: &Path) -> Result<()> {
    let out = Command::new("git").args(["ls-files", "-z"]).current_dir(root).output().context("git ls-files")?;
    anyhow::ensure!(out.status.success(), "git ls-files: {}", String::from_utf8_lossy(&out.stderr));
    let mut found = vec![];
    for path in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path).into_owned();
        // a file that is not text (a font, an image) holds no markers
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else { continue };
        let at = text.lines().enumerate().filter(|(_, l)| conflict_marker(l)).map(|(i, _)| format!("{path}:{}", i + 1));
        found.extend(at);
    }
    anyhow::ensure!(found.is_empty(), "conflict markers are committed:\n{}", found.join("\n"));
    Ok(())
}

/// Every first-party JavaScript file parses, on the pinned Node (fetched
/// into target/tools on first use; no node_modules needed): seconds here,
/// where the e2e would find it most of an hour in (xtask/src/js_syntax.rs).
fn javascript_parses(root: &Path) -> Result<()> {
    let node = devstack::node::locate(&root.join(devstack::TOOLS_DIR))?;
    let started = std::time::Instant::now();
    let files = js_syntax::files(root)?;
    js_syntax::check(&node, &root.join(devstack::CACHE_DIR), root, &files)?;
    println!("JavaScript: {} files parse (node --check on Node v{}, {:.1?})", files.len(), devstack::node::NODE_VERSION, started.elapsed());
    Ok(())
}

/// No first-party JavaScript turns a string into markup
/// (xtask/src/html_sinks.rs): text becomes DOM only by the DOM's own
/// constructors, so no file or message can script a fragment's origin.
fn no_html_sinks(root: &Path) -> Result<()> {
    let sources = html_sinks::sources(root)?;
    html_sinks::check(&sources, &html_sinks::ALLOWED)?;
    println!("HTML sinks: none in {} sources ({} allowed, each with why)", sources.len(), html_sinks::ALLOWED.len());
    Ok(())
}

/// No runtime code runs a string as script (xtask/src/eval_sinks.rs): no
/// DevTools evaluate, no page driver's, no `eval`, `Function` or timer of a
/// string, no `import()` of a URL made at run time.
fn no_eval_sinks(root: &Path) -> Result<()> {
    let files = eval_sinks::sources(root)?;
    eval_sinks::check(&files, &eval_sinks::ALLOWED)?;
    println!("eval sinks: none in {} runtime files ({} allowed, each with why)", files.len(), eval_sinks::ALLOWED.len());
    Ok(())
}

fn check() -> Result<()> {
    let root = devstack::repo_root();
    no_conflict_markers(&root)?;
    no_html_sinks(&root)?;
    no_eval_sinks(&root)?;
    schema_lock::holds(&root)?;
    let read = |path: &str| std::fs::read_to_string(root.join(path)).with_context(|| format!("read {path}"));
    let copies = ["cli/GUIDE.md", "README.md", "cell/shell/shell.js"].map(|path| read(path).map(|text| (path, text)));
    let copies: Vec<(&str, String)> = copies.into_iter().collect::<Result<_>>()?;
    skill_installs_release(&read("cli/SKILL.md")?, &read(".github/workflows/release.yml")?, &copies)?;
    javascript_parses(&root)?;
    run(Command::new("cargo").args(["test", "--workspace", "--all-features"]).current_dir(&root))?;
    run(Command::new("cargo")
        .args(["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])
        .current_dir(&root))?;
    run(Command::new("cargo")
        .args(["clippy", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"])
        .current_dir(Path::new(&devstack::cell_dir())))
}

/// Where the one-line install fetches the CLI from.
const RELEASES: &str = "https://github.com/finitecomputer/fragment/releases/latest/download/";
/// Each target the release builds, and its asset's name: the `uname -s` and
/// `uname -m` of the machines it runs on, which the install command names.
const RELEASE_ASSETS: [(&str, &str); 4] = [
    ("aarch64-apple-darwin", "Darwin-arm64"),
    ("x86_64-apple-darwin", "Darwin-x86_64"),
    ("x86_64-unknown-linux-musl", "Linux-x86_64"),
    ("aarch64-unknown-linux-musl", "Linux-aarch64"),
];

/// cli/SKILL.md (`fragment skill`) is a skill, and its one-line install,
/// which the shell's settings, GUIDE.md, and the README copy, fetches
/// the latest release's `fragment-$(uname -s)-$(uname -m).tar.gz`: the
/// release workflow (`.github/workflows/release.yml`) must build exactly
/// those assets, each a tarball of `fragment` alone. Otherwise a renamed
/// asset or a stale copy breaks the install, and nothing says so until
/// someone runs it.
fn skill_installs_release(skill: &str, workflow: &str, copies: &[(&str, String)]) -> Result<()> {
    anyhow::ensure!(skill.starts_with("---\nname: fragment\ndescription: "), "cli/SKILL.md opens with a skill's name and description");
    let install = skill.lines().map(str::trim).find(|l| l.contains(RELEASES)).context("cli/SKILL.md shows no install command")?;
    let fetched = format!("{RELEASES}fragment-$(uname -s)-$(uname -m).tar.gz | tar -xzf - -C ~/.local/bin");
    anyhow::ensure!(install.ends_with(&fetched), "the install command unpacks {fetched}, not: {install}");
    for (path, text) in copies {
        anyhow::ensure!(text.contains(install), "{path} does not show cli/SKILL.md's install command: {install}");
    }
    let field = |key: &str| workflow.lines().filter_map(|l| l.trim().strip_prefix(key)).collect::<Vec<_>>();
    let built: Vec<(&str, &str)> = field("target: ").into_iter().zip(field("asset: ")).collect();
    anyhow::ensure!(built == RELEASE_ASSETS, "the release builds {built:?}, not the assets the install fetches: {RELEASE_ASSETS:?}");
    let packed = r#"tar -czf "$RUNNER_TEMP/fragment-${{ matrix.asset }}.tar.gz" -C "target/${{ matrix.target }}/release" fragment"#;
    anyhow::ensure!(workflow.contains(packed), "each release asset is a tarball of `fragment` alone: {packed}");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build(),
        Some("dev") => dev(&args[1..]),
        Some("try") => try_template(&args[1..]),
        Some("e2e") => e2e(&args[1..]),
        Some("check") => check(),
        Some("schema-lock") => schema_lock::write(&devstack::repo_root()),
        Some("secret") => secret::secret(&args[1..]),
        Some("deploy") => deploy::deploy(&args[1..]),
        Some("teardown") => deploy::teardown(&args[1..]),
        Some("stripe") => deploy::stripe(&args[1..]),
        Some("image") => image::image(&args[1..]),
        _ => bail!("usage: cargo xtask build | dev [--clean] [--runtime wrangler|celld] | try <template> [name] | e2e [--build-only | --no-build] [--only | --except <section>[,...] | --shard <k>/<n>] [--rehearse] | e2e --hosted --config <file> --branch <name> [--dry-run | --sweep] | check | schema-lock | secret set <name> | gen <name> | list --config <file> | deploy --config <file> [--branch <name>] [--pinned-only] | image push <image> | copy <image> --from <reference> --repository <name> --tag <tag> [--summary <file>] | teardown --config <file> --branch <name>"),
    }
}

#[cfg(test)]
mod tests {
    /// Git's markers, the diff3 base's included, are caught at a line's
    /// start; text that only mentions one, or a longer rule, is not.
    #[test]
    fn conflict_markers_are_caught_at_a_line_start() {
        for line in ["<<<<<<< HEAD", "||||||| parent of e8dd007 (e2e: …)", "=======", ">>>>>>> e8dd007 (e2e: …)"] {
            assert!(super::conflict_marker(line), "{line}");
        }
        for line in ["  <<<<<<< indented", "a ======= b", "========", "the `<<<<<<< ` marker", ">>>>>>>"] {
            assert!(!super::conflict_marker(line), "{line}");
        }
    }

    /// The repo's own skill, release workflow, and copies agree; a renamed
    /// asset, a stale copy, or a skill without its frontmatter does not.
    #[test]
    fn the_install_fetches_what_the_release_builds() {
        let read = |path: &str| std::fs::read_to_string(super::devstack::repo_root().join(path)).unwrap();
        let (skill, workflow) = (read("cli/SKILL.md"), read(".github/workflows/release.yml"));
        let copies = [("README.md", read("README.md"))];
        assert!(super::skill_installs_release(&skill, &workflow, &copies).is_ok());
        assert!(super::skill_installs_release(&skill, &workflow.replace("asset: Darwin-arm64", "asset: Darwin-aarch64"), &copies).is_err());
        assert!(super::skill_installs_release(&skill, &workflow, &[("README.md", read("README.md").replace("uname -m", "arch"))]).is_err());
        assert!(super::skill_installs_release(&skill.replacen("name: fragment", "title: fragment", 1), &workflow, &copies).is_err());
    }

    #[test]
    fn a_failed_command_never_shows_its_environment() {
        let mut cmd = std::process::Command::new("wrangler");
        cmd.args(["dev", "--port", "8790"]).env("FRAGMENT_KEYS_HOST_SECRET", "not-a-real-secret");
        assert_eq!(super::shown(&cmd), "wrangler dev --port 8790");
    }
}
