//! `fragment session import`: an agent's past conversations into the
//! person's session archive (templates/sessions/README.md), a fragment of
//! theirs of kind `sessions`, made at their first import. Its format, its
//! limits and its merge are `fragment_core::session`'s; the stores it
//! reads are this module's (`chats`: the old Finite's export; `hermes`:
//! Hermes' own `state.db`). It writes as
//! the archive's owner or as their agent acting for them (`FRAGMENT_FOR`),
//! which is an editor of what its owner owns: one commit a batch of
//! conversations, then one for the indexes, each on the head it read (a
//! commit that lost that race is made again on the new one).
//!
//! **Bounded as it reads.** Each conversation is filed as it is read, and
//! its file spooled to a folder of the import's own (`Spool`), so an import
//! holds entries in memory, never every conversation's bytes; one that
//! passes an archive's limits stops there, refused, before an archive is
//! made or anything written.

mod chats;
mod hermes;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use fragment_core::blob;
use fragment_core::session::{self, Entry, Index, Kind, Session};
use fragment_proto::{CreateFragment, Created, FragmentKind, FragmentList, RepoPath, Role};
use serde::Serialize;

use crate::api::{Client, Code, CodedError};
use crate::blobs::Blobs;
use crate::codestorage::{Author, Change, CodeStorage, CsError, MAIN};

#[derive(Subcommand)]
pub enum SessionCmd {
    /// Bring an agent's past conversations into your Session Archive, a
    /// private fragment of yours (made the first time): from the old
    /// Finite's export (its chats/ folder, or the folder holding it), or
    /// Hermes' own state.db (or a Hermes home holding one). Each
    /// conversation is kept by its own id, so importing again adds nothing
    /// twice and only a newer copy of one replaces it
    Import {
        path: PathBuf,
        /// Only conversations of these kinds: chat, cron (a scheduled
        /// job's runs), subagent (side tasks). All of them unless named
        #[arg(long = "kind", value_name = "KIND")]
        kinds: Vec<String>,
    },
}

/// Conversations one commit carries, and the bytes of theirs it carries in
/// git (a file of 1 MiB or more goes as a blob, its pointer in the commit).
const BATCH_FILES: usize = 256;
const BATCH_BYTES: usize = 16 * 1024 * 1024;
/// Tries of one commit that lost its head to another writer, or whose
/// answer was lost.
const COMMIT_TRIES: u32 = 5;
/// Conversations' files read at once when the indexes are made again from
/// `sessions/` (after an import that stopped before its indexes).
const READERS: usize = 8;
/// Commits of main read back from its head for the indexes' last one
/// (`held`): an import makes at most this many commits of conversations
/// before its indexes', so one cut off before them is within it. Past it
/// (other writers' commits too), every file is read.
const COMMITS_WALKED: u64 = 100;
const _: () = assert!((session::SESSIONS_MAX / BATCH_FILES) as u64 + 2 <= COMMITS_WALKED);
/// An id said in a refusal, at most (it is refused for being odd: escaped).
const SAID_ID_MAX_CHARS: usize = 80;

/// What an import did.
#[derive(Debug, Serialize)]
pub struct Imported {
    /// The archive, in full, and whether this import made it.
    pub archive: String,
    pub made: bool,
    pub added: usize,
    pub updated: usize,
    /// Held as they were: the same, or a copy as new held already.
    pub unchanged: usize,
    /// Conversations not imported, each with why (the rest were).
    pub refused: Vec<RefusedOne>,
    /// What could not be read at all, each in a sentence.
    pub problems: Vec<String>,
    /// What the archive holds now, by kind.
    pub holds: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Serialize)]
pub struct RefusedOne {
    /// Its id, escaped and cut (`said_id`).
    pub id: String,
    pub why: String,
}

/// An id as a refusal says it: control characters escaped, at most
/// `SAID_ID_MAX_CHARS`, so an id refused for being odd prints as text.
pub(crate) fn said_id(id: &str) -> String {
    let mut out: String = id.chars().take(SAID_ID_MAX_CHARS).collect::<String>().escape_debug().to_string();
    if id.chars().count() > SAID_ID_MAX_CHARS {
        out.push('…');
    }
    out
}

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
}

fn too_much(e: session::TooMuch) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::TooLarge, msg: e.to_string() })
}

/// The conversations an import read, filed: each one's file spooled to a
/// folder of the import's own, its entry kept, within an archive's limits
/// as they are read (`add`). The folder goes with it.
pub struct Spool {
    dir: PathBuf,
    entries: BTreeMap<String, Entry>,
    bytes: u64,
    kinds: BTreeSet<Kind>,
    refused: Vec<RefusedOne>,
}

impl Spool {
    fn new(kinds: BTreeSet<Kind>) -> Result<Spool> {
        clear_stale_spools(&std::env::temp_dir());
        let dir = std::env::temp_dir().join(format!("{SPOOL_PREFIX}{}-{:016x}", std::process::id(), rand::random::<u64>()));
        std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
        Ok(Spool { dir, entries: BTreeMap::new(), bytes: 0, kinds, refused: Vec::new() })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// One conversation read: filed and spooled (if of the kinds asked; one
    /// the format refuses is said, and the rest go on), the newer of two
    /// copies of one kept. An import past an archive's limits stops here.
    pub fn add(&mut self, s: Session) -> Result<()> {
        if !self.kinds.is_empty() && !self.kinds.contains(&s.kind) {
            return Ok(());
        }
        let id = s.id.clone();
        let (entry, bytes) = match s.normalized().filed() {
            Ok(filed) => filed,
            Err(why) => {
                self.refused.push(RefusedOne { id: said_id(&id), why: why.to_string() });
                return Ok(());
            }
        };
        let replaced = match self.entries.get(&entry.key) {
            Some(held) if !entry.newer(held) => return Ok(()),
            Some(held) => held.bytes,
            None => 0,
        };
        let count = self.entries.len() + usize::from(replaced == 0 && !self.entries.contains_key(&entry.key));
        session::within(count, self.bytes - replaced + entry.bytes).map_err(too_much)?;
        std::fs::write(self.path(&entry.key), &bytes).with_context(|| format!("spooling {}", entry.key))?;
        self.bytes = self.bytes - replaced + entry.bytes;
        self.entries.insert(entry.key.clone(), entry);
        Ok(())
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const SPOOL_PREFIX: &str = "fragment-session-import-";
/// How long a spool is left before another import clears it: one an
/// import stopped short of dropping (killed, or interrupted). No import
/// runs this long.
const SPOOL_STALE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Clears the spools under `tmp` imports left past `SPOOL_STALE`, as well
/// as it can: a spool it cannot clear is left for the next.
fn clear_stale_spools(tmp: &Path) {
    let Ok(dir) = std::fs::read_dir(tmp) else { return };
    for e in dir.flatten() {
        let stale = e.file_name().to_str().is_some_and(|n| n.starts_with(SPOOL_PREFIX))
            && e.metadata().is_ok_and(|m| m.is_dir() && m.modified().is_ok_and(|t| t.elapsed().is_ok_and(|age| age > SPOOL_STALE)));
        if stale {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// `fragment session import`.
pub fn run(c: &Client, sub: SessionCmd) -> Result<Imported> {
    let SessionCmd::Import { path, kinds } = sub;
    let kinds: BTreeSet<Kind> = kinds.iter().map(|k| Kind::parse(k).ok_or_else(|| usage(format!("--kind is chat, cron or subagent, not {k:?}")))).collect::<Result<_>>()?;
    if let Some(mode) = c.agent() {
        if mode.acting_for.is_none() {
            return Err(usage(format!("a session archive is a person's: {} imports into its owner's, acting for them (FRAGMENT_FOR)", mode.agent)));
        }
    }
    if !path.exists() {
        bail!(usage(format!("{} is no file or folder", path.display())));
    }
    let mut spool = Spool::new(kinds)?;
    let problems = match source(&path)? {
        Source::Export(root) => chats::read(&root, &mut spool)?,
        Source::Hermes(db) => {
            // on a platform computer (as its agent), its own Hermes home's
            // databases are its chats here already; a person's own Hermes is theirs
            let own = c.agent().and(std::env::var_os("HERMES_HOME")).filter(|h| !h.is_empty()).map(PathBuf::from);
            hermes::read(&db, &mut spool, own.as_deref())?
        }
    };
    let (archive, made) = archive(c)?;
    let written = write(c, &archive, &spool)?;
    let mut holds: BTreeMap<&'static str, usize> = Kind::ALL.iter().map(|k| (k.as_str(), 0)).collect();
    for e in written.entries.values() {
        *holds.entry(e.kind.as_str()).or_default() += 1;
    }
    let refused = std::mem::take(&mut spool.refused);
    Ok(Imported { archive, made, added: written.added, updated: written.updated, unchanged: written.unchanged, refused, problems, holds })
}

/// Where a path a person names reads from.
enum Source {
    Export(PathBuf),
    Hermes(PathBuf),
}

/// The old Finite's export (its `chats/`, or the folder holding it), else
/// Hermes' database (a file, or a Hermes home holding `state.db`).
fn source(path: &std::path::Path) -> Result<Source> {
    if let Some(root) = chats::root_of(path) {
        return Ok(Source::Export(root));
    }
    if path.is_file() {
        return Ok(Source::Hermes(path.to_path_buf()));
    }
    if path.join("state.db").is_file() {
        return Ok(Source::Hermes(path.join("state.db")));
    }
    Err(usage(format!("{} holds no export's chats/ (its index.json) and no state.db: name the old Finite's export, Hermes' state.db, or a Hermes home", path.display())))
}

/// The person's archive: theirs already (the first by name, should two
/// imports at once have made one each), or made now. Asked only once what
/// was read is within an archive's limits (`Spool::add`), so a refused
/// import makes none.
fn archive(c: &Client) -> Result<(String, bool)> {
    let list: FragmentList = c.call_as(c.get("/api/fragments")?)?;
    let theirs = list.fragments.iter().filter(|f| f.kind == FragmentKind::Sessions && (f.role == Role::Owner || f.owned)).map(|f| f.name.clone()).min();
    if let Some(name) = theirs {
        return Ok((name, false));
    }
    let body = CreateFragment {
        name: String::new(),
        label: Some(session::LABEL.to_string()),
        visibility: None,
        template: Some(session::TEMPLATE.to_string()),
        title: Some(session::TITLE.to_string()),
    };
    let made: Created = c.call_as(c.post_json("/api/fragments", &body)?)?;
    Ok((made.name, true))
}

/// The archive at one head of main: what its indexes list, and each file
/// `sessions/` holds, by key.
struct Held {
    head: Option<String>,
    indexed: BTreeMap<String, Entry>,
    stored: BTreeMap<String, Stored>,
}

struct Stored {
    /// Its size in git (a blob's pointer's).
    size: u64,
    /// Changed in a commit after the indexes were last written (or that
    /// commit is past `COMMITS_WALKED`): an import cut off before them, or
    /// another writer, changed it since, so they may not describe it.
    since: bool,
}

impl Held {
    fn keys(&self) -> BTreeSet<String> {
        self.stored.keys().cloned().collect()
    }
}

fn held(storage: &CodeStorage, blobs: &Blobs) -> Result<Held> {
    let head = storage.branch_head(MAIN)?;
    let Some(rev) = head.clone() else { return Ok(Held { head, indexed: BTreeMap::new(), stored: BTreeMap::new() }) };
    let listed = storage.list_files(&rev)?;
    let paths: BTreeSet<&str> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    // The indexes' last commit was made after its import read every file of
    // main as it was then: a file last changed no later is one they
    // describe, and one changed in a commit after it may be any copy (its
    // size the same or not), so it is read. A file's last commit, with its
    // path, names its bytes (code.storage lists it).
    let index_paths: BTreeSet<String> = Kind::ALL.iter().map(|k| session::index_path(*k)).collect();
    let index_commits: BTreeSet<&str> = listed.entries.iter().filter(|e| index_paths.contains(e.path.as_str())).map(|e| e.last_commit_sha.as_str()).collect();
    let since: Option<BTreeSet<String>> = if index_commits.is_empty() {
        Some(BTreeSet::new())
    } else {
        let commits = storage.list_commits(&rev, COMMITS_WALKED)?;
        commits.iter().position(|c| index_commits.contains(c.sha.as_str())).map(|at| commits[..at].iter().map(|c| c.sha.clone()).collect())
    };
    let stored = listed
        .entries
        .iter()
        .filter_map(|e| {
            let since = since.as_ref().is_none_or(|s| s.contains(&e.last_commit_sha));
            session::key_of_path(e.path.as_str()).map(|k| (k.to_string(), Stored { size: e.size, since }))
        })
        .collect();
    let mut indexed = BTreeMap::new();
    for kind in Kind::ALL {
        let path = session::index_path(kind);
        if !paths.contains(path.as_str()) {
            continue;
        }
        let bytes = blobs.resolve(storage.read_file(&path, &rev)?)?;
        // an index that is not one (an agent edited it): made again from
        // the conversations, as an index that is missing is
        let Ok(index) = serde_json::from_slice::<Index>(&bytes) else {
            eprintln!("warning: {path} is no index of the archive's: it is made again from sessions/");
            continue;
        };
        indexed.extend(index.sessions.into_iter().filter(|e| e.kind == kind).map(|e| (e.key.clone(), e)));
    }
    Ok(Held { head, indexed, stored })
}

/// The entry of each conversation whose file in `sessions/` the indexes
/// may not describe: one they lack, one changed since they were last
/// written (`Stored::since`: an import that stopped before its indexes,
/// whatever its size), or one they name another copy than (its size in git
/// is not their entry's, or, for a blob, its pointer names other bytes).
/// One this run wrote is its entry as written, whatever the indexes say
/// (the indexes' commit lost its race, or its answer); any other is read
/// from its file, `READERS` at once.
fn found(c: &Client, name: &str, storage: &CodeStorage, h: &Held, written: &BTreeMap<String, Entry>) -> Result<Vec<Entry>> {
    let rev = h.head.as_deref().unwrap_or(MAIN);
    let mut out: Vec<Entry> = written.values().filter(|e| h.stored.contains_key(&e.key)).cloned().collect();
    // each file its index entry may not describe, and the entry a blob's
    // pointer must name to describe it
    let mut unread: Vec<(&String, Option<&Entry>)> = Vec::new();
    for (key, Stored { size, since }) in &h.stored {
        if written.contains_key(key) {
            continue;
        }
        match h.indexed.get(key) {
            Some(_) if *since => unread.push((key, None)),
            Some(e) if *size == e.bytes => {}
            Some(e) if *size as usize <= blob::POINTER_MAX_BYTES && e.bytes as usize >= blob::BLOB_MIN_BYTES => unread.push((key, Some(e))),
            _ => unread.push((key, None)),
        }
    }
    let read = |keys: &[(&String, Option<&Entry>)]| -> Result<Vec<Entry>> {
        let blobs = Blobs::new(c, name);
        let mut entries = Vec::new();
        for (key, pointed) in keys {
            let path = session::session_path(key);
            let stored = storage.read_file(&path, rev)?;
            if pointed.is_some_and(|e| blob::parse(&stored).is_some_and(|p| p.sha256 == e.sha256 && p.size == e.bytes)) {
                continue;
            }
            let bytes = blobs.resolve(stored)?;
            match serde_json::from_slice::<Session>(&bytes).map_err(|e| e.to_string()).and_then(|s| s.normalized().filed().map_err(|e| e.to_string())) {
                // described as stored, which another CLI's may have written
                // otherwise, so that it is found described next time
                Ok((e, _)) if e.key == **key => entries.push(Entry { sha256: blob::sha256_hex(&bytes), bytes: bytes.len() as u64, ..e }),
                Ok((e, _)) => eprintln!("warning: {path} holds conversation {}, filed under another key: left out of the indexes", said_id(&e.key)),
                Err(e) => eprintln!("warning: {path} is no conversation ({e}): left out of the indexes"),
            }
        }
        Ok(entries)
    };
    let per = unread.len().div_ceil(READERS).max(1);
    let parts: Vec<Result<Vec<Entry>>> = std::thread::scope(|scope| {
        let readers: Vec<_> = unread.chunks(per).map(|keys| scope.spawn(move || read(keys))).collect();
        readers.into_iter().map(|r| r.join().unwrap_or_else(|_| Err(anyhow!("a reader of the archive's files panicked")))).collect()
    });
    for part in parts {
        out.extend(part?);
    }
    Ok(out)
}

/// An import written: its counts, and every entry the archive holds after.
struct Written {
    added: usize,
    updated: usize,
    unchanged: usize,
    entries: BTreeMap<String, Entry>,
}

fn write(c: &Client, name: &str, spool: &Spool) -> Result<Written> {
    let storage = CodeStorage::connect(c, name)?;
    let blobs = Blobs::new(c, name);
    let author = Author::writer(&c.writer_id());
    let h = held(&storage, &blobs)?;
    let orphans = found(c, name, &storage, &h, &BTreeMap::new())?;
    let incoming: Vec<(Entry, PathBuf)> = spool.entries.values().map(|e| (e.clone(), spool.path(&e.key))).collect();
    let merge = session::merge(&h.indexed, &h.keys(), orphans, incoming).map_err(too_much)?;
    let (added, updated, unchanged) = (merge.added, merge.updated, merge.unchanged);
    let mut head = h.head;

    // the conversations, a batch a commit
    let mut batch: Vec<Change> = Vec::new();
    let mut batch_bytes = 0;
    let total = merge.writes.len();
    let mut sent = 0;
    for (key, path) in &merge.writes {
        let stored = blobs.store(std::fs::read(path).with_context(|| format!("reading back {}", path.display()))?)?;
        batch_bytes += stored.len();
        batch.push(Change::Upsert { path: RepoPath::parse(&session::session_path(key)).map_err(|e| anyhow!("{e}"))?, bytes: stored });
        if batch.len() >= BATCH_FILES || batch_bytes >= BATCH_BYTES {
            sent += batch.len();
            head = commit(&storage, &author, head, &format!("session import: {sent} of {total} conversations"), &batch)?;
            (batch, batch_bytes) = (Vec::new(), 0);
        }
    }
    if !batch.is_empty() {
        head = commit(&storage, &author, head, &format!("session import: {total} conversations"), &batch)?;
    }
    let written: BTreeMap<String, Entry> = merge.entries.iter().filter(|(k, _)| merge.writes.contains_key(*k)).map(|(k, e)| (k.clone(), e.clone())).collect();
    let indexes = Indexes { c, name, storage: &storage, author: &author };
    let (entries, indexed) = indexes.write(head, merge.entries, merge.indexes, &written)?;
    if !merge.writes.is_empty() || indexed {
        crate::sync::refresh_pins(c, name);
    }
    Ok(Written { added, updated, unchanged, entries })
}

/// Where the indexes are written, and as whom.
struct Indexes<'a> {
    c: &'a Client,
    name: &'a str,
    storage: &'a CodeStorage,
    author: &'a Author,
}

impl Indexes<'_> {
    /// The indexes of `kinds` over `entries`, in one commit on `head`. One
    /// that loses its race to another writer, or whose answer is lost, is
    /// made again from main as it is then, the conversations this run wrote
    /// (`written`) listed by their entries as written. With every entry the
    /// indexes list, and whether a commit landed.
    fn write(&self, mut head: Option<String>, mut entries: BTreeMap<String, Entry>, mut kinds: Vec<Kind>, written: &BTreeMap<String, Entry>) -> Result<(BTreeMap<String, Entry>, bool)> {
        let blobs = Blobs::new(self.c, self.name);
        for attempt in 1..=COMMIT_TRIES {
            if kinds.is_empty() {
                return Ok((entries, false));
            }
            let mut changes = Vec::new();
            for kind in &kinds {
                let bytes = blobs.store(Index::of(*kind, &entries).file())?;
                changes.push(Change::Upsert { path: RepoPath::parse(&session::index_path(*kind)).map_err(|e| anyhow!("{e}"))?, bytes });
            }
            match self.storage.commit(head.as_deref(), "session import: the indexes", self.author, &changes) {
                Ok(_) => return Ok((entries, true)),
                // nothing changed (they are as main holds them already)
                Err(CsError::Precondition(_)) => return Ok((entries, false)),
                Err(CsError::CasRejected { .. } | CsError::OutcomeUnknown(_)) if attempt < COMMIT_TRIES => {
                    let h = held(self.storage, &blobs)?;
                    let orphans = found(self.c, self.name, self.storage, &h, written)?;
                    let again = session::merge::<()>(&h.indexed, &h.keys(), orphans, Vec::new()).map_err(too_much)?;
                    (head, entries, kinds) = (h.head, again.entries, again.indexes);
                }
                Err(e) => return Err(anyhow::Error::new(e).context("writing the archive's indexes")),
            }
        }
        unreachable!("the last try returns")
    }
}

/// One commit of `changes` on `head`: on another head when another writer
/// moved main meanwhile (the conversations are upserts: any head takes
/// them), and once more when its answer was lost. Its new head.
fn commit(storage: &CodeStorage, author: &Author, mut head: Option<String>, message: &str, changes: &[Change]) -> Result<Option<String>> {
    for attempt in 1..=COMMIT_TRIES {
        match storage.commit(head.as_deref(), message, author, changes) {
            Ok(tip) => return Ok(Some(tip)),
            // main holds them already (a try whose answer was lost landed)
            Err(CsError::Precondition(_)) => return Ok(storage.branch_head(MAIN)?),
            Err(CsError::CasRejected { .. } | CsError::OutcomeUnknown(_)) if attempt < COMMIT_TRIES => head = storage.branch_head(MAIN)?,
            Err(e) => return Err(anyhow::Error::new(e).context("writing the archive's conversations")),
        }
    }
    unreachable!("the last try returns")
}

/// An import as a person reads it.
pub fn print(r: &Imported) {
    let made = if r.made { " (made now)" } else { "" };
    println!("Session Archive {}{made}: {} added, {} updated, {} already there", r.archive, r.added, r.updated, r.unchanged);
    for one in &r.refused {
        println!("  not imported: {} ({})", one.id, one.why);
    }
    for p in &r.problems {
        println!("  could not read: {p}");
    }
    let holds: usize = r.holds.values().sum();
    let noun = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    println!(
        "It holds {}: {}, {}, {}.",
        noun(holds, "conversation", "conversations"),
        noun(r.holds["chat"], "chat", "chats"),
        noun(r.holds["cron"], "scheduled run", "scheduled runs"),
        noun(r.holds["subagent"], "side task", "side tasks")
    );
}

/// A fresh folder in the system's temporary place (a test's).
#[cfg(test)]
pub(crate) fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("fragment-session-{}-{}", std::process::id(), rand::random::<u64>()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_core::session::Message;

    const NAME: &str = "sessions--k3x9";

    fn session(id: &str, kind: Kind, at: &str, words: &[&str]) -> Session {
        let messages = words.iter().enumerate().map(|(i, w)| Message { role: if i % 2 == 0 { "user" } else { "assistant" }.into(), at: Some(at.into()), text: Some(w.to_string()), ..Default::default() }).collect();
        Session { format: String::new(), id: id.into(), origin: "hermes".into(), reader: 10, kind, agent: None, title: String::new(), source: None, with: None, model: None, started_at: Some(at.into()), ended_at: None, parts: vec![], messages }
    }

    fn spooled(sessions: Vec<Session>) -> Spool {
        let mut spool = Spool::new(BTreeSet::new()).unwrap();
        for s in sessions {
            spool.add(s).unwrap();
        }
        spool
    }

    fn file_of(s: Session) -> Vec<u8> {
        s.normalized().filed().unwrap().1
    }

    fn archive() -> (crate::mockcs::MockServer, Client) {
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[("fragment.json", br#"{"template":"sessions"}"#)]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        (mock, c)
    }

    fn index(mock: &crate::mockcs::MockServer, kind: Kind) -> Vec<(String, String)> {
        let bytes = mock.file_at(NAME, "main", &session::index_path(kind)).unwrap_or_default();
        serde_json::from_slice::<Index>(&bytes).map(|i| i.sessions.into_iter().map(|e| (e.key, e.sha256)).collect()).unwrap_or_default()
    }

    fn keys(mock: &crate::mockcs::MockServer, kind: Kind) -> Vec<String> {
        index(mock, kind).into_iter().map(|(k, _)| k).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        fragment_core::blob::sha256_hex(bytes)
    }

    /// Goal: an import writes its conversations and then the indexes that
    /// list them, and the same import again (a replay) writes nothing.
    /// Method: two chats and a run into an empty archive, its files and
    /// commits read back from the code.storage fake; then the same again.
    #[test]
    fn an_import_writes_its_conversations_then_its_indexes_and_again_nothing() {
        let (mock, c) = archive();
        let these = || vec![session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["first", "hi"]), session("b", Kind::Chat, "2026-10-03T09:00:00Z", &["second"]), session("r", Kind::Cron, "2026-10-02T06:00:00Z", &["digest"])];
        let packs = mock.requests(NAME, "POST commit-pack");
        let w = write(&c, NAME, &spooled(these())).unwrap();
        assert_eq!((w.added, w.updated, w.unchanged, w.entries.len()), (3, 0, 0, 3));
        assert_eq!(mock.requests(NAME, "POST commit-pack") - packs, 2, "one commit of conversations, one of the indexes");
        for s in these() {
            let path = session::session_path(&session::key("hermes", &s.id).unwrap());
            assert_eq!(mock.file_at(NAME, "main", &path), Some(file_of(s)));
        }
        assert_eq!(keys(&mock, Kind::Chat), ["hermes-b", "hermes-a"], "newest first");
        assert_eq!(keys(&mock, Kind::Cron), ["hermes-r"]);
        assert!(mock.file_at(NAME, "main", &session::index_path(Kind::Subagent)).is_none(), "no index for a kind it holds none of");
        assert!(mock.refresh_count() >= 1, "the cell is told main moved");

        let packs = mock.requests(NAME, "POST commit-pack");
        let again = write(&c, NAME, &spooled(these())).unwrap();
        assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 3));
        assert_eq!(mock.requests(NAME, "POST commit-pack"), packs, "a replay commits nothing");
    }

    /// Goal: an import after a crash (restart) leaves the indexes as
    /// `sessions/` is, whatever it imports. Method: an archive holding a
    /// conversation's file its index lacks, a newer copy of one under its
    /// older entry (an update whose indexes never landed), and an entry
    /// whose file is gone; an import of something else.
    #[test]
    fn an_import_after_a_crash_puts_the_indexes_right() {
        let (mock, c) = archive();
        write(&c, NAME, &spooled(vec![session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hello"]), session("gone", Kind::Chat, "2026-10-02T09:00:00Z", &["gone"])])).unwrap();
        let orphan = file_of(session("orphan", Kind::Subagent, "2026-10-01T09:00:00Z", &["left"]));
        let newer_a = file_of(session("a", Kind::Chat, "2026-10-04T09:00:00Z", &["hello", "and a longer reply"]));
        let (orphan_path, a_path, gone_path) = (session::session_path("hermes-orphan"), session::session_path("hermes-a"), session::session_path("hermes-gone"));
        mock.external_commit(NAME, "main", &[(orphan_path.as_str(), Some(&orphan)), (a_path.as_str(), Some(&newer_a)), (gone_path.as_str(), None)], "a crashed import");
        let w = write(&c, NAME, &spooled(vec![session("new", Kind::Cron, "2026-10-03T09:00:00Z", &["new"])])).unwrap();
        assert_eq!((w.added, w.entries.len()), (1, 3));
        assert_eq!(index(&mock, Kind::Chat), [("hermes-a".to_string(), sha(&newer_a))], "the newer copy's entry, the gone one dropped");
        assert_eq!(keys(&mock, Kind::Subagent), ["hermes-orphan"]);
        let packs = mock.requests(NAME, "POST commit-pack");
        assert_eq!(write(&c, NAME, &spooled(vec![])).unwrap().entries.len(), 3);
        assert_eq!(mock.requests(NAME, "POST commit-pack"), packs, "then nothing is left to put right");
    }

    /// Goal: an import cut off after its conversations' commit and before
    /// its indexes' leaves nothing a later import trusts, whatever the
    /// copies' sizes: an import of nothing else (a restart, nothing held
    /// from the run before) puts the indexes right, the new conversation
    /// and the updated one, and an older copy imported after never replaces
    /// the newer. Method: through the writer, a chat; then an update of the
    /// same length and a new chat, the indexes' commit beaten every try (the
    /// fake's sabotage); an empty import; then a copy between the two.
    #[test]
    fn an_import_cut_off_before_its_indexes_is_put_right_whatever_the_sizes() {
        let (mock, c) = archive();
        let first = || session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hellp"]);
        let third = || session("a", Kind::Chat, "2026-10-03T09:00:00Z", &["hello"]);
        let second = || session("a", Kind::Chat, "2026-10-02T09:00:00Z", &["hellq"]);
        let new = || session("b", Kind::Chat, "2026-10-03T10:00:00Z", &["new"]);
        let size = |s: Session| file_of(s).len();
        assert!(size(first()) == size(third()) && size(second()) == size(third()), "the same size: only their bytes tell them apart");
        let a_path = session::session_path("hermes-a");
        write(&c, NAME, &spooled(vec![first()])).unwrap();

        mock.sabotage_commit_packs_after(1, COMMIT_TRIES);
        assert!(write(&c, NAME, &spooled(vec![third(), new()])).is_err(), "cut off before its indexes");
        assert_eq!(mock.file_at(NAME, "main", &a_path), Some(file_of(third())), "its conversations landed");
        assert_eq!(index(&mock, Kind::Chat), [("hermes-a".to_string(), sha(&file_of(first())))], "its indexes did not");

        let repaired = write(&c, NAME, &spooled(vec![])).unwrap();
        assert_eq!((repaired.added, repaired.updated, repaired.entries.len()), (0, 0, 2));
        assert_eq!(index(&mock, Kind::Chat), [("hermes-b".to_string(), sha(&file_of(new()))), ("hermes-a".to_string(), sha(&file_of(third())))], "an import of nothing puts them right");

        let between = write(&c, NAME, &spooled(vec![second()])).unwrap();
        assert_eq!((between.updated, between.unchanged), (0, 1), "a copy older than the one held");
        assert_eq!(mock.file_at(NAME, "main", &a_path), Some(file_of(third())), "the newer copy stays");
        assert_eq!(index(&mock, Kind::Chat)[1], ("hermes-a".to_string(), sha(&file_of(third()))));
    }

    /// Goal: when the indexes' last commit is further back than an import
    /// looks, every file is read, so a copy of the same size is still found.
    /// Method: an update of the same length, then more of another writer's
    /// commits than `COMMITS_WALKED`.
    #[test]
    fn past_the_commits_walked_every_file_is_read() {
        let (mock, c) = archive();
        write(&c, NAME, &spooled(vec![session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hellp"])])).unwrap();
        let later = file_of(session("a", Kind::Chat, "2026-10-03T09:00:00Z", &["hello"]));
        mock.external_commit(NAME, "main", &[(session::session_path("hermes-a").as_str(), Some(&later))], "an update cut off before its indexes");
        for i in 0..COMMITS_WALKED {
            mock.external_commit(NAME, "main", &[("notes.md", Some(format!("{i}").as_bytes()))], "another writer");
        }
        write(&c, NAME, &spooled(vec![])).unwrap();
        assert_eq!(index(&mock, Kind::Chat), [("hermes-a".to_string(), sha(&later))]);
    }

    /// Goal: a commit that loses its race, or whose answer is lost, is made
    /// again, and the indexes then list what this import wrote, not what
    /// main's indexes said. Method: the conversations' commit beaten once by
    /// a competitor; then an update whose indexes' commit is beaten (its
    /// copy the same size as the one indexed, so only what it wrote says
    /// it); then one whose indexes' answer is lost.
    #[test]
    fn a_commit_that_loses_its_race_is_made_again() {
        let (mock, c) = archive();
        mock.sabotage_commit_packs(1);
        write(&c, NAME, &spooled(vec![session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hellp"])])).unwrap();
        assert!(mock.file_at(NAME, "main", "competitor.txt").is_some() && keys(&mock, Kind::Chat) == ["hermes-a"], "both land");

        let same_size = session("a", Kind::Chat, "2026-10-02T09:00:00Z", &["hello"]);
        let bytes = file_of(same_size.clone());
        assert_eq!(bytes.len(), file_of(session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hellp"])).len());
        mock.sabotage_commit_packs_after(1, 1);
        let w = write(&c, NAME, &spooled(vec![same_size])).unwrap();
        assert_eq!(w.updated, 1);
        assert_eq!(index(&mock, Kind::Chat), [("hermes-a".to_string(), sha(&bytes))], "the indexes list what was written");

        let repo = mock.repo_url(NAME).unwrap();
        mock.drop_commit_answers(&repo, 2);
        let w = write(&c, NAME, &spooled(vec![session("b", Kind::Chat, "2026-10-03T09:00:00Z", &["b"])])).unwrap();
        assert_eq!(w.added, 1);
        assert_eq!(keys(&mock, Kind::Chat), ["hermes-b", "hermes-a"]);
    }

    /// Goal: a conversation's file is indexed as it is stored, however the
    /// CLI that wrote it laid it out, so the next import reads it no more.
    /// Method: an archive holding a file laid out another way (compact) its
    /// indexes lack; an import; then another, counting the files read.
    #[test]
    fn a_file_is_indexed_as_it_is_stored() {
        let (mock, c) = archive();
        let left = || session("o", Kind::Chat, "2026-10-01T09:00:00Z", &["left"]);
        let compact = serde_json::to_vec(&left().normalized()).unwrap();
        assert_ne!(compact, file_of(left()));
        let path = session::session_path("hermes-o");
        mock.external_commit(NAME, "main", &[(path.as_str(), Some(&compact))], "another CLI's import");
        write(&c, NAME, &spooled(vec![])).unwrap();
        assert_eq!(index(&mock, Kind::Chat), [("hermes-o".to_string(), sha(&compact))]);
        let reads = mock.requests(NAME, "GET file");
        let again = write(&c, NAME, &spooled(vec![left()])).unwrap();
        assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 1));
        assert_eq!(mock.requests(NAME, "GET file") - reads, 1, "the chats' index, and no conversation");
    }

    /// Goal: an import clears the spools imports left behind (killed, or
    /// interrupted) a day on, and no other. Method: a stale spool, a fresh
    /// one, and a stale folder not a spool.
    #[test]
    fn an_import_clears_the_spools_left_behind() {
        let tmp = tempdir();
        let old = std::time::SystemTime::now() - SPOOL_STALE - std::time::Duration::from_secs(60);
        let (stale, fresh, other) = (tmp.join(format!("{SPOOL_PREFIX}1-a")), tmp.join(format!("{SPOOL_PREFIX}2-b")), tmp.join("other"));
        for dir in [&stale, &fresh, &other] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("hermes-a.json"), "{}").unwrap();
        }
        for dir in [&stale, &other] {
            std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
        }
        clear_stale_spools(&tmp);
        assert!(!stale.exists() && fresh.exists() && other.exists());
    }

    /// Goal: a newer copy of a conversation replaces the one held, and an
    /// older copy never does. Method: a chat, then it later with a reply
    /// more, then the first copy again.
    #[test]
    fn only_a_newer_copy_replaces_a_conversation() {
        let (mock, c) = archive();
        let first = || session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hello"]);
        let later = session("a", Kind::Chat, "2026-10-02T09:00:00Z", &["hello", "hi there"]);
        write(&c, NAME, &spooled(vec![first()])).unwrap();
        assert_eq!(write(&c, NAME, &spooled(vec![later.clone()])).unwrap().updated, 1);
        let back = write(&c, NAME, &spooled(vec![first()])).unwrap();
        assert_eq!((back.updated, back.unchanged), (0, 1));
        assert_eq!(mock.file_at(NAME, "main", &session::session_path("hermes-a")), Some(file_of(later)));
    }

    /// Goal: a path names a store, or says what it should be. Invalid: a
    /// folder with neither.
    #[test]
    fn a_path_names_a_store_or_says_what_to_name() {
        let dir = tempdir();
        let e = source(&dir).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("name the old Finite's export, Hermes' state.db, or a Hermes home"), "{e}");
        hermes::tests::database(&dir.join("state.db"), "");
        assert!(matches!(source(&dir), Ok(Source::Hermes(p)) if p == dir.join("state.db")));
        assert!(matches!(source(&dir.join("state.db")), Ok(Source::Hermes(_))));
        chats::tests::export(&dir);
        assert!(matches!(source(&dir), Ok(Source::Export(p)) if p == dir), "an export's chats/ wins over a state.db beside it");
    }

    /// Goal: a spool holds what an import read within an archive's limits,
    /// the newer of two copies of one, refusing the rest as it reads.
    /// Method: two copies of one, an id that is no id (said escaped), and a
    /// kind not asked for.
    #[test]
    fn a_spool_keeps_the_newer_copy_and_says_what_it_refuses() {
        let mut spool = Spool::new(BTreeSet::from([Kind::Chat])).unwrap();
        spool.add(session("a", Kind::Chat, "2026-10-02T09:00:00Z", &["later"])).unwrap();
        spool.add(session("a", Kind::Chat, "2026-10-01T09:00:00Z", &["earlier"])).unwrap();
        spool.add(session("run", Kind::Cron, "2026-10-01T09:00:00Z", &["not asked"])).unwrap();
        spool.add(session(&format!("\u{1b}[31m{}", "x".repeat(300)), Kind::Chat, "2026-10-01T09:00:00Z", &["odd"])).unwrap();
        assert_eq!(spool.entries.keys().collect::<Vec<_>>(), ["hermes-a"]);
        assert_eq!(std::fs::read(spool.path("hermes-a")).unwrap(), file_of(session("a", Kind::Chat, "2026-10-02T09:00:00Z", &["later"])));
        assert_eq!(spool.refused.len(), 1);
        let said = &spool.refused[0].id;
        assert!(said.starts_with("\\u{1b}[31m") && said.ends_with('…') && said.chars().count() < 100, "{said}");
        let dir = spool.dir.clone();
        drop(spool);
        assert!(!dir.exists(), "its folder goes with it");
    }
}
