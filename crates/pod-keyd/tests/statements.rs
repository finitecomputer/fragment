//! The owner's statements to the key service, `/deposit` and
//! `/approve`: valid, invalid, replayed, and after a restart. Also the
//! key service's own attestation and the public view of a record.

mod common;

use common::{approval, code, Keyd, Owner, Td, KEYD, POD};
use fragment_pod::keyd::approve::{Approval, Approve};
use fragment_pod::keyd::binding;
use fragment_pod::keyd::deposit::{Deposit, MAX_DEPOSIT_BODY};
use fragment_pod::unlock::sign::OwnerKey;

fn pod_td(k: &Keyd) -> Td {
    k.attestor.td(POD)
}

/// An owner whose record holds approvals for seq 1 and 2, min_seq 1.
fn deposited(k: &mut Keyd) -> (Owner, [u8; 32]) {
    let owner = Owner::new();
    k.admit(&owner);
    let td = pod_td(k);
    let mut s = owner.settings(vec![approval(td.release(1)), approval(td.release(2))]);
    s.min_seq = 1;
    let r = k.deposit(&owner.deposit_to(k, s));
    assert_eq!(code(&r), (200, "ok".into()), "{}", common::text(&r));
    let id = k.view(&owner.public()).unwrap().record;
    (owner, id)
}

fn approve(k: &Keyd, signer: &OwnerKey, record: [u8; 32], a: Approval) -> (u16, String) {
    let body = Approve::make(signer, &k.pubkey(), record, a).unwrap().to_json().unwrap();
    code(&k.post("/approve", &body))
}

#[test]
fn the_key_service_attests_its_boot_key_and_serves_its_event_log() {
    let k = Keyd::new();
    let s = k.status([7; 32]);
    assert_eq!(s.boot_pubkey, k.pubkey());
    assert_eq!(&s.quote[..5], &KEYD);
    assert_eq!(&s.quote[5..], &binding::report_data(&[7; 32], &k.pubkey()));
    assert_eq!(k.get("/eventlog").body, b"KEYD LOG");
    assert_eq!(code(&k.get("/status?nonce=00")), (400, "bad_nonce".into()));
    assert_eq!(code(&k.get("/nowhere")).0, 404);
    assert_eq!(code(&k.get("/deposit")).0, 405);
    assert_eq!(code(&k.post("/pods/00", b"")).0, 405);
}

#[test]
fn a_deposit_makes_a_record_whose_view_holds_no_key() {
    let mut k = Keyd::new();
    let (owner, id) = deposited(&mut k);
    let v = k.view(&owner.public()).unwrap();
    assert_eq!((v.owner, v.record, v.min_seq, v.approvals.len(), v.last_release), (owner.public(), id, 1, 2, None));
    let text = common::text(&k.get(&format!("/pods/{}", hex::encode(owner.public()))));
    assert!(!text.contains(&owner.disk.to_hex().as_str()[..16]), "no disk key in the view");
    assert_eq!(k.view(&[9; 32]).unwrap_err().error, "unknown_owner");
    assert_eq!(code(&k.get("/pods/zz")).0, 400);
}

#[test]
fn a_tampered_or_misdirected_deposit_is_refused() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let mut d = owner.deposit_to(&k, owner.settings(vec![]));
    d.settings.min_seq = 9;
    assert_eq!(code(&k.deposit(&d)), (401, "bad_signature".into()));
    let mut d = owner.deposit_to(&k, owner.settings(vec![]));
    d.ct[0] ^= 1;
    assert_eq!(code(&k.deposit(&d)), (401, "bad_signature".into()), "the sealed key is signed too");
    // Signed correctly over a seal for another key service's boot key.
    let other = Keyd::new();
    let d = Deposit::make(&owner.key, &other.pubkey(), &owner.disk, owner.settings(vec![])).unwrap();
    assert_eq!(code(&k.deposit(&d)), (401, "bad_signature".into()));
    assert_eq!(k.view(&owner.public()).unwrap_err().error, "unknown_owner");
}

#[test]
fn a_deposit_with_bad_settings_or_body_is_refused() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let mut s = owner.settings(vec![approval(pod_td(&k).release(1))]);
    s.min_seq = 2;
    assert_eq!(code(&k.deposit(&owner.deposit_to(&k, s))), (409, "below_min_seq".into()));
    let mut s = owner.settings(vec![]);
    s.gce_instance = "projects/x/zones/z/instances/y".into();
    assert_eq!(code(&k.deposit(&owner.deposit_to(&k, s))), (400, "bad_field".into()));
    let mut s = owner.settings(vec![]);
    s.allow_tcb_status = vec!["Revoked".into()];
    assert_eq!(code(&k.deposit(&owner.deposit_to(&k, s))), (400, "bad_field".into()));
    assert_eq!(code(&k.post("/deposit", b"{")), (400, "bad_request".into()));
    assert_eq!(code(&k.post("/deposit", &vec![b' '; MAX_DEPOSIT_BODY + 1])), (413, "too_large".into()));
}

#[test]
fn a_deposit_replayed_to_the_same_boot_cannot_roll_the_record_back() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let first = owner.deposit_to(&k, owner.settings(vec![]));
    assert_eq!(code(&k.deposit(&first)).0, 200);
    assert_eq!(code(&k.deposit(&first)), (409, "stale_record".into()), "replayed");
    // A newer deposit raises min_seq over the first...
    let mut s = owner.settings(vec![]);
    s.replaces = Some(first.id(&k.pubkey()));
    s.min_seq = 5;
    let second = owner.deposit_to(&k, s);
    assert_eq!(code(&k.deposit(&second)).0, 200);
    assert_eq!(k.view(&owner.public()).unwrap().min_seq, 5);
    // ...and neither earlier deposit can put min_seq 0 back.
    assert_eq!(code(&k.deposit(&first)), (409, "stale_record".into()));
    assert_eq!(code(&k.deposit(&second)), (409, "stale_record".into()));
    assert_eq!(k.view(&owner.public()).unwrap().min_seq, 5);
}

#[test]
fn a_deposit_replayed_to_a_restarted_key_service_is_refused() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let d = owner.deposit_to(&k, owner.settings(vec![]));
    assert_eq!(code(&k.deposit(&d)).0, 200);
    k.restart();
    assert_eq!(k.view(&owner.public()).unwrap_err().error, "unknown_owner", "a restart forgets");
    assert_eq!(code(&k.deposit(&d)), (401, "bad_signature".into()));
}

#[test]
fn an_approval_adds_a_release_once() {
    let mut k = Keyd::new();
    let (owner, id) = deposited(&mut k);
    let a = approval(pod_td(&k).release(3));
    assert_eq!(approve(&k, &owner.key, id, a.clone()), (200, "ok".into()));
    assert_eq!(k.view(&owner.public()).unwrap().approvals.last(), Some(&a));
    assert_eq!(approve(&k, &owner.key, id, a), (409, "duplicate_seq".into()), "replayed");
}

#[test]
fn approvals_below_min_seq_or_past_the_limit_are_refused() {
    let mut k = Keyd::new();
    let (owner, id) = deposited(&mut k);
    assert_eq!(approve(&k, &owner.key, id, approval(pod_td(&k).release(0))), (409, "below_min_seq".into()));
    for seq in 3..=32 {
        assert_eq!(approve(&k, &owner.key, id, approval(pod_td(&k).release(seq))).0, 200, "seq {seq}");
    }
    assert_eq!(approve(&k, &owner.key, id, approval(pod_td(&k).release(33))), (409, "too_many_approvals".into()));
}

#[test]
fn an_approval_signed_by_anyone_but_the_records_owner_is_refused() {
    let mut k = Keyd::new();
    let (owner, id) = deposited(&mut k);
    let mallory = OwnerKey::generate().unwrap();
    let a = approval(pod_td(&k).release(3));
    assert_eq!(approve(&k, &mallory, id, a.clone()), (403, "not_owner".into()), "signed as herself");
    let mut forged = Approve::make(&mallory, &k.pubkey(), id, a.clone()).unwrap();
    forged.owner = owner.public();
    assert_eq!(code(&k.post("/approve", &forged.to_json().unwrap())), (401, "bad_signature".into()), "in the owner's name");
    let mut tampered = Approve::make(&owner.key, &k.pubkey(), id, a).unwrap();
    tampered.approval.expires = Some(1);
    assert_eq!(code(&k.post("/approve", &tampered.to_json().unwrap())), (401, "bad_signature".into()));
    assert_eq!(k.view(&owner.public()).unwrap().approvals.len(), 2);
}

#[test]
fn an_approval_does_not_outlive_its_record() {
    let mut k = Keyd::new();
    let (owner, id) = deposited(&mut k);
    let a = Approve::make(&owner.key, &k.pubkey(), id, approval(pod_td(&k).release(3))).unwrap();
    // The owner replaces the record, dropping seq 2: the old approval
    // names the old record and cannot bring anything back.
    let mut s = owner.settings(vec![approval(pod_td(&k).release(1))]);
    s.replaces = Some(id);
    assert_eq!(code(&k.deposit(&owner.deposit_to(&k, s))).0, 200);
    assert_eq!(code(&k.post("/approve", &a.to_json().unwrap())), (404, "unknown_record".into()));
    assert_eq!(approve(&k, &owner.key, [0; 32], approval(pod_td(&k).release(3))), (404, "unknown_record".into()));
    k.restart();
    assert_eq!(code(&k.post("/approve", &a.to_json().unwrap())), (401, "bad_signature".into()), "bound to the old boot key");
    let fresh = Approve::make(&owner.key, &k.pubkey(), id, approval(pod_td(&k).release(3))).unwrap();
    assert_eq!(code(&k.post("/approve", &fresh.to_json().unwrap())), (404, "unknown_record".into()), "a restart forgets");
}

#[test]
fn malformed_approvals_are_4xx() {
    let k = Keyd::new();
    assert_eq!(code(&k.post("/approve", b"{}")), (400, "bad_request".into()));
    assert_eq!(code(&k.post("/approve", &vec![b' '; 5000])), (413, "too_large".into()));
}

/// A key service takes deposits from the owners its operator named when
/// it started, and keeps nothing for anyone else, however well signed.
#[test]
fn a_deposit_from_an_owner_the_operator_did_not_name_is_refused() {
    let k = Keyd::new();
    let stranger = Owner::new();
    let d = stranger.deposit_to(&k, stranger.settings(vec![]));
    assert_eq!(code(&k.deposit(&d)), (403, "not_admitted".into()));
    assert_eq!(k.view(&stranger.public()).unwrap_err().error, "unknown_owner", "nothing was kept");
    assert_eq!(k.service.store().unwrap().records.len(), 0);
    // In an admitted owner's name it is refused as any forgery is.
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let mut forged = stranger.deposit_to(&k, stranger.settings(vec![]));
    forged.owner = owner.public();
    assert_eq!(code(&k.deposit(&forged)).0, 401);
    assert_eq!(k.service.store().unwrap().records.len(), 0);
}

/// The operator's list is the same after a restart; the records are gone.
#[test]
fn a_restarted_key_service_serves_the_same_owners() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    k.restart();
    assert_eq!(code(&k.deposit(&owner.deposit_to(&k, owner.settings(vec![])))).0, 200);
    let stranger = Owner::new();
    assert_eq!(code(&k.deposit(&stranger.deposit_to(&k, stranger.settings(vec![])))), (403, "not_admitted".into()));
}
