//! `cargo xtask audit`: every dependency the repo locks, checked against
//! the public advisory databases (FIN-576; docs/security/dependency-scanning.md).
//!
//! Rust: cargo-deny's advisories check (the RustSec database), on each
//! workspace with a Cargo.lock of its own (`RUST_WORKSPACES`), each with
//! the deny.toml beside its manifest: its policy and its ignore list, each
//! entry with its reason. cargo-deny is a tool of the machine's, at
//! `CARGO_DENY_VERSION` (CI installs that release, audit.yml), refused at
//! any other: a version's findings are its own.
//!
//! JavaScript: npm audit (GitHub's advisory database), on each
//! package-lock.json (`NPM_LOCKS`), run by the pinned Node's own npm
//! (docs/pinned-node.md) with `--package-lock-only`: it reads the lock and
//! installs nothing. npm has no ignore list, so `NPM_IGNORED` is that list,
//! here, each entry with its lock and its reason.
//!
//! Every finding fails the run but an ignored one, and an ignore entry no
//! longer found fails it too (deny.toml's `unused-ignored-advisory`, and
//! `stale` here), so a fixed finding leaves its list in the change that
//! fixes it. Each lock is audited, the failures gathered, and all named at
//! the end.
//!
//! Not in `check`: an advisory published upstream would turn every pull
//! request red with no change of its own. CI runs this on master's pushes,
//! once a day, and on a pull request that changes a lock or this policy
//! (.github/workflows/audit.yml), and nothing waits on it.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

/// The cargo-deny release CI installs (.github/workflows/audit.yml, which a
/// test holds to this) and the run requires.
pub const CARGO_DENY_VERSION: &str = "0.20.2";

/// Each Rust workspace with a Cargo.lock of its own, by its directory; each
/// has its deny.toml there. A test holds this to the locks git tracks.
pub const RUST_WORKSPACES: [&str; 4] = [".", "cell", "images", "bench/search"];

/// Each directory with a package-lock.json. A test holds this to the locks
/// git tracks.
pub const NPM_LOCKS: [&str; 2] = [".", "templates/slides"];

/// An npm advisory a lock may hold for now: its GitHub advisory id, and why.
pub struct Ignored {
    pub lock: &'static str,
    pub id: &'static str,
    pub reason: &'static str,
}

/// wrangler 4.145's miniflare: the dev stack's and the e2e's local
/// runtime, never in the production Worker (workerd on Cloudflare runs no
/// miniflare). wrangler 4.167 fixes them all; that bump needs the whole
/// e2e and a hosted check (FIN-640).
const WRANGLER: &str = "miniflare under wrangler 4.145: dev, e2e and deploy tooling, not in the Worker; wrangler 4.167 bump is FIN-640";

pub const NPM_IGNORED: &[Ignored] = &[
    // undici before 7.30.1, in miniflare
    Ignored { lock: ".", id: "GHSA-2m5v-pch6-74hx", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-4w7w-5xrc-9h26", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-qrxm-fqhp-jwvm", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-xjj6-f33q-929c", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-4qv8-2p9j-2685", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-f72v-6w45-q472", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-9fm3-97px-rjf6", reason: WRANGLER },
    Ignored { lock: ".", id: "GHSA-55wc-3pg8-jfw6", reason: WRANGLER },
    // sharp before 0.35.5 (librsvg), in miniflare's local Images
    Ignored { lock: ".", id: "GHSA-wq5f-xc86-pv6w", reason: WRANGLER },
    Ignored {
        lock: ".",
        id: "GHSA-mrhx-c7xc-m3w5",
        reason: "zod 4.5.4 under @cloudflare/sandbox 1.0.0, in the cell's bundle: an oversized collection can exhaust one isolate's memory, no disclosure; \
                 sandbox 1.0.3 fixes it and needs the computers' e2e (FIN-639)",
    },
    Ignored {
        lock: "templates/slides",
        id: "GHSA-g3hv-75wg-m249",
        reason: "minimatch 10 under shadcn's ts-morph, in the slides template's build: only a hostile glob pattern costs time, and the build's are its own; \
                 no release of @open-slide/core takes a fixed one yet",
    },
];

/// `cargo xtask audit`.
pub fn run(args: &[String]) -> Result<()> {
    ensure!(args.is_empty(), "audit takes no arguments, not {args:?}");
    let root = fragment_devstack::repo_root();
    deny_is_pinned()?;
    let mut failed = Vec::new();
    for ws in RUST_WORKSPACES {
        if let Err(e) = rust(&root, ws) {
            failed.push(format!("{ws}/Cargo.lock: {e:#}"));
        }
    }
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR))?;
    let cache = root.join(fragment_devstack::CACHE_DIR);
    for lock in NPM_LOCKS {
        if let Err(e) = npm(&node, &cache, &root, lock) {
            failed.push(format!("{lock}/package-lock.json: {e:#}"));
        }
    }
    if !failed.is_empty() {
        bail!("{} of {} locks have findings (docs/security/dependency-scanning.md says how each is handled):\n  {}", failed.len(), RUST_WORKSPACES.len() + NPM_LOCKS.len(), failed.join("\n  "));
    }
    println!("audit: {} Cargo.lock and {} package-lock.json files hold no advisory but their ignore lists'", RUST_WORKSPACES.len(), NPM_LOCKS.len());
    Ok(())
}

fn deny_is_pinned() -> Result<()> {
    let out = Command::new("cargo").args(["deny", "--version"]).stdin(Stdio::null()).output();
    let install = format!("cargo install --locked cargo-deny --version {CARGO_DENY_VERSION}");
    let out = out.with_context(|| format!("could not run cargo deny: {install}"))?;
    let version = String::from_utf8_lossy(&out.stdout);
    ensure!(out.status.success(), "cargo deny is not installed: {install}");
    ensure!(version.trim() == format!("cargo-deny {CARGO_DENY_VERSION}"), "audit runs cargo-deny {CARGO_DENY_VERSION}, not {:?}: {install}", version.trim());
    Ok(())
}

/// One workspace's lock, by cargo-deny with its deny.toml; its report goes
/// to the terminal as it comes.
fn rust(root: &Path, ws: &str) -> Result<()> {
    let dir = root.join(ws);
    let status = Command::new("cargo")
        .args(["deny", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--workspace")
        .arg("--config")
        .arg(dir.join("deny.toml"))
        .args(["check", "--show-stats", "advisories"])
        .stdin(Stdio::null())
        .status()
        .context("run cargo deny")?;
    ensure!(status.success(), "cargo deny check advisories failed ({status}): its report is above");
    println!("audit: {ws}/Cargo.lock: no advisory but deny.toml's ignore list");
    Ok(())
}

/// One package-lock.json, by npm audit, held to `NPM_IGNORED`.
fn npm(node: &fragment_devstack::node::Node, cache: &Path, root: &Path, lock: &str) -> Result<()> {
    let out = node
        .script(&node.npm_cli, cache)?
        .args(["audit", "--json", "--package-lock-only"])
        .env("npm_config_cache", cache.join("npm"))
        .env("npm_config_update_notifier", "false")
        .current_dir(root.join(lock))
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("run npm audit")?;
    // npm exits 1 when it finds any advisory; its report says which
    let report: Value = serde_json::from_slice(&out.stdout).with_context(|| format!("npm audit ({}) printed no JSON report", out.status))?;
    let found = advisories(&report)?;
    let verdict = judge(lock, &found, NPM_IGNORED);
    for (id, package) in &found {
        let mark = if verdict.new.contains(&(id.clone(), package.clone())) { "FOUND  " } else { "ignored" };
        println!("audit: {lock}/package-lock.json: {mark} {id} in {package}");
    }
    let mut problems = Vec::new();
    if !verdict.new.is_empty() {
        let named: Vec<String> = verdict.new.iter().map(|(id, p)| format!("{id} in {p} (https://github.com/advisories/{id})")).collect();
        problems.push(format!("advisories not on NPM_IGNORED (xtask/src/audit.rs): {}", named.join(", ")));
    }
    if !verdict.stale.is_empty() {
        problems.push(format!("NPM_IGNORED names advisories this lock no longer holds; remove them: {}", verdict.stale.join(", ")));
    }
    ensure!(problems.is_empty(), "{}", problems.join("; "));
    println!("audit: {lock}/package-lock.json: no advisory but NPM_IGNORED's");
    Ok(())
}

/// Each advisory npm audit's report names (v2, npm 7 and on), with the
/// package it is in: a vulnerability's `via` lists advisories (objects) and
/// the packages it has them through (names), and the names add nothing.
fn advisories(report: &Value) -> Result<BTreeSet<(String, String)>> {
    if let Some(error) = report.get("error") {
        bail!("npm audit failed: {error}");
    }
    ensure!(report.get("auditReportVersion") == Some(&Value::from(2)), "npm audit's report is not version 2: {report:.200}");
    let vulnerabilities = report.get("vulnerabilities").and_then(Value::as_object).context("npm audit's report has no vulnerabilities map")?;
    let mut found = BTreeSet::new();
    for (package, entry) in vulnerabilities {
        let via = entry.get("via").and_then(Value::as_array).with_context(|| format!("{package}'s entry has no via list"))?;
        for advisory in via.iter().filter(|v| v.is_object()) {
            let url = advisory.get("url").and_then(Value::as_str).with_context(|| format!("an advisory on {package} has no url: {advisory}"))?;
            let id = url.rsplit('/').next().filter(|id| id.starts_with("GHSA-")).with_context(|| format!("an advisory on {package} names no GHSA id: {url}"))?;
            found.insert((id.to_string(), package.clone()));
        }
    }
    Ok(found)
}

/// What a lock's findings come to under the ignore list.
#[derive(Debug, PartialEq)]
struct Verdict {
    /// advisories found that `lock` may not hold
    new: Vec<(String, String)>,
    /// ignore entries for `lock` that nothing found
    stale: Vec<String>,
}

fn judge(lock: &str, found: &BTreeSet<(String, String)>, ignored: &[Ignored]) -> Verdict {
    let mine: BTreeSet<&str> = ignored.iter().filter(|i| i.lock == lock).map(|i| i.id).collect();
    let new = found.iter().filter(|(id, _)| !mine.contains(id.as_str())).cloned().collect();
    let held: BTreeSet<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    let stale = mine.iter().filter(|id| !held.contains(*id)).map(|id| id.to_string()).collect();
    Verdict { new, stale }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo() -> std::path::PathBuf {
        fragment_devstack::repo_root()
    }

    /// The locks git tracks, by directory.
    fn tracked(name: &str) -> BTreeSet<String> {
        let out = Command::new("git").args(["ls-files", "--", name, &format!("**/{name}")]).current_dir(repo()).output().expect("git runs");
        assert!(out.status.success(), "git ls-files: {}", String::from_utf8_lossy(&out.stderr));
        let dir = |path: &str| Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
        String::from_utf8_lossy(&out.stdout).lines().map(dir).collect()
    }

    /// Goal: no lock goes unscanned. Valid: each Cargo.lock and
    /// package-lock.json git tracks is in its list. Invalid: a new
    /// workspace or package whose lock nothing audits, or a listed one gone.
    #[test]
    fn every_tracked_lock_is_audited() {
        assert_eq!(tracked("Cargo.lock"), RUST_WORKSPACES.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
        assert_eq!(tracked("package-lock.json"), NPM_LOCKS.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
    }

    /// Each workspace's deny.toml exists, checks advisories, and fails on
    /// an ignore entry it no longer needs; each of its entries has a reason.
    #[test]
    fn each_workspace_keeps_its_policy() {
        for ws in RUST_WORKSPACES {
            let path = repo().join(ws).join("deny.toml");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(text.contains("[advisories]") && text.contains("unused-ignored-advisory = \"deny\""), "{}: the advisories policy, failing on unused ignores", path.display());
            for line in text.lines().map(str::trim).filter(|l| l.starts_with("{ id = ")) {
                assert!(line.contains("reason = \"") && !line.contains("reason = \"\""), "{}: an ignore without its reason: {line}", path.display());
            }
        }
    }

    /// CI installs the cargo-deny release the run requires.
    #[test]
    fn ci_installs_the_pinned_cargo_deny() {
        let workflow = std::fs::read_to_string(repo().join(".github/workflows/audit.yml")).expect("audit.yml");
        assert!(workflow.contains(&format!("cargo-deny@{CARGO_DENY_VERSION}")), "audit.yml installs cargo-deny@{CARGO_DENY_VERSION}");
        assert!(workflow.contains("cargo xtask audit"), "audit.yml runs cargo xtask audit");
    }

    /// Each npm ignore entry names a lock this audits, a GHSA id, and a reason, once.
    #[test]
    fn npm_ignores_are_well_formed() {
        let mut seen = BTreeSet::new();
        for i in NPM_IGNORED {
            assert!(NPM_LOCKS.contains(&i.lock), "{}: no lock {}", i.id, i.lock);
            assert!(i.id.starts_with("GHSA-") && i.id.len() == 19, "not a GHSA id: {}", i.id);
            assert!(!i.reason.trim().is_empty(), "{} has no reason", i.id);
            assert!(seen.insert((i.lock, i.id)), "{} is listed twice for {}", i.id, i.lock);
        }
    }

    fn report(vulnerabilities: Value) -> Value {
        json!({ "auditReportVersion": 2, "vulnerabilities": vulnerabilities, "metadata": {} })
    }

    /// The report's advisories, with their packages; the names in a `via`
    /// (a package it comes through) are not advisories.
    #[test]
    fn reads_advisories_from_the_report() {
        let r = report(json!({
            "wrangler": { "via": ["miniflare"] },
            "undici": { "via": [
                { "source": 1, "url": "https://github.com/advisories/GHSA-aaaa-bbbb-cccc" },
                { "source": 2, "url": "https://github.com/advisories/GHSA-dddd-eeee-ffff" }
            ] }
        }));
        let found = advisories(&r).unwrap();
        let want: BTreeSet<(String, String)> = [("GHSA-aaaa-bbbb-cccc", "undici"), ("GHSA-dddd-eeee-ffff", "undici")].map(|(a, b)| (a.into(), b.into())).into();
        assert_eq!(found, want);
        assert!(advisories(&report(json!({}))).unwrap().is_empty(), "a clean lock");
    }

    /// Invalid: npm's own failure (no registry), another report version, an
    /// advisory with no GHSA id. Each is an error, never a clean lock.
    #[test]
    fn refuses_what_is_not_a_report() {
        assert!(advisories(&json!({ "error": { "code": "ENOTFOUND" } })).is_err());
        assert!(advisories(&json!({ "auditReportVersion": 1, "advisories": {} })).is_err());
        let odd = report(json!({ "x": { "via": [{ "url": "https://example.com/advisory/1" }] } }));
        assert!(advisories(&odd).is_err());
    }

    /// Valid: everything found is ignored for this lock. Invalid: a new
    /// advisory; an entry nothing found; another lock's entry, which does
    /// not cover this one.
    #[test]
    fn judges_findings_against_the_ignore_list() {
        let ignored = [Ignored { lock: ".", id: "GHSA-1111-1111-1111", reason: "r" }, Ignored { lock: "other", id: "GHSA-2222-2222-2222", reason: "r" }];
        let found = |ids: &[&str]| ids.iter().map(|id| (id.to_string(), "p".to_string())).collect::<BTreeSet<_>>();
        assert_eq!(judge(".", &found(&["GHSA-1111-1111-1111"]), &ignored), Verdict { new: vec![], stale: vec![] });
        assert_eq!(
            judge(".", &found(&["GHSA-1111-1111-1111", "GHSA-2222-2222-2222"]), &ignored),
            Verdict { new: vec![("GHSA-2222-2222-2222".into(), "p".into())], stale: vec![] }
        );
        assert_eq!(judge(".", &found(&[]), &ignored), Verdict { new: vec![], stale: vec!["GHSA-1111-1111-1111".into()] });
    }
}
