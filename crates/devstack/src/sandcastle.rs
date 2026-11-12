//! Computers on sandcastle nodes (docs/self-host.md, seam 2), for dev and
//! the e2e: the deployment's node list, read from a file (`NodesFile`), and
//! a node started here (`SandcastleNode`): a `sandcastle-node` in front of
//! this machine's sandcastle engine, already running under its service
//! (root, every container a jailed microVM). Only the node runs as this
//! user; the engine serves one client uid, which must be this one.
//!
//! The images computers run are built here, loaded into the engine
//! (`load_images`), and named by the digest the engine gives each, as a
//! deployment pins its images (docs/computers.md, "Stable and canary";
//! fragment_core::placement refuses any other). The containers a node
//! started here carry its run's label (`RUN_LABEL`), and only those are
//! destroyed when it stops (`destroy_run`): the engine stays up, and so do
//! the containers other nodes in front of it started (another stack's, a
//! person's own).

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use fragment_core::placement::{self, Nodes};
use serde_json::{json, Value};

/// The `sandcastle-node` binary (sandcastle's `node` branch, built
/// `cargo build --release -p sandcastle-node`, or as installed beside the
/// engine).
pub const NODE_BIN_VAR: &str = "SANDCASTLE_NODE";
/// The engine's state directory, where its sockets are (its config's
/// `state_dir`; `/var/lib/sandcastle` unless named).
pub const ENGINE_DIR_VAR: &str = "SANDCASTLE_ENGINE_DIR";
pub const ENGINE_DIR_DEFAULT: &str = "/var/lib/sandcastle";
/// The dev stack's node list (`NodesFile`).
pub const NODES_FILE_VAR: &str = "FRAGMENT_NODES_FILE";
/// A node's API must take a connection within this of its start.
pub const NODE_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// A node's stop must finish within this; past it the process is killed.
pub const NODE_STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// A node's secret, made here: 32 random bytes, as hex.
const SECRET_HEX_BYTES: usize = 32;
/// The label every container a node started here carries (its config's
/// `labels`, sandcastle's docs/node.md), naming the run or stack: on an
/// engine other nodes front too, what tells its containers from theirs.
pub const RUN_LABEL: &str = "fragment-run";
/// A unix socket's path, at most this many bytes: Linux's `sun_path` holds
/// 108, the last a NUL. A node handed a longer one cannot bind it, and says
/// so only in its own log.
pub const SOCKET_PATH_MAX: usize = 107;
/// The most one answer from the engine's API may be (its image list).
const ENGINE_ANSWER_BYTES_MAX: u64 = 16 << 20;
/// An engine call's limit: an image's load builds its disk.
const ENGINE_CALL_TIMEOUT: Duration = Duration::from_secs(900);

/// `dir/name`, the path of a socket a node binds or dials, refused when it
/// is too long to be one (a deep checkout's scratch).
pub fn socket_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let len = path.as_os_str().len();
    if len > SOCKET_PATH_MAX {
        bail!("{} is {len} bytes, past a unix socket's {SOCKET_PATH_MAX}: name its directory shorter", path.display());
    }
    Ok(path)
}

/// The deployment's nodes as the cell reads them: `FRAGMENT_NODES`, and
/// each node's secret, by its id (the cell reads it through the binding
/// `placement::secret_binding` names, of the store's `secret_store_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodesVars {
    pub nodes: String,
    /// (node id, its secret)
    pub secrets: Vec<(String, String)>,
}

impl NodesVars {
    /// The image new computers are pinned to: `hermes` when the list names
    /// it, else `stub`, else its first.
    pub fn default_image(&self) -> Result<String> {
        let nodes = Nodes::parse(&self.nodes).map_err(|e| anyhow::anyhow!("{e}"))?;
        let names = nodes.image_names();
        Ok(["hermes", "stub"].into_iter().find(|n| names.contains(n)).or(names.first().copied()).context("a node list names its images")?.to_string())
    }
}

/// The store name a node's secret is kept under (OpenBao, wrangler's
/// store), bound as `placement::secret_binding`.
pub fn secret_store_name(id: &str) -> String {
    format!("fragment-node-{id}-secret")
}

/// The dev stack's node list (`FRAGMENT_NODES_FILE`): `FRAGMENT_NODES`'
/// shape (fragment_core::placement), each node with its secret's file
/// beside it (`"secret_file"`), which is read here and never written into
/// the variable.
pub struct NodesFile;

impl NodesFile {
    pub fn read(path: &Path) -> Result<NodesVars> {
        let text = fs::read_to_string(path).with_context(|| format!("read {} ({NODES_FILE_VAR})", path.display()))?;
        let mut v: Value = serde_json::from_str(&text).with_context(|| format!("{} is FRAGMENT_NODES' JSON, each node with its \"secret_file\"", path.display()))?;
        let mut secrets = vec![];
        for n in v["nodes"].as_array_mut().context("a node list names its \"nodes\"")? {
            let obj = n.as_object_mut().context("a node is an object")?;
            let id = obj.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            let file = obj.remove("secret_file").and_then(|f| f.as_str().map(PathBuf::from)).with_context(|| format!("the node {id:?} names its \"secret_file\""))?;
            let secret = fs::read_to_string(&file).with_context(|| format!("read the node {id}'s secret_file {}", file.display()))?.trim().to_string();
            if secret.len() < placement::SECRET_BYTES_MIN {
                bail!("the node {id}'s secret ({}) is at least {} bytes", file.display(), placement::SECRET_BYTES_MIN);
            }
            secrets.push((id, secret));
        }
        let nodes = v.to_string();
        // checked as the cell checks it, so a bad list stops here
        Nodes::parse(&nodes).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(NodesVars { nodes, secrets })
    }
}

/// The node binary, from `SANDCASTLE_NODE`.
pub fn node_bin() -> Result<PathBuf> {
    let bin = std::env::var_os(NODE_BIN_VAR).map(PathBuf::from).with_context(|| format!("set {NODE_BIN_VAR} to sandcastle-node (sandcastle's node branch: cargo build --release -p sandcastle-node)"))?;
    anyhow::ensure!(bin.is_file(), "no {} ({NODE_BIN_VAR})", bin.display());
    Ok(bin)
}

/// The engine's state directory, from `SANDCASTLE_ENGINE_DIR`.
pub fn engine_dir() -> PathBuf {
    std::env::var_os(ENGINE_DIR_VAR).map_or_else(|| PathBuf::from(ENGINE_DIR_DEFAULT), PathBuf::from)
}

/// A node to start: its id, port and state, in front of the engine at
/// `engine` (its state directory).
pub struct NodeSpec {
    pub id: String,
    /// The loopback port it listens on.
    pub port: u16,
    pub engine: PathBuf,
    /// Its state: its config, its secret, its egress socket.
    pub dir: PathBuf,
    /// The platform's origin: `/api/nodes/egress`.
    pub platform: String,
    /// Where its log goes (`sandcastle-<id>-node.log`).
    pub log_dir: PathBuf,
    /// The run or stack it serves: each container it starts carries it as
    /// `RUN_LABEL`.
    pub run: String,
}

/// The node's config (sandcastle's crates/node/src/config.rs).
fn node_config(spec: &NodeSpec) -> Result<Value> {
    Ok(json!({
        "listen": format!("127.0.0.1:{}", spec.port),
        "engine": socket_path(&spec.engine, "engine.sock")?,
        "ports": socket_path(&spec.engine, "ports.sock")?,
        "egress": socket_path(&spec.dir, "egress.sock")?,
        "secret_file": spec.dir.join("node.secret"),
        "platform": spec.platform,
        "labels": { RUN_LABEL: spec.run },
    }))
}

/// A node started here, in front of the machine's engine.
pub struct SandcastleNode {
    pub id: String,
    pub port: u16,
    /// Its secret, which the platform holds too.
    pub secret: String,
    /// The run its containers carry as `RUN_LABEL`.
    pub run: String,
    engine_sock: PathBuf,
    log: PathBuf,
    child: Option<crate::Started>,
}

impl SandcastleNode {
    /// Checks the engine answers, then starts the node, and waits until its
    /// port takes a connection.
    pub fn start(bin: &Path, spec: &NodeSpec) -> Result<SandcastleNode> {
        assert!(placement::valid_node_id(&spec.id), "a node's id is the platform's form");
        assert!(!spec.run.is_empty() && spec.run.len() <= 63, "a node's containers carry its run's name, a label's value");
        let engine_sock = socket_path(&spec.engine, "engine.sock")?;
        let (status, body) = engine_call(&engine_sock, "GET", "/v1/health", None).with_context(|| format!("the engine at {} (is its service running, and is this user its client?)", engine_sock.display()))?;
        if status != 200 {
            bail!("the engine's health at {}: {status} {}", engine_sock.display(), String::from_utf8_lossy(&body));
        }
        fs::create_dir_all(&spec.dir).with_context(|| format!("create {}", spec.dir.display()))?;
        fs::create_dir_all(&spec.log_dir)?;
        let secret = crate::random_hex(SECRET_HEX_BYTES);
        let mut f = fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(spec.dir.join("node.secret"))?;
        std::io::Write::write_all(&mut f, secret.as_bytes())?;
        let config = spec.dir.join("node.json");
        fs::write(&config, serde_json::to_string_pretty(&node_config(spec)?)?)?;
        let log = spec.log_dir.join(format!("sandcastle-{}-node.log", spec.id));
        let out = fs::OpenOptions::new().create(true).append(true).open(&log)?;
        let child = crate::spawn(Command::new(bin).arg("serve").arg("--config").arg(&config).stdin(Stdio::null()).stdout(out.try_clone()?).stderr(out)).with_context(|| format!("start {}", bin.display()))?;
        let mut node = SandcastleNode { id: spec.id.clone(), port: spec.port, secret, run: spec.run.clone(), engine_sock, log, child: Some(child) };
        let t0 = Instant::now();
        // bounded by NODE_READY_TIMEOUT
        while std::net::TcpStream::connect(("127.0.0.1", node.port)).is_err() {
            if let Some(status) = node.child.as_mut().map(|c| c.try_wait()).transpose()?.flatten() {
                node.child = None;
                bail!("sandcastle-node {} exited ({status}) before its port answered:\n{}", node.id, node.log_text());
            }
            if t0.elapsed() > NODE_READY_TIMEOUT {
                bail!("sandcastle-node {}'s port did not answer within {NODE_READY_TIMEOUT:?}:\n{}", node.id, node.log_text());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(node)
    }

    /// Its own log.
    pub fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Its row in `FRAGMENT_NODES`.
    pub fn listing(&self) -> Value {
        json!({ "id": self.id, "url": format!("http://127.0.0.1:{}", self.port), "arch": std::env::consts::ARCH })
    }

    /// Its containers running now, by name.
    pub fn running(&self) -> Result<Vec<String>> {
        Ok(containers(&self.engine_sock)?.into_iter().filter(|c| c.running && c.run.as_deref() == Some(self.run.as_str())).map(|c| c.name).collect())
    }

    /// Kills its container `name` on the engine (SIGKILL), behind the
    /// platform's back: to its computer, the container died under it.
    pub fn kill(&self, name: &str) -> Result<()> {
        anyhow::ensure!(containers(&self.engine_sock)?.iter().any(|c| c.name == name && c.run.as_deref() == Some(self.run.as_str())), "{name} is no container of this node's run");
        let (status, body) = engine_call(&self.engine_sock, "POST", &format!("/v1/containers/{name}/signal"), Some(EngineBody::Bytes(br#"{"signal":9}"#.to_vec())))?;
        anyhow::ensure!(status == 200 || status == 204, "the engine's signal to {name}: {status} {}", String::from_utf8_lossy(&body));
        Ok(())
    }

    /// The node stops, then the containers its run started are destroyed.
    pub fn stop(mut self) -> Result<()> {
        let stopped = self.child.take().map(|c| stop(c, "sandcastle-node")).transpose();
        let destroyed = destroy_run(&self.engine_sock, &self.run);
        stopped?;
        destroyed.map(|_| ())
    }
}

impl Drop for SandcastleNode {
    /// An early error leaves neither the node nor its run's containers.
    fn drop(&mut self) {
        if let Some(c) = self.child.take() {
            let _ = stop(c, "sandcastle-node");
            let _ = destroy_run(&self.engine_sock, &self.run);
        }
    }
}

/// SIGTERM to a child this process started, and its exit awaited; killed
/// past NODE_STOP_TIMEOUT.
fn stop(mut child: crate::Started, what: &str) -> Result<()> {
    // SAFETY: kill(2) on our own child's pid, which `child` keeps unreaped
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let t0 = Instant::now();
    // bounded by NODE_STOP_TIMEOUT
    while child.try_wait()?.is_none() {
        if t0.elapsed() > NODE_STOP_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{what} did not stop within {NODE_STOP_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

enum EngineBody {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// One HTTP/1.1 call to the engine's API on its unix socket: its status
/// and body. A body is a file, streamed (an image's tar), or bytes.
fn engine_call(sock: &Path, method: &str, path: &str, body: Option<EngineBody>) -> Result<(u16, Vec<u8>)> {
    use std::io::{Read, Write};
    let mut c = std::os::unix::net::UnixStream::connect(sock).with_context(|| format!("connect {}", sock.display()))?;
    c.set_read_timeout(Some(ENGINE_CALL_TIMEOUT))?;
    c.set_write_timeout(Some(ENGINE_CALL_TIMEOUT))?;
    let len = match &body {
        None => 0,
        Some(EngineBody::Bytes(b)) => b.len() as u64,
        Some(EngineBody::File(p)) => fs::metadata(p)?.len(),
    };
    write!(c, "{method} {path} HTTP/1.1\r\nhost: engine\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {len}\r\n\r\n")?;
    match body {
        None => {}
        Some(EngineBody::Bytes(b)) => c.write_all(&b)?,
        Some(EngineBody::File(p)) => {
            std::io::copy(&mut fs::File::open(&p)?, &mut c)?;
        }
    }
    let mut answer = vec![];
    c.take(ENGINE_ANSWER_BYTES_MAX).read_to_end(&mut answer)?;
    let head_end = answer.windows(4).position(|w| w == b"\r\n\r\n").context("an engine's answer has a head")?;
    let head = String::from_utf8_lossy(&answer[..head_end]).into_owned();
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).context("an engine's answer has a status")?;
    let mut rest = answer[head_end + 4..].to_vec();
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        rest = unchunk(&rest)?;
    }
    Ok((status, rest))
}

/// A chunked body, joined.
fn unchunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![];
    // bounded by the body: each pass consumes one chunk
    loop {
        let line_end = b.windows(2).position(|w| w == b"\r\n").context("a chunk's size line")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..line_end]).split(';').next().unwrap_or("").trim(), 16).context("a chunk's size")?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        anyhow::ensure!(b.len() >= size + 2, "a chunk shorter than its size");
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

/// The images built into local Docker (`build_images`: each name and its
/// tag), loaded into the engine: each saved from Docker to a file under
/// `scratch`, then handed to the engine as `docker.io/library/<tag>`, which
/// builds its disk (once per image: one it holds is found by its digest).
/// Answers each name's reference pinned by the digest the engine gives the
/// image (`<repository>@sha256:<64 hex>`: the digest of the OCI manifest of
/// its config and layers as loaded, not a registry's), which the engine
/// resolves to the image it holds. A rebuild is a new reference, so a
/// computer on it is told an update is ready, and never starts its old
/// image's snapshot under the new one's name. Each load is printed with its
/// size and what it took.
pub fn load_images(engine: &Path, built: &[(String, String)], scratch: &Path) -> Result<Vec<(String, String)>> {
    let sock = socket_path(engine, "engine.sock")?;
    fs::create_dir_all(scratch)?;
    let mut pinned = vec![];
    // bounded: the images given
    for (name, tag) in built {
        let t0 = Instant::now();
        let tar = scratch.join(format!("{}.tar", tag.replace([':', '/'], "-")));
        let out = Command::new("docker").args(["save", "-o"]).arg(&tar).arg(tag).stdin(Stdio::null()).output().context("run docker save")?;
        if !out.status.success() {
            bail!("docker save {tag}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let saved = t0.elapsed();
        let bytes = fs::metadata(&tar)?.len();
        let reference = format!("docker.io/library/{tag}");
        let path = format!("/v1/images/load?reference={}", reference.replace('/', "%2F").replace(':', "%3A"));
        let answer = engine_call(&sock, "POST", &path, Some(EngineBody::File(tar.clone())));
        let _ = fs::remove_file(&tar);
        let (status, body) = answer?;
        if status != 200 {
            bail!("the engine's load of {reference}: {status} {}", String::from_utf8_lossy(&body));
        }
        let meta: Value = serde_json::from_slice(&body).with_context(|| format!("the engine's load of {reference} answers its image"))?;
        let by_digest = pinned_reference(&reference, meta["digest"].as_str().unwrap_or_default())?;
        let loaded = t0.elapsed() - saved;
        println!("      (the engine holds {by_digest}: {} MB saved in {saved:.1?}, loaded in {loaded:.1?})", bytes >> 20);
        pinned.push((name.clone(), by_digest));
    }
    Ok(pinned)
}

/// `reference` (`<repository>:<tag>`) pinned by `digest` (`sha256:` and 64
/// lowercase hex) instead: `<repository>@<digest>`.
pub fn pinned_reference(reference: &str, digest: &str) -> Result<String> {
    let hex = digest.strip_prefix("sha256:").filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    anyhow::ensure!(hex.is_some(), "the engine named {reference}'s digest {digest:?}, not sha256: and 64 hex");
    let repository = match reference.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => repo,
        _ => reference,
    };
    let repository = repository.split_once('@').map_or(repository, |(r, _)| r);
    let pinned = format!("{repository}@{digest}");
    assert!(placement::pinned_by_digest(&pinned), "{pinned} is a reference the platform takes");
    Ok(pinned)
}

/// One container as the engine lists it (sandcastle's `api::Info`).
struct Listed {
    name: String,
    running: bool,
    /// Its `RUN_LABEL`: the run whose node started it.
    run: Option<String>,
}

/// The containers the engine holds (at most its VMs).
fn containers(sock: &Path) -> Result<Vec<Listed>> {
    let (status, body) = engine_call(sock, "GET", "/v1/containers", None)?;
    anyhow::ensure!(status == 200, "the engine's containers: {status}");
    let list: Value = serde_json::from_slice(&body)?;
    let all = list.as_array().cloned().unwrap_or_default();
    Ok(all
        .iter()
        .filter_map(|c| {
            Some(Listed { name: c["name"].as_str()?.to_string(), running: c["running"].as_bool().unwrap_or(false), run: c["labels"][RUN_LABEL].as_str().map(str::to_string) })
        })
        .collect())
}

/// Every container on the engine at `sock` that carries `run` as its
/// `RUN_LABEL` (the run's node started it), destroyed; the rest stay.
/// Answers how many it destroyed.
pub fn destroy_run(sock: &Path, run: &str) -> Result<usize> {
    assert!(!run.is_empty(), "a run's name");
    let mut destroyed = 0;
    // bounded: the engine's containers (at most its VMs)
    for c in containers(sock)? {
        if c.run.as_deref() == Some(run) {
            let (status, body) = engine_call(sock, "POST", &format!("/v1/containers/{}/destroy", c.name), Some(EngineBody::Bytes(b"{}".to_vec())))?;
            anyhow::ensure!(status == 200 || status == 404, "the engine's destroy of {}: {status} {}", c.name, String::from_utf8_lossy(&body));
            destroyed += 1;
        }
    }
    Ok(destroyed)
}

/// `images` (name to a config's image: its dockerfile, context and build
/// vars), built for this machine's architecture into local Docker, each
/// tagged `fragment-<name>:<tag>`: (name, tag).
pub fn build_images(images: &[(String, Value)], tag: &str) -> Result<Vec<(String, String)>> {
    let platform = match std::env::consts::ARCH {
        "x86_64" => "linux/amd64",
        "aarch64" => "linux/arm64",
        other => bail!("no computer images for {other}"),
    };
    let mut built = vec![];
    // bounded: the images given
    for (name, image) in images {
        let tag = format!("fragment-{name}:{tag}");
        let dockerfile = image["dockerfile"].as_str().context("an image names its dockerfile")?;
        let context = image["build_context"].as_str().context("an image names its build context")?;
        let mut cmd = Command::new("docker");
        cmd.args(["build", "--load", "--platform", platform, "--provenance=false", "-t", &tag, "-f", dockerfile]);
        for (k, v) in image["build_vars"].as_object().into_iter().flatten() {
            cmd.arg("--build-arg").arg(format!("{k}={}", v.as_str().with_context(|| format!("build var {k} is a string"))?));
        }
        let out = cmd.arg(context).stdin(Stdio::null()).output().context("run docker build (is Docker running, and may this user reach it?)")?;
        if !out.status.success() {
            bail!("docker build of {name} failed: {}\n{}{}", out.status, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        }
        built.push((name.clone(), tag));
    }
    Ok(built)
}

/// The computer images of the cell project at `project` (its
/// `wrangler.jsonc`'s `containers` images, which wrangler builds for the
/// runtime's own containers), by name, their paths absolute.
pub fn cell_images(project: &Path) -> Result<Vec<(String, Value)>> {
    let mut config = crate::read_config(project)?;
    crate::absolute_images(&mut config, project)?;
    let mut images = vec![];
    for container in config["containers"].as_array().into_iter().flatten() {
        for (name, image) in container["images"].as_object().into_iter().flatten() {
            images.push((name.clone(), image.clone()));
        }
    }
    Ok(images)
}

/// `nodes` as `FRAGMENT_NODES` lists them, with `images` (name to its
/// reference, pinned by digest), and their secrets.
pub fn vars(nodes: &[&SandcastleNode], images: &[(String, String)]) -> Result<NodesVars> {
    let images: serde_json::Map<String, Value> = images.iter().map(|(n, r)| (n.clone(), json!(r))).collect();
    let list = json!({ "nodes": nodes.iter().map(|n| n.listing()).collect::<Vec<_>>(), "images": images });
    let text = list.to_string();
    Nodes::parse(&text).map_err(|e| anyhow::anyhow!("the nodes started here: {e}"))?;
    Ok(NodesVars { nodes: text, secrets: nodes.iter().map(|n| (n.id.clone(), n.secret.clone())).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> NodeSpec {
        NodeSpec {
            id: "box".into(),
            port: 9401,
            engine: PathBuf::from("/var/lib/sandcastle"),
            dir: PathBuf::from("/tmp/n"),
            platform: "http://127.0.0.1:9000".into(),
            log_dir: PathBuf::from("/tmp"),
            run: "1f2e3d".into(),
        }
    }

    /// Goal: a node listens on its loopback port, in front of the engine's
    /// own sockets, its secret and egress socket its own, its intercepts to
    /// the platform, every container it starts carrying the run's label.
    #[test]
    fn a_node_is_configured() {
        let c = node_config(&spec()).unwrap();
        assert_eq!(c["listen"], "127.0.0.1:9401");
        assert_eq!(c["engine"], "/var/lib/sandcastle/engine.sock");
        assert_eq!(c["ports"], "/var/lib/sandcastle/ports.sock");
        assert_eq!(c["egress"], "/tmp/n/egress.sock");
        assert_eq!(c["secret_file"], "/tmp/n/node.secret");
        assert_eq!(c["platform"], "http://127.0.0.1:9000");
        assert_eq!(c["labels"], json!({ RUN_LABEL: "1f2e3d" }));
    }

    /// Goal: a socket's path that fits is given back; one past `sun_path`
    /// (a deep checkout's scratch) is refused, saying which and why, before
    /// a node is handed it.
    #[test]
    fn a_socket_path_fits_sun_path_or_is_refused() {
        let fits = PathBuf::from(format!("/{}", "d".repeat(SOCKET_PATH_MAX - "/egress.sock".len() - 1)));
        assert_eq!(socket_path(&fits, "egress.sock").unwrap().as_os_str().len(), SOCKET_PATH_MAX);
        let long = PathBuf::from(format!("/{}", "d".repeat(SOCKET_PATH_MAX - "/egress.sock".len())));
        let e = socket_path(&long, "egress.sock").unwrap_err().to_string();
        assert!(e.contains("108 bytes") && e.contains("egress.sock"), "{e}");
        assert!(node_config(&NodeSpec { dir: long, ..spec() }).is_err());
    }

    /// Goal: a loaded image's reference is pinned by the digest the engine
    /// gives it, its tag dropped (a registry's port kept); a digest that is
    /// not sha256 and 64 hex is refused.
    #[test]
    fn a_loaded_image_is_named_by_its_digest() {
        let d = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(pinned_reference("docker.io/library/fragment-stub:e2e-1f", &d).unwrap(), format!("docker.io/library/fragment-stub@{d}"));
        assert_eq!(pinned_reference("registry.lan:5000/fragment-hermes:local", &d).unwrap(), format!("registry.lan:5000/fragment-hermes@{d}"));
        assert_eq!(pinned_reference("registry.lan:5000/fragment-hermes", &d).unwrap(), format!("registry.lan:5000/fragment-hermes@{d}"));
        for bad in ["", "sha256:abc", &d.to_uppercase(), &d.replace("sha256", "sha512")] {
            assert!(pinned_reference("docker.io/library/fragment-stub:x", bad).is_err(), "{bad}");
        }
    }

    /// Goal: a chunked answer is joined; one cut short is refused.
    #[test]
    fn chunked_answers() {
        assert_eq!(unchunk(b"4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\n\r\n").unwrap(), b"Wikipedia");
        assert!(unchunk(b"9\r\nWiki\r\n").is_err());
        assert!(unchunk(b"zz\r\n").is_err());
    }

    /// Goal: a node list from a file keeps the secrets out of the variable,
    /// names each by the binding the cell reads, and is refused as the cell
    /// would refuse it. Method: files written here, good and bad.
    #[test]
    fn a_node_list_from_a_file() {
        let dir = std::env::temp_dir().join(format!("fragment-nodes-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("box.secret");
        fs::write(&secret, format!("{}\n", "s".repeat(40))).unwrap();
        let short = dir.join("short.secret");
        fs::write(&short, "short").unwrap();
        let pinned = format!("docker.io/library/fragment-stub@sha256:{}", "a".repeat(64));
        let list = |secret_file: &Path, arch: &str, image: &str| {
            json!({
                "nodes": [{ "id": "box", "url": "http://127.0.0.1:9400", "arch": arch, "secret_file": secret_file }],
                "images": { "stub": image }
            })
        };
        let file = dir.join("nodes.json");
        fs::write(&file, list(&secret, "x86_64", &pinned).to_string()).unwrap();
        let vars = NodesFile::read(&file).unwrap();
        assert_eq!(vars.secrets, vec![("box".to_string(), "s".repeat(40))]);
        assert!(!vars.nodes.contains("secret"), "{}", vars.nodes);
        assert_eq!(vars.default_image().unwrap(), "stub");
        fs::write(&file, list(&short, "x86_64", &pinned).to_string()).unwrap();
        assert!(NodesFile::read(&file).unwrap_err().to_string().contains("at least 32 bytes"));
        fs::write(&file, list(&secret, "sparc", &pinned).to_string()).unwrap();
        assert!(NodesFile::read(&file).is_err());
        fs::write(&file, list(&secret, "x86_64", "docker.io/library/fragment-stub:local").to_string()).unwrap();
        assert!(NodesFile::read(&file).unwrap_err().to_string().contains("pinned by its digest"));
        fs::write(&file, list(&dir.join("missing"), "x86_64", &pinned).to_string()).unwrap();
        assert!(NodesFile::read(&file).unwrap_err().to_string().contains("secret_file"));
        let _ = fs::remove_dir_all(&dir);
    }
}
