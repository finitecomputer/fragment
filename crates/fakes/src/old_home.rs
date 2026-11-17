//! The old agent's home the importer is tested on (FIN-463):
//! `fixtures/old-home`, synthetic and already filtered, laid out as
//! finite.computer kept one, and its layout (`fixtures/old-home.layout.json`).
//! A stage brings a home as one zip (zip64): `stage` makes one of it.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The home's folder.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/old-home")
}

/// Its layout file.
pub fn layout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/old-home.layout.json")
}

/// Every file of the home, by its path relative to `root`, in path order.
fn files() -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut folders = vec![root()];
    while let Some(dir) = folders.pop() {
        for e in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = e?.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let rel = path.strip_prefix(root())?.to_str().context("a fixture's path is text")?.to_string();
                out.push((rel, path));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The home as one zip at `to`, as a stage brings it, with `extra` files
/// (path, bytes) beside its own.
pub fn stage(to: &Path, extra: &[(&str, Vec<u8>)]) -> Result<()> {
    let mut all: Vec<(String, Vec<u8>)> = Vec::new();
    for (rel, path) in files()? {
        all.push((rel, std::fs::read(&path)?));
    }
    all.extend(extra.iter().map(|(p, b)| (p.to_string(), b.clone())));
    zip(to, &all)
}

/// `files` (path, bytes) as a zip at `to`, as a stage writes one: zip64,
/// each entry stored (a stage's bytes are mostly media, which deflate
/// gains nothing on).
pub fn zip(to: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    let mut w = zip::ZipWriter::new(std::fs::File::create(to).with_context(|| format!("writing {}", to.display()))?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    for (rel, bytes) in files {
        w.start_file(rel.as_str(), options)?;
        w.write_all(bytes)?;
    }
    w.finish()?;
    Ok(())
}
