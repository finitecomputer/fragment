//! `cargo xtask pod image platform <step>`: the platform the image
//! carries, built on the build host from the pins in
//! pod-image/pinned-platform/ (recorded debt) and staged as the tree
//! mkosi copies to /usr/lib/pod. Nothing in the image is fetched or
//! compiled at boot: what the stack's first start fetched on the demo VM
//! is fetched here, once, by running the stack (`trial`). It replaces the
//! proof of concept's platform.sh, step for step.
//!
//! Steps, in order, each safe to run again: `sources` (clones at their
//! pins, patched), `build` (cargo builds of celld, sandcastle's three,
//! the fork's xtask, fragment-lan and its cell), `trial` (`xtask dev
//! --lan` once, so the tree fetches Node, node_modules, Dex, OpenBao and
//! chrome-headless-shell; then its state removed), `images` (the
//! computers' Docker images, saved), `debs` (Docker CE, containerd.io and
//! Tailscale, held to debs.sha256), `stage`, and `all`.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

use super::host::{Append, HostCmd};
use super::image::Builder;
use super::pins::{self, Platform};
use super::plan::{Check, Otherwise, Plan, Probe, Step};

pub const STEPS: [&str; 6] = ["sources", "build", "trial", "images", "debs", "stage"];

/// What chrome-headless-shell needs to run (50-platform.conf carries the
/// same list into the image), and NSS's certutil: the trial checks the
/// browser it fetches by running it.
const BROWSER_LIBS: [&str; 17] = [
    "libnss3", "libnspr4", "libatk1.0-0t64", "libatk-bridge2.0-0t64", "libcups2t64", "libdrm2", "libxkbcommon0", "libxcomposite1", "libxdamage1",
    "libxfixes3", "libxrandr2", "libgbm1", "libasound2t64", "libpango-1.0-0", "libcairo2", "fonts-liberation", "libnss3-tools",
];

/// How long the trial's stack may take to say it is ready: ten minutes,
/// as platform.sh waited.
const TRIAL_TRIES: u32 = 120;
const TRIAL_PAUSE: Duration = Duration::from_secs(5);

struct Ctx<'a> {
    b: &'a Builder<'a>,
    pin: Platform,
    /// The platform's folder on the host.
    p: String,
    tree: String,
}

impl Ctx<'_> {
    fn at(&self, rel: &str) -> String {
        format!("{}/{rel}", self.p)
    }

    fn step(&self, what: impl Into<String>, c: HostCmd) -> Step {
        self.b.on_host(what, c)
    }

    fn cargo(&self) -> String {
        format!("{}/.cargo/bin/cargo", self.b.build.home)
    }

    /// A pinned file from the tree shipped to the host.
    fn pinned(&self, file: &str) -> String {
        self.b.at(&format!("src/{}/{file}", pins::PLATFORM_DIR))
    }

    /// A clone at exactly `commit`, clean but for its build outputs.
    fn checkout(&self, plan: &mut Plan, name: &str, url: &str, dir: &str, commit: &str, branch: &str) {
        plan.push(
            self.step(format!("{name}: clone"), HostCmd::new(["git", "clone", "-q", url, dir]))
                .unless(Probe::Succeeds(self.b.host.cmd(&HostCmd::new(["test", "-d", &format!("{dir}/.git")]))), Otherwise::Skip("cloned before".into())),
        );
        plan.push(self.step(format!("{name}: fetch"), HostCmd::new(["git", "-C", dir, "fetch", "-q", "--all"])));
        plan.push(self.step(format!("{name}: at {commit} (on {branch})"), HostCmd::new(["git", "-C", dir, "checkout", "-q", "--force", "--detach", commit])));
        plan.push(self.step(format!("{name}: clean"), HostCmd::new(["git", "-C", dir, "clean", "-q", "-fdx", "-e", "target", "-e", "node_modules"])));
        plan.push(self.step(format!("{name}: its commit"), HostCmd::new(["git", "-C", dir, "log", "-1", "--format=%H"])));
    }

    fn sources(&self, plan: &mut Plan) {
        let pin = &self.pin;
        plan.push(self.step("the platform's folder", HostCmd::new(["mkdir", "-p", &self.p])));
        self.checkout(plan, "celld", &pin.celld.url, &self.at("celld"), &pin.celld.commit, &pin.celld.branch);
        self.checkout(plan, "sandcastle", &pin.sandcastle.url, &self.at("sandcastle"), &pin.sandcastle.commit, &pin.sandcastle.branch);
        if let Some(patch) = &pin.sandcastle.patch {
            plan.push(self.step(format!("sandcastle: {patch}"), HostCmd::new(["git", "-C", &self.at("sandcastle"), "apply", &self.pinned(patch)])));
        }
        // the tree's owner is the stack's user, as on the volume
        let (uid, gid) = (pin.stack_uid.to_string(), pin.stack_gid.to_string());
        plan.push(self.step(format!("the build's user is the stack's, uid {uid}"), HostCmd::new(["id", "-u"])).check(Check::FirstWord(uid.clone())));
        plan.push(self.step(format!("and gid {gid}"), HostCmd::new(["id", "-g"])).check(Check::FirstWord(gid.clone())));
        let home = Path::new(&self.tree).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let top = Path::new(&home).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        plan.push(self.step(format!("{top}, as on the volume"), HostCmd::new(["sudo", "install", "-d", "-m", "0755", &top])));
        plan.push(self.step(format!("{home}, the stack's user's"), HostCmd::new(["sudo", "install", "-d", "-m", "0700", "-o", &uid, "-g", &gid, &home])));
        self.checkout(plan, "fragment", &pin.fragment.url, &self.tree, &pin.fragment.commit, &pin.fragment.branch);
        plan.push(self.step(format!("fragment: {}", pin.fragment.patch), HostCmd::new(["git", "-C", &self.tree, "apply", &self.pinned(&pin.fragment.patch)])));
        plan.push(self.step("fragment: what the patch changed", HostCmd::new(["git", "-C", &self.tree, "status", "--short"])));
    }

    fn build(&self, plan: &mut Plan) {
        let log = self.at("build.log");
        let cargo = self.cargo();
        let tail = self.b.host.cmd(&HostCmd::new(["tail", "-30", &log]));
        let build = |what: &str, dir: &str, args: &[&str]| {
            let argv = std::iter::once(cargo.as_str()).chain(args.iter().copied());
            self.step(what.to_string(), HostCmd::new(argv).cwd(dir).log(&log, Append::Yes)).on_failure(tail.clone())
        };
        plan.push(self.step("a fresh build log", HostCmd::new(["truncate", "-s", "0", &log])));
        plan.push(build("celld", &self.at("celld"), &["build", "--locked", "--release", "-p", "celld"]));
        plan.push(build("sandcastle's node and Docker double", &self.at("sandcastle"), &["build", "--locked", "--release", "-p", "sandcastle-node", "-p", "sandcastle-docker-engine"]));
        plan.push(build("sandcastle's relay, static", &self.at("sandcastle"), &["build", "--locked", "--release", "-p", "sandcastle-docker-relay", "--target", "x86_64-unknown-linux-musl"]));
        // `cargo xtask` is `cargo run --package xtask` (debug); the image
        // runs that binary directly
        plan.push(build("fragment's xtask", &self.tree, &["build", "--locked", "-p", "xtask"]));
        plan.push(build("fragment-lan", &self.tree, &["build", "--locked", "--release", "-p", "fragment-lan"]));
        let path = format!("{}/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", self.b.build.home);
        plan.push(self.step("fragment's cell", HostCmd::new(["target/debug/xtask", "build"]).cwd(&self.tree).env("PATH", path).log(&log, Append::Yes)).on_failure(tail));
        plan.push(self.step("the build's end", HostCmd::new(["tail", "-3", &log])));
        // worker-build's esbuild, which the cell's build fetched: one, kept
        // where the trial and the stage take it
        let cache = format!("{}/.cache/worker-build", self.b.build.home);
        let find = |rest: &[&str]| HostCmd::new(["find", &cache, "-path", "*/bin/esbuild", "-type", "f"].into_iter().chain(rest.iter().copied()));
        plan.push(self.step("one esbuild in worker-build's cache", find(&[])).check(Check::Lines(1)));
        plan.push(self.step("keep it", find(&["-exec", "install", "-m", "0755", "{}", &self.at("esbuild"), ";"])));
    }

    /// The settings the image's stack runs with (pod-stack), on loopback
    /// and with no engine: this run only fills target/tools and
    /// node_modules.
    fn trial(&self, plan: &mut Plan) {
        let (state, log, pid) = (self.at("trial-state"), self.at("trial.log"), self.at("trial.pid"));
        let apt = ["sudo", "env", "DEBIAN_FRONTEND=noninteractive", "apt-get", "install", "-y", "-q"].into_iter().chain(BROWSER_LIBS);
        plan.push(self.step("what chrome-headless-shell needs", HostCmd::new(apt)));
        plan.push(self.step("clear the last trial", HostCmd::new(["rm", "-rf", &state, &log, &pid])));
        plan.push(self.step("its state's folder", HostCmd::new(["mkdir", "-p", &state])));
        let t = &self.tree;
        let env = [
            ("FRAGMENT_PREBUILT", "1".to_string()),
            ("CELLD_BIN", self.at("celld/target/release/celld")),
            ("CELLD_ESBUILD", self.at("esbuild")),
            ("SANDCASTLE_NODE_BIN", self.at("sandcastle/target/release/sandcastle-node")),
            ("FRAGMENT_LAN_BIN", format!("{t}/target/release/fragment-lan")),
            ("FRAGMENT_LAN_STATE", state.clone()),
            ("FRAGMENT_LAN_ZONE", "private.home.arpa".into()),
            ("FRAGMENT_LAN_ADDR", "127.0.0.1".into()),
            ("FRAGMENT_LAN_BIND", "127.0.0.1".into()),
            ("FRAGMENT_LAN_HTTPS_PORT", "8443".into()),
            ("FRAGMENT_LAN_HTTP_PORT", "0".into()),
            ("FRAGMENT_LAN_DNS_PORT", "0".into()),
            ("FRAGMENT_LAN_DNS_UPSTREAM", "169.254.169.254:53".into()),
            ("FRAGMENT_LAN_USERS", "alex".into()),
            ("FRAGMENT_LAN_ENGINE", "none".into()),
        ];
        let xtask = format!("{t}/target/debug/xtask");
        let mut start = HostCmd::new([
            "start-stop-daemon", "--start", "--background", "--make-pidfile", "--pidfile", &pid, "--chdir", t, "--output", &log, "--exec", &xtask, "--", "dev", "--lan",
        ]);
        for (k, v) in env {
            start = start.env(k, v);
        }
        plan.push(self.step("start the stack once, in the background", start));
        plan.push(
            self.step("wait until it says it is ready (ten minutes at most)", HostCmd::new(["grep", "-q", "^fragment dev:", &log]))
                .retry(TRIAL_TRIES, TRIAL_PAUSE)
                .on_failure(self.b.host.cmd(&HostCmd::new(["tail", "-30", &log]))),
        );
        plan.push(self.step("stop it: Ctrl-C, then a kill after 90 s", HostCmd::new(["start-stop-daemon", "--stop", "--oknodo", "--pidfile", &pid, "--retry", "INT/90/KILL/5"])));
        // what the run left that is state, not code: never in the image
        let gone = [format!("{t}/cell/.celld"), format!("{t}/target/devstack"), format!("{t}/cell/.dev.vars"), format!("{t}/cell/wrangler.celld.jsonc"), state];
        plan.push(self.step("remove the state it left", HostCmd::new(["rm".to_string(), "-rf".into()].into_iter().chain(gone))));
        plan.push(self.step("what it fetched", HostCmd::new(["ls", &format!("{t}/target/tools")])));
        plan.push(self.step("what it said", HostCmd::new(["grep", "-E", "fetching|npm ci|ready", &log])).may_fail());
    }

    fn images(&self, plan: &mut Plan) {
        let (log, engine) = (self.at("images.log"), self.at("no-engine"));
        let tags = ["fragment-hermes:demo", "fragment-stub:demo"];
        plan.push(self.step("a folder where no engine is", HostCmd::new(["mkdir", "-p", &engine])));
        // its last step loads into an engine; there is none, so it fails
        // after the build, which is the part wanted
        plan.push(self.step("the computers' images", HostCmd::new([&format!("{}/target/debug/xtask", self.tree), "node-images", "--tag", "demo", "--engine", &engine]).cwd(&self.tree).log(&log, Append::No)).may_fail());
        plan.push(
            self.step("both were built", HostCmd::new(["docker", "image", "inspect", tags[0], tags[1]]).to("/dev/null", Append::No))
                .on_failure(self.b.host.cmd(&HostCmd::new(["tail", "-20", &log]))),
        );
        plan.push(self.step("its build's scratch is state, not code", HostCmd::new(["rm", "-rf", &format!("{}/target/devstack", self.tree)])));
        plan.push(self.step("save both", HostCmd::new(["docker", "save", tags[0], tags[1], "-o", &self.at("computers.tar")])));
        for (i, tag) in tags.iter().enumerate() {
            let append = if i == 0 { Append::No } else { Append::Yes };
            plan.push(self.step(format!("{tag}'s id"), HostCmd::new(["docker", "image", "inspect", "-f", &format!("{tag} {{{{.Id}}}}"), tag]).to(self.at("computers.ids"), append)));
        }
        plan.push(self.step("the ids", HostCmd::new(["cat", &self.at("computers.ids")])));
        plan.push(self.step("the tar", HostCmd::new(["ls", "-l", &self.at("computers.tar")])));
    }

    /// Docker's packages through apt from Docker's repository (its signed
    /// index checks them), Tailscale's from pkgs.tailscale.com; all four
    /// then held to debs.sha256, so a later build gets the same bytes or
    /// stops.
    fn debs(&self, plan: &mut Plan) {
        let d = self.at("debs");
        let k = &self.pin.packages;
        plan.push(self.step("clear the packages", HostCmd::new(["rm", "-rf", &d])));
        plan.push(self.step("their folder", HostCmd::new(["mkdir", "-p", &d])));
        let docker = |p: &str, v: &str| format!("{p}={v}");
        plan.push(self.step(
            "Docker CE and containerd.io",
            HostCmd::new(["apt-get", "download", "-q", &docker("docker-ce", &k.docker), &docker("docker-ce-cli", &k.docker), &docker("containerd.io", &k.containerd)]).cwd(&d).to("/dev/null", Append::No),
        ));
        let ts = format!("tailscale_{}_amd64.deb", k.tailscale);
        plan.push(self.step("Tailscale", HostCmd::new(["curl", "-fsSL", "-o", &format!("{d}/{ts}"), &format!("https://pkgs.tailscale.com/stable/ubuntu/pool/{ts}")])));
        plan.push(self.step(format!("each is held to {}", k.sha256s), HostCmd::new(["sha256sum", "-c", "--quiet", &self.pinned(&k.sha256s)]).cwd(&d)));
        plan.push(self.step("exactly the four pinned packages", HostCmd::new(["find", &d, "-name", "*.deb"])).check(Check::Lines(4)));
    }

    /// The image's /usr/lib/pod, as mkosi copies it (ExtraTrees=): the
    /// binaries, the tree (no .git, no state, and of target/ only what
    /// runs), the computers' images and what each part is.
    fn stage(&self, plan: &mut Plan) {
        let s = self.at("stage");
        let lib = format!("{s}/usr/lib/pod");
        let t = &self.tree;
        let home = format!("{lib}/fragment-home");
        plan.push(self.step("clear the stage", HostCmd::new(["sudo", "rm", "-rf", &s])));
        plan.push(self.step("its bin", HostCmd::new(["mkdir", "-p", &format!("{lib}/bin")])));
        plan.push(self.step("celld", HostCmd::new(["install", "-m", "0755", &self.at("celld/target/release/celld"), &format!("{lib}/bin/celld")])));
        let sc = self.at("sandcastle/target");
        plan.push(self.step("sandcastle's node and double", HostCmd::new(["install", "-m", "0755", &format!("{sc}/release/sandcastle-node"), &format!("{sc}/release/sandcastle-docker-engine"), &format!("{lib}/bin/")])));
        plan.push(self.step("sandcastle's relay", HostCmd::new(["install", "-m", "0755", &format!("{sc}/x86_64-unknown-linux-musl/release/sandcastle-docker-relay"), &format!("{lib}/bin/")])));
        plan.push(self.step("esbuild", HostCmd::new(["install", "-m", "0755", &self.at("esbuild"), &format!("{lib}/bin/esbuild")])));
        for state in ["cell/.celld", "target/devstack"] {
            plan.push(self.step(format!("the tree holds no {state} (run trial again if it does)"), HostCmd::new(["test", "!", "-e", &format!("{t}/{state}")])));
        }
        plan.push(self.step(
            "the tree, without .git, target or state",
            HostCmd::new(["rsync", "-a", "--exclude=/.git", "--exclude=/target", "--exclude=/cell/.dev.vars", "--exclude=/cell/wrangler.celld.jsonc", &format!("{t}/"), &format!("{home}/")]),
        ));
        plan.push(self.step("its target's folders", HostCmd::new(["mkdir", "-p", &format!("{home}/target/debug"), &format!("{home}/target/release")])));
        plan.push(self.step("its tools", HostCmd::new(["rsync", "-a", &format!("{t}/target/tools"), &format!("{home}/target/")])));
        plan.push(
            self.step("its cache", HostCmd::new(["rsync", "-a", &format!("{t}/target/cache"), &format!("{home}/target/")]))
                .only_if(Probe::Succeeds(self.b.host.cmd(&HostCmd::new(["test", "-d", &format!("{t}/target/cache")]))), Otherwise::Skip("the tree has no target/cache".into())),
        );
        plan.push(self.step("its xtask", HostCmd::new(["install", "-m", "0755", &format!("{t}/target/debug/xtask"), &format!("{home}/target/debug/xtask")])));
        plan.push(self.step("its fragment-lan", HostCmd::new(["install", "-m", "0755", &format!("{t}/target/release/fragment-lan"), &format!("{home}/target/release/fragment-lan")])));
        plan.push(self.step("the computers' images", HostCmd::new(["install", "-m", "0644", &self.at("computers.tar"), &self.at("computers.ids"), &format!("{lib}/")])));
        let versions = format!("{lib}/platform-versions");
        let pin = &self.pin;
        let sc_patch = pin.sandcastle.patch.as_deref().map_or(String::new(), |p| format!(" + {p}"));
        let lines = [
            (self.at("celld"), "celld %H".to_string()),
            (self.at("sandcastle"), format!("sandcastle %H{sc_patch}")),
            (self.tree.clone(), format!("fragment %H + {}", pin.fragment.patch)),
        ];
        for (i, (dir, format)) in lines.iter().enumerate() {
            let append = if i == 0 { Append::No } else { Append::Yes };
            plan.push(self.step(format!("platform-versions: {}", format.split(' ').next().unwrap_or("")), HostCmd::new(["git", "-C", dir, "log", "-1", &format!("--format={format}")]).to(&versions, append)));
        }
        plan.push(self.step("platform-versions: the computers' images", HostCmd::new(["cat", &self.at("computers.ids")]).to(&versions, Append::Yes)));
        let hashed = [
            "bin/celld", "bin/esbuild", "bin/sandcastle-docker-engine", "bin/sandcastle-docker-relay", "bin/sandcastle-node",
            "fragment-home/target/debug/xtask", "fragment-home/target/release/fragment-lan", "computers.tar",
        ];
        plan.push(self.step("platform-versions: each binary's hash", HostCmd::new(std::iter::once("sha256sum").chain(hashed)).cwd(&lib).to(&versions, Append::Yes)));
        plan.push(self.step("what is staged", HostCmd::new(["cat", &versions])));
    }
}

/// `pod image platform <step>`.
pub fn plan(b: &Builder<'_>, step: &str) -> Result<Plan> {
    let pin = pins::platform(&b.root)?;
    let c = Ctx { b, p: b.at("platform"), tree: pin.fragment.tree.clone(), pin };
    let steps: Vec<&str> = match step {
        "all" => STEPS.to_vec(),
        s if STEPS.contains(&s) => vec![s],
        other => bail!("a platform step is one of {} or all, not {other:?}", STEPS.join(", ")),
    };
    let mut p = Plan::new(format!("cargo xtask pod image platform {step}: the pinned platform ({}) on the build host", pins::PLATFORM_DIR));
    p.note("pinned, as recorded debt: docs/technical-debt-ledger.md, \"The pod image carries a pinned fork of the platform\"");
    p.note("its patches and hashes come from the tree last shipped by `pod image build` (src/ on the host); ship it first on a fresh host");
    for s in steps {
        match s {
            "sources" => c.sources(&mut p),
            "build" => c.build(&mut p),
            "trial" => c.trial(&mut p),
            "images" => c.images(&mut p),
            "debs" => c.debs(&mut p),
            "stage" => c.stage(&mut p),
            _ => unreachable!("checked"),
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::super::config::tests::example;
    use super::super::image::tests::builder;
    use super::super::plan::stub::Stub;
    use super::*;

    const SSH: &str = "gcloud --project=pod-project --quiet compute ssh pod-build --zone=us-central1-f";

    fn lines(p: &Plan) -> Vec<String> {
        p.render().lines().filter_map(|l| l.trim().strip_prefix("$ ").map(str::to_string)).collect()
    }

    /// Each step's commands, against platform.sh's. Where they differ:
    /// the trial starts the stack with start-stop-daemon and a pid file,
    /// and stops it with its --retry, where the script ran it under setsid
    /// and killed its process group; esbuild is found once and kept, where
    /// the script globbed for it at each use; platform-versions is written
    /// by git's own --format, where the script used printf.
    #[test]
    fn each_step_runs_what_platform_sh_ran() {
        let cfg = example();
        let b = builder(&cfg);
        let l = lines(&plan(&b, "all").unwrap());
        let has = |line: &str| assert!(l.iter().any(|c| c == &format!("{SSH} {}", super::super::plan::quote(&format!("--command={line}")))), "missing: {line}");
        let p = "/home/builder/podimg/platform";
        let t = "/srv/private/home/fragment-home";
        has(&format!("git -C {p}/celld checkout -q --force --detach b19f09d"));
        has(&format!("git -C {p}/sandcastle apply /home/builder/podimg/src/pod-image/pinned-platform/sandcastle-double-vms-max.patch"));
        has("sudo install -d -m 0700 -o 1001 -g 1002 /srv/private/home");
        has(&format!("git -C {t} checkout -q --force --detach 2e123d40"));
        has(&format!("git -C {t} apply /home/builder/podimg/src/pod-image/pinned-platform/fragment-prebuilt.patch"));
        has(&format!("cd {p}/sandcastle && /home/builder/.cargo/bin/cargo build --locked --release -p sandcastle-docker-relay --target x86_64-unknown-linux-musl >> {p}/build.log 2>&1"));
        has(&format!("cd {t} && env PATH=/home/builder/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin target/debug/xtask build >> {p}/build.log 2>&1"));
        has(&format!("grep -q '^fragment dev:' {p}/trial.log"));
        has(&format!("rm -rf {t}/cell/.celld {t}/target/devstack {t}/cell/.dev.vars {t}/cell/wrangler.celld.jsonc {p}/trial-state"));
        has(&format!("cd {t} && {t}/target/debug/xtask node-images --tag demo --engine {p}/no-engine > {p}/images.log 2>&1"));
        has(&format!("docker image inspect -f 'fragment-stub:demo {{{{.Id}}}}' fragment-stub:demo >> {p}/computers.ids"));
        has(&format!("cd {p}/debs && apt-get download -q docker-ce=5:29.9.3-1~ubuntu.24.04~noble docker-ce-cli=5:29.9.3-1~ubuntu.24.04~noble containerd.io=2.4.2-3~ubuntu.24.04~noble > /dev/null"));
        has(&format!("curl -fsSL -o {p}/debs/tailscale_1.104.6_amd64.deb https://pkgs.tailscale.com/stable/ubuntu/pool/tailscale_1.104.6_amd64.deb"));
        has(&format!("cd {p}/debs && sha256sum -c --quiet /home/builder/podimg/src/pod-image/pinned-platform/debs.sha256"));
        has(&format!("rsync -a --exclude=/.git --exclude=/target --exclude=/cell/.dev.vars --exclude=/cell/wrangler.celld.jsonc {t}/ {p}/stage/usr/lib/pod/fragment-home/"));
        has(&format!("git -C {p}/sandcastle log -1 '--format=sandcastle %H + sandcastle-double-vms-max.patch' >> {p}/stage/usr/lib/pod/platform-versions"));
        let trial = l.iter().find(|c| c.contains("start-stop-daemon --start")).expect("the trial's start");
        for v in ["FRAGMENT_PREBUILT=1", "FRAGMENT_LAN_ENGINE=none", "FRAGMENT_LAN_HTTPS_PORT=8443", &format!("CELLD_ESBUILD={p}/esbuild")] {
            assert!(trial.contains(v), "{v}: {trial}");
        }
        assert!(plan(&b, "nothing").is_err());
        assert_eq!(lines(&plan(&b, "debs").unwrap()).len(), 6, "one step alone");
    }

    /// Invalid: a build user that is not the stack's stops before the
    /// tree is made; four packages where the pins hold four, or not.
    #[test]
    fn stops_on_the_wrong_user_or_package_count() {
        let cfg = example();
        let b = builder(&cfg);
        let mut s = Stub::default().answer("id -u", "1000\n", true);
        assert!(format!("{:#}", plan(&b, "sources").unwrap().run(&mut s).unwrap_err()).contains("1001"));
        assert!(!s.ran.iter().any(|c| c.contains("/srv/private")), "nothing made as another user");
        let mut s = Stub::default().answer("find /home/builder/podimg/platform/debs", "a.deb\nb.deb\nc.deb\n", true);
        assert!(plan(&b, "debs").unwrap().run(&mut s).is_err());
        let mut s = Stub::default().answer("find /home/builder/podimg/platform/debs", "a.deb\nb.deb\nc.deb\nd.deb\n", true);
        assert!(plan(&b, "debs").unwrap().run(&mut s).is_ok());
    }
}
