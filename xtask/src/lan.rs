//! `cargo xtask dev --lan`: the dev stack on celld, served to the home
//! network as a company serves an intranet (docs/self-host-lan.md). Beside
//! the stack it starts:
//!
//! - **the CA** (`fragment_lan::ca`): made once in the state directory, its
//!   root constrained to the zone; a fresh certificate for the zone and
//!   `*.<zone>` each start;
//! - **Dex** (`fragment_lan::dex`), pinned, on loopback: the issuer
//!   `https://dex.<zone>`, the cell its client, the people `FRAGMENT_LAN_USERS`
//!   with their passwords in files;
//! - **`fragment-lan serve`**: DNS for the zone (every other name to the
//!   router), the TLS front door before the cell and Dex, and the root's page
//!   over http. It is the one process that binds 53, 80 and 443, so it may
//!   run from a stable path that holds `cap_net_bind_service`
//!   (`FRAGMENT_LAN_BIN`);
//! - **the box's node** (`box_node`): a `sandcastle-node` in front of this
//!   machine's sandcastle engine, its platform the front door and its roots
//!   the CA's, unless `FRAGMENT_NODES_FILE` lists the nodes itself.
//!
//! The cell is told its zone (`FRAGMENT_HOST_SUFFIX`), its origin
//! (`FRAGMENT_PLATFORM_URL`, https) and its issuer, and celld trusts the root
//! for the cell's own fetches (`CELLD_EXTRA_CA_FILE`). Every setting is an
//! environment variable (`Settings::from_env`); unset, each has the
//! intranet's default.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;
use fragment_lan::{ca, dex, serve};

/// The zone: a delegated subzone of the home's special-use domain (RFC 8375).
pub const DEFAULT_ZONE: &str = "fragment.home.arpa";
/// How long Dex, the front door and the box's node have to come up.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// The box's node in the node list.
pub const BOX_NODE: &str = "box";
/// What `cargo xtask node-images` loaded last, by name (`NodeImages`).
pub const NODE_IMAGES_FILE: &str = "target/devstack/node-images.json";

/// The LAN mode's settings, each from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `FRAGMENT_LAN_ZONE` (default `fragment.home.arpa`).
    pub zone: String,
    /// `FRAGMENT_LAN_ADDR`: what the zone's names answer (default: this
    /// machine's address toward its default gateway).
    pub addr: Ipv4Addr,
    /// `FRAGMENT_LAN_BIND`: where the DNS server and the front door listen,
    /// a comma-separated list (default: `addr`).
    pub bind: Vec<IpAddr>,
    /// `FRAGMENT_LAN_DNS_UPSTREAM`: where every other name is asked
    /// (default: the default gateway, on 53).
    pub dns_upstream: SocketAddr,
    /// `FRAGMENT_LAN_HTTPS_PORT` (443), `FRAGMENT_LAN_HTTP_PORT` (80; 0 for
    /// none), `FRAGMENT_LAN_DNS_PORT` (53; 0 for none).
    pub https_port: u16,
    pub http_port: u16,
    pub dns_port: u16,
    /// `FRAGMENT_LAN_STATE`: the CA, Dex's secrets, OpenBao, the box's
    /// node, the front door's config, and the deployment's models (default
    /// `$XDG_STATE_HOME/fragment-lan`, else `~/.local/state/fragment-lan`:
    /// outside the checkout, so `cargo clean` never takes the root the
    /// devices trust).
    pub state: PathBuf,
    /// `FRAGMENT_LAN_USERS`: the people Dex signs in (default `paul`).
    pub users: Vec<String>,
    /// `FRAGMENT_LAN_BIN`: a `fragment-lan` at a stable path (one that holds
    /// `cap_net_bind_service`); unset, the one this checkout builds.
    pub bin: Option<PathBuf>,
    /// `DEX_BIN`: another Dex binary; unset, the pinned one.
    pub dex_bin: Option<PathBuf>,
    /// `FRAGMENT_LAN_ENGINE`: the directory of this machine's sandcastle
    /// engine, its sockets (default `/var/lib/sandcastle`); `none`, no
    /// computers on this box.
    pub engine: Option<PathBuf>,
    /// `SANDCASTLE_NODE_BIN`: the `sandcastle-node` that fronts it (default
    /// `sandcastle-node` on the PATH).
    pub node_bin: PathBuf,
}

impl Settings {
    pub fn from_env() -> Result<Settings> {
        Settings::from(|k| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()), default_route)
    }

    /// The settings from `var`, with `route` naming the default gateway and
    /// this machine's address toward it when they are not set.
    fn from(var: impl Fn(&str) -> Option<String>, route: impl Fn() -> Result<(Ipv4Addr, Ipv4Addr)>) -> Result<Settings> {
        let zone = var("FRAGMENT_LAN_ZONE").unwrap_or_else(|| DEFAULT_ZONE.into());
        anyhow::ensure!(fragment_lan::valid_zone(&zone), "FRAGMENT_LAN_ZONE is a lower-case DNS name of two labels or more, not {zone:?}");
        let port = |k: &str, default: u16, zero: bool| -> Result<u16> {
            match var(k) {
                None => Ok(default),
                Some(v) => match v.parse::<u16>() {
                    Ok(0) if zero => Ok(0),
                    Ok(p) if p > 0 => Ok(p),
                    _ => bail!("{k} is a port{}, not {v:?}", if zero { " (0 for none)" } else { "" }),
                },
            }
        };
        let (https_port, http_port, dns_port) = (port("FRAGMENT_LAN_HTTPS_PORT", 443, false)?, port("FRAGMENT_LAN_HTTP_PORT", 80, true)?, port("FRAGMENT_LAN_DNS_PORT", 53, true)?);
        let explicit_addr = var("FRAGMENT_LAN_ADDR").map(|a| a.parse::<Ipv4Addr>().with_context(|| format!("FRAGMENT_LAN_ADDR is an IPv4 address, not {a:?}"))).transpose()?;
        let explicit_upstream = var("FRAGMENT_LAN_DNS_UPSTREAM")
            .map(|u| u.parse::<SocketAddr>().or_else(|_| u.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 53))).with_context(|| format!("FRAGMENT_LAN_DNS_UPSTREAM is an address (and :port), not {u:?}")))
            .transpose()?;
        let (addr, dns_upstream) = match (explicit_addr, explicit_upstream) {
            (Some(a), Some(u)) => (a, u),
            (a, u) => {
                let (gateway, local) = route().context("finding the LAN address and the router (set FRAGMENT_LAN_ADDR and FRAGMENT_LAN_DNS_UPSTREAM)")?;
                (a.unwrap_or(local), u.unwrap_or(SocketAddr::new(IpAddr::V4(gateway), 53)))
            }
        };
        let bind = match var("FRAGMENT_LAN_BIND") {
            None => vec![IpAddr::V4(addr)],
            Some(list) => list.split(',').map(|a| a.trim().parse::<IpAddr>().with_context(|| format!("FRAGMENT_LAN_BIND is a list of addresses, not {a:?}"))).collect::<Result<_>>()?,
        };
        anyhow::ensure!((1..=serve::LISTENERS_MAX).contains(&bind.len()), "FRAGMENT_LAN_BIND names 1 to {} addresses", serve::LISTENERS_MAX);
        let users: Vec<String> = var("FRAGMENT_LAN_USERS").unwrap_or_else(|| "paul".into()).split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect();
        anyhow::ensure!((1..=dex::USERS_MAX).contains(&users.len()), "FRAGMENT_LAN_USERS names 1 to {} people", dex::USERS_MAX);
        let state = match var("FRAGMENT_LAN_STATE") {
            Some(s) => PathBuf::from(s),
            None => match (var("XDG_STATE_HOME"), var("HOME")) {
                (Some(x), _) => PathBuf::from(x).join("fragment-lan"),
                (None, Some(home)) => PathBuf::from(home).join(".local/state/fragment-lan"),
                (None, None) => bail!("set FRAGMENT_LAN_STATE (no HOME to keep the LAN's state under)"),
            },
        };
        let engine = match var("FRAGMENT_LAN_ENGINE").as_deref() {
            Some("none") => None,
            Some(dir) => Some(PathBuf::from(dir)),
            None => Some(PathBuf::from("/var/lib/sandcastle")),
        };
        Ok(Settings {
            zone,
            addr,
            bind,
            dns_upstream,
            https_port,
            http_port,
            dns_port,
            state,
            users,
            bin: var("FRAGMENT_LAN_BIN").map(PathBuf::from),
            dex_bin: var("DEX_BIN").map(PathBuf::from),
            engine,
            node_bin: var("SANDCASTLE_NODE_BIN").map_or_else(|| PathBuf::from("sandcastle-node"), PathBuf::from),
        })
    }

    /// The platform's origin: `https://<zone>`, with the port when not 443.
    pub fn platform_url(&self) -> String {
        fragment_lan::https_origin(&self.zone, self.https_port)
    }

    /// Where the front door takes https from this box: its first address.
    pub fn door(&self) -> SocketAddr {
        SocketAddr::new(self.bind[0], self.https_port)
    }

    /// Dex's issuer: `https://dex.<zone>`, likewise.
    pub fn issuer(&self) -> String {
        fragment_lan::https_origin(&format!("dex.{}", self.zone), self.https_port)
    }

    /// The deployment's models, when its state holds them and
    /// `FRAGMENT_MODELS_FILE` names none: `models.jsonc`, its key in
    /// `model.key` (docs/self-host-lan.md, step 2).
    pub fn models_file(&self) -> Option<(PathBuf, PathBuf)> {
        let file = self.state.join("models.jsonc");
        file.is_file().then(|| (file, self.state.join("model.key")))
    }
}

/// The default gateway (`/proc/net/route`) and this machine's address
/// toward it (the source a UDP socket connected there would use; nothing
/// is sent).
fn default_route() -> Result<(Ipv4Addr, Ipv4Addr)> {
    let table = std::fs::read_to_string("/proc/net/route").context("read /proc/net/route")?;
    let gateway = gateway_of(&table).context("no IPv4 default route")?;
    let s = UdpSocket::bind("0.0.0.0:0")?;
    s.connect((gateway, 53))?;
    match s.local_addr()?.ip() {
        IpAddr::V4(local) => Ok((gateway, local)),
        IpAddr::V6(_) => bail!("the route to {gateway} has no IPv4 source"),
    }
}

/// The default route's gateway in a `/proc/net/route` table: the row whose
/// destination and mask are both 0 (fields in hex, little-endian).
fn gateway_of(table: &str) -> Option<Ipv4Addr> {
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        let hex = |s: &str| u32::from_str_radix(s, 16).ok();
        match (f.get(1).and_then(|d| hex(d)), f.get(2).and_then(|g| hex(g)), f.get(7).and_then(|m| hex(m))) {
            (Some(0), Some(g), Some(0)) if g != 0 => Some(Ipv4Addr::from(g.to_le_bytes())),
            _ => None,
        }
    })
}

/// A child of this process, killed (by its own PID) and reaped when this
/// is dropped: an early error never leaves one holding a port.
struct Owned(Child);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The LAN half of a running stack: Dex and `fragment-lan serve`, each this
/// process's child, stopped when this is dropped.
pub struct Lan {
    pub settings: Settings,
    /// The root, for celld's own fetches (`CELLD_EXTRA_CA_FILE`), the
    /// renderer's browser and the box's node.
    pub ca_file: PathBuf,
    pub fingerprint: String,
    pub users: Vec<dex::User>,
    pub door_log: PathBuf,
    pub dex_log: PathBuf,
    client_secret: String,
    /// The front door exits when this closes (`--until-stdin-closes`): it
    /// never outlives this process, however this process ends.
    _door_stdin: ChildStdin,
    _door: Owned,
    _dex: Owned,
}

/// The CA, Dex and the front door, ready, for a cell on `cell_port`, Dex on
/// loopback at `dex_port`.
pub fn start(s: Settings, cell_port: u16, dex_port: u16) -> Result<Lan> {
    let log_dir = devstack::repo_root().join("target/devstack");
    std::fs::create_dir_all(&log_dir)?;
    std::fs::create_dir_all(&s.state).with_context(|| format!("create {} (FRAGMENT_LAN_STATE)", s.state.display()))?;
    // the root, made once; a fresh certificate for the zone
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_default();
    let name = if host.is_empty() { "fragment LAN CA".to_string() } else { format!("fragment LAN CA ({host})") };
    let root = ca::ensure_ca(&s.state.join("ca"), &s.zone, &name)?;
    let leaf = ca::issue_leaf(&root)?;
    // the front door's binary first: a stale one stops the start before anything runs
    let bin = door_bin(&s)?;
    // Dex, on loopback behind the door
    let dex_bin = match &s.dex_bin {
        Some(b) => b.clone(),
        None => dex::locate(&devstack::repo_root().join(devstack::TOOLS_DIR))?,
    };
    let dex_listen = SocketAddr::from((Ipv4Addr::LOCALHOST, dex_port));
    let setup = dex::configure(&s.state.join("dex"), &s.zone, &s.issuer(), dex_listen, &s.platform_url(), &s.users)?;
    let dex_log = log_dir.join("lan-dex.log");
    let (out, err) = log_file(&dex_log)?;
    let mut dex = Owned(Command::new(&dex_bin).arg("serve").arg(&setup.config_file).stdin(Stdio::null()).stdout(out).stderr(err).spawn().with_context(|| format!("start {}", dex_bin.display()))?);
    // the front door and DNS
    let cfg = serve::ServeConfig {
        version: serve::CONFIG_VERSION,
        zone: s.zone.clone(),
        addresses: vec![s.addr],
        dns_listen: if s.dns_port == 0 { vec![] } else { s.bind.iter().map(|ip| SocketAddr::new(*ip, s.dns_port)).collect() },
        dns_upstream: s.dns_upstream,
        https_listen: s.bind.iter().map(|ip| SocketAddr::new(*ip, s.https_port)).collect(),
        https_port: s.https_port,
        http_listen: if s.http_port == 0 { vec![] } else { s.bind.iter().map(|ip| SocketAddr::new(*ip, s.http_port)).collect() },
        cert_file: leaf.cert_file.clone(),
        key_file: leaf.key_file.clone(),
        ca_dir: root.dir.clone(),
        cell: SocketAddr::from((Ipv4Addr::LOCALHOST, cell_port)),
        routes: [("dex".to_string(), dex_listen)].into(),
    };
    cfg.check()?;
    let cfg_file = s.state.join("serve.json");
    std::fs::write(&cfg_file, serde_json::to_string_pretty(&cfg)?)?;
    let door_log = log_dir.join("lan-door.log");
    let (out, err) = log_file(&door_log)?;
    let mut door = Owned(
        Command::new(&bin).arg("serve").arg(&cfg_file).arg("--until-stdin-closes").stdin(Stdio::piped()).stdout(out).stderr(err).spawn().with_context(|| format!("start {}", bin.display()))?,
    );
    let door_stdin = door.0.stdin.take().expect("a piped stdin");
    wait_ready(&mut door.0, &door_log, "fragment-lan serve", |text| text.contains("fragment-lan: ready"))?;
    let discovery = format!("http://{dex_listen}/.well-known/openid-configuration");
    let issuer = s.issuer();
    wait_ready(&mut dex.0, &dex_log, "dex", |_| {
        reqwest::blocking::Client::new().get(&discovery).timeout(Duration::from_secs(2)).send().ok().and_then(|r| r.json::<serde_json::Value>().ok()).is_some_and(|d| d["issuer"] == issuer.as_str())
    })?;
    let client_secret = std::fs::read_to_string(&setup.client_secret_file)?.trim().to_string();
    Ok(Lan {
        ca_file: root.pem_file(),
        fingerprint: root.fingerprint(),
        users: setup.users.clone(),
        door_log,
        dex_log,
        client_secret,
        settings: s,
        _door_stdin: door_stdin,
        _door: door,
        _dex: dex,
    })
}

impl Lan {
    /// The cell's sign-in: Dex, its client the cell.
    pub fn oidc(&self) -> devstack::OidcVars {
        devstack::OidcVars { issuer: self.settings.issuer(), client_id: dex::CLIENT_ID.into(), client_secret: Some(self.client_secret.clone()), scopes: None, claims: None, auth: None }
    }

    /// What the card renderer reaches fragments through on the LAN: the
    /// front door (its first address, on the https port: the door may
    /// listen on the LAN's address alone, never loopback), and the root
    /// the door's certificate is under (docs/self-host.md, seam 7).
    pub fn renderer_defaults(&self) -> (SocketAddr, PathBuf) {
        (self.settings.door(), self.ca_file.clone())
    }
}

/// What `cargo xtask node-images` loaded into an engine, last: the `images`
/// a node list names them by, each pinned by the digest the engine gave it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeImages {
    pub engine: PathBuf,
    pub arch: String,
    pub images: serde_json::Map<String, serde_json::Value>,
}

impl NodeImages {
    pub fn write(&self) -> Result<()> {
        let path = devstack::repo_root().join(NODE_IMAGES_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("write {}", path.display()))
    }

    fn read_file(path: &Path) -> Result<NodeImages> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} is node-images' record", path.display()))
    }

    /// The `images` of the box's node list: this checkout's record for
    /// `engine` (this machine's architecture), and the records of other
    /// architectures' engines kept in `state` as `node-images-<arch>.json`
    /// (a paired Mac's, copied from its checkout), each image by
    /// architecture: a digest an engine gave names nothing on another's.
    fn merged(engine: &Path, state: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
        let path = devstack::repo_root().join(NODE_IMAGES_FILE);
        anyhow::ensure!(path.is_file(), "no {} yet: load the computers' images into the engine first, `cargo xtask node-images` (docs/self-host-lan.md, step 2)", path.display());
        let own = NodeImages::read_file(&path)?;
        anyhow::ensure!(own.engine == engine, "{} names the images loaded into {}, not {}: `cargo xtask node-images --engine {}`", path.display(), own.engine.display(), engine.display(), engine.display());
        anyhow::ensure!(own.arch == std::env::consts::ARCH, "{} holds {} images, not {}'s", path.display(), own.arch, std::env::consts::ARCH);
        let mut records = vec![own];
        let mut others: Vec<PathBuf> = std::fs::read_dir(state)
            .with_context(|| format!("read {}", state.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("node-images-") && n.ends_with(".json")))
            .collect();
        others.sort();
        // bounded: the state's files
        for path in others {
            let record = NodeImages::read_file(&path)?;
            let named = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            anyhow::ensure!(named == format!("node-images-{}.json", record.arch), "{} holds {} images: name it node-images-{}.json", path.display(), record.arch, record.arch);
            anyhow::ensure!(record.arch != std::env::consts::ARCH, "{}: this machine's own images are this checkout's record ({NODE_IMAGES_FILE})", path.display());
            records.push(record);
        }
        Ok(by_arch(records))
    }
}

/// Each image's references by the architecture of the engine that gave
/// them (`FRAGMENT_NODES`' `{"<name>": {"<arch>": "<reference>"}}`).
fn by_arch(records: Vec<NodeImages>) -> serde_json::Map<String, serde_json::Value> {
    let mut images = serde_json::Map::new();
    for r in records {
        for (name, reference) in r.images {
            let refs = images.entry(name).or_insert_with(|| serde_json::json!({}));
            refs[r.arch.as_str()] = reference;
        }
    }
    images
}

/// The box's node: `sandcastle-node serve` in front of this machine's
/// engine, this process's child, stopped when this is dropped.
pub struct BoxNode {
    /// The node list the stack wrote for it (`$FRAGMENT_LAN_STATE/nodes.json`,
    /// written at each start): the cell's, unless `FRAGMENT_NODES_FILE`
    /// names another.
    pub list: PathBuf,
    pub log: PathBuf,
    pub url: String,
    _node: Owned,
}

/// When the LAN has an engine (`FRAGMENT_LAN_ENGINE`), the box's node in
/// front of it on loopback at `port`, ready: its config and secret in
/// `$FRAGMENT_LAN_STATE/node` (the secret made once, 0600), its platform
/// the front door and its roots the CA's (a computer's intercepts go to
/// `<platform>/api/nodes/egress`, which the platform takes on its own host
/// alone), and a node list naming it (`box`, its capacity the engine's
/// `vms_max`) with the images `cargo xtask node-images` loaded last
/// (`NodeImages::merged`).
pub fn box_node(lan: &Lan, port: u16) -> Result<Option<BoxNode>> {
    let s = &lan.settings;
    let Some(engine) = &s.engine else { return Ok(None) };
    let vms = devstack::sandcastle::real_engine_vms(engine).with_context(|| {
        format!("the sandcastle engine at {} (is its service running, and may this user reach its socket? FRAGMENT_LAN_ENGINE=none runs the stack with no computers)", engine.display())
    })?;
    let images = NodeImages::merged(engine, &s.state)?;
    let dir = s.state.join("node");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let secret_file = dir.join("node.secret");
    dex::secret_file(&secret_file, dex::Shape::Hex(32))?;
    let listen = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let config = serde_json::json!({
        "listen": listen.to_string(),
        "engine": devstack::sandcastle::socket_path(engine, "engine.sock")?,
        "ports": devstack::sandcastle::socket_path(engine, "ports.sock")?,
        "egress": devstack::sandcastle::socket_path(&dir, "egress.sock")?,
        "secret_file": secret_file,
        "platform": s.platform_url(),
        "ca_file": lan.ca_file,
    });
    let config_file = dir.join("node.json");
    std::fs::write(&config_file, serde_json::to_string_pretty(&config)?)?;
    let url = format!("http://{listen}");
    let list = serde_json::json!({
        "nodes": [{ "id": BOX_NODE, "url": url, "arch": std::env::consts::ARCH, "capacity": vms, "secret_file": secret_file }],
        "images": images,
    });
    let list_file = s.state.join("nodes.json");
    std::fs::write(&list_file, serde_json::to_string_pretty(&list)?)?;
    let log = devstack::repo_root().join("target/devstack/lan-node.log");
    let (out, err) = log_file(&log)?;
    let mut node = Owned(Command::new(&s.node_bin).arg("serve").arg("--config").arg(&config_file).stdin(Stdio::null()).stdout(out).stderr(err).spawn().with_context(|| {
        format!("start {} (SANDCASTLE_NODE_BIN: the sandcastle-node of the engine's install, e.g. ~/.local/opt/sandcastle/bin/sandcastle-node)", s.node_bin.display())
    })?);
    wait_ready(&mut node.0, &log, "sandcastle-node", |_| std::net::TcpStream::connect(listen).is_ok())?;
    Ok(Some(BoxNode { list: list_file, log, url, _node: node }))
}

/// A log file, truncated at each start, for a child's stdout and stderr.
fn log_file(path: &Path) -> Result<(std::fs::File, std::fs::File)> {
    let f = std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(path).with_context(|| format!("open {}", path.display()))?;
    Ok((f.try_clone()?, f))
}

/// Waits until `ready` holds of the log's text (or of the world), the
/// child still running.
fn wait_ready(child: &mut Child, log: &Path, what: &str, mut ready: impl FnMut(&str) -> bool) -> Result<()> {
    let t0 = Instant::now();
    // bounded by READY_TIMEOUT
    loop {
        let text = std::fs::read(log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if ready(&text) {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("{what} exited ({status}) before it was ready:\n{}", tail(&text));
        }
        if t0.elapsed() > READY_TIMEOUT {
            bail!("{what} was not ready after {READY_TIMEOUT:?}:\n{}", tail(&text));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The last lines of a log, for an error.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(30)..].join("\n")
}

/// The `fragment-lan` to run: `FRAGMENT_LAN_BIN`'s, checked to read this
/// checkout's config; else the one this checkout builds.
fn door_bin(s: &Settings) -> Result<PathBuf> {
    let bin = match &s.bin {
        Some(b) => b.clone(),
        None => {
            let root = devstack::repo_root();
            let status = Command::new("cargo").args(["build", "--quiet", "--release", "-p", "fragment-lan", "--manifest-path"]).arg(root.join("Cargo.toml")).status().context("cargo build -p fragment-lan")?;
            anyhow::ensure!(status.success(), "building fragment-lan failed");
            root.join("target/release/fragment-lan")
        }
    };
    let out = Command::new(&bin).arg("version").output().with_context(|| format!("run {} version", bin.display()))?;
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    let want = format!("fragment-lan {}", serve::CONFIG_VERSION);
    anyhow::ensure!(
        said.trim() == want,
        "{} says {:?}, not {want:?}: install the one this checkout builds again (docs/self-host-lan.md, step 1)",
        bin.display(),
        said.trim()
    );
    Ok(bin)
}

/// The lines the dev banner adds.
pub fn banner(lan: &Lan) -> Vec<String> {
    let s = &lan.settings;
    let show = |ips: &[IpAddr], port: u16| ips.iter().map(|ip| SocketAddr::new(*ip, port).to_string()).collect::<Vec<_>>().join(", ");
    let mut lines = vec![
        format!("  LAN:          {}/ (the front door on {}; log {})", s.platform_url(), show(&s.bind, s.https_port), lan.door_log.display()),
        format!("  fragments:    {}", fragment_lan::https_origin(&format!("<label>--<suffix>.{}", s.zone), s.https_port)),
        format!("  DNS:          {} and *.{} -> {}, on {} (every other name to {})", s.zone, s.zone, s.addr, if s.dns_port == 0 { "no port".into() } else { show(&s.bind, s.dns_port) }, s.dns_upstream),
        format!("  the root:     http://{}{}/ca  (SHA-256 {})", s.addr, if s.http_port == 80 { String::new() } else { format!(":{}", s.http_port) }, lan.fingerprint),
        format!("                {}", lan.ca_file.display()),
        format!("  Dex:          {} (log {})", s.issuer(), lan.dex_log.display()),
    ];
    for u in &lan.users {
        lines.push(format!("  person:       {} ({}), the password in {}", u.name, u.email, u.password_file.display()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(vars: &[(&str, &str)]) -> Result<Settings> {
        let mut map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        map.entry("HOME".into()).or_insert_with(|| "/home/paul".into());
        Settings::from(|k| map.get(k).cloned(), || Ok((Ipv4Addr::new(192, 168, 50, 1), Ipv4Addr::new(192, 168, 50, 7))))
    }

    // Goal: unset, the LAN is the intranet's: the zone, the box's address
    // toward its router, the router for every other name, 53, 80 and 443,
    // its state outside the checkout, the box's engine.
    #[test]
    fn the_defaults_are_the_intranet() {
        let s = settings(&[]).unwrap();
        assert_eq!(s.zone, "fragment.home.arpa");
        assert_eq!(s.addr, Ipv4Addr::new(192, 168, 50, 7));
        assert_eq!(s.bind, [IpAddr::V4(Ipv4Addr::new(192, 168, 50, 7))]);
        assert_eq!(s.dns_upstream, "192.168.50.1:53".parse().unwrap());
        assert_eq!((s.https_port, s.http_port, s.dns_port), (443, 80, 53));
        assert_eq!(s.users, ["paul"]);
        assert_eq!(s.platform_url(), "https://fragment.home.arpa");
        assert_eq!(s.issuer(), "https://dex.fragment.home.arpa");
        assert_eq!(s.door(), "192.168.50.7:443".parse().unwrap(), "the door is on the LAN's address, not loopback");
        assert_eq!(s.state, Path::new("/home/paul/.local/state/fragment-lan"));
        assert_eq!(settings(&[("XDG_STATE_HOME", "/s")]).unwrap().state, Path::new("/s/fragment-lan"));
        assert_eq!(s.engine.as_deref(), Some(Path::new("/var/lib/sandcastle")));
        assert_eq!(s.node_bin, Path::new("sandcastle-node"));
    }

    // Goal: each setting moves what it names; high ports show in the origins.
    #[test]
    fn each_setting_moves_its_part() {
        let s = settings(&[
            ("FRAGMENT_LAN_ZONE", "corp.example"),
            ("FRAGMENT_LAN_ADDR", "10.0.0.5"),
            ("FRAGMENT_LAN_BIND", "127.0.0.1, 10.0.0.5"),
            ("FRAGMENT_LAN_DNS_UPSTREAM", "10.0.0.1:5353"),
            ("FRAGMENT_LAN_HTTPS_PORT", "8443"),
            ("FRAGMENT_LAN_HTTP_PORT", "0"),
            ("FRAGMENT_LAN_USERS", "paul, ana"),
            ("FRAGMENT_LAN_STATE", "/srv/lan"),
            ("FRAGMENT_LAN_ENGINE", "none"),
            ("SANDCASTLE_NODE_BIN", "/opt/sandcastle/bin/sandcastle-node"),
        ])
        .unwrap();
        assert_eq!(s.addr, Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(s.bind.len(), 2);
        assert_eq!(s.dns_upstream, "10.0.0.1:5353".parse().unwrap());
        assert_eq!(s.http_port, 0);
        assert_eq!(s.users, ["paul", "ana"]);
        assert_eq!(s.platform_url(), "https://corp.example:8443");
        assert_eq!(s.issuer(), "https://dex.corp.example:8443");
        assert_eq!(s.door(), "127.0.0.1:8443".parse().unwrap(), "the first address bound");
        assert_eq!(s.state, Path::new("/srv/lan"));
        assert_eq!(s.engine, None);
        assert_eq!(s.node_bin, Path::new("/opt/sandcastle/bin/sandcastle-node"));
        assert_eq!(settings(&[("FRAGMENT_LAN_DNS_UPSTREAM", "10.0.0.1")]).unwrap().dns_upstream, "10.0.0.1:53".parse().unwrap());
    }

    // Goal: a bad value stops the start, saying which.
    #[test]
    fn bad_settings_are_refused() {
        for (k, v) in [("FRAGMENT_LAN_ZONE", "Fragment.Home.Arpa"), ("FRAGMENT_LAN_ADDR", "fragment"), ("FRAGMENT_LAN_HTTPS_PORT", "0"), ("FRAGMENT_LAN_DNS_PORT", "99999"), ("FRAGMENT_LAN_BIND", "x")] {
            let e = settings(&[(k, v)]).unwrap_err();
            assert!(format!("{e:#}").contains(k), "{k}: {e:#}");
        }
    }

    // Goal: the box's list names each image by the architecture of the
    // engine whose digest it is, and parses as the cell parses
    // FRAGMENT_NODES: a node of each architecture runs its own reference,
    // and an image one architecture lacks is none of the other's.
    #[test]
    fn the_box_list_names_each_image_by_architecture() {
        use fragment_core::placement::{Arch, Byoc, Nodes};
        let record = |arch: &str, refs: serde_json::Value| NodeImages { engine: "/var/lib/sandcastle".into(), arch: arch.into(), images: refs.as_object().unwrap().clone() };
        let images = by_arch(vec![
            record("x86_64", serde_json::json!({ "hermes": "docker.io/library/fragment-hermes@sha256:aa", "stub": "docker.io/library/fragment-stub@sha256:bb" })),
            record("aarch64", serde_json::json!({ "hermes": "docker.io/library/fragment-hermes@sha256:cc" })),
        ]);
        let list = serde_json::json!({ "nodes": [{ "id": BOX_NODE, "url": "http://127.0.0.1:8808", "arch": "x86_64", "capacity": 16 }], "images": images });
        let nodes = Nodes::parse(&list.to_string(), Byoc::On).unwrap();
        assert_eq!(nodes.reference("hermes", Arch::parse("x86_64").unwrap()), Some("docker.io/library/fragment-hermes@sha256:aa"));
        assert_eq!(nodes.reference("hermes", Arch::parse("aarch64").unwrap()), Some("docker.io/library/fragment-hermes@sha256:cc"));
        assert_eq!(nodes.reference("stub", Arch::parse("aarch64").unwrap()), None, "an x86_64 digest names nothing on arm64");
    }

    // Goal: the default route's gateway is read from the kernel's table.
    #[test]
    fn the_gateway_is_the_default_routes() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n\
                     enp11s0\t00000000\t0132A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
        assert_eq!(gateway_of(table), Some(Ipv4Addr::new(192, 168, 50, 1)));
        assert_eq!(gateway_of("Iface\tDestination\n"), None);
    }
}
