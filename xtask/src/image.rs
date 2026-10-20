//! `cargo xtask image push <image> --repository <name> --tag <tag>
//! [--summary <file>]`: a computer image built from the tree as a deploy's
//! wrangler builds it (build.rs, `image_build`), pushed with wrangler's
//! `containers push` on the pinned Node to the Cloudflare registry of the
//! account `CLOUDFLARE_ACCOUNT_ID` names (wrangler authenticates with
//! `CLOUDFLARE_API_TOKEN`), and named by the digest the registry holds:
//! the reference a deploy config pins it at (deploy.rs, `pinned_ok`;
//! docs/computers.md, "Stable and canary"). The Image workflow
//! (.github/workflows/image.yml) runs it by hand on a runner, so a canary
//! round needs no machine of anyone's; anywhere Docker runs, it pushes
//! the same way.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;
use serde_json::json;

use crate::deploy::{pinned_ok, repository_ok, REGISTRY};

const USAGE: &str = "usage: cargo xtask image push <image> --repository <name> --tag <tag> [--summary <file>], with CLOUDFLARE_ACCOUNT_ID (and wrangler's CLOUDFLARE_API_TOKEN) set; images: hermes";

/// The images it builds, by name: each one's Dockerfile and build context,
/// relative to the repo, as a deploy config's line builds it
/// (deploy/example.jsonc: the Hermes image builds from the repo's root,
/// as it carries the fragment CLI).
const IMAGES: [(&str, &str, &str); 1] = [("hermes", "images/hermes/Dockerfile", ".")];

/// How many times a push is tried, and the pause after each that fails:
/// the Hermes image's base is one layer of 1.4 GB compressed, whose push
/// has timed out. Each try is wrangler's own, which logs in to the
/// registry afresh (its credentials last 15 minutes) and pushes what the
/// registry lacks.
const PUSH_TRIES: u32 = 4;
const PUSH_PAUSE: Duration = Duration::from_secs(30);

/// What a push is asked for, checked before anything is built.
#[derive(Debug)]
struct Push {
    image: &'static str,
    dockerfile: &'static str,
    context: &'static str,
    account: String,
    repository: String,
    tag: String,
    summary: Option<PathBuf>,
}

impl Push {
    /// The name it is built and pushed under:
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

fn parse(rest: &[String], account: Option<&str>) -> Result<Push> {
    let usage = || anyhow::anyhow!(USAGE);
    let (verb, rest) = rest.split_first().ok_or_else(usage)?;
    anyhow::ensure!(verb == "push", USAGE);
    let (mut image, mut repository, mut tag, mut summary) = (None, None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repository" => repository = Some(it.next().ok_or_else(usage)?.clone()),
            "--tag" => tag = Some(it.next().ok_or_else(usage)?.clone()),
            "--summary" => summary = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            name if image.is_none() && !name.starts_with('-') => image = Some(name.to_string()),
            _ => return Err(usage()),
        }
    }
    let (image, repository, tag) = (image.ok_or_else(usage)?, repository.ok_or_else(usage)?, tag.ok_or_else(usage)?);
    let Some(&(image, dockerfile, context)) = IMAGES.iter().find(|(name, ..)| *name == image) else {
        bail!("no image {image:?} to build: {}", IMAGES.map(|(name, ..)| name).join(", "));
    };
    let account = account.filter(|a| !a.is_empty()).context("CLOUDFLARE_ACCOUNT_ID is not set: it names the account whose registry the image goes to, the deployment's own (its config's account_id)")?;
    anyhow::ensure!(account_ok(account), "CLOUDFLARE_ACCOUNT_ID is no account id (32 lowercase hex digits)");
    anyhow::ensure!(repository_ok(&repository), "--repository {repository:?} is no repository: lowercase letters and digits joined by `.`, `_` or `-`");
    anyhow::ensure!(tag_ok(&tag), "--tag {tag:?} is no tag: up to 128 letters, digits, `_`, `.` or `-`, not starting with `.` or `-`");
    Ok(Push { image, dockerfile, context, account: account.to_string(), repository, tag, summary })
}

/// `cargo xtask image …`.
pub fn image(rest: &[String]) -> Result<()> {
    let push = parse(rest, std::env::var("CLOUDFLARE_ACCOUNT_ID").ok().as_deref())?;
    // the pinned Node and wrangler first: one that cannot be had stops the
    // run before a build of minutes
    let tools = devstack::Tools::locate()?;
    let root = devstack::repo_root();
    let tagged = push.tagged();
    println!("building {} as {tagged}", push.image);
    crate::build::image_build(&json!({ "dockerfile": root.join(push.dockerfile), "build_context": root.join(push.context) }), Some(&tagged))?;
    tried(PUSH_TRIES, PUSH_PAUSE, |n| {
        println!("pushing {tagged} (try {n} of {PUSH_TRIES})");
        let mut cmd = crate::deploy::wrangler(&tools, &push.account)?;
        cmd.args(["containers", "push", &tagged]).stdin(Stdio::null());
        crate::run(&mut cmd)
    })?;
    let out = Command::new("docker").args(["image", "inspect", "--format", "{{json .RepoDigests}}", &tagged]).stdin(Stdio::null()).output().context("could not start docker image inspect")?;
    anyhow::ensure!(out.status.success(), "docker image inspect {tagged}: {}", String::from_utf8_lossy(&out.stderr).trim());
    let reference = pinned_of(&String::from_utf8_lossy(&out.stdout), &push.account, &push.repository)?;
    let text = summary(&push, &reference);
    println!("\n{text}");
    if let Some(file) = &push.summary {
        let mut f = fs::OpenOptions::new().create(true).append(true).open(file).with_context(|| format!("open {}", file.display()))?;
        writeln!(f, "{text}")?;
    }
    Ok(())
}

/// `attempt` until it succeeds, at most `tries` times, `pause` after each
/// that fails but the last; the last one's error when none succeeds.
fn tried(tries: u32, pause: Duration, mut attempt: impl FnMut(u32) -> Result<()>) -> Result<()> {
    assert!(tries > 0, "at least one try");
    for n in 1..=tries {
        match attempt(n) {
            Ok(()) => return Ok(()),
            Err(e) if n == tries => return Err(e.context(format!("the push failed {tries} times"))),
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

/// What a push says it made (and, with `--summary`, writes as Markdown):
/// the reference, readable with the account as `<account>`, and whole in
/// the canary's line of a deploy config's `computers.images`. A runner
/// masks the account in that line, as it is the environment's secret:
/// it is the config's own `account_id`.
fn summary(push: &Push, reference: &str) -> String {
    let readable = reference.replacen(&push.account, "<account>", 1);
    let name = format!("{}-canary", push.image);
    format!(
        "### {} pushed as {}:{}\n\n`{readable}`\n\nIts line in the deploy config's `computers.images` as the canary, the account the config's `account_id` (promoted, the same `image` is `\"{}\"`'s; docs/computers.md, \"One round\"):\n\n```jsonc\n\"{name}\": {{ \"image\": \"{reference}\" }},\n```\n",
        push.image, push.repository, push.tag, push.image
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Goal: a push is checked whole before anything is built (a build is
    /// minutes). Valid: the image, its repository and tag, the account
    /// from the environment, and the summary file. Invalid: an image xtask
    /// does not build, a repository or tag Docker or the deploy would
    /// refuse, a missing or malformed account, a flag without its value.
    #[test]
    fn a_push_is_checked_before_it_builds() {
        let p = parse(&args(&["push", "hermes", "--repository", "fragment-preview-computer-hermes-canary", "--tag", "0123456789ab", "--summary", "/tmp/s.md"]), Some(ACCOUNT)).unwrap();
        assert_eq!((p.image, p.dockerfile, p.context), ("hermes", "images/hermes/Dockerfile", "."));
        assert_eq!(p.tagged(), format!("registry.cloudflare.com/{ACCOUNT}/fragment-preview-computer-hermes-canary:0123456789ab"));
        assert_eq!(p.summary, Some(PathBuf::from("/tmp/s.md")));
        let refused = |list: &[&str], account: Option<&str>| parse(&args(list), account).unwrap_err().to_string();
        let ok = ["push", "hermes", "--repository", "fragment-computer-hermes", "--tag", "r9"];
        assert!(parse(&args(&ok), Some(ACCOUNT)).is_ok());
        assert!(refused(&["push", "stub", "--repository", "r", "--tag", "t"], Some(ACCOUNT)).contains("no image \"stub\""));
        assert!(refused(&["push", "hermes", "--repository", "Hermes", "--tag", "t"], Some(ACCOUNT)).contains("is no repository"));
        assert!(refused(&["push", "hermes", "--repository", "a/-b", "--tag", "t"], Some(ACCOUNT)).contains("is no repository"));
        let long = "a".repeat(129);
        for bad in ["-r9", ".r9", "r9:x", "r/9", long.as_str()] {
            assert!(refused(&["push", "hermes", "--repository", "r", "--tag", bad], Some(ACCOUNT)).contains("is no tag"), "{bad}");
        }
        assert!(refused(&ok, None).contains("CLOUDFLARE_ACCOUNT_ID is not set"));
        assert!(refused(&ok, Some("")).contains("CLOUDFLARE_ACCOUNT_ID is not set"));
        assert!(refused(&ok, Some(&ACCOUNT.to_uppercase())).contains("no account id"));
        assert!(refused(&ok, Some(&ACCOUNT[1..])).contains("no account id"));
        for list in [&["push", "hermes", "--repository", "r"][..], &["push", "hermes", "--tag"], &["pull", "hermes", "--repository", "r", "--tag", "t"], &[], &["push", "hermes", "hermes", "--repository", "r", "--tag", "t"]] {
            assert!(refused(list, Some(ACCOUNT)).starts_with("usage:"), "{list:?}");
        }
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

    /// Goal: a push that times out is tried again, a bounded number of
    /// times. Valid: two failures, then success, tried three times.
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
        assert_eq!(format!("{e:#}"), "the push failed 4 times: timed out 4");
    }

    /// The summary names the reference readably and whole, in the line a
    /// deploy config takes: one `deploy.rs` parses as a pinned image.
    #[test]
    fn the_summary_is_the_line_a_config_takes() {
        let p = parse(&args(&["push", "hermes", "--repository", "fragment-preview-computer-hermes-canary", "--tag", "r9"]), Some(ACCOUNT)).unwrap();
        let reference = format!("registry.cloudflare.com/{ACCOUNT}/fragment-preview-computer-hermes-canary@sha256:{}", "9f".repeat(32));
        let text = summary(&p, &reference);
        assert!(text.contains(&format!("`registry.cloudflare.com/<account>/fragment-preview-computer-hermes-canary@sha256:{}`", "9f".repeat(32))), "{text}");
        let line = text.lines().find(|l| l.starts_with("\"hermes-canary\"")).expect("the config's line");
        let images: serde_json::Value = serde_json::from_str(&format!("{{{}}}", line.trim_end_matches(','))).unwrap();
        assert_eq!(images["hermes-canary"]["image"], reference.as_str());
    }
}
