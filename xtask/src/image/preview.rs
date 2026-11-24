//! Preview follows master's image inputs, not a mutable `latest` tag:
//! each image it runs (image.rs, `IMAGES`: Hermes and the SimpleX door) by
//! its own input key, in its own preview repository (`Image::preview`).
//! The registry keeps the input-keyed tag even when BuildKit's cache is
//! evicted. Only a missing tag authorizes a build; registry errors fail.
//! The workflow passes digests alone between jobs: a reference that
//! contains the masked account ID would be dropped as a secret output.
//! Master's image job, one at a time, is the only writer of an input tag.

use super::*;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, serde::Serialize)]
struct Resolution {
    key: String,
    digest: Option<String>,
    /// Whether resolving needs an image built: master's input tag is not in
    /// the registry (a dry run asks nothing: true).
    build: bool,
}

/// A probe writes, for each image by name, its key, an existing input
/// tag's digest or null, and whether a build is needed. `--build` reuses
/// each input tag, or builds it, and waits for every image's preparation
/// before writing the digests the deploy can consume. No Worker, DNS or
/// deploy config changes here.
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
    anyhow::ensure!(!(build && dry_run), "--build and --dry-run are two ways to run: one at a time");
    anyhow::ensure!(!dry_run || summary.is_none(), "--dry-run cannot build or record a ready image summary");
    // A failed retry must not leave a prior resolution for a caller to use.
    if output.exists() { fs::remove_file(&output)?; }
    let keys = input_keys(&devstack::repo_root())?;
    if dry_run {
        let mut plan = BTreeMap::new();
        for (image, key) in keys {
            println!("preview image {} input key: {key}\nwould resolve {REGISTRY}/<account>/{}:inputs-{key}, and build it only if it is not there", image.name, image.preview);
            plan.insert(image.name, Resolution { key, digest: None, build: true });
        }
        println!("then prepare every image before the deploy, which pins each at its digest (no requests made)");
        fs::write(output, serde_json::to_vec(&plan)?)?;
        return Ok(());
    }
    let tools = devstack::Tools::locate()?;
    // every image's input tag looked up before any is built: a registry
    // that fails one lookup builds nothing
    let mut found = vec![];
    for (image, key) in keys {
        let inputs = tagged(image, &format!("inputs-{key}"))?;
        let reference = existing(&inputs, &tools)?;
        found.push(Found { image, build: reference.is_none(), key, inputs, reference });
    }
    if build {
        for f in found.iter_mut().filter(|f| f.reference.is_none()) {
            // built under a tag of its own, and named by its inputs only once
            // it fits the disk budget (disk.rs): a retry after a refusal finds
            // no input-keyed image, so it builds and measures again, never
            // reusing one unmeasured (Codex on #1422)
            let built = push(&tagged(f.image, &format!("unchecked-{}", f.key))?, &tools)?;
            super::disk::check(&built)?;
            f.reference = Some(named(&f.inputs, &built, &tools)?);
        }
        let references: Vec<&str> = found.iter().map(|f| f.reference.as_deref().expect("reused or built each input-keyed image")).collect();
        println!("preview images {}; waiting for Cloudflare before allowing their preview deploy (the running preview is unchanged)", references.join(", "));
        prepare(&found[0].inputs.account, &references)?;
        if let Some(path) = summary {
            let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
            for Found { image, key, inputs, reference, .. } in &found {
                let reference = reference.as_deref().expect("ready");
                let uses = match image.runs {
                    Runs::Computers => format!("`{}` uses this image", image.name),
                    Runs::Doors => "The SimpleX doors use this image".to_string(),
                };
                writeln!(file, "### Preview image `{}`\n\nInputs: `{key}`\n\nDigest: `{}`\n\nReference: `{}`\n\n{uses}; production is pinned separately.\n",
                    image.name, reference.split_once('@').expect("pinned").1, reference.replace(&inputs.account, "<account>"))?;
            }
        }
    }
    let written: BTreeMap<_, _> = found.into_iter().map(|f| {
        let digest = f.reference.as_deref().map(|r| r.split_once('@').expect("pinned").1.to_string());
        (f.image.name, Resolution { key: f.key, digest, build: f.build })
    }).collect();
    fs::write(output, serde_json::to_vec(&written)?)?;
    Ok(())
}

/// One image's resolution as a run makes it: its key, the push its input
/// tag names, the image there (or built), and whether it had to be built.
struct Found {
    image: &'static Image,
    key: String,
    inputs: Request,
    reference: Option<String>,
    build: bool,
}

/// `image`'s push to its preview repository under `tag`, checked as any
/// push is (image.rs, `parse`), the account from the environment.
fn tagged(image: &Image, tag: &str) -> Result<Request> {
    parse(&["push".into(), image.name.into(), "--repository".into(), image.preview.into(), "--tag".into(), tag.into()], |name| std::env::var(name).ok())
}

/// Names `built` (pushed as `unchecked-<key>`, its share of the disk
/// measured and within the budget) by its inputs: skopeo copies it within
/// the account's registry, its digest kept (`--preserve-digests`), and the
/// tag read back holds that digest. The reference the deploy pins.
fn named(inputs: &Request, built: &str, tools: &devstack::Tools) -> Result<String> {
    let dir = devstack::repo_root().join("target/image-preview");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let auth = login(tools, &inputs.account, None, &["--push", "--pull"], dir.join("name.json"))?;
    let mut cmd = Command::new("skopeo");
    cmd.args(["--command-timeout", "5m", "copy", "--all", "--preserve-digests", "--authfile"]).arg(&auth.0);
    cmd.arg(format!("docker://{built}")).arg(format!("docker://{}", inputs.tagged())).stdin(Stdio::null());
    crate::run(&mut cmd)?;
    let found = existing(inputs, tools)?.with_context(|| format!("{} is not there once named", inputs.tagged()))?;
    anyhow::ensure!(found == built, "{} is {found}, not the measured {built}", inputs.tag);
    Ok(found)
}

/// Waits until Cloudflare has prepared each of `references` on `account`'s
/// network (prepare.rs): a deploy that pins them then starts them at once.
fn prepare(account: &str, references: &[&str]) -> Result<()> {
    let mut api = crate::cloudflare::Api::from_env(account)?;
    crate::prepare::until_ready(&mut api, &mut crate::prepare::Wall::new(), references, crate::prepare::IMAGE_LIMIT)
        .context("a preview image is not ready: blocked this preview deploy before any Worker change; retry this job to reuse its input-keyed images")
}

/// Each image's key: git's path, mode and blob records of its inputs
/// (`Image::inputs`) hashed, rather than a commit ID or worktree mtimes,
/// so a change to one image's inputs alone moves that image alone. A dirty
/// image input is refused: Docker must build the content the key names.
/// Git's ignored targets and caches are not inputs. Nothing stamps a
/// commit, clock, cell, shell or chat into a key.
fn input_keys(root: &Path) -> Result<Vec<(&'static Image, String)>> {
    IMAGES.iter().map(|image| Ok((image, crate::inputs::key(root, image.inputs)?))).collect()
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
    println!("found {}: {reference}", req.tag);
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

    /// Each image's Dockerfile and every local COPY of it are covered by
    /// its inputs. Stage copies and inline heredocs are already content of
    /// the Dockerfile; remote ADDs are checksum-pinned.
    #[test]
    fn the_key_covers_the_dockerfiles_local_inputs() {
        for image in &IMAGES {
            let covered = |path: &str| image.inputs.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")));
            assert!(covered(image.dockerfile), "{}'s inputs lack its Dockerfile", image.name);
            let text = fs::read_to_string(devstack::repo_root().join(image.dockerfile)).unwrap();
            for line in text.lines().filter_map(|l| l.strip_prefix("COPY ")) {
                if line.contains("--from=") || line.contains("<<") { continue; }
                let fields: Vec<_> = line.split_whitespace().collect();
                for field in &fields[..fields.len() - 1] {
                    assert!(!field.starts_with('-'), "new COPY syntax needs review: {line}");
                    assert!(covered(field), "{}'s input key lacks {field}", image.name);
                }
            }
            for line in text.lines().filter(|l| l.starts_with("ADD ")) {
                assert!(line.contains("--checksum=sha256:") && line.contains("https://"), "local ADD or unpinned remote inputs need review: {line}");
            }
            for path in ["cell/src/lib.rs", "cell/shell/index.html", "templates/chat/index.html", "xtask/src/main.rs", "crates/e2e/src/main.rs"] {
                assert!(!covered(path), "not an input of {}: {path}", image.name);
            }
        }
    }

    /// images.yml's Docker lane runs on a change to any input of the Hermes
    /// image, which it builds (#715): each is in its `docker` filter. The
    /// workflow itself starts on every change (no `paths` filter of its own:
    /// a required check that never starts never reports).
    #[test]
    fn the_docker_lane_watches_every_input() {
        let root = devstack::repo_root();
        let workflow = fs::read_to_string(root.join(".github/workflows/images.yml")).unwrap();
        assert!(!workflow.lines().any(|l| l.trim_start().starts_with("paths:")), "the workflow starts on every change; its jobs decide");
        for input in by_name("hermes").unwrap().inputs {
            let glob = if root.join(input).is_dir() { format!("{input}/**") } else { input.to_string() };
            assert!(workflow.lines().any(|l| l.trim() == format!("- {glob}")), "the docker filter lacks {glob}");
        }
    }

    /// images.yml's `check` runs on a change to anything the images'
    /// workspace compiles in: images/, and each file outside it that one of
    /// its sources includes (`include_str!`, `include_bytes!`), each in
    /// the `check` filter, itself or under a `/**` of it.
    #[test]
    fn the_images_check_watches_what_it_compiles() {
        let root = devstack::repo_root();
        let workflow = fs::read_to_string(root.join(".github/workflows/images.yml")).unwrap();
        let lines: Vec<&str> = workflow.lines().collect();
        let at = lines.iter().position(|l| l.trim() == "check:" && l.starts_with("            ")).expect("the `check` filter");
        let globs: Vec<&str> = lines[at + 1..].iter().map(|l| l.trim()).take_while(|l| l.starts_with("- ") || l.starts_with('#')).filter_map(|l| l.strip_prefix("- ")).collect();
        assert!(globs.contains(&"images/**"), "{globs:?}");
        let covered = |path: &str| globs.iter().any(|g| *g == path || g.strip_suffix("/**").is_some_and(|dir| path.starts_with(&format!("{dir}/"))));
        let images = root.join("images");
        let mut sources = vec![];
        let mut dirs = vec![images.clone()];
        // bounded by the images workspace's tree, its build output left out
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() && entry.file_name() != "target" {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    sources.push(path);
                }
            }
        }
        let mut outside = 0;
        for source in sources {
            let text = fs::read_to_string(&source).unwrap();
            for macro_ in ["include_str!(\"", "include_bytes!(\""] {
                for (at, _) in text.match_indices(macro_) {
                    let rest = &text[at + macro_.len()..];
                    let named = &rest[..rest.find('"').unwrap()];
                    let full = std::path::absolute(source.parent().unwrap().join(named)).unwrap();
                    let normal: std::path::PathBuf = full.components().fold(std::path::PathBuf::new(), |mut p, c| {
                        if c == std::path::Component::ParentDir { p.pop(); } else { p.push(c); }
                        p
                    });
                    let Ok(from_root) = normal.strip_prefix(&root) else { panic!("{} includes {named}, outside the repo", source.display()) };
                    let path = from_root.to_string_lossy().replace('\\', "/");
                    if !path.starts_with("images/") {
                        outside += 1;
                    }
                    assert!(covered(&path), "{} includes {path}, which the `check` filter does not watch", source.display());
                }
            }
        }
        assert!(outside > 0, "the images' sources include files outside images/ (as of 2026-10-31)");
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

    /// One way to run at a time, refused before any key, registry or
    /// file is touched: a probe, `--build` or `--dry-run`, and a summary
    /// only from `--build`, the run that names the deployed image.
    #[test]
    fn a_run_is_one_of_its_modes() {
        let output = std::env::temp_dir().join(format!("preview-modes-{}", std::process::id()));
        let refused = |extra: &[&str]| {
            let mut args = vec!["--output".to_string(), output.display().to_string()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args).unwrap_err().to_string()
        };
        assert!(refused(&["--build", "--dry-run"]).contains("one at a time"));
        assert!(refused(&["--dry-run", "--summary", "s.md"]).contains("--dry-run"));
        assert!(refused(&["--build", "--all"]).starts_with("usage:"));
        assert!(refused(&["--queue"]).starts_with("usage:"));
        assert!(run(&["--build".to_string()]).unwrap_err().to_string().starts_with("usage:"));
        assert!(!output.exists());
    }

    /// A new commit, dirty unrelated files, or a restarted process cannot
    /// change identity. A changed input does; reverting its content reuses
    /// the old key. Dirty inputs must never be built under a committed key.
    /// Preview resolves each image by its own key: the CLI moves Hermes
    /// alone, the door's runner the door alone, and the bridge, in both,
    /// both.
    #[test]
    fn identity_is_inputs_not_the_commit_or_build_cache() {
        let root = std::env::temp_dir().join(format!("preview-inputs-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        for dir in ["cli", "images/bridge", "images/simplex-door"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let git = |args: &[&str]| {
            let out = Command::new("git").args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"]).args(args).current_dir(&root).output().unwrap();
            assert!(out.status.success(), "{}", said(&out));
        };
        let keys = || input_keys(&root).map(|k| k.into_iter().map(|(image, key)| (image.name, key)).collect::<BTreeMap<_, _>>());
        let commit = |path: &str, text: &str, message: &str| {
            fs::write(root.join(path), text).unwrap();
            git(&["add", path]);
            git(&["commit", "-qm", message]);
            keys().unwrap()
        };
        git(&["init", "-q"]);
        fs::write(root.join("cli/main.rs"), "first").unwrap();
        fs::write(root.join("images/bridge/lib.rs"), "first").unwrap();
        fs::write(root.join("images/simplex-door/main.rs"), "first").unwrap();
        git(&["add", "cli", "images"]);
        git(&["commit", "-qm", "first"]);
        let first = keys().unwrap();
        assert_eq!(first.keys().copied().collect::<Vec<_>>(), ["door", "hermes"], "one key for each image preview follows");
        assert_ne!(first["door"], first["hermes"]);
        fs::write(root.join("cell.txt"), "not in image").unwrap();
        assert_eq!(keys().unwrap(), first);
        assert_eq!(commit("cell.txt", "not in image", "cell changed"), first);
        fs::write(root.join("cli/main.rs"), "second").unwrap();
        assert!(keys().is_err());
        let cli = commit("cli/main.rs", "second", "CLI changed");
        assert!(cli["hermes"] != first["hermes"] && cli["door"] == first["door"], "the CLI moves Hermes alone");
        assert_eq!(commit("cli/main.rs", "first", "CLI restored"), first);
        let door = commit("images/simplex-door/main.rs", "second", "door changed");
        assert!(door["door"] != first["door"] && door["hermes"] == first["hermes"], "the door's runner moves the door alone");
        assert_eq!(commit("images/simplex-door/main.rs", "first", "door restored"), first);
        let bridge = commit("images/bridge/lib.rs", "second", "bridge changed");
        assert!(bridge["door"] != first["door"] && bridge["hermes"] != first["hermes"], "the bridge moves both");
        fs::write(root.join("cli/untracked.rs"), "not keyed").unwrap();
        assert!(keys().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
