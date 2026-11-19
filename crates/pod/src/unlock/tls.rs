//! The pod's TLS root as the binding names it: SHA-256 of the DER of
//! the one certificate in its PEM file.

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use std::fmt;

/// A root certificate is a few kilobytes; refuse anything far larger.
pub const MAX_PEM: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsRootError {
    TooLarge(usize),
    Pem(String),
    /// The file must hold exactly one certificate.
    Count(usize),
}

impl fmt::Display for TlsRootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "TLS root PEM is {n} bytes; at most {MAX_PEM}"),
            Self::Pem(e) => write!(f, "TLS root PEM: {e}"),
            Self::Count(n) => write!(f, "TLS root PEM holds {n} certificates; expected exactly one"),
        }
    }
}

impl std::error::Error for TlsRootError {}

pub fn root_der(pem: &[u8]) -> Result<CertificateDer<'static>, TlsRootError> {
    if pem.len() > MAX_PEM {
        return Err(TlsRootError::TooLarge(pem.len()));
    }
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| TlsRootError::Pem(e.to_string()))?;
    let n = certs.len();
    let mut certs = certs.into_iter();
    match (certs.next(), certs.next()) {
        (Some(one), None) => Ok(one),
        _ => Err(TlsRootError::Count(n)),
    }
}

pub fn der_hash(der: &[u8]) -> [u8; 32] {
    Sha256::digest(der).into()
}

pub fn root_hash(pem: &[u8]) -> Result<[u8; 32], TlsRootError> {
    root_der(pem).map(|der| der_hash(&der))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEM: &[u8] = include_bytes!("../../testdata/tls-root.pem");

    #[test]
    fn hashes_the_der_of_the_one_certificate() {
        // `openssl x509 -outform DER | shasum -a 256` of the same certificate.
        let expected = "e876bc6978bf4f3da445f98a0a82363c8c0bae5a1fc033c6df65846a6cb0f18c";
        assert_eq!(hex::encode(root_hash(PEM).unwrap()), expected);
    }

    #[test]
    fn refuses_none_two_and_garbage() {
        assert_eq!(root_hash(b"hello").unwrap_err(), TlsRootError::Count(0));
        assert_eq!(root_hash(&[PEM, PEM].concat()).unwrap_err(), TlsRootError::Count(2));
        let broken = b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        assert!(matches!(root_hash(broken), Err(TlsRootError::Pem(_))));
        assert!(matches!(root_hash(&vec![b'a'; MAX_PEM + 1]), Err(TlsRootError::TooLarge(_))));
    }
}
