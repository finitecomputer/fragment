// `fragment pod keyd` against a real pod-keyd service on loopback, and a
// real pod-init service (fake volume and quoter) that asks it: the key
// service's tests' own harness (mod.rs's `harness`). Its fake attestor plays
// Intel and Google for both TDs, the pod's and the key service's, each
// with the registers and instance a test sets. What is proven is the
// commands' order and refusals, their files and key, and that a deposit
// made here opens the pod; not Intel's signature.
//
// Goal of each: valid (deposit, then the pod opens with no one at the
// keyboard; approve; status), invalid (a key service failing a check, a
// usage mistake), replay (a deposit replacing the record it names), and
// restart (a restarted key service holds nothing: approve refuses, a new
// deposit works).

use super::harness as kd;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fragment_pod::keyd::view::RecordView;
use fragment_pod::owner::dir::{Dir, PodState, POLICY};
use fragment_pod::owner::keyd::KEYD_POLICY;
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::unlock::release::Release;
use fragment_pod_init::ask::{self, Backoff};
use fragment_pod_init::http;
use fragment_pod_keyd::service::MAX_BODY;
use kd::{Keyd, Td, KEYD, KEYD_INSTANCE, POD, POD_INSTANCE};
use serde_json::Value;

use super::keyd::{parse_duration, window, KeydCmd};
use super::{run, Ctx, PodCmd, Report};
use crate::api::{Code, CodedError};
use crate::auth::{fixed, Identity};

const ONCE: Backoff = Backoff { first: std::time::Duration::from_millis(1), max: std::time::Duration::from_millis(1) };

fn wire(r: &Release) -> String {
    serde_json::to_string(&r.to_wire()).unwrap()
}

/// This machine: the CLI's config folder and key, a key service on
/// loopback, and a locked pod this key owns, whose disk key is in the
/// pod's folder with a policy naming releases 1 to 3 of the pod's TD.
struct Setup {
    _scratch: kd::pod::scratch::Scratch,
    pods: PathBuf,
    dir: PathBuf,
    id: Identity,
    keyd: Arc<Keyd>,
    addr: String,
    pod: kd::pod::Pod,
}

impl Setup {
    fn new() -> Self {
        let scratch = kd::pod::scratch::Scratch::new();
        let pods = super::pods_dir(&scratch.path().join("fragment/config.json"));
        let id = fixed(7);
        let owner = kd::Owner { key: super::owner_key(&id), disk: DiskKey::random().unwrap() };
        let pod = owner.locked_pod();
        let (keyd, addr) = kd::serve(Keyd::admitting(&[owner.public()]));
        let dir = super::pod_dir(&pods, &super::Address::parse("127.0.0.1").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        Dir::new(&dir).write_disk_key(&owner.disk).unwrap();
        let td = keyd.attestor.td(POD);
        let releases: Vec<String> = (1..=3).map(|s| wire(&td.release(s))).collect();
        std::fs::write(dir.join(POLICY), format!(r#"{{"releases":[{}],"gce_instance":"{POD_INSTANCE}"}}"#, releases.join(","))).unwrap();
        let k = wire(&keyd.attestor.td(KEYD).release(1));
        std::fs::write(dir.join(KEYD_POLICY), format!(r#"{{"releases":[{k}],"gce_instance":"{KEYD_INSTANCE}"}}"#)).unwrap();
        Setup { _scratch: scratch, pods, dir, id, keyd, addr, pod }
    }

    fn run(&self, cmd: KeydCmd, json: bool) -> (anyhow::Result<Report>, String) {
        let ctx = Ctx { pods: &self.pods, key: Some(&self.id), attestor: self.keyd.attestor.as_ref(), json };
        let mut out = Vec::new();
        let r = run(&ctx, PodCmd::Keyd { cmd }, &mut out);
        (r, String::from_utf8(out).unwrap())
    }

    fn deposit(&self) -> KeydCmd {
        KeydCmd::Deposit { pod: "127.0.0.1".into(), keyd: self.addr.clone() }
    }

    fn status(&self) -> KeydCmd {
        KeydCmd::Status { pod: "127.0.0.1".into(), keyd: self.addr.clone() }
    }

    fn approve(&self, file: &Path, delay: Option<&str>, expires: Option<&str>) -> KeydCmd {
        KeydCmd::Approve { pod: "127.0.0.1".into(), keyd: self.addr.clone(), release: file.into(), delay: delay.map(Into::into), expires: expires.map(Into::into) }
    }

    fn view(&self) -> Option<RecordView> {
        self.keyd.view(&super::owner_key(&self.id).public()).ok()
    }

    fn release_file(&self, td: &Td, seq: u64) -> PathBuf {
        let path = self.dir.join(format!("release-{seq}.json"));
        std::fs::write(&path, wire(&td.release(seq))).unwrap();
        path
    }
}

fn code(e: &anyhow::Error) -> Option<Code> {
    e.downcast_ref::<CodedError>().map(|c| c.code)
}

fn refused(r: anyhow::Result<Report>, what: &str) {
    let e = r.expect_err("refused");
    assert_eq!(code(&e), Some(Code::PodRefused), "{e:#}");
    assert!(format!("{e:#}").contains(what), "{what:?} in {e:#}");
}

/// Valid: the key service's four checks pass, the deposit is made with
/// policy.json's releases at or above the floor and the floor as
/// min_seq; then the pod, locked, asks it and opens with no one here.
#[test]
fn a_deposit_lets_the_pod_open_on_its_own() {
    let s = Setup::new();
    Dir::new(&s.dir).write_state(&PodState { last_unlocked_seq: Some(2) }).unwrap();
    let (r, out) = s.run(s.deposit(), false);
    assert!(r.unwrap().passed, "{out}");
    for check in ["quote ", "release ", "platform ", "google_instance "] {
        assert!(out.lines().any(|l| l.starts_with(check) && l.contains("PASS")), "{check} in {out}");
    }
    assert!(out.contains("deposited this pod's disk key"), "{out}");
    let v = s.view().unwrap();
    assert_eq!((v.min_seq, v.approvals.iter().map(|a| a.release.seq).collect::<Vec<_>>()), (2, vec![2, 3]), "the floor holds");
    assert_eq!(v.gce_instance, POD_INSTANCE);
    assert_eq!(ask::run(&s.pod.service, &s.addr, ONCE, Some(1)), Ok(()));
    assert_eq!(s.pod.status().state, State::Open);
}

/// Invalid: a key service on registers keyd-policy.json does not name,
/// on another instance, or whose quote does not verify gets nothing.
#[test]
fn a_key_service_failing_a_check_gets_no_key() {
    let s = Setup::new();
    let good = s.keyd.attestor.td(KEYD);
    let mut other = good.clone();
    other.registers[2][0] ^= 1;
    s.keyd.attestor.set(KEYD, other);
    let (r, out) = s.run(s.deposit(), false);
    refused(r, "the key service's release check did not pass");
    assert!(out.lines().any(|l| l.starts_with("release ") && l.contains("FAIL")), "{out}");
    let mut moved = good.clone();
    moved.instance = fragment_pod::attest::pzid::Instance::parse("projects/1/zones/us-central1-b/instances/999").unwrap();
    s.keyd.attestor.set(KEYD, moved);
    refused(s.run(s.deposit(), false).0, "the key service's google_instance check did not pass");
    let mut debug = good;
    debug.td_attributes |= fragment_pod::attest::report::attr::DEBUG;
    s.keyd.attestor.set(KEYD, debug);
    refused(s.run(s.deposit(), false).0, "the key service's platform check did not pass");
    assert_eq!(s.view(), None, "nothing was deposited");
}

/// Replay: a second deposit replaces the record the first made (it names
/// it), and the pod then opens under the second.
#[test]
fn a_second_deposit_replaces_the_first() {
    let s = Setup::new();
    s.run(s.deposit(), false).0.unwrap();
    let first = s.view().unwrap().record;
    let (r, _) = s.run(s.deposit(), true);
    let data = r.unwrap().data;
    let second = s.view().unwrap().record;
    assert_ne!(first, second);
    assert_eq!(data["record"]["id"].as_str(), Some(hex::encode(second).as_str()));
    assert_eq!(ask::run(&s.pod.service, &s.addr, ONCE, Some(1)), Ok(()));
}

/// Replay: the key service raises its floor with each release it serves,
/// and this machine's floor (state.json) does not follow it. A deposit
/// made after that, signed with this machine's lower floor, replaces the
/// record and leaves the key service's floor where it was: it is never
/// what lets an older release back in.
#[test]
fn a_deposit_after_a_newer_release_was_served_keeps_the_floor() {
    let s = Setup::new();
    s.run(s.deposit(), false).0.unwrap();
    assert_eq!(s.view().unwrap().min_seq, 0);
    assert_eq!(ask::run(&s.pod.service, &s.addr, ONCE, Some(1)), Ok(()));
    assert_eq!(s.view().unwrap().min_seq, 3, "the pod is on release 3, and the key service served it");
    assert_eq!(Dir::new(&s.dir).state().unwrap().last_unlocked_seq, None, "this machine unlocked nothing");
    let (r, out) = s.run(s.deposit(), false);
    assert!(r.unwrap().passed, "{out}");
    let v = s.view().unwrap();
    assert_eq!((v.min_seq, v.approvals.iter().map(|a| a.release.seq).collect::<Vec<_>>()), (3, vec![1, 2, 3]));
}

/// A middleman between this machine and the key service: every request
/// goes through to it, and a record's view comes back with `min_seq`
/// raised to `lie`. The view is plain HTTP, so anyone on the path can.
fn lying_about_the_floor(keyd: Arc<Keyd>, lie: u64) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        http::serve(&listener, "middleman", http::Body::UpTo(MAX_BODY), None, &|r| {
            let view = matches!(&r, Ok(req) if req.method == "GET" && req.path.starts_with("/pods/"));
            let mut answer = keyd.service.respond(r);
            if view && answer.status == 200 {
                let mut v = RecordView::from_json(&answer.body).unwrap();
                v.min_seq = lie;
                answer.body = v.to_json().unwrap();
            }
            answer
        })
    });
    addr
}

/// Invalid: what the key service says a record's floor is, is not
/// attested, so nothing signed here rests on it. A middleman who raises
/// it in the answer does not get a higher floor signed: the pod, on a
/// release older than the lie, still opens.
#[test]
fn a_middleman_lying_about_the_floor_cannot_raise_it() {
    let mut s = Setup::new();
    let (pod, newer) = (s.keyd.attestor.td(POD), Td::new(0x30, POD_INSTANCE));
    let releases = format!("{},{}", wire(&pod.release(1)), wire(&newer.release(2)));
    std::fs::write(s.dir.join(POLICY), format!(r#"{{"releases":[{releases}],"gce_instance":"{POD_INSTANCE}"}}"#)).unwrap();
    s.run(s.deposit(), false).0.unwrap();
    let honest = std::mem::replace(&mut s.addr, lying_about_the_floor(s.keyd.clone(), 2));
    let (r, out) = s.run(s.deposit(), false);
    assert!(r.unwrap().passed, "{out}");
    let v = s.view().unwrap();
    assert_eq!((v.min_seq, v.approvals.iter().map(|a| a.release.seq).collect::<Vec<_>>()), (0, vec![1, 2]), "as this machine's floor has it");
    assert_eq!(ask::run(&s.pod.service, &honest, ONCE, Some(1)), Ok(()), "the pod, on release 1, opens");
}

/// Valid and windows: a release approved with a delay is not served
/// until the delay is over (the key service's clock); its seq is above
/// the record's.
#[test]
fn an_approval_with_a_delay_opens_later() {
    let s = Setup::new();
    s.run(s.deposit(), false).0.unwrap();
    let new = Td::new(0x30, POD_INSTANCE);
    let file = s.release_file(&new, 5);
    let (r, out) = s.run(s.approve(&file, Some("1h"), Some("30d")), false);
    assert!(r.unwrap().passed, "{out}");
    assert!(out.contains("approved         release-5 (seq 5) from "), "{out}");
    let a = s.view().unwrap().approvals.into_iter().find(|a| a.release.seq == 5).unwrap();
    let (from, until) = (a.not_before.unwrap(), a.expires.unwrap());
    assert_eq!(until - from, 30 * 86_400 - 3600);
    s.keyd.attestor.set(POD, new);
    s.keyd.clock.set(from as i64 - 1);
    let e = ask::run(&s.pod.service, &s.addr, ONCE, Some(1)).unwrap_err();
    assert!(e.to_string().contains("not_yet_valid"), "{e}");
    s.keyd.clock.set(from as i64);
    assert_eq!(ask::run(&s.pod.service, &s.addr, ONCE, Some(1)), Ok(()));
}

/// Invalid: an approval below the record's min_seq is the key service's
/// refusal, named.
#[test]
fn an_approval_below_the_floor_is_refused() {
    let s = Setup::new();
    Dir::new(&s.dir).write_state(&PodState { last_unlocked_seq: Some(3) }).unwrap();
    s.run(s.deposit(), false).0.unwrap();
    let file = s.release_file(&s.keyd.attestor.td(POD), 2);
    refused(s.run(s.approve(&file, None, None), false).0, "below_min_seq");
}

/// Restart: a restarted key service holds nothing. An approval is
/// refused before anything is signed, and a new deposit works.
#[test]
fn after_the_key_service_restarts_approve_asks_for_a_deposit() {
    let mut s = Setup::new();
    s.run(s.deposit(), false).0.unwrap();
    let (restarted, addr) = kd::serve(Keyd::admitting(&[super::owner_key(&s.id).public()]));
    s.keyd = restarted;
    s.addr = addr;
    let file = s.release_file(&s.keyd.attestor.td(POD), 4);
    refused(s.run(s.approve(&file, None, None), false).0, "deposit first");
    let (r, out) = s.run(s.status(), false);
    assert!(r.unwrap().passed, "{out}");
    assert!(out.contains("record           none for this key"), "{out}");
    s.run(s.deposit(), false).0.unwrap();
    assert!(s.view().is_some());
}

/// status: the checks and the record, as text and as `--json`; a failed
/// check is exit 1, not an error.
#[test]
fn status_shows_the_checks_and_the_record() {
    let s = Setup::new();
    s.run(s.deposit(), false).0.unwrap();
    let (r, out) = s.run(s.status(), true);
    assert_eq!(out, "", "with --json nothing is printed but the envelope, by main");
    let data = r.unwrap().data;
    let names: Vec<&str> = data["checks"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["quote", "release", "platform", "google_instance"]);
    assert_eq!(data["record"]["minSeq"], 0);
    assert_eq!(data["record"]["owner"].as_str(), Some(s.id.npub().as_str()));
    assert_eq!(data["record"]["approvals"].as_array().map(Vec::len), Some(3));
    assert_eq!(data["bootKey"].as_str(), Some(hex::encode(s.keyd.pubkey()).as_str()));
    let mut other = s.keyd.attestor.td(KEYD);
    other.tcb_status = "OutOfDate".into();
    s.keyd.attestor.set(KEYD, other);
    let (r, out) = s.run(s.status(), false);
    assert_eq!(r.unwrap().exit_code(), 1, "{out}");
    assert!(out.lines().any(|l| l.starts_with("platform ") && l.contains("FAIL")), "{out}");
}

/// Usage: no keyd-policy.json, a bad duration, an empty window, an
/// unclaimed pod (no disk key).
#[test]
fn usage_mistakes_are_named() {
    let s = Setup::new();
    std::fs::remove_file(s.dir.join(KEYD_POLICY)).unwrap();
    let e = s.run(s.status(), false).0.unwrap_err();
    assert_eq!(code(&e), Some(Code::InvalidUsage), "{e:#}");
    assert!(format!("{e:#}").contains(KEYD_POLICY), "{e:#}");
    assert_eq!(parse_duration("90s").unwrap(), 90);
    assert_eq!(parse_duration("2d").unwrap(), 172_800);
    for bad in ["", "1", "0d", "1w", "d", "-1h", "3651d"] {
        assert_eq!(parse_duration(bad).err().as_ref().and_then(code), Some(Code::InvalidUsage), "{bad:?}");
    }
    assert_eq!(window(Some("1h"), Some("1h"), 100).err().as_ref().and_then(code), Some(Code::InvalidUsage));
    assert_eq!(window(None, Some("1m"), 100).unwrap().expires, Some(160));
    let fresh = Setup::new();
    std::fs::remove_file(fresh.dir.join(fragment_pod::owner::dir::DISK_KEY)).unwrap();
    let e = fresh.run(fresh.deposit(), false).0.unwrap_err();
    assert!(format!("{e:#}").contains("fragment pod init"), "{e:#}");
    assert_eq!(fresh.view(), None);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(fresh.dir.join(POLICY)).unwrap()).unwrap();
    assert_eq!(v["releases"].as_array().map(Vec::len), Some(3));
}
