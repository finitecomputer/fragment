//! The service over real sockets, on loopback and high ports: a stand-in
//! node behind the door, DNS asked over UDP and TCP, the root's page, a
//! restart, and the addresses nothing listens on.
//!
//! Goal: a device that trusts the root and asks the box's DNS reaches the
//! platform and each fragment by name, each request marked by the door
//! (its peer's address, https), never by the client; nothing else is
//! served or answered; the root outlives a restart and the leaf does not.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use fragment_lan::ca::Opened;
use fragment_lan::dns::{lookup, Ask, Heard};
use fragment_lan::{Config, Lan};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ZONE: &str = "lan.home.arpa";
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const WAIT: Duration = Duration::from_secs(5);

type Body = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|never| match never {}).boxed()
}

fn tmp(name: &str) -> PathBuf {
    let mut b = [0u8; 6];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
    std::env::temp_dir().join(format!("fragment-lan-it-{name}-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()))
}

/// A stand-in node: `/echo` answers the request's headers, one a line,
/// sorted, then its body; `/sse` writes one event, then another once
/// `/release` is asked; `/ws` switches protocols and echoes bytes.
async fn node() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let gate = gate.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |mut req: Request<Incoming>| {
                    let gate = gate.clone();
                    async move {
                        let resp: Response<Body> = match req.uri().path() {
                            "/echo" => {
                                let mut lines: Vec<String> = req.headers().iter().map(|(k, v)| format!("{k}: {}", v.to_str().unwrap())).collect();
                                lines.sort();
                                lines.insert(0, format!("target: {}", req.uri()));
                                let body = req.into_body().collect().await.unwrap().to_bytes();
                                lines.push(format!("body: {}", String::from_utf8_lossy(&body)));
                                Response::new(full(lines.join("\n")))
                            }
                            "/release" => {
                                gate.notify_waiters();
                                Response::new(full("released"))
                            }
                            "/sse" => {
                                let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<hyper::body::Frame<Bytes>, hyper::Error>>(4);
                                tokio::spawn(async move {
                                    let _ = tx.send(Ok(hyper::body::Frame::data(Bytes::from("data: one\n\n")))).await;
                                    gate.notified().await;
                                    let _ = tx.send(Ok(hyper::body::Frame::data(Bytes::from("data: two\n\n")))).await;
                                });
                                let stream = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
                                Response::builder().header("content-type", "text/event-stream").body(BodyExt::boxed(http_body_util::StreamBody::new(stream))).unwrap()
                            }
                            "/ws" => {
                                let on = hyper::upgrade::on(&mut req);
                                tokio::spawn(async move {
                                    let mut io = TokioIo::new(on.await.unwrap());
                                    let mut buf = [0u8; 64];
                                    while let Ok(n) = io.read(&mut buf).await {
                                        if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                                            break;
                                        }
                                    }
                                });
                                Response::builder().status(StatusCode::SWITCHING_PROTOCOLS).header("connection", "upgrade").header("upgrade", "echo").body(full("")).unwrap()
                            }
                            _ => Response::builder().status(404).body(full("no")).unwrap(),
                        };
                        Ok::<_, Infallible>(resp)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tcp), svc).with_upgrades().await;
            });
        }
    });
    addr
}

fn config(node: SocketAddr, state: &std::path::Path) -> Config {
    Config { zone: ZONE.into(), address: LOOPBACK, https_port: 0, http_port: Some(0), dns_port: Some(0), node, state_dir: state.to_path_buf() }
}

/// A TLS connection to the door, as a device that trusts `root` and
/// names `name` (SNI).
async fn tls(lan_https: SocketAddr, root: &[u8], name: &str) -> Result<tokio_rustls::client::TlsStream<TcpStream>, std::io::Error> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(root.to_vec())).unwrap();
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = TcpStream::connect(lan_https).await?;
    tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(ServerName::try_from(name.to_string()).unwrap(), tcp).await
}

/// One https request through the door: its status, headers and body.
async fn get(lan: &Lan, name: &str, host: &str, path: &str, extra: &[(&str, &str)], body: &'static str) -> (u16, hyper::HeaderMap, String) {
    let stream = tls(lan.https, &lan.root_der, name).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let mut req = Request::builder().method(if body.is_empty() { "GET" } else { "POST" }).uri(path).header("host", host);
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let resp = send.send_request(req.body(full(body)).unwrap()).await.unwrap();
    let (status, headers) = (resp.status().as_u16(), resp.headers().clone());
    (status, headers, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned())
}

/// One plain-http request: status, headers, body bytes.
async fn plain(at: SocketAddr, host: &str, path: &str) -> (u16, String, Vec<u8>) {
    let mut tcp = TcpStream::connect(at).await.unwrap();
    tcp.write_all(format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut raw = vec![];
    tcp.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

fn port(p: u16) -> String {
    format!(":{p}")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_serves_the_zone_by_name_and_marks_each_request() {
    let state = tmp("door");
    let lan = Lan::start(config(node().await, &state)).unwrap();
    let p = port(lan.https.port());
    let platform = format!("{ZONE}{p}");
    let spoof = [("cf-connecting-ip", "203.0.113.9"), ("x-forwarded-proto", "http"), ("cf-ipcountry", "XX"), ("x-forwarded-for", "203.0.113.9"), ("forwarded", "for=203.0.113.9")];

    // valid: the platform, with the door's marks and none of the client's
    let (status, _, body) = get(&lan, ZONE, &platform, "/echo?x=1", &spoof, "").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("target: /echo?x=1") && body.contains(&format!("host: {platform}")), "{body}");
    assert!(body.contains("cf-connecting-ip: 127.0.0.1") && body.contains("x-forwarded-proto: https"), "{body}");
    assert!(!body.contains("203.0.113.9") && !body.contains("cf-ipcountry") && !body.contains("forwarded: for") && !body.contains("proto: http\n"), "a client's edge headers are dropped: {body}");
    // a fragment by its own name, a body streamed to it
    let fragment = format!("todo--k8ew.{ZONE}");
    let (status, _, body) = get(&lan, &fragment, &format!("{fragment}{p}"), "/echo", &[], "a body").await;
    assert_eq!(status, 200);
    assert!(body.contains(&format!("host: {fragment}{p}")) && body.contains("body: a body"), "{body}");
    // replay: the same request again, the same answer
    let (_, _, again) = get(&lan, ZONE, &platform, "/echo?x=1", &spoof, "").await;
    assert!(again.contains("cf-connecting-ip: 127.0.0.1") && !again.contains("203.0.113.9"));

    // invalid: a name outside the zone, or two labels under it, is refused here
    for host in ["bank.example", "a.b.lan.home.arpa", "evillan.home.arpa", "127.0.0.1"] {
        let (status, _, body) = get(&lan, ZONE, &format!("{host}{p}"), "/echo", &[], "").await;
        assert_eq!(status, 421, "{host}: {body}");
        assert!(!body.contains("target:"), "never reached the node");
    }
    // a device that has not trusted the root fails its handshake; the door serves on
    let stranger = rcgen::generate_simple_self_signed(vec!["x.example".to_string()]).unwrap().cert.der().to_vec();
    assert!(tls(lan.https, &stranger, ZONE).await.is_err());
    assert_eq!(get(&lan, ZONE, &platform, "/echo", &[], "").await.0, 200);
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_and_upgrades_pass_through() {
    let state = tmp("streams");
    let lan = Lan::start(config(node().await, &state)).unwrap();
    let platform = format!("{ZONE}{}", port(lan.https.port()));
    // an event stream reaches the device as it is written
    let stream = tls(lan.https, &lan.root_der, ZONE).await.unwrap();
    let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let resp = send.send_request(Request::get("/sse").header("host", &platform).body(full("")).unwrap()).await.unwrap();
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut body = resp.into_body();
    let first = tokio::time::timeout(WAIT, body.frame()).await.expect("the first event while the second is held").unwrap().unwrap();
    assert_eq!(first.into_data().unwrap(), "data: one\n\n");
    assert_eq!(get(&lan, ZONE, &platform, "/release", &[], "").await.0, 200);
    let second = tokio::time::timeout(WAIT, body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(second.into_data().unwrap(), "data: two\n\n");
    // an upgrade is spliced: bytes go both ways once both ends switch
    let mut stream = tls(lan.https, &lan.root_der, ZONE).await.unwrap();
    stream.write_all(format!("GET /ws HTTP/1.1\r\nhost: {platform}\r\nconnection: upgrade\r\nupgrade: echo\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = vec![0u8; 512];
    let n = tokio::time::timeout(WAIT, stream.read(&mut head)).await.unwrap().unwrap();
    let head = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 101") && head.contains("upgrade: echo"), "{head}");
    for m in ["hello", "again"] {
        stream.write_all(m.as_bytes()).await.unwrap();
        let mut back = vec![0u8; m.len()];
        tokio::time::timeout(WAIT, stream.read_exact(&mut back)).await.unwrap().unwrap();
        assert_eq!(back, m.as_bytes());
    }
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_is_down_is_502_at_once() {
    let state = tmp("down");
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let lan = Lan::start(config(dead, &state)).unwrap();
    let t0 = std::time::Instant::now();
    let (status, _, body) = get(&lan, ZONE, ZONE, "/", &[], "").await;
    assert_eq!((status, body.as_str()), (502, "the node is not answering\n"));
    assert!(t0.elapsed() < WAIT);
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_offers_the_root_and_moves_the_zone_to_https() {
    let state = tmp("plain");
    let lan = Lan::start(config(node().await, &state)).unwrap();
    let http = lan.http.unwrap();
    let (status, _, page) = plain(http, "127.0.0.1", "/").await;
    let page = String::from_utf8(page).unwrap();
    assert_eq!(status, 200);
    assert!(page.contains(&lan.fingerprint), "the page shows the root's fingerprint");
    assert!(page.contains(&format!("https://{ZONE}:{}/", lan.https.port())));
    let (status, head, pem) = plain(http, "127.0.0.1", "/ca/root.pem").await;
    assert_eq!((status, String::from_utf8(pem).unwrap()), (200, lan.root_pem.clone()));
    assert!(head.contains("content-type: application/x-pem-file"));
    let (status, head, der) = plain(http, ZONE, "/ca/root.crt").await;
    assert_eq!((status, der), (200, lan.root_der.clone()));
    assert!(head.contains("content-type: application/x-x509-ca-cert"));
    let (status, head, _) = plain(http, &format!("todo--k8ew.{ZONE}"), "/a/b?c=d").await;
    assert_eq!(status, 308);
    assert!(head.contains(&format!("location: https://todo--k8ew.{ZONE}:{}/a/b?c=d", lan.https.port())), "{head}");
    for host in ["bank.example", "a.b.lan.home.arpa"] {
        assert_eq!(plain(http, host, "/x").await.0, 404, "{host}");
    }
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}

/// One DNS query over TCP (RFC 7766's framing), its answer's code.
async fn tcp_rcode(server: SocketAddr, name: &str) -> u8 {
    let mut q = simple_dns::Packet::new_query(0x4242);
    q.questions.push(simple_dns::Question::new(simple_dns::Name::new(name).unwrap(), simple_dns::QTYPE::TYPE(simple_dns::TYPE::A), simple_dns::QCLASS::CLASS(simple_dns::CLASS::IN), false));
    let q = q.build_bytes_vec().unwrap();
    let mut s = TcpStream::connect(server).await.unwrap();
    let mut framed = (q.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(&q);
    // two queries on one connection
    s.write_all(&framed).await.unwrap();
    s.write_all(&framed).await.unwrap();
    let mut codes = vec![];
    for _ in 0..2 {
        let mut len = [0u8; 2];
        s.read_exact(&mut len).await.unwrap();
        let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
        s.read_exact(&mut msg).await.unwrap();
        let p = simple_dns::Packet::parse(&msg).unwrap();
        assert_eq!(p.id(), 0x4242);
        codes.push(p.rcode() as u8);
    }
    assert_eq!(codes[0], codes[1], "the same answer twice");
    codes[0]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_zone_answers_its_names_and_refuses_others_on_udp_and_tcp() {
    let state = tmp("dns");
    let lan = Lan::start(config(node().await, &state)).unwrap();
    let dns = lan.dns.unwrap();
    let ask = |name: &str, a: Ask| lookup(dns, name, a, WAIT).unwrap();
    assert_eq!(ask(ZONE, Ask::A), Heard::Addresses(vec![LOOPBACK]));
    assert_eq!(ask(&format!("todo--k8ew.{ZONE}"), Ask::A), Heard::Addresses(vec![LOOPBACK]));
    assert_eq!(ask(&format!("todo--k8ew.{ZONE}"), Ask::Aaaa), Heard::Addresses(vec![]), "NODATA for the other family");
    assert_eq!(ask(&format!("a.b.{ZONE}"), Ask::A), Heard::NoSuchName);
    for other in ["example.com", "home.arpa", "evillan.home.arpa"] {
        assert_eq!(ask(other, Ask::A), Heard::Refused, "{other}");
    }
    assert_eq!(tcp_rcode(dns, ZONE).await, 0);
    assert_eq!(tcp_rcode(dns, "example.com").await, 5, "REFUSED over TCP too");
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}

/// The leaf a door serves, as a device that trusts `root` sees it.
async fn served_leaf(https: SocketAddr, root: &[u8]) -> Vec<u8> {
    let stream = tls(https, root, ZONE).await.unwrap();
    stream.get_ref().1.peer_certificates().unwrap()[0].to_vec()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_root_survives_a_restart_and_the_leaf_does_not() {
    let state = tmp("restart");
    let node = node().await;
    let first = Lan::start(config(node, &state)).unwrap();
    assert_eq!(first.opened, Opened::Made);
    let (root, fingerprint, ports) = (first.root_der.clone(), first.fingerprint.clone(), (first.https.port(), first.http.unwrap().port(), first.dns.unwrap().port()));
    let leaf1 = served_leaf(first.https, &root).await;
    drop(first);
    // the same ports again: the first start freed them all
    let again = Config { https_port: ports.0, http_port: Some(ports.1), dns_port: Some(ports.2), ..config(node, &state) };
    let second = Lan::start(again).unwrap();
    assert_eq!(second.opened, Opened::Kept);
    assert_eq!((second.root_der.clone(), second.fingerprint.clone()), (root.clone(), fingerprint), "the same root");
    let leaf2 = served_leaf(second.https, &root).await;
    assert_ne!(leaf1, leaf2, "a new leaf, which a device that trusted the first root trusts");
    let (status, _, body) = get(&second, ZONE, ZONE, "/echo", &[], "").await;
    assert_eq!(status, 200, "{body}");
    drop(second);
    std::fs::remove_dir_all(&state).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_listens_on_another_address() {
    let state = tmp("addresses");
    let lan = Lan::start(config(node().await, &state)).unwrap();
    // this machine's other addresses: IPv6 loopback, and its own address
    // toward the network when it has one (read from a UDP socket's route;
    // nothing is sent), which a listener on every address would answer
    let route = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect("192.0.2.1:9").and_then(|_| s.local_addr()));
    let mut others: Vec<IpAddr> = vec!["::1".parse().unwrap()];
    others.extend(route.ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified()));
    for other in &others {
        let elsewhere = |a: SocketAddr| SocketAddr::new(*other, a.port());
        for at in [lan.https, lan.http.unwrap(), lan.dns.unwrap()] {
            let reached = tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(elsewhere(at))).await;
            assert!(!matches!(reached, Ok(Ok(_))), "nothing on {}", elsewhere(at));
        }
        let quiet = lookup(elsewhere(lan.dns.unwrap()), ZONE, Ask::A, Duration::from_millis(500));
        assert!(quiet.is_err(), "no DNS answer on {other}: {quiet:?}");
    }
    if others.len() == 1 {
        eprintln!("this machine has no address toward a network: only ::1 was checked");
    }
    for at in [lan.https, lan.http.unwrap(), lan.dns.unwrap()] {
        assert!(TcpStream::connect(at).await.is_ok(), "the named address serves {at}");
    }
    // a config that would listen everywhere does not start
    let everywhere = Config { address: "0.0.0.0".parse().unwrap(), ..config(node().await, &state) };
    assert!(Lan::start(everywhere).is_err());
    drop(lan);
    std::fs::remove_dir_all(&state).unwrap();
}
