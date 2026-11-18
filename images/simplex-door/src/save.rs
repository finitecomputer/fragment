//! SPIKE: a save is the core's two databases, each copied whole and
//! consistent by `VACUUM INTO` while the core runs (a read transaction:
//! the core's writer is not paused), and the door's own state, packed
//! (`SXD1`, then each file's name and bytes) and gzipped.
//!
//! The two databases are copied one after the other, so a save's pair can
//! straddle a change of the core's (one database newer than the other);
//! the spike measures whether that ever shows (docs: "SimpleX door spike:
//! results").

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

/// The core's databases under `-d <dir>/sx`.
pub const DBS: [&str; 2] = ["sx_chat.db", "sx_agent.db"];
/// The door's own state, rolled back with them.
pub const STATE: &str = "door.json";
const MAGIC: &[u8; 4] = b"SXD1";
/// A save's files, unpacked, at most this (a restore reads no bomb).
const UNPACKED_MAX: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Timing {
    pub vacuum_ms: u64,
    pub pack_ms: u64,
    pub raw_bytes: u64,
    pub packed_bytes: u64,
}

/// A consistent copy of `src` at `dst`, made beside the live writer.
pub fn vacuum(src: &Path, dst: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(dst);
    let c = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| format!("{}: {e}", src.display()))?;
    c.busy_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())?;
    c.execute("VACUUM INTO ?1", [dst.to_string_lossy()]).map_err(|e| format!("VACUUM INTO from {}: {e}", src.display()))?;
    Ok(())
}

/// The save: `state` first (so its cursor is never ahead of the databases
/// copied after it), then each database copied into `work` and packed.
pub fn make(dir: &Path, work: &Path, state: &[u8]) -> Result<(Vec<u8>, Timing), String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut files: Vec<(String, Vec<u8>)> = vec![(STATE.to_string(), state.to_vec())];
    for db in DBS {
        let src = dir.join(db);
        if !src.exists() {
            continue;
        }
        let dst = work.join(db);
        vacuum(&src, &dst)?;
        files.push((db.to_string(), std::fs::read(&dst).map_err(|e| e.to_string())?));
        let _ = std::fs::remove_file(&dst);
    }
    let vacuum_ms = t0.elapsed().as_millis() as u64;
    let t1 = Instant::now();
    let (packed, raw) = pack(&files)?;
    let packed_bytes = packed.len() as u64;
    Ok((packed, Timing { vacuum_ms, pack_ms: t1.elapsed().as_millis() as u64, raw_bytes: raw, packed_bytes }))
}

pub fn pack(files: &[(String, Vec<u8>)]) -> Result<(Vec<u8>, u64), String> {
    let mut raw = Vec::new();
    raw.extend_from_slice(MAGIC);
    for (name, bytes) in files {
        raw.extend_from_slice(&(name.len() as u16).to_be_bytes());
        raw.extend_from_slice(name.as_bytes());
        raw.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        raw.extend_from_slice(bytes);
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    gz.write_all(&raw).map_err(|e| e.to_string())?;
    Ok((gz.finish().map_err(|e| e.to_string())?, raw.len() as u64))
}

/// A save's files, by name.
pub fn unpack(packed: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(packed).take(UNPACKED_MAX + 1).read_to_end(&mut raw).map_err(|e| format!("a save that is no gzip: {e}"))?;
    if raw.len() as u64 > UNPACKED_MAX || raw.len() < 4 || &raw[..4] != MAGIC {
        return Err("a save that is not SXD1".into());
    }
    let mut at = 4usize;
    let mut out = vec![];
    let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = at.checked_add(n).filter(|e| *e <= raw.len()).ok_or("a save cut short")?;
        let s = &raw[*at..end];
        *at = end;
        Ok(s)
    };
    while at < raw.len() {
        let n = u16::from_be_bytes(take(&mut at, 2)?.try_into().expect("2 bytes")) as usize;
        let name = String::from_utf8(take(&mut at, n)?.to_vec()).map_err(|_| "a save's name that is not text")?;
        if name != STATE && !DBS.contains(&name.as_str()) {
            return Err(format!("a save names {name:?}"));
        }
        let len = u64::from_be_bytes(take(&mut at, 8)?.try_into().expect("8 bytes")) as usize;
        out.push((name, take(&mut at, len)?.to_vec()));
    }
    Ok(out)
}

/// Each database in WAL mode, before the core opens it (made empty when
/// none is there yet, with the core's own `auto_vacuum`). The core sets no
/// journal mode, so SQLite's default (a rollback journal) is what it runs
/// with otherwise; and in that mode any reader holding its lock as the
/// core commits (a save's `VACUUM INTO`) fails the core's COMMIT, whose
/// busy loop then begins again inside the open transaction ("cannot start
/// a transaction within a transaction"), breaking its store for good
/// (measured in the spike's first local run). In WAL mode a reader never
/// blocks the writer.
pub fn wal(dir: &Path) -> Result<Vec<String>, String> {
    let mut modes = vec![];
    for db in DBS {
        let path = dir.join(db);
        let fresh = !path.exists();
        let c = Connection::open(&path).map_err(|e| format!("{db}: {e}"))?;
        if fresh {
            c.execute_batch("PRAGMA auto_vacuum = FULL;").map_err(|e| e.to_string())?;
        }
        let mode: String = c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)).map_err(|e| format!("{db}: {e}"))?;
        if fresh {
            // a page written, so the file says WAL before the core reads it
            c.execute_batch("CREATE TABLE door_wal (x); DROP TABLE door_wal;").map_err(|e| e.to_string())?;
        }
        modes.push(format!("{db}:{mode}"));
    }
    Ok(modes)
}

/// Each database's `data_version`, read on connections of this watcher's
/// own: a number that moves when another connection (the core) commits.
pub struct Watch {
    conns: Vec<Connection>,
}

impl Watch {
    pub fn open(dir: &Path) -> Result<Watch, String> {
        let mut conns = vec![];
        for db in DBS {
            let c = Connection::open_with_flags(dir.join(db), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| format!("{db}: {e}"))?;
            c.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            conns.push(c);
        }
        Ok(Watch { conns })
    }

    pub fn versions(&self) -> Vec<i64> {
        self.conns.iter().map(|c| c.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)).unwrap_or(-1)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_unpacks_as_it_was_packed() {
        let files = vec![(STATE.to_string(), b"{}".to_vec()), (DBS[0].to_string(), vec![7u8; 100_000])];
        let (packed, raw) = pack(&files).unwrap();
        assert!(packed.len() < raw as usize);
        assert_eq!(unpack(&packed).unwrap(), files);
        assert!(unpack(&packed[..packed.len() / 2]).is_err());
    }
}
