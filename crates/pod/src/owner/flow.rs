//! The owner's actions, in their safe order. A key leaves this machine
//! only after all five checks pass on a fresh attestation, and only sealed
//! to the boot key that attestation vouched for; after the pod says
//! "open", a second fresh attestation must show it open, owned by this
//! key, on the same boot. What a person sees of each step is the
//! caller's: a step is handed to `on` as it happens (the CLI prints it).

use super::attest::{self, Fetched, Trust};
use super::checks::{Check, Mode, Outcome};
use super::client::Pod;
use super::dir::{Dir, PodState, DISK_KEY};
use super::tlscheck;
use crate::checks::attestor::Attestor;
use crate::checks::release::Matched;
use crate::unlock::binding::State;
use crate::unlock::keys::DiskKey;
use crate::unlock::request;
use crate::unlock::seal::Kind;
use crate::unlock::sign::OwnerKey;
use crate::verify::policy::Status;
use anyhow::{bail, Result};
use rustls::pki_types::CertificateDer;

/// A check failed or the pod is in the wrong state: the CLI's exit 1,
/// not 2 (a usage, I/O or connection error).
#[derive(Debug)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for Refusal {}

fn refuse<T>(why: String) -> Result<T> {
    Err(Refusal(why).into())
}

/// A step of a key request, as it happens.
#[derive(Debug)]
pub enum Step<'a> {
    /// The checks of the first attestation, before any key is sent.
    Checked(&'a [Check]),
    /// The pod answered the key request "open"; confirming.
    Sent,
    /// The confirming attestation's checks, before they are required.
    Confirming(&'a [Check]),
    /// Confirmed: attested open, on the same boot, owned by `owner`.
    Confirmed { owner: [u8; 32] },
    /// The release unlocked, now the oldest this owner will unlock again.
    Recorded(&'a Matched),
}

/// A new owner key in `dir` (never over one there); its public key.
pub fn keygen(dir: &Dir) -> Result<[u8; 32]> {
    let key = OwnerKey::generate()?;
    dir.write_owner_key(&key)?;
    Ok(key.public())
}

/// One round of attestation and the checks, changing nothing.
pub fn status(dir: &Dir, a: &dyn Attestor, host: &str) -> Result<(Fetched, Outcome)> {
    let ctx = Trust::load(dir)?;
    let f = attest::fetch(a, &Pod::new(host)?)?;
    let o = ctx.check(&f, Mode::Status);
    Ok((f, o))
}

/// All five checks pass, and the pod is in `state`.
pub fn require(o: &Outcome, f: &Fetched, state: State) -> Result<()> {
    if let Some(c) = o.first_not_passed() {
        return refuse(format!("the {} check did not pass: {}", c.name, c.detail));
    }
    if f.status.state != state {
        return refuse(format!("the pod is {}; this needs it {}", f.status.state.as_str(), state.as_str()));
    }
    Ok(())
}

/// After a key request: a fresh attestation shows the same boot, open,
/// owned by this key, with every check passing.
fn confirm(ctx: &Trust, pod: &Pod, a: &dyn Attestor, boot_pubkey: [u8; 32], on: &mut dyn FnMut(Step<'_>)) -> Result<Outcome> {
    let f = attest::fetch(a, pod)?;
    let o = ctx.check(&f, Mode::Unlock);
    on(Step::Confirming(&o.checks));
    require(&o, &f, State::Open)?;
    if f.status.boot_pubkey != boot_pubkey {
        return refuse("the pod rebooted between the request and the confirmation".into());
    }
    on(Step::Confirmed { owner: f.status.owner.unwrap_or_default() });
    Ok(o)
}

fn record_release(dir: &Dir, o: &Outcome, on: &mut dyn FnMut(Step<'_>)) -> Result<()> {
    if let Some(m) = &o.release {
        dir.write_state(&PodState { last_unlocked_seq: Some(m.seq) })?;
        on(Step::Recorded(m));
    }
    Ok(())
}

/// `init` (a blank pod: a new disk key, saved before it is sent) or
/// `unlock` (a locked pod: the saved disk key), each step handed to `on`.
pub fn key_request(dir: &Dir, a: &dyn Attestor, host: &str, kind: Kind, on: &mut dyn FnMut(Step<'_>)) -> Result<()> {
    let (mode, state) = match kind {
        Kind::Init => (Mode::Init, State::Blank),
        Kind::Unlock => (Mode::Unlock, State::Locked),
    };
    let me = dir.owner_key()?;
    if kind == Kind::Init && dir.has_disk_key() {
        bail!("{} exists: this directory already holds a pod's disk key", dir.path(DISK_KEY).display());
    }
    let ctx = Trust::load(dir)?;
    let pod = Pod::new(host)?;
    let f = attest::fetch(a, &pod)?;
    let o = ctx.check(&f, mode);
    on(Step::Checked(&o.checks));
    require(&o, &f, state)?;
    let key = match kind {
        Kind::Init => {
            // Saved before it is sent, so a lost answer never loses the key.
            let key = DiskKey::random()?;
            dir.write_disk_key(&key)?;
            key
        }
        Kind::Unlock => dir.disk_key()?,
    };
    let req = request::make(kind, &f.status.boot_pubkey, &me, &key)?;
    drop(key);
    pod.send(kind, &req)?;
    on(Step::Sent);
    confirm(&ctx, &pod, a, f.status.boot_pubkey, on)?;
    record_release(dir, &o, on)
}

/// A TLS server whose leaf must chain to the attested root.
pub struct TlsTarget<'a> {
    pub addr: &'a str,
    pub sni: &'a str,
}

/// What `verify` found: the attestation, its checks, the TLS checks
/// after them, and the attested root's PEM if `/tls-root` served it.
pub struct Verification {
    pub fetched: Fetched,
    pub outcome: Outcome,
    pub tls: Vec<Check>,
    pub root_pem: Option<Vec<u8>>,
}

impl Verification {
    pub fn any_fail(&self) -> bool {
        self.outcome.any_fail() || self.tls.iter().any(|c| c.status == Status::Fail)
    }
}

/// The checks, then `tls_root` (the served PEM hashes to the attested
/// root) and, with `tls`, `tls_leaf` (a handshake trusting only that root).
pub fn verify(dir: &Dir, a: &dyn Attestor, host: &str, tls: Option<TlsTarget<'_>>) -> Result<Verification> {
    let ctx = Trust::load(dir)?;
    let pod = Pod::new(host)?;
    let f = attest::fetch(a, &pod)?;
    let o = ctx.check(&f, Mode::Status);
    let mut extra = Vec::new();
    let root: Option<(Vec<u8>, CertificateDer<'static>)> = if o.checks.first().is_some_and(|c| c.status == Status::Pass) {
        let r = pod.tls_root().map_err(|e| e.to_string()).and_then(|pem| tlscheck::check_root(&pem, f.status.tls_root).map(|d| (pem, d)));
        extra.push(Check::new("tls_root", r.as_ref().map(|_| "/tls-root is the attested root".to_owned()).map_err(Clone::clone)));
        r.ok()
    } else {
        extra.push(Check::skipped("tls_root", "the quote did not verify"));
        None
    };
    match (&tls, &root) {
        (Some(t), Some((_, der))) => extra.push(Check::new("tls_leaf", tlscheck::check_leaf(t.addr, t.sni, der.clone()))),
        (Some(_), None) => extra.push(Check::skipped("tls_leaf", "no attested root to check against")),
        (None, _) => {}
    }
    Ok(Verification { fetched: f, outcome: o, tls: extra, root_pem: root.map(|(pem, _)| pem) })
}
