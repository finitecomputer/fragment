//! One round of attestation: a fresh nonce, `/status`, the quote
//! verified to Intel's root and Google's host registry for its PPID,
//! through an Attestor (live: collateral from Intel and the registry
//! from Google, kept for the confirming round right after a key
//! request); then the five checks.

use super::checks::{self, Evidence, Expect, Mode, Outcome};
use super::client::Pod;
use super::dir::Dir;
use super::policy::Policy;
use anyhow::{Context, Result};
use crate::checks::attestor::Attestor;
use crate::unlock::binding::NONCE_LEN;
use crate::unlock::keys;
use crate::unlock::wire::Status;
use crate::verify::failure::Failure;
use crate::verify::provenance::HostRegistry;
use crate::verify::time;
use crate::verify::tdx::Verified;

pub struct Fetched {
    pub nonce: [u8; NONCE_LEN],
    pub status: Status,
    pub chain: Result<Verified, Failure>,
    pub registry: Result<HostRegistry, Failure>,
}

/// The chain and, only if it verified, the registry document.
pub fn attest_quote(a: &dyn Attestor, quote: &[u8]) -> (Result<Verified, Failure>, Result<HostRegistry, Failure>) {
    let chain = a.chain(quote, time::now());
    let registry = match &chain {
        Ok(_) => a.registry(quote),
        Err(_) => Err(Failure::HostRegistry { detail: "not fetched: the quote did not verify".into() }),
    };
    (chain, registry)
}

pub fn fetch(a: &dyn Attestor, pod: &Pod) -> Result<Fetched> {
    let nonce = *keys::random::<NONCE_LEN>()?;
    let status = pod.status(&nonce)?;
    let (chain, registry) = attest_quote(a, &status.quote);
    Ok(Fetched { nonce, status, chain, registry })
}

/// What the checks are held to, from the state directory.
pub struct Trust {
    pub policy: Policy,
    pub last_unlocked_seq: Option<u64>,
    pub me: Option<[u8; 32]>,
}

impl Trust {
    pub fn load(dir: &Dir) -> Result<Self> {
        let policy = Policy::load(&dir.path(super::dir::POLICY)).context("policy.json (docs/pod.md, \"The owner's side\")")?;
        Ok(Self { policy, last_unlocked_seq: dir.state()?.last_unlocked_seq, me: dir.owner_public()? })
    }

    pub fn check(&self, f: &Fetched, mode: Mode) -> Outcome {
        let e = Evidence { nonce: f.nonce, status: &f.status, chain: f.chain.as_ref(), registry: f.registry.as_ref() };
        let x = Expect { policy: &self.policy, last_unlocked_seq: self.last_unlocked_seq, me: self.me, mode };
        checks::run(&e, &x)
    }
}
