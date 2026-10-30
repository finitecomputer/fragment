//! `fragment session import`: an agent's past conversations into the
//! person's session archive (templates/sessions/README.md), a fragment of
//! theirs of kind `sessions`, made at their first import. Its format, its
//! limits and its merge are `fragment_core::session`'s; the stores it
//! reads are this module's (`hermes`: Hermes' `state.db`; `chats`: the old
//! Finite's export). It writes as the archive's owner or as their agent
//! acting for them (`FRAGMENT_FOR`), which is an editor of what its owner
//! owns: one commit a batch of conversations, then one for the indexes,
//! each on the head it read (a commit that lost that race reads again).

mod chats;
mod hermes;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use clap::Subcommand;
use fragment_core::session::{self, Entry, Index, Kind, Session};
use fragment_proto::{CreateFragment, Created, FragmentKind, FragmentList, RepoPath, Role};
use serde::Serialize;

use crate::api::{Client, Code, CodedError};
use crate::blobs::Blobs;
use crate::codestorage::{Author, Change, CodeStorage, CsError, MAIN};

#[derive(Subcommand)]
pub enum SessionCmd {
    /// Bring an agent's past conversations into your Session Archive, a
    /// private fragment of yours (made the first time): from Hermes'
    /// state.db, a Hermes home holding one, or the old Finite's export
    /// (its chats/ folder, or the folder holding it). Each conversation is
    /// kept by its own id, so importing again adds nothing twice and only
    /// a newer copy of one replaces it
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

/// What an import did.
#[derive(Debug, Serialize)]
pub struct Imported {
    /// The archive, in full, and whether this import made it.
    pub archive: String,
    pub made: bool,
    pub added: usize,
    pub updated: usize,
    /// Held as they were: the same, or a newer copy held already.
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
    pub id: String,
    pub why: String,
}

/// Where a path a person names reads from.
enum Source {
    Hermes(PathBuf),
    Export(PathBuf),
}

fn source(path: &Path) -> Result<Source> {
    if path.is_file() {
        return Ok(Source::Hermes(path.to_path_buf()));
    }
    if !path.is_dir() {
        bail!(usage(format!("{} is no file or folder", path.display())));
    }
    if let Some(root) = chats::root_of(path) {
        return Ok(Source::Export(root));
    }
    if path.join("state.db").is_file() {
        return Ok(Source::Hermes(path.join("state.db")));
    }
    Err(usage(format!("{} holds no state.db and no export's chats/ (index.json): name Hermes' state.db, a Hermes home, or the old Finite's export", path.display())))
}

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
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
    let read = match source(&path)? {
        Source::Hermes(db) => {
            refuse_own_home(&db)?;
            hermes::read(&db)?
        }
        Source::Export(root) => chats::read(&root)?,
    };
    let mut refused = Vec::new();
    let mut incoming: BTreeMap<String, (Entry, Vec<u8>)> = BTreeMap::new();
    for s in read.sessions.into_iter().filter(|s| kinds.is_empty() || kinds.contains(&s.kind)) {
        let id = s.id.clone();
        match s.normalized().filed() {
            // the same conversation twice in one store: its newer copy
            Ok((e, bytes)) => match incoming.get(&e.key) {
                Some((held, _)) if !e.newer(held) => {}
                _ => {
                    incoming.insert(e.key.clone(), (e, bytes));
                }
            },
            Err(why) => refused.push(RefusedOne { id, why: why.to_string() }),
        }
    }
    let (archive, made) = archive(c)?;
    let written = write(c, &archive, incoming.into_values().collect())?;
    let mut holds: BTreeMap<&'static str, usize> = Kind::ALL.iter().map(|k| (k.as_str(), 0)).collect();
    for e in written.entries.values() {
        *holds.entry(e.kind.as_str()).or_default() += 1;
    }
    Ok(Imported { archive, made, added: written.added, updated: written.updated, unchanged: written.unchanged, refused, problems: read.problems, holds })
}

/// Hermes' own database on this computer is its agent's conversations
/// here, which are chats here already: importing them would show each
/// twice.
fn refuse_own_home(db: &Path) -> Result<()> {
    let Some(home) = std::env::var_os("HERMES_HOME").filter(|h| !h.is_empty()) else { return Ok(()) };
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b);
    if same(db, &Path::new(&home).join("state.db")) {
        return Err(usage(format!("{} is this agent's own state.db: its conversations are your chats here already. Import another agent's (its state.db, or its export)", db.display())));
    }
    Ok(())
}

/// The person's archive: theirs already (the first by name, should two
/// imports at once have made one each), or made now.
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

/// The archive at one head of main: what its indexes list, the keys
/// `sessions/` holds files for, and the head.
struct Held {
    head: Option<String>,
    indexed: BTreeMap<String, Entry>,
    stored: BTreeSet<String>,
}

fn held(storage: &CodeStorage, blobs: &Blobs) -> Result<Held> {
    let head = storage.branch_head(MAIN)?;
    let Some(rev) = head.clone() else { return Ok(Held { head, indexed: BTreeMap::new(), stored: BTreeSet::new() }) };
    let listed = storage.list_files(&rev)?;
    let paths: BTreeSet<&str> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    let stored = paths.iter().filter_map(|p| session::key_of_path(p)).map(str::to_string).collect();
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

/// The entry of each conversation `sessions/` holds and the indexes lack:
/// one this run wrote, as it wrote it; any other read from its file.
fn found(storage: &CodeStorage, blobs: &Blobs, h: &Held, written: &BTreeMap<String, Entry>) -> Result<Vec<Entry>> {
    let rev = h.head.as_deref().unwrap_or(MAIN);
    let mut out = Vec::new();
    for key in h.stored.iter().filter(|k| !h.indexed.contains_key(*k)) {
        if let Some(e) = written.get(key) {
            out.push(e.clone());
            continue;
        }
        let path = session::session_path(key);
        let bytes = blobs.resolve(storage.read_file(&path, rev)?)?;
        match serde_json::from_slice::<Session>(&bytes).map_err(|e| e.to_string()).and_then(|s| s.normalized().filed().map_err(|e| e.to_string())) {
            Ok((e, _)) if e.key == *key => out.push(e),
            Ok((e, _)) => eprintln!("warning: {path} holds conversation {}, filed under another key: left out of the indexes", e.key),
            Err(e) => eprintln!("warning: {path} is no conversation ({e}): left out of the indexes"),
        }
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

fn write(c: &Client, name: &str, incoming: Vec<(Entry, Vec<u8>)>) -> Result<Written> {
    let storage = CodeStorage::connect(c, name)?;
    let blobs = Blobs::new(c, name);
    let author = Author::writer(&c.writer_id());
    let h = held(&storage, &blobs)?;
    let orphans = found(&storage, &blobs, &h, &BTreeMap::new())?;
    let merge = session::merge(&h.indexed, &h.stored, orphans, incoming).map_err(|e| anyhow::Error::new(CodedError { code: Code::TooLarge, msg: e.to_string() }))?;
    let (added, updated, unchanged) = (merge.added, merge.updated, merge.unchanged);
    let mut head = h.head;

    // the conversations, a batch a commit
    let mut batch: Vec<Change> = Vec::new();
    let mut batch_bytes = 0;
    let total = merge.writes.len();
    let mut sent = 0;
    for (key, bytes) in &merge.writes {
        let stored = blobs.store(bytes.clone())?;
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

    // the indexes, made from main as it is when they are written
    let mut entries = merge.entries;
    let mut indexes = merge.indexes;
    let mut moved = !merge.writes.is_empty();
    for attempt in 1..=COMMIT_TRIES {
        if indexes.is_empty() {
            break;
        }
        let mut changes = Vec::new();
        for kind in &indexes {
            let bytes = blobs.store(Index::of(*kind, &entries).file())?;
            changes.push(Change::Upsert { path: RepoPath::parse(&session::index_path(*kind)).map_err(|e| anyhow!("{e}"))?, bytes });
        }
        match storage.commit(head.as_deref(), "session import: the indexes", &author, &changes) {
            Ok(_) => {
                moved = true;
                break;
            }
            // nothing changed (they are as main holds them already)
            Err(CsError::Precondition(_)) => break,
            Err(CsError::CasRejected { .. } | CsError::OutcomeUnknown(_)) if attempt < COMMIT_TRIES => {
                // another writer moved main (or this one's answer was lost):
                // what the indexes list is made again from main as it is now
                let h = held(&storage, &blobs)?;
                let orphans = found(&storage, &blobs, &h, &written)?;
                let again = session::merge(&h.indexed, &h.stored, orphans, Vec::new()).map_err(|e| anyhow!("{e}"))?;
                (head, entries, indexes) = (h.head, again.entries, again.indexes);
            }
            Err(e) => return Err(anyhow::Error::new(e).context("writing the archive's indexes")),
        }
    }
    if moved {
        crate::sync::refresh_pins(c, name);
    }
    Ok(Written { added, updated, unchanged, entries })
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

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_core::session::Message;

    const NAME: &str = "sessions--k3x9";

    fn conversation(id: &str, kind: Kind, at: &str, words: &[&str]) -> (Entry, Vec<u8>) {
        let messages = words.iter().enumerate().map(|(i, w)| Message { role: if i % 2 == 0 { "user" } else { "assistant" }.into(), at: Some(at.into()), text: Some(w.to_string()), ..Default::default() }).collect();
        let s = Session { format: String::new(), id: id.into(), origin: "hermes".into(), kind, title: String::new(), source: None, with: None, model: None, started_at: Some(at.into()), ended_at: None, parts: vec![], messages };
        s.normalized().filed().unwrap()
    }

    fn archive() -> (crate::mockcs::MockServer, Client) {
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[("fragment.json", br#"{"template":"sessions"}"#)]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        (mock, c)
    }

    fn index(mock: &crate::mockcs::MockServer, kind: Kind) -> Vec<String> {
        let bytes = mock.file_at(NAME, "main", &session::index_path(kind)).unwrap_or_default();
        serde_json::from_slice::<Index>(&bytes).map(|i| i.sessions.into_iter().map(|e| e.key).collect()).unwrap_or_default()
    }

    /// Goal: an import writes its conversations and then the indexes that
    /// list them, and the same import again (a replay) writes nothing.
    /// Method: two chats and a run into an empty archive, its files and
    /// commits read back from the code.storage fake; then the same again.
    #[test]
    fn an_import_writes_its_conversations_then_its_indexes_and_again_nothing() {
        let (mock, c) = archive();
        let these = vec![
            conversation("a", Kind::Chat, "2026-10-01T09:00:00Z", &["first", "hi"]),
            conversation("b", Kind::Chat, "2026-10-03T09:00:00Z", &["second"]),
            conversation("r", Kind::Cron, "2026-10-02T06:00:00Z", &["digest"]),
        ];
        let packs = mock.requests(NAME, "POST commit-pack");
        let w = write(&c, NAME, these.clone()).unwrap();
        assert_eq!((w.added, w.updated, w.unchanged, w.entries.len()), (3, 0, 0, 3));
        assert_eq!(mock.requests(NAME, "POST commit-pack") - packs, 2, "one commit of conversations, one of the indexes");
        for (e, bytes) in &these {
            assert_eq!(mock.file_at(NAME, "main", &session::session_path(&e.key)).as_ref(), Some(bytes));
        }
        assert_eq!(index(&mock, Kind::Chat), ["hermes-b", "hermes-a"], "newest first");
        assert_eq!(index(&mock, Kind::Cron), ["hermes-r"]);
        assert!(mock.file_at(NAME, "main", &session::index_path(Kind::Subagent)).is_none(), "no index for a kind it holds none of");
        assert!(mock.refresh_count() >= 1, "the cell is told main moved");

        let packs = mock.requests(NAME, "POST commit-pack");
        let again = write(&c, NAME, these).unwrap();
        assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 3));
        assert_eq!(mock.requests(NAME, "POST commit-pack"), packs, "a replay commits nothing");
    }

    /// Goal: an import after a crash (restart) leaves the indexes as
    /// `sessions/` is. Method: an archive holding a conversation's file its
    /// index lacks (an import stopped before its last commit), and an
    /// index entry whose file is gone; an import of one new conversation.
    #[test]
    fn an_import_after_a_crash_puts_the_indexes_right() {
        let (mock, c) = archive();
        let (orphan, orphan_bytes) = conversation("orphan", Kind::Chat, "2026-10-01T09:00:00Z", &["left"]);
        let (gone, _) = conversation("gone", Kind::Chat, "2026-10-02T09:00:00Z", &["gone"]);
        let stale = Index::of(Kind::Chat, &BTreeMap::from([(gone.key.clone(), gone)])).file();
        let orphan_path = session::session_path(&orphan.key);
        let index_path = session::index_path(Kind::Chat);
        mock.external_commit(NAME, "main", &[(orphan_path.as_str(), Some(&orphan_bytes)), (index_path.as_str(), Some(&stale))], "a crashed import");
        let w = write(&c, NAME, vec![conversation("new", Kind::Chat, "2026-10-03T09:00:00Z", &["new"])]).unwrap();
        assert_eq!((w.added, w.entries.len()), (1, 2));
        assert_eq!(index(&mock, Kind::Chat), ["hermes-new", "hermes-orphan"], "the orphan is listed, the entry without a file dropped");
        let nothing = write(&c, NAME, vec![]).unwrap();
        assert_eq!(nothing.entries.len(), 2);
    }

    /// Goal: a newer copy of a conversation replaces the one held, and an
    /// older copy never does. Method: a chat, then it with a reply more,
    /// then the first copy again.
    #[test]
    fn only_a_newer_copy_replaces_a_conversation() {
        let (mock, c) = archive();
        let first = conversation("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hello"]);
        let longer = conversation("a", Kind::Chat, "2026-10-01T09:00:00Z", &["hello", "hi there"]);
        write(&c, NAME, vec![first.clone()]).unwrap();
        let w = write(&c, NAME, vec![longer.clone()]).unwrap();
        assert_eq!(w.updated, 1);
        let back = write(&c, NAME, vec![first]).unwrap();
        assert_eq!((back.updated, back.unchanged), (0, 1));
        assert_eq!(mock.file_at(NAME, "main", &session::session_path("hermes-a")), Some(longer.1));
    }

    /// Goal: a path names a store, or says what it should be. Invalid: a
    /// folder with neither, a path that is nothing.
    #[test]
    fn a_path_names_a_store_or_says_what_to_name() {
        let dir = hermes::tests::tempdir();
        let e = source(&dir).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("name Hermes' state.db, a Hermes home, or the old Finite's export"), "{e}");
        assert!(source(&dir.join("nothing")).is_err());
        hermes::tests::database(&dir.join("state.db"), "");
        assert!(matches!(source(&dir), Ok(Source::Hermes(p)) if p == dir.join("state.db")));
        chats::tests::export(&dir);
        assert!(matches!(source(&dir), Ok(Source::Export(p)) if p == dir), "an export's chats/ wins over a state.db beside it");
    }
}
