//! pod-init's service with a fake volume and quoter, through the real
//! HTTP reader and handler: every path a key request can take.

mod common;

use common::{error, Pod};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use fragment_pod::unlock::wire::{self, Opened, Status};
use std::fs;
use std::io::{Read, Write};

fn hex_owner(o: &OwnerKey) -> String {
    format!("{}\n", hex::encode(o.public()))
}

/// A pod initialised by `owner` with `key`, then rebooted: locked.
fn locked(owner: &OwnerKey, key: &DiskKey) -> Pod {
    let mut pod = Pod::new();
    let r = pod.request(Kind::Init, owner, key);
    assert_eq!(pod.send(Kind::Init, &r).status, 200);
    pod.service.volume.close().unwrap();
    pod.reboot(None);
    pod
}

#[test]
fn status_binds_its_fields_and_the_nonce_into_report_data() {
    let pod = Pod::new();
    let s = pod.status();
    assert_eq!((s.state, s.owner, s.tls_root), (State::Blank, None, None));
    assert_eq!(s.boot_pubkey, pod.service.boot.public());
    let expected = s.binding([0x11; 32]).report_data();
    assert_eq!(&s.quote[5..], &expected);
    assert_eq!(&s.quote[..5], b"QUOTE");
}

#[test]
fn status_needs_a_32_byte_hex_nonce() {
    let pod = Pod::new();
    for target in ["/status", "/status?nonce=abcd", &format!("/status?nonce={}", "zz".repeat(32))] {
        let r = pod.get(target);
        assert_eq!((r.status, error(&r).error.as_str()), (400, "bad_nonce"), "{target}");
    }
}

#[test]
fn init_on_blank_opens_and_records_the_owner_inside_and_beside() {
    let pod = Pod::new();
    let owner = OwnerKey::generate().unwrap();
    let r = pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &DiskKey::random().unwrap()));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let opened: Opened = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(opened.state, State::Open);
    assert_eq!(fs::read_to_string(pod.path("mnt/pod/owner")).unwrap(), hex_owner(&owner));
    assert_eq!(fs::read_to_string(pod.path("owner.hint")).unwrap(), hex_owner(&owner));
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Open, Some(owner.public())));
}

#[test]
fn init_on_a_locked_pod_is_refused() {
    let owner = OwnerKey::generate().unwrap();
    let pod = locked(&owner, &DiskKey::random().unwrap());
    let r = pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &DiskKey::random().unwrap()));
    assert_eq!((r.status, error(&r).error.as_str()), (409, "wrong_state"));
    assert_eq!(pod.status().state, State::Locked);
}

#[test]
fn the_owner_unlocks_and_the_owner_comes_from_the_hint_until_then() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let pod = locked(&owner, &key);
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Locked, Some(owner.public())));
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(pod.status().state, State::Open);
}

#[test]
fn with_no_hint_file_the_hint_is_kept_in_the_volume_header() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let mut pod = Pod::configured(common::fake_volume, true);
    let r = pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &key));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(!pod.path("owner.hint").exists(), "no file beside the volume");
    assert_eq!(fs::read_to_string(pod.path("header.hint")).unwrap(), hex_owner(&owner));
    pod.service.volume.close().unwrap();
    pod.reboot(None);
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Locked, Some(owner.public())), "after a reboot the header names the owner");
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(pod.status().state, State::Open);
}

#[test]
fn a_wrong_disk_key_is_refused_and_the_pod_stays_locked() {
    let owner = OwnerKey::generate().unwrap();
    let pod = locked(&owner, &DiskKey::random().unwrap());
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &DiskKey::random().unwrap()));
    assert_eq!((r.status, error(&r).error.as_str()), (403, "wrong_key"));
    assert_eq!(pod.status().state, State::Locked);
}

#[test]
fn a_second_claimant_with_the_right_key_is_refused_and_the_volume_closed() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let pod = locked(&owner, &key);
    let claimant = OwnerKey::generate().unwrap();
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &claimant, &key));
    assert_eq!((r.status, error(&r).error.as_str()), (403, "owner_mismatch"));
    assert_eq!(pod.status().state, State::Locked);
    assert!(!pod.path("mnt").exists());
}

#[test]
fn a_request_captured_on_a_previous_boot_is_refused() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let mut pod = locked(&owner, &key);
    let captured = pod.request(Kind::Unlock, &owner, &key);
    pod.reboot(None);
    let r = pod.send(Kind::Unlock, &captured);
    assert_eq!((r.status, error(&r).error.as_str()), (401, "bad_signature"));
    assert_eq!(pod.status().state, State::Locked);
}

#[test]
fn unlock_when_open_is_refused() {
    let pod = Pod::new();
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    assert_eq!(pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &key)).status, 200);
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!((r.status, error(&r).error.as_str()), (409, "wrong_state"));
}

#[test]
fn an_init_seal_does_not_unlock() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let pod = locked(&owner, &key);
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Init, &owner, &key));
    assert_eq!((r.status, error(&r).error.as_str()), (401, "bad_signature"));
}

#[test]
fn bad_bodies_are_4xx_and_never_panic() {
    let pod = Pod::new();
    let good = pod.request(Kind::Init, &OwnerKey::generate().unwrap(), &DiskKey::random().unwrap());
    let mut v: serde_json::Value = serde_json::from_slice(&good.to_json().unwrap()).unwrap();
    v["owner"] = "ab".into();
    let short = serde_json::to_vec(&v).unwrap();
    v["owner"] = "zz".repeat(32).into();
    let not_hex = serde_json::to_vec(&v).unwrap();
    let cases: [(&[u8], u16, &str); 5] = [
        (b"{not json", 400, "bad_request"),
        (b"", 400, "bad_request"),
        (b"{\"owner\":1}", 400, "bad_request"),
        (&short, 400, "bad_field"),
        (&not_hex, 400, "bad_field"),
    ];
    for (body, status, code) in cases {
        let r = pod.post("/init", body);
        assert_eq!((r.status, error(&r).error.as_str()), (status, code), "{}", String::from_utf8_lossy(body));
    }
    let big = vec![b' '; fragment_pod_init::service::MAX_BODY + 1];
    let r = pod.post("/init", &big);
    assert_eq!((r.status, error(&r).error.as_str()), (413, "too_large"));
    let r = pod.raw(b"POST /init HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert_eq!(r.status, 400);
    assert_eq!(pod.get("/init").status, 405);
    assert_eq!(pod.get("/nowhere").status, 404);
    assert_eq!(pod.status().state, State::Blank);
}

#[test]
fn a_failing_open_hook_is_reported_and_the_volume_stays_open() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let mut pod = locked(&owner, &key);
    let hook = pod.path("hook");
    fs::write(&hook, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    pod.reboot(Some(hook));
    let r = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!((r.status, error(&r).error.as_str()), (500, "open_hook_failed"));
    assert_eq!(pod.status().state, State::Open);
}

#[test]
fn serves_the_event_log_and_the_tls_root_it_attests() {
    let pod = Pod::new();
    assert_eq!(pod.get("/eventlog").body, b"LOG");
    let owner = OwnerKey::generate().unwrap();
    assert_eq!(pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &DiskKey::random().unwrap())).status, 200);
    assert_eq!(pod.get("/tls-root").status, 404);
    let pem = include_bytes!("../../pod/testdata/tls-root.pem");
    fs::create_dir_all(pod.path("mnt/tls")).unwrap();
    fs::write(pod.path("mnt/tls/root.pem"), pem).unwrap();
    let r = pod.get("/tls-root");
    assert_eq!((r.status, r.body.as_slice()), (200, pem.as_slice()));
    assert_eq!(pod.status().tls_root, Some(fragment_pod::unlock::tls::root_hash(pem).unwrap()));
}

#[test]
fn serves_status_over_tcp() {
    let pod = Pod::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = std::thread::spawn(move || {
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        write!(c, "GET /status?nonce={} HTTP/1.1\r\nHost: x\r\n\r\n", "22".repeat(32)).unwrap();
        let mut out = Vec::new();
        c.read_to_end(&mut out).unwrap();
        out
    });
    pod.service.serve(&listener, Some(1));
    let out = client.join().unwrap();
    let body = &out[out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4..];
    let s = Status::from_json(body).unwrap();
    assert_eq!(&s.quote[5..], &s.binding([0x22; 32]).report_data());
    assert!(body.len() < wire::MAX_QUOTE);
}
