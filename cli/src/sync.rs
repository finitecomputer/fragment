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
use crate::api::Client;
use crate::api::{Code, CodedError};
use crate::codestorage::{Author, Change, CodeStorage, CsError, Wants, LIVE, MAIN, MAX_CAS_ATTEMPTS};
use fragment_core::blob::sha256_hex;
use fragment_core::codestorage::TreeEntry;
use fragment_proto::{NotRepoPath, RepoPath};
use anyhow::{anyhow, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// stat cache v3: content sha + last-seen remote commit per path.
/// Anything else (or a v2 file from the old engine) reads as fresh —
/// corrupt or old states never wedge the folder.
#[derive(Serialize, Deserialize, Default)]
pub struct SyncState {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32, // 3
    pub name: String,
    /// url-form repo identity this folder last synced against, plus the
    /// fragment host it synced through. The mirror trusts the journal to
    /// mean "same world": a journal from another (a recreated fragment, or
    /// the same name on another host) makes every file look remotely
    /// changed, and the pull mirrors the wrong world over the folder.
    /// None on old journals; bound on the first sync.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// by path, in order: the journal serializes the same for the same
    /// state, so an unchanged one is recognized and not written again. A
    /// key that is not a repo path makes the journal unreadable, so it
    /// starts fresh.
    pub files: BTreeMap<RepoPath, FileState>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileState {
    pub sha256: String,
    pub size: u64,
    pub mtime_ns: i128,
    /// last commit SHA we saw touch this path (the listing's
    /// last_commit_sha) — our per-file remote version marker
    pub commit: String,
}

#[derive(Default, Serialize, Debug, Clone)]
pub struct Report {
    pub pushed: Vec<String>,
    pub pulled: Vec<String>,
    pub deleted_remote: Vec<String>,
    pub deleted_local: Vec<String>,
    pub withheld_deletions: Vec<String>,
    pub conflicts: Vec<String>,
    /// the token only read (a viewer's): nothing was sent
    pub read_only: bool,
    /// what a writer's mirror would have sent, kept in the folder unsent
    /// (a file deleted here is pulled back): its token only read
    pub unsent: Vec<String>,
    /// synced paths the scan no longer takes, though something is still
    /// there (a `.gitignore`d file, what is not a regular file): kept as
    /// they are, never sent, and the fragment keeps its copy
    pub outside_sync: Vec<String>,
    /// paths in the folder or the repo that are not repo paths
    /// (`valid_repo_path`): never pushed, pulled, or deleted. One in the
    /// repo was written past the cell (a storage token's commit); its
    /// absolute or `..` path would have landed outside the folder.
    pub refused: Vec<String>,
    pub mass_delete_guard: Option<usize>,
    pub scan: ScanStats,
    pub mode: String,
    /// main's head as the pass left it (its own commit, when one landed)
    #[serde(skip)]
    pub head: Option<String>,
    /// a commit of this pass landed (or may have: its answer was lost)
    #[serde(skip)]
    pub landed: bool,
}

#[derive(Default, Serialize, Clone, Copy, Debug)]
pub struct ScanStats {
    pub files: usize,
    pub hashed: usize,
}

impl Report {
    /// exit code: 0 clean, 3 conflicts present, 4 mass-deletion guard, 5
    /// changes not sent (the fragment only read)
    pub fn exit_code(&self) -> i32 {
        if self.mass_delete_guard.is_some() {
            4
        } else if !self.conflicts.is_empty() {
            3
        } else if !self.unsent.is_empty() {
            5
        } else {
            0
        }
    }

    pub fn print(&self) {
        let p = |label: &str, v: &Vec<String>| {
            if !v.is_empty() {
                println!("  {label}: {}", v.len());
                for f in v {
                    println!("    {f}");
                }
            }
        };
        p("pushed", &self.pushed);
        p("pulled", &self.pulled);
        p("deleted remotely", &self.deleted_remote);
        p("deleted locally", &self.deleted_local);
        if !self.withheld_deletions.is_empty() {
            println!("  deletions withheld (pull mode; --prune to apply): {})", self.withheld_deletions.len());
        }
        if !self.refused.is_empty() {
            println!("  REFUSED ({}): not repo paths, so nothing was written, pushed, or deleted for them", self.refused.len());
            for f in &self.refused {
                println!("    {f}");
            }
        }
        if !self.conflicts.is_empty() {
            println!("  CONFLICTS ({}): local keeps yours; the remote copy is saved beside it (.conflict-…)", self.conflicts.len());
            for f in &self.conflicts {
                println!("    {f}");
            }
        }
        if !self.unsent.is_empty() {
            println!("  NOT SENT ({}): you may read this fragment, not change it. Your changes stay here, unsent; a file you deleted was pulled back", self.unsent.len());
            for f in &self.unsent {
                println!("    {f}");
            }
        }
        if !self.outside_sync.is_empty() {
            println!("  NO LONGER SYNCED ({}): ignored here now, or not a file; kept as they are, and the fragment keeps its copy", self.outside_sync.len());
            for f in &self.outside_sync {
                println!("    {f}");
            }
        }
        if let Some(n) = self.mass_delete_guard {
            println!("  REFUSING to propagate {n} deletion(s) — folder looks unmounted/reset.");
            println!("  re-run with --apply-mass-delete if this is intended");
        }
        let clean = self.pushed.is_empty()
            && self.pulled.is_empty()
            && self.conflicts.is_empty()
            && self.deleted_remote.is_empty()
            && self.deleted_local.is_empty()
            && self.unsent.is_empty()
            && self.outside_sync.is_empty();
        if clean {
            println!("all {} files match the repo", self.scan.files);
        }
    }
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join(".fragment").join("state.json")
}

/// The single-watcher lock: an advisory flock on .fragment/sync.lock, held
/// for the process lifetime. Stale by construction (the OS drops it when
/// the holder dies).
pub struct SyncLock(#[allow(dead_code)] File); // the field IS the lock (RAII)
impl SyncLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir.join(".fragment"))?;
        let p = dir.join(".fragment").join("sync.lock");
        let f = File::create(&p)?;
        f.try_lock_exclusive()
            .map_err(|_| anyhow!("another fragment sync holds this folder ({}). If that's wrong, no process should own it; otherwise stop it or use a different folder.", p.display()))?;
        Ok(SyncLock(f))
    }
}

pub fn load_state(dir: &Path, name: &str) -> Result<SyncState> {
    Ok(read_state(dir, name)?.0)
}

/// The journal, and the bytes it was read from (`None` when there was
/// none, or it was unreadable and starts fresh).
fn read_state(dir: &Path, name: &str) -> Result<(SyncState, Option<Vec<u8>>)> {
    let fresh = || SyncState { schema_version: 3, name: name.to_string(), host: None, repo: None, files: BTreeMap::new() };
    let p = state_path(dir);
    if !p.exists() {
        return Ok((fresh(), None));
    }
    let bytes = fs::read(&p)?;
    match serde_json::from_slice::<SyncState>(&bytes) {
        Ok(mut s) if s.schema_version == 3 => {
            // a journal a sync named by its bare label wrote (`todo`, before
            // a sync resolved it) is the same fragment's, named in full now
            let bare = fragment_proto::split_fragment_name(name).is_some_and(|(label, _)| label == s.name);
            if s.name != name && !bare {
                anyhow::bail!("directory is synced to fragment '{}', not '{}'", s.name, name);
            }
            s.name = name.to_string();
            Ok((s, Some(bytes)))
        }
        _ => {
            eprintln!("warning: {} unreadable or old format — rebuilding state", p.display());
            Ok((fresh(), None))
        }
    }
}

/// atomic write: tmp in the same directory (unique name), fsync, rename
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = partial_path(path);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Where `atomic_write` puts `path`'s bytes before the rename: beside it,
/// named after its whole file name and a count of this process's writes,
/// so files written at once never share one (`a.md` and `a.txt` did, by
/// extension, when a pull wrote one file at a time). `syncable` leaves
/// it out by its `.fragment-partial` mark.
fn partial_path(path: &Path) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!("{name}.fragment-partial-{}-{n}", std::process::id()))
}

/// Files a pass reads from code.storage at once: the pull's, and the
/// bootstrap's content checks (`adopt_identical`). A read is a round trip
/// of about 40 ms from a laptop (preview and prod alike, 2026-11-02), so
/// one at a time a 400-file pull took 17 s; 16 at once is about a second,
/// and still a handful of connections for one person's sync. A blob's
/// bytes, up to `BLOB_MAX_BYTES` each, keep a bound of their own
/// (`BLOB_TRANSFERS_AT_ONCE`).
const READS_AT_ONCE: usize = 16;

/// Blobs (files of 1 MiB or more, up to `BLOB_MAX_BYTES` each) a pass moves
/// at once, up or down. A blob's PUT took about 450 ms before its bytes and
/// 150 ms a MB after, its HEAD about 120 ms (preview, 2026-11-02): four at
/// a time overlap those waits, and hold at most four in memory (1 GiB at
/// the largest). A push reads its files this many at a time too, each a
/// blob's upload when it is one.
const BLOB_TRANSFERS_AT_ONCE: usize = 4;

/// At most `max` holders at once: a counting gate (std has none). A
/// holder's `Pass` lets it go when dropped.
struct Gate {
    max: usize,
    held: Mutex<usize>,
    freed: Condvar,
}

struct Pass<'a>(&'a Gate);

impl Gate {
    fn new(max: usize) -> Gate {
        assert!(max >= 1, "a gate lets one through at least");
        Gate { max, held: Mutex::new(0), freed: Condvar::new() }
    }

    /// Waits for a place, and holds it until the pass is dropped.
    fn enter(&self) -> Pass<'_> {
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let mut held = self.freed.wait_while(held, |n| *n >= self.max).unwrap_or_else(|e| e.into_inner());
        *held += 1;
        Pass(self)
    }

    /// How many hold a place now.
    #[cfg(test)]
    fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        *self.0.held.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

/// `f` over `items`, at most `at_once` at a time on scoped threads: each
/// item's answer in order, or `None` for one never started because one
/// before it failed (the pass is failing, so nothing more is read).
fn at_once<T: Sync, R: Send, E: Send>(items: &[T], at_once: usize, f: impl Fn(&T) -> Result<R, E> + Sync) -> Vec<Option<Result<R, E>>> {
    assert!(at_once >= 1, "at least one at a time");
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let slots: Vec<Mutex<Option<Result<R, E>>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..at_once.min(items.len()) {
            scope.spawn(|| {
                while !failed.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let answer = f(item);
                    if answer.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
                }
            });
        }
    });
    slots.into_iter().map(|slot| slot.into_inner().unwrap_or_else(|e| e.into_inner())).collect()
}

/// Writes the journal, unless it is byte for byte what was read (`before`):
/// a pass that changed nothing leaves the file alone, with no fsync'd
/// rewrite.
fn save_state(dir: &Path, state: &SyncState, before: Option<&[u8]>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if before == Some(bytes.as_slice()) {
        return Ok(());
    }
    atomic_write(&state_path(dir), &bytes)
}

/// The folder as a pass starts: its journal (with the bytes it was read
/// from) and a scan of the folder against it.
pub struct Local {
    state: SyncState,
    journal: Option<Vec<u8>>,
    files: BTreeMap<RepoPath, LocalFile>,
    stats: ScanStats,
    refused: Vec<NotRepoPath>,
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
/// network.
pub fn read_local(dir: &Path, name: &str) -> Result<Local, SyncError> {
    let (state, journal) = read_state(dir, name).map_err(|e| SyncError::Io(e.to_string()))?;
    let Scan { files, stats, refused } = scan_local(dir, Some(&state), false).map_err(|e| SyncError::Io(e.to_string()))?;
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
fn relative(root: &Path, path: &Path) -> String {
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
}

/// Walk the folder. With state, a size+mtime match adopts the cached hash
/// (O(changes)); with verify, everything is read and hashed. Takes the
/// `syncable` paths, never following symlinks, and honors .gitignore
/// exactly when git would (only inside a repo).
pub fn scan_local(dir: &Path, state: Option<&SyncState>, verify: bool) -> Result<Scan> {
    let mut out = BTreeMap::new();
    let mut refused = Vec::new();
    let mut stats = ScanStats::default();
    let root = dir.to_path_buf();
    let walker = ignore::WalkBuilder::new(dir)
        // `syncable` owns the dotfile rule, as it does for the watcher
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        // a folder that is out is never entered
        .filter_entry(move |e| e.depth() == 0 || syncable(&relative(&root, e.path())))
        .build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
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
        let sha = if let (Some(st), false) = (cached, verify) {
            st.sha256.clone()
        } else {
            stats.hashed += 1;
            sha256_hex(&fs::read(entry.path())?)
        };
        stats.files += 1;
        out.insert(rel, LocalFile { sha256: sha, size, mtime_ns });
    }
    Ok(Scan { files: out, stats, refused })
}

/// What one push pass wants to do, computed purely from (local, remote,
/// state) so CAS retries can rebuild it against a fresh listing.
struct PushPlan {
    upserts: Vec<RepoPath>,
    deletes: Vec<RepoPath>, // paths to delete remotely (deleted locally)
    conflicts: Vec<RepoPath>,
}

fn push_plan(
    local: &BTreeMap<RepoPath, LocalFile>,
    remote: &HashMap<RepoPath, TreeEntry>,
    state: &SyncState,
) -> PushPlan {
    let mut plan = PushPlan { upserts: Vec::new(), deletes: Vec::new(), conflicts: Vec::new() };
    let mut paths: Vec<&RepoPath> = local.keys().chain(remote.keys()).collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let l = local.get(path);
        let r = remote.get(path);
        let s = state.files.get(path);
        match (l, r, s) {
            (Some(lf), Some(rf), Some(st)) => {
                let local_changed = st.sha256 != lf.sha256;
                let remote_changed = st.commit != rf.last_commit_sha;
                if local_changed && remote_changed {
                    plan.conflicts.push(path.clone());
                } else if local_changed {
                    plan.upserts.push(path.clone());
                }
                // local unchanged: remote-side changes are the pull
                // phase's business, never a push
            }
            (Some(_), Some(_), None) => {
                // stateless bootstrap: equal size is NOT identity — the
                // bootstrap pass in sync_once adopts verified-equal files
                // into the journal, so anything still journal-absent here
                // is content-different (or the pass could not run) and
                // local wins; the prior remote content survives in git
                // history
                plan.upserts.push(path.clone());
            }
            (Some(lf), None, Some(st)) => {
                if st.sha256 != lf.sha256 {
                    plan.upserts.push(path.clone()); // content beats deletion
                }
                // else: deleted remotely, local untouched -> pull phase
                // propagates the deletion (guarded)
            }
            (Some(_), None, None) => plan.upserts.push(path.clone()), // new file
            (None, Some(rf), Some(st)) => {
                // deleted locally, remotely untouched since we saw it -> delete remotely
                if st.commit == rf.last_commit_sha {
                    plan.deletes.push(path.clone());
                }
                // else remote moved on: leave it; pull re-materializes
            }
            (None, Some(_), None) => {} // remote-only: pull fetches
            (None, None, _) => {}       // stale state row; pull phase cleans
        }
    }
    plan
}

/// The mass-deletion guard's floor: a pass may always delete this many
/// files. It is 3, not 10: with a floor of 10, deleting 8 of a 10-file
/// fragment (a whole world change, found live) sailed through because
/// 8 ≤ 10 while being 80% of the folder. Small folders deserve the
/// percent protection most.
pub const MASS_DELETE_FLOOR: usize = 3;
/// Past the floor, the share of the known files (percent) a pass may delete.
pub const MASS_DELETE_PERCENT: usize = 30;

/// Mass-deletion guard (ported): a pass that would delete more than
/// max(MASS_DELETE_FLOOR, MASS_DELETE_PERCENT%) of the known files, or ALL
/// of them (the unmounted-disk / replaced-folder case the old
/// root-identity check covered), is refused until --apply-mass-delete.
/// Counts both directions — deletions pushed remotely and deletions
/// applied locally.
fn mass_delete_trips(push_deletes: usize, local_deletes: usize, known: usize, apply: bool) -> Option<usize> {
    if apply {
        return None;
    }
    let pending = push_deletes + local_deletes;
    let known = known.max(1);
    let over_share = pending > MASS_DELETE_FLOOR && pending * 100 > known * MASS_DELETE_PERCENT;
    if over_share || (pending == known && pending > 0) {
        Some(pending)
    } else {
        None
    }
}

/// The fragment `name` names, in full (`Client::fragment`): a folder's
/// journal names it so, whether a command was given its label (`todo`)
/// or its full name (`todo--k3x9`).
pub fn full_name(client: &Client, name: &str) -> Result<String, SyncError> {
    client.fragment(name).map_err(|e| match e.downcast::<CodedError>() {
        Ok(refused) => SyncError::Host(refused),
        Err(e) => SyncError::Io(format!("naming {name}: {e:#}")),
    })
}

/// `fragment sync` (and a deploy's): connects, runs one pass, and nudges
/// the cell's pins when a commit landed.
pub fn sync_once(client: &Client, name: &str, dir: &Path, opts: &SyncOptions) -> Result<Report, SyncError> {
    let name = &full_name(client, name)?;
    let storage = CodeStorage::connect(client, name, opts.mode.wants())?;
    let report = pass_over(client, &storage, name, dir, opts, read_local(dir, name)?)?;
    if report.landed {
        refresh_pins(client, name, MAIN);
    }
    Ok(report)
}

/// One pass over a folder already read (`read_local`: a watcher reads it
/// first to learn whether it needs a pass at all), with a connected
/// `storage`: a command's own, or a watcher's held one. It does not nudge
/// the cell's pins: a caller whose pass `landed` calls `refresh_pins`.
///
/// The invariant every phase keeps (pull, push, mirror, a folder with no
/// journal, a viewer made an editor, a pass again): **a pass changes only
/// what its scan accounted for.**
/// - In the folder, a write or a deletion lands only where the scan saw
///   the bytes the journal says git holds, or where nothing at all is
///   (`vacant`). Anything else there (an edit, a file never synced, one
///   outside sync, a pipe) stays, and the fragment's bytes land beside it
///   under a name nothing holds (`save_copy`).
/// - On main, a push sends only bytes the scan read, and deletes a path
///   only where the folder has nothing at all. A synced path the scan no
///   longer takes, with something still there (a `.gitignore`d file, what
///   is not a regular file, a path through a symlinked folder), is outside
///   sync: its journal row is dropped before any plan reads it, nothing is
///   sent or deleted for it, and the report names it (`drop_outside_sync`).
/// - A journal row's hash is always bytes git holds, so a conflict records
///   the fragment's bytes, never the folder's unsent ones
///   (`record_conflicts`): a folder's own bytes are never "in sync" until
///   sent.
///
/// It holds for the folder as scanned; a save during the pass's own
/// requests is a race this does not close.
/// Tested as a matrix: `every_pass_changes_only_what_its_scan_accounted_for`.
pub fn pass_over(client: &Client, storage: &CodeStorage, name: &str, dir: &Path, opts: &SyncOptions, local: Local) -> Result<Report, SyncError> {
    let Local { mut state, journal, files: local, stats, refused } = local;
    // World binding (`SyncState::host`): refuse before any read or write.
    let bound_host = client.host.trim_end_matches('/').to_string();
    match (&state.host, &state.repo) {
        (Some(h), Some(r)) if h != &bound_host || r != storage.repo() => {
            return Err(SyncError::Repo(format!(
                "this folder is bound to {} repo {r}, but fragment '{name}' is {} repo {} — the fragment was likely recreated or this folder synced a different host.\n  to sync here anyway: rm {}/.fragment/state.json (rebinds; remote content survives in git history)",
                h, bound_host, storage.repo(), dir.display()
            )));
        }
        _ => {
            state.host = Some(bound_host);
            state.repo = Some(storage.repo().to_string());
        }
    }
    let outside_sync = drop_outside_sync(dir, &local, &mut state)?;
    // a token that only reads (a viewer's): the pass pulls alone, and what
    // a mirror would have sent stays in the folder (`unsent`)
    let read_only = !storage.writes();
    if read_only && opts.mode == Mode::Push {
        return Err(SyncError::Host(CodedError { code: Code::Forbidden, msg: format!("you may read {name}, not change it: a push is an editor's (`--mode pull` reads it)") }));
    }
    let mode = if read_only { Mode::Pull } else { opts.mode };
    let author = Author::writer(&opts.writer_id);
    let blobs = crate::blobs::Blobs::new(client, name);
    let outside_sync = outside_sync.iter().map(|p| p.to_string()).collect();
    let mut report = Report { scan: stats, mode: format!("{:?}", opts.mode).to_lowercase(), read_only, outside_sync, ..Default::default() };
    let mut recorded: HashSet<RepoPath> = HashSet::new();

    // main as this pass sees it: read once, reused by the push's first
    // attempt and by the pull, and after our own commit derived rather than
    // listed again
    assert!(!opts.live || opts.mode == Mode::Pull, "live is only pulled");
    let mut listing = list_branch(storage, if opts.live { LIVE } else { MAIN })?;
    report.refused = refused.iter().chain(&listing.refused).map(|not| not.to_string()).collect();
    for not in &listing.refused {
        eprintln!("warning: refusing the repo's {not}: nothing is written for it");
    }
    adopt_identical(storage, &local, &listing, &mut state)?;
    // candidate local deletions: known remotely before, gone from the
    // listing now, local copy untouched since we saw it
    let local_delete_candidates: Vec<RepoPath> = state
        .files
        .iter()
        .filter(|(p, st)| {
            !listing.files.contains_key(*p)
                && local.get(*p).is_some_and(|lf| lf.sha256 == st.sha256)
        })
        .map(|(p, _)| p.clone())
        .collect();
    let pre = push_plan(&local, &listing.files, &state);
    if read_only && opts.mode == Mode::Mirror {
        report.unsent = pre.upserts.iter().chain(&pre.deletes).chain(&pre.conflicts).map(|p| p.to_string()).collect();
        report.unsent.sort();
    }
    // pull mode never deletes remotely, so would-be push deletions must not
    // trip the guard there (a wiped folder in pull mode just re-downloads)
    let push_side = if mode == Mode::Pull { 0 } else { pre.deletes.len() };
    if let Some(n) = mass_delete_trips(push_side, local_delete_candidates.len(), state.files.len(), opts.apply_mass_delete) {
        report.mass_delete_guard = Some(n);
        // dropping stale rows keeps the guard from re-tripping forever on
        // state that no longer matches either side
        for p in local_delete_candidates {
            state.files.remove(&p);
        }
        save_state(dir, &state, journal.as_deref()).map_err(|e| SyncError::Io(e.to_string()))?;
        report.head = listing.head;
        return Ok(report);
    }

    // ---- push: one commit with expected-parent CAS, bounded rebuilds ----
    if mode != Mode::Pull {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            if attempt > 1 {
                listing = list_main(storage)?;
                // a commit whose answer was lost may have landed: what it
                // wrote now equals the folder, and adopting it (here and in
                // record_conflicts) is what keeps it from reading as a new
                // file to push again or as a conflict with our own bytes
                adopt_identical(storage, &local, &listing, &mut state)?;
            }
            let plan = push_plan(&local, &listing.files, &state);
            record_conflicts(
                ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                &plan.conflicts,
            )?;
            if plan.upserts.is_empty() && plan.deletes.is_empty() {
                break; // nothing to commit (clean, or a replay absorbed)
            }
            let mut changes: Vec<Change> = Vec::with_capacity(plan.upserts.len() + plan.deletes.len());
            // read BLOB_TRANSFERS_AT_ONCE at a time: a file is its bytes, a
            // blob its pointer once uploaded (a HEAD, and the PUT the store
            // lacks). Nothing is committed if one fails.
            let stored = at_once(&plan.upserts, BLOB_TRANSFERS_AT_ONCE, |p| {
                let bytes = fs::read(p.under(dir)).map_err(|e| SyncError::Io(format!("read {p}: {e}")))?;
                blobs.store(bytes).map_err(|e| SyncError::blob(p.as_str(), e))
            });
            let mut failure = None;
            for (p, got) in plan.upserts.iter().zip(stored) {
                match got {
                    Some(Ok(bytes)) => changes.push(Change::Upsert { path: p.clone(), bytes }),
                    Some(Err(e)) => {
                        failure.get_or_insert(e);
                    }
                    None => {}
                }
            }
            if let Some(e) = failure {
                return Err(e);
            }
            for p in &plan.deletes {
                changes.push(Change::Delete { path: p.clone() });
            }
            let msg = format!(
                "fragment sync {}: +{} -{}",
                name,
                plan.upserts.len(),
                plan.deletes.len()
            );
            match storage.commit(listing.head.as_deref(), &msg, &author, &changes) {
                Ok(tip) => {
                    for p in &plan.upserts {
                        if let Some(lf) = local.get(p) {
                            state.files.insert(
                                p.clone(),
                                FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: tip.clone() },
                            );
                        }
                        report.pushed.push(p.to_string());
                    }
                    for p in &plan.deletes {
                        state.files.remove(p);
                        report.deleted_remote.push(p.to_string());
                    }
                    listing = listing.landed(tip, &changes);
                    report.landed = true;
                    break;
                }
                Err(CsError::CasRejected { .. }) if attempt < MAX_CAS_ATTEMPTS => {
                    eprintln!("  branch moved under us — refetching head and rebuilding the diff (attempt {attempt}/{MAX_CAS_ATTEMPTS})");
                    continue;
                }
                Err(CsError::OutcomeUnknown(detail)) if attempt < MAX_CAS_ATTEMPTS => {
                    // never resent blind: the rebuilt diff is empty if it
                    // landed, and the same commit on a fresh head if not
                    eprintln!("  the commit's answer was lost ({detail}) — re-reading the branch to see whether it landed (attempt {attempt}/{MAX_CAS_ATTEMPTS})");
                    report.landed = true; // perhaps: the refresh nudge is harmless either way
                    continue;
                }
                Err(CsError::CasRejected { detail }) => {
                    return Err(SyncError::Cs(CsError::CasRejected {
                        detail: format!("branch kept moving after {MAX_CAS_ATTEMPTS} attempts ({detail}); re-run fragment sync"),
                    }));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // ---- pull: make the folder match main (fetch new/changed; propagate
    // remote deletions per mode) ----
    if mode != Mode::Push {
        let mut fetches: Vec<&TreeEntry> = vec![];
        for rf in listing.files.values() {
            let l = local.get(&rf.path);
            let s = state.files.get(&rf.path);
            // what the scan did not take, at a path the fragment has: none,
            // or something outside sync (`keep_unscanned`)
            let occupied = match l {
                None => match place(dir, &rf.path)? {
                    Some(at) if !vacant(&at)? => Some(at),
                    _ => None,
                },
                Some(_) => None,
            };
            let fetch = match (l, s) {
                (None, _) if occupied.is_some() => {
                    let target = occupied.expect("matched above");
                    keep_unscanned(
                        ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                        &rf.path,
                        &target,
                    )?;
                    false
                }
                // nothing there: fetch it
                (None, _) => true,
                // journal-absent but present locally, and content-different
                // (the bootstrap pass adopted every match): the folder's own
                // bytes, never sent, stay, and the remote's land beside them.
                // A writer's mirror pushed them already, so this is a pass
                // that only reads.
                (Some(_), None) => {
                    record_conflicts(
                        ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                        std::slice::from_ref(&rf.path),
                    )?;
                    false
                }
                (Some(lf), Some(st)) => {
                    let local_changed = st.sha256 != lf.sha256;
                    let remote_changed = st.commit != rf.last_commit_sha;
                    if local_changed && remote_changed {
                        // both changed and push didn't resolve it (push
                        // modes off, or a race) — same conflict treatment
                        record_conflicts(
                            ConflictCtx { storage, blobs: &blobs, dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: &opts.writer_id },
                            std::slice::from_ref(&rf.path),
                        )?;
                        false
                    } else {
                        remote_changed && !local_changed
                    }
                }
            };
            if fetch {
                fetches.push(rf);
            }
        }
        // read READS_AT_ONCE at a time; the journal takes them only if
        // every one landed (it is not saved on a failure, so a partial
        // pull never reads as a complete one: the next pass adopts what
        // was written by content and fetches the rest)
        fetches.sort_by(|a, b| a.path.cmp(&b.path));
        let blob_gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let fetched = at_once(&fetches, READS_AT_ONCE, |rf| pull_file(storage, &blobs, &blob_gate, dir, listing.rev(), rf));
        let mut failure = None;
        for (rf, got) in fetches.iter().zip(fetched) {
            match got {
                Some(Ok(Some(row))) => {
                    state.files.insert(rf.path.clone(), row);
                    report.pulled.push(rf.path.to_string());
                }
                Some(Ok(None)) => report.refused.push(through_link(&rf.path)),
                Some(Err(e)) => {
                    failure.get_or_insert(e);
                }
                None => {}
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        // remote deletions: known before, gone now, local copy untouched
        let mut paths: Vec<RepoPath> = state.files.keys().cloned().collect();
        paths.sort();
        for p in paths {
            if listing.files.contains_key(&p) {
                continue;
            }
            let untouched = local.get(&p).is_some_and(|lf| Some(&lf.sha256) == state.files.get(&p).map(|s| &s.sha256));
            if !untouched {
                continue;
            }
            if opts.mode == Mode::Pull && !opts.prune {
                // keep the state row: dropping it here made the deletion
                // unknowable, so a later --prune pass could never apply it.
                // Re-reporting each pass is honest — it is still pending.
                report.withheld_deletions.push(p.to_string());
                continue;
            }
            let Some(target) = place(dir, &p)? else {
                report.refused.push(through_link(&p));
                continue;
            };
            fs::remove_file(target).map_err(|e| SyncError::Io(format!("delete {p}: {e}")))?;
            state.files.remove(&p);
            report.deleted_local.push(p.to_string());
        }
    }

    save_state(dir, &state, journal.as_deref()).map_err(|e| SyncError::Io(e.to_string()))?;
    report.head = listing.head;
    Ok(report)
}

/// Tells the cell its fragment's `moved` ref moved. Our commits and ref moves are
/// EXTERNAL pushes from the cell's perspective: without this nudge its
/// pins wait out the 5-minute poll backstop before app reads and serving
/// see them. Best-effort: the poll covers a missed nudge.
pub fn refresh_pins(client: &Client, name: &str, moved: &str) {
    // the cell reads again only the ref we moved (a cell before that reads both)
    let asked = fragment_proto::Refresh { moved: Some(moved.to_string()) };
    if let Err(e) = client.post_json(&format!("/api/f/{name}/refresh"), &asked) {
        eprintln!("warning: cell pin refresh failed ({e:#}); the poll backstop will catch up");
    }
}

/// main as one pass reads it: its head, and the files at that head (read
/// at the head's commit, not at `main`, so the two always agree).
struct Listing {
    head: Option<String>,
    files: HashMap<RepoPath, TreeEntry>,
    /// the paths at the head that are not repo paths: never pulled
    refused: Vec<NotRepoPath>,
}

impl Listing {
    /// The ref a pass reads files at: the listing's own commit, so a read
    /// sees the bytes the listing describes even if main moved since.
    fn rev(&self) -> &str {
        self.head.as_deref().unwrap_or(MAIN)
    }

    /// main once our commit `tip` landed on this listing's head. The
    /// commit named this head as its expected parent, so our changes are
    /// the only difference, and listing main again would only read back
    /// what we just wrote.
    fn landed(mut self, tip: String, changes: &[Change]) -> Listing {
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

fn list_main(storage: &CodeStorage) -> Result<Listing, SyncError> {
    list_branch(storage, MAIN)
}

fn list_branch(storage: &CodeStorage, branch: &str) -> Result<Listing, SyncError> {
    let Some(head) = storage.branch_head(branch)? else {
        return Ok(Listing { head: None, files: HashMap::new(), refused: vec![] }); // empty repo: everything local is new
    };
    let listed = storage.list_files(&head)?;
    let files = listed.entries.into_iter().filter(|f| syncable(f.path.as_str())).map(|f| (f.path.clone(), f)).collect();
    Ok(Listing { head: Some(head), files, refused: listed.refused })
}

/// Brings the journal up to what both sides already agree on, before any
/// plan is made from it:
/// - a journal-absent file present on both sides with the same content is
///   adopted (the stateless bootstrap, and a new file whose commit landed
///   though its answer was lost). Equal size used to be "provisionally
///   same" — but equal size is not identity, and a same-size
///   different-content file silently never pushed (found live). So each
///   equal-size unknown is fetched and hashed; mismatches stay
///   journal-absent and the push plan treats them as local-wins.
/// - a row for a path gone from both sides is dropped (a deletion that
///   landed, whoever made it); no plan reads it, and it would count toward
///   the mass-deletion guard's known files forever.
fn adopt_identical(
    storage: &CodeStorage,
    local: &BTreeMap<RepoPath, LocalFile>,
    listing: &Listing,
    state: &mut SyncState,
) -> Result<(), SyncError> {
    let unknown: Vec<(&RepoPath, &TreeEntry, &LocalFile)> = listing
        .files
        .iter()
        .filter(|(p, _)| !state.files.contains_key(*p))
        .filter_map(|(p, rf)| local.get(p).map(|lf| (p, rf, lf)))
        .filter(|(_, rf, lf)| lf.size == rf.size || crate::blobs::could_point(rf.size, lf.size))
        .collect();
    // read as the pull reads, READS_AT_ONCE at a time: a folder a pull
    // left part-way is all of these on its next pass. Each is compared
    // where it is read, so a body is let go before the next read: what
    // these hold at once is READS_AT_ONCE files, not the folder's
    let same = at_once(&unknown, READS_AT_ONCE, |(p, _, lf)| storage.read_file(p.as_str(), listing.rev()).map(|bytes| crate::blobs::content_sha(&bytes) == lf.sha256));
    for ((p, rf, lf), got) in unknown.iter().zip(same) {
        match got {
            Some(Ok(true)) => {
                state.files.insert(
                    (*p).clone(),
                    FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: rf.last_commit_sha.clone() },
                );
            }
            Some(Ok(false)) | None => {}
            Some(Err(e)) => return Err(e.into()),
        }
    }
    state.files.retain(|p, _| local.contains_key(p) || listing.files.contains_key(p));
    Ok(())
}

/// Both sides changed: keep local, save the remote bytes beside it
/// (`save_copy`), then record the remote's bytes at its commit, never the
/// folder's: the folder's stay a change, so a writer's next pass sends them
/// (theirs survives in the copy and in git history), a pass that only reads
/// keeps them unsent, and neither ever fetches over them. The remote's next
/// change is a conflict again. Both sides changed to the SAME bytes is no
/// conflict (our own commit whose answer was lost, or two writers
/// agreeing): that adopts silently.
fn record_conflicts(
    ctx: ConflictCtx<'_>,
    conflicts: &[RepoPath],
) -> Result<(), SyncError> {
    for path in conflicts {
        // by path, not by report line: `a.md`'s line starts with "a", so
        // matching the text skipped a real conflict on `a` whenever `a.md`
        // was recorded first
        if !ctx.recorded.insert(path.clone()) {
            continue; // already recorded this pass
        }
        let rf_commit = ctx.listing.files.get(path).map(|r| r.last_commit_sha.clone()).unwrap_or_default();
        let git_bytes = ctx.storage.read_file(path.as_str(), ctx.listing.rev());
        let remote_sha = git_bytes.as_ref().ok().map(|bytes| crate::blobs::content_sha(bytes));
        let same = match (&remote_sha, ctx.local.get(path)) {
            (Some(sha), Some(lf)) => *sha == lf.sha256,
            _ => false,
        };
        if !same {
            match git_bytes.map_err(anyhow::Error::from).and_then(|b| ctx.blobs.resolve(b)) {
                Ok(bytes) => match save_copy(ctx.dir, path, ctx.writer_id, epoch_s(), &bytes)? {
                    Ok(copy) => ctx.report.conflicts.push(format!("{path} (remote copy: {copy})")),
                    Err(why) => ctx.report.conflicts.push(format!("{path} (remote copy not saved, {why}; it is in git history)")),
                },
                Err(e) => ctx.report.conflicts.push(format!("{path} (remote copy unavailable: {e})")),
            }
        }
        let entry = match ctx.local.get(path) {
            // the remote commit's bytes, not the folder's: an mtime no file
            // has keeps the scan hashing the folder's
            Some(_) if !same => {
                let size = ctx.listing.files.get(path).map_or(0, |r| r.size);
                FileState { sha256: remote_sha.clone().unwrap_or_default(), size, mtime_ns: -1, commit: rf_commit }
            }
            Some(lf) => FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: rf_commit },
            None => match ctx.state.files.get(path) {
                Some(st) => FileState { commit: rf_commit, ..st.clone() },
                None => continue,
            },
        };
        ctx.state.files.insert(path.clone(), entry);
    }
    Ok(())
}

/// Conflict copies of one path `save_copy` names within one second: past
/// it, the copy is not saved (its bytes are in git history), and the
/// report says so.
const COPIES_A_SECOND: usize = 100;

/// The `n`th name (from 1) for the remote's bytes of `path` beside the
/// folder's own in second `ts`: `<stem>.conflict-<ts>-<writer><ext>`, then
/// `<ts>-2`, `<ts>-3`…; or why there is no room.
fn conflict_copy(path: &RepoPath, writer_id: &str, ts: u64, n: usize) -> Result<RepoPath, NotRepoPath> {
    let text = path.as_str();
    let (stem, ext) = match text.rfind('.') {
        Some(i) if !text[i..].contains('/') => (&text[..i], &text[i..]),
        _ => (text, ""),
    };
    let time = if n == 1 { ts.to_string() } else { format!("{ts}-{n}") };
    RepoPath::parse(&format!("{stem}.conflict-{time}-{writer_id}{ext}"))
}

/// Saves the remote's `bytes` of `path` beside the folder's own as a new
/// file (`conflict_copy`), never over anything, an earlier copy included:
/// the name is taken before the bytes land, and a taken one moves on to the
/// next. Its name, or why it was not saved (a path near the limit has no
/// room for the copy's mark; a folder on its way is a symlink). `ts`: the
/// second it is named for (`epoch_s`).
fn save_copy(dir: &Path, path: &RepoPath, writer_id: &str, ts: u64, bytes: &[u8]) -> Result<Result<RepoPath, String>, SyncError> {
    for n in 1..=COPIES_A_SECOND {
        let copy = match conflict_copy(path, writer_id, ts, n) {
            Ok(copy) => copy,
            Err(not) => return Ok(Err(format!("its name {not}"))),
        };
        let Some(target) = place(dir, &copy)? else { return Ok(Err(through_link(&copy))) };
        let io = |e: std::io::Error| SyncError::Io(format!("{}: {e}", target.display()));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(_) => {
                atomic_write(&target, bytes).map_err(|e| SyncError::Io(e.to_string()))?;
                return Ok(Ok(copy));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io(e)),
        }
    }
    Ok(Err(format!("{COPIES_A_SECOND} copies of it were saved this second")))
}

/// Seconds since the Unix epoch: the time in a conflict copy's name.
fn epoch_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Something the scan did not take sits where the fragment has a file: a
/// file outside sync (its .gitignore's), or what is not a regular file.
/// Pulled over, it was lost (nothing of it is in git), so it stays as it
/// is and outside sync: no journal row, so it is never sent nor its
/// absence a deletion. The remote's bytes land beside it once per version
/// (a copy of the same bytes there already is not written again): a
/// conflict, reported each pass while both are there. The same bytes on
/// both sides are nothing to report. Only a regular file is ever read,
/// never through a link: a pipe waits forever for a writer, a device
/// never ends.
fn keep_unscanned(ctx: ConflictCtx<'_>, path: &RepoPath, target: &Path) -> Result<(), SyncError> {
    let regular = |at: &Path| fs::symlink_metadata(at).is_ok_and(|md| md.file_type().is_file());
    if !ctx.recorded.insert(path.clone()) {
        return Ok(());
    }
    ctx.state.files.remove(path);
    let git = ctx.storage.read_file(path.as_str(), ctx.listing.rev())?;
    let bytes = ctx.blobs.resolve(git).map_err(|e| SyncError::blob(path.as_str(), e))?;
    if regular(target) && fs::read(target).is_ok_and(|here| here == bytes) {
        return Ok(());
    }
    // a copy of these bytes beside it already: this version's was saved
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_string).unwrap_or_default();
    let theirs = name(target);
    let (stem, ext) = match theirs.rfind('.') {
        Some(i) if i > 0 => (&theirs[..i], &theirs[i..]),
        _ => (theirs.as_str(), ""),
    };
    let (prefix, suffix) = (format!("{stem}.conflict-"), format!("-{}{ext}", ctx.writer_id));
    let saved = target.parent().and_then(|at| fs::read_dir(at).ok()).is_some_and(|entries| {
        entries.flatten().any(|e| {
            let n = name(&e.path());
            n.starts_with(&prefix) && n.ends_with(&suffix) && e.file_type().is_ok_and(|t| t.is_file()) && fs::read(e.path()).is_ok_and(|b| b == bytes)
        })
    });
    let kept = "a file outside sync is here, kept";
    if saved {
        ctx.report.conflicts.push(format!("{path} ({kept}; remote copy beside it: {stem}.conflict-…{ext})"));
        return Ok(());
    }
    match save_copy(ctx.dir, path, ctx.writer_id, epoch_s(), &bytes)? {
        Ok(copy) => ctx.report.conflicts.push(format!("{path} ({kept}; remote copy: {copy})")),
        Err(why) => ctx.report.conflicts.push(format!("{path} ({kept}; remote copy not saved, {why}; it is in git history)")),
    }
    Ok(())
}

/// A synced path the scan no longer takes stands for a deletion only when
/// nothing at all is there. Anything else (a `.gitignore`d file, what is
/// not a regular file, a path through a symlinked folder, which the scan
/// never follows) is outside sync: its journal row is dropped here, before
/// any plan reads it, so it is never sent, its absence never deletes the
/// fragment's copy, and a pull keeps it (`keep_unscanned`). The paths, for
/// the report.
fn drop_outside_sync(dir: &Path, local: &BTreeMap<RepoPath, LocalFile>, state: &mut SyncState) -> Result<Vec<RepoPath>, SyncError> {
    let mut outside = Vec::new();
    for path in state.files.keys().filter(|p| !local.contains_key(*p)) {
        let gone = match place(dir, path)? {
            Some(at) => vacant(&at)?,
            None => false,
        };
        if !gone {
            outside.push(path.clone());
        }
    }
    for path in &outside {
        state.files.remove(path);
    }
    Ok(outside)
}

/// Whether nothing at all is at `at`, not even a link or a pipe: the one
/// test of absence, for a pull's write and a push's deletion. A folder on
/// the way that is a file means nothing is there either.
fn vacant(at: &Path) -> Result<bool, SyncError> {
    match fs::symlink_metadata(at) {
        Ok(_) => Ok(false),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Ok(true),
        Err(e) => Err(SyncError::Io(format!("{}: {e}", at.display()))),
    }
}

/// Everything record_conflicts needs; keeps it at one argument.
struct ConflictCtx<'a> {
    storage: &'a CodeStorage,
    blobs: &'a crate::blobs::Blobs<'a>,
    dir: &'a Path,
    local: &'a BTreeMap<RepoPath, LocalFile>,
    listing: &'a Listing,
    state: &'a mut SyncState,
    report: &'a mut Report,
    /// the paths recorded as conflicts this pass (the push plan and the
    /// pull can both name one)
    recorded: &'a mut HashSet<RepoPath>,
    writer_id: &'a str,
}

/// Where a repo path is in the folder, for a write or a delete, or `None`
/// when a folder on its way is a symlink. `RepoPath::under` keeps the path
/// under `dir` as a string; a symlinked folder (`cfg` linked to
/// `~/.config`) would carry a write outside all the same. The scan never
/// follows one, so sync never writes or deletes through one either. `dir`
/// itself may be a link: it is the person's to choose.
fn place(dir: &Path, path: &RepoPath) -> Result<Option<PathBuf>, SyncError> {
    let segments: Vec<&str> = path.as_str().split('/').collect();
    let mut at = dir.to_path_buf();
    // the folders on the way, every segment but the file's own: a repo
    // path's few (at most PATH_MAX_BYTES / 2)
    for seg in &segments[..segments.len() - 1] {
        at.push(seg);
        match fs::symlink_metadata(&at) {
            Ok(md) if md.file_type().is_symlink() => return Ok(None),
            Ok(_) => {}
            // the rest of the way is made fresh, as folders
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(SyncError::Io(format!("{}: {e}", at.display()))),
        }
    }
    let target = path.under(dir);
    assert!(target.starts_with(&at), "the way checked leads to the target");
    Ok(Some(target))
}

/// What sync says of a path it would write or delete through a symlink.
fn through_link(path: &RepoPath) -> String {
    format!("{path}: a folder on its way in this folder is a symlink, and sync writes and deletes nothing through one")
}

/// Fetches one file at `rev` (the listing's commit) into the folder: its
/// journal row, or `None` when a folder on its way is a symlink. Files are
/// fetched READS_AT_ONCE at a time; a blob's bytes (up to `BLOB_MAX_BYTES`)
/// BLOB_TRANSFERS_AT_ONCE at a time, through `blob_gate`.
fn pull_file(storage: &CodeStorage, blobs: &crate::blobs::Blobs<'_>, blob_gate: &Gate, dir: &Path, rev: &str, entry: &TreeEntry) -> Result<Option<FileState>, SyncError> {
    let (path, commit) = (&entry.path, entry.last_commit_sha.as_str());
    // the one join of a listing's path onto the folder, checked before
    // anything is fetched for it
    let Some(target) = place(dir, path)? else { return Ok(None) };
    let git = storage.read_file(path.as_str(), rev)?;
    let (sha, md) = land(
        blob_gate,
        git,
        |git| blobs.resolve(git).map_err(|e| SyncError::blob(path.as_str(), e)),
        |bytes| {
            atomic_write(&target, bytes).map_err(|e| SyncError::Io(e.to_string()))?;
            fs::metadata(&target).map_err(|e| SyncError::Io(e.to_string()))
        },
    )?;
    let mtime_ns = md.modified().map(|m| m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0)).unwrap_or(0);
    Ok(Some(FileState { sha256: sha, size: md.len(), mtime_ns, commit: commit.to_string() }))
}

/// A file's bytes from what git holds (`resolve`), their hash, and what
/// `write` made of them. A pointer's bytes (a blob, up to
/// `BLOB_MAX_BYTES`) are fetched and written with a place at `gate`, held
/// until they are written and dropped: however many files a pull fetches at
/// once, the gate's few blobs are in memory. A plain file takes no place.
fn land<T>(
    gate: &Gate,
    git: Vec<u8>,
    resolve: impl FnOnce(Vec<u8>) -> Result<Vec<u8>, SyncError>,
    write: impl FnOnce(&[u8]) -> Result<T, SyncError>,
) -> Result<(String, T), SyncError> {
    let _place = fragment_core::blob::parse(&git).is_some().then(|| gate.enter());
    let bytes = resolve(git)?;
    let sha = sha256_hex(&bytes);
    let wrote = write(&bytes)?;
    drop(bytes);
    Ok((sha, wrote))
}

/// Full-content audit: local truth vs the repo listing + fetched bytes
/// (no shortcuts — every remote file is fetched and hashed).
pub fn verify(client: &Client, name: &str, dir: &Path) -> Result<Report, SyncError> {
    let name = &full_name(client, name)?;
    let state = load_state(dir, name).map_err(|e| SyncError::Io(e.to_string()))?;
    let Scan { files: local, stats, refused } = scan_local(dir, Some(&state), true).map_err(|e| SyncError::Io(e.to_string()))?;
    let storage = CodeStorage::connect(client, name, Wants::Read)?;
    let mut drift = Report { scan: stats, mode: "verify".into(), ..Default::default() };
    let listing = list_main(&storage)?;
    drift.refused = refused.iter().chain(&listing.refused).map(|not| not.to_string()).collect();
    for p in listing.files.keys() {
        match local.get(p) {
            None => drift.conflicts.push(format!("{p}: in the repo, missing locally")),
            Some(lf) => {
                let bytes = storage.read_file(p.as_str(), listing.rev())?;
                if crate::blobs::content_sha(&bytes) != lf.sha256 {
                    drift.conflicts.push(format!("{p}: content differs from the repo"));
                }
            }
        }
    }
    for p in local.keys() {
        if !listing.files.contains_key(p) {
            drift.conflicts.push(format!("{p}: not in the repo"));
        }
    }
    Ok(drift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth;
    use crate::mockcs::MockServer;

    fn client_for(mock: &MockServer) -> Client {
        Client::new(&mock.url, auth::fixed(7))
    }

    fn opts(mode: Mode) -> SyncOptions {
        SyncOptions { mode, writer_id: "deadbeef".into(), ..Default::default() }
    }

    fn rp(path: &str) -> RepoPath {
        RepoPath::parse(path).unwrap()
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fragment-sync3-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scan_uses_cache() {
        let dir = tmpdir("scan");
        fs::create_dir_all(dir.join("site")).unwrap();
        fs::write(dir.join("site/index.html"), b"hi").unwrap();
        let Scan { files: l1, stats: s1, .. } = scan_local(&dir, None, false).unwrap();
        assert_eq!(s1.hashed, 1);
        let mut st = SyncState { schema_version: 3, name: "x".into(), host: None, repo: None, files: BTreeMap::new() };
        let lf = l1.get("site/index.html").unwrap();
        st.files.insert(rp("site/index.html"), FileState { sha256: lf.sha256.clone(), size: lf.size, mtime_ns: lf.mtime_ns, commit: "c".into() });
        let s2 = scan_local(&dir, Some(&st), false).unwrap().stats;
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
        let scanned = scan_local(&src, None, true).unwrap().files;
        assert_eq!(scanned.keys().collect::<Vec<_>>(), ["a.md", "sub/c.md"]);
        fs::remove_dir_all(&src).ok();
    }

    /// Goal: a repo file that does not sync is left alone both ways: never
    /// pulled, and never deleted for being absent from the folder (the
    /// scan skips it). Method: a repo with a dotfile and a node_modules
    /// file; a mirror pass, then another.
    #[test]
    fn repo_files_that_do_not_sync_are_left_alone() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), (".env.example", b"X=1"), ("node_modules/x.js", b"x")]);
        let c = client_for(&mock);
        let dir = tmpdir("left-alone");
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"]);
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.deleted_remote.is_empty() && second.pushed.is_empty(), "{second:?}");
        assert_eq!(mock.paths("t--k3x9", "main"), [".env.example", "a.md", "node_modules/x.js"]);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a listing's path that is not a repo path writes nothing,
    /// anywhere (the audit's M5: an absolute path joined onto the folder
    /// replaced it). Method: a repo seeded past the cell, as a storage
    /// token's commit is, with an absolute path to a file outside the
    /// folder, a `..` path, and a good file; a mirror pass, then another.
    /// A file in the folder whose name is no repo path is not pushed.
    #[test]
    fn a_listing_path_that_is_not_a_repo_path_writes_nothing() {
        let mock = crate::mockcs::start();
        let outside = tmpdir("escape-target");
        let planted = outside.join("planted.txt");
        let planted_path = planted.to_string_lossy().to_string();
        assert!(planted_path.starts_with('/'));
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), (planted_path.as_str(), b"owned"), ("../escaped.txt", b"owned"), ("sub/../../up.txt", b"owned")]);
        let c = client_for(&mock);
        let dir = tmpdir("escape");
        fs::write(dir.join("back\\slash.md"), b"local").unwrap();
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"]);
        assert!(first.pushed.is_empty(), "a name that is no repo path is not pushed: {first:?}");
        assert!(!planted.exists(), "nothing is written at an absolute path");
        assert!(!dir.join("../escaped.txt").exists() && !dir.join("../up.txt").exists(), "nothing is written above the folder");
        let mut refused = first.refused.clone();
        refused.sort();
        assert_eq!(refused.len(), 4, "{refused:?}");
        for (named, path) in refused.iter().zip(["\"../escaped.txt\"", &format!("{planted_path:?}"), "\"back\\\\slash.md\"", "\"sub/../../up.txt\""]) {
            assert!(named.starts_with(path), "{named} names {path}");
        }
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.pulled.is_empty() && second.pushed.is_empty() && second.deleted_remote.is_empty(), "{second:?}");
        assert_eq!(second.refused.len(), 4, "refused again, each pass");
        assert_eq!(mock.paths("t--k3x9", "main").len(), 4, "the repo is left as it was");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    /// Goal: a repo path is never written through a symlinked folder in
    /// the sync folder (a folder `cfg` linked to `~/.config`: the path is
    /// under the folder as a string, its bytes would not be). Method: a
    /// link to a folder outside, a repo file beneath it and one beside it;
    /// a mirror pass, then another.
    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_symlinked_folder() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"a"), ("cfg/fish/config.fish", b"owned"), ("cfg/top.txt", b"owned")]);
        let c = client_for(&mock);
        let dir = tmpdir("through-link");
        let outside = tmpdir("through-link-target");
        std::os::unix::fs::symlink(&outside, dir.join("cfg")).unwrap();
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(first.pulled, ["a.md"], "{first:?}");
        assert!(!outside.join("fish/config.fish").exists() && !outside.join("top.txt").exists(), "nothing is written through the link");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "the folder outside is left as it was");
        assert_eq!(first.refused.len(), 2, "{:?}", first.refused);
        assert!(first.refused.iter().all(|r| r.contains("is a symlink")), "{:?}", first.refused);
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(second.pushed.is_empty() && second.deleted_remote.is_empty() && second.pulled.is_empty(), "the repo's files are left alone: {second:?}");
        assert_eq!(mock.paths("t--k3x9", "main").len(), 3);
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn atomic_write_leaves_no_partial() {
        let dir = tmpdir("atomic");
        atomic_write(&dir.join("a.txt"), b"content").unwrap();
        assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"content");
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains("partial")).collect();
        assert!(leftovers.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_state_reads_fresh() {
        let dir = tmpdir("oldstate");
        fs::create_dir_all(dir.join(".fragment")).unwrap();
        fs::write(dir.join(".fragment/state.json"), b"{\"name\":\"x\",\"files\":{\"a\":{\"rev\":1}}}").unwrap();
        let st = load_state(&dir, "x").unwrap();
        assert!(st.files.is_empty()); // v2 → fresh
        assert_eq!(st.schema_version, 3);
        fs::remove_dir_all(&dir).ok();
    }

    /// The guide states the guard with the code's own numbers.
    #[test]
    fn the_guide_names_the_guards_numbers() {
        let guide = include_str!("../GUIDE.md").split_whitespace().collect::<Vec<_>>().join(" ");
        let rule = format!("more than max({MASS_DELETE_FLOOR}, {MASS_DELETE_PERCENT}%) of known files, or all of them");
        assert!(guide.contains(&rule), "GUIDE.md should say: {rule}");
    }

    /// Goal: a pass tells a script what happened by its exit code alone,
    /// as GUIDE.md says, and a tripped guard is the code a script sees even
    /// beside conflicts (it needs someone to look before anything more
    /// moves). Method: a report of each outcome, the codes written out.
    /// The e2e's sync section checks one of them through the process.
    #[test]
    fn a_report_exits_with_its_outcome() {
        let paths = |ps: &[&str]| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let busy = Report {
            pushed: paths(&["a.md"]),
            pulled: paths(&["b.md"]),
            deleted_remote: paths(&["c.md"]),
            deleted_local: paths(&["d.md"]),
            withheld_deletions: paths(&["e.md"]),
            ..Report::default()
        };
        let conflicted = Report { conflicts: paths(&["f.md (remote copy: f.conflict-1-deadbeef.md)"]), ..Report::default() };
        let guarded = Report { mass_delete_guard: Some(15), ..Report::default() };
        let both = Report { conflicts: paths(&["f.md"]), mass_delete_guard: Some(15), ..Report::default() };
        let unsent = Report { unsent: paths(&["g.md"]), read_only: true, ..Report::default() };
        let unsent_and_conflicted = Report { conflicts: paths(&["f.md"]), ..unsent.clone() };
        assert_eq!(Report::default().exit_code(), 0);
        assert_eq!(busy.exit_code(), 0, "work done, withheld deletions too, is a clean pass");
        assert_eq!(conflicted.exit_code(), 3);
        assert_eq!(guarded.exit_code(), 4);
        assert_eq!(both.exit_code(), 4);
        assert_eq!(unsent.exit_code(), 5);
        assert_eq!(unsent_and_conflicted.exit_code(), 3, "a conflict is the code a script sees beside changes unsent");
        let guide = include_str!("../GUIDE.md");
        assert!(guide.contains("**Exit codes**: 0 clean, 1 failure, 3 conflicts, 4 guard tripped, 5 changes not sent (you may only read it)."), "GUIDE.md names the codes");
    }

    #[test]
    fn mass_delete_guard_semantics() {
        // rule: pending > max(3, 30% of known), or ALL of them
        assert!(mass_delete_trips(11, 0, 30, false).is_some());
        assert!(mass_delete_trips(11, 0, 100, false).is_none()); // 11% only
        assert!(mass_delete_trips(9, 0, 30, false).is_none()); // 30% exactly
        // the live incident: 8 of a 10-file folder (80%) must trip — the
        // old floor of 10 let a whole world change through (8 ≤ 10)
        assert!(mass_delete_trips(0, 8, 10, false).is_some());
        assert!(mass_delete_trips(8, 0, 10, false).is_some());
        // small legitimate deletions stay quiet: 3 or fewer, or a small
        // share of a big folder
        assert!(mass_delete_trips(3, 0, 10, false).is_none());
        assert!(mass_delete_trips(3, 0, 1000, false).is_none());
        // full wipe trips regardless of count (unmounted-folder case)
        assert!(mass_delete_trips(3, 0, 3, false).is_some());
        assert!(mass_delete_trips(1, 0, 1, false).is_some());
        // override
        assert!(mass_delete_trips(11, 0, 30, true).is_none());
        // pull-side deletions count toward the same guard
        assert!(mass_delete_trips(0, 11, 30, false).is_some());
        // empty state floors `known` at 1: mass deletions without state
        // cannot happen for real, but the wipe rule still catches them
        assert!(mass_delete_trips(100, 0, 1, false).is_some());
    }

    // ---- end-to-end against the mock code.storage ----

    /// The fake's request counts, as a test states them.
    fn counts(routes: &[(&str, u32)]) -> BTreeMap<String, u32> {
        routes.iter().map(|(route, n)| (route.to_string(), *n)).collect()
    }

    /// Goal: a pass reads main's head and listing once. Method: count the
    /// fake's requests for passes over a synced folder. Before, a pass with
    /// nothing to push read the head three times and listed twice, and one
    /// that pushed listed again after its own commit.
    #[test]
    fn a_pass_reads_the_head_and_the_listing_once() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a"), ("b.txt", b"b")]);
        let c = client_for(&mock);
        let dir = tmpdir("reads-once");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        mock.take_requests("");
        let read_once = counts(&[("GET branch", 1), ("GET files/metadata", 1), ("GET storage-token", 1)]);

        let idle = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(idle.pushed.is_empty() && idle.pulled.is_empty() && !idle.landed, "{idle:?}");
        assert_eq!(mock.take_requests(""), read_once, "a mirror pass with nothing to do");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert_eq!(mock.take_requests(""), read_once, "a pull pass with nothing to do");

        fs::write(dir.join("a.txt"), b"changed").unwrap();
        fs::remove_file(dir.join("b.txt")).unwrap();
        let pushed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!((pushed.pushed.as_slice(), pushed.deleted_remote.as_slice()), (&["a.txt".to_string()][..], &["b.txt".to_string()][..]));
        assert!(pushed.pulled.is_empty() && pushed.landed, "{pushed:?}");
        assert_eq!(pushed.head, mock.branch("t--k3x9", "main"), "the pass ends on its own commit");
        assert_eq!(
            mock.take_requests(""),
            counts(&[("GET branch", 1), ("GET files/metadata", 1), ("POST commit-pack", 1), ("GET storage-token", 1), ("POST refresh", 1)]),
            "a pass that pushes lists once, and its pull reads the listing it derived"
        );

        // the derived listing named our commit, so nothing reads as changed
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.pushed.is_empty() && again.pulled.is_empty() && again.conflicts.is_empty(), "{again:?}");
        assert_eq!(mock.take_requests(""), read_once);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: the journal is written when a pass changed it, and only
    /// then. Method: its inode across an idle pass and a pushing one (the
    /// write is a rename, so a new inode). Before, every pass rewrote and
    /// fsynced it.
    #[cfg(unix)]
    #[test]
    fn the_journal_is_written_only_when_it_changed() {
        use std::os::unix::fs::MetadataExt;
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a")]);
        let c = client_for(&mock);
        let dir = tmpdir("journal-writes");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        let journal = || fs::metadata(dir.join(".fragment/state.json")).unwrap().ino();
        let first = journal();
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(journal(), first, "an idle pass leaves it alone");
        fs::write(dir.join("b.txt"), b"b").unwrap();
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_ne!(journal(), first, "a pass that pushed wrote it");
        assert!(load_state(&dir, "t--k3x9").unwrap().files.contains_key("b.txt"));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: the listing a pass derives after its own commit is the one
    /// main lists. Method: commit changes (an edit, a new file, a
    /// deletion) on a listing's head, derive, list main, and compare every
    /// entry.
    #[test]
    fn a_derived_listing_is_the_listing_of_main() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a"), ("b.txt", b"b"), ("c/d.txt", b"d")]);
        let c = client_for(&mock);
        let storage = CodeStorage::connect(&c, "t--k3x9", Wants::Write).unwrap();
        let before = list_main(&storage).unwrap();
        let changes = [
            Change::Upsert { path: rp("a.txt"), bytes: b"a, longer now".to_vec() },
            Change::Upsert { path: rp("c/new.txt"), bytes: b"new".to_vec() },
            Change::Delete { path: rp("b.txt") },
        ];
        let tip = storage.commit(before.head.as_deref(), "edit", &Author::writer("deadbeef"), &changes).unwrap();
        let derived = before.landed(tip, &changes);
        let listed = list_main(&storage).unwrap();
        let entries = |l: &Listing| -> BTreeMap<RepoPath, (u64, String, String)> {
            l.files.values().map(|f| (f.path.clone(), (f.size, f.mode.clone(), f.last_commit_sha.clone()))).collect()
        };
        assert_eq!(derived.head, listed.head);
        assert_eq!(entries(&derived), entries(&listed));
        assert_eq!(entries(&listed).len(), 3, "a.txt, c/d.txt, c/new.txt");
    }

    #[test]
    fn push_conflicting_parent_retries_then_succeeds() {
        // competitor moves the tip once: our pack 409s, we refetch,
        // rebuild, and land on the new tip — both changes survive
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[]);
        mock.sabotage_commit_packs(1);
        let c = client_for(&mock);
        let dir = tmpdir("push-conflict-once");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert_eq!(report.pushed.len(), 1);
        assert!(mock.file_at("t--k3x9", "main", "competitor.txt").is_some(), "competitor commit survives");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.txt").unwrap(), b"alpha");
        assert!(mock.commit_pack_count() >= 2, "one rejected attempt plus one landing");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_conflicting_parent_forever_is_bounded() {
        // every attempt is sabotaged: explicit error after MAX_CAS_ATTEMPTS
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[]);
        mock.sabotage_commit_packs(99);
        let c = client_for(&mock);
        let dir = tmpdir("push-conflict-forever");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        let err = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap_err();
        assert!(err.to_string().contains("branch kept moving"), "got: {err}");
        assert_eq!(mock.commit_pack_count(), MAX_CAS_ATTEMPTS, "exactly the bounded number of attempts");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a sync whose commit landed but lost its answer ends clean.
    /// Method: the fake applies the pass's commit and drops the answer. The
    /// old client sent the pack again (a 409), then recorded a conflict
    /// with a copy of its own bytes and exited 3. Now: one pack, no
    /// conflict, exit 0, and the next pass has nothing to do.
    #[test]
    fn a_lost_commit_answer_adopts_what_landed() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("doc.md", b"base"), ("gone.md", b"g"), ("kept.md", b"k")]);
        let c = client_for(&mock);
        let dir = tmpdir("lost-answer");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join("doc.md"), b"ours, changed").unwrap();
        fs::write(dir.join("new.md"), b"new").unwrap();
        fs::remove_file(dir.join("gone.md")).unwrap();
        mock.drop_commit_answers("t--k3x9", 1);
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(mock.commit_pack_count(), 1, "the pack was sent once");
        let copies = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".conflict-")).count();
        assert_eq!(copies, 0, "no conflict copy");
        assert_eq!(mock.file_at("t--k3x9", "main", "doc.md").unwrap(), b"ours, changed");
        assert_eq!(mock.file_at("t--k3x9", "main", "new.md").unwrap(), b"new");
        assert!(mock.file_at("t--k3x9", "main", "gone.md").is_none());
        assert_eq!(fs::read(dir.join("doc.md")).unwrap(), b"ours, changed");
        let st = load_state(&dir, "t--k3x9").unwrap();
        assert!(!st.files.contains_key("gone.md"), "the landed deletion leaves no row");
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.pushed.is_empty() && again.pulled.is_empty() && again.conflicts.is_empty(), "{again:?}");
        assert_eq!(mock.commit_pack_count(), 1, "and nothing is left to send");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: `at_once` runs that many together, never more, and answers
    /// in order. Method: the first `max` tasks each wait until `max` are in
    /// flight, which a pool running fewer at once never reaches (the wait
    /// is bounded so such a pool fails rather than hangs).
    #[test]
    fn at_once_runs_that_many_together_and_answers_in_order() {
        use std::sync::Condvar;
        let (n, max) = (40, READS_AT_ONCE);
        // (in flight now, the most at once, whether `max` were)
        let flight = Mutex::new((0usize, 0usize, false));
        let all_in = Condvar::new();
        let items: Vec<usize> = (0..n).collect();
        let answers = at_once(&items, max, |&i| -> Result<usize, ()> {
            let mut f = flight.lock().unwrap();
            f.0 += 1;
            f.1 = f.1.max(f.0);
            if f.0 == max {
                f.2 = true;
                all_in.notify_all();
            }
            let (mut f, waited) = all_in.wait_timeout_while(f, std::time::Duration::from_secs(10), |f| !f.2).unwrap();
            assert!(!waited.timed_out(), "{max} were never in flight at once");
            f.0 -= 1;
            Ok(i * 2)
        });
        assert_eq!(flight.lock().unwrap().1, max, "{max} at once, never more");
        assert_eq!(answers, (0..n).map(|i| Some(Ok(i * 2))).collect::<Vec<_>>());
    }

    /// Goal: once a task fails, nothing more starts. Method: one at a time,
    /// so which tasks started is certain.
    #[test]
    fn at_once_starts_nothing_after_a_failure() {
        let answers = at_once(&[1, 2, 3, 4, 5], 1, |&i| if i == 3 { Err(i) } else { Ok(i) });
        assert_eq!(answers, vec![Some(Ok(1)), Some(Ok(2)), Some(Err(3)), None, None]);
        assert!(at_once(&[] as &[u8], 4, |_| Ok::<_, ()>(())).is_empty());
    }

    /// Goal: a blob's bytes keep their place at the gate until they are
    /// written and dropped, however many files a pull fetches at once.
    /// Method: land a pointer's bytes and a plain file's, reading the
    /// gate from inside the write. (With the place let go once the bytes
    /// arrived, as a317d948 did, the write sees none: checked.)
    #[test]
    fn a_blobs_gate_is_held_until_its_bytes_are_written() {
        let gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let pointer = fragment_core::blob::pointer(&"a".repeat(64), 2 * 1024 * 1024).into_bytes();
        let (sha, (len, held)) = land(&gate, pointer, |_| Ok(vec![7u8; 16]), |bytes| Ok((bytes.len(), gate.held()))).unwrap();
        assert_eq!((len, held), (16, 1), "a place is held through a blob's write");
        assert_eq!(sha, sha256_hex(&[7u8; 16]), "the hash is the blob's bytes'");
        assert_eq!(gate.held(), 0, "and let go after");
        let (_, held) = land(&gate, b"plain".to_vec(), Ok, |_| Ok(gate.held())).unwrap();
        assert_eq!(held, 0, "a plain file takes no place");
    }

    /// Goal: the gate lets `max` through together and never more. Method:
    /// each of twice that many waits (bounded) until `max` hold a place,
    /// which a gate letting fewer through never reaches.
    #[test]
    fn a_gate_lets_that_many_through_together() {
        let gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let peak = Mutex::new(0usize);
        std::thread::scope(|scope| {
            for _ in 0..BLOB_TRANSFERS_AT_ONCE * 2 {
                scope.spawn(|| {
                    let _place = gate.enter();
                    let now = gate.held();
                    let mut p = peak.lock().unwrap();
                    *p = (*p).max(now);
                    drop(p);
                    let t0 = std::time::Instant::now();
                    while *peak.lock().unwrap() < BLOB_TRANSFERS_AT_ONCE {
                        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{BLOB_TRANSFERS_AT_ONCE} never held places at once");
                        std::thread::yield_now();
                    }
                });
            }
        });
        assert_eq!(*peak.lock().unwrap(), BLOB_TRANSFERS_AT_ONCE);
        assert_eq!(gate.held(), 0);
    }

    /// Goal: a push whose blob is not stored commits nothing, and says
    /// which file. Method: a folder of notes and one file of 1 MiB, whose
    /// upload fails (the fake host answers no blob threshold), the notes
    /// read at once beside it.
    #[test]
    fn a_push_whose_blob_is_not_stored_commits_nothing() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("seed.md", b"s")]);
        let c = client_for(&mock);
        let dir = tmpdir("push-blob-fails");
        for i in 0..12 {
            fs::write(dir.join(format!("n-{i:02}.md")), format!("note {i}")).unwrap();
        }
        fs::write(dir.join("big.bin"), vec![7u8; fragment_core::blob::BLOB_MIN_BYTES + 17]).unwrap();
        let before = mock.commit_pack_count();
        let failed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap_err();
        assert!(failed.to_string().contains("big.bin"), "{failed}");
        assert_eq!(mock.commit_pack_count(), before, "nothing committed");
        assert!(!state_path(&dir).exists(), "and no journal");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: files written at once never share a partial file. Before,
    /// `a.md` and `a.txt` both wrote `a.fragment-partial-<pid>`.
    #[test]
    fn partial_files_are_named_for_their_whole_file() {
        let dir = Path::new("/folder/notes");
        let (md, txt) = (partial_path(&dir.join("a.md")), partial_path(&dir.join("a.txt")));
        assert_ne!(md, txt);
        assert_ne!(partial_path(&dir.join("a.md")), md, "and each write its own");
        assert_eq!(md.parent(), Some(dir), "beside its file, for the rename");
        let name = md.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("a.md.") && !syncable(&format!("notes/{name}")), "{name}");
    }

    /// Goal: a pull reads its files READS_AT_ONCE at a time, and a failed
    /// read leaves no journal that reads as complete. Method: a 40-file
    /// main pulled into an empty folder (valid), again (replay); into
    /// another with one file's read failing (a typed error, no journal),
    /// then again once it answers (restart): each file is read once in
    /// all, what the failed pass wrote adopted by its content.
    #[test]
    fn a_pull_reads_at_once_and_a_failed_read_saves_no_journal() {
        let mock = crate::mockcs::start();
        let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("d{}/f-{i:02}.md", i % 3), format!("file {i}\n").into_bytes())).collect();
        let seeded: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
        mock.seed_repo("t--k3x9", &seeded);
        let c = client_for(&mock);
        let all_there = |dir: &Path| {
            for (p, b) in &files {
                assert_eq!(fs::read(dir.join(p)).ok().as_deref(), Some(b.as_slice()), "{p}");
            }
            assert_eq!(load_state(dir, "t--k3x9").unwrap().files.len(), files.len(), "a journal row for each");
        };
        let reads = |mock: &MockServer| mock.take_requests("").get("GET file").copied().unwrap_or(0);

        let dir = tmpdir("pull-at-once");
        mock.take_requests("");
        let pulled = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert_eq!(pulled.pulled.len(), files.len());
        all_there(&dir);
        assert_eq!(reads(&mock), 40, "each file read once");
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert!(again.pulled.is_empty(), "{again:?}");
        assert_eq!(reads(&mock), 0, "a replay reads nothing");

        let cut = tmpdir("pull-cut");
        mock.fail_file_read(Some("d1/f-19.md"));
        let failed = sync_once(&c, "t--k3x9", &cut, &opts(Mode::Pull)).unwrap_err();
        assert!(matches!(failed, SyncError::Cs(_)), "{failed}");
        assert!(!state_path(&cut).exists(), "a failed pull saves no journal");
        assert!(!cut.join("d1/f-19.md").exists());
        mock.fail_file_read(None);
        mock.take_requests("");
        let resumed = sync_once(&c, "t--k3x9", &cut, &opts(Mode::Pull)).unwrap();
        all_there(&cut);
        assert!(resumed.pulled.contains(&"d1/f-19.md".to_string()), "{resumed:?}");
        assert_eq!(reads(&mock), 40, "each file read once more: what landed adopted by its content, the rest fetched");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&cut).ok();
    }

    #[test]
    fn mirror_pushes_and_pulls() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("site/remote-only.txt", b"r")]);
        let c = client_for(&mock);
        let dir = tmpdir("mirror");
        fs::write(dir.join("local-only.txt"), b"l").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(report.pushed, vec!["local-only.txt"]);
        assert_eq!(report.pulled, vec!["site/remote-only.txt"]);
        assert_eq!(fs::read(dir.join("site/remote-only.txt")).unwrap(), b"r");
        assert!(mock.file_at("t--k3x9", "main", "local-only.txt").is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn live_is_pulled_not_main() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"a1"), ("gone.txt", b"g")]);
        mock.set_branch("t--k3x9", "live", &mock.branch("t--k3x9", "main").unwrap());
        mock.external_commit("t--k3x9", "main", &[("a.txt", Some(b"a2")), ("new.txt", Some(b"n"))], "not deployed");
        let c = client_for(&mock);
        let dir = tmpdir("live");
        let live = SyncOptions { live: true, prune: true, ..opts(Mode::Pull) };
        sync_once(&c, "t--k3x9", &dir, &live).unwrap();
        assert_eq!((fs::read(dir.join("a.txt")).unwrap(), dir.join("new.txt").exists()), (b"a1".to_vec(), false), "live's files, not main's");
        fs::write(dir.join("local.txt"), b"l").unwrap();
        // a deploy moves live: the next pull brings it, deletions included, and pushes nothing
        mock.external_commit("t--k3x9", "main", &[("gone.txt", None)], "remove");
        mock.set_branch("t--k3x9", "live", &mock.branch("t--k3x9", "main").unwrap());
        let report = sync_once(&c, "t--k3x9", &dir, &live).unwrap();
        assert_eq!((fs::read(dir.join("a.txt")).unwrap(), dir.join("new.txt").exists(), dir.join("gone.txt").exists()), (b"a2".to_vec(), true, false));
        assert!(report.pushed.is_empty() && mock.file_at("t--k3x9", "main", "local.txt").is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deletion_pushes_and_propagates_back() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
        let c = client_for(&mock);
        let dir = tmpdir("delete");
        // first pass: adopt both files locally
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        // delete one locally, push the deletion
        fs::remove_file(dir.join("gone.txt")).unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert_eq!(report.deleted_remote, vec!["gone.txt"]);
        assert!(mock.file_at("t--k3x9", "main", "gone.txt").is_none());
        assert!(mock.file_at("t--k3x9", "main", "kept.txt").is_some());
        // a second folder (fresh state) pulling sees the deletion
        let dir2 = tmpdir("delete-other");
        sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Pull)).unwrap();
        assert!(dir2.join("kept.txt").exists());
        assert!(!dir2.join("gone.txt").exists(), "pull of an empty path must not create it");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn remote_deletion_propagates_in_mirror() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
        let c = client_for(&mock);
        // two folders adopt both files
        let dir = tmpdir("rdelete");
        let dir2 = tmpdir("rdelete-other");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        // dir2 pulls, deletes gone.txt, pushes the deletion
        sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Pull)).unwrap();
        fs::remove_file(dir2.join("gone.txt")).unwrap();
        sync_once(&c, "t--k3x9", &dir2, &opts(Mode::Push)).unwrap();
        // mirror folder: local untouched → deletion propagates locally
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(report.deleted_local, vec!["gone.txt"]);
        assert!(!dir.join("gone.txt").exists());
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[test]
    fn withheld_deletion_still_applies_on_a_later_prune_pass() {
        // pull without --prune withholds; the FOLLOWING pass with --prune
        // must apply it — dropping the state row on withhold made the
        // deletion unknowable (e2e: filesync modes sequence)
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("gone.txt", b"g"), ("kept.txt", b"k")]);
        let c = client_for(&mock);
        let dir = tmpdir("prune-late");
        let other = tmpdir("prune-late-other");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
        fs::remove_file(other.join("gone.txt")).unwrap();
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Push)).unwrap();
        // withhold first
        let withheld = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert_eq!(withheld.withheld_deletions, vec!["gone.txt"]);
        assert!(dir.join("gone.txt").exists());
        // then apply
        let report = sync_once(&c, "t--k3x9", &dir, &SyncOptions { prune: true, ..opts(Mode::Pull) }).unwrap();
        assert_eq!(report.deleted_local, vec!["gone.txt"]);
        assert!(!dir.join("gone.txt").exists());
        assert!(dir.join("kept.txt").exists());
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn mass_deletion_guard_blocks_push() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"1"), ("b", b"2"), ("c", b"3")]);
        let c = client_for(&mock);
        let dir = tmpdir("guard");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        // wipe the folder locally: full-wipe rule trips (3 known, 3 pending)
        fs::remove_file(dir.join("a")).unwrap();
        fs::remove_file(dir.join("b")).unwrap();
        fs::remove_file(dir.join("c")).unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert!(report.mass_delete_guard.is_some());
        assert_eq!(report.exit_code(), 4);
        assert!(mock.file_at("t--k3x9", "main", "a").is_some(), "nothing deleted while guarded");
        // override applies it
        let report = sync_once(&c, "t--k3x9", &dir, &SyncOptions { apply_mass_delete: true, ..opts(Mode::Push) }).unwrap();
        assert_eq!(report.deleted_remote.len(), 3);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wiped_folder_in_pull_mode_re_downloads_not_guard() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"1"), ("b", b"2"), ("c", b"3")]);
        let c = client_for(&mock);
        let dir = tmpdir("wipe-pull");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        fs::remove_file(dir.join("a")).unwrap();
        fs::remove_file(dir.join("b")).unwrap();
        fs::remove_file(dir.join("c")).unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        assert!(report.mass_delete_guard.is_none(), "pull must not trip on local deletions");
        assert_eq!(report.pulled.len(), 3, "everything re-downloads");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_both_changed_saves_remote_copy() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("shared.txt", b"base")]);
        let c = client_for(&mock);
        let dir = tmpdir("conflict");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        // remote side changes it
        let other = tmpdir("conflict-other");
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
        fs::write(other.join("shared.txt"), b"theirs").unwrap();
        sync_once(&c, "t--k3x9", &other, &opts(Mode::Push)).unwrap();
        // local side changes it too
        fs::write(dir.join("shared.txt"), b"ours").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.exit_code(), 3);
        assert_eq!(fs::read(dir.join("shared.txt")).unwrap(), b"ours", "local keeps ours");
        // the copy name splits stem/ext like the old engine: shared.conflict-<ts>-<w>.txt
        let copy = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).find(|n| n.starts_with("shared.conflict-"));
        assert!(copy.is_some(), "remote copy saved beside it");
        let copy_bytes = fs::read(dir.join(copy.unwrap())).unwrap();
        assert_eq!(copy_bytes, b"theirs");
        assert_eq!(mock.file_at("t--k3x9", "main", "shared.txt").unwrap(), b"theirs", "remote untouched by our conflict");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&other).ok();
    }

    /// Goal: a conflict on `a` is recorded even when one on `a.md` was
    /// recorded first. Method: record them in that order within one pass
    /// (the pull walks a HashMap, so in a real pass the order is chance);
    /// matching report text found "a.md (remote copy: …)" starting with "a"
    /// and skipped `a` silently, on every pass.
    #[test]
    fn conflicts_are_tracked_by_path_not_report_text() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"theirs a"), ("a.md", b"theirs a.md")]);
        let c = client_for(&mock);
        let dir = tmpdir("conflict-paths");
        fs::write(dir.join("a"), b"ours a").unwrap();
        fs::write(dir.join("a.md"), b"ours a.md").unwrap();
        let storage = CodeStorage::connect(&c, "t--k3x9", Wants::Write).unwrap();
        let blobs = crate::blobs::Blobs::new(&c, "t--k3x9");
        let listing = list_main(&storage).unwrap();
        let local = scan_local(&dir, None, true).unwrap().files;
        let mut state = SyncState { schema_version: 3, name: "t--k3x9".into(), host: None, repo: None, files: BTreeMap::new() };
        let mut report = Report::default();
        let mut recorded = HashSet::new();
        for path in ["a.md", "a", "a.md", "a"] {
            let ctx = ConflictCtx { storage: &storage, blobs: &blobs, dir: &dir, local: &local, listing: &listing, state: &mut state, report: &mut report, recorded: &mut recorded, writer_id: "deadbeef" };
            record_conflicts(ctx, &[rp(path)]).unwrap();
        }
        assert_eq!(report.conflicts.len(), 2, "each path once: {:?}", report.conflicts);
        assert!(report.conflicts[0].starts_with("a.md (remote copy: a.conflict-"), "{:?}", report.conflicts);
        assert!(report.conflicts[1].starts_with("a (remote copy: a.conflict-"), "{:?}", report.conflicts);
        let copies: Vec<Vec<u8>> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".conflict-")).map(|e| fs::read(e.path()).unwrap()).collect();
        assert!(copies.contains(&b"theirs a".to_vec()) && copies.contains(&b"theirs a.md".to_vec()), "both remote copies saved");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_records_both_a_and_a_md_conflicts() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a", b"base a"), ("a.md", b"base a.md")]);
        let c = client_for(&mock);
        let dir = tmpdir("pull-conflict-paths");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        mock.external_commit("t--k3x9", "main", &[("a", Some(b"theirs a")), ("a.md", Some(b"theirs a.md"))], "theirs");
        // a different size, so the scan re-hashes whatever the mtime says
        fs::write(dir.join("a"), b"ours, a").unwrap();
        fs::write(dir.join("a.md"), b"ours, a.md").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let mut paths: Vec<&str> = report.conflicts.iter().filter_map(|c| c.split(' ').next()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a", "a.md"], "{:?}", report.conflicts);
        assert_eq!(fs::read(dir.join("a")).unwrap(), b"ours, a");
        assert_eq!(fs::read(dir.join("a.md")).unwrap(), b"ours, a.md");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_catches_drift() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"same"), ("b.txt", b"will-drift")]);
        let c = client_for(&mock);
        let dir = tmpdir("verify");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let clean = verify(&c, "t--k3x9", &dir).unwrap();
        assert!(clean.conflicts.is_empty(), "{:?}", clean.conflicts);
        assert_eq!(clean.exit_code(), 0, "a folder in sync exits 0");
        // same size, different content — the exact lie the audit exists for
        fs::write(dir.join("b.txt"), b"went-drft").unwrap();
        let report = verify(&c, "t--k3x9", &dir).unwrap();
        assert!(report.conflicts.iter().any(|c| c.starts_with("b.txt")), "{:?}", report.conflicts);
        assert!(report.conflicts.iter().all(|c| !c.starts_with("a.txt")));
        assert_eq!(report.exit_code(), 3, "drift exits as a conflict does");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: `fragment sync` (and `verify`) take a fragment's bare label,
    /// as every other subcommand does (`Client::fragment`): a folder synced
    /// as `t--k3x9` syncs as `t` (prod, 2026-11-02: `fragment sync
    /// workarounds` refused a folder bound to `workarounds--f3xz`). Valid:
    /// the label syncs the bound folder, and the journal still names it in
    /// full. Restart: a later run's client, its names unresolved, verifies
    /// it by the label. Invalid: another fragment's label, and a label of
    /// none, refuse before anything moves. Replay: a journal an earlier
    /// bare-label sync wrote (`"name": "t"`) syncs by either name, and is
    /// named in full after.
    #[test]
    fn a_bare_label_syncs_the_folder_its_fragment_is_bound_to() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"a")]);
        mock.seed_repo("u--m4pq", &[("u.md", b"u")]);
        let dir = tmpdir("bare-label");
        let journal = || -> serde_json::Value { serde_json::from_slice(&fs::read(state_path(&dir)).unwrap()).unwrap() };
        sync_once(&client_for(&mock), "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join("b.md"), b"b").unwrap();
        let report = sync_once(&client_for(&mock), "t", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(report.pushed, ["b.md"]);
        assert_eq!(journal()["name"], "t--k3x9", "the journal names it in full");
        let checked = verify(&client_for(&mock), "t", &dir).unwrap();
        assert_eq!((checked.exit_code(), checked.conflicts.len()), (0, 0), "{:?}", checked.conflicts);

        let packs = mock.commit_pack_count();
        fs::write(dir.join("c.md"), b"c").unwrap();
        let other = sync_once(&client_for(&mock), "u", &dir, &opts(Mode::Mirror)).unwrap_err();
        assert!(other.to_string().contains("synced to fragment 't--k3x9', not 'u--m4pq'"), "{other}");
        let none = sync_once(&client_for(&mock), "garden", &dir, &opts(Mode::Mirror)).unwrap_err();
        assert!(matches!(&none, SyncError::Host(e) if e.code == crate::api::Code::NotFound), "{none}");
        assert!(verify(&client_for(&mock), "u", &dir).is_err(), "verify refuses another fragment's label too");
        assert_eq!(mock.commit_pack_count(), packs, "a refused name sends nothing");
        assert!(!dir.join("u.md").exists(), "nor pulls anything");

        for name in ["t--k3x9", "t"] {
            let mut bare = journal();
            bare["name"] = "t".into();
            fs::write(state_path(&dir), serde_json::to_vec(&bare).unwrap()).unwrap();
            sync_once(&client_for(&mock), name, &dir, &opts(Mode::Mirror)).unwrap();
            assert_eq!(journal()["name"], "t--k3x9", "a bare journal synced as {name} is named in full after");
        }
        let mut wrong = journal();
        wrong["name"] = "u".into();
        fs::write(state_path(&dir), serde_json::to_vec(&wrong).unwrap()).unwrap();
        let err = sync_once(&client_for(&mock), "t", &dir, &opts(Mode::Mirror)).unwrap_err();
        assert!(err.to_string().contains("synced to fragment 'u', not 't--k3x9'"), "another label's journal still refuses: {err}");
        fs::remove_dir_all(&dir).ok();
    }

    // ---- the world-change regressions (both found live in one day) ----

    #[test]
    fn journal_bound_to_other_world_refuses() {
        // a journal from a DIFFERENT world must refuse before mirroring
        // that world over the folder. Two dimensions, two incidents:
        // (a) different repo, same host — a recreated fragment
        // (b) same repo name, different host — dev vs prod (repo ids
        //     alone can't tell these apart when the service reports
        //     name-form identities)
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("old.txt", b"remote world")]);
        let c = client_for(&mock);
        for (tag, journal) in [
            (
                "other repo, same host",
                format!("{{\"schemaVersion\":3,\"name\":\"t--k3x9\",\"host\":\"{}\",\"repo\":\"urn:some-other-repo\",\"files\":{{}}}}", mock.url),
            ),
            (
                "same repo, other host",
                r#"{"schemaVersion":3,"name":"t--k3x9","host":"https://fragment.club","repo":"t--k3x9","files":{}}"#.to_string(),
            ),
        ] {
            let dir = tmpdir("world-mismatch");
            fs::write(dir.join("local.txt"), b"local world").unwrap();
            fs::create_dir_all(dir.join(".fragment")).unwrap();
            fs::write(dir.join(".fragment/state.json"), journal).unwrap();
            let err = match sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)) {
                Err(SyncError::Repo(msg)) => msg,
                other => panic!("{tag}: expected Repo error, got {other:?}"),
            };
            assert!(err.contains("rebinds"), "{tag}: error offers the remedy: {err}");
            // nothing touched: local intact, no commits sent
            assert_eq!(fs::read(dir.join("local.txt")).unwrap(), b"local world", "{tag}");
            assert_eq!(mock.commit_pack_count(), 0, "{tag}");
            fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn recreated_remote_world_guard_holds_local_files() {
        // same repo identity, but the remote was reset (fresh seed, empty
        // tree): the journal's files look remotely-deleted and the mirror
        // would delete them locally — the guard must refuse instead
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[]);
        let c = client_for(&mock);
        let dir = tmpdir("world-reset");
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.txt")), format!("content {i}").as_bytes()).unwrap();
        }
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap(); // binds the journal
        assert_eq!(mock.refresh_count(), 1, "landing sync nudges the pin refresh");
        mock.seed_repo("t--k3x9", &[]); // server-side world reset, same repo id
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(report.mass_delete_guard.is_some(), "5 of 5 remotely-deleted must trip the guard");
        for i in 0..5 {
            assert!(dir.join(format!("f{i}.txt")).exists(), "f{i}.txt must survive the refusal");
        }
        // the escape hatch applies it
        let o = SyncOptions { apply_mass_delete: true, ..opts(Mode::Mirror) };
        let report = sync_once(&c, "t--k3x9", &dir, &o).unwrap();
        assert!(report.mass_delete_guard.is_none());
        assert_eq!(report.deleted_local.len() + report.pushed.len(), 5, "re-seeded local files push back");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bootstrap_same_size_different_content_pushes() {
        // stateless bootstrap used to treat equal SIZE as same FILE — a
        // same-size different-content file silently never pushed (found
        // live). Content must decide.
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"aaaa")]);
        let c = client_for(&mock);
        let dir = tmpdir("bootstrap-diff");
        fs::write(dir.join("a.txt"), b"bbbb").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert_eq!(report.pushed, vec!["a.txt".to_string()], "equal size, different content must push");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.txt").unwrap(), b"bbbb");
        assert!(load_state(&dir, "t--k3x9").unwrap().files.contains_key("a.txt"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bootstrap_identical_content_adopts_without_commit() {
        // the flip side: byte-identical files adopt into the journal with
        // no commit and no re-fetch on the next pass
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"same")]);
        let c = client_for(&mock);
        let dir = tmpdir("bootstrap-same");
        fs::write(dir.join("a.txt"), b"same").unwrap();
        let report = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap();
        assert!(report.pushed.is_empty(), "identical content is not a push");
        assert_eq!(mock.commit_pack_count(), 0, "no commit-packs sent — the seed commit is server-side");
        let r2 = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(r2.pushed.is_empty() && r2.pulled.is_empty(), "adopted file stays quiet");
        let st = load_state(&dir, "t--k3x9").unwrap();
        assert_eq!(st.repo.as_deref(), Some("t--k3x9"), "journal binds to the repo identity");
        assert_eq!(st.host.as_deref(), Some(mock.url.as_str()), "journal binds to the host identity");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a viewer syncs a fragment read-only (its token only reads),
    /// and never loses an edit: a mirror pulls, sends nothing, and keeps
    /// what it would have sent in the folder, named and exiting 5; a file
    /// deleted there comes back; both sides' change is a conflict with
    /// theirs beside it (exit 3); a pull reads whoever asks. Method: the
    /// host routes hold the caller a viewer; count the commit packs.
    #[test]
    fn a_viewers_mirror_pulls_and_sends_nothing() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"theirs 1"), ("b.md", b"b")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = tmpdir("viewer-mirror");
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(first.read_only && first.unsent.is_empty() && first.exit_code() == 0, "{first:?}");
        assert_eq!(first.pulled, ["a.md", "b.md"]);

        // edited, made and deleted here: none of it sent, none of it lost
        fs::write(dir.join("a.md"), "mine").unwrap();
        fs::write(dir.join("c.md"), "new").unwrap();
        fs::remove_file(dir.join("b.md")).unwrap();
        let held = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(held.unsent, ["a.md", "b.md", "c.md"]);
        assert_eq!(held.exit_code(), 5);
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(dir.join("c.md")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dir.join("b.md")).unwrap(), "b", "a file deleted here is pulled back");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.md").unwrap(), b"theirs 1");
        assert!(mock.file_at("t--k3x9", "main", "c.md").is_none());

        // its editor changes what this viewer changed: theirs lands beside
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"theirs 2"))], "theirs");
        let both = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(both.conflicts.len(), 1, "{both:?}");
        assert_eq!(both.exit_code(), 3);
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "the viewer's copy stays");
        let beside: Vec<String> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.contains(".conflict-")).collect();
        assert_eq!(beside.len(), 1, "theirs saved beside: {beside:?}");
        assert_eq!(fs::read_to_string(dir.join(&beside[0])).unwrap(), "theirs 2");

        // a pull asks for a token that only reads, so a viewer's pulls
        let other = tmpdir("viewer-pull");
        let pulled = sync_once(&c, "t--k3x9", &other, &opts(Mode::Pull)).unwrap();
        assert!(pulled.read_only && pulled.unsent.is_empty() && fs::read_to_string(other.join("a.md")).unwrap() == "theirs 2", "{pulled:?}");
        assert_eq!(mock.commit_pack_count(), 0, "nothing was ever sent");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&other).ok();
    }

    /// Goal: a viewer's push is refused before anything moves, in the
    /// host's words, and an editor's mirror still writes. Method: the host
    /// routes hold the caller a viewer, then its editor again.
    #[test]
    fn a_viewers_push_is_refused_and_an_editors_mirror_writes() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"1")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = tmpdir("viewer-push");
        fs::write(dir.join("b.md"), "mine").unwrap();
        let refused = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Push)).unwrap_err();
        assert!(matches!(&refused, SyncError::Cs(CsError::Host(e)) if e.code == Code::Forbidden), "{refused}");
        assert!(refused.to_string().contains("editor"), "{refused}");
        assert_eq!(mock.commit_pack_count(), 0);

        mock.view_only("t--k3x9", false);
        let wrote = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(!wrote.read_only && wrote.pushed == ["b.md"] && wrote.unsent.is_empty(), "{wrote:?}");
        fs::remove_dir_all(&dir).ok();
    }

    /// The `.conflict-` copies in a folder, their text by name.
    fn conflict_copies(dir: &Path) -> Vec<String> {
        let mut copies: Vec<String> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.contains(".conflict-")).collect();
        copies.sort();
        copies.iter().map(|n| fs::read_to_string(dir.join(n)).unwrap()).collect()
    }

    /// Goal: a viewer's bytes are never overwritten, where the folder has
    /// no journal for them (a folder its own, or a file the fragment adds
    /// later beside the viewer's own new one): theirs lands beside, the
    /// viewer's stays unsent, and the next pass leaves both. Method: the
    /// host routes hold the caller a viewer; a folder without a journal,
    /// then a remote addition colliding with an unsent new file.
    #[test]
    fn a_viewers_own_file_is_never_fetched_over() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"remote-original")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = tmpdir("viewer-bootstrap");
        fs::write(dir.join("a.md"), "local-unpublished").unwrap();
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "local-unpublished");
        assert_eq!(conflict_copies(&dir), ["remote-original"]);
        assert!(first.unsent == ["a.md"] && first.exit_code() == 3, "{first:?}");
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "local-unpublished");
        assert!(again.unsent == ["a.md"] && again.conflicts.is_empty() && again.exit_code() == 5, "the same remote is no new conflict: {again:?}");
        assert_eq!(conflict_copies(&dir).len(), 1);

        // a new file of the viewer's, then the fragment adds one there
        fs::write(dir.join("c.md"), "mine, new").unwrap();
        let unsent = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(unsent.unsent, ["a.md", "c.md"]);
        mock.external_commit("t--k3x9", "main", &[("c.md", Some(b"theirs, new"))], "theirs");
        let collided = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("c.md")).unwrap(), "mine, new");
        assert!(conflict_copies(&dir).contains(&"theirs, new".to_string()), "{collided:?}");
        assert_eq!(collided.exit_code(), 3);
        assert_eq!(mock.commit_pack_count(), 0);
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a viewer's edit outlives every later remote change: each new
    /// remote version is a conflict again (theirs beside, the edit kept),
    /// across a restart (a fresh process reads the journal), and a remote
    /// deletion leaves the edit. Method: the host routes hold the caller a
    /// viewer; three remote changes after one local edit.
    #[test]
    fn a_viewers_edit_outlives_each_later_remote_change() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"v1")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = tmpdir("viewer-edit-lives");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join("a.md"), "mine").unwrap();
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v2"))], "theirs");
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(second.exit_code(), 3);
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v3"))], "theirs again");
        let third = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "never fetched over: {third:?}");
        assert!(third.exit_code() == 3 && third.unsent == ["a.md"], "{third:?}");
        // theirs beside it, each version its own copy
        let mut copies = conflict_copies(&dir);
        copies.sort();
        assert_eq!(copies, ["v2", "v3"]);
        let quiet = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(quiet.conflicts.is_empty() && quiet.unsent == ["a.md"] && quiet.exit_code() == 5, "{quiet:?}");

        mock.external_commit("t--k3x9", "main", &[("a.md", None)], "removed");
        let removed = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine", "a remote deletion leaves the edit: {removed:?}");
        assert!(removed.deleted_local.is_empty() && removed.unsent == ["a.md"] && removed.exit_code() == 5, "{removed:?}");

        // its editor again, the same folder sends the edit
        mock.view_only("t--k3x9", false);
        let sent = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(sent.pushed == ["a.md"] && mock.file_at("t--k3x9", "main", "a.md").as_deref() == Some(&b"mine"[..]), "{sent:?}");
        fs::remove_dir_all(&dir).ok();
    }

    /// A folder git keeps, ignoring `ignored` (the scan honors .gitignore
    /// inside a git working tree).
    fn git_folder(tag: &str, ignored: &str) -> PathBuf {
        let dir = tmpdir(tag);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".gitignore"), format!("{ignored}\n")).unwrap();
        dir
    }

    /// Goal: a file outside sync (.gitignore'd) where the fragment has one
    /// is never pulled over, for a viewer or an editor, in a folder's first
    /// pass or a later one: it stays as it is, theirs lands beside it once
    /// per version, the pass reports a conflict, and the ignored file is
    /// never sent. Method: a git folder ignoring a.md holds a draft of it;
    /// the fragment has another a.md; a viewer's mirror, then an editor's
    /// pull and mirror, then a new remote version.
    #[test]
    fn a_file_outside_sync_is_never_pulled_over() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"remote"), ("b.md", b"b")]);
        mock.view_only("t--k3x9", true);
        let c = client_for(&mock);
        let dir = git_folder("outside-sync", "a.md");
        fs::write(dir.join("a.md"), "my private draft").unwrap();
        let copies = |dir: &Path| conflict_copies(dir);

        let viewer = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft", "{viewer:?}");
        assert!(viewer.pulled == ["b.md"] && viewer.conflicts.len() == 1 && viewer.unsent.is_empty() && viewer.exit_code() == 3, "{viewer:?}");
        assert_eq!(copies(&dir), ["remote"]);
        let again = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert!(again.conflicts.len() == 1 && again.pulled.is_empty(), "reported while both are there: {again:?}");
        assert_eq!(copies(&dir), ["remote"], "one copy a version");

        // its editor's pull and mirror keep it too, and never send it
        mock.view_only("t--k3x9", false);
        let pulled = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let mirrored = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft");
        assert!(pulled.conflicts.len() == 1 && mirrored.conflicts.len() == 1 && mirrored.pushed.is_empty() && mirrored.deleted_remote.is_empty(), "{pulled:?} {mirrored:?}");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.md").as_deref(), Some(&b"remote"[..]), "the ignored file is never sent, nor theirs deleted");
        assert_eq!(mock.commit_pack_count(), 0);

        // a new remote version lands beside it too
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"remote 2"))], "theirs");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "my private draft");
        assert!(copies(&dir).contains(&"remote 2".to_string()), "{:?}", copies(&dir));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: an editor's file, synced and then put outside sync (added to
    /// .gitignore), is kept when the fragment's copy moves on, and the
    /// fragment's is never deleted for it. Method: an editor syncs a.md,
    /// ignores it and edits it; another writer changes it; mirror twice.
    #[test]
    fn an_editors_file_put_outside_sync_is_kept() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"v1")]);
        let c = client_for(&mock);
        let dir = git_folder("editor-outside", "nothing.md");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        fs::write(dir.join(".gitignore"), "a.md\n").unwrap();
        fs::write(dir.join("a.md"), "mine, now private").unwrap();
        mock.external_commit("t--k3x9", "main", &[("a.md", Some(b"v2"))], "theirs");
        let first = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        let second = sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "mine, now private", "{first:?}");
        assert!(first.conflicts.len() == 1 && second.deleted_remote.is_empty() && second.pushed.is_empty(), "{first:?} {second:?}");
        assert_eq!(mock.file_at("t--k3x9", "main", "a.md").as_deref(), Some(&b"v2"[..]));
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: what is not a regular file where the fragment has one (a pipe,
    /// a link to one) is kept and reported, never read: a pass ends. Method:
    /// a FIFO with no writer at a.md and a link to it at b.md; the pass runs
    /// on a thread, and if it has not ended in 20 s the test opens the
    /// pipe's write end (so the read ends) and fails.
    #[cfg(unix)]
    #[test]
    fn a_pipe_where_the_fragment_has_a_file_is_kept_unread() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.md", b"remote a"), ("b.md", b"remote b")]);
        mock.view_only("t--k3x9", true);
        let dir = tmpdir("pipe-kept");
        let fifo = dir.join("a.md");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success(), "mkfifo");
        std::os::unix::fs::symlink(&fifo, dir.join("b.md")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (url, at) = (mock.url.clone(), dir.clone());
        let pass = std::thread::spawn(move || {
            let c = Client::new(&url, auth::fixed(7));
            tx.send(sync_once(&c, "t--k3x9", &at, &opts(Mode::Mirror)).map(|r| r.conflicts.len())).ok();
        });
        let mut ended = rx.recv_timeout(std::time::Duration::from_secs(20));
        let stuck = ended.is_err();
        // stuck reading the pipe (through either path): its write end,
        // opened without waiting and closed, ends each read, at most 50
        // times, so the test fails and never hangs
        for _ in 0..50 {
            if ended.is_ok() {
                break;
            }
            use std::os::unix::fs::OpenOptionsExt;
            drop(fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo));
            ended = rx.recv_timeout(std::time::Duration::from_millis(200));
        }
        pass.join().unwrap();
        assert!(!stuck, "a pass read the pipe: {ended:?}");
        assert!(matches!(ended, Ok(Ok(2))), "the pass ended, both kept and reported: {ended:?}");
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(&fs::symlink_metadata(&fifo).unwrap().file_type()) && fs::symlink_metadata(dir.join("b.md")).unwrap().file_type().is_symlink());
        assert_eq!(conflict_copies(&dir).len(), 2, "theirs beside each");
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal: a conflict copy never replaces a file, an earlier copy of the
    /// same second included: each takes the next free name. Invalid: past
    /// `COPIES_A_SECOND` of one second, or through a symlinked folder, none
    /// is saved, and nothing is written. Method: `save_copy` at one fixed
    /// second, over a copy someone is editing.
    #[test]
    fn a_conflict_copy_never_replaces_a_file() {
        let dir = tmpdir("copy-names");
        let at = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        fs::write(dir.join("a.conflict-7-deadbeef.md"), "my merge, half done").unwrap();
        assert_eq!(save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v2").unwrap().unwrap(), rp("a.conflict-7-2-deadbeef.md"));
        assert_eq!(save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v3").unwrap().unwrap(), rp("a.conflict-7-3-deadbeef.md"));
        assert_eq!((at("a.conflict-7-deadbeef.md"), at("a.conflict-7-2-deadbeef.md"), at("a.conflict-7-3-deadbeef.md")), ("my merge, half done".into(), "v2".into(), "v3".into()));
        assert_eq!(save_copy(&dir, &rp("n/b"), "deadbeef", 7, b"x").unwrap().unwrap(), rp("n/b.conflict-7-deadbeef"), "its folder is made");

        for n in 4..=COPIES_A_SECOND {
            fs::write(dir.join(format!("a.conflict-7-{n}-deadbeef.md")), "taken").unwrap();
        }
        let files = fs::read_dir(&dir).unwrap().count();
        let full = save_copy(&dir, &rp("a.md"), "deadbeef", 7, b"v4").unwrap().unwrap_err();
        assert!(full.contains("this second") && fs::read_dir(&dir).unwrap().count() == files, "{full}");
        #[cfg(unix)]
        {
            let outside = tmpdir("copy-names-outside");
            std::os::unix::fs::symlink(&outside, dir.join("cfg")).unwrap();
            let linked = save_copy(&dir, &rp("cfg/c.md"), "deadbeef", 7, b"x").unwrap().unwrap_err();
            assert!(linked.contains("is a symlink") && fs::read_dir(&outside).unwrap().count() == 0, "{linked}");
            fs::remove_dir_all(&outside).ok();
        }
        fs::remove_dir_all(&dir).ok();
    }

    // ---- the pass's invariant, as a matrix (`pass_over`) ----

    /// What the folder holds at `a.md` when the pass runs, beside `b.md`
    /// (synced, untouched).
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Here {
        Same,
        Edited,
        /// nothing is there
        Deleted,
        /// synced, then `.gitignore`d, its bytes as synced
        Ignored,
        /// synced, then `.gitignore`d and edited
        IgnoredEdited,
        /// the folder's own file, never synced (main had none)
        New,
        NewIgnored,
        /// a folder with no journal holds its own `a.md`
        Own,
        OwnIgnored,
    }

    /// What main holds at `a.md` when the pass runs, against what the
    /// folder last synced (a folder with no journal: the seed).
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum There {
        Same,
        Edited,
        Deleted,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Who {
        Viewer,
        Editor,
        /// synced as a viewer, made an editor before the pass
        Promoted,
    }

    /// One case's fragment and folder, and every (path, bytes) main has
    /// ever held: the bytes git keeps.
    struct World<'a> {
        mock: &'a MockServer,
        name: String,
        dir: PathBuf,
        held: HashSet<(String, Vec<u8>)>,
    }

    impl World<'_> {
        fn main(&self) -> BTreeMap<String, Vec<u8>> {
            self.mock.paths(&self.name, "main").into_iter().filter_map(|p| Some((p.clone(), self.mock.file_at(&self.name, "main", &p)?))).collect()
        }

        fn note_main(&mut self) {
            let main = self.main();
            self.held.extend(main);
        }

        fn commit(&mut self, bytes: Option<&[u8]>) {
            self.mock.external_commit(&self.name, "main", &[("a.md", bytes)], "theirs");
            self.note_main();
        }

        /// Every regular file in the folder but sync's and git's own.
        fn folder(&self) -> BTreeMap<String, Vec<u8>> {
            fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
                for e in fs::read_dir(at).unwrap().flatten() {
                    let (path, kind) = (e.path(), e.file_type().unwrap());
                    let rel = relative(root, &path);
                    if rel == ".fragment" || rel == ".git" {
                        continue;
                    }
                    if kind.is_dir() {
                        walk(root, &path, out);
                    } else if kind.is_file() {
                        out.insert(rel, fs::read(&path).unwrap());
                    }
                }
            }
            let mut out = BTreeMap::new();
            walk(&self.dir, &self.dir, &mut out);
            out
        }

        /// One pass, checked against the invariant: bytes git never held
        /// stay where they were; main loses a path only where the folder
        /// had nothing; what is outside sync is never sent; a pass that
        /// only reads sends nothing. `None`: a viewer's push, refused with
        /// nothing moved.
        fn pass(&mut self, mode: Mode, ignored: bool, case: &str) -> Option<Report> {
            self.note_main();
            let (folder, main) = (self.folder(), self.main());
            let packs = self.mock.requests(&self.name, "POST commit-pack");
            let o = SyncOptions { prune: true, ..opts(mode) };
            let got = sync_once(&client_for(self.mock), &self.name, &self.dir, &o);
            let (folder_after, main_after) = (self.folder(), self.main());
            let sent = self.mock.requests(&self.name, "POST commit-pack") - packs;
            let report = match got {
                Ok(report) => report,
                Err(SyncError::Cs(CsError::Host(e))) if e.code == Code::Forbidden && mode == Mode::Push => {
                    assert!(folder == folder_after && main == main_after && sent == 0, "{case}: a refused push moves nothing");
                    return None;
                }
                Err(e) => panic!("{case}: {e}"),
            };
            self.note_main();
            for (path, bytes) in &folder {
                if !self.held.contains(&(path.clone(), bytes.clone())) {
                    assert_eq!(folder_after.get(path), Some(bytes), "{case}: {path}'s bytes, in no commit, are kept: {report:?}");
                }
            }
            for path in main.keys().filter(|p| !main_after.contains_key(*p)) {
                assert!(!folder.contains_key(path), "{case}: main lost {path}, which the folder still had: {report:?}");
            }
            if ignored {
                if let Some(mine) = folder.get("a.md").filter(|b| b.starts_with(b"mine")) {
                    assert!(main_after.get("a.md") != Some(mine), "{case}: an ignored file was sent: {report:?}");
                }
            }
            if report.read_only {
                assert_eq!(sent, 0, "{case}: a pass that only reads sends nothing");
            }
            Some(report)
        }
    }

    /// Goal: one invariant holds for every pass, whoever runs it, in any
    /// mode, whatever the folder and main did to a file: a pass changes
    /// only what its scan accounted for. Bytes that are in no commit stay
    /// in the folder; main loses a path only where the folder has nothing
    /// at all; a file outside sync (here `.gitignore`d) is never sent, its
    /// journal row is dropped and the pass names it once; a pass that only
    /// reads sends nothing; and a writer's own bytes reach main. Method:
    /// each (folder, main, role, mode) from a synced folder (or one with
    /// no journal), then main changes the file again, then two more
    /// passes, the last of which moves nothing. Replay and restart: each
    /// pass is a fresh `sync_once`, which reads the journal again.
    #[test]
    fn every_pass_changes_only_what_its_scan_accounted_for() {
        let mock = crate::mockcs::start();
        let heres = [Here::Same, Here::Edited, Here::Deleted, Here::Ignored, Here::IgnoredEdited, Here::New, Here::NewIgnored, Here::Own, Here::OwnIgnored];
        let mut cases = Vec::new();
        for here in heres {
            for there in [There::Same, There::Edited, There::Deleted] {
                for who in [Who::Viewer, Who::Editor, Who::Promoted] {
                    for mode in [Mode::Mirror, Mode::Pull, Mode::Push] {
                        let fresh = matches!(here, Here::New | Here::NewIgnored);
                        let own = matches!(here, Here::Own | Here::OwnIgnored);
                        if !(fresh && there == There::Deleted) && !(own && who == Who::Promoted) {
                            cases.push((cases.len(), here, there, who, mode));
                        }
                    }
                }
            }
        }
        assert_eq!(cases.len(), 207);
        // each case its own fragment and folder, eight at a time
        let ran = at_once(&cases, 8, |&(i, here, there, who, mode)| {
            one_case(&mock, i, here, there, who, mode);
            Ok::<(), ()>(())
        });
        assert!(ran.iter().all(|r| matches!(r, Some(Ok(())))));
    }

    /// One case of the matrix above.
    fn one_case(mock: &MockServer, i: usize, here: Here, there: There, who: Who, mode: Mode) {
        let case = format!("{here:?}/{there:?}/{who:?}/{mode:?}");
        let name = format!("m{i}--k3x9");
        let fresh = matches!(here, Here::New | Here::NewIgnored);
        let seed: &[(&str, &[u8])] = if fresh { &[("b.md", b"b")] } else { &[("a.md", b"v1"), ("b.md", b"b")] };
        mock.seed_repo(&name, seed);
        mock.view_only(&name, who != Who::Editor);
        let dir = git_folder(&format!("matrix-{i}"), "nothing.md");
        let mut w = World { mock, name, dir: dir.clone(), held: HashSet::new() };
        w.note_main();
        if !matches!(here, Here::Own | Here::OwnIgnored) {
            sync_once(&client_for(mock), &w.name, &dir, &opts(Mode::Mirror)).unwrap();
        }
        let ignore = || fs::write(dir.join(".gitignore"), "nothing.md\na.md\n").unwrap();
        match here {
            Here::Same => {}
            Here::Edited => fs::write(dir.join("a.md"), "mine: edited").unwrap(),
            Here::Deleted => fs::remove_file(dir.join("a.md")).unwrap(),
            Here::Ignored => ignore(),
            Here::IgnoredEdited | Here::NewIgnored | Here::OwnIgnored => {
                ignore();
                fs::write(dir.join("a.md"), "mine: ignored").unwrap();
            }
            Here::New | Here::Own => fs::write(dir.join("a.md"), "mine: own").unwrap(),
        }
        match there {
            There::Same => {}
            There::Edited => w.commit(Some(b"v2")),
            There::Deleted => w.commit(None),
        }
        if who == Who::Promoted {
            mock.view_only(&w.name, false);
        }
        let ignored = matches!(here, Here::Ignored | Here::IgnoredEdited | Here::NewIgnored | Here::OwnIgnored);
        let Some(first) = w.pass(mode, ignored, &case) else {
            assert!(who == Who::Viewer && mode == Mode::Push, "{case}: only a viewer's push is refused");
            fs::remove_dir_all(&dir).ok();
            return;
        };
        // a file synced, then put outside sync: named once, and the same
        // bytes on both sides are no conflict
        let left = matches!(here, Here::Ignored | Here::IgnoredEdited);
        assert_eq!(first.outside_sync, if left { vec!["a.md".to_string()] } else { vec![] }, "{case}: {first:?}");
        if here == Here::Ignored && there == There::Same {
            assert!(first.conflicts.is_empty() && first.exit_code() == 0 && w.main().get("a.md").map(Vec::as_slice) == Some(b"v1"), "{case}: {first:?}");
        }
        w.commit(Some(b"v3"));
        let second = w.pass(mode, ignored, &case).expect("a pass");
        let third = w.pass(mode, ignored, &case).expect("a pass");
        assert!(second.outside_sync.is_empty() && third.outside_sync.is_empty(), "{case}: named once: {second:?} {third:?}");
        let (folder, main) = (w.folder(), w.main());
        let last = w.pass(mode, ignored, &case).expect("a pass");
        let still = last.pushed.is_empty() && last.pulled.is_empty() && last.deleted_remote.is_empty() && last.deleted_local.is_empty();
        assert!(still && w.folder() == folder && w.main() == main, "{case}: the passes settle: {last:?}");
        // a writer's own bytes reach main (a conflict keeps them, theirs
        // beside, and the next pass sends them)
        let writes = who != Who::Viewer && mode != Mode::Pull;
        if writes && matches!(here, Here::Edited | Here::New | Here::Own) {
            let mine = if here == Here::Edited { "mine: edited" } else { "mine: own" };
            assert!(w.held.contains(&("a.md".to_string(), mine.as_bytes().to_vec())), "{case}: an editor's bytes are sent");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
