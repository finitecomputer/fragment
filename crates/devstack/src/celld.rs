//! The stack on celld (docs/self-host.md, seam 1): one `celld dev` process
//! serving the platform Worker, as `wrangler dev` does, from a config
//! rendered without what celld has no part in, and with its `.dev.vars`.
//! celld is a self-hosted Workers runtime (github.com/futurepaul/celld: its
//! `selfhost` line is upstream's release and the fork's few parity fixes).
//!
//! What celld lacks is never asked for, and the cell runs without it
//! (`DROPPED`): no Workers AI (the model route's slots on an
//! OpenAI-compatible API, or the fake's URL), no Browser Rendering (no
//! cards), no Email Sending (no mail, said once per send), no Images (no
//! icons drawn), and no Secrets Store (the cell's stand-ins for its
//! bindings, cell/secrets.mjs: `crate::Secrets::Shim`).
//!
//! The node's port is its front door's (front.rs), as on wrangler: celld
//! listens on a port of its own, which the front door learns from its
//! `ready` line and passes each connection to.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::{boot_log, front, read_config, spawn_node, Node, NodeOptions, READY_TIMEOUT};

/// What the celld render leaves out of the cell's config, and why. celld
/// refuses each of the first five at deploy (its SUPPORTED_KEYS; its
/// docs/cloudflare-compat.md); it runs `containers` (experimental), but on
/// its Docker engine with no egress intercepts or snapshots, which a
/// computer needs (docs/self-host.md, seam 2), so the render drops it and
/// the stack makes no computers.
pub const DROPPED: [(&str, &str); 7] = [
    ("ai", "Workers AI: the model route's slots run on an OpenAI-compatible API (FRAGMENT_MODELS), or on the fake (FRAGMENT_AI_URL)"),
    ("browser", "Browser Rendering: no preview cards (said once a shot, card.skipped no_browser)"),
    ("send_email", "Email Sending: no mail; each send says so and carries on (docs/technical-debt-ledger.md)"),
    ("images", "Cloudflare Images: no icons drawn (a typed refusal)"),
    ("build", "worker-build has run (cargo xtask build)"),
    ("secrets_store_secrets", "Secrets Store: the cell's stand-ins read the fleet's (cell/secrets.mjs)"),
    ("containers", "computers need the runtime's egress intercepts and snapshots, which celld's Docker engine lacks (docs/self-host.md, seam 2)"),
];

/// The project's config as celld runs it: `wrangler.celld.jsonc`, beside
/// its own (so its paths and `.dev.vars` resolve the same), less `DROPPED`.
/// It binds no secrets: celld has no Secrets Store, and the cell's
/// stand-ins take their place (cell/secrets.mjs).
pub fn celld_config(project: &Path) -> Result<PathBuf> {
    let mut config = read_config(project)?;
    let obj = config.as_object_mut().context("a wrangler config is an object")?;
    anyhow::ensure!(!obj.contains_key("secrets_store_secrets"), "{} binds no secrets of its own: on celld, the cell's stand-ins take the fleet's", project.join("wrangler.jsonc").display());
    for (key, _) in DROPPED {
        obj.remove(key);
    }
    let path = project.join("wrangler.celld.jsonc");
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(path)
}

/// A self-hosted deployment's connectors (`FRAGMENT_CONNECTORS`): the
/// cell's default, less Telegram, whose webhook needs the platform reachable
/// from the internet, which a box on a tailnet is not (docs/self-host.md).
pub fn connectors() -> String {
    let all = fragment_core::connectors::Policy::default();
    let rows: Vec<&fragment_core::connectors::Connector> = all.entries().iter().filter(|c| c.id != "telegram").collect();
    serde_json::to_string(&rows).expect("connectors serialize")
}

/// Where `celld dev` keeps a project's local state.
pub fn state_dir(project: &Path) -> PathBuf {
    project.join(".celld/dev")
}

/// What a celld stack runs on: celld itself (`CELLD_BIN`), and the esbuild
/// it bundles the cell's entry with (`CELLD_ESBUILD`, else the newest in
/// worker-build's cache).
#[derive(Debug, Clone)]
pub struct CelldTools {
    pub celld: PathBuf,
    pub esbuild: PathBuf,
}

impl CelldTools {
    pub fn locate() -> Result<CelldTools> {
        let celld = std::env::var_os("CELLD_BIN")
            .map(PathBuf::from)
            .context("set CELLD_BIN to a celld built from github.com/futurepaul/celld's selfhost line (cargo build --release -p celld)")?;
        if !celld.is_file() {
            bail!("no celld at {} (CELLD_BIN)", celld.display());
        }
        let esbuild = match std::env::var_os("CELLD_ESBUILD") {
            Some(e) => PathBuf::from(e),
            None => worker_build_esbuild()?,
        };
        if !esbuild.is_file() {
            bail!("no esbuild at {} (CELLD_ESBUILD)", esbuild.display());
        }
        Ok(CelldTools { celld, esbuild })
    }
}

/// The newest esbuild in worker-build's cache (`~/.cache/worker-build`),
/// which `cargo xtask build` fills.
fn worker_build_esbuild() -> Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    let dir = cache.join("worker-build");
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .with_context(|| format!("no worker-build cache at {} (build once with cargo xtask build, or set CELLD_ESBUILD)", dir.display()))?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("esbuild-"))
        .map(|e| e.path().join("bin/esbuild"))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop().context("no esbuild in worker-build's cache (set CELLD_ESBUILD)")
}

/// A facet's database at most this (`CELLD_FACET_MAX_BYTES`, the fork's):
/// past the cell's own cap (16 MiB), so the cell's refusal is the one met.
const FACET_MAX_BYTES: u64 = 20 << 20;

/// Requests one object may hold at once (`CELLD_MAX_CELL_REQUESTS`; celld's
/// default is 64, past which it answers 503 "cell request limit reached").
/// Cloudflare queues an object's requests with no such count, and one
/// fragment's busiest moment is past 64: its triggered runs
/// (`limits::TRIGGERED_RUNS_PER_HOUR`, 120), each a Workflow whose step
/// waits on its object at once, beside its own clients.
const CELL_REQUESTS_MAX: usize = 512;
const _: () = assert!(CELL_REQUESTS_MAX as u64 >= 2 * fragment_proto::limits::TRIGGERED_RUNS_PER_HOUR, "an object holds its triggered runs and as many clients");

/// What `celld dev` says once it serves (`  ready  http://127.0.0.1:<port>`,
/// with NO_COLOR).
fn ready_line(port: u16) -> String {
    format!("ready  http://127.0.0.1:{port}")
}

/// Starts `celld dev` on `opts.project` (its fleet configured first, as for
/// wrangler: `Fleet::configure` with `Secrets::Shim`), behind the node's
/// front door, ready once it says so. It stops on SIGINT, as Ctrl-C stops
/// it; `Node`'s stop and crash serve it as they serve wrangler's.
pub fn start(tools: &CelldTools, opts: &NodeOptions) -> Result<(Node, Duration)> {
    let config = celld_config(&opts.project)?;
    let port = opts.port();
    // celld's own port, which only the front door dials: picked and let go
    // just before celld binds it (`celld dev` takes no port 0)
    let runtime = crate::free_port()?;
    let (log, out) = boot_log(&opts.log_dir, port)?;
    let out = Arc::new(Mutex::new(out));
    let heard = Arc::new(front::Heard::default());
    let front = front::Front::serve(opts.listener.try_clone()?, heard.clone(), out.clone())?;
    let esbuild_dir = tools.esbuild.parent().context("esbuild's directory")?;
    let path = std::env::join_paths(std::iter::once(esbuild_dir.to_path_buf()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())))?;
    let mut cmd = Command::new(&tools.celld);
    cmd.arg("dev").arg(&config).args(["--host", "127.0.0.1", "--port", &runtime.to_string(), "--logs", "--no-watch"]);
    // the fork's hardening (its docs/hardening.md): a facet's database
    // capped past the cell's own, and loaded code locked down
    cmd.env("PATH", path)
        .env("CELLD_FACET_MAX_BYTES", FACET_MAX_BYTES.to_string())
        .env("CELLD_DYNAMIC_LOCKDOWN", "1")
        .env("CELLD_MAX_CELL_REQUESTS", CELL_REQUESTS_MAX.to_string())
        .env("NO_COLOR", "1");
    cmd.current_dir(&opts.project);
    let t0 = Instant::now();
    let child = spawn_node(cmd, opts, out, heard.clone())?;
    let mut node = Node { child, own_group: opts.own_group, what: "celld dev", base: format!("http://127.0.0.1:{port}"), port, log: log.clone(), reaped: false, _front: front };
    let ready = ready_line(runtime);
    loop {
        let text = fs::read(&log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if text.contains(&ready) {
            break;
        }
        if crate::exited_unreaped(node.child.id(), false)? {
            let status = node.reap()?;
            bail!("celld dev exited ({status}) before it was ready:\n{text}");
        }
        if t0.elapsed() > READY_TIMEOUT {
            bail!("celld dev on :{runtime} was not ready after {READY_TIMEOUT:?}:\n{text}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    heard.runtime_at(runtime);
    Ok((node, t0.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a self-hosted stack's connectors are the default's but
    /// Telegram, and the cell takes them as it takes a deployment's.
    #[test]
    fn a_self_hosted_stack_links_no_telegram() {
        let policy = fragment_core::connectors::Policy::parse(&connectors()).unwrap();
        let ids: Vec<&str> = policy.entries().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["simplex"]);
        assert_eq!(policy.entries()[0], fragment_core::connectors::Policy::default().entries()[0], "SimpleX as the default names it");
    }

    /// Goal: the rendered config keeps every binding celld runs and drops
    /// each key it has no part in, whatever the source's comments; one that
    /// binds secrets of its own is refused (the stand-ins take the fleet's).
    /// Method: cell/wrangler.jsonc itself, then one with a store binding.
    #[test]
    fn renders_what_celld_runs() {
        let dir = std::env::temp_dir().join(format!("celld-config-{}", crate::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        fs::copy(crate::cell_dir().join("wrangler.jsonc"), dir.join("wrangler.jsonc")).unwrap();
        let source = read_config(&dir).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(celld_config(&dir).unwrap()).unwrap()).unwrap();
        for (k, _) in DROPPED {
            assert!(v.get(k).is_none(), "{k} dropped");
        }
        for (k, kept) in source.as_object().unwrap() {
            if !DROPPED.iter().any(|(d, _)| d == k) {
                assert_eq!(v.get(k), Some(kept), "{k} kept as it is");
            }
        }
        for k in ["assets", "worker_loaders", "workflows", "r2_buckets", "queues", "durable_objects", "migrations"] {
            assert!(v.get(k).is_some(), "{k} kept");
        }
        fs::write(dir.join("wrangler.jsonc"), r#"{ "name": "x", "main": "a.mjs", "secrets_store_secrets": [] }"#).unwrap();
        assert!(celld_config(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
