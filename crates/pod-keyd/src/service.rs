//! pod-keyd's routes. Its own attestation (`/status`, `/eventlog`) so
//! an owner checks it as a pod is checked; the owner's statements
//! (`/deposit`, `/approve`); the pod's request (`/nonce`, `/release`);
//! and the public view of a record (`/pods/{owner}`).
//!
//! Anyone on the network reaches every route, so none lets a caller who
//! proves nothing take what another needs: `/nonce` keeps nothing, and
//! `/deposit` keeps a record only for an owner the operator named.

use crate::clock::Clock;
use crate::error::Refusal;
use crate::nonces::Challenges;
use crate::store::Store;
use crate::{approve, deposit, release};
use fragment_pod::checks::attestor::Attestor;
use fragment_pod_init::http::{self, HttpError, Request, Response};
use fragment_pod_init::quoter::Quoter;
use fragment_pod_init::eventlog;
use fragment_pod::keyd::binding::KeydStatus;
use fragment_pod::keyd::delivery::{self, MAX_RELEASE_BODY};
use fragment_pod::unlock::keys::BootKey;
use fragment_pod::unlock::wire;
use std::collections::HashSet;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// The largest body any route takes; each route then holds its own.
pub const MAX_BODY: usize = MAX_RELEASE_BODY;
const ROUTES: [&str; 8] = ["/status", "/eventlog", "/healthz", "/nonce", "/pods/<owner>", "/deposit", "/approve", "/release"];

pub struct Service {
    /// Made when the service starts; lives only in this process. Every
    /// deposit and approval is bound to it.
    pub boot: BootKey,
    /// Makes the nonces `/nonce` gives; made at start, as the boot key.
    pub challenges: Challenges,
    /// The owners this service takes deposits from: its operator's list
    /// (`--owner`), fixed at start. It is neither attested nor secret. It
    /// decides who may use the service, never what a key is released to:
    /// that is each owner's own deposit and approvals.
    pub owners: HashSet<[u8; 32]>,
    pub quoter: Box<dyn Quoter + Send + Sync>,
    pub attestor: Box<dyn Attestor>,
    pub clock: Box<dyn Clock>,
    pub ccel: PathBuf,
    pub store: Mutex<Store>,
}

impl Service {
    pub fn new(
        boot: BootKey,
        challenges: Challenges,
        owners: HashSet<[u8; 32]>,
        quoter: Box<dyn Quoter + Send + Sync>,
        attestor: Box<dyn Attestor>,
        clock: Box<dyn Clock>,
        ccel: PathBuf,
    ) -> Self {
        Self { boot, challenges, owners, quoter, attestor, clock, ccel, store: Mutex::new(Store::default()) }
    }

    /// One request at a time, as pod-init: a release fetches collateral
    /// and the registry, so a slow one delays the next. Enough for a
    /// handful of pods; not for many.
    pub fn serve(&self, listener: &TcpListener, max_connections: Option<usize>) {
        http::serve(listener, "pod-keyd", http::Body::UpTo(MAX_BODY), max_connections, &|r| self.respond(r));
    }

    pub fn respond(&self, req: Result<Request, HttpError>) -> Response {
        let result = match req {
            Ok(r) => self.handle(&r),
            Err(e @ HttpError::BodyTooLarge { .. }) => Err(Refusal::TooLarge(e.to_string())),
            Err(e) => Err(Refusal::BadRequest(e.to_string())),
        };
        result.unwrap_or_else(|e| e.response())
    }

    fn handle(&self, req: &Request) -> Result<Response, Refusal> {
        let pods = req.path.strip_prefix("/pods/");
        match (req.method.as_str(), req.path.as_str(), pods) {
            ("GET", _, Some(owner)) => self.pod(owner),
            ("GET", "/status", _) => self.status(req.param("nonce")),
            ("GET", "/eventlog", _) => Ok(eventlog::serve(&self.ccel)),
            ("GET", "/healthz", _) => Ok(Response::text(200, "ok")),
            ("GET", "/nonce", _) => self.nonce(),
            ("POST", "/deposit", _) => deposit::handle(self, &req.body),
            ("POST", "/approve", _) => approve::handle(self, &req.body),
            ("POST", "/release", _) => release::handle(self, &req.body),
            (_, p, pods) if pods.is_some() || ROUTES.contains(&p) => Err(Refusal::Method(format!("{} {p}", req.method))),
            _ => Err(Refusal::NotFound(format!("routes: {}", ROUTES.join(", ")))),
        }
    }

    pub fn store(&self) -> Result<MutexGuard<'_, Store>, Refusal> {
        self.store.lock().map_err(|_| Refusal::Internal("store lock poisoned".into()))
    }

    fn status(&self, nonce: Option<&str>) -> Result<Response, Refusal> {
        let nonce = wire::nonce(nonce.unwrap_or_default()).map_err(|e| Refusal::BadNonce(format!("/status?nonce=<64 hex>: {e}")))?;
        let mut status = KeydStatus { boot_pubkey: self.boot.public(), quote: Vec::new() };
        status.quote = self.quoter.quote(&status.report_data(&nonce)).map_err(Refusal::QuoteFailed)?;
        Ok(Response::json(200, status.to_json().map_err(|e| Refusal::Internal(e.to_string()))?))
    }

    fn nonce(&self) -> Result<Response, Refusal> {
        let nonce = self.challenges.issue(self.clock.now())?;
        Ok(Response::json(200, delivery::nonce_to_json(&nonce).map_err(|e| Refusal::Internal(e.to_string()))?))
    }

    fn pod(&self, owner: &str) -> Result<Response, Refusal> {
        let owner = wire::hex32("owner", owner).map_err(Refusal::wire)?;
        let store = self.store()?;
        let record = store.records.get(&owner).ok_or(Refusal::UnknownOwner)?;
        Ok(Response::json(200, record.view().to_json().map_err(|e| Refusal::Internal(e.to_string()))?))
    }
}
