//! A person's export of everything that is theirs (docs/api.md, Export),
//! pure: the archive's layout and its README, its limits, the tar it is
//! written as, the parts it is uploaded in, when a press starts one, and
//! the transcript a chat's records read as. The cell builds it (cell
//! export.rs): in the person's list (`Principal`), on its alarm, reading
//! each fragment and computer through the routes their owner reads them by.
//!
//! **Why tar.** An archive is written front to back in fixed-size parts
//! (R2's multipart upload), and a crash or a restart goes on from the last
//! part written, so a format that needs nothing but what came before suits
//! it: a tar entry is a header that names its size, then its bytes. Zip
//! needs each entry's CRC-32 before its bytes (or a data descriptor after
//! them, which a reader of a stored entry cannot find its end by) and a
//! directory of them all at its end. Every system opens a tar (macOS's
//! Archive Utility, `tar -xf` on Linux, macOS and Windows 10 and later,
//! 7-Zip), and a computer's saves are tars already (`tar+zstd`). It is not
//! compressed: most of its bytes (saves, pictures, audio) are already, and
//! the build stays within one alarm's CPU.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a manifest names its format, and the version of it this writes.
pub const FORMAT: &str = "fragment-export";
pub const VERSION: u32 = 1;

/// The one folder an archive holds, so extracting it spills nothing.
pub const ROOT: &str = "fragment-export";

/// The limits of an export: each enforced, and each a typed failure its
/// view shows (`fragment_proto::export::ExportFailure`).
pub mod limits {
    pub const GIB: u64 = 1024 * 1024 * 1024;
    /// The files one agent's home import brings at most (`fragment home
    /// files`, Paul, FIN-463), into one fragment: that fragment's part of
    /// its person's export holds them with room (`FRAGMENT_MAX_BYTES`), so
    /// whoever imports an agent can still export everything.
    pub const HOME_FILES_MAX_BYTES: u64 = 10 * GIB;
    /// The whole archive: past it, the export fails `too_large`. Four
    /// fragments at their most, so several imported agents' files: in
    /// `PART_BYTES` parts, 8,192 of R2's `R2_PARTS_MAX`.
    pub const ARCHIVE_MAX_BYTES: u64 = 64 * GIB;
    /// One fragment's part of it (its files, database, channels, transcript
    /// and blobs): past it, `too_large`. An imported agent's files, and
    /// room for the rest and every entry's header.
    pub const FRAGMENT_MAX_BYTES: u64 = 16 * GIB;
    const _: () = assert!(HOME_FILES_MAX_BYTES + 2 * GIB <= FRAGMENT_MAX_BYTES && 4 * FRAGMENT_MAX_BYTES <= ARCHIVE_MAX_BYTES, "an imported agent's files fit its fragment's part, and several fragments the archive");
    const _: () = assert!(ARCHIVE_MAX_BYTES.div_ceil(PART_BYTES as u64) <= R2_PARTS_MAX as u64, "an archive at its most is within R2's parts");
    const _: () = assert!(PART_BYTES as u64 >= R2_PART_MIN_BYTES && PART_BYTES as u64 <= R2_PART_MAX_BYTES, "a part is a size R2 takes");
    /// R2's multipart upload: at most 10,000 parts, each 5 MiB to 5 GiB
    /// (all but the last).
    pub const R2_PARTS_MAX: usize = 10_000;
    pub const R2_PART_MIN_BYTES: u64 = 5 * 1024 * 1024;
    pub const R2_PART_MAX_BYTES: u64 = 5 * GIB;
    /// Fragments exported (the person's own and their agents'): past it, `too_many`.
    pub const FRAGMENTS_MAX: usize = 1000;
    /// Fragments named under `sharedWithMe`: past it, `too_many`.
    pub const SHARED_MAX: usize = 2000;
    /// Files one fragment's main holds: past it, `too_many`. An imported
    /// agent's files are as many at most (`fragment home files` takes its
    /// count from here). Not more: a fragment lists its files whole
    /// (`GET /api/f/{name}/files`), in its object's memory and the build's.
    pub const FILES_MAX: usize = 20_000;
    /// Blobs one fragment's records attach: past it, `too_many`.
    pub const BLOBS_MAX: usize = 10_000;
    /// Tables in one app's database: past it, `too_many`.
    pub const TABLES_MAX: usize = 256;
    /// Entries in the archive (`tar`): past it, `too_many`. A computer's
    /// 16 agents, each with an imported agent's files (`FILES_MAX`), and
    /// room for the rest.
    pub const ENTRIES_MAX: u32 = 500_000;
    const _: () = assert!(16 * FILES_MAX + 100_000 <= ENTRIES_MAX as usize, "a computer's agents' files fit the archive's entries");
    /// Exports a person starts in any 24 hours: one more is 429.
    pub const PER_DAY: usize = 3;
    pub const DAY_MS: i64 = 24 * 3600 * 1000;
    /// How long a ready archive is kept, then deleted.
    pub const KEPT_MS: i64 = 7 * DAY_MS;
    /// How long a build may take: past it, `timed_out`. An archive at its
    /// most at 1 MB/s, and its entries at about 6 a second; well within the
    /// 7 days R2 keeps an upload unfinished.
    pub const BUILD_MAX_MS: i64 = 24 * 3600 * 1000;
    /// The archive's parts (R2's multipart upload: at least 5 MiB, all but
    /// the last the same size): what the build holds of it at most.
    pub const PART_BYTES: usize = 8 * 1024 * 1024;
    /// Most of an entry of unknown size (a channel's records, a table's
    /// rows) the build holds before it puts it aside in R2, and most of a
    /// stored source (a save, a blob) it reads at once.
    pub const PIECE_BYTES: usize = 8 * 1024 * 1024;
    /// One alarm's work, then its place is kept and the next alarm goes on.
    pub const SLICE_MS: i64 = 20_000;
    /// Tries of a part that fails for a reason that may pass, then the
    /// export fails `unavailable`.
    pub const ATTEMPTS_MAX: u32 = 8;
    /// A computer on a disk keeps no save of its own: its export asks for
    /// one, taken awake (docs/computers.md, "A disk"), and looks again
    /// every `COMPUTER_WAIT_MS`, until `COMPUTER_WAIT_MAX_MS` from its first
    /// look (about a start's deadline and its save's), then goes on without
    /// it, saying so.
    pub const COMPUTER_WAIT_MS: i64 = 20_000;
    pub const COMPUTER_WAIT_MAX_MS: i64 = 20 * 60_000;
    /// Records read from a channel a page (the platform cuts a page at
    /// about 4 MiB of bodies, `limits::CHANNEL_HTTP_PAGE_MAX_BYTES`).
    pub const CHANNEL_PAGE: usize = 100;
    /// Rows of a table read a page, and the most of their JSON a page holds.
    pub const TABLE_PAGE: usize = 1000;
    pub const TABLE_PAGE_MAX_BYTES: usize = 4 * 1024 * 1024;
    /// Exports a person's list keeps the rows of (for the day's count).
    pub const KEPT_ROWS: usize = 16;
}

// ---- the layout ----

/// `README.md`, at the root.
pub fn readme_path() -> String {
    format!("{ROOT}/README.md")
}

/// `manifest.json`, at the root.
pub fn manifest_path() -> String {
    format!("{ROOT}/manifest.json")
}

/// A fragment's folder.
pub fn fragment_dir(name: &str) -> String {
    assert!(fragment_proto::valid_fragment_name(name), "a fragment's folder is its name");
    format!("{ROOT}/fragments/{name}")
}

/// One of its files, at its path in git.
pub fn file_path(fragment: &str, path: &str) -> String {
    assert!(fragment_proto::valid_repo_path(path), "a file's path is a repo path");
    format!("{}/files/{path}", fragment_dir(fragment))
}

pub fn meta_path(fragment: &str) -> String {
    format!("{}/meta.json", fragment_dir(fragment))
}

pub fn schema_path(fragment: &str) -> String {
    format!("{}/database/schema.sql", fragment_dir(fragment))
}

/// A table's rows; its name made a file's (`segment`).
pub fn table_path(fragment: &str, table: &str) -> String {
    format!("{}/database/{}.jsonl", fragment_dir(fragment), segment(table))
}

pub fn channel_path(fragment: &str, channel: &str) -> String {
    assert!(fragment_proto::valid_channel_name(channel), "a channel's file is its name");
    format!("{}/channels/{channel}.jsonl", fragment_dir(fragment))
}

pub fn transcript_path(fragment: &str) -> String {
    format!("{}/transcript.md", fragment_dir(fragment))
}

/// A blob its records attach, by its SHA-256.
pub fn blob_path(fragment: &str, sha: &str) -> String {
    assert!(crate::blob::valid_sha(sha), "a blob's file is its hash");
    format!("{}/blobs/{sha}", fragment_dir(fragment))
}

/// A computer's folder: its id's 24 hex (a `:` is no file name everywhere).
pub fn computer_dir(id: &str) -> String {
    let hex = id.strip_prefix("computer:").filter(|h| h.len() == 24 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    format!("{ROOT}/computers/{}", hex.expect("a computer's id is computer:<24 hex>"))
}

pub fn computer_meta_path(id: &str) -> String {
    format!("{}/meta.json", computer_dir(id))
}

/// A save's record of `dir` (`/data` → `data.tar.zst`, `/data/work` →
/// `data-work.tar.zst`), as stored: a tar compressed with zstd.
pub fn save_path(id: &str, dir: &str) -> String {
    let name = dir.trim_matches('/').split('/').map(segment).collect::<Vec<_>>().join("-");
    format!("{}/{}.tar.zst", computer_dir(id), if name.is_empty() { "root".to_string() } else { name })
}

/// A record of the newest save set aside (docs/computers.md, "Saves and
/// what a wake restores"), beside the save exported: `set-aside/`, named
/// as `save_path` names its records.
pub fn set_aside_path(id: &str, dir: &str) -> String {
    let file = save_path(id, dir);
    let name = file.rsplit('/').next().expect("a save's record names a file");
    format!("{}/set-aside/{name}", computer_dir(id))
}

/// `path` as the JSON file in `dir` names it: relative to that folder (a
/// manifest's to the root, a meta's to its fragment's or computer's).
pub fn within(path: &str, dir: &str) -> String {
    let rest = path.strip_prefix(dir).and_then(|r| r.strip_prefix('/'));
    rest.expect("a file named is under the folder that names it").to_string()
}

/// A name made one path segment: ASCII letters, digits, `.`, `_` and `-`
/// as they are, any other byte as `%XX` (so `.`, `..` and the empty name,
/// which name no file, are written `%2E`, `%2E%2E` and `%`).
pub fn segment(name: &str) -> String {
    if name.is_empty() {
        return "%".into();
    }
    if name == "." || name == ".." {
        return name.replace('.', "%2E");
    }
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---- tar ----

/// A tar block.
pub const BLOCK: usize = 512;

/// The archive's end: two empty blocks.
pub const END: [u8; 2 * BLOCK] = [0; 2 * BLOCK];

/// The largest entry a ustar header's size field holds (11 octal digits,
/// 8 GiB less a byte); a larger one (a computer's save may be) is named by
/// a pax `size` record.
const USTAR_SIZE_MAX: u64 = 0o777_7777_7777;

/// The zeros after an entry's `size` bytes, to its block's end.
pub fn padding(size: u64) -> usize {
    ((BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64) as usize
}

/// An entry's header: one ustar block, or, for a path ustar cannot hold
/// (past its 100 + 155 bytes, or not ASCII) or a size past its field's, a
/// pax header naming them first. A regular file, `0644`, modified at
/// `mtime_s`.
pub fn header(path: &str, size: u64, mtime_s: u64) -> Vec<u8> {
    assert!(!path.is_empty() && !path.starts_with('/') && !path.split('/').any(|s| s.is_empty() || s == "." || s == ".."), "an entry's path is relative, with no empty, . or .. segment: {path:?}");
    let fits = size <= USTAR_SIZE_MAX;
    if let (Some((prefix, name)), true) = (ustar_split(path), fits) {
        return ustar(name, prefix, size, mtime_s, b'0').to_vec();
    }
    // a pax header: its records name the path in UTF-8 and, past ustar's
    // field, the size (the entry's own header then says 0)
    let record = match fits {
        true => pax_record("path", path),
        false => pax_record("path", path) + &pax_record("size", &size.to_string()),
    };
    let size = if fits { size } else { 0 };
    let mut out = ustar(&format!("{ROOT}/PaxHeader"), "", record.len() as u64, mtime_s, b'x').to_vec();
    out.extend_from_slice(record.as_bytes());
    out.resize(out.len() + padding(record.len() as u64), 0);
    // what a reader without pax finds: the path's ASCII, cut to fit
    let plain: String = path.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '_' }).collect();
    let cut = &plain[plain.len().saturating_sub(100)..];
    out.extend_from_slice(&ustar(cut.trim_start_matches('/'), "", size, mtime_s, b'0'));
    assert_eq!(out.len() % BLOCK, 0, "a header is whole blocks");
    out
}

/// A path as ustar holds one: all of it as its name (at most 100 bytes), or
/// split at a `/` into a prefix (at most 155) and a name; `None` when it
/// cannot be, or is not ASCII.
fn ustar_split(path: &str) -> Option<(&str, &str)> {
    if !path.is_ascii() || path.bytes().any(|b| b.is_ascii_control()) {
        return None;
    }
    if path.len() <= 100 {
        return Some(("", path));
    }
    path.match_indices('/').map(|(i, _)| (&path[..i], &path[i + 1..])).find(|(p, n)| p.len() <= 155 && n.len() <= 100 && !n.is_empty())
}

/// One pax record, `<length> <key>=<value>\n`, its length counting itself.
fn pax_record(key: &str, value: &str) -> String {
    let rest = key.len() + value.len() + 3;
    let mut len = rest + 1;
    // bounded: the length's digits settle within two rounds
    for _ in 0..3 {
        let total = rest + len.to_string().len();
        if total == len {
            break;
        }
        len = total;
    }
    let record = format!("{len} {key}={value}\n");
    assert_eq!(record.len(), len, "a pax record's length counts itself");
    record
}

fn ustar(name: &str, prefix: &str, size: u64, mtime_s: u64, kind: u8) -> [u8; BLOCK] {
    assert!(name.len() <= 100 && prefix.len() <= 155, "ustar's fields hold them");
    let mut h = [0u8; BLOCK];
    let put = |h: &mut [u8; BLOCK], at: usize, bytes: &[u8]| h[at..at + bytes.len()].copy_from_slice(bytes);
    put(&mut h, 0, name.as_bytes());
    put(&mut h, 100, b"0000644\0");
    put(&mut h, 108, b"0000000\0");
    put(&mut h, 116, b"0000000\0");
    put(&mut h, 124, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, format!("{:011o}\0", mtime_s.min(USTAR_SIZE_MAX)).as_bytes());
    put(&mut h, 148, b"        ");
    h[156] = kind;
    put(&mut h, 257, b"ustar\0");
    put(&mut h, 263, b"00");
    put(&mut h, 345, prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    put(&mut h, 148, format!("{sum:06o}\0 ").as_bytes());
    h
}

// ---- the parts ----

/// What the build holds of the archive that is not uploaded yet: whole
/// parts leave it as they fill (`full`), all the same size, and its last,
/// smaller one leaves it at the end. It is what each alarm keeps between
/// alarms, so it is never more than a part.
#[derive(Debug, Default)]
pub struct Tail {
    part: usize,
    bytes: Vec<u8>,
}

impl Tail {
    pub fn new(part: usize, kept: Vec<u8>) -> Tail {
        assert!(part > 0 && kept.len() < part, "a tail kept is less than a part");
        Tail { part, bytes: kept }
    }

    /// Takes `data`, answering the whole parts it filled, in order.
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.bytes.extend_from_slice(data);
        let mut parts = Vec::with_capacity(self.bytes.len() / self.part);
        // bounded: each round takes a part's bytes
        while self.bytes.len() >= self.part {
            let rest = self.bytes.split_off(self.part);
            parts.push(std::mem::replace(&mut self.bytes, rest));
        }
        assert!(self.bytes.len() < self.part && parts.iter().all(|p| p.len() == self.part), "only whole parts leave, and less than one stays");
        parts
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// What it holds: kept between alarms, or the archive's last part.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

// ---- starting one ----

/// An export a person's list keeps, as a press reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begun {
    pub id: String,
    pub started_at: i64,
    pub building: bool,
}

/// What a press of "Export everything" does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// One is building: the same one, nothing started.
    Replay(String),
    /// `limits::PER_DAY` started in the last 24 hours: the next may start at `next_at`.
    Refused { next_at: i64 },
    /// A new one.
    New,
}

/// What a press does, at `now`, given what the list keeps.
pub fn start(begun: &[Begun], now: i64) -> Start {
    if let Some(b) = begun.iter().find(|b| b.building) {
        return Start::Replay(b.id.clone());
    }
    let mut recent: Vec<i64> = begun.iter().map(|b| b.started_at).filter(|at| *at > now - limits::DAY_MS).collect();
    if recent.len() >= limits::PER_DAY {
        recent.sort_unstable();
        // the day's oldest leaves the window first
        return Start::Refused { next_at: recent[recent.len() - limits::PER_DAY] + limits::DAY_MS };
    }
    Start::New
}

/// What an export does after a look at a disk's save (a computer on a disk
/// keeps none of its own: `limits::COMPUTER_WAIT_MS`): look `Again` after a
/// wait; it came (`Done`); or `GiveUp`, its wait from the first look run
/// out, and `drop` the ask when this look woke the computer for it (an
/// ask it would otherwise take later, for no one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerLook {
    Again,
    Done,
    GiveUp { drop: bool },
}

/// `ComputerLook` for a look that answered `pending` (or not), `waited_ms`
/// from the first look as it answered, made `woke` (within the wait, so it
/// may have asked and woken).
pub fn computer_look(pending: bool, waited_ms: i64, woke: bool) -> ComputerLook {
    match (pending, waited_ms < limits::COMPUTER_WAIT_MAX_MS) {
        (false, _) => ComputerLook::Done,
        (true, true) => ComputerLook::Again,
        (true, false) => ComputerLook::GiveUp { drop: woke },
    }
}

/// The wait before a part's next try: 5 s, doubling, at most 5 minutes.
pub fn retry_ms(attempt: u32) -> i64 {
    (5_000i64 << attempt.min(6)).min(300_000)
}

// ---- what it says ----

/// `manifest.json`: what the archive holds, and whose it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub deployment: Deployment,
    /// When the export began (ms), and as an RFC 3339 time.
    pub made_at: i64,
    pub made: String,
    pub export: String,
    pub person: Person,
    /// The person's agents (their npubs).
    pub agents: Vec<String>,
    pub fragments: Vec<Listed>,
    pub computers: Vec<ComputerListed>,
    /// Fragments others own that the person, or an agent of theirs, is in:
    /// named, never copied.
    pub shared_with_me: Vec<Shared>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deployment {
    pub id: String,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Person {
    pub npub: String,
    pub email: Option<String>,
}

/// A fragment the archive holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Listed {
    pub name: String,
    pub kind: fragment_proto::FragmentKind,
    pub title: Option<String>,
    /// Whose it is: the person, or an agent of theirs.
    pub owner: String,
    /// Its folder in the archive.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerListed {
    pub id: String,
    pub path: String,
}

/// A fragment someone else owns, shared with the person or their agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shared {
    pub name: String,
    pub kind: fragment_proto::FragmentKind,
    pub title: Option<String>,
    /// Its owner (an npub), as its status names them; `None` when it would
    /// not say (it was deleted meanwhile).
    pub owner: Option<String>,
    pub role: fragment_proto::Role,
    /// Whose membership it is: the person's, or an agent's (its npub).
    pub member: String,
}

/// Something of a fragment's (or a computer's) the export could not take,
/// and why: a refusal (an agent's fragment's invites, which only its owner
/// reads) or what is not there (no app code, so no database).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub part: String,
    pub why: String,
}

/// A part of a fragment exported, and its file in the archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Counted {
    pub name: String,
    pub count: u64,
    pub file: String,
}

/// A fragment's `meta.json`. `status`, `members`, `invites` and `channels`
/// are what its owner reads at those routes (docs/api.md, Control API),
/// less every token: `shareLink` and `inboxToken` say only whether they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FragmentMeta {
    pub name: String,
    pub kind: fragment_proto::FragmentKind,
    pub title: Option<String>,
    /// The blessed template it runs (`chat`, `agent`, `brain`, `skills`), if one.
    pub template: Option<String>,
    pub npub: Option<String>,
    pub owner: Option<String>,
    pub visibility: Option<fragment_proto::Visibility>,
    /// Its share link opens it (`link` or `public`, with a token).
    pub share_link: bool,
    /// It has an inbox token (its webhooks' door).
    pub inbox_token: bool,
    pub pins: Option<fragment_proto::Pins>,
    /// The code live runs (`code.id` of status), and why the newest was not installed.
    pub code: Option<String>,
    pub code_error: Option<String>,
    /// Its members as its owner's share sheet shows them: a person with
    /// their email and their agents (a share is a person's and their
    /// agents'), an agent with its name (its label), its title and its owner's email.
    pub members: Vec<Value>,
    /// The invites waiting on an email.
    pub invites: Vec<Value>,
    /// Its secrets' names: their values never leave.
    pub secrets: Vec<String>,
    /// Its scheduled records, as its viewers list them (docs/api.md,
    /// "Scheduled records"); absent when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedules: Vec<Value>,
    /// Its files at `ref` (main's commit), and their bytes.
    pub files: FilesCounted,
    /// Its app's tables, their rows counted; `None` without app code.
    pub database: Option<Vec<Counted>>,
    /// Its channels, their records counted.
    pub channels: Vec<Counted>,
    /// Its blobs its records attach, by SHA-256, and their sizes.
    pub blobs: Vec<BlobCounted>,
    pub transcript: Option<String>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesCounted {
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobCounted {
    pub sha256: String,
    pub size: u64,
}

/// A computer's `meta.json`: its view as its owner reads it, and the save
/// exported (`None`: it was never saved), each record's file beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerMeta {
    pub view: Value,
    pub save: Option<SaveExported>,
    /// The newest save set aside, when one is kept: newer files than the
    /// save exported may be in it (its newest save did not fit its disk,
    /// or an operator started it from an older one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_aside: Option<SaveExported>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveExported {
    pub number: u64,
    pub id: String,
    pub at: i64,
    pub generation: u64,
    pub held: bool,
    /// Each `DirectoryBackup` record (`{id, dir, size, sha256, format}`) and its file.
    pub records: Vec<Value>,
}

/// A JSON file's bytes, as the archive holds it: pretty, and ending in a newline.
pub fn json_file<T: Serialize>(v: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(v).expect("export metadata serializes");
    bytes.push(b'\n');
    bytes
}

// ---- time ----

/// `ms` as an RFC 3339 time, to the second, in UTC.
pub fn rfc3339(ms: i64) -> String {
    let (y, mo, d, h, mi, s) = clock(ms);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `ms` as a person reads a time: `2026-10-13 14:02 UTC`.
pub fn readable(ms: i64) -> String {
    let (y, mo, d, h, mi, _) = clock(ms);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")
}

fn clock(ms: i64) -> (i64, u32, u32, i64, i64, i64) {
    let s = ms.div_euclid(1000);
    let (y, mo, d) = crate::cron::civil(s.div_euclid(86_400));
    let t = s.rem_euclid(86_400);
    (y, mo, d, t / 3600, t / 60 % 60, t % 60)
}

// ---- a chat's transcript ----

/// Who a principal is, as a transcript names them: a member's email or an
/// agent's name, a short npub for anyone the fragment no longer names.
/// `members` holds the fragment's members (`principal -> owner` for agents,
/// `None` for people) so relayed and room-heard records are honored only from
/// the keys that may carry them.
#[derive(Debug, Default, Clone)]
pub struct Names {
    pub display: BTreeMap<String, String>,
    pub members: BTreeMap<String, Option<String>>,
}

impl Names {
    pub fn of(&self, principal: &str) -> String {
        if let Some(n) = self.display.get(principal) {
            return n.clone();
        }
        if principal == "platform" {
            return "Fragment".into();
        }
        if principal.starts_with("anon:") {
            return "A visitor".into();
        }
        let shown = crate::npub::display(principal);
        match shown.len() {
            n if n > 20 => format!("{}…{}", &shown[..12], &shown[n - 6..]),
            _ => shown,
        }
    }

    fn relayed_owner<'a>(&self, principal: &str, author: Option<&str>, body: &'a Value) -> Option<(&'a str, &'a str)> {
        if author.is_some_and(|a| a != "door") || self.members.is_empty() {
            return None;
        }
        if body.get("heard").is_some_and(|v| !v.is_null()) {
            return None;
        }
        let r = body.get("relayed")?.as_object()?;
        let via = r.get("via")?.as_str().filter(|v| crate::connectors::id_ok(v))?;
        let target = r.get("for")?.as_str().filter(|f| crate::npub::is_identity(f))?;
        let allowed = match self.members.get(principal) {
            None => true,
            Some(Some(owner)) => owner == target,
            Some(None) => false,
        };
        allowed.then_some((via, target))
    }

    /// Someone a door heard: only on a record the platform stamped a door's,
    /// as the bridge runs it (docs/chat-records.md, "A message heard in a room").
    fn heard_speaker<'a>(&self, principal: &str, author: Option<&str>, body: &'a Value) -> Option<(&'a str, String)> {
        if author != Some("door") || self.members.is_empty() {
            return None;
        }
        if !crate::npub::is_identity(principal) || self.members.contains_key(principal) {
            return None;
        }
        if body.get("relayed").is_some_and(|v| !v.is_null()) {
            return None;
        }
        let h = body.get("heard")?.as_object()?;
        let via = h.get("via")?.as_str().filter(|v| crate::connectors::id_ok(v))?;
        let _from = h.get("from")?.as_str().filter(|f| door_token(f, 128))?;
        let name = one_line(h.get("name")?.as_str()?);
        if name.is_empty() {
            return None;
        }
        Some((via, cut_chars(&name, NAME_MAX_CHARS)))
    }
}

/// One record of a chat, as the transcript reads it.
#[derive(Debug, Clone, Copy)]
pub struct Said<'a> {
    pub channel: &'a str,
    pub seq: i64,
    pub at: i64,
    pub principal: &'a str,
    pub author: Option<&'a str>,
    pub body: &'a Value,
}

impl Said<'_> {
    /// Where it falls in the transcript: by time, a message before the
    /// work of the same moment, then by its place in its channel.
    pub fn order(&self) -> (i64, bool, i64) {
        (self.at, self.channel != "chat", self.seq)
    }
}

/// The transcript's first lines.
pub fn transcript_head(name: &str, title: Option<&str>, made_at: i64) -> String {
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or(name);
    format!(
        "# {}\n\nThe conversation in {name}, as its records hold it (`channels/chat.jsonl` and\n`channels/work.jsonl`), exported {}. Times are UTC; a file attached is in\n`blobs/`, named by its SHA-256.\n",
        one_line(title),
        readable(made_at)
    )
}

/// What one record adds to a transcript (docs/chat-records.md), or nothing
/// for a record a person never reads there (a turn's claim, a menu, a
/// page's own kinds).
pub fn transcript_part(r: &Said, names: &Names) -> Option<String> {
    let who = names.of(r.principal);
    let when = readable(r.at);
    let s = |k: &str| r.body.get(k).and_then(Value::as_str).unwrap_or_default();
    match r.channel {
        "chat" => {
            let kind = r.body.get("kind").and_then(Value::as_str);
            match kind {
                None | Some("message") => {
                    let text = r.body.as_str().unwrap_or_else(|| s("text"));
                    let files = attachments(r.body);
                    if text.trim().is_empty() && files.is_empty() {
                        return None;
                    }
                    let is_turn = r.body.get("turn").and_then(Value::as_str).is_some();
                    let room = if is_turn { RoomParsed::Absent } else { parse_room(r.body) };
                    let (speaker, origin, has_room) = if is_turn {
                        (one_line(&who), None, false)
                    } else if let (RoomParsed::Valid(place), Some((via, name))) = (&room, names.heard_speaker(r.principal, r.author, r.body)) {
                        (escape_md(&name), Some(format!("{} · {}", door_name(via), escape_md(place))), true)
                    } else if let Some((via, owner)) = names.relayed_owner(r.principal, r.author, r.body) {
                        // a relayed message's room only when stamped a door's; a
                        // malformed one is no one's relay either way (speakers.js)
                        let room = match room {
                            RoomParsed::Valid(_) if r.author != Some("door") => RoomParsed::Absent,
                            room => room,
                        };
                        match &room {
                            RoomParsed::Malformed => (one_line(&who), None, false),
                            RoomParsed::Valid(place) => (one_line(&names.of(owner)), Some(format!("via {} · {}", door_name(via), escape_md(place))), true),
                            RoomParsed::Absent => (one_line(&names.of(owner)), Some(format!("via {}", door_name(via))), false),
                        }
                    } else {
                        (one_line(&who), None, false)
                    };
                    let mut out = match origin {
                        Some(o) => format!("\n**{speaker}** ({o}) · {when}\n\n"),
                        None => format!("\n**{speaker}** · {when}\n\n"),
                    };
                    if has_room {
                        let ctx = parse_context(r.body);
                        if !ctx.is_empty() {
                            for (who, line) in ctx {
                                out.push_str(&format!("> **{}:** {}\n", escape_md(&who), escape_md(&line)));
                            }
                            out.push('\n');
                        }
                    }
                    if !text.trim().is_empty() {
                        out.push_str(text.trim_end());
                        out.push('\n');
                    }
                    for f in files {
                        out.push_str(&f);
                    }
                    Some(out)
                }
                Some("stop") => Some(format!("\n*{} stopped a turn · {when}*\n", one_line(&who))),
                Some("prompt_response") => {
                    let said = if s("text").is_empty() { String::new() } else { format!(": {}", one_line(s("text"))) };
                    Some(format!("\n*{} answered {}{said} · {when}*\n", one_line(&who), code(s("option"))))
                }
                Some("command") => Some(format!("\n**{}** · {when}\n\n{}\n", one_line(&who), code(format!("/{} {}", s("command"), s("args")).trim_end()))),
                Some(_) => None,
            }
        }
        "work" => {
            let kind = r.body.get("kind").and_then(Value::as_str)?;
            let line = match kind {
                "turn.step" => {
                    let verb = match s("category") {
                        "shell" => "ran",
                        "web" => "searched the web for",
                        "read" => "read",
                        "write" => "wrote",
                        "browser" => "used the browser:",
                        "image" => "made an image:",
                        "delegate" => "handed off",
                        "memory" => "changed its memory:",
                        _ => "used",
                    };
                    let what = if s("args").is_empty() { s("tool").to_string() } else { s("args").to_string() };
                    let failed = if r.body.get("ok") == Some(&Value::Bool(false)) { " (it failed)" } else { "" };
                    format!("{} {verb} {}{failed}", one_line(&who), code(&what))
                }
                "turn.prompt" => {
                    let options: Vec<&str> = r.body.get("options").and_then(Value::as_array).map(|o| o.iter().filter_map(|x| x["label"].as_str()).collect()).unwrap_or_default();
                    format!("{} asked: {} ({})", one_line(&who), one_line(s("text")), options.join(" / "))
                }
                "turn.prompt.closed" => match s("outcome") {
                    "answered" => format!("Answered {} by {}", code(s("option")), names.of(s("by"))),
                    "expired" => "The question expired".to_string(),
                    _ => "The question was stopped".to_string(),
                },
                "turn.notice" => format!("{}: {}", one_line(&who), one_line(s("text"))),
                "turn.end" => match s("outcome") {
                    "error" => format!("{}'s turn ended: {}", one_line(&who), one_line(s("error"))),
                    "stopped" => format!("{}'s turn was stopped", one_line(&who)),
                    _ => return None,
                },
                _ => return None,
            };
            Some(format!("\n> {line} · {when}\n"))
        }
        _ => None,
    }
}

/// A record's attachments (docs/chat-records.md), each a line naming its file.
fn attachments(body: &Value) -> Vec<String> {
    let Some(list) = body.get("attachments").and_then(Value::as_array) else { return vec![] };
    list.iter()
        .filter_map(|a| {
            let sha = a.get("sha256")?.as_str().filter(|s| crate::blob::valid_sha(s))?;
            let name = a.get("name").and_then(Value::as_str).map(one_line).unwrap_or_else(|| sha[..12].to_string());
            let kind = a.get("type").and_then(Value::as_str).unwrap_or("a file");
            Some(format!("\nAttached: [{name}](blobs/{sha}) ({})\n", one_line(kind)))
        })
        .collect()
}

/// A text on one line, for a line of markdown.
fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Escapes inline Markdown and HTML metacharacters so untrusted outside display
/// names, room names, and quoted context cannot break transcript formatting or
/// inject links, images, or raw HTML.
fn escape_md(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '<' | '>' | '!' | '~' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn cut_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_none() {
        head
    } else {
        let mut s: String = head.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn door_name(via: &str) -> &str {
    match via {
        "simplex" => "SimpleX",
        "telegram" => "Telegram",
        "slack" => "Slack",
        other => other,
    }
}

/// A door's id for a room (at most 64) or a speaker (at most 128), by the
/// record's own rule (docs/chat-records.md, "A message heard in a room"),
/// as the bridge and the chat page read it.
fn door_token(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A name a door gives (a room's, a speaker's), cut at this many characters.
const NAME_MAX_CHARS: usize = 60;

enum RoomParsed {
    Absent,
    Malformed,
    Valid(String),
}

fn parse_room(body: &Value) -> RoomParsed {
    let Some(v) = body.get("room").filter(|v| !v.is_null()) else {
        return RoomParsed::Absent;
    };
    let Some(r) = v.as_object() else { return RoomParsed::Malformed };
    let Some(id) = r.get("id").and_then(Value::as_str).filter(|s| door_token(s, 64)) else {
        return RoomParsed::Malformed;
    };
    let name = match r.get("name") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => cut_chars(&one_line(s), NAME_MAX_CHARS),
        Some(_) => return RoomParsed::Malformed,
    };
    // `thread` is the door's own, for its replies: read by no one here
    RoomParsed::Valid(if name.is_empty() { id.to_string() } else { name })
}

fn parse_context(body: &Value) -> Vec<(String, String)> {
    const MAX_MESSAGES: usize = 20;
    const MAX_BYTES: usize = 16 * 1024;
    let Some(items) = body.get("context").and_then(Value::as_array) else {
        return vec![];
    };
    if items.is_empty() || items.len() > MAX_MESSAGES {
        return vec![];
    }
    let mut bytes = 0usize;
    let mut out = Vec::with_capacity(items.len());
    for c in items {
        let Some(o) = c.as_object() else { return vec![] };
        let (Some(raw_name), Some(raw_text)) = (o.get("name").and_then(Value::as_str), o.get("text").and_then(Value::as_str)) else {
            return vec![];
        };
        if !matches!(o.get("at"), None | Some(Value::Null)) && o.get("at").and_then(Value::as_i64).is_none() {
            return vec![];
        }
        let name = one_line(raw_name);
        let text = one_line(raw_text);
        if name.is_empty() || text.is_empty() {
            return vec![];
        }
        bytes = bytes.saturating_add(raw_name.len()).saturating_add(raw_text.len());
        if bytes > MAX_BYTES {
            return vec![];
        }
        out.push((cut_chars(&name, NAME_MAX_CHARS), cut_chars(&text, 4000)));
    }
    out
}

/// A code span around `text`, fenced past any run of backticks in it.
fn code(text: impl AsRef<str>) -> String {
    let text = one_line(text.as_ref());
    let mut run = 0;
    let mut longest = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{text}{pad}{fence}")
}

// ---- the README ----

// `README.md` (`readme_path`): the archive's layout and every format in
// it, the same in every export (its manifest says whose, from where, and
// when). Its text is `export_readme.md`, a release file in the Worker's
// Static Assets (crates/templates, `EXPORT_README`), which the cell reads
// as it writes an archive: not compiled into the cell's Wasm (its budget:
// FIN-327).

#[cfg(test)]
mod tests {

    /// Goal: an export's look at a disk's save gives up at its bound, and a
    /// give-up after a look that woke the computer drops that ask, so no
    /// later moment takes a save for no one. Method: a look within the
    /// wait, one past it that woke (sent within it, answered after), one
    /// past it that woke nothing, and one that found the save.
    #[test]
    fn a_disk_saves_look_gives_up_at_its_bound_and_drops_the_ask_it_made() {
        let max = limits::COMPUTER_WAIT_MAX_MS;
        assert_eq!(computer_look(true, max - 1, true), ComputerLook::Again);
        assert_eq!(computer_look(true, max, true), ComputerLook::GiveUp { drop: true }, "sent within the wait, answered past it: its ask goes");
        assert_eq!(computer_look(true, max + 1, false), ComputerLook::GiveUp { drop: false }, "the last look asked nothing, and dropped it itself");
        assert_eq!(computer_look(false, max + 1, true), ComputerLook::Done, "the save came, late or not");
    }
    /// The README, as the release's file holds it.
    const README: &str = include_str!("export_readme.md");
    use super::*;
    use std::io::Read;

    const NAME: &str = "todo--k3x9";

    /// An archive of `entries`, as the build writes one: each header, its
    /// bytes and their padding, then the end, through parts of `part`.
    fn archive(entries: &[(String, Vec<u8>)], part: usize) -> (Vec<u8>, Vec<usize>) {
        let mut tail = Tail::new(part, vec![]);
        let (mut out, mut sizes) = (Vec::new(), Vec::new());
        let mut take = |parts: Vec<Vec<u8>>, out: &mut Vec<u8>| {
            for p in parts {
                sizes.push(p.len());
                out.extend(p);
            }
        };
        for (path, bytes) in entries {
            let parts = tail.push(&header(path, bytes.len() as u64, 1_791_000_000));
            take(parts, &mut out);
            take(tail.push(bytes), &mut out);
            take(tail.push(&vec![0; padding(bytes.len() as u64)]), &mut out);
        }
        take(tail.push(&END), &mut out);
        let last = tail.into_bytes();
        sizes.push(last.len());
        out.extend(last);
        (out, sizes)
    }

    fn read(tar_bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut a = tar::Archive::new(tar_bytes);
        a.entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let path = e.path().unwrap().to_string_lossy().to_string();
                let mut b = vec![];
                e.read_to_end(&mut b).unwrap();
                (path, b)
            })
            .collect()
    }

    /// Goal: what the build writes is a tar every reader takes, with each
    /// path and its bytes as written: short ones, one only ustar's prefix
    /// holds, one past both (pax), one not ASCII, empty and odd sizes.
    #[test]
    fn a_tar_of_every_path() {
        let deep = format!("a/{}/b.txt", "d".repeat(140));
        let past = format!("{}/x", "p".repeat(300));
        let entries = vec![
            (readme_path(), README.as_bytes().to_vec()),
            (manifest_path(), b"{}\n".to_vec()),
            (file_path(NAME, "index.html"), b"<p>hi</p>\n".to_vec()),
            (file_path(NAME, &deep), vec![7; 513]),
            (file_path(NAME, "notes/caf\u{e9} \u{1f600}.md"), b"accents".to_vec()),
            (format!("{ROOT}/{past}"), vec![]),
            (blob_path(NAME, &"a".repeat(64)), vec![1; 1024]),
        ];
        let (bytes, sizes) = archive(&entries, 5 * BLOCK);
        assert_eq!(bytes.len() % BLOCK, 0, "an archive is whole blocks");
        assert!(sizes[..sizes.len() - 1].iter().all(|s| *s == 5 * BLOCK), "every part but the last is a part: {sizes:?}");
        assert!(*sizes.last().unwrap() <= 5 * BLOCK);
        assert_eq!(read(&bytes), entries, "every entry reads back, path and bytes");
        // a reader without pax still finds an entry in ASCII
        let pax = header(&file_path(NAME, "caf\u{e9}"), 3, 0);
        assert_eq!(pax.len(), 3 * BLOCK, "a pax header, its record, then the entry's header");
        assert_eq!(pax[156], b'x');
        assert_eq!(pax[2 * BLOCK + 156], b'0');
    }

    /// Goal: an entry past ustar's size field (8 GiB less a byte: a
    /// computer's save may be) is named by a pax `size` record, which a
    /// reader takes over the header's; one within it is plain ustar.
    #[test]
    fn an_entry_past_ustar_size() {
        let big = USTAR_SIZE_MAX + 1 + 12345;
        for path in [save_path(&format!("computer:{}", "ab".repeat(12)), "/data"), file_path(NAME, "caf\u{e9}.bin")] {
            let h = header(&path, big, 7);
            assert_eq!(h.len() % BLOCK, 0, "a header is whole blocks");
            let mut archive = tar::Archive::new(std::io::Cursor::new(h));
            let mut entries = archive.entries().unwrap();
            let entry = entries.next().expect("one entry").unwrap();
            assert_eq!((entry.path().unwrap().to_string_lossy().to_string(), entry.size()), (path.clone(), big), "the pax record's path and size are the entry's");
        }
        assert_eq!(header(&readme_path(), USTAR_SIZE_MAX, 0).len(), BLOCK, "at ustar's most, one block");
    }

    /// Goal: a header's checksum is ustar's, and a path that is no relative
    /// path (absolute, `..`, empty segments) is never written.
    #[test]
    fn headers_hold_only_relative_paths() {
        let h = header("fragment-export/README.md", 10, 0);
        let field = std::str::from_utf8(&h[148..154]).unwrap();
        let mut blank = h.clone();
        blank[148..156].copy_from_slice(b"        ");
        let sum: u32 = blank.iter().map(|b| u32::from(*b)).sum();
        assert_eq!(u32::from_str_radix(field, 8).unwrap(), sum);
        for bad in ["/etc/passwd", "a/../b", "a//b", "", "./a"] {
            assert!(std::panic::catch_unwind(|| header(bad, 0, 0)).is_err(), "{bad:?} is refused");
        }
        assert_eq!(padding(0), 0);
        assert_eq!(padding(1), 511);
        assert_eq!(padding(512), 0);
        assert_eq!(padding(513), 511);
    }

    /// Goal: a pax record's length counts its own digits, at every boundary.
    #[test]
    fn pax_lengths() {
        for n in 0..300 {
            let r = pax_record("path", &"x".repeat(n));
            let (len, _) = r.split_once(' ').unwrap();
            assert_eq!(len.parse::<usize>().unwrap(), r.len());
        }
    }

    /// Goal: the parts leave whole and the same size, and less than one is
    /// ever held: a tail kept across alarms is under a part.
    #[test]
    fn parts_are_whole() {
        let mut t = Tail::new(4, vec![]);
        assert!(t.push(b"abc").is_empty());
        assert_eq!(t.push(b"defghij"), vec![b"abcd".to_vec(), b"efgh".to_vec()]);
        assert_eq!(t.bytes(), b"ij");
        let mut again = Tail::new(4, t.bytes().to_vec());
        assert_eq!(again.push(b"kl"), vec![b"ijkl".to_vec()]);
        assert!(again.is_empty());
        assert!(std::panic::catch_unwind(|| Tail::new(4, b"abcd".to_vec())).is_err(), "a tail kept is less than a part");
    }

    /// Goal: a press while one builds is that one (replay); a fourth in 24
    /// hours is refused, saying when the next may start; a day later, one
    /// starts again.
    #[test]
    fn presses() {
        let at = |id: &str, t: i64, building: bool| Begun { id: id.into(), started_at: t, building };
        let now = 10 * limits::DAY_MS;
        assert_eq!(start(&[], now), Start::New);
        assert_eq!(start(&[at("a", now - 5, true)], now), Start::Replay("a".into()));
        let three = [at("a", now - 3000, false), at("b", now - 2000, false), at("c", now - 1000, false)];
        assert_eq!(start(&three, now), Start::Refused { next_at: now - 3000 + limits::DAY_MS });
        assert_eq!(start(&three, now - 3000 + limits::DAY_MS), Start::New, "the oldest left the day");
        // a building one is answered even past the day's count
        let mut busy = three.to_vec();
        busy.push(at("d", now - 10, true));
        assert_eq!(start(&busy, now), Start::Replay("d".into()));
        assert_eq!(retry_ms(0), 5_000);
        assert_eq!(retry_ms(3), 40_000);
        assert_eq!(retry_ms(30), 300_000);
    }

    /// Goal: every name becomes one safe path segment, and the layout's
    /// paths are under the one folder.
    #[test]
    fn layout() {
        assert_eq!(segment("notes"), "notes");
        assert_eq!(segment("a/b c"), "a%2Fb%20c");
        assert_eq!(segment(".."), "%2E%2E");
        assert_eq!(segment(""), "%");
        assert_eq!(segment("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(table_path(NAME, "../x"), "fragment-export/fragments/todo--k3x9/database/..%2Fx.jsonl");
        assert_eq!(channel_path(NAME, "chat"), "fragment-export/fragments/todo--k3x9/channels/chat.jsonl");
        let id = format!("computer:{}", "ab".repeat(12));
        assert_eq!(save_path(&id, "/data"), format!("fragment-export/computers/{}/data.tar.zst", "ab".repeat(12)));
        assert_eq!(set_aside_path(&id, "/data/work"), format!("fragment-export/computers/{}/set-aside/data-work.tar.zst", "ab".repeat(12)));
        assert_eq!(save_path(&id, "/data/work"), format!("fragment-export/computers/{}/data-work.tar.zst", "ab".repeat(12)));
        assert!(std::panic::catch_unwind(|| computer_dir("computer:../x")).is_err());
        assert!(std::panic::catch_unwind(|| fragment_dir("../x")).is_err());
        assert!(std::panic::catch_unwind(|| file_path(NAME, "../x")).is_err());
        assert!(std::panic::catch_unwind(|| channel_path(NAME, "../x")).is_err());
    }

    /// Goal: a chat's records read as a conversation: who said what and
    /// when, an attachment's file, each step, a question and its answer, a
    /// turn's error; the records a person never reads there add nothing.
    #[test]
    fn a_transcript() {
        let mut names = Names::default();
        names.display.insert("npub1person".into(), "paul@example.com".into());
        names.display.insert("npub1agent".into(), "Juniper".into());
        names.members.insert("npub1person".into(), None);
        names.members.insert("npub1agent".into(), Some("npub1person".into()));
        let rec = |channel, seq, principal, body: Value| (channel, seq, principal, body);
        let sha = "b".repeat(64);
        let recs = vec![
            rec("chat", 1, "npub1person", serde_json::json!({ "text": "hi @juniper", "attachments": [{ "sha256": sha, "name": "cat.png", "type": "image/png" }] })),
            rec("work", 1, "npub1agent", serde_json::json!({ "kind": "turn.start", "turn": "t" })),
            rec("work", 2, "npub1agent", serde_json::json!({ "kind": "turn.step", "category": "shell", "tool": "terminal", "args": "ls `-la`", "ok": false })),
            rec("work", 3, "npub1agent", serde_json::json!({ "kind": "turn.prompt", "text": "Run it?", "options": [{ "id": "once", "label": "Allow once" }, { "id": "deny", "label": "Deny" }] })),
            rec("work", 4, "npub1agent", serde_json::json!({ "kind": "turn.prompt.closed", "outcome": "answered", "option": "once", "by": "npub1person" })),
            rec("chat", 2, "npub1agent", serde_json::json!({ "text": "Hello!\n\nI am Juniper.", "turn": "t" })),
            rec("work", 5, "npub1agent", serde_json::json!({ "kind": "turn.end", "outcome": "error", "error": "the runtime failed" })),
            rec("work", 6, "npub1agent", serde_json::json!({ "kind": "turn.end", "outcome": "idle" })),
            rec("chat", 3, "npub1gone0000000000000000000000000000000000000000000000000000", serde_json::json!({ "kind": "stop" })),
            rec("chat", 4, "anon:ab", serde_json::json!("a bare string")),
            rec("chat", 5, "npub1person", serde_json::json!({ "kind": "draft-ui-thing" })),
        ];
        let mut out = transcript_head(NAME, Some("Garden"), 1_791_000_000_000);
        for (i, (channel, seq, principal, body)) in recs.iter().enumerate() {
            let said = Said { channel, seq: *seq, at: 1_791_000_000_000 + i as i64 * 60_000, principal, author: None, body };
            out.extend(transcript_part(&said, &names));
        }
        assert!(out.starts_with("# Garden\n"), "{out}");
        assert!(out.contains("**paul@example.com** · 2026-10-03 04:00 UTC\n\nhi @juniper\n"), "{out}");
        assert!(out.contains(&format!("Attached: [cat.png](blobs/{sha}) (image/png)")), "{out}");
        assert!(out.contains("\n> Juniper ran `` ls `-la` `` (it failed)"), "{out}");
        assert!(out.contains("> Juniper asked: Run it? (Allow once / Deny)"), "{out}");
        assert!(out.contains("> Answered `once` by paul@example.com"), "{out}");
        assert!(out.contains("**Juniper** · 2026-10-03 04:05 UTC\n\nHello!\n\nI am Juniper.\n"), "{out}");
        assert!(out.contains("> Juniper's turn ended: the runtime failed"), "{out}");
        assert!(out.contains("*npub1gone000…000000 stopped a turn"), "{out}");
        assert!(out.contains("**A visitor**"), "{out}");
        assert!(!out.contains("turn.start") && !out.contains("draft-ui-thing") && !out.contains("idle"), "{out}");
        // the order: time, then a message before work of the same moment
        let chat = serde_json::json!({});
        let a = Said { channel: "work", seq: 1, at: 5, principal: "x", author: None, body: &chat };
        let b = Said { channel: "chat", seq: 9, at: 5, principal: "x", author: None, body: &chat };
        assert!(b.order() < a.order());
    }

    /// Goal (FIN-508): transcripts show outside speakers, rooms, prior room
    /// context, and relayed owners from trusted doors, while ignoring spoofed
    /// or malformed room/heard/relayed fields from members or non-door authors.
    #[test]
    fn transcript_rooms_and_outside_speakers() {
        let paul = crate::npub::identity_of(&"11".repeat(32));
        let juniper = crate::npub::identity_of(&"22".repeat(32));
        let conn = crate::npub::identity_of(&"33".repeat(32));
        let mut names = Names::default();
        names.display.insert(paul.clone(), "paul@example.com".into());
        names.display.insert(juniper.clone(), "Juniper".into());
        names.members.insert(paul.clone(), None);
        names.members.insert(juniper.clone(), Some(paul.clone()));

        let heard_body = serde_json::json!({
            "text": "@Juniper is the deploy green?",
            "heard": { "via": "slack", "from": "U0456", "name": "Maple\nLee" },
            "room": { "id": "C0123", "name": "#deploys", "thread": "1790000000.000100" },
            "context": [
                { "name": "Sam", "text": "deploy started", "at": 1_790_000_000_000i64 },
                { "name": "Alex", "text": "watching\nlogs" }
            ]
        });
        let heard = transcript_part(&Said { channel: "chat", seq: 1, at: 1_791_000_000_000, principal: &conn, author: Some("door"), body: &heard_body }, &names).unwrap();
        assert!(heard.contains("**Maple Lee** (Slack · #deploys) · 2026-10-03 04:00 UTC\n\n> **Sam:** deploy started\n> **Alex:** watching logs\n\n@Juniper is the deploy green?\n"), "{heard}");

        // Markdown/HTML metacharacters in outside speaker, room name, and context are escaped
        let tricky = serde_json::json!({
            "text": "check link",
            "heard": { "via": "slack", "from": "U0457", "name": "**Admin** [x](https://evil.test)" },
            "room": { "id": "C0124", "name": "#ops <img>" },
            "context": [{ "name": "Bot_1", "text": "![px](https://evil.test/t) <script>" }]
        });
        let r_tricky = transcript_part(&Said { channel: "chat", seq: 10, at: 1_791_000_030_000, principal: &conn, author: Some("door"), body: &tricky }, &names).unwrap();
        assert!(r_tricky.contains(r"**\*\*Admin\*\* \[x\]\(https://evil.test\)** (Slack · #ops \<img\>)"), "{r_tricky}");
        assert!(r_tricky.contains(r"> **Bot\_1:** \!\[px\]\(https://evil.test/t\) \<script\>"), "{r_tricky}");

        // Owner relayed in a Slack room vs a Telegram DM (no room)
        let relayed_room = serde_json::json!({
            "text": "ship it",
            "relayed": { "via": "slack", "for": &paul, "agent": "juniper--k3x9" },
            "room": { "id": "C0123", "name": "#deploys" }
        });
        let r_room = transcript_part(&Said { channel: "chat", seq: 2, at: 1_791_000_060_000, principal: &conn, author: Some("door"), body: &relayed_room }, &names).unwrap();
        assert!(r_room.contains("**paul@example.com** (via Slack · #deploys) · 2026-10-03 04:01 UTC\n\nship it\n"), "{r_room}");

        let relayed_dm = serde_json::json!({
            "text": "ping from phone",
            "relayed": { "via": "telegram", "for": &paul, "agent": "juniper--k3x9" }
        });
        let r_dm = transcript_part(&Said { channel: "chat", seq: 3, at: 1_791_000_120_000, principal: &conn, author: Some("door"), body: &relayed_dm }, &names).unwrap();
        assert!(r_dm.contains("**paul@example.com** (via Telegram) · 2026-10-03 04:02 UTC\n\nping from phone\n"), "{r_dm}");

        // Unnamed room falls back to room.id
        let unnamed = serde_json::json!({
            "text": "hi",
            "heard": { "via": "slack", "from": "U9", "name": "Sam" },
            "room": { "id": "D9999" }
        });
        let r_unnamed = transcript_part(&Said { channel: "chat", seq: 4, at: 1_791_000_180_000, principal: &conn, author: Some("door"), body: &unnamed }, &names).unwrap();
        assert!(r_unnamed.contains("**Sam** (Slack · D9999)"), "{r_unnamed}");

        // Spoofing rejected: chat member, app code, external person on public chat, contradictory relayed+heard
        let spoof_member = transcript_part(&Said { channel: "chat", seq: 5, at: 1_791_000_240_000, principal: &paul, author: Some("door"), body: &heard_body }, &names).unwrap();
        assert!(spoof_member.contains("**paul@example.com** · 2026-10-03 04:04 UTC\n\n@Juniper is the deploy green?"), "{spoof_member}");
        assert!(!spoof_member.contains("Maple") && !spoof_member.contains("#deploys") && !spoof_member.contains("deploy started"), "{spoof_member}");

        let spoof_code = transcript_part(&Said { channel: "chat", seq: 6, at: 1_791_000_300_000, principal: &conn, author: Some("code"), body: &heard_body }, &names).unwrap();
        assert!(!spoof_code.contains("Maple") && !spoof_code.contains("#deploys"), "{spoof_code}");

        let spoof_person = transcript_part(&Said { channel: "chat", seq: 7, at: 1_791_000_360_000, principal: &conn, author: Some("person"), body: &heard_body }, &names).unwrap();
        assert!(!spoof_person.contains("Maple") && !spoof_person.contains("#deploys"), "{spoof_person}");

        // Unstamped, as the bridge reads it (docs/chat-records.md): no one
        // heard, and a relayed owner's message without its room.
        let unstamped = transcript_part(&Said { channel: "chat", seq: 11, at: 1_791_000_480_000, principal: &conn, author: None, body: &heard_body }, &names).unwrap();
        assert!(!unstamped.contains("Maple") && !unstamped.contains("#deploys"), "{unstamped}");
        let unstamped_relay = transcript_part(&Said { channel: "chat", seq: 12, at: 1_791_000_540_000, principal: &conn, author: None, body: &relayed_room }, &names).unwrap();
        assert!(unstamped_relay.contains("**paul@example.com** (via Slack) · ") && !unstamped_relay.contains("#deploys"), "{unstamped_relay}");
        // a malformed room is no one's relay, stamped or not, as the page reads it
        let bad_room = serde_json::json!({ "text": "odd", "relayed": { "via": "slack", "for": &paul, "agent": "juniper--k3x9" }, "room": "C0123" });
        let unstamped_bad = transcript_part(&Said { channel: "chat", seq: 13, at: 1_791_000_600_000, principal: &conn, author: None, body: &bad_room }, &names).unwrap();
        assert!(!unstamped_bad.contains("paul@example.com") && !unstamped_bad.contains("via Slack"), "{unstamped_bad}");

        // the record's own rules (docs/chat-records.md), as the bridge and the
        // page read them: a `thread` is the door's own and refuses nothing; a
        // room id past 64, or a speaker's id that is no `[A-Za-z0-9._-]`, is none
        let odd_thread = serde_json::json!({ "text": "t", "relayed": { "via": "slack", "for": &paul, "agent": "juniper--k3x9" }, "room": { "id": "C0123", "thread": "id\u{85}tail" } });
        let r_thread = transcript_part(&Said { channel: "chat", seq: 14, at: 1_791_000_660_000, principal: &conn, author: None, body: &odd_thread }, &names).unwrap();
        assert!(r_thread.contains("**paul@example.com** (via Slack) · "), "{r_thread}");
        let long_room = serde_json::json!({ "text": "t", "relayed": { "via": "slack", "for": &paul, "agent": "juniper--k3x9" }, "room": { "id": "C".repeat(65) } });
        let r_long = transcript_part(&Said { channel: "chat", seq: 15, at: 1_791_000_720_000, principal: &conn, author: Some("door"), body: &long_room }, &names).unwrap();
        assert!(!r_long.contains("paul@example.com"), "{r_long}");
        let accented = serde_json::json!({ "text": "t", "heard": { "via": "slack", "from": "U\u{e9}", "name": "Maple" }, "room": { "id": "C0123" } });
        let r_accented = transcript_part(&Said { channel: "chat", seq: 16, at: 1_791_000_780_000, principal: &conn, author: Some("door"), body: &accented }, &names).unwrap();
        assert!(!r_accented.contains("Maple"), "{r_accented}");
        let named = serde_json::json!({ "text": "t", "heard": { "via": "slack", "from": "U1", "name": "M".repeat(70) }, "room": { "id": "C0123" } });
        let r_named = transcript_part(&Said { channel: "chat", seq: 17, at: 1_791_000_840_000, principal: &conn, author: Some("door"), body: &named }, &names).unwrap();
        assert!(r_named.contains(&format!("**{}…** (Slack · C0123)", "M".repeat(59))), "a name is cut at 60: {r_named}");

        let both = serde_json::json!({
            "text": "conflicting",
            "relayed": { "via": "slack", "for": &paul, "agent": "juniper--k3x9" },
            "heard": { "via": "slack", "from": "U1", "name": "Maple" },
            "room": { "id": "C0123", "name": "#deploys" }
        });
        let r_both = transcript_part(&Said { channel: "chat", seq: 8, at: 1_791_000_420_000, principal: &conn, author: Some("door"), body: &both }, &names).unwrap();
        assert!(!r_both.contains("Maple") && !r_both.contains("paul@example.com") && !r_both.contains("#deploys"), "{r_both}");
    }

    /// Goal: times read as RFC 3339 and as a person reads them, in UTC.
    #[test]
    fn times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_000_000_000), "2026-10-03T04:00:00Z");
        assert_eq!(readable(1_791_000_059_999), "2026-10-03 04:00 UTC");
    }

    /// Goal: the manifest and a meta read back as written, and the README
    /// names every file the layout writes.
    #[test]
    fn what_it_says() {
        let m = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            deployment: Deployment { id: "dev".into(), platform: "https://fragment.club".into() },
            made_at: 1,
            made: rfc3339(1),
            export: "0123456789abcdef".into(),
            person: Person { npub: "npub1p".into(), email: Some("p@example.com".into()) },
            agents: vec!["npub1a".into()],
            fragments: vec![Listed { name: NAME.into(), kind: fragment_proto::FragmentKind::Chat, title: None, owner: "npub1p".into(), path: fragment_dir(NAME) }],
            computers: vec![],
            shared_with_me: vec![Shared { name: "x--abcd".into(), kind: fragment_proto::FragmentKind::App, title: None, owner: Some("npub1o".into()), role: fragment_proto::Role::Viewer, member: "npub1p".into() }],
        };
        let bytes = json_file(&m);
        assert!(bytes.ends_with(b"}\n"));
        let back: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, m);
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["sharedWithMe"][0]["role"] == "viewer" && v["madeAt"] == 1 && v["fragments"][0]["kind"] == "chat");
        for file in ["manifest.json", "meta.json", "files/", "schema.sql", ".jsonl", "transcript.md", "blobs/", "data.tar.zst", "data-work.tar.zst", "sharedWithMe"] {
            assert!(README.contains(file), "the README names {file}");
        }
    }
}
