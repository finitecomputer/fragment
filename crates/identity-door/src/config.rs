//! The door's configuration: one JSON file of its operator's, checked
//! whole at start (`Config::read`), so a door that runs meets nothing
//! malformed later. Its two secrets are files of their own, owner-only
//! (refused otherwise): the door's own (`secret_file`, 32 bytes or more,
//! which seals its state and derives each sign-in's PKCE verifier and
//! nonce) and its client secret at the provider. Each platform it signs
//! people in for is kept as its secret's SHA-256 alone: that secret stays
//! in the platform's secrets backend, its `WORKOS_KEY`.
//!
//! ```json
//! {
//!   "listen": "127.0.0.1:8803",
//!   "public_url": "https://door.example.ts.net",
//!   "secret_file": "/etc/fragment-door/door.secret",
//!   "provider": {
//!     "issuer": "https://idp.example.ts.net",
//!     "client_id": "…",
//!     "client_secret_file": "/etc/fragment-door/provider.secret"
//!   },
//!   "vouched_domains": ["example.com"],
//!   "clients": [{
//!     "client_id": "client_fragment",
//!     "client_secret_sha256": "<64 hex: printf %s \"$secret\" | sha256sum>",
//!     "redirect_uris": ["https://fragment.example.ts.net/auth/callback"]
//!   }]
//! }
//! ```
//!
//! `provider` also takes `scopes` (default `openid email profile`),
//! `email_claim` (default `email`), `auth` (`client_secret_basic` or
//! `client_secret_post`; default: what the provider lists) and `ca_file`
//! (a PEM CA the provider's certificate chains to, beside Mozilla's roots).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::oidc::{OidcError, Settings, Vars, SECRET_MIN_BYTES};
use crate::person::{DomainError, Vouched};

/// The most clients (platforms) one door signs people in for.
pub const CLIENTS_MAX: usize = 16;
/// The most callbacks one client registers (its index fits the state's byte).
pub const REDIRECT_URIS_MAX: usize = 16;
const _: () = assert!(REDIRECT_URIS_MAX <= u8::MAX as usize, "a callback's index is a byte of the state");
/// A client id at most: WorkOS's are `client_` and 26 characters.
pub const CLIENT_ID_BYTES_MAX: usize = 128;
/// A registered callback, or the door's public URL, at most.
pub const URL_BYTES_MAX: usize = 2048;
/// The config file, and each file it names, at most.
pub const FILE_BYTES_MAX: u64 = 1 << 20;
/// Where the provider sends the browser back to the door, under its public URL.
pub const CALLBACK_PATH: &str = "/oidc/callback";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    listen: SocketAddr,
    public_url: String,
    secret_file: PathBuf,
    provider: RawProvider,
    #[serde(default)]
    vouched_domains: Vec<String>,
    clients: Vec<RawClient>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvider {
    issuer: String,
    client_id: String,
    client_secret_file: PathBuf,
    scopes: Option<String>,
    email_claim: Option<String>,
    auth: Option<String>,
    ca_file: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClient {
    client_id: String,
    client_secret_sha256: String,
    redirect_uris: Vec<String>,
}

/// Why a config is refused.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{path} is past {FILE_BYTES_MAX} bytes")]
    TooLarge { path: PathBuf },
    #[error("{path} holds a secret, and others may read it: make it owner-only (chmod 600)")]
    Exposed { path: PathBuf },
    #[error("not the door's config: {0}")]
    Shape(serde_json::Error),
    #[error("{field}: {why}")]
    Invalid { field: &'static str, why: String },
    #[error(transparent)]
    Provider(#[from] OidcError),
    #[error("vouched_domains: {0}")]
    Domains(#[from] DomainError),
}

fn invalid(field: &'static str, why: impl Into<String>) -> ConfigError {
    ConfigError::Invalid { field, why: why.into() }
}

/// A platform the door signs people in for: its id, its secret's SHA-256,
/// and the callbacks it may be sent back to.
#[derive(Clone)]
pub struct Client {
    pub id: String,
    secret_sha256: [u8; 32],
    pub redirect_uris: Vec<Url>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("id", &self.id).field("redirect_uris", &self.redirect_uris).finish_non_exhaustive()
    }
}

impl Client {
    /// A client whose secret is `secret` (dev, the e2e and tests: a box's
    /// config names its SHA-256 alone).
    pub fn with_secret(id: &str, secret: &str, redirect_uris: &[&str]) -> Result<Client, ConfigError> {
        let raw = RawClient {
            client_id: id.into(),
            client_secret_sha256: hex::encode(Sha256::digest(secret.as_bytes())),
            redirect_uris: redirect_uris.iter().map(|u| u.to_string()).collect(),
        };
        Client::of(raw)
    }

    fn of(raw: RawClient) -> Result<Client, ConfigError> {
        let id_ok = !raw.client_id.is_empty() && raw.client_id.len() <= CLIENT_ID_BYTES_MAX && raw.client_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !id_ok {
            return Err(invalid("clients.client_id", format!("{:?}: 1 to {CLIENT_ID_BYTES_MAX} letters, digits, _ or -", raw.client_id)));
        }
        let mut secret_sha256 = [0u8; 32];
        if hex::decode_to_slice(raw.client_secret_sha256.trim(), &mut secret_sha256).is_err() {
            return Err(invalid("clients.client_secret_sha256", "the secret's SHA-256, 64 hex digits"));
        }
        if raw.redirect_uris.is_empty() || raw.redirect_uris.len() > REDIRECT_URIS_MAX {
            return Err(invalid("clients.redirect_uris", format!("1 to {REDIRECT_URIS_MAX} callbacks")));
        }
        let mut redirect_uris = Vec::with_capacity(raw.redirect_uris.len());
        for raw_uri in &raw.redirect_uris {
            redirect_uris.push(web_url(raw_uri).map_err(|why| invalid("clients.redirect_uris", why))?);
        }
        Ok(Client { id: raw.client_id, secret_sha256, redirect_uris })
    }

    /// Whether `given` is this client's secret: its SHA-256 compared in
    /// constant time.
    pub fn secret_is(&self, given: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(given.as_bytes()).into();
        bool::from(digest.ct_eq(&self.secret_sha256))
    }

    /// The registered callback `given` names, exactly (as a URL parses and
    /// prints it), and its index: never one merely like it.
    pub fn redirect(&self, given: &str) -> Option<(u8, &Url)> {
        let parsed = Url::parse(given).ok()?;
        let i = self.redirect_uris.iter().position(|u| u.as_str() == parsed.as_str())?;
        Some((u8::try_from(i).expect("at most REDIRECT_URIS_MAX callbacks"), &self.redirect_uris[i]))
    }
}

/// An http(s) URL with a host, and no fragment or credentials.
fn web_url(raw: &str) -> Result<Url, String> {
    let u = Url::parse(raw).map_err(|e| format!("{raw:?}: {e}"))?;
    let ok = matches!(u.scheme(), "http" | "https") && u.host().is_some() && u.fragment().is_none() && u.username().is_empty() && u.password().is_none() && raw.len() <= URL_BYTES_MAX;
    match ok {
        true => Ok(u),
        false => Err(format!("{raw:?}: an http(s) URL with a host, no fragment or credentials, at most {URL_BYTES_MAX} bytes")),
    }
}

/// The provider part of the config, checked, its secret read.
pub struct ProviderConfig {
    pub settings: Settings,
    pub client_secret: String,
    pub ca_pem: Option<Vec<u8>>,
}

/// The door's configuration, checked, its secrets read.
pub struct Config {
    pub listen: SocketAddr,
    /// Where browsers reach the door: the platform's `WORKOS_API_URL`, and
    /// the root of its callback at the provider (`callback_url`).
    pub public_url: Url,
    /// The door's own secret, 32 bytes or more.
    pub secret: Vec<u8>,
    pub provider: ProviderConfig,
    pub vouched: Vouched,
    pub clients: Vec<Client>,
}

/// A file of the operator's: bounded, and, holding a secret, owner-only.
fn read_file(path: &Path, secret: bool) -> Result<Vec<u8>, ConfigError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|source| ConfigError::Read { path: path.into(), source })?;
    if meta.len() > FILE_BYTES_MAX {
        return Err(ConfigError::TooLarge { path: path.into() });
    }
    if secret && meta.permissions().mode() & 0o077 != 0 {
        return Err(ConfigError::Exposed { path: path.into() });
    }
    std::fs::read(path).map_err(|source| ConfigError::Read { path: path.into(), source })
}

/// A secret file's value: its text, trimmed.
fn read_secret(path: &Path, field: &'static str) -> Result<String, ConfigError> {
    let bytes = read_file(path, true)?;
    let text = String::from_utf8(bytes).map_err(|_| invalid(field, "a secret file holds text"))?;
    let value = text.trim().to_string();
    if value.is_empty() {
        return Err(invalid(field, "the file is empty"));
    }
    Ok(value)
}

impl Config {
    /// The config in the file at `path`, and the secrets it names.
    pub fn read(path: &Path) -> Result<Config, ConfigError> {
        let raw: RawConfig = serde_json::from_slice(&read_file(path, false)?).map_err(ConfigError::Shape)?;
        let vouched = Vouched::new(&raw.vouched_domains)?;
        let p = raw.provider;
        let settings = Settings::parse(Vars {
            issuer: &p.issuer,
            client_id: &p.client_id,
            scopes: p.scopes.as_deref(),
            email_claim: p.email_claim.as_deref(),
            auth: p.auth.as_deref(),
            vouches: vouched.any(),
        })?;
        let client_secret = read_secret(&p.client_secret_file, "provider.client_secret_file")?;
        let ca_pem = p.ca_file.as_deref().map(|f| read_file(f, false)).transpose()?;
        let secret = read_secret(&raw.secret_file, "secret_file")?.into_bytes();
        if raw.clients.len() > CLIENTS_MAX {
            return Err(invalid("clients", format!("1 to {CLIENTS_MAX} clients")));
        }
        let clients = raw.clients.into_iter().map(Client::of).collect::<Result<Vec<_>, _>>()?;
        Config::new(raw.listen, &raw.public_url, secret, ProviderConfig { settings, client_secret, ca_pem }, vouched, clients)
    }

    /// A config of checked parts (dev, the e2e and tests build one so).
    pub fn new(listen: SocketAddr, public_url: &str, secret: Vec<u8>, provider: ProviderConfig, vouched: Vouched, clients: Vec<Client>) -> Result<Config, ConfigError> {
        let public_url = web_url(public_url).map_err(|why| invalid("public_url", why))?;
        if public_url.query().is_some() {
            return Err(invalid("public_url", "the door's root: no query"));
        }
        if secret.len() < SECRET_MIN_BYTES {
            return Err(invalid("secret_file", format!("at least {SECRET_MIN_BYTES} bytes (head -c 32 /dev/urandom | base64)")));
        }
        if provider.client_secret.is_empty() {
            return Err(invalid("provider.client_secret_file", "the door is a confidential client: its secret at the provider"));
        }
        if clients.is_empty() || clients.len() > CLIENTS_MAX {
            return Err(invalid("clients", format!("1 to {CLIENTS_MAX} clients")));
        }
        for (i, c) in clients.iter().enumerate() {
            if clients[..i].iter().any(|d| d.id == c.id) {
                return Err(invalid("clients.client_id", format!("{:?} is named twice", c.id)));
            }
        }
        if provider.settings.email_claim != "email" && !vouched.any() {
            return Err(invalid("provider.email_claim", "a claim email_verified does not speak for is taken only beside vouched_domains"));
        }
        Ok(Config { listen, public_url, secret, provider, vouched, clients })
    }

    pub fn client(&self, id: &str) -> Option<&Client> {
        self.clients.iter().find(|c| c.id == id)
    }

    /// The door's callback at the provider: registered there as the
    /// client's redirect URI.
    pub fn callback_url(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.public_url.as_str().trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const SECRET_SHA256: &str = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"; // "secret"

    /// A scratch directory of the test's own, gone when dropped.
    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("identity-door-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn secret_file(d: &Path, name: &str, value: &str, mode: u32) -> PathBuf {
        let p = d.join(name);
        std::fs::write(&p, value).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    fn config(d: &Path, extra: &str, mode: u32) -> PathBuf {
        let door = secret_file(d, "door.secret", &"d".repeat(32), mode);
        let provider = secret_file(d, "provider.secret", "s3cret\n", 0o600);
        let json = format!(
            r#"{{"listen":"127.0.0.1:0","public_url":"https://door.example.com/","secret_file":{door:?},"provider":{{"issuer":"https://idp.example.com","client_id":"door","client_secret_file":{provider:?}}},"clients":[{{"client_id":"client_a","client_secret_sha256":"{SECRET_SHA256}","redirect_uris":["https://fragment.example.com/auth/callback"]}}]{extra}}}"#
        );
        let p = d.join("door.json");
        std::fs::write(&p, json).unwrap();
        p
    }

    /// Goal: a config of the door's shape is read with its secrets; a
    /// client's secret is matched by hash, its callback exactly.
    #[test]
    fn reads_a_config() {
        let d = dir("reads");
        let c = Config::read(&config(&d.0, r#","vouched_domains":["example.com"]"#, 0o600)).unwrap();
        assert_eq!(c.provider.client_secret, "s3cret", "trimmed");
        assert_eq!(c.callback_url(), "https://door.example.com/oidc/callback");
        let client = c.client("client_a").unwrap();
        assert!(client.secret_is("secret"));
        assert!(!client.secret_is("secret "), "the secret exactly");
        assert_eq!(client.redirect("https://fragment.example.com/auth/callback").map(|(i, _)| i), Some(0));
        assert!(client.redirect("https://fragment.example.com/auth/callback?x=1").is_none());
        assert!(client.redirect("https://evil.example/auth/callback").is_none());
        assert!(!format!("{client:?}").contains("secret_sha256"), "no hash in a debug print");
    }

    /// Goal: what is not the door's config is refused at start, saying
    /// why: a secret others may read, unknown fields, a GitHub "domain",
    /// a short secret, a client twice, malformed callbacks.
    #[test]
    fn refuses_a_bad_config() {
        let d = dir("refuses");
        assert!(matches!(Config::read(&config(&d.0, "", 0o644)), Err(ConfigError::Exposed { .. })));
        assert!(matches!(Config::read(&config(&d.0, r#","extra":1"#, 0o600)), Err(ConfigError::Shape(_))));
        assert!(matches!(Config::read(&config(&d.0, r#","vouched_domains":["github"]"#, 0o600)), Err(ConfigError::Domains(_))));
        let short = config(&d.0, "", 0o600);
        secret_file(&d.0, "door.secret", "short", 0o600);
        assert!(matches!(Config::read(&short), Err(ConfigError::Invalid { field: "secret_file", .. })));
        assert!(matches!(Client::with_secret("a b", "s", &["https://x.example/cb"]), Err(ConfigError::Invalid { field: "clients.client_id", .. })));
        assert!(matches!(Client::with_secret("a", "s", &["javascript:alert(1)"]), Err(ConfigError::Invalid { field: "clients.redirect_uris", .. })));
        assert!(matches!(Client::with_secret("a", "s", &["https://x.example/cb#f"]), Err(ConfigError::Invalid { field: "clients.redirect_uris", .. })));
        assert!(matches!(Client::with_secret("a", "s", &[]), Err(ConfigError::Invalid { field: "clients.redirect_uris", .. })));
        let a = Client::with_secret("a", "s", &["https://x.example/cb"]).unwrap();
        let settings = Settings::parse(Vars { issuer: "https://idp.example.com", client_id: "door", ..Vars::default() }).unwrap();
        let provider = || ProviderConfig { settings: settings.clone(), client_secret: "s".into(), ca_pem: None };
        let twice = Config::new("127.0.0.1:0".parse().unwrap(), "https://door.example", vec![7; 32], provider(), Vouched::default(), vec![a.clone(), a.clone()]);
        assert!(matches!(twice, Err(ConfigError::Invalid { field: "clients.client_id", .. })));
        let query = Config::new("127.0.0.1:0".parse().unwrap(), "https://door.example/?x", vec![7; 32], provider(), Vouched::default(), vec![a]);
        assert!(matches!(query, Err(ConfigError::Invalid { field: "public_url", .. })));
    }
}
