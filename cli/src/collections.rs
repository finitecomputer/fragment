//! A local reference manifest. It confers no authority and writes nothing.
use crate::{api::Client, brain_search, sync};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    version: u32,
    pub host: String,
    sources: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    alias: String,
    fragment: String,
    dir: PathBuf,
}
#[derive(Serialize)]
pub struct Disposition {
    alias: String,
    fragment: String,
    dir: PathBuf,
    /// checked, denied, unavailable, or unchecked_offline; never a cached grant.
    access: String,
    local: String,
    reason: Option<String>,
    skipped: Vec<brain_search::Skipped>,
}
#[derive(Serialize)]
pub struct Hit {
    alias: String,
    source_rank: usize,
    #[serde(flatten)]
    hit: brain_search::Hit,
}
#[derive(Serialize)]
pub struct Answer {
    host: String,
    pub sources: Vec<Disposition>,
    q: Option<String>,
    pub results: Vec<Hit>,
}

pub fn load(path: &Path) -> Result<Manifest> {
    // This is trusted local configuration, never an automatically opened remote file.
    let bytes = fs::read(path).context("read collection manifest")?;
    ensure!(bytes.len() <= 65536, "collection manifest exceeds 64 KiB");
    let manifest: Manifest = serde_json::from_slice(&bytes).context("parse collection manifest")?;
    ensure!(manifest.version == 1, "collection version must be 1");
    ensure!(
        !manifest.host.is_empty() && !manifest.host.ends_with('/'),
        "collection host must be nonempty, without trailing slash"
    );
    ensure!(
        (1..=32).contains(&manifest.sources.len()),
        "collection needs 1–32 sources"
    );
    let mut aliases = BTreeSet::new();
    for s in &manifest.sources {
        ensure!(
            !s.alias.is_empty()
                && s.alias.len() <= 64
                && s.alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "alias must be 1–64 ASCII letters, digits, - or _"
        );
        ensure!(
            aliases.insert(&s.alias),
            "duplicate collection alias: {}",
            s.alias
        );
        ensure!(
            fragment_proto::valid_fragment_name(&s.fragment),
            "source needs a full fragment name: {}",
            s.fragment
        );
        ensure!(!s.dir.as_os_str().is_empty(), "source directory is empty");
    }
    Ok(manifest)
}

pub fn browse(
    manifest: Manifest,
    path: &Path,
    client: Option<&Client>,
    query: Option<&str>,
    limit: u32,
) -> Result<Answer> {
    if let Some(q) = query {
        brain_search::validate_query(q).map_err(anyhow::Error::msg)?;
    }
    ensure!((1..=50).contains(&limit), "limit must be 1–50");
    if let Some(c) = client {
        // A manifest must never choose where credentials are sent.
        ensure!(
            manifest.host == c.host,
            "collection host differs from CLI host; select its host explicitly"
        );
    }
    let mut sources = Vec::new();
    let mut hits = Vec::new();
    for s in manifest.sources {
        let dir = path.parent().unwrap_or(Path::new(".")).join(&s.dir);
        let mut disposition = Disposition {
            alias: s.alias.clone(),
            fragment: s.fragment.clone(),
            dir: dir.clone(),
            access: "unchecked_offline".into(),
            local: "unavailable".into(),
            reason: None,
            skipped: Vec::new(),
        };
        let read = (|| -> Result<Vec<brain_search::Hit>> {
            // Check before even opening a local journal. No fallback on any refusal.
            let remote = if let Some(c) = client {
                disposition.access = "unavailable".into();
                let r = c.get(&format!("/api/f/{}/status", s.fragment))?;
                if !r.ok() {
                    if matches!(r.status, 401 | 403 | 404 | 410) {
                        disposition.access = "denied".into();
                    }
                    // Don't echo response bodies (status can carry private metadata).
                    anyhow::bail!("source access check returned HTTP {}", r.status);
                }
                let status: fragment_proto::FragmentStatus = c.call_as(r)?;
                ensure!(status.name == s.fragment, "source identity differs");
                disposition.access = "checked".into();
                Some(status)
            } else {
                None
            };
            ensure!(dir.is_dir(), "local directory unavailable");
            let state = sync::load_state(&dir, &s.fragment)?;
            ensure!(state.host.as_deref() == Some(manifest.host.as_str()) && state.repo.is_some(), "source needs a sync journal bound to this host and repository; sync it separately first");
            if let Some(remote) = remote {
                ensure!(
                    state.repo.as_deref() == Some(remote.repo.as_str()),
                    "source repository changed; sync it separately first"
                );
            }
            disposition.local = "journal_bound".into();
            if let Some(q) = query {
                let answer = brain_search::search(&s.fragment, &dir, q, limit)?;
                disposition.skipped = answer.skipped;
                disposition.local = "searched_current".into();
                Ok(answer.results)
            } else {
                Ok(Vec::new())
            }
        })();
        match read {
            Ok(results) => hits.push(
                results
                    .into_iter()
                    .map(|hit| Hit {
                        alias: s.alias.clone(),
                        source_rank: hit.rank,
                        hit,
                    })
                    .collect::<Vec<_>>(),
            ),
            Err(e) => {
                disposition.local = "unavailable".into();
                disposition.reason = Some(format!("{e:#}"));
                hits.push(Vec::new());
            }
        }
        sources.push(disposition);
    }
    // BM25 scores from independent corpora aren't comparable. Interleave each
    // source's ranked hits in manifest order, with a global result limit.
    let mut results = Vec::new();
    let mut iters: Vec<_> = hits.into_iter().map(Vec::into_iter).collect();
    for _ in 0..limit {
        for iter in &mut iters {
            if results.len() == limit as usize {
                break;
            }
            if let Some(mut h) = iter.next() {
                h.hit.rank = results.len() + 1;
                results.push(h);
            }
        }
    }
    Ok(Answer {
        host: manifest.host,
        sources,
        q: query.map(str::to_owned),
        results,
    })
}

pub fn print(answer: &Answer) -> Result<()> {
    for s in &answer.sources {
        println!(
            "{} -> {} [{}; {}]{}",
            s.alias,
            s.fragment,
            s.access,
            s.local,
            s.reason
                .as_ref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        );
        for skip in &s.skipped {
            eprintln!("skipped {}:{}: {}", s.alias, skip.path, skip.reason);
        }
    }
    for h in &answer.results {
        println!(
            "{}. {} ({}):{} [{}; source rank {}] {}",
            h.hit.rank,
            h.alias,
            h.hit.fragment,
            h.hit.path,
            h.hit.state,
            h.source_rank,
            h.hit.title
        );
        println!("  {}", h.hit.snippet);
    }
    Ok(())
}
