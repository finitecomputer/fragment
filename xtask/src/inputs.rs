//! Content identity shared by the preview image and CLI releases. Git's
//! path/mode/blob records survive unrelated commits and cache eviction.
use std::{path::Path, process::Command};
use anyhow::Result;

pub(crate) fn key(root: &Path, paths: &[&str]) -> Result<String> {
    let status = Command::new("git").args(["status", "--porcelain", "--untracked-files=normal", "--"]).args(paths).current_dir(root).output()?;
    anyhow::ensure!(status.status.success() && status.stdout.is_empty(), "build inputs must be committed and clean");
    let tree = Command::new("git").args(["ls-tree", "-rz", "HEAD", "--"]).args(paths).current_dir(root).output()?;
    anyhow::ensure!(tree.status.success() && !tree.stdout.is_empty(), "git did not name the build inputs");
    Ok(fragment_core::blob::sha256_hex(&tree.stdout))
}
