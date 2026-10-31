//! The builds `build`, `dev`, `deploy` and `e2e` share: the cell for
//! wasm32 (worker-build), and the computer images `wrangler dev` builds at
//! boot, built ahead beside the Rust so its build finds every layer cached.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;

use crate::{run, shown};

pub const WORKER_BUILD_VERSION: &str = "0.8.5";

/// The most the cell's Wasm may be, raw: every isolate copies its data
/// into its memory, and every deploy uploads it. The files it serves are
/// the release's Static Assets, not its data (cell/src/assets.rs); what
/// grows it past this is a choice, made by moving this number: 6.5 MB
/// since launch week (2026-10-14, the debt ledger's "The cell's Wasm
/// budget was raised for launch").
pub const CELL_WASM_MAX_BYTES: u64 = 6_500_000;

/// Which build of the cell: the release's, which deploys upload and the
/// e2e stages, or `cargo xtask dev`'s, which only `wrangler dev` under it
/// runs (`cargo xtask build --dev`: cell/wrangler.jsonc's `build`, and
/// `dev`'s first build). The two compile the same source with the same
/// flags into the same `build/`, and write the same release's files
/// beside it, which the cell reads by the same index through the same
/// code; dev's compiles without fat LTO, into many codegen units,
/// incrementally, and skips wasm-opt (cell/Cargo.toml, `[profile.devstack]`),
/// so a change to a file of the release, which moves only the index the
/// cell carries, rebuilds in seconds, not minutes. Its Wasm is about 7.7 MB
/// to the release's 5.5, and no budget holds it. Each build marks what it built
/// (devstack's built.rs), and a deploy and the e2e take the release's alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Release,
    Dev,
}

/// `Profile::Dev`'s cargo profile, in cell/Cargo.toml.
const DEV_PROFILE: &str = "devstack";

impl Profile {
    /// Its cargo profile (cell/Cargo.toml), as its build marks it.
    fn name(self) -> &'static str {
        match self {
            Profile::Release => devstack::built::RELEASE,
            Profile::Dev => DEV_PROFILE,
        }
    }

    fn worker_build_args(self) -> &'static [&'static str] {
        match self {
            Profile::Release => &["--release"],
            Profile::Dev => &["--profile", DEV_PROFILE, "--no-opt"],
        }
    }
}

/// The release's cell (`Profile::Release`).
pub fn cell() -> Result<()> {
    cell_as(Profile::Release)
}

/// cell/ for wasm32, into its `build/`, and the release's files beside it
/// in `build/assets`, the Worker's Static Assets (cell/wrangler.jsonc): what
/// a deploy uploads, `wrangler dev` serves and the e2e's staged copy holds.
pub fn cell_as(profile: Profile) -> Result<()> {
    worker_build_installed()?;
    let t0 = Instant::now();
    let dir = devstack::cell_dir();
    let out = dir.join("build");
    // what `build/` holds is no finished build until this one is, and this
    // build alone writes it, and no reader reads it, until its mark
    let building = devstack::built::building(&out, profile.name())?;
    let local = LocalPaths::here(&dir)?;
    let given = (std::env::var("CARGO_ENCODED_RUSTFLAGS").ok(), std::env::var("RUSTFLAGS").ok());
    let flags = encoded_rustflags(given, &local.remapped());
    run(Command::new("worker-build").args(profile.worker_build_args()).env("CARGO_ENCODED_RUSTFLAGS", flags).current_dir(&dir))?;
    let wasm = out.join(devstack::built::WASM);
    let built = std::fs::read(&wasm).with_context(|| format!("{} after worker-build", wasm.display()))?;
    let stripped = without_names(&built).with_context(|| format!("{} after worker-build", wasm.display()))?;
    std::fs::write(&wasm, &stripped).with_context(|| format!("write {}", wasm.display()))?;
    local.named_in(&stripped)?;
    let size = match profile {
        Profile::Release => within_budget(stripped.len() as u64)?,
        Profile::Dev => format!("{} bytes of Wasm ({:.2} MB), dev's: no budget holds it (the release's is {CELL_WASM_MAX_BYTES} bytes)", stripped.len(), stripped.len() as f64 / 1e6),
    };
    let (files, bytes) = fragment_templates::write_assets(&out.join("assets")).context("writing the release's files into cell/build/assets")?;
    building.finish()?;
    println!(
        "built the cell ({}) for wasm32 in {:.1?}: {size}, and {files} files of the release, {:.2} MB, in its Static Assets",
        profile.name(),
        t0.elapsed(),
        bytes as f64 / 1e6
    );
    Ok(())
}

/// This machine's paths a build would write into the cell's Wasm (a
/// panic's location names its file): the repo, cargo's home (the
/// registry's sources and git checkouts), and the toolchain's sysroot.
struct LocalPaths {
    repo: String,
    cargo_home: String,
    sysroot: String,
}

impl LocalPaths {
    /// The paths the cell's build in `cell` reads from: its toolchain is
    /// the one `rustc` names there (`RUSTC`, else rustup's choice).
    fn here(cell: &Path) -> Result<LocalPaths> {
        let utf8 = |p: PathBuf, what: &str| p.into_os_string().into_string().map_err(|p| anyhow::anyhow!("{what} {p:?} is not UTF-8: the cell's build cannot remap it"));
        let cargo_home = match std::env::var_os("CARGO_HOME") {
            Some(home) => PathBuf::from(home),
            None => PathBuf::from(std::env::var_os("HOME").context("neither CARGO_HOME nor HOME is set")?).join(".cargo"),
        };
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let out = Command::new(&rustc).args(["--print", "sysroot"]).current_dir(cell).output().context("rustc --print sysroot")?;
        anyhow::ensure!(out.status.success(), "rustc --print sysroot: {}", String::from_utf8_lossy(&out.stderr));
        let sysroot = String::from_utf8(out.stdout).context("rustc's sysroot is not UTF-8")?.trim().to_string();
        let repo = cell.parent().context("cell/ is in the repo")?.to_path_buf();
        Ok(LocalPaths { repo: utf8(repo, "the repo")?, cargo_home: utf8(cargo_home, "CARGO_HOME")?, sysroot })
    }

    /// The flags that keep these paths out of the Wasm: a panic's location
    /// reads relative to the repo (`crates/core/src/…`; the cell's own,
    /// which cargo names relative to cell/, `src/…`, as `cell/src/…`), a
    /// dependency's under `/cargo` (`/cargo/registry/src/…`, `/cargo/git/checkouts/…`)
    /// and the toolchain's under `/rustc/sysroot`, so the Wasm's size, and
    /// its headroom under `CELL_WASM_MAX_BYTES`, are the same wherever the
    /// repo is checked out, on any machine. Only what the object holds is
    /// remapped (`--remap-path-scope=object`): the compiler's messages keep
    /// the paths an editor opens. rustc takes the last prefix that matches,
    /// so the repo's comes first, and a cargo home inside it is still
    /// cargo's. What still moves with the checkout is a few bytes: cargo
    /// hashes a path dependency outside cell/'s workspace (crates/*) by its
    /// absolute path, which reorders a few of the Wasm's imports.
    fn remapped(&self) -> Vec<String> {
        vec![
            format!("--remap-path-prefix={}=", self.repo),
            "--remap-path-prefix=src=cell/src".into(),
            format!("--remap-path-prefix={}=/cargo", self.cargo_home),
            format!("--remap-path-prefix={}=/rustc/sysroot", self.sysroot),
            "--remap-path-scope=object".into(),
        ]
    }

    /// Refuses a Wasm that names a source under any of these paths: one the
    /// flags missed (a new kind of dependency, a crate that bakes in its own
    /// directory) fails the build here, before its size moves with the
    /// checkout's. A path that already reads as its remap (a cargo home at
    /// `/cargo`, a Docker image's) is what the flags write, not a leak.
    fn named_in(&self, wasm: &[u8]) -> Result<()> {
        let (repo, home, sysroot) = (&self.repo, &self.cargo_home, &self.sysroot);
        let mut paths = vec![format!("{repo}/crates/"), format!("{repo}/cell/")];
        if home != "/cargo" {
            paths.extend([format!("{home}/registry/"), format!("{home}/git/")]);
        }
        if sysroot != "/rustc/sysroot" {
            paths.push(format!("{sysroot}/lib/"));
        }
        for path in paths {
            if wasm.windows(path.len()).any(|w| w == path.as_bytes()) {
                bail!("cell/build/index_bg.wasm names {path}…: its build remaps that path (xtask/src/build.rs, LocalPaths::remapped), and something wrote it in unmapped");
            }
        }
        Ok(())
    }
}

/// `CARGO_ENCODED_RUSTFLAGS` for the cell's build: the flags the
/// environment gives (`given`: its `CARGO_ENCODED_RUSTFLAGS`, else its
/// `RUSTFLAGS`, as cargo reads them), then `ours`. Set, it is all cargo
/// reads: no config file's `rustflags` (the repo has none).
fn encoded_rustflags(given: (Option<String>, Option<String>), ours: &[String]) -> String {
    let given: Vec<String> = match given {
        (Some(encoded), _) => encoded.split('\x1f').filter(|f| !f.is_empty()).map(String::from).collect(),
        (None, Some(plain)) => plain.split_whitespace().map(String::from).collect(),
        (None, None) => vec![],
    };
    assert!(ours.iter().all(|f| !f.contains('\x1f')), "a flag holds no separator");
    given.into_iter().chain(ours.iter().cloned()).collect::<Vec<_>>().join("\x1f")
}

/// A Wasm module's custom section of function names (the Wasm spec's
/// `name` section).
const NAMES: &[u8] = b"name";

/// `wasm` less its `name` section: the function names worker-build keeps
/// (its wasm-opt runs with `--debuginfo`, which no setting turns off), 15%
/// of the cell's Wasm, about 970 KB counted against `CELL_WASM_MAX_BYTES`
/// and copied into every isolate. They name a stack's frames and nothing
/// else; without them a frame reads `wasm-function[<index>]`. A panic's
/// message, its file, line and column, is the Wasm's data, and stays. Every
/// other section is copied as it was (no section refers to another's offset).
fn without_names(wasm: &[u8]) -> Result<Vec<u8>> {
    const HEADER: &[u8] = b"\0asm\x01\0\0\0";
    anyhow::ensure!(wasm.starts_with(HEADER), "not a Wasm module (version 1)");
    let mut out = Vec::with_capacity(wasm.len());
    out.extend_from_slice(HEADER);
    let mut at = HEADER.len();
    // bounded: each turn moves past one section, and a section is at least two bytes
    while at < wasm.len() {
        let start = at;
        let id = wasm[at];
        at += 1;
        let size = leb_u32(wasm, &mut at)? as usize;
        let end = at.checked_add(size).filter(|&end| end <= wasm.len()).context("a section runs past the module's end")?;
        let names = id == 0 && {
            let mut name_at = at;
            let len = leb_u32(wasm, &mut name_at)? as usize;
            wasm.get(name_at..name_at + len) == Some(NAMES)
        };
        if !names {
            out.extend_from_slice(&wasm[start..end]);
        }
        at = end;
    }
    Ok(out)
}

/// A LEB128 u32 at `*at`, which it moves past it.
fn leb_u32(wasm: &[u8], at: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *wasm.get(*at).context("a number runs past the module's end")?;
        *at += 1;
        value |= u32::from(byte & 0x7f).checked_shl(shift).context("a number over 32 bits")?;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a number over five bytes")
}

/// The checked size and exact headroom within `CELL_WASM_MAX_BYTES`, or
/// the exact overage and why the build is refused.
fn within_budget(size: u64) -> Result<String> {
    if size > CELL_WASM_MAX_BYTES {
        let over = size - CELL_WASM_MAX_BYTES;
        bail!(
            "cell/build/index_bg.wasm is {size} bytes, over its budget of {CELL_WASM_MAX_BYTES} bytes (excess bytes: {over}; xtask/src/build.rs, CELL_WASM_MAX_BYTES): every isolate copies the Wasm's data into its memory. A file the cell serves belongs in its Static Assets (crates/templates, cell/src/assets.rs), not in an include_bytes!; anything else that grows it this far is a choice to make, and this number with it"
        );
    }
    let remaining = CELL_WASM_MAX_BYTES - size;
    Ok(format!("{size} bytes of Wasm ({:.2} MB; budget {CELL_WASM_MAX_BYTES} bytes; remaining bytes: {remaining})", size as f64 / 1e6))
}

fn worker_build_installed() -> Result<()> {
    let out = Command::new("worker-build")
        .arg("--version")
        .output()
        .context("worker-build is not installed: cargo install worker-build --version 0.8.5 --locked")?;
    let version = String::from_utf8_lossy(&out.stdout);
    if !version.contains(WORKER_BUILD_VERSION) {
        bail!("worker-build {WORKER_BUILD_VERSION} is required, found {}", version.trim());
    }
    Ok(())
}

/// The computer images the cell's config names (`containers[].images`),
/// each built as `wrangler dev` builds it at boot (wrangler's
/// `constructBuildCommand`: `docker build --load --platform linux/amd64
/// --provenance=false [--build-arg …] -f - <context>`, the Dockerfile on
/// stdin), so the boot's build finds every layer cached. A cold build of
/// the stub compiles its bridge in Docker: minutes that pass beside the
/// Rust and Wasm builds instead of after them. CI builds the stub just
/// before, from its layers in the Actions cache unless the bridge changed
/// (ci.yml, e2e-shard), so there every step here is cached.
pub fn images() -> Result<()> {
    let cell = devstack::cell_dir();
    let mut config = devstack::read_config(&cell)?;
    devstack::absolute_images(&mut config, &cell)?;
    let containers = config["containers"].as_array().cloned().unwrap_or_default();
    // bounded: the images one config names
    for container in &containers {
        for (name, image) in container["images"].as_object().into_iter().flatten() {
            let t0 = Instant::now();
            image_build(image, None).with_context(|| format!("building the {name} image ahead of the node"))?;
            println!("image {name} built ahead of the node in {:.1?}", t0.elapsed());
        }
    }
    Ok(())
}

/// One image, as wrangler builds it. Ahead of the node it is untagged and
/// its output shown only when it fails; a push's (`tag`: image.rs) is
/// tagged, and its output shown as it runs.
pub fn image_build(image: &serde_json::Value, tag: Option<&str>) -> Result<()> {
    let dockerfile = image["dockerfile"].as_str().context("an image names its dockerfile")?;
    let context = image["build_context"].as_str().context("an image names its build context")?;
    let text = std::fs::read(dockerfile).with_context(|| format!("read {dockerfile}"))?;
    let mut cmd = Command::new("docker");
    cmd.args(["build", "--load", "--platform", "linux/amd64", "--provenance=false"]);
    if let Some(tag) = tag {
        cmd.args(["-t", tag]);
    }
    for (k, v) in image["build_vars"].as_object().into_iter().flatten() {
        let v = v.as_str().with_context(|| format!("build var {k} is a string"))?;
        cmd.arg("--build-arg").arg(format!("{k}={v}"));
    }
    cmd.args(["-f", "-", context]).stdin(Stdio::piped());
    if tag.is_none() {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let shown = shown(&cmd);
    let mut child = cmd.spawn().with_context(|| format!("could not start {shown} (is Docker running?)"))?;
    child.stdin.take().context("docker's stdin")?.write_all(&text)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("{shown} failed: {}\n{}{}", out.status, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The budget is inclusive, and a one-byte difference stays visible
    /// even when the rounded MB is identical. Extreme sizes never wrap.
    #[test]
    fn the_cell_is_held_to_its_budget() {
        assert_eq!(super::CELL_WASM_MAX_BYTES, 6_500_000);
        for (size, expected) in [
            (0, "0 bytes of Wasm (0.00 MB; budget 6500000 bytes; remaining bytes: 6500000)"),
            (6_499_999, "6499999 bytes of Wasm (6.50 MB; budget 6500000 bytes; remaining bytes: 1)"),
            (6_500_000, "6500000 bytes of Wasm (6.50 MB; budget 6500000 bytes; remaining bytes: 0)"),
        ] {
            assert_eq!(super::within_budget(size).unwrap(), expected);
        }
        let refused = super::within_budget(super::CELL_WASM_MAX_BYTES + 1).unwrap_err().to_string();
        assert!(refused.starts_with("cell/build/index_bg.wasm is 6500001 bytes, over its budget of 6500000 bytes (excess bytes: 1;"), "{refused}");
        assert!(refused.contains("CELL_WASM_MAX_BYTES") && refused.contains("Static Assets"), "{refused}");
        let refused = super::within_budget(u64::MAX).unwrap_err().to_string();
        assert!(refused.starts_with("cell/build/index_bg.wasm is 18446744073709551615 bytes, over its budget of 6500000 bytes (excess bytes: 18446744073703051615;"), "{refused}");
    }

    /// Goal: the cell's Wasm holds no path of the machine that built it.
    /// Method: the flags for a checkout's paths, in rustc's order (the last
    /// match wins), after the flags the environment gives, as cargo reads
    /// them; and a Wasm naming a path refused, one without allowed.
    #[test]
    fn the_cell_names_no_local_path() {
        let local = super::LocalPaths { repo: "/w t/fragment".into(), cargo_home: "/home/me/.cargo".into(), sysroot: "/home/me/.rustup/toolchains/stable".into() };
        let ours = local.remapped();
        assert_eq!(
            ours,
            [
                "--remap-path-prefix=/w t/fragment=",
                "--remap-path-prefix=src=cell/src",
                "--remap-path-prefix=/home/me/.cargo=/cargo",
                "--remap-path-prefix=/home/me/.rustup/toolchains/stable=/rustc/sysroot",
                "--remap-path-scope=object"
            ]
        );
        let encoded = |given| super::encoded_rustflags(given, &ours[..1]);
        assert_eq!(encoded((None, None)), "--remap-path-prefix=/w t/fragment=", "a path with a space stays one flag");
        assert_eq!(encoded((None, Some(" -C  target-cpu=mvp ".into()))), "-C\x1ftarget-cpu=mvp\x1f--remap-path-prefix=/w t/fragment=");
        assert_eq!(encoded((Some("-Cdebuginfo=0".into()), Some("ignored".into()))), "-Cdebuginfo=0\x1f--remap-path-prefix=/w t/fragment=", "cargo reads the encoded flags first");
        assert_eq!(encoded((Some(String::new()), None)), "--remap-path-prefix=/w t/fragment=");
        assert!(local.named_in(b"\0asm panicked at crates/core/src/oidc.rs:7:5 /cargo/registry/src/x /w t/fragment").is_ok());
        for named in [&b"at /w t/fragment/crates/core/src/oidc.rs"[..], b"/w t/fragment/cell/src/lib.rs", b"/home/me/.cargo/registry/src/x", b"/home/me/.cargo/git/checkouts/x", b"/home/me/.rustup/toolchains/stable/lib/rustlib"] {
            let refused = local.named_in(named).unwrap_err().to_string();
            assert!(refused.contains("LocalPaths::remapped"), "{refused}");
        }
        // a cargo home at /cargo already reads as its remap: allowed, while
        // the repo's paths are still refused
        let docker = super::LocalPaths { cargo_home: "/cargo".into(), ..local };
        assert!(docker.named_in(b"/cargo/registry/src/x /cargo/git/checkouts/x").is_ok());
        assert!(docker.named_in(b"/w t/fragment/crates/core/src/oidc.rs").is_err());
    }

    /// Goal: the cell's Wasm drops its function names and keeps everything
    /// else, byte for byte. Method: a module of a type section, a custom
    /// section that is not `name`, the `name` section (its size over one
    /// LEB128 byte), and the code section; then modules that are not Wasm,
    /// or end inside a section, refused.
    #[test]
    fn the_cell_keeps_no_function_names() {
        let section = |id: u8, payload: &[u8]| -> Vec<u8> {
            let mut s = vec![id];
            let mut n = payload.len();
            loop {
                let byte = (n & 0x7f) as u8;
                n >>= 7;
                s.push(if n == 0 { byte } else { byte | 0x80 });
                if n == 0 {
                    break s.into_iter().chain(payload.iter().copied()).collect();
                }
            }
        };
        let custom = |name: &[u8], body: &[u8]| section(0, &[&[name.len() as u8][..], name, body].concat());
        let header = b"\0asm\x01\0\0\0".to_vec();
        let types = section(1, &[1, 0x60, 0, 0]);
        let producers = custom(b"producers", b"\x00");
        let names = custom(b"name", &[7u8; 300]);
        let code = section(10, &[1, 2, 0, 0x0b]);
        let module = [&header[..], &types, &producers, &names, &code].concat();
        assert_eq!(super::without_names(&module).unwrap(), [&header[..], &types, &producers, &code].concat());
        let kept = [&header[..], &types, &custom(b"names", b"x"), &code].concat();
        assert_eq!(super::without_names(&kept).unwrap(), kept, "a custom section named otherwise stays");
        assert!(super::without_names(b"\0asm\x02\0\0\0").is_err());
        assert!(super::without_names(&module[..module.len() - 1]).is_err(), "a module cut short");
        assert!(super::without_names(&[&header[..], &[1, 0xff, 0xff, 0xff, 0xff, 0x7f]].concat()).is_err(), "a size over 32 bits");
    }

    /// Goal: the Hermes image's inputs are what it runs (docs/computers.md,
    /// "Preview and production": what changes a version), so a change to the
    /// cell, the shell, a blessed template, the e2e, xtask or the docs
    /// builds the same image. Method: its CLI stage
    /// (images/hermes/Dockerfile), and the stub's, the same, against
    /// cargo's own account of the workspace. It copies, whole, the CLI and each workspace crate it
    /// builds from (its normal and build dependencies by path, and theirs);
    /// each other member's manifest alone, and stubs exactly those; the
    /// root manifest and lock; and each template but the platform's own,
    /// which the CLI builds without. Invalid: a new dependency of the CLI's,
    /// a new member or a new template fails here, naming what the stage
    /// lacks, before it fails the image's build or silently leaves it out.
    #[test]
    fn the_cli_stage_copies_what_the_cli_builds_from() {
        use std::collections::{BTreeMap, BTreeSet};
        use std::path::{Path, PathBuf};

        let root = super::devstack::repo_root().canonicalize().expect("the repo");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let out = super::Command::new(cargo).args(["metadata", "--format-version", "1", "--no-deps", "--offline"]).current_dir(&root).output().expect("cargo metadata");
        assert!(out.status.success(), "cargo metadata: {}", String::from_utf8_lossy(&out.stderr));
        let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("its JSON");
        let rel = |p: &str| -> String { Path::new(p).strip_prefix(&root).unwrap_or_else(|_| panic!("{p} is in the repo")).to_string_lossy().into_owned() };
        // each member's directory, by name, and the members it builds from
        let mut dirs = BTreeMap::new();
        let mut needs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for p in meta["packages"].as_array().expect("its packages") {
            let name = p["name"].as_str().expect("a name").to_string();
            let manifest = PathBuf::from(p["manifest_path"].as_str().expect("its manifest"));
            dirs.insert(name.clone(), rel(manifest.parent().expect("its directory").to_str().expect("a path")));
            let deps = p["dependencies"].as_array().into_iter().flatten();
            let built = deps.filter(|d| d["kind"].is_null() || d["kind"] == "build").filter(|d| d["path"].is_string());
            needs.insert(name, built.map(|d| d["name"].as_str().expect("a name").to_string()).collect());
        }
        let mut closure = BTreeSet::from(["fragment-cli".to_string()]);
        let mut next = vec!["fragment-cli".to_string()];
        // bounded: each member enters once
        while let Some(n) = next.pop() {
            for d in needs.get(&n).into_iter().flatten() {
                if closure.insert(d.clone()) {
                    next.push(d.clone());
                }
            }
        }
        let whole: BTreeSet<String> = closure.iter().map(|n| dirs[n].clone()).collect();
        let stubbed: BTreeSet<String> = dirs.iter().filter(|(n, _)| !closure.contains(*n)).map(|(_, d)| d.clone()).collect();
        assert!(whole.contains("cli") && whole.contains("crates/templates") && stubbed.contains("crates/e2e"), "{whole:?} {stubbed:?}");

        let mut templates: BTreeSet<String> = BTreeSet::new();
        for entry in std::fs::read_dir(root.join("templates")).expect("templates/").flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !fragment_templates::blessed::BLESSED.contains(&name.as_str()) {
                templates.insert(format!("templates/{name}"));
            }
        }
        // both images carry the CLI. Hermes' stage builds it whole, its other
        // members' manifests and empty sources beside it; the stub's builds
        // its dependencies first, from every member's manifest and empty
        // sources (the CLI's a `main`), then the CLI's crates whole
        let every: BTreeSet<String> = whole.union(&stubbed).cloned().collect();
        let emptied: BTreeSet<String> = every.iter().filter(|d| *d != "cli").cloned().collect();
        for (file, manifests, empties) in [("images/hermes/Dockerfile", &stubbed, &stubbed), ("images/stub/Dockerfile", &every, &emptied)] {
            let dockerfile = std::fs::read_to_string(root.join(file)).expect("the Dockerfile");
            let stage: Vec<&str> = dockerfile.lines().skip_while(|l| !l.starts_with("FROM rust:") || !l.ends_with(" AS cli")).skip(1).take_while(|l| !l.starts_with("FROM ")).collect();
            assert!(!stage.is_empty(), "{file} has its CLI stage (`FROM rust:… AS cli`)");
            let copies: Vec<Vec<&str>> = stage.iter().filter_map(|l| l.strip_prefix("COPY ")).map(|l| l.split_whitespace().collect()).collect();
            let lock = ["Cargo.toml", "Cargo.lock", "./"];
            assert!(copies.iter().any(|c| c.as_slice() == lock), "{file}: the root manifest and lock: {copies:?}");
            let paths: Vec<&str> = copies.iter().filter(|c| c.as_slice() != lock).map(|c| {
                assert!(c.len() == 2 && c[0] == c[1] && !c[0].starts_with("--"), "each copy keeps its path: {c:?}");
                c[0]
            }).collect();
            let copied_whole: BTreeSet<String> = paths.iter().filter(|p| !p.ends_with("/Cargo.toml") && !p.starts_with("templates/")).map(|p| p.to_string()).collect();
            let copied_manifests: BTreeSet<String> = paths.iter().filter_map(|p| p.strip_suffix("/Cargo.toml")).map(str::to_string).collect();
            assert_eq!(copied_whole, whole, "{file}'s stage copies the CLI and each crate it builds from, whole");
            assert_eq!(&copied_manifests, manifests, "{file}: and the members' manifests its stage copies alone");
            let stubs = stage.iter().find_map(|l| l.trim().strip_prefix("for m in ").and_then(|l| l.split_once(';'))).map(|(list, _)| list.split_whitespace().map(str::to_string).collect::<BTreeSet<_>>());
            assert_eq!(stubs.as_ref(), Some(empties), "{file}: and empties exactly those members' sources (`for m in …;`)");

            let copied_templates: BTreeSet<String> = paths.iter().filter(|p| p.starts_with("templates/")).map(|p| p.to_string()).collect();
            assert_eq!(copied_templates, templates, "{file}: and each template the CLI scaffolds: every one but the platform's own");
        }
    }
}
