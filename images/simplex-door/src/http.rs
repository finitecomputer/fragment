//! SPIKE: one HTTP/1.1 request on a connection of its own: plain to the
//! door's Durable Object (`http://door.fragment.internal`, intercepted), TLS
//! to the platform's public host (the connector's inbox).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls::{self, pki_types::ServerName, ClientConfig, RootCertStore};

const ANSWER_MAX: usize = 64 * 1024 * 1024;

pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn tls() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring speaks TLS 1.2 and 1.3")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

struct Url {
    https: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse(url: &str) -> Result<Url, String> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("{url}: http or https"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("{url}: a bad port"))?),
        None => (authority, if https { 443 } else { 80 }),
    };
    Ok(Url { https, host: host.to_string(), port, path: path.to_string() })
}

pub async fn call(method: &str, url: &str, headers: &[(&str, String)], body: Bytes, wait: Duration) -> Result<Answer, String> {
    let u = parse(url)?;
    let go = async {
        let tcp = tokio::net::TcpStream::connect((u.host.as_str(), u.port)).await.map_err(|e| format!("{}:{}: {e}", u.host, u.port))?;
        let _ = tcp.set_nodelay(true);
        if u.https {
            let name = ServerName::try_from(u.host.clone()).map_err(|_| format!("{}: no name TLS verifies", u.host))?;
            let io = tokio_rustls::TlsConnector::from(tls()).connect(name, tcp).await.map_err(|e| format!("TLS with {}: {e}", u.host))?;
            send(io, &u, method, headers, body).await
        } else {
            send(tcp, &u, method, headers, body).await
        }
    };
    tokio::time::timeout(wait, go).await.map_err(|_| format!("{method} {url}: no answer in {wait:?}"))?
}

async fn send<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(io: S, u: &Url, method: &str, headers: &[(&str, String)], body: Bytes) -> Result<Answer, String> {
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let host = if (u.https && u.port == 443) || (!u.https && u.port == 80) { u.host.clone() } else { format!("{}:{}", u.host, u.port) };
    let mut req = Request::builder().method(method).uri(u.path.as_str()).header("host", host).header("content-length", body.len()).header("user-agent", "fragment-simplex-door");
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let req = req.body(Full::new(body)).map_err(|e| e.to_string())?;
    let res = sender.send_request(req).await.map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let headers = res.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let body = Limited::new(res.into_body(), ANSWER_MAX).collect().await.map_err(|e| e.to_string())?.to_bytes();
    Ok(Answer { status, headers, body })
}
