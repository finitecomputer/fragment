//! The owner's side of a key service (docs/pod.md, "A key service"):
//! check it as a pod is checked, then deposit this pod's disk key with
//! it, approve a further release, or read the record it holds.
//!
//! The key service is held to `keyd-policy.json` in the pod's folder,
//! policy.json's shape naming the key service's own releases and Google
//! instance. Four checks on a fresh quote, each the pod's with the same
//! code (`checks`): `quote` (Intel's chain, and REPORTDATA is the key
//! service's binding of its boot key and our nonce), `release`,
//! `platform`, `google_instance`. No statement is signed for a key
//! service until all four pass, and every statement names the boot key
//! that quote vouched for, so it applies to that boot alone.

use super::checks::{Check, Outcome};
use super::client::{read_body, refused};
use super::dir::{Dir, POLICY};
use super::flow::Refusal;
use super::policy::Policy;
use crate::checks::attestor::Attestor;
use crate::checks::release::{matches, Matched};
use crate::checks;
use crate::keyd::approve::{Approval, Approve};
use crate::keyd::binding::KeydStatus;
use crate::keyd::deposit::{Deposit, Settings};
use crate::keyd::view::RecordView;
use crate::unlock::binding::NONCE_LEN;
use crate::unlock::keys;
use crate::unlock::release::Release;
use crate::unlock::sign::OwnerKey;
use crate::verify::failure::Failure;
use crate::verify::policy::Status;
use crate::verify::provenance::HostRegistry;
use crate::verify::tdx::Verified;
use crate::verify::time;
use anyhow::{bail, Context, Result};
use std::time::Duration;

/// The key service's policy, in the pod's folder beside policy.json.
pub const KEYD_POLICY: &str = "keyd-policy.json";
/// pod-keyd's port when an address names none.
pub const DEFAULT_PORT: u16 = 7100;
const TIMEOUT: Duration = Duration::from_secs(60);

/// pod-keyd over plain HTTP. Nothing it says is believed for being on
/// the wire: `/status` once its binding checks out, a record's view only
/// as far as the next signed statement, which the key service refuses if
/// the view lied.
pub struct KeyService {
    base: String,
    client: reqwest::blocking::Client,
}

impl KeyService {
    /// `host` is host:port.
    pub fn new(host: &str) -> Result<Self> {
        if host.contains('/') {
            bail!("{host:?} is host:port, not a URL");
        }
        let client = reqwest::blocking::Client::builder().user_agent("pod").timeout(TIMEOUT).build()?;
        Ok(Self { base: format!("http://{host}"), client })
    }

    fn call(&self, route: &'static str, path: &str, body: Option<Vec<u8>>) -> Result<(u16, Vec<u8>)> {
        let url = format!("{}{path}", self.base);
        let req = match body {
            None => self.client.get(&url),
            Some(b) => self.client.post(&url).header("content-type", "application/json").body(b),
        };
        let resp = req.send().with_context(|| format!("{route} at {url}"))?;
        read_body(resp)
    }

    fn ok(&self, route: &'static str, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>> {
        let (status, answer) = self.call(route, path, body)?;
        if status != 200 {
            return Err(refused("key service", route, status, &answer));
        }
        Ok(answer)
    }

    pub fn status(&self, nonce: &[u8; NONCE_LEN]) -> Result<KeydStatus> {
        let body = self.ok("/status", &format!("/status?nonce={}", hex::encode(nonce)), None)?;
        KeydStatus::from_json(&body).context("the key service's /status")
    }

    /// The record it holds for `owner`; None when it holds none.
    pub fn record(&self, owner: &[u8; 32]) -> Result<Option<RecordView>> {
        let (status, body) = self.call("/pods", &format!("/pods/{}", hex::encode(owner)), None)?;
        let none = serde_json::from_slice::<crate::unlock::wire::ErrorBody>(&body).is_ok_and(|e| e.error == "unknown_owner");
        match status {
            200 => RecordView::from_json(&body).map(Some).context("the key service's /pods"),
            404 if none => Ok(None),
            _ => Err(refused("key service", "/pods", status, &body)),
        }
    }

    pub fn deposit(&self, d: &Deposit) -> Result<RecordView> {
        let body = self.ok("/deposit", "/deposit", Some(d.to_json()?))?;
        RecordView::from_json(&body).context("the key service's answer to /deposit")
    }

    pub fn approve(&self, a: &Approve) -> Result<RecordView> {
        let body = self.ok("/approve", "/approve", Some(a.to_json()?))?;
        RecordView::from_json(&body).context("the key service's answer to /approve")
    }
}

/// One round of the key service's attestation.
pub struct Fetched {
    pub nonce: [u8; NONCE_LEN],
    pub status: KeydStatus,
    pub chain: Result<Verified, Failure>,
    pub registry: Result<HostRegistry, Failure>,
}

pub fn fetch(a: &dyn Attestor, ks: &KeyService) -> Result<Fetched> {
    let nonce = *keys::random::<NONCE_LEN>()?;
    let status = ks.status(&nonce)?;
    let (chain, registry) = super::attest::attest_quote(a, &status.quote);
    Ok(Fetched { nonce, status, chain, registry })
}

/// The newest of the key service's releases that the quote is.
fn release(releases: &[Release], v: &Verified) -> Result<Matched, String> {
    let best = releases.iter().filter(|r| matches(r, &v.report)).max_by_key(|r| r.seq);
    best.map(Matched::of).ok_or_else(|| format!("no release in {KEYD_POLICY} has these registers ({} listed)", releases.len()))
}

/// The four checks, in the pod's order; if the quote fails, nothing the
/// key service said is trusted and the rest are skipped.
pub fn check(f: &Fetched, p: &Policy) -> Outcome {
    let v = match checks::quote(f.chain.as_ref(), &f.status.report_data(&f.nonce)) {
        Ok(v) => v,
        Err(why) => {
            let mut checks = vec![Check::new("quote", Err(why))];
            for name in ["release", "platform", "google_instance"] {
                checks.push(Check::skipped(name, "the quote did not verify, so nothing it carries is trusted"));
            }
            return Outcome { checks, release: None };
        }
    };
    let matched = release(&p.releases, v);
    let checks = vec![
        Check::new("quote", Ok("Intel root, collateral fresh; REPORTDATA binds nonce and the key service's boot key".into())),
        Check::new("release", matched.as_ref().map(Matched::detail).map_err(Clone::clone)),
        Check::new("platform", checks::platform(v, &p.allow_tcb_status)),
        Check::new("google_instance", checks::google_instance(v, &p.gce_instance, f.registry.as_ref())),
    ];
    Outcome { checks, release: matched.ok() }
}

fn keyd_policy(dir: &Dir) -> Result<Policy> {
    Policy::load(&dir.path(KEYD_POLICY)).context("keyd-policy.json: the key service's releases and Google instance (docs/pod.md, \"A key service\")")
}

/// What `status` found: the checks, and the record held for this key.
pub struct Report {
    pub fetched: Fetched,
    pub outcome: Outcome,
    /// Unattested (plain HTTP): what the key service says it holds.
    pub record: Option<RecordView>,
}

/// The key service's checks and this owner's record, changing nothing.
pub fn status(dir: &Dir, me: Option<[u8; 32]>, a: &dyn Attestor, host: &str) -> Result<Report> {
    let p = keyd_policy(dir)?;
    let ks = KeyService::new(host)?;
    let fetched = fetch(a, &ks)?;
    let outcome = check(&fetched, &p);
    let record = match me {
        Some(me) => ks.record(&me)?,
        None => None,
    };
    Ok(Report { fetched, outcome, record })
}

/// A fresh attestation that passes every check, or a refusal naming the
/// first that did not; `on` sees the checks first either way.
fn attested(dir: &Dir, a: &dyn Attestor, ks: &KeyService, on: &mut dyn FnMut(&[Check])) -> Result<Fetched> {
    let p = keyd_policy(dir)?;
    let f = fetch(a, ks)?;
    let o = check(&f, &p);
    on(&o.checks);
    if let Some(c) = o.first_not_passed() {
        return Err(Refusal(format!("the key service's {} check did not pass: {}", c.name, c.detail)).into());
    }
    Ok(f)
}

/// Deposit this pod's disk key with the key service: the instance and
/// TCB statuses of policy.json, its releases at or above the floor as
/// the approvals, and the floor (state.json's) as min_seq. It replaces
/// the record held for this key, if any; the key service keeps that
/// record's min_seq if it is higher, having raised it with each release
/// it served.
///
/// The floor signed is this machine's alone. What the key service says
/// about the record it holds is an answer no one vouches for: its id is
/// taken from it, and a wrong one only gets the deposit refused; its
/// min_seq is not, since anyone on the path could raise it, and a floor
/// signed that high would hold.
pub fn deposit(dir: &Dir, me: &OwnerKey, a: &dyn Attestor, host: &str, on: &mut dyn FnMut(&[Check])) -> Result<RecordView> {
    let pod = Policy::load(&dir.path(POLICY)).context("policy.json")?;
    let floor = dir.state()?.last_unlocked_seq.unwrap_or(0);
    let approvals: Vec<Approval> =
        pod.releases.into_iter().filter(|r| r.seq >= floor).map(|release| Approval { release, not_before: None, expires: None }).collect();
    if approvals.is_empty() {
        return Err(Refusal(format!("policy.json has no release at or above the floor (seq {floor}): nothing to approve")).into());
    }
    let key = dir.disk_key().context("this pod's disk key (claim it with `fragment pod init` first)")?;
    let ks = KeyService::new(host)?;
    let f = attested(dir, a, &ks, on)?;
    let replaces = ks.record(&me.public())?.map(|v| v.record);
    let settings = Settings { replaces, gce_instance: pod.gce_instance_text, allow_tcb_status: pod.allow_tcb_status, min_seq: floor, approvals };
    let d = Deposit::make(me, &f.status.boot_pubkey, &key, settings)?;
    drop(key);
    let view = ks.deposit(&d)?;
    if view.record != d.id(&f.status.boot_pubkey) || view.owner != me.public() {
        return Err(Refusal("the key service answered with a record that is not this deposit's".into()).into());
    }
    Ok(view)
}

/// When an approval opens and closes, unix seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    pub not_before: Option<u64>,
    pub expires: Option<u64>,
}

/// Approve one more release (a release file's) for the record the key
/// service holds for this key.
pub fn approve(dir: &Dir, me: &OwnerKey, a: &dyn Attestor, host: &str, release: Release, w: Window, on: &mut dyn FnMut(&[Check])) -> Result<RecordView> {
    let ks = KeyService::new(host)?;
    let f = attested(dir, a, &ks, on)?;
    let held = ks.record(&me.public())?.ok_or_else(|| {
        Refusal("the key service holds no record for this key: deposit first (it holds nothing after it restarts)".into())
    })?;
    let approval = Approval { release, not_before: w.not_before, expires: w.expires };
    let signed = Approve::make(me, &f.status.boot_pubkey, held.record, approval)?;
    ks.approve(&signed)
}

/// Seconds since 1970, for a window from now.
pub fn now() -> u64 {
    u64::try_from(time::now()).unwrap_or(0)
}

impl Report {
    pub fn any_fail(&self) -> bool {
        self.outcome.checks.iter().any(|c| c.status == Status::Fail)
    }
}
