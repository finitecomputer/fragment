//! Where an agent's home is read from (`fragment home`): a folder, or a
//! zip of one, read in place (an old agent's stage is one zip64 file,
//! FIN-463). Every reader works on either: the survey, the files it
//! brings, and the databases it reads. A database is a file SQLite opens,
//! so a zip's is copied out first (`local`), into a scratch folder of the
//! import's own that goes with it (`Scratch`).
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

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use super::stage;
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
    /// The folder or the zip it was opened from (none: a stage, no file
    /// here).
    origin: Option<PathBuf>,
    backing: Backing,
    /// Every file, by path.
    entries: Vec<Entry>,
    /// Each file's place in `entries`, and in a zip its index there.
    at: HashMap<String, (usize, usize)>,
    /// What was not read as a file, each in a few words (a name that is
    /// not text, a folder that could not be read).
    pub unread: Vec<String>,
}

enum Backing {
    Dir(PathBuf),
    Zip(zip::ZipArchive<ZipReader>),
}

/// What a zip is read from: a file here, or a stage by ranges.
#[derive(Clone)]
pub enum ZipReader {
    File(At),
    Stage(stage::Ranged),
}

impl Read for ZipReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ZipReader::File(r) => r.read(buf),
            ZipReader::Stage(r) => r.read(buf),
        }
    }
}

impl Seek for ZipReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            ZipReader::File(r) => r.seek(to),
            ZipReader::Stage(r) => r.seek(to),
        }
    }
}

/// A file read at an offset of its own, so that each clone of a zip reads
/// it apart from the others (`zip::ZipArchive` is `Clone` when its reader
/// is: one parse of the central directory serves every reader).
#[derive(Clone)]
pub struct At {
    file: Arc<File>,
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
        Ok(Source::with(Some(root), Backing::Dir(root.to_path_buf()), entries.into_iter().map(|e| (e, 0)).collect(), unread))
    }

    fn zip(path: &Path) -> Result<Source> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata()?.len();
        let archive = zip::ZipArchive::new(ZipReader::File(At { file: Arc::new(file), pos: 0, len })).with_context(|| format!("reading {} as a zip", path.display()))?;
        Source::from_archive(Some(path), archive, &path.display().to_string())
    }

    /// A stage (`stage:<id>`), opened through the platform as its agent,
    /// its zip read by ranges.
    pub fn stage(c: &Client, id: &str) -> Result<Source> {
        let opened = stage::open(c, id)?;
        let mut client = Client::new(&c.host, c.signer.clone());
        client.verbose = c.verbose;
        let blocks = stage::Blocks::new(Arc::new(client), id, opened.manifest.zip_bytes);
        let said = format!("the stage {id}");
        let archive = zip::ZipArchive::new(ZipReader::Stage(stage::Ranged::new(blocks))).with_context(|| format!("reading {said} as a zip"))?;
        Source::from_archive(None, archive, &said)
    }

    /// The source an argument names: a stage (`stage:<id>`, through the
    /// platform, `c`), a folder, or a zip file.
    pub fn named(arg: &Path, c: Option<&Client>) -> Result<Source> {
        match arg.to_str().and_then(stage::named) {
            Some(id) => {
                let id = id?;
                let c = c.ok_or_else(|| usage(format!("a stage is read through the platform: {} needs your key, or an agent's computer", arg.display())))?;
                Source::stage(c, id)
            }
            None => Source::open(arg),
        }
    }

    /// A zip's files, from its central directory (no file's bytes are
    /// read). Each is named by a plain path (`plain`), or the whole zip is
    /// refused: a name a reader resolves another way than its text (`a/./b`
    /// is `a/b`) would pass what is matched by its text, a secret's glob.
    pub fn from_archive(origin: Option<&Path>, archive: zip::ZipArchive<ZipReader>, said_as: &str) -> Result<Source> {
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

    fn with(origin: Option<&Path>, backing: Backing, mut entries: Vec<(Entry, usize)>, unread: Vec<String>) -> Source {
        entries.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        // a zip may name one path twice: the last written is the file
        entries.dedup_by(|later, earlier| later.0.path == earlier.0.path && {
            *earlier = later.clone();
            true
        });
        let at = entries.iter().enumerate().map(|(n, (e, i))| (e.path.clone(), (n, *i))).collect();
        Source { origin: origin.map(Path::to_path_buf), backing, entries: entries.into_iter().map(|(e, _)| e).collect(), at, unread }
    }

    /// The folder or the zip it was opened from, as named (none: a stage).
    pub fn origin(&self) -> Option<&Path> {
        self.origin.as_deref()
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
        match &self.backing {
            Backing::Dir(_) => "folder",
            Backing::Zip(a) if matches!(a.clone().into_inner(), ZipReader::Stage(_)) => "stage",
            Backing::Zip(_) => "zip",
        }
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
                // a stage's large file stored whole: by ranges at once (stage.rs)
                let meta = archive.by_index_data(index).with_context(|| format!("reading {path}'s entry"))?;
                let (stored, size, crc) = (meta.compression() == zip::CompressionMethod::Stored, meta.size(), meta.crc32());
                if let (true, true, ZipReader::Stage(r)) = (stored, size >= stage::PARALLEL_MIN, archive.clone().into_inner()) {
                    let start = archive.by_index_raw(index).with_context(|| format!("opening {path} in the zip"))?.data_start().ok_or_else(|| anyhow!("{path}: its data's place is unknown"))?;
                    return f(&mut stage::Parallel::new(r.blocks, start, size, crc));
                }
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
            r.take(max.saturating_add(1)).read_to_end(&mut out).with_context(|| format!("reading {path}"))?;
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
    /// path (SQLite): a folder's own, or a zip's copied into `scratch`.
    pub fn local(&self, path: &str, scratch: &Scratch) -> Result<PathBuf> {
        match &self.backing {
            Backing::Dir(root) => {
                self.get(path).ok_or_else(|| anyhow!("{path}: no such file"))?;
                Ok(root.join(path))
            }
            Backing::Zip(_) => {
                let to = scratch.dir.join(path);
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).with_context(|| format!("making {}", parent.display()))?;
                }
                let size = self.get(path).map(|e| e.size).unwrap_or(0);
                let copied = self.with_reader(path, |r| {
                    let mut out = File::create(&to).with_context(|| format!("writing {}", to.display()))?;
                    io::copy(r, &mut out).with_context(|| format!("copying {path} out of the zip"))
                })?;
                if copied != size {
                    bail!("{path} came out of the zip as {copied} bytes, not the {size} listed");
                }
                Ok(to)
            }
        }
    }
}

/// A zip's time of a file (no zone), as seconds since 1970 in UTC.
fn dos_seconds(t: zip::DateTime) -> Option<i64> {
    let civil = jiff::civil::DateTime::new(t.year() as i16, t.month() as i8, t.day() as i8, t.hour() as i8, t.minute() as i8, t.second() as i8, 0).ok()?;
    civil.to_zoned(jiff::tz::TimeZone::UTC).ok().map(|z| z.timestamp().as_second())
}

/// A folder of an import's own for files copied out of a zip, under the
/// system's temporary folder, removed with it (a crash's is cleared by the
/// next import's, a day on).
pub struct Scratch {
    pub dir: PathBuf,
}

const SCRATCH_PREFIX: &str = "fragment-home-";
const SCRATCH_STALE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

impl Scratch {
    pub fn new() -> Result<Scratch> {
        let tmp = std::env::temp_dir();
        if let Ok(dir) = std::fs::read_dir(&tmp) {
            for e in dir.flatten() {
                let stale = e.file_name().to_str().is_some_and(|n| n.starts_with(SCRATCH_PREFIX))
                    && e.metadata().is_ok_and(|m| m.is_dir() && m.modified().is_ok_and(|t| t.elapsed().is_ok_and(|age| age > SCRATCH_STALE)));
                if stale {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let dir = tmp.join(format!("{SCRATCH_PREFIX}{}-{:016x}", std::process::id(), rand::random::<u64>()));
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
        let scratch = Scratch::new().unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&scratch.dir).unwrap().permissions().mode() & 0o777, 0o700, "the scratch is its owner's alone");
        let out = z.local("big.dat", &scratch).unwrap();
        assert!(out.starts_with(&scratch.dir));
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 70_000]);
        assert_eq!(d.local("big.dat", &scratch).unwrap(), dir.join("big.dat"), "a folder's file is its own");
        let held = scratch.dir.clone();
        drop(scratch);
        assert!(!held.exists(), "the scratch goes with the import");
        assert_eq!(z.under("a").map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a/b.txt", "a/c/d.bin"]);
        assert!(z.has_folder("a/c") && !z.has_folder("a/b"));
        assert!(z.get("a").is_none(), "a folder is not a file");
        assert_eq!(z.get("e.md").unwrap().mtime, Some(1772539200), "a zip's time read as UTC (2026-03-03 12:00, its third file's)");
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
