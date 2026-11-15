//! pod-init (fake volume and quoter) asking an in-process key service
//! over loopback: it boots locked, asks and opens; and every way that
//! must leave it locked for its owner does.

mod common;

use common::{approval, serve, Keyd, Owner, POD};
use fragment_pod_init::ask::{self, AskError, Backoff};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::seal::Kind;
use std::time::Duration;

const QUICK: Backoff = Backoff { first: Duration::from_millis(5), max: Duration::from_millis(40) };

fn deposited(k: &Keyd, owner: &Owner) {
    let s = owner.settings(vec![approval(k.attestor.td(POD).release(1))]);
    assert_eq!(k.deposit(&owner.deposit_to(k, s)).status, 200);
}

fn owner_unlock(pod: &common::pod::Pod, owner: &Owner) -> (u16, String) {
    common::code(&pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner.key, &owner.disk)))
}

#[test]
fn a_locked_pod_asks_the_key_service_and_opens() {
    let owner = Owner::new();
    let k = Keyd::admitting(&[owner.public()]);
    deposited(&k, &owner);
    let (k, addr) = serve(k);
    let pod = owner.locked_pod();
    assert_eq!(pod.status().state, State::Locked);
    assert_eq!(ask::run(&pod.service, &addr, QUICK, Some(1)), Ok(()));
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Open, Some(owner.public())), "the owner inside the volume");
    assert_eq!(k.view(&owner.public()).unwrap().last_release, Some(common::T0));
    assert_eq!(owner_unlock(&pod, &owner), (409, "wrong_state".into()), "the owner, second, is refused cleanly");
}

#[test]
fn the_owner_first_and_the_key_service_is_not_asked() {
    let owner = Owner::new();
    let k = Keyd::admitting(&[owner.public()]);
    deposited(&k, &owner);
    let (k, addr) = serve(k);
    let pod = owner.locked_pod();
    assert_eq!(owner_unlock(&pod, &owner).0, 200);
    assert_eq!(ask::run(&pod.service, &addr, QUICK, Some(1)), Err(AskError::NotLocked(State::Open)));
    assert_eq!(k.view(&owner.public()).unwrap().last_release, None);
}

#[test]
fn a_key_for_another_owners_volume_opens_it_and_closes_it_again() {
    // Mallory holds a copy of the owner's disk key and deposits it as
    // hers, naming the owner's instance; the host rewrites the hint to
    // her. The key service, which serves her too, rightly serves her
    // record, and the volume opens, but its own record names the owner:
    // closed again.
    let owner = Owner::new();
    let pod = owner.locked_pod();
    let mallory = Owner { key: fragment_pod::unlock::sign::OwnerKey::generate().unwrap(), disk: fragment_pod::unlock::keys::DiskKey::from_bytes(owner.disk.as_bytes()).unwrap() };
    let k = Keyd::admitting(&[owner.public(), mallory.public()]);
    deposited(&k, &mallory);
    fragment_pod_init::owner::write(&pod.path("owner.hint"), &mallory.public()).unwrap();
    let (_k, addr) = serve(k);
    match ask::attempt(&pod.service, &addr) {
        Err(AskError::Pod(e)) => assert_eq!((e.status, e.code), (403, "owner_mismatch")),
        other => panic!("{other:?}"),
    }
    assert_eq!(pod.status().state, State::Locked);
    assert!(!pod.path("mnt").exists(), "closed again");
}

#[test]
fn a_restarted_key_service_has_no_record_and_the_owner_still_unlocks() {
    let owner = Owner::new();
    let mut k = Keyd::admitting(&[owner.public()]);
    deposited(&k, &owner);
    k.restart();
    let (_k, addr) = serve(k);
    let pod = owner.locked_pod();
    let e = ask::run(&pod.service, &addr, QUICK, Some(3)).unwrap_err();
    assert!(matches!(&e, AskError::Refused { status: 404, code, .. } if code == "unknown_owner"), "{e}");
    assert_eq!(pod.status().state, State::Locked);
    assert_eq!(owner_unlock(&pod, &owner).0, 200);
}

#[test]
fn the_pod_keeps_asking_until_its_owner_deposits() {
    let owner = Owner::new();
    let (k, addr) = serve(Keyd::admitting(&[owner.public()]));
    let pod = owner.locked_pod();
    std::thread::scope(|scope| {
        let asking = scope.spawn(|| ask::run(&pod.service, &addr, QUICK, Some(100)));
        std::thread::sleep(Duration::from_millis(30));
        deposited(&k, &owner);
        assert_eq!(asking.join().unwrap(), Ok(()));
    });
    assert_eq!(pod.status().state, State::Open);
}
