//! The pod's TLS root: the PEM `/tls-root` serves must hash to the root
//! the quote attested, and with `--tls`, a server's leaf must chain to
//! that root for the name asked. No system roots: the attested root is
//! the only trust anchor.

use crate::unlock::tls;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);

/// The root's DER, if `pem` holds one certificate whose hash is `attested`.
pub fn check_root(pem: &[u8], attested: Option<[u8; 32]>) -> Result<CertificateDer<'static>, String> {
    let attested = attested.ok_or("the pod attests no TLS root")?;
    let der = tls::root_der(pem).map_err(|e| e.to_string())?;
    let served = tls::der_hash(&der);
    if served != attested {
        return Err(format!(
            "/tls-root hashes to {}, not the attested {}",
            hex::encode(served),
            hex::encode(attested)
        ));
    }
    Ok(der)
}

/// A TLS handshake with `addr` for `sni`, trusting only `root`. It
/// passes only if the served chain leads to `root`, names `sni`, and
/// the server holds the leaf's key.
pub fn check_leaf(addr: &str, sni: &str, root: CertificateDer<'static>) -> Result<String, String> {
    let mut roots = RootCertStore::empty();
    roots.add(root).map_err(|e| format!("the attested root is not a usable trust anchor: {e}"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(sni.to_owned()).map_err(|e| format!("--sni {sni:?}: {e}"))?;
    let mut conn = ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let sock_addr = addr
        .to_socket_addrs()
        .map_err(|e| format!("{addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("{addr}: no address"))?;
    let mut sock = TcpStream::connect_timeout(&sock_addr, TIMEOUT).map_err(|e| format!("{addr}: {e}"))?;
    sock.set_read_timeout(Some(TIMEOUT)).and_then(|_| sock.set_write_timeout(Some(TIMEOUT))).map_err(|e| e.to_string())?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| format!("TLS handshake with {addr} for {sni}: {e}"))?;
    }
    Ok(format!("{addr} served a leaf for {sni} that chains to the attested root"))
}
