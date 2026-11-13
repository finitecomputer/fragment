//! The box's own certificate authority (rcgen), and the zone's certificate.
//!
//! - **The root** is made once, in the state directory, and kept: devices
//!   trust it. It is constrained to the zone (RFC 5280 name constraints,
//!   critical, a DNS subtree): a device that trusts it trusts it for the
//!   zone and the names under it, never for anyone else's, even if its key
//!   leaks. It signs end-entity certificates alone (path length 0). Its key
//!   (`root.key`, PKCS#8, mode 0600, in a 0700 directory) never leaves the
//!   box, and a key file others may read is refused rather than used.
//! - **The zone's certificate** names the apex (the platform) and
//!   `*.<zone>` (each fragment's own origin), for serverAuth. Its key is
//!   made anew at each start and held in memory alone: nothing pins it, and
//!   no leaf key is ever on disk.
//!
//! Nothing here parses X.509: the issuer is the root's own parameters
//! (`root_params`, the same at every start) and its key, and a root on disk
//! is checked to carry that key.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, PublicKeyData as _,
    SerialNumber,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

use crate::zone::Zone;

/// The root's key: PKCS#8 PEM, mode 0600.
pub const ROOT_KEY: &str = "root.key";
/// The root, PEM: what a device, `curl --cacert` and celld's
/// `CELLD_EXTRA_CA_FILE` take.
pub const ROOT_PEM: &str = "root.pem";
/// The zone the root is constrained to, beside it.
pub const ROOT_ZONE: &str = "root.zone";

/// How long the root is valid.
pub const ROOT_DAYS: i64 = 3650;
/// How long the zone's certificate is valid: within Apple's 398 days for
/// any server certificate (and 825 for one from a root its owner
/// installed). A box that runs past it without a restart serves an
/// expired certificate; each start issues a fresh one.
pub const LEAF_DAYS: i64 = 397;
/// Certificates are dated this far back, for a device whose clock lags.
const BACKDATE: Duration = Duration::hours(1);
/// What the root is called on a device.
const ROOT_NAME: &str = "fragment LAN root";

#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path} is mode {mode:o}: a key is readable by its owner alone (chmod 600 it)")]
    KeyMode { path: PathBuf, mode: u32 },
    #[error("the root in {dir} is constrained to {have}, not {want}: name another state directory, or remove that one and trust the new root on every device")]
    ZoneMismatch { dir: PathBuf, have: String, want: String },
    #[error("{dir} holds part of a root ({missing} is missing): restore it, or remove the directory to make a new root")]
    Partial { dir: PathBuf, missing: &'static str },
    #[error("{path} is not the root its key makes: restore the pair, or remove the directory to make a new root")]
    NotItsKey { path: PathBuf },
    #[error("certificate: {0}")]
    Cert(#[from] rcgen::Error),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CaError + '_ {
    move |source| CaError::Io { path: path.to_path_buf(), source }
}

/// The root, as its state directory holds it.
pub struct Root {
    zone: Zone,
    pem: String,
    der: Vec<u8>,
    key: KeyPair,
}

/// Whether `Root::open_or_make` made the root or found it.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Opened {
    Made,
    Kept,
}

/// The zone's certificate, its key in memory alone.
pub struct Leaf {
    pub der: Vec<u8>,
    /// PKCS#8.
    pub key_der: Vec<u8>,
    pub not_after: OffsetDateTime,
}

/// The root's parameters, the same at every start but for its dates and
/// serial: an issuer made from them names the root as its certificates'
/// issuer, byte for byte.
fn root_params(zone: &Zone) -> Result<CertificateParams, CaError> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, ROOT_NAME);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    params.name_constraints = Some(NameConstraints { permitted_subtrees: vec![GeneralSubtree::DnsName(zone.name().to_string())], excluded_subtrees: vec![] });
    Ok(params)
}

impl Root {
    /// The root in `dir` for `zone`: made there on first use (`dir` 0700,
    /// its key 0600), else read and checked.
    pub fn open_or_make(dir: &Path, zone: &Zone) -> Result<(Root, Opened), CaError> {
        fs::create_dir_all(dir).map_err(io(dir))?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io(dir))?;
        let (key, pem, named) = (dir.join(ROOT_KEY), dir.join(ROOT_PEM), dir.join(ROOT_ZONE));
        match (key.exists(), pem.exists(), named.exists()) {
            (false, false, false) => Ok((make(dir, zone)?, Opened::Made)),
            (true, true, true) => Ok((read(dir, zone)?, Opened::Kept)),
            (k, p, _) => Err(CaError::Partial { dir: dir.to_path_buf(), missing: if !k { ROOT_KEY } else if !p { ROOT_PEM } else { ROOT_ZONE } }),
        }
    }

    pub fn pem(&self) -> &str {
        &self.pem
    }

    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// The root's SHA-256 over its DER, as `openssl x509 -fingerprint
    /// -sha256` prints it: what a person compares on a device before
    /// trusting it.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.der)
    }

    /// A fresh certificate for the zone and `*.<zone>`, its key new.
    pub fn issue_leaf(&self) -> Result<Leaf, CaError> {
        let issuer = Issuer::new(root_params(&self.zone)?, &self.key);
        let key = KeyPair::generate()?;
        let zone = self.zone.name();
        let mut params = CertificateParams::new(vec![zone.to_string(), format!("*.{zone}")])?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, zone);
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - BACKDATE;
        params.not_after = now + Duration::days(LEAF_DAYS);
        params.serial_number = Some(serial());
        let cert = params.signed_by(&key, &issuer)?;
        Ok(Leaf { der: cert.der().to_vec(), key_der: key.serialize_der(), not_after: params.not_after })
    }
}

fn make(dir: &Path, zone: &Zone) -> Result<Root, CaError> {
    let key = KeyPair::generate()?;
    let mut params = root_params(zone)?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now - BACKDATE;
    params.not_after = now + Duration::days(ROOT_DAYS);
    params.serial_number = Some(serial());
    let cert = params.self_signed(&key)?;
    // the key first, the root's name last: a start cut short leaves a
    // partial root, refused, never a root whose key is lost
    write_key(&dir.join(ROOT_KEY), &key.serialize_pem())?;
    write_public(&dir.join(ROOT_PEM), cert.pem().as_bytes())?;
    write_public(&dir.join(ROOT_ZONE), format!("{zone}\n").as_bytes())?;
    Ok(Root { zone: zone.clone(), pem: cert.pem(), der: cert.der().to_vec(), key })
}

fn read(dir: &Path, zone: &Zone) -> Result<Root, CaError> {
    let named = dir.join(ROOT_ZONE);
    let have = fs::read_to_string(&named).map_err(io(&named))?.trim().to_string();
    if have != zone.name() {
        return Err(CaError::ZoneMismatch { dir: dir.to_path_buf(), have, want: zone.to_string() });
    }
    let key = KeyPair::from_pem(&read_key(&dir.join(ROOT_KEY))?)?;
    let pem_path = dir.join(ROOT_PEM);
    let pem = fs::read_to_string(&pem_path).map_err(io(&pem_path))?;
    let der = pem_der(&pem).ok_or_else(|| CaError::NotItsKey { path: pem_path.clone() })?;
    // the root carries its key's public half: a pair from two roots is refused
    if !der.windows(key.der_bytes().len()).any(|w| w == key.der_bytes()) {
        return Err(CaError::NotItsKey { path: pem_path });
    }
    Ok(Root { zone: zone.clone(), pem, der, key })
}

/// SHA-256, colon-separated, upper case.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// A positive 128-bit serial, random.
fn serial() -> SerialNumber {
    let mut bytes = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).expect("the system's randomness");
    bytes[0] &= 0x7f;
    SerialNumber::from_slice(&bytes)
}

/// The first CERTIFICATE in a PEM text, as DER.
pub fn pem_der(pem: &str) -> Option<Vec<u8>> {
    use rustls::pki_types::pem::PemObject as _;
    rustls::pki_types::CertificateDer::from_pem_slice(pem.as_bytes()).ok().map(|c| c.to_vec())
}

/// A key file, readable by its owner alone; refused if it is not.
fn read_key(path: &Path) -> Result<String, CaError> {
    let mode = fs::metadata(path).map_err(io(path))?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(CaError::KeyMode { path: path.to_path_buf(), mode });
    }
    fs::read_to_string(path).map_err(io(path))
}

/// A new key file, mode 0600 from its creation.
fn write_key(path: &Path, pem: &str) -> Result<(), CaError> {
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(io(path))?;
    f.write_all(pem.as_bytes()).map_err(io(path))?;
    f.sync_all().map_err(io(path))
}

/// A public file, written whole then renamed over.
fn write_public(path: &Path, bytes: &[u8]) -> Result<(), CaError> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(io(&tmp))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).map_err(io(&tmp))?;
    fs::rename(&tmp, path).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use std::sync::Arc;

    fn zone() -> Zone {
        Zone::parse("fragment.home.arpa").unwrap()
    }

    pub(crate) fn tmp(name: &str) -> PathBuf {
        let mut b = [0u8; 6];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).unwrap();
        std::env::temp_dir().join(format!("fragment-lan-{name}-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()))
    }

    /// A verifier trusting `root` alone, as a device that installed it does.
    fn device(root: &[u8]) -> Arc<dyn rustls::client::danger::ServerCertVerifier> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(root.to_vec())).expect("the root is a trust anchor");
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::new(rustls::crypto::ring::default_provider())).build().expect("a verifier")
    }

    fn verifies(root: &[u8], leaf: &[u8], name: &str) -> bool {
        let name = ServerName::try_from(name.to_string()).unwrap();
        device(root).verify_server_cert(&CertificateDer::from(leaf.to_vec()), &[], &name, &[], UnixTime::now()).is_ok()
    }

    /// Goal: a device that trusts the root accepts the zone's certificate
    /// for the apex and one label under it, and for no other name.
    #[test]
    fn the_zone_certificate_verifies_for_the_zone_alone() {
        let dir = tmp("verify");
        let (root, made) = Root::open_or_make(&dir, &zone()).unwrap();
        assert_eq!(made, Opened::Made);
        let leaf = root.issue_leaf().unwrap();
        for ok in ["fragment.home.arpa", "todo--k8ew.fragment.home.arpa"] {
            assert!(verifies(root.der(), &leaf.der, ok), "{ok}");
        }
        for bad in ["example.com", "a.b.fragment.home.arpa", "fragment.home.arpa.evil.example", "home.arpa"] {
            assert!(!verifies(root.der(), &leaf.der, bad), "{bad}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal: the root vouches for nothing outside the zone, even signed by
    /// its own key: a certificate it issues for another name does not
    /// verify (the name constraint holds), and one inside the zone does.
    #[test]
    fn the_roots_name_constraint_holds() {
        let dir = tmp("constrained");
        let (root, _) = Root::open_or_make(&dir, &zone()).unwrap();
        let issuer = Issuer::new(root_params(&zone()).unwrap(), &root.key);
        let forge = |names: Vec<String>| {
            let key = KeyPair::generate().unwrap();
            let mut params = CertificateParams::new(names).unwrap();
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            params.use_authority_key_identifier_extension = true;
            params.signed_by(&key, &issuer).unwrap().der().to_vec()
        };
        assert!(!verifies(root.der(), &forge(vec!["bank.example".into()]), "bank.example"), "a name outside the zone is refused");
        assert!(!verifies(root.der(), &forge(vec!["fragment.home.arpa.bank.example".into()]), "fragment.home.arpa.bank.example"));
        assert!(!verifies(root.der(), &forge(vec!["notfragment.home.arpa".into()]), "notfragment.home.arpa"), "a look-alike is outside the subtree");
        // the control: the same forging, inside the zone, verifies
        assert!(verifies(root.der(), &forge(vec!["x.fragment.home.arpa".into()]), "x.fragment.home.arpa"));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal (restart): the root is made once and kept, its key private;
    /// each leaf is new. Replay: opening again answers the same root.
    #[test]
    fn the_root_is_kept_and_each_leaf_is_new() {
        let dir = tmp("kept");
        let (first, made) = Root::open_or_make(&dir, &zone()).unwrap();
        let leaf1 = first.issue_leaf().unwrap();
        let (again, kept) = Root::open_or_make(&dir, &zone()).unwrap();
        assert_eq!((made, kept), (Opened::Made, Opened::Kept));
        assert_eq!(first.der(), again.der(), "the same root");
        assert_eq!(first.fingerprint(), again.fingerprint());
        let leaf2 = again.issue_leaf().unwrap();
        assert_ne!(leaf1.der, leaf2.der, "a fresh certificate");
        assert_ne!(leaf1.key_der, leaf2.key_der, "with a fresh key");
        assert!(verifies(first.der(), &leaf2.der, "fragment.home.arpa"), "a device that trusted the first root trusts the next leaf");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(ROOT_KEY)), 0o600);
        assert_eq!(mode(&dir.join(ROOT_PEM)), 0o644);
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names.len(), 3, "no leaf on disk: {names:?}");
        assert!((leaf2.not_after - OffsetDateTime::now_utc()).whole_days() <= LEAF_DAYS);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Goal (invalid): refused rather than used: a key others can read, a
    /// root for another zone, half a root, and a root that is not its key's.
    #[test]
    fn a_readable_key_another_zone_half_a_root_and_a_stranger_are_refused() {
        let dir = tmp("refused");
        Root::open_or_make(&dir, &zone()).unwrap();
        fs::set_permissions(dir.join(ROOT_KEY), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(Root::open_or_make(&dir, &zone()), Err(CaError::KeyMode { mode: 0o644, .. })));
        fs::set_permissions(dir.join(ROOT_KEY), fs::Permissions::from_mode(0o600)).unwrap();
        let other = Root::open_or_make(&dir, &Zone::parse("corp.example").unwrap()).err().expect("another zone is refused");
        assert!(other.to_string().contains("constrained to fragment.home.arpa, not corp.example"), "{other}");
        let stranger = tmp("stranger");
        Root::open_or_make(&stranger, &zone()).unwrap();
        fs::copy(stranger.join(ROOT_PEM), dir.join(ROOT_PEM)).unwrap();
        assert!(matches!(Root::open_or_make(&dir, &zone()), Err(CaError::NotItsKey { .. })), "another box's root beside this key");
        fs::remove_file(dir.join(ROOT_PEM)).unwrap();
        assert!(matches!(Root::open_or_make(&dir, &zone()), Err(CaError::Partial { missing: ROOT_PEM, .. })));
        assert!(matches!(Root::open_or_make(&dir, &zone()), Err(CaError::Partial { .. })), "refused again");
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&stranger).unwrap();
    }
}
