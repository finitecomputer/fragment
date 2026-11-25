//! `fragment home files --store`: nothing left behind. Every file of an
//! agent's home that its files' pick does not bring (files.rs), but those
//! never read (a name that may hold a password or key), is kept whole in a
//! storage fragment the way `fragment offload` keeps what it moves
//! (offload.rs): under `home/<its path>`, a file past one upload in parts,
//! and a note where it was, under `--notes`, so `fragment offload --back
//! <note>` brings it back, checked against the zip's own CRC-32 of it.
//! Paul (2026-11-19): a move always carries all the data, never less.
//!
//! **Never through this disk.** From a move's stage, each part of a blob's
//! size is copied on the platform, stage to blobs (`Source::copy`); a
//! smaller one is read by range into git. An old agent's home can be many
//! times this computer's disk (2026-11-18: a 47 GB import failed on 20 GB).
//!
//! **A note per bounded group.** A folder whose kept files are at most
//! `NOTE_FILES_MAX` and `NOTE_BYTES_MAX` gets one note, so any note can
//! come back onto a computer with room; past either, its folders and its
//! files each get their own (a file alone, whatever its size).
//!
//! **Too deep is kept, not refused** (FIN-755). A file whose own place,
//! `home/<its path>`, would be past a repo path's length (an old export's
//! run logs, folders deep) is a group alone, kept under a short place of
//! its own (`LONG`), or a shorter one still for a long name. Its note is
//! still where it was, and names that place; a name too long for its
//! note's own is named short (`offload::note_name`).
//!
//! **Proven before its note.** A group's parts are committed, then found
//! at main, each the size it was written, and each blob in the store. Only
//! then is its note written (whole, never over another: offload's
//! `publish`). Cut off, a run goes on: the notes say what is kept (and a
//! file brought back beside its note's place, what was), and a group with
//! no note yet is made again: the same paths, the same blobs.
//!
//! **Never over what is kept.** Each notes folder is its own place in
//! storage, named as it is (`~/old-agent/home`'s files under `home/`): a
//! file the notes name counts as kept only while its size and CRC-32 are
//! the home's, and a home whose files differ from what its notes keep (an
//! old agent that ran again before another move) is refused before
//! anything is written, to be kept beside, with a new notes folder. Every
//! path a run writes is checked first against every other, and against
//! what the notes keep: a file named as another's part never takes it.
//!
//! **One storage fragment holds at most `STORE_FILES_MAX` files**, what a
//! person's export takes of one. A home with more is kept in several: a run
//! stops before the first note past it and says what is left, for another
//! (`--store` again, naming the next).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use fragment_core::blob;
use fragment_proto::RepoPath;
use serde::Serialize;

use super::source::Source;
use super::survey::size;
use crate::api::{Client, Code, CodedError};
use crate::blobs::Blobs;
use crate::codestorage::{Author, Change, Held, Wants, MAIN};
use crate::home::files::{commit, manifest, BATCH_BYTES, BATCH_FILES};
use crate::offload::{self, Folder, Moved, Record, Said, NOTE_SUFFIX};

/// The files one note keeps, at most: what `fragment offload` moves at once.
pub const NOTE_FILES_MAX: usize = offload::FILES_MAX;
/// The bytes one folder's note keeps, at most: a third of the room a
/// computer's owner is told it is short of (`ROOM_TOLD_MAX`), so any note
/// comes back where there is room. A file past it has a note alone.
pub const NOTE_BYTES_MAX: u64 = fragment_core::computer::ROOM_TOLD_MAX / 3;
/// The files one storage fragment holds, at most: what a person's export
/// takes of one fragment, less room for its own (manifest, and the like).
pub const STORE_FILES_MAX: usize = fragment_core::export::limits::FILES_MAX - STORE_ROOM;
const STORE_ROOM: usize = 16;
/// Copies on the platform at once.
const COPIES_AT_ONCE: usize = 4;
/// The parts of a group read or copied before they are committed, at most:
/// what a run holds in memory at once (small parts are under a blob's size).
const WINDOW: usize = 16;
const _: () = assert!(NOTE_FILES_MAX as u64 <= offload::PARTS_MAX && STORE_FILES_MAX as u64 <= offload::PARTS_MAX, "a note a store writes is one offload reads");
const _: () = assert!(NOTE_BYTES_MAX > 0 && NOTE_BYTES_MAX < fragment_core::computer::ROOM_TOLD_MAX, "a note comes back where there is room");

/// Where a file too deep for its own place is kept (FIN-755): under
/// `<root>/.long/`, in a folder named by its path's sha256 (the first
/// `LONG_HEX` hex digits), as its own name; or, its name too long for that
/// (past 242 bytes under `home`), under `.long/` at the top, in a folder
/// named by the first `SHORT_HEX` of the sha256 of `<root>/<its path>`,
/// with room for any name a file system keeps (`offload::NAME_MAX`). Its
/// note is where the file was, as every note is, and names this place (a
/// record's `at`), so `--back` brings it there unchanged.
const LONG: &str = ".long";
const LONG_HEX: usize = 24;
const SHORT_HEX: usize = 16;

/// Whether `path` of the home has its own place in storage, `<root>/<path>`,
/// for any part it could be in: a repo path, as `offload::record_of` checks
/// a note's.
fn fits(root: &str, path: &str) -> bool {
    RepoPath::parse(&format!("{root}/{path}.part-{n}-of-{n}", n = offload::PARTS_MAX)).is_ok()
}

fn coded(code: Code, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(CodedError { code, msg: msg.into() })
}

/// What `keep` did, and what it leaves for another storage fragment.
#[derive(Debug, Serialize)]
pub struct Stored {
    /// The storage fragment of this run.
    pub fragment: String,
    /// Where the notes are.
    pub notes: String,
    /// Kept now, and the notes written for them.
    pub files: usize,
    pub bytes: u64,
    pub notes_written: usize,
    /// Kept before (a note says so, or brought back from one).
    pub already: usize,
    pub already_bytes: u64,
    /// Not kept yet: this storage fragment is full. Keep them in another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<More>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct More {
    pub files: usize,
    pub bytes: u64,
}

/// A file to keep: its path in the home, size, CRC-32 and bits.
#[derive(Debug, Clone)]
struct Keep {
    path: String,
    size: u64,
    crc32: u32,
    mode: u32,
}

/// A note's worth: a folder of the home (and all it keeps), or a file.
#[derive(Debug)]
struct Group {
    /// Its path in the home.
    key: String,
    folder: bool,
    files: Vec<Keep>,
}

impl Group {
    /// Its note's place, under `notes`, where it was.
    fn note(&self, notes: &Path) -> PathBuf {
        notes.join(self.parent()).join(offload::note_name(self.name()))
    }

    /// Its place in storage, its note's `at`: where it is in the home, under
    /// `root`; or, a file alone too deep for that, a short one (`LONG`), or
    /// a shorter one for a name too long for that. None when none is a
    /// place a note can name (its name past a file system's).
    fn at(&self, root: &str) -> Option<String> {
        let parent = self.parent();
        if self.folder || fits(root, &self.key) {
            return Some(if parent.is_empty() { root.to_string() } else { format!("{root}/{parent}") });
        }
        let long = format!("{root}/{LONG}/{}", &blob::sha256_hex(self.key.as_bytes())[..LONG_HEX]);
        if fits(&long, self.name()) {
            return Some(long);
        }
        let short = format!("{LONG}/{}", &blob::sha256_hex(format!("{root}/{}", self.key).as_bytes())[..SHORT_HEX]);
        fits(&short, self.name()).then_some(short)
    }

    /// Its parent folder in the home ("" at the top).
    fn parent(&self) -> &str {
        self.key.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// Its own name.
    fn name(&self) -> &str {
        self.key.rsplit('/').next().expect("a key names something")
    }

    /// Its record, its files' sums as the zip names them, at `at` in the
    /// storage fragment (`Group::at`).
    fn record(&self, fragment: &str, at: String, notes: &Path, part_bytes: u64) -> Record {
        let parent = self.parent();
        let rel = |p: &str| if parent.is_empty() { p.to_string() } else { p[parent.len() + 1..].to_string() };
        let files: Vec<Moved> = self.files.iter().map(|k| Moved { path: rel(&k.path), size: k.size, sha256: None, crc32: Some(k.crc32), mode: k.mode }).collect();
        // a folder's folders, its own first (a child sorts after its parent)
        let mut dirs = BTreeSet::new();
        if self.folder {
            for f in &files {
                let mut at = f.path.as_str();
                while let Some((up, _)) = at.rsplit_once('/') {
                    dirs.insert(up.to_string());
                    at = up;
                }
            }
        }
        let dirs = dirs.into_iter().map(|path| Folder { path, mode: 0o755 }).collect();
        Record { fragment: fragment.to_string(), at, from: notes.join(&self.key).display().to_string(), part_bytes, dirs, files }
    }
}

/// The groups of `keep`, each a note's worth: a file's group is the
/// outermost folder on its way whose kept files are within a note's
/// bounds, else the file alone. A file too deep for its own place under
/// `root` (`fits`) is alone, whatever its folder: kept elsewhere (`LONG`),
/// it is no part of its folder's note. Iterative, in path order.
fn groups(keep: Vec<Keep>, part_bytes: u64, root: &str) -> Vec<Group> {
    assert!(part_bytes > 0, "a part holds bytes");
    let (keep, deep): (Vec<Keep>, Vec<Keep>) = keep.into_iter().partition(|k| fits(root, &k.path));
    // each folder's files, bytes, parts and folders (its own among them)
    let mut sums: HashMap<&str, (usize, u64, u64, usize)> = HashMap::new();
    let mut folders: BTreeSet<&str> = BTreeSet::new();
    for k in &keep {
        let parts = k.size.div_ceil(part_bytes).max(1);
        for (i, _) in k.path.match_indices('/') {
            let s = sums.entry(&k.path[..i]).or_default();
            (s.0, s.1, s.2) = (s.0 + 1, s.1 + k.size, s.2 + parts);
            folders.insert(&k.path[..i]);
        }
    }
    for f in &folders {
        for (i, _) in f.match_indices('/') {
            sums.get_mut(&f[..i]).expect("a folder's folders are folders").3 += 1;
        }
        sums.get_mut(f).expect("a folder").3 += 1;
    }
    let fits = |(files, bytes, parts, dirs): (usize, u64, u64, usize)| files <= NOTE_FILES_MAX && bytes <= NOTE_BYTES_MAX && parts <= offload::PARTS_MAX && dirs <= offload::FILES_MAX;
    let key_of = |k: &Keep| -> (String, bool) {
        for (i, _) in k.path.match_indices('/') {
            if fits(sums[&k.path[..i]]) {
                return (k.path[..i].to_string(), true);
            }
        }
        (k.path.clone(), false)
    };
    let keys: Vec<(String, bool)> = keep.iter().map(key_of).collect();
    let mut out: BTreeMap<String, Group> = BTreeMap::new();
    for (k, (key, folder)) in keep.into_iter().zip(keys) {
        out.entry(key.clone()).or_insert_with(|| Group { key, folder, files: Vec::new() }).files.push(k);
    }
    for k in deep {
        out.insert(k.path.clone(), Group { key: k.path.clone(), folder: false, files: vec![k] });
    }
    let out: Vec<Group> = out.into_values().collect();
    assert!(out.iter().all(|g| g.files.len() <= NOTE_FILES_MAX && (g.folder || g.files.len() == 1)), "each group is a note's worth");
    out
}

/// Each note under `notes` (iterative), by its path, read: what it says is
/// kept, its places held. Every file named as a note is
/// (`<name>.offloaded.md`) whose record reads is one, whatever else holds
/// its bytes. One whose record does not read is refused, but the home's
/// own file `brought` back from its note (FIN-757), which holds no place.
fn notes_under(notes: &Path, brought: &dyn Fn(&Path) -> bool) -> Result<Vec<(PathBuf, Record)>> {
    let mut out = Vec::new();
    let mut folders = vec![notes.to_path_buf()];
    while let Some(dir) = folders.pop() {
        let listed = match std::fs::read_dir(&dir) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", dir.display()))),
        };
        for e in listed {
            let e = e?;
            let t = e.file_type()?;
            if t.is_dir() {
                folders.push(e.path());
            } else if t.is_file() && e.file_name().to_str().is_some_and(|n| n.ends_with(NOTE_SUFFIX)) {
                // every record there is a note, its places held, whatever
                // else holds its bytes; one that reads as none is refused,
                // but a home's file brought back
                let text = std::fs::read(e.path()).with_context(|| format!("reading {}", e.path().display()))?;
                match String::from_utf8(text).map_err(anyhow::Error::new).and_then(|t| offload::record_of(&t)) {
                    Ok(r) => out.push((e.path(), r)),
                    Err(_) if brought(&e.path()) => {}
                    Err(why) => bail!(coded(Code::Conflict, format!("{} is not a note this import or `fragment offload` wrote ({why:#}): move it aside, then run this again", e.path().display()))),
                }
            }
        }
    }
    Ok(out)
}

/// Whether the file at `at` is one brought back from a note: a file, of
/// the home's file's size and CRC-32.
fn brought_back(at: &Path, size: u64, crc32: u32) -> bool {
    let same = |mut f: std::fs::File| -> std::io::Result<bool> {
        let mut h = crc32fast::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = std::io::Read::read(&mut f, &mut buf)?;
            if n == 0 {
                return Ok(h.finalize() == crc32);
            }
            h.update(&buf[..n]);
        }
    };
    std::fs::symlink_metadata(at).is_ok_and(|m| m.is_file() && m.len() == size) && std::fs::File::open(at).and_then(same).unwrap_or(false)
}

/// What a part is in git once kept: a blob's pointer, whose blob must be
/// in the store, or its own bytes.
enum Kept {
    Blob(String),
    Bytes,
}

/// Keeps `keep` (every file of `src` not brought otherwise) in the storage
/// fragment `into`, with a note for each group under `notes`.
pub fn keep(c: &Client, src: &Source, keep: Vec<String>, into: &str, notes: &Path) -> Result<Stored> {
    keep_within(c, src, keep, into, notes, STORE_FILES_MAX)
}

/// `keep`, a storage fragment holding at most `files_max` files.
fn keep_within(c: &Client, src: &Source, keep: Vec<String>, into: &str, notes: &Path, files_max: usize) -> Result<Stored> {
    assert!(files_max <= STORE_FILES_MAX, "a storage fragment holds what an export takes");
    if src.kind() == "folder" {
        bail!(coded(Code::InvalidUsage, "--store keeps a zip's or a stage's files, whose CRC-32 each note names: zip the folder first"));
    }
    let into = offload::check_into(c, into)?;
    std::fs::create_dir_all(notes).with_context(|| format!("making {}", notes.display()))?;
    let notes = notes.canonicalize().with_context(|| format!("finding {}", notes.display()))?;
    let list: fragment_proto::FragmentList = c.call_as(c.get("/api/fragments")?)?;
    let part_bytes = crate::session::upload_max(&list, c.agent().is_some());

    // this notes folder's own place in storage: its name, never `LONG`,
    // where any notes folder's long names are kept
    let root = notes.file_name().and_then(|n| n.to_str()).filter(|n| *n != LONG && RepoPath::parse(&format!("{n}/x")).is_ok()).map(str::to_string);
    let root = root.ok_or_else(|| coded(Code::InvalidUsage, format!("{}: --notes names a folder whose name a fragment's folder can be (such as home), but {LONG}", notes.display())))?;

    // what the notes keep: each file as kept (its size and sums), and the
    // paths its parts hold, by fragment
    let mut covered: HashMap<String, Moved> = HashMap::new();
    let mut taken: HashMap<(String, String), String> = HashMap::new();
    // a file there named as a note is, reading as no note, the home's own
    // with its bytes, was brought back from its note: it is that file
    let brought = |at: &Path| -> bool {
        let rel = at.strip_prefix(&notes).ok().and_then(Path::to_str);
        let sums = rel.and_then(|rel| Some((src.get(rel)?.size, src.sums(rel).ok()??.0)));
        sums.is_some_and(|(size, crc32)| brought_back(at, size, crc32))
    };
    let found = notes_under(&notes, &brought)?;
    for (note, r) in &found {
        let dir = note.parent().expect("a note has a folder").strip_prefix(&notes).expect("found under the notes").to_string_lossy().to_string();
        for f in &r.files {
            let path = if dir.is_empty() { f.path.clone() } else { format!("{dir}/{}", f.path) };
            for (part, _) in r.parts(f) {
                taken.insert((r.fragment.clone(), part), path.clone());
            }
            covered.insert(path, f.clone());
        }
    }
    // a path that is a file and a folder of the home both is no home a
    // filesystem held: its notes could not say which comes back
    let folders: BTreeSet<&str> = src.entries().iter().flat_map(|e| e.path.match_indices('/').map(|(i, _)| &e.path[..i])).collect();
    if let Some(both) = src.entries().iter().find(|e| folders.contains(e.path.as_str())) {
        bail!(coded(Code::InvalidRequest, format!("{} is a file and a folder of this home both: it is not kept", both.path)));
    }
    let (mut todo, mut already, mut already_bytes, mut differ) = (Vec::new(), 0, 0u64, Vec::new());
    for path in keep {
        let size = src.get(&path).ok_or_else(|| anyhow!("{path}: no such file"))?.size;
        let (crc32, mode) = src.sums(&path)?.ok_or_else(|| anyhow!("{path}: its zip names no CRC-32"))?;
        match covered.get(&path) {
            // kept already, as this home holds it
            Some(f) if f.size == size && f.crc32 == Some(crc32) => (already, already_bytes) = (already + 1, already_bytes + size),
            // kept, but other bytes: another copy of the home's
            Some(_) => differ.push(path),
            None if brought_back(&notes.join(&path), size, crc32) => (already, already_bytes) = (already + 1, already_bytes + size),
            None => todo.push(Keep { path, size, crc32, mode: mode.filter(|m| *m != 0).unwrap_or(0o644) }),
        }
    }
    if let Some(first) = differ.first() {
        bail!(coded(
            Code::Conflict,
            format!(
                "{} files the notes in {} keep are other bytes than this home's ({first}, …): they keep another copy of it. Nothing was written. Keep this one beside it, with --notes naming a new folder (such as {}-2)",
                differ.len(),
                notes.display(),
                notes.display()
            )
        ));
    }

    let mut held = Held::new(&into, Wants::Write);
    if !held.get(c)?.writes() {
        bail!(coded(Code::Forbidden, format!("you can read {into} but not write it: keep files in a fragment you edit")));
    }
    let blobs = Blobs::new(c, &into);
    let author = Author::writer(&c.writer_id());
    let mut head = held.get(c)?.branch_head(MAIN)?;
    let mut there: HashMap<String, u64> = match &head {
        Some(rev) => held.get(c)?.list_files(rev)?.entries.into_iter().map(|e| (e.path.as_str().to_string(), e.size)).collect(),
        None => HashMap::new(),
    };
    let had_manifest = there.contains_key("fragment.json");
    let mut done = Stored { fragment: into.clone(), notes: notes.display().to_string(), files: 0, bytes: 0, notes_written: 0, already, already_bytes, more: None };
    let groups = groups(todo, part_bytes, &root);
    let records: Vec<Record> = groups
        .iter()
        .map(|g| {
            let at = g.at(&root).ok_or_else(|| coded(Code::InvalidUsage, format!("{} cannot be kept: its name is longer than a file's can be ({} bytes), or one no fragment's file can have. Nothing was written", g.key, offload::NAME_MAX)))?;
            Ok(g.record(&into, at, &notes, part_bytes))
        })
        .collect::<Result<_>>()?;
    // every path a run writes is its file's alone: never another's, nor
    // one a note keeps (a file named as another's part), and each note's
    // place its own, never a note's there (a name named short, crafted to
    // be another's), checked first
    let mut noted: BTreeSet<PathBuf> = found.into_iter().map(|(note, _)| note).collect();
    for (g, r) in groups.iter().zip(&records) {
        if !noted.insert(g.note(&notes)) {
            bail!(coded(Code::InvalidRequest, format!("{}'s note would be {}, a note's place already: nothing was written", g.key, g.note(&notes).display())));
        }
        for (k, f) in g.files.iter().zip(&r.files) {
            for (part, _) in r.parts(f) {
                RepoPath::parse(&part).map_err(|e| coded(Code::InvalidUsage, format!("{} cannot be kept as {e}", k.path)))?;
                if let Some(other) = taken.insert((into.clone(), part.clone()), k.path.clone()).filter(|o| *o != k.path) {
                    bail!(coded(Code::InvalidRequest, format!("{} and {other} would both be kept as {part}: nothing was written", k.path)));
                }
            }
        }
    }
    let total = groups.len();
    for (n, (g, r)) in groups.iter().zip(&records).enumerate() {
        let parts: Vec<(&Keep, &Moved, u64, String, u64)> = g
            .files
            .iter()
            .zip(&r.files)
            .flat_map(|(k, f)| r.parts(f).into_iter().enumerate().map(move |(i, (path, len))| (k, f, i as u64 * part_bytes, path, len)))
            .collect();
        let new = parts.iter().filter(|p| !there.contains_key(&p.3)).count();
        if new > files_max {
            bail!(coded(Code::TooLarge, format!("{} is {new} parts, more than a storage fragment holds ({files_max}): it is not kept", g.key)));
        }
        if there.len() + new > files_max {
            let rest = &groups[n..];
            done.more = Some(More { files: rest.iter().map(|g| g.files.len()).sum(), bytes: rest.iter().flat_map(|g| &g.files).map(|k| k.size).sum() });
            break;
        }
        // its parts, a window at a time: the blobs copied (or uploaded),
        // the small ones read, then committed
        let mut kept: Vec<(String, u64, Kept)> = Vec::with_capacity(parts.len());
        // each file's CRC-32 as its parts were kept, in order, to check
        // against the zip's own of the old file before its note
        let mut sums: HashMap<&str, crc32fast::Hasher> = HashMap::new();
        let (mut batch, mut batch_bytes) = (Vec::new(), 0usize);
        let message = format!("home store: {}", g.key);
        for window in parts.chunks(WINDOW) {
            let made = part_bytes_of(src, &blobs, &into, window)?;
            for ((file, _, _, path, len), (bytes, k, crc)) in window.iter().zip(made) {
                sums.entry(file.path.as_str()).or_default().combine(&crc32fast::Hasher::new_with_initial_len(crc, *len));
                batch_bytes += bytes.len();
                batch.push(Change::Upsert { path: RepoPath::parse(path).expect("checked above"), bytes });
                kept.push((path.clone(), *len, k));
                if batch.len() >= BATCH_FILES || batch_bytes >= BATCH_BYTES {
                    head = commit(c, &mut held, &author, head, &message, &batch)?;
                    (batch, batch_bytes) = (Vec::new(), 0);
                }
            }
        }
        if !batch.is_empty() {
            head = commit(c, &mut held, &author, head, &message, &batch)?;
        }
        // proven: each file the old one (its parts' CRC-32, combined, the
        // zip's of it), each part at main, the size it was written, each
        // blob in the store
        for k in &g.files {
            let crc = sums.remove(k.path.as_str()).map(|h| h.finalize());
            if crc != Some(k.crc32) {
                bail!("{} in the stage is not the old file it names (its CRC-32 is {crc:08x?}, the zip's {:08x}); no note was written for {}", k.path, k.crc32, g.key);
            }
        }
        let rev = head.clone().ok_or_else(|| anyhow!("{into}'s main has no head after its commit"))?;
        there = held.get(c)?.list_files(&rev)?.entries.into_iter().map(|e| (e.path.as_str().to_string(), e.size)).collect();
        for (path, len, k) in &kept {
            let want = match k {
                Kept::Blob(sha) => blob::pointer(sha, *len).len() as u64,
                Kept::Bytes => *len,
            };
            if there.get(path) != Some(&want) {
                bail!("{path} is not in {into} as it was kept ({:?} bytes there, {want} written); no note was written for {}", there.get(path), g.key);
            }
            if let Kept::Blob(sha) = k {
                let found = c.head(&format!("/api/f/{into}/blobs/{sha}"))?;
                if !found.ok() {
                    bail!("{path}'s blob {sha} is not in {into}'s store (http {}); no note was written for {}", found.status, g.key);
                }
            }
        }
        let note = g.note(&notes);
        let dir = note.parent().expect("a note has a folder");
        std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
        let when = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
        let said = Said { title: "Kept in storage", came: format!("came from the old agent's home in its move here on {when}, and was kept off this disk") };
        let text = offload::readable_note(r, &note, g.name(), said).with_context(|| format!("{}'s files are in {into}, under {}, but no note was written", g.key, r.at))?;
        offload::publish(&note, &text)?;
        done.files += g.files.len();
        done.bytes += g.files.iter().map(|k| k.size).sum::<u64>();
        done.notes_written += 1;
        eprintln!("kept {} of {total} ({}): {} files, {}", n + 1, g.key, done.files + done.already, size(done.bytes + done.already_bytes));
    }
    if !had_manifest {
        if let Some(rev) = head.clone() {
            manifest(c, &mut held, &author, rev)?;
        }
    }
    crate::sync::refresh_pins(c, &into, MAIN);
    Ok(done)
}

/// Each part of `window`, as git keeps it, and its bytes' CRC-32: a blob's
/// copied on the platform from a stage (`COPIES_AT_ONCE` at once, its
/// CRC-32 the platform's) or uploaded from a zip here, its pointer; a small
/// one read, its bytes.
fn part_bytes_of(src: &Source, blobs: &Blobs, into: &str, window: &[(&Keep, &Moved, u64, String, u64)]) -> Result<Vec<(Vec<u8>, Kept, u32)>> {
    let mut out: Vec<Option<(Vec<u8>, Kept, u32)>> = (0..window.len()).map(|_| None).collect();
    let mut copies = Vec::new();
    for (i, (k, _, offset, _, len)) in window.iter().enumerate() {
        match (blobs.keeps(*len)?, src.stage_id().is_some(), src.span(&k.path)?) {
            (true, true, Some((start, size))) => {
                assert!(size == k.size && offset + len <= size, "a part is within its file");
                copies.push((i, start + offset, *len));
            }
            (true, _, _) => {
                let read = src.read_range(&k.path, *offset, *len)?;
                let crc = crc32fast::hash(&read);
                let stored = blobs.store(read)?;
                let sha = blob::parse(&stored).map(|p| p.sha256).ok_or_else(|| anyhow!("{}: kept as a blob, but no pointer came back", k.path))?;
                out[i] = Some((stored, Kept::Blob(sha), crc));
            }
            (false, _, _) => {
                let read = src.read_range(&k.path, *offset, *len)?;
                let crc = crc32fast::hash(&read);
                out[i] = Some((read, Kept::Bytes, crc));
            }
        }
    }
    for chunk in copies.chunks(COPIES_AT_ONCE) {
        let copied: Vec<Result<(String, u32)>> = std::thread::scope(|s| {
            let running: Vec<_> = chunk.iter().map(|(_, at, len)| s.spawn(move || src.copy(into, *at, *len))).collect();
            running.into_iter().map(|t| t.join().unwrap_or_else(|_| Err(anyhow!("a copy's thread panicked")))).collect()
        });
        for ((i, _, len), sha) in chunk.iter().zip(copied) {
            let (sha, crc) = sha.with_context(|| format!("copying {}", window[*i].0.path))?;
            out[*i] = Some((blob::pointer(&sha, *len).into_bytes(), Kept::Blob(sha), crc));
        }
    }
    Ok(out.into_iter().map(|o| o.expect("every part made")).collect())
}

pub fn print(s: &Stored) {
    println!(
        "Kept {} files ({}) in {}, {} notes in {}; {} kept before ({})",
        s.files,
        size(s.bytes),
        s.fragment,
        s.notes_written,
        s.notes,
        s.already,
        size(s.already_bytes)
    );
    if let Some(m) = s.more {
        println!("  {} files ({}) more to keep: {} is full. Run this again with --store naming another storage fragment.", m.files, size(m.bytes), s.fragment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::files::{bring, Store};
    use crate::home::source::tests::Temp;
    use crate::home::Layout;

    const FILES: &str = "old-files--k3x9";
    const STORE: &str = "old-storage--b2c3";
    const MIB: usize = 1024 * 1024;

    fn layout() -> Layout {
        Layout { work: Some("work".into()), ..Layout::hermes() }
    }

    fn bytes(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u32).wrapping_mul(2_654_435_761).rotate_right(seed as u32) as u8).collect()
    }

    /// The home every test keeps: its own self, small work the files' pick
    /// brings, and what it does not: weights, a dependency, a hidden
    /// folder, a video past one upload (2 MiB here: three parts), and a
    /// key file, never read.
    fn home() -> Vec<(String, Vec<u8>)> {
        vec![
            ("agent/hermes-home/SOUL.md".into(), b"me".to_vec()),
            ("agent/hermes-home/state.db".into(), bytes(300_000, 3)),
            ("work/notes/a.md".into(), b"# a".to_vec()),
            ("work/photos/p.png".into(), bytes(MIB + 7, 5)),
            ("work/models/w.gguf".into(), bytes(3 * MIB, 7)),
            ("work/app/node_modules/x/i.js".into(), b"x".to_vec()),
            ("work/proj/.git/HEAD".into(), b"ref: refs/heads/main\n".to_vec()),
            ("work/proj/.npmrc".into(), b"//r/:_authToken=x".to_vec()),
            ("work/videos/big.mp4".into(), bytes(5 * MIB + 3, 9)),
        ]
    }

    struct Setup {
        _t: Temp,
        notes: PathBuf,
        mock: crate::mockcs::MockServer,
        c: Client,
        src: Source,
    }

    fn setup(files: &[(String, Vec<u8>)]) -> Setup {
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, files).unwrap();
        let mock = crate::mockcs::start();
        mock.keep_blobs();
        mock.advertise_upload_max(Some(2 * MIB as u64));
        mock.seed_repo(FILES, &[]);
        mock.seed_repo(STORE, &[]);
        mock.seed_stage("old1.p1.1", std::fs::read(&zip).unwrap());
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let src = crate::home::open_from(Some(&c), Path::new("stage:old1.p1.1"), &layout()).unwrap();
        let notes = t.0.join("old-agent/home");
        Setup { _t: t, notes, mock, c, src }
    }

    /// A file of `fragment` at `path`, whole: its bytes in git, or its
    /// blob's, or its parts' in order.
    fn kept(mock: &crate::mockcs::MockServer, fragment: &str, path: &str) -> Option<Vec<u8>> {
        let one = |p: &str| {
            let git = mock.file_at(fragment, "main", p)?;
            match blob::parse(&git) {
                Some(ptr) => mock.blob(fragment, &ptr.sha256),
                None => Some(git),
            }
        };
        if let Some(b) = one(path) {
            return Some(b);
        }
        let n = mock.paths(fragment, "main").iter().filter(|p| p.starts_with(&format!("{path}.part-"))).count();
        (n > 0).then(|| (1..=n).flat_map(|i| one(&format!("{path}.part-{i:04}-of-{n:04}")).unwrap_or_default()).collect())
    }

    /// Goal (Paul, 2026-11-19: a move carries all the data, never less):
    /// every file of a home is brought into its files' fragment or kept
    /// whole in storage, each counted and compared byte for byte, but the
    /// key file, which nothing reads; a file past one upload in parts; the
    /// large ones copied on the platform, never read here; a note where
    /// each group was, which `fragment offload --back` brings back, checked
    /// against the old file's CRC-32. Replay: again, nothing more is kept.
    #[test]
    fn every_file_of_a_home_is_brought_or_kept_whole() {
        let files = home();
        let s = setup(&files);
        let b = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let stored = b.stored.as_ref().expect("kept");
        assert_eq!((b.home_files, b.unread), (files.len(), 1), "the key file, unread");
        assert_eq!(b.files + stored.files, files.len() - 1, "every other file brought or kept");
        for (path, want) in &files {
            let here = path.strip_prefix("work/").and_then(|rel| kept(&s.mock, FILES, &format!("files/{rel}")));
            let there = kept(&s.mock, STORE, &format!("home/{path}"));
            match path.as_str() {
                "work/proj/.npmrc" => assert!(here.is_none() && there.is_none(), "{path}: never read, kept nowhere"),
                _ => assert!((here.as_ref() == Some(want)) ^ (there.as_ref() == Some(want)), "{path}: in one place, whole ({:?} {:?})", here.map(|b| b.len()), there.map(|b| b.len())),
            }
        }
        assert!(s.mock.paths(STORE, "main").iter().any(|p| p == "home/work/videos/big.mp4.part-0003-of-0003"), "past one upload: in parts");
        let (_, read) = s.mock.stage_reads();
        assert!(read < 3 * MIB as u64, "the large files were copied on the platform, never read here: {read} bytes read");
        let left = String::from_utf8(s.mock.file_at(FILES, "main", "left-behind.md").unwrap()).unwrap();
        assert!(left.starts_with("# Kept in storage") && left.contains(STORE), "{left}");
        // a note for each group, which reads as a record of what it keeps
        let notes = notes_under(&s.notes, &|_| false).unwrap();
        let named: usize = notes.iter().map(|(_, r)| r.files.len()).sum();
        assert_eq!((named, stored.notes_written, notes.len()), (stored.files, notes.len(), notes.len()));
        assert!(notes.iter().all(|(_, r)| r.fragment == STORE && r.files.iter().all(|f| f.crc32.is_some())));

        // replay: nothing kept twice, nothing committed
        let head = s.mock.branch(STORE, "main");
        let again = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let again = again.stored.unwrap();
        assert_eq!((again.files, again.already), (0, stored.files));
        assert_eq!(s.mock.branch(STORE, "main"), head);

        // back: the video's note brings it, from its parts, as it was
        let (note, _) = notes.iter().find(|(_, r)| r.files.iter().any(|f| f.path.ends_with("big.mp4"))).expect("the video's note");
        let back = crate::offload::back(&s.c, note).unwrap();
        let video = &files.iter().find(|(p, _)| p.ends_with("big.mp4")).unwrap().1;
        assert_eq!(std::fs::read(s.notes.join("work/videos/big.mp4")).unwrap(), *video);
        assert!(back.files >= 1 && !note.exists());
        // brought back, it is kept already: a run again keeps it no more
        let after = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((after.files, after.already), (0, stored.files));
        // changed since (its size the same), it is no copy of the home's: kept again
        let mut changed = video.clone();
        changed[0] ^= 1;
        std::fs::write(s.notes.join("work/videos/big.mp4"), &changed).unwrap();
        let again = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((again.files, again.already), (1, stored.files - 1), "the video kept again, its note back");
    }

    /// Goal: a run cut off partway (a copy refused) leaves no note for
    /// what it did not prove, and run again it goes on: in the end each
    /// file kept once, whole, as a run that was never cut off keeps it.
    #[test]
    fn a_run_cut_off_goes_on_and_keeps_each_file_once() {
        let files = home();
        let s = setup(&files);
        s.mock.fail_stage_copies(1, 1);
        let e = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("503"), "{e}");
        let cut: usize = notes_under(&s.notes, &|_| false).unwrap().iter().map(|(_, r)| r.files.len()).sum();
        let b = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let stored = b.stored.unwrap();
        assert_eq!(stored.already, cut, "what was proven before is kept already");
        assert_eq!(b.files + b.already + stored.files + stored.already, files.len() - 1);
        for (path, want) in files.iter().filter(|(p, _)| !p.starts_with("work/notes") && !p.starts_with("work/photos") && !p.ends_with(".npmrc")) {
            assert_eq!(kept(&s.mock, STORE, &format!("home/{path}")).as_ref(), Some(want), "{path}");
        }
        let named: usize = notes_under(&s.notes, &|_| false).unwrap().iter().map(|(_, r)| r.files.len()).sum();
        assert_eq!(named, stored.files + stored.already, "each file named by one note");
    }

    /// Goal: a storage fragment holds what an export takes of one; a home
    /// with more is kept in two, the run that filled the first saying what
    /// is left, and the next keeping it in the second, each note naming
    /// its own. Method: a fragment of a few files at most.
    #[test]
    fn a_full_storage_fragment_says_what_is_left_for_another() {
        let files = home();
        let s = setup(&files);
        let rest: Vec<String> = s.src.entries().iter().map(|e| e.path.clone()).filter(|p| !p.starts_with("work/notes")).collect();
        // its own self (2 parts) fits; its work (8 parts) does not, beside it
        let first = keep_within(&s.c, &s.src, rest.clone(), STORE, &s.notes, 8).unwrap();
        let more = first.more.expect("it is full");
        assert!(first.files > 0 && s.mock.paths(STORE, "main").len() <= 8, "{first:?}");
        // a group no storage fragment could hold is refused, never put off
        s.mock.seed_repo("old-storage-3--f6g7", &[]);
        let e = keep_within(&s.c, &s.src, rest.clone(), "old-storage-3--f6g7", &s.notes, 7).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("more than a storage fragment holds"), "{e}");
        let second_name = "old-storage-2--d4e5";
        s.mock.seed_repo(second_name, &[]);
        let second = keep_within(&s.c, &s.src, rest.clone(), second_name, &s.notes, STORE_FILES_MAX).unwrap();
        assert!(second.more.is_none());
        assert_eq!((second.files, second.already), (more.files, first.files));
        let fragments: BTreeSet<String> = notes_under(&s.notes, &|_| false).unwrap().into_iter().map(|(_, r)| r.fragment).collect();
        assert_eq!(fragments, BTreeSet::from([STORE.to_string(), second_name.to_string()]));
    }

    /// Invalid: a note there that no store or offload wrote, a storage
    /// fragment not members-only, or a home read from a folder (no CRC-32
    /// to name): refused, nothing kept.
    #[test]
    fn what_cannot_be_kept_as_promised_is_refused() {
        let s = setup(&home());
        let rest: Vec<String> = s.src.entries().iter().map(|e| e.path.clone()).collect();
        std::fs::create_dir_all(s.notes.join("work")).unwrap();
        std::fs::write(s.notes.join("work/odd.offloaded.md"), "# not a note").unwrap();
        let e = keep(&s.c, &s.src, rest.clone(), STORE, &s.notes).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("is not a note this import"), "{e}");
        std::fs::remove_file(s.notes.join("work/odd.offloaded.md")).unwrap();
        s.mock.describe(STORE, "link", "app");
        let e = keep(&s.c, &s.src, rest.clone(), STORE, &s.notes).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("members-only"), "{e}");
        assert!(s.mock.paths(STORE, "main").is_empty() && notes_under(&s.notes, &|_| false).unwrap().is_empty());
        let folder = Source::open(&s.notes).unwrap();
        let e = keep(&s.c, &folder, vec![], STORE, &s.notes).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("zip the folder first"), "{e}");
        // notes named as the place every notes folder's long names are kept
        let other = "old-storage-4--h8j9";
        s.mock.seed_repo(other, &[]);
        let e = keep(&s.c, &s.src, rest.clone(), other, &s.notes.with_file_name(LONG)).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains(&format!("but {LONG}")), "{e}");
        assert!(s.mock.paths(other, "main").is_empty());
    }

    /// Goal: a copy whose blob is not what was kept never comes back as
    /// the file: `--back` refuses it by its CRC-32, and the note stays.
    #[test]
    fn a_kept_file_that_changed_in_storage_is_not_brought_back() {
        let s = setup(&home());
        bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let notes = notes_under(&s.notes, &|_| false).unwrap();
        let (note, r) = notes.iter().find(|(_, r)| r.files.iter().any(|f| f.path.ends_with("w.gguf"))).expect("the weights' note");
        // its first part of two (3 MiB, in parts of 2)
        let at = format!("{}/{}.part-0001-of-0002", r.at, r.files.iter().find(|f| f.path.ends_with("w.gguf")).unwrap().path);
        let ptr = blob::parse(&s.mock.file_at(STORE, "main", &at).unwrap()).unwrap();
        s.mock.corrupt_blob(&ptr.sha256, true);
        let e = crate::offload::back(&s.c, note).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("not those stored") || e.contains("other bytes"), "{e}");
        assert!(note.exists(), "the note stays");
    }

    /// Goal (Codex on #1561): a note's place is never freed by a copy of the
    /// note in the home. A home holding its own copy of a note this import
    /// wrote, and other bytes for the file that note keeps, is refused
    /// before anything is written, and the note still brings back its own.
    #[test]
    fn a_copy_of_a_note_in_the_home_never_frees_its_place() {
        let mut files = home();
        files.push(("model.gguf".into(), bytes(4096, 17)));
        let s = setup(&files);
        bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let note = s.notes.join(format!("model.gguf{NOTE_SUFFIX}"));
        let mut changed = files.clone();
        changed.last_mut().unwrap().1[0] ^= 1;
        changed.push((format!("model.gguf{NOTE_SUFFIX}"), std::fs::read(&note).unwrap()));
        let zip = s._t.0.join("stage-copy.zip");
        fragment_fakes::old_home::zip(&zip, &changed).unwrap();
        s.mock.seed_stage("old1.p1.3", std::fs::read(&zip).unwrap());
        let src2 = crate::home::open_from(Some(&s.c), Path::new("stage:old1.p1.3"), &layout()).unwrap();
        let head = s.mock.branch(STORE, "main");
        let e = bring(&s.c, &src2, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("other bytes than this home's") && e.contains("model.gguf"), "{e}");
        assert_eq!(s.mock.branch(STORE, "main"), head, "nothing written");
        crate::offload::back(&s.c, &note).unwrap();
        assert_eq!(std::fs::read(s.notes.join("model.gguf")).unwrap(), files.last().unwrap().1, "the note brings back its own");
    }

    /// Goal (Codex on #1389): a home whose files differ from what its
    /// notes keep (an old agent that ran again before another move) never
    /// counts them kept, and never writes over them: refused before
    /// anything is written; kept beside, under a new notes folder's own
    /// place in storage; and each note still brings back its own bytes.
    #[test]
    fn a_changed_copy_of_the_home_is_kept_beside_never_over() {
        let files = home();
        let s = setup(&files);
        bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let mut changed = files.clone();
        let db = changed.iter_mut().find(|(p, _)| p.ends_with("state.db")).unwrap();
        db.1[0] ^= 1;
        let zip = s._t.0.join("stage-2.zip");
        fragment_fakes::old_home::zip(&zip, &changed).unwrap();
        s.mock.seed_stage("old1.p1.2", std::fs::read(&zip).unwrap());
        let src2 = crate::home::open_from(Some(&s.c), Path::new("stage:old1.p1.2"), &layout()).unwrap();
        let head = s.mock.branch(STORE, "main");
        let e = bring(&s.c, &src2, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("other bytes than this home's") && e.contains("state.db"), "{e}");
        assert_eq!(s.mock.branch(STORE, "main"), head, "nothing written");
        let beside = s.notes.with_file_name("home-2");
        let b = bring(&s.c, &src2, &layout(), FILES, Some(Store { into: STORE, notes: &beside })).unwrap().stored.unwrap();
        assert_eq!(b.already, 0);
        assert_eq!(kept(&s.mock, STORE, "home-2/agent/hermes-home/state.db").as_ref(), Some(&changed.iter().find(|(p, _)| p.ends_with("state.db")).unwrap().1));
        let first = &files.iter().find(|(p, _)| p.ends_with("state.db")).unwrap().1;
        assert_eq!(kept(&s.mock, STORE, "home/agent/hermes-home/state.db").as_ref(), Some(first), "the first copy is as it was kept");
        crate::offload::back(&s.c, &s.notes.join("agent.offloaded.md")).unwrap();
        assert_eq!(std::fs::read(s.notes.join("agent/hermes-home/state.db")).unwrap(), *first, "and its note brings it back");
    }

    /// Goal (Codex on #1388): a file damaged in the stage, its directory
    /// whole, is never kept as the old file: its parts' CRC-32s, combined,
    /// are checked against the zip's before its note, and no note is
    /// written for it.
    #[test]
    fn a_damaged_file_in_parts_gets_no_note() {
        let files = home();
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, &files).unwrap();
        let mut bytes = std::fs::read(&zip).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let i = archive.index_for_name("work/videos/big.mp4").unwrap();
        let start = archive.by_index_raw(i).unwrap().data_start().unwrap();
        bytes[start as usize + 3 * MIB] ^= 1;
        let s = setup(&files);
        s.mock.seed_stage("old1.p1.9", bytes);
        let src = crate::home::open_from(Some(&s.c), Path::new("stage:old1.p1.9"), &layout()).unwrap();
        let e = bring(&s.c, &src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("work/videos/big.mp4 in the stage is not the old file"), "{e}");
        assert!(notes_under(&s.notes, &|_| false).unwrap().iter().all(|(_, r)| r.files.iter().all(|f| !f.path.ends_with("big.mp4"))), "no note names it");
    }

    /// Goal (Codex on #1389): a file named as another's part never takes
    /// its place: refused before anything is written.
    #[test]
    fn a_file_named_as_anothers_part_is_refused_before_anything_is_written() {
        let mut files = home();
        files.push(("agent/big.bin".into(), bytes(5 * MIB, 11)));
        files.push(("agent/big.bin.part-0001-of-0003".into(), b"not a part".to_vec()));
        let s = setup(&files);
        let head = s.mock.branch(STORE, "main");
        let e = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("would both be kept as home/agent/big.bin.part-0001-of-0003"), "{e}");
        assert_eq!(s.mock.branch(STORE, "main"), head);
        assert!(notes_under(&s.notes, &|_| false).unwrap().is_empty());
    }

    /// Goal (FIN-755): a file whose own place in storage would be past a
    /// repo path's length (an old export's deep run logs) is kept whole
    /// under a short place of its own, and never stops the rest. Its note
    /// stays where the file was, and `--back` brings it there, byte for
    /// byte, from its parts too. Replay: kept already, nothing committed.
    #[test]
    fn a_file_too_deep_for_its_own_place_is_kept_under_a_short_one() {
        let deep = format!("work/finite-export-v2/run-1/stage/{}", (0..6).map(|i| format!("{i}{}", "d".repeat(49))).collect::<Vec<_>>().join("/"));
        let (log, video) = (format!("{deep}/run.log"), format!("{deep}/trace.bin"));
        assert!(!fits("home", &log), "past a repo path where it was");
        let mut files = home();
        files.push((log.clone(), b"step 1 ok\n".to_vec()));
        files.push((video.clone(), bytes(5 * MIB + 1, 13)));
        files.push(("work/finite-export-v2/run-1/stage/top.json".into(), b"{}".to_vec()));
        let s = setup(&files);
        let b = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let stored = b.stored.expect("kept");
        assert_eq!(b.files + stored.files, files.len() - 1, "every file but the key brought or kept");
        let notes = notes_under(&s.notes, &|_| false).unwrap();
        for (path, want) in [(&log, &files[files.len() - 3].1), (&video, &files[files.len() - 2].1)] {
            let (note, r) = notes.iter().find(|(_, r)| r.files.iter().any(|f| path.ends_with(&format!("/{}", f.path)) && r.from.ends_with(path.as_str()))).expect("its own note");
            assert!(r.at.starts_with(&format!("home/{LONG}/")) && r.at.len() < 40, "a short place: {}", r.at);
            assert_eq!(note, &s.notes.join(format!("{path}{NOTE_SUFFIX}")), "its note where it was");
            assert_eq!(kept(&s.mock, STORE, &format!("{}/{}", r.at, r.files[0].path)).as_ref(), Some(want), "{path}: whole");
        }
        assert!(s.mock.paths(STORE, "main").iter().all(|p| RepoPath::parse(p).is_ok()));
        // its folder's other files, as they were: brought, or kept where they were
        let here = kept(&s.mock, FILES, "files/finite-export-v2/run-1/stage/top.json");
        let there = kept(&s.mock, STORE, "home/work/finite-export-v2/run-1/stage/top.json");
        assert!((here.as_deref() == Some(&b"{}"[..])) ^ (there.as_deref() == Some(&b"{}"[..])), "{here:?} {there:?}");

        // replay: nothing kept twice, nothing committed
        let head = s.mock.branch(STORE, "main");
        let again = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((again.files, again.already), (0, stored.files));
        assert_eq!(s.mock.branch(STORE, "main"), head);

        // back: each comes to where it was, as it was
        for (path, want) in [(&log, &files[files.len() - 3].1), (&video, &files[files.len() - 2].1)] {
            crate::offload::back(&s.c, &s.notes.join(format!("{path}{NOTE_SUFFIX}"))).unwrap();
            assert_eq!(std::fs::read(s.notes.join(path)).unwrap(), *want, "{path}: back");
        }
    }

    /// Goal (FIN-755; Codex on #1557): a file too deep for its own place
    /// whose name is too long even for the short one (243 to 255 bytes, a
    /// name a file system keeps) is kept whole under a shorter place still;
    /// a file or a folder whose note's name would be past a file name's
    /// most has a note named short, where it was. `--back`, by the note or
    /// by where it was, brings each to its own name, byte for byte. Replay:
    /// kept already, nothing committed; brought back, kept already.
    #[test]
    fn a_file_whose_name_is_too_long_for_its_place_or_its_note_is_kept() {
        let deep = format!("work/x/{}", (0..6).map(|i| format!("{i}{}", "d".repeat(49))).collect::<Vec<_>>().join("/"));
        let wide = format!("{}x", "é".repeat(127));
        assert_eq!(wide.len(), 255, "the longest name a file system keeps, in letters of two bytes");
        // too deep, its name 250 bytes (in parts) and 255; and at the top, its note's name too long
        let long = [(format!("{deep}/{}", "n".repeat(250)), bytes(5 * MIB + 1, 21)), (format!("{deep}/{wide}"), b"wide".to_vec()), ("m".repeat(250), b"top".to_vec())];
        // and a folder whose own name is too long for its note's
        let folder = "f".repeat(250);
        let mut files = home();
        files.extend(long.iter().cloned());
        files.push((format!("{folder}/inner.txt"), b"in".to_vec()));
        let s = setup(&files);
        let b = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let stored = b.stored.expect("kept");
        assert_eq!(b.files + stored.files, files.len() - 1, "every file but the key brought or kept");
        let notes = notes_under(&s.notes, &|_| false).unwrap();
        let note_of = |path: &str| notes.iter().find(|(_, r)| r.from.ends_with(path)).cloned().expect("its own note");
        let (folder_note, r) = note_of(&folder);
        let named = folder_note.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        assert!(named.len() <= 255 && named.ends_with(NOTE_SUFFIX) && r.at == "home" && r.dirs[0].path == folder, "{}", folder_note.display());
        for (i, (path, want)) in long.iter().enumerate() {
            let (note, r) = note_of(path);
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            let named = note.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            assert!(named.len() <= 255 && named.ends_with(NOTE_SUFFIX) && note.parent() == Some(&*s.notes.join(parent)), "a note a file system keeps, where it was: {}", note.display());
            assert_eq!((r.files.len(), r.files[0].path.as_str()), (1, name), "its own name");
            match i {
                2 => assert_eq!(r.at, "home", "where it was"),
                _ => assert!(r.at.starts_with(&format!("{LONG}/")) && r.at.len() <= 22, "a shorter place: {}", r.at),
            }
            assert_eq!(kept(&s.mock, STORE, &format!("{}/{name}", r.at)).as_ref(), Some(want), "{path}: whole");
        }
        assert!(s.mock.paths(STORE, "main").iter().all(|p| RepoPath::parse(p).is_ok()));

        // replay: nothing kept twice, nothing committed
        let head = s.mock.branch(STORE, "main");
        let again = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((again.files, again.already), (0, stored.files));
        assert_eq!(s.mock.branch(STORE, "main"), head);

        // back, by its note or by where it was: to its own name, as it was
        for (i, (path, want)) in long.iter().enumerate() {
            let (note, _) = note_of(path);
            let by = if i == 2 { s.notes.join(path) } else { note.clone() };
            crate::offload::back(&s.c, &by).unwrap();
            assert_eq!(std::fs::read(s.notes.join(path)).unwrap(), *want, "{path}: back");
            assert!(!note.exists());
        }
        crate::offload::back(&s.c, &s.notes.join(&folder)).unwrap();
        assert_eq!(std::fs::read(s.notes.join(&folder).join("inner.txt")).unwrap(), b"in");
        assert!(!folder_note.exists());
        let after = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((after.files, after.already), (0, stored.files), "brought back: kept already");
    }

    /// Goal (FIN-757): a kept file named as a note is (`<name>.offloaded.md`:
    /// an old agent's own note, or a log), where it was or too deep for
    /// that, has a note `--back` reads, and comes back by its own name,
    /// byte for byte. Back, it is the home's file, not a note of this
    /// import: a run again keeps it no more.
    #[test]
    fn a_kept_file_named_as_a_note_comes_back() {
        let deep = format!("work/x/{}", (0..6).map(|i| format!("{i}{}", "d".repeat(49))).collect::<Vec<_>>().join("/"));
        let theirs = Record {
            fragment: "elsewhere--x1y2".into(),
            at: "2026-01-02T030405Z-abcd/home/hermes".into(),
            from: "/home/hermes/old".into(),
            part_bytes: 4,
            dirs: vec![],
            files: vec![Moved { path: "old".into(), size: 3, sha256: Some("a".repeat(64)), crc32: None, mode: 0o644 }],
        };
        let said = offload::Said { title: "Moved to storage", came: "was moved off this disk".into() };
        let theirs = offload::note_text(&theirs, Path::new("/home/hermes/old.offloaded.md"), "old", said).into_bytes();
        let named = [(format!("{deep}/log{NOTE_SUFFIX}"), b"# a log, not a note\n".to_vec()), (format!("old{NOTE_SUFFIX}"), theirs)];
        let mut files = home();
        files.extend(named.iter().cloned());
        let s = setup(&files);
        let stored = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        // back by where it was (Codex on #1561), and by its note
        for (i, (path, want)) in named.iter().enumerate() {
            let note = s.notes.join(format!("{path}{NOTE_SUFFIX}"));
            assert!(note.is_file(), "its note: {}", note.display());
            crate::offload::back(&s.c, &if i == 0 { s.notes.join(path) } else { note.clone() }).unwrap();
            assert_eq!(std::fs::read(s.notes.join(path)).unwrap(), *want, "{path}: back by its own name");
            assert!(!note.exists());
        }
        let after = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap().stored.unwrap();
        assert_eq!((after.files, after.already), (0, stored.files), "brought back: the home's own files, kept already");
    }

    /// Invalid: a deep file with no place a note can name, its own name
    /// past what a file system keeps (no home on one held it), is refused
    /// before anything is written: never kept without a note.
    #[test]
    fn a_file_with_no_place_in_storage_is_refused_before_anything_is_written() {
        let deep = format!("work/x/{}", (0..6).map(|i| format!("{i}{}", "d".repeat(49))).collect::<Vec<_>>().join("/"));
        let mut files = home();
        files.push((format!("{deep}/{}", "n".repeat(offload::NAME_MAX + 1)), b"x".to_vec()));
        let s = setup(&files);
        let e = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("its name is longer than a file's can be (255 bytes)") && e.contains("Nothing was written"), "{e}");
        assert!(s.mock.paths(STORE, "main").is_empty() && notes_under(&s.notes, &|_| false).unwrap().is_empty());
    }

    /// Invalid: a file whose note would be at a note's place already (a
    /// name crafted as another's named short, or a note there) is refused
    /// before anything is written, never after its parts.
    #[test]
    fn a_note_at_a_notes_place_is_refused_before_anything_is_written() {
        let long = "n".repeat(250);
        let crafted = offload::note_name(&long).strip_suffix(NOTE_SUFFIX).expect("a note's name").to_string();
        let mut files = home();
        files.push((long, b"long".to_vec()));
        files.push((crafted, b"crafted".to_vec()));
        let s = setup(&files);
        let head = s.mock.branch(STORE, "main");
        let e = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("a note's place already: nothing was written"), "{e}");
        assert_eq!(s.mock.branch(STORE, "main"), head);
        assert!(notes_under(&s.notes, &|_| false).unwrap().is_empty());
    }

    /// Goal: each group is a note's worth: a folder within the bounds is
    /// one note; past them, its folders and files each their own (a file
    /// past the bytes alone), every file in exactly one group.
    #[test]
    fn groups_are_bounded_notes() {
        let k = |path: &str, size: u64| Keep { path: path.into(), size, crc32: 0, mode: 0o644 };
        let gb = NOTE_BYTES_MAX;
        let keep = vec![
            k("agent/a", 10), k("agent/b/c", 10),
            k("work/videos/one.mp4", gb + 1), k("work/videos/thumb.png", 10),
            k("work/data/x.csv", gb / 2), k("work/data/y.csv", gb / 2),
            k("top.txt", 1),
        ];
        let made = groups(keep.clone(), 256 * MIB as u64, "home");
        let keys: Vec<(&str, bool, usize)> = made.iter().map(|g| (g.key.as_str(), g.folder, g.files.len())).collect();
        assert_eq!(keys, [("agent", true, 2), ("top.txt", false, 1), ("work/data", true, 2), ("work/videos/one.mp4", false, 1), ("work/videos/thumb.png", false, 1)]);
        assert_eq!(made.iter().map(|g| g.files.len()).sum::<usize>(), keep.len());
        let many: Vec<Keep> = (0..NOTE_FILES_MAX + 1).map(|i| k(&format!("deps/p{}/f{i}", i % 2), 1)).collect();
        let keys: Vec<String> = groups(many, MIB as u64, "home").into_iter().map(|g| g.key).collect();
        assert_eq!(keys, ["deps/p0", "deps/p1"], "a folder past the count: its folders");
        // past a note's folders (Codex on #1389): 5,001 files in 10,003 folders
        let deep: Vec<Keep> = (0..5_001).map(|i| k(&format!("deps/a{i}/b/f"), 1)).collect();
        let made = groups(deep, MIB as u64, "home");
        assert!(made.len() == 5_001 && made.iter().all(|g| g.folder && g.key.starts_with("deps/a")), "a folder past the folders: its own");
        // too deep for its own place (FIN-755): alone, never in its folder's
        let far = format!("run/{}/x.log", "d".repeat(280));
        let made = groups(vec![k("run/a.txt", 1), k(&far, 1)], MIB as u64, "home");
        let keys: Vec<(&str, bool)> = made.iter().map(|g| (g.key.as_str(), g.folder)).collect();
        assert_eq!(keys, [("run", true), (far.as_str(), false)]);
        assert_eq!(made[0].files.len(), 1);
        // its place: where it was if that fits (a home's own `.long` file too), else short,
        // else shorter (a long name, under any notes folder's name), else none
        let alone = |key: &str| Group { key: key.into(), folder: false, files: vec![k(key, 1)] };
        assert_eq!(alone("run/a.txt").at("home").as_deref(), Some("home/run"));
        assert_eq!(alone(&format!("{LONG}/0123/v.bin")).at("home").as_deref(), Some(&*format!("home/{LONG}/0123")));
        assert_eq!(alone(&far).at("home"), Some(format!("home/{LONG}/{}", &blob::sha256_hex(far.as_bytes())[..LONG_HEX])));
        // under `home`, a name of 242 bytes is the longest the short place takes
        let edge = |n: usize| alone(&format!("run/{}/{}", "d".repeat(280), "n".repeat(n))).at("home").map(|at| at.starts_with(&format!("home/{LONG}/")));
        assert_eq!((edge(242), edge(243)), (Some(true), Some(false)));
        let named = format!("run/{}/{}", "d".repeat(280), "n".repeat(offload::NAME_MAX));
        assert_eq!(alone(&named).at("home"), Some(format!("{LONG}/{}", &blob::sha256_hex(format!("home/{named}").as_bytes())[..SHORT_HEX])));
        let root = "h".repeat(200);
        assert_eq!(alone(&named).at(&root), Some(format!("{LONG}/{}", &blob::sha256_hex(format!("{root}/{named}").as_bytes())[..SHORT_HEX])));
        assert_ne!(alone(&named).at("home"), alone(&named).at("home-2"), "each notes folder its own");
        assert_eq!(alone(&format!("run/{}/{}", "d".repeat(280), "n".repeat(offload::NAME_MAX + 1))).at("home"), None);
    }
}
