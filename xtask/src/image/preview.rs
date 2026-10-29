//! Preview follows master's image inputs, not a mutable `latest` tag.
//! The registry keeps the input-keyed tag even when BuildKit's cache is
//! evicted. Only a missing tag authorizes a build; registry errors fail.
//! The workflow passes the digest alone between jobs: a reference that
//! contains the masked account ID would be dropped as a secret output.

use super::*;
use std::path::Path;

pub(crate) const REPOSITORY: &str = "fragment-preview-computer-hermes-canary";

/// The Dockerfile's local inputs, including manifest-only workspace
/// members. The host test below holds this list to every local COPY.
/// Nothing stamps a commit, clock, cell, shell or chat into the key.
const INPUTS: &[&str] = &[
    ".dockerignore", "images/hermes", "images/bridge", "images/Cargo.toml", "images/Cargo.lock",
    "Cargo.toml", "Cargo.lock", "cli", "crates/core", "crates/nip98", "crates/proto", "crates/templates",
    "templates/blank", "templates/calories", "templates/doc", "templates/inbox", "templates/notes", "templates/releasegoal", "templates/todo",
    "crates/devstack/Cargo.toml", "crates/e2e/Cargo.toml", "crates/fakes/Cargo.toml", "xtask/Cargo.toml",
];

#[derive(Debug, serde::Serialize)]
struct Resolution {
    key: String,
    digest: Option<String>,
}

/// A probe writes the key and an existing digest, or null. `--build`
/// fills a missing tag and waits for preparation before writing a digest
/// the deploy can consume. No Worker, DNS or deploy config changes here.
pub(super) fn run(args: &[String]) -> Result<()> {
    let usage = || anyhow::anyhow!("usage: cargo xtask image preview --output <file> [--build | --dry-run] [--summary <file>]");
    let (mut output, mut build, mut dry_run, mut summary) = (None, false, false, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--output" => output = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            "--build" => build = true,
            "--dry-run" => dry_run = true,
            "--summary" => summary = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            _ => return Err(usage()),
        }
    }
    let output = output.ok_or_else(usage)?;
    anyhow::ensure!(!dry_run || (!build && summary.is_none()), "--dry-run cannot build or record a ready image summary");
    // A failed retry must not leave a prior resolution for a caller to use.
    if output.exists() { fs::remove_file(&output)?; }
    let root = devstack::repo_root();
    let key = input_key(&root)?;
    if dry_run {
        println!("preview image input key: {key}\nwould resolve {REGISTRY}/<account>/{REPOSITORY}:inputs-{key}; build only if absent, prepare before deploy, then pin both preview names at its digest (no requests made)");
        fs::write(output, serde_json::to_vec(&Resolution { key, digest: None })?)?;
        return Ok(());
    }
    let req = parse(&[
        "push".into(), "hermes".into(), "--repository".into(), REPOSITORY.into(),
        "--tag".into(), format!("inputs-{key}"),
    ], |name| std::env::var(name).ok())?;
    let tools = devstack::Tools::locate()?;
    let mut reference = existing(&req, &tools)?;
    if reference.is_none() && build {
        reference = Some(push(&req, &tools)?);
    }
    if build {
        let reference = reference.as_deref().expect("built or found the input-keyed image");
        println!("preview image inputs {key}: {reference}; waiting for Cloudflare before allowing its preview deploy (the running preview is unchanged)");
        let mut api = crate::cloudflare::Api::from_env(&req.account)?;
        crate::prepare::until_ready(&mut api, &mut crate::prepare::Wall::new(), &[reference], crate::prepare::IMAGE_LIMIT)
            .context("preview image is not ready: blocked this preview deploy before any Worker change; retry this job to reuse its input-keyed image")?;
        if let Some(path) = summary {
            let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(file, "### Preview image\n\nInputs: `{key}`\n\nDigest: `{}`\n\nReference: `{}`\n\nBoth `hermes` and `hermes-canary` use this image; production is pinned separately.\n",
                reference.split_once('@').expect("pinned").1, reference.replace(&req.account, "<account>"))?;
        }
    }
    let digest = reference.as_deref().map(|r| r.split_once('@').expect("pinned").1.to_string());
    fs::write(output, serde_json::to_vec(&Resolution { key, digest })?)?;
    Ok(())
}

/// Hash git's path, mode and blob records rather than a commit ID or
/// worktree mtimes. A dirty image input is refused: Docker must build the
/// content the key names. Git's ignored targets and caches are not inputs.
fn input_key(root: &Path) -> Result<String> {
    let status = Command::new("git").args(["status", "--porcelain", "--untracked-files=normal", "--"]).args(INPUTS).current_dir(root).output()?;
    anyhow::ensure!(status.status.success() && status.stdout.is_empty(), "preview image inputs must be committed and clean");
    let tree = Command::new("git").args(["ls-tree", "-rz", "HEAD", "--"]).args(INPUTS).current_dir(root).output()?;
    anyhow::ensure!(tree.status.success() && !tree.stdout.is_empty(), "git did not name the preview image inputs");
    Ok(fragment_core::blob::sha256_hex(&tree.stdout))
}

/// Skopeo handles the registry's authentication protocol. Listing tags
/// distinguishes absence from a credentials/network failure without
/// interpreting an error's prose. This repository already exists (it was
/// the preview canary's). Inspect the raw manifest to pin its real digest.
fn existing(req: &Request, tools: &devstack::Tools) -> Result<Option<String>> {
    skopeo_installed()?;
    let dir = devstack::repo_root().join("target/image-preview");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let auth = login(tools, &req.account, None, &["--pull"], dir.join("auth.json"))?;
    let repository = format!("{REGISTRY}/{}/{}", req.account, req.repository);
    let out = Command::new("skopeo").args(["--command-timeout", "2m", "list-tags", "--authfile"]).arg(&auth.0).arg(format!("docker://{repository}")).stdin(Stdio::null()).output()?;
    anyhow::ensure!(out.status.success(), "cannot resolve preview image tags: {}", said(&out));
    if !has_tag(&out.stdout, &req.tag)? { return Ok(None); }
    let out = Command::new("skopeo").args(["--command-timeout", "2m", "inspect", "--raw", "--authfile"]).arg(&auth.0).arg(format!("docker://{}", req.tagged())).stdin(Stdio::null()).output()?;
    anyhow::ensure!(out.status.success(), "cannot inspect preview image: {}", said(&out));
    let reference = format!("{repository}@sha256:{}", fragment_core::blob::sha256_hex(&out.stdout));
    anyhow::ensure!(pinned_ok(&reference, &req.account), "registry named no pinned preview image");
    println!("reusing {}: {reference}", req.tag);
    Ok(Some(reference))
}

fn has_tag(bytes: &[u8], tag: &str) -> Result<bool> {
    anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "preview image tag listing exceeds 4 MiB");
    let v: Value = serde_json::from_slice(bytes)?;
    let tags = v["Tags"].as_array().context("registry tag listing has no Tags array")?;
    anyhow::ensure!(tags.len() <= 16_384, "preview registry exceeds 16384 tags; archive old input tags before adding more");
    anyhow::ensure!(tags.iter().all(Value::is_string), "registry tag listing has a non-string tag");
    Ok(tags.iter().any(|t| t.as_str() == Some(tag)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every local COPY is covered. Stage copies and inline heredocs are
    /// already content of the Dockerfile; remote ADDs are checksum-pinned.
    #[test]
    fn the_key_covers_the_dockerfiles_local_inputs() {
        let text = fs::read_to_string(devstack::repo_root().join("images/hermes/Dockerfile")).unwrap();
        for line in text.lines().filter_map(|l| l.strip_prefix("COPY ")) {
            if line.contains("--from=") || line.contains("<<") { continue; }
            let fields: Vec<_> = line.split_whitespace().collect();
            for field in &fields[..fields.len() - 1] {
                assert!(!field.starts_with('-'), "new COPY syntax needs review: {line}");
                assert!(INPUTS.iter().any(|p| field == p || field.starts_with(&format!("{p}/"))), "input key lacks {field}");
            }
        }
        for line in text.lines().filter(|l| l.starts_with("ADD ")) {
            assert!(line.contains("--checksum=sha256:") && line.contains("https://"), "local ADD or unpinned remote inputs need review: {line}");
        }
        for path in ["cell/src/lib.rs", "cell/shell/index.html", "templates/chat/index.html", "xtask/src/main.rs", "crates/e2e/src/main.rs"] {
            assert!(!INPUTS.iter().any(|p| path == *p || path.starts_with(&format!("{p}/"))), "not an image input: {path}");
        }
    }

    /// Absence is a successful listing without the tag, never malformed
    /// output, which could be a registry outage or an authentication error.
    #[test]
    fn only_a_valid_listing_can_authorize_a_build() {
        assert!(has_tag(br#"{"Tags":["inputs-abc","r12"]}"#, "inputs-abc").unwrap());
        assert!(!has_tag(br#"{"Tags":["r12"]}"#, "inputs-abc").unwrap());
        assert!(!has_tag(br#"{"Tags":[]}"#, "inputs-abc").unwrap());
        for bad in [br#"{}"#.as_slice(), br#"{"Tags":null}"#, br#"{"Tags":[1]}"#, b"error"] {
            assert!(has_tag(bad, "inputs-abc").is_err());
        }
    }

    /// A new commit, dirty unrelated files, or a restarted process cannot
    /// change identity. A changed input does; reverting its content reuses
    /// the old key. Dirty inputs must never be built under a committed key.
    #[test]
    fn identity_is_inputs_not_the_commit_or_build_cache() {
        let root = std::env::temp_dir().join(format!("preview-inputs-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(root.join("cli")).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"]).args(args).current_dir(&root).output().unwrap();
            assert!(out.status.success(), "{}", said(&out));
        };
        git(&["init", "-q"]);
        fs::write(root.join("cli/main.rs"), "first").unwrap();
        git(&["add", "cli"]);
        git(&["commit", "-qm", "first"]);
        let first = input_key(&root).unwrap();
        fs::write(root.join("cell.txt"), "not in image").unwrap();
        assert_eq!(input_key(&root).unwrap(), first);
        git(&["add", "cell.txt"]);
        git(&["commit", "-qm", "cell changed"]);
        assert_eq!(input_key(&root).unwrap(), first);
        fs::write(root.join("cli/main.rs"), "second").unwrap();
        assert!(input_key(&root).is_err());
        git(&["add", "cli"]);
        git(&["commit", "-qm", "CLI changed"]);
        assert_ne!(input_key(&root).unwrap(), first);
        fs::write(root.join("cli/main.rs"), "first").unwrap();
        git(&["add", "cli"]);
        git(&["commit", "-qm", "CLI restored"]);
        assert_eq!(input_key(&root).unwrap(), first);
        fs::write(root.join("cli/untracked.rs"), "not keyed").unwrap();
        assert!(input_key(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
