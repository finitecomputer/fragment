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
    "templates/blank", "templates/calories", "templates/doc", "templates/inbox", "templates/notes", "templates/todo",
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
    crate::inputs::key(root, INPUTS)
}

/// Inspect this one tag, never list the repository: Cloudflare rejects
/// skopeo's paginated tag-list query (run 40933094816). Only a definite
/// missing manifest permits a build; all other failures stop the job.
fn existing(req: &Request, tools: &devstack::Tools) -> Result<Option<String>> {
    skopeo_installed()?;
    let dir = devstack::repo_root().join("target/image-preview");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let auth = login(tools, &req.account, None, &["--pull"], dir.join("auth.json"))?;
    let repository = format!("{REGISTRY}/{}/{}", req.account, req.repository);
    let out = Command::new("skopeo").args(["--command-timeout", "2m", "inspect", "--raw", "--authfile"]).arg(&auth.0).arg(format!("docker://{}", req.tagged())).stdin(Stdio::null()).output()?;
    let Some(digest) = inspect_answer(&req.tagged(), out.status.success(), &out.stdout, &out.stderr)? else { return Ok(None); };
    let reference = format!("{repository}@{digest}");
    anyhow::ensure!(pinned_ok(&reference, &req.account), "registry named no pinned preview image");
    println!("reusing {}: {reference}", req.tag);
    Ok(Some(reference))
}

/// Skopeo reports registry error codes in its fatal diagnostic. Decode
/// logrus's quoted msg, then require the exact tag/repository context and
/// a MANIFEST_UNKNOWN diagnostic or explicit manifest HTTP 404. A new or
/// ambiguous diagnostic fails closed, never authorizes a cold rebuild.
fn inspect_answer(tagged: &str, success: bool, manifest: &[u8], stderr: &[u8]) -> Result<Option<String>> {
    if success {
        anyhow::ensure!(!manifest.is_empty(), "registry returned an empty preview manifest for {tagged}");
        return Ok(Some(format!("sha256:{}", fragment_core::blob::sha256_hex(manifest))));
    }
    let diagnostic = String::from_utf8_lossy(stderr);
    let last = diagnostic.trim().lines().last().unwrap_or_default();
    let message = match last.split_once("level=fatal msg=") {
        Some((_, quoted)) => serde_json::from_str::<String>(quoted).unwrap_or_else(|_| last.to_string()),
        None => last.to_string(),
    };
    let (repository, tag) = tagged.rsplit_once(':').context("preview lookup must name a tag")?;
    let context = format!("reading manifest {tag} in {repository}: ");
    let cause = [format!("Error parsing image name \"docker://{tagged}\": "), format!("parsing image name \"docker://{tagged}\": "), "retrieving manifest for image: ".into()]
        .iter().find_map(|prefix| message.strip_prefix(prefix).and_then(|m| m.strip_prefix(&context)));
    if manifest.is_empty() && cause.is_some_and(|c| c == "manifest unknown" || c.starts_with("manifest unknown: ") || c.starts_with("StatusCode: 404, ")) {
        return Ok(None);
    }
    bail!("cannot inspect preview image {tagged}: {}", diagnostic.trim())
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

    /// images.yml's Docker lane runs on a change to any input of the image
    /// (#715): each is in its `docker` filter, and in the paths that start
    /// the workflow on a push and a pull request (`images/**` covers images/).
    #[test]
    fn the_docker_lane_watches_every_input() {
        let root = devstack::repo_root();
        let workflow = fs::read_to_string(root.join(".github/workflows/images.yml")).unwrap();
        let starts: Vec<_> = workflow.lines().filter(|l| l.trim_start().starts_with("paths: [")).collect();
        assert_eq!(starts.len(), 2, "a push's paths and a pull request's");
        for input in INPUTS {
            let glob = if root.join(input).is_dir() { format!("{input}/**") } else { input.to_string() };
            assert!(workflow.lines().any(|l| l.trim() == format!("- {glob}")), "the docker filter lacks {glob}");
            for paths in &starts {
                assert!(paths.contains(&format!("\"{glob}\"")) || (input.starts_with("images/") && paths.contains("\"images/**\"")), "a start's paths lack {glob}: {paths}");
            }
        }
    }

    /// Found manifests keep their actual bytes' digest. Only an explicit
    /// absence at the requested manifest authorizes building; auth,
    /// transport, server, malformed and unrelated 404 errors do not.
    #[test]
    fn direct_manifest_lookup_distinguishes_absence_from_failure() {
        let tagged = "registry.cloudflare.com/account/repo:inputs-abc";
        let manifest = br#"{"schemaVersion":2,"layers":[]}"#;
        assert_eq!(inspect_answer(tagged, true, manifest, b"").unwrap(), Some(format!("sha256:{}", fragment_core::blob::sha256_hex(manifest))));
        assert!(inspect_answer(tagged, true, b"", b"").is_err());
        let context = format!("Error parsing image name \"docker://{tagged}\": reading manifest inputs-abc in registry.cloudflare.com/account/repo: ");
        let logged = |message: &str| format!("time=\"2026-10-29T00:00:00Z\" level=fatal msg={}", serde_json::to_string(message).unwrap());
        for missing in ["manifest unknown", "manifest unknown: manifest unknown", "manifest unknown: The named manifest is not known to the registry.", "StatusCode: 404, Not Found"] {
            let message = format!("{context}{missing}");
            for diagnostic in [message.clone(), logged(&message)] {
                assert_eq!(inspect_answer(tagged, false, b"", diagnostic.as_bytes()).unwrap(), None, "{diagnostic}");
            }
        }
        for error in ["unauthorized: authentication required", "denied: access forbidden", "StatusCode: 429, Too Many Requests", "StatusCode: 500, Internal Server Error", "context deadline exceeded", "unknown: manifest unknown", "unknown: 404 Not Found"] {
            assert!(inspect_answer(tagged, false, b"", logged(&format!("{context}{error}")).as_bytes()).is_err(), "{error}");
        }
        for error in ["manifest unknown", "404 Not Found", "pinging registry: StatusCode: 404, Not Found", "malformed response"] {
            assert!(inspect_answer(tagged, false, b"", error.as_bytes()).is_err(), "{error}");
        }
        let missing = logged(&format!("{context}manifest unknown"));
        assert!(inspect_answer("registry.cloudflare.com/account/repo:another-tag", false, b"", missing.as_bytes()).is_err());
        assert!(inspect_answer(tagged, false, manifest, missing.as_bytes()).is_err());
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
