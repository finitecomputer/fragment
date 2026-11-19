//! The bridge's state on disk (`/data/bridge`): a snapshot, and a log of
//! what each step changed since it.
//!
//! - `state.json`, the snapshot: the whole state, in the format every
//!   bridge before the log wrote and read, written beside and renamed over.
//! - `state.log`, the log: one line per step that changed the state, each
//!   only what changed (a cursor, a turn, a count), appended and synced
//!   before the step's effects. Its first line names the snapshot it
//!   follows (its bytes' SHA-256), so a log that follows another one (an
//!   older bridge wrote the snapshot since, or a save copied the two at
//!   different moments) is read as none.
//!
//! So a step writes what it changed, one sync, not the whole state (up to
//! `STATE_FILE_MAX_BYTES`) and two (F14 of FIN-453's audit). The snapshot
//! is written whole again with a fresh log (a compaction) as the bridge
//! starts, once the log would pass its room (`room`), and after a hold.
//! While the platform holds the computer, the first write is whole with an
//! empty log, and the hold's answer waits for it (`Gate`); each after
//! writes the snapshot alone, as before the log, the empty log naming an
//! earlier snapshot read as none. So a save holds the state whole in
//! `state.json`, which is what an older bridge, rolled back to, reads, and
//! no step of a log it copies is read over a later snapshot.
//!
//! An append-only log beside a snapshot, rather than SQLite, because of the
//! hold's rule (docs/computers.md, "What changes under the hold": each file
//! replaced whole or appended, so a save's copy is whole), which a log meets
//! as it is and a database rewriting its pages in place would meet only by
//! a copy under every hold; and the snapshot stays the file an older bridge
//! reads.
//!
//! What a crash leaves: the state as of the last step whose write returned,
//! or the one being written. Mid-append, the log's last line is torn (no
//! newline, or a checksum that fails), and the log is read up to its first
//! line that is not whole. Mid-compaction, either the old snapshot and its
//! log, or the new snapshot (holding all of the log) with the old log,
//! which follows another snapshot and is read as none. Any pair a save's
//! copy holds reads as a state the bridge wrote at some step, which is
//! all a cache needs (docs/computers.md, "Durability").

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::driver::BridgeError;
use crate::engine::{self, Helper, State, Turn};
use crate::limits;

/// The snapshot's name in the state's directory.
pub const SNAPSHOT: &str = "state.json";
/// The log's.
pub const LOG: &str = "state.log";
/// The log's format, named in its first line: a log of another is read as
/// none.
const LOG_FORMAT: u64 = 1;

/// What the hold's answer and the store share (driver.rs, `answer_holds`).
/// Each side stores its own flag before it loads the other's (both
/// `SeqCst`), so at least one sees the other: either the answer sees the
/// log hold a change and waits, or the write sees the hold and writes the
/// state whole.
#[derive(Debug, Default)]
pub struct Gate {
    /// The platform holds the computer: every write is whole.
    pub holding: AtomicBool,
    /// The log holds a change its snapshot does not: the hold is not
    /// answered until a whole write (`Store::settle`) clears it. A write
    /// that sees the hold first never sets it.
    pub logged: AtomicBool,
    /// Told as a whole write clears `logged`: the answer is looked at again
    /// then, not a poll later.
    pub settled: tokio::sync::Notify,
}

/// What one step changed: each of the state's fields only as it changed,
/// and of its maps only the entries that changed (`None`: removed).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    cursors: BTreeMap<String, Option<u64>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    turns: BTreeMap<String, Option<Turn>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admitted: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    said: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    agent_turns: BTreeMap<String, Option<Vec<i64>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Helper>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    unheard: BTreeMap<String, Option<u64>>,
}

impl Change {
    fn is_empty(&self) -> bool {
        let Change { version, boot, cursors, turns, admitted, said, agent_turns, children, unheard } = self;
        version.is_none() && boot.is_none() && cursors.is_empty() && turns.is_empty() && admitted.is_none() && said.is_none() && agent_turns.is_empty() && children.is_none() && unheard.is_empty()
    }
}

/// What changed from `was` to `now`. Every field of the state is named (no
/// `..`): one added to it fails to compile here until its change is.
fn changed(was: &State, now: &State) -> Change {
    let State { version, boot, cursors, turns, admitted, said, agent_turns, children, unheard } = now;
    Change {
        version: (*version != was.version).then_some(*version),
        boot: (*boot != was.boot).then_some(*boot),
        cursors: entries(&was.cursors, cursors),
        turns: entries(&was.turns, turns),
        admitted: (*admitted != was.admitted).then_some(*admitted),
        said: (*said != was.said).then_some(*said),
        agent_turns: entries(&was.agent_turns, agent_turns),
        children: (*children != was.children).then(|| children.clone()),
        unheard: entries(&was.unheard, unheard),
    }
}

/// The entries of `now` that are not `was`'s, and `was`'s that `now` lacks.
fn entries<V: Clone + PartialEq>(was: &BTreeMap<String, V>, now: &BTreeMap<String, V>) -> BTreeMap<String, Option<V>> {
    let set = now.iter().filter(|(k, v)| was.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), Some(v.clone())));
    let gone = was.keys().filter(|k| !now.contains_key(*k)).map(|k| (k.clone(), None));
    set.chain(gone).collect()
}

/// `change` made to `state`.
fn apply(state: &mut State, change: Change) {
    let Change { version, boot, cursors, turns, admitted, said, agent_turns, children, unheard } = change;
    if let Some(v) = version {
        state.version = v;
    }
    if let Some(b) = boot {
        state.boot = b;
    }
    put(&mut state.cursors, cursors);
    put(&mut state.turns, turns);
    if let Some(a) = admitted {
        state.admitted = a;
    }
    if let Some(s) = said {
        state.said = s;
    }
    put(&mut state.agent_turns, agent_turns);
    if let Some(c) = children {
        state.children = c;
    }
    put(&mut state.unheard, unheard);
}

fn put<V>(map: &mut BTreeMap<String, V>, entries: BTreeMap<String, Option<V>>) {
    for (k, v) in entries {
        match v {
            Some(v) => map.insert(k, v),
            None => map.remove(&k),
        };
    }
}

/// A log line: its JSON's CRC-32 (8 lowercase hex), a space, the JSON
/// (serde_json's compact form holds no newline), a newline.
fn frame(json: &[u8]) -> Vec<u8> {
    debug_assert!(!json.contains(&b'\n'), "compact JSON is one line");
    let mut line = format!("{:08x} ", crc32fast::hash(json)).into_bytes();
    line.extend_from_slice(json);
    line.push(b'\n');
    line
}

/// The JSON of each whole line of `bytes`, in order, up to the first that
/// is not whole (no newline, or its checksum fails: a crash cut its
/// append), and whether one was not.
fn lines(bytes: &[u8]) -> (Vec<&[u8]>, bool) {
    let mut whole = Vec::new();
    let mut rest = bytes;
    // bounded by `bytes`: each pass takes a line off it
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|b| *b == b'\n') else { return (whole, true) };
        let line = &rest[..end];
        let sum = line.get(..8).and_then(|h| std::str::from_utf8(h).ok()).filter(|h| h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).and_then(|h| u32::from_str_radix(h, 16).ok());
        let json = line.get(9..).filter(|_| line[8] == b' ');
        match (sum, json) {
            (Some(sum), Some(json)) if crc32fast::hash(json) == sum => whole.push(json),
            _ => return (whole, true),
        }
        rest = &rest[end + 1..];
    }
    (whole, false)
}

/// The SHA-256 of a snapshot's bytes, which its log's first line names.
fn snapshot_id(bytes: &[u8]) -> String {
    crate::records::hex(&Sha256::digest(bytes))
}

/// The log's first line: its format, and the snapshot it follows.
fn header(snapshot: &[u8]) -> Vec<u8> {
    frame(json!({ "log": LOG_FORMAT, "snapshot": snapshot_id(snapshot) }).to_string().as_bytes())
}

/// `path`'s bytes, or none; past `max` bytes, `Err(len)` read no further.
fn read_at_most(path: &Path, max: usize) -> std::io::Result<Option<Result<Vec<u8>, usize>>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    Ok(Some(if bytes.len() > max { Err(bytes.len()) } else { Ok(bytes) }))
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOT)
}

/// The state as last written: the snapshot, and each step its log holds
/// after it; or a fresh one. A state of another format (an older or newer
/// bridge's) is set aside for a fresh one: the state is a cache of the
/// journal (one life per turn), so starting from none reads the chats again
/// and runs nothing twice. A snapshot that does not read, or a state that
/// contradicts itself, is refused. A log that follows another snapshot,
/// or of another format, is read as none, and one cut by a crash up to its
/// first line that is not whole: each is a state the bridge wrote.
pub fn load(dir: &Path) -> Result<State, BridgeError> {
    let path = state_path(dir);
    let corrupt = |why: String| BridgeError::Corrupt(format!("{}: {why}", path.display()));
    let bytes = match read_at_most(&path, limits::STATE_FILE_MAX_BYTES) {
        Ok(Some(Ok(bytes))) => bytes,
        Ok(Some(Err(len))) => return Err(corrupt(format!("{len} bytes or more, past the bound"))),
        // no snapshot: no log follows it
        Ok(None) => return Ok(State::default()),
        Err(e) => return Err(BridgeError::Disk(format!("{}: {e}", path.display()))),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| corrupt(e.to_string()))?;
    let Some(version) = value.get("version").and_then(Value::as_u64) else { return Err(corrupt("no version".into())) };
    if !ours(version) {
        crate::ev!("state.set_aside", { "path": path.display().to_string(), "version": version, "why": "another bridge's format: the journal says which turns ran" });
        return Ok(State::default());
    }
    let mut state: State = serde_json::from_value(value).map_err(|e| corrupt(e.to_string()))?;
    let steps = replay(dir, &bytes, &mut state)?;
    if !ours(u64::from(state.version)) {
        crate::ev!("state.set_aside", { "path": dir.join(LOG).display().to_string(), "version": state.version, "why": "another bridge's format: the journal says which turns ran" });
        return Ok(State::default());
    }
    engine::check(&state).map_err(|c| corrupt(c.0))?;
    crate::ev!("state.read", { "snapshotBytes": bytes.len(), "steps": steps });
    Ok(state)
}

fn ours(version: u64) -> bool {
    engine::STATE_VERSIONS.iter().any(|v| u64::from(*v) == version)
}

/// Each step the log in `dir` holds after the snapshot `snapshot`, made to
/// `state`: how many.
fn replay(dir: &Path, snapshot: &[u8], state: &mut State) -> Result<usize, BridgeError> {
    let path = dir.join(LOG);
    let aside = |why: &str| crate::ev!("state.log_set_aside", { "path": path.display().to_string(), "why": why });
    let bytes = match read_at_most(&path, limits::STATE_FILE_MAX_BYTES) {
        Ok(Some(Ok(bytes))) => bytes,
        Ok(Some(Err(_))) => {
            aside("past its bound: not one this bridge writes");
            return Ok(0);
        }
        Ok(None) => return Ok(0),
        Err(e) => return Err(BridgeError::Disk(format!("{}: {e}", path.display()))),
    };
    let (whole, torn) = lines(&bytes);
    let Some((head, steps)) = whole.split_first() else {
        aside("no whole first line");
        return Ok(0);
    };
    let head: Value = serde_json::from_slice(head).unwrap_or(Value::Null);
    if head["log"] != LOG_FORMAT {
        aside("another bridge's format");
        return Ok(0);
    }
    if head["snapshot"] != snapshot_id(snapshot).as_str() {
        aside("it follows another snapshot, which holds what it does or is newer");
        return Ok(0);
    }
    let mut n = 0;
    for step in steps {
        match serde_json::from_slice::<Change>(step) {
            Ok(change) => apply(state, change),
            Err(e) => {
                crate::ev!("state.log_cut", { "path": path.display().to_string(), "steps": n, "why": e.to_string() });
                return Ok(n);
            }
        }
        n += 1;
    }
    if torn {
        crate::ev!("state.log_cut", { "path": path.display().to_string(), "steps": n, "why": "its last line is torn: a crash cut its write" });
    }
    Ok(n)
}

/// The state on disk, open: what was written last, and the log at its end.
pub struct Store {
    dir: PathBuf,
    /// The state as written (snapshot and log): what a step's change is
    /// from.
    last: State,
    /// The log, open at its end, while it follows the snapshot; none before
    /// the first compaction, after a write that failed, and once a held
    /// write replaced the snapshot alone (the next write is whole).
    log: Option<File>,
    log_bytes: usize,
    snapshot_bytes: usize,
    /// The log on disk may hold steps after its first line: set before an
    /// append is tried, cleared only once an empty log is in place.
    log_steps: bool,
    gate: Arc<Gate>,
    /// A test's crash: the disk actions left before one fails, the files
    /// as they were.
    #[cfg(test)]
    fuel: Option<u32>,
}

impl Store {
    /// The state in `dir` (`load`), written whole again: a fresh snapshot
    /// and an empty log, which drops a torn line and a log of another
    /// snapshot.
    pub fn open(dir: &Path, gate: Arc<Gate>) -> Result<(Store, State), BridgeError> {
        let state = load(dir)?;
        let mut store = Store {
            dir: dir.to_path_buf(),
            last: state.clone(),
            log: None,
            log_bytes: 0,
            snapshot_bytes: 0,
            log_steps: true,
            gate,
            #[cfg(test)]
            fuel: None,
        };
        store.compact(&state)?;
        Ok((store, state))
    }

    /// Writes `now` before its step's effects: what changed since the last
    /// write, appended to the log and synced; or, held, the snapshot alone
    /// (`write_held`); or the state whole (`compact`) once the log would
    /// pass its room, after a hold, or after a write that failed. Nothing
    /// changed, nothing written.
    pub fn write(&mut self, now: &State) -> Result<(), BridgeError> {
        let change = changed(&self.last, now);
        if change.is_empty() {
            return Ok(());
        }
        // held already: written whole, and the log never said to hold it,
        // so a busy turn never keeps the hold's answer back (`Gate`)
        if self.gate.holding.load(Ordering::SeqCst) {
            return self.write_held(now);
        }
        // said before the hold is looked at again: one that came as this
        // step began is seen here, or sees this (`Gate`)
        self.gate.logged.store(true, Ordering::SeqCst);
        if self.gate.holding.load(Ordering::SeqCst) {
            return self.write_held(now);
        }
        let line = frame(&serde_json::to_vec(&change).map_err(|e| BridgeError::Disk(e.to_string()))?);
        if self.log.is_none() || self.log_bytes + line.len() > room(self.snapshot_bytes) {
            return self.compact(now);
        }
        self.log_steps = true;
        if let Err(e) = self.append(&line) {
            // a torn line may end the log: nothing more goes after it
            self.log = None;
            return Err(BridgeError::Disk(format!("{}: {e}", self.dir.join(LOG).display())));
        }
        self.log_bytes += line.len();
        apply(&mut self.last, change);
        debug_assert!(self.last == *now, "the log's change is the step's");
        Ok(())
    }

    /// While the platform holds the computer, the log is written into the
    /// snapshot (once), so its save holds the state whole in `state.json`
    /// (`Gate`).
    pub fn settle(&mut self) -> Result<(), BridgeError> {
        if self.gate.holding.load(Ordering::SeqCst) && self.gate.logged.load(Ordering::SeqCst) {
            crate::ev!("state.settled", { "logBytes": self.log_bytes });
            let last = self.last.clone();
            return self.write_held(&last);
        }
        Ok(())
    }

    /// One action on disk; a test's crash fails it, and every one after.
    fn act(&mut self) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(fuel) = self.fuel.as_mut() {
            if *fuel == 0 {
                return Err(std::io::Error::other("a test's crash"));
            }
            *fuel -= 1;
        }
        Ok(())
    }

    fn append(&mut self, line: &[u8]) -> std::io::Result<()> {
        self.act()?;
        self.log.as_mut().expect("a log is open to append to").write_all(line)?;
        self.act()?;
        self.log.as_mut().expect("a log is open to append to").sync_data()
    }

    /// Held: the first write while the log holds steps is whole with an
    /// empty log (`compact`), before the hold is answered (`Gate`), so no
    /// step of the log is on disk for a save to copy and read over a later
    /// snapshot. Each one after writes `state` as the snapshot alone, as
    /// before the log (one file and its directory synced), the empty log
    /// left as it is; the first write after the hold writes it again.
    fn write_held(&mut self, state: &State) -> Result<(), BridgeError> {
        if self.log_steps {
            return self.compact(state);
        }
        let bytes = serde_json::to_vec(state).map_err(|e| BridgeError::Disk(e.to_string()))?;
        assert!(bytes.len() <= limits::STATE_FILE_MAX_BYTES, "the engine bounds its state: {} bytes", bytes.len());
        self.log = None;
        self.write_whole(SNAPSHOT, &bytes).map_err(|e| BridgeError::Disk(format!("{}: {e}", self.dir.display())))?;
        self.last = state.clone();
        self.snapshot_bytes = bytes.len();
        self.gate.logged.store(false, Ordering::SeqCst);
        self.gate.settled.notify_one();
        Ok(())
    }

    /// Writes `state` whole: the snapshot (written beside, synced, renamed
    /// over, its directory synced), then an empty log that follows it (the
    /// same way). A crash between leaves the new snapshot with the old log:
    /// one that follows another snapshot, read as none; or, the state's
    /// bytes unchanged since the log began, one whose steps lead to the
    /// state before this step, as written.
    fn compact(&mut self, state: &State) -> Result<(), BridgeError> {
        self.log = None;
        let bytes = serde_json::to_vec(state).map_err(|e| BridgeError::Disk(e.to_string()))?;
        assert!(bytes.len() <= limits::STATE_FILE_MAX_BYTES, "the engine bounds its state: {} bytes", bytes.len());
        let head = header(&bytes);
        let disk = |e: std::io::Error, dir: &Path| BridgeError::Disk(format!("{}: {e}", dir.display()));
        self.write_whole(SNAPSHOT, &bytes).map_err(|e| disk(e, &self.dir))?;
        let log = self.write_whole(LOG, &head).map_err(|e| disk(e, &self.dir))?;
        self.last = state.clone();
        self.snapshot_bytes = bytes.len();
        self.log = Some(log);
        self.log_bytes = head.len();
        self.log_steps = false;
        self.gate.logged.store(false, Ordering::SeqCst);
        self.gate.settled.notify_one();
        Ok(())
    }

    /// `bytes` as the file `name`, whole: open, at its end.
    fn write_whole(&mut self, name: &str, bytes: &[u8]) -> std::io::Result<File> {
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!("{name}.tmp"));
        self.act()?;
        std::fs::create_dir_all(&self.dir)?;
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        self.act()?;
        std::fs::rename(&tmp, &path)?;
        self.act()?;
        File::open(&self.dir)?.sync_all()?;
        Ok(f)
    }
}

/// How many bytes the log may hold (its first line's included) before the
/// state is written whole: the snapshot's size, at least
/// `STATE_LOG_FLOOR_BYTES`, so compactions write at most about twice the
/// bytes the steps changed; and no more than the state's bound leaves the
/// snapshot, since a step's change is at least what it adds to the state
/// (the state's size is at most the snapshot's and the log's): a state
/// past its bound is found at the step that passed it, as when it was
/// written whole at each.
pub fn room(snapshot_bytes: usize) -> usize {
    snapshot_bytes.max(limits::STATE_LOG_FLOOR_BYTES).min(limits::STATE_FILE_MAX_BYTES.saturating_sub(snapshot_bytes))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
