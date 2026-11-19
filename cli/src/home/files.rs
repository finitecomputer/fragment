//! `fragment home files`: an agent's own files brought into a fragment of
//! files, as the plan says (`plan.rs`): each at `files/<its path>`, one
//! of 1 MiB or more as a blob, and `left-behind.md` beside them, listing
//! what stayed behind and why. A fragment with no manifest gets one whose
//! page is the platform's files view.
//!
//! **Resumable.** A file main holds already at its path, the same size (or
//! a blob's pointer for it), is not sent again, so an import cut off goes
//! on where it stopped when run again, and a finished one adds nothing.
//! Commits go in batches, each on the head it read, made again on the new
//! one when another writer moved main meanwhile (the files are upserts).

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use fragment_proto::limits::BLOB_MAX_BYTES;
use fragment_proto::{FragmentList, RepoPath};
use serde::Serialize;

use super::layout::Layout;
use super::plan::{self, Counted, Plan, Why};
use super::source::Source;
use super::survey::size;
use crate::api::{Client, Code, CodedError};
use crate::blobs::{could_point, Blobs};
use fragment_core::blob;
use crate::codestorage::{Author, Change, CsError, Held, Wants, MAIN};

/// The files one commit carries, and the bytes of theirs it carries in
/// git (a blob's are its pointer's).
pub(crate) const BATCH_FILES: usize = 256;
pub(crate) const BATCH_BYTES: usize = 16 * 1024 * 1024;
/// Tries of one commit that lost its head to another writer, or whose
/// answer was lost.
const COMMIT_TRIES: u32 = 5;
/// What stayed behind, beside the files.
pub const LEFT_BEHIND: &str = "left-behind.md";
/// The entries `left-behind.md` lists, at most (past it, how many more).
const LEFT_LISTED_MAX: usize = 10_000;
/// The manifest of a fragment of files: its page is the files view.
const MANIFEST: &str = "fragment.json";
const MANIFEST_BYTES: &[u8] = b"{\n  \"files\": { \"page\": true }\n}\n";

#[derive(Debug, Serialize)]
pub struct Brought {
    pub fragment: String,
    /// The home's files in all, those that may hold a password or key
    /// among them, which nothing reads (`unread`).
    pub home_files: usize,
    pub unread: usize,
    /// Sent now.
    pub files: usize,
    pub bytes: u64,
    /// There already (an import run before).
    pub already: usize,
    /// What the plan leaves behind, listed in `left_behind`.
    pub left: usize,
    pub left_bytes: u64,
    pub left_behind: &'static str,
    /// The rest of the home, kept in storage (`--store`): every file not
    /// brought here, but those unread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored: Option<super::store::Stored>,
}

/// Where the rest of a home is kept (`--store`): the storage fragment, and
/// the folder its notes go in.
pub struct Store<'a> {
    pub into: &'a str,
    pub notes: &'a std::path::Path,
}

fn coded(code: Code, msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code, msg })
}

/// A path in backticks as markdown shows it, whatever it holds.
fn code_span(path: &str) -> String {
    let shown = path.escape_debug().to_string();
    if shown.contains('`') {
        format!("`` {shown} ``")
    } else {
        format!("`{shown}`")
    }
}

/// `left-behind.md`: what the plan leaves, by reason, each reason's
/// folders first and then its files, the largest first; and where they
/// are kept, when they are (`kept_in`, a storage fragment).
pub fn left_behind(plan: &Plan, counted: &Counted, kept_in: Option<&str>) -> String {
    let mut out = match kept_in {
        None => String::from("# Left behind\n\nThe old agent's files that were not brought here, and why. They are still where they were.\n\n"),
        Some(store) => format!(
            "# Kept in storage\n\nThe old agent's files that were not brought here, and why. Each is kept whole in storage, as it was in the old agent's home: in `{}`, or in the next storage fragment once one fills, a note for each saying where. But those whose names say they may hold a password or key were never read, and stay with the old platform.\n\n",
            code_span(store).trim_matches('`')
        ),
    };
    out.push_str(&format!(
        "Brought: {} files ({}). Left behind: {} files ({}).\n",
        counted.brought.files,
        size(counted.brought.bytes),
        counted.left.files,
        size(counted.left.bytes)
    ));
    let mut listed = 0;
    for (why, r) in &counted.left_by_why {
        let said = counted.said(*why);
        out.push_str(&format!("\n## {}{} ({} files, {})\n\n", said[..1].to_uppercase(), &said[1..], r.files, size(r.bytes)));
        let all: Vec<_> = plan.left.iter().filter(|l| l.why == *why).collect();
        let mut shown = 0;
        for l in &all {
            if listed >= LEFT_LISTED_MAX {
                break;
            }
            (listed, shown) = (listed + 1, shown + 1);
            if l.path.ends_with('/') {
                out.push_str(&format!("- {} ({} files, {})\n", code_span(&l.path), l.files, size(l.bytes)));
            } else {
                out.push_str(&format!("- {} ({})\n", code_span(&l.path), size(l.bytes)));
            }
        }
        if shown < all.len() {
            out.push_str(&format!("- and {} more\n", all.len() - shown));
        }
    }
    out
}

/// The plan alone (`--plan`): what would be brought and left, nothing sent
/// and nothing asked, so not knowing the caller's upload limit.
pub fn planned(src: &Source, layout: &Layout) -> Result<Counted> {
    let p = plan::plan(src, layout, plan::FILES_MAX_BYTES, plan::FILES_MAX, None)?;
    Ok(p.counted(plan::FILES_MAX_BYTES, plan::FILES_MAX))
}

/// One commit of `changes` on `head`, as session import makes them: on
/// another head when another writer moved main, once more when its answer
/// was lost. Its new head.
pub(crate) fn commit(c: &Client, held: &mut Held, author: &Author, mut head: Option<String>, message: &str, changes: &[Change]) -> Result<Option<String>> {
    for attempt in 1..=COMMIT_TRIES {
        let storage = held.get(c)?;
        match storage.commit(head.as_deref(), message, author, changes) {
            Ok(tip) => return Ok(Some(tip)),
            // main holds them already (a try whose answer was lost landed)
            Err(CsError::Precondition(_)) => return Ok(storage.branch_head(MAIN)?),
            Err(CsError::CasRejected { .. } | CsError::OutcomeUnknown(_)) if attempt < COMMIT_TRIES => head = storage.branch_head(MAIN)?,
            Err(CsError::Auth(_)) if attempt < COMMIT_TRIES => held.refused(),
            Err(e) => return Err(anyhow::Error::new(e).context("writing the files")),
        }
    }
    unreachable!("the last try returns")
}

/// Whether main holds `p` already: a file of its size in git, or a blob's
/// pointer naming its size (read, not guessed from the pointer's own size:
/// another file's pointer, or a placeholder, is not it).
fn held_already(c: &Client, held: &mut Held, there: &HashMap<String, u64>, head: Option<&str>, p: &plan::Picked) -> Result<bool> {
    Ok(match there.get(&p.to) {
        Some(&n) if n == p.bytes => true,
        Some(&n) if could_point(n, p.bytes) => {
            let pointer = held.get(c)?.read_file(&p.to, head.unwrap_or(MAIN))?;
            blob::parse(&pointer).is_some_and(|ptr| ptr.size == p.bytes)
        }
        _ => false,
    })
}

/// The manifest of a fragment of files, last, in a commit of its own,
/// where main has none: on a head it was read missing from (never with no
/// head: a commit naming none lands on whatever main is), and on one that
/// moved meanwhile only if that one has none either, so another writer's
/// own manifest is kept.
pub(crate) fn manifest(c: &Client, held: &mut Held, author: &Author, mut head: String) -> Result<()> {
    let change = [Change::Upsert { path: RepoPath::parse(MANIFEST).expect("a repo path"), bytes: MANIFEST_BYTES.to_vec() }];
    for attempt in 1..=COMMIT_TRIES {
        let storage = held.get(c)?;
        match storage.read_file(MANIFEST, &head) {
            Ok(_) => return Ok(()),
            Err(CsError::NotFound(_)) => {}
            Err(e) => return Err(anyhow::Error::new(e).context("reading the fragment's manifest")),
        }
        match storage.commit(Some(&head), "home files: a page of its files", author, &change) {
            Ok(_) | Err(CsError::Precondition(_)) => return Ok(()),
            Err(CsError::CasRejected { .. } | CsError::OutcomeUnknown(_)) if attempt < COMMIT_TRIES => {
                head = storage.branch_head(MAIN)?.ok_or_else(|| anyhow!("main has no head"))?;
            }
            Err(CsError::Auth(_)) if attempt < COMMIT_TRIES => held.refused(),
            Err(e) => return Err(anyhow::Error::new(e).context("writing the fragment's manifest")),
        }
    }
    unreachable!("the last try returns")
}

/// What git holds for a picked file: from a stage, a blob's bytes copied
/// there on the platform (never through this computer, which may hold far
/// less than its stage), its pointer; else its bytes as read, a blob's
/// uploaded first.
fn stored(src: &Source, blobs: &Blobs, into: &str, p: &plan::Picked) -> Result<Vec<u8>> {
    if src.stage_id().is_some() && blobs.keeps(p.bytes)? {
        if let Some((offset, len)) = src.span(&p.from)? {
            let (sha, crc) = src.copy(into, offset, len).with_context(|| format!("copying {}", p.from))?;
            // the zip's own check of the file, which reading it made
            let (want, _) = src.sums(&p.from)?.ok_or_else(|| anyhow!("{}: its zip names no CRC-32", p.from))?;
            if crc != want {
                bail!("{} in the stage is not the old file it names (its CRC-32 is {crc:08x}, the zip's {want:08x}): nothing was committed for it", p.from);
            }
            return Ok(blob::pointer(&sha, len).into_bytes());
        }
    }
    let read = src.read_max(&p.from, BLOB_MAX_BYTES).with_context(|| format!("reading {}", p.from))?;
    blobs.store(read)
}

/// The files the plan brings, into the fragment `into`, and
/// `left-behind.md`.
pub fn bring(c: &Client, src: &Source, layout: &Layout, into: &str, store: Option<Store>) -> Result<Brought> {
    // the most one upload of the caller's may be, as its platform says,
    // before anything is written: a file past it stays behind, listed
    let list: FragmentList = c.call_as(c.get("/api/fragments")?)?;
    let upload_max = crate::session::upload_max(&list, c.agent().is_some());
    let plan = plan::plan(src, layout, plan::FILES_MAX_BYTES, plan::FILES_MAX, Some(upload_max))?;
    let counted = plan.counted(plan::FILES_MAX_BYTES, plan::FILES_MAX);
    let mut held = Held::new(into, Wants::Write);
    if !held.get(c)?.writes() {
        bail!(coded(Code::Forbidden, format!("you can read {into} but not write it: bring the files into a fragment you edit")));
    }
    let blobs = Blobs::new(c, into);
    let author = Author::writer(&c.writer_id());
    let mut head = held.get(c)?.branch_head(MAIN)?;
    let there: HashMap<String, u64> = match &head {
        Some(rev) => held.get(c)?.list_files(rev)?.entries.into_iter().map(|e| (e.path.as_str().to_string(), e.size)).collect(),
        None => HashMap::new(),
    };

    let mut batch: Vec<Change> = Vec::new();
    let mut batch_bytes = 0;
    let (total, total_bytes) = (plan.picked.len(), plan.picked.iter().map(|p| p.bytes).sum::<u64>());
    let (mut files, mut bytes, mut already) = (0, 0u64, 0);
    for p in &plan.picked {
        if held_already(c, &mut held, &there, head.as_deref(), p)? {
            already += 1;
            continue;
        }
        let stored = stored(src, &blobs, into, p)?;
        batch_bytes += stored.len();
        batch.push(Change::Upsert { path: RepoPath::parse(&p.to).map_err(|e| anyhow!("{e}"))?, bytes: stored });
        (files, bytes) = (files + 1, bytes + p.bytes);
        if batch.len() >= BATCH_FILES || batch_bytes >= BATCH_BYTES {
            head = commit(c, &mut held, &author, head, &format!("home files: {} of {total}", files + already), &batch)?;
            eprintln!("{} of {total} files ({} of {})", files + already, size(bytes), size(total_bytes));
            (batch, batch_bytes) = (Vec::new(), 0);
        }
    }
    batch.push(Change::Upsert { path: RepoPath::parse(LEFT_BEHIND).expect("a repo path"), bytes: blobs.store(left_behind(&plan, &counted, store.as_ref().map(|s| s.into)).into_bytes())? });
    let head = commit(c, &mut held, &author, head, &format!("home files: {total} files, and what stayed behind"), &batch)?;
    if !there.contains_key(MANIFEST) {
        let head = head.ok_or_else(|| anyhow!("{into}'s main has no head after its files' commit"))?;
        manifest(c, &mut held, &author, head)?;
    }
    crate::sync::refresh_pins(c, into, MAIN);
    // the rest of the home, every file not brought here but those unread
    let stored = match store {
        Some(s) => {
            let picked: std::collections::HashSet<&str> = plan.picked.iter().map(|p| p.from.as_str()).collect();
            let rest: Vec<String> = src.entries().iter().map(|e| e.path.clone()).filter(|p| !picked.contains(p.as_str())).collect();
            let stored = super::store::keep(c, src, rest, s.into, s.notes)?;
            let accounted = stored.files + stored.already + stored.more.map_or(0, |m| m.files) + plan.picked.len();
            assert!(accounted == src.entries().len(), "every file of the home is brought or kept: {accounted} of {}", src.entries().len());
            Some(stored)
        }
        None => None,
    };
    let unread = src.skipped.len();
    Ok(Brought { fragment: into.to_string(), home_files: src.entries().len() + unread, unread, files, bytes, already, left: counted.left.files, left_bytes: counted.left.bytes, left_behind: LEFT_BEHIND, stored })
}

pub fn print(b: &Brought) {
    match &b.stored {
        None => println!("{}: {} files brought ({}), {} there already; {} left behind ({}), listed in {}", b.fragment, b.files, size(b.bytes), b.already, b.left, size(b.left_bytes), b.left_behind),
        Some(s) => {
            println!("{}: {} files brought ({}), {} there already; the rest listed, and why, in {}", b.fragment, b.files, size(b.bytes), b.already, b.left_behind);
            super::store::print(s);
            println!("The home's {} files: {} here, {} in storage, {} unread (their names say they may hold a password or key).", b.home_files, b.files + b.already, s.files + s.already, b.unread);
        }
    }
}

pub fn print_planned(c: &Counted) {
    println!("Would bring {} files ({}) and leave {} behind ({}); nothing sent", c.brought.files, size(c.brought.bytes), c.left.files, size(c.left.bytes));
    for (why, r) in &c.left_by_why {
        println!("  {} file{} ({}): {}", r.files, if r.files == 1 { "" } else { "s" }, size(r.bytes), c.said(*why));
    }
    if c.left_by_why.contains_key(&Why::OverCap) {
        println!("  The cap brings photos, video, audio and documents first, the newest first.");
    }
    if c.upload_max_bytes.is_none() {
        println!("  Planned without your upload limit (read when the files are brought): a file past it stays behind then, listed.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::plan::Left;
    use crate::home::rules::Kind;
    use crate::home::source::tests::{zip_of, Temp};

    const NAME: &str = "old-files--k3x9";

    fn layout() -> Layout {
        Layout { work: Some("work".into()), ..Layout::hermes() }
    }

    /// Goal: the plan's files land under `files/`, with a files page and
    /// `left-behind.md`; the same again sends nothing; an import cut off
    /// goes on where it stopped. Method: a zip into an empty fragment of
    /// the code.storage fake; again; then a fragment holding some already.
    #[test]
    fn the_files_land_once_and_a_cut_import_goes_on() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        zip_of(&zip, &[("work/notes/a.md", b"# a"), ("work/photos/b.png", b"png"), ("work/app/node_modules/x/i.js", b"x"), ("work/w.gguf", b"weights"), ("SOUL.md", b"me")]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let b = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((b.files, b.already, b.left), (2, 0, 2));
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
        assert_eq!(mock.file_at(NAME, "main", "files/photos/b.png").as_deref(), Some(&b"png"[..]));
        assert_eq!(mock.file_at(NAME, "main", MANIFEST).as_deref(), Some(MANIFEST_BYTES), "an empty fragment's page is the files view");
        let left = String::from_utf8(mock.file_at(NAME, "main", LEFT_BEHIND).unwrap()).unwrap();
        assert!(left.contains("`app/node_modules/`") && left.contains("`w.gguf`"), "{left}");
        assert!(mock.paths(NAME, "main").iter().all(|p| !p.contains("SOUL")), "the home is no work");
        assert!(mock.refresh_count() >= 1);

        let head = mock.branch(NAME, "main");
        let again = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((again.files, again.already), (0, 2));
        assert_eq!(mock.branch(NAME, "main"), head, "nothing sent, and the same left-behind.md: no commit lands");

        let other = "old-files--b2c3";
        mock.seed_repo(other, &[("fragment.json", br#"{"meta":{"title":"Mine"}}"#), ("files/notes/a.md", b"# a")]);
        let cut = bring(&c, &src, &layout(), other, None).unwrap();
        assert_eq!((cut.files, cut.already), (1, 1), "what is there already is not sent again");
        assert_eq!(mock.file_at(other, "main", MANIFEST).as_deref(), Some(&br#"{"meta":{"title":"Mine"}}"#[..]), "a manifest of its own is kept");
    }

    /// Goal (2026-11-18: a move's import must never bring its stage through
    /// its computer): from a move's stage, a file kept as a blob is copied
    /// on the platform, stage to blobs, never read here; a small one is
    /// read, as git keeps it; each lands whole. Again, nothing is copied or
    /// read twice. Method: a stage in the code.storage fake, whose copy
    /// route stores a range in a fragment's blobs, counting what its zip
    /// answered and what it copied.
    #[test]
    fn a_stages_large_files_are_copied_on_the_platform_never_read_here() {
        let t = Temp::new();
        let video: Vec<u8> = (0..5 * 1024 * 1024u32).map(|i| (i % 249) as u8).collect();
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, &[("work/notes/a.md".into(), b"# a".to_vec()), ("work/videos/trip.mp4".into(), video.clone())]).unwrap();
        let mock = crate::mockcs::start();
        mock.keep_blobs();
        mock.seed_repo(NAME, &[]);
        mock.seed_stage("old1.p1.1", std::fs::read(&zip).unwrap());
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let src = super::super::open_from(Some(&c), std::path::Path::new("stage:old1.p1.1"), &layout()).unwrap();
        let b = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((b.files, b.already), (2, 0));
        let pointer = blob::parse(&mock.file_at(NAME, "main", "files/videos/trip.mp4").unwrap()).expect("the video is a blob's pointer");
        assert_eq!((pointer.sha256.as_str(), pointer.size), (blob::sha256_hex(&video).as_str(), video.len() as u64));
        assert_eq!(mock.blob(NAME, &pointer.sha256), Some(video.clone()), "its bytes in the fragment's blobs");
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
        let (copies, copied) = mock.stage_copies();
        let (_, read) = mock.stage_reads();
        assert_eq!((copies, copied), (1, video.len() as u64), "the video copied on the platform, once");
        assert!(read < video.len() as u64, "and never read here: {read} bytes read");
        let again = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((again.files, again.already, mock.stage_copies().0), (0, 2, 1), "again: nothing copied twice");
    }

    /// Goal (Codex on #1388): a stage's file whose bytes are not those its
    /// zip's CRC-32 names (damaged, its directory whole) is never taken for
    /// the old file: a copy on the platform answers its CRC-32, checked as
    /// reading the file checks it, and nothing is committed for it.
    #[test]
    fn a_damaged_file_in_a_stage_is_never_taken_for_the_old_one() {
        let t = Temp::new();
        let video: Vec<u8> = (0..5 * 1024 * 1024u32).map(|i| (i % 247) as u8).collect();
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, &[("work/videos/trip.mp4".into(), video)]).unwrap();
        let mut bytes = std::fs::read(&zip).unwrap();
        let start = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap().by_index_raw(0).unwrap().data_start().unwrap();
        bytes[start as usize + 100] ^= 1;
        let mock = crate::mockcs::start();
        mock.keep_blobs();
        mock.seed_repo(NAME, &[]);
        mock.seed_stage("old1.p1.1", bytes);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let src = super::super::open_from(Some(&c), std::path::Path::new("stage:old1.p1.1"), &layout()).unwrap();
        let e = bring(&c, &src, &layout(), NAME, None).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("is not the old file it names"), "{e}");
        assert!(mock.file_at(NAME, "main", "files/videos/trip.mp4").is_none(), "nothing committed for it");
    }

    /// Goal: a file of 1 MiB or more counts as there already only when a
    /// pointer at its path names its size. Invalid: another file's pointer,
    /// a placeholder the size of one.
    #[test]
    fn a_pointer_must_name_the_file() {
        let mock = crate::mockcs::start();
        let mib = 1024 * 1024u64;
        let pointer = |size: u64| fragment_core::blob::pointer(&"a".repeat(64), size).into_bytes();
        let (two, three) = (pointer(2 * mib), pointer(3 * mib));
        mock.seed_repo(NAME, &[("files/two.jpg", &two), ("files/three.jpg", &three), ("files/stub.jpg", b"x"), ("files/small.md", b"abc")]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let mut held = Held::new(NAME, Wants::Write);
        let head = held.get(&c).unwrap().branch_head(MAIN).unwrap();
        let listed = held.get(&c).unwrap().list_files(head.as_deref().unwrap()).unwrap();
        let there: HashMap<String, u64> = listed.entries.into_iter().map(|e| (e.path.as_str().to_string(), e.size)).collect();
        let picked = |to: &str, bytes: u64| plan::Picked { from: String::new(), to: to.into(), bytes, kind: crate::home::rules::Kind::Photo };
        let is = |p: plan::Picked, held: &mut Held| held_already(&c, held, &there, head.as_deref(), &p).unwrap();
        assert!(is(picked("files/three.jpg", 3 * mib), &mut held), "its pointer names its size");
        assert!(!is(picked("files/two.jpg", 3 * mib), &mut held), "a pointer to a 2 MiB file is not a 3 MiB one's");
        assert!(!is(picked("files/stub.jpg", 3 * mib), &mut held), "a placeholder is no pointer");
        assert!(is(picked("files/small.md", 3), &mut held) && !is(picked("files/new.md", 3), &mut held));
    }

    /// Goal: a manifest another writer made while an import ran is kept.
    /// Method: the import's first commit loses its head to a writer of
    /// `fragment.json` (the fake's competitor).
    #[test]
    fn another_writers_manifest_is_kept() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        zip_of(&zip, &[("work/notes/a.md", b"# a")]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[("notes.md", b"theirs")]);
        let theirs = br#"{"meta":{"title":"Theirs"}}"#.to_vec();
        mock.sabotage_commit_packs_with(1, vec![(MANIFEST.to_string(), Some(theirs.clone()))]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let b = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!(b.files, 1);
        assert_eq!(mock.file_at(NAME, "main", MANIFEST), Some(theirs), "the other writer's manifest, not the files page's");
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
    }

    /// Goal: a fragment with no main yet, which another writer gives one
    /// (their own manifest in it) while the import runs, keeps theirs.
    #[test]
    fn a_manifest_made_on_a_new_main_is_kept() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        zip_of(&zip, &[("work/notes/a.md", b"# a")]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        mock.delete_branch(NAME, "main");
        let theirs = br#"{"meta":{"title":"Theirs"}}"#.to_vec();
        mock.land_before_next_commit(vec![(MANIFEST.to_string(), Some(theirs.clone()))]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        assert_eq!(bring(&c, &src, &layout(), NAME, None).unwrap().files, 1);
        assert_eq!(mock.file_at(NAME, "main", MANIFEST), Some(theirs), "theirs, not the files page's");
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
    }

    /// Goal: a zip a person made holding one file that may hold a secret
    /// (an `.npmrc` in a project) imports everything else, and its
    /// `left-behind.md` names that file, and why; the file goes nowhere.
    #[test]
    fn a_zip_with_one_secret_imports_the_rest() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        zip_of(&zip, &[("work/notes/a.md", b"# a"), ("work/proj/.npmrc", b"//registry.npmjs.org/:_authToken=x"), ("work/proj/main.py", b"print(1)")]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let b = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((b.files, b.left), (2, 1));
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
        assert_eq!(mock.file_at(NAME, "main", "files/proj/main.py").as_deref(), Some(&b"print(1)"[..]));
        assert!(mock.paths(NAME, "main").iter().all(|p| !p.contains(".npmrc")), "the secret went nowhere");
        let left = String::from_utf8(mock.file_at(NAME, "main", LEFT_BEHIND).unwrap()).unwrap();
        assert!(left.contains("may hold a password or key") && left.contains("`proj/.npmrc`"), "{left}");
    }

    /// Goal: a file past the caller's upload limit, as the platform
    /// advertises it, stays behind, listed with why, and the rest land; run
    /// again, nothing is sent and it is still listed; under a larger limit
    /// it is brought. Invalid before: planned against a blob's most, its
    /// upload was refused (413) and stopped the whole import, every run.
    #[test]
    fn a_file_past_the_upload_limit_stays_behind_and_the_rest_land() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        // small, so each goes in git (the fake keeps no blobs): the rule is
        // the plan's, whatever the file's size
        let (clip, photo) = (vec![b'v'; 3_000], vec![b'p'; 2_000]);
        zip_of(&zip, &[("work/notes/a.md", b"# a"), ("work/videos/clip.mp4", &clip[..]), ("work/photos/p.png", &photo[..])]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        mock.advertise_upload_max(Some(2_000));
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let b = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((b.files, b.left), (2, 1));
        assert!(mock.file_at(NAME, "main", "files/videos/clip.mp4").is_none());
        assert!(mock.file_at(NAME, "main", "files/photos/p.png").is_some() && mock.file_at(NAME, "main", "files/notes/a.md").is_some());
        let left = String::from_utf8(mock.file_at(NAME, "main", LEFT_BEHIND).unwrap()).unwrap();
        assert!(left.contains("## Larger than one upload here may be (2.0 KB) (1 files, 3.0 KB)\n\n- `videos/clip.mp4` (3.0 KB)"), "{left}");

        let head = mock.branch(NAME, "main");
        let again = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((again.files, again.already, again.left), (0, 2, 1), "a rerun stops at nothing");
        assert_eq!(mock.branch(NAME, "main"), head);

        mock.advertise_upload_max(Some(4_000));
        let more = bring(&c, &src, &layout(), NAME, None).unwrap();
        assert_eq!((more.files, more.already, more.left), (1, 2, 0), "a larger limit brings it");
        assert_eq!(mock.file_at(NAME, "main", "files/videos/clip.mp4").map(|b| b.len()), Some(3_000));
    }

    /// A fragment its caller only views is refused before anything is sent.
    #[test]
    fn a_fragment_only_viewed_is_refused() {
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        zip_of(&zip, &[("work/a.md", b"a")]);
        let src = super::super::open(&zip, &layout()).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        mock.view_only(NAME, true);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let e = bring(&c, &src, &layout(), NAME, None).unwrap_err();
        assert!(format!("{e:#}").contains("editor"), "{e:#}");
        assert!(mock.paths(NAME, "main").is_empty());
    }

    #[test]
    fn what_stayed_behind_listed_by_reason() {
        let plan = Plan {
            root: Some("work".into()),
            picked: Vec::new(),
            left: vec![
                Left { path: "app/node_modules/".into(), files: 1200, bytes: 300_000_000, why: Why::Dependencies, kind: None },
                Left { path: "videos/trip.mov".into(), files: 1, bytes: 3_200_000_000, why: Why::OverCap, kind: Some(Kind::Video) },
                Left { path: "odd`name.txt".into(), files: 1, bytes: 10, why: Why::OverCap, kind: Some(Kind::Document) },
            ],
            upload_max_bytes: Some(100_000_000),
        };
        let text = left_behind(&plan, &plan.counted(10, 10), None);
        assert!(text.contains("Left behind: 1202 files (3.5 GB)"), "{text}");
        assert!(text.contains("## Dependencies or a cache, made again by the tools that made them (1200 files, 300.0 MB)\n\n- `app/node_modules/` (1200 files, 300.0 MB)"), "{text}");
        assert!(text.contains("- `videos/trip.mov` (3.2 GB)\n- `` odd`name.txt `` (10 bytes)"), "{text}");
    }
}
