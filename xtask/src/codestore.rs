//! `cargo xtask codestore`: a dev stack's repos carried from the
//! code.storage fake into macrofiche (docs/self-host.md, seam 5), for a box
//! that ran the fake and now keeps its apps' source in macrofiche.
//!
//! - `carry --state <codestorage.json> --config <macrofiche.json> --org <org>`:
//!   each repo of the fake's saved state imported (`macrofiche import`,
//!   `MACROFICHE_BIN`) while nothing serves macrofiche's state, keeping the
//!   url form and id the cell holds it by and each commit's fake id
//!   (crates/fakes, `codestorage::carry`). A repo already there is left as
//!   it is: `verify` says whether it is whole.
//! - `verify --state <codestorage.json> --url <macrofiche> --org <org>
//!   --key-file <org key>`: against macrofiche serving, every branch's
//!   every file, and every file at every fake commit id, read back byte
//!   for byte, and each one's tree listed, as the cell will ask for them.
//!   Nothing is written.
//!
//! The fake's state is read, never changed: it is the carry's rollback.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fragment_core::codestorage::{self as cs, Claims, OrgKey};
use fragment_devstack as devstack;
use fragment_fakes::codestorage::carry::{read_state, StateRepo};

/// One call's deadline.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Pages one listing reads, 1000 entries each.
const LISTING_PAGES_MAX: usize = 100;
/// The verifier's tokens.
const TOKEN_TTL_S: i64 = 300;

pub fn codestore(args: &[String]) -> Result<()> {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let need = |name: &str| flag(name).with_context(|| format!("codestore {} needs {name}", args.first().map(String::as_str).unwrap_or("")));
    match args.first().map(String::as_str) {
        Some("carry") => carry(Path::new(&need("--state")?), Path::new(&need("--config")?), &need("--org")?),
        Some("verify") => verify(Path::new(&need("--state")?), &need("--url")?, &need("--org")?, Path::new(&need("--key-file")?)),
        _ => bail!("usage: cargo xtask codestore carry --state <codestorage.json> --config <macrofiche.json> --org <org> | verify --state <codestorage.json> --url <macrofiche> --org <org> --key-file <org key>"),
    }
}

/// Every repo of `state` imported into the macrofiche `config` names.
fn carry(state: &Path, config: &Path, org: &str) -> Result<()> {
    let bin = devstack::codestore::macrofiche_bin()?;
    let repos = read_state(state)?;
    for r in &repos {
        r.whole()?;
    }
    let work = config.parent().unwrap_or(Path::new(".")).join("carry");
    std::fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;
    let (mut made, mut there, mut commits) = (0usize, 0usize, 0usize);
    for r in &repos {
        let c = r.carried();
        let stream = work.join(format!("{}.fi", c.url));
        std::fs::write(&stream, &c.stream).with_context(|| format!("write {}", stream.display()))?;
        let mut cmd = Command::new(&bin);
        cmd.args(["import", "--config"]).arg(config).args(["--org", org, "--name", &c.name, "--repo-id", &c.repo_id, "--created-at-ms", &c.created_at_ms.to_string()]);
        // a repo the fake made by its name (`seed_repo`) keeps it as its url form
        if c.url != c.name {
            cmd.args(["--url", &c.url]);
        }
        let out = cmd.arg(&stream).output().with_context(|| format!("run {}", bin.display()))?;
        let err = String::from_utf8_lossy(&out.stderr);
        match out.status.success() {
            true => {
                made += 1;
                commits += c.commits;
                println!("carried {} ({}): {} commits, {} bytes of stream", c.url, c.name, c.commits, c.stream.len());
            }
            false if err.contains("is taken") => {
                there += 1;
                println!("left   {} ({}): already in macrofiche (verify says whether it is whole)", c.url, c.name);
            }
            false => bail!("macrofiche import {} ({}): {}", c.url, c.name, err.trim()),
        }
    }
    println!("{made} repos carried ({commits} commits), {there} already there, of {} in {}", repos.len(), state.display());
    Ok(())
}

/// What a store holds of a repo, through its API (`cell/src/cs.rs`'s calls).
struct Store {
    url: String,
    org: String,
    key: OrgKey,
    http: reqwest::blocking::Client,
}

impl Store {
    fn get(&self, repo: &str, op: &str, query: &[(&str, &str)]) -> Result<(u16, Vec<u8>)> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        let token = self.key.token(&Claims { iss: &self.org, sub: "codestore-verify", repo, scopes: &["git:read"], iat: now, exp: now + TOKEN_TTL_S });
        let r = self.http.get(format!("{}/api/repos/{repo}/{op}", self.url)).query(query).bearer_auth(token).send().with_context(|| format!("GET {op} of {repo}"))?;
        Ok((r.status().as_u16(), r.bytes()?.to_vec()))
    }

    fn head(&self, repo: &str, branch: &str) -> Result<Option<String>> {
        match self.get(repo, "branch", &[("name", branch)])? {
            (200, b) => Ok(cs::branch_head(&serde_json::from_slice(&b)?)),
            (404, _) => Ok(None),
            (s, b) => bail!("GET branch {branch} of {repo}: {s} {}", String::from_utf8_lossy(&b)),
        }
    }

    /// Path to size, at `at`, every page.
    fn tree(&self, repo: &str, at: &str) -> Result<BTreeMap<String, u64>> {
        let mut out = BTreeMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..LISTING_PAGES_MAX {
            let mut q = vec![("ref", at), ("limit", "1000")];
            if let Some(c) = &cursor {
                q.push(("cursor", c));
            }
            let (s, b) = self.get(repo, "files/metadata", &q)?;
            if s != 200 {
                bail!("GET files/metadata at {at} of {repo}: {s} {}", String::from_utf8_lossy(&b));
            }
            let page = cs::tree_page(&serde_json::from_slice(&b)?).map_err(|e| anyhow::anyhow!("{e}"))?;
            out.extend(page.entries.into_iter().map(|e| (e.path.to_string(), e.size)));
            match page.next {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
        bail!("{repo} lists more than {LISTING_PAGES_MAX} pages at {at}")
    }
}

/// The fake's tree at `commit`: path to size.
fn fake_tree(r: &StateRepo, commit: &str) -> BTreeMap<String, u64> {
    r.trees[commit].iter().map(|(p, (_, size))| (p.clone(), *size)).collect()
}

/// Every repo of `state` read back from macrofiche at `url`.
fn verify(state: &Path, url: &str, org: &str, key_file: &Path) -> Result<()> {
    let key = OrgKey::from_pem(&std::fs::read_to_string(key_file).with_context(|| format!("read {}", key_file.display()))?).map_err(|e| anyhow::anyhow!("{}: {e}", key_file.display()))?;
    let store = Store { url: url.trim_end_matches('/').into(), org: org.into(), key, http: reqwest::blocking::Client::builder().timeout(CALL_TIMEOUT).build()? };
    let repos = read_state(state)?;
    let (mut files, mut bytes, mut ids, mut id_files, mut bad) = (0usize, 0u64, 0usize, 0usize, Vec::<String>::new());
    for r in &repos {
        for (branch, fake_head) in &r.branches {
            let Some(head) = store.head(&r.url, branch)? else {
                bad.push(format!("{} has no {branch}", r.url));
                continue;
            };
            let want = fake_tree(r, fake_head);
            let got = store.tree(&r.url, &head)?;
            if got != want {
                bad.push(format!("{}'s {branch} lists {} files, not the fake's {}", r.url, got.len(), want.len()));
                continue;
            }
            for (path, (blob, _)) in &r.trees[fake_head] {
                let (s, b) = store.get(&r.url, "file", &[("path", path), ("ref", &head)])?;
                if s != 200 || b != r.blobs[blob] {
                    bad.push(format!("{}'s {branch}: {path} reads {s}, {} bytes, not the fake's {}", r.url, b.len(), r.blobs[blob].len()));
                }
                files += 1;
                bytes += b.len() as u64;
            }
        }
        // each fake id, as a cell's pin names it: its tree, and every
        // file's bytes there (a commit no branch names now included)
        for id in r.trees.keys() {
            match store.tree(&r.url, id) {
                Ok(t) if t == fake_tree(r, id) => {}
                Ok(t) => {
                    bad.push(format!("{} at the fake's {id} lists {} files, not {}", r.url, t.len(), r.trees[id].len()));
                    continue;
                }
                Err(e) => {
                    bad.push(format!("{} at the fake's {id}: {e:#}", r.url));
                    continue;
                }
            }
            let mut whole = true;
            for (path, (blob, _)) in &r.trees[id] {
                let (s, b) = store.get(&r.url, "file", &[("path", path), ("ref", id)])?;
                if s != 200 || b != r.blobs[blob] {
                    bad.push(format!("{} at the fake's {id}: {path} reads {s}, {} bytes, not the fake's {}", r.url, b.len(), r.blobs[blob].len()));
                    whole = false;
                }
                id_files += 1;
            }
            ids += usize::from(whole);
        }
    }
    if !bad.is_empty() {
        bail!("{} mismatches between the fake and macrofiche:\n  {}", bad.len(), bad.join("\n  "));
    }
    println!("verified {} repos: {files} files ({bytes} bytes) on their branches, and {ids} fake commit ids ({id_files} files at them), each its tree, byte for byte", repos.len());
    Ok(())
}
