//! Where a verified quote and its host registry document come from,
//! behind a trait so the checks are tested offline with a fake. The
//! live one fetches collateral from Intel and the registry from Google,
//! and keeps what it fetched while it is still good: the CLI runs two
//! rounds a minute apart, and the key service runs for weeks.

use crate::verify::collateral::{self, Collateral};
use crate::verify::failure::Failure;
use crate::verify::provenance::{self, HostRegistry};
use crate::verify::time;
use crate::verify::tdx::{self, Verified};
use std::sync::{Arc, Mutex};

pub trait Attestor: Send + Sync {
    /// The quote verified to Intel's root at `at` (unix seconds).
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure>;
    /// Google's host registry document for the quote's PPID.
    fn registry(&self, quote: &[u8]) -> Result<HostRegistry, Failure>;
}

impl<T: Attestor + ?Sized> Attestor for Arc<T> {
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure> {
        (**self).chain(quote, at)
    }
    fn registry(&self, quote: &[u8]) -> Result<HostRegistry, Failure> {
        (**self).registry(quote)
    }
}

/// Platforms (FMSPC and CA) and hosts remembered; past these the oldest
/// is dropped. A pod and its key service are two hosts, often one platform.
const MAX_PLATFORMS: usize = 8;
const MAX_HOSTS: usize = 64;
/// A registry document says where a host is; refetched after an hour,
/// so a host Google moves or retires is noticed.
const REGISTRY_MAX_AGE: i64 = 3600;

#[derive(Default)]
pub struct Live {
    collateral: Mutex<Vec<Collateral>>,
    registries: Mutex<Vec<HostRegistry>>,
}

fn poisoned() -> Failure {
    Failure::Fetch { detail: "cache lock poisoned".into() }
}

fn keep<T>(cache: &mut Vec<T>, item: T, max: usize) {
    if cache.len() >= max {
        cache.remove(0);
    }
    cache.push(item);
}

impl Attestor for Live {
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure> {
        let (fmspc, ca) = collateral::platform_of(quote)?;
        let mut cache = self.collateral.lock().map_err(|_| poisoned())?;
        cache.retain(|c| c.check_fresh(at).is_ok());
        let found = cache.iter().find(|c| c.source.fmspc == fmspc && c.source.ca == ca).cloned();
        let c = match found {
            Some(c) => c,
            None => {
                let c = collateral::fetch(quote)?;
                keep(&mut cache, c.clone(), MAX_PLATFORMS);
                c
            }
        };
        drop(cache);
        tdx::chain(quote, &c, at)
    }

    fn registry(&self, quote: &[u8]) -> Result<HostRegistry, Failure> {
        let ppid = provenance::ppid_of(quote)?;
        let now = time::now();
        let mut cache = self.registries.lock().map_err(|_| poisoned())?;
        cache.retain(|r| now - r.fetched_at < REGISTRY_MAX_AGE);
        if let Some(r) = cache.iter().find(|r| r.ppid.eq_ignore_ascii_case(&ppid)) {
            return Ok(r.clone());
        }
        let r = HostRegistry::fetch(quote)?;
        keep(&mut cache, r.clone(), MAX_HOSTS);
        Ok(r)
    }
}
