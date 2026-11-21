//! An image's share of a computer's disk (docs/computers.md, "An image's
//! share of the disk"): its files written to a fresh computer's disk (btrfs
//! with zstd, the size a computer's `df` reads), as a computer's restore
//! finds it, held to the budget every computer's room counts
//! (`fragment_core::computer::IMAGE_DISK_MAX`). The one measure: a push of
//! an image is refused past it before anything deploys it (image.rs,
//! preview.rs), and images/'s Docker lane holds the Hermes image to it
//! (`the_hermes_image_fits_its_disk_budget`) through `cargo xtask image
//! disk`. Raising the budget is Paul's word, recorded beside it.

use std::process::{Command, Stdio};

use anyhow::{bail, ensure, Context, Result};

/// The image a disk's share is measured with: Alpine, pinned, with
/// btrfs-progs added as it runs.
const HELPER: &str = "alpine:3.20@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc";

/// A computer's disk as its `df` reads it: `DISK_MB` megabytes, in whole
/// 4 KiB blocks (20,000,002,048 bytes, production's).
pub fn disk_bytes() -> u64 {
    (u64::from(fragment_core::price::DISK_MB) * 1_000_000).div_ceil(4096) * 4096
}

/// What runs in the helper, the image's files on its standard input: a
/// fresh computer's disk on a loop device of its own (its node made when
/// the host has none yet), the files written to it, and its `stat -f`. Its
/// loop device is let go however it ends (`trap`, each step whatever the
/// one before did: `set -e` would end it at an `umount` of no mount), so a
/// measure that fails leaks none on the host (Codex on #1422).
fn script(disk: u64) -> String {
    format!(
        "set -e; apk add --no-cache btrfs-progs > /dev/null; truncate -s {disk} /disk.img; mkfs.btrfs -q -f /disk.img > /dev/null; \
         dev=$(losetup -f); [ -e \"$dev\" ] || mknod \"$dev\" b 7 \"${{dev#/dev/loop}}\"; losetup \"$dev\" /disk.img; \
         trap 'umount /m 2> /dev/null || :; losetup -d \"$dev\" 2> /dev/null || :' EXIT; mkdir -p /m; mount -o compress=zstd:3 \"$dev\" /m; \
         mkdir -p /m/image /m/data; tar -x -C /m/image -f -; sync; btrfs filesystem sync /m > /dev/null; stat -f -c '%b %a %S' /m"
    )
}

/// The bytes used, from `stat -f -c '%b %a %S'`: blocks, those available,
/// and their size.
fn used(stat: &str) -> Result<u64> {
    let n: Vec<u64> = stat.split_whitespace().map(str::parse).collect::<Result<_, _>>().with_context(|| format!("stat -f said {stat:?}"))?;
    let [blocks, available, unit] = n[..] else { bail!("stat -f said {stat:?}") };
    blocks.checked_sub(available).and_then(|b| b.checked_mul(unit)).with_context(|| format!("stat -f said {stat:?}"))
}

/// `reference`'s share of a fresh computer's disk, in bytes: Docker's
/// export of a container made of it (pulled if it is not here) written to
/// the disk (`script`). An export or a measure that fails is an error.
pub fn share(reference: &str) -> Result<u64> {
    let said = |o: &std::process::Output| String::from_utf8_lossy(&o.stderr).trim().to_string();
    let created = Command::new("docker").args(["create", "--platform", "linux/amd64", reference]).output().context("could not start docker create (is Docker running?)")?;
    ensure!(created.status.success(), "docker create {reference}: {}", said(&created));
    let id = String::from_utf8_lossy(&created.stdout).trim().to_string();
    let measured = (|| {
        let mut export = Command::new("docker").args(["export", &id]).stdout(Stdio::piped()).spawn().context("could not start docker export")?;
        let tar = export.stdout.take().context("docker export's tar")?;
        let out = Command::new("docker").args(["run", "--rm", "-i", "--privileged", HELPER, "sh", "-c", &script(disk_bytes())]).stdin(tar).output().context("could not start docker run")?;
        let exported = export.wait()?;
        ensure!(exported.success(), "docker export of {reference} failed ({exported}): its share is not measured");
        ensure!(out.status.success(), "the disk's measure of {reference}: {}", said(&out));
        used(&String::from_utf8_lossy(&out.stdout))
    })();
    let _ = Command::new("docker").args(["rm", &id]).output();
    measured
}

/// Holds `reference` to the budget: its share, or an error past it.
pub fn check(reference: &str) -> Result<u64> {
    let (budget, disk) = (fragment_core::computer::IMAGE_DISK_MAX, disk_bytes());
    let share = share(reference)?;
    println!("image disk: {share} bytes of a fresh computer's {disk}-byte disk ({:.2} GB; budget {budget} bytes, IMAGE_DISK_MAX; remaining bytes: {})", share as f64 / 1e9, budget as i64 - share as i64);
    ensure!(share > 0 && share < disk, "a measure of {reference}: {share} bytes");
    ensure!(
        share <= budget,
        "{reference} takes {share} bytes of a fresh computer's disk, over its budget of {budget} bytes (excess bytes: {}; crates/core/src/computer/disk.rs, IMAGE_DISK_MAX): every computer's save must still restore beside it. Shrink the image, or ask Paul to raise the budget, recorded beside it",
        share - budget
    );
    Ok(share)
}

/// `cargo xtask image disk <reference>`.
pub(super) fn run(args: &[String]) -> Result<()> {
    let [reference] = args else { bail!("usage: cargo xtask image disk <reference>: its share of a fresh computer's disk, held to IMAGE_DISK_MAX") };
    check(reference).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: the disk is a computer's, as production's `df` reads it.
    #[test]
    fn the_disk_is_a_computers() {
        assert_eq!(disk_bytes(), 20_000_002_048);
    }

    /// Goal: what `stat -f` says read as bytes used; anything else an error.
    /// Valid, invalid (too few, not numbers, more available than blocks).
    #[test]
    fn a_measure_is_read_from_stat() {
        assert_eq!(used("4882813 4146400 4096\n").unwrap(), (4_882_813 - 4_146_400) * 4096);
        for bad in ["", "1 2", "1 2 3 4", "a b c", "1 2 4096"] {
            assert!(used(bad).is_err(), "{bad:?}");
        }
    }

    /// Goal (Codex on #1422): the measure lets its loop device go however
    /// it ends, and writes the image's files to a disk of a computer's size.
    #[test]
    fn the_measure_lets_its_loop_device_go() {
        let s = script(disk_bytes());
        let trap = s.find("trap '").expect("a trap");
        assert!(trap > s.find("losetup \"$dev\"").unwrap() && trap < s.find("mount -o").unwrap(), "set once the device is attached, before anything can fail");
        assert!(s.contains("umount /m 2> /dev/null || :; losetup -d \"$dev\" 2> /dev/null || :"), "each step of the trap runs, under set -e, whether the mount was made or not (Codex on #1422)");
        assert!(s.contains("truncate -s 20000002048 ") && s.contains("compress=zstd:3"));
        assert!(run(&[]).is_err() && run(&["a".into(), "b".into()]).is_err());
    }
}
