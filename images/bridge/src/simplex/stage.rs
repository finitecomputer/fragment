//! An owner's SimpleX conversation with their old finite.computer agent,
//! carried to its new agent (FIN-463; docs/migrations.md, "SimpleX in a
//! stage"). The old agent's mover puts its SimpleX identity in the stage's
//! zip, as two plain SQLite files and its owner's contact:
//!
//! - `.finite-stage/simplex/chat.db` (the old core's `identity_chat.db`),
//! - `.finite-stage/simplex/agent.db` (its `identity_agent.db`),
//! - `.finite-stage/simplex/owner.json`: `{"contact": <its id>}`.
//!
//! The agent never imports them (its import skill skips `.finite-stage/`);
//! the bridge does, as root, with the agent's own key (the platform answers
//! a stage's bytes to its bound agent alone, while it imports):
//!
//! - **Set aside** before the move's routine reaches the agent (`of_routine`;
//!   driver.rs `feed` holds it until this ends), so its import, and the
//!   `done` that makes the stage's routes refuse, come after:
//!   read by ranges (`take_aside`), checked (each file whole by its CRC, a
//!   SQLite database, within `DB_BYTES_MAX`), and kept in the agent's
//!   SimpleX directory (`staged/`, root's, 0700) with nothing run. The
//!   stage's id is written first (`pending`), so a bridge started again
//!   finishes it.
//! - **Adopted** only at its owner's word, from their Settings (a ticket
//!   the platform signed: `run.rs`, `Handle::adopt`): placed as the core's
//!   databases (`place`), read once in maintenance mode, then the link is
//!   bound to the owner (`link::Input::Adopt`), and the staged copy goes.
//!
//! The old core (v7.0.2) and ours (v7.0.3) are the same database version
//! (`SimpleX Chat v7.0.0.12`, the same migrations: the 2026-11-10 spike),
//! so the files are opened as they are.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::wire::ContactId;
use crate::api::{Api, ApiError};

/// The stage's SimpleX files, by their path in its zip.
pub const CHAT: &str = ".finite-stage/simplex/chat.db";
pub const AGENT: &str = ".finite-stage/simplex/agent.db";
pub const OWNER: &str = ".finite-stage/simplex/owner.json";
const PREFIX: &str = ".finite-stage/simplex/";
/// Where the agent's SimpleX directory keeps a staged identity.
pub const STAGED_DIR: &str = "staged";
/// One database, at most, as it is staged (uncompressed).
pub const DB_BYTES_MAX: u64 = 256 * 1024 * 1024;
/// `owner.json`, at most.
const OWNER_BYTES_MAX: u64 = 1_024;
/// The zip's central directory, at most, as it is read for these entries
/// (a home of many files has a large one).
const DIRECTORY_BYTES_MAX: u64 = 64 * 1024 * 1024;
/// One range read, at most.
const READ_BYTES: u64 = 4 * 1024 * 1024;
/// The zip's end, as read to find its directory: the end record, its
/// comment (at most 64 KiB), and zip64's locator and record.
const TAIL_BYTES: u64 = 22 + 65_535 + 20 + 56;
/// The SQLite header every database starts with.
const SQLITE: &[u8; 16] = b"SQLite format 3\0";

/// Why a stage gave no identity to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The stage is not this agent's to read now (its import ended, or it
    /// never was): nothing more to try.
    Gone(String),
    /// It holds no SimpleX identity.
    Absent,
    /// It holds one that is not whole, or not as the contract says.
    Invalid(String),
    /// The platform did not answer: try again.
    Transient(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Gone(why) => write!(f, "the stage is not readable: {why}"),
            Refusal::Absent => write!(f, "the stage holds no SimpleX identity"),
            Refusal::Invalid(why) => write!(f, "the stage's SimpleX identity is not whole: {why}"),
            Refusal::Transient(why) => write!(f, "the stage did not answer: {why}"),
        }
    }
}

fn invalid(why: impl Into<String>) -> Refusal {
    Refusal::Invalid(why.into())
}

/// A stage's id (`fragment_proto::migrations::parse_stage`'s grammar):
/// `<old>.<p1|p2>.<attempt>`, the old id of letters, digits, `_` and `-`.
pub fn stage_ok(stage: &str) -> bool {
    let mut parts = stage.rsplitn(3, '.');
    let (Some(attempt), Some(phase), Some(old)) = (parts.next(), parts.next(), parts.next()) else { return false };
    let attempt_ok = attempt.parse::<u32>().is_ok_and(|a| a >= 1 && attempt == a.to_string());
    let old_ok = !old.is_empty() && old.len() <= 64 && old.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    attempt_ok && matches!(phase, "p1" | "p2") && old_ok
}

/// The stage the move's routine names (`fragment_core::migrations::
/// routine_text`: "… layout finite-computer, stage <id>."), if `text` is one.
pub fn of_routine(text: &str) -> Option<String> {
    let rest = text.strip_prefix("Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage ")?;
    let stage = rest.strip_suffix('.')?;
    stage_ok(stage).then(|| stage.to_string())
}

/// A staged identity, whole: its stage, and its owner's contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub stage: String,
    pub owner: ContactId,
}

fn staged(dir: &Path) -> PathBuf {
    dir.join(STAGED_DIR)
}

/// The identity `dir` (an agent's SimpleX directory) keeps aside, whole:
/// its ready mark is written last.
pub fn ready(dir: &Path) -> Option<Staged> {
    let s = staged(dir);
    let text = std::fs::read_to_string(s.join("ready.json")).ok()?;
    let r: Staged = serde_json::from_str(&text).ok()?;
    (r.owner > 0 && stage_ok(&r.stage) && s.join("sx_chat.db").is_file() && s.join("sx_agent.db").is_file()).then_some(r)
}

/// The stage a set-aside began on and did not finish (a bridge stopped
/// mid-read), so it is read again.
pub fn pending(dir: &Path) -> Option<String> {
    if ready(dir).is_some() {
        return None;
    }
    let stage = std::fs::read_to_string(staged(dir).join("stage")).ok()?;
    let stage = stage.trim();
    stage_ok(stage).then(|| stage.to_string())
}

/// Writes `stage` as the set-aside under way, before it is read: its
/// folder made, root's alone (0700).
pub fn begin(dir: &Path, stage: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    assert!(stage_ok(stage), "a stage's id is checked before it is kept");
    let s = staged(dir);
    let _ = std::fs::remove_dir_all(&s);
    std::fs::create_dir_all(&s)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o700))?;
    write_whole(&s.join("stage"), stage.as_bytes())
}

/// An adoption's intent, written whole before anything of it is placed
/// (`run.rs`, `adopted`): what its maintenance read found. While it is
/// there, the adoption is finished (the databases placed, the link bound and
/// written) at every start before anything else, so a crash or a failed
/// write anywhere in between is finished, never half done.
pub fn commit<T: Serialize>(dir: &Path, intent: &T) -> std::io::Result<()> {
    write_whole(&staged(dir).join("commit.json"), serde_json::to_string(intent).expect("it serializes").as_bytes())
}

/// The adoption's intent `commit` wrote, while its staged copy is there.
pub fn committed<T: serde::de::DeserializeOwned>(dir: &Path) -> Option<T> {
    ready(dir)?;
    serde_json::from_slice(&std::fs::read(staged(dir).join("commit.json")).ok()?).ok()
}

/// The staged identity let go (adopted, refused, or a link made instead).
pub fn forget(dir: &Path) {
    let _ = std::fs::remove_dir_all(staged(dir));
}

/// Where a staged identity's databases are, as a core's `-d` names them
/// (`<it>_chat.db`, `<it>_agent.db`): the maintenance read runs on them
/// there, before anything is placed.
pub fn core_prefix(dir: &Path) -> PathBuf {
    staged(dir).join("sx")
}

/// The staged identity placed as `dir`'s core's databases (`sx_chat.db`,
/// `sx_agent.db`: `core_args`' `-d <dir>/sx`), each copied whole and
/// renamed into place, replacing any there; the staged copy stays until
/// the adoption is done. A database's journals of a core before are let go.
pub fn place(dir: &Path) -> std::io::Result<()> {
    let s = staged(dir);
    for (from, to) in [("sx_chat.db", "sx_chat.db"), ("sx_agent.db", "sx_agent.db")] {
        for side in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(dir.join(format!("{to}{side}")));
        }
        let tmp = dir.join(format!("{to}.adopting"));
        std::fs::copy(s.join(from), &tmp)?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, dir.join(to))?;
    }
    Ok(())
}

fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The stage's zip, read by ranges as `agent`.
struct Zip<'a> {
    api: &'a Api,
    agent: &'a str,
    stage: &'a str,
    len: u64,
}

fn refusal(e: ApiError) -> Refusal {
    match e.status() {
        Some(403 | 404 | 409 | 410) => Refusal::Gone(e.to_string()),
        _ => Refusal::Transient(e.to_string()),
    }
}

impl Zip<'_> {
    async fn read(&self, from: u64, len: u64) -> Result<bytes::Bytes, Refusal> {
        assert!(len > 0 && len <= READ_BYTES, "a range read is bounded");
        if from.checked_add(len).is_none_or(|end| end > self.len) {
            return Err(invalid("an entry past the zip's end"));
        }
        let (bytes, _) = self.api.stage_range(self.agent, self.stage, from, from + len - 1).await.map_err(refusal)?;
        if bytes.len() as u64 != len {
            return Err(Refusal::Transient(format!("{} bytes of a range of {len}", bytes.len())));
        }
        Ok(bytes)
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

/// Where a zip's central directory is, and how large.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directory {
    pub offset: u64,
    pub size: u64,
}

/// The end record's place in `tail` (the zip's last bytes), and the
/// directory it names; `Err(at)` when it is zip64's, whose record is at
/// `at` (from the zip's start), to be read (`zip64_directory`).
pub fn end_of(tail: &[u8]) -> Result<Result<Directory, u64>, Refusal> {
    let at = (0..tail.len().saturating_sub(21)).rev().find(|i| u32_at(tail, *i) == 0x0605_4b50).ok_or_else(|| invalid("no zip end record"))?;
    let (size, offset) = (u32_at(tail, at + 12), u32_at(tail, at + 16));
    if size != u32::MAX && offset != u32::MAX && !(at >= 20 && u32_at(tail, at - 20) == 0x0706_4b50) {
        return Ok(Ok(Directory { offset: offset.into(), size: size.into() }));
    }
    if at < 20 || u32_at(tail, at - 20) != 0x0706_4b50 {
        return Err(invalid("a zip64 end with no locator"));
    }
    Ok(Err(u64_at(tail, at - 20 + 8)))
}

/// Zip64's end record (56 bytes): the directory it names.
pub fn zip64_directory(record: &[u8]) -> Result<Directory, Refusal> {
    if record.len() < 56 || u32_at(record, 0) != 0x0606_4b50 {
        return Err(invalid("no zip64 end record where its locator says"));
    }
    Ok(Directory { size: u64_at(record, 40), offset: u64_at(record, 48) })
}

/// One file of the zip, as its central directory says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub packed: u64,
    pub size: u64,
    /// Its local header, from the zip's start.
    pub at: u64,
    pub encrypted: bool,
}

/// The whole entries at `buf`'s start (a central directory's bytes), and
/// how many bytes they took; one cut off at its end waits for the next read.
pub fn entries(buf: &[u8]) -> Result<(Vec<Entry>, usize), Refusal> {
    let mut out = vec![];
    let mut i = 0;
    // bounded by `buf`: each entry takes at least 46 bytes
    while i + 46 <= buf.len() {
        if u32_at(buf, i) != 0x0201_4b50 {
            return Err(invalid("a central directory entry of another kind"));
        }
        let (name_len, extra_len, comment_len) = (u16_at(buf, i + 28) as usize, u16_at(buf, i + 30) as usize, u16_at(buf, i + 32) as usize);
        let end = i + 46 + name_len + extra_len + comment_len;
        if end > buf.len() {
            break;
        }
        let name = String::from_utf8_lossy(&buf[i + 46..i + 46 + name_len]).to_string();
        let (mut size, mut packed, mut at) = (u64::from(u32_at(buf, i + 24)), u64::from(u32_at(buf, i + 20)), u64::from(u32_at(buf, i + 42)));
        // zip64's extra field: the values the record could not hold, in order
        let mut extra = &buf[i + 46 + name_len..i + 46 + name_len + extra_len];
        while extra.len() >= 4 {
            let (id, len) = (u16_at(extra, 0), u16_at(extra, 2) as usize);
            let field = extra.get(4..4 + len).ok_or_else(|| invalid("an extra field past its entry"))?;
            if id == 0x0001 {
                let mut vals = field.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c));
                for v in [&mut size, &mut packed, &mut at] {
                    if *v == u64::from(u32::MAX) {
                        *v = vals.next().ok_or_else(|| invalid("a zip64 field short of its values"))?;
                    }
                }
            }
            extra = &extra[4 + len..];
        }
        out.push(Entry { name, method: u16_at(buf, i + 10), crc: u32_at(buf, i + 16), packed, size, at, encrypted: u16_at(buf, i + 8) & 1 == 1 });
        i = end;
    }
    Ok((out, i))
}

/// The stage's three SimpleX entries, as its directory lists them: none
/// when it has none; refused when it has some but not all three, or
/// anything else of SimpleX's (a journal: the contract stages none).
pub fn simplex_entries(all: Vec<Entry>) -> Result<Option<[Entry; 3]>, Refusal> {
    let mine: Vec<Entry> = all.into_iter().filter(|e| e.name.starts_with(PREFIX) && !e.name.ends_with('/')).collect();
    if mine.is_empty() {
        return Ok(None);
    }
    let take = |name: &str| mine.iter().find(|e| e.name == name).cloned();
    if let Some(other) = mine.iter().find(|e| ![CHAT, AGENT, OWNER].contains(&e.name.as_str())) {
        return Err(invalid(format!("an entry the contract does not name: {}", other.name)));
    }
    if mine.len() != 3 {
        return Err(invalid("not exactly the three files the contract names"));
    }
    let (Some(chat), Some(agent), Some(owner)) = (take(CHAT), take(AGENT), take(OWNER)) else { return Err(invalid("a file the contract names is missing")) };
    for e in [&chat, &agent, &owner] {
        if e.encrypted || !matches!(e.method, 0 | 8) {
            return Err(invalid(format!("{} is encrypted or compressed some other way", e.name)));
        }
    }
    if chat.size > DB_BYTES_MAX || agent.size > DB_BYTES_MAX || owner.size > OWNER_BYTES_MAX {
        return Err(invalid("a file larger than a staged identity's"));
    }
    Ok(Some([chat, agent, owner]))
}

/// Reads `owner.json`: the owner's contact.
pub fn owner_of(bytes: &[u8]) -> Result<ContactId, Refusal> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| invalid(format!("owner.json: {e}")))?;
    match v.as_object().map(|o| (o.len(), o.get("contact").and_then(serde_json::Value::as_i64))) {
        Some((1, Some(contact))) if contact > 0 => Ok(contact),
        _ => Err(invalid("owner.json is {\"contact\": <its id>}, nothing else")),
    }
}

/// What an entry's bytes go through to its file: never past its size.
struct Bounded<W: Write> {
    out: W,
    n: u64,
    max: u64,
}

impl<W: Write> Write for Bounded<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.n + buf.len() as u64 > self.max {
            return Err(std::io::Error::other("more bytes than its entry says"));
        }
        self.out.write_all(buf)?;
        self.n += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// One entry, whole, to `to` (renamed into place only once its size and
/// CRC are its directory's); its first 16 bytes for the caller's check.
async fn extract(zip: &Zip<'_>, e: &Entry, to: &Path) -> Result<Vec<u8>, Refusal> {
    let head = zip.read(e.at, 30).await?;
    if u32_at(&head, 0) != 0x0403_4b50 {
        return Err(invalid(format!("{}: no local header where its directory says", e.name)));
    }
    let start = e.at + 30 + u64::from(u16_at(&head, 26)) + u64::from(u16_at(&head, 28));
    let end = start.checked_add(e.packed).ok_or_else(|| invalid("an entry past the zip's end"))?;
    let tmp = to.with_extension("part");
    let file = std::fs::File::create(&tmp).map_err(|err| Refusal::Transient(format!("{}: {err}", tmp.display())))?;
    let bounded = Bounded { out: std::io::BufWriter::new(file), n: 0, max: e.size };
    let (mut stored, mut deflated) = if e.method == 8 { (None, Some(flate2::write::DeflateDecoder::new(bounded))) } else { (Some(bounded), None) };
    let mut from = start;
    // bounded by the entry's packed size, a read at a time
    while from < end {
        let n = (end - from).min(READ_BYTES);
        let bytes = zip.read(from, n).await?;
        let wrote = match (&mut stored, &mut deflated) {
            (Some(w), _) => w.write_all(&bytes),
            (_, Some(d)) => d.write_all(&bytes),
            _ => unreachable!("one way or the other"),
        };
        wrote.map_err(|err| invalid(format!("{}: {err}", e.name)))?;
        from += n;
    }
    let mut out = match (stored, deflated) {
        (Some(w), _) => w,
        (_, Some(d)) => d.finish().map_err(|err| invalid(format!("{}: {err}", e.name)))?,
        _ => unreachable!("one way or the other"),
    };
    out.flush().map_err(|err| Refusal::Transient(err.to_string()))?;
    drop(out);
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    let (crc, head) = file_crc(&tmp).map_err(|err| Refusal::Transient(err.to_string()))?;
    if size != e.size || crc != e.crc {
        let _ = std::fs::remove_file(&tmp);
        return Err(invalid(format!("{}: not whole (its size or CRC)", e.name)));
    }
    std::fs::rename(&tmp, to).map_err(|err| Refusal::Transient(err.to_string()))?;
    Ok(head)
}

/// A file's CRC-32 and its first 16 bytes.
fn file_crc(path: &Path) -> std::io::Result<(u32, Vec<u8>)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut crc = crc32fast::Hasher::new();
    let mut head = vec![];
    let mut buf = vec![0u8; 1 << 16];
    // bounded by the file, which `Checked` held to its entry's size
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if head.len() < 16 {
            head.extend_from_slice(&buf[..n.min(16 - head.len())]);
        }
        crc.update(&buf[..n]);
    }
    Ok((crc.finalize(), head))
}

/// The stage's SimpleX identity, set aside in `dir`'s `staged/` (begun by
/// `begin`): its directory read for the three entries, each read whole and
/// checked, its ready mark written last.
pub async fn take_aside(api: &Api, agent: &str, stage: &str, dir: &Path) -> Result<Staged, Refusal> {
    let (_, len) = api.stage_range(agent, stage, 0, 0).await.map_err(refusal)?;
    let zip = Zip { api, agent, stage, len };
    let tail_len = TAIL_BYTES.min(len);
    let mut tail = Vec::with_capacity(tail_len as usize);
    let mut at = len - tail_len;
    // bounded: the tail is a few reads at most
    while at < len {
        let n = (len - at).min(READ_BYTES);
        tail.extend_from_slice(&zip.read(at, n).await?);
        at += n;
    }
    let dir_at = match end_of(&tail)? {
        Ok(d) => d,
        Err(record) => zip64_directory(&zip.read(record, 56).await?)?,
    };
    if dir_at.size > DIRECTORY_BYTES_MAX {
        return Err(invalid("a central directory larger than is read"));
    }
    let (mut all, mut buf, mut from) = (vec![], Vec::new(), dir_at.offset);
    let end = dir_at.offset.checked_add(dir_at.size).ok_or_else(|| invalid("a directory past the zip's end"))?;
    // bounded by DIRECTORY_BYTES_MAX, a read at a time
    while from < end {
        let n = (end - from).min(READ_BYTES);
        buf.extend_from_slice(&zip.read(from, n).await?);
        from += n;
        let (found, used) = entries(&buf)?;
        all.extend(found.into_iter().filter(|e| e.name.starts_with(PREFIX)));
        buf.drain(..used);
    }
    if !buf.is_empty() {
        return Err(invalid("a central directory cut short"));
    }
    let Some([chat, agent_db, owner]) = simplex_entries(all)? else { return Err(Refusal::Absent) };
    let s = staged(dir);
    let owner_file = s.join("owner.json");
    extract(&zip, &owner, &owner_file).await?;
    let owner = owner_of(&std::fs::read(&owner_file).map_err(|e| Refusal::Transient(e.to_string()))?)?;
    for (e, name) in [(&chat, "sx_chat.db"), (&agent_db, "sx_agent.db")] {
        let head = extract(&zip, e, &s.join(name)).await?;
        if head.as_slice() != SQLITE {
            return Err(invalid(format!("{} is no plain SQLite database", e.name)));
        }
    }
    let staged = Staged { stage: stage.to_string(), owner };
    write_whole(&s.join("ready.json"), serde_json::to_string(&staged).expect("it serializes").as_bytes()).map_err(|e| Refusal::Transient(e.to_string()))?;
    Ok(staged)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A zip of `files` (stored, or deflated), its directory's records in
    /// zip64's form when `zip64` (as the mover writes one): the shapes
    /// `take_aside` reads, made here so each is tested.
    pub(crate) fn zip(files: &[(&str, &[u8])], deflate: bool, zip64: bool) -> Vec<u8> {
        let mut out = vec![];
        let mut central = vec![];
        for (name, data) in files {
            let mut crc = crc32fast::Hasher::new();
            crc.update(data);
            let crc = crc.finalize();
            let packed = if deflate {
                let mut e = flate2::write::DeflateEncoder::new(vec![], flate2::Compression::default());
                e.write_all(data).expect("deflated");
                e.finish().expect("deflated")
            } else {
                data.to_vec()
            };
            let at = out.len() as u64;
            let method: u16 = if deflate { 8 } else { 0 };
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[45, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            let big = |v: u64| if zip64 { u32::MAX } else { v as u32 };
            let extra: Vec<u8> = if zip64 {
                let mut x = vec![1, 0, 24, 0];
                for v in [data.len() as u64, packed.len() as u64, at] {
                    x.extend_from_slice(&v.to_le_bytes());
                }
                x
            } else {
                vec![]
            };
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[45, 3, 45, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&big(packed.len() as u64).to_le_bytes());
            central.extend_from_slice(&big(data.len() as u64).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&big(at).to_le_bytes());
            central.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&extra);
        }
        let (dir_at, dir_len, n) = (out.len() as u64, central.len() as u64, files.len() as u64);
        out.extend_from_slice(&central);
        if zip64 {
            let rec = out.len() as u64;
            out.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
            out.extend_from_slice(&44u64.to_le_bytes());
            out.extend_from_slice(&[45, 3, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            for v in [n, n, dir_len, dir_at] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&rec.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        let short = |v: u64, max: u64| if zip64 { max } else { v };
        out.extend_from_slice(&(short(n, 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&(short(n, 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&(short(dir_len, 0xFFFF_FFFF) as u32).to_le_bytes());
        out.extend_from_slice(&(short(dir_at, 0xFFFF_FFFF) as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// The directory's entries of `z`, read as `take_aside` reads them.
    fn listed(z: &[u8]) -> Vec<Entry> {
        let tail = &z[z.len().saturating_sub(TAIL_BYTES as usize)..];
        let d = match end_of(tail).expect("an end") {
            Ok(d) => d,
            Err(rec) => zip64_directory(&z[rec as usize..]).expect("zip64's record"),
        };
        let (e, used) = entries(&z[d.offset as usize..(d.offset + d.size) as usize]).expect("entries");
        assert_eq!(used as u64, d.size, "every entry read");
        e
    }

    fn db(n: usize) -> Vec<u8> {
        let mut v = SQLITE.to_vec();
        v.extend((0..n).map(|i| (i % 251) as u8));
        v
    }

    /// Goal: the move's routine names its stage, read back exactly; any
    /// other text, or a stage of another grammar, names none.
    #[test]
    fn the_routine_names_its_stage() {
        // fragment_core::migrations::routine_text's words, to the letter
        let routine = "Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage u_0a-9Z.p1.2.";
        assert_eq!(of_routine(routine).as_deref(), Some("u_0a-9Z.p1.2"));
        for not in ["remind me to water the basil", "Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage ../x.p1.1.", "Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage a.p3.1.", "Bring yourself over from finite.computer: use the import-agent-home skill, layout finite-computer, stage a.p1.01."] {
            assert_eq!(of_routine(not), None, "{not}");
        }
        assert!(stage_ok("abc.p2.10") && !stage_ok("abc.p1.0") && !stage_ok("p1.1") && !stage_ok(&format!("{}.p1.1", "a".repeat(65))));
    }

    /// Goal: a zip's directory is read as the mover writes it (zip64, its
    /// sizes and offsets in the extra field) and as a plain zip; an entry
    /// cut off at a read's end waits for the next read.
    #[test]
    fn a_zips_directory_reads_in_either_form() {
        let files: [(&str, &[u8]); 3] = [("workspace/a.txt", b"hello"), (CHAT, &db(1_000)), (OWNER, br#"{"contact":3}"#)];
        for zip64 in [false, true] {
            for deflate in [false, true] {
                let z = zip(&files, deflate, zip64);
                let e = listed(&z);
                assert_eq!(e.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), [files[0].0, files[1].0, files[2].0]);
                assert_eq!((e[1].size, e[1].method), (1_016, if deflate { 8 } else { 0 }));
                assert_eq!(&z[e[0].at as usize..e[0].at as usize + 4], &0x0403_4b50u32.to_le_bytes(), "its local header");
            }
        }
        let z = zip(&files, false, true);
        let tail = &z[z.len() - TAIL_BYTES.min(z.len() as u64) as usize..];
        let Err(rec) = end_of(tail).expect("an end") else { panic!("zip64's") };
        let d = zip64_directory(&z[rec as usize..]).expect("zip64's record");
        let whole = &z[d.offset as usize..(d.offset + d.size) as usize];
        let (first, used) = entries(&whole[..whole.len() - 3]).expect("entries");
        assert_eq!(first.len(), 2, "the third, cut off, waits");
        let (rest, _) = entries(&whole[used..]).expect("the rest");
        assert_eq!(rest[0].name, OWNER);
        assert!(end_of(b"not a zip at all, not even close").is_err());
    }

    /// Goal (the contract): only the three files whole are an identity; a
    /// stage of none is no identity; a journal, a missing file, an
    /// encrypted or oversize one, or an owner of another shape is refused.
    #[test]
    fn only_the_contracts_three_files_are_an_identity() {
        let three = |extra: &[(&str, &[u8])]| {
            let mut f: Vec<(&str, &[u8])> = vec![("workspace/a", b"x")];
            f.extend_from_slice(extra);
            listed(&zip(&f, true, true))
        };
        let (chat, agent) = (db(10), db(20));
        let ok = three(&[(CHAT, &chat), (AGENT, &agent), (OWNER, b"{\"contact\":3}")]);
        assert!(matches!(simplex_entries(ok), Ok(Some([c, a, o])) if c.name == CHAT && a.name == AGENT && o.name == OWNER));
        assert_eq!(simplex_entries(three(&[])), Ok(None), "no SimpleX: nothing to keep");
        assert!(matches!(simplex_entries(three(&[(CHAT, &chat), (AGENT, &agent)])), Err(Refusal::Invalid(_))), "no owner");
        assert!(matches!(simplex_entries(three(&[(CHAT, &chat), (AGENT, &agent), (OWNER, b"{}"), (".finite-stage/simplex/chat.db-wal", b"w")])), Err(Refusal::Invalid(_))), "a journal");
        let mut enc = three(&[(CHAT, &chat), (AGENT, &agent), (OWNER, b"{}")]);
        enc[1].encrypted = true;
        assert!(simplex_entries(enc).is_err(), "encrypted");
        let mut big = three(&[(CHAT, &chat), (AGENT, &agent), (OWNER, b"{}")]);
        big[2].size = DB_BYTES_MAX + 1;
        assert!(simplex_entries(big).is_err(), "too large");
        assert_eq!(owner_of(br#"{"contact":3}"#), Ok(3));
        for bad in [&br#"{"contact":0}"#[..], br#"{"contact":"3"}"#, br#"{"contact":3,"also":4}"#, b"[3]", b""] {
            assert!(owner_of(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// Goal: a set-aside is pending until its ready mark, which only a
    /// whole identity has; placing it makes the core's databases, its
    /// copy kept until it is forgotten.
    #[test]
    fn a_staged_identity_is_kept_then_placed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("simplex-stage-{}", std::process::id())).join("juniper--k3x9");
        let _ = std::fs::remove_dir_all(&dir);
        begin(&dir, "old1.p1.1").expect("begun");
        assert_eq!(std::fs::metadata(dir.join(STAGED_DIR)).expect("there").permissions().mode() & 0o777, 0o700);
        assert_eq!((pending(&dir).as_deref(), ready(&dir)), (Some("old1.p1.1"), None));
        std::fs::write(dir.join("staged/sx_chat.db"), db(5)).expect("written");
        std::fs::write(dir.join("staged/sx_agent.db"), db(6)).expect("written");
        write_whole(&dir.join("staged/ready.json"), br#"{"stage":"old1.p1.1","owner":3}"#).expect("written");
        assert_eq!(ready(&dir), Some(Staged { stage: "old1.p1.1".into(), owner: 3 }));
        assert_eq!(pending(&dir), None);
        std::fs::write(dir.join("sx_chat.db-wal"), b"an old core's").expect("written");
        place(&dir).expect("placed");
        assert_eq!(std::fs::read(dir.join("sx_chat.db")).expect("placed"), db(5));
        assert_eq!(std::fs::read(dir.join("sx_agent.db")).expect("placed"), db(6));
        assert!(!dir.join("sx_chat.db-wal").exists(), "no journal of the core before");
        assert!(ready(&dir).is_some(), "the staged copy stays until the adoption is done");
        forget(&dir);
        assert_eq!((ready(&dir), pending(&dir)), (None, None));
        let _ = std::fs::remove_dir_all(dir.parent().expect("its root"));
    }
}
