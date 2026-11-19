//! POST /release: a boot of a pod asks for its owner's disk key. First
//! the nonce: one of this boot's, made in the last minute, and not one a
//! release was already made for. Then the record of the owner the pod's
//! status names, and the checks the owner's CLI runs, in its order,
//! refusing at the first that fails: `quote` (Intel's chain, and
//! REPORTDATA is the pod's binding of its status and our nonce),
//! `release` (an approval, open now, not below min_seq), `platform`,
//! `google_instance` (the record's), and the pod's state is `locked`.
//! Only then is the nonce spent and min_seq raised to the release
//! matched, so an older one is refused from then on, and after that the
//! key sealed to the boot key the quote vouched for.
//!
//! A refused request spends nothing: what is kept (nonces.rs) is added
//! to only by a boot that passed every check, so a caller who proves
//! nothing cannot fill it.

use crate::decide;
use crate::error::Refusal;
use crate::service::Service;
use fragment_pod::attest::pzid::Instance;
use fragment_pod::checks;
use fragment_pod::checks::release::Matched;
use fragment_pod_init::http::Response;
use fragment_pod::unlock::binding::State;
use fragment_pod::keyd::approve::Approval;
use fragment_pod::keyd::delivery::{Delivery, ReleaseRequest};

/// What the checks need from the record, copied out so the store is not
/// held while collateral and the registry are fetched.
struct Held {
    id: [u8; 32],
    instance: Instance,
    allow_tcb_status: Vec<String>,
    approvals: Vec<Approval>,
    min_seq: u64,
}

pub fn handle(s: &Service, body: &[u8]) -> Result<Response, Refusal> {
    let req = ReleaseRequest::from_json(body).map_err(Refusal::wire)?;
    let now = s.clock.now();
    let nonce_time = s.challenges.check(&req.nonce, now)?;
    let owner = req.status.owner.ok_or(Refusal::NoOwner)?;
    let held = held(s, &owner, &req)?;
    let matched = check(s, &req, &held, now)?;
    let mut guard = s.store()?;
    let store = &mut *guard;
    let record = store.records.get_mut(&owner).filter(|r| r.id == held.id).ok_or(Refusal::UnknownRecord)?;
    // Raised by another release while this one was checked: hold to it.
    if matched.seq < record.min_seq {
        return Err(Refusal::Rollback { name: matched.name, seq: matched.seq, min_seq: record.min_seq });
    }
    // Spent by this release alone, having passed every check; refused
    // if another release took it while this one was checked.
    store.spent.spend(owner, req.nonce, nonce_time)?;
    // Recorded before the key is sealed, as the owner's CLI saves its
    // floor before it sends one: if the delivery is made and lost, a boot
    // of an older release still gets nothing.
    record.min_seq = record.min_seq.max(matched.seq);
    record.last_release = Some(now);
    let delivery = Delivery::make(&s.boot, owner, record.grant, matched.seq, &req.status.boot_pubkey, &record.disk_key)
        .map_err(|e| Refusal::Internal(e.to_string()))?;
    Ok(Response::json(200, delivery.to_json().map_err(|e| Refusal::Internal(e.to_string()))?))
}

fn held(s: &Service, owner: &[u8; 32], req: &ReleaseRequest) -> Result<Held, Refusal> {
    let store = s.store()?;
    let r = store.records.get(owner).ok_or(Refusal::UnknownOwner)?;
    if store.spent.contains(owner, &req.nonce) {
        return Err(Refusal::UnknownNonce);
    }
    Ok(Held {
        id: r.id,
        instance: r.instance.clone(),
        allow_tcb_status: r.allow_tcb_status.clone(),
        approvals: r.approvals.clone(),
        min_seq: r.min_seq,
    })
}

fn check(s: &Service, req: &ReleaseRequest, held: &Held, now: i64) -> Result<Matched, Refusal> {
    let chain = s.attestor.chain(&req.status.quote, now);
    let expected = req.status.binding(req.nonce).report_data();
    let v = checks::quote(chain.as_ref(), &expected).map_err(Refusal::Quote)?;
    let matched = decide::choose(&held.approvals, &v.report, held.min_seq, now)?;
    checks::platform(v, &held.allow_tcb_status).map_err(Refusal::Platform)?;
    let registry = s.attestor.registry(&req.status.quote);
    checks::google_instance(v, &held.instance, registry.as_ref()).map_err(Refusal::GoogleInstance)?;
    if req.status.state != State::Locked {
        return Err(Refusal::WrongState(req.status.state));
    }
    Ok(matched)
}
