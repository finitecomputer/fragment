// `fragment pod status|verify|init|unlock <pod>`: the owner's side of a
// pod (docs/pod.md), a thin command line over fragment_pod's `owner`;
// `fragment pod keyd …`, a key service for it (keyd.rs).
// The pod's owner is this CLI's own key (the config's `secret_key`, the
// one `fragment login` makes): no second key. What the owner keeps per
// pod is a folder beside the config, `pods/<host>/` (`pod_dir`):
// policy.json (the owner's), state.json and disk.key (fragment_pod's
// `owner::dir`, 0600 in a 0700 folder, as the config itself is 0600).
//
// Exit: 0 when every check passed and the action was done; 1 when a
// check failed or the pod refused (`pod_refused`, naming which), or the
// pod did not answer (`unavailable`); 2 for a usage mistake.

mod keyd;
// The key service's test harness, which includes pod-init's: one copy
// for both test files, so each module is loaded once.
#[cfg(test)]
#[path = "../../../crates/pod-keyd/tests/common/mod.rs"]
mod harness;
#[cfg(test)]
mod keyd_tests;
mod output;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use fragment_pod::checks::attestor::Attestor;
use fragment_pod::owner::client;
use fragment_pod::owner::dir::{Dir, DISK_KEY, POLICY};
use fragment_pod::owner::flow::{self, Refusal, Step, TlsTarget};
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use serde_json::{json, Value};

use crate::api::{Code, CodedError};
use crate::auth::Identity;

/// pod-init's port when `<pod>` names none (docs/pod.md, pod-init).
pub const DEFAULT_PORT: u16 = 7000;
/// The folder beside the config that holds one folder per pod.
pub const PODS: &str = "pods";
/// A DNS name's longest (RFC 1035): a host is a folder's name too.
const HOST_MAX: usize = 253;
/// Pod folders `claimed` reads at most: an owner has a few.
const CLAIMED_MAX: usize = 1024;

#[derive(Subcommand)]
pub enum PodCmd {
    /// The pod's state and the five checks (quote, release, platform,
    /// google_instance, owner), changing nothing; exit 1 if one failed
    Status {
        /// The pod: host[:port] (port 7000 if none)
        pod: String,
    },
    /// The checks, then that the TLS root the pod serves is the one it
    /// attested (tls_root) and, with --tls and --sni, that a server's
    /// leaf chains to it (tls_leaf); exit 1 if one failed
    Verify {
        pod: String,
        /// host:port of a TLS server whose leaf must chain to the attested root
        #[arg(long, requires = "sni")]
        tls: Option<String>,
        /// The name that leaf must be valid for
        #[arg(long, requires = "tls")]
        sni: Option<String>,
    },
    /// Claim a blank pod with this CLI's key: every check, then a new disk
    /// key (saved first, printed once as the recovery code) sealed to
    /// this boot of the pod, then a fresh attestation that it is open and
    /// yours
    Init { pod: String },
    /// Give a locked pod (after a restart or upgrade) its disk key: every
    /// check, the key sealed to this boot, then a fresh attestation that
    /// it is open and yours
    Unlock { pod: String },
    /// A key service (pod-keyd) that unlocks this pod after a restart on
    /// your terms: check it and deposit the disk key, approve a further
    /// release, or show its checks and the record it holds for you
    Keyd {
        #[command(subcommand)]
        cmd: keyd::KeydCmd,
    },
}

/// A pod's address: the host names its folder, the port does not (the
/// same pod on another port is the same pod).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// `host[:port]`: a DNS name or an IPv4 address, lower-cased.
    pub fn parse(text: &str) -> Result<Address> {
        Self::parse_port(text, DEFAULT_PORT)
    }

    /// As `parse`, with `default` when the text names no port.
    pub fn parse_port(text: &str, default: u16) -> Result<Address> {
        let (host, port) = match text.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| usage(format!("{text:?}: the port must be 1 to 65535")))?),
            None => (text, default),
        };
        let host = host.to_ascii_lowercase();
        let shaped = (1..=HOST_MAX).contains(&host.len())
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            && !host.starts_with(['.', '-'])
            && !host.contains("..");
        if !shaped {
            return Err(usage(format!("{text:?} is not a host: host[:port], a DNS name or an IPv4 address (port {default} if none)")));
        }
        Ok(Address { host, port })
    }

    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// The folder of one pod: `<pods>/<host>`.
pub fn pod_dir(pods: &Path, a: &Address) -> PathBuf {
    pods.join(&a.host)
}

/// The folder of every pod, beside the config file.
pub fn pods_dir(config: &Path) -> PathBuf {
    config.parent().unwrap_or_else(|| Path::new(".")).join(PODS)
}

/// Pods this machine has claimed (their folder holds a disk key), by
/// host; bounded by `CLAIMED_MAX` folders read.
pub fn claimed(pods: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(pods) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", pods.display())),
    };
    let mut hosts = Vec::new();
    for entry in entries.take(CLAIMED_MAX) {
        let entry = entry.with_context(|| format!("reading {}", pods.display()))?;
        if entry.path().join(DISK_KEY).exists() {
            hosts.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    hosts.sort();
    Ok(hosts)
}

/// What a command runs with.
pub struct Ctx<'a> {
    /// `pods_dir`'s answer.
    pub pods: &'a Path,
    /// This CLI's key, if it has one.
    pub key: Option<&'a Identity>,
    pub attestor: &'a dyn Attestor,
    pub json: bool,
}

/// A command's answer: `--json`'s data, and whether every check passed.
#[derive(Debug)]
pub struct Report {
    pub data: Value,
    pub passed: bool,
}

impl Report {
    pub fn exit_code(&self) -> i32 {
        if self.passed {
            0
        } else {
            1
        }
    }
}

fn usage(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg: msg.into() })
}

/// A check failed, or the pod refused what it was sent: `pod_refused`.
/// Anything else (the pod unreached, a file unread) keeps its own code.
fn coded(e: anyhow::Error) -> anyhow::Error {
    if e.downcast_ref::<Refusal>().is_some() || client::is_refusal(&e) {
        return anyhow::Error::new(CodedError { code: Code::PodRefused, msg: format!("{e:#}") });
    }
    e
}

/// The owner's signing key: this CLI's own, as the pod's key type.
pub fn owner_key(id: &Identity) -> OwnerKey {
    OwnerKey::from_hex(&id.secret_hex()).expect("a CLI identity's secret is a secp256k1 secret")
}

fn require_key<'a>(ctx: &Ctx<'a>) -> Result<&'a Identity> {
    ctx.key.ok_or_else(|| {
        anyhow::Error::new(CodedError {
            code: Code::AuthFailed,
            msg: "no key here to own a pod with: `fragment login` makes this machine's key (a pod needs no account on a platform)".into(),
        })
    })
}

/// The pod's folder, which must hold the owner's policy.json. The owner
/// makes it by hand, so it is made private here (0700) before a disk key
/// can be written in it.
fn open_dir(ctx: &Ctx<'_>, a: &Address) -> Result<(Dir, PathBuf)> {
    let path = pod_dir(ctx.pods, a);
    let policy = path.join(POLICY);
    if !policy.is_file() {
        return Err(usage(format!(
            "no policy for the pod {}: write {} (the releases and the Google instance this pod must be; cli/GUIDE.md, \"Your pod\")",
            a.host,
            policy.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).with_context(|| format!("making {} private (0700)", path.display()))?;
    }
    Ok((Dir::new(&path), path))
}

pub fn run(ctx: &Ctx<'_>, cmd: PodCmd, out: &mut dyn Write) -> Result<Report> {
    match cmd {
        PodCmd::Status { pod } => status(ctx, &Address::parse(&pod)?, out),
        PodCmd::Verify { pod, tls, sni } => verify(ctx, &Address::parse(&pod)?, tls.as_deref().zip(sni.as_deref()), out),
        PodCmd::Init { pod } => key_request(ctx, &Address::parse(&pod)?, Kind::Init, out),
        PodCmd::Unlock { pod } => key_request(ctx, &Address::parse(&pod)?, Kind::Unlock, out),
        PodCmd::Keyd { cmd } => keyd::run(ctx, cmd, out),
    }
}

fn me(ctx: &Ctx<'_>) -> Option<[u8; 32]> {
    ctx.key.map(|id| owner_key(id).public())
}

fn status(ctx: &Ctx<'_>, a: &Address, out: &mut dyn Write) -> Result<Report> {
    let (dir, path) = open_dir(ctx, a)?;
    let (f, o) = flow::status(&dir, me(ctx), ctx.attestor, &a.host_port())?;
    let passed = !o.any_fail();
    let mut data = output::attested(&f, &o, &[], me(ctx));
    data["pod"] = json!(a.host_port());
    data["dir"] = json!(path.display().to_string());
    if !ctx.json {
        output::print_attested(out, &f, &o, &[], me(ctx))?;
    }
    Ok(Report { data, passed })
}

fn verify(ctx: &Ctx<'_>, a: &Address, tls: Option<(&str, &str)>, out: &mut dyn Write) -> Result<Report> {
    let (dir, path) = open_dir(ctx, a)?;
    let target = tls.map(|(addr, sni)| TlsTarget { addr, sni });
    let v = flow::verify(&dir, me(ctx), ctx.attestor, &a.host_port(), target)?;
    let mut data = output::attested(&v.fetched, &v.outcome, &v.tls, me(ctx));
    data["pod"] = json!(a.host_port());
    data["dir"] = json!(path.display().to_string());
    data["tlsRootPem"] = json!(v.root_pem.as_deref().map(String::from_utf8_lossy));
    if !ctx.json {
        output::print_attested(out, &v.fetched, &v.outcome, &v.tls, me(ctx))?;
    }
    Ok(Report { data, passed: !v.any_fail() })
}

/// `init` or `unlock`, each step printed as it happens (or, with
/// `--json`, collected into the answer).
fn key_request(ctx: &Ctx<'_>, a: &Address, kind: Kind, out: &mut dyn Write) -> Result<Report> {
    let id = require_key(ctx)?;
    let (dir, path) = open_dir(ctx, a)?;
    let had_key = dir.has_disk_key();
    let mut steps = output::Steps::new(ctx.json);
    let mut failed_write = None;
    let r = flow::key_request(&dir, &owner_key(id), ctx.attestor, &a.host_port(), kind, &mut |s: Step<'_>| {
        if let Err(e) = steps.on(out, s) {
            failed_write.get_or_insert(e);
        }
    });
    if let Err(e) = r {
        let e = coded(e);
        // a key made here and saved before it was sent is the pod's only copy
        // if the pod took it: say where it is
        if kind == Kind::Init && !had_key && dir.has_disk_key() {
            return Err(e.context(format!("the new disk key was saved to {} before it was sent; keep it until the pod is claimed", dir.path(DISK_KEY).display())));
        }
        return Err(e);
    }
    if let Some(e) = failed_write {
        return Err(e).context("printing the steps");
    }
    let mut data = steps.into_json();
    data["pod"] = json!(a.host_port());
    data["dir"] = json!(path.display().to_string());
    data["owner"] = json!(id.npub());
    if kind == Kind::Init {
        let code = dir.disk_key()?.to_hex();
        data["recoveryCode"] = json!(code.as_str());
        if !ctx.json {
            output::print_recovery(out, code.as_str(), &dir.path(DISK_KEY))?;
        }
    }
    Ok(Report { data, passed: true })
}
