//! Where a stack's git lives (docs/self-host.md, seam 5): the code.storage
//! fake (`crates/fakes`, the default, which the caller starts), macrofiche
//! (github.com/futurepaul/macrofiche, a self-hosted git store that answers
//! code.storage's API) started here on a state directory, or a store
//! already running, named as the cell names one in production.
//!
//! The cell needs nothing of this: it calls whatever `CODESTORAGE_API_URL`
//! names, with tokens signed by the org's key. macrofiche is given the
//! key's public half alone, derived here (`public_half`), in a config file
//! of its own (its docs/design.md, Config and command line): no macrofiche
//! process ever reads the private key. It takes no
//! per-fragment webhook, and the platform registers none with any store:
//! a fragment's pins move by its writers' refreshes and the poll, as on
//! the fake.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use fragment_core::codestorage::OrgKey;

/// The API base the cell calls (`cell/src/config.rs`).
pub const API_URL_VAR: &str = "CODESTORAGE_API_URL";
/// The org: every token's `iss`.
pub const ORG_VAR: &str = "CODESTORAGE_ORG";
/// The org's private key (PKCS#8 PEM, P-256), by its file: a secret is
/// named by its path, never given as a value.
pub const KEY_FILE_VAR: &str = "CODESTORAGE_PRIVATE_KEY_FILE";
/// The macrofiche binary a stack starts.
pub const MACROFICHE_BIN_VAR: &str = "MACROFICHE_BIN";
/// macrofiche answers HTTP within this of its start: it reads every
/// repository's identity and sweeps what a crash left first (its
/// docs/design.md, Recovery), so a large state takes longer than an empty one.
pub const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// A stop gives requests in flight 15 s (its docs/design.md); past this it
/// is killed.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// An org's name: what a token's `iss` carries (the cell's own bound).
pub const ORG_BYTES_MAX: usize = 64;

/// Which store a stack keeps its git in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The in-repo fake, which the caller starts.
    Fake,
    /// macrofiche, started by the stack (`MACROFICHE_BIN`).
    Macrofiche,
    /// A store already running (`CODESTORAGE_API_URL`, `CODESTORAGE_ORG`,
    /// `CODESTORAGE_PRIVATE_KEY_FILE`): a box's own macrofiche service.
    External,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Kind> {
        match s {
            "fake" => Ok(Kind::Fake),
            "macrofiche" => Ok(Kind::Macrofiche),
            "external" => Ok(Kind::External),
            other => bail!("a code store is fake, macrofiche or external, not {other:?}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Fake => "fake",
            Kind::Macrofiche => "macrofiche",
            Kind::External => "external",
        }
    }
}

/// A store already running, as the cell is configured with it.
pub struct ExternalStore {
    pub url: String,
    pub org: String,
    /// The org's private key, PKCS#8 PEM.
    pub key_pem: String,
}

impl ExternalStore {
    /// From `CODESTORAGE_API_URL`, `CODESTORAGE_ORG` and the key's file
    /// `CODESTORAGE_PRIVATE_KEY_FILE`, each required.
    pub fn from_env() -> Result<ExternalStore> {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let url = var(API_URL_VAR).with_context(|| format!("an external code store is named by {API_URL_VAR}, {ORG_VAR} and {KEY_FILE_VAR}: {API_URL_VAR} is unset"))?;
        let org = var(ORG_VAR).with_context(|| format!("{API_URL_VAR} needs {ORG_VAR}, the org its tokens name"))?;
        let file = var(KEY_FILE_VAR).with_context(|| format!("{API_URL_VAR} needs {KEY_FILE_VAR}, the file of the org's private key (PKCS#8 PEM)"))?;
        let key_pem = fs::read_to_string(&file).with_context(|| format!("read {file} ({KEY_FILE_VAR})"))?;
        ExternalStore::new(&url, &org, &key_pem)
    }

    /// Checked as the cell reads them: an http(s) base (no trailing slash),
    /// an org a token can name, and a P-256 key.
    pub fn new(url: &str, org: &str, key_pem: &str) -> Result<ExternalStore> {
        let url = url.trim_end_matches('/');
        let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"));
        if rest.is_none_or(str::is_empty) {
            bail!("{API_URL_VAR} is an http(s) URL, not {url:?}");
        }
        if org.is_empty() || org.len() > ORG_BYTES_MAX || !org.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            bail!("{ORG_VAR} is 1 to {ORG_BYTES_MAX} letters, digits, - and _, not {org:?}");
        }
        OrgKey::from_pem(key_pem).map_err(|e| anyhow::anyhow!("{KEY_FILE_VAR}: {e}"))?;
        Ok(ExternalStore { url: url.to_string(), org: org.to_string(), key_pem: key_pem.to_string() })
    }
}

/// `MACROFICHE_BIN`, which must be a file.
pub fn macrofiche_bin() -> Result<PathBuf> {
    let bin = std::env::var_os(MACROFICHE_BIN_VAR)
        .map(PathBuf::from)
        .with_context(|| format!("set {MACROFICHE_BIN_VAR} to a macrofiche binary (github.com/futurepaul/macrofiche: cargo xtask engine, then cargo build --release -p macrofiche)"))?;
    if !bin.is_file() {
        bail!("no macrofiche at {} ({MACROFICHE_BIN_VAR})", bin.display());
    }
    Ok(bin)
}

pub struct MacroficheOptions {
    /// Its directory: `state/` (the repositories), its config, and the
    /// org's public key.
    pub dir: PathBuf,
    /// The loopback port it listens on.
    pub port: u16,
    pub org: String,
    /// The org's private key (PKCS#8 PEM): macrofiche is given its public
    /// half, never this.
    pub key_pem: String,
    /// Its log (`macrofiche-<port>.log`): its JSON lines, appended across starts.
    pub log_dir: PathBuf,
}

impl MacroficheOptions {
    pub fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn config(&self) -> PathBuf {
        self.dir.join("macrofiche.json")
    }

    fn public_key(&self) -> PathBuf {
        self.dir.join(format!("{}.pub.pem", self.org))
    }
}

/// macrofiche's config for `opts` (its docs/design.md, Config and command
/// line): the three keys it requires, nothing else. Plain HTTP on
/// loopback, its git jailed (`jail` defaults to `enforce`).
fn macrofiche_config(opts: &MacroficheOptions) -> serde_json::Value {
    serde_json::json!({
        "listen": format!("127.0.0.1:{}", opts.port),
        "state_dir": opts.state(),
        "orgs": [{ "name": opts.org, "keys": [{ "pem_file": opts.public_key() }] }],
    })
}

/// The org key's public half, SPKI PEM: what macrofiche verifies the
/// cell's tokens with. Derived here, so the private key stays with the
/// stack and the cell.
fn public_half(key_pem: &str) -> Result<String> {
    use p256::pkcs8::{DecodePrivateKey as _, EncodePublicKey as _, LineEnding};
    let key = p256::SecretKey::from_pkcs8_pem(key_pem.replace("\\n", "\n").trim()).map_err(|e| anyhow::anyhow!("the org key: {e}"))?;
    key.public_key().to_public_key_pem(LineEnding::LF).map_err(|e| anyhow::anyhow!("the org key's public half: {e}"))
}

/// One macrofiche process, this process's child: stopped (`stop`, or when
/// dropped) with the stack.
pub struct Macrofiche {
    child: Option<crate::Started>,
    pub url: String,
    pub log: PathBuf,
}

impl Macrofiche {
    /// Starts macrofiche on `opts.dir` (made on first use; a state already
    /// there is served as it is) and waits until it answers: an unsigned
    /// `GET /api/repos`, which it refuses (401) once it serves.
    pub fn start(bin: &Path, opts: &MacroficheOptions) -> Result<(Macrofiche, Duration)> {
        fs::create_dir_all(&opts.dir).with_context(|| format!("create {}", opts.dir.display()))?;
        fs::create_dir_all(&opts.log_dir)?;
        fs::write(opts.public_key(), public_half(&opts.key_pem)?)?;
        fs::write(opts.config(), serde_json::to_string_pretty(&macrofiche_config(opts))?)?;
        let log = opts.log_dir.join(format!("macrofiche-{}.log", opts.port));
        let mut out = fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&log).with_context(|| format!("open {}", log.display()))?;
        writeln!(out, "== fragment: macrofiche {} on 127.0.0.1:{}, state {}", bin.display(), opts.port, opts.state().display())?;
        let t0 = Instant::now();
        // in the caller's process group, so the Ctrl-C that stops `xtask
        // dev` stops it too; its git children are its own to end
        let child = crate::spawn(Command::new(bin).arg("serve").arg("--config").arg(opts.config()).stdin(Stdio::null()).stdout(out.try_clone()?).stderr(out)).with_context(|| format!("start {}", bin.display()))?;
        let mut m = Macrofiche { child: Some(child), url: format!("http://127.0.0.1:{}", opts.port), log };
        let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build()?;
        // bounded by READY_TIMEOUT
        loop {
            if http.get(format!("{}/api/repos", m.url)).send().is_ok_and(|r| r.status().as_u16() == 401) {
                return Ok((m, t0.elapsed()));
            }
            if let Some(status) = m.child.as_mut().expect("started").try_wait()? {
                m.child = None;
                bail!("macrofiche exited ({status}) before it answered; its log, {}:\n{}", m.log.display(), tail(&m.log));
            }
            if t0.elapsed() > READY_TIMEOUT {
                bail!("macrofiche on :{} did not answer within {READY_TIMEOUT:?}; its log, {}:\n{}", opts.port, m.log.display(), tail(&m.log));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Whether it still runs: a stack whose store has died says so.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.as_mut()?.try_wait().ok().flatten()
    }

    /// SIGTERM and its exit awaited: a write it answered is durable (its
    /// contract, section 9), so a stop loses none. Killed past `STOP_TIMEOUT`.
    pub fn stop(&mut self) -> Result<()> {
        let Some(mut child) = self.child.take() else { return Ok(()) };
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        // its own PID: this process started it
        let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).stderr(Stdio::null()).status();
        let t0 = Instant::now();
        // bounded by STOP_TIMEOUT
        while child.try_wait()?.is_none() {
            if t0.elapsed() > STOP_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("macrofiche did not stop within {STOP_TIMEOUT:?}, and was killed");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
}

impl Drop for Macrofiche {
    /// An early error leaves it holding neither its port nor its state.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// A log's last lines, for an error.
fn tail(log: &Path) -> String {
    let text = fs::read(log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A P-256 key as PKCS#8 PEM, the shape the cell reads, made for the test.
    fn key() -> String {
        fragment_fakes::codestorage::generate_org_key_pem()
    }

    /// Goal: an external store is taken only whole and well formed, as
    /// the cell would read it. Method: each part malformed in turn.
    #[test]
    fn an_external_store_is_checked_as_the_cell_reads_it() {
        let key = key();
        let ok = ExternalStore::new("http://127.0.0.1:9000/", "fragment-dev", &key).expect("a well-formed store");
        assert_eq!((ok.url.as_str(), ok.org.as_str()), ("http://127.0.0.1:9000", "fragment-dev"), "a trailing slash goes, as the cell trims it");
        for (url, org, pem) in [
            ("127.0.0.1:9000", "fragment-dev", key.as_str()),
            ("https://", "fragment-dev", &key),
            ("http://127.0.0.1:9000", "", &key),
            ("http://127.0.0.1:9000", "an org", &key),
            ("http://127.0.0.1:9000", &"o".repeat(ORG_BYTES_MAX + 1), &key),
            ("http://127.0.0.1:9000", "fragment-dev", "not a key"),
        ] {
            assert!(ExternalStore::new(url, org, pem).is_err(), "{url} {org:?}");
        }
    }

    /// Goal: macrofiche is told its state, its port and its org with the
    /// org's public half, never the private key. Method: the config read back.
    #[test]
    fn macrofiche_is_given_the_public_half_alone() {
        let opts = MacroficheOptions { dir: PathBuf::from("/srv/mf"), port: 9101, org: "fragment-e2e".into(), key_pem: key(), log_dir: PathBuf::from("/srv") };
        let config = macrofiche_config(&opts);
        assert_eq!(
            config,
            serde_json::json!({
                "listen": "127.0.0.1:9101",
                "state_dir": "/srv/mf/state",
                "orgs": [{ "name": "fragment-e2e", "keys": [{ "pem_file": "/srv/mf/fragment-e2e.pub.pem" }] }],
            })
        );
        assert!(!config.to_string().contains("PRIVATE"), "{config}");
    }

    /// Goal: macrofiche's key is the org key's public half, which verifies
    /// what the private half signs, and holds nothing private. Method: a
    /// token the cell's own signer makes, verified with the half alone.
    #[test]
    fn the_public_half_verifies_the_cells_tokens() {
        use base64::Engine as _;
        use p256::ecdsa::signature::Verifier as _;
        use p256::pkcs8::DecodePublicKey as _;
        let key = key();
        let public = public_half(&key).unwrap();
        assert!(public.starts_with("-----BEGIN PUBLIC KEY-----") && !public.contains("PRIVATE"), "{public}");
        let token = OrgKey::from_pem(&key).unwrap().token(&fragment_core::codestorage::Claims { iss: "o", sub: "s", repo: "r", scopes: &["git:read"], iat: 1, exp: 2 });
        let (signed, sig) = token.rsplit_once('.').unwrap();
        let sig = p256::ecdsa::Signature::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig).unwrap()).unwrap();
        let verifying = p256::ecdsa::VerifyingKey::from_public_key_pem(&public).unwrap();
        assert!(verifying.verify(signed.as_bytes(), &sig).is_ok(), "the half verifies the cell's token");
        assert!(public_half("not a key").is_err());
    }

    /// Goal: a store's kind is one of three names, and nothing else.
    #[test]
    fn a_code_store_is_one_of_three() {
        for k in [Kind::Fake, Kind::Macrofiche, Kind::External] {
            assert_eq!(Kind::parse(k.name()).unwrap(), k);
        }
        for bad in ["", "Macrofiche", "code.storage", "fake "] {
            assert!(Kind::parse(bad).is_err(), "{bad:?}");
        }
    }
}
