//! The service: the root opened (or made), the zone's certificate issued,
//! every listener bound on the one address, then DNS and the door served
//! until a signal (`Lan::run`, the binary's) or the handle is dropped
//! (`Lan::start`: dev, the e2e and tests).

use std::future::Future;
use std::net::{SocketAddr, TcpListener as StdTcp, UdpSocket as StdUdp};
use std::sync::Arc;

use time::OffsetDateTime;
use tokio::sync::oneshot;

use crate::ca::{CaError, Opened, Root};
use crate::config::{Config, ConfigError};
use crate::dns::{self, Answerer};
use crate::door::Door;

/// Tries at a UDP port whose TCP twin is free too, for a DNS port of 0.
const DNS_PAIR_TRIES: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Ca(#[from] CaError),
    #[error("{what} on {addr}: {}", bind_hint(source, addr))]
    Bind { what: &'static str, addr: SocketAddr, source: std::io::Error },
    #[error("the zone's certificate: {0}")]
    Tls(#[from] rustls::Error),
    #[error("the service's runtime: {0}")]
    Runtime(std::io::Error),
}

fn bind_hint(e: &std::io::Error, addr: &SocketAddr) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied if addr.port() < 1024 => {
            format!("{e}: a port below 1024 needs the capability to bind it (on Linux, setcap cap_net_bind_service=+ep on this binary), or a port above 1023")
        }
        std::io::ErrorKind::AddrInUse => format!("{e}: another server holds it"),
        std::io::ErrorKind::AddrNotAvailable => format!("{e}: this machine has no such address"),
        _ => e.to_string(),
    }
}

/// What a started service serves, and where.
pub struct Lan {
    pub https: SocketAddr,
    pub http: Option<SocketAddr>,
    pub dns: Option<SocketAddr>,
    /// The root, as devices trust it.
    pub root_pem: String,
    pub root_der: Vec<u8>,
    pub fingerprint: String,
    /// Whether this start made the root or found it.
    pub opened: Opened,
    /// When the zone's certificate this start issued expires.
    pub leaf_not_after: OffsetDateTime,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn bind_tcp(addr: SocketAddr, what: &'static str) -> Result<StdTcp, StartError> {
    let l = StdTcp::bind(addr).map_err(|source| StartError::Bind { what, addr, source })?;
    l.set_nonblocking(true).map_err(|source| StartError::Bind { what, addr, source })?;
    Ok(l)
}

/// DNS's two sockets on one port: the one named, or (port 0) a free UDP
/// port whose TCP twin is free too.
fn bind_dns(addr: SocketAddr) -> Result<(StdUdp, StdTcp), StartError> {
    let udp_err = |source| StartError::Bind { what: "DNS (udp)", addr, source };
    let tries = if addr.port() == 0 { DNS_PAIR_TRIES } else { 1 };
    let mut last = None;
    for _ in 0..tries {
        let udp = StdUdp::bind(addr).map_err(udp_err)?;
        let at = udp.local_addr().map_err(udp_err)?;
        match bind_tcp(at, "DNS (tcp)") {
            Ok(tcp) => {
                udp.set_nonblocking(true).map_err(udp_err)?;
                return Ok((udp, tcp));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("at least one try"))
}

fn local(addr: Result<SocketAddr, std::io::Error>, what: &'static str, at: SocketAddr) -> Result<SocketAddr, StartError> {
    addr.map_err(|source| StartError::Bind { what, addr: at, source })
}

/// Everything checked, made and bound, before anything is served: a
/// refused port or a bad root stops the start with its reason.
fn prepare(config: &Config) -> Result<(Lan, ServeParts), StartError> {
    let zone = config.check()?;
    let (root, opened) = Root::open_or_make(&config.state_dir, &zone)?;
    let leaf = root.issue_leaf()?;
    let at = |port| SocketAddr::new(config.address, port);
    let https = bind_tcp(at(config.https_port), "the door (https)")?;
    let http = config.http_port.map(|p| bind_tcp(at(p), "the root's page (http)")).transpose()?;
    let dns = config.dns_port.map(|p| bind_dns(at(p))).transpose()?;
    let https_at = local(https.local_addr(), "the door (https)", at(config.https_port))?;
    let http_at = http.as_ref().map(|l| local(l.local_addr(), "the root's page (http)", at(0))).transpose()?;
    let dns_at = dns.as_ref().map(|(u, _)| local(u.local_addr(), "DNS (udp)", at(0))).transpose()?;
    let tls = tls_config(&leaf)?;
    let door = Door::new(zone.clone(), config.node, https_at.port(), root.pem(), root.der());
    let answerer = Arc::new(Answerer::new(zone, config.address));
    let lan = Lan {
        https: https_at,
        http: http_at,
        dns: dns_at,
        root_pem: root.pem().to_string(),
        root_der: root.der().to_vec(),
        fingerprint: root.fingerprint(),
        opened,
        leaf_not_after: leaf.not_after,
        stop: None,
        thread: None,
    };
    Ok((lan, ServeParts { https, http, dns, door, answerer, tls }))
}

/// The rustls config for the zone's certificate: TLS 1.2 and 1.3,
/// HTTP/1.1 by ALPN.
fn tls_config(leaf: &crate::ca::Leaf) -> Result<tokio_rustls::TlsAcceptor, StartError> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf.key_der.clone()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(leaf.der.clone())], key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

fn runtime() -> Result<tokio::runtime::Runtime, StartError> {
    tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("fragment-lan").build().map_err(StartError::Runtime)
}

/// Serves `parts` until `until` is done.
async fn serve(parts: ServeParts, until: impl Future<Output = ()>) -> Result<(), std::io::Error> {
    let ServeParts { https, http, dns, door, answerer, tls } = parts;
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(door.clone().serve_https(tokio::net::TcpListener::from_std(https)?, tls));
    if let Some(l) = http {
        tasks.spawn(door.serve_http(tokio::net::TcpListener::from_std(l)?));
    }
    if let Some((udp, tcp)) = dns {
        tasks.spawn(dns::serve_udp(tokio::net::UdpSocket::from_std(udp)?, answerer.clone()));
        tasks.spawn(dns::serve_tcp(tokio::net::TcpListener::from_std(tcp)?, answerer));
    }
    until.await;
    // the listeners close as their tasks are dropped
    tasks.abort_all();
    Ok(())
}

/// The listeners and what serves them.
struct ServeParts {
    https: StdTcp,
    http: Option<StdTcp>,
    dns: Option<(StdUdp, StdTcp)>,
    door: Arc<Door>,
    answerer: Arc<Answerer>,
    tls: tokio_rustls::TlsAcceptor,
}

impl Lan {
    /// Starts the service on a thread of its own; dropping the handle
    /// stops it and frees every port.
    pub fn start(config: Config) -> Result<Lan, StartError> {
        let (mut lan, parts) = prepare(&config)?;
        let rt = runtime()?;
        let (stop, stopped) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("fragment-lan".into())
            .spawn(move || {
                let served = rt.block_on(serve(parts, async {
                    let _ = stopped.await;
                }));
                if let Err(e) = served {
                    eprintln!("fragment-lan: {e}");
                }
            })
            .map_err(StartError::Runtime)?;
        lan.stop = Some(stop);
        lan.thread = Some(thread);
        Ok(lan)
    }

    /// Serves on this thread until `until` is done (the binary's: a
    /// signal), saying first what it serves.
    pub fn run(config: Config, until: impl Future<Output = ()>) -> Result<(), StartError> {
        let (lan, parts) = prepare(&config)?;
        let rt = runtime()?;
        let show = |a: Option<SocketAddr>| a.map_or("none".to_string(), |a| a.to_string());
        eprintln!(
            "fragment-lan: serving {zone} at {address}: https {https}, http {http}, dns {dns}; the node at {node}",
            zone = config.zone,
            address = config.address,
            https = lan.https,
            http = show(lan.http),
            dns = show(lan.dns),
            node = config.node,
        );
        let made = match lan.opened {
            Opened::Made => "made now",
            Opened::Kept => "kept",
        };
        eprintln!("fragment-lan: the root ({made}, in {}): SHA-256 {}", config.state_dir.display(), lan.fingerprint);
        eprintln!("fragment-lan: the zone's certificate is new, valid until {}", lan.leaf_not_after);
        rt.block_on(serve(parts, until)).map_err(StartError::Runtime)
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
