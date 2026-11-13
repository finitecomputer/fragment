//! The pod image's pins, read from the tree: its tools
//! (pod-image/pins.jsonc) and the platform it carries
//! (pod-image/pinned-platform/pins.jsonc, recorded debt). The plans take
//! every version, commit and hash from here, and from nowhere else.

use std::path::Path;

use anyhow::{Context, Result};
use fragment_devstack as devstack;
use serde::Deserialize;

pub const TOOLS: &str = "pod-image/pins.jsonc";
pub const PLATFORM_DIR: &str = "pod-image/pinned-platform";
pub const PLATFORM: &str = "pod-image/pinned-platform/pins.jsonc";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tools {
    pub mkosi: Mkosi,
    pub stub: Stub,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mkosi {
    pub url: String,
    pub tag: String,
    pub commit: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stub {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub celld: Repo,
    pub sandcastle: Repo,
    pub fragment: Fragment,
    pub stack_uid: u32,
    pub stack_gid: u32,
    pub packages: Packages,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub url: String,
    pub branch: String,
    pub commit: String,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    pub url: String,
    pub branch: String,
    pub commit: String,
    pub patch: String,
    pub tree: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    pub docker: String,
    pub containerd: String,
    pub tailscale: String,
    pub sha256s: String,
}

fn read<T: serde::de::DeserializeOwned>(root: &Path, path: &str) -> Result<T> {
    let text = std::fs::read_to_string(root.join(path)).with_context(|| format!("read {path}"))?;
    serde_json::from_str(&devstack::strip_comments(&text)).with_context(|| format!("parse {path}"))
}

fn hex_ok(what: &str, s: &str, lens: &[usize]) -> Result<()> {
    anyhow::ensure!(lens.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{what}: {s:?} is not a pinned hash");
    Ok(())
}

pub fn tools(root: &Path) -> Result<Tools> {
    let t: Tools = read(root, TOOLS)?;
    hex_ok("mkosi.commit", &t.mkosi.commit, &[40])?;
    hex_ok("stub.sha256", &t.stub.sha256, &[64])?;
    anyhow::ensure!(t.stub.url.ends_with(".deb") && t.stub.url.rsplit('/').next().is_some_and(|f| !f.is_empty()), "stub.url names a .deb");
    Ok(t)
}

pub fn platform(root: &Path) -> Result<Platform> {
    let p: Platform = read(root, PLATFORM)?;
    for (what, c) in [("celld", &p.celld.commit), ("sandcastle", &p.sandcastle.commit), ("fragment", &p.fragment.commit)] {
        hex_ok(what, c, &[7, 8, 40])?;
    }
    for patch in [p.sandcastle.patch.as_deref(), p.celld.patch.as_deref(), Some(p.fragment.patch.as_str()), Some(p.packages.sha256s.as_str())].into_iter().flatten() {
        anyhow::ensure!(root.join(PLATFORM_DIR).join(patch).is_file(), "{PLATFORM_DIR}/{patch} is missing");
    }
    anyhow::ensure!(p.fragment.tree.starts_with('/'), "fragment.tree is an absolute path");
    Ok(p)
}

/// The stub package's file name.
pub fn stub_file(t: &Tools) -> &str {
    t.stub.url.rsplit('/').next().expect("checked")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in pins read, and say what the proof of concept's
    /// images were built from (the debt ledger names the same).
    #[test]
    fn the_pins_read_and_are_the_proof_of_concepts() {
        let root = devstack::repo_root();
        let t = tools(&root).unwrap();
        assert_eq!((t.mkosi.tag.as_str(), t.mkosi.commit.as_str()), ("v27.1", "ecf7f5702bb2052b7a69cab3fe7d37bc3b9ad77b"));
        assert_eq!(stub_file(&t), "systemd-boot-efi_257.13-1~deb13u1_amd64.deb");
        let p = platform(&root).unwrap();
        assert_eq!((p.fragment.commit.as_str(), p.celld.commit.as_str(), p.sandcastle.commit.as_str()), ("2e123d40", "b19f09d", "5d64525"));
        let ledger = std::fs::read_to_string(root.join("docs/technical-debt-ledger.md")).unwrap();
        for pin in ["2e123d40", "b19f09d", "5d64525", "selfhost-http-port", "pod-image/pinned-platform/"] {
            assert!(ledger.contains(pin), "the debt entry names {pin}");
        }
        let debs = std::fs::read_to_string(root.join(PLATFORM_DIR).join(&p.packages.sha256s)).unwrap();
        assert_eq!(debs.lines().count(), 4, "four packages, each held to its hash");
        for v in [&p.packages.docker, &p.packages.containerd, &p.packages.tailscale] {
            let file_form = v.replace(':', "%3a");
            assert!(debs.contains(&file_form), "debs.sha256 holds {v}");
        }
    }
}
