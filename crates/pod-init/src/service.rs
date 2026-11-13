//! pod-init's routes: `/status` (the attested state), `/eventlog`,
//! `/tls-root`, and the two key requests (src/keyreq.rs). Nothing is
//! remembered between requests but the boot key: the state, the owner
//! and the TLS root are read from the system each time.

use crate::config::Config;
use crate::error::{code, ApiError};
use crate::keyreq;
use crate::owner;
use crate::quoter::Quoter;
use crate::volume::Volume;
use crate::http::{self, HttpError, Request, Response};
use crate::eventlog;
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::BootKey;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::tls::{self, MAX_PEM};
use fragment_pod::unlock::wire::{self, Status};
use std::io::{self, Read};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

pub const MAX_BODY: usize = wire::MAX_REQUEST_BODY;
const ROUTES: [&str; 6] = ["/status", "/eventlog", "/tls-root", "/healthz", "/init", "/unlock"];

pub struct Service {
    pub config: Config,
    pub ccel: PathBuf,
    pub volume: Box<dyn Volume + Send + Sync>,
    pub quoter: Box<dyn Quoter + Send + Sync>,
    /// Made when the service starts; lives only in this process.
    pub boot: BootKey,
    pub hook_timeout: Duration,
}

impl Service {
    /// One request at a time, so two key requests never race on the volume.
    pub fn serve(&self, listener: &TcpListener, max_connections: Option<usize>) {
        http::serve(listener, "pod-init", http::Body::UpTo(MAX_BODY), max_connections, &|r| self.respond(r));
    }

    pub fn respond(&self, req: Result<Request, HttpError>) -> Response {
        let result = match req {
            Ok(r) => self.handle(&r),
            Err(e @ HttpError::BodyTooLarge { .. }) => Err(ApiError::new(413, code::TOO_LARGE, e.to_string())),
            Err(e) => Err(ApiError::new(400, code::BAD_REQUEST, e.to_string())),
        };
        result.unwrap_or_else(|e| e.response())
    }

    fn handle(&self, req: &Request) -> Result<Response, ApiError> {
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/status") => self.status(req.param("nonce")),
            ("GET", "/eventlog") => Ok(eventlog::serve(&self.ccel)),
            ("GET", "/tls-root") => self.tls_root_pem(),
            ("GET", "/healthz") => Ok(Response::text(200, "ok")),
            ("POST", "/init") => keyreq::handle(self, Kind::Init, &req.body),
            ("POST", "/unlock") => keyreq::handle(self, Kind::Unlock, &req.body),
            (_, p) if ROUTES.contains(&p) => Err(ApiError::new(405, code::METHOD, format!("{} {p}", req.method))),
            _ => Err(ApiError::new(404, code::NOT_FOUND, format!("routes: {}", ROUTES.join(", ")))),
        }
    }

    /// The owner `/status` reports, by state. While locked it is the hint
    /// beside the volume, which the host can rewrite: it only tells the
    /// owner's CLI which pod claims to be theirs. A forged hint gains the
    /// host nothing: the key still only opens under the boot key of a TD
    /// running an attested release, and unlock checks the owner recorded
    /// inside the volume. Once open it is that file, written at init
    /// inside the encrypted volume, which only the TD can read.
    /// The hint is a file, or (no file configured) the volume's header.
    pub fn owner(&self, state: State) -> Option<[u8; 32]> {
        let read = match (state, &self.config.owner_hint) {
            (State::Blank, _) => return None,
            (State::Locked, Some(hint)) => owner::read(hint),
            (State::Locked, None) => self.volume.header_hint().map_err(|e| e.to_string()),
            (State::Open, _) => owner::read(&self.config.owner_path()),
        };
        read.unwrap_or_else(|e| {
            eprintln!("pod-init: owner: {e}");
            None
        })
    }

    fn read_tls_root(&self) -> io::Result<Vec<u8>> {
        let mut pem = Vec::new();
        std::fs::File::open(&self.config.tls_root)?.take(MAX_PEM as u64 + 1).read_to_end(&mut pem)?;
        Ok(pem)
    }

    /// None when there is no root yet, or it cannot be read: the binding
    /// then says "no root", and a client wanting one refuses.
    fn tls_root(&self) -> Option<[u8; 32]> {
        let pem = match self.read_tls_root() {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                eprintln!("pod-init: tls root {}: {e}", self.config.tls_root.display());
                return None;
            }
        };
        tls::root_hash(&pem).map_err(|e| eprintln!("pod-init: {e}")).ok()
    }

    fn tls_root_pem(&self) -> Result<Response, ApiError> {
        match self.read_tls_root() {
            Ok(pem) => Ok(Response { status: 200, content_type: "application/x-pem-file", body: pem }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ApiError::new(404, code::NOT_FOUND, "no TLS root yet")),
            Err(e) => Err(ApiError::new(500, code::INTERNAL, format!("reading the TLS root: {e}"))),
        }
    }

    fn status(&self, nonce: Option<&str>) -> Result<Response, ApiError> {
        let nonce = wire::nonce(nonce.unwrap_or_default())
            .map_err(|e| ApiError::new(400, code::BAD_NONCE, format!("/status?nonce=<64 hex>: {e}")))?;
        let state = self.volume.state().map_err(|e| ApiError::new(500, code::VOLUME_FAILED, e.to_string()))?;
        let mut status = Status {
            state,
            boot_pubkey: self.boot.public(),
            owner: self.owner(state),
            tls_root: self.tls_root(),
            quote: Vec::new(),
        };
        let report_data = status.binding(nonce).report_data();
        status.quote = self.quoter.quote(&report_data).map_err(|e| ApiError::new(500, code::QUOTE_FAILED, e))?;
        let body = status.to_json().map_err(|e| ApiError::new(500, code::INTERNAL, e.to_string()))?;
        Ok(Response::json(200, body))
    }
}
