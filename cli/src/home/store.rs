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
//! **Proven before its note.** A group's parts are committed, then found
//! at main, each the size it was written, and each blob in the store. Only
//! then is its note written (whole, never over another: offload's
//! `publish`). Cut off, a run goes on: the notes say what is kept (and a
//! file brought back beside its note's place, what was), and a group with
//! no note yet is made again: the same paths, the same blobs.
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

/// Where kept files go in the storage fragment: the old home, as it was.
pub const PREFIX: &str = "home";
/// The files one note keeps, at most: what `fragment offload` moves at once.
pub const NOTE_FILES_MAX: usize = offload::FILES_MAX;
/// The bytes one folder's note keeps, at most: a fifth of the room a
/// computer's owner is told it is short of (`DISK_TOLD_FREE_MAX`), so any
/// note comes back where there is room. A file past it has a note alone.
pub const NOTE_BYTES_MAX: u64 = fragment_core::computer::DISK_TOLD_FREE_MAX / 5;
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
const _: () = assert!(NOTE_BYTES_MAX > 0 && NOTE_BYTES_MAX < fragment_core::computer::DISK_TOLD_FREE_MAX, "a note comes back where there is room");

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
    /// Its note's place, under `notes`.
    fn note(&self, notes: &Path) -> PathBuf {
        notes.join(format!("{}{NOTE_SUFFIX}", self.key))
    }

    /// Its parent folder in the home ("" at the top).
    fn parent(&self) -> &str {
        self.key.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// Its record, its files' sums as the zip names them.
    fn record(&self, fragment: &str, notes: &Path, part_bytes: u64) -> Record {
        let parent = self.parent();
        let rel = |p: &str| if parent.is_empty() { p.to_string() } else { p[parent.len() + 1..].to_string() };
        let at = if parent.is_empty() { PREFIX.to_string() } else { format!("{PREFIX}/{parent}") };
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
/// bounds, else the file alone. Iterative, in path order.
fn groups(keep: Vec<Keep>, part_bytes: u64) -> Vec<Group> {
    assert!(part_bytes > 0, "a part holds bytes");
    let mut sums: HashMap<&str, (usize, u64, u64)> = HashMap::new();
    for k in &keep {
        let parts = k.size.div_ceil(part_bytes).max(1);
        for (i, _) in k.path.match_indices('/') {
            let s = sums.entry(&k.path[..i]).or_default();
            *s = (s.0 + 1, s.1 + k.size, s.2 + parts);
        }
    }
    let fits = |(files, bytes, parts): (usize, u64, u64)| files <= NOTE_FILES_MAX && bytes <= NOTE_BYTES_MAX && parts <= offload::PARTS_MAX;
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
    let out: Vec<Group> = out.into_values().collect();
    assert!(out.iter().all(|g| g.files.len() <= NOTE_FILES_MAX && (g.folder || g.files.len() == 1)), "each group is a note's worth");
    out
}

/// Each note under `notes` (iterative), by its path, read: what it says is
/// kept. A note that is not one a move or an import writes is refused.
fn notes_under(notes: &Path) -> Result<Vec<(PathBuf, Record)>> {
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
                let text = std::fs::read_to_string(e.path()).with_context(|| format!("reading {}", e.path().display()))?;
                let r = offload::record_of(&text).map_err(|why| coded(Code::Conflict, format!("{} is not a note this import or `fragment offload` wrote ({why:#}): move it aside, then run this again", e.path().display())))?;
                out.push((e.path(), r));
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

    // what is kept already: a note names it, or it was brought back from one
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for (note, r) in notes_under(&notes)? {
        let dir = note.parent().expect("a note has a folder").strip_prefix(&notes).expect("found under the notes").to_string_lossy().to_string();
        covered.extend(r.files.iter().map(|f| if dir.is_empty() { f.path.clone() } else { format!("{dir}/{}", f.path) }));
    }
    // a path that is a file and a folder of the home both is no home a
    // filesystem held: its notes could not say which comes back
    let folders: BTreeSet<&str> = src.entries().iter().flat_map(|e| e.path.match_indices('/').map(|(i, _)| &e.path[..i])).collect();
    if let Some(both) = src.entries().iter().find(|e| folders.contains(e.path.as_str())) {
        bail!(coded(Code::InvalidRequest, format!("{} is a file and a folder of this home both: it is not kept", both.path)));
    }
    let (mut todo, mut already, mut already_bytes) = (Vec::new(), 0, 0u64);
    for path in keep {
        let size = src.get(&path).ok_or_else(|| anyhow!("{path}: no such file"))?.size;
        let (crc32, mode) = src.sums(&path)?.ok_or_else(|| anyhow!("{path}: its zip names no CRC-32"))?;
        if covered.contains(&path) || brought_back(&notes.join(&path), size, crc32) {
            (already, already_bytes) = (already + 1, already_bytes + size);
            continue;
        }
        todo.push(Keep { path, size, crc32, mode: mode.filter(|m| *m != 0).unwrap_or(0o644) });
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
    let groups = groups(todo, part_bytes);
    let total = groups.len();
    for (n, g) in groups.iter().enumerate() {
        let r = g.record(&into, &notes, part_bytes);
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
        for p in &parts {
            RepoPath::parse(&p.3).map_err(|e| coded(Code::InvalidUsage, format!("{} cannot be kept as {e}", p.0.path)))?;
        }
        // its parts, a window at a time: the blobs copied (or uploaded),
        // the small ones read, then committed
        let mut kept: Vec<(String, u64, Kept)> = Vec::with_capacity(parts.len());
        let (mut batch, mut batch_bytes) = (Vec::new(), 0usize);
        let message = format!("home store: {}", g.key);
        for window in parts.chunks(WINDOW) {
            let made = part_bytes_of(src, &blobs, &into, window)?;
            for ((_, _, _, path, len), (bytes, k)) in window.iter().zip(made) {
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
        // proven: each part at main, the size it was written, each blob in the store
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
        let name = g.key.rsplit('/').next().expect("a key names something");
        let when = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
        let said = Said { title: "Kept in storage", came: format!("came from the old agent's home in its move here on {when}, and was kept off this disk") };
        offload::publish(&note, &offload::note_text(&r, &note, name, said))?;
        assert!(offload::record_of(&std::fs::read_to_string(&note)?).is_ok_and(|back| back == r), "a note reads back as written");
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

/// Each part of `window`, as git keeps it: a blob's copied on the platform
/// from a stage (`COPIES_AT_ONCE` at once) or uploaded from a zip here, its
/// pointer; a small one read, its bytes.
fn part_bytes_of(src: &Source, blobs: &Blobs, into: &str, window: &[(&Keep, &Moved, u64, String, u64)]) -> Result<Vec<(Vec<u8>, Kept)>> {
    let mut out: Vec<Option<(Vec<u8>, Kept)>> = (0..window.len()).map(|_| None).collect();
    let mut copies = Vec::new();
    for (i, (k, _, offset, _, len)) in window.iter().enumerate() {
        match (blobs.keeps(*len)?, src.stage_id().is_some(), src.span(&k.path)?) {
            (true, true, Some((start, size))) => {
                assert!(size == k.size && offset + len <= size, "a part is within its file");
                copies.push((i, start + offset, *len));
            }
            (true, _, _) => {
                let stored = blobs.store(src.read_range(&k.path, *offset, *len)?)?;
                let sha = blob::parse(&stored).map(|p| p.sha256).ok_or_else(|| anyhow!("{}: kept as a blob, but no pointer came back", k.path))?;
                out[i] = Some((stored, Kept::Blob(sha)));
            }
            (false, _, _) => out[i] = Some((src.read_range(&k.path, *offset, *len)?, Kept::Bytes)),
        }
    }
    for chunk in copies.chunks(COPIES_AT_ONCE) {
        let copied: Vec<Result<String>> = std::thread::scope(|s| {
            let running: Vec<_> = chunk.iter().map(|(_, at, len)| s.spawn(move || src.copy(into, *at, *len))).collect();
            running.into_iter().map(|t| t.join().unwrap_or_else(|_| Err(anyhow!("a copy's thread panicked")))).collect()
        });
        for ((i, _, len), sha) in chunk.iter().zip(copied) {
            let sha = sha.with_context(|| format!("copying {}", window[*i].0.path))?;
            out[*i] = Some((blob::pointer(&sha, *len).into_bytes(), Kept::Blob(sha)));
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
        let notes = notes_under(&s.notes).unwrap();
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
        let cut: usize = notes_under(&s.notes).unwrap().iter().map(|(_, r)| r.files.len()).sum();
        let b = bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let stored = b.stored.unwrap();
        assert_eq!(stored.already, cut, "what was proven before is kept already");
        assert_eq!(b.files + b.already + stored.files + stored.already, files.len() - 1);
        for (path, want) in files.iter().filter(|(p, _)| !p.starts_with("work/notes") && !p.starts_with("work/photos") && !p.ends_with(".npmrc")) {
            assert_eq!(kept(&s.mock, STORE, &format!("home/{path}")).as_ref(), Some(want), "{path}");
        }
        let named: usize = notes_under(&s.notes).unwrap().iter().map(|(_, r)| r.files.len()).sum();
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
        let fragments: BTreeSet<String> = notes_under(&s.notes).unwrap().into_iter().map(|(_, r)| r.fragment).collect();
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
        assert!(s.mock.paths(STORE, "main").is_empty() && notes_under(&s.notes).unwrap().is_empty());
        let folder = Source::open(&s.notes).unwrap();
        let e = keep(&s.c, &folder, vec![], STORE, &s.notes).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("zip the folder first"), "{e}");
    }

    /// Goal: a copy whose blob is not what was kept never comes back as
    /// the file: `--back` refuses it by its CRC-32, and the note stays.
    #[test]
    fn a_kept_file_that_changed_in_storage_is_not_brought_back() {
        let s = setup(&home());
        bring(&s.c, &s.src, &layout(), FILES, Some(Store { into: STORE, notes: &s.notes })).unwrap();
        let notes = notes_under(&s.notes).unwrap();
        let (note, r) = notes.iter().find(|(_, r)| r.files.iter().any(|f| f.path.ends_with("w.gguf"))).expect("the weights' note");
        // its first part of two (3 MiB, in parts of 2)
        let at = format!("{}/{}.part-0001-of-0002", r.at, r.files.iter().find(|f| f.path.ends_with("w.gguf")).unwrap().path);
        let ptr = blob::parse(&s.mock.file_at(STORE, "main", &at).unwrap()).unwrap();
        s.mock.corrupt_blob(&ptr.sha256, true);
        let e = crate::offload::back(&s.c, note).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("not those stored") || e.contains("other bytes"), "{e}");
        assert!(note.exists(), "the note stays");
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
        let made = groups(keep.clone(), 256 * MIB as u64);
        let keys: Vec<(&str, bool, usize)> = made.iter().map(|g| (g.key.as_str(), g.folder, g.files.len())).collect();
        assert_eq!(keys, [("agent", true, 2), ("top.txt", false, 1), ("work/data", true, 2), ("work/videos/one.mp4", false, 1), ("work/videos/thumb.png", false, 1)]);
        assert_eq!(made.iter().map(|g| g.files.len()).sum::<usize>(), keep.len());
        let many: Vec<Keep> = (0..NOTE_FILES_MAX + 1).map(|i| k(&format!("deps/p{}/f{i}", i % 2), 1)).collect();
        let keys: Vec<String> = groups(many, MIB as u64).into_iter().map(|g| g.key).collect();
        assert_eq!(keys, ["deps/p0", "deps/p1"], "a folder past the count: its folders");
    }
}
