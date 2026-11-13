//! `cargo xtask pod`: the pod's measured image (pod-image/), its
//! registers, and its VMs (docs/pod.md, "The image").
//!
//! - `image build|publish|platform --config <file>`: the image on a build
//!   host (image.rs, platform.rs), and published to Compute Engine;
//!   `image compare <a> <b>`: two builds side by side (compare.rs).
//! - `measure`: the release a boot of an image must match (measure.rs).
//! - `vm create|start|stop|describe|delete --config <file>`: a pod VM
//!   (vm.rs).
//!
//! Every command that reaches a build host or Compute Engine is a plan
//! (plan.rs): `--dry-run` prints it, every vendor command in order, and
//! runs nothing; the ones that spend money or delete things also need
//! their own confirming flag. Deployment settings come from the config
//! (config.rs); versions and hashes from pod-image/'s pins (pins.rs).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;

mod compare;
mod config;
mod host;
mod image;
mod measure;
mod pins;
mod plan;
mod platform;
mod vm;

use config::Config;
use plan::{Plan, Processes, Runner};

const USAGE: &str = "usage: cargo xtask pod image build --config <file> [--variant locked|debug|both] [--dry-run]
       cargo xtask pod image platform <sources|build|trial|images|debs|stage|all> --config <file> [--dry-run]
       cargo xtask pod image publish --config <file> --variant locked|debug [--dry-run | --confirm-spend]
       cargo xtask pod image compare <build output> <build output> [--same]
       cargo xtask pod measure (--dir <build output> | --efi <f> --gpt <f> | --image <pod.raw>) --name <n> --seq <n> [--quote <f> [--log <f>]] [--mrtd <hex>] [--rtmr0 <hex>] [--out <file>]
       cargo xtask pod vm create|start --config <file> [--dry-run | --confirm-spend]
       cargo xtask pod vm stop --config <file> [--dry-run | --confirm-stop]
       cargo xtask pod vm describe --config <file> [--dry-run] [--save <dir>]
       cargo xtask pod vm delete --config <file> [--delete-data-disk <name>] [--dry-run | --confirm-delete]
docs/pod.md, \"The image\", shows the config's shape";

/// The two images one definition makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// No SSH, no login: what an owner's policy accepts.
    Locked,
    /// The locked image plus SSH for one key and sudo, for development
    /// and rescue; it measures differently by construction.
    Debug,
}

impl Variant {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "locked" => Ok(Self::Locked),
            "debug" => Ok(Self::Debug),
            other => bail!("a variant is locked or debug, not {other:?}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Debug => "debug",
        }
    }
}

/// The flags every planned command takes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Flags {
    config: Option<PathBuf>,
    variant: Option<String>,
    dry_run: bool,
    confirmed: Vec<String>,
    save: Option<PathBuf>,
    delete_data_disk: Option<String>,
    /// Words that are not flags: a subcommand's own (a platform step).
    words: Vec<String>,
}

const CONFIRMS: [&str; 3] = ["--confirm-spend", "--confirm-stop", "--confirm-delete"];

fn flags(args: &[String]) -> Result<Flags> {
    let mut f = Flags::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().with_context(|| format!("{a} takes a value\n{USAGE}"));
        match a.as_str() {
            "--config" => f.config = Some(value()?.into()),
            "--variant" => f.variant = Some(value()?),
            "--save" => f.save = Some(value()?.into()),
            "--delete-data-disk" => f.delete_data_disk = Some(value()?),
            "--dry-run" => f.dry_run = true,
            c if CONFIRMS.contains(&c) => f.confirmed.push(c.to_string()),
            w if !w.starts_with('-') => f.words.push(w.to_string()),
            other => bail!("unknown flag {other}\n{USAGE}"),
        }
    }
    anyhow::ensure!(!(f.dry_run && !f.confirmed.is_empty()), "--dry-run runs nothing: it takes no confirming flag");
    Ok(f)
}

impl Flags {
    fn config(&self) -> Result<Config> {
        Config::load(self.config.as_deref().with_context(|| format!("name the pod's config: --config <file>\n{USAGE}"))?)
    }

    /// Only the flags this subcommand takes.
    fn only(&self, allowed: &[&str]) -> Result<()> {
        let given = [
            ("--variant", self.variant.is_some()),
            ("--save", self.save.is_some()),
            ("--delete-data-disk", self.delete_data_disk.is_some()),
        ];
        for (flag, set) in given {
            anyhow::ensure!(!set || allowed.contains(&flag), "this command takes no {flag}");
        }
        for c in &self.confirmed {
            anyhow::ensure!(allowed.contains(&c.as_str()), "this command's confirming flag is not {c}");
        }
        Ok(())
    }
}

/// How a plan is to be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    DryRun,
    Run,
    /// It would spend or delete, and its flag was not given.
    Unconfirmed(&'static str),
}

fn mode(f: &Flags, confirm: Option<&'static str>) -> Mode {
    match (f.dry_run, confirm) {
        (true, _) => Mode::DryRun,
        (false, Some(flag)) if !f.confirmed.iter().any(|c| c == flag) => Mode::Unconfirmed(flag),
        _ => Mode::Run,
    }
}

/// Prints the plan, or runs it with `runner`.
fn take(plan: &Plan, mode: Mode, runner: &mut dyn Runner) -> Result<()> {
    match mode {
        Mode::DryRun => {
            print!("{}", plan.render());
            Ok(())
        }
        Mode::Unconfirmed(flag) => {
            print!("{}", plan.render());
            bail!("ran nothing: this plan spends money or deletes things; pass {flag} to run it (--dry-run only prints it)")
        }
        Mode::Run => plan.run(runner).map(|_| ()),
    }
}

/// The commit `git archive` ships: HEAD, which for a run must be clean.
fn commit(root: &Path, run: bool) -> Result<String> {
    if run {
        let s = Command::new("git").args(["status", "--porcelain", "--untracked-files=normal"]).current_dir(root).output().context("git status")?;
        anyhow::ensure!(s.status.success() && s.stdout.is_empty(), "the tree must be committed and clean: the build ships and builds the commit");
    }
    let out = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).output().context("git rev-parse")?;
    anyhow::ensure!(out.status.success(), "git rev-parse HEAD failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn variants(f: &Flags, both_by_default: bool) -> Result<Vec<Variant>> {
    match f.variant.as_deref() {
        None if both_by_default => Ok(vec![Variant::Locked, Variant::Debug]),
        None => bail!("name the image: --variant locked or --variant debug"),
        Some("both") if both_by_default => Ok(vec![Variant::Locked, Variant::Debug]),
        Some(v) => Ok(vec![Variant::parse(v)?]),
    }
}

fn image_cmd(args: &[String], runner: &mut dyn Runner) -> Result<()> {
    let root = devstack::repo_root();
    let (sub, rest) = args.split_first().with_context(|| USAGE.to_string())?;
    if sub == "compare" {
        return compare::run(rest);
    }
    let f = flags(rest)?;
    match sub.as_str() {
        "build" => {
            f.only(&["--variant"])?;
            anyhow::ensure!(f.words.is_empty(), "{USAGE}");
            let cfg = f.config()?;
            let m = mode(&f, None);
            let vs = variants(&f, true)?;
            if m == Mode::Run && cfg.build()?.host == config::HostKind::Local {
                anyhow::ensure!(cfg!(target_os = "linux"), "mkosi needs Linux: on this machine, build on a VM (build.host \"gce\")");
            }
            let b = image::Builder::new(&cfg, &root, &commit(&root, m == Mode::Run)?)?;
            take(&image::build_plan(&b, &vs)?, m, runner)?;
            if m == Mode::Run {
                for v in vs {
                    let out = image::local_out(&root, v);
                    std::fs::write(out.join("build.json"), image::build_info(&b, v)?).with_context(|| format!("write {}", out.display()))?;
                    println!("{}: {}", v.name(), out.display());
                }
            }
            Ok(())
        }
        "platform" => {
            f.only(&[])?;
            let [step] = f.words.as_slice() else { bail!("name one platform step: {} or all", platform::STEPS.join(", ")) };
            let cfg = f.config()?;
            let b = image::Builder::new(&cfg, &root, &commit(&root, false)?)?;
            take(&platform::plan(&b, step)?, mode(&f, None), runner)
        }
        "publish" => {
            f.only(&["--variant", "--confirm-spend"])?;
            anyhow::ensure!(f.words.is_empty(), "{USAGE}");
            let cfg = f.config()?;
            let [v] = variants(&f, false)?[..] else { unreachable!("one variant") };
            let built = image::Built::read(&image::local_out(&root, v))?;
            let b = image::Builder::new(&cfg, &root, &built.commit)?;
            take(&image::publish_plan(&b, cfg.images()?, v, &built)?, mode(&f, Some("--confirm-spend")), runner)
        }
        _ => bail!("{USAGE}"),
    }
}

fn vm_cmd(args: &[String], runner: &mut dyn Runner) -> Result<()> {
    let (sub, rest) = args.split_first().with_context(|| USAGE.to_string())?;
    let f = flags(rest)?;
    anyhow::ensure!(f.words.is_empty(), "{USAGE}");
    let sub = match sub.as_str() {
        "create" => vm::Sub::Create,
        "start" => vm::Sub::Start,
        "stop" => vm::Sub::Stop,
        "describe" => vm::Sub::Describe { save: f.save.clone() },
        "delete" => vm::Sub::Delete { data_disk: f.delete_data_disk.clone() },
        _ => bail!("{USAGE}"),
    };
    let mut allowed: Vec<&str> = sub.confirm_flag().into_iter().collect();
    match sub {
        vm::Sub::Describe { .. } => allowed.push("--save"),
        vm::Sub::Delete { .. } => allowed.push("--delete-data-disk"),
        _ => {}
    }
    f.only(&allowed)?;
    let cfg = f.config()?;
    let plan = vm::plan(&cfg, &sub)?;
    if let (Some(dir), false) = (&f.save, f.dry_run) {
        std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    }
    take(&plan, mode(&f, sub.confirm_flag()), runner)
}

pub fn pod(args: &[String]) -> Result<()> {
    run(args, &mut Processes)
}

fn run(args: &[String], runner: &mut dyn Runner) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("image") => image_cmd(&args[1..], runner),
        Some("measure") => measure::run(&args[1..]),
        Some("vm") => vm_cmd(&args[1..], runner),
        _ => bail!("{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::plan::stub::Stub;
    use super::*;

    fn config_file(body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("pod-config-{}.jsonc", devstack::random_hex(6)));
        std::fs::write(&p, body).unwrap();
        p
    }

    fn args(a: &[&str], cfg: &Path) -> Vec<String> {
        a.iter().map(|s| s.replace("CFG", &cfg.to_string_lossy())).collect()
    }

    /// Nothing that spends or deletes runs without its own flag: a delete
    /// without --confirm-delete, a create with the stop's flag, a dry run
    /// with a confirming flag are each refused, and the stub saw no
    /// command at all. A dry run asks nothing either.
    #[test]
    fn nothing_runs_without_its_confirming_flag() {
        let cfg = config_file(config::tests::EXAMPLE);
        for a in [
            &["vm", "delete", "--config", "CFG"][..],
            &["vm", "create", "--config", "CFG"],
            &["vm", "create", "--config", "CFG", "--confirm-stop"],
            &["vm", "delete", "--config", "CFG", "--confirm-spend"],
            &["vm", "stop", "--config", "CFG", "--dry-run", "--confirm-stop"],
            &["vm", "delete", "--config", "CFG", "--delete-data-disk", "pod-2-data", "--confirm-delete"],
            &["vm", "describe", "--config", "CFG", "--confirm-spend"],
            &["vm", "create", "--config", "CFG", "--variant", "debug", "--confirm-spend"],
            &["vm", "reboot", "--config", "CFG"],
            &["image", "publish", "--config", "CFG", "--variant", "locked", "--confirm-delete"],
        ] {
            let mut s = Stub::default();
            assert!(run(&args(a, &cfg), &mut s).is_err(), "{a:?}");
            assert!(s.ran.is_empty(), "{a:?} ran {:?}", s.ran);
        }
        for a in [&["vm", "delete", "--config", "CFG", "--dry-run"][..], &["vm", "create", "--config", "CFG", "--dry-run"], &["image", "build", "--config", "CFG", "--dry-run"], &["image", "platform", "all", "--config", "CFG", "--dry-run"]] {
            let mut s = Stub::default();
            run(&args(a, &cfg), &mut s).unwrap();
            assert!(s.ran.is_empty(), "{a:?}");
        }
        // with its flag, it runs (on the stub)
        let mut s = Stub::default().answer("instances list", "pod-1\n", true);
        run(&args(&["vm", "delete", "--config", "CFG", "--confirm-delete"], &cfg), &mut s).unwrap();
        assert!(s.ran.iter().any(|c| c.contains("instances delete pod-1")));
        std::fs::remove_file(cfg).unwrap();
    }

    #[test]
    fn a_config_is_named_and_must_load() {
        let mut s = Stub::default();
        assert!(run(&["vm".into(), "describe".into(), "--dry-run".into()], &mut s).unwrap_err().to_string().contains("--config"));
        let cfg = config_file(&config::tests::EXAMPLE.replace(r#""zone": "us-central1-f", "machine_type""#, r#""machine_type""#));
        let e = run(&args(&["vm", "create", "--config", "CFG", "--dry-run"], &cfg), &mut s).unwrap_err();
        assert!(format!("{e:#}").contains("missing field `zone`"), "{e:#}");
        std::fs::remove_file(cfg).unwrap();
        assert!(flags(&["--config".into()]).is_err());
        assert!(flags(&["--bogus".into()]).is_err());
    }

    /// The config's example in docs/pod.md is one this reads.
    #[test]
    fn the_docs_example_config_reads() {
        let doc = std::fs::read_to_string(devstack::repo_root().join("docs/pod.md")).unwrap();
        let start = doc.find("```jsonc\n").expect("docs/pod.md shows the config") + 9;
        let end = start + doc[start..].find("```").unwrap();
        let c = Config::parse(&doc[start..end]).unwrap();
        assert!(c.build.is_some() && c.images.is_some() && c.vm.is_some());
    }
}
