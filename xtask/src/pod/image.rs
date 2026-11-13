//! `cargo xtask pod image build|publish`: the pod image built from
//! pod-image/ on the build host, its small outputs brought back here, and
//! the disk published as a Compute Engine image.
//!
//! The build ships the committed tree to the host (`git archive`), builds
//! pod-init from it, assembles mkosi's work folder from pod-image/mkosi
//! (and pod-image/debug/mkosi for the debug image), the pinned stub and
//! the staged platform (platform.rs), runs mkosi, and fetches what
//! `pod measure` reads into target/pod-image/<variant>/: the UKI, the
//! partition table, the root hash, the partition list, sizes and hashes.
//! The disk image stays on the host. It replaces the proof of concept's
//! build.sh and box.sh (setup, pod-init, build, info).
//!
//! Publishing writes the disk onto a scratch disk attached to the build
//! VM, reads it back, makes an image from it with the guest features a
//! TDX guest needs, and deletes the scratch disk: no storage bucket. It
//! replaces publish.sh and box.sh's `write`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::config::{region, Build, Config, Images};
use super::host::{gcloud, Append, Host, HostCmd};
use super::pins::{self, Tools};
use super::plan::{Check, Cmd, Otherwise, Plan, Probe, Stdin, Step, Stdout};
use super::Variant;

/// The files a build brings back, each small: what `pod measure` and
/// `pod image compare` read, and what publish checks the disk by.
pub const FETCHED: [&str; 7] = ["pod.efi", "gpt.bin", "pod.roothash", "sha256sums", "partitions.json", "sizes", "pod-init.sha256"];

/// Google's custom-image requirements for a Confidential VM with TDX:
/// UEFI boot, TDX, and the gVNIC driver (C3 has no virtio-net).
pub const GUEST_OS_FEATURES: &str = "UEFI_COMPATIBLE,TDX_CAPABLE,GVNIC";

/// The name each image's `/usr/lib/pod/variant` holds, and its build's
/// log and outputs are under on the host: the names the proof of
/// concept's images were measured under. mkosi.postinst.chroot reads it.
pub fn marker(v: Variant) -> &'static str {
    match v {
        Variant::Locked => "P",
        Variant::Debug => "P-debug",
    }
}

/// Where the build's outputs land here.
pub fn local_out(root: &Path, v: Variant) -> PathBuf {
    root.join("target/pod-image").join(v.name())
}

/// What a build plan is made from.
pub struct Builder<'a> {
    pub cfg: &'a Config,
    pub build: &'a Build,
    pub host: Host,
    pub tools: Tools,
    /// The repository here.
    pub root: PathBuf,
    /// The commit shipped and built.
    pub commit: String,
}

impl<'a> Builder<'a> {
    pub fn new(cfg: &'a Config, root: &Path, commit: &str) -> Result<Self> {
        let build = cfg.build()?;
        Ok(Self { cfg, build, host: Host { project: cfg.project.clone(), kind: build.host.clone() }, tools: pins::tools(root)?, root: root.to_path_buf(), commit: commit.to_string() })
    }

    pub fn on_host(&self, what: impl Into<String>, c: HostCmd) -> Step {
        Step::new(what, self.host.cmd(&c))
    }

    /// A path in the build's folder on the host.
    pub fn at(&self, rel: &str) -> String {
        format!("{}/{rel}", self.build.dir)
    }

    fn cargo(&self) -> String {
        format!("{}/.cargo/bin/cargo", self.build.home)
    }

    /// The tree, the tools and pod-init on the host: every build's start.
    fn prepare(&self, p: &mut Plan, debug: bool) -> Result<()> {
        let src = self.at("src");
        let mkosi = self.at("mkosi");
        let m = &self.tools.mkosi;
        p.push(self.on_host("make the build's folder", HostCmd::new(["mkdir", "-p", &self.build.dir])));
        p.push(self.on_host("clear the last tree shipped", HostCmd::new(["rm", "-rf", &src])));
        p.push(self.on_host("make its folder", HostCmd::new(["mkdir", "-p", &src])));
        let archive = vec!["git".into(), "-C".into(), self.root.to_string_lossy().into_owned(), "archive".into(), "--format=tar".into(), self.commit.clone()];
        p.push(Step::new(format!("ship the tree being built ({}) to the build host", self.commit), self.host.cmd(&HostCmd::new(["tar", "-x", "-C", &src])).stdin(Stdin::Pipe(archive))));
        p.push(
            self.on_host(format!("mkosi {}", m.tag), HostCmd::new(["git", "clone", "-q", "--branch", &m.tag, &m.url, &mkosi]))
                .unless(Probe::Succeeds(self.host.cmd(&HostCmd::new(["test", "-d", &format!("{mkosi}/.git")]))), Otherwise::Skip("mkosi is cloned".into())),
        );
        p.push(self.on_host(format!("mkosi is {}", m.commit), HostCmd::new(["git", "-C", &mkosi, "rev-parse", "HEAD"])).check(Check::FirstWord(m.commit.clone())));
        // mkosi runs as root: Ubuntu 24.04's AppArmor refuses the
        // unprivileged user namespaces its sandbox would use. Every other
        // tool comes from the Debian tools tree mkosi builds itself.
        p.push(self.on_host(
            "the host's packages mkosi needs",
            HostCmd::new(["sudo", "env", "DEBIAN_FRONTEND=noninteractive", "apt-get", "install", "-y", "-q", "bubblewrap", "ubuntu-keyring", "debian-archive-keyring"]),
        ));
        p.push(self.on_host("Rust's musl target, for a static pod-init", HostCmd::new([format!("{}/.cargo/bin/rustup", self.build.home), "target".into(), "add".into(), "x86_64-unknown-linux-musl".into()]).cwd(&src)));
        p.push(self.on_host(
            "pod-init from the tree being built, static",
            HostCmd::new([self.cargo().as_str(), "build", "--locked", "--release", "-p", "fragment-pod-init", "--bin", "pod-init", "--target", "x86_64-unknown-linux-musl"])
                .cwd(&src)
                .env("CARGO_TARGET_DIR", self.at("pod-init-target")),
        ));
        p.push(self.on_host("keep it", HostCmd::new(["install", "-m", "0755", &self.at("pod-init-target/x86_64-unknown-linux-musl/release/pod-init"), &self.at("pod-init")])));
        p.push(self.on_host("its hash", HostCmd::new(["sha256sum", "pod-init"]).cwd(&self.build.dir).to(self.at("pod-init.sha256"), Append::No)));
        let stub = self.at("stub257");
        let deb = format!("{stub}/{}", pins::stub_file(&self.tools));
        p.push(self.on_host("systemd-stub 257's folder", HostCmd::new(["mkdir", "-p", &stub])));
        p.push(
            self.on_host("systemd-stub 257's package", HostCmd::new(["curl", "-fsSL", "-o", &deb, &self.tools.stub.url]))
                .unless(Probe::Succeeds(self.host.cmd(&HostCmd::new(["test", "-f", &deb]))), Otherwise::Skip("fetched before".into())),
        );
        p.push(self.on_host("its hash is the pinned one", HostCmd::new(["sha256sum", &deb])).check(Check::FirstWord(self.tools.stub.sha256.clone())));
        p.push(self.on_host("clear its last unpacking", HostCmd::new(["rm", "-rf", &format!("{stub}/x")])));
        p.push(self.on_host("unpack it", HostCmd::new(["dpkg-deb", "-x", &deb, &format!("{stub}/x")])));
        p.push(self.on_host("the platform is staged (cargo xtask pod image platform all stages it)", HostCmd::new(["test", "-d", &self.at("platform/stage/usr/lib/pod")])));
        p.push(self.on_host("and its packages fetched", HostCmd::new(["test", "-d", &self.at("platform/debs")])));
        if debug {
            let key = self.build.debug_key.as_ref().context("the debug image admits one SSH key: name its public key as build.debug_key")?;
            p.push(Step::new(format!("the one key the debug image admits ({})", key.display()), self.host.cmd(&HostCmd::new(["tee", &self.at("debug-authorized-key")])).stdin(Stdin::File(key.clone())).stdout(Stdout::Discard)));
        }
        Ok(())
    }

    /// One image: the work folder assembled, mkosi, the outputs kept and
    /// described, and the small ones fetched here.
    fn image(&self, p: &mut Plan, v: Variant) {
        let work = self.at("work");
        let extra = format!("{work}/mkosi.extra");
        let mark = marker(v);
        let out = self.at(&format!("out/{mark}"));
        let log = self.at(&format!("build-{mark}.log"));
        let n = v.name();
        p.push(self.on_host(format!("{n}: mkosi's work folder (its tools tree is kept between builds)"), HostCmd::new(["mkdir", "-p", &work])));
        let last: Vec<String> = ["mkosi.conf", "mkosi.conf.d", "mkosi.repart", "mkosi.extra", "mkosi.postinst.chroot"].iter().map(|f| format!("{work}/{f}")).collect();
        p.push(self.on_host(format!("{n}: clear the last build's definition"), HostCmd::new(["sudo".to_string(), "rm".into(), "-rf".into()].into_iter().chain(last))));
        p.push(self.on_host(format!("{n}: the image's definition"), HostCmd::new(["cp", "-a", &self.at("src/pod-image/mkosi/."), &format!("{work}/")])));
        if v == Variant::Debug {
            p.push(self.on_host("debug: what the debug sibling adds", HostCmd::new(["cp", "-a", &self.at("src/pod-image/debug/mkosi/."), &format!("{work}/")])));
            p.push(self.on_host("debug: its one key", HostCmd::new(["install", "-D", "-m", "0644", &self.at("debug-authorized-key"), &format!("{extra}/etc/ssh/authorized_keys/alex")])));
        }
        p.push(self.on_host(format!("{n}: pod-init"), HostCmd::new(["install", "-D", "-m", "0755", &self.at("pod-init"), &format!("{extra}/usr/bin/pod-init")])));
        let paths = format!(
            "# written by cargo xtask pod image build: the platform it staged\n[Content]\nExtraTrees={}\nPackageDirectories={}\n",
            self.at("platform/stage"),
            self.at("platform/debs")
        );
        p.push(Step::new(format!("{n}: where the staged platform is"), self.host.cmd(&HostCmd::new(["tee", &format!("{work}/mkosi.conf.d/40-platform-paths.conf")])).stdin(Stdin::Bytes(paths.into_bytes())).stdout(Stdout::Discard)));
        let stub = "usr/lib/systemd/boot/efi/linuxx64.efi.stub";
        p.push(self.on_host(format!("{n}: systemd-stub 257"), HostCmd::new(["install", "-D", "-m", "0644", &self.at(&format!("stub257/x/{stub}")), &format!("{extra}/{stub}")])));
        p.push(self.on_host(format!("{n}: /usr/lib/pod's folder"), HostCmd::new(["mkdir", "-p", &format!("{extra}/usr/lib/pod")])));
        p.push(Step::new(format!("{n}: /usr/lib/pod/variant, {mark}"), self.host.cmd(&HostCmd::new(["tee", &format!("{extra}/usr/lib/pod/variant")])).stdin(Stdin::Bytes(format!("{mark}\n").into_bytes())).stdout(Stdout::Discard)));
        p.push(self.on_host(format!("{n}: clear the last build's outputs"), HostCmd::new(["sudo", "find", &work, "-maxdepth", "1", "(", "-name", "pod*", "-o", "-name", "initrd*", ")", "-delete"])));
        p.push(
            self.on_host(format!("{n}: mkosi build, as root (its log: {log})"), HostCmd::new(["sudo", &self.at("mkosi/bin/mkosi"), "build"]).cwd(&work).log(&log, Append::No))
                .on_failure(self.host.cmd(&HostCmd::new(["tail", "-40", &log]))),
        );
        p.push(self.on_host(format!("{n}: clear its output folder"), HostCmd::new(["sudo", "rm", "-rf", &out])));
        p.push(self.on_host(format!("{n}: make it"), HostCmd::new(["mkdir", "-p", &out])));
        p.push(self.on_host(format!("{n}: keep the disk, the UKI and the root hash"), HostCmd::new(["sudo", "cp", &format!("{work}/pod.raw"), &format!("{work}/pod.efi"), &format!("{work}/pod.roothash"), &format!("{out}/")])));
        p.push(self.on_host(format!("{n}: owned by the build's user"), HostCmd::new(["sudo", "chown", "-R", &format!("--reference={}", self.build.dir), &out])));
        p.push(self.on_host(format!("{n}: their hashes"), HostCmd::new(["sha256sum", "pod.raw", "pod.efi", "pod.roothash"]).cwd(&out).to(format!("{out}/sha256sums"), Append::No)));
        p.push(self.on_host(format!("{n}: the GPT as the firmware reads it, LBA 0 to 33"), HostCmd::new(["dd", "if=pod.raw", "of=gpt.bin", "bs=512", "count=34", "status=none"]).cwd(&out)));
        p.push(self.on_host(format!("{n}: the partitions"), HostCmd::new(["sudo", "sfdisk", "--json", "pod.raw"]).cwd(&out).to(format!("{out}/partitions.json"), Append::No)));
        p.push(self.on_host(format!("{n}: the sizes"), HostCmd::new(["stat", "-c", "%s %n", "pod.raw", "pod.efi"]).cwd(&out).to(format!("{out}/sizes"), Append::No)));
        p.push(self.on_host(format!("{n}: pod-init's hash, beside them"), HostCmd::new(["cp", &self.at("pod-init.sha256"), &format!("{out}/")])));
        let here = local_out(&self.root, v);
        p.push(Step::new(format!("{n}: the outputs' folder here"), Cmd::new(["mkdir".to_string(), "-p".into(), here.to_string_lossy().into_owned()])));
        for f in FETCHED {
            p.push(Step::new(format!("{n}: fetch {f}"), self.host.cmd(&HostCmd::new(["cat", &format!("{out}/{f}")])).stdout(Stdout::File(here.join(f)))));
        }
    }
}

/// `pod image build`: each variant asked for, in order.
pub fn build_plan(b: &Builder<'_>, variants: &[Variant]) -> Result<Plan> {
    let names: Vec<&str> = variants.iter().map(|v| v.name()).collect();
    let mut p = Plan::new(format!("cargo xtask pod image build: {} from {} on {}", names.join(" and "), b.commit, host_name(&b.host)));
    p.note("the tree must be committed and clean: the commit is what is shipped and built");
    p.note(format!("then writes {} for each", local_out(&b.root, Variant::Locked).with_file_name("<variant>/build.json").display()));
    b.prepare(&mut p, variants.contains(&Variant::Debug))?;
    for v in variants {
        b.image(&mut p, *v);
    }
    Ok(p)
}

fn host_name(h: &Host) -> String {
    match &h.kind {
        super::config::HostKind::Local => "this machine".into(),
        super::config::HostKind::Gce { instance, zone } => format!("{instance} ({zone})"),
    }
}

/// What a build here says of its disk image (its `sizes`, `sha256sums`
/// and `build.json`), read before a publish so its plan is exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub raw_bytes: u64,
    pub raw_sha256: String,
    pub efi_sha256: String,
    pub commit: String,
}

impl Built {
    pub fn read(dir: &Path) -> Result<Self> {
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).with_context(|| format!("read {}: build this variant first", dir.join(f).display()));
        let sizes = read("sizes")?;
        let raw_bytes = sizes.lines().find_map(|l| l.strip_suffix(" pod.raw")).context("sizes names no pod.raw")?.trim().parse().context("pod.raw's size")?;
        let sums = read("sha256sums")?;
        let sum = |f: &str| -> Result<String> {
            let s = sums.lines().find_map(|l| l.strip_suffix(&format!("  {f}"))).with_context(|| format!("sha256sums names no {f}"))?;
            anyhow::ensure!(s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()), "{f}'s hash {s:?}");
            Ok(s.to_string())
        };
        let info: serde_json::Value = serde_json::from_str(&read("build.json")?).context("build.json")?;
        let commit = info["commit"].as_str().context("build.json names no commit")?.to_string();
        Ok(Self { raw_bytes, raw_sha256: sum("pod.raw")?, efi_sha256: sum("pod.efi")?, commit })
    }

    /// The scratch disk: the image's size rounded up to a whole GB, one
    /// more, and 10 at least.
    pub fn scratch_gb(&self) -> u64 {
        (self.raw_bytes.div_ceil(1 << 30) + 1).max(10)
    }
}

/// `pod image publish`: one variant's disk as a Compute Engine image.
pub fn publish_plan(b: &Builder<'_>, images: &Images, v: Variant, built: &Built) -> Result<Plan> {
    let super::config::HostKind::Gce { instance, zone } = &b.build.host else {
        anyhow::bail!("publishing writes the disk onto a scratch disk attached to the build VM: build.host must be \"gce\"");
    };
    let project = b.cfg.project.as_str();
    let image = images.of(v);
    let disk = format!("{image}-scratch");
    anyhow::ensure!(disk != "persistent-disk-0", "the scratch disk is never the boot disk");
    let dev = format!("/dev/disk/by-id/google-{disk}");
    let raw = b.at(&format!("out/{}/pod.raw", marker(v)));
    let zone_flag = format!("--zone={zone}");
    let mut p = Plan::new(format!("cargo xtask pod image publish: {} ({} bytes, UKI sha256 {}) as image {image}", v.name(), built.raw_bytes, built.efi_sha256));
    p.note(format!("makes scratch disk {disk} ({} GB) in {zone}, writes the disk onto it through {instance}, makes image {image}, then deletes {disk}", built.scratch_gb()));
    let list = |kind: &str, extra: &[&str], name: &str| {
        let filter = format!("--filter=name={name}");
        let mut a = vec!["compute", kind, "list"];
        a.extend_from_slice(extra);
        a.extend([filter.as_str(), "--format=value(name)"]);
        Probe::Lists(gcloud(project, &a), name.to_string())
    };
    p.push(Step::gate(format!("image {image} does not exist yet"), list("images", &["--no-standard-images"], image), false, Otherwise::Done(format!("image {image} exists; nothing to do"))));
    let labels = b.cfg.labels_with(&[("pod-role", "scratch"), ("pod-variant", v.name())]);
    let zones = format!("--zones={zone}");
    p.push(
        Step::new(
            format!("scratch disk {disk}"),
            gcloud(project, &["compute", "disks", "create", &disk, &zone_flag, &format!("--size={}GB", built.scratch_gb()), "--type=pd-balanced", &format!("--labels={labels}"), &format!("--description=cargo xtask pod image publish: scratch for {image}")]),
        )
        .unless(list("disks", &[&zones], &disk), Otherwise::Skip(format!("{disk} is left from a run that stopped: reused"))),
    );
    p.push(
        Step::new(format!("attach it to {instance}"), gcloud(project, &["compute", "instances", "attach-disk", instance, &zone_flag, &format!("--disk={disk}"), &format!("--device-name={disk}")]))
            .unless(Probe::Lists(gcloud(project, &["compute", "instances", "describe", instance, &zone_flag, "--format=value(disks[].deviceName.list())"]), disk.clone()), Otherwise::Skip("attached".into())),
    );
    p.push(b.on_host("its device is there", HostCmd::new(["test", "-b", &dev])).retry(12, std::time::Duration::from_secs(5)));
    p.push(b.on_host("the disk on the host is the one built here", HostCmd::new(["sha256sum", &raw])).check(Check::FirstWord(built.raw_sha256.clone())));
    p.push(b.on_host("write it", HostCmd::new(["sudo", "dd", &format!("if={raw}"), &format!("of={dev}"), "bs=4M", "conv=fsync", "status=none"])));
    p.push(b.on_host("read it back", HostCmd::new(["sudo", "cmp", "-n", &built.raw_bytes.to_string(), &raw, &dev])));
    p.push(Step::new("detach it", gcloud(project, &["compute", "instances", "detach-disk", instance, &zone_flag, &format!("--disk={disk}")])));
    let labels = b.cfg.labels_with(&[("pod-role", "image"), ("pod-variant", v.name())]);
    p.push(Step::new(
        format!("image {image}"),
        gcloud(project, &[
            "compute", "images", "create", image,
            &format!("--source-disk={disk}"), &format!("--source-disk-zone={zone}"),
            &format!("--guest-os-features={GUEST_OS_FEATURES}"), &format!("--storage-location={}", region(zone)),
            &format!("--labels={labels}"),
            &format!("--description=cargo xtask pod image publish: {} ({}) of {}; UKI sha256 {}", v.name(), marker(v), built.commit, built.efi_sha256),
        ]),
    ));
    p.push(Step::new(format!("delete scratch disk {disk}"), gcloud(project, &["compute", "disks", "delete", &disk, &zone_flag])));
    p.push(Step::new("the image", gcloud(project, &["compute", "images", "describe", image, "--format=table(name,status,diskSizeGb,archiveSizeBytes,guestOsFeatures[].type.list())"])));
    Ok(p)
}

/// build.json beside a build's outputs: what it was built from.
pub fn build_info(b: &Builder<'_>, v: Variant) -> Result<String> {
    let pin = pins::platform(&b.root)?;
    let info = serde_json::json!({
        "variant": v.name(),
        "marker": marker(v),
        "commit": b.commit,
        "mkosi": b.tools.mkosi.commit,
        "stub_sha256": b.tools.stub.sha256,
        "platform": {
            "pinned": pins::PLATFORM_DIR,
            "fragment": pin.fragment.commit,
            "celld": pin.celld.commit,
            "sandcastle": pin.sandcastle.commit,
        },
        "host": host_name(&b.host),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&info)?))
}

#[cfg(test)]
pub mod tests {
    use super::super::config::tests::example;
    use super::super::config::HostKind;
    use super::super::plan::stub::Stub;
    use super::super::plan::Ran;
    use super::*;
    use fragment_devstack as devstack;

    pub fn builder(cfg: &Config) -> Builder<'_> {
        Builder::new(cfg, &devstack::repo_root(), "0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    fn lines(p: &Plan) -> Vec<String> {
        p.render().lines().filter_map(|l| l.trim().strip_prefix("$ ").map(str::to_string)).collect()
    }

    const SSH: &str = "gcloud --project=pod-project --quiet compute ssh pod-build --zone=us-central1-f";

    /// The build's commands, against box.sh's and build.sh's: the same
    /// steps, in the same order, on the named host. Where one differs it
    /// says why here.
    #[test]
    fn the_build_runs_what_box_sh_ran() {
        let cfg = example();
        let b = builder(&cfg);
        let p = build_plan(&b, &[Variant::Locked, Variant::Debug]).unwrap();
        let l = lines(&p);
        let root = devstack::repo_root();
        // build.sh's ship_sources tarred poc/pod; this ships the commit
        assert_eq!(l[3], format!("git -C {} archive --format=tar 0123456789abcdef0123456789abcdef01234567 | {SSH} '--command=tar -x -C /home/builder/podimg/src'", root.display()));
        assert!(l.contains(&format!("{SSH} '--command=git clone -q --branch v27.1 https://github.com/systemd/mkosi /home/builder/podimg/mkosi'")));
        // box.sh built pod-init in poc/pod/attest; this, crates/pod-init
        assert!(l.contains(&format!("{SSH} '--command=cd /home/builder/podimg/src && env CARGO_TARGET_DIR=/home/builder/podimg/pod-init-target /home/builder/.cargo/bin/cargo build --locked --release -p fragment-pod-init --bin pod-init --target x86_64-unknown-linux-musl'")));
        assert!(l.contains(&format!("{SSH} '--command=sudo rm -rf /home/builder/podimg/work/mkosi.conf /home/builder/podimg/work/mkosi.conf.d /home/builder/podimg/work/mkosi.repart /home/builder/podimg/work/mkosi.extra /home/builder/podimg/work/mkosi.postinst.chroot'")));
        assert!(l.contains(&format!("{SSH} '--command=cp -a /home/builder/podimg/src/pod-image/mkosi/. /home/builder/podimg/work/'")));
        assert!(l.contains(&format!("{SSH} '--command=cd /home/builder/podimg/work && sudo /home/builder/podimg/mkosi/bin/mkosi build > /home/builder/podimg/build-P.log 2>&1'")));
        assert!(l.contains(&format!("{SSH} '--command=tee /home/builder/podimg/work/mkosi.extra/usr/lib/pod/variant' <<< P > /dev/null")));
        assert!(l.contains(&format!("{SSH} '--command=tee /home/builder/podimg/work/mkosi.extra/usr/lib/pod/variant' <<< P-debug > /dev/null")));
        assert!(l.contains(&format!("{SSH} '--command=tee /home/builder/podimg/debug-authorized-key' < /tmp/pod-debug-key.pub > /dev/null")));
        assert!(l.contains(&format!("{SSH} '--command=install -D -m 0644 /home/builder/podimg/debug-authorized-key /home/builder/podimg/work/mkosi.extra/etc/ssh/authorized_keys/alex'")));
        assert!(l.contains(&format!("{SSH} '--command=cd /home/builder/podimg/out/P && dd if=pod.raw of=gpt.bin bs=512 count=34 status=none'")));
        let here = local_out(&root, Variant::Debug);
        assert!(l.contains(&format!("{SSH} '--command=cat /home/builder/podimg/out/P-debug/pod.efi' > {}", here.join("pod.efi").display())));
        // the locked image gets no debug files
        let locked = build_plan(&b, &[Variant::Locked]).unwrap().render();
        assert!(!locked.contains("debug") && !locked.contains("authorized_keys"), "{locked}");
        assert!(l.iter().all(|c| c.starts_with(SSH) || c.starts_with("git -C") || c.starts_with("mkdir -p")), "every command runs on the host, but the archive and the folder here");
    }

    /// The same plan on this machine as the host: each line through `sh
    /// -c`, the same lines.
    #[test]
    fn a_local_host_runs_the_same_lines() {
        let mut cfg = example();
        cfg.build.as_mut().unwrap().host = HostKind::Local;
        let p = build_plan(&builder(&cfg), &[Variant::Locked]).unwrap();
        assert!(lines(&p).contains(&"sh -c 'cd /home/builder/podimg/work && sudo /home/builder/podimg/mkosi/bin/mkosi build > /home/builder/podimg/build-P.log 2>&1'".to_string()));
    }

    #[test]
    fn the_debug_image_needs_its_key() {
        let mut cfg = example();
        cfg.build.as_mut().unwrap().debug_key = None;
        let e = build_plan(&builder(&cfg), &[Variant::Debug]).unwrap_err();
        assert!(e.to_string().contains("build.debug_key"), "{e}");
        assert!(build_plan(&builder(&cfg), &[Variant::Locked]).is_ok());
    }

    /// Replay: a second build on the same host clones nothing and fetches
    /// no stub; a mkosi checkout at another commit stops it.
    #[test]
    fn a_rebuild_reuses_the_tools_and_checks_them() {
        let cfg = example();
        let p = build_plan(&builder(&cfg), &[Variant::Locked]).unwrap();
        let stub = Stub::default()
            .answer("rev-parse HEAD", "ecf7f5702bb2052b7a69cab3fe7d37bc3b9ad77b\n", true)
            .answer("stub257/systemd-boot-efi", "390ecdcef9bbb753f51bb6d8f696bdae2f1dbedde7239c49ccb2512f137a8933  x.deb\n", true);
        let mut s = stub;
        assert_eq!(p.run(&mut s).unwrap(), Ran::All);
        assert!(!s.ran.iter().any(|c| c.contains("git clone") || c.contains("curl")), "the clone and the stub were there");
        let mut s = Stub::default().answer("rev-parse HEAD", "1111111111111111111111111111111111111111\n", true);
        let e = format!("{:#}", p.run(&mut s).unwrap_err());
        assert!(e.contains("mkosi is ecf7f570"), "{e}");
        assert!(!s.ran.iter().any(|c| c.contains("mkosi build")), "nothing built on another mkosi");
    }

    fn built() -> Built {
        Built { raw_bytes: 4_789_059_584, raw_sha256: "73".repeat(32), efi_sha256: "9a".repeat(32), commit: "0123456".into() }
    }

    /// The publish's commands, against publish.sh's: the same gcloud calls
    /// in the same order. Its scratch disk is `<image>-scratch` (the
    /// script's `<prefix>-img-disk-<v>`), the read-back is `cmp` against
    /// the host's file after its hash is checked against the one fetched
    /// here (the script hashed the device and compared on the host), and
    /// the description names the commit.
    #[test]
    fn the_publish_runs_what_publish_sh_ran() {
        let cfg = example();
        let b = builder(&cfg);
        let p = publish_plan(&b, cfg.images().unwrap(), Variant::Locked, &built()).unwrap();
        let l = lines(&p);
        let g = "gcloud --project=pod-project --quiet compute";
        assert_eq!(l[0], format!("{g} disks create pod-p-scratch --zone=us-central1-f --size=10GB --type=pd-balanced --labels=owner=alex,pod-role=scratch,pod-variant=locked '--description=cargo xtask pod image publish: scratch for pod-p'"));
        assert_eq!(l[1], format!("{g} instances attach-disk pod-build --zone=us-central1-f --disk=pod-p-scratch --device-name=pod-p-scratch"));
        assert_eq!(l[4], format!("{SSH} '--command=sudo dd if=/home/builder/podimg/out/P/pod.raw of=/dev/disk/by-id/google-pod-p-scratch bs=4M conv=fsync status=none'"));
        assert_eq!(l[5], format!("{SSH} '--command=sudo cmp -n 4789059584 /home/builder/podimg/out/P/pod.raw /dev/disk/by-id/google-pod-p-scratch'"));
        assert_eq!(l[6], format!("{g} instances detach-disk pod-build --zone=us-central1-f --disk=pod-p-scratch"));
        assert_eq!(l[7], format!("{g} images create pod-p --source-disk=pod-p-scratch --source-disk-zone=us-central1-f --guest-os-features=UEFI_COMPATIBLE,TDX_CAPABLE,GVNIC --storage-location=us-central1 --labels=owner=alex,pod-role=image,pod-variant=locked '--description=cargo xtask pod image publish: locked (P) of 0123456; UKI sha256 {}'", "9a".repeat(32)));
        assert_eq!(l[8], format!("{g} disks delete pod-p-scratch --zone=us-central1-f"));
        assert!(p.render().contains(&format!("first: {g} images list --no-standard-images --filter=name=pod-p '--format=value(name)'")));
        assert_eq!(built().scratch_gb(), 10, "4.8 GB: 5, one more, 10 at least");
        assert_eq!(Built { raw_bytes: 20 << 30, ..built() }.scratch_gb(), 21);
    }

    /// Replay: an image that exists ends the publish before anything is
    /// made. Restart: a scratch disk left attached by a run that stopped
    /// is reused, not made again. A disk on the host whose hash is not the
    /// build's is never written.
    #[test]
    fn a_publish_again_does_nothing_and_a_stopped_one_resumes() {
        let cfg = example();
        let b = builder(&cfg);
        let p = publish_plan(&b, cfg.images().unwrap(), Variant::Locked, &built()).unwrap();
        let mut s = Stub::default().answer("images list", "pod-p\n", true);
        assert_eq!(p.run(&mut s).unwrap(), Ran::Done { at: 1 });
        assert_eq!(s.ran.len(), 1, "{:?}", s.ran);
        let sha = format!("{}  /home/builder/podimg/out/P/pod.raw", "73".repeat(32));
        let mut s = Stub::default().answer("disks list", "pod-p-scratch\n", true).answer("deviceName", "persistent-disk-0;pod-p-scratch", true).answer("--command=sha256sum", &sha, true);
        assert_eq!(p.run(&mut s).unwrap(), Ran::All);
        assert!(!s.ran.iter().any(|c| c.contains("disks create") || c.contains("attach-disk")), "{:?}", s.ran);
        assert!(s.ran.iter().any(|c| c.contains("images create pod-p")));
        let mut s = Stub::default().answer("--command=sha256sum", &format!("{}  x", "00".repeat(32)), true);
        assert!(p.run(&mut s).is_err());
        assert!(!s.ran.iter().any(|c| c.contains("dd if=")), "a disk not the build's is never written");
    }

    #[test]
    fn publishing_needs_a_build_vm() {
        let mut cfg = example();
        cfg.build.as_mut().unwrap().host = HostKind::Local;
        let b = builder(&cfg);
        assert!(publish_plan(&b, cfg.images().unwrap(), Variant::Locked, &built()).unwrap_err().to_string().contains("\"gce\""));
    }

    #[test]
    fn reads_what_a_build_fetched() {
        let dir = std::env::temp_dir().join(format!("pod-built-{}", devstack::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sizes"), "4789059584 pod.raw\n56205312 pod.efi\n").unwrap();
        std::fs::write(dir.join("sha256sums"), format!("{}  pod.raw\n{}  pod.efi\n", "73".repeat(32), "9a".repeat(32))).unwrap();
        assert!(Built::read(&dir).unwrap_err().to_string().contains("build.json"));
        std::fs::write(dir.join("build.json"), r#"{"commit": "0123456"}"#).unwrap();
        assert_eq!(Built::read(&dir).unwrap(), built());
        std::fs::write(dir.join("sha256sums"), "zz  pod.raw\n").unwrap();
        assert!(Built::read(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
