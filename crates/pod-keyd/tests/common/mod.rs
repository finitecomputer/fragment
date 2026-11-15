//! A key service over fakes, driven through the real request reader and
//! handler, beside real pod-init services (pod-init's fake volume and
//! quoter). Quotes are fake: five bytes naming the TD, then REPORTDATA.
//! The fake attestor plays Intel and Google: it "verifies" a quote of a
//! TD it knows, with the registers, instance and platform the test set.
//! Also included by the CLI's src/pod/keyd_tests.rs, which uses only part of it.
#![allow(dead_code)]

#[path = "../../../pod-init/tests/common/mod.rs"]
pub mod pod;

use fragment_pod::attest::pzid::Instance;
use fragment_pod::attest::report::{attr, RegistersHex, TdReport};
use fragment_pod::checks::attestor::Attestor;
use fragment_pod_keyd::clock::Clock;
use fragment_pod_keyd::nonces::Challenges;
use fragment_pod_keyd::service::{Service, MAX_BODY};
use fragment_pod_init::http::{self, Response};
use fragment_pod_init::quoter::Quoter;
use fragment_pod::keyd::approve::Approval;
use fragment_pod::keyd::binding::KeydStatus;
use fragment_pod::keyd::deposit::{Deposit, Settings};
use fragment_pod::keyd::delivery::{self, ReleaseRequest};
use fragment_pod::keyd::view::RecordView;
use fragment_pod::unlock::keys::{BootKey, DiskKey};
use fragment_pod::unlock::release::Release;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use fragment_pod::unlock::wire::{ErrorBody, Status};
use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::provenance::{Document, HostRegistry};
use fragment_pod::verify::tdx::{PckFlags, Verified};
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// pod-init's fake quoter's prefix, and the key service's.
pub const POD: [u8; 5] = *b"QUOTE";
pub const KEYD: [u8; 5] = *b"KEYD!";
pub const POD_INSTANCE: &str = "projects/1/zones/us-central1-b/instances/100";
pub const KEYD_INSTANCE: &str = "projects/1/zones/us-central1-b/instances/200";
/// The fake clock starts here (2026-11-13).
pub const T0: i64 = 1_794_528_000;

#[derive(Clone)]
pub struct Td {
    /// MRTD, RTMR0..3.
    pub registers: [[u8; 48]; 5],
    pub instance: Instance,
    pub tcb_status: String,
    pub td_attributes: u64,
    /// The zone Google's registry puts the host in.
    pub host_zone: String,
}

impl Td {
    pub fn new(seed: u8, instance: &str) -> Self {
        let instance = Instance::parse(instance).unwrap();
        Self {
            registers: [0, 1, 2, 3, 4].map(|i| [seed.wrapping_add(i); 48]),
            host_zone: instance.region().to_owned(),
            instance,
            tcb_status: "UpToDate".into(),
            td_attributes: attr::SEPT_VE_DISABLE,
        }
    }

    /// The release that pins this TD's MRTD, RTMR1 and RTMR2.
    pub fn release(&self, seq: u64) -> Release {
        let r = &self.registers;
        Release { seq, name: format!("release-{seq}"), registers: [Some(r[0]), None, Some(r[2]), Some(r[3]), None] }
    }

    fn report(&self, report_data: [u8; 64]) -> TdReport {
        let r = &self.registers;
        TdReport {
            quote_version: 4,
            tee_tcb_svn: [0; 16],
            mr_seam: [0; 48],
            td_attributes: self.td_attributes.to_le_bytes(),
            xfam: [0; 8],
            mr_td: r[0],
            mr_config_id: [0; 48],
            mr_owner: self.instance.mr_owner(),
            mr_owner_config: [0; 48],
            rtmr: [r[1], r[2], r[3], r[4]],
            report_data,
        }
    }
}

pub fn approval(release: Release) -> Approval {
    Approval { release, not_before: None, expires: None }
}

#[derive(Default)]
pub struct FakeAttestor {
    tds: Mutex<Vec<([u8; 5], Td)>>,
}

impl FakeAttestor {
    pub fn set(&self, prefix: [u8; 5], td: Td) {
        let mut tds = self.tds.lock().unwrap();
        tds.retain(|(p, _)| *p != prefix);
        tds.push((prefix, td));
    }

    pub fn td(&self, prefix: [u8; 5]) -> Td {
        self.tds.lock().unwrap().iter().find(|(p, _)| *p == prefix).unwrap().1.clone()
    }

    fn known(&self, quote: &[u8]) -> Result<Td, Failure> {
        if quote.len() != 69 {
            return Err(Failure::Quote { detail: format!("{} bytes; a fake quote is 69", quote.len()) });
        }
        let tds = self.tds.lock().unwrap();
        let found = tds.iter().find(|(p, _)| p == &quote[..5]).map(|(_, td)| td.clone());
        found.ok_or_else(|| Failure::Chain { detail: "not a TD this fake Intel signed".into() })
    }
}

impl Attestor for FakeAttestor {
    fn chain(&self, quote: &[u8], at: i64) -> Result<Verified, Failure> {
        let td = self.known(quote)?;
        let report = td.report(quote[5..].try_into().unwrap());
        let flag = || "false".to_owned();
        Ok(Verified {
            quote_version: 4,
            tcb_status: td.tcb_status.clone(),
            advisory_ids: vec![],
            tcb_eval_data_number: 1,
            platform_tcb_date: "2026-01-01".into(),
            fmspc: "00806F050000".into(),
            ppid: hex::encode(&quote[..5]),
            pck_flags: PckFlags { smt_enabled: flag(), dynamic_platform: flag(), cached_keys: flag() },
            collateral_expires: at + 86_400,
            verified_at: at,
            td_attribute_flags: vec![],
            registers: RegistersHex::from(&report),
            report,
        })
    }

    fn registry(&self, quote: &[u8]) -> Result<HostRegistry, Failure> {
        let td = self.known(quote)?;
        let ppid = hex::encode(&quote[..5]);
        let document = Document { zone: td.host_zone, timestamp: "2026-02-17T11:25:12Z".into() };
        Ok(HostRegistry { url: format!("fake/{ppid}"), ppid, fetched_at: T0, document })
    }
}

pub struct FakeClock(pub AtomicI64);

impl FakeClock {
    pub fn set(&self, t: i64) {
        self.0.store(t, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct KeydQuoter;

impl Quoter for KeydQuoter {
    fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, String> {
        Ok([KEYD.as_slice(), report_data].concat())
    }
}

pub struct Keyd {
    pub service: Service,
    pub attestor: Arc<FakeAttestor>,
    pub clock: Arc<FakeClock>,
    dir: pod::scratch::Scratch,
}

fn service(attestor: &Arc<FakeAttestor>, clock: &Arc<FakeClock>, dir: &std::path::Path, owners: HashSet<[u8; 32]>) -> Service {
    std::fs::write(dir.join("CCEL"), b"KEYD LOG").unwrap();
    let (boot, challenges) = (BootKey::generate().unwrap(), Challenges::generate().unwrap());
    Service::new(boot, challenges, owners, Box::new(KeydQuoter), Box::new(attestor.clone()), Box::new(clock.clone()), dir.join("CCEL"))
}

impl Keyd {
    /// A key service whose operator named no owner yet (`admit`).
    pub fn new() -> Self {
        Self::admitting(&[])
    }

    /// A key service that takes deposits from `owners`; the fake Intel
    /// knows it and one pod TD.
    pub fn admitting(owners: &[[u8; 32]]) -> Self {
        let attestor = Arc::new(FakeAttestor::default());
        attestor.set(KEYD, Td::new(0x40, KEYD_INSTANCE));
        attestor.set(POD, Td::new(0x10, POD_INSTANCE));
        let clock = Arc::new(FakeClock(AtomicI64::new(T0)));
        let dir = pod::scratch::Scratch::new();
        let service = service(&attestor, &clock, dir.path(), owners.iter().copied().collect());
        Self { service, attestor, clock, dir }
    }

    /// As the operator naming `owner` when the service starts; before it
    /// is served.
    pub fn admit(&mut self, owner: &Owner) {
        self.service.owners.insert(owner.public());
    }

    /// A restart: new boot keys, the same owners named, and nothing else
    /// remembered.
    pub fn restart(&mut self) {
        self.service = service(&self.attestor, &self.clock, self.dir.path(), self.service.owners.clone());
    }

    pub fn pubkey(&self) -> [u8; 32] {
        self.service.boot.public()
    }

    pub fn raw(&self, bytes: &[u8]) -> Response {
        self.service.respond(http::read_request_with_body(bytes, MAX_BODY))
    }

    pub fn get(&self, target: &str) -> Response {
        self.raw(format!("GET {target} HTTP/1.1\r\nHost: keyd\r\n\r\n").as_bytes())
    }

    pub fn post(&self, path: &str, body: &[u8]) -> Response {
        let head = format!("POST {path} HTTP/1.1\r\nHost: keyd\r\nContent-Length: {}\r\n\r\n", body.len());
        self.raw(&[head.as_bytes(), body].concat())
    }

    pub fn status(&self, nonce: [u8; 32]) -> KeydStatus {
        let r = self.get(&format!("/status?nonce={}", hex::encode(nonce)));
        assert_eq!(r.status, 200, "{}", text(&r));
        KeydStatus::from_json(&r.body).unwrap()
    }

    pub fn nonce(&self) -> [u8; 32] {
        let r = self.get("/nonce");
        assert_eq!(r.status, 200, "{}", text(&r));
        delivery::nonce_from_json(&r.body).unwrap()
    }

    pub fn view(&self, owner: &[u8; 32]) -> Result<RecordView, ErrorBody> {
        let r = self.get(&format!("/pods/{}", hex::encode(owner)));
        match r.status {
            200 => Ok(RecordView::from_json(&r.body).unwrap()),
            _ => Err(error(&r)),
        }
    }

    pub fn deposit(&self, d: &Deposit) -> Response {
        self.post("/deposit", &d.to_json().unwrap())
    }

    pub fn release(&self, nonce: [u8; 32], status: Status) -> Response {
        self.post("/release", &ReleaseRequest { nonce, status }.to_json().unwrap())
    }
}

/// An owner with a disk key, and a locked pod of theirs (inited, then
/// rebooted) whose volume that key opens.
pub struct Owner {
    pub key: OwnerKey,
    pub disk: DiskKey,
}

impl Owner {
    pub fn new() -> Self {
        Self { key: OwnerKey::generate().unwrap(), disk: DiskKey::random().unwrap() }
    }

    pub fn public(&self) -> [u8; 32] {
        self.key.public()
    }

    pub fn locked_pod(&self) -> pod::Pod {
        let mut p = pod::Pod::new();
        let r = p.request(Kind::Init, &self.key, &self.disk);
        assert_eq!(p.send(Kind::Init, &r).status, 200);
        p.service.volume.close().unwrap();
        p.reboot(None);
        p
    }

    pub fn settings(&self, approvals: Vec<Approval>) -> Settings {
        Settings { replaces: None, gce_instance: POD_INSTANCE.into(), allow_tcb_status: vec![], min_seq: 0, approvals }
    }

    pub fn deposit_to(&self, keyd: &Keyd, settings: Settings) -> Deposit {
        Deposit::make(&self.key, &keyd.pubkey(), &self.disk, settings).unwrap()
    }
}

/// The pod's `/status` for `nonce`, through its real route.
pub fn pod_status(pod: &pod::Pod, nonce: [u8; 32]) -> Status {
    let r = pod.get(&format!("/status?nonce={}", hex::encode(nonce)));
    assert_eq!(r.status, 200, "{}", text(&r));
    Status::from_json(&r.body).unwrap()
}

pub fn text(r: &Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

pub fn error(r: &Response) -> ErrorBody {
    pod::error(r)
}

pub fn code(r: &Response) -> (u16, String) {
    (r.status, if r.status == 200 { "ok".into() } else { error(r).error })
}

/// The key service on a loopback port, served for the rest of the test
/// process (the thread is not joined; the process ends it).
pub fn serve(k: Keyd) -> (Arc<Keyd>, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let k = Arc::new(k);
    let served = k.clone();
    std::thread::spawn(move || served.service.serve(&listener, None));
    (k, addr)
}
