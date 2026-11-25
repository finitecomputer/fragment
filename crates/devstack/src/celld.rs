//! The stack on celld (docs/self-host.md, seam 1): one `celld dev` process
//! serving the platform Worker, as `wrangler dev` does, from the config a
//! local node runs (`crate::local_config`), and with its `.dev.vars`.
//! celld is a self-hosted Workers runtime (github.com/finitecomputer/celld: its
//! `selfhost` line is upstream's release and the fork's parity fixes).
//!
//! celld presents Cloudflare's binding surface, so the cell runs on it as
//! it runs on Cloudflare, with no code of celld's own. It binds every key of
//! the config: the Secrets Store's secrets, read from the node's secrets
//! backend (`secrets_file`: a file of their values, or OpenBao); and the
//! services it does not run (Email Sending, Images, Browser Rendering,
//! Workers AI), each with workerd's class, every call rejecting, saying the
//! node has none. Its computers run on the container engine
//! `CELLD_CONTAINER_ENGINE` names (`Engine`): this machine's sandcastle
//! engine, a microVM per container with the egress intercepts and
//! snapshots a computer needs, or none, where the `Computer` class's
//! container is there, not running, and a start throws (docs/self-host.md,
//! seam 2).
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

use crate::{boot_log, front, spawn_node, Node, NodeOptions, READY_TIMEOUT};

/// What the celld render leaves out of the local config, and why.
pub const DROPPED: [(&str, &str); 2] = [
    ("build", "worker-build has run (cargo xtask build)"),
    // the cell then answers the admin's routes with a host failure saying
    // so (cell/src/admin.rs `forward`; docs/technical-debt-ledger.md)
    ("services", "celld runs one Worker, the cell: not the admin Worker beside it"),
];

/// The config celld runs: `wrangler.celld.jsonc`, beside the project's own
/// (so its paths and `.dev.vars` resolve the same), the local config a node
/// runs (`crate::local_config`, its secrets bound by name as on wrangler)
/// less `DROPPED`. `Fleet::configure` writes the local config first.
pub fn celld_config(project: &Path) -> Result<PathBuf> {
    let local = crate::local_config(project);
    let text = fs::read_to_string(&local).with_context(|| format!("read {} (Fleet::configure writes it)", local.display()))?;
    let mut config: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("parse {}", local.display()))?;
    let obj = config.as_object_mut().context("a wrangler config is an object")?;
    for (key, _) in DROPPED {
        obj.remove(key);
    }
    let path = project.join("wrangler.celld.jsonc");
    fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(path)
}

/// The celld node's secrets file (its `CELLD_SECRETS_FILE`), beside the
/// project's `.dev.vars`: its backend, `vars` with the secrets' values, or
/// `openbao` with where OpenBao is and the path of the token that reads it
/// (`crate::Secrets`). The node's configuration, never the Worker's
/// variables.
pub fn secrets_file(project: &Path) -> PathBuf {
    project.join(".celld.secrets.json")
}

/// Writes `config` as the node's secrets file (`secrets_file`), mode 600
/// (celld refuses one others may read), replacing the one before whole. The
/// file it writes first (`.json.new`) holds the same secrets: it is removed
/// when a step fails, and .gitignore covers it for a process killed between
/// the two.
pub fn write_secrets_file(project: &Path, config: &serde_json::Value) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let path = secrets_file(project);
    let tmp = path.with_extension("json.new");
    let _ = fs::remove_file(&tmp);
    let written = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(serde_json::to_string(config)?.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &path).with_context(|| format!("move {} to {}", tmp.display(), path.display()))?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
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

/// The celld node's container engine, from celld's own setting
/// `CELLD_CONTAINER_ENGINE` (docs/self-host.md, seam 2): `none`, the
/// default, where it runs no computers; or `krun:<socket>`, this machine's
/// sandcastle engine (its `engine.sock`), where each computer is a microVM
/// with the egress intercepts and snapshots the cell's computers use, and
/// `celld dev` loads into it the images it builds. celld's Docker engine has
/// neither intercepts nor snapshots, so it is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engine {
    None,
    Krun(PathBuf),
}

impl Engine {
    pub const VAR: &'static str = "CELLD_CONTAINER_ENGINE";

    pub fn parse(value: Option<&str>) -> Result<Engine> {
        match value {
            None | Some("" | "none") => Ok(Engine::None),
            Some(v) => match v.strip_prefix("krun:") {
                Some(socket) if Path::new(socket).is_absolute() => Ok(Engine::Krun(PathBuf::from(socket))),
                _ if v == "docker" => bail!("{} is none or krun:<socket>: celld's Docker engine has no egress intercepts or snapshots, which a computer needs (docs/self-host.md, seam 2)", Self::VAR),
                _ => bail!("{} is none or krun:<the absolute path of the engine's socket>, not {v:?}", Self::VAR),
            },
        }
    }

    /// Whether the node runs computers.
    pub fn computers(&self) -> bool {
        matches!(self, Engine::Krun(_))
    }

    /// The value the node is given.
    pub fn value(&self) -> String {
        match self {
            Engine::None => "none".into(),
            Engine::Krun(socket) => format!("krun:{}", socket.display()),
        }
    }

    /// Checks that the engine answers its health (`GET /v1/health`), so a
    /// stopped engine or a socket this user may not open stops the run at
    /// once, naming it, and not at a computer's first start.
    fn check(&self) -> Result<()> {
        let Engine::Krun(socket) = self else { return Ok(()) };
        match engine_call(socket, "GET", "/v1/health") {
            Ok((200, _)) => Ok(()),
            Ok((status, body)) => bail!("the krun engine at {} answers its health with {status}: {}", socket.display(), String::from_utf8_lossy(&body)),
            Err(e) => bail!("the krun engine at {} ({}): {e:#} (is its service running, and is this user its client?)", socket.display(), Self::VAR),
        }
    }

    /// Destroys the containers a `celld dev` node on `project` left on the
    /// krun engine, and answers how many: a node that stops reaps its own,
    /// and one that starts again on the project reaps what it left, but a
    /// run killed outright (a signal's `kill_started`) left its node no
    /// chance to, and the engine keeps a VM until it is told. Only this
    /// project's node's containers are touched (`container_prefix`): the
    /// engine serves every stack and person on the machine. So is celld's
    /// image builder when the celld that started it is gone (`orphan_builder`):
    /// a run killed while its node built images left it holding the
    /// engine's memory.
    pub fn remove(&self, project: &Path) -> Result<u32> {
        let Engine::Krun(socket) = self else { return Ok(0) };
        let prefix = container_prefix(&dev_node(project)?);
        let (status, body) = engine_call(socket, "GET", "/v1/containers")?;
        anyhow::ensure!(status == 200, "the krun engine's containers: {status} {}", String::from_utf8_lossy(&body));
        let listed: serde_json::Value = serde_json::from_slice(&body).context("the krun engine's containers are JSON")?;
        let mut removed = 0;
        // bounded: the engine's containers (at most its VMs)
        for name in listed.as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).filter(|n| n.starts_with(&prefix)) {
            let (status, body) = engine_call(socket, "POST", &format!("/v1/containers/{name}/destroy"))?;
            anyhow::ensure!(status == 200 || status == 404, "the krun engine's destroy of {name}: {status} {}", String::from_utf8_lossy(&body));
            removed += 1;
        }
        // On the builder's lock: a session holding it runs, and reaps a
        // builder left before it itself; one that starts after this waits.
        if let Some(_turn) = builder_turn(socket)? {
            let (status, body) = engine_call(socket, "GET", &format!("/v1/containers/{BUILDER}"))?;
            if status == 200 {
                let builder: serde_json::Value = serde_json::from_slice(&body).context("the krun engine's builder is JSON")?;
                if orphan_builder(&builder, proc_start) {
                    let (status, body) = engine_call(socket, "POST", &format!("/v1/containers/{BUILDER}/destroy"))?;
                    anyhow::ensure!(status == 200 || status == 404, "the krun engine's destroy of {BUILDER}: {status} {}", String::from_utf8_lossy(&body));
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

/// celld's image builder on the krun engine (the fork's krun_build.rs):
/// one VM by this name, its owner in the label of the same name as
/// `<pid>-<start>`, the celld process and its start time (`/proc/<pid>/stat`
/// field 22), so a reused pid is told apart.
const BUILDER: &str = "celld-builder";

/// The lock celld's build sessions on the engine at `socket` take turns
/// by (the fork's krun_build.rs `lock_path`, `engine_id`):
/// `celld-builder-<uid>-<12 hex of the SHA-256 of the socket's canonical
/// path>.lock`, in `/run/user/<uid>` when there is one, else `/tmp`.
fn builder_lock(socket: &Path, uid: u32, runtime: Option<&Path>) -> PathBuf {
    use sha2::Digest as _;
    let canonical = fs::canonicalize(socket).unwrap_or_else(|_| socket.to_path_buf());
    let digest = sha2::Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let name = format!("celld-builder-{uid}-{}.lock", &format!("{digest:x}")[..12]);
    runtime.unwrap_or(Path::new("/tmp")).join(name)
}

/// The builder's lock, taken when no session holds it; `None` while one
/// does.
fn builder_turn(socket: &Path) -> Result<Option<fs::File>> {
    use std::os::unix::fs::MetadataExt as _;
    let uid = fs::metadata("/proc/self").context("this process's user")?.uid();
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let path = builder_lock(socket, uid, runtime.is_dir().then_some(runtime.as_path()));
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("lock {}", path.display())),
    }
}

/// Whether `builder` (the engine's view of it) belongs to a celld that is
/// gone: its owner's pid runs no more, or runs a process started at
/// another time (`start` reads a pid's start). One with no owner is left.
fn orphan_builder(builder: &serde_json::Value, start: impl Fn(u32) -> Option<u64>) -> bool {
    let Some((pid, at)) = builder["labels"][BUILDER].as_str().and_then(|o| o.split_once('-')) else { return false };
    match (pid.parse::<u32>(), at.parse::<u64>()) {
        (Ok(pid), Ok(at)) => start(pid) != Some(at),
        _ => false,
    }
}

/// A running process's start time, in clock ticks since boot:
/// `/proc/<pid>/stat`'s field 22, after its command's parenthesis. None
/// for one that has exited, a zombie (`Z`) or dead (`X`) included: a
/// celld a run killed and has not reaped yet still has its start there.
fn proc_start(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    if matches!(fields.next()?, "Z" | "X") {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

/// The name `celld dev` gives its node on `project` (the fork's dev.rs):
/// `dev-` and the first 12 hex digits of the SHA-256 of the project's
/// canonical path. It names the node's containers (`container_prefix`).
fn dev_node(project: &Path) -> Result<String> {
    let project = fs::canonicalize(project).with_context(|| format!("resolve {}", project.display()))?;
    Ok(dev_node_of(&project))
}

/// `dev_node` of a path already canonical.
fn dev_node_of(canonical: &Path) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    format!("dev-{}", &format!("{digest:x}")[..12])
}

/// The start of the name of every container of celld's node `node` (the
/// fork's container.rs `node_prefix`): `celld-`, the first 6 hex digits of
/// the SHA-256 of the node's name, and `-`. Leans on celld's naming at the
/// pinned version (docs/technical-debt-ledger.md).
fn container_prefix(node: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(node.as_bytes());
    format!("celld-{}-", &format!("{digest:x}")[..6])
}

/// An engine answer, at most: its container list is at most its VMs'.
const ENGINE_ANSWER_BYTES_MAX: u64 = 1 << 20;

/// One call to the krun engine's API on its unix socket (a body-less
/// request; a POST sends `{}`), within 30 s: its status and body.
fn engine_call(socket: &Path, method: &str, path: &str) -> Result<(u16, Vec<u8>)> {
    use std::io::{Read as _, Write as _};
    let mut c = std::os::unix::net::UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    c.set_read_timeout(Some(Duration::from_secs(30)))?;
    c.set_write_timeout(Some(Duration::from_secs(30)))?;
    let body: &[u8] = if method == "POST" { b"{}" } else { b"" };
    write!(c, "{method} {path} HTTP/1.1\r\nhost: engine\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n", body.len())?;
    c.write_all(body)?;
    let mut answer = vec![];
    c.take(ENGINE_ANSWER_BYTES_MAX).read_to_end(&mut answer)?;
    let end = answer.windows(4).position(|w| w == b"\r\n\r\n").context("the engine's answer has a head")?;
    let head = String::from_utf8_lossy(&answer[..end]).to_ascii_lowercase();
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).context("the engine's answer has a status")?;
    let rest = answer[end + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        return Ok((status, unchunk(&rest)?));
    }
    Ok((status, rest))
}

/// A chunked body, joined; one cut short is refused.
fn unchunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![];
    // bounded by the body: each pass consumes one chunk
    loop {
        let line = b.windows(2).position(|w| w == b"\r\n").context("a chunk's size line")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..line]).split(';').next().unwrap_or("").trim(), 16).context("a chunk's size")?;
        b = &b[line + 2..];
        if size == 0 {
            return Ok(out);
        }
        anyhow::ensure!(b.len() >= size + 2, "a chunk shorter than its size");
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

/// What a celld stack runs on: celld itself (`CELLD_BIN`), the esbuild it
/// bundles the cell's entry with (`CELLD_ESBUILD`, else the newest in
/// worker-build's cache), and the engine its computers run on (`Engine`).
#[derive(Debug, Clone)]
pub struct CelldTools {
    pub celld: PathBuf,
    pub esbuild: PathBuf,
    pub engine: Engine,
}

impl CelldTools {
    pub fn locate() -> Result<CelldTools> {
        let engine = Engine::parse(std::env::var(Engine::VAR).ok().as_deref())?;
        engine.check()?;
        let celld = std::env::var_os("CELLD_BIN")
            .map(PathBuf::from)
            .context("set CELLD_BIN to a celld built from github.com/finitecomputer/celld's selfhost line (cargo build --release -p celld)")?;
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
        Ok(CelldTools { celld, esbuild, engine })
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
/// wrangler: `Fleet::configure` with `Secrets::Vars` or `Secrets::OpenBao`,
/// which writes its secrets file), behind the node's front door, ready once
/// it says so. It stops on SIGINT, as Ctrl-C stops it; `Node`'s stop and
/// crash serve it as they serve wrangler's.
pub fn start(tools: &CelldTools, opts: &NodeOptions) -> Result<(Node, Duration)> {
    let config = celld_config(&opts.project)?;
    let secrets = secrets_file(&opts.project);
    anyhow::ensure!(secrets.is_file(), "no secrets file for the celld node at {} (Fleet::configure writes it, with Secrets::Vars or Secrets::OpenBao)", secrets.display());
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
        // the node's own settings: its secrets backend, and its computers'
        // engine (seam 2): with none, its deploy builds no image
        .env("CELLD_SECRETS_FILE", &secrets)
        .env(Engine::VAR, tools.engine.value())
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

    /// Goal: the node's engine is none unless named, the krun engine by its
    /// socket's absolute path, given back as celld takes it; celld's Docker
    /// engine (no intercepts, no snapshots) and anything else are refused,
    /// saying what is taken. Replay: parsed again, the same.
    #[test]
    fn the_engine_is_none_or_krun() {
        for _ in 0..2 {
            for none in [None, Some(""), Some("none")] {
                let e = Engine::parse(none).unwrap();
                assert_eq!((e.clone(), e.computers(), e.value()), (Engine::None, false, "none".to_string()));
            }
            let krun = Engine::parse(Some("krun:/var/lib/sandcastle/engine.sock")).unwrap();
            assert_eq!(krun, Engine::Krun(PathBuf::from("/var/lib/sandcastle/engine.sock")));
            assert!(krun.computers());
            assert_eq!(krun.value(), "krun:/var/lib/sandcastle/engine.sock");
            let e = Engine::parse(Some("docker")).unwrap_err().to_string();
            assert!(e.contains("intercepts") && e.contains("snapshots"), "{e}");
            for bad in ["krun:", "krun:engine.sock", "podman", "krun"] {
                let e = Engine::parse(Some(bad)).unwrap_err().to_string();
                assert!(e.contains("krun:<") && e.contains(bad), "{bad}: {e}");
            }
        }
    }

    /// Goal: a project's celld node and its containers are named as celld
    /// names them (values from the fork's dev.rs and container.rs, for this
    /// path), so `remove` finds that project's containers and no other's.
    #[test]
    fn a_projects_containers_are_named_as_celld_names_them() {
        assert_eq!(dev_node_of(Path::new("/srv/fragment/target/e2e/0123456789/cell")), "dev-3d91734ffb1d");
        assert_eq!(container_prefix("dev-3d91734ffb1d"), "celld-0f30ad-");
        let dir = std::env::temp_dir().join(format!("celld-names-{}", crate::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        let node = dev_node(&dir).unwrap();
        assert!(node.starts_with("dev-") && node.len() == 16, "{node}");
        assert_eq!(dev_node(&dir.join(".")).unwrap(), node, "the canonical path's");
        let other = dir.join("other");
        fs::create_dir_all(&other).unwrap();
        assert_ne!(container_prefix(&dev_node(&other).unwrap()), container_prefix(&node));
        assert!(dev_node(&dir.join("missing")).is_err());
        assert_eq!(Engine::None.remove(&dir).unwrap(), 0, "no engine, nothing to remove");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal: an engine's chunked answer is joined; one cut short is refused.
    #[test]
    fn chunked_answers() {
        assert_eq!(unchunk(b"4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\n\r\n").unwrap(), b"Wikipedia");
        assert!(unchunk(b"9\r\nWiki\r\n").is_err());
        assert!(unchunk(b"zz\r\n").is_err());
    }

    /// Goal: a krun engine that does not answer stops a stack at once,
    /// naming its socket; no engine needs no check.
    #[test]
    fn an_engine_that_does_not_answer_is_refused() {
        assert!(Engine::None.check().is_ok());
        let missing = std::env::temp_dir().join(format!("no-engine-{}.sock", crate::random_hex(6)));
        let e = Engine::Krun(missing.clone()).check().unwrap_err().to_string();
        assert!(e.contains(&missing.display().to_string()) && e.contains(Engine::VAR), "{e}");
    }

    /// Goal: a self-hosted stack's connectors are the default's but
    /// Telegram, and the cell takes them as it takes a deployment's.
    #[test]
    fn a_self_hosted_stack_links_no_telegram() {
        let policy = fragment_core::connectors::Policy::parse(&connectors()).unwrap();
        let ids: Vec<&str> = policy.entries().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["simplex"]);
        assert_eq!(policy.entries()[0], fragment_core::connectors::Policy::default().entries()[0], "SimpleX as the default names it");
    }

    /// Goal: celld runs the local config a wrangler node runs, its secrets
    /// bound by name, less `build` and the binding to the admin Worker
    /// (`DROPPED`): the services it does not run and
    /// the computers' containers stay in it, for celld binds them all. With
    /// no local config written yet, it is refused, naming the file. Replay:
    /// rendered twice, the same. Method: cell/wrangler.jsonc, through
    /// `write_local_config` with a secret bound.
    fn json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Goal: a builder whose celld is gone is told apart from a live one's,
    /// as the fork labels it. Valid: a live owner's is kept; a dead pid's,
    /// or a reused pid's (another start), goes. Invalid: no label, or one
    /// not of the form, is kept: it is not known to be an orphan.
    #[test]
    fn an_orphan_builder_is_one_whose_celld_is_gone() {
        let mine = std::process::id();
        let started = super::proc_start(mine).expect("this process's start");
        let builder = |owner: &str| serde_json::json!({ "name": super::BUILDER, "labels": { super::BUILDER: owner } });
        assert!(!super::orphan_builder(&builder(&format!("{mine}-{started}")), super::proc_start), "a live celld's");
        assert!(super::orphan_builder(&builder(&format!("{mine}-{}", started + 1)), super::proc_start), "a reused pid");
        assert!(super::orphan_builder(&builder("4194305-1"), super::proc_start), "past pid_max: gone");
        assert!(!super::orphan_builder(&serde_json::json!({ "labels": {} }), super::proc_start), "no owner");
        assert!(!super::orphan_builder(&builder("x"), super::proc_start));
    }

    /// Goal: a celld killed and not yet reaped (a zombie, its start time
    /// still in `/proc`) is gone: a run's Ctrl-C kills its node, then
    /// reaps the builder before it waits for the node.
    #[test]
    fn a_zombie_owner_is_gone() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        let stat = || fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let t0 = std::time::Instant::now();
        // bounded: `true` exits at once
        while !stat().rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')) {
            assert!(t0.elapsed() < Duration::from_secs(10), "the child is a zombie");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(super::proc_start(pid), None, "a zombie runs no more");
        child.wait().unwrap();
        assert_eq!(super::proc_start(pid), None);
    }

    /// Goal: devstack takes the builder's lock where celld does, so a
    /// session that holds it keeps its builder. Method: the fork's naming,
    /// a symlink to the socket naming the same lock, and the lock held.
    #[test]
    fn the_builders_lock_is_celld_s() {
        let dir = std::env::temp_dir().join(format!("celld-lock-{}", crate::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("engine.sock");
        fs::write(&socket, "").unwrap();
        std::os::unix::fs::symlink(&socket, dir.join("link.sock")).unwrap();
        let lock = super::builder_lock(&socket, 1000, Some(&dir));
        assert_eq!(lock, super::builder_lock(&dir.join("link.sock"), 1000, Some(&dir)));
        let name = lock.file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.starts_with("celld-builder-1000-") && name.ends_with(".lock") && name.len() == "celld-builder-1000-".len() + 12 + 5, "{name}");
        assert!(super::builder_lock(&socket, 1000, None).starts_with("/tmp"));
        let held = fs::File::create(&lock).unwrap();
        held.try_lock().unwrap();
        let other = fs::OpenOptions::new().write(true).open(&lock).unwrap();
        assert!(matches!(other.try_lock(), Err(fs::TryLockError::WouldBlock)), "a session holds it: the builder is kept");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn renders_the_local_config_less_build() {
        let dir = std::env::temp_dir().join(format!("celld-config-{}", crate::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        // as a staged copy is: its images' Dockerfiles the source tree's
        let mut config = crate::read_config(&crate::cell_dir()).unwrap();
        crate::absolute_images(&mut config, &crate::cell_dir()).unwrap();
        fs::write(dir.join("wrangler.jsonc"), config.to_string()).unwrap();
        fs::copy(crate::cell_dir().join(crate::ADMIN_CONFIG), dir.join(crate::ADMIN_CONFIG)).unwrap();
        let e = celld_config(&dir).unwrap_err().to_string();
        assert!(e.contains("wrangler.local.jsonc"), "{e}");
        crate::write_local_config(&dir, &[("HOST_SECRET".to_string(), "fragment-host-secret")], &[], &[], true, false, false).unwrap();
        let local = json(&crate::local_config(&dir));
        for _ in 0..2 {
            let v = json(&celld_config(&dir).unwrap());
            assert!(v.get("build").is_none() && v.get("services").is_none(), "build and the admin's binding dropped");
            assert!(local.get("services").is_some(), "which the local config has");
            for (k, kept) in local.as_object().unwrap() {
                if !DROPPED.iter().any(|(d, _)| d == k) {
                    assert_eq!(v.get(k), Some(kept), "{k} kept as it is");
                }
            }
            for k in ["assets", "worker_loaders", "workflows", "r2_buckets", "queues", "durable_objects", "migrations", "send_email", "images", "browser", "containers", "secrets_store_secrets"] {
                assert!(v.get(k).is_some(), "{k} kept");
            }
            assert_eq!(v["secrets_store_secrets"][0]["secret_name"], "fragment-host-secret");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal: the node's secrets file is its owner's alone (celld refuses
    /// one others may read), written whole, and replaced whole.
    #[test]
    fn the_secrets_file_is_owner_only_and_replaced_whole() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("celld-secrets-{}", crate::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        for value in ["one", "two"] {
            write_secrets_file(&dir, &serde_json::json!({ "backend": "vars", "values": { "s": value } })).unwrap();
            let path = secrets_file(&dir);
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(json(&path)["values"]["s"], value);
            assert!(!path.with_extension("json.new").exists());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal: a write that fails leaves no file of the secrets behind (the
    /// review's P2 on #1256): here the move fails, the file's place being a
    /// directory, and the `.json.new` it wrote first is gone.
    #[test]
    fn a_failed_secrets_write_leaves_no_secrets_behind() {
        let dir = std::env::temp_dir().join(format!("celld-secrets-{}", crate::random_hex(6)));
        let path = secrets_file(&dir);
        fs::create_dir_all(path.join("in-the-way")).unwrap();
        let e = write_secrets_file(&dir, &serde_json::json!({ "backend": "vars", "values": { "s": "dummy-secret" } })).unwrap_err();
        assert!(format!("{e:#}").contains("move"), "{e:#}");
        assert!(!path.with_extension("json.new").exists(), "no secrets left beside the file");
        fs::remove_dir_all(&dir).unwrap();
    }
}
