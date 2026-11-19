//! Where an agent's home is read from (`fragment home`): a folder, a zip
//! of one, or a move's stage (an old agent's home as one zip64 the
//! platform keeps, FIN-463), each read in place. Every reader works on
//! each: the survey, the files it brings, and the databases it reads. A
//! database is a file SQLite opens, so a zip's is copied out first
//! (`local`), into a scratch folder of the import's own that goes with it
//! (`Scratch`).
//!
//! **A stage is never downloaded** (`Stage`): its zip's directory, and
//! each file an import reads, come by ranges from the platform, as its
//! bound agent. An old agent's home can be tens of gigabytes, and its new
//! computer's disk holds 20 (a 47 GB import that downloaded first failed,
//! 2026-11-18): what lands on the disk is only what the import keeps.
//!
//! **Paths.** Every file is named by its path relative to the root,
//! `/`-separated, as text. A zip entry that would leave the root (`..`, an
//! absolute name) refuses the whole source: what made it is not to be
//! trusted with the rest. Links and folders are not files: a folder's
//! links are left where they are, never followed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use fragment_proto::migrations::StageOpened;
use serde::Serialize;

use crate::api::{Client, Code, CodedError};

/// Files one source holds, at most: an old agent's stage holds about
/// 100,000 at the most (FIN-463), and each is a row in memory here.
pub const ENTRIES_MAX: usize = 1_000_000;
/// A file read whole (`Source::read`), at most: a SOUL, a memory, a skill's
/// file, a job list. Anything larger is streamed (`Source::with_reader`).
pub const READ_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Names said in a refusal, at most.
const SAID_MAX: usize = 10;

/// One file of a source.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the source's root, `/`-separated.
    pub path: String,
    pub size: u64,
    /// When it last changed, in seconds since 1970 (a zip's own time has
    /// no zone: it is read as UTC, which orders files all the same).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
}

/// A source: its files, and how their bytes are read.
pub struct Source {
    /// The folder or the zip it was opened from (a stage's `stage:<id>`).
    origin: PathBuf,
    /// The stage's reader, when it is one: its copies.
    remote: Option<Arc<Stage>>,
    /// The stage it reads, when it is one.
    stage: Option<String>,
    backing: Backing,
    /// Every file, by path.
    entries: Vec<Entry>,
    /// Each file's place in `entries`, and in a zip its index there.
    at: HashMap<String, (usize, usize)>,
    /// What was not read as a file, each in a few words (a name that is
    /// not text, a folder that could not be read).
    pub unread: Vec<String>,
    /// What was left out unread for what its path says (`skip`): a file
    /// that may hold a secret. No reader finds it.
    pub skipped: Vec<Entry>,
}

enum Backing {
    Dir(PathBuf),
    Zip(zip::ZipArchive<At>),
}

/// Bytes read at an offset: a zip on this computer, or a stage on the
/// platform (`Stage`).
pub trait ReadAt: Send + Sync {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
}

impl ReadAt for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        FileExt::read_at(self, buf, offset)
    }
}

/// A zip read at an offset of its own, so that each clone of it reads
/// apart from the others (`zip::ZipArchive` is `Clone` when its reader
/// is: one parse of the central directory serves every reader).
#[derive(Clone)]
pub struct At {
    file: Arc<dyn ReadAt>,
    pos: u64,
    len: u64,
}

impl Read for At {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read_at(buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

/// The first window a stage's read fetches, and the most one grows to
/// while reads go on where the last ended (a database copied out, the
/// zip's directory). A file's own window is its local header and, for a
/// small file, its bytes: one request.
const WINDOW_MIN: u64 = 64 * 1024;
const WINDOW_MAX: u64 = 8 * 1024 * 1024;
/// Windows kept, the latest last: the most a stage holds in memory is
/// `WINDOWS_KEPT * WINDOW_MAX`.
const WINDOWS_KEPT: usize = 8;

/// A move's stage, read where the platform keeps it: its zip by ranges
/// (`GET /api/migrations/stages/{stage}/zip`), as the agent the move is
/// bound to, a window at a time, never whole.
pub struct Stage {
    client: Client,
    path: String,
    len: u64,
    windows: Mutex<Windows>,
}

#[derive(Default)]
struct Windows {
    kept: VecDeque<(u64, Arc<Vec<u8>>)>,
    /// Where the last fetch ended, and how long it was.
    end: u64,
    last: u64,
}

impl Stage {
    /// Opens `stage` (`POST …/open`, harmless again): its manifest, and a
    /// reader of its zip.
    pub fn open(c: &Client, stage: &str) -> Result<(StageOpened, Stage)> {
        if stage.is_empty() || !stage.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
            bail!(usage(format!("{stage:?} is no stage: a move's routine names it (<old>.<phase>.<attempt>)")));
        }
        let base = format!("/api/migrations/stages/{stage}");
        let opened: StageOpened = c.call_as(c.post_json(&format!("{base}/open"), &serde_json::json!({}))?).with_context(|| format!("opening stage {stage}"))?;
        let client = Client::new(&c.host, c.signer.clone());
        let len = opened.manifest.zip_bytes;
        Ok((opened, Stage { client, path: format!("{base}/zip"), len, windows: Mutex::default() }))
    }
}

impl Stage {
    /// `len` bytes of the zip from `offset` copied into `into`'s blobs on
    /// the platform (`POST …/copy`), never through this computer: the blob
    /// they are, by their SHA-256. The same range again is the same blob.
    pub fn copy(&self, into: &str, offset: u64, len: u64) -> Result<(String, u32)> {
        let base = self.path.strip_suffix("/zip").expect("a stage's zip path");
        let path = format!("{base}/copy?into={}&offset={offset}&bytes={len}", crate::api::encode_q(into));
        let copied: fragment_proto::migrations::StageCopied = self.client.call_as(self.client.post_safe(&path, len)?).with_context(|| format!("copying {len} bytes of the stage into {into}"))?;
        if copied.bytes != len || !fragment_core::blob::valid_sha(&copied.sha256) {
            bail!("a copy of {len} bytes answered {} bytes, blob {:?}", copied.bytes, copied.sha256);
        }
        Ok((copied.sha256, copied.crc32))
    }
}

impl ReadAt for Stage {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let mut w = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let copy = |buf: &mut [u8], at: u64, bytes: &[u8]| {
            let from = (offset - at) as usize;
            let n = buf.len().min(bytes.len() - from);
            buf[..n].copy_from_slice(&bytes[from..from + n]);
            n
        };
        if let Some((at, bytes)) = w.kept.iter().find(|(at, b)| (*at..*at + b.len() as u64).contains(&offset)) {
            return Ok(copy(buf, *at, bytes));
        }
        // a read going on where the last fetch ended takes a window twice
        // as long; any other starts small again
        let grown = if offset == w.end { (w.last * 2).min(WINDOW_MAX) } else { WINDOW_MIN };
        let len = grown.max((buf.len() as u64).min(WINDOW_MAX)).min(self.len - offset);
        let bytes = Arc::new(self.client.get_range(&self.path, offset, len).map_err(|e| io::Error::other(format!("{e:#}")))?);
        (w.end, w.last) = (offset + len, len);
        if w.kept.len() >= WINDOWS_KEPT {
            w.kept.pop_front();
        }
        w.kept.push_back((offset, bytes.clone()));
        Ok(copy(buf, offset, &bytes))
    }
}

impl Seek for At {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))?;
        Ok(self.pos)
    }
}

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
}

fn refused(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidRequest, msg })
}

/// Some of `names`, for a refusal.
pub fn said(names: &[String]) -> String {
    let mut out = names.iter().take(SAID_MAX).map(|n| format!("{:?}", n)).collect::<Vec<_>>().join(", ");
    if names.len() > SAID_MAX {
        out.push_str(&format!(" and {} more", names.len() - SAID_MAX));
    }
    out
}

/// How a source names a move's stage: `stage:<the stage>`.
pub const STAGE_PREFIX: &str = "stage:";

/// The stage a source's path names, if it names one.
pub fn stage_of(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix(STAGE_PREFIX)
}

/// Whether the file at `path` is a zip (by its first bytes).
pub fn is_zip(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::metadata(path).is_ok_and(|m| m.is_file()) && File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && (head == *b"PK\x03\x04" || head == *b"PK\x05\x06")
}

/// A plain relative path: names between single slashes, none of them
/// `.` or `..`, no backslash or control character. Every path a source
/// lists is one, so it is matched, written and joined as its text says.
pub fn plain(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..") && !path.bytes().any(|b| b < 0x20 || b == b'\\')
}

impl Source {
    /// The source a person names: a folder, or a zip file.
    pub fn open(path: &Path) -> Result<Source> {
        let meta = std::fs::metadata(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        if meta.is_dir() {
            return Source::dir(path);
        }
        if !is_zip(path) {
            bail!(usage(format!("{} is no folder and no zip: name an agent's home, or a zip of one", path.display())));
        }
        Source::zip(path)
    }

    fn dir(root: &Path) -> Result<Source> {
        let mut entries = Vec::new();
        let mut unread = Vec::new();
        let mut folders = vec![(root.to_path_buf(), String::new())];
        while let Some((dir, rel)) = folders.pop() {
            let listed = match std::fs::read_dir(&dir) {
                Ok(l) => l,
                Err(e) => {
                    unread.push(format!("{}: {e}", if rel.is_empty() { "." } else { &rel }));
                    continue;
                }
            };
            for e in listed.flatten() {
                let Some(name) = e.file_name().to_str().map(str::to_string) else {
                    unread.push(format!("{}/{}: its name is not text", rel, e.file_name().to_string_lossy()));
                    continue;
                };
                let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                // a name a zip could not hold either (a backslash, a control
                // character) is not read: every path a source lists is plain
                if !plain(&path) {
                    unread.push(format!("{}: its name is not a plain path", path.escape_debug()));
                    continue;
                }
                // a link is neither followed nor read: what it names is not the home's
                let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
                if meta.is_dir() {
                    folders.push((e.path(), path));
                } else if meta.is_file() {
                    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
                    entries.push(Entry { path, size: meta.len(), mtime });
                    if entries.len() > ENTRIES_MAX {
                        bail!(refused(format!("{} holds more than {ENTRIES_MAX} files", root.display())));
                    }
                }
            }
        }
        Ok(Source::with(root, Backing::Dir(root.to_path_buf()), entries.into_iter().map(|e| (e, 0)).collect(), unread))
    }

    fn zip(path: &Path) -> Result<Source> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata()?.len();
        let archive = zip::ZipArchive::new(At { file: Arc::new(file), pos: 0, len }).with_context(|| format!("reading {} as a zip", path.display()))?;
        Source::from_archive(path, archive, &path.display().to_string())
    }

    /// A move's stage, read in place (`Stage`): its manifest, and its home.
    pub fn stage(c: &Client, stage: &str) -> Result<(StageOpened, Source)> {
        let (opened, reader) = Stage::open(c, stage)?;
        let len = reader.len;
        let reader = Arc::new(reader);
        let said = format!("stage {stage}");
        // a stage's zip starts at its first byte (the mover writes it so):
        // the crate's default looks for where it starts by reading forward
        // from there to its directory, the whole stage
        let config = zip::read::Config { archive_offset: zip::read::ArchiveOffset::Known(0) };
        let archive = zip::ZipArchive::with_config(config, At { file: reader.clone(), pos: 0, len }).with_context(|| format!("reading {said}'s zip"))?;
        let mut src = Source::from_archive(&PathBuf::from(format!("{STAGE_PREFIX}{stage}")), archive, &said)?;
        (src.stage, src.remote) = (Some(stage.to_string()), Some(reader));
        Ok((opened, src))
    }

    /// A zip's files, from its central directory (no file's bytes are
    /// read). Each is named by a plain path (`plain`), or the whole zip is
    /// refused: a name a reader resolves another way than its text (`a/./b`
    /// is `a/b`) would pass what is matched by its text, a secret's glob.
    pub fn from_archive(origin: &Path, archive: zip::ZipArchive<At>, said_as: &str) -> Result<Source> {
        if archive.len() > ENTRIES_MAX {
            bail!(refused(format!("{said_as} holds more than {ENTRIES_MAX} files")));
        }
        let mut entries = Vec::with_capacity(archive.len());
        let mut odd = Vec::new();
        let mut unread = Vec::new();
        for i in 0..archive.len() {
            let e = archive.by_index_data(i).with_context(|| format!("reading {said_as}'s entry {i}"))?;
            let name = match e.name() {
                Ok(n) => n.to_string(),
                Err(_) => {
                    unread.push(format!("entry {i}: its name is not text"));
                    continue;
                }
            };
            let path = if e.is_dir() { name.strip_suffix('/').unwrap_or(&name) } else { name.as_str() };
            if e.enclosed_name().is_none() || !plain(path) {
                odd.push(name);
                continue;
            }
            if e.is_dir() || e.is_symlink() {
                continue;
            }
            entries.push((Entry { path: name, size: e.size(), mtime: e.last_modified().and_then(dos_seconds) }, i));
        }
        if !odd.is_empty() {
            bail!(refused(format!("{said_as} names files outside itself, or by paths that are not plain ({}): it is not read", said(&odd))));
        }
        Ok(Source::with(origin, Backing::Zip(archive), entries, unread))
    }

    fn with(origin: &Path, backing: Backing, mut entries: Vec<(Entry, usize)>, unread: Vec<String>) -> Source {
        entries.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        // a zip may name one path twice: the last written is the file
        entries.dedup_by(|later, earlier| later.0.path == earlier.0.path && {
            *earlier = later.clone();
            true
        });
        let at = entries.iter().enumerate().map(|(n, (e, i))| (e.path.clone(), (n, *i))).collect();
        Source { origin: origin.to_path_buf(), remote: None, stage: None, backing, entries: entries.into_iter().map(|(e, _)| e).collect(), at, unread, skipped: Vec::new() }
    }

    /// The folder or the zip it was opened from, as named.
    pub fn origin(&self) -> &Path {
        &self.origin
    }

    /// Leaves out every file `skip` names: listed in `skipped`, never read,
    /// since no reader finds a file the source no longer lists.
    pub fn skip(&mut self, skip: impl Fn(&str) -> bool) {
        let mut at = HashMap::with_capacity(self.at.len());
        let mut kept = Vec::with_capacity(self.entries.len());
        for e in std::mem::take(&mut self.entries) {
            if skip(&e.path) {
                self.skipped.push(e);
                continue;
            }
            let (_, index) = self.at[&e.path];
            at.insert(e.path.clone(), (kept.len(), index));
            kept.push(e);
        }
        (self.entries, self.at) = (kept, at);
    }

    /// Every file, by path.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.at.get(path).map(|(n, _)| &self.entries[*n])
    }

    /// The files under `folder` (`""` for all), by path.
    pub fn under<'a>(&'a self, folder: &str) -> impl Iterator<Item = &'a Entry> + 'a {
        let prefix = if folder.is_empty() || folder == "." { String::new() } else { format!("{}/", folder.trim_end_matches('/')) };
        let start = self.entries.partition_point(|e| e.path.as_str() < prefix.as_str());
        self.entries[start..].iter().take_while(move |e| e.path.starts_with(&prefix))
    }

    /// Whether any file is under `folder`.
    pub fn has_folder(&self, folder: &str) -> bool {
        self.under(folder).next().is_some()
    }

    /// What kind of source this is, as the survey says it.
    pub fn kind(&self) -> &'static str {
        match (&self.backing, &self.stage) {
            (Backing::Dir(_), _) => "folder",
            (Backing::Zip(_), None) => "zip",
            (Backing::Zip(_), Some(_)) => "stage",
        }
    }

    /// The stage it reads, when it is one.
    pub fn stage_id(&self) -> Option<&str> {
        self.stage.as_deref()
    }

    /// Where a zip's file's bytes are in it, as they are (stored):
    /// (offset, length). None for a folder's, or one compressed in the zip.
    pub fn span(&self, path: &str) -> Result<Option<(u64, u64)>> {
        let Backing::Zip(archive) = &self.backing else { return Ok(None) };
        let (_, index) = *self.at.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?;
        let mut archive = archive.clone();
        let e = archive.by_index_raw(index).with_context(|| format!("finding {path} in the zip"))?;
        let start = e.data_start().ok_or_else(|| anyhow!("{path}: its bytes' place in the zip is not known"))?;
        Ok((e.compression() == zip::CompressionMethod::Stored && e.compressed_size() == e.size()).then_some((start, e.size())))
    }

    /// A zip's file's CRC-32 and permission bits, as its directory names
    /// them (the old file's own, made where it was zipped). None for a
    /// folder's file.
    pub fn sums(&self, path: &str) -> Result<Option<(u32, Option<u32>)>> {
        let Backing::Zip(archive) = &self.backing else { return Ok(None) };
        let (_, index) = *self.at.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?;
        let e = archive.by_index_data(index).with_context(|| format!("finding {path} in the zip"))?;
        Ok(Some((e.crc32(), e.unix_mode().map(|m| m & 0o777))))
    }

    /// `len` bytes of a file from `offset` (a part of it): for one stored
    /// in a zip, read where they are; else read from its start.
    pub fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let size = self.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?.size;
        let end = offset.checked_add(len).filter(|e| *e <= size).ok_or_else(|| anyhow!("{path}: bytes {offset}+{len} are past its {size}"))?;
        assert!(end - offset == len, "a range is its length");
        let mut out = vec![0u8; len as usize];
        match (&self.backing, self.span(path)?) {
            (Backing::Zip(archive), Some((start, _))) => {
                let mut at = archive.clone().into_inner();
                at.seek(SeekFrom::Start(start + offset))?;
                at.read_exact(&mut out).with_context(|| format!("reading {path}"))?;
            }
            (Backing::Dir(root), _) => {
                let mut f = File::open(root.join(path)).with_context(|| format!("opening {path}"))?;
                f.seek(SeekFrom::Start(offset))?;
                f.read_exact(&mut out).with_context(|| format!("reading {path}"))?;
            }
            (Backing::Zip(_), None) => self.with_reader(path, |r| {
                io::copy(&mut r.take(offset), &mut io::sink())?;
                r.read_exact(&mut out).with_context(|| format!("reading {path}"))
            })?,
        }
        Ok(out)
    }

    /// `len` bytes of a stage's zip from `offset` copied into `into`'s blobs
    /// on the platform (`Stage::copy`): the blob they are, and their CRC-32.
    pub fn copy(&self, into: &str, offset: u64, len: u64) -> Result<(String, u32)> {
        self.remote.as_ref().ok_or_else(|| anyhow!("only a stage's bytes are copied on the platform"))?.copy(into, offset, len)
    }

    /// One file's bytes as they are read.
    pub fn with_reader<T>(&self, path: &str, f: impl FnOnce(&mut dyn Read) -> Result<T>) -> Result<T> {
        let (_, index) = *self.at.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?;
        match &self.backing {
            Backing::Dir(root) => {
                let mut file = File::open(root.join(path)).with_context(|| format!("opening {path}"))?;
                f(&mut file)
            }
            Backing::Zip(archive) => {
                let mut archive = archive.clone();
                let mut member = archive.by_index(index).with_context(|| format!("opening {path} in the zip"))?;
                f(&mut member)
            }
        }
    }

    /// One file's bytes, whole: at most `READ_MAX_BYTES`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.read_max(path, READ_MAX_BYTES)
    }

    /// One file's bytes, whole: at most `max` (a blob's most, for a file
    /// brought whole).
    pub fn read_max(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        let size = self.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?.size;
        if size > max {
            bail!("{path} is {size} bytes: more than {max} to read whole");
        }
        self.with_reader(path, |r| {
            let mut out = Vec::with_capacity(size as usize);
            r.take(max + 1).read_to_end(&mut out).with_context(|| format!("reading {path}"))?;
            if out.len() as u64 != size {
                bail!("{path} read as {} bytes, not the {size} listed", out.len());
            }
            Ok(out)
        })
    }

    /// One file's text, or `None` when it holds bytes that are not text.
    pub fn text(&self, path: &str) -> Result<Option<String>> {
        let bytes = self.read(path)?;
        if bytes.iter().take(8192).any(|b| *b == 0) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// One file as a file on this computer, for what opens a file by its
    /// path (SQLite): a copy in `scratch`, a folder's as a zip's. Never
    /// its own place: SQLite reads a WAL beside a database by itself, so
    /// what it opens has beside it only what the source still lists (the
    /// caller copies those), never a sidecar it skipped.
    pub fn local(&self, path: &str, scratch: &Scratch) -> Result<PathBuf> {
        let listed = self.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?.size;
        let to = scratch.dir.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("making {}", parent.display()))?;
        }
        let copied = self.with_reader(path, |r| {
            let mut out = File::create(&to).with_context(|| format!("writing {}", to.display()))?;
            io::copy(r, &mut out).with_context(|| format!("copying {path}"))
        })?;
        // a zip's file is the size its directory lists; a folder's may have
        // grown since it was listed (a Hermes writing to it)
        if matches!(self.backing, Backing::Zip(_)) && copied != listed {
            bail!("{path} came out of the zip as {copied} bytes, not the {listed} listed");
        }
        Ok(to)
    }
}

/// A zip's time of a file (no zone), as seconds since 1970 in UTC.
fn dos_seconds(t: zip::DateTime) -> Option<i64> {
    let civil = jiff::civil::DateTime::new(t.year() as i16, t.month() as i8, t.day() as i8, t.hour() as i8, t.minute() as i8, t.second() as i8, 0).ok()?;
    civil.to_zoned(jiff::tz::TimeZone::UTC).ok().map(|z| z.timestamp().as_second())
}

/// A folder for files copied out of a zip, inside a folder of the import's
/// own (`root`: the session import's spool), removed with it; a crash's
/// goes when that folder is cleared.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn in_dir(root: &Path) -> Result<Scratch> {
        let dir = root.join(format!("scratch-{:016x}", rand::random::<u64>()));
        // its owner's alone from its making: the system's temporary folder is
        // everyone's, and what is copied here is a person's private database
        std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).create(&dir).with_context(|| format!("making {}", dir.display()))?;
        Ok(Scratch { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    /// A temporary folder, removed with it.
    pub struct Temp(pub PathBuf);
    impl Temp {
        pub fn new() -> Temp {
            let dir = std::env::temp_dir().join(format!("fragment-home-test-{}-{:016x}", std::process::id(), rand::random::<u64>()));
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `files` (path, bytes) as a zip at `to`, stored and deflated in turn.
    pub fn zip_of(to: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(to).unwrap());
        for (n, (path, bytes)) in files.iter().enumerate() {
            let method = if n % 2 == 0 { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
            let options = zip::write::SimpleFileOptions::default().compression_method(method).last_modified_time(zip::DateTime::from_date_and_time(2026, 3, 1 + n as u8 % 27, 12, 0, 0).unwrap());
            w.start_file(*path, options).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }

    /// A folder and the zip of it read the same: the same files, sizes and
    /// bytes, and a database copied out of the zip opens as a file.
    #[test]
    fn a_folder_and_its_zip_read_the_same() {
        let t = Temp::new();
        let files: &[(&str, &[u8])] = &[("a/b.txt", b"hello"), ("a/c/d.bin", &[0, 1, 2, 3]), ("e.md", b"# e\n"), ("big.dat", &[7u8; 70_000])];
        let dir = t.0.join("home");
        for (path, bytes) in files {
            std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            std::fs::write(dir.join(path), bytes).unwrap();
        }
        let zip = t.0.join("home.zip");
        zip_of(&zip, files);
        let (d, z) = (Source::open(&dir).unwrap(), Source::open(&zip).unwrap());
        assert_eq!((d.kind(), z.kind()), ("folder", "zip"));
        let paths = |s: &Source| s.entries().iter().map(|e| (e.path.clone(), e.size)).collect::<Vec<_>>();
        assert_eq!(paths(&d), paths(&z));
        assert_eq!(paths(&d), vec![("a/b.txt".into(), 5), ("a/c/d.bin".into(), 4), ("big.dat".into(), 70_000), ("e.md".into(), 4)]);
        for (path, bytes) in files {
            assert_eq!(d.read(path).unwrap(), *bytes);
            assert_eq!(z.read(path).unwrap(), *bytes, "{path} out of the zip");
        }
        assert_eq!(z.text("a/c/d.bin").unwrap(), None, "bytes with a NUL are not text");
        let scratch = Scratch::in_dir(&t.0).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&scratch.dir).unwrap().permissions().mode() & 0o777, 0o700, "the scratch is its owner's alone");
        let out = z.local("big.dat", &scratch).unwrap();
        assert!(out.starts_with(&scratch.dir));
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 70_000]);
        let copy = d.local("big.dat", &scratch).unwrap();
        assert!(copy.starts_with(&scratch.dir) && std::fs::read(&copy).unwrap() == vec![7u8; 70_000], "a folder's file is copied too");
        let held = scratch.dir.clone();
        drop(scratch);
        assert!(!held.exists(), "the scratch goes with the import");
        assert_eq!(z.under("a").map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a/b.txt", "a/c/d.bin"]);
        assert!(z.has_folder("a/c") && !z.has_folder("a/b"));
        assert!(z.get("a").is_none(), "a folder is not a file");
        assert_eq!(z.get("e.md").unwrap().mtime, Some(1772539200), "a zip's time read as UTC (2026-03-03 12:00, its third file's)");
    }

    /// Goal (2026-11-18: an import that downloaded its 47 GB stage first
    /// could not hold it): a move's stage reads as its zip does, in place:
    /// the same files, sizes and bytes, a database copied out whole, and no
    /// more of the zip fetched than what was read, so a large file never
    /// read is never fetched, and one read comes in windows. Invalid: a
    /// stage that is not there, a name that is no stage's. Method: the zip
    /// as a stage writes it (stored, zip64), in the code.storage fake's
    /// stage routes, which answer a range alone.
    #[test]
    fn a_stage_reads_as_its_zip_in_place() {
        let t = Temp::new();
        let video: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        let db: Vec<u8> = (0..300_000u32).map(|i| (i % 7) as u8).collect();
        let files: Vec<(String, Vec<u8>)> = vec![
            ("agent/hermes-home/SOUL.md".into(), b"me".to_vec()),
            ("agent/hermes-home/state.db".into(), db.clone()),
            ("workspace/notes/a.md".into(), b"# a\n".to_vec()),
            ("workspace/videos/trip.mp4".into(), video.clone()),
        ];
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, &files).unwrap();
        let bytes = std::fs::read(&zip).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_stage("old1.p1.1", bytes.clone());
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let (opened, s) = Source::stage(&c, "old1.p1.1").unwrap();
        let (reads, served) = mock.stage_reads();
        assert!(reads <= 3 && served <= 3 * WINDOW_MIN, "its directory read from the end alone: {reads} reads, {served} bytes");
        assert_eq!((opened.manifest.zip_bytes, s.kind(), s.stage_id()), (bytes.len() as u64, "stage", Some("old1.p1.1")));
        let local = Source::open(&zip).unwrap();
        let listed = |s: &Source| s.entries().iter().map(|e| (e.path.clone(), e.size, e.mtime)).collect::<Vec<_>>();
        assert_eq!(listed(&s), listed(&local), "the same files, sizes and times as the zip");
        assert_eq!(s.read("agent/hermes-home/SOUL.md").unwrap(), b"me");
        assert_eq!(s.read("workspace/notes/a.md").unwrap(), b"# a\n");
        let scratch = Scratch::in_dir(&t.0).unwrap();
        assert_eq!(std::fs::read(s.local("agent/hermes-home/state.db", &scratch).unwrap()).unwrap(), db, "a database copied out whole");
        let (_, served) = mock.stage_reads();
        assert!(served < video.len() as u64, "the video, never read, was never fetched: {served} bytes of {}", bytes.len());
        // read, it comes in windows, each at most WINDOW_MAX, its bytes whole
        let before = mock.stage_reads();
        let read = s.with_reader("workspace/videos/trip.mp4", |r| {
            let mut out = Vec::new();
            r.read_to_end(&mut out)?;
            Ok(out)
        });
        assert!(read.unwrap() == video, "the video read whole");
        let (reads, served) = (mock.stage_reads().0 - before.0, mock.stage_reads().1 - before.1);
        assert!(reads >= video.len() as u64 / WINDOW_MAX && served < video.len() as u64 + 2 * WINDOW_MAX, "{reads} reads, {served} bytes");
        // invalid
        let e = Source::stage(&c, "old2.p1.1").err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("no stage old2.p1.1"), "{e}");
        let e = Source::stage(&c, "../x").err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("is no stage"), "{e}");
        assert_eq!(stage_of(Path::new("stage:old1.p1.1")), Some("old1.p1.1"));
        assert_eq!(stage_of(Path::new("./stage:x")), None, "a path that only holds the word is a path");
    }

    /// A zip that names a file outside itself is not read at all; a link in
    /// a folder is not followed; something that is neither is refused.
    #[test]
    fn what_leaves_the_root_is_refused() {
        let t = Temp::new();
        let zip = t.0.join("bad.zip");
        zip_of(&zip, &[("ok.txt", b"ok"), ("../escape.txt", b"no")]);
        let e = Source::open(&zip).err().expect("refused");
        assert!(format!("{e:#}").contains("(\"../escape.txt\"): it is not read"), "{e:#}");
        // a path a reader resolves past its text: `agent/./keys` is `agent/keys`
        for odd in ["agent/./keys/signing.txt", "./SOUL.md", "a//b", "a\\b", "a/b/../c"] {
            zip_of(&zip, &[("ok.txt", b"ok"), (odd, b"no")]);
            let e = Source::open(&zip).err().unwrap_or_else(|| panic!("{odd} refused"));
            assert!(format!("{e:#}").contains("not plain"), "{odd}: {e:#}");
        }
        assert!(plain("a/b.c/d") && !plain("") && !plain("/a") && !plain("a/") && !plain("a/\nb"));
        let dir = t.0.join("home");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(t.0.join("outside.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(t.0.join("outside.txt"), dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&t.0, dir.join("up")).unwrap();
        std::fs::write(dir.join("in.txt"), "in").unwrap();
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/a\\b.txt"), "odd").unwrap();
        let d = Source::open(&dir).unwrap();
        assert_eq!(d.entries().iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["in.txt"], "links are left, and a name no path is plain is not read");
        assert_eq!(d.unread, ["notes/a\\\\b.txt: its name is not a plain path"]);
        std::fs::write(t.0.join("plain.txt"), "not a zip").unwrap();
        let e = Source::open(&t.0.join("plain.txt")).err().expect("refused");
        assert!(format!("{e:#}").contains("is no folder and no zip"), "{e:#}");
        assert!(Source::open(&t.0.join("missing")).is_err());
    }
}
