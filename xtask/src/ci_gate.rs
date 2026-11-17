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
