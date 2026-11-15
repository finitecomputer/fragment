//! POST /deposit, in this order: shape, that the owner is one this
//! service serves, the owner's signature for this boot of the key
//! service, the grant, the seal, the settings, then the record it
//! replaces. The answer is the record's public view.
//!
//! Admission comes before anything is verified or kept. A signature only
//! shows that the depositor holds the key the deposit names, and anyone
//! can make keys: without it, whoever made the most would fill the
//! service's records and no owner after them could deposit.

use crate::error::Refusal;
use crate::service::Service;
use crate::store::Record;
use fragment_pod_init::http::Response;
use fragment_pod::keyd::deposit::Deposit;
use fragment_pod::keyd::KeydError;
use fragment_pod::unlock::sign::SignError;

fn opened(e: KeydError) -> Refusal {
    match e {
        KeydError::Sign(SignError::Owner) => Refusal::BadField(e.to_string()),
        KeydError::Sign(_) => Refusal::BadSignature(e.to_string()),
        KeydError::Grant => Refusal::BadGrant,
        KeydError::Seal(_) => Refusal::BadSeal(e.to_string()),
        KeydError::Wire(w) => Refusal::wire(w),
        KeydError::Owner => Refusal::BadField(e.to_string()),
    }
}

pub fn handle(s: &Service, body: &[u8]) -> Result<Response, Refusal> {
    let d = Deposit::from_json(body).map_err(Refusal::wire)?;
    if !s.owners.contains(&d.owner) {
        return Err(Refusal::NotAdmitted);
    }
    let key = d.open(&s.boot).map_err(opened)?;
    let record = Record::new(d.owner, d.id(&s.boot.public()), &d.settings, key, d.grant)?;
    let mut store = s.store()?;
    let view = store.deposit(record, d.settings.replaces)?.view();
    Ok(Response::json(200, view.to_json().map_err(|e| Refusal::Internal(e.to_string()))?))
}
