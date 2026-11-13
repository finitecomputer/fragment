//! A pod's deployment settings: a JSONC file outside the repo, named with
//! `--config` (as `cargo xtask deploy` takes its own), its shape shown in
//! docs/pod.md ("The image"). Nothing here has a default that names a
//! project, a zone, a machine or a disk: each comes from the file, and a
//! missing or misspelt key is refused before anything runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;
use serde::Deserialize;

use super::Variant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    project: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    build: Option<BuildFile>,
    images: Option<ImagesFile>,
    vm: Option<VmFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildFile {
    host: String,
    instance: Option<String>,
    zone: Option<String>,
    home: String,
    dir: Option<String>,
    debug_key: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagesFile {
    locked: String,
    debug: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VmFile {
    name: String,
    zone: String,
    machine_type: String,
    variant: String,
    boot_disk_gb: u32,
    boot_disk_type: Option<String>,
    data_disk: DataDiskFile,
    network: String,
    subnet: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    max_run_duration: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DataDiskFile {
    name: String,
    size_gb: u32,
    #[serde(rename = "type")]
    kind: String,
}

/// The settings, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub project: String,
    /// Put on everything made, beside the tool's own (`pod-role`,
    /// `pod-variant`).
    pub labels: BTreeMap<String, String>,
    pub build: Option<Build>,
    pub images: Option<Images>,
    pub vm: Option<Vm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    /// This machine, which must be Linux.
    Local,
    /// A Compute Engine VM, reached with `gcloud compute ssh`.
    Gce { instance: String, zone: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub host: HostKind,
    /// The build user's home on the host: its cargo and worker-build's
    /// cache are there.
    pub home: String,
    /// Where the build keeps its sources, tools tree and outputs.
    pub dir: String,
    /// The one SSH public key the debug image admits (a file here).
    pub debug_key: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Images {
    pub locked: String,
    pub debug: String,
}

impl Images {
    pub fn of(&self, v: Variant) -> &str {
        match v {
            Variant::Locked => &self.locked,
            Variant::Debug => &self.debug,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vm {
    pub name: String,
    pub zone: String,
    pub machine_type: String,
    pub variant: Variant,
    pub boot_disk_gb: u32,
    pub boot_disk_type: String,
    pub data_disk: DataDisk,
    pub network: String,
    pub subnet: Option<String>,
    pub tags: Vec<String>,
    pub max_run_duration: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDisk {
    pub name: String,
    pub size_gb: u32,
    pub kind: String,
}

/// The largest a disk here may be asked for: a pod's data disk is tens
/// of GB, and a typo of a few digits should not reach the bill.
pub const MAX_DISK_GB: u32 = 4096;

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("{}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let f: File = serde_json::from_str(&devstack::strip_comments(text)).context("parse the pod config")?;
        project_ok(&f.project)?;
        for (k, v) in &f.labels {
            label_ok(k, v)?;
            anyhow::ensure!(!k.starts_with("pod-"), "labels: {k} is the tool's own (pod-role, pod-variant)");
        }
        let build = f.build.map(build).transpose().context("build")?;
        let images = f.images.map(images).transpose().context("images")?;
        let vm = f.vm.map(vm).transpose().context("vm")?;
        Ok(Self { project: f.project, labels: f.labels, build, images, vm })
    }

    pub fn build(&self) -> Result<&Build> {
        self.build.as_ref().context("the config has no build section")
    }

    pub fn images(&self) -> Result<&Images> {
        self.images.as_ref().context("the config has no images section")
    }

    pub fn vm(&self) -> Result<&Vm> {
        self.vm.as_ref().context("the config has no vm section")
    }

    /// `--labels`' value: the config's labels and these, sorted.
    pub fn labels_with(&self, own: &[(&str, &str)]) -> String {
        let mut all: BTreeMap<&str, &str> = self.labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        all.extend(own.iter().copied());
        all.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
    }
}

fn build(b: BuildFile) -> Result<Build> {
    let host = match (b.host.as_str(), b.instance, b.zone) {
        ("local", None, None) => HostKind::Local,
        ("local", _, _) => bail!("host \"local\" takes no instance or zone"),
        ("gce", Some(instance), Some(zone)) => {
            name_ok("instance", &instance)?;
            zone_ok(&zone)?;
            HostKind::Gce { instance, zone }
        }
        ("gce", _, _) => bail!("host \"gce\" needs instance and zone"),
        (other, _, _) => bail!("host is \"local\" or \"gce\", not {other:?}"),
    };
    path_ok("home", &b.home)?;
    let dir = b.dir.unwrap_or_else(|| format!("{}/podimg", b.home.trim_end_matches('/')));
    path_ok("dir", &dir)?;
    Ok(Build { host, home: b.home.trim_end_matches('/').to_string(), dir: dir.trim_end_matches('/').to_string(), debug_key: b.debug_key })
}

fn images(i: ImagesFile) -> Result<Images> {
    for (field, name) in [("locked", &i.locked), ("debug", &i.debug)] {
        name_ok(field, name)?;
        // its scratch disk is `<image>-scratch`, a name of 63 at most
        anyhow::ensure!(name.len() <= 55, "{field}: {name:?} is over 55 characters, and its scratch disk's name would be over 63");
    }
    anyhow::ensure!(i.locked != i.debug, "the locked and debug images need two names");
    Ok(Images { locked: i.locked, debug: i.debug })
}

fn vm(v: VmFile) -> Result<Vm> {
    name_ok("name", &v.name)?;
    zone_ok(&v.zone)?;
    name_ok("machine_type", &v.machine_type)?;
    // TDX on Compute Engine: C3 and C4 (docs/explorations/private-pod-findings.md)
    anyhow::ensure!(
        v.machine_type.starts_with("c3-") || v.machine_type.starts_with("c4-"),
        "machine_type {:?}: a TDX machine type, c3-* or c4-*",
        v.machine_type
    );
    let variant = Variant::parse(&v.variant)?;
    anyhow::ensure!((10..=MAX_DISK_GB).contains(&v.boot_disk_gb), "boot_disk_gb {}: 10 to {MAX_DISK_GB}", v.boot_disk_gb);
    // a C4 takes only Hyperdisk; a C3 pd-balanced
    let boot_disk_type = v.boot_disk_type.unwrap_or_else(|| if v.machine_type.starts_with("c4-") { "hyperdisk-balanced" } else { "pd-balanced" }.into());
    name_ok("boot_disk_type", &boot_disk_type)?;
    let d = v.data_disk;
    name_ok("data_disk.name", &d.name)?;
    name_ok("data_disk.type", &d.kind)?;
    anyhow::ensure!((1..=MAX_DISK_GB).contains(&d.size_gb), "data_disk.size_gb {}: 1 to {MAX_DISK_GB}", d.size_gb);
    anyhow::ensure!(d.name != v.name, "data_disk.name is the VM's name, which its boot disk takes");
    name_ok("network", &v.network)?;
    if let Some(s) = &v.subnet {
        name_ok("subnet", s)?;
    }
    for t in &v.tags {
        name_ok("tags", t)?;
    }
    if let Some(d) = &v.max_run_duration {
        duration_ok(d)?;
    }
    Ok(Vm {
        name: v.name,
        zone: v.zone,
        machine_type: v.machine_type,
        variant,
        boot_disk_gb: v.boot_disk_gb,
        boot_disk_type,
        data_disk: DataDisk { name: d.name, size_gb: d.size_gb, kind: d.kind },
        network: v.network,
        subnet: v.subnet,
        tags: v.tags,
        max_run_duration: v.max_run_duration,
    })
}

/// A Compute Engine resource name: a lowercase letter, then up to 62
/// lowercase letters, digits and hyphens, not ending in a hyphen.
pub fn name_ok(field: &str, name: &str) -> Result<()> {
    let b = name.as_bytes();
    let ok = (1..=63).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !name.ends_with('-');
    anyhow::ensure!(ok, "{field}: {name:?} is not a Compute Engine name (a lowercase letter, then lowercase letters, digits and hyphens, at most 63, not ending in a hyphen)");
    Ok(())
}

/// A zone: `<region>-<letter>`, the region `<area>-<place><n>`.
pub fn zone_ok(zone: &str) -> Result<()> {
    let parts: Vec<&str> = zone.split('-').collect();
    let ok = parts.len() == 3
        && parts[0].len() >= 2
        && parts[0].bytes().all(|c| c.is_ascii_lowercase())
        && parts[1].bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && parts[1].bytes().last().is_some_and(|c| c.is_ascii_digit())
        && parts[1].bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && parts[2].len() == 1
        && parts[2].bytes().all(|c| c.is_ascii_lowercase());
    anyhow::ensure!(ok, "zone {zone:?} is not a zone (say us-central1-f)");
    Ok(())
}

/// The region a zone is in.
pub fn region(zone: &str) -> &str {
    zone.rsplit_once('-').map_or(zone, |(r, _)| r)
}

/// A project id: 6 to 30 lowercase letters, digits and hyphens, starting
/// with a letter, not ending in a hyphen.
fn project_ok(p: &str) -> Result<()> {
    let ok = (6..=30).contains(&p.len())
        && p.as_bytes()[0].is_ascii_lowercase()
        && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !p.ends_with('-');
    anyhow::ensure!(ok, "project {p:?} is not a project id");
    Ok(())
}

fn label_ok(k: &str, v: &str) -> Result<()> {
    let word = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_';
    let key = (1..=63).contains(&k.len()) && k.as_bytes()[0].is_ascii_lowercase() && k.bytes().all(word);
    anyhow::ensure!(key && v.len() <= 63 && v.bytes().all(word), "label {k}={v}: lowercase letters, digits, - and _, at most 63");
    Ok(())
}

fn path_ok(field: &str, p: &str) -> Result<()> {
    let ok = p.starts_with('/') && p.trim_end_matches('/').len() > 1 && !p.chars().any(char::is_control) && !p.split('/').any(|c| c == "..");
    anyhow::ensure!(ok, "{field}: {p:?} must be an absolute path on the build host, not / and with no ..");
    Ok(())
}

/// `--max-run-duration`'s form: a number and s, m, h or d.
fn duration_ok(d: &str) -> Result<()> {
    let (n, unit) = d.split_at(d.len().saturating_sub(1));
    let ok = !n.is_empty() && n.len() <= 6 && n.bytes().all(|c| c.is_ascii_digit()) && ["s", "m", "h", "d"].contains(&unit);
    anyhow::ensure!(ok, "max_run_duration {d:?}: a duration such as 3h or 90m");
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A config with every section, as docs/pod.md shows it.
    pub const EXAMPLE: &str = r#"{
      // a comment, as JSONC has
      "project": "pod-project",
      "labels": { "owner": "alex" },
      "build": {
        "host": "gce", "instance": "pod-build", "zone": "us-central1-f",
        "home": "/home/builder",
        "debug_key": "/tmp/pod-debug-key.pub"
      },
      "images": { "locked": "pod-p", "debug": "pod-p-debug" },
      "vm": {
        "name": "pod-1", "zone": "us-central1-f", "machine_type": "c3-standard-8",
        "variant": "locked", "boot_disk_gb": 10,
        "data_disk": { "name": "pod-1-data", "size_gb": 20, "type": "pd-balanced" },
        "network": "pod-net", "subnet": "pod-subnet", "tags": ["pod-7000"],
        "max_run_duration": "3h"
      }
    }"#;

    pub fn example() -> Config {
        Config::parse(EXAMPLE).unwrap()
    }

    #[test]
    fn reads_every_section_and_its_defaults() {
        let c = example();
        let b = c.build().unwrap();
        assert_eq!(b.host, HostKind::Gce { instance: "pod-build".into(), zone: "us-central1-f".into() });
        assert_eq!(b.dir, "/home/builder/podimg", "dir defaults beside home");
        let v = c.vm().unwrap();
        assert_eq!(v.boot_disk_type, "pd-balanced", "a C3's boot disk");
        assert_eq!(v.variant, Variant::Locked);
        assert_eq!(c.images().unwrap().of(Variant::Debug), "pod-p-debug");
        assert_eq!(c.labels_with(&[("pod-role", "vm")]), "owner=alex,pod-role=vm");
        let c4 = Config::parse(&EXAMPLE.replace("c3-standard-8", "c4-standard-8")).unwrap();
        assert_eq!(c4.vm().unwrap().boot_disk_type, "hyperdisk-balanced");
        assert_eq!(region("us-central1-f"), "us-central1");
    }

    /// A section a command does not need may be left out; a section it
    /// needs may not.
    #[test]
    fn sections_are_asked_for_by_the_command_that_needs_them() {
        let c = Config::parse(r#"{"project": "pod-project"}"#).unwrap();
        assert!(c.build().unwrap_err().to_string().contains("no build section"));
        assert!(c.vm().is_err() && c.images().is_err());
    }

    /// Invalid: a field missing, an unknown one, a name Compute Engine
    /// refuses, a zone that is not one, a host half given, a label of the
    /// tool's own, a duration and sizes out of bounds.
    #[test]
    fn refuses_what_compute_engine_would_or_a_typo() {
        let bad = |from: &str, to: &str| {
            assert!(EXAMPLE.contains(from), "{from}");
            let e = Config::parse(&EXAMPLE.replacen(from, to, 1)).unwrap_err();
            format!("{e:#}")
        };
        assert!(bad(r#""machine_type": "c3-standard-8","#, "").contains("missing field `machine_type`"));
        assert!(bad(r#""network": "pod-net","#, r#""netwrok": "pod-net","#).contains("unknown field `netwrok`"));
        assert!(bad(r#""name": "pod-1","#, r#""name": "Pod_1","#).contains("not a Compute Engine name"));
        assert!(bad(r#""name": "pod-1","#, r#""name": "pod-1-","#).contains("not a Compute Engine name"));
        assert!(bad(r#""name": "pod-1","#, &format!(r#""name": "p{}","#, "o".repeat(63))).contains("not a Compute Engine name"));
        assert!(bad(r#""locked": "pod-p""#, &format!(r#""locked": "p{}""#, "o".repeat(55))).contains("over 55"));
        assert!(bad(r#""zone": "us-central1-f", "machine_type""#, r#""zone": "us-central1", "machine_type""#).contains("not a zone"));
        assert!(bad(r#""instance": "pod-build", "#, "").contains("needs instance and zone"));
        assert!(bad(r#""host": "gce""#, r#""host": "ssh""#).contains("\"local\" or \"gce\""));
        assert!(bad(r#""owner": "alex""#, r#""pod-role": "x""#).contains("the tool's own"));
        assert!(bad(r#""3h""#, r#""3 hours""#).contains("max_run_duration"));
        assert!(bad(r#""size_gb": 20"#, r#""size_gb": 40000"#).contains("size_gb"));
        assert!(bad(r#""c3-standard-8""#, r#""n2d-standard-4""#).contains("TDX machine type"));
        assert!(bad(r#""variant": "locked""#, r#""variant": "P""#).contains("locked or debug"));
        assert!(bad(r#""/home/builder""#, r#""home/builder""#).contains("absolute path"));
        assert!(bad(r#""project": "pod-project""#, r#""project": "Pod""#).contains("not a project id"));
        assert!(bad(r#""pod-1-data""#, r#""pod-1""#).contains("its boot disk"));
        assert!(bad(r#""debug": "pod-p-debug""#, r#""debug": "pod-p""#).contains("two names"));
    }
}
