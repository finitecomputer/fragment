//! The TLS-root checks: a root that matches the attested hash and one
//! that does not, and a real handshake with a leaf under the attested
//! root, one under another root, and one for another name.
//!
//! The certificates are in testdata/tls, made once with openssl (P-256,
//! valid until 2126): two roots, `attested` and `stranger`, whose keys
//! were not kept, and three leaves with their PKCS#8 keys: pod.test and
//! other.test under the attested root, pod.test under the stranger.

use fragment_pod::owner::tlscheck::{check_leaf, check_root};
use fragment_pod::unlock::tls::{root_der, root_hash};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::TcpListener;
use std::sync::Arc;

fn read(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/tls/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn root(name: &str) -> CertificateDer<'static> {
    root_der(&read(&format!("{name}-root.pem"))).unwrap()
}

/// Serve one TLS handshake on loopback with the leaf `leaf` (.pem, .key).
fn serve(leaf: &str) -> String {
    let chain = vec![CertificateDer::from_pem_slice(&read(&format!("{leaf}.pem"))).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(&read(&format!("{leaf}.key"))).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        while conn.is_handshaking() {
            if conn.complete_io(&mut sock).is_err() {
                return;
            }
        }
    });
    addr
}

#[test]
fn the_served_root_must_hash_to_the_attested_one() {
    let pem = read("attested-root.pem");
    let attested = root_hash(&pem).unwrap();
    let der = check_root(&pem, Some(attested)).unwrap();
    assert_eq!(der, root("attested"));
    let other = read("stranger-root.pem");
    assert!(check_root(&other, Some(attested)).unwrap_err().contains("not the attested"));
    assert!(check_root(&pem, None).unwrap_err().contains("no TLS root"));
}

#[test]
fn a_leaf_under_the_attested_root_passes() {
    let addr = serve("leaf-pod-test");
    let ok = check_leaf(&addr, "pod.test", root("attested")).unwrap();
    assert!(ok.contains("pod.test"), "{ok}");
}

#[test]
fn a_leaf_under_another_root_or_for_another_name_is_refused() {
    let addr = serve("stranger-leaf-pod-test");
    assert!(check_leaf(&addr, "pod.test", root("attested")).is_err());
    let addr = serve("leaf-other-test");
    assert!(check_leaf(&addr, "pod.test", root("attested")).is_err());
}
