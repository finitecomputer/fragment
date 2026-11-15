//! `/nonce` and `/release` against real pod-init services: a release
//! that succeeds, and a refusal at each check, with its stable code.

mod common;

use common::{approval, code, pod_status, Keyd, Owner, Td, POD, T0};
use fragment_pod_keyd::nonces::{MAX_SPENT, NONCE_TTL};
use fragment_pod::attest::report::attr;
use fragment_pod::unlock::binding::State;
use fragment_pod::keyd::approve::Approval;
use fragment_pod::keyd::delivery::{Delivery, MAX_RELEASE_BODY};
use fragment_pod::keyd::deposit::Settings;
use fragment_pod::unlock::seal::SealError;
use fragment_pod::keyd::KeydError;

/// A key service holding `settings` for an owner with a locked pod.
fn setup(settings: impl FnOnce(&Owner, &Td) -> Settings) -> (Keyd, Owner, common::pod::Pod) {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let s = settings(&owner, &k.attestor.td(POD));
    let r = k.deposit(&owner.deposit_to(&k, s));
    assert_eq!(r.status, 200, "{}", common::text(&r));
    let pod = owner.locked_pod();
    (k, owner, pod)
}

fn approved(seqs: &[u64]) -> impl FnOnce(&Owner, &Td) -> Settings + '_ {
    move |o, td| o.settings(seqs.iter().map(|s| approval(td.release(*s))).collect())
}

/// Ask as the pod does: a nonce, the pod's status for it, `/release`.
fn ask(k: &Keyd, pod: &common::pod::Pod) -> fragment_pod_init::http::Response {
    let nonce = k.nonce();
    k.release(nonce, pod_status(pod, nonce))
}

#[test]
fn a_locked_pod_on_an_approved_release_receives_its_key_sealed_to_this_boot() {
    let (k, owner, pod) = setup(approved(&[1]));
    let r = ask(&k, &pod);
    assert_eq!(code(&r), (200, "ok".into()), "{}", common::text(&r));
    let d = Delivery::from_json(&r.body).unwrap();
    assert_eq!((d.owner, d.keyd_pubkey, d.seq), (owner.public(), k.pubkey(), 1));
    assert_eq!(d.open(&pod.service.boot, &owner.public()).unwrap().as_bytes(), owner.disk.as_bytes());
    let v = k.view(&owner.public()).unwrap();
    assert_eq!((v.min_seq, v.last_release), (1, Some(T0)));
}

#[test]
fn a_delivery_does_not_open_on_a_later_boot_of_the_pod() {
    let (k, owner, mut pod) = setup(approved(&[1]));
    let d = Delivery::from_json(&ask(&k, &pod).body).unwrap();
    pod.reboot(None);
    assert_eq!(d.open(&pod.service.boot, &owner.public()).unwrap_err(), KeydError::Seal(SealError::Open));
}

#[test]
fn other_registers_are_refused_at_the_release_check() {
    let (k, _, pod) = setup(approved(&[1]));
    let mut td = k.attestor.td(POD);
    td.registers[2][0] ^= 1;
    k.attestor.set(POD, td);
    assert_eq!(code(&ask(&k, &pod)), (403, "release".into()));
}

#[test]
fn once_a_newer_release_is_served_an_older_one_is_refused() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let old = k.attestor.td(POD);
    let new = Td::new(0x20, common::POD_INSTANCE);
    let s = owner.settings(vec![approval(old.release(1)), approval(new.release(2))]);
    assert_eq!(k.deposit(&owner.deposit_to(&k, s)).status, 200);
    let mut pod = owner.locked_pod();
    k.attestor.set(POD, new);
    assert_eq!(code(&ask(&k, &pod)).0, 200, "the update");
    assert_eq!(k.view(&owner.public()).unwrap().min_seq, 2);
    pod.reboot(None);
    k.attestor.set(POD, old);
    let r = ask(&k, &pod);
    assert_eq!(code(&r), (403, "rollback".into()), "{}", common::text(&r));
}

/// The floor is raised before the key is sealed: a release that passed
/// every check and then failed to seal (here, a boot key no X25519 seal
/// takes, bound into the quote as the pod's) still raised it, so an
/// older release is refused after it, as if the key had gone.
#[test]
fn the_floor_is_raised_before_the_key_is_sealed() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let old = k.attestor.td(POD);
    let new = Td::new(0x20, common::POD_INSTANCE);
    let s = owner.settings(vec![approval(old.release(1)), approval(new.release(2))]);
    assert_eq!(k.deposit(&owner.deposit_to(&k, s)).status, 200);
    let pod = owner.locked_pod();
    k.attestor.set(POD, new);
    let nonce = k.nonce();
    let mut status = pod_status(&pod, nonce);
    status.boot_pubkey = [0; 32];
    status.quote = [POD.as_slice(), &status.binding(nonce).report_data()].concat();
    let r = k.release(nonce, status);
    assert_eq!(code(&r), (500, "internal".into()), "no seal to an all-zero key: {}", common::text(&r));
    assert_eq!(k.view(&owner.public()).unwrap().min_seq, 2);
    k.attestor.set(POD, old);
    assert_eq!(code(&ask(&k, &pod)), (403, "rollback".into()));
}

#[test]
fn an_approval_is_refused_before_not_before_and_from_expires() {
    let (k, _, pod) = setup(|o, td| {
        let window = Approval { not_before: Some(T0 as u64 + 600), expires: Some(T0 as u64 + 1200), ..approval(td.release(1)) };
        o.settings(vec![window])
    });
    assert_eq!(code(&ask(&k, &pod)), (403, "not_yet_valid".into()));
    k.clock.set(T0 + 1200);
    assert_eq!(code(&ask(&k, &pod)), (403, "expired".into()));
    k.clock.set(T0 + 600);
    assert_eq!(code(&ask(&k, &pod)).0, 200, "inside the window");
}

#[test]
fn another_google_instance_or_region_is_refused() {
    let (k, _, pod) = setup(approved(&[1]));
    let original = k.attestor.td(POD);
    let mut td = original.clone();
    td.instance = fragment_pod::attest::pzid::Instance::parse("projects/1/zones/us-central1-b/instances/999").unwrap();
    k.attestor.set(POD, td);
    assert_eq!(code(&ask(&k, &pod)), (403, "google_instance".into()));
    let mut td = original;
    td.host_zone = "europe-west4".into();
    k.attestor.set(POD, td);
    assert_eq!(code(&ask(&k, &pod)), (403, "google_instance".into()), "a host the registry puts elsewhere");
}

#[test]
fn a_debug_td_or_an_old_tcb_is_refused_unless_the_owner_allowed_it() {
    let (k, _, pod) = setup(approved(&[1]));
    let original = k.attestor.td(POD);
    let mut td = original.clone();
    td.td_attributes |= attr::DEBUG;
    k.attestor.set(POD, td);
    assert_eq!(code(&ask(&k, &pod)), (403, "platform".into()));
    let mut td = original;
    td.tcb_status = "OutOfDate".into();
    k.attestor.set(POD, td.clone());
    assert_eq!(code(&ask(&k, &pod)), (403, "platform".into()));
    let (k, _, pod) = setup(|o, t| Settings { allow_tcb_status: vec!["OutOfDate".into()], ..approved(&[1])(o, t) });
    k.attestor.set(POD, td);
    assert_eq!(code(&ask(&k, &pod)).0, 200);
}

#[test]
fn a_pod_that_is_not_locked_is_refused() {
    let (k, owner, pod) = setup(approved(&[1]));
    let r = pod.send(fragment_pod::unlock::seal::Kind::Unlock, &pod.request(fragment_pod::unlock::seal::Kind::Unlock, &owner.key, &owner.disk));
    assert_eq!(r.status, 200);
    assert_eq!(code(&ask(&k, &pod)), (409, "wrong_state".into()));
    assert_eq!(k.view(&owner.public()).unwrap().last_release, None);
}

#[test]
fn a_status_not_bound_to_the_nonce_or_edited_is_refused_at_the_quote_check() {
    let (k, _, pod) = setup(approved(&[1]));
    let nonce = k.nonce();
    let other = k.nonce();
    assert_eq!(code(&k.release(nonce, pod_status(&pod, other))), (403, "quote".into()), "made for another nonce");
    let nonce = k.nonce();
    let mut s = pod_status(&pod, nonce);
    s.state = State::Open;
    assert_eq!(code(&k.release(nonce, s)), (403, "quote".into()), "the state edited");
    let nonce = k.nonce();
    let mut s = pod_status(&pod, nonce);
    s.boot_pubkey = [9; 32];
    assert_eq!(code(&k.release(nonce, s)), (403, "quote".into()), "a middleman's boot key");
    let nonce = k.nonce();
    let mut s = pod_status(&pod, nonce);
    s.quote[0] = b'X';
    assert_eq!(code(&k.release(nonce, s)), (403, "quote".into()), "a TD fake Intel never signed");
}

#[test]
fn a_nonce_is_good_once_and_only_if_issued_and_fresh() {
    let (k, _, pod) = setup(approved(&[1]));
    let nonce = k.nonce();
    let status = pod_status(&pod, nonce);
    assert_eq!(code(&k.release(nonce, status.clone())).0, 200);
    assert_eq!(code(&k.release(nonce, status)), (403, "unknown_nonce".into()), "reused");
    assert_eq!(code(&k.release([5; 32], pod_status(&pod, [5; 32]))), (403, "unknown_nonce".into()), "never issued");
    let nonce = k.nonce();
    k.clock.set(T0 + NONCE_TTL);
    assert_eq!(code(&k.release(nonce, pod_status(&pod, nonce))), (403, "unknown_nonce".into()), "stale");
    k.clock.set(T0 - 1);
    assert_eq!(code(&k.release(nonce, pod_status(&pod, nonce))), (403, "unknown_nonce".into()), "the clock went back");
    let mut edited = k.nonce();
    edited[0] ^= 1;
    assert_eq!(code(&k.release(edited, pod_status(&pod, edited))), (403, "unknown_nonce".into()), "its time edited");
    let other = Keyd::new().nonce();
    assert_eq!(code(&k.release(other, pod_status(&pod, other))), (403, "unknown_nonce".into()), "another key service's");
}

/// Only a release that is made spends its nonce: a refused request
/// keeps nothing, so no one fills what is kept by being refused.
#[test]
fn a_refused_release_spends_nothing_and_a_served_one_its_nonce() {
    let (k, _, pod) = setup(approved(&[1]));
    let nonce = k.nonce();
    assert_eq!(code(&k.release(nonce, pod_status(&pod, [0; 32]))), (403, "quote".into()), "a status made for another nonce");
    assert_eq!(k.service.store().unwrap().spent.held(), 0);
    assert_eq!(code(&k.release(nonce, pod_status(&pod, nonce))).0, 200, "the nonce is still good");
    assert_eq!(k.service.store().unwrap().spent.held(), 1);
    assert_eq!(code(&k.release(nonce, pod_status(&pod, nonce))), (403, "unknown_nonce".into()), "served once");
}

/// An owner's pods are given their key so many times a minute. Past
/// that, that owner waits, and no one else does.
#[test]
fn releases_in_a_minute_are_limited_by_owner() {
    let (mut k, _, pod) = setup(approved(&[1]));
    let other = Owner::new();
    k.admit(&other);
    let s = other.settings(vec![approval(k.attestor.td(POD).release(1))]);
    assert_eq!(k.deposit(&other.deposit_to(&k, s)).status, 200);
    let others = other.locked_pod();
    for i in 0..MAX_SPENT {
        assert_eq!(code(&ask(&k, &pod)).0, 200, "release {i}");
    }
    assert_eq!(code(&ask(&k, &pod)), (503, "too_many_releases".into()));
    assert_eq!(code(&ask(&k, &others)).0, 200, "another owner's pod is served");
    k.clock.set(T0 + NONCE_TTL);
    assert_eq!(code(&ask(&k, &pod)).0, 200, "a minute on");
}

#[test]
fn an_unknown_owner_or_a_pod_naming_none_is_refused() {
    let k = Keyd::new();
    let pod = Owner::new().locked_pod();
    assert_eq!(code(&ask(&k, &pod)), (404, "unknown_owner".into()));
    let blank = common::pod::Pod::new();
    assert_eq!(code(&ask(&k, &blank)), (400, "no_owner".into()));
}

#[test]
fn after_a_restart_the_key_service_knows_no_owner() {
    let (mut k, owner, pod) = setup(approved(&[1]));
    k.restart();
    assert_eq!(code(&ask(&k, &pod)), (404, "unknown_owner".into()));
    assert_eq!(k.view(&owner.public()).unwrap_err().error, "unknown_owner");
}

#[test]
fn malformed_release_requests_are_4xx() {
    let k = Keyd::new();
    assert_eq!(code(&k.post("/release", b"{")), (400, "bad_request".into()));
    assert_eq!(code(&k.post("/release", br#"{"version":1,"nonce":"00","status":{}}"#)).0, 400);
    let big = vec![b' '; MAX_RELEASE_BODY + 1];
    assert_eq!(code(&k.post("/release", &big)), (413, "too_large".into()));
}

/// A replacement the owner signed before a newer release was served,
/// held back on the way and delivered after it, names the record still
/// there, and replaces it. It does not take the floor back down with it.
#[test]
fn a_replacement_deposit_held_back_cannot_lower_the_floor() {
    let mut k = Keyd::new();
    let owner = Owner::new();
    k.admit(&owner);
    let old = k.attestor.td(POD);
    let new = Td::new(0x20, common::POD_INSTANCE);
    let settings = |replaces| Settings { replaces, min_seq: 1, ..owner.settings(vec![approval(old.release(1)), approval(new.release(2))]) };
    let first = owner.deposit_to(&k, settings(None));
    assert_eq!(k.deposit(&first).status, 200);
    let held_back = owner.deposit_to(&k, settings(Some(first.id(&k.pubkey()))));
    let mut pod = owner.locked_pod();
    k.attestor.set(POD, new);
    assert_eq!(code(&ask(&k, &pod)).0, 200, "the update is served");
    assert_eq!(k.view(&owner.public()).unwrap().min_seq, 2);
    assert_eq!(code(&k.deposit(&held_back)).0, 200, "it replaces the record it names");
    let v = k.view(&owner.public()).unwrap();
    assert_eq!((v.record, v.min_seq), (held_back.id(&k.pubkey()), 2), "and the floor stays where the release left it");
    pod.reboot(None);
    k.attestor.set(POD, old);
    assert_eq!(code(&ask(&k, &pod)), (403, "rollback".into()));
}

/// Asking for a nonce takes nothing from anyone else: however many are
/// asked for and left unused, a pod still gets one and is served.
#[test]
fn unused_nonces_keep_no_pod_from_asking() {
    let (k, _, pod) = setup(approved(&[1]));
    for i in 0..1000 {
        assert_eq!(code(&k.get("/nonce")).0, 200, "nonce {i}");
    }
    assert_eq!(code(&ask(&k, &pod)).0, 200);
}

/// The host sets the clock. Moving it past a spent nonce's minute, so
/// that the nonce is forgotten, and then back inside it, must not make
/// the nonce good again: a release is made once for a nonce.
#[test]
fn a_spent_nonce_stays_spent_when_the_clock_goes_forward_and_back() {
    let (k, _, pod) = setup(approved(&[1]));
    let nonce = k.nonce();
    let status = pod_status(&pod, nonce);
    assert_eq!(code(&k.release(nonce, status.clone())).0, 200);
    k.clock.set(T0 + NONCE_TTL);
    assert_eq!(code(&ask(&k, &pod)).0, 200, "a release a minute on: the first nonce has expired and is forgotten");
    k.clock.set(T0 + 1);
    assert_eq!(code(&k.release(nonce, status)), (403, "unknown_nonce".into()), "the first request again, the clock back in its minute");
    assert_eq!(code(&ask(&k, &pod)).0, 200, "a pod asking now is still served");
}
