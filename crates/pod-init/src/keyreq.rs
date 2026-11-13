//! POST /init and POST /unlock, in this order: shape, the signature
//! under the named owner, the seal under this boot's secret, the state,
//! the volume, (unlock) the owner recorded inside it, the open hook.
//! Nothing touches the volume until the request is proven to be the
//! owner's and for this boot; the disk key lives only in this call.

use crate::error::{code, ApiError};
use crate::hook;
use crate::owner;
use crate::service::Service;
use crate::volume::VolumeError;
use crate::http::Response;
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::unlock::request::{self, RequestError};
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::SignError;
use fragment_pod::unlock::wire::{KeyRequest, Opened, WireError};

fn wire_error(e: WireError) -> ApiError {
    match e {
        WireError::TooLarge { .. } => ApiError::new(413, code::TOO_LARGE, e.to_string()),
        WireError::Field { .. } => ApiError::new(400, code::BAD_FIELD, e.to_string()),
        WireError::Json(_) | WireError::Version(_) => ApiError::new(400, code::BAD_REQUEST, e.to_string()),
    }
}

fn request_error(e: RequestError) -> ApiError {
    match e {
        RequestError::Sign(SignError::Owner) => ApiError::new(400, code::BAD_FIELD, e.to_string()),
        RequestError::Sign(_) => ApiError::new(401, code::BAD_SIGNATURE, e.to_string()),
        RequestError::Seal(_) => ApiError::new(403, code::BAD_SEAL, e.to_string()),
    }
}

fn volume_error(e: VolumeError) -> ApiError {
    match e {
        VolumeError::WrongKey => ApiError::new(403, code::WRONG_KEY, e.to_string()),
        VolumeError::Step { .. } => ApiError::new(500, code::VOLUME_FAILED, e.to_string()),
    }
}

pub fn handle(s: &Service, kind: Kind, body: &[u8]) -> Result<Response, ApiError> {
    let r = KeyRequest::from_json(body).map_err(wire_error)?;
    let key = request::open(kind, &s.boot, &r).map_err(request_error)?;
    let needed = match kind {
        Kind::Init => State::Blank,
        Kind::Unlock => State::Locked,
    };
    let state = s.volume.state().map_err(volume_error)?;
    if state != needed {
        let detail = format!("the pod is {}; {} needs it {}", state.as_str(), kind_name(kind), needed.as_str());
        return Err(ApiError::new(409, code::WRONG_STATE, detail));
    }
    match kind {
        Kind::Init => init(s, &r.owner, &key)?,
        Kind::Unlock => unlock(s, &r.owner, &key)?,
    }
    drop(key);
    if let Some(h) = &s.config.open_hook {
        hook::run(h, s.hook_timeout).map_err(|e| ApiError::new(500, code::OPEN_HOOK_FAILED, format!("volume is open; {e}")))?;
    }
    let body = serde_json::to_vec(&Opened { state: State::Open }).map_err(|e| ApiError::new(500, code::INTERNAL, e.to_string()))?;
    Ok(Response::json(200, body))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Init => "init",
        Kind::Unlock => "unlock",
    }
}

/// The owner goes inside the volume (the truth) and beside it (the hint
/// `/status` shows while locked).
fn init(s: &Service, owner: &[u8; 32], key: &DiskKey) -> Result<(), ApiError> {
    s.volume.create(key).map_err(volume_error)?;
    let write = |path: &std::path::Path| {
        owner::write(path, owner)
            .map_err(|e| ApiError::new(500, code::VOLUME_FAILED, format!("volume is open; writing {}: {e}", path.display())))
    };
    write(&s.config.owner_path())?;
    match &s.config.owner_hint {
        Some(hint) => write(hint),
        None => s.volume.set_header_hint(owner).map_err(|e| ApiError::new(500, code::VOLUME_FAILED, format!("volume is open; the header's owner hint: {e}"))),
    }
}

/// The key opened the volume; it is this owner's only if the owner
/// recorded inside agrees. Anyone else holding the key (a copy of
/// disk.key) is refused, and the volume closed again.
fn unlock(s: &Service, owner: &[u8; 32], key: &DiskKey) -> Result<(), ApiError> {
    s.volume.open(key).map_err(volume_error)?;
    let why = match owner::read(&s.config.owner_path()) {
        Ok(Some(o)) if &o == owner => return Ok(()),
        Ok(Some(_)) => "the volume belongs to another owner".to_owned(),
        Ok(None) => "the volume records no owner".to_owned(),
        Err(e) => format!("reading the volume's owner: {e}"),
    };
    match s.volume.close() {
        Ok(()) => Err(ApiError::new(403, code::OWNER_MISMATCH, format!("{why}; closed it again"))),
        Err(e) => Err(ApiError::new(500, code::VOLUME_FAILED, format!("{why}, and closing it failed: {e}"))),
    }
}
