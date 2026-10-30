//! `cargo xtask <command>`: the repo's tooling, in Rust.
//!
//!   build [--dev]    build cell/ for wasm32 (worker-build 0.8.5: build.rs);
//!                    --dev is `dev`'s build (cell/wrangler.jsonc's rebuild):
//!                    no fat LTO and no wasm-opt, so a change to a file of the
//!                    release rebuilds in seconds; deploys and the e2e refuse it
//!   dev [--clean] [--real-agent [--models <config>]]
//!                    build, then run the stack in the foreground under
//!                    `wrangler dev`: the cell on :8790 (fragments at
//!                    <name>.fragment.localhost:8790), the
//!                    code.storage fake on :8792, and the Workers AI fake on :8796
//!                    behind the model route (its calls spend their payer's
//!                    ledger; new people are seats with the month's included
//!                    credit); --real-agent runs computers on our Hermes image
//!                    and text on real models (xtask/src/dev_agent.rs)
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
//!   deploy --config <file> [--branch <name>] [--pinned-only] [--preview-image-digest <sha256:…>]
//!                    build and deploy to Cloudflare from a deployment's config
//!                    (deploy/example.jsonc), once its secrets are all in the
//!                    store: a branch gets a complete copy of its own at
//!                    <branch>.<zone> (xtask/src/deploy.rs); --pinned-only
//!                    (CI's) refuses a config that builds a computer image
//!                    rather than keeping it at its digest
//!                    --preview-image-digest requires --branch preview
//!                    --pinned-only, and changes only its in-memory pins
//!   image preview --output <file> [--build | --dry-run] [--summary <file>]
//!                    resolve master's input-keyed Hermes image, or build
//!                    and prepare it once if absent (--build); write its
//!                    input key and digest for CI's preview deploy
//!                    --dry-run prints its plan without credentials or calls
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
//!   cli id [<rev>]   the CLI's content id at a commit (default HEAD): a hash
//!                    of what it is built from, never of the commit
//!                    (xtask/src/cli.rs)
//!   cli plan [--at <rev>] [--latest <tag>] [--github-output <file>] [--summary <file>]
//!                    whether production's deploy of a commit publishes a
//!                    CLI release (its CLI is not the latest release's), and
//!                    its version: CI's `cli-plan` (ci.yml)
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
mod cli;
mod cloudflare;
mod deploy;
mod dev_agent;
mod dns;
mod eval_sinks;
mod guards;
mod html_sinks;
mod image;
mod js_syntax;
mod prepare;
#[cfg(test)]
mod markdown_dom;
mod rollout;
mod schema_lock;
// CI's freshness of its cached builds, compiled by itself with rustc
// (ci.yml); here for its tests alone
#[cfg(test)]
mod stamp;
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
const DEV_STRIPE_WEBHOOK: &str = "whsec_fragment_dev";
/// Where dev's mail says it comes from.
const DEV_MAIL_FROM: &str = "fragment <mail@fragment.localhost>";
/// The WorkOS fake's environment in dev.
const DEV_WORKOS_CLIENT: &str = "client_fragment_dev";
const DEV_WORKOS_KEY: &str = "sk_test_fragment_dev";
const DEV_ORG: &str = "fragment-dev";

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

/// cell/ for wasm32, the release's (build.rs).
fn build() -> Result<()> {
    build::cell()
}

/// `cargo xtask build [--dev]`.
fn build_cmd(args: &[String]) -> Result<()> {
    match args {
        [] => build(),
        [dev] if dev == "--dev" => build::cell_as(build::Profile::Dev),
        _ => bail!("build takes --dev alone, not {args:?}"),
    }
}

/// What `cargo xtask dev` is asked for: `--clean`, `--real-agent`, and
/// `--models <config>` with it.
struct DevArgs {
    clean: bool,
    real_agent: bool,
    models: Option<std::path::PathBuf>,
}

fn dev_args(args: &[String]) -> Result<DevArgs> {
    let mut out = DevArgs { clean: false, real_agent: false, models: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--clean" => out.clean = true,
            "--real-agent" => out.real_agent = true,
            "--models" => out.models = Some(it.next().context("--models names a deployment's config")?.into()),
            other => bail!("dev takes --clean, --real-agent and --models <config>, not {other:?}"),
        }
    }
    if out.models.is_some() && !out.real_agent {
        bail!("--models picks --real-agent's models: name --real-agent too");
    }
    Ok(out)
}

fn dev(args: &[String]) -> Result<()> {
    let args = dev_args(args)?;
    // its models and key are read before anything builds
    let real = args.real_agent.then(|| dev_agent::RealAgent::read(args.models.as_deref())).transpose()?;
    // the pinned Node and node_modules first: one that cannot be had stops
    // the run before a build
    let tools = devstack::Tools::locate()?;
    // dev's build, as wrangler's rebuild makes it (cell/wrangler.jsonc)
    build::cell_as(build::Profile::Dev)?;
    let clean = args.clean;
    let state = devstack::repo_root().join("target/devstack/codestorage.json");
    if clean {
        let _ = std::fs::remove_file(&state);
        // the node's state too, before the fleet seeds its local store again
        devstack::clear_state(&devstack::cell_dir())?;
    }
    let key = devstack::dev_secret("codestorage-org-key.pem", fragment_fakes::codestorage::generate_org_key_pem)?;
    let fake = fragment_fakes::codestorage::CodeStorage::start(fragment_fakes::codestorage::Options {
        org: DEV_ORG.into(),
        org_key_pem: Some(key.clone()),
        state_file: Some(state),
        port: DEV_CODESTORAGE_PORT,
        ..Default::default()
    })?;
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
            let fake = fragment_fakes::workos::WorkOs::start_on(DEV_WORKOS_PORT, DEV_WORKOS_CLIENT, DEV_WORKOS_KEY)?;
            (devstack::WorkOsVars { client_id: DEV_WORKOS_CLIENT.into(), api_key: DEV_WORKOS_KEY.into(), api_url: Some(fake.url.clone()) }, Some(fake))
        }
    };
    // an operator key no person holds, the dev node's own, so a person is
    // wiped here as on a deployment (`fragment operator wipe`), and signs in
    // again as a new one: a fresh agent's first run
    let operator_file = devstack::repo_root().join("target/devstack/operator-key");
    let operator = devstack::dev_secret("operator-key", || fragment_nip98::Keys::generate().secret_hex())?;
    let operator = fragment_nip98::Keys::from_secret_hex(operator.trim()).with_context(|| format!("{} holds an operator key (64 hex)", operator_file.display()))?;
    let ai = fragment_fakes::workers_ai::WorkersAi::start(DEV_AI_PORT)?;
    let mailer = fragment_fakes::mail::Mailer::start(DEV_MAIL_PORT, true)?;
    let stripe = fragment_fakes::stripe::Stripe::start(DEV_STRIPE_PORT, DEV_STRIPE_KEY)?;
    stripe.set_endpoint(&format!("http://127.0.0.1:{DEV_PORT}/api/stripe/webhook"), DEV_STRIPE_WEBHOOK);
    let workos_label = match &workos.api_url {
        Some(u) => format!("{u} (the fake)"),
        None => format!("WorkOS {}", workos.client_id),
    };
    let fleet = devstack::Fleet {
        host_secret: devstack::dev_secret("host-secret", || devstack::random_hex(32))?,
        codestorage_org: DEV_ORG.into(),
        codestorage_key_pem: key,
        codestorage_url: fake.url.clone(),
        host_suffix: "fragment.localhost".into(),
        host_label_suffix: None,
        // A dev fragment's pins move by its own moves, the CLI's refresh, and this poll.
        poll_interval_s: 10,
        egress_local: true,
        job_retry_delay_s: 2,
        blob_grace_s: None,
        // text and images: the Workers AI fake, every tier on it, unless
        // --real-agent puts the text tiers on real models
        ai_url: Some(ai.url.clone()),
        ai_gateway: None,
        models: real.as_ref().map(|r| r.models.clone()),
        // its slots on an OpenAI-compatible API, each on the one key dev binds
        model_keys: real
            .as_ref()
            .map(|r| r.slots.iter().map(|slot| devstack::ModelKey { slot: slot.clone(), name: devstack::store::MODEL_KEY.into(), value: r.key.clone() }).collect())
            .unwrap_or_default(),
        // the mail fake, which prints each message (dev never sends real mail)
        mail_url: Some(mailer.url.clone()),
        mail_from: Some(DEV_MAIL_FROM.into()),
        // a dev person is a seat, with the month's included credit
        default_plan: Some("seat".into()),
        delivery_retry_s: None,
        workos: Some(workos),
        stripe: Some(devstack::StripeVars {
            key: DEV_STRIPE_KEY.into(),
            webhook_secret: DEV_STRIPE_WEBHOOK.into(),
            api_url: Some(stripe.url.clone()),
            portal: None,
            tax: false,
        }),
        // the CLI's host: sign-in and approvals happen where it points
        platform_url: format!("http://127.0.0.1:{DEV_PORT}"),
        operators: Some(fragment_core::npub::encode(operator.pubkey_hex())),
        signins_pending_max: None,
        cli_begins_per_ip_minute: None,
        test_secret: None,
        computer_image: Some(if real.is_some() { dev_agent::RealAgent::IMAGE } else { "stub" }.into()),
        images: match &real {
            Some(_) => vec![(dev_agent::RealAgent::IMAGE.into(), devstack::hermes_image())],
            None => vec![],
        },
        computer_snapshots: false,
        computer_unsaved_max_ms: None,
        providers: None,
        operator_key_values: vec![],
        connection_secret_values: vec![],
        swap_upstream: None,
        cli_id: None,
    };
    // its secrets go to wrangler's local store under cell/.wrangler/state
    fleet.configure(&tools, &devstack::cell_dir())?;
    let opts = devstack::NodeOptions {
        project: devstack::cell_dir(),
        port: DEV_PORT,
        log_dir: devstack::repo_root().join("target/devstack"),
        // wrangler's own debug logs, when asked for
        node_logs: std::env::var_os("FRAGMENT_NODE_LOGS").is_some(),
        // in the terminal's group: Ctrl-C stops it with xtask
        own_group: false,
    };
    let (node, took) = devstack::Node::start(&tools, &opts)?;
    println!("fragment dev: {} (ready in {took:.1?}; Ctrl-C stops it)", node.base);
    println!("  node log:     {}", node.log.display());
    println!("  fragments:    http://<name>.fragment.localhost:{DEV_PORT}/");
    println!("  code.storage: {} (the fake)", fake.url);
    match &real {
        Some(r) => {
            println!("  models:       {} on real models, {}'s (paid by {}'s key); the rest on the Workers AI fake at {}", r.slots.join(", "), r.from.display(), dev_agent::KEY_FILE_ENV, ai.url);
            println!("  computers:    our Hermes image, built from the tree (its first build takes a while)");
        }
        None => println!("  models:       {} (the Workers AI fake: echoes, never a real model; --real-agent for real ones)", ai.url),
    }
    println!("  seats:        {} (the Stripe fake: its Checkout is a Pay button)", stripe.url);

    println!("  sign-in:      http://127.0.0.1:{DEV_PORT}/ via {workos_label}");
    println!("  try one:      cargo xtask try todo | inbox   (in another terminal)");
    println!("  wipe someone: fragment --host http://127.0.0.1:{DEV_PORT} operator wipe <email> --yes --key-file {}", operator_file.display());
    // Ctrl-C stops the node, and xtask outlives it to remove the containers
    // it left (wrangler's teardown removes its computers, not their sidecars)
    devstack::signals::outlive_interrupt()?;
    let status = node.wait()?;
    println!("wrangler dev exited: {status}");
    let removed = devstack::containers::remove(&devstack::cell_dir())?;
    println!("removed {} containers the dev node left", removed.containers);
    Ok(())
}

/// The templates `try` scaffolds.
const TRY_TEMPLATES: [&str; 4] = ["todo", "inbox", "notes", "doc"];

fn try_template(args: &[String]) -> Result<()> {
    let usage = || format!("usage: cargo xtask try <{}> [name]", TRY_TEMPLATES.join("|"));
    let tpl = args.first().ok_or_else(|| anyhow::anyhow!(usage()))?;
    if !TRY_TEMPLATES.contains(&tpl.as_str()) {
        bail!("{}", usage());
    }
    if TcpStream::connect(("127.0.0.1", DEV_PORT)).is_err() {
        bail!("nothing answers on :{DEV_PORT}: start `cargo xtask dev` in another terminal, wait for `ready`, then try again");
    }
    let root = devstack::repo_root();
    run(Command::new("cargo").args(["build", "--quiet", "--manifest-path"]).arg(root.join("Cargo.toml")).args(["-p", "fragment-cli"]))?;
    let cli = root.join("target/debug/fragment");
    let host = format!("http://127.0.0.1:{DEV_PORT}");
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
        return run(Command::new(devstack::repo_root().join(E2E_BIN)).args(suite_args).env("FRAGMENT_CLI_ID", e2e_cli_id()?));
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
    run(Command::new(devstack::repo_root().join(E2E_BIN)).args(suite_args).env("FRAGMENT_CLI_ID", e2e_cli_id()?))
}

/// The CLI's content id at HEAD (xtask/src/cli.rs), which the e2e's CLI is
/// built with and its run names to its node (`FRAGMENT_CLI_ID`), so the
/// CLI the lanes drive is the one their platform ships, as a release is
/// production's. A `--no-build` run finds the build's, as it is HEAD's.
fn e2e_cli_id() -> Result<String> {
    cli::id(&devstack::repo_root(), "HEAD")
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
/// ahead beside them all so its build finds every layer cached.
fn build_e2e() -> Result<()> {
    let t0 = std::time::Instant::now();
    let built = std::thread::scope(|s| {
        let images = s.spawn(build::images);
        let native = s.spawn(build_native);
        let cell = build::cell();
        cell.and(native.join().expect("the native build does not panic")).and(images.join().expect("the images' build does not panic"))
    });
    println!("built the cell, the CLI, the suite and the images in {:.1?}", t0.elapsed());
    built
}

/// The CLI (with its content id, as a release's: `e2e_cli_id`) and the
/// suite, for this machine.
fn build_native() -> Result<()> {
    let t0 = std::time::Instant::now();
    let manifest = devstack::repo_root().join("Cargo.toml");
    run(Command::new("cargo").args(["build", "--quiet", "--release", "--manifest-path"]).arg(&manifest).args(["-p", "fragment-cli"]).env("FRAGMENT_CLI_ID", e2e_cli_id()?))?;
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
    let cli = read("cli/src/main.rs")? + &read("cli/src/notice.rs")?;
    release_follows_production(&read(".github/workflows/release.yml")?, &read(".github/workflows/ci.yml")?, &cli)?;
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

/// Production's deploy calls the release workflow (ci.yml's `cli-release`)
/// with the plan's content id, and the release builds the tag's version
/// and that id in (release.yml's build sets FRAGMENT_CLI_VERSION and
/// FRAGMENT_CLI_ID, which the CLI's `VERSION` and `CLI_ID` read: `cli`,
/// cli/src/main.rs and notice.rs). A side renamed alone would publish
/// nothing after a deploy, or binaries that say Cargo.toml's version or
/// never hear they are out of step, and only a release would say so.
///
/// Called, the release runs in ci.yml's run, whose artifacts are its too:
/// it downloads its builds' alone (`fragment-*`, a name no artifact of
/// ci.yml's takes) and publishes the four tarballs the install fetches by
/// name, never what else `dist` holds (a tested tree's record, a failed
/// shard's scratch, which holds directories and breaks the upload).
fn release_follows_production(workflow: &str, ci: &str, cli: &str) -> Result<()> {
    anyhow::ensure!(workflow.contains("\n  workflow_call:\n"), "release.yml is callable (workflow_call), as production's deploy calls it");
    anyhow::ensure!(ci.contains("    uses: ./.github/workflows/release.yml\n"), "ci.yml's cli-release calls release.yml");
    let build = r#"FRAGMENT_CLI_VERSION="${TAG#v}" cargo build --release -p fragment-cli"#;
    anyhow::ensure!(workflow.contains(build), "release.yml builds the tag's version in: {build}");
    anyhow::ensure!(cli.contains(r#"option_env!("FRAGMENT_CLI_VERSION")"#), "cli/src/main.rs reads its version from FRAGMENT_CLI_VERSION");
    anyhow::ensure!(ci.contains("      id: ${{ needs.cli-plan.outputs.id }}\n"), "ci.yml's cli-release passes the plan's content id");
    anyhow::ensure!(workflow.contains("          FRAGMENT_CLI_ID: ${{ needs.id.outputs.id }}\n"), "release.yml builds the content id in: FRAGMENT_CLI_ID");
    anyhow::ensure!(cli.contains(r#"option_env!("FRAGMENT_CLI_ID")"#), "cli/src/notice.rs reads its content id from FRAGMENT_CLI_ID");
    let download = "      - uses: actions/download-artifact@v4\n        with:\n          pattern: fragment-*\n";
    anyhow::ensure!(workflow.contains(download) && workflow.matches("actions/download-artifact@").count() == 1, "release.yml downloads its builds' artifacts alone: pattern: fragment-*");
    let taken: Vec<&str> = ci.lines().map(str::trim).filter(|l| l.starts_with("name: fragment-")).collect();
    anyhow::ensure!(taken.is_empty(), "ci.yml names {taken:?}, which a release it calls would take for its builds' (fragment-*)");
    anyhow::ensure!(!workflow.contains("dist/*"), "release.yml publishes the four tarballs by name, never dist/*");
    for (_, asset) in RELEASE_ASSETS {
        anyhow::ensure!(workflow.contains(&format!("dist/fragment-{asset}.tar.gz")), "release.yml publishes dist/fragment-{asset}.tar.gz by name");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build_cmd(&args[1..]),
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
        Some("cli") => cli::cli(&args[1..]),
        _ => bail!("usage: cargo xtask build [--dev] | dev [--clean] | try <template> [name] | e2e [--build-only | --no-build] [--only | --except <section>[,...] | --shard <k>/<n>] [--rehearse] | e2e --hosted --config <file> --branch <name> [--dry-run | --sweep] | check | schema-lock | secret set <name> | gen <name> | list --config <file> | deploy --config <file> [--branch <name>] [--pinned-only] | image push <image> | copy <image> --from <reference> --repository <name> --tag <tag> [--summary <file>] | teardown --config <file> --branch <name> | cli id [<rev>] | cli plan [--at <rev>] [--latest <tag>] [--github-output <file>] [--summary <file>]"),
    }
}

#[cfg(test)]
mod tests {
    /// `cargo xtask dev`'s flags: each alone and together; an unknown one,
    /// `--models` without its config, and `--models` without `--real-agent`
    /// are refused before anything builds.
    #[test]
    fn dev_takes_its_flags_and_refuses_others() {
        let args = |a: &[&str]| super::dev_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let none = args(&[]).unwrap();
        assert!(!none.clean && !none.real_agent && none.models.is_none());
        let all = args(&["--real-agent", "--clean", "--models", "preview.jsonc"]).unwrap();
        assert!(all.clean && all.real_agent && all.models.as_deref() == Some(std::path::Path::new("preview.jsonc")));
        for bad in [&["--hermes"][..], &["--real-agent", "--models"], &["--models", "preview.jsonc"]] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }

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

    /// The repo's own workflows and CLI agree; a release that is not
    /// callable, a deploy that calls none, a version or an id renamed on
    /// one side alone, or a release that would publish its caller's
    /// artifacts (an unfiltered download, `dist/*`, an asset not named, a
    /// ci.yml artifact its pattern takes) does not.
    #[test]
    fn production_releases_the_cli() {
        let read = |path: &str| std::fs::read_to_string(super::devstack::repo_root().join(path)).unwrap();
        let (workflow, ci, cli) = (read(".github/workflows/release.yml"), read(".github/workflows/ci.yml"), read("cli/src/main.rs") + &read("cli/src/notice.rs"));
        assert!(super::release_follows_production(&workflow, &ci, &cli).is_ok());
        assert!(super::release_follows_production(&workflow.replace("workflow_call:", "workflow_dispatch:"), &ci, &cli).is_err());
        assert!(super::release_follows_production(&workflow, &ci.replace("uses: ./.github/workflows/release.yml", "uses: ./.github/workflows/build.yml"), &cli).is_err());
        assert!(super::release_follows_production(&workflow.replace("FRAGMENT_CLI_VERSION=", "CLI_VERSION="), &ci, &cli).is_err());
        assert!(super::release_follows_production(&workflow, &ci, &cli.replace("\"FRAGMENT_CLI_VERSION\"", "\"CLI_VERSION\"")).is_err());
        assert!(super::release_follows_production(&workflow, &ci.replace("needs.cli-plan.outputs.id", "needs.cli-plan.outputs.tag"), &cli).is_err());
        assert!(super::release_follows_production(&workflow.replace("FRAGMENT_CLI_ID:", "CLI_ID:"), &ci, &cli).is_err());
        assert!(super::release_follows_production(&workflow, &ci, &cli.replace("\"FRAGMENT_CLI_ID\"", "\"CLI_ID\"")).is_err());
        // what a release called by ci.yml publishes: its builds' alone
        assert!(super::release_follows_production(&workflow.replace("          pattern: fragment-*\n", ""), &ci, &cli).is_err(), "every artifact of the caller's run");
        assert!(super::release_follows_production(&workflow.replace("pattern: fragment-*", "pattern: '*'"), &ci, &cli).is_err());
        assert!(super::release_follows_production(&workflow.replace(r#""${assets[@]}" --repo"#, "dist/* --repo"), &ci, &cli).is_err(), "whatever dist holds");
        assert!(super::release_follows_production(&workflow.replace(" dist/fragment-Linux-aarch64.tar.gz", ""), &ci, &cli).is_err(), "an asset the install fetches, left out");
        let scratch = ci.replace("name: e2e-scratch-${{ matrix.shard }}", "name: fragment-scratch-${{ matrix.shard }}");
        assert!(super::release_follows_production(&workflow, &scratch, &cli).is_err(), "an artifact of ci.yml's the pattern would take");
    }

    #[test]
    fn a_failed_command_never_shows_its_environment() {
        let mut cmd = std::process::Command::new("wrangler");
        cmd.args(["dev", "--port", "8790"]).env("FRAGMENT_KEYS_HOST_SECRET", "not-a-real-secret");
        assert_eq!(super::shown(&cmd), "wrangler dev --port 8790");
    }
}
