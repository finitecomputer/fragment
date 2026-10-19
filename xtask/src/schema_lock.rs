//! The schema lock (`crates/core/migrations/schema.lock`;
//! fragment_core::migrate::host): `cargo xtask schema-lock` writes it,
//! and `cargo xtask check` fails when it is not what the declared steps
//! make. The writer appends new steps and refuses to change or drop a
//! locked one. Both hold the tree's lock to master's too, step for step
//! (`host::lost`): a released step edited, its line deleted by hand and
//! locked again, passes the tree's own check, and not master's. Master is
//! `FRAGMENT_SCHEMA_BASE` (a git ref: CI's, a pull request's base or what
//! a push to master moved it from), or else `fragment-rs/master` or
//! `origin/master`, whichever git knows.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use fragment_core::migrate::host::{self, Drift};
use fragment_core::migrate::CLASSES;

/// `cargo xtask schema-lock`.
pub fn write(root: &Path) -> Result<()> {
    let path = root.join(host::LOCK_PATH);
    let locked = std::fs::read_to_string(&path).unwrap_or_default();
    let drift = host::drift(&locked, &CLASSES);
    let refused: Vec<&str> = drift.iter().filter(|d| !matches!(d, Drift::Unlocked(_))).map(Drift::message).collect();
    if !refused.is_empty() {
        bail!("{} keeps its locked steps, so it is not written:\n  {}", host::LOCK_PATH, refused.join("\n  "));
    }
    let lock = host::platform_lock().map_err(unapplied)?;
    if let Some((base, at)) = master_lock(root)? {
        let lost = host::lost(&base, &lock);
        if !lost.is_empty() {
            bail!("the lock would lose {at}'s steps, so it is not written:\n  {}", lost.join("\n  "));
        }
    }
    if lock == locked {
        println!("{} is current", host::LOCK_PATH);
        return Ok(());
    }
    std::fs::write(&path, &lock).with_context(|| format!("write {}", path.display()))?;
    println!("wrote {}: commit it with the steps", host::LOCK_PATH);
    Ok(())
}

/// That the lock is what the declared steps make: `cargo xtask check`'s.
pub fn holds(root: &Path) -> Result<()> {
    let path = root.join(host::LOCK_PATH);
    let locked = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let drift = host::drift(&locked, &CLASSES);
    if !drift.is_empty() {
        let found: Vec<&str> = drift.iter().map(Drift::message).collect();
        bail!("the Durable Objects' schemas changed without their steps ({}):\n  {}", host::LOCK_PATH, found.join("\n  "));
    }
    let lock = host::platform_lock().map_err(unapplied)?;
    if lock != locked {
        bail!("{} is not what the steps make (edited by hand?): run `cargo xtask schema-lock`", host::LOCK_PATH);
    }
    match master_lock(root)? {
        Some((base, at)) => {
            let lost = host::lost(&base, &locked);
            if !lost.is_empty() {
                bail!("the Durable Objects' schemas lost steps {at} released ({}):\n  {}", host::LOCK_PATH, lost.join("\n  "));
            }
        }
        None if std::env::var_os("CI").is_some() => bail!("CI holds the schema lock to master's: name it in FRAGMENT_SCHEMA_BASE"),
        None => println!("(the schema lock is not held to master's: no fragment-rs/master or origin/master here)"),
    }
    Ok(())
}

/// A step that does not apply to a new object, as the check says it.
fn unapplied(e: fragment_core::migrate::SchemaError) -> anyhow::Error {
    let refusal = if e.to_string().contains("not authorized") {
        " (a Durable Object's SQLite refuses PRAGMAs that write, transactions and savepoints, ATTACH and TEMP objects; the host's refuses them as it does)"
    } else {
        ""
    };
    anyhow::anyhow!("the steps do not apply to a new object: {e}{refusal}")
}

/// Master's lock and the ref it was read at: none when no ref names
/// master here (a clone without the remote; CI names one), empty when
/// master has no lock yet.
fn master_lock(root: &Path) -> Result<Option<(String, String)>> {
    let named = std::env::var("FRAGMENT_SCHEMA_BASE").ok().filter(|r| !r.is_empty());
    let refs = match &named {
        Some(r) => vec![r.clone()],
        None => vec!["fragment-rs/master".to_string(), "origin/master".to_string()],
    };
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(root).args(args).output().context("run git");
    for r in refs {
        if !git(&["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")])?.status.success() {
            anyhow::ensure!(named.is_none(), "FRAGMENT_SCHEMA_BASE names {r}, which git does not know: fetch it");
            continue;
        }
        let shown = git(&["show", &format!("{r}:{}", host::LOCK_PATH)])?;
        let base = if shown.status.success() { String::from_utf8(shown.stdout).context("master's lock is UTF-8")? } else { String::new() };
        return Ok(Some((base, r)));
    }
    Ok(None)
}
