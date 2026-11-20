//! A door's save: SimpleX's two databases and the runner's own state, as
//! one packed file its door keeps (docs/computers.md, "F: messengers
//! outside the computer"). The core never stops for one: each database is
//! copied by SQLite's online backup while it runs, which is why both run in
//! WAL mode (`wal`). When a save is due is `Due`'s, pure.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// The core's databases under `-d <dir>/sx`, the chat's first.
pub const DBS: [&str; 2] = ["sx_chat.db", "sx_agent.db"];
/// The runner's own state, in the same save: rolled back with them.
pub const STATE: &str = "door.json";
const MAGIC: &[u8; 4] = b"SXD1";
/// A save's files together, at most: a restore unpacks no more (a save is
/// the door's, but a gzip bomb fills a 256 MiB container all the same), and
/// a save is not made of more. Measured: 1.7–2.8 MB with a day's messages
/// (the core's own expiry, `wire::Cmd::ExpireDay`, keeps them to a day).
pub const UNPACKED_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// gzip's fastest level: on a `lite` container (1/16 vCPU) level 6 took
/// 1.3 s a save during a burst and level 1 0.2 s (measured, preview,
/// 2026-11-18), and the core needs that CPU for the messages themselves.
const GZIP_LEVEL: u32 = 1;
/// How long a copy waits for the core's writer (WAL: only at a checkpoint).
const BUSY_MS: u64 = 10_000;

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The databases or the save's file, as SQLite or the disk said.
    Io(String),
    /// Past `UNPACKED_MAX_BYTES`: never made, never unpacked.
    TooLarge(u64),
    /// Not a save this runner made: no gzip, no `SXD1`, cut short, or a
    /// file it never packs.
    Corrupt(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "{e}"),
            SaveError::TooLarge(n) => write!(f, "{n} bytes, past the save's {UNPACKED_MAX_BYTES}"),
            SaveError::Corrupt(e) => write!(f, "not a door's save: {e}"),
        }
    }
}

fn io(e: impl std::fmt::Display) -> SaveError {
    SaveError::Io(e.to_string())
}

/// Each database in WAL mode before the core opens it; one that is not
/// there yet is made empty with `auto_vacuum = FULL`, so what the core's
/// expiry deletes leaves the file (and every save) smaller.
///
/// Why WAL: the core sets no journal mode, so SQLite's rollback journal,
/// in which a reader's lock (a save's copy) fails the core's COMMIT. Its
/// store's busy loop then begins again inside the failed transaction
/// ("cannot start a transaction within a transaction") on every later
/// command, for good (simplexmq's `withImmediateTransaction`, its
/// `busy_timeout` 100 ms; seen in the spike's first local kill and
/// restore). In WAL mode a reader never blocks the writer.
pub fn wal(dir: &Path) -> Result<(), SaveError> {
    for db in DBS {
        let path = dir.join(db);
        let fresh = !path.exists();
        let c = Connection::open(&path).map_err(io)?;
        if fresh {
            c.execute_batch("PRAGMA auto_vacuum = FULL;").map_err(io)?;
        }
        let mode: String = c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)).map_err(io)?;
        if fresh {
            // a page written, so the new file says WAL before the core reads it
            c.execute_batch("CREATE TABLE door_wal (x); DROP TABLE door_wal;").map_err(io)?;
        }
        assert_eq!(mode, "wal", "{db} is in WAL mode before the core opens it");
    }
    Ok(())
}

/// A consistent copy of `source` at `target`, by SQLite's online backup: its
/// pages copied as they are, in one step, in a read transaction the core's
/// writer goes on beside. `VACUUM INTO` rebuilt every b-tree, about 4x the
/// CPU at 1/16 of a vCPU (measured locally: 150–200 ms against 40 ms for
/// both databases).
pub fn copy(source: &Path, target: &Path) -> Result<(), SaveError> {
    let _ = std::fs::remove_file(target);
    let c = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(io)?;
    c.busy_timeout(Duration::from_millis(BUSY_MS)).map_err(io)?;
    c.backup(rusqlite::MAIN_DB, target, None).map_err(io)
}

/// The save: `state` first, then each database copied into `work` and
/// packed. The state is read before the copies, so what it says (its
/// cursor, its mark) the databases after it already hold.
pub fn make(dir: &Path, work: &Path, state: &[u8]) -> Result<Vec<u8>, SaveError> {
    std::fs::create_dir_all(work).map_err(io)?;
    let mut files: Vec<(&str, Vec<u8>)> = vec![(STATE, state.to_vec())];
    for db in DBS {
        let target = work.join(db);
        copy(&dir.join(db), &target)?;
        let bytes = std::fs::read(&target).map_err(io)?;
        let _ = std::fs::remove_file(&target);
        files.push((db, bytes));
    }
    pack(&files)
}

pub fn pack(files: &[(&str, Vec<u8>)]) -> Result<Vec<u8>, SaveError> {
    let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
    if total > UNPACKED_MAX_BYTES {
        return Err(SaveError::TooLarge(total));
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(GZIP_LEVEL));
    gz.write_all(MAGIC).map_err(io)?;
    for (name, bytes) in files {
        assert!(*name == STATE || DBS.contains(name), "a save packs only its own files");
        gz.write_all(&(name.len() as u16).to_be_bytes()).map_err(io)?;
        gz.write_all(name.as_bytes()).map_err(io)?;
        gz.write_all(&(bytes.len() as u64).to_be_bytes()).map_err(io)?;
        gz.write_all(bytes).map_err(io)?;
    }
    gz.finish().map_err(io)
}

/// A save's files, by name: the state and each database at most once.
pub fn unpack(packed: &[u8]) -> Result<Vec<(String, Vec<u8>)>, SaveError> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(packed).take(UNPACKED_MAX_BYTES + 1).read_to_end(&mut raw).map_err(|e| SaveError::Corrupt(format!("no gzip: {e}")))?;
    if raw.len() as u64 > UNPACKED_MAX_BYTES {
        return Err(SaveError::TooLarge(raw.len() as u64));
    }
    if raw.len() < MAGIC.len() || &raw[..MAGIC.len()] != MAGIC {
        return Err(SaveError::Corrupt("no SXD1".into()));
    }
    let mut at = MAGIC.len();
    let mut take = |n: usize| -> Result<&[u8], SaveError> {
        let end = at.checked_add(n).filter(|e| *e <= raw.len()).ok_or_else(|| SaveError::Corrupt("cut short".into()))?;
        let s = &raw[at..end];
        at = end;
        Ok(s)
    };
    let mut out: Vec<(String, Vec<u8>)> = vec![];
    // bounded: each pass takes a file of its own name, of which there are 3
    while out.len() < 1 + DBS.len() {
        let Ok(n) = take(2) else { break };
        let n = u16::from_be_bytes([n[0], n[1]]) as usize;
        let name = String::from_utf8(take(n)?.to_vec()).map_err(|_| SaveError::Corrupt("a name that is not text".into()))?;
        let known = name == STATE || DBS.contains(&name.as_str());
        if !known || out.iter().any(|(f, _)| *f == name) {
            return Err(SaveError::Corrupt(format!("it names {name:?}")));
        }
        let len = take(8)?;
        let len = u64::from_be_bytes(len.try_into().expect("8 bytes")) as usize;
        out.push((name, take(len)?.to_vec()));
    }
    if take(1).is_ok() {
        return Err(SaveError::Corrupt("more after its files".into()));
    }
    Ok(out)
}

/// Each database's `data_version`, on connections of the watcher's own: it
/// moves when another connection (the core's) commits.
pub struct Watch {
    conns: Vec<Connection>,
}

impl Watch {
    pub fn open(dir: &Path) -> Result<Watch, SaveError> {
        let mut conns = vec![];
        for db in DBS {
            let c = Connection::open_with_flags(dir.join(db), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(io)?;
            c.busy_timeout(Duration::from_millis(BUSY_MS)).map_err(io)?;
            conns.push(c);
        }
        Ok(Watch { conns })
    }

    /// The chat database's version, then the agent database's.
    pub fn versions(&self) -> [i64; 2] {
        let read = |c: &Connection| c.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)).unwrap_or(-1);
        [read(&self.conns[0]), read(&self.conns[1])]
    }
}

/// A change is saved this long after it is seen (more meanwhile go in the
/// same save).
pub const DEBOUNCE_MS: u64 = 1_000;
/// Saves are at least this far apart: each takes CPU the core needs for
/// the messages themselves (measured on `lite`, 2026-11-18: 53 saves of
/// 3.4 s each during a burst of 100 each way, the burst itself slowed).
pub const GAP_MS_MIN: u64 = 3_000;
/// A change of the agent database alone, with nothing said by the core
/// meanwhile, is its server statistics (`servers_stats`, rewritten every
/// 10 s while idle: measured): saved this late at most.
pub const SLOW_MS: u64 = 10 * 60_000;
const _: () = assert!(DEBOUNCE_MS < GAP_MS_MIN && GAP_MS_MIN < SLOW_MS, "a change waits less than the gap, the gap less than idle statistics");

/// When the next save is due, from what changed: the rule alone, its clock
/// the caller's (ms since the runner started), so it is tested on its own.
#[derive(Debug, Default)]
pub struct Due {
    /// A change that matters (a message, a contact, the runner's own
    /// state), first seen at.
    fast: Option<u64>,
    /// The core's idle statistics alone, first seen at.
    slow: Option<u64>,
    /// When the last save began.
    last: Option<u64>,
}

impl Due {
    /// A change that matters, seen at `now`.
    pub fn changed(&mut self, now: u64) {
        self.fast.get_or_insert(now);
    }

    /// The agent database alone changed at `now`, the core silent.
    pub fn idle_stats(&mut self, now: u64) {
        self.slow.get_or_insert(now);
    }

    /// Whether a save begins at `now`: a change `DEBOUNCE_MS` old and the
    /// last save `GAP_MS_MIN` ago, or idle statistics `SLOW_MS` old.
    pub fn take(&mut self, now: u64) -> bool {
        let gap = self.last.is_none_or(|l| now.saturating_sub(l) >= GAP_MS_MIN);
        let fast = self.fast.is_some_and(|t| now.saturating_sub(t) >= DEBOUNCE_MS) && gap;
        let slow = self.slow.is_some_and(|t| now.saturating_sub(t) >= SLOW_MS);
        if fast || slow {
            self.fast = None;
            self.slow = None;
            self.last = Some(now);
        }
        fast || slow
    }

    /// A save that failed: what it held is due again (after the gap).
    pub fn failed(&mut self, at: u64) {
        self.fast.get_or_insert(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("door-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The core's own way of writing (simplexmq: `busy_timeout` 100 ms,
    /// `BEGIN IMMEDIATE`), on a database a save is copying.
    fn core_writer(path: &Path) -> Connection {
        let c = Connection::open(path).unwrap();
        c.busy_timeout(Duration::from_millis(100)).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS msgs (id INTEGER PRIMARY KEY, body BLOB)").unwrap();
        c
    }

    /// Goal: a save's copy never fails the core's commit (the spike's first
    /// fix). Method: a reader holds a read transaction, as a copy does,
    /// while the core's writer commits; in the rollback journal the commit
    /// is BUSY, in WAL mode (`wal`) it commits.
    #[test]
    fn a_save_reading_never_fails_the_cores_commit() {
        for (wal_first, commits) in [(false, false), (true, true)] {
            let d = dir(&format!("wal-{wal_first}"));
            if wal_first {
                wal(&d).unwrap();
            }
            let writer = core_writer(&d.join(DBS[0]));
            writer.execute("INSERT INTO msgs (body) VALUES (x'00')", []).unwrap();
            let reader = Connection::open(d.join(DBS[0])).unwrap();
            reader.execute_batch("BEGIN; SELECT count(*) FROM msgs;").unwrap();
            writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO msgs (body) VALUES (x'01');").unwrap();
            let commit = writer.execute_batch("COMMIT");
            assert_eq!(commit.is_ok(), commits, "WAL first: {wal_first}, the commit: {commit:?}");
        }
    }

    /// Goal: `wal` leaves both databases in WAL mode for the core, a new one
    /// made with `auto_vacuum = FULL`, an existing one (an adopted link's)
    /// switched in place with its rows. Method: each case, read back.
    #[test]
    fn both_databases_are_left_in_wal_mode() {
        let d = dir("modes");
        let old = core_writer(&d.join(DBS[1]));
        old.execute("INSERT INTO msgs (body) VALUES (x'07')", []).unwrap();
        drop(old);
        wal(&d).unwrap();
        for (db, vacuum) in [(DBS[0], 1), (DBS[1], 0)] {
            let c = Connection::open(d.join(db)).unwrap();
            let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
            let auto: i64 = c.query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
            assert_eq!((mode.as_str(), auto), ("wal", vacuum), "{db}");
        }
        let n: i64 = Connection::open(d.join(DBS[1])).unwrap().query_row("SELECT count(*) FROM msgs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "the adopted database keeps its rows");
    }

    /// Goal: a save is a page copy, never a rebuild (the spike's second
    /// fix: `VACUUM INTO` rebuilt every b-tree, 4x the CPU on `lite`), and
    /// it holds what was committed, not what the core is still writing.
    /// Method: a database with free pages (a rebuild drops them) and an
    /// open write transaction, copied: the copy has the source's pages,
    /// its committed rows, and none of the open one's.
    #[test]
    fn a_save_copies_pages_and_only_what_was_committed() {
        let d = dir("copy");
        // an adopted link's database: made without `auto_vacuum`, so what
        // it deleted stays as free pages
        let w = Connection::open(d.join(DBS[1])).unwrap();
        w.execute_batch("CREATE TABLE msgs (id INTEGER PRIMARY KEY, body BLOB)").unwrap();
        for _ in 0..200 {
            w.execute("INSERT INTO msgs (body) VALUES (zeroblob(1000))", []).unwrap();
        }
        w.execute("DELETE FROM msgs WHERE id > 20", []).unwrap();
        drop(w);
        wal(&d).unwrap();
        let w = Connection::open(d.join(DBS[1])).unwrap();
        let pages = |p: &Path| -> (i64, i64) {
            let c = Connection::open(p).unwrap();
            (c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap(), c.query_row("PRAGMA freelist_count", [], |r| r.get(0)).unwrap())
        };
        let source = pages(&d.join(DBS[1]));
        assert!(source.1 > 0, "the source has free pages: {source:?}");
        w.execute_batch("BEGIN IMMEDIATE; INSERT INTO msgs (body) VALUES (x'ff');").unwrap();
        copy(&d.join(DBS[1]), &d.join("copy.db")).unwrap();
        assert_eq!(pages(&d.join("copy.db")), source, "the copy's pages are the source's");
        let n: i64 = Connection::open(d.join("copy.db")).unwrap().query_row("SELECT count(*) FROM msgs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 20, "only committed rows");
    }

    /// Goal: a save packs at gzip's fastest level (the spike's measure), and
    /// unpacks as it was packed; one cut short, one with a stranger's file,
    /// a file twice, or bytes after its files is refused, as is one past
    /// the size bound either way. Method: each case.
    #[test]
    fn a_save_unpacks_whole_or_not_at_all() {
        let files = vec![(STATE, b"{}".to_vec()), (DBS[0], vec![7u8; 100_000]), (DBS[1], vec![9u8; 10])];
        let packed = pack(&files).unwrap();
        // the gzip header's XFL: 4, "compressor used fastest algorithm"
        assert_eq!(packed[8], 4, "level {GZIP_LEVEL} is gzip's fastest");
        let back: Vec<(String, Vec<u8>)> = files.iter().map(|(n, b)| (n.to_string(), b.clone())).collect();
        assert_eq!(unpack(&packed).unwrap(), back);
        assert!(matches!(unpack(&packed[..packed.len() / 2]), Err(SaveError::Corrupt(_))));
        let raw = |parts: &[(&str, &[u8])], tail: &[u8]| {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
            gz.write_all(MAGIC).unwrap();
            for (n, b) in parts {
                gz.write_all(&(n.len() as u16).to_be_bytes()).unwrap();
                gz.write_all(n.as_bytes()).unwrap();
                gz.write_all(&(b.len() as u64).to_be_bytes()).unwrap();
                gz.write_all(b).unwrap();
            }
            gz.write_all(tail).unwrap();
            gz.finish().unwrap()
        };
        assert!(matches!(unpack(&raw(&[("../etc/passwd", b"x")], b"")), Err(SaveError::Corrupt(_))));
        assert!(matches!(unpack(&raw(&[(DBS[0], b"a"), (DBS[0], b"b")], b"")), Err(SaveError::Corrupt(_))));
        assert!(matches!(unpack(&raw(&[(STATE, b"{}"), (DBS[0], b"a"), (DBS[1], b"b")], b"!")), Err(SaveError::Corrupt(_))));
        assert!(matches!(pack(&[(DBS[0], vec![0u8; UNPACKED_MAX_BYTES as usize + 1])]), Err(SaveError::TooLarge(_))));
        let bomb = raw(&[(DBS[0], &vec![0u8; UNPACKED_MAX_BYTES as usize + 1])], b"");
        assert!(matches!(unpack(&bomb), Err(SaveError::TooLarge(_))));
    }

    /// Goal: while busy, saves are at least `GAP_MS_MIN` apart (the spike's
    /// third fix). Method: a change every 100 ms for a minute; then one
    /// change alone is saved `DEBOUNCE_MS` after it.
    #[test]
    fn a_burst_is_saved_once_a_gap() {
        let mut due = Due::default();
        let mut saves = vec![];
        for now in (0..60_000).step_by(100) {
            due.changed(now);
            if due.take(now) {
                saves.push(now);
            }
        }
        assert!(saves.windows(2).all(|w| w[1] - w[0] >= GAP_MS_MIN), "{saves:?}");
        assert!(saves.len() as u64 <= 60_000 / GAP_MS_MIN + 1, "{} saves", saves.len());
        let mut due = Due::default();
        due.changed(5_000);
        assert!(!due.take(5_000 + DEBOUNCE_MS - 1));
        assert!(due.take(5_000 + DEBOUNCE_MS));
        assert!(!due.take(5_000 + DEBOUNCE_MS + 1), "nothing more is due");
    }

    /// Goal: the core's idle statistics alone wait `SLOW_MS` (the spike's
    /// fourth fix: a save every 10 s idle was most of `lite`'s CPU), and
    /// a change that matters meanwhile is saved at once with them. Method:
    /// the statistics every 10 s for 20 minutes; then the same with a
    /// message's change halfway.
    #[test]
    fn idle_statistics_wait_ten_minutes() {
        let mut due = Due::default();
        let saves: Vec<u64> = (0..1_300_000).step_by(10_000).filter(|now| {
            due.idle_stats(*now);
            due.take(*now)
        }).collect();
        // the first change seen after a save waits its `SLOW_MS`
        assert_eq!(saves, vec![SLOW_MS, 10_000 + 2 * SLOW_MS], "only every {SLOW_MS} ms");
        let mut due = Due::default();
        due.idle_stats(0);
        due.changed(30_000);
        assert!(due.take(30_000 + DEBOUNCE_MS), "a message's change is saved at once");
        assert!(!due.take(30_000 + DEBOUNCE_MS + SLOW_MS - 1), "and the statistics with it");
    }
}
