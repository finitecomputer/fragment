//! Exercise the actual record command with stdout redirected exactly as
//! release.yml does. gh's successful edit prints a URL, not key=value.
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::{Command, Stdio}};
use serde_json::json;

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn record_stdout_is_an_actions_output_file_for_fresh_existing_and_repaired_releases() {
    let dir = std::env::temp_dir().join(format!("cli-release-output-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // A fixed committed input tree lets this command-level test run even
    // while CLI sources are being edited. Git hashing has its own tests.
    let tree = b"100644 blob 1234567890123456789012345678901234567890\tcli/main.rs\0";
    fs::write(dir.join("tree"), tree).unwrap();
    let key = fragment_core::blob::sha256_hex(tree);
    executable(&dir.join("git"), r#"#!/bin/sh
set -eu
case "$1" in
  status) ;;
  ls-tree) cat "$FAKE_GH_DIR/tree" ;;
  *) exit 23 ;;
esac
"#);
    executable(&dir.join("gh"), r#"#!/bin/sh
set -eu
printf '%s\n' "$1 $2" >> "$FAKE_GH_DIR/calls"
case "$1 $2" in
  'api repos/finitecomputer/fragment/releases/tags/v0.15.1')
    if [ -f "$FAKE_GH_DIR/edited" ]; then
      cat "$FAKE_GH_DIR/marked.json"
    elif [ "$FAKE_GH_CASE" = repair ] || [ "$FAKE_GH_CASE" = repair-fails ]; then
      if [ -f "$FAKE_GH_DIR/uploaded" ]; then cat "$FAKE_GH_DIR/fresh.json"; else cat "$FAKE_GH_DIR/partial.json"; fi
    elif [ "$FAKE_GH_CASE" = existing ]; then cat "$FAKE_GH_DIR/marked.json"
    else cat "$FAKE_GH_DIR/fresh.json"; fi ;;
  'release edit')
    test "$6" = --notes-file
    grep -q 'fragment-cli-inputs:' "$7"
    touch "$FAKE_GH_DIR/edited"
    printf '%s\n' 'https://github.com/finitecomputer/fragment/releases/tag/v0.15.1' ;;
  'release upload')
    printf '%s\n' 'Uploaded fragment-Linux-x86_64.tar.gz'
    if [ "$FAKE_GH_CASE" = repair-fails ]; then echo 'fake upload failure' >&2; exit 1; fi
    touch "$FAKE_GH_DIR/uploaded" ;;
  *) echo 'unexpected gh invocation' >&2; exit 24 ;;
esac
"#);
    let assets: Vec<_> = ["Darwin-arm64", "Darwin-x86_64", "Linux-x86_64", "Linux-aarch64"].iter()
        .map(|a| json!({"name": format!("fragment-{a}.tar.gz"), "size": 123, "state": "uploaded"})).collect();
    let fresh = json!({"tag_name": "v0.15.1", "draft": false, "prerelease": false, "published_at": "2026-10-30", "body": "Notes", "assets": assets});
    let mut marked = fresh.clone(); marked["body"] = json!(format!("Notes\n\n<!-- fragment-cli-inputs:{key} -->\n"));
    let mut partial = fresh.clone(); partial["assets"].as_array_mut().unwrap().remove(2);
    for (name, release) in [("fresh", fresh), ("marked", marked), ("partial", partial)] {
        fs::write(dir.join(format!("{name}.json")), release.to_string()).unwrap();
    }
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&inherited))).unwrap();
    for case in ["fresh", "existing", "repair", "repair-fails"] {
        for state in ["edited", "uploaded", "calls"] { let _ = fs::remove_file(dir.join(state)); }
        let output_file = dir.join("GITHUB_OUTPUT");
        let out = Command::new(env!("CARGO_BIN_EXE_xtask")).args(["cli-release", "record", "v0.15.1"])
            .env("PATH", &path).env("FAKE_GH_DIR", &dir).env("FAKE_GH_CASE", case)
            .env("GITHUB_REPOSITORY", "finitecomputer/fragment").env("GITHUB_OUTPUT", &output_file)
            .stdout(Stdio::from(fs::File::create(&output_file).unwrap())).output().unwrap();
        let output = fs::read_to_string(&output_file).unwrap();
        let calls = fs::read_to_string(dir.join("calls")).unwrap();
        assert!(output.lines().all(|l| l.split_once('=').is_some_and(|(k, v)| k == "version" && v == "0.15.1")), "invalid Actions output for {case}: {output:?}");
        if case == "repair-fails" {
            assert!(!out.status.success());
            assert!(output.is_empty(), "a failed upload must advertise no version");
            assert!(String::from_utf8_lossy(&out.stderr).contains("fake upload failure"));
        } else {
            assert!(out.status.success(), "{case}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(output, "version=0.15.1\n", "{case}");
        }
        assert_eq!(calls.lines().filter(|l| *l == "release upload").count(), usize::from(case.starts_with("repair")));
        assert_eq!(calls.lines().filter(|l| *l == "release edit").count(), usize::from(matches!(case, "fresh" | "repair")));
    }
    fs::remove_dir_all(dir).unwrap();
}
