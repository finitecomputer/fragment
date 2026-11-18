//! Main as one pass reads it: its head, and the files at that head.

use super::scan::syncable;
use super::SyncError;
use crate::codestorage::{Change, CodeStorage, MAIN};
use fragment_core::codestorage::TreeEntry;
use fragment_proto::{NotRepoPath, RepoPath};
use std::collections::HashMap;

/// main as one pass reads it: its head, and the files at that head (read
/// at the head's commit, not at `main`, so the two always agree).
pub(super) struct Listing {
    pub(super) head: Option<String>,
    pub(super) files: HashMap<RepoPath, TreeEntry>,
    /// the paths at the head that are not repo paths: never pulled
    pub(super) refused: Vec<NotRepoPath>,
}

impl Listing {
    /// The ref a pass reads files at: the listing's own commit, so a read
    /// sees the bytes the listing describes even if main moved since.
    pub(super) fn rev(&self) -> &str {
        self.head.as_deref().unwrap_or(MAIN)
    }

    /// main once our commit `tip` landed on this listing's head. The
    /// commit named this head as its expected parent, so our changes are
    /// the only difference, and listing main again would only read back
    /// what we just wrote.
    pub(super) fn landed(mut self, tip: String, changes: &[Change]) -> Listing {
        for change in changes {
            match change {
                Change::Upsert { path, bytes } => {
                    let entry = TreeEntry { path: path.clone(), size: bytes.len() as u64, mode: FILE_MODE.to_string(), last_commit_sha: tip.clone() };
                    self.files.insert(path.clone(), entry);
                }
                Change::Delete { path } => {
                    self.files.remove(path);
                }
            }
        }
        assert!(self.files.values().all(|f| !f.last_commit_sha.is_empty()), "every entry names its commit");
        Listing { head: Some(tip), files: self.files, refused: self.refused }
    }
}

/// The git mode of every file sync commits (the commit pack's default).
const FILE_MODE: &str = "100644";

pub(super) fn list_main(storage: &CodeStorage) -> Result<Listing, SyncError> {
    list_branch(storage, MAIN)
}

pub(super) fn list_branch(storage: &CodeStorage, branch: &str) -> Result<Listing, SyncError> {
    let Some(head) = storage.branch_head(branch)? else {
        return Ok(Listing { head: None, files: HashMap::new(), refused: vec![] }); // empty repo: everything local is new
    };
    let listed = storage.list_files(&head)?;
    let files = listed.entries.into_iter().filter(|f| syncable(f.path.as_str())).map(|f| (f.path.clone(), f)).collect();
    Ok(Listing { head: Some(head), files, refused: listed.refused })
}
