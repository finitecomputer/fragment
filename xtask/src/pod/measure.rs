//! `cargo xtask pod measure`: the registers a boot of a pod image must
//! show, and the release an owner's policy.json holds for it
//! (`unlock::release`, docs/pod.md "The owner's side").
//!
//! RTMR1 and RTMR2 come from the image's own bytes, its partition table
//! and its UKI, with fragment-pod's code (`attest::expect`,
//! `verify::measure`), so anyone can recompute them. MRTD (Google's
//! firmware) and RTMR0 (the host's tables, which differ by machine type
//! and by the disks attached at boot) cannot be computed from the image:
//! they are taken as observed, from a boot's quote or as given, and left
//! out of the release when neither is. RTMR3 must be zero, so the release
//! pins it so. This replaces the proof of concept's `pod-verify expect`
//! and check-pod.sh's policy-release.json.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fragment_pod::attest::report::{self, Mr};
use fragment_pod::unlock::release::{Release, ReleaseWire};
use fragment_pod::verify::{disk, measure};

const USAGE: &str = "usage: cargo xtask pod measure (--dir <build output> | --efi <pod.efi> --gpt <gpt.bin> | --image <pod.raw>)
         --name <release name> --seq <n> [--quote <quote.bin> [--log <ccel.bin>]] [--mrtd <hex>] [--rtmr0 <hex>] [--out <file>]
  --dir names a build's output folder (cargo xtask pod image build: pod.efi and gpt.bin);
  --quote takes MRTD and RTMR0 from a boot's quote and refuses one whose RTMR1, RTMR2 or RTMR3 differ;
  --mrtd and --rtmr0 give them as observed instead; the release is written to --out, else printed";

/// The largest file each input may be: a UKI is about 56 MB, a quote a
/// few KiB, a CCEL 256 KiB.
const MAX_UKI: u64 = 512 << 20;
const MAX_SMALL: u64 = 4 << 20;
/// The GPT as the firmware reads it: LBA 0 to 33.
const GPT_LEN: usize = 34 * 512;

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    dir: Option<PathBuf>,
    efi: Option<PathBuf>,
    gpt: Option<PathBuf>,
    image: Option<PathBuf>,
    name: Option<String>,
    seq: Option<u64>,
    quote: Option<PathBuf>,
    log: Option<PathBuf>,
    mrtd: Option<String>,
    rtmr0: Option<String>,
    out: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Args> {
    let usage = || anyhow::anyhow!(USAGE);
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(usage);
        let slot = match flag.as_str() {
            "--dir" => &mut a.dir,
            "--efi" => &mut a.efi,
            "--gpt" => &mut a.gpt,
            "--image" => &mut a.image,
            "--quote" => &mut a.quote,
            "--log" => &mut a.log,
            "--out" => &mut a.out,
            "--name" => {
                a.name = Some(value()?);
                continue;
            }
            "--seq" => {
                let v = value()?;
                a.seq = Some(v.parse().map_err(|_| anyhow::anyhow!("--seq {v:?}: a whole number"))?);
                continue;
            }
            "--mrtd" => {
                a.mrtd = Some(value()?);
                continue;
            }
            "--rtmr0" => {
                a.rtmr0 = Some(value()?);
                continue;
            }
            _ => return Err(usage()),
        };
        anyhow::ensure!(slot.is_none(), "{flag} is given twice");
        *slot = Some(PathBuf::from(value()?));
    }
    let sources = [a.dir.is_some(), a.efi.is_some() || a.gpt.is_some(), a.image.is_some()].iter().filter(|x| **x).count();
    anyhow::ensure!(sources == 1, "name the image once: --dir, --efi with --gpt, or --image\n{USAGE}");
    anyhow::ensure!(a.efi.is_some() == a.gpt.is_some(), "--efi and --gpt go together");
    anyhow::ensure!(a.name.is_some() && a.seq.is_some(), "a release has a --name and a --seq\n{USAGE}");
    anyhow::ensure!(a.log.is_none() || a.quote.is_some(), "--log lines the prediction up against a boot: give its --quote too");
    Ok(a)
}

fn read(path: &Path, max: u64) -> Result<Vec<u8>> {
    let len = std::fs::metadata(path).with_context(|| format!("read {}", path.display()))?.len();
    anyhow::ensure!(len <= max, "{} is {len} bytes, over {max}", path.display());
    std::fs::read(path).with_context(|| format!("read {}", path.display()))
}

/// The UKI and the disk's first 34 sectors.
fn image_bytes(a: &Args) -> Result<(Vec<u8>, Vec<u8>)> {
    let (uki, head) = if let Some(img) = &a.image {
        let d = disk::read(img).with_context(|| format!("read the ESP of {}", img.display()))?;
        (d.efi, d.head)
    } else {
        let (efi, gpt) = match &a.dir {
            Some(d) => (d.join("pod.efi"), d.join("gpt.bin")),
            None => (a.efi.clone().expect("checked"), a.gpt.clone().expect("checked")),
        };
        (read(&efi, MAX_UKI)?, read(&gpt, MAX_SMALL)?)
    };
    anyhow::ensure!(head.len() >= GPT_LEN, "the partition table is {} bytes, not the {GPT_LEN} of LBA 0 to 33", head.len());
    Ok((uki, head))
}

/// What a boot showed, for the registers the image cannot say.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Observed {
    pub mrtd: Option<Mr>,
    pub rtmr0: Option<Mr>,
    /// The quote's RTMR1 to RTMR3, to check against the prediction.
    pub quote: Option<[Mr; 3]>,
}

fn register(flag: &str, hex_text: &str) -> Result<Mr> {
    let mut out = [0u8; 48];
    hex::decode_to_slice(hex_text.trim(), &mut out).map_err(|_| anyhow::anyhow!("{flag}: 96 hex digits"))?;
    Ok(out)
}

/// One observed register: as given, or the quote's; both must agree.
fn one(flag: &str, given: Option<Mr>, quoted: Option<Mr>) -> Result<Option<Mr>> {
    match (given, quoted) {
        (Some(g), Some(q)) if g != q => bail!("{flag} is {} but the quote's is {}", hex::encode(g), hex::encode(q)),
        (g, q) => Ok(g.or(q)),
    }
}

/// The release: RTMR1 and RTMR2 as predicted, RTMR3 zero, MRTD and RTMR0
/// as observed. A quote whose RTMR1 to RTMR3 are not these is refused:
/// that boot was not of this image (or ran with a command line passed in,
/// which stub 257 measures into RTMR2).
pub fn release(seq: u64, name: &str, rtmr1: Mr, rtmr2: Mr, seen: &Observed) -> Result<ReleaseWire> {
    if let Some([q1, q2, q3]) = seen.quote {
        for (reg, want, got) in [("RTMR1", rtmr1, q1), ("RTMR2", rtmr2, q2), ("RTMR3", [0; 48], q3)] {
            anyhow::ensure!(want == got, "the quote's {reg} is {}, not the image's {}: that boot is not this release", hex::encode(got), hex::encode(want));
        }
    }
    let w = ReleaseWire {
        seq,
        name: name.to_string(),
        mrtd: seen.mrtd.map(hex::encode),
        rtmr0: seen.rtmr0.map(hex::encode),
        rtmr1: Some(hex::encode(rtmr1)),
        rtmr2: Some(hex::encode(rtmr2)),
        rtmr3: Some(hex::encode([0u8; 48])),
    };
    // the shape a policy reads, checked as it will be read
    Release::from_wire(w.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(w)
}

pub fn run(args: &[String]) -> Result<()> {
    let a = parse(args)?;
    let (uki, head) = image_bytes(&a)?;
    let quote = a.quote.as_deref().map(|p| read(p, MAX_SMALL)).transpose()?;
    let log = a.log.as_deref().map(|p| read(p, MAX_SMALL)).transpose()?;
    let regs = measure::build(&measure::Inputs { uki: &uki, disk_head: &head, log: log.as_deref(), quote: quote.as_deref() })?;
    for r in &regs {
        eprintln!("RTMR{} {}", r.rtmr, r.expected);
        for e in &r.events {
            let logged = match e.in_log {
                Some(true) => " (in the log)",
                Some(false) => " (NOT in the log at this place)",
                None => "",
            };
            eprintln!("  {} [{}] {}{logged}", e.digest, e.origin, e.what);
        }
        for u in &r.unpredicted {
            eprintln!("  the log also has: {u}");
        }
    }
    let predicted = |n: usize| -> Result<Mr> { register("prediction", &regs.iter().find(|r| r.rtmr == n).context("a register")?.expected) };
    let report = quote.as_deref().map(report::parse).transpose().context("read the quote")?;
    let seen = Observed {
        mrtd: one("--mrtd", a.mrtd.as_deref().map(|h| register("--mrtd", h)).transpose()?, report.as_ref().map(|r| r.mr_td))?,
        rtmr0: one("--rtmr0", a.rtmr0.as_deref().map(|h| register("--rtmr0", h)).transpose()?, report.as_ref().map(|r| r.rtmr[0]))?,
        quote: report.as_ref().map(|r| [r.rtmr[1], r.rtmr[2], r.rtmr[3]]),
    };
    if seen.mrtd.is_none() || seen.rtmr0.is_none() {
        eprintln!("MRTD and RTMR0 are not computable from the image: the release leaves out what no boot showed (a release compares only what it names)");
    }
    let w = release(a.seq.expect("checked"), a.name.as_deref().expect("checked"), predicted(1)?, predicted(2)?, &seen)?;
    let json = format!("{}\n", serde_json::to_string_pretty(&w)?);
    match &a.out {
        Some(p) => {
            std::fs::write(p, &json).with_context(|| format!("write {}", p.display()))?;
            eprintln!("wrote the release to {}", p.display());
        }
        None => print!("{json}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Args> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn takes_one_image_and_a_release_name() {
        let a = args(&["--dir", "out/locked", "--name", "pod-p", "--seq", "1"]).unwrap();
        assert_eq!((a.dir.as_deref(), a.seq), (Some(Path::new("out/locked")), Some(1)));
        assert!(args(&["--efi", "a", "--gpt", "b", "--name", "n", "--seq", "2", "--quote", "q", "--log", "l"]).is_ok());
        for bad in [
            &["--name", "n", "--seq", "1"][..],
            &["--dir", "d", "--image", "i", "--name", "n", "--seq", "1"],
            &["--efi", "a", "--name", "n", "--seq", "1"],
            &["--dir", "d", "--seq", "1"],
            &["--dir", "d", "--name", "n", "--seq", "one"],
            &["--dir", "d", "--name", "n", "--seq", "1", "--log", "l"],
            &["--dir", "d", "--dir", "e", "--name", "n", "--seq", "1"],
            &["--dir", "d", "--name", "n", "--seq", "1", "--rtmr2", "00"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }

    /// Valid: the release names RTMR1 to RTMR3 and what was observed.
    /// Invalid: a quote of another boot, an observed value that disagrees
    /// with the quote, a short register.
    #[test]
    fn a_release_pins_what_the_image_and_the_boot_say() {
        let (r1, r2) = ([1u8; 48], [2u8; 48]);
        let none = release(1, "pod-p", r1, r2, &Observed::default()).unwrap();
        assert_eq!((none.mrtd, none.rtmr0), (None, None));
        assert_eq!(none.rtmr3.as_deref(), Some("00".repeat(48).as_str()));
        let seen = Observed { mrtd: Some([9; 48]), rtmr0: Some([8; 48]), quote: Some([r1, r2, [0; 48]]) };
        let w = release(3, "pod-p", r1, r2, &seen).unwrap();
        assert_eq!((w.seq, w.mrtd.as_deref()), (3, Some("09".repeat(48).as_str())));
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.starts_with(r#"{"seq":3,"name":"pod-p","mrtd":"#), "policy.json's order: {json}");
        let other = Observed { quote: Some([r1, [7; 48], [0; 48]]), ..seen.clone() };
        assert!(release(3, "pod-p", r1, r2, &other).unwrap_err().to_string().contains("RTMR2"));
        let rtmr3 = Observed { quote: Some([r1, r2, [1; 48]]), ..seen };
        assert!(release(3, "pod-p", r1, r2, &rtmr3).unwrap_err().to_string().contains("RTMR3"));
        assert!(one("--mrtd", Some([1; 48]), Some([2; 48])).is_err());
        assert_eq!(one("--mrtd", None, Some([2; 48])).unwrap(), Some([2; 48]));
        assert!(register("--mrtd", "00").is_err());
        assert!(release(1, "", r1, r2, &Observed::default()).is_err(), "a release has a name");
    }

    /// The real images: the proof of concept's built outputs, P and
    /// P-debug, against the registers their boots showed (each folder's
    /// policy-release.json, from check-pod.sh). Ignored unless
    /// POD_IMAGE_OUT names the folder holding them (each image is 56 MB,
    /// so they are not in the repo). Replay: measuring twice gives the
    /// same release.
    #[test]
    #[ignore]
    fn reproduces_the_proof_of_concepts_registers() {
        let out = PathBuf::from(std::env::var("POD_IMAGE_OUT").expect("POD_IMAGE_OUT names the built images' folder"));
        for v in ["P", "P-debug"] {
            let dir = out.join(v);
            let recorded: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("policy-release.json")).unwrap()).unwrap();
            let a = Args { dir: Some(dir.clone()), name: Some("x".into()), seq: Some(1), ..Args::default() };
            let (uki, head) = image_bytes(&a).unwrap();
            let regs = measure::build(&measure::Inputs { uki: &uki, disk_head: &head, log: None, quote: None }).unwrap();
            for (n, key) in [(1, "rtmr1"), (2, "rtmr2")] {
                let got = &regs.iter().find(|r| r.rtmr == n).unwrap().expected;
                assert_eq!(got, recorded[key].as_str().unwrap(), "{v} {key}");
            }
            let again = measure::build(&measure::Inputs { uki: &uki, disk_head: &head, log: None, quote: None }).unwrap();
            assert_eq!(regs.iter().map(|r| &r.expected).collect::<Vec<_>>(), again.iter().map(|r| &r.expected).collect::<Vec<_>>());
        }
    }
}
