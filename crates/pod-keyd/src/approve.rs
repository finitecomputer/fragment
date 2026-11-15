//! POST /approve: shape, the signature under the owner it names for
//! this boot of the key service, the record it names, that the signer
//! is that record's owner, then the record's own rules (seq not below
//! min_seq, not already approved, within the limit).

use crate::error::Refusal;
use crate::service::Service;
use fragment_pod_init::http::Response;
use fragment_pod::keyd::approve::Approve;
use fragment_pod::unlock::sign::SignError;

pub fn handle(s: &Service, body: &[u8]) -> Result<Response, Refusal> {
    let a = Approve::from_json(body).map_err(Refusal::wire)?;
    a.verify(&s.boot.public()).map_err(|e| match e {
        SignError::Owner => Refusal::BadField(e.to_string()),
        _ => Refusal::BadSignature(e.to_string()),
    })?;
    let mut store = s.store()?;
    let record = store.by_id(&a.record).ok_or(Refusal::UnknownRecord)?;
    if record.owner != a.owner {
        return Err(Refusal::NotOwner);
    }
    record.approve(a.approval)?;
    Ok(Response::json(200, record.view().to_json().map_err(|e| Refusal::Internal(e.to_string()))?))
}
