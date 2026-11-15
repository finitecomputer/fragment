// `fragment pod` against a real pod-init service on loopback, with
// pod-init's fake volume and quoter (its tests' own, through mod.rs's `harness`). The
// fake quote is "QUOTE" and REPORTDATA, unsigned, so the chain and the
// host registry come from a fake `Attestor` that vouches for it with the
// registers and instance a test sets: what is proven here is the
// command's order and refusals, its files and its key, not Intel's
// signature (crates/pod's tests/real_quote.rs does that).
//
// Goal of each: valid (init, then unlock after a reboot), invalid (a
// changed register, another instance, a wrong disk key, another owner, a
// pod naming no owner), replay (an older release after a newer one), and
// restart (the pod between init and unlock, and the CLI: each command
// reads its files afresh).

use super::harness::pod as common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fragment_pod::attest::pzid::Instance;
use fragment_pod::attest::report::{attr, RegistersHex, TdReport};
use fragment_pod::checks::attestor::Attestor;
use fragment_pod::owner::dir::{DISK_KEY, POLICY, STATE};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::provenance::{Document, HostRegistry};
use fragment_pod::verify::tdx::{PckFlags, Verified};
use serde_json::Value;

use super::{run, Address, Ctx, PodCmd, Report};
use crate::api::{Code, CodedError};
use crate::auth::{fixed, Identity};

const INSTANCE: &str = "projects/1/zones/us-central1-b/instances/2";
const OTHER_INSTANCE: &str = "projects/1/zones/us-central1-b/instances/3";
const PPID: &str = "00aa";
/// Release `pod-a` (seq 3) and `pod-b` (seq 4), by MRTD; both pin RTMR1.
const MRTD_A: u8 = 0xaa;
const MRTD_B: u8 = 0xbb;
const RTMR1: u8 = 0x11;
/// The most connections one command makes is three (status, the key
/// request, the confirming status); the rest are drained after it.
const CONNECTIONS: usize = 8;

/// Vouches for any fake quote as a TD with the MRTD, RTMR1 and instance
/// it holds now, debug off, up to date, on a Google host in its region.
/// With `fail_after(n)`, the next n quotes verify and the ones after do
/// not (a confirming attestation that fails).
struct Vouching(Mutex<(u8, u8, &'static str)>, Mutex<Option<u32>>);

impl Vouching {
    fn new() -> Self {
        Self(Mutex::new((MRTD_A, RTMR1, INSTANCE)), Mutex::new(None))
    }
    fn set(&self, mrtd: u8, rtmr1: u8, instance: &'static str) {
        *self.0.lock().unwrap() = (mrtd, rtmr1, instance);
    }
    fn fail_after(&self, n: Option<u32>) {
        *self.1.lock().unwrap() = n;
    }
}

impl Attestor for Vouching {
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure> {
        if let Some(left) = self.1.lock().unwrap().as_mut() {
            if *left == 0 {
                return Err(Failure::Quote { detail: "the fake refuses this quote".into() });
            }
            *left -= 1;
        }
        let (mrtd, rtmr1, instance) = *self.0.lock().unwrap();
        let report_data: [u8; 64] = quote.get(5..69).and_then(|r| r.try_into().ok()).ok_or(Failure::Quote { detail: "not a fake quote".into() })?;
        let mut rtmr = [[0; 48]; 4];
        rtmr[1] = [rtmr1; 48];
        let report = TdReport {
            quote_version: 4,
            tee_tcb_svn: [0; 16],
            mr_seam: [0; 48],
            td_attributes: attr::SEPT_VE_DISABLE.to_le_bytes(),
            xfam: [0; 8],
            mr_td: [mrtd; 48],
            mr_config_id: [0; 48],
            mr_owner: Instance::parse(instance).unwrap().mr_owner(),
            mr_owner_config: [0; 48],
            rtmr,
            report_data,
        };
        let pck = || "not stated".to_owned();
        Ok(Verified {
            quote_version: 4,
            tcb_status: "UpToDate".into(),
            advisory_ids: vec![],
            tcb_eval_data_number: 1,
            platform_tcb_date: "2026-01-01T00:00:00Z".into(),
            fmspc: "00806F050000".into(),
            ppid: PPID.into(),
            pck_flags: PckFlags { smt_enabled: pck(), dynamic_platform: pck(), cached_keys: pck() },
            collateral_expires: at + 3600,
            verified_at: at,
            td_attribute_flags: vec!["SEPT_VE_DISABLE"],
            registers: RegistersHex::from(&report),
            report,
        })
    }

    fn registry(&self, _quote: &[u8]) -> Result<HostRegistry, Failure> {
        let document = Document { zone: "us-central1".into(), timestamp: "2026-02-17T11:25:12Z".into() };
        Ok(HostRegistry { ppid: PPID.into(), url: format!("https://example.invalid/{PPID}"), fetched_at: 0, document })
    }
}

/// An owner's machine: the CLI's config folder (its `pods/`), a key, the
/// fake attestor.
struct Owner {
    _scratch: common::scratch::Scratch,
    pods: PathBuf,
    key: Option<Identity>,
    attestor: Vouching,
}

impl Owner {
    fn new(key: Option<Identity>) -> Self {
        let scratch = common::scratch::Scratch::new();
        let pods = super::pods_dir(&scratch.path().join("fragment/config.json"));
        Owner { _scratch: scratch, pods, key, attestor: Vouching::new() }
    }

    /// The pod's folder with a policy trusting `pod-a` (seq 3) and `pod-b` (seq 4).
    fn trust(&self, addr: &str) -> PathBuf {
        let dir = super::pod_dir(&self.pods, &Address::parse(addr).unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let release = |seq: u32, name: &str, mrtd: u8| format!(r#"{{"seq":{seq},"name":"{name}","mrtd":"{}","rtmr1":"{}"}}"#, hex::encode([mrtd; 48]), hex::encode([RTMR1; 48]));
        let policy = format!(r#"{{"releases":[{},{}],"gce_instance":"{INSTANCE}"}}"#, release(3, "pod-a", MRTD_A), release(4, "pod-b", MRTD_B));
        std::fs::write(dir.join(POLICY), policy).unwrap();
        dir
    }

    /// One command, as `fragment pod …` runs it: its report and what it
    /// printed, or its error.
    fn run(&self, cmd: PodCmd, json: bool) -> (anyhow::Result<Report>, String) {
        let ctx = Ctx { pods: &self.pods, key: self.key.as_ref(), attestor: &self.attestor, json };
        let mut out = Vec::new();
        let r = run(&ctx, cmd, &mut out);
        (r, String::from_utf8(out).unwrap())
    }
}

/// `f` with the pod serving on loopback at the address it is handed;
/// connections it did not use are drained so the pod's thread ends.
fn serving<T>(pod: &common::Pod, f: impl FnOnce(&str) -> T) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::scope(|scope| {
        let server = scope.spawn(|| pod.service.serve(&listener, Some(CONNECTIONS)));
        let r = f(&addr);
        // bounded: the server ends after CONNECTIONS connections
        for _ in 0..=CONNECTIONS {
            if server.is_finished() {
                break;
            }
            if let Ok(mut c) = TcpStream::connect(&addr) {
                let _ = c.write_all(b"GET /healthz HTTP/1.1\r\nHost: pod\r\n\r\n");
                let _ = c.read_to_end(&mut Vec::new());
            }
        }
        r
    })
}

fn init(addr: &str) -> PodCmd {
    PodCmd::Init { pod: addr.into() }
}
fn unlock(addr: &str) -> PodCmd {
    PodCmd::Unlock { pod: addr.into() }
}
fn status(addr: &str) -> PodCmd {
    PodCmd::Status { pod: addr.into() }
}

fn code(e: &anyhow::Error) -> Option<Code> {
    e.downcast_ref::<CodedError>().map(|c| c.code)
}

/// The pod refused, or a check failed: `pod_refused`, its message naming `what`.
fn refused(r: anyhow::Result<Report>, what: &str) {
    let e = r.expect_err("refused");
    assert_eq!(code(&e), Some(Code::PodRefused), "{e:#}");
    assert!(format!("{e:#}").contains(what), "{what:?} in {e:#}");
}

/// A reboot: the volume closed (as a power cut leaves it), a new boot key.
fn reboot(pod: &mut common::Pod) {
    if pod.status().state == State::Open {
        pod.service.volume.close().unwrap();
    }
    pod.reboot(None);
}

fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Valid and restart: init claims a blank pod with the CLI's own key,
/// prints the recovery code once and records the release; after the pod
/// restarts (a new boot key, the volume locked), a new run of the CLI
/// unlocks it from its files alone.
#[test]
fn init_then_unlock_after_the_pod_restarts() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    // each run serves on a new port: the folder is the host's alone
    let dir = owner.trust("127.0.0.1");

    let (r, out) = serving(&pod, |addr| owner.run(init(addr), false));
    let report = r.unwrap();
    assert!(report.passed);
    for check in ["quote ", "release ", "platform ", "google_instance ", "owner "] {
        assert!(out.lines().any(|l| l.starts_with(check) && l.contains("PASS")), "{check} in {out}");
    }
    let key = std::fs::read_to_string(dir.join(DISK_KEY)).unwrap();
    assert!(out.contains(&format!("  {}\n", key.trim())), "the recovery code is the disk key: {out}");
    assert_eq!(report.data["recoveryCode"].as_str(), Some(key.trim()));
    assert!(out.contains("recorded release pod-a (seq 3)"), "{out}");
    assert_eq!(pod.status().state, State::Open);
    // the files: 0700 and 0600, as the config
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(DISK_KEY)), 0o600);
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(STATE)).unwrap()).unwrap()["last_unlocked_seq"], 3);

    reboot(&mut pod);
    assert_eq!(pod.status().state, State::Locked);
    let (r, out) = serving(&pod, |a| owner.run(unlock(a), false));
    assert!(r.unwrap().passed, "{out}");
    assert!(out.contains("confirmed: attested open on the same boot"), "{out}");
    assert!(!out.contains("recovery code"), "printed once, at init: {out}");
    assert_eq!(pod.status().state, State::Open);
}

/// The CLI's own key (the config's secret_key) is the owner the pod
/// records, inside its volume, and status says so.
#[test]
fn the_pods_owner_is_this_clis_key() {
    let id = Identity::generate();
    let owner = Owner::new(Some(id.clone()));
    let pod = common::Pod::new();
    owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    let recorded = std::fs::read_to_string(pod.path("mnt/pod/owner")).unwrap();
    assert_eq!(recorded.trim(), id.pubkey_hex(), "the volume's owner record");
    assert_eq!(pod.status().owner.map(hex::encode).as_deref(), Some(id.pubkey_hex()));
    assert_eq!(hex::encode(super::owner_key(&id).public()), id.pubkey_hex());
    let (r, out) = serving(&pod, |a| owner.run(status(a), true));
    let data = r.unwrap().data;
    assert_eq!(data["owner"].as_str(), Some(id.npub().as_str()), "{out}");
    assert_eq!(data["ownerIsThisKey"], true);
    assert_eq!(data["state"], "open");
}

/// Invalid: a changed register refuses before any key is made or sent.
#[test]
fn a_changed_register_is_refused_before_any_key_is_made() {
    let owner = Owner::new(Some(fixed(5)));
    let pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    owner.attestor.set(MRTD_A, 0x12, INSTANCE);
    let (r, out) = serving(&pod, |a| owner.run(init(a), false));
    refused(r, "the release check did not pass");
    assert!(out.lines().any(|l| l.starts_with("release ") && l.contains("FAIL")), "{out}");
    assert!(!dir.join(DISK_KEY).exists(), "no disk key was made");
    assert_eq!(pod.status().state, State::Blank);
    // status says the same, and exits 1
    let (r, out) = serving(&pod, |a| owner.run(status(a), false));
    assert_eq!(r.unwrap().exit_code(), 1, "{out}");
}

/// Invalid: another Google instance (MR_OWNER) is refused.
#[test]
fn another_instance_is_refused() {
    let owner = Owner::new(Some(fixed(5)));
    let pod = common::Pod::new();
    owner.trust("127.0.0.1");
    owner.attestor.set(MRTD_A, RTMR1, OTHER_INSTANCE);
    let (r, out) = serving(&pod, |a| owner.run(init(a), false));
    refused(r, "the google_instance check did not pass");
    assert!(out.lines().any(|l| l.starts_with("google_instance ") && l.contains("FAIL")), "{out}");
    assert_eq!(pod.status().state, State::Blank);
}

/// Invalid: a disk key that is not the pod's is refused by the pod, which
/// stays locked; the right one then opens it.
#[test]
fn a_wrong_disk_key_is_refused_by_the_pod() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    let right = std::fs::read_to_string(dir.join(DISK_KEY)).unwrap();
    std::fs::write(dir.join(DISK_KEY), format!("{}\n", DiskKey::random().unwrap().to_hex().as_str())).unwrap();
    reboot(&mut pod);
    refused(serving(&pod, |a| owner.run(unlock(a), false)).0, "wrong_key");
    assert_eq!(pod.status().state, State::Locked);
    std::fs::write(dir.join(DISK_KEY), right).unwrap();
    serving(&pod, |a| owner.run(unlock(a), false)).0.unwrap();
    assert_eq!(pod.status().state, State::Open);
}

/// Invalid: another owner's CLI is refused at the owner check, and, with
/// the hint forged to name it and the right disk key, by the pod.
#[test]
fn another_owner_is_refused() {
    let first = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    let dir = first.trust("127.0.0.1");
    serving(&pod, |a| first.run(init(a), false)).0.unwrap();
    reboot(&mut pod);

    let other = Owner::new(Some(fixed(6)));
    let other_dir = other.trust("127.0.0.1");
    std::fs::copy(dir.join(DISK_KEY), other_dir.join(DISK_KEY)).unwrap();
    let (r, out) = serving(&pod, |a| other.run(unlock(a), false));
    refused(r, "the owner check did not pass");
    assert!(out.lines().any(|l| l.starts_with("owner ") && l.contains("FAIL") && l.contains("is not this key")), "{out}");

    // the host rewrites the hint beside the volume to name the other key
    let other_pub: [u8; 32] = hex::decode(fixed(6).pubkey_hex()).unwrap().try_into().unwrap();
    fragment_pod_init::owner::write(&pod.path("owner.hint"), &other_pub).unwrap();
    refused(serving(&pod, |a| other.run(unlock(a), false)).0, "owner_mismatch");
    assert_eq!(pod.status().state, State::Locked, "the volume was closed again");
}

/// Invalid: a locked pod that names no owner is refused before any key is sent.
#[test]
fn a_pod_naming_no_owner_is_refused() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    reboot(&mut pod);
    std::fs::remove_file(pod.path("owner.hint")).unwrap();
    assert_eq!(pod.status().owner, None);
    refused(serving(&pod, |a| owner.run(unlock(a), false)).0, "names no owner");
    assert_eq!(pod.status().state, State::Locked);
}

/// Replay: once seq 4 was unlocked, the pod booting the older seq 3 again
/// is refused, by a new run of the CLI, from state.json.
#[test]
fn an_older_release_after_a_newer_one_is_refused() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    reboot(&mut pod);
    owner.attestor.set(MRTD_B, RTMR1, INSTANCE);
    let (r, out) = serving(&pod, |a| owner.run(unlock(a), false));
    r.unwrap();
    assert!(out.contains("recorded release pod-b (seq 4)"), "{out}");
    reboot(&mut pod);
    owner.attestor.set(MRTD_A, RTMR1, INSTANCE);
    refused(serving(&pod, |a| owner.run(unlock(a), false)).0, "older than the last unlocked (seq 4)");
    assert_eq!(pod.status().state, State::Locked);
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(STATE)).unwrap()).unwrap()["last_unlocked_seq"], 4);
}

/// Invalid: init on a pod already claimed is refused before anything is
/// sent, and so is unlock on an open pod; a second init from the same
/// folder is refused at its disk key, which it never overwrites.
#[test]
fn the_wrong_state_is_refused_and_a_disk_key_never_overwritten() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    let key = std::fs::read_to_string(dir.join(DISK_KEY)).unwrap();
    refused(serving(&pod, |a| owner.run(unlock(a), false)).0, "the pod is open; this needs it locked");
    let e = serving(&pod, |a| owner.run(init(a), false)).0.unwrap_err();
    assert!(format!("{e:#}").contains("already holds a pod's disk key"), "{e:#}");
    assert_eq!(std::fs::read_to_string(dir.join(DISK_KEY)).unwrap(), key);
    reboot(&mut pod);
    std::fs::remove_file(dir.join(DISK_KEY)).unwrap();
    refused(serving(&pod, |a| owner.run(init(a), false)).0, "the pod is locked; this needs it blank");
    assert!(!dir.join(DISK_KEY).exists());
}

/// `--json`: the data names every check, the release and the recovery
/// code; verify adds tls_root, which fails here: the fake pod serves no root.
#[test]
fn json_names_every_check() {
    let owner = Owner::new(Some(fixed(5)));
    let pod = common::Pod::new();
    owner.trust("127.0.0.1");
    let (r, out) = serving(&pod, |a| owner.run(init(a), true));
    assert_eq!(out, "", "with --json nothing is printed but the envelope, by main");
    let data = r.unwrap().data;
    let names: Vec<&str> = data["checks"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["quote", "release", "platform", "google_instance", "owner"]);
    assert!(data["checks"].as_array().unwrap().iter().all(|c| c["status"] == "pass"));
    assert_eq!(data["confirmed"].as_array().map(Vec::len), Some(5));
    assert_eq!((data["release"]["seq"].as_u64(), data["recoveryCode"].as_str().map(str::len)), (Some(3), Some(128)));

    let (r, _) = serving(&pod, |a| owner.run(PodCmd::Verify { pod: a.into(), tls: None, sni: None }, true));
    let report = r.unwrap();
    assert!(!report.passed);
    let tls = report.data["checks"].as_array().unwrap().iter().find(|c| c["name"] == "tls_root").cloned().unwrap();
    assert_eq!(tls["status"], "fail", "{tls}");
}

/// Usage: a pod's address, a folder with no policy, and no key.
#[test]
fn usage_mistakes_are_named() {
    assert_eq!(Address::parse("Pod-A.example.com").unwrap(), Address { host: "pod-a.example.com".into(), port: 7000 });
    assert_eq!(Address::parse("10.0.0.2:7443").unwrap().host_port(), "10.0.0.2:7443");
    for bad in ["", ":7000", "a/b", "../x", ".x", "-x", "a..b", "x:0", "x:70000", "x:y", "[::1]:7000", "a b"] {
        assert_eq!(Address::parse(bad).err().as_ref().and_then(code), Some(Code::InvalidUsage), "{bad:?}");
    }
    let owner = Owner::new(Some(fixed(5)));
    let e = owner.run(status("127.0.0.1:1"), false).0.unwrap_err();
    assert_eq!(code(&e), Some(Code::InvalidUsage), "{e:#}");
    assert!(format!("{e:#}").contains(&owner.pods.join("127.0.0.1").join(POLICY).display().to_string()), "{e:#}");
    let keyless = Owner::new(None);
    keyless.trust("127.0.0.1");
    assert_eq!(keyless.run(init("127.0.0.1:1"), false).0.err().as_ref().and_then(code), Some(Code::AuthFailed));
    // a pod that does not answer is unavailable (main's reqwest rule), not a refusal
    let dir = owner.trust("127.0.0.1");
    let e = owner.run(init("127.0.0.1:1"), false).0.unwrap_err();
    assert_eq!(code(&e), None, "{e:#}");
    assert!(e.chain().any(|c| c.downcast_ref::<reqwest::Error>().is_some()), "{e:#}");
    assert!(!dir.join(DISK_KEY).exists());
}

/// A pod claimed here is named by `claimed`, which `keys rotate` and
/// `login --force` refuse on: the key that owns it must stay.
#[test]
fn a_claimed_pod_is_named() {
    let owner = Owner::new(Some(fixed(5)));
    assert!(super::claimed(&owner.pods).unwrap().is_empty(), "no pods folder yet");
    owner.trust("127.0.0.1");
    owner.trust("pod-b.example.com");
    assert!(super::claimed(&owner.pods).unwrap().is_empty(), "a policy alone claims nothing");
    let pod = common::Pod::new();
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    assert_eq!(super::claimed(&owner.pods).unwrap(), ["127.0.0.1"]);
}

/// A key saved by another tool (an owner.key's 64 hex) is the same owner
/// once it is the config's `secret_key`, whichever y its point has: the
/// CLI keeps the even-y secret (auth.rs), the pod compares x-only keys.
#[test]
fn a_key_saved_elsewhere_is_the_same_owner() {
    use fragment_pod::unlock::sign::OwnerKey;
    for n in 1..=16u8 {
        let saved = hex::encode([n; 32]);
        let id = Identity::from_secret_hex(&saved).unwrap();
        assert_eq!(super::owner_key(&id).public(), OwnerKey::from_hex(&saved).unwrap().public(), "{n}");
        assert_eq!(hex::encode(OwnerKey::from_hex(&saved).unwrap().public()), id.pubkey_hex(), "{n}");
    }
}

fn floor(dir: &Path) -> Value {
    serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join(STATE)).unwrap()).unwrap()["last_unlocked_seq"].clone()
}

/// Replay, the floor's own: the pod takes the key for seq 4 and opens,
/// but its confirming attestation fails (as a lost answer would leave
/// it). The floor is 4 all the same, so the pod rebooted into seq 3 gets
/// nothing: refused at the release check, before any key is sent.
#[test]
fn the_floor_holds_when_the_answer_is_lost() {
    let owner = Owner::new(Some(fixed(5)));
    let mut pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    serving(&pod, |a| owner.run(init(a), false)).0.unwrap();
    assert_eq!(floor(&dir), 3);
    reboot(&mut pod);
    owner.attestor.set(MRTD_B, RTMR1, INSTANCE);
    owner.attestor.fail_after(Some(1));
    let (r, out) = serving(&pod, |a| owner.run(unlock(a), false));
    refused(r, "the quote check did not pass");
    assert!(out.contains("the pod says open"), "the key was sent: {out}");
    assert_eq!(pod.status().state, State::Open, "the pod took it");
    assert_eq!(floor(&dir), 4, "the floor is the release a key was sent to");

    owner.attestor.fail_after(None);
    reboot(&mut pod);
    owner.attestor.set(MRTD_A, RTMR1, INSTANCE);
    let (r, out) = serving(&pod, |a| owner.run(unlock(a), false));
    refused(r, "older than the last unlocked (seq 4)");
    assert!(!out.contains("the pod says open"), "nothing sent: {out}");
    assert_eq!(pod.status().state, State::Locked);
}

/// The floor is written before the key is sent: a state that cannot be
/// written (its temporary file's place taken) sends nothing, and makes no
/// disk key.
#[test]
fn an_unwritable_state_sends_no_key() {
    let owner = Owner::new(Some(fixed(5)));
    let pod = common::Pod::new();
    let dir = owner.trust("127.0.0.1");
    std::fs::create_dir(dir.join("state.json.tmp")).unwrap();
    let (r, out) = serving(&pod, |a| owner.run(init(a), false));
    let e = r.unwrap_err();
    assert!(format!("{e:#}").contains("state.json"), "{e:#}");
    assert!(!out.contains("the pod says open"), "nothing sent: {out}");
    assert_eq!(pod.status().state, State::Blank);
    assert!(!dir.join(DISK_KEY).exists(), "no disk key made");
}
