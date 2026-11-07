//! The wire's schema and its lock (docs/wire): `cargo xtask wire` writes
//! the OpenAPI document, its reference and the lock from crates/proto (its
//! routes.rs and the types it names); `cargo xtask check` fails when any
//! is not what proto makes, or when the lock breaks a fact master's holds
//! in a way an older client would feel (lock.rs), but those wire.retired
//! names. Before any of it, the types the routes reach are read from
//! crates/proto's source and held to the serde attributes the wire
//! supports (source.rs); then every default the document states is held
//! to serde's reading of its field left out (defaults.rs).
//! wire.retired is appended to by hand, never edited: master's
//! lines stay as they are. Master is read as the schema lock reads it
//! (`schema_lock::master_file`).
//!
//! The threat model: the lock guards against honest mistakes. A change to
//! proto written in ordinary Rust that would break an older client or
//! computer must never pass. A deliberate evasion inside proto's own
//! source, code written so that the guard reads one thing while rustc
//! compiles another, is out of scope beyond source.rs's cheap rules
//! (proto's manifest holds the rest to the compiler: its dependencies,
//! and `non_local_definitions` forbidden); code review catches the rest.
//! What is left is in the PR's "Known gaps" and FIN-426. The planned
//! successor is schema-first (FIN-379's option B): proto's Rust generated
//! from a schema file, when the web and iOS generators come.

mod defaults;
#[cfg(test)]
#[allow(dead_code)]
mod fixtures;
mod lock;
mod openapi;
mod reference;
mod source;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};

pub const OPENAPI_PATH: &str = "docs/wire/openapi.json";
pub const REFERENCE_PATH: &str = "docs/wire/README.md";
pub const LOCK_PATH: &str = "crates/proto/wire.lock";
pub const RETIRED_PATH: &str = "crates/proto/wire.retired";

const RETIRED_HEADER: &str = "\
# Wire facts retired on purpose (crates/proto/wire.lock; xtask/src/wire/).
# Each line waives exactly the fact it names, once the clients that read
# or send it are gone: `retire <YYYY-MM-DD> <the lock's line> # <why, and
# the client floor that makes it safe>`. Append only: master's lines stay
# as they are.
";

/// What crates/proto makes: each file's text.
pub struct Generated {
    pub openapi: String,
    pub lock: String,
    pub reference: String,
}

pub fn generate() -> Result<Generated> {
    let doc = openapi::document()?;
    // what serde reads of the types reached, from their source, holds to
    // the supported list (source.rs), before anything is read of their schemas
    let src = source::Source::read_dir(&source::proto_src())?;
    let mut roots = src.route_types();
    // the refusal every route may answer
    roots.push((syn::parse_str("ErrorBody").context("a refusal's type")?, "crate".to_string()));
    let components: Vec<String> = doc["components"]["schemas"].as_object().context("the document has components")?.keys().cloned().collect();
    let guarded = source::guard(&src, &roots, &components, &source::Allowed::wire())?;
    // with the facts serde's source says that the schema does not
    let lock = lock::project_with(&doc, &guarded.facts)?;
    // each default it states is what serde reads of its field left out
    defaults::hold(&doc, &lock)?;
    let mut openapi = serde_json::to_string_pretty(&doc).context("the document is JSON")?;
    openapi.push('\n');
    Ok(Generated { lock, reference: reference::render(&doc)?, openapi })
}

/// The files as the tree holds them; `None` for one it does not.
#[derive(Default)]
pub struct OnDisk {
    pub openapi: Option<String>,
    pub lock: Option<String>,
    pub reference: Option<String>,
    pub retired: Option<String>,
}

impl OnDisk {
    pub fn read(root: &Path) -> OnDisk {
        let read = |p: &str| std::fs::read_to_string(root.join(p)).ok();
        OnDisk { openapi: read(OPENAPI_PATH), lock: read(LOCK_PATH), reference: read(REFERENCE_PATH), retired: read(RETIRED_PATH) }
    }
}

/// Master's lock and retirements, and the ref they were read at.
pub struct Base {
    pub lock: String,
    pub retired: String,
    pub at: String,
}

fn base(root: &Path) -> Result<Option<Base>> {
    let Some((lock, at)) = crate::schema_lock::master_file(root, LOCK_PATH)? else { return Ok(None) };
    let (retired, _) = crate::schema_lock::master_file(root, RETIRED_PATH)?.context("master's ref is still there")?;
    Ok(Some(Base { lock, retired, at }))
}

fn in_ci() -> bool {
    std::env::var_os("CI").is_some()
}

/// `cargo xtask wire`.
pub fn write(root: &Path) -> Result<()> {
    let generated = generate()?;
    match write_files(root, &generated)?.as_slice() {
        [] => println!("the wire's files are current"),
        paths => println!("wrote {}: commit them with the change", paths.join(", ")),
    }
    // what it breaks, now rather than in check
    verdict(&generated, &OnDisk::read(root), base(root)?.as_ref(), false)
}

/// Writes each generated file that differs, and wire.retired's header if
/// there is none: the paths it wrote.
pub fn write_files(root: &Path, generated: &Generated) -> Result<Vec<&'static str>> {
    let disk = OnDisk::read(root);
    let mut wrote = vec![];
    for (path, text, was) in [
        (OPENAPI_PATH, &generated.openapi, &disk.openapi),
        (REFERENCE_PATH, &generated.reference, &disk.reference),
        (LOCK_PATH, &generated.lock, &disk.lock),
    ] {
        if was.as_ref() != Some(text) {
            let at = root.join(path);
            std::fs::create_dir_all(at.parent().expect("a file is in a directory")).with_context(|| format!("make {path}'s directory"))?;
            std::fs::write(&at, text).with_context(|| format!("write {path}"))?;
            wrote.push(path);
        }
    }
    if disk.retired.is_none() {
        let at = root.join(RETIRED_PATH);
        std::fs::create_dir_all(at.parent().expect("a file is in a directory")).with_context(|| format!("make {RETIRED_PATH}'s directory"))?;
        std::fs::write(&at, RETIRED_HEADER).with_context(|| format!("write {RETIRED_PATH}"))?;
        wrote.push(RETIRED_PATH);
    }
    Ok(wrote)
}

/// That the files are what proto makes, and hold master's facts:
/// `cargo xtask check`'s.
pub fn holds(root: &Path) -> Result<()> {
    verdict(&generate()?, &OnDisk::read(root), base(root)?.as_ref(), in_ci())
}

/// The check, apart from where its inputs come from: each file is what
/// proto makes, wire.retired is well formed and master's lines are still
/// its first, and nothing master's lock holds is broken but what it names.
/// With no master to read, CI refuses (as the schema lock does).
pub fn verdict(generated: &Generated, disk: &OnDisk, base: Option<&Base>, ci: bool) -> Result<()> {
    for (path, text, was) in [
        (OPENAPI_PATH, &generated.openapi, &disk.openapi),
        (REFERENCE_PATH, &generated.reference, &disk.reference),
        (LOCK_PATH, &generated.lock, &disk.lock),
    ] {
        match was {
            None => bail!("{path} is missing: run `cargo xtask wire` and commit it"),
            Some(was) if was != text => bail!("{path} is not what crates/proto makes (edited by hand, or a type changed): run `cargo xtask wire` and commit it"),
            Some(_) => {}
        }
    }
    let retired_text = disk.retired.as_deref().with_context(|| format!("{RETIRED_PATH} is missing: run `cargo xtask wire`"))?;
    let retired = retired(retired_text)?;
    let Some(base) = base else {
        if ci {
            bail!("CI holds the wire lock to master's: name it in FRAGMENT_SCHEMA_BASE");
        }
        println!("(the wire lock is not held to master's: no fragment-rs/master or origin/master here)");
        return Ok(());
    };
    if !retired_text.starts_with(base.retired.as_str()) {
        bail!("{RETIRED_PATH} changes or drops lines {} holds: retirements are only ever appended", base.at);
    }
    let found = lock::breaches(&lock::parse(&base.lock)?, &lock::parse(&generated.lock)?, &retired);
    if !found.is_empty() {
        let listed: Vec<String> = found.iter().map(|b| format!("{}\n    {}", b.fact, b.why)).collect();
        bail!(
            "the wire breaks what {} holds, for older clients ({LOCK_PATH}):\n  {}\nKeep the old shape beside the new one, or, once the clients that use it are gone, retire the fact in {RETIRED_PATH}: `retire <YYYY-MM-DD> <the line> # <why, and the client floor>`",
            base.at,
            listed.join("\n  ")
        );
    }
    Ok(())
}

/// wire.retired's facts: each line `retire <YYYY-MM-DD> <fact> # <why>`.
pub fn retired(text: &str) -> Result<BTreeSet<String>> {
    let mut facts = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |what: &str| format!("{RETIRED_PATH} line {}: {what}: {line:?}", n + 1);
        let rest = line.strip_prefix("retire ").with_context(|| bad("a line is `retire <date> <fact> # <why>`"))?;
        let (date, rest) = rest.split_once(' ').with_context(|| bad("no fact"))?;
        let b = date.as_bytes();
        let dated = b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
        if !dated {
            bail!(bad("the date is YYYY-MM-DD"));
        }
        let (fact, why) = rest.split_once(" # ").with_context(|| bad("no `# why`"))?;
        if why.trim().is_empty() {
            bail!(bad("no why"));
        }
        if fact.trim().is_empty() || fact.starts_with('#') {
            bail!(bad("no fact"));
        }
        lock::parse_fact(fact).with_context(|| bad("not one of the lock's facts"))?;
        facts.insert(fact.trim().to_string());
    }
    Ok(facts)
}
