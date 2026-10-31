//! Master's `prune` (.github/workflows/ci.yml) deletes every cache entry a
//! pull request's or a merge group's ref holds: such a ref's cache is its
//! own, which no other run reads, and every save is master's. These run
//! that very step, read from ci.yml, against a fake `gh` whose listing
//! holds every kind of ref, and hold it to deleting those refs' entries
//! alone: never master's, production's, a tag's or another branch's.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use fragment_devstack as devstack;
use serde_json::json;

const STEP: &str = "- name: delete pull requests' and merge groups' caches";

/// The step's `run` script as ci.yml has it, its indentation removed.
fn step_script() -> String {
    let text = fs::read_to_string(devstack::repo_root().join(".github/workflows/ci.yml")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let prune = lines.iter().position(|l| *l == "  prune:").expect("ci.yml has `prune`");
    let at = lines.iter().position(|l| l.trim() == STEP).expect("ci.yml has the step");
    assert!(at > prune && !lines[prune + 1..at].iter().any(|l| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':')), "the step is prune's");
    let run = (at..lines.len()).find(|&i| lines[i].trim() == "run: |").expect("the step runs a script");
    let indent = |l: &str| l.len() - l.trim_start().len();
    let body: Vec<&str> = lines[run + 1..].iter().copied().take_while(|l| l.trim().is_empty() || indent(l) > indent(lines[run])).collect();
    let strip = body.iter().filter(|l| !l.trim().is_empty()).map(|l| indent(l)).min().expect("a script");
    body.iter().map(|l| l.get(strip..).unwrap_or("")).collect::<Vec<_>>().join("\n") + "\n"
}

/// The step run with a fake `gh` that lists `pages` (as `gh api
/// --paginate` prints them, one JSON document a page) or, given none,
/// fails to list; what it said, and the deletions it asked for.
fn run_step(pages: &[serde_json::Value]) -> (Output, Vec<String>) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("ci-caches-{}-{nanos}", std::process::id()));
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let listing: PathBuf = dir.join("listing.json");
    fs::write(&listing, pages.iter().map(|p| p.to_string() + "\n").collect::<String>()).unwrap();
    let deleted = dir.join("deleted");
    let gh = bin.join("gh");
    fs::write(&gh, r#"#!/usr/bin/env bash
set -eu
case " $* " in
  *" -X GET "*)
    [[ " $* " == *" --paginate "* ]] || { echo "the listing must page" >&2; exit 3; }
    [ "$FAIL_LISTING" = 0 ] || { echo "HTTP 502" >&2; exit 1; }
    cat "$LISTING" ;;
  *" -X DELETE "*) echo "${@: -1}" >> "$DELETED" ;;
  *) echo "unexpected: gh $*" >&2; exit 2 ;;
esac
"#).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Command::new("jq").arg("--version").output().is_ok_and(|o| o.status.success()), "the step runs jq, which is not installed");
    let out = Command::new("bash")
        .args(["-c", &step_script()])
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()))
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("LISTING", &listing)
        .env("DELETED", &deleted)
        .env("FAIL_LISTING", if pages.is_empty() { "1" } else { "0" })
        .output()
        .unwrap();
    let asked = fs::read_to_string(&deleted).unwrap_or_default().lines().map(str::to_string).collect();
    fs::remove_dir_all(dir).unwrap();
    (out, asked)
}

fn page(refs: &[(u32, &str)]) -> serde_json::Value {
    json!({ "total_count": refs.len(), "actions_caches": refs.iter().map(|(id, r)| json!({ "id": id, "ref": r, "key": format!("key-{id}"), "size_in_bytes": 1 })).collect::<Vec<_>>() })
}

/// Valid: a pull request's merge ref and a merge group's queue ref, on
/// either page, deleted by id. Kept, whatever else the listing holds:
/// master, production, another branch, a tag, a pull request's head, and
/// refs that only look like the deleted ones.
#[test]
fn prune_deletes_pull_requests_and_merge_groups_caches_alone() {
    let pages = [
        page(&[(1, "refs/heads/master"), (2, "refs/pull/823/merge"), (3, "refs/heads/production"), (4, "refs/heads/gh-readonly-queue/master/pr-830-d0b6c3aa")]),
        page(&[
            (5, "refs/heads/claude/a-branch"),
            (6, "refs/tags/v1"),
            (7, "refs/pull/12/head"),
            (8, "refs/pull/x/merge"),
            (9, "refs/pull/7/merge/more"),
            (10, "refs/heads/gh-readonly-queue-x/y"),
            (11, "refs/heads/master/gh-readonly-queue/z"),
            (12, "refs/pull/9/merge"),
        ]),
    ];
    let (out, asked) = run_step(&pages);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(asked, ["repos/owner/repo/actions/caches/2", "repos/owner/repo/actions/caches/4", "repos/owner/repo/actions/caches/12"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("deleted: 2 refs/pull/823/merge key-2") && said.contains("deleted: 4 refs/heads/gh-readonly-queue/master/pr-830-d0b6c3aa key-4"), "{said}");
}

/// Invalid: nothing deletable is no deletion and a green step; a listing
/// that fails fails the step, and deletes nothing.
#[test]
fn prune_deletes_nothing_it_cannot_name() {
    let (out, asked) = run_step(&[page(&[(1, "refs/heads/master"), (2, "refs/heads/production")])]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(asked.is_empty(), "{asked:?}");
    let (out, asked) = run_step(&[]);
    assert!(!out.status.success(), "a failed listing fails the step");
    assert!(asked.is_empty(), "{asked:?}");
}
