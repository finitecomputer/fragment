//! `fragment offload`: a file or a folder moved off this disk into a
//! members-only fragment of files, and back (`--back`). A computer's disk
//! holds 20 GB, and a full one fails its saves (crates/core/src/computer/
//! disk.rs); this is its agent's way to make room without losing a file.
//!
//! **Nothing is removed before its copy is proven.** Each file is read in
//! parts of at most one upload (`upload_max`: a blob's most, from a
//! computer), each kept as git keeps a file (1 MiB and up a blob, its
//! pointer in git), and committed under a folder of its own in the
//! fragment, `<when>/<where it was>`, so no move ever writes over another.
//! Each is then read back at that commit, its blob's bytes too, and hashed.
//! Only once every file read back as it was read, and is unchanged on disk
//! since, is a note written in its place (`<name>.offloaded.md`: where it
//! went, how to bring it back, and each file's size and sha256), and only
//! then are the files removed.
//!
//! **A move's import keeps files the same way** (`home::store`): what an
//! old agent's home holds past its files' pick goes from its stage into
//! storage, never through this disk, with a note where each was. Its note
//! names each file's CRC-32 (the zip's, made from the old file itself)
//! where it knows no sha256 (a file of several parts, copied on the
//! platform part by part), and `--back` reads it back the same way.
//!
//! **Back** brings each file from main into a hidden file beside where it
//! goes, checks it against the note's sha256, and links it into place,
//! never over a file there (one with the note's bytes is back already).
//! The note goes last. The fragment keeps its copy: a computer that goes
//! back to an earlier save (docs/computers.md) can come back with the note
//! and without the file, and the note must still lead to it.
//!
//! **One at a time, never over a file.** A move or a back holds its
//! folder's lock (`flock` on the folder) throughout, so a back never runs
//! between a move's note and its removal. Every file it writes is made
//! new under a random hidden name (`create_new`, private) and linked into
//! place, so neither a note nor a file brought back ever replaces one, and
//! each file is checked unchanged again just before it is removed.
//!
//! **Replay and restart.** A move cut off before its note leaves its
//! files where they were (and a copy in the fragment, harmless); run again,
//! it moves them under a new folder. One cut off after its note, its files
//! still beside it as the note says, is checked against the fragment again
//! and then removed. Any other path with a note beside it is refused. A
//! back cut off goes on: what is in place already with its bytes is kept.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Result};
use fragment_core::blob;
use fragment_proto::{FragmentKind, FragmentList, FragmentStatus, RepoPath, Visibility};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::{Client, Code, CodedError};
use crate::blobs::Blobs;
use crate::codestorage::{Author, Change, CsError, Held, Wants, MAIN};
use crate::home::files::{commit, manifest, BATCH_BYTES, BATCH_FILES};
use crate::home::survey::size;

/// The note left in a moved file's or folder's place, after its name.
pub const NOTE_SUFFIX: &str = ".offloaded.md";
/// The files one move takes, at most: a folder of more is moved a
/// subfolder at a time (and one of dependencies or a cache, which its tools
/// make again, is deleted instead).
pub const FILES_MAX: usize = 10_000;
/// The parts one move's files are in, at most (20 GB in parts of 100 MB
/// is 200, and a part a small file): a note naming more is no move's.
pub(crate) const PARTS_MAX: u64 = 100_000;
/// Where the note's record begins and ends.
const RECORD_OPEN: &str = "\n```json\n";
const RECORD_CLOSE: &str = "\n```\n";

fn coded(code: Code, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(CodedError { code, msg: msg.into() })
}

/// What a note records of a move: the fragment and the folder in it its
/// files went to, where they were, and each file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) fragment: String,
    /// The folder in the fragment each file's path is under.
    pub(crate) at: String,
    /// The path moved, as it was then.
    pub(crate) from: String,
    /// Each file's parts are this long, its last one at most.
    pub(crate) part_bytes: u64,
    /// A folder's folders, its own first, each with its permission bits:
    /// made again as they were, the empty ones too. None for a file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) dirs: Vec<Folder>,
    pub(crate) files: Vec<Moved>,
}

/// One folder of a folder moved: its path from the note's folder, and its
/// permission bits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Folder {
    pub(crate) path: String,
    pub(crate) mode: u32,
}

/// One file moved: its path from the note's folder (the name moved first),
/// its size, its bytes' sha256 or their CRC-32 (a move's import, which
/// knows the zip's: at least one), and its permission bits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Moved {
    pub(crate) path: String,
    pub(crate) size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) crc32: Option<u32>,
    pub(crate) mode: u32,
}

impl Moved {
    /// Whether bytes that hash to `sha256` and `crc32` are this file's, by
    /// every sum its record names.
    pub(crate) fn matches(&self, sha256: &str, crc32: u32) -> bool {
        assert!(self.sha256.is_some() || self.crc32.is_some(), "a record names a sum of each file");
        self.sha256.as_deref().is_none_or(|s| s == sha256) && self.crc32.is_none_or(|c| c == crc32)
    }
}

impl Record {
    /// The fragment's paths of `f`'s parts, in order, with each one's
    /// length: the file's own path when it is one part, else
    /// `<path>.part-0001-of-0003` and on.
    pub(crate) fn parts(&self, f: &Moved) -> Vec<(String, u64)> {
        assert!(self.part_bytes > 0, "a part holds bytes");
        let n = f.size.div_ceil(self.part_bytes).max(1);
        (0..n)
            .map(|i| {
                let len = (f.size - i * self.part_bytes).min(self.part_bytes);
                match n {
                    1 => (format!("{}/{}", self.at, f.path), len),
                    _ => (format!("{}/{}.part-{:04}-of-{n:04}", self.at, f.path, i + 1), len),
                }
            })
            .collect()
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// A path quoted for a shell.
fn quoted(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}

/// What a note says of how its files came to storage: its title, and the
/// words after what it moved.
pub(crate) struct Said {
    pub(crate) title: &'static str,
    pub(crate) came: String,
}

impl Said {
    /// Moved off this disk by `fragment offload`, `when`.
    fn offloaded(when: &str) -> Said {
        Said { title: "Moved to storage", came: format!("was moved off this disk on {when}") }
    }
}

/// The note: what was moved, where to, and how to bring it back, in words;
/// then its record, one file a line.
pub(crate) fn note_text(r: &Record, note: &Path, name: &str, said: Said) -> String {
    let what = match r.files.as_slice() {
        [one] if one.path == name => format!("`{name}` ({})", size(one.size)),
        files => format!("`{name}/` ({} files, {})", files.len(), size(r.bytes())),
    };
    let mut out = format!(
        "# {}\n\n{what} {}, in the fragment `{}`, under `{}/`.\n\nTo bring it back here: `fragment offload --back {}`\n\nIts record, which that reads (keep it as it is):\n",
        said.title,
        said.came,
        r.fragment,
        r.at,
        quoted(note)
    );
    let head = serde_json::json!({ "fragment": r.fragment, "at": r.at, "from": r.from, "partBytes": r.part_bytes });
    let head = head.to_string();
    out.push_str(RECORD_OPEN.trim_start_matches('\n'));
    out.push_str(&head[..head.len() - 1]);
    if !r.dirs.is_empty() {
        let dirs: Vec<String> = r.dirs.iter().map(|d| serde_json::to_string(d).expect("a folder's record is JSON")).collect();
        out.push_str(",\"dirs\":[\n");
        out.push_str(&dirs.join(",\n"));
        out.push_str("\n]");
    }
    out.push_str(",\"files\":[\n");
    let lines: Vec<String> = r.files.iter().map(|f| serde_json::to_string(f).expect("a file's record is JSON")).collect();
    out.push_str(&lines.join(",\n"));
    out.push_str("\n]}");
    out.push_str(RECORD_CLOSE);
    out
}

/// A note's record (`note_text`).
pub(crate) fn record_of(text: &str) -> Result<Record> {
    let start = text.rfind(RECORD_OPEN).ok_or_else(|| anyhow!("it holds no record"))? + RECORD_OPEN.len();
    let end = text[start..].find(RECORD_CLOSE).ok_or_else(|| anyhow!("its record has no end"))? + start;
    let r: Record = serde_json::from_str(&text[start..end]).context("its record")?;
    let plain = |p: &str| !p.is_empty() && Path::new(p).components().all(|c| matches!(c, Component::Normal(_)));
    // its longest path in the fragment, a part's, is a repo path
    let in_fragment = |f: &Moved| RepoPath::parse(&format!("{}/{}.part-{PARTS_MAX}-of-{PARTS_MAX}", r.at, f.path)).is_ok();
    let parts = r.files.iter().try_fold(0u64, |n, f| (r.part_bytes > 0).then(|| n.checked_add(f.size.div_ceil(r.part_bytes).max(1))).flatten());
    let fits = parts.is_some_and(|n| n <= PARTS_MAX)
        && (1..=FILES_MAX).contains(&r.files.len())
        && r.dirs.len() <= FILES_MAX
        && r.files.iter().all(|f| f.sha256.as_deref().is_none_or(blob::valid_sha) && (f.sha256.is_some() || f.crc32.is_some()) && f.mode <= 0o777 && plain(&f.path) && in_fragment(f))
        && r.dirs.iter().all(|d| d.mode <= 0o777 && plain(&d.path))
        && r.files.iter().try_fold(0u64, |n, f| n.checked_add(f.size)).is_some();
    if !fits {
        bail!("its record is not one a move writes");
    }
    Ok(r)
}

/// A file to move, as it was read.
#[derive(Debug, Clone)]
struct Local {
    abs: PathBuf,
    rel: String,
    size: u64,
    mode: u32,
    modified: SystemTime,
    /// Its device, inode and change time (unix): a file replaced, or
    /// written with its time put back, is another.
    id: (u64, u64, i64, i64),
}

/// A file's or a folder's permission bits.
fn mode_of(m: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777;
    #[cfg(not(unix))]
    let mode = if m.is_dir() { 0o755 } else { 0o644 };
    mode
}

/// A file's identity beside its size and time (`Local::id`).
fn id_of(m: &fs::Metadata) -> (u64, u64, i64, i64) {
    #[cfg(unix)]
    let id = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino(), m.ctime(), m.ctime_nsec())
    };
    #[cfg(not(unix))]
    let id = (0, 0, 0, 0);
    id
}

fn local(abs: PathBuf, rel: String, m: &fs::Metadata) -> Result<Local> {
    Ok(Local { abs, rel, size: m.len(), mode: mode_of(m), modified: m.modified()?, id: id_of(m) })
}

/// What a path to move is: the folder its note goes in, the name moved,
/// and its files (a folder's, by path; never through a link).
struct Planned {
    dir: PathBuf,
    name: String,
    note: PathBuf,
    files: Vec<Local>,
    dirs: Vec<Folder>,
}

fn name_of(p: &Path) -> Result<String> {
    p.file_name().and_then(|n| n.to_str()).map(str::to_string).ok_or_else(|| coded(Code::InvalidUsage, format!("{} names no file or folder by a UTF-8 name", p.display())))
}

/// Its folder and name, its links resolved above it but never its own.
fn place(path: &Path) -> Result<(PathBuf, String)> {
    let abs = std::path::absolute(path).with_context(|| format!("finding {}", path.display()))?;
    let name = name_of(&abs)?;
    let dir = abs.parent().ok_or_else(|| coded(Code::InvalidUsage, format!("{} is no file or folder to move", abs.display())))?;
    let dir = dir.canonicalize().with_context(|| format!("finding {}", dir.display()))?;
    Ok((dir, name))
}

/// The files under `dir` (a folder being moved), each by its path from
/// `root`'s folder; refused whole when one is a link or not a file, or
/// past `FILES_MAX`.
fn walk(dir: &Path, rel: &str, out: &mut Vec<Local>, dirs: &mut Vec<Folder>) -> Result<()> {
    dirs.push(Folder { path: rel.to_string(), mode: mode_of(&fs::symlink_metadata(dir)?) });
    if dirs.len() > FILES_MAX {
        bail!(coded(Code::InvalidUsage, format!("{} holds more than {FILES_MAX} folders: move them one at a time", dir.display())));
    }
    let mut entries: Vec<_> = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = name_of(&e.path())?;
        let (abs, rel) = (e.path(), format!("{rel}/{name}"));
        let m = fs::symlink_metadata(&abs)?;
        if m.is_dir() {
            walk(&abs, &rel, out, dirs)?;
        } else if m.is_file() {
            if crate::home::rules::secret_name(&name) {
                bail!(coded(Code::InvalidUsage, format!("{} may hold a password or key: a folder holding one is not moved (move the files beside it instead)", abs.display())));
            }
            out.push(local(abs, rel, &m)?);
            if out.len() > FILES_MAX {
                bail!(coded(Code::InvalidUsage, format!("{} holds more than {FILES_MAX} files: move its folders one at a time (a folder of dependencies or a cache, which its tools make again, is deleted instead)", dir.display())));
            }
        } else {
            bail!(coded(Code::InvalidUsage, format!("{} is a link or not a file: a folder holding one is not moved (move the files beside it instead)", abs.display())));
        }
    }
    Ok(())
}

/// The runtime's own home, which nothing moves out of: Hermes' (the
/// image sets `HERMES_HOME`), whose memory, skills, sessions and settings
/// its agents need to run.
fn runtime_home() -> Option<PathBuf> {
    std::env::var_os("HERMES_HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok())
}

/// What `path` is to move, refused when it is a note, inside the runtime's
/// home (`keep`) or holding it, or not a file or a folder of files.
fn plan(path: &Path, keep: Option<&Path>) -> Result<Planned> {
    let (dir, name) = place(path)?;
    if name.ends_with(NOTE_SUFFIX) {
        bail!(coded(Code::InvalidUsage, format!("{name} is the note of a move: bring it back with --back")));
    }
    let abs = dir.join(&name);
    if let Some(keep) = keep.filter(|k| abs.starts_with(k) || k.starts_with(&abs)) {
        bail!(coded(Code::InvalidUsage, format!("{} is in your runtime's own home ({}), or holds it: what it keeps there (memory, skills, sessions, settings) is never moved", abs.display(), keep.display())));
    }
    let note = dir.join(format!("{name}{NOTE_SUFFIX}"));
    let m = match fs::symlink_metadata(&abs) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && note.exists() => bail!(coded(Code::InvalidUsage, format!("{} was moved already: its note is {}", abs.display(), note.display()))),
        m => m.with_context(|| format!("reading {}", abs.display()))?,
    };
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    if m.is_file() {
        if crate::home::rules::secret_name(&name) {
            bail!(coded(Code::InvalidUsage, format!("{} may hold a password or key: it is not moved", abs.display())));
        }
        files.push(local(abs.clone(), name.clone(), &m)?);
    } else if m.is_dir() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        crate::sync::refuse_dir(&abs, home.as_deref()).map_err(|e| coded(Code::InvalidUsage, e.to_string().replace("refusing to sync", "refusing to move it").replace("Point --dir at the folder you mean, by its full path", "Name the folder you mean, by its full path")))?;
        walk(&abs, &name, &mut files, &mut dirs)?;
        if files.is_empty() {
            bail!(coded(Code::InvalidUsage, format!("{} holds no files: nothing to move", abs.display())));
        }
    } else {
        bail!(coded(Code::InvalidUsage, format!("{} is a link or not a file: move what it points at by its own path", abs.display())));
    }
    Ok(Planned { dir, name, note, files, dirs })
}

/// The folder in the fragment a move made now goes under: when, then a
/// few random letters (two moves in a second never share one), then where
/// it was.
fn at_now(dir: &Path) -> (String, String) {
    let now = jiff::Timestamp::now();
    let stamp = format!("{}-{:04x}", now.strftime("%Y-%m-%dT%H%M%SZ"), rand::random::<u16>());
    let under = dir.to_string_lossy().trim_matches('/').to_string();
    let at = if under.is_empty() { stamp } else { format!("{stamp}/{under}") };
    (at, now.strftime("%Y-%m-%d at %H:%M UTC").to_string())
}

/// A file's bytes hashed as they pass: its part's and its whole's, and
/// written on to `out` when there is one.
struct Hashing<'a> {
    part: Sha256,
    whole: &'a mut Sha256,
    crc: &'a mut crc32fast::Hasher,
    n: u64,
    out: Option<&'a mut File>,
}

impl Write for Hashing<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(out) = self.out.as_mut() {
            out.write_all(buf)?;
        }
        self.part.update(buf);
        self.whole.update(buf);
        self.crc.update(buf);
        self.n += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.as_mut().map_or(Ok(()), |o| o.flush())
    }
}

/// Reads `f` back from `rev` of the fragment, each part as git holds it
/// (its blob's bytes from the store), into `out` when given: an error
/// unless every part and the whole hash as the record says.
pub(crate) fn read_back(c: &Client, held: &mut Held, r: &Record, rev: &str, f: &Moved, mut out: Option<&mut File>) -> Result<()> {
    let (mut whole, mut crc) = (Sha256::new(), crc32fast::Hasher::new());
    for (path, len) in r.parts(f) {
        let git = match held.get(c)?.read_file(&path, rev) {
            Ok(git) => git,
            Err(CsError::NotFound(_)) => bail!(coded(Code::NotFound, format!("{} has no {path} at {rev}: its copy is not there", r.fragment))),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {path} back"))),
        };
        let mut h = Hashing { part: Sha256::new(), whole: &mut whole, crc: &mut crc, n: 0, out: out.as_deref_mut() };
        match blob::parse(&git) {
            Some(p) if p.size == len => {
                c.download_into(&format!("/api/f/{}/blobs/{}", r.fragment, p.sha256), len, || Ok(&mut h)).with_context(|| format!("reading {path}'s bytes back"))?;
                if h.n != len || hex::encode(h.part.clone().finalize()) != p.sha256 {
                    bail!("{path}'s bytes read back are not those stored ({} of {len} bytes)", h.n);
                }
            }
            Some(_) => bail!("{path} in {} names a blob of another size than the part it holds", r.fragment),
            None => h.write_all(&git)?,
        }
        if h.n != len {
            bail!("{path} reads back as {} bytes, not the {len} moved", h.n);
        }
    }
    if !f.matches(&hex::encode(whole.finalize()), crc.finalize()) {
        bail!("{} reads back from {} as other bytes than were moved", f.path, r.fragment);
    }
    Ok(())
}

/// What `offload` did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offloaded {
    pub fragment: String,
    pub at: String,
    pub note: String,
    pub files: usize,
    pub bytes: u64,
    /// A move cut off after its note, whose files were removed now.
    pub resumed: bool,
    /// Files and folders left in place: a file that changed after its copy
    /// was made, a folder something new is in.
    pub kept: Vec<String>,
}

/// A file unchanged since it was read: the same file (`Local::id`), its
/// size and its time.
fn unchanged(l: &Local) -> bool {
    fs::symlink_metadata(&l.abs).is_ok_and(|m| m.is_file() && m.len() == l.size && m.modified().ok() == Some(l.modified) && id_of(&m) == l.id)
}

/// A file's sha256 (hex) and CRC-32, as `Moved::matches` takes them.
fn sums_of(path: &Path) -> Result<(String, u32)> {
    struct Both(Sha256, crc32fast::Hasher);
    impl Write for Both {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.update(buf);
            self.1.update(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut h = Both(Sha256::new(), crc32fast::Hasher::new());
    std::io::copy(&mut File::open(path).with_context(|| format!("reading {}", path.display()))?, &mut h)?;
    Ok((hex::encode(h.0.finalize()), h.1.finalize()))
}

/// A new file in `dir` under a random hidden name, made exclusively and
/// private: never one that was there, nor through a link.
fn fresh(dir: &Path) -> Result<(PathBuf, File)> {
    loop {
        let path = dir.join(format!(".offload-{:016x}", rand::random::<u64>()));
        let mut open = fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
        match open.open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(anyhow::Error::new(e).context(format!("writing in {}", dir.display()))),
        }
    }
}

/// Writes `text` as the note at `path` whole or not at all: into a fresh
/// file, synced, then linked into place (never over a note there), and its
/// folder synced. A disk too full for it keeps no part of it.
pub(crate) fn publish(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().expect("a note has a folder");
    let (tmp, mut f) = fresh(dir).with_context(|| format!("writing {}; nothing was removed", path.display()))?;
    let wrote = f.write_all(text.as_bytes()).and_then(|()| f.sync_all()).and_then(|()| fs::hard_link(&tmp, path));
    fs::remove_file(&tmp).ok();
    wrote.with_context(|| format!("writing {}; nothing was removed (a disk with no room for it needs a little room made first)", path.display()))?;
    sync_dir(dir)
}

/// The lock of the folder a move or a back works in, held while it works:
/// one at a time (another is refused at once, never waited on).
fn lock(dir: &Path) -> Result<File> {
    let f = File::open(dir).with_context(|| format!("opening {}", dir.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(fs::TryLockError::WouldBlock) => bail!(coded(Code::Conflict, format!("another fragment offload is working in {}: run this again once it is done", dir.display()))),
        Err(fs::TryLockError::Error(e)) => Err(anyhow::Error::new(e).context(format!("locking {}", dir.display()))),
    }
}

pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir).and_then(|d| d.sync_all()).with_context(|| format!("syncing {}", dir.display()))
}

/// Removes the planned files, then their folders from the deepest, each
/// only once empty: the folders that kept something new are named.
fn remove(p: &Planned) -> Result<Vec<String>> {
    let mut kept = Vec::new();
    for l in &p.files {
        // changed since it was read (and its copy made): it stays
        if !unchanged(l) {
            kept.push(l.abs.display().to_string());
            continue;
        }
        fs::remove_file(&l.abs).with_context(|| format!("removing {} (its copy is in place)", l.abs.display()))?;
    }
    // the folders it read, the deepest first (a child sorts after its
    // parent), each only once empty: one made since stays, and so does
    // the folder holding it
    let mut dirs: Vec<&str> = p.dirs.iter().map(|d| d.path.as_str()).collect();
    dirs.sort_unstable();
    for d in dirs.iter().rev().map(|d| p.dir.join(d)) {
        if fs::remove_dir(&d).is_err() && d.exists() {
            kept.push(d.display().to_string());
        }
    }
    sync_dir(&p.dir)?;
    Ok(kept)
}

/// The fragment a move goes into: one the caller edits, an app's (never
/// a chat, an agent, a brain, skills or a Session Archive), members-only.
pub(crate) fn check_into(c: &Client, into: &str) -> Result<String> {
    let name = c.fragment(into)?;
    let s: FragmentStatus = c.call_as(c.get(&format!("/api/f/{name}/status"))?)?;
    if s.kind != FragmentKind::App {
        bail!(coded(Code::InvalidUsage, format!("{name} is a {}, not a fragment of files: move files into one made for them (fragment create <name> --visibility members)", s.kind.as_str())));
    }
    if s.visibility != Visibility::Members {
        bail!(coded(Code::Forbidden, format!("{name} is {}: files are moved only into a members-only fragment, so only its members can read them", s.visibility.as_str())));
    }
    Ok(name)
}

/// Moves `path` (a file, or a folder and all in it) into `into`.
pub fn offload(c: &Client, path: &Path, into: &str) -> Result<Offloaded> {
    let p = plan(path, runtime_home().as_deref())?;
    let _held = lock(&p.dir)?;
    if fs::symlink_metadata(&p.note).is_ok() {
        return resume(c, &p);
    }
    let into = check_into(c, into)?;
    let list: FragmentList = c.call_as(c.get("/api/fragments")?)?;
    let part_bytes = crate::session::upload_max(&list, c.agent().is_some());
    let (at, when) = at_now(&p.dir);
    let mut r = Record { fragment: into.clone(), at, from: p.dir.join(&p.name).display().to_string(), part_bytes, dirs: p.dirs.clone(), files: Vec::new() };
    if p.files.iter().map(|l| l.size.div_ceil(part_bytes).max(1)).sum::<u64>() > PARTS_MAX {
        bail!(coded(Code::TooLarge, format!("{} is too large to move in parts of {}: move less at once", r.from, size(part_bytes))));
    }
    // every path it writes is a repo path, and its own
    let mut paths = std::collections::BTreeSet::new();
    for l in &p.files {
        let probe = Moved { path: l.rel.clone(), size: l.size, sha256: None, crc32: None, mode: l.mode };
        for (path, _) in r.parts(&probe) {
            RepoPath::parse(&path).map_err(|e| coded(Code::InvalidUsage, format!("{} cannot go into a fragment as {e}", l.abs.display())))?;
            if !paths.insert(path.clone()) {
                bail!(coded(Code::InvalidUsage, format!("two files here would both be {path} in {into}: move them one at a time")));
            }
        }
    }

    let mut held = Held::new(&into, Wants::Write);
    if !held.get(c)?.writes() {
        bail!(coded(Code::Forbidden, format!("you can read {into} but not write it: move files into a fragment you edit")));
    }
    let blobs = Blobs::new(c, &into);
    let author = Author::writer(&c.writer_id());
    let mut head = held.get(c)?.branch_head(MAIN)?;
    let (mut batch, mut batch_bytes) = (Vec::new(), 0usize);
    let message = format!("offload: {}", r.from);
    for l in &p.files {
        let mut file = File::open(&l.abs).with_context(|| format!("reading {}", l.abs.display()))?;
        let mut whole = Sha256::new();
        let probe = Moved { path: l.rel.clone(), size: l.size, sha256: None, crc32: None, mode: l.mode };
        for (path, len) in r.parts(&probe) {
            let mut bytes = vec![0u8; len as usize];
            file.read_exact(&mut bytes).with_context(|| format!("reading {}: it changed while it was read; nothing was removed", l.abs.display()))?;
            whole.update(&bytes);
            let stored = blobs.store(bytes).with_context(|| format!("storing {} in {into}; nothing was removed", l.abs.display()))?;
            batch_bytes += stored.len();
            batch.push(Change::Upsert { path: RepoPath::parse(&path).expect("checked above"), bytes: stored });
            if batch.len() >= BATCH_FILES || batch_bytes >= BATCH_BYTES {
                head = commit(c, &mut held, &author, head, &message, &batch)?;
                (batch, batch_bytes) = (Vec::new(), 0);
            }
        }
        if file.read(&mut [0u8; 1])? != 0 {
            bail!(coded(Code::Conflict, format!("{} grew while it was read; nothing was removed: move it once it is done changing", l.abs.display())));
        }
        r.files.push(Moved { path: l.rel.clone(), size: l.size, sha256: Some(hex::encode(whole.finalize())), crc32: None, mode: l.mode });
    }
    if !batch.is_empty() {
        head = commit(c, &mut held, &author, head, &message, &batch)?;
    }
    let head = head.ok_or_else(|| anyhow!("{into}'s main has no head after the move's commit"))?;
    manifest(c, &mut held, &author, head)?;
    crate::sync::refresh_pins(c, &into, MAIN);

    // proven: each file read back from main as it is now, which a back reads
    let mut read = Held::new(&into, Wants::Read);
    let main = read.get(c)?.branch_head(MAIN)?.ok_or_else(|| anyhow!("{into}'s main has no head after the move's commit"))?;
    for f in &r.files {
        read_back(c, &mut read, &r, &main, f, None).with_context(|| format!("checking {}'s copy; nothing was removed", f.path))?;
    }
    if let Some(l) = p.files.iter().find(|l| !unchanged(l)) {
        bail!(coded(Code::Conflict, format!("{} changed while it was moved; nothing was removed (a copy of it as it was is in {into}, under {}/): move it once it is done changing", l.abs.display(), r.at)));
    }
    publish(&p.note, &note_text(&r, &p.note, &p.name, Said::offloaded(&when)))?;
    let kept = remove(&p)?;
    Ok(Offloaded { fragment: into, at: r.at.clone(), note: p.note.display().to_string(), files: r.files.len(), bytes: r.bytes(), resumed: false, kept })
}

/// A path with a note beside it: a move cut off after its note, when its
/// files here are the note's, each as it says; its copy is read back
/// again at main (it may have gone since), then its files removed.
fn resume(c: &Client, p: &Planned) -> Result<Offloaded> {
    let text = fs::read_to_string(&p.note).with_context(|| format!("reading {}", p.note.display()))?;
    let r = record_of(&text).with_context(|| format!("reading {}", p.note.display()))?;
    // each file here is one the note moved, as it moved it (a removal cut
    // off may have removed the rest)
    let moved: std::collections::BTreeMap<&str, &Moved> = r.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let same = p.files.iter().all(|l| moved.get(l.rel.as_str()).is_some_and(|f| f.size == l.size && sums_of(&l.abs).is_ok_and(|(s, c)| f.matches(&s, c))));
    if !same {
        bail!(coded(
            Code::Conflict,
            format!("{} is there already: {} was moved before, and what is here now is not what it moved. Bring that back first (fragment offload --back {}), or move this one aside", p.note.display(), p.name, quoted(&p.note))
        ));
    }
    let mut held = Held::new(&r.fragment, Wants::Read);
    let head = held.get(c)?.branch_head(MAIN)?.ok_or_else(|| coded(Code::NotFound, format!("{} has no files: its copy is gone; nothing was removed", r.fragment)))?;
    for l in &p.files {
        let f = moved[l.rel.as_str()];
        read_back(c, &mut held, &r, &head, f, None).with_context(|| format!("checking {}'s copy; nothing was removed", f.path))?;
    }
    let kept = remove(p)?;
    Ok(Offloaded { fragment: r.fragment.clone(), at: r.at.clone(), note: p.note.display().to_string(), files: p.files.len(), bytes: p.files.iter().map(|l| l.size).sum(), resumed: true, kept })
}

/// What `back` did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Back {
    pub fragment: String,
    /// Brought back now.
    pub files: usize,
    pub bytes: u64,
    /// In place already, with the note's bytes.
    pub already: usize,
    pub path: String,
}

/// A note's path, from it or from the path it is the note of.
fn note_of(path: &Path) -> Result<PathBuf> {
    let (dir, name) = place(path)?;
    Ok(match name.ends_with(NOTE_SUFFIX) {
        true => dir.join(name),
        false => dir.join(format!("{name}{NOTE_SUFFIX}")),
    })
}

/// Brings back what the note at (or for) `path` moved, beside the note.
pub fn back(c: &Client, path: &Path) -> Result<Back> {
    let note = note_of(path)?;
    let dir = note.parent().expect("a note has a folder").to_path_buf();
    let _held = lock(&dir)?;
    let text = match fs::read_to_string(&note) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(coded(Code::NotFound, format!("no {}: nothing moved from here to bring back", note.display()))),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", note.display()))),
    };
    let r = record_of(&text).with_context(|| format!("reading {}", note.display()))?;
    let name = name_of(&note)?.trim_end_matches(NOTE_SUFFIX).to_string();
    // each path under the name the note is for; each file's folders, the
    // note's own, or ones it names
    let named: std::collections::BTreeSet<&str> = r.dirs.iter().map(|d| d.path.as_str()).collect();
    let under = |p: &str| Path::new(p).components().next() == Some(Component::Normal(name.as_ref()));
    for p in r.dirs.iter().map(|d| &d.path).chain(r.files.iter().map(|f| &f.path)) {
        if !under(p) {
            bail!("{} names {p}, which is not under {name}", note.display());
        }
    }
    for p in r.dirs.iter().map(|d| &d.path).chain(r.files.iter().map(|f| &f.path)) {
        let folders: Vec<&Path> = Path::new(p).ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()).collect();
        if folders.iter().any(|a| !named.contains(a.to_str().unwrap_or_default())) {
            bail!("{} names {p}, in a folder it does not name", note.display());
        }
    }
    // its folders, where they are already, are folders (never a link out)
    for d in &r.dirs {
        let at = dir.join(&d.path);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => bail!(coded(Code::Conflict, format!("{} is there already, and is not a folder: move it aside, then bring this back; nothing was changed", at.display()))),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", at.display()))),
        }
    }
    // what is in place already: its bytes are the note's, or nothing moves
    let mut todo = Vec::new();
    let mut already = 0;
    for f in &r.files {
        let target = dir.join(&f.path);
        match fs::symlink_metadata(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => todo.push(f),
            Ok(m) if m.is_file() && m.len() == f.size && { let (s, c) = sums_of(&target)?; f.matches(&s, c) } => already += 1,
            Ok(_) => bail!(coded(Code::Conflict, format!("{} is there already, and is not what was moved: move it aside, then bring this back; nothing was changed", target.display()))),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", target.display()))),
        }
    }
    let mut held = Held::new(&r.fragment, Wants::Read);
    let head = held.get(c)?.branch_head(MAIN)?.ok_or_else(|| coded(Code::NotFound, format!("{} has no files: its copy is gone", r.fragment)))?;
    // its folders made again, parents first, private while files come
    // into them; their own bits once all are in (below), so a back cut off
    // and run again sets them too
    for d in &r.dirs {
        let at = dir.join(&d.path);
        let mut make = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut make, 0o700);
        match make.create(&at) {
            Ok(()) => sync_dir(at.parent().expect("under the note's folder"))?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) => {}
            Err(e) => return Err(anyhow::Error::new(e).context(format!("making {}; the note stays", at.display()))),
        }
    }
    let mut bytes = 0;
    for f in &todo {
        let target = dir.join(&f.path);
        let into = target.parent().expect("under the note's folder");
        let (tmp, mut out) = fresh(into)?;
        let got = read_back(c, &mut held, &r, &head, f, Some(&mut out)).and_then(|()| {
            out.sync_all()?;
            set_mode(&tmp, f.mode)?;
            // never over a file that came meanwhile
            fs::hard_link(&tmp, &target).with_context(|| format!("putting {} in place", target.display()))
        });
        fs::remove_file(&tmp).ok();
        got.with_context(|| format!("bringing back {}; the note stays", f.path))?;
        sync_dir(into)?;
        bytes += f.size;
    }
    for d in r.dirs.iter().rev() {
        set_mode(&dir.join(&d.path), d.mode).with_context(|| format!("setting {}'s permissions; the note stays", d.path))?;
    }
    fs::remove_file(&note).with_context(|| format!("removing {}", note.display()))?;
    sync_dir(&dir)?;
    Ok(Back { fragment: r.fragment, files: todo.len(), bytes, already, path: dir.join(&name).display().to_string() })
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode & 0o777))?;
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

pub fn print(o: &Offloaded) {
    let how = if o.resumed { "Finished moving" } else { "Moved" };
    println!("{how} {} files ({}) into {}, under {}/. A note is in their place: {}", o.files, size(o.bytes), o.fragment, o.at, o.note);
    for k in &o.kept {
        println!("  kept {k}: it changed while it was moved, or something new is in it");
    }
}

pub fn print_back(b: &Back) {
    println!("Brought back {} files ({}) from {} to {}; {} were in place already. Its copy stays in {}.", b.files, size(b.bytes), b.fragment, b.path, b.already, b.fragment);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mockcs::MockServer;

    const NAME: &str = "storage--k3x9";
    const MIB: usize = 1024 * 1024;

    /// A folder of its own in the temp folder, removed with it (passed or
    /// failed: a test's folder never outlives it).
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }
    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl AsRef<Path> for Temp {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    fn tempdir(tag: &str) -> Temp {
        let d = std::env::temp_dir().join(format!("fragment-offload-{tag}-{}-{:08x}", std::process::id(), rand::random::<u32>()));
        fs::create_dir_all(&d).unwrap();
        Temp(d.canonicalize().unwrap())
    }

    fn host() -> (MockServer, Client) {
        let mock = crate::mockcs::start();
        mock.seed_repo(NAME, &[]);
        mock.keep_blobs();
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        (mock, c)
    }

    /// Bytes no two of which repeat soon, `n` long.
    fn bytes(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| ((i * 31 + seed as usize) % 251) as u8).collect()
    }

    fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    out.push((e.path().strip_prefix(root).unwrap().display().to_string(), fs::read(e.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// Goal (Paul, 2026-11-18: a pressure release for a disk filling): a
    /// folder of a small file, a blob and a file past one upload moves into
    /// the fragment, a note in its place, and comes back byte for byte,
    /// its modes kept; the fragment keeps its copy. Valid, then back.
    #[test]
    fn a_folder_moves_and_comes_back_byte_for_byte() {
        let (mock, c) = host();
        mock.advertise_upload_max(Some(2 * MIB as u64));
        let root = tempdir("roundtrip");
        let photos = root.join("photos");
        fs::create_dir_all(photos.join("trip/empty")).unwrap();
        #[cfg(unix)]
        fs::set_permissions(photos.join("trip"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        fs::write(photos.join("a.md"), b"# a").unwrap();
        fs::write(photos.join("trip/b.jpg"), bytes(MIB + 5, 1)).unwrap();
        fs::write(photos.join("trip/c.mov"), bytes(5 * MIB + 3, 2)).unwrap();
        fs::write(photos.join("trip/run.sh"), b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        fs::set_permissions(photos.join("trip/run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let before = tree(&photos);
        #[cfg(unix)]
        let before_mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&photos).unwrap().permissions()) & 0o777;
        #[cfg(unix)]
        assert_ne!(before_mode, 0o700, "a folder's bits other than a back's private ones");

        let o = offload(&c, &photos, NAME).unwrap();
        assert_eq!((o.files, o.bytes, o.resumed), (4, before.iter().map(|(_, b)| b.len() as u64).sum(), false));
        assert!(!photos.exists(), "the folder is gone, its empty folders too");
        let note = root.join("photos.offloaded.md");
        let text = fs::read_to_string(&note).unwrap();
        assert!(text.starts_with("# Moved to storage\n\n`photos/` (4 files, ") && text.contains(&format!("fragment offload --back '{}'", note.display())), "{text}");
        let r = record_of(&text).unwrap();
        assert_eq!(r.fragment, NAME);
        // in the fragment: the small file in git, the blob's pointer, the big one in three parts
        let at = |p: &str| mock.file_at(NAME, "main", &format!("{}/{p}", r.at));
        assert_eq!(at("photos/a.md").as_deref(), Some(&b"# a"[..]));
        let pointer = blob::parse(&at("photos/trip/b.jpg").unwrap()).expect("a blob's pointer");
        assert_eq!(mock.blob(NAME, &pointer.sha256), Some(bytes(MIB + 5, 1)));
        assert!(at("photos/trip/c.mov").is_none() && at("photos/trip/c.mov.part-0003-of-0003").is_some(), "{:?}", mock.paths(NAME, "main"));
        assert_eq!(mock.file_at(NAME, "main", "fragment.json").as_deref(), Some(&b"{\n  \"files\": { \"page\": true }\n}\n"[..]), "its page is the files view");

        // a back cut off after making a folder, private, before its bits
        #[cfg(unix)]
        {
            fs::DirBuilder::new().create(&photos).unwrap();
            fs::set_permissions(&photos, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        }
        let b = back(&c, &note).unwrap();
        assert_eq!((b.files, b.already), (4, 0));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&photos).unwrap().permissions()) & 0o777, before_mode, "its own bits, run again");
        assert_eq!(tree(&photos), before, "byte for byte");
        assert!(photos.join("trip/empty").is_dir(), "an empty folder comes back too");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&fs::metadata(photos.join("trip")).unwrap().permissions()) & 0o777, 0o700, "and a folder's bits");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&fs::metadata(photos.join("trip/run.sh")).unwrap().permissions()) & 0o777, 0o755);
        assert!(!note.exists(), "the note goes last");
        assert!(at("photos/trip/b.jpg").is_some(), "the fragment keeps its copy");
        assert!(fs::read_dir(&root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')), "no hidden file left");
    }

    /// Goal: a failed upload, or a copy that reads back as other bytes,
    /// leaves every local file in place and writes no note; run again once
    /// the store answers, it moves. Invalid, then restart.
    #[test]
    fn a_failed_upload_or_check_removes_nothing() {
        let (mock, c) = host();
        let root = tempdir("fails");
        let file = root.join("dataset.bin");
        let data = bytes(3 * MIB, 3);
        fs::write(&file, &data).unwrap();
        let note = root.join("dataset.bin.offloaded.md");

        mock.fail_blob_puts(1);
        let e = offload(&c, &file, NAME).unwrap_err();
        assert!(format!("{e:#}").contains("nothing was removed"), "{e:#}");
        assert_eq!(fs::read(&file).unwrap(), data);
        assert!(!note.exists());

        let sha = blob::sha256_hex(&data);
        mock.corrupt_blob(&sha, true);
        let e = offload(&c, &file, NAME).unwrap_err();
        assert!(format!("{e:#}").contains("nothing was removed"), "{e:#}");
        assert_eq!(fs::read(&file).unwrap(), data, "a copy that reads back wrong removes nothing");
        assert!(!note.exists());

        mock.corrupt_blob(&sha, false);
        let o = offload(&c, &file, NAME).unwrap();
        assert_eq!((o.files, o.bytes), (1, data.len() as u64));
        assert!(!file.exists() && note.exists());
        // and a copy that went bad since: back brings nothing, the note stays
        mock.corrupt_blob(&sha, true);
        assert!(back(&c, &note).is_err());
        assert!(!file.exists() && note.exists() && !root.join(".dataset.bin.offload-back").exists());
        mock.corrupt_blob(&sha, false);
        back(&c, &file).unwrap();
        assert_eq!(fs::read(&file).unwrap(), data);
    }

    /// Goal: replay and restart. The same move again is refused while its
    /// note stands for other bytes; one cut off after its note (its file
    /// still here, as the note says) finishes, its copy checked again; a
    /// back cut off goes on, keeping what is in place; back again, with
    /// the note gone, says there is nothing to bring. Another file where
    /// one comes back is never written over.
    #[test]
    fn replay_and_restart() {
        let (mock, c) = host();
        let root = tempdir("replay");
        let dir = root.join("downloads");
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (bytes(2 * MIB, 4), bytes(10, 5));
        fs::write(dir.join("a.zip"), &a).unwrap();
        fs::write(dir.join("b.txt"), &b).unwrap();
        let note = root.join("downloads.offloaded.md");
        offload(&c, &dir, NAME).unwrap();
        let e = offload(&c, &dir, NAME).unwrap_err();
        assert!(format!("{e:#}").contains("was moved already"), "the same again: {e:#}");

        // cut off after its note: the files back beside it, as moved
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.zip"), &a).unwrap();
        fs::write(dir.join("b.txt"), &b).unwrap();
        let commits = mock.commit_pack_count();
        let o = offload(&c, &dir, NAME).unwrap();
        assert!(o.resumed && !dir.exists() && note.exists());
        assert_eq!(mock.commit_pack_count(), commits, "nothing sent again");
        // cut off halfway through its removal: the one left is removed
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.zip"), &a).unwrap();
        let o = offload(&c, &dir, NAME).unwrap();
        assert!(o.resumed && o.files == 1 && !dir.exists() && note.exists());

        // a folder of that name with other bytes: refused, untouched
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.zip"), b"new").unwrap();
        let e = offload(&c, &dir, NAME).unwrap_err();
        assert!(format!("{e:#}").contains("is there already"), "{e:#}");
        assert_eq!(fs::read(dir.join("a.zip")).unwrap(), b"new");
        // and back will not write over it
        let e = back(&c, &note).unwrap_err();
        assert!(format!("{e:#}").contains("is not what was moved"), "{e:#}");
        assert_eq!(fs::read(dir.join("a.zip")).unwrap(), b"new");
        fs::remove_dir_all(&dir).unwrap();

        // a back cut off: one file in place already
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.txt"), &b).unwrap();
        let r = back(&c, &dir).unwrap();
        assert_eq!((r.files, r.already), (1, 1));
        assert_eq!(fs::read(dir.join("a.zip")).unwrap(), a);
        assert!(!note.exists());
        let e = back(&c, &dir).unwrap_err();
        assert!(format!("{e:#}").contains("nothing moved from here"), "{e:#}");

        // moved again, after it came back: under a folder of its own
        let first = mock.paths(NAME, "main").len();
        let o = offload(&c, &dir, NAME).unwrap();
        assert!(!o.resumed && mock.paths(NAME, "main").len() == first + 2, "{:?}", mock.paths(NAME, "main"));
    }

    /// Goal: what is never moved, and where. Invalid: a note, a link, a
    /// folder holding a link, an empty folder, a public fragment, one only
    /// viewed, a chat; each refused before anything is sent or removed.
    #[test]
    fn what_is_refused() {
        let (mock, c) = host();
        let root = tempdir("refused");
        let file = root.join("a.bin");
        fs::write(&file, b"a").unwrap();
        let sent = || mock.paths(NAME, "main");
        let refused = |p: &Path, into: &str, says: &str| {
            let e = offload(&c, p, into).unwrap_err();
            assert!(format!("{e:#}").contains(says), "{e:#}");
        };
        fs::write(root.join("x.offloaded.md"), b"x").unwrap();
        refused(&root.join("x.offloaded.md"), NAME, "the note of a move");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&file, root.join("link")).unwrap();
            refused(&root.join("link"), NAME, "a link");
            fs::create_dir_all(root.join("held")).unwrap();
            std::os::unix::fs::symlink(&file, root.join("held/link")).unwrap();
            refused(&root.join("held"), NAME, "a link");
        }
        fs::create_dir_all(root.join("empty/inner")).unwrap();
        refused(&root.join("empty"), NAME, "holds no files");
        // the runtime's own home, anything in it, and what holds it
        let runtime = root.join("hermes");
        fs::create_dir_all(runtime.join("profiles/juniper")).unwrap();
        fs::write(runtime.join("profiles/juniper/state.db"), b"db").unwrap();
        for p in [runtime.join("profiles/juniper/state.db"), runtime.join("profiles"), runtime.clone(), root.to_path_buf()] {
            let e = plan(&p, Some(&runtime)).err().unwrap_or_else(|| panic!("{} moves", p.display()));
            assert!(format!("{e:#}").contains("runtime's own home"), "{e:#}");
        }
        assert!(plan(&file, Some(&runtime)).is_ok(), "a file beside it moves");
        mock.describe(NAME, "public", "app");
        refused(&file, NAME, "members-only");
        mock.describe(NAME, "members", "chat");
        refused(&file, NAME, "not a fragment of files");
        mock.describe(NAME, "members", "app");
        mock.view_only(NAME, true);
        refused(&file, NAME, "editor");
        assert!(sent().is_empty(), "nothing sent");
        assert_eq!(fs::read(&file).unwrap(), b"a", "nothing removed");
    }

    /// Goal: a note's record reads back as written; one edited out of
    /// shape, naming a path that climbs, or parts past `PARTS_MAX` (which
    /// a back would hold all at once) is refused before anything is read.
    #[test]
    fn a_notes_record() {
        let r = Record {
            fragment: NAME.into(),
            at: "2026-11-18T103102Z-00ff/data/work".into(),
            from: "/data/work/odd `name'".into(),
            part_bytes: 4,
            dirs: vec![Folder { path: "odd `name'".into(), mode: 0o700 }],
            files: vec![Moved { path: "odd `name'/f".into(), size: 9, sha256: Some("a".repeat(64)), crc32: None, mode: 0o644 }],
        };
        let text = note_text(&r, Path::new("/data/work/odd `name'.offloaded.md"), "odd `name'", Said::offloaded("2026-11-18 at 10:31 UTC"));
        assert_eq!(record_of(&text).unwrap(), r);
        assert!(text.contains(r#"fragment offload --back '/data/work/odd `name'\''.offloaded.md'"#), "{text}");
        assert_eq!(r.parts(&r.files[0]).iter().map(|(p, n)| (p.rsplit('.').next().unwrap().to_string(), *n)).collect::<Vec<_>>(), [("part-0001-of-0003".into(), 4), ("part-0002-of-0003".into(), 4), ("part-0003-of-0003".into(), 1)]);
        for bad in [
            text.replace("\"size\":9", "\"size\":9,\"x\":1"),
            text.replace("\"size\":9", "\"size\":18446744073709551615"),
            text.replace("\"mode\":420", "\"mode\":4095"),
            text.replace("odd `name'/f", "odd `name'/../../etc/f"),
            // a path past a repo path's, which every part would repeat
            text.replace("2026-11-18T103102Z-00ff/data/work", &"x".repeat(32 * 1024)),
            "# Moved to storage\n".to_string(),
        ] {
            assert!(record_of(&bad).is_err(), "{bad}");
        }
        let empty = Moved { path: "e".into(), size: 0, sha256: Some("a".repeat(64)), crc32: None, mode: 0 };
        assert_eq!(r.parts(&empty), [(format!("{}/e", r.at), 0)], "an empty file is one part");
    }

    /// Goal: back brings back only under the note's own name, into folders
    /// the note names.
    #[test]
    fn back_stays_under_its_name() {
        let (_mock, c) = host();
        let root = tempdir("escape");
        let file = |path: &str| Moved { path: path.into(), size: 1, sha256: Some("a".repeat(64)), crc32: None, mode: 0 };
        let note = root.join("a.offloaded.md");
        let folder = |path: &str| Folder { path: path.into(), mode: 0o755 };
        for (dirs, files, says) in [
            (vec![folder("a")], vec![file("b")], "not under a"),
            (vec![folder("a")], vec![file("a/x/y")], "does not name"),
            // a folder whose own folder it leaves out (a link, there)
            (vec![folder("a"), folder("a/link/new")], vec![file("a/f")], "does not name"),
        ] {
            let r = Record { fragment: NAME.into(), at: "x".into(), from: "/x".into(), part_bytes: 4, dirs, files };
            fs::write(&note, note_text(&r, &note, "a", Said::offloaded("now"))).unwrap();
            let e = back(&c, &note).unwrap_err();
            assert!(format!("{e:#}").contains(says), "{e:#}");
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1, "nothing made");
    }

    /// Goal: a folder replaced by a link before its back is refused, and
    /// nothing is written where the link points.
    #[cfg(unix)]
    #[test]
    fn back_never_follows_a_link() {
        let (_mock, c) = host();
        let root = tempdir("link");
        let dir = root.join("work");
        fs::create_dir_all(dir.join("inner")).unwrap();
        fs::write(dir.join("inner/a.txt"), b"a").unwrap();
        offload(&c, &dir, NAME).unwrap();
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        let e = back(&c, &dir).unwrap_err();
        assert!(format!("{e:#}").contains("is not a folder"), "{e:#}");
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing written through the link");
        assert!(root.join("work.offloaded.md").exists());
    }

    /// Goal: a file whose name says it may hold a password or key is never
    /// moved, alone or in a folder; nothing is sent.
    #[test]
    fn a_secret_is_never_moved() {
        let (mock, c) = host();
        let root = tempdir("secret");
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("main.py"), b"print(1)").unwrap();
        fs::write(project.join(".env"), b"KEY=x").unwrap();
        for p in [project.clone(), project.join(".env")] {
            let e = offload(&c, &p, NAME).unwrap_err();
            assert!(format!("{e:#}").contains("may hold a password or key"), "{e:#}");
        }
        assert!(mock.paths(NAME, "main").is_empty() && project.join(".env").exists() && project.join("main.py").exists());
    }

    /// Goal: one move or back works in a folder at a time: while another
    /// holds it, both are refused at once, nothing changed.
    #[test]
    fn one_at_a_time_in_a_folder() {
        let (mock, c) = host();
        let root = tempdir("lock");
        let file = root.join("a.bin");
        fs::write(&file, b"a").unwrap();
        {
            let _other = lock(&root).unwrap();
            for e in [offload(&c, &file, NAME).unwrap_err(), back(&c, &file).unwrap_err()] {
                assert!(format!("{e:#}").contains("another fragment offload is working"), "{e:#}");
            }
            assert!(mock.paths(NAME, "main").is_empty() && file.exists());
        }
        offload(&c, &file, NAME).unwrap();
        assert!(!file.exists());
    }

    /// Goal: a file changed after its copy was made is never removed: it
    /// stays, named among what was kept. Method: the plan, then a write,
    /// then its removal.
    #[test]
    fn a_file_changed_after_its_copy_stays() {
        let root = tempdir("changed");
        let dir = root.join("d");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("b.txt"), b"b").unwrap();
        fs::write(dir.join("c.txt"), b"c").unwrap();
        let p = plan(&dir, None).unwrap();
        fs::write(dir.join("b.txt"), b"bigger now").unwrap();
        // replaced by as many other bytes, its time put back
        let was = fs::metadata(dir.join("c.txt")).unwrap().modified().unwrap();
        fs::remove_file(dir.join("c.txt")).unwrap();
        fs::write(dir.join("c.txt"), b"C").unwrap();
        File::options().write(true).open(dir.join("c.txt")).unwrap().set_modified(was).unwrap();
        // and a folder made since
        fs::create_dir_all(dir.join("new")).unwrap();
        let kept = remove(&p).unwrap();
        assert!(!dir.join("a.txt").exists());
        assert_eq!(fs::read(dir.join("b.txt")).unwrap(), b"bigger now");
        assert_eq!(fs::read(dir.join("c.txt")).unwrap(), b"C");
        assert!(dir.join("new").is_dir(), "a folder it never read stays");
        assert!(kept.iter().any(|k| k.ends_with("b.txt")) && kept.iter().any(|k| k.ends_with("c.txt")) && kept.iter().any(|k| k.ends_with("/d")), "{kept:?}");
    }

    /// Goal: files named as a back's and a note's own were once
    /// (`.<name>.offload-back`, `.<name>.writing`) come back beside the
    /// file they were named for: a temporary file's name is random, never
    /// one there.
    #[test]
    fn names_beside_come_back() {
        let (_mock, c) = host();
        let root = tempdir("beside");
        let dir = root.join("d");
        fs::create_dir_all(&dir).unwrap();
        let files: [(&str, &[u8]); 3] = [("report", b"r"), (".report.offload-back", b"hidden"), (".d.offloaded.md.writing", b"w")];
        for (n, b) in files {
            fs::write(dir.join(n), b).unwrap();
        }
        let before = tree(&dir);
        offload(&c, &dir, NAME).unwrap();
        back(&c, &dir).unwrap();
        assert_eq!(tree(&dir), before);
    }
}
