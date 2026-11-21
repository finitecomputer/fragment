//! The folder as a pass reads it: what syncs, and the scan against the journal.

use super::journal::{read_state, SyncState};
use super::refuse::{browser_profile, refuse_place, DirRefused};
use super::report::ScanStats;
use super::SyncError;
use anyhow::Result;
use fragment_core::blob::sha256_hex;
use fragment_proto::{NotRepoPath, RepoPath};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The folder as a pass starts: its journal (with the bytes it was read
/// from) and a scan of the folder against it.
pub struct Local {
    pub(super) state: SyncState,
    pub(super) journal: Option<Vec<u8>>,
    pub(super) files: BTreeMap<RepoPath, LocalFile>,
    pub(super) stats: ScanStats,
    pub(super) refused: Vec<NotRepoPath>,
}

impl Local {
    /// Whether the folder is what the journal says: the same paths, with
    /// the same content. A watcher woken by the OS needs no pass when it is
    /// (the event was the pass's own write, or touched nothing that syncs).
    pub fn matches_journal(&self) -> bool {
        self.files.len() == self.state.files.len()
            && self.files.iter().all(|(path, lf)| self.state.files.get(path).is_some_and(|st| st.sha256 == lf.sha256))
    }

    /// The folder's paths and their content, as one hash: a watcher whose
    /// pass left changes unsent (the fragment only reads) needs no pass
    /// for a folder still the same.
    pub fn content_hash(&self) -> String {
        let mut text = String::new();
        for (path, lf) in &self.files {
            text.push_str(path.as_str());
            text.push('\0');
            text.push_str(&lf.sha256);
            text.push('\n');
        }
        sha256_hex(text.as_bytes())
    }
}

/// Reads the folder for a pass: the journal and a scan against it. No
/// network. Every pass reads here, a watcher's too, so a folder that
/// becomes one sync refuses (`refuse_dir`) is refused before it is sent:
/// its place first, then a browser profile the scan met. The scan's own
/// walk decides it, so a profile is never in the files a pass sends while
/// it says yes: a walk of its own before the scan once let a watch send a
/// profile's `Cookies` made between the two, or one `.gitignore` stopped
/// keeping out then.
pub fn read_local(dir: &Path, name: &str) -> Result<Local, SyncError> {
    refuse_place(dir, dirs::home_dir().as_deref()).map_err(SyncError::Refused)?;
    let (state, journal) = read_state(dir, name).map_err(|e| SyncError::Io(e.to_string()))?;
    let Scan { files, stats, refused, profile } = scan_local(dir, Some(&state), Reader::Pass).map_err(|e| SyncError::Io(e.to_string()))?;
    if let Some(found) = profile {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        return Err(SyncError::Refused(DirRefused::BrowserProfile { dir, found }));
    }
    Ok(Local { state, journal, files, stats, refused })
}

pub(crate) struct LocalFile {
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i128,
}

/// Whether a path in the folder (relative, `/`-separated) takes part in
/// sync, in either direction. One rule for the scan, the watcher, and the
/// repo's listing (a repo file that is out is never pulled, and never
/// deleted for being absent here), checked on every segment, so a folder
/// that is out takes everything under it:
/// - dot files and folders: sync's own `.fragment/`, `.git/`, an editor's
///   workspace state (`.obsidian/`), `.DS_Store`, `.#` lock files;
/// - the top-level `node_modules/`: the platform never loads or serves it
///   (an app's modules are `applib/`; `site::is_machinery`), and one
///   install writes thousands of files. One deeper down is content (the
///   site serves `site/node_modules/…`), so it syncs;
/// - editor droppings: `~` backups, `~$` lock files, `.swp` swap files;
/// - sync's own `.conflict-` copies and `.fragment-partial` temp files.
pub fn syncable(rel: &str) -> bool {
    for (depth, seg) in rel.split('/').enumerate() {
        let hidden = seg.starts_with('.');
        let modules = depth == 0 && seg == "node_modules";
        let dropping = seg.ends_with('~') || seg.starts_with("~$") || seg.ends_with(".swp");
        let ours = seg.contains(".conflict-") || seg.contains(".fragment-partial");
        if hidden || modules || dropping || ours {
            return false;
        }
    }
    true
}

/// `path` relative to `root`, its segments joined by `/` ("" for the root
/// itself). A segment keeps its name whole: a Unix file named `a\b` is
/// one segment with a backslash (no repo path), never `a/b`.
pub(super) fn relative(root: &Path, path: &Path) -> String {
    let Ok(rel) = path.strip_prefix(root) else { return String::new() };
    let segments: Vec<std::borrow::Cow<'_, str>> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    segments.join("/")
}

/// A folder's scan: its files by repo path, and the names in it that are
/// not repo paths (a backslash, a control byte, too long), which sync
/// refuses rather than commit a path the platform never reads.
pub struct Scan {
    pub(crate) files: BTreeMap<RepoPath, LocalFile>,
    pub stats: ScanStats,
    pub refused: Vec<NotRepoPath>,
    /// the first browser profile its walk met (`browser_profile`), by its
    /// resolved path: a pass's walk ends there and the pass refuses the
    /// folder (`read_local`), its `files` partial; the audit's reads on
    /// and reports a profile's files as any others
    pub(crate) profile: Option<PathBuf>,
}

/// Who reads the folder (`scan_local`), which says how much of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reader {
    /// a pass (`read_local`): a size+mtime match adopts the journal's hash
    /// (O(changes)), and the walk ends at a browser profile, so nothing in
    /// it is read (a file there no one may read once made the refusal an
    /// error a watch retried, and a large profile was hashed for nothing)
    Pass,
    /// the audit (`verify`): every file is read and hashed, a profile's too
    Audit,
}

/// What sync sends of `dir`: its `syncable` paths, never following
/// symlinks, honoring .gitignore exactly when git would (only inside a
/// repo). A folder that is out is never entered.
pub(super) fn walk(dir: &Path) -> ignore::WalkBuilder {
    let root = dir.to_path_buf();
    let mut w = ignore::WalkBuilder::new(dir);
    // `syncable` owns the dotfile rule, as it does for the watcher
    w.hidden(false).git_ignore(true).git_global(true).git_exclude(true).parents(true);
    w.filter_entry(move |e| e.depth() == 0 || syncable(&relative(&root, e.path())));
    w
}

/// Walk the folder, as `reader` reads it (`Reader`). Takes the `syncable`
/// paths, never following symlinks, and honors .gitignore exactly when git
/// would (only inside a repo). Notes the first browser profile it walks
/// (`Scan::profile`), judged under the folder as it resolves, as
/// `refuse_dir` judges it.
pub fn scan_local(dir: &Path, state: Option<&SyncState>, reader: Reader) -> Result<Scan> {
    let mut out = BTreeMap::new();
    let mut refused = Vec::new();
    let mut stats = ScanStats::default();
    let mut profile = None;
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for entry in walk(dir).build() {
        let entry = entry?;
        let file = entry.file_type().is_some_and(|t| t.is_file());
        if profile.is_none() {
            let resolved = root.join(entry.path().strip_prefix(dir).unwrap_or(entry.path()));
            if browser_profile(&resolved, entry.depth(), file) {
                profile = Some(resolved);
                if reader == Reader::Pass {
                    break;
                }
            }
        }
        if !file {
            continue;
        }
        // a name that is not UTF-8 reads lossily, as no file is named
        let lossy = relative(dir, entry.path());
        let parsed = if entry.path().to_str().is_some() { RepoPath::parse(&lossy) } else { Err(NotRepoPath(lossy)) };
        let rel = match parsed {
            Ok(rel) => rel,
            Err(not) => {
                refused.push(not);
                continue;
            }
        };
        let md = fs::metadata(entry.path())?;
        let size = md.len();
        let mtime_ns = md
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        let cached = state.and_then(|s| s.files.get(&rel)).filter(|st| st.size == size && st.mtime_ns == mtime_ns);
        let sha = if let (Some(st), Reader::Pass) = (cached, reader) {
            st.sha256.clone()
        } else {
            stats.hashed += 1;
            sha256_hex(&fs::read(entry.path())?)
        };
        stats.files += 1;
        out.insert(rel, LocalFile { sha256: sha, size, mtime_ns });
    }
    Ok(Scan { files: out, stats, refused, profile })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::journal::FileState;
    use crate::sync::tests::{rp, tmpdir};

    #[test]
    fn scan_uses_cache() {
        let dir = tmpdir("scan");
        fs::create_dir_all(dir.join("site")).unwrap();
        fs::write(dir.join("site/index.html"), b"hi").unwrap();
        let Scan { files: l1, stats: s1, .. } = scan_local(&dir, None, Reader::Pass).unwrap();
        assert_eq!(s1.hashed, 1);
        let mut st = SyncState { schema_version: 3, name: "x".into(), host: None, repo: None, files: BTreeMap::new() };
        let lf = l1.get("site/index.html").unwrap();
        st.files.insert(rp("site/index.html"), FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: "c".into() });
        let s2 = scan_local(&dir, Some(&st), Reader::Pass).unwrap().stats;
        assert_eq!(s2.hashed, 0);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: one rule says what syncs. Method: the paths each old list
    /// disagreed on, and ordinary ones.
    #[test]
    fn what_syncs() {
        // a node_modules below the top is the site's content: it serves it
        for rel in ["a.md", "site/index.html", "notes/2026/x.md", "a.b/c.d.md", "applib/x.mjs", "fragment.json", "site/node_modules/y.js"] {
            assert!(syncable(rel), "{rel} syncs");
        }
        for rel in [
            ".obsidian/workspace.json", // the watcher's old rule tested one segment for a two-segment path
            ".DS_Store",
            "site/.DS_Store",
            ".git/HEAD",
            ".fragment/state.json",
            ".trash/old.md",
            "node_modules/x/index.js", // the scan uploaded it while the watcher ignored it
            "a.md~",
            "~$report.docx",
            "notes.swp",
            ".#notes.md",
            "a.conflict-1700000000-deadbeef.md",
            "doc.fragment-partial-4242",
        ] {
            assert!(!syncable(rel), "{rel} does not sync");
        }
    }

    /// Goal: the scan takes what `syncable` takes. Method: a folder with
    /// one file of each kind.
    #[test]
    fn the_scan_takes_what_syncs() {
        let src = tmpdir("rule-src");
        for (rel, bytes) in [("a.md", "a"), (".obsidian/workspace.json", "{}"), ("node_modules/x/index.js", "x"), ("b.md~", "b"), ("sub/.DS_Store", "d"), ("sub/c.md", "c")] {
            fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            fs::write(src.join(rel), bytes).unwrap();
        }
        let scanned = scan_local(&src, None, Reader::Audit).unwrap().files;
        assert_eq!(scanned.keys().collect::<Vec<_>>(), ["a.md", "sub/c.md"]);
        fs::remove_dir_all(&src).ok();
    }
}
