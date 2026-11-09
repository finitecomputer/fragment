//! An old agent's stage (FIN-463): one zip of its home that the platform
//! keeps for its new agent while it imports, read in place by HTTP ranges
//! (`GET /api/migrations/stages/{stage}/zip`), so nothing of it is copied
//! to the computer but what a reader copies out; and the two calls around
//! an import, `open` and `done` (docs/migrations.md). A source names one
//! as `stage:<id>`. Removable with the move: the routes go, and a source
//! is a folder or a zip again.
//!
//! **Reads.** The zip is read a block at a time (`BLOCK`), the last few
//! kept (`CACHED`), so the central directory and a run of small files in
//! the zip's order take a request a block, not one a file. A large file
//! stored whole (the stage's are stored, not deflated) is read `CHUNK` at
//! a time, `IN_FLIGHT` ranges at once, in order, its CRC-32 checked at its
//! end: the spike read 158 MB/s with 8 ranges, 20 MB/s with one.

use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{bail, Context, Result};
use fragment_proto::migrations::{parse_stage, StageDone, StageOpened, STAGE_FORMAT};
use serde_json::Value;

use crate::api::{Client, Code, CodedError};

/// How a source names a stage: `stage:<id>`.
pub const SCHEME: &str = "stage:";
/// A block read of the zip, and how many the reader keeps.
const BLOCK: u64 = 1024 * 1024;
const CACHED: usize = 16;
/// A stored file at least this large is read by ranges at once.
pub const PARALLEL_MIN: u64 = 16 * 1024 * 1024;
/// One range of such a file, and how many are in flight at once.
const CHUNK: u64 = 8 * 1024 * 1024;
/// Neighbouring files read together (`Source::read_all`) are one range of
/// at most this, and what lies between two of them is read too while it
/// is at most `GAP_MAX`: cheaper than another request.
pub const RANGE_MAX: u64 = 16 * 1024 * 1024;
pub const GAP_MAX: u64 = 1024 * 1024;
const IN_FLIGHT: usize = 8;

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
}

/// The stage a source names (`stage:<id>`), its id checked, or `None` for
/// any other source.
pub fn named(arg: &str) -> Option<Result<&str>> {
    let id = arg.strip_prefix(SCHEME)?;
    Some(parse_stage(id).map(|_| id).ok_or_else(|| usage(format!("{id:?} is no stage: a stage is named <old>.<p1|p2>.<attempt>, as its routine says"))))
}

fn route(stage: &str, what: &str) -> String {
    format!("/api/migrations/stages/{stage}/{what}")
}

/// `POST …/open`: the stage, as the agent bound to it opens it.
pub fn open(c: &Client, stage: &str) -> Result<StageOpened> {
    let opened: StageOpened = c.call_as(c.post_json(&route(stage, "open"), &serde_json::json!({}))?).with_context(|| format!("opening the stage {stage}"))?;
    if opened.manifest.format != STAGE_FORMAT {
        bail!("the stage {stage} is {:?}, and this CLI reads {STAGE_FORMAT:?}", opened.manifest.format);
    }
    Ok(opened)
}

/// `POST …/done`: the import ended, well or not. After it the stage's
/// routes refuse, so it is the import's last call.
pub fn done(c: &Client, stage: &str, body: &StageDone) -> Result<Value> {
    c.call_as(c.post_json(&route(stage, "done"), body)?).with_context(|| format!("saying the stage {stage} is done"))
}

#[derive(clap::Subcommand)]
pub enum StageCmd {
    /// Open an old agent's stage, as the agent it was staged for: its
    /// manifest (what it holds, its layout, what stayed behind). Read it
    /// as a source named `stage:<id>`
    Open { stage: String },
    /// Say the import of a stage ended: `--ok`, or `--failed <why>`. Its
    /// last call: after it the stage is gone
    Done {
        stage: String,
        #[arg(long, conflicts_with = "failed", required_unless_present = "failed")]
        ok: bool,
        /// Why it failed, in a sentence
        #[arg(long)]
        failed: Option<String>,
        /// The report written for the owner, whose hash is kept with it
        #[arg(long)]
        report: Option<std::path::PathBuf>,
    },
}

/// `fragment stage`: its answer, for main to print.
pub fn run(c: &Client, sub: StageCmd) -> Result<Value> {
    match sub {
        StageCmd::Open { stage } => {
            let id = named(&format!("{SCHEME}{stage}")).expect("a stage name")?.to_string();
            Ok(serde_json::to_value(open(c, &id)?)?)
        }
        StageCmd::Done { stage, ok, failed, report } => {
            let id = named(&format!("{SCHEME}{stage}")).expect("a stage name")?.to_string();
            let report_sha256 = match report {
                Some(p) => Some(fragment_core::blob::sha256_hex(&std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?)),
                None => None,
            };
            done(c, &id, &StageDone { ok: ok && failed.is_none(), report_sha256, why: failed })
        }
    }
}

/// The zip's bytes, by ranges, a block at a time, the last `CACHED` kept
/// for every reader of it.
pub struct Blocks {
    client: Arc<Client>,
    path: String,
    len: u64,
    cache: Mutex<VecDeque<(u64, Arc<Vec<u8>>)>>,
}

impl Blocks {
    pub fn new(client: Arc<Client>, stage: &str, len: u64) -> Arc<Blocks> {
        Arc::new(Blocks { client, path: route(stage, "zip"), len, cache: Mutex::new(VecDeque::new()) })
    }

    /// Bytes `start..end` of the zip, read now.
    fn fetch(&self, start: u64, end: u64) -> io::Result<Vec<u8>> {
        let r = self.client.get_range(&self.path, start, end).map_err(io::Error::other)?;
        let whole = r.status == 200 && start == 0 && end == self.len;
        if !(r.status == 206 || whole) || r.body.len() as u64 != end - start {
            let why = if r.ok() { format!("answered {} bytes for {start}..{end} (status {})", r.body.len(), r.status) } else { r.refusal().to_string() };
            return Err(io::Error::other(format!("reading the stage: {why}")));
        }
        Ok(r.body)
    }

    fn block(&self, n: u64) -> io::Result<Arc<Vec<u8>>> {
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) = cache.iter().position(|(k, _)| *k == n) {
                let hit = cache.remove(at).expect("found");
                let bytes = Arc::clone(&hit.1);
                cache.push_back(hit);
                return Ok(bytes);
            }
        }
        let start = n * BLOCK;
        let bytes = Arc::new(self.fetch(start, (start + BLOCK).min(self.len))?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.push_back((n, Arc::clone(&bytes)));
        if cache.len() > CACHED {
            cache.pop_front();
        }
        Ok(bytes)
    }
}

/// The zip as a file: a position of its own over the shared blocks, so
/// each clone of the archive reads apart from the others.
#[derive(Clone)]
pub struct Ranged {
    pub blocks: Arc<Blocks>,
    pos: u64,
}

impl Ranged {
    pub fn new(blocks: Arc<Blocks>) -> Ranged {
        Ranged { blocks, pos: 0 }
    }
}

impl Read for Ranged {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.blocks.len || buf.is_empty() {
            return Ok(0);
        }
        let n = self.pos / BLOCK;
        let block = self.blocks.block(n)?;
        let off = (self.pos - n * BLOCK) as usize;
        let k = buf.len().min(block.len() - off);
        buf[..k].copy_from_slice(&block[off..off + k]);
        self.pos += k as u64;
        Ok(k)
    }
}

impl Seek for Ranged {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.blocks.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))?;
        Ok(self.pos)
    }
}

/// Spans of the zip (`start..end` each), fetched `IN_FLIGHT` at once and
/// yielded in their order.
pub struct Spans {
    blocks: Arc<Blocks>,
    todo: VecDeque<(u64, u64)>,
    pending: VecDeque<JoinHandle<io::Result<Vec<u8>>>>,
}

impl Spans {
    pub fn new(blocks: Arc<Blocks>, spans: impl IntoIterator<Item = (u64, u64)>) -> Spans {
        Spans { blocks, todo: spans.into_iter().collect(), pending: VecDeque::new() }
    }

    fn fill(&mut self) {
        while self.pending.len() < IN_FLIGHT {
            let Some((start, end)) = self.todo.pop_front() else { break };
            let blocks = Arc::clone(&self.blocks);
            self.pending.push_back(std::thread::spawn(move || blocks.fetch(start, end)));
        }
    }

    /// The next span's bytes, or `None` when every one was yielded.
    pub fn next_span(&mut self) -> Option<io::Result<Vec<u8>>> {
        self.fill();
        let next = self.pending.pop_front()?;
        let got = next.join().map_err(|_| io::Error::other("a range read panicked")).and_then(|r| r);
        self.fill();
        Some(got)
    }
}

/// A stored file's bytes, `start..start + len` of the zip, read `CHUNK` at
/// a time with `IN_FLIGHT` ranges at once, in order; its CRC-32 is checked
/// when its last byte is read.
pub struct Parallel {
    spans: Spans,
    cur: Vec<u8>,
    at: usize,
    crc: crc32fast::Hasher,
    want: u32,
    checked: bool,
}

impl Parallel {
    pub fn new(blocks: Arc<Blocks>, start: u64, len: u64, crc32: u32) -> Parallel {
        let end = start + len;
        let chunks = (start..end).step_by(CHUNK as usize).map(|a| (a, (a + CHUNK).min(end)));
        Parallel { spans: Spans::new(blocks, chunks), cur: Vec::new(), at: 0, crc: crc32fast::Hasher::new(), want: crc32, checked: false }
    }
}

impl Read for Parallel {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at == self.cur.len() {
            match self.spans.next_span() {
                Some(next) => {
                    self.cur = next?;
                    self.at = 0;
                    self.crc.update(&self.cur);
                }
                None => {
                    if !self.checked {
                        self.checked = true;
                        let got = std::mem::take(&mut self.crc).finalize();
                        if got != self.want {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a file of the stage read with CRC-32 {got:08x}, not its {:08x}", self.want)));
                        }
                    }
                    return Ok(0);
                }
            }
        }
        let k = buf.len().min(self.cur.len() - self.at);
        buf[..k].copy_from_slice(&self.cur[self.at..self.at + k]);
        self.at += k;
        Ok(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::source::{tests::Temp, Source};
    use std::path::Path;
    use fragment_fakes::http::{Request, Response, Server};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    const ID: &str = "prj_123.p1.1";

    /// A platform's stage routes over `zip`: `open`, `zip` by ranges, and
    /// `done`; ranges counted, and levers to ignore a range or spoil bytes.
    struct Fake {
        server: Server,
        ranges: Arc<AtomicU32>,
        whole: Arc<AtomicBool>,
        spoil_from: Arc<AtomicU32>,
        done: Arc<Mutex<Option<Value>>>,
    }

    fn fake(zip: Vec<u8>) -> Fake {
        let (ranges, whole, spoil_from, done) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicU32::new(u32::MAX)), Arc::new(Mutex::new(None)));
        let (r, w, sp, d) = (Arc::clone(&ranges), Arc::clone(&whole), Arc::clone(&spoil_from), Arc::clone(&done));
        let zip = Arc::new(zip);
        let server = Server::start(
            0,
            Arc::new(move |req: &Request| {
                let open = format!("/api/migrations/stages/{ID}/open");
                let zip_path = format!("/api/migrations/stages/{ID}/zip");
                let done_path = format!("/api/migrations/stages/{ID}/done");
                if req.method == "POST" && req.path == open {
                    let manifest = serde_json::json!({ "format": STAGE_FORMAT, "old": "prj_123", "phase": "p1", "attempt": 1, "layout": "test", "zip_bytes": zip.len(), "zip_sha256": "0".repeat(64), "bytes": 0, "files": 0 });
                    return Response::json(200, &serde_json::json!({ "stage": ID, "manifest": manifest }));
                }
                if req.method == "POST" && req.path == done_path {
                    *d.lock().unwrap() = serde_json::from_slice(&req.body).ok();
                    return Response::json(200, &serde_json::json!({ "state": "retiring" }));
                }
                if req.method != "GET" || req.path != zip_path {
                    return Response::json(404, &serde_json::json!({ "error": "not_found" }));
                }
                if w.load(Ordering::Relaxed) {
                    return Response::bytes(200, "application/zip", zip.to_vec());
                }
                let range = req.header("range").and_then(|v| v.strip_prefix("bytes=")).and_then(|v| v.split_once('-')).and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)));
                let Some((a, b)) = range.filter(|(a, b)| a <= b && *b < zip.len()) else { return Response::json(416, &serde_json::json!({ "error": "range" })) };
                r.fetch_add(1, Ordering::Relaxed);
                let mut body = zip[a..=b].to_vec();
                let spoil = sp.load(Ordering::Relaxed) as usize;
                if (a..=b).contains(&spoil) {
                    body[spoil - a] ^= 0xff;
                }
                Response::bytes(206, "application/zip", body).with_header("content-range", &format!("bytes {a}-{b}/{}", zip.len()))
            }),
        )
        .unwrap();
        Fake { server, ranges, whole, spoil_from, done }
    }

    fn client(f: &Fake) -> Client {
        Client::new(&f.server.url, crate::auth::fixed(7))
    }

    /// Goal: a stage reads as the zip it is: the same files and bytes, a
    /// large stored file by ranges at once checked whole, a run of small
    /// files in a request a block, not one a file. Invalid: a stage named
    /// oddly, and one read with no platform to read it through.
    #[test]
    fn a_stage_reads_as_its_zip() {
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        let big: Vec<u8> = (0..(PARALLEL_MIN + 3 * CHUNK + 5) as u32).map(|i| (i % 251) as u8).collect();
        let small: Vec<(String, Vec<u8>)> = (0..200).map(|i| (format!("notes/{i:03}.md"), format!("note {i}").into_bytes())).collect();
        let mut files = small.clone();
        files.push(("media/film.mp4".into(), big.clone()));
        fragment_fakes::old_home::zip(&zip, &files).unwrap();
        let f = fake(std::fs::read(&zip).unwrap());
        let c = client(&f);
        let staged = Source::named(Path::new(&format!("stage:{ID}")), Some(&c)).unwrap();
        let local = Source::open(&zip).unwrap();
        assert_eq!(staged.kind(), "stage");
        assert_eq!(staged.entries().iter().map(|e| (&e.path, e.size)).collect::<Vec<_>>(), local.entries().iter().map(|e| (&e.path, e.size)).collect::<Vec<_>>());
        let before = f.ranges.load(Ordering::Relaxed);
        for (p, b) in &small {
            assert_eq!(&staged.read(p).unwrap(), b, "{p}");
        }
        let small_reads = f.ranges.load(Ordering::Relaxed) - before;
        assert!(small_reads <= 2, "200 small files in the zip's order: {small_reads} ranges");
        let before = f.ranges.load(Ordering::Relaxed);
        assert_eq!(staged.read_max("media/film.mp4", u64::MAX).unwrap(), big);
        let big_reads = f.ranges.load(Ordering::Relaxed) - before;
        let chunks = (big.len() as u64).div_ceil(CHUNK);
        assert!((chunks..=chunks + 1).contains(&(big_reads as u64)), "a range a chunk ({chunks}), and its local header's block if not read already: {big_reads}");
        assert!(format!("{:#}", Source::named(Path::new("stage:nope"), Some(&c)).err().unwrap()).contains("is no stage"));
        assert!(format!("{:#}", Source::named(Path::new(&format!("stage:{ID}")), None).err().unwrap()).contains("read through the platform"));
        drop(t);
    }

    /// Invalid: a platform that answers a range with the whole zip, and a
    /// large file whose bytes arrive spoiled (its CRC-32 says so).
    #[test]
    fn a_stage_read_wrong_is_refused() {
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        let big: Vec<u8> = vec![7u8; (PARALLEL_MIN + 1) as usize];
        fragment_fakes::old_home::zip(&zip, &[("a.md".into(), b"a".to_vec()), ("big.bin".into(), big)]).unwrap();
        let bytes = std::fs::read(&zip).unwrap();
        let f = fake(bytes.clone());
        let c = client(&f);
        let staged = Source::named(Path::new(&format!("stage:{ID}")), Some(&c)).unwrap();
        f.spoil_from.store((bytes.len() / 2) as u32, Ordering::Relaxed);
        let e = staged.read_max("big.bin", u64::MAX).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("CRC-32"), "{e}");
        // a small file read with its neighbours, its byte spoiled
        let at = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap().by_index_raw(0).unwrap().data_start().unwrap();
        f.spoil_from.store(at as u32, Ordering::Relaxed);
        let e = staged.read_all(&["a.md"], u64::MAX, |_, _| Ok(())).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("a.md: its bytes read with another CRC-32"), "{e}");
        f.spoil_from.store(u32::MAX, Ordering::Relaxed);
        let fresh = Source::named(Path::new(&format!("stage:{ID}")), Some(&c));
        f.whole.store(true, Ordering::Relaxed);
        let e = fresh.and_then(|s| s.read("a.md")).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("reading the stage: answered"), "{e}");
    }

    /// Goal: what `home files` brings of a stage is read in the zip's
    /// order, neighbours in one range, whatever order the cap chose it in:
    /// N small files cost about their bytes over `RANGE_MAX` requests, not
    /// N. Method: 2,000 documents of 16 KiB (32 MB, past the blocks a
    /// reader keeps), written to the zip in another order than the cap's
    /// (by path), brought into the code.storage fake; the stage's ranges
    /// counted while it runs.
    #[test]
    fn a_stages_picks_are_read_in_its_order() {
        use crate::home::layout::Layout;
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        let n = 2_000usize;
        let body = |i: usize| format!("{i:05}").repeat(16 * 1024 / 5).into_bytes();
        // the zip's order: a stride through them, not theirs by path
        let files: Vec<(String, Vec<u8>)> = (0..n).map(|k| (k * 7919) % n).map(|i| (format!("work/notes/{i:05}.md"), body(i))).collect();
        let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
        fragment_fakes::old_home::zip(&zip, &files).unwrap();
        let f = fake(std::fs::read(&zip).unwrap());
        let staged = Source::named(Path::new(&format!("stage:{ID}")), Some(&client(&f))).unwrap();
        let mock = crate::mockcs::start();
        mock.seed_repo("old-files--k3x9", &[]);
        let c = Client::new(&mock.url, crate::auth::fixed(7));
        let layout = Layout { work: Some("work".into()), ..Layout::hermes() };
        let before = f.ranges.load(Ordering::Relaxed);
        let b = crate::home::files::bring(&c, &staged, &layout, "old-files--k3x9").unwrap();
        let ranges = (f.ranges.load(Ordering::Relaxed) - before) as u64;
        assert_eq!(b.files, n);
        let bound = total.div_ceil(RANGE_MAX) + 2;
        println!("      ({n} files of {total} bytes, chosen by path and stored in another order: {ranges} ranges)");
        assert!(ranges <= bound, "{n} files of {total} bytes: {ranges} ranges, at most {bound}");
        for i in [0, 1, 999, 1_999] {
            assert_eq!(mock.file_at("old-files--k3x9", "main", &format!("files/notes/{i:05}.md")), Some(body(i)), "{i}");
        }
    }

    /// `fragment stage open|done`: the manifest, then the end, its report's
    /// hash with it.
    #[test]
    fn open_then_done() {
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        fragment_fakes::old_home::zip(&zip, &[("a.md".into(), b"a".to_vec())]).unwrap();
        let f = fake(std::fs::read(&zip).unwrap());
        let c = client(&f);
        let opened = run(&c, StageCmd::Open { stage: ID.into() }).unwrap();
        assert_eq!(opened["manifest"]["format"], STAGE_FORMAT);
        let report = t.0.join("report.md");
        std::fs::write(&report, "all came over").unwrap();
        let answer = run(&c, StageCmd::Done { stage: ID.into(), ok: true, failed: None, report: Some(report) }).unwrap();
        assert_eq!(answer["state"], "retiring");
        let sent = f.done.lock().unwrap().clone().unwrap();
        assert_eq!((sent["ok"].clone(), sent["report_sha256"].clone()), (serde_json::json!(true), serde_json::json!(fragment_core::blob::sha256_hex(b"all came over"))));
        run(&c, StageCmd::Done { stage: ID.into(), ok: false, failed: Some("the files app refused".into()), report: None }).unwrap();
        let sent = f.done.lock().unwrap().clone().unwrap();
        assert_eq!((sent["ok"].clone(), sent["why"].clone()), (serde_json::json!(false), serde_json::json!("the files app refused")));
        assert!(run(&c, StageCmd::Open { stage: "a.b".into() }).is_err());
    }
}
