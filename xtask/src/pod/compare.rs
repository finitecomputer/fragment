//! `cargo xtask pod image compare <a> <b> [--same]`: two builds' outputs
//! (target/pod-image/<variant>/, or any folder `pod image build`
//! fetched) side by side: the disk's, UKI's and root hash's hashes, the
//! partition table, and the UKI section by section, with what differs in
//! its command line. `--same` fails unless the two are byte for byte the
//! same build, which is how a rebuild shows it reproduces.
//!
//! It replaces compare-builds.sh for what the fetched files show. That
//! script also mounted both disks on the build box and diffed their root
//! filesystems file by file, which needs the disks, root and Linux; the
//! root is covered here by the root hash and the UKI's `.cmdline`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fragment_pod::attest::pe;

const USAGE: &str = "usage: cargo xtask pod image compare <build output> <build output> [--same]";
const MAX_UKI: u64 = 512 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub what: String,
    pub same: bool,
    pub detail: Option<String>,
}

fn read(dir: &Path, f: &str) -> Result<Vec<u8>> {
    let p = dir.join(f);
    let len = std::fs::metadata(&p).with_context(|| format!("read {}", p.display()))?.len();
    anyhow::ensure!(len <= MAX_UKI, "{} is over {MAX_UKI} bytes", p.display());
    std::fs::read(&p).with_context(|| format!("read {}", p.display()))
}

fn sums(dir: &Path) -> Result<Vec<(String, String)>> {
    let text = String::from_utf8(read(dir, "sha256sums")?).context("sha256sums")?;
    Ok(text.lines().filter_map(|l| l.split_once("  ").map(|(h, f)| (f.to_string(), h.to_string()))).collect())
}

/// Each comparison, in order.
pub fn compare(a: &Path, b: &Path) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    let (sa, sb) = (sums(a)?, sums(b)?);
    for (file, ha) in &sa {
        let hb = sb.iter().find(|(f, _)| f == file).map(|(_, h)| h.as_str());
        out.push(Line { what: format!("{file} sha256"), same: hb == Some(ha.as_str()), detail: Some(format!("{ha} / {}", hb.unwrap_or("none"))) });
    }
    for f in ["gpt.bin", "partitions.json"] {
        out.push(Line { what: f.to_string(), same: read(a, f)? == read(b, f)?, detail: None });
    }
    let (ua, ub) = (read(a, "pod.efi")?, read(b, "pod.efi")?);
    let (pa, pb) = (pe::parse(&ua).context("a's UKI")?, pe::parse(&ub).context("b's UKI")?);
    let differing = ua.iter().zip(&ub).filter(|(x, y)| x != y).count() + ua.len().abs_diff(ub.len());
    out.push(Line { what: "pod.efi bytes".into(), same: ua == ub, detail: Some(format!("{} and {} bytes, {differing} differing", ua.len(), ub.len())) });
    let mut names: Vec<&str> = pa.sections.iter().map(|s| s.name.as_str()).collect();
    for s in &pb.sections {
        if !names.contains(&s.name.as_str()) {
            names.push(&s.name);
        }
    }
    for name in names {
        let (x, y) = (pa.section_in_memory(name)?, pb.section_in_memory(name)?);
        let detail = match (&x, &y) {
            (Some(x), Some(y)) if x != y && name == ".cmdline" => Some(format!("a: {}\n      b: {}", text(x), text(y))),
            (None, _) | (_, None) => Some(format!("only in {}", if x.is_some() { "a" } else { "b" })),
            _ => None,
        };
        out.push(Line { what: format!("UKI section {name}"), same: x == y, detail });
    }
    Ok(out)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_end_matches(['\0', '\n']).to_string()
}

pub fn run(args: &[String]) -> Result<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut same = false;
    for a in args {
        match a.as_str() {
            "--same" => same = true,
            d if !d.starts_with('-') && dirs.len() < 2 => dirs.push(PathBuf::from(d)),
            _ => anyhow::bail!("{USAGE}"),
        }
    }
    let [a, b] = <[PathBuf; 2]>::try_from(dirs).map_err(|_| anyhow::anyhow!("{USAGE}"))?;
    let lines = compare(&a, &b)?;
    println!("a: {}\nb: {}", a.display(), b.display());
    for l in &lines {
        println!("  {} {}", if l.same { "same   " } else { "differs" }, l.what);
        if let Some(d) = &l.detail {
            if !l.same || l.what.ends_with("sha256") {
                println!("      {d}");
            }
        }
    }
    let differ = lines.iter().filter(|l| !l.same).count();
    println!("{} of {} differ", differ, lines.len());
    anyhow::ensure!(!same || differ == 0, "--same: the two builds differ in {differ} places");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The proof of concept's builds: A and its rebuild A-again are the
    /// same build; A and B (one file changed in the root) differ in the
    /// disk, the root hash, the partition table (its UUIDs follow the
    /// root hash) and the UKI's .cmdline (its roothash=) alone. Ignored
    /// unless POD_IMAGE_OUT names their folder.
    #[test]
    #[ignore]
    fn a_rebuild_is_the_same_and_one_file_moves_the_root_hash() {
        let out = PathBuf::from(std::env::var("POD_IMAGE_OUT").expect("POD_IMAGE_OUT names the built images' folder"));
        let again = compare(&out.join("A"), &out.join("A-again")).unwrap();
        assert!(again.iter().all(|l| l.same), "{again:?}");
        let b = compare(&out.join("A"), &out.join("B")).unwrap();
        let differ: Vec<&str> = b.iter().filter(|l| !l.same).map(|l| l.what.as_str()).collect();
        assert_eq!(differ, ["pod.raw sha256", "pod.efi sha256", "pod.roothash sha256", "gpt.bin", "partitions.json", "pod.efi bytes", "UKI section .cmdline"]);
    }

    #[test]
    fn needs_two_folders() {
        assert!(run(&["a".into()]).is_err());
        assert!(run(&["a".into(), "b".into(), "c".into()]).is_err());
        assert!(run(&["a".into(), "--bad".into()]).is_err());
    }
}
