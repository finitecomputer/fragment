//! A fake volume and quoter, and a pod-init service over a temporary
//! directory, driven through the real request reader and handler. Each
//! test file uses only part of it.
#![allow(dead_code)]

#[path = "../../src/scratch.rs"]
pub mod scratch;

use fragment_pod_init::config::Config;
use fragment_pod_init::quoter::Quoter;
use fragment_pod_init::service::Service;
use fragment_pod_init::volume::{Volume, VolumeError};
use fragment_pod_init::http::{self, Response};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::{BootKey, DiskKey};
use fragment_pod::unlock::request;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use fragment_pod::unlock::wire::{ErrorBody, KeyRequest, Status};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The "image" holds the key's hex where LUKS keeps its header. Open is
/// the store directory renamed to the mount point; close renames it
/// back, so files written while open are there at the next open, as on
/// a real volume, and absent while locked.
/// Its header's owner hint is a file beside the "image".
pub struct FakeVolume {
    image: PathBuf,
    store: PathBuf,
    mount: PathBuf,
    header_hint: PathBuf,
}

impl FakeVolume {
    pub fn new(c: &Config, dir: &Path) -> Self {
        Self { image: c.volume_image.clone(), store: dir.join("store"), mount: c.mount.clone(), header_hint: dir.join("header.hint") }
    }
}

fn io(step: &'static str) -> impl Fn(std::io::Error) -> VolumeError {
    move |e| VolumeError::Step { step, detail: e.to_string() }
}

impl Volume for FakeVolume {
    fn state(&self) -> Result<State, VolumeError> {
        Ok(match (self.image.exists(), self.mount.exists()) {
            (false, _) => State::Blank,
            (true, true) => State::Open,
            (true, false) => State::Locked,
        })
    }
    fn create(&self, key: &DiskKey) -> Result<(), VolumeError> {
        fs::write(&self.image, key.to_hex().as_bytes()).map_err(io("format"))?;
        fs::create_dir(&self.mount).map_err(io("mount"))
    }
    fn open(&self, key: &DiskKey) -> Result<(), VolumeError> {
        let header = fs::read_to_string(&self.image).map_err(io("open"))?;
        if header != *key.to_hex() {
            return Err(VolumeError::WrongKey);
        }
        fs::rename(&self.store, &self.mount).map_err(io("mount"))
    }
    fn close(&self) -> Result<(), VolumeError> {
        fs::rename(&self.mount, &self.store).map_err(io("umount"))
    }
    fn header_hint(&self) -> Result<Option<[u8; 32]>, VolumeError> {
        fragment_pod_init::owner::read(&self.header_hint).map_err(|detail| VolumeError::Step { step: "header hint", detail })
    }
    fn set_header_hint(&self, owner: &[u8; 32]) -> Result<(), VolumeError> {
        fragment_pod_init::owner::write(&self.header_hint, owner).map_err(io("header hint"))
    }
}

/// "QUOTE" and the REPORTDATA, as tsm's fake kernel makes them.
pub struct FakeQuoter;

impl Quoter for FakeQuoter {
    fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, String> {
        Ok([b"QUOTE".as_slice(), report_data].concat())
    }
}

pub type MakeVolume = fn(&Config, &Path) -> Box<dyn Volume + Send + Sync>;

pub fn fake_volume(c: &Config, dir: &Path) -> Box<dyn Volume + Send + Sync> {
    Box::new(FakeVolume::new(c, dir))
}

pub struct Pod {
    pub dir: scratch::Scratch,
    pub service: Service,
    volume: MakeVolume,
    /// No hint file: the hint is the volume header's.
    header_hint: bool,
}

pub fn config(dir: &Path, hook: Option<PathBuf>) -> Config {
    Config {
        volume_image: dir.join("volume.img"),
        volume_size_gb: 1,
        mapper_name: "pod".into(),
        mount: dir.join("mnt"),
        owner_file: "pod/owner".into(),
        owner_hint: Some(dir.join("owner.hint")),
        tls_root: dir.join("mnt/tls/root.pem"),
        open_hook: hook,
    }
}

pub fn service(dir: &Path, hook: Option<PathBuf>, volume: MakeVolume) -> Service {
    service_with(dir, hook, volume, false)
}

pub fn service_with(dir: &Path, hook: Option<PathBuf>, volume: MakeVolume, header_hint: bool) -> Service {
    let mut config = config(dir, hook);
    if header_hint {
        config.owner_hint = None;
    }
    fs::write(dir.join("CCEL"), b"LOG").unwrap();
    Service {
        volume: volume(&config, dir),
        quoter: Box::new(FakeQuoter),
        boot: BootKey::generate().unwrap(),
        ccel: dir.join("CCEL"),
        hook_timeout: Duration::from_secs(10),
        config,
    }
}

impl Pod {
    pub fn new() -> Self {
        Self::with(fake_volume)
    }

    pub fn with(volume: MakeVolume) -> Self {
        Self::configured(volume, false)
    }

    /// `header_hint`: the config names no hint file, so the volume's
    /// header keeps the hint (a measured image's read-only root).
    pub fn configured(volume: MakeVolume, header_hint: bool) -> Self {
        let dir = scratch::Scratch::new();
        let service = service_with(dir.path(), None, volume, header_hint);
        Self { dir, service, volume, header_hint }
    }

    /// A reboot: the same disk, a new boot key.
    pub fn reboot(&mut self, hook: Option<PathBuf>) {
        self.service = service_with(self.dir.path(), hook, self.volume, self.header_hint);
    }

    /// Raw HTTP bytes through the real reader, then the handler.
    pub fn raw(&self, bytes: &[u8]) -> Response {
        self.service.respond(http::read_request_with_body(bytes, fragment_pod_init::service::MAX_BODY))
    }

    pub fn get(&self, target: &str) -> Response {
        self.raw(format!("GET {target} HTTP/1.1\r\nHost: pod\r\n\r\n").as_bytes())
    }

    pub fn post(&self, path: &str, body: &[u8]) -> Response {
        let head = format!("POST {path} HTTP/1.1\r\nHost: pod\r\nContent-Length: {}\r\n\r\n", body.len());
        self.raw(&[head.as_bytes(), body].concat())
    }

    pub fn status(&self) -> Status {
        let r = self.get(&format!("/status?nonce={}", "11".repeat(32)));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        Status::from_json(&r.body).unwrap()
    }

    pub fn request(&self, kind: Kind, owner: &OwnerKey, key: &DiskKey) -> KeyRequest {
        request::make(kind, &self.status().boot_pubkey, owner, key).unwrap()
    }

    pub fn send(&self, kind: Kind, r: &KeyRequest) -> Response {
        let path = match kind {
            Kind::Init => "/init",
            Kind::Unlock => "/unlock",
        };
        self.post(path, &r.to_json().unwrap())
    }

    pub fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }
}

pub fn error(r: &Response) -> ErrorBody {
    serde_json::from_slice(&r.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&r.body)))
}
