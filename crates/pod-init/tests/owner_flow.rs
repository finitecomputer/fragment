//! The owner's key request (fragment_pod's `owner::flow`) end to end
//! against a real pod-init service on loopback: init on a blank pod, then
//! unlock after a reboot, and a refusal that sends no key. The fake quote
//! is "QUOTE" and REPORTDATA, unsigned, so the chain and the registry come
//! from a fake `Attestor` that vouches for it: what is proven here is the
//! order (checks, seal, send, confirm, record), not Intel's signature.

mod common;

use fragment_pod::attest::pzid::Instance;
use fragment_pod::attest::report::{attr, RegistersHex, TdReport};
use fragment_pod::checks::attestor::Attestor;
use fragment_pod::owner::dir::{Dir, DISK_KEY, POLICY};
use fragment_pod::owner::flow::{self, Refusal, Step};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::provenance::{Document, HostRegistry};
use fragment_pod::verify::tdx::{PckFlags, Verified};
use std::net::TcpListener;

const INSTANCE: &str = "projects/1/zones/us-central1-b/instances/2";
const MRTD: u8 = 0xaa;
const PPID: &str = "00aa";

/// Vouches for any fake quote: a TD with MRTD `MRTD`, debug off, on
/// `INSTANCE`, up to date, on a Google host in its region.
struct Vouching;

impl Attestor for Vouching {
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure> {
        let report_data: [u8; 64] = quote.get(5..69).and_then(|r| r.try_into().ok()).ok_or(Failure::Quote { detail: "not a fake quote".into() })?;
        let report = TdReport {
            quote_version: 4,
            tee_tcb_svn: [0; 16],
            mr_seam: [0; 48],
            td_attributes: attr::SEPT_VE_DISABLE.to_le_bytes(),
            xfam: [0; 8],
            mr_td: [MRTD; 48],
            mr_config_id: [0; 48],
            mr_owner: Instance::parse(INSTANCE).unwrap().mr_owner(),
            mr_owner_config: [0; 48],
            rtmr: [[0; 48]; 4],
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

/// An owner's directory: a key, and a policy trusting MRTD `mrtd` as seq 3.
fn owner_dir(scratch: &common::scratch::Scratch, mrtd: u8) -> Dir {
    let dir = Dir::new(&scratch.path().join("owner"));
    flow::keygen(&dir).unwrap();
    let policy = format!(r#"{{"releases":[{{"seq":3,"name":"pod-a","mrtd":"{}"}}],"gce_instance":"{INSTANCE}"}}"#, hex::encode([mrtd; 48]));
    std::fs::write(dir.path(POLICY), policy).unwrap();
    dir
}

/// `kind` against `pod`, which serves `connections` requests; the steps.
fn request(pod: &common::Pod, dir: &Dir, kind: Kind, connections: usize) -> (anyhow::Result<()>, Vec<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::scope(|scope| {
        scope.spawn(|| pod.service.serve(&listener, Some(connections)));
        let mut steps = Vec::new();
        let r = flow::key_request(dir, &Vouching, &host, kind, &mut |s: Step<'_>| {
            steps.push(match s {
                Step::Checked(c) => format!("checked {}", c.len()),
                Step::Sent => "sent".into(),
                Step::Confirming(c) => format!("confirming {}", c.len()),
                Step::Confirmed { .. } => "confirmed".into(),
                Step::Recorded(m) => format!("recorded {}", m.seq),
            })
        });
        (r, steps)
    })
}

#[test]
fn init_then_unlock_after_a_reboot_in_order_and_record_the_release() {
    let scratch = common::scratch::Scratch::new();
    let dir = owner_dir(&scratch, MRTD);
    let mut pod = common::Pod::new();
    // status, the key request, the confirming status
    let (r, steps) = request(&pod, &dir, Kind::Init, 3);
    r.unwrap();
    assert_eq!(steps, ["checked 5", "sent", "confirming 5", "confirmed", "recorded 3"]);
    assert!(dir.has_disk_key(), "the disk key was saved");
    assert_eq!(dir.state().unwrap().last_unlocked_seq, Some(3));
    assert_eq!(pod.status().state, State::Open);

    pod.service.volume.close().unwrap();
    pod.reboot(None);
    assert_eq!(pod.status().state, State::Locked);
    let (r, steps) = request(&pod, &dir, Kind::Unlock, 3);
    r.unwrap();
    assert_eq!(steps.last().map(String::as_str), Some("recorded 3"));
    assert_eq!(pod.status().state, State::Open);
}

#[test]
fn an_unknown_release_is_refused_before_any_key_is_made_or_sent() {
    let scratch = common::scratch::Scratch::new();
    let dir = owner_dir(&scratch, 0xbb);
    let pod = common::Pod::new();
    // the status alone: nothing is sent after it
    let (r, steps) = request(&pod, &dir, Kind::Init, 1);
    let err = r.unwrap_err();
    assert!(err.downcast_ref::<Refusal>().is_some(), "{err:#}");
    assert!(format!("{err:#}").contains("release"), "{err:#}");
    assert_eq!(steps, ["checked 5"]);
    assert!(!dir.path(DISK_KEY).exists(), "no disk key was made");
    assert_eq!(pod.status().state, State::Blank);
}
