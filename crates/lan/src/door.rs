//! The front door: TLS for the zone in front of the node, and the root's
//! page over plain http.
//!
//! - **https:** the zone's certificate, HTTP/1.1. The apex and every name
//!   one label under it go to the node, each with its own `Host`; any other
//!   name is 421. The door is the box's edge, so it says who asked, as
//!   Cloudflare does: `CF-Connecting-IP` is the connection's own peer and
//!   `X-Forwarded-Proto` is `https`, and whatever a client sent under those
//!   names, under any other `cf-` name, or as `Forwarded`,
//!   `X-Forwarded-*`, `X-Real-IP` or `True-Client-IP`, is dropped first. A
//!   client cannot name its own address (docs/self-host.md, found 48).
//!   Bodies stream both ways unbuffered, and an upgrade (a WebSocket) is
//!   spliced once both ends switch.
//! - **Each request is its own connection to the node**, opened for it and
//!   closed after: no pool, so no request is written onto a connection the
//!   node is closing (devstack's front.rs tells that loss).
//! - **http:** `/ca` is the root's page (its fingerprint, and the root as
//!   PEM and DER), on any host, so a device fetches it by the box's address
//!   before it trusts anything; `/` by address shows the same page; a name
//!   in the zone moves to https; anything else is 404.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST, UPGRADE};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::zone::{host_name, https_origin, Place, Zone};

type Body = BoxBody<Bytes, hyper::Error>;

/// Connections at once, https and http together; past it one is closed.
pub const CONNECTIONS_MAX: usize = 1024;
/// A request's head must arrive within this, and fit in this many bytes.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
pub const HEAD_BYTES_MAX: usize = 64 * 1024;
/// A TLS handshake must finish within this.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// The node must accept a connection within this.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The header the door names a request's peer in, as Cloudflare does.
pub const CLIENT_ADDRESS: &str = "cf-connecting-ip";
/// Headers that describe one hop, never passed on (RFC 9110, 7.6.1).
const HOP: [&str; 8] = ["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding"];
/// Headers only an edge may set (besides every `cf-` one): a client's are dropped.
const EDGE: [&str; 6] = ["forwarded", "x-forwarded-for", "x-forwarded-host", "x-forwarded-proto", "x-real-ip", "true-client-ip"];

/// Root downloads, under `/ca/`.
pub const ROOT_PEM_PATH: &str = "/ca/root.pem";
pub const ROOT_CRT_PATH: &str = "/ca/root.crt";

pub struct Door {
    zone: Zone,
    node: SocketAddr,
    /// The https port devices use, for the moves to it and the page's link.
    https_port: u16,
    root_pem: Bytes,
    root_der: Bytes,
    page: String,
    connections: Arc<Semaphore>,
}

impl Door {
    pub fn new(zone: Zone, node: SocketAddr, https_port: u16, root_pem: &str, root_der: &[u8]) -> Arc<Door> {
        let page = page(&zone, https_port, &crate::ca::fingerprint(root_der));
        Arc::new(Door {
            zone,
            node,
            https_port,
            root_pem: Bytes::from(root_pem.to_string()),
            root_der: Bytes::from(root_der.to_vec()),
            page,
            connections: Arc::new(Semaphore::new(CONNECTIONS_MAX)),
        })
    }

    /// Serves https on `listener` until the task is dropped.
    pub async fn serve_https(self: Arc<Self>, listener: TcpListener, tls: tokio_rustls::TlsAcceptor) {
        // unbounded by design: a server's accept loop, each connection bounded by its permit
        loop {
            let Ok((tcp, peer)) = listener.accept().await else { continue };
            let Ok(permit) = self.connections.clone().try_acquire_owned() else { continue };
            let (door, tls) = (self.clone(), tls.clone());
            tokio::spawn(async move {
                let _ = tcp.set_nodelay(true);
                let stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, tls.accept(tcp)).await {
                    Ok(Ok(s)) => s,
                    // a device that does not trust the root yet ends here: say so
                    Ok(Err(e)) => return eprintln!("fragment-lan: TLS from {peer} failed: {e}"),
                    Err(_) => return,
                };
                let service = hyper::service::service_fn(move |req| {
                    let door = door.clone();
                    async move { Ok::<_, Infallible>(door.proxy(req, peer).await) }
                });
                let _ = http1().serve_connection(TokioIo::new(stream), service).with_upgrades().await;
                drop(permit);
            });
        }
    }

    /// Serves plain http on `listener` until the task is dropped.
    pub async fn serve_http(self: Arc<Self>, listener: TcpListener) {
        // unbounded by design: a server's accept loop, each connection bounded by its permit
        loop {
            let Ok((tcp, _)) = listener.accept().await else { continue };
            let Ok(permit) = self.connections.clone().try_acquire_owned() else { continue };
            let door = self.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let door = door.clone();
                    async move { Ok::<_, Infallible>(door.plain(&req)) }
                });
                let _ = http1().serve_connection(TokioIo::new(tcp), service).await;
                drop(permit);
            });
        }
    }

    async fn proxy(&self, mut req: Request<Incoming>, peer: SocketAddr) -> Response<Body> {
        let Some(host) = req.headers().get(HOST).and_then(|h| h.to_str().ok()).map(str::to_string) else {
            return text(StatusCode::BAD_REQUEST, "a request names its host");
        };
        let served = host_name(&host).is_some_and(|name| matches!(self.zone.place(name), Place::Apex | Place::Under(_)));
        if !served {
            return text(StatusCode::MISDIRECTED_REQUEST, &format!("this door serves {} and the names one label under it", self.zone));
        }
        let upgrade = wants_upgrade(req.headers());
        let client_side = upgrade.is_some().then(|| hyper::upgrade::on(&mut req));
        let (mut parts, body) = req.into_parts();
        let path = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string();
        parts.uri = match path.parse() {
            Ok(u) => u,
            Err(_) => return text(StatusCode::BAD_REQUEST, "a request's target is a path"),
        };
        parts.version = hyper::Version::HTTP_11;
        edge_headers(&mut parts.headers, peer);
        match &upgrade {
            Some(protocol) => {
                parts.headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
                parts.headers.insert(UPGRADE, protocol.clone());
            }
            // an Upgrade that Connection does not name is no upgrade
            None => {
                parts.headers.remove(UPGRADE);
            }
        }
        let mut resp = match self.send(Request::from_parts(parts, body)).await {
            Ok(resp) => resp,
            Err(why) => {
                eprintln!("fragment-lan: {host} {}: the node at {} did not answer: {why}", path.split('?').next().unwrap_or("/"), self.node);
                return text(StatusCode::BAD_GATEWAY, "the node is not answering");
            }
        };
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            let Some(client_side) = client_side else { return text(StatusCode::BAD_GATEWAY, "the node switched protocols unasked") };
            let node_side = hyper::upgrade::on(&mut resp);
            tokio::spawn(async move {
                if let (Ok(a), Ok(b)) = tokio::join!(client_side, node_side) {
                    let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
                }
            });
            let (parts, _) = resp.into_parts();
            return Response::from_parts(parts, empty());
        }
        let (mut parts, body) = resp.into_parts();
        strip_hop(&mut parts.headers);
        parts.headers.remove(UPGRADE);
        Response::from_parts(parts, body.boxed())
    }

    /// One request on a connection of its own to the node.
    async fn send(&self, req: Request<Incoming>) -> Result<Response<Incoming>, String> {
        let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.node)).await {
            Ok(Ok(tcp)) => tcp,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err(format!("no connection within {CONNECT_TIMEOUT:?}")),
        };
        let _ = tcp.set_nodelay(true);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await.map_err(|e| e.to_string())?;
        // the connection lives as long as its answer's body (or its upgrade)
        tokio::spawn(conn.with_upgrades());
        sender.send_request(req).await.map_err(|e| e.to_string())
    }

    /// Plain http: the root's page and the root, else a move to https.
    fn plain(&self, req: &Request<Incoming>) -> Response<Body> {
        let host = req.headers().get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
        let name = host_name(host);
        let file = |bytes: &Bytes, content_type: &'static str, file: &str| {
            Response::builder()
                .header("content-type", content_type)
                .header("content-disposition", format!("attachment; filename=\"{file}\""))
                .header("cache-control", "no-store")
                .body(full(bytes.clone()))
                .expect("a well-formed answer")
        };
        match req.uri().path() {
            "/ca" | "/ca/" => html(&self.page),
            "/" if name.is_none() => html(&self.page),
            ROOT_PEM_PATH => file(&self.root_pem, "application/x-pem-file", "fragment-lan-root.pem"),
            // what iOS installs as a profile, macOS's Keychain and Windows take
            ROOT_CRT_PATH => file(&self.root_der, "application/x-x509-ca-cert", "fragment-lan-root.crt"),
            _ => match name.map(|n| (n, self.zone.place(n))) {
                Some((n, Place::Apex | Place::Under(_))) => {
                    let to = format!("{}{}", https_origin(&n.to_ascii_lowercase(), self.https_port), req.uri().path_and_query().map_or("/", |p| p.as_str()));
                    Response::builder().status(StatusCode::PERMANENT_REDIRECT).header("location", to).body(empty()).expect("a well-formed move")
                }
                _ => text(StatusCode::NOT_FOUND, "nothing here: the root is at /ca"),
            },
        }
    }
}

fn http1() -> hyper::server::conn::http1::Builder {
    let mut b = hyper::server::conn::http1::Builder::new();
    b.timer(TokioTimer::new()).header_read_timeout(HEAD_TIMEOUT).max_buf_size(HEAD_BYTES_MAX);
    b
}

/// A request's headers as the node gets them: the hop's dropped, every
/// edge header a client sent dropped, then the door's own set.
pub fn edge_headers(h: &mut HeaderMap, peer: SocketAddr) {
    strip_hop(h);
    let edge: Vec<HeaderName> = h.keys().filter(|k| k.as_str().starts_with("cf-") || EDGE.contains(&k.as_str())).cloned().collect();
    for name in edge {
        h.remove(name);
    }
    let address = peer.ip().to_canonical().to_string();
    h.insert(HeaderName::from_static(CLIENT_ADDRESS), HeaderValue::from_str(&address).expect("an address is a header value"));
    h.insert(HeaderName::from_static("x-forwarded-proto"), HeaderValue::from_static("https"));
}

/// The protocol a request asks to switch to, when it asks (`Connection:
/// upgrade` with an `Upgrade`).
fn wants_upgrade(h: &HeaderMap) -> Option<HeaderValue> {
    let asks = h.get_all(CONNECTION).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).any(|t| t.trim().eq_ignore_ascii_case("upgrade"));
    asks.then(|| h.get(UPGRADE).cloned()).flatten()
}

/// Drops the hop-by-hop headers, and those `Connection` names.
fn strip_hop(h: &mut HeaderMap) {
    let named: Vec<String> =
        h.get_all(CONNECTION).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()).collect();
    for name in HOP.iter().copied().map(str::to_string).chain(named) {
        h.remove(name.as_str());
    }
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).map_err(|never| match never {}).boxed()
}

fn empty() -> Body {
    full(Bytes::new())
}

fn text(status: StatusCode, msg: &str) -> Response<Body> {
    Response::builder().status(status).header("content-type", "text/plain; charset=utf-8").body(full(format!("{msg}\n"))).expect("a well-formed answer")
}

fn html(page: &str) -> Response<Body> {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .header("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'")
        .body(full(page.to_string()))
        .expect("a well-formed answer")
}

/// The page a device opens first, by the box's address.
fn page(zone: &Zone, https_port: u16, fingerprint: &str) -> String {
    let origin = https_origin(zone.name(), https_port);
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>fragment on {zone}</title>
<style>body{{font:16px/1.5 system-ui,sans-serif;max-width:36rem;margin:2rem auto;padding:0 1rem}}code{{word-break:break-all;font-size:.85rem}}a.b{{display:block;margin:.5rem 0;padding:.75rem 1rem;border:1px solid #888;border-radius:.5rem;text-decoration:none}}@media (prefers-color-scheme:dark){{body{{background:#111;color:#eee}}a{{color:#9cf}}}}</style>
</head><body>
<h1>fragment on {zone}</h1>
<p>This box signs its own certificates. Trust its root once on each device, then open <a href="{origin}/">{origin}</a>.</p>
<a class="b" href="{crt}">The root certificate (.crt): iPhone, iPad, Mac, Windows</a>
<a class="b" href="{pem}">The root certificate (PEM): Linux, curl, a browser's own store</a>
<p>The root is valid for {zone} and the names under it only. Its SHA-256 fingerprint, to compare with the one the device shows and the one the box printed when it started:</p>
<p><code>{fingerprint}</code></p>
</body></html>
"#,
        crt = ROOT_CRT_PATH,
        pem = ROOT_PEM_PATH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal (invalid): a client's own edge headers never reach the node:
    /// the address is the peer's, the scheme https; hop-by-hop headers and
    /// those Connection names stay on their hop; the rest pass.
    #[test]
    fn a_client_cannot_name_its_own_address() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("cf-connecting-ip", "203.0.113.9"),
            ("cf-ipcountry", "XX"),
            ("cf-ray", "x"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-proto", "http"),
            ("x-forwarded-host", "evil.example"),
            ("forwarded", "for=203.0.113.9"),
            ("x-real-ip", "203.0.113.9"),
            ("true-client-ip", "203.0.113.9"),
            ("connection", "keep-alive, x-secret"),
            ("keep-alive", "timeout=5"),
            ("x-secret", "1"),
            ("te", "trailers"),
            ("cookie", "a=b"),
            ("authorization", "Nostr x"),
        ] {
            h.append(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        edge_headers(&mut h, "100.64.0.9:51000".parse().unwrap());
        let mut left: Vec<String> = h.iter().map(|(k, v)| format!("{k}: {}", v.to_str().unwrap())).collect();
        left.sort();
        assert_eq!(left, ["authorization: Nostr x", "cf-connecting-ip: 100.64.0.9", "cookie: a=b", "x-forwarded-proto: https"]);
        // an IPv4 peer on a dual-stack socket is named as IPv4; replay: the same again
        edge_headers(&mut h, "[::ffff:100.64.0.9]:51000".parse().unwrap());
        assert_eq!(h.get_all(CLIENT_ADDRESS).iter().collect::<Vec<_>>(), [HeaderValue::from_static("100.64.0.9")]);
    }

    #[test]
    fn an_upgrade_is_asked_with_connection_upgrade_alone() {
        let mut h = HeaderMap::new();
        h.insert(UPGRADE, HeaderValue::from_static("websocket"));
        assert_eq!(wants_upgrade(&h), None);
        h.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert_eq!(wants_upgrade(&h), Some(HeaderValue::from_static("websocket")));
    }
}
