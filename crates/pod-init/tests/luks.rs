//! The real volume (cryptsetup, mkfs.ext4, mount) through the service.
//! Needs root, cryptsetup, mkfs.ext4 and loop devices, so it is ignored
//! unless asked for; on a Mac, run it in a privileged Linux container
//! (docs/pod.md, "pod-init").

mod common;

use common::{error, Pod};
use fragment_pod_init::config::Config;
use fragment_pod_init::luks::Luks;
use fragment_pod_init::volume::Volume;
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// A mapper name of each volume's own: the tests run in parallel, and
/// device-mapper names are the kernel's, shared by every test.
fn luks(c: &Config, _dir: &Path) -> Box<dyn Volume + Send + Sync> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mapper_name = format!("podtest{}x{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    Box::new(Luks::new(Config { mapper_name, ..c.clone() }))
}

#[test]
#[ignore = "needs root, cryptsetup and loop devices"]
fn init_lock_and_unlock_a_real_luks_volume() {
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    let mut pod = Pod::with(luks);
    assert_eq!(pod.status().state, State::Blank);
    let r = pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &key));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(pod.status().state, State::Open);
    assert!(pod.path("mnt/pod/owner").exists());

    pod.service.volume.close().unwrap();
    pod.reboot(None);
    assert_eq!(pod.status().state, State::Locked);
    assert!(!pod.path("mnt/pod/owner").exists());

    let wrong = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &DiskKey::random().unwrap()));
    assert_eq!((wrong.status, error(&wrong).error.as_str()), (403, "wrong_key"));
    assert_eq!(pod.status().state, State::Locked);

    let claimant = OwnerKey::generate().unwrap();
    let other = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &claimant, &key));
    assert_eq!((other.status, error(&other).error.as_str()), (403, "owner_mismatch"));
    assert_eq!(pod.status().state, State::Locked);

    let ok = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Open, Some(owner.public())));
    pod.service.volume.close().unwrap();
}

/// The measured image's layout: the volume is a block device (here a loop
/// device over a file, standing in for the data disk), the config names
/// no hint file, so the owner hint lives in the LUKS2 header and survives
/// the reboot that a read-only root with /var in memory would not.
#[test]
#[ignore = "needs root, cryptsetup, losetup and loop devices"]
fn a_block_device_volume_keeps_its_owner_hint_in_its_header() {
    let scratch = common::scratch::Scratch::new();
    let backing = scratch.path().join("backing.img");
    std::fs::File::create(&backing).unwrap().set_len(64 << 20).unwrap();
    let out = std::process::Command::new("losetup").args(["--find", "--show"]).arg(&backing).output().unwrap();
    assert!(out.status.success(), "losetup: {}", String::from_utf8_lossy(&out.stderr));
    let dev = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let owner = OwnerKey::generate().unwrap();
    let key = DiskKey::random().unwrap();
    // a volume maker is a fn pointer, which captures nothing: the device
    // goes through the environment
    std::env::set_var("POD_INIT_TEST_DEV", &dev);
    let mut pod = Pod::configured(loop_volume, true);
    assert_eq!(pod.status().state, State::Blank);
    let r = pod.send(Kind::Init, &pod.request(Kind::Init, &owner, &key));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(!pod.path("owner.hint").exists());
    pod.service.volume.close().unwrap();
    pod.reboot(None);
    let s = pod.status();
    assert_eq!((s.state, s.owner), (State::Locked, Some(owner.public())), "the header names the owner after a reboot");
    let ok = pod.send(Kind::Unlock, &pod.request(Kind::Unlock, &owner, &key));
    assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
    assert_eq!(pod.status().state, State::Open);
    pod.service.volume.close().unwrap();
    let _ = std::process::Command::new("losetup").args(["-d", &dev]).status();
}

fn loop_volume(c: &Config, dir: &Path) -> Box<dyn Volume + Send + Sync> {
    let dev = std::env::var("POD_INIT_TEST_DEV").unwrap();
    luks(&Config { volume_image: dev.into(), ..c.clone() }, dir)
}
