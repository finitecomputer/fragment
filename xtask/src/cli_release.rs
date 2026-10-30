//! A production push releases only new CLI inputs, before deploying. The
//! published release's body is the completed-build record; tags alone (or
//! drafts, partial uploads) never authorize an update notice.
use std::{fs, process::Command};
use anyhow::{Context, Result};
use fragment_core::release::cli_version;
use serde_json::Value;

const INPUTS: &[&str] = &[
    "Cargo.toml", "Cargo.lock", "cli", "crates/core", "crates/nip98", "crates/proto", "crates/templates",
    "templates/blank", "templates/calories", "templates/doc", "templates/inbox", "templates/notes", "templates/todo",
    "crates/devstack/Cargo.toml", "crates/e2e/Cargo.toml", "crates/fakes/Cargo.toml", "xtask/Cargo.toml",
    ".github/workflows/release.yml",
];

fn marker(key: &str) -> String { format!("<!-- fragment-cli-inputs:{key} -->") }

fn missing_assets(release: &Value) -> Vec<String> {
    crate::RELEASE_ASSETS.iter().map(|(_, asset)| format!("fragment-{asset}.tar.gz"))
        .filter(|name| !release["assets"].as_array().into_iter().flatten().any(|a|
            a["name"] == *name && a["size"].as_u64().is_some_and(|s| s > 0) && a["state"] == "uploaded"))
        .collect()
}

/// GitHub has reported a successful upload while omitting a tarball
/// (v0.15.0, run 41070578493). Repair only missing/incomplete assets once,
/// then read back and fail rather than recording an incomplete release.
fn complete_assets(release: Value, repair: impl FnOnce(&[String]) -> Result<Value>) -> Result<Value> {
    let missing = missing_assets(&release);
    let release = if missing.is_empty() { release } else { repair(&missing)? };
    let missing = missing_assets(&release);
    anyhow::ensure!(missing.is_empty(), "CLI release still lacks uploaded nonempty assets after one repair: {}", missing.join(", "));
    Ok(release)
}

fn published(release: &Value, key: &str) -> Option<[u64; 3]> {
    if release["draft"] != false || release["prerelease"] != false || !release["published_at"].is_string()
        || !missing_assets(release).is_empty() || !release["body"].as_str()?.lines().any(|l| l == marker(key)) { return None; }
    cli_version(release["tag_name"].as_str()?.strip_prefix('v')?)
}

fn version_text(version: [u64; 3]) -> String { format!("{}.{}.{}", version[0], version[1], version[2]) }

fn next_version(floor: &str, tags: &Value) -> Result<String> {
    let floor = cli_version(floor).context("cli/Cargo.toml must name a stable major.minor.patch version")?;
    let top = tags.as_array().context("GitHub must list tag pages")?.iter().flat_map(|p| p.as_array().into_iter().flatten())
        .filter_map(|t| t["ref"].as_str()?.strip_prefix("refs/tags/v").and_then(cli_version)).max();
    let next = match top {
        Some([a, b, c]) => [a, b, c.checked_add(1).context("CLI patch version overflow")?],
        None => floor,
    };
    Ok(version_text(next.max(floor)))
}

/// gh handles authentication and pagination. A failed lookup stops the
/// release job; it never masquerades as missing inputs or a missing tag.
fn api(repo: &str, path: &str, pages: bool) -> Result<Value> {
    let mut cmd = Command::new("gh");
    cmd.args(["api", &format!("repos/{repo}/{path}")]);
    if pages { cmd.args(["--paginate", "--slurp"]); }
    let out = cmd.output().context("start gh api")?;
    anyhow::ensure!(out.status.success(), "GitHub release lookup failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let root = crate::devstack::repo_root();
    let key = crate::inputs::key(&root, INPUTS)?;
    let repo = std::env::var("GITHUB_REPOSITORY").context("GITHUB_REPOSITORY is required")?;
    match args {
        [plan] if plan == "plan" => {
            let releases = api(&repo, "releases?per_page=100", true)?;
            let found = releases.as_array().context("GitHub must list release pages")?.iter()
                .flat_map(|p| p.as_array().into_iter().flatten()).filter_map(|r| published(r, &key)).max();
            let version = match found {
                Some(version) => version_text(version),
                None => {
                    let manifest = fs::read_to_string(root.join("cli/Cargo.toml"))?;
                    let floor = manifest.lines().find_map(|l| l.strip_prefix("version = \"")?.strip_suffix('"')).context("CLI version floor")?;
                    next_version(floor, &api(&repo, "git/matching-refs/tags/v?per_page=100", true)?)?
                }
            };
            println!("key={key}\ntag=v{version}\npublish={}", found.is_none());
            // Only an already published, verified release is advertised.
            if found.is_some() { println!("version={version}"); }
            eprintln!("CLI inputs {key}: {} v{version}", if found.is_some() { "reuse" } else { "build" });
        }
        [record, tag] if record == "record" => {
            anyhow::ensure!(tag.strip_prefix('v').and_then(cli_version).is_some(), "release tag must be v<major.minor.patch>");
            let release = api(&repo, &format!("releases/tags/{tag}"), false)?;
            anyhow::ensure!(release["draft"] == false && release["prerelease"] == false && release["published_at"].is_string(), "CLI release is not published and stable");
            let release = complete_assets(release, |missing| {
                eprintln!("repairing CLI release assets once: {}", missing.join(", "));
                let assets = missing.iter().map(|name| root.join("dist").join(name));
                crate::run(Command::new("gh").args(["release", "upload", tag, "--repo", &repo, "--clobber"]).args(assets))?;
                api(&repo, &format!("releases/tags/{tag}"), false)
            })?;
            let body = release["body"].as_str().unwrap_or_default();
            let mark = marker(&key);
            anyhow::ensure!(!body.lines().any(|l| l.starts_with("<!-- fragment-cli-inputs:") && l != mark), "release already records different CLI inputs");
            if !body.lines().any(|l| l == mark) {
                let notes = root.join("target/cli-release-notes.md");
                fs::create_dir_all(notes.parent().expect("target"))?;
                fs::write(&notes, format!("{body}\n\n{mark}\n"))?;
                crate::run(Command::new("gh").args(["release", "edit", tag, "--repo", &repo, "--notes-file"]).arg(&notes))?;
            }
            let verified = api(&repo, &format!("releases/tags/{tag}"), false)?;
            let version = published(&verified, &key).context("published CLI release did not verify after recording its inputs")?;
            println!("version={}", version_text(version));
        }
        _ => anyhow::bail!("usage: cargo xtask cli-release plan | record <tag>"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_asset_is_reuploaded_once_and_still_missing_fails_the_job() {
        let assets: Vec<_> = crate::RELEASE_ASSETS.iter().map(|(_, a)| json!({"name": format!("fragment-{a}.tar.gz"), "size": 123, "state": "uploaded"})).collect();
        let complete = json!({"assets": assets});
        assert!(complete_assets(complete.clone(), |_| panic!("a complete release uploads nothing")).is_ok());
        let mut partial = complete.clone();
        partial["assets"].as_array_mut().unwrap().retain(|a| a["name"] != "fragment-Linux-x86_64.tar.gz");
        assert!(complete_assets(partial.clone(), |missing| {
            assert_eq!(missing, ["fragment-Linux-x86_64.tar.gz"]);
            Ok(complete.clone())
        }).is_ok());
        let mut tries = 0;
        let failed = complete_assets(partial.clone(), |_| { tries += 1; Ok(partial) });
        assert_eq!(tries, 1);
        assert!(failed.unwrap_err().to_string().contains("fragment-Linux-x86_64.tar.gz"));
        let mut empty = complete.clone(); empty["assets"][0]["size"] = json!(0);
        assert!(complete_assets(empty, |missing| {
            assert_eq!(missing, ["fragment-Darwin-arm64.tar.gz"]);
            Ok(complete)
        }).is_ok());
    }

    #[test]
    fn only_a_complete_published_release_matches_the_inputs() {
        let assets: Vec<_> = crate::RELEASE_ASSETS.iter().map(|(_, a)| json!({"name": format!("fragment-{a}.tar.gz"), "size": 123, "state": "uploaded"})).collect();
        let release = json!({"tag_name": "v0.15.10", "draft": false, "prerelease": false, "published_at": "2026-10-30", "body": format!("Notes\n\n{}\n", marker("key")), "assets": assets});
        assert_eq!(published(&release, "key"), Some([0, 15, 10]));
        assert_eq!(published(&release, "another"), None);
        for field in ["draft", "prerelease"] {
            let mut bad = release.clone(); bad[field] = json!(true);
            assert_eq!(published(&bad, "key"), None);
        }
        for field in ["published_at", "assets", "body", "tag_name"] {
            let mut bad = release.clone(); bad[field] = Value::Null;
            assert_eq!(published(&bad, "key"), None);
        }
        for bad_asset in [json!([]), json!([{"name": "fragment-Linux-x86_64.tar.gz", "size": 0}])] {
            let mut bad = release.clone(); bad["assets"] = bad_asset;
            assert_eq!(published(&bad, "key"), None);
        }
        for index in 0..4 {
            let mut missing = release.clone();
            missing["assets"].as_array_mut().unwrap().remove(index);
            assert_eq!(published(&missing, "key"), None);
            for (field, value) in [("size", json!(0)), ("state", json!("new")), ("name", json!("unrelated-artifact"))] {
                let mut bad = release.clone(); bad["assets"][index][field] = value;
                assert_eq!(published(&bad, "key"), None);
            }
        }
    }

    #[test]
    fn versions_advance_above_all_tags_with_the_manifest_as_floor() {
        let tags = json!([[{"ref":"refs/tags/v0.15.9"},{"ref":"refs/tags/v0.15.10"},{"ref":"refs/tags/v99.0.0-rc1"}], [{"ref":"refs/tags/cli-inputs-abc"}]]);
        assert_eq!(next_version("0.15.0", &tags).unwrap(), "0.15.11");
        assert_eq!(next_version("0.16.0", &tags).unwrap(), "0.16.0");
        assert_eq!(next_version("0.15.0", &json!([[]])).unwrap(), "0.15.0");
        for invalid in ["", "v0.15.0", "0.15", "0.15.0.1", "0.15.0-rc1", "0.01.0", "0.15.18446744073709551616", "0.15.💥"] {
            assert!(cli_version(invalid).is_none(), "{invalid}");
        }
    }

    /// The existing Dockerfile/metadata test holds its CLI stage to the
    /// dependency graph and all scaffolds. Hold this key to that same set.
    #[test]
    fn the_key_covers_the_cli_build_and_its_release_workflow() {
        let text = fs::read_to_string(crate::devstack::repo_root().join("images/hermes/Dockerfile")).unwrap();
        let stage = text.lines().skip_while(|l| !l.ends_with(" AS cli")).skip(1).take_while(|l| !l.starts_with("FROM "));
        for line in stage.filter_map(|l| l.strip_prefix("COPY ")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            for path in &fields[..fields.len() - 1] { assert!(INPUTS.contains(path), "CLI key lacks {path}"); }
        }
        assert!(INPUTS.contains(&".github/workflows/release.yml"));
        for path in ["cell/src", "cell/shell", "templates/chat", "xtask/src", "crates/e2e/src", "docs"] { assert!(!INPUTS.contains(&path)); }
    }
}
