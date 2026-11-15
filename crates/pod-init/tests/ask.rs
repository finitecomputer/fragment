//! pod-init asking a key service that is absent, broken or hostile: it
//! stays locked, and the owner's unlock still works. (A real key
//! service is in crates/pod-keyd's tests/with_pod_init.rs.)

mod common;

use common::Pod;
use fragment_pod_init::ask::{self, AskError, Backoff};
use fragment_pod_init::http::{self, Response};
use fragment_pod::unlock::binding::State;
use fragment_pod::keyd::delivery::{self, Delivery};
use fragment_pod::keyd::KeydError;
use fragment_pod::unlock::keys::{BootKey, DiskKey};
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use fragment_pod::unlock::wire::Status;
use std::net::TcpListener;
use std::time::Duration;

struct Owner {
    key: OwnerKey,
    disk: DiskKey,
}

fn locked() -> (Owner, Pod) {
    let owner = Owner { key: OwnerKey::generate().unwrap(), disk: DiskKey::random().unwrap() };
    let mut pod = Pod::new();
    assert_eq!(pod.send(Kind::Init, &pod.request(Kind::Init, &owner.key, &owner.disk)).status, 200);
    pod.service.volume.close().unwrap();
    pod.reboot(None);
    (owner, pod)
}

fn owner_unlocks(pod: &Pod, owner: &Owner) {
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner.key, &owner.disk));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(pod.status().state, State::Open);
}

/// A key service that answers `/nonce` and then `release` to `/release`.
fn fake(release: impl Fn(Status) -> Response + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        http::serve(&listener, "fake-keyd", http::Body::UpTo(1 << 20), None, &|req| {
            let req = req.unwrap();
            match req.path.as_str() {
                "/nonce" => Response::json(200, delivery::nonce_to_json(&[4; 32]).unwrap()),
                _ => release(fragment_pod::keyd::delivery::ReleaseRequest::from_json(&req.body).unwrap().status),
            }
        })
    });
    addr
}

#[test]
fn an_unreachable_key_service_leaves_the_pod_locked_for_its_owner() {
    let (owner, pod) = locked();
    let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    assert!(matches!(ask::attempt(&pod.service, &free), Err(AskError::Unreachable(_))));
    let quick = Backoff { first: Duration::from_millis(1), max: Duration::from_millis(2) };
    assert!(matches!(ask::run(&pod.service, &free, quick, Some(3)), Err(AskError::Unreachable(_))));
    assert_eq!(pod.status().state, State::Locked);
    owner_unlocks(&pod, &owner);
    assert_eq!(ask::run(&pod.service, &free, quick, None), Err(AskError::NotLocked(State::Open)), "stops once open");
}

#[test]
fn a_refusal_or_garbage_leaves_the_pod_locked() {
    let (owner, pod) = locked();
    let no = fake(|_| Response::json(404, br#"{"error":"unknown_owner","detail":"no record\u001b[2J"}"#.to_vec()));
    let e = ask::attempt(&pod.service, &no).unwrap_err();
    assert_eq!(e, AskError::Refused { status: 404, code: "unknown_owner".into(), detail: "no record?[2J".into() });
    let junk = fake(|_| Response::json(200, b"{\"state\":\"open\"}".to_vec()));
    assert!(matches!(ask::attempt(&pod.service, &junk), Err(AskError::Garbled(_))));
    let huge = fake(|_| Response::json(200, vec![b'x'; 64 * 1024]));
    assert!(matches!(ask::attempt(&pod.service, &huge), Err(AskError::Unreachable(_))));
    assert_eq!(pod.status().state, State::Locked);
    owner_unlocks(&pod, &owner);
}

#[test]
fn a_hostile_key_service_cannot_open_the_pod_even_with_the_right_key() {
    // It knows the disk key (say, a stolen copy) and seals it to the
    // pod's boot key from its own key, or names the owner's grant for
    // another key service it does not hold. Neither opens anything.
    let (owner, pod) = locked();
    let disk = DiskKey::from_bytes(owner.disk.as_bytes()).unwrap();
    let me = owner.key.public();
    let addr = fake(move |status| {
        let hostile = BootKey::generate().unwrap();
        let d = Delivery::make(&hostile, me, [0; 64], 1, &status.boot_pubkey, &disk).unwrap();
        Response::json(200, d.to_json().unwrap())
    });
    assert_eq!(ask::attempt(&pod.service, &addr), Err(AskError::Delivery(KeydError::Grant)));
    assert_eq!(pod.status().state, State::Locked);
    owner_unlocks(&pod, &owner);
}

#[test]
fn nothing_is_asked_for_a_blank_pod_or_one_naming_no_owner() {
    let blank = Pod::new();
    assert_eq!(ask::attempt(&blank.service, "127.0.0.1:1"), Err(AskError::NotLocked(State::Blank)));
    let (owner, pod) = locked();
    std::fs::remove_file(pod.path("owner.hint")).unwrap();
    assert_eq!(ask::attempt(&pod.service, "127.0.0.1:1"), Err(AskError::NoOwner));
    owner_unlocks(&pod, &owner);
}
