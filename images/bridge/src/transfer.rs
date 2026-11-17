//! A file's bytes between the platform, this computer's disk and Hermes,
//! streamed: never whole in memory, so a file may be as large as a blob
//! (`limits::ATTACHMENT_MAX_BYTES`, 256 MiB).
//!
//! - **A message's file** (`download`) streams from the chat's blobs to
//!   `<scratch>/<sha256>`, hashed on the way, through a `.part` renamed once
//!   its length and hash are its record's. A transfer that moves no byte
//!   for `TRANSFER_STALL_MS` is cut, never one that is merely slow; a cut
//!   one resumes where it stopped (`Range`), at most `DOWNLOAD_TRIES_MAX`
//!   requests in all. Each request goes through the egress, which signs it
//!   afresh as the agent: no URL that outlives it, nothing to leak.
//! - **A reply's file** (`hash_file`, then `Api::put_blob_file`) is hashed
//!   from disk, then streamed up with its length declared.
//! - **Hermes' upload** to `/relay/media` (`receive`) streams to disk.
//!
//! Every write first reserves its room in its scratch folder, and what it
//! wrote is in use as it is handed back (scratch.rs).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::Frame;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::api::{Api, ApiError};
use crate::limits;
use crate::records::{self, AttachmentRef};
use crate::scratch::{InUse, NoRoom, Reservation, Scratch};

/// The bytes read from a file at a time, as it is hashed or sent.
const CHUNK_BYTES: usize = 64 * 1024;

/// Why a message's file was not downloaded.
#[derive(Debug)]
pub enum Unread {
    /// The platform refused it (not the agent's to read, gone).
    Refused(ApiError),
    /// Cut every time it was asked: `DOWNLOAD_TRIES_MAX` requests, the last
    /// one's failure.
    Cut { tries: u32, last: String },
    /// Longer than its record says.
    TooLarge,
    /// Its bytes are not its hash.
    NotItsHash,
    NoRoom(NoRoom),
    Disk(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::Refused(e) => write!(f, "refused: {e}"),
            Unread::Cut { tries, last } => write!(f, "cut {tries} times; last: {last}"),
            Unread::TooLarge => write!(f, "longer than its record says"),
            Unread::NotItsHash => write!(f, "its bytes are not its hash"),
            Unread::NoRoom(e) => write!(f, "no room: {e}"),
            Unread::Disk(e) => write!(f, "the disk: {e}"),
        }
    }
}

/// How a body being written stopped short.
#[derive(Debug)]
enum Cut {
    /// Nothing came for `TRANSFER_STALL_MS`.
    Stalled,
    /// The connection failed.
    Transport(String),
    /// Past the bytes it may have.
    TooLarge,
    Disk(String),
}

impl std::fmt::Display for Cut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cut::Stalled => write!(f, "no byte in {} ms", limits::TRANSFER_STALL_MS),
            Cut::Transport(e) => write!(f, "{e}"),
            Cut::TooLarge => write!(f, "past its length"),
            Cut::Disk(e) => write!(f, "the disk: {e}"),
        }
    }
}

/// A message's file, from the chat's blobs to `<scratch>/<sha256>` (one
/// downloaded already, of its length, is used again), in use until the
/// `InUse` handed back drops: the turn's, whose driver keeps it until the
/// engine ends the turn.
pub async fn download(api: &Api, agent: &str, fragment: &str, r: &AttachmentRef, scratch: &Scratch) -> Result<(PathBuf, InUse), Unread> {
    if let Some(had) = scratch.reuse(&r.sha256, r.size) {
        return Ok(had);
    }
    // a part of this download's own: two of one blob at once never write
    // into each other. Its room is held through every retry and wait.
    let room = scratch.reserve(&format!("{}.{:016x}", r.sha256, crate::net::jitter_seed()), r.size).map_err(Unread::NoRoom)?;
    let path = scratch.dir().join(&r.sha256);
    // in use before the rename shows it, and its room held until then:
    // never a moment it may go
    let used = InUse::new([path.clone()]);
    let got = match fetch_into(api, agent, fragment, r, room.part(), stall()).await {
        Ok(()) => tokio::fs::rename(room.part(), &path).await.map_err(|e| Unread::Disk(e.to_string())),
        Err(e) => Err(e),
    };
    if got.is_err() {
        let _ = tokio::fs::remove_file(room.part()).await;
    }
    got?;
    drop(room);
    Ok((path, used))
}

fn stall() -> Duration {
    Duration::from_millis(limits::TRANSFER_STALL_MS)
}

/// Writes the blob to `part`, resuming each cut request where it stopped
/// (a request that moves nothing for `stall` is cut).
async fn fetch_into(api: &Api, agent: &str, fragment: &str, r: &AttachmentRef, part: &Path, stall: Duration) -> Result<(), Unread> {
    let disk = |e: std::io::Error| Unread::Disk(format!("{}: {e}", part.display()));
    let mut file = tokio::fs::File::create(part).await.map_err(disk)?;
    let mut hasher = Sha256::new();
    let mut have: u64 = 0;
    let mut last = String::new();
    let mut backoff = crate::net::Backoff::default();
    for attempt in 1..=limits::DOWNLOAD_TRIES_MAX {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_millis(backoff.next_ms())).await;
            crate::ev!("attachment.resumed", { "sha256": r.sha256, "from": have, "attempt": attempt, "why": last });
        }
        let res = match api.blob_from(agent, fragment, &r.sha256, have).await {
            Ok(res) => res,
            Err(e) if e.retryable() => {
                last = e.to_string();
                continue;
            }
            Err(e) => return Err(Unread::Refused(e)),
        };
        let resumed = res.status() == hyper::StatusCode::PARTIAL_CONTENT && range_start(&res) == Some(have);
        if have > 0 && !resumed {
            // the range was not honored: from its start again
            file.set_len(0).await.map_err(disk)?;
            file.seek(std::io::SeekFrom::Start(0)).await.map_err(disk)?;
            hasher = Sha256::new();
            have = 0;
        }
        let before = have;
        let cut = write_body(res.into_body(), &mut file, Some(&mut hasher), &mut have, r.size, stall).await;
        if have > before {
            backoff.reset();
        }
        match cut {
            Ok(()) if have == r.size => {
                file.flush().await.map_err(disk)?;
                return if records::hex(&hasher.finalize()) == r.sha256 { Ok(()) } else { Err(Unread::NotItsHash) };
            }
            Ok(()) => last = format!("its answer ended at byte {have} of {}", r.size),
            Err(Cut::TooLarge) => return Err(Unread::TooLarge),
            Err(Cut::Disk(e)) => return Err(Unread::Disk(e)),
            Err(e @ (Cut::Stalled | Cut::Transport(_))) => last = e.to_string(),
        }
    }
    Err(Unread::Cut { tries: limits::DOWNLOAD_TRIES_MAX, last })
}

/// The first byte a `206`'s `Content-Range: bytes <first>-<last>/<size>` names.
fn range_start(res: &hyper::Response<hyper::body::Incoming>) -> Option<u64> {
    let v = res.headers().get("content-range")?.to_str().ok()?;
    v.strip_prefix("bytes ")?.split_once('-')?.0.parse().ok()
}

/// A body's bytes appended to `file` (and to `hasher`), `have` counting
/// them, at most `max` in all: until it ends, or nothing comes for `stall`.
async fn write_body<B>(mut body: B, file: &mut tokio::fs::File, mut hasher: Option<&mut Sha256>, have: &mut u64, max: u64, stall: Duration) -> Result<(), Cut>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    // bounded by `max`: each pass takes a frame of bytes, or ends
    loop {
        let frame = match tokio::time::timeout(stall, body.frame()).await {
            Err(_) => return Err(Cut::Stalled),
            Ok(None) => return Ok(()),
            Ok(Some(Err(e))) => return Err(Cut::Transport(e.to_string())),
            Ok(Some(Ok(frame))) => frame,
        };
        let Ok(data) = frame.into_data() else { continue };
        if (*have).saturating_add(data.len() as u64) > max {
            return Err(Cut::TooLarge);
        }
        file.write_all(&data).await.map_err(|e| Cut::Disk(e.to_string()))?;
        if let Some(h) = hasher.as_deref_mut() {
            h.update(&data);
        }
        *have += data.len() as u64;
    }
}

/// Why Hermes' upload was not taken.
#[derive(Debug)]
pub enum Unreceived {
    TooLarge,
    Empty,
    Cut(String),
    Disk(String),
}

/// A request's body (Hermes' upload to `/relay/media`) streamed through
/// `room`'s part to `path`, at most the bytes `room` holds: its length, and
/// the file in use.
pub async fn receive<B>(body: B, room: Reservation, path: &Path, max: u64) -> Result<(u64, InUse), Unreceived>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let part = room.part().to_path_buf();
    // in use before the rename shows it: never a moment it may go
    let used = InUse::new([path.to_path_buf()]);
    let got = async {
        let mut file = tokio::fs::File::create(&part).await.map_err(|e| Unreceived::Disk(e.to_string()))?;
        let mut have = 0;
        match write_body(body, &mut file, None, &mut have, max, stall()).await {
            Ok(()) if have == 0 => Err(Unreceived::Empty),
            Ok(()) => {
                file.flush().await.map_err(|e| Unreceived::Disk(e.to_string()))?;
                tokio::fs::rename(&part, path).await.map_err(|e| Unreceived::Disk(e.to_string()))?;
                Ok(have)
            }
            Err(Cut::TooLarge) => Err(Unreceived::TooLarge),
            Err(Cut::Disk(e)) => Err(Unreceived::Disk(e)),
            Err(e) => Err(Unreceived::Cut(e.to_string())),
        }
    }
    .await;
    if got.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    drop(room);
    got.map(|have| (have, used))
}

/// Why a reply's file was not hashed.
#[derive(Debug)]
pub enum Unhashed {
    Empty,
    TooLarge,
    Disk(String),
}

impl std::fmt::Display for Unhashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unhashed::Empty => write!(f, "empty"),
            Unhashed::TooLarge => write!(f, "past {} bytes", limits::ATTACHMENT_MAX_BYTES),
            Unhashed::Disk(e) => write!(f, "{e}"),
        }
    }
}

/// A file's SHA-256 and length, read from disk a chunk at a time, at most
/// `ATTACHMENT_MAX_BYTES`.
pub async fn hash_file(path: &Path) -> Result<(String, u64), Unhashed> {
    let mut file = tokio::fs::File::open(path).await.map_err(|e| Unhashed::Disk(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut size: u64 = 0;
    // bounded by ATTACHMENT_MAX_BYTES: each pass reads a chunk
    loop {
        let n = file.read(&mut buf).await.map_err(|e| Unhashed::Disk(e.to_string()))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > limits::ATTACHMENT_MAX_BYTES {
            return Err(Unhashed::TooLarge);
        }
        hasher.update(&buf[..n]);
    }
    if size == 0 {
        return Err(Unhashed::Empty);
    }
    Ok((records::hex(&hasher.finalize()), size))
}

/// When a streamed upload is cut (`Api::put_blob_file`): `TRANSFER_STALL_MS`
/// after the platform last took a byte, and `HTTP_TIMEOUT_MS` after it took
/// the last one, for its answer (it hashes as it reads, so it answers at
/// once).
#[derive(Clone)]
pub struct Pace(Arc<Mutex<Instant>>);

impl Default for Pace {
    fn default() -> Pace {
        Pace::new()
    }
}

impl Pace {
    pub fn new() -> Pace {
        Pace(Arc::new(Mutex::new(Instant::now() + Duration::from_millis(limits::TRANSFER_STALL_MS))))
    }

    fn moved(&self) {
        *self.0.lock().expect("the pace") = Instant::now() + Duration::from_millis(limits::TRANSFER_STALL_MS);
    }

    fn sent(&self) {
        *self.0.lock().expect("the pace") = Instant::now() + Duration::from_millis(limits::HTTP_TIMEOUT_MS);
    }

    /// `call`, unless it is past its due before it ends: then `None`.
    pub async fn watch<T>(&self, call: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::pin!(call);
        // bounded by the call: each pass waits for it or its due, which
        // moves only as bytes do
        loop {
            let due = *self.0.lock().expect("the pace");
            tokio::select! {
                r = &mut call => return Some(r),
                () = tokio::time::sleep_until(due) => {
                    if Instant::now() >= *self.0.lock().expect("the pace") {
                        return None;
                    }
                }
            }
        }
    }
}

/// A request's body: the first `size` bytes of `file`, a chunk at a time,
/// each taken marking `pace`; a file shorter than that fails the request.
pub fn file_body(file: tokio::fs::File, size: u64, pace: Pace) -> http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error> {
    let chunks = futures_util::stream::unfold((file, size, pace), |(mut file, left, pace)| async move {
        if left == 0 {
            pace.sent();
            return None;
        }
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(CHUNK_BYTES);
        let mut buf = BytesMut::zeroed(want);
        let read = match file.read(&mut buf).await {
            Ok(0) => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the file is shorter than it was")),
            Ok(n) => {
                buf.truncate(n);
                Ok(n)
            }
            Err(e) => Err(e),
        };
        match read {
            Ok(n) => {
                pace.moved();
                Some((Ok(Frame::data(buf.freeze())), (file, left - n as u64, pace)))
            }
            Err(e) => Some((Err(e), (file, 0, pace))),
        }
    });
    http_body_util::StreamBody::new(chunks).boxed_unsync()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    /// How the test's server answers one request.
    #[derive(Clone, Copy)]
    enum Answer {
        /// `200`, the whole of its bytes.
        Whole,
        /// `200` declaring them all, then the connection closed after this many.
        CutAt(usize),
        /// `200` declaring them all, then nothing more after this many.
        StallAt(usize),
        /// A range asked for honored (`206`), else whole.
        Ranged,
        Status(u16),
    }

    /// A server of one blob, answering each request as `answers` says in
    /// turn: the requests' `range` headers, as they came.
    async fn server(bytes: Vec<u8>, answers: Vec<Answer>) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for answer in answers {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = tokio::io::BufReader::new(stream);
                let mut range = None;
                loop {
                    let mut line = String::new();
                    stream.read_line(&mut line).await.unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: ") {
                        range = Some(v.trim().to_string());
                    }
                }
                log.lock().unwrap().push(range.clone());
                let mut stream = stream.into_inner();
                let n = bytes.len();
                let head = |status: &str, len: usize, extra: &str| format!("HTTP/1.1 {status}\r\ncontent-length: {len}\r\n{extra}connection: close\r\n\r\n");
                match answer {
                    Answer::Whole => {
                        stream.write_all(head("200 OK", n, "").as_bytes()).await.unwrap();
                        stream.write_all(&bytes).await.unwrap();
                    }
                    Answer::CutAt(at) => {
                        stream.write_all(head("200 OK", n, "").as_bytes()).await.unwrap();
                        stream.write_all(&bytes[..at]).await.unwrap();
                    }
                    Answer::StallAt(at) => {
                        stream.write_all(head("200 OK", n, "").as_bytes()).await.unwrap();
                        stream.write_all(&bytes[..at]).await.unwrap();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            drop(stream);
                        });
                        continue;
                    }
                    Answer::Ranged => {
                        let from = range.as_deref().and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse::<usize>().ok()).unwrap_or(0);
                        let extra = format!("content-range: bytes {from}-{}/{n}\r\n", n - 1);
                        stream.write_all(head(if from > 0 { "206 Partial Content" } else { "200 OK" }, n - from, &extra).as_bytes()).await.unwrap();
                        stream.write_all(&bytes[from..]).await.unwrap();
                    }
                    Answer::Status(code) => {
                        let body = r#"{"error":"not_found","message":"no such blob"}"#;
                        stream.write_all(head(&format!("{code} No"), body.len(), "").as_bytes()).await.unwrap();
                        stream.write_all(body.as_bytes()).await.unwrap();
                    }
                }
                let _ = stream.shutdown().await;
            }
        });
        (url, seen)
    }

    fn blob(n: usize) -> (Vec<u8>, AttachmentRef) {
        let bytes: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
        let r = AttachmentRef { sha256: records::hex(&Sha256::digest(&bytes)), size: n as u64, media_type: "video/mp4".into(), name: "film.mp4".into() };
        (bytes, r)
    }

    fn part(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bridge-transfer-{tag}-{}-{:x}.part", std::process::id(), crate::net::jitter_seed()))
    }

    async fn fetch(bytes: &[u8], r: &AttachmentRef, answers: Vec<Answer>, tag: &str) -> (Result<(), Unread>, Vec<u8>, Vec<Option<String>>) {
        let (url, seen) = server(bytes.to_vec(), answers).await;
        let api = Api::new(&url).unwrap();
        let p = part(tag);
        let got = fetch_into(&api, "juniper--k3x9", "talk--k3x9", r, &p, Duration::from_millis(300)).await;
        let written = std::fs::read(&p).unwrap_or_default();
        let _ = std::fs::remove_file(&p);
        let seen = seen.lock().unwrap().clone();
        (got, written, seen)
    }

    #[tokio::test]
    async fn a_cut_download_resumes_where_it_stopped() {
        let (bytes, r) = blob(300_000);
        let (got, written, seen) = fetch(&bytes, &r, vec![Answer::CutAt(120_000), Answer::Ranged], "cut").await;
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(written, bytes);
        assert_eq!(seen, vec![None, Some("bytes=120000-".into())], "the second request asks for the rest alone");
    }

    #[tokio::test]
    async fn a_stalled_download_is_cut_and_resumed() {
        let (bytes, r) = blob(300_000);
        let (got, written, seen) = fetch(&bytes, &r, vec![Answer::StallAt(100_000), Answer::Ranged], "stall").await;
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(written, bytes);
        assert_eq!(seen, vec![None, Some("bytes=100000-".into())]);
    }

    #[tokio::test]
    async fn a_range_not_honored_starts_again() {
        let (bytes, r) = blob(300_000);
        let (got, written, seen) = fetch(&bytes, &r, vec![Answer::CutAt(50_000), Answer::Whole], "whole").await;
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(written, bytes, "written from its start again, hashed afresh");
        assert_eq!(seen.len(), 2);
    }

    #[tokio::test]
    async fn bytes_that_are_not_the_record_are_refused() {
        let (bytes, r) = blob(10_000);
        let mut other = bytes.clone();
        other[5] ^= 1;
        let (got, _, _) = fetch(&other, &r, vec![Answer::Whole], "hash").await;
        assert!(matches!(got, Err(Unread::NotItsHash)), "{got:?}");
        let longer = [bytes.as_slice(), b"x"].concat();
        let (got, _, _) = fetch(&longer, &r, vec![Answer::Whole], "long").await;
        assert!(matches!(got, Err(Unread::TooLarge)), "{got:?}");
        let (got, _, seen) = fetch(&bytes, &r, vec![Answer::Status(404)], "gone").await;
        assert!(matches!(&got, Err(Unread::Refused(e)) if e.status() == Some(404)), "{got:?}");
        assert_eq!(seen.len(), 1, "a refusal is not asked again");
    }

    #[tokio::test]
    async fn a_download_lands_whole_or_not_at_all() {
        let (bytes, r) = blob(200_000);
        let (url, _) = server(bytes.clone(), vec![Answer::Whole, Answer::Status(404)]).await;
        let api = Api::new(&url).unwrap();
        let dir = part("scratch").with_extension("d");
        let scratch = Scratch::new(dir.clone(), limits::SCRATCH_MAX_BYTES).unwrap();
        let (path, _used) = download(&api, "juniper--k3x9", "talk--k3x9", &r, &scratch).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(download(&api, "juniper--k3x9", "talk--k3x9", &r, &scratch).await.unwrap().0, path, "one on disk is used again, unasked");
        let (_, gone) = blob(1000);
        assert!(download(&api, "juniper--k3x9", "talk--k3x9", &gone, &scratch).await.is_err());
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![r.sha256.clone()], "no part left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_upload_lands_in_its_room_or_gives_it_back() {
        let dir = part("receive").with_extension("d");
        let scratch = Scratch::bounded(dir.clone(), 100, 4).unwrap();
        let body = |n: usize| http_body_util::Full::new(Bytes::from(vec![9u8; n]));
        let got = receive(body(80), scratch.reserve("up-1", 80).unwrap(), &dir.join("up-1"), 80).await;
        let (size, used) = got.unwrap();
        assert_eq!(size, 80);
        assert!(scratch.reserve("up-2", 30).is_err(), "landed and in use: its room is taken");
        drop(used);
        let room = scratch.reserve("up-2", 30).unwrap();
        assert!(!dir.join("up-1").exists(), "let go, it went for the next");
        assert!(matches!(receive(body(31), room, &dir.join("up-2"), 30).await, Err(Unreceived::TooLarge)));
        assert!(!dir.join("up-2").exists() && !dir.join("up-2.part").exists(), "nothing left behind");
        assert!(matches!(receive(body(0), scratch.reserve("up-3", 100).unwrap(), &dir.join("up-3"), 100).await, Err(Unreceived::Empty)));
        drop(scratch.reserve("up-4", 100).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_file_is_hashed_from_disk_within_the_bound() {
        let p = part("hash");
        std::fs::write(&p, b"abc").unwrap();
        let (sha, size) = hash_file(&p).await.unwrap();
        assert_eq!((sha.as_str(), size), ("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", 3));
        std::fs::write(&p, b"").unwrap();
        assert!(matches!(hash_file(&p).await, Err(Unhashed::Empty)));
        std::fs::File::options().write(true).open(&p).unwrap().set_len(limits::ATTACHMENT_MAX_BYTES + 1).unwrap();
        assert!(matches!(hash_file(&p).await, Err(Unhashed::TooLarge)));
        let _ = std::fs::remove_file(&p);
    }
}
