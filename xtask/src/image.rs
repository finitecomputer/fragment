//! `cargo xtask image push|copy`: a computer image put in the Cloudflare
//! registry of the account `CLOUDFLARE_ACCOUNT_ID` names (wrangler acts
//! with `CLOUDFLARE_API_TOKEN`), and named by the digest the registry
//! holds: the reference a deploy config pins it at (deploy.rs,
//! `pinned_ok`; docs/computers.md, "Preview and production"). The Image
//! workflow (.github/workflows/image.yml) runs it by hand on a runner, so
//! a round needs no machine of anyone's; anywhere Docker runs, it works the
//! same way.
//!
//! - `push <image> --repository <name> --tag <tag> [--summary <file>]`
//!   builds the image from the tree as a deploy's wrangler builds it
//!   (build.rs, `image_build`) and pushes it with wrangler's `containers
//!   push` on the pinned Node.
//! - `copy <image> --from <reference> --repository <name> --tag <tag>
//!   [--summary <file>]` builds nothing: it copies an image another
//!   account's registry holds (the image the preview proved), byte for
//!   byte, so its digest is the one the round proved. The source's
//!   account and token are `FROM_CLOUDFLARE_ACCOUNT_ID` and
//!   `FROM_CLOUDFLARE_API_TOKEN` (Containers on that account).
//!
//! Either waits, before its summary names the image, until Cloudflare has
//! prepared it on the account's network (prepare.rs, `IMAGE_LIMIT`): a
//! copy is not prepared until something asks, and production's copy of
//! round 11 (2026-10-24) was still pending 50 minutes on, at its
//! promotion's deploy.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;
use serde_json::{json, Value};

use crate::deploy::{pinned_ok, repository_ok, REGISTRY};

pub(crate) mod preview;

const USAGE: &str = "usage: cargo xtask image push <image> --repository <name> --tag <tag> [--summary <file>]\n       cargo xtask image copy <image> --from <reference> --repository <name> --tag <tag> [--summary <file>]\nthe registry's account in CLOUDFLARE_ACCOUNT_ID (wrangler's token in CLOUDFLARE_API_TOKEN); a copy's source's in FROM_CLOUDFLARE_ACCOUNT_ID and FROM_CLOUDFLARE_API_TOKEN; images: hermes";

/// The images it builds, by name: each one's Dockerfile and build context,
/// relative to the repo, as a deploy config's line builds it
/// (deploy/example.jsonc: the Hermes image builds from the repo's root,
/// as it carries the fragment CLI).
const IMAGES: [(&str, &str, &str); 1] = [("hermes", "images/hermes/Dockerfile", ".")];

/// How many times a push or a copy is tried, and the pause after each that
/// fails: the Hermes image's base is one layer of 1.4 GB compressed, whose
/// push has timed out. Each try logs in to the registry afresh and sends
/// what the registry lacks.
const TRIES: u32 = 4;
const PAUSE: Duration = Duration::from_secs(30);

/// How long the registry credentials a copy mints last, as long as
/// wrangler's own for a push: the copy of the base must fit in them.
const CREDENTIAL_MINUTES: &str = "15";

/// What a run is asked for, checked before anything is built or minted.
#[derive(Debug)]
struct Request {
    image: &'static str,
    dockerfile: &'static str,
    context: &'static str,
    /// A copy's source; a push builds.
    from: Option<Source>,
    account: String,
    repository: String,
    tag: String,
    summary: Option<PathBuf>,
}

/// A copy's source: a reference pinned by digest in `account`'s registry.
#[derive(Debug, PartialEq)]
struct Source {
    reference: String,
    account: String,
}

impl Source {
    /// Its digest, `sha256:<64 hex>`.
    fn digest(&self) -> &str {
        self.reference.split_once('@').expect("a source is pinned by digest").1
    }
}

impl Request {
    /// The name it is pushed or copied to:
    /// `registry.cloudflare.com/<account>/<repository>:<tag>`. Whole, so
    /// wrangler takes it as it is: a bare `<repository>:<tag>` whose tag
    /// is a number reads to it as a host and a port.
    fn tagged(&self) -> String {
        format!("{REGISTRY}/{}/{}:{}", self.account, self.repository, self.tag)
    }
}

/// A Cloudflare account's id: 32 lowercase hex digits.
fn account_ok(account: &str) -> bool {
    account.len() == 32 && account.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A Docker tag: a letter, digit or `_`, then up to 127 of those, `.` or `-`.
fn tag_ok(tag: &str) -> bool {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    (1..=128).contains(&tag.len()) && tag.bytes().next().is_some_and(word) && tag.bytes().all(|b| word(b) || b"._-".contains(&b))
}

/// `--from` as given, made whole: a reference pinned by digest in
/// `account`'s registry, its account written out, or as `<account>` (a
/// run's summary writes it so) or `***` (a run's log masks it so).
fn source_of(from: &str, account: &str) -> Option<Source> {
    let from = from.trim();
    let whole = ["<account>", "***"]
        .iter()
        .find_map(|stand_in| from.strip_prefix(&format!("{REGISTRY}/{stand_in}/")))
        .map_or_else(|| from.to_string(), |rest| format!("{REGISTRY}/{account}/{rest}"));
    pinned_ok(&whole, account).then(|| Source { reference: whole, account: account.to_string() })
}

/// The request, from the arguments and the environment (`env`).
fn parse(rest: &[String], env: impl Fn(&str) -> Option<String>) -> Result<Request> {
    let usage = || anyhow::anyhow!(USAGE);
    let (verb, rest) = rest.split_first().ok_or_else(usage)?;
    let copy = match verb.as_str() {
        "push" => false,
        "copy" => true,
        _ => return Err(usage()),
    };
    let (mut image, mut from, mut repository, mut tag, mut summary) = (None, None, None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" if copy => from = Some(it.next().ok_or_else(usage)?.clone()),
            "--repository" => repository = Some(it.next().ok_or_else(usage)?.clone()),
            "--tag" => tag = Some(it.next().ok_or_else(usage)?.clone()),
            "--summary" => summary = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            name if image.is_none() && !name.starts_with('-') => image = Some(name.to_string()),
            _ => return Err(usage()),
        }
    }
    let (image, repository, tag) = (image.ok_or_else(usage)?, repository.ok_or_else(usage)?, tag.ok_or_else(usage)?);
    let Some(&(image, dockerfile, context)) = IMAGES.iter().find(|(name, ..)| *name == image) else {
        bail!("no image {image:?}: {}", IMAGES.map(|(name, ..)| name).join(", "));
    };
    let set = |name: &str| env(name).filter(|v| !v.is_empty());
    let account = set("CLOUDFLARE_ACCOUNT_ID").context("CLOUDFLARE_ACCOUNT_ID is not set: it names the account whose registry the image goes to, the deployment's own (its config's account_id)")?;
    anyhow::ensure!(account_ok(&account), "CLOUDFLARE_ACCOUNT_ID is no account id (32 lowercase hex digits)");
    anyhow::ensure!(repository_ok(&repository), "--repository {repository:?} is no repository: lowercase letters and digits joined by `.`, `_` or `-`");
    anyhow::ensure!(tag_ok(&tag), "--tag {tag:?} is no tag: up to 128 letters, digits, `_`, `.` or `-`, not starting with `.` or `-`");
    let from = match (copy, from) {
        (false, _) => None,
        (true, None) => return Err(usage()),
        (true, Some(from)) => {
            let source_account = set("FROM_CLOUDFLARE_ACCOUNT_ID").context("FROM_CLOUDFLARE_ACCOUNT_ID is not set: it names the account whose registry a copy reads")?;
            anyhow::ensure!(account_ok(&source_account), "FROM_CLOUDFLARE_ACCOUNT_ID is no account id (32 lowercase hex digits)");
            anyhow::ensure!(set("FROM_CLOUDFLARE_API_TOKEN").is_some(), "FROM_CLOUDFLARE_API_TOKEN is not set: the token a copy mints its credentials for the source's registry with");
            let source = source_of(&from, &source_account).with_context(|| format!("--from {from:?} is no reference pinned by digest in FROM_CLOUDFLARE_ACCOUNT_ID's registry ({REGISTRY}/<account>/<repository>@sha256:<64 hex>)"))?;
            Some(source)
        }
    };
    Ok(Request { image, dockerfile, context, from, account, repository, tag, summary })
}

/// `cargo xtask image …`.
pub fn image(rest: &[String]) -> Result<()> {
    if rest.first().is_some_and(|s| s == "preview") {
        return preview::run(&rest[1..]);
    }
    let req = parse(rest, |name| std::env::var(name).ok())?;
    // the pinned Node and wrangler first, and the API the image is prepared
    // through: one that cannot be had stops the run before a build or a
    // copy of minutes
    let tools = devstack::Tools::locate()?;
    let mut api = crate::cloudflare::Api::from_env(&req.account)?;
    let reference = match &req.from {
        None => push(&req, &tools)?,
        Some(source) => copy(&req, source, &tools)?,
    };
    // ready on the account's network before the summary names it, so the
    // deploy that pins it finds it ready (prepare.rs): the one that would
    // wait is a promotion's, production's
    crate::prepare::until_ready(&mut api, &mut crate::prepare::Wall::new(), &[&reference], crate::prepare::IMAGE_LIMIT)
        .with_context(|| format!("{reference} is in the registry, and not yet ready to run: a deploy that pins it waits for it (prepare.rs), or run this again"))?;
    let text = summary(&req, &reference);
    println!("\n{text}");
    if let Some(file) = &req.summary {
        let mut f = fs::OpenOptions::new().create(true).append(true).open(file).with_context(|| format!("open {}", file.display()))?;
        writeln!(f, "{text}")?;
    }
    Ok(())
}

/// Builds and pushes the image; the reference the registry holds it at.
fn push(req: &Request, tools: &devstack::Tools) -> Result<String> {
    let root = devstack::repo_root();
    let tagged = req.tagged();
    println!("building {} as {tagged}", req.image);
    crate::build::image_build(&json!({ "dockerfile": root.join(req.dockerfile), "build_context": root.join(req.context) }), Some(&tagged))?;
    tried(TRIES, PAUSE, |n| {
        println!("pushing {tagged} (try {n} of {TRIES})");
        let mut cmd = crate::deploy::wrangler(tools, &req.account)?;
        cmd.args(["containers", "push", &tagged]).stdin(Stdio::null());
        crate::run(&mut cmd)
    })?;
    let out = Command::new("docker").args(["image", "inspect", "--format", "{{json .RepoDigests}}", &tagged]).stdin(Stdio::null()).output().context("could not start docker image inspect")?;
    anyhow::ensure!(out.status.success(), "docker image inspect {tagged}: {}", String::from_utf8_lossy(&out.stderr).trim());
    pinned_of(&String::from_utf8_lossy(&out.stdout), &req.account, &req.repository)
}

/// Copies the source to the request's name with skopeo, which copies a
/// manifest and its blobs as they are (`--preserve-digests` refuses any
/// copy that would change the digest), and logs in to each side from a
/// file of its own: the two accounts are two namespaces of one registry
/// host, which one Docker login (and so `docker buildx imagetools
/// create`) holds one credential for. Then reads the tag back, and holds
/// its manifest's SHA-256 to the source's digest. The reference the
/// registry holds the copy at.
fn copy(req: &Request, source: &Source, tools: &devstack::Tools) -> Result<String> {
    skopeo_installed()?;
    let dir = devstack::repo_root().join("target/image-copy");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let from_token = std::env::var("FROM_CLOUDFLARE_API_TOKEN").context("FROM_CLOUDFLARE_API_TOKEN")?;
    let to = req.tagged();
    tried(TRIES, PAUSE, |n| {
        println!("copying {} to {to} (try {n} of {TRIES})", source.reference);
        let from = login(tools, &source.account, Some(&from_token), &["--pull"], dir.join("from.json"))?;
        let dest = login(tools, &req.account, None, &["--push", "--pull"], dir.join("to.json"))?;
        let mut cmd = Command::new("skopeo");
        cmd.args(["copy", "--all", "--preserve-digests", "--src-authfile"]).arg(&from.0).arg("--dest-authfile").arg(&dest.0);
        cmd.arg(format!("docker://{}", source.reference)).arg(format!("docker://{to}")).stdin(Stdio::null());
        crate::run(&mut cmd)
    })?;
    let dest = login(tools, &req.account, None, &["--pull"], dir.join("to.json"))?;
    let out = Command::new("skopeo").args(["inspect", "--raw", "--authfile"]).arg(&dest.0).arg(format!("docker://{to}")).stdin(Stdio::null()).output().context("could not start skopeo inspect")?;
    anyhow::ensure!(out.status.success(), "skopeo inspect {to}: {}", String::from_utf8_lossy(&out.stderr).trim());
    let copied = format!("sha256:{}", fragment_core::blob::sha256_hex(&out.stdout));
    anyhow::ensure!(copied == source.digest(), "{to} is {copied}, not the source's {}: the copy changed the image", source.digest());
    let reference = format!("{REGISTRY}/{}/{}@{copied}", req.account, req.repository);
    assert!(pinned_ok(&reference, &req.account), "a copy's reference is one a deploy pins: {reference}");
    Ok(reference)
}

fn skopeo_installed() -> Result<()> {
    match Command::new("skopeo").arg("--version").stdin(Stdio::null()).output() {
        Ok(out) if out.status.success() => Ok(()),
        _ => bail!("a copy runs skopeo, which copies an image byte for byte from one account's registry to another's, and it is not installed (GitHub's Ubuntu runners have it; `apt install skopeo`)"),
    }
}

/// A registry login of skopeo's, in a file of its own (`--authfile`, mode
/// 600, under target/image-copy, mode 700), removed when dropped.
struct Login(PathBuf);

impl Drop for Login {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Logs skopeo in to `account`'s registry into `file`, with credentials
/// minted for it (`credentials`), the password on its standard input.
fn login(tools: &devstack::Tools, account: &str, token: Option<&str>, permissions: &[&str], file: PathBuf) -> Result<Login> {
    let (username, password) = credentials(tools, account, token, permissions)?;
    let login = Login(file);
    let _ = fs::remove_file(&login.0);
    let mut cmd = Command::new("skopeo");
    cmd.args(["login", "--authfile"]).arg(&login.0).args(["--username", &username, "--password-stdin", REGISTRY]);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("could not start skopeo login")?;
    child.stdin.take().context("skopeo's stdin")?.write_all(password.as_bytes())?;
    let out = child.wait_with_output()?;
    anyhow::ensure!(out.status.success(), "skopeo login to {REGISTRY} ({}): {}", permissions.join(" "), said(&out));
    Ok(login)
}

/// Registry credentials minted for `account`, as wrangler mints its own for
/// a push (`wrangler containers registries credentials`: the Containers
/// API's `POST /accounts/{account}/containers/registries/
/// registry.cloudflare.com/credentials`), with `permissions` (`--pull`,
/// `--push`), lasting `CREDENTIAL_MINUTES`; wrangler's token is `token`
/// when given, else its own environment's. Never printed: the output of a
/// call that failed, which minted none, alone is shown.
fn credentials(tools: &devstack::Tools, account: &str, token: Option<&str>, permissions: &[&str]) -> Result<(String, String)> {
    let mut cmd = crate::deploy::wrangler(tools, account)?;
    cmd.args(["containers", "registries", "credentials", REGISTRY, "--expiration-minutes", CREDENTIAL_MINUTES, "--json"]).args(permissions).stdin(Stdio::null());
    if let Some(token) = token {
        cmd.env("CLOUDFLARE_API_TOKEN", token);
    }
    let out = cmd.output().context("could not start wrangler")?;
    anyhow::ensure!(out.status.success(), "wrangler minted no {REGISTRY} credentials ({}): {}", permissions.join(" "), said(&out));
    credentials_of(&String::from_utf8_lossy(&out.stdout)).with_context(|| format!("wrangler's {REGISTRY} credentials ({}) are no JSON with a username and a password", permissions.join(" ")))
}

/// The username and password in wrangler's `--json` answer (the API's
/// `result`), whatever surrounds the object.
fn credentials_of(text: &str) -> Option<(String, String)> {
    let json = text.get(text.find('{')?..=text.rfind('}')?)?;
    let v: Value = serde_json::from_str(json).ok()?;
    let field = |name: &str| v[name].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Some((field("username")?, field("password")?))
}

/// A command's output, both streams, as text.
fn said(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string()
}

/// `attempt` until it succeeds, at most `tries` times, `pause` after each
/// that fails but the last; the last one's error when none succeeds.
fn tried(tries: u32, pause: Duration, mut attempt: impl FnMut(u32) -> Result<()>) -> Result<()> {
    assert!(tries > 0, "at least one try");
    for n in 1..=tries {
        match attempt(n) {
            Ok(()) => return Ok(()),
            Err(e) if n == tries => return Err(e.context(format!("it failed {tries} times"))),
            Err(e) => {
                eprintln!("try {n} of {tries} failed ({e:#}); trying again in {pause:?}");
                std::thread::sleep(pause);
            }
        }
    }
    unreachable!("the last try returns")
}

/// The reference the registry holds the pushed image at, from its
/// `RepoDigests` (`docker image inspect`, after the push: what wrangler
/// itself reads, and Cloudflare's docs say to): the one in `account`'s
/// registry and `repository`, and only one, a reference a deploy pins.
fn pinned_of(repo_digests: &str, account: &str, repository: &str) -> Result<String> {
    let listed: Option<Vec<String>> = serde_json::from_str(repo_digests.trim()).with_context(|| format!("RepoDigests is no list of references: {}", repo_digests.trim()))?;
    let at = format!("{REGISTRY}/{account}/{repository}@");
    let mut found: Vec<String> = listed.unwrap_or_default().into_iter().filter(|r| r.starts_with(&at)).collect();
    found.sort();
    found.dedup();
    match found.as_slice() {
        [one] if pinned_ok(one, account) => Ok(one.clone()),
        [one] => bail!("{one} is not a reference a deploy pins ({REGISTRY}/<account>/<repository>@sha256:<64 hex>)"),
        [] => bail!("the pushed image has no digest in {REGISTRY}/<account>/{repository} (its RepoDigests: {})", repo_digests.trim()),
        many => bail!("the pushed image has {} digests in {REGISTRY}/<account>/{repository}, not one: {}", many.len(), many.join(", ")),
    }
}

/// What a run says it made (and, with `--summary`, writes as Markdown):
/// the reference, readable with the account as `<account>`, and whole in
/// its line of a deploy config's `computers.images`, under the image's own
/// name. A runner masks the account in that line, as it is the
/// environment's secret: it is the config's own `account_id`.
fn summary(req: &Request, reference: &str) -> String {
    let readable = |r: &str, account: &str| r.replacen(account, "<account>", 1);
    let (done, from) = match &req.from {
        None => ("pushed", String::new()),
        Some(s) => ("copied", format!("From `{}`, its digest unchanged.\n\n", readable(&s.reference, &s.account))),
    };
    let line = req.image;
    format!(
        "### {} {done} as {}:{}\n\n{from}`{}`\n\nIts line in the deploy config's `computers.images`, the account the config's `account_id` (docs/computers.md, \"Preview and production\"):\n\n```jsonc\n\"{line}\": {{ \"image\": \"{reference}\" }},\n```\n",
        req.image,
        req.repository,
        req.tag,
        readable(reference, &req.account)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
    const PREVIEW: &str = "fedcba9876543210fedcba9876543210";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The environment a run reads: `set`'s names, each with a value.
    fn env(set: &[(&'static str, &str)]) -> impl Fn(&str) -> Option<String> {
        let set: Vec<(&'static str, String)> = set.iter().map(|(k, v)| (*k, v.to_string())).collect();
        move |name| set.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())
    }

    /// Goal: a push is checked whole before anything is built (a build is
    /// minutes). Valid: the image, its repository and tag, the account
    /// from the environment, and the summary file. Invalid: an image xtask
    /// does not build, a repository or tag Docker or the deploy would
    /// refuse, a missing or malformed account, a flag without its value, a
    /// source given to a push.
    #[test]
    fn a_push_is_checked_before_it_builds() {
        let with = |account: &str| env(&[("CLOUDFLARE_ACCOUNT_ID", account)]);
        let p = parse(&args(&["push", "hermes", "--repository", "fragment-preview-computer-hermes-canary", "--tag", "0123456789ab", "--summary", "/tmp/s.md"]), with(ACCOUNT)).unwrap();
        assert_eq!((p.image, p.dockerfile, p.context, &p.from), ("hermes", "images/hermes/Dockerfile", ".", &None));
        assert_eq!(p.tagged(), format!("registry.cloudflare.com/{ACCOUNT}/fragment-preview-computer-hermes-canary:0123456789ab"));
        assert_eq!(p.summary, Some(PathBuf::from("/tmp/s.md")));
        let refused = |list: &[&str], account: &str| parse(&args(list), with(account)).unwrap_err().to_string();
        let ok = ["push", "hermes", "--repository", "fragment-computer-hermes", "--tag", "r9"];
        assert!(parse(&args(&ok), with(ACCOUNT)).is_ok());
        assert!(refused(&["push", "stub", "--repository", "r", "--tag", "t"], ACCOUNT).contains("no image \"stub\""));
        assert!(refused(&["push", "hermes", "--repository", "Hermes", "--tag", "t"], ACCOUNT).contains("is no repository"));
        assert!(refused(&["push", "hermes", "--repository", "a/-b", "--tag", "t"], ACCOUNT).contains("is no repository"));
        let long = "a".repeat(129);
        for bad in ["-r9", ".r9", "r9:x", "r/9", long.as_str()] {
            assert!(refused(&["push", "hermes", "--repository", "r", "--tag", bad], ACCOUNT).contains("is no tag"), "{bad}");
        }
        assert!(parse(&args(&ok), env(&[])).unwrap_err().to_string().contains("CLOUDFLARE_ACCOUNT_ID is not set"));
        assert!(refused(&ok, "").contains("CLOUDFLARE_ACCOUNT_ID is not set"));
        assert!(refused(&ok, &ACCOUNT.to_uppercase()).contains("no account id"));
        assert!(refused(&ok, &ACCOUNT[1..]).contains("no account id"));
        for list in [&["push", "hermes", "--repository", "r"][..], &["push", "hermes", "--tag"], &["pull", "hermes", "--repository", "r", "--tag", "t"], &[], &["push", "hermes", "hermes", "--repository", "r", "--tag", "t"], &["push", "hermes", "--from", "x", "--repository", "r", "--tag", "t"]] {
            assert!(refused(list, ACCOUNT).starts_with("usage:"), "{list:?}");
        }
    }

    /// Goal: a copy reads only a reference the source account's registry
    /// pins, and is checked whole before anything is minted. Valid: the
    /// source with its account written out, as `<account>` (a summary's)
    /// or as `***` (a masked log's). Invalid: no source, a tag rather than
    /// a digest, another account's reference, a missing source account or
    /// token.
    #[test]
    fn a_copy_reads_a_pinned_reference_of_the_source_account() {
        let digest = "9f".repeat(32);
        let whole = format!("registry.cloudflare.com/{PREVIEW}/fragment-preview-computer-hermes-canary@sha256:{digest}");
        let all = env(&[("CLOUDFLARE_ACCOUNT_ID", ACCOUNT), ("FROM_CLOUDFLARE_ACCOUNT_ID", PREVIEW), ("FROM_CLOUDFLARE_API_TOKEN", "t")]);
        let copy = |from: &str| args(&["copy", "hermes", "--from", from, "--repository", "fragment-computer-hermes", "--tag", "r9"]);
        for given in [whole.clone(), whole.replace(PREVIEW, "<account>"), whole.replace(PREVIEW, "***"), format!(" {whole}\n")] {
            let req = parse(&copy(&given), &all).unwrap();
            assert_eq!(req.from, Some(Source { reference: whole.clone(), account: PREVIEW.into() }), "{given}");
            assert_eq!(req.from.as_ref().unwrap().digest(), format!("sha256:{digest}"));
            assert_eq!(req.tagged(), format!("registry.cloudflare.com/{ACCOUNT}/fragment-computer-hermes:r9"));
        }
        let refused = |list: Vec<String>, e: &dyn Fn(&str) -> Option<String>| parse(&list, e).unwrap_err().to_string();
        assert!(refused(args(&["copy", "hermes", "--repository", "fragment-computer-hermes", "--tag", "r9"]), &all).starts_with("usage:"));
        for bad in [whole.replace(&format!("@sha256:{digest}"), ":r8"), whole.replace(PREVIEW, ACCOUNT), whole.replace(PREVIEW, "<other>"), whole.replace(&digest, &digest[..12])] {
            assert!(refused(copy(&bad), &all).contains("is no reference pinned by digest"), "{bad}");
        }
        assert!(refused(copy(&whole), &env(&[("CLOUDFLARE_ACCOUNT_ID", ACCOUNT), ("FROM_CLOUDFLARE_API_TOKEN", "t")])).contains("FROM_CLOUDFLARE_ACCOUNT_ID is not set"));
        assert!(refused(copy(&whole), &env(&[("CLOUDFLARE_ACCOUNT_ID", ACCOUNT), ("FROM_CLOUDFLARE_ACCOUNT_ID", "x"), ("FROM_CLOUDFLARE_API_TOKEN", "t")])).contains("no account id"));
        assert!(refused(copy(&whole), &env(&[("CLOUDFLARE_ACCOUNT_ID", ACCOUNT), ("FROM_CLOUDFLARE_ACCOUNT_ID", PREVIEW), ("FROM_CLOUDFLARE_API_TOKEN", "")])).contains("FROM_CLOUDFLARE_API_TOKEN is not set"));
    }

    /// Goal: the reference named is the registry's, and one a deploy pins.
    /// Valid: the pushed repository's digest among others' (another
    /// repository's, another registry's), listed twice. Invalid: none
    /// there, two different ones, one in another account, a digest cut
    /// short, output that is no list.
    #[test]
    fn the_reference_is_the_one_the_registry_holds() {
        let digest = "9f".repeat(32);
        let ours = format!("registry.cloudflare.com/{ACCOUNT}/fragment-computer-hermes@sha256:{digest}");
        let others = [format!("registry.cloudflare.com/{ACCOUNT}/fragment-computer-hermes-canary@sha256:{}", "ab".repeat(32)), format!("fragment-computer-hermes@sha256:{digest}")];
        let list = |refs: &[&str]| serde_json::to_string(refs).unwrap();
        let found = pinned_of(&list(&[&others[0], &ours, &others[1], &ours]), ACCOUNT, "fragment-computer-hermes").unwrap();
        assert_eq!(found, ours);
        assert!(pinned_ok(&found, ACCOUNT));
        assert!(pinned_of(&list(&[&others[0], &others[1]]), ACCOUNT, "fragment-computer-hermes").unwrap_err().to_string().contains("no digest"));
        assert!(pinned_of("null\n", ACCOUNT, "fragment-computer-hermes").unwrap_err().to_string().contains("no digest"));
        let second = format!("registry.cloudflare.com/{ACCOUNT}/fragment-computer-hermes@sha256:{}", "cd".repeat(32));
        assert!(pinned_of(&list(&[&ours, &second]), ACCOUNT, "fragment-computer-hermes").unwrap_err().to_string().contains("2 digests"));
        let other_account = ours.replace(ACCOUNT, &"f".repeat(32));
        assert!(pinned_of(&list(&[&other_account]), ACCOUNT, "fragment-computer-hermes").is_err());
        let short = ours.replace(&digest, &digest[..12]);
        assert!(pinned_of(&list(&[&short]), ACCOUNT, "fragment-computer-hermes").unwrap_err().to_string().contains("not a reference a deploy pins"));
        assert!(pinned_of("registry.cloudflare.com/x", ACCOUNT, "fragment-computer-hermes").unwrap_err().to_string().contains("no list"));
    }

    /// Goal: the credentials wrangler mints are read from its answer, and
    /// nothing else passes for them. Valid: its JSON, pretty or not, after
    /// a line of its own. Invalid: no JSON, a username without a password,
    /// an empty password.
    #[test]
    fn credentials_are_read_from_wrangler_s_answer() {
        let answer = json!({ "account_id": PREVIEW, "registry_host": "registry.cloudflare.com", "username": "v1", "password": "p" });
        let want = Some(("v1".to_string(), "p".to_string()));
        assert_eq!(credentials_of(&serde_json::to_string_pretty(&answer).unwrap()), want);
        assert_eq!(credentials_of(&format!("a line first\n{answer}\n")), want);
        assert_eq!(credentials_of("p"), None);
        assert_eq!(credentials_of(&json!({ "username": "v1" }).to_string()), None);
        assert_eq!(credentials_of(&json!({ "username": "v1", "password": "" }).to_string()), None);
        assert_eq!(credentials_of("} {"), None);
    }

    /// Goal: a push or copy that times out is tried again, a bounded number
    /// of times. Valid: two failures, then success, tried three times.
    /// Invalid: failing every time, tried exactly `tries` times, and the
    /// last failure is the error, saying how many.
    #[test]
    fn a_push_is_tried_again_and_no_more_than_its_tries() {
        let mut calls = 0;
        tried(4, Duration::ZERO, |n| {
            calls += 1;
            assert_eq!(n, calls);
            if n < 3 { bail!("timed out {n}") } else { Ok(()) }
        })
        .unwrap();
        assert_eq!(calls, 3);
        let mut calls = 0;
        let e = tried(4, Duration::ZERO, |n| {
            calls += 1;
            bail!("timed out {n}")
        })
        .unwrap_err();
        assert_eq!(calls, 4);
        assert_eq!(format!("{e:#}"), "it failed 4 times: timed out 4");
    }

    /// The summary names the reference readably and whole, in the line a
    /// deploy config takes (one `deploy.rs` parses as a pinned image), under
    /// the image's own name, a copy's saying where it came from.
    #[test]
    fn the_summary_is_the_line_a_config_takes() {
        let line_of = |text: &str, name: &str| -> serde_json::Value {
            let line = text.lines().find(|l| l.starts_with(&format!("\"{name}\""))).unwrap_or_else(|| panic!("{name}'s line in {text}"));
            serde_json::from_str(&format!("{{{}}}", line.trim_end_matches(','))).unwrap()
        };
        let all = env(&[("CLOUDFLARE_ACCOUNT_ID", ACCOUNT), ("FROM_CLOUDFLARE_ACCOUNT_ID", PREVIEW), ("FROM_CLOUDFLARE_API_TOKEN", "t")]);
        let digest = "9f".repeat(32);
        let p = parse(&args(&["push", "hermes", "--repository", "fragment-preview-computer-hermes-canary", "--tag", "r9"]), &all).unwrap();
        let reference = format!("registry.cloudflare.com/{ACCOUNT}/fragment-preview-computer-hermes-canary@sha256:{digest}");
        let text = summary(&p, &reference);
        assert!(text.starts_with("### hermes pushed as fragment-preview-computer-hermes-canary:r9") && text.contains(&format!("`registry.cloudflare.com/<account>/fragment-preview-computer-hermes-canary@sha256:{digest}`")), "{text}");
        assert_eq!(line_of(&text, "hermes")["hermes"]["image"], reference.as_str());
        let from = format!("registry.cloudflare.com/{PREVIEW}/fragment-preview-computer-hermes-canary@sha256:{digest}");
        let c = parse(&args(&["copy", "hermes", "--from", &from, "--repository", "fragment-computer-hermes", "--tag", "r9"]), &all).unwrap();
        let reference = format!("registry.cloudflare.com/{ACCOUNT}/fragment-computer-hermes@sha256:{digest}");
        let text = summary(&c, &reference);
        assert!(text.starts_with("### hermes copied as fragment-computer-hermes:r9") && text.contains(&format!("From `registry.cloudflare.com/<account>/fragment-preview-computer-hermes-canary@sha256:{digest}`, its digest unchanged")), "{text}");
        assert!(!text.contains(PREVIEW), "{text}");
        assert_eq!(line_of(&text, "hermes")["hermes"]["image"], reference.as_str());
    }
}
