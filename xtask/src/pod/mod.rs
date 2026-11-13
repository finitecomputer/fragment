//! `cargo xtask pod`: the pod's measured image (pod-image/) and its
//! registers (docs/pod.md, "The image").
//!
//! - `measure`: the release a boot of an image must match (measure.rs).

use anyhow::{bail, Result};

mod measure;

const USAGE: &str = "usage: cargo xtask pod measure (--dir <build output> | --efi <f> --gpt <f> | --image <pod.raw>) --name <n> --seq <n> [--quote <f> [--log <f>]] [--mrtd <hex>] [--rtmr0 <hex>] [--out <file>]";

pub fn pod(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("measure") => measure::run(&args[1..]),
        _ => bail!("{USAGE}"),
    }
}
