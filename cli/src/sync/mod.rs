// Thin sync over the code.storage commit builder — commit without a local
// clone:
//   push  = scan folder, diff against the branch-head listing, ONE
//           commit-pack with expected-parent CAS; conflict -> refetch head,
//           rebuild the diff, retry (bounded, explicit error after)
//   pull  = fetch tree/files from main into the folder
//   mirror= push then pull
// No local .git, no git2, no host file API. `.fragment/state.json` is a
// stat cache only (path -> content sha + mtime + last-seen commit); the
// repo is the truth, the folder is a disposable working copy.

mod adopt;
mod at_once;
mod conflicts;
mod folder;
mod journal;
mod listing;
mod outside;
mod pass;
mod plan;
mod refuse;
mod report;
mod scan;
#[cfg(test)]
pub(crate) mod tests;
mod verify;

pub use pass::{pass_over, refresh_pins, sync_once};
pub use refuse::{refuse_dir, DirRefused};
pub use report::Report;
pub use journal::SyncLock;
pub use scan::{read_local, syncable};
pub use verify::verify;

use crate::api::CodedError;
use crate::codestorage::{CsError, Wants};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Push,
    Pull,
    Mirror,
}

impl Mode {
    /// The token a pass asks for: a pull's only reads; a push's writes; a
    /// mirror's writes where the host grants it, else reads (a viewer's
    /// mirror pulls, and sends nothing).
    pub fn wants(self) -> Wants {
        match self {
            Mode::Pull => Wants::Read,
            Mode::Push => Wants::Write,
            Mode::Mirror => Wants::Either,
        }
    }
}

#[derive(Clone)]
pub struct SyncOptions {
    pub mode: Mode,
    pub apply_mass_delete: bool,
    /// in pull mode, delete local files that were deleted remotely
    /// (pull never deletes without it; mirror always propagates)
    pub prune: bool,
    /// in pull mode, pull `live` rather than main
    pub live: bool,
    pub writer_id: String, // 8 hex of our pubkey, for conflict-copy names
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            mode: Mode::Mirror,
            apply_mass_delete: false,
            prune: false,
            live: false,
            writer_id: "anon".into(),
        }
    }
}

/// Typed sync errors (engineering style: no anyhow in the sync module).
#[derive(Debug)]
pub enum SyncError {
    Cs(CsError),
    /// the fragment host refused a blob's upload or download: its code
    /// (budget, size, role) is the one to act on
    Host(CodedError),
    Io(String),
    /// the folder's journal belongs to a different repo than the fragment
    /// being synced — refuse before touching anything
    Repo(String),
    /// a folder sync never sends (`refuse_dir`), found by any pass's read
    Refused(DirRefused),
}
impl From<CsError> for SyncError {
    fn from(e: CsError) -> Self {
        SyncError::Cs(e)
    }
}
impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> Self {
        SyncError::Io(e.to_string())
    }
}
impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Cs(e) => write!(f, "{e}"),
            SyncError::Host(e) => write!(f, "{e}"),
            SyncError::Io(e) => write!(f, "io: {e}"),
            SyncError::Repo(e) => write!(f, "{e}"),
            SyncError::Refused(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for SyncError {}

impl SyncError {
    /// A blob's failure for `path`: the host's refusal keeps its code.
    fn blob(path: &str, e: anyhow::Error) -> SyncError {
        match e.downcast::<CodedError>() {
            Ok(refused) => SyncError::Host(CodedError { code: refused.code, msg: format!("{path}: {}", refused.msg) }),
            Err(e) => SyncError::Io(format!("{path}: {e:#}")),
        }
    }
}
