//! Master's push is the one gate before the preview (.github/workflows/
//! ci.yml). These run two of its steps, read from the workflows, against a
//! fake `gh` that answers from canned API bodies and applies each call's
//! own `--jq`:
//!
//! - images.yml's `changes` compares a master push with the newest master
//!   commit the workflow passed on, never the push's `before`: a push
//!   whose waiting run a newer one replaced never ran, and its image
//!   change is the newer run's to test (Codex review of #1118).
//! - ci.yml's `master-red` names what failed, and says the preview keeps
//!   its last deploy only when the deploy never started.
//! - ci.yml's `check` holds the schema and wire locks to a base it names
//!   (`FRAGMENT_SCHEMA_BASE`): for a run by hand, where the branch last
//!   took master, never master's tip, which a branch merely behind it has
//!   lost nothing of.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use crate::ci_caches::step_script;

/// A run's scratch: the fake `gh` on its PATH, the bodies it answers with
/// (`answers/<path with / as _>`), the calls it was asked, and the files a
/// step writes to.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ci-gate-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("answers")).unwrap();
        let gh = dir.join("bin/gh");
        fs::write(&gh, r#"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$ASKED"
args=("$@"); expr=""; path=""
for ((i = 1; i < ${#args[@]}; i++)); do
  case "${args[i]}" in
    --jq) expr=${args[i+1]}; i=$((i + 1)) ;;
    -X | -f) i=$((i + 1)) ;;
    --paginate) ;;
    *) path=${args[i]} ;;
  esac
done
[ "${args[0]}" = api ] || { echo "unexpected: gh $*" >&2; exit 2; }
body=$(cat "$ANSWERS/${path//\//_}" 2> /dev/null) || { echo "HTTP 502: $path" >&2; exit 1; }
if [ -n "$expr" ]; then jq -r "$expr" <<< "$body"; else printf '%s\n' "$body"; fi
"#)
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Command::new("jq").arg("--version").output().is_ok_and(|o| o.status.success()), "the fake gh runs jq, which is not installed");
        Scratch { dir }
    }

    fn answer(&self, path: &str, body: Value) {
        fs::write(self.dir.join("answers").join(path.replace('/', "_")), body.to_string()).unwrap();
    }

    /// `script` run with the fake `gh` and `env`, GITHUB_OUTPUT and
    /// GITHUB_STEP_SUMMARY in the scratch.
    fn run(&self, script: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new("bash");
        cmd.args(["-c", script])
            .env("PATH", format!("{}:{}", self.dir.join("bin").display(), std::env::var("PATH").unwrap_or_default()))
            .env("ASKED", self.dir.join("asked"))
            .env("ANSWERS", self.dir.join("answers"))
            .env("GITHUB_OUTPUT", self.dir.join("output"))
            .env("GITHUB_STEP_SUMMARY", self.dir.join("summary"))
            .env("GITHUB_REPOSITORY", "owner/repo")
            .env("GITHUB_RUN_ID", "41")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("GITHUB_SHA", "c".repeat(40));
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", said(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn base_step() -> String {
    step_script("images.yml", "changes", "the newest master commit this workflow passed on")
}

/// Valid, the case the review found: while master's run of A is under
/// way, B changes the bridge and waits, and C, a change to the shell
/// alone, replaces B's waiting run. C's run compares with A, the newest
/// commit the workflow passed on (B never ran), so B's image change is in
/// its diff; the push's `before`, B, would have left it out.
#[test]
fn a_master_push_compares_with_the_newest_commit_its_workflow_passed_on() {
    let repo = Scratch::new("repo");
    let dir = repo.dir.join("git");
    fs::create_dir_all(dir.join("images/bridge")).unwrap();
    fs::create_dir_all(dir.join("cell/shell")).unwrap();
    git(&dir, &["init", "-q"]);
    let commit = |path: &str, text: &str, message: &str| {
        fs::write(dir.join(path), text).unwrap();
        git(&dir, &["add", path]);
        git(&dir, &["commit", "-qm", message]);
        git(&dir, &["rev-parse", "HEAD"])
    };
    let a = commit("images/bridge/engine.rs", "a", "A");
    let b = commit("images/bridge/engine.rs", "b", "B: the bridge");
    let c = commit("cell/shell/shell.js", "c", "C: the shell");

    let s = Scratch::new("base");
    s.answer("repos/owner/repo/actions/runs/41", json!({ "id": 41, "workflow_id": 7 }));
    // as the API lists the workflow's green master pushes, newest first:
    // A's passed, B's was replaced before it ran, C's is this one
    s.answer("repos/owner/repo/actions/workflows/7/runs", json!({ "total_count": 1, "workflow_runs": [{ "id": 40, "head_sha": a, "conclusion": "success" }] }));
    let out = s.run(&base_step(), &[]);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(s.read("output").trim(), format!("sha={a}"));
    let asked = s.read("asked");
    for want in ["-f branch=master", "-f event=push", "-f status=success", "-f per_page=1"] {
        assert!(asked.contains(want), "the runs asked for are {want}: {asked}");
    }
    let since = |base: &str| git(&dir, &["diff", "--name-only", &format!("{base}..{c}")]);
    assert!(since(&a).lines().any(|f| f == "images/bridge/engine.rs"), "C's run tests B's bridge change: {}", since(&a));
    assert!(!since(&b).lines().any(|f| f.starts_with("images/")), "the push's `before` would not: {}", since(&b));
}

/// Invalid: with no green master run to name (none listed, one without a
/// commit, a malformed one, or an API that fails), the step fails and names
/// no base, so `changes` fails and every job runs.
#[test]
fn with_no_passed_run_to_name_every_job_runs() {
    for runs in [Some(json!({ "total_count": 0, "workflow_runs": [] })), Some(json!({ "workflow_runs": [{ "id": 40 }] })), Some(json!({ "workflow_runs": [{ "head_sha": "not-a-sha" }] })), None] {
        let s = Scratch::new("none");
        s.answer("repos/owner/repo/actions/runs/41", json!({ "workflow_id": 7 }));
        if let Some(runs) = &runs {
            s.answer("repos/owner/repo/actions/workflows/7/runs", runs.clone());
        }
        let out = s.run(&base_step(), &[]);
        assert!(!out.status.success(), "{runs:?}: {}", said(&out));
        assert!(!s.read("output").contains("sha="), "{runs:?}: {}", s.read("output"));
    }
}

fn red_step() -> String {
    step_script("ci.yml", "master-red", "name what failed")
}

fn jobs(list: &[(&str, &str)]) -> Value {
    json!({ "total_count": list.len(), "jobs": list.iter().map(|(name, conclusion)| json!({ "name": name, "conclusion": conclusion, "html_url": format!("https://example.com/{}", name.replace(' ', "-")) })).collect::<Vec<_>>() })
}

const JOBS: &str = "repos/owner/repo/actions/runs/41/attempts/1/jobs";

/// Valid: a red gate (a shard failed, so `e2e` did, and the deploy never
/// started) names those jobs, and only then says the preview keeps its
/// last deploy. The step is red itself, its annotation naming them.
#[test]
fn a_red_gate_names_what_failed_and_the_preview_it_left() {
    let s = Scratch::new("skipped");
    s.answer(JOBS, jobs(&[("check", "success"), ("e2e shard 3/5", "failure"), ("e2e", "failure"), ("preview-image", "success"), ("deploy", "skipped")]));
    let out = s.run(&red_step(), &[("DEPLOY", "skipped")]);
    assert!(!out.status.success());
    let summary = s.read("summary");
    assert!(summary.starts_with("### Preview not deployed\n"), "{summary}");
    assert!(summary.contains("the preview keeps its last deploy"), "{summary}");
    assert!(summary.contains("- [e2e shard 3/5](https://example.com/e2e-shard-3/5): failure") && summary.contains("- [e2e](https://example.com/e2e): failure"), "{summary}");
    assert!(!summary.contains("[check]") && !summary.contains("[preview-image]"), "{summary}");
    assert!(said(&out).contains("::error title=Preview not deployed::e2e shard 3/5, e2e"), "{}", said(&out));
}

/// Invalid to claim: a deploy that started and did not end green either
/// rolled back to the release before (verified, and failed all the same)
/// or is stuck forward-only with this release live (rollout.rs); a
/// cancelled one, either or neither. None says the preview kept its last
/// deploy: each points at the deploy's log.
#[test]
fn a_deploy_that_started_claims_no_preview() {
    for result in ["failure", "cancelled"] {
        let s = Scratch::new(result);
        s.answer(JOBS, jobs(&[("check", "success"), ("e2e", "success"), ("preview-image", "success"), ("deploy", result)]));
        let out = s.run(&red_step(), &[("DEPLOY", result)]);
        assert!(!out.status.success());
        let summary = s.read("summary");
        assert!(summary.starts_with(&format!("### Preview deploy {result}\n")), "{summary}");
        assert!(!summary.contains("keeps its last deploy"), "{result}: {summary}");
        assert!(summary.contains("rolled back to the release before, or is stuck forward-only") && summary.contains("only its log says which"), "{summary}");
        assert!(summary.contains(&format!("- [deploy](https://example.com/deploy): {result}")), "{summary}");
    }
}

/// Invalid input: when this run's jobs cannot be read, the step still
/// says what it knows of the preview and fails, rather than stopping
/// before its summary.
#[test]
fn an_unread_job_list_still_says_what_the_preview_runs() {
    let s = Scratch::new("unread");
    let out = s.run(&red_step(), &[("DEPLOY", "skipped")]);
    assert!(!out.status.success());
    let summary = s.read("summary");
    assert!(summary.contains("### Preview not deployed") && summary.contains("this run's jobs could not be read"), "{summary}");
}

fn lock_base_step() -> String {
    step_script("ci.yml", "check", "the base the locks are held to")
}

/// A repo as GitHub serves one (any commit fetched by its id), its master
/// holding a schema lock of two steps (`m0`), then a third (`m1`); a
/// branch `work` from `m0` that changes no step; and a branch `dropper`
/// from `m0` that drops the second, released before it left master.
struct Locked {
    scratch: Scratch,
    origin: PathBuf,
    m0: String,
    m1: String,
    work: String,
    dropper: String,
}

const LOCK: &str = fragment_core::migrate::host::LOCK_PATH;

impl Locked {
    fn new(name: &str) -> Locked {
        let scratch = Scratch::new(name);
        let origin = scratch.dir.join("origin");
        fs::create_dir_all(origin.join(Path::new(LOCK).parent().unwrap())).unwrap();
        git(&origin, &["init", "-q", "-b", "master"]);
        git(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        let commit = |path: &str, text: &str, message: &str| {
            fs::write(origin.join(path), text).unwrap();
            git(&origin, &["add", path]);
            git(&origin, &["commit", "-qm", message]);
            git(&origin, &["rev-parse", "HEAD"])
        };
        let m0 = commit(LOCK, "step Computer 1 baseline aa\nstep Computer 2 keys bb\n", "two steps");
        git(&origin, &["checkout", "-qb", "work"]);
        let work = commit("notes.md", "a change that adds no step\n", "work");
        git(&origin, &["checkout", "-qb", "dropper", &m0]);
        let dropper = commit(LOCK, "step Computer 1 baseline aa\n", "a released step dropped");
        git(&origin, &["checkout", "-q", "master"]);
        let m1 = commit(LOCK, "step Computer 1 baseline aa\nstep Computer 2 keys bb\nstep Computer 3 calls cc\n", "master takes a third step");
        Locked { scratch, origin, m0, m1, work, dropper }
    }

    /// The step run in a checkout of `branch` one commit deep, as
    /// actions/checkout leaves it, for the event `event` whose base the
    /// workflow's expression gave as `base`: how it ended, and the
    /// checkout.
    fn check(&self, branch: &str, event: &str, base: &str) -> (Output, PathBuf) {
        let dir = self.scratch.dir.join(format!("checkout-{branch}-{event}"));
        git(&self.scratch.dir, &["clone", "-q", "--depth=1", "--branch", branch, &format!("file://{}", self.origin.display()), dir.to_str().unwrap()]);
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let env_file = dir.join(".git/github-env");
        let script = format!("cd '{}'\n{}", dir.display(), lock_base_step());
        let out = self.scratch.run(&script, &[("GITHUB_EVENT_NAME", event), ("GITHUB_SHA", &head), ("GITHUB_REF_NAME", branch), ("BASE", base), ("GITHUB_ENV", env_file.to_str().unwrap())]);
        (out, dir)
    }

    /// The lock at `rev` of a checkout.
    fn lock(dir: &Path, rev: &str) -> String {
        git(dir, &["show", &format!("{rev}:{LOCK}")])
    }

    /// The steps the base the step named holds, which the checkout's tree
    /// lost, as `cargo xtask check` asks (schema_lock.rs).
    fn lost(dir: &Path) -> Vec<String> {
        assert_eq!(fs::read_to_string(dir.join(".git/github-env")).unwrap(), "FRAGMENT_SCHEMA_BASE=FETCH_HEAD\n");
        fragment_core::migrate::host::lost(&Locked::lock(dir, "FETCH_HEAD"), &Locked::lock(dir, "HEAD"))
    }
}

/// Goal: a run by hand holds a branch's locks to where it last took
/// master, so a branch merely behind a step master took since passes, and
/// one that drops a step released by then still fails. Valid: `work`,
/// from master's two steps, behind its third: the step asks the API for
/// the merge base, fetches it, and the tree lost none of its steps; held
/// to master's tip, as the step held it before, it "lost" the third,
/// which it never had (run 44614825582). Invalid: `dropper`, from the
/// same commit, dropped the second step: held to the merge base, it lost
/// it. Method: ci.yml's own step, in a checkout one commit deep of a repo
/// that serves any commit by its id, as GitHub does.
#[test]
fn a_run_by_hand_holds_the_locks_to_where_its_branch_last_took_master() {
    let repo = Locked::new("by-hand");
    repo.scratch.answer(&format!("repos/owner/repo/compare/master...{}", repo.work), json!({ "merge_base_commit": { "sha": repo.m0 } }));
    let (out, dir) = repo.check("work", "workflow_dispatch", "master");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(git(&dir, &["rev-parse", "FETCH_HEAD"]), repo.m0, "the merge base, not master's tip");
    assert!(repo.scratch.read("asked").contains(&format!("compare/master...{}", repo.work)), "{}", repo.scratch.read("asked"));
    assert!(said(&out).contains(&repo.m0) && said(&out).contains("work"), "it says what it holds them to: {}", said(&out));
    assert_eq!(Locked::lost(&dir), Vec::<String>::new(), "behind master's third step, it lost none");
    // held to master's tip, as before: a step it never had reads as lost
    git(&dir, &["fetch", "-q", "--no-tags", "--depth=1", "origin", "master"]);
    let at_tip = fragment_core::migrate::host::lost(&Locked::lock(&dir, "FETCH_HEAD"), &Locked::lock(&dir, "HEAD"));
    assert!(at_tip.len() == 1 && at_tip[0].contains("step Computer 3 calls cc"), "{at_tip:?}");

    repo.scratch.answer(&format!("repos/owner/repo/compare/master...{}", repo.dropper), json!({ "merge_base_commit": { "sha": repo.m0 } }));
    let (out, dir) = repo.check("dropper", "workflow_dispatch", "master");
    assert!(out.status.success(), "{}", said(&out));
    let lost = Locked::lost(&dir);
    assert!(lost.len() == 1 && lost[0].contains("step Computer 2 keys bb"), "a step released before it left master, dropped: {lost:?}");
}

/// Goal: a push and a pull request hold the locks to what they did
/// before, and ask the API nothing. Valid: a push's base is what it moved
/// master on from (`github.event.before`), so a push that drops a step
/// its master held loses it; a pull request's is its base branch's tip,
/// which its merge commit holds.
#[test]
fn a_push_and_a_pull_request_hold_the_locks_as_before() {
    let repo = Locked::new("as-before");
    let (out, dir) = repo.check("master", "push", &repo.m0);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(git(&dir, &["rev-parse", "FETCH_HEAD"]), repo.m0, "what the push moved master on from");
    assert_eq!(Locked::lost(&dir), Vec::<String>::new());
    // a push of `dropper` over master's tip drops the step that tip held
    let (out, dir) = repo.check("dropper", "push", &repo.m1);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(git(&dir, &["rev-parse", "FETCH_HEAD"]), repo.m1);
    let lost = Locked::lost(&dir);
    assert!(lost.len() == 2 && lost[0].contains("step Computer 2 keys bb") && lost[1].contains("step Computer 3 calls cc"), "{lost:?}");
    let (out, dir) = repo.check("work", "pull_request", "master");
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(git(&dir, &["rev-parse", "FETCH_HEAD"]), repo.m1, "its base branch's tip");
    assert_eq!(repo.scratch.read("asked"), "", "neither asks the API");
}

/// Invalid: a run by hand whose merge base cannot be read (the API fails,
/// or answers none) names no base and fails, so `check` is red: never
/// held to nothing, nor quietly to master's tip.
#[test]
fn a_run_by_hand_with_no_merge_base_to_name_fails() {
    for answer in [None, Some(json!({})), Some(json!({ "merge_base_commit": null }))] {
        let repo = Locked::new("no-base");
        if let Some(answer) = &answer {
            repo.scratch.answer(&format!("repos/owner/repo/compare/master...{}", repo.work), answer.clone());
        }
        let (out, dir) = repo.check("work", "workflow_dispatch", "master");
        assert!(!out.status.success(), "{answer:?}: {}", said(&out));
        assert!(!dir.join(".git/github-env").exists(), "{answer:?}: no base named");
    }
}
