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
use fragment_proto::RepoPath;
use serde::Serialize;

use super::layout::Layout;
use super::plan::{self, Counted, Plan, Why};
use super::source::Source;
use super::survey::size;
use crate::api::{Client, Code, CodedError};
use crate::blobs::{could_point, Blobs};
use crate::codestorage::{Author, Change, CsError, Held, Wants, MAIN};

/// The files one commit carries, and the bytes of theirs it carries in
/// git (a blob's are its pointer's).
const BATCH_FILES: usize = 256;
const BATCH_BYTES: usize = 16 * 1024 * 1024;
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
    /// Sent now.
    pub files: usize,
    pub bytes: u64,
    /// There already (an import run before).
    pub already: usize,
    /// What the plan leaves behind, listed in `left_behind`.
    pub left: usize,
    pub left_bytes: u64,
    pub left_behind: &'static str,
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
/// folders first and then its files, the largest first.
pub fn left_behind(plan: &Plan, counted: &Counted) -> String {
    let mut out = String::from("# Left behind\n\nThe old agent's files that were not brought here, and why. They are still where they were.\n\n");
    out.push_str(&format!(
        "Brought: {} files ({}). Left behind: {} files ({}).\n",
        counted.brought.files,
        size(counted.brought.bytes),
        counted.left.files,
        size(counted.left.bytes)
    ));
    let mut listed = 0;
    for (why, r) in &counted.left_by_why {
        let said = why.said();
        out.push_str(&format!("\n## {}{} ({} files, {})\n\n", said[..1].to_uppercase(), &said[1..], r.files, size(r.bytes)));
        let all: Vec<_> = plan.left.iter().filter(|l| l.why == *why).collect();
        for l in &all {
            if listed >= LEFT_LISTED_MAX {
                break;
            }
            listed += 1;
            if l.path.ends_with('/') {
                out.push_str(&format!("- {} ({} files, {})\n", code_span(&l.path), l.files, size(l.bytes)));
            } else {
                out.push_str(&format!("- {} ({})\n", code_span(&l.path), size(l.bytes)));
            }
        }
        let shown = all.len().min(listed);
        if shown < all.len() {
            out.push_str(&format!("- and {} more\n", all.len() - shown));
        }
    }
    out
}

/// The plan alone (`--plan`): what would be brought and left, nothing sent.
pub fn planned(src: &Source, layout: &Layout) -> Result<Counted> {
    let p = plan::plan(src, layout, plan::FILES_MAX_BYTES, plan::FILES_MAX)?;
    Ok(p.counted(plan::FILES_MAX_BYTES, plan::FILES_MAX))
}

/// One commit of `changes` on `head`, as session import makes them: on
/// another head when another writer moved main, once more when its answer
/// was lost. Its new head.
fn commit(c: &Client, held: &mut Held, author: &Author, mut head: Option<String>, message: &str, changes: &[Change]) -> Result<Option<String>> {
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

/// The files the plan brings, into the fragment `into`, and
/// `left-behind.md`.
pub fn bring(c: &Client, src: &Source, layout: &Layout, into: &str) -> Result<Brought> {
    let plan = plan::plan(src, layout, plan::FILES_MAX_BYTES, plan::FILES_MAX)?;
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
    if !there.contains_key(MANIFEST) {
        batch.push(Change::Upsert { path: RepoPath::parse(MANIFEST).expect("a repo path"), bytes: MANIFEST_BYTES.to_vec() });
    }
    let (total, total_bytes) = (plan.picked.len(), plan.picked.iter().map(|p| p.bytes).sum::<u64>());
    let (mut files, mut bytes, mut already) = (0, 0u64, 0);
    for p in &plan.picked {
        if there.get(&p.to).is_some_and(|&held| held == p.bytes || could_point(held, p.bytes)) {
            already += 1;
            continue;
        }
        let read = src.read_max(&p.from, BLOB_MAX_BYTES).with_context(|| format!("reading {}", p.from))?;
        let stored = blobs.store(read)?;
        batch_bytes += stored.len();
        batch.push(Change::Upsert { path: RepoPath::parse(&p.to).map_err(|e| anyhow!("{e}"))?, bytes: stored });
        (files, bytes) = (files + 1, bytes + p.bytes);
        if batch.len() >= BATCH_FILES || batch_bytes >= BATCH_BYTES {
            head = commit(c, &mut held, &author, head, &format!("home files: {} of {total}", files + already), &batch)?;
            eprintln!("{} of {total} files ({} of {})", files + already, size(bytes), size(total_bytes));
            (batch, batch_bytes) = (Vec::new(), 0);
        }
    }
    batch.push(Change::Upsert { path: RepoPath::parse(LEFT_BEHIND).expect("a repo path"), bytes: blobs.store(left_behind(&plan, &counted).into_bytes())? });
    commit(c, &mut held, &author, head, &format!("home files: {total} files, and what stayed behind"), &batch)?;
    crate::sync::refresh_pins(c, into, MAIN);
    Ok(Brought { fragment: into.to_string(), files, bytes, already, left: counted.left.files, left_bytes: counted.left.bytes, left_behind: LEFT_BEHIND })
}

pub fn print(b: &Brought) {
    println!("{}: {} files brought ({}), {} there already; {} left behind ({}), listed in {}", b.fragment, b.files, size(b.bytes), b.already, b.left, size(b.left_bytes), b.left_behind);
}

pub fn print_planned(c: &Counted) {
    println!("Would bring {} files ({}) and leave {} behind ({}); nothing sent", c.brought.files, size(c.brought.bytes), c.left.files, size(c.left.bytes));
    for (why, r) in &c.left_by_why {
        println!("  {} file{} ({}): {}", r.files, if r.files == 1 { "" } else { "s" }, size(r.bytes), why.said());
    }
    if c.left_by_why.contains_key(&Why::OverCap) {
        println!("  The cap brings photos, video, audio and documents first, the newest first.");
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
        let b = bring(&c, &src, &layout(), NAME).unwrap();
        assert_eq!((b.files, b.already, b.left), (2, 0, 2));
        assert_eq!(mock.file_at(NAME, "main", "files/notes/a.md").as_deref(), Some(&b"# a"[..]));
        assert_eq!(mock.file_at(NAME, "main", "files/photos/b.png").as_deref(), Some(&b"png"[..]));
        assert_eq!(mock.file_at(NAME, "main", MANIFEST).as_deref(), Some(MANIFEST_BYTES), "an empty fragment's page is the files view");
        let left = String::from_utf8(mock.file_at(NAME, "main", LEFT_BEHIND).unwrap()).unwrap();
        assert!(left.contains("`app/node_modules/`") && left.contains("`w.gguf`"), "{left}");
        assert!(mock.paths(NAME, "main").iter().all(|p| !p.contains("SOUL")), "the home is no work");
        assert!(mock.refresh_count() >= 1);

        let head = mock.branch(NAME, "main");
        let again = bring(&c, &src, &layout(), NAME).unwrap();
        assert_eq!((again.files, again.already), (0, 2));
        assert_eq!(mock.branch(NAME, "main"), head, "nothing sent, and the same left-behind.md: no commit lands");

        let other = "old-files--b2c3";
        mock.seed_repo(other, &[("fragment.json", br#"{"meta":{"title":"Mine"}}"#), ("files/notes/a.md", b"# a")]);
        let cut = bring(&c, &src, &layout(), other).unwrap();
        assert_eq!((cut.files, cut.already), (1, 1), "what is there already is not sent again");
        assert_eq!(mock.file_at(other, "main", MANIFEST).as_deref(), Some(&br#"{"meta":{"title":"Mine"}}"#[..]), "a manifest of its own is kept");
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
        let e = bring(&c, &src, &layout(), NAME).unwrap_err();
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
        };
        let text = left_behind(&plan, &plan.counted(10, 10));
        assert!(text.contains("Left behind: 1202 files (3.5 GB)"), "{text}");
        assert!(text.contains("## Dependencies or a cache, made again by the tools that made them (1200 files, 300.0 MB)\n\n- `app/node_modules/` (1200 files, 300.0 MB)"), "{text}");
        assert!(text.contains("- `videos/trip.mov` (3.2 GB)\n- `` odd`name.txt `` (10 bytes)"), "{text}");
    }
}
