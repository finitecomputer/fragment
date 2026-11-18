//! The state on disk (store.rs): what a step writes, what a crash at each
//! action on disk leaves, what a save's copy holds, and what an older
//! bridge reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::engine::{cursor_key, Phase, State, Turn};
use crate::records;

const PAUL: &str = "npub1paul";

/// The state's two files as a copy found them (none: not there).
type Files = (Option<Vec<u8>>, Option<Vec<u8>>);

/// A fresh directory for one test's state.
fn dir(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("bridge-store-{}-{name}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A queued turn of `agent`'s in `fragment`, caused by `chat` record `seq`.
fn turn(agent: &str, fragment: &str, seq: u64, order: u64, text: &str) -> Turn {
    serde_json::from_value(json!({
        "id": records::turn_id(agent, fragment, records::CHAT, seq), "agent": agent, "fragment": fragment,
        "cause": { "fragment": fragment, "channel": records::CHAT, "seq": seq },
        "asker": PAUL, "asker_name": "Paul", "text": text, "attachments": [],
        "routine": false, "hop": 0, "order": order, "phase": "queued", "last_ms": 0, "steps": 0,
        "replies": 0, "last_part": 0, "prompts": [], "stop_requested": false, "owed": [],
    }))
    .expect("a turn")
}

/// A computer's state: `agents` agents, each following `fragments` chats
/// (`chat` and `work`), at `seq`; none of their turns open.
fn computer(agents: usize, fragments: usize, seq: u64) -> State {
    let mut s = State::default();
    for a in 0..agents {
        for f in 0..fragments {
            for channel in [records::CHAT, records::WORK] {
                s.cursors.insert(cursor_key(&format!("agent{a}--k3x9"), &format!("chat{f}--k3x9"), channel), seq);
            }
        }
    }
    s
}

/// `s` once `agent` read `chat` record `seq` in `fragment`, a turn of it
/// admitted.
fn admitted(s: &State, agent: &str, fragment: &str, seq: u64, text: &str) -> State {
    let mut s = s.clone();
    s.cursors.insert(cursor_key(agent, fragment, records::CHAT), seq);
    s.admitted += 1;
    let t = turn(agent, fragment, seq, s.admitted, text);
    s.turns.insert(t.id.clone(), t);
    s
}

/// What an older bridge (before the log) reads: `state.json` alone, as its
/// `load` did.
fn older_bridge_reads(dir: &Path) -> State {
    let bytes = std::fs::read(dir.join(SNAPSHOT)).expect("a snapshot");
    let value: Value = serde_json::from_slice(&bytes).expect("JSON");
    let version = value["version"].as_u64().expect("a version");
    assert!(engine::STATE_VERSIONS.iter().any(|v| u64::from(*v) == version), "a format it reads: {version}");
    let state: State = serde_json::from_value(value).expect("a state");
    engine::check(&state).expect("whole");
    state
}

fn open(dir: &Path) -> Store {
    Store::open(dir, Arc::new(Gate::default())).expect("opens").0
}

fn log_lines(dir: &Path) -> usize {
    std::fs::read(dir.join(LOG)).expect("a log").iter().filter(|b| **b == b'\n').count()
}

/// The steps of a conversation on a computer of 3 agents in 20 chats each:
/// each record read, a turn admitted, run and ended, a helper.
fn conversation() -> Vec<State> {
    let s0 = computer(3, 20, 4);
    let s1 = admitted(&s0, "agent0--k3x9", "chat0--k3x9", 5, "What's on today?");
    let id = records::turn_id("agent0--k3x9", "chat0--k3x9", records::CHAT, 5);
    let mut s2 = s1.clone();
    let t = s2.turns.get_mut(&id).expect("its turn");
    t.phase = Phase::Running;
    t.steps = 1;
    *s2.cursors.get_mut(&cursor_key("agent0--k3x9", "chat0--k3x9", records::WORK)).expect("a cursor") = 6;
    let mut s3 = s2.clone();
    s3.children.push(engine::Helper { agent: "agent0--k3x9".into(), fragment: "chat0--k3x9".into(), turn: id.clone(), child: "0123456789abcdef01234567".into(), goal: "look it up".into(), since: 1 });
    s3.agent_turns.insert("chat0--k3x9".into(), vec![10, 20]);
    s3.said += 1;
    let mut s4 = s3.clone();
    s4.turns.get_mut(&id).expect("its turn").replies = 1;
    *s4.cursors.get_mut(&cursor_key("agent1--k3x9", "chat3--k3x9", records::CHAT)).expect("a cursor") = 9;
    let mut s5 = s4.clone();
    s5.children.clear();
    s5.turns.remove(&id);
    s5.version = 2;
    s5.boot += 1;
    s5.agent_turns.clear();
    s5.cursors.remove(&cursor_key("agent2--k3x9", "chat19--k3x9", records::WORK));
    vec![s0, s1, s2, s3, s4, s5]
}

/// Goal: a step writes what it changed, appended to the log, and never the
/// snapshot; each reads back. The measure (F14): a record read, before
/// the log, wrote the whole state (here a computer of 3 agents in 20
/// chats, its cursors only) and synced twice; now one line and one sync.
#[test]
fn a_step_appends_only_what_it_changed() {
    let d = dir("appends");
    let s0 = computer(3, 20, 4);
    let (mut store, read) = Store::open(&d, Arc::new(Gate::default())).expect("opens");
    assert_eq!(read, State::default(), "nothing written yet");
    store.write(&s0).expect("written");
    let snapshot = std::fs::read(d.join(SNAPSHOT)).expect("a snapshot");
    let log = std::fs::metadata(d.join(LOG)).expect("a log").len();
    // a record read: its cursor
    let mut s1 = s0.clone();
    *s1.cursors.get_mut(&cursor_key("agent1--k3x9", "chat7--k3x9", records::CHAT)).expect("a cursor") = 5;
    store.write(&s1).expect("written");
    let line = std::fs::metadata(d.join(LOG)).expect("a log").len() - log;
    let whole = serde_json::to_vec(&s1).expect("serializes").len();
    eprintln!("a record read: {line} bytes appended, one sync (before: {whole} bytes written whole, two syncs)");
    assert!(line < 100, "one cursor's line: {line} bytes");
    assert!(whole > 50 * line as usize, "not the state: {whole} bytes whole");
    assert_eq!(std::fs::read(d.join(SNAPSHOT)).expect("a snapshot"), snapshot, "the snapshot untouched");
    assert_eq!(load(&d).expect("reads"), s1);
    // nothing changed: nothing written
    store.write(&s1).expect("written");
    assert_eq!(std::fs::metadata(d.join(LOG)).expect("a log").len(), log + line);
    let _ = std::fs::remove_dir_all(&d);
}

/// Valid: every field of the state, set, changed and removed, reads back
/// after each step, and again once opened (written whole).
#[test]
fn every_change_reads_back() {
    let d = dir("every");
    let mut store = open(&d);
    let steps = conversation();
    for (i, s) in steps.iter().enumerate() {
        store.write(s).expect("written");
        assert_eq!(load(&d).expect("reads"), *s, "step {i}");
    }
    assert_eq!(log_lines(&d), 1 + steps.len(), "its first line, then one per step");
    let (_, read) = Store::open(&d, Arc::new(Gate::default())).expect("opens again");
    assert_eq!(read, *steps.last().unwrap());
    assert_eq!(older_bridge_reads(&d), read, "opened, it is whole in the snapshot");
    assert_eq!(log_lines(&d), 1, "and the log is empty");
    let _ = std::fs::remove_dir_all(&d);
}

/// Restart: a crash at each action on disk (an append's write and sync, a
/// compaction's writes, renames and syncs) leaves the state as of the step
/// before or the step written, never another; and the next start opens
/// it and goes on, as a crash did when the state was written whole.
#[test]
fn a_crash_at_each_disk_action_leaves_a_state_written() {
    let steps = conversation();
    let (before, after) = (&steps[3], &steps[4]);
    for whole in [false, true] {
        let mut crashed = 0;
        for fuel in 0.. {
            let d = dir("crash");
            let mut store = open(&d);
            for s in &steps[..4] {
                store.write(s).expect("written");
            }
            // held: its first write (as the hold comes) whole with an empty
            // log, then the step's, the snapshot alone
            store.gate.holding.store(whole, Ordering::SeqCst);
            store.settle().expect("settles");
            store.fuel = Some(fuel);
            let r = store.write(after);
            let read = load(&d).expect("reads after a crash");
            match r {
                Ok(()) => {
                    assert_eq!(read, *after, "written");
                    let _ = std::fs::remove_dir_all(&d);
                    break;
                }
                Err(BridgeError::Disk(_)) => assert!(read == *before || read == *after, "a crash at action {fuel} (whole: {whole}) leaves a state written"),
                Err(e) => panic!("{e}"),
            }
            crashed += 1;
            // the same step written again, by a store that failed it, lands
            store.fuel = None;
            store.write(after).expect("written again");
            assert_eq!(load(&d).expect("reads"), *after, "a step that failed at action {fuel} (whole: {whole}) is written when asked again");
            drop(store);
            // the next life opens it, and its steps are written
            let mut next = open(&d);
            next.write(&steps[5]).expect("written");
            assert_eq!(load(&d).expect("reads"), steps[5]);
            let _ = std::fs::remove_dir_all(&d);
        }
        assert_eq!(crashed, if whole { 3 } else { 2 }, "each action crashed once");
    }
    // and a crash as a start writes the state whole
    for fuel in 0..6 {
        let d = dir("crash-open");
        let mut store = open(&d);
        for s in &steps {
            store.write(s).expect("written");
        }
        drop(store);
        let state = load(&d).expect("reads");
        let mut store = Store { dir: d.clone(), last: state.clone(), log: None, log_bytes: 0, snapshot_bytes: 0, log_steps: true, gate: Arc::new(Gate::default()), fuel: Some(fuel) };
        assert!(store.compact(&state).is_err(), "crashed at {fuel}");
        assert_eq!(load(&d).expect("reads"), state, "a start's crash at action {fuel} loses nothing");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Restart: a crash mid-append tears the log's last line, at any byte, or
/// leaves zeros past it (a size the disk kept without its data): the log is
/// read up to it, the step before.
#[test]
fn a_torn_last_line_is_read_up_to() {
    let d = dir("torn");
    let mut store = open(&d);
    let steps = conversation();
    for s in &steps {
        store.write(s).expect("written");
    }
    drop(store);
    let log = std::fs::read(d.join(LOG)).expect("a log");
    let last = log[..log.len() - 1].iter().rposition(|b| *b == b'\n').expect("lines") + 1;
    let before = &steps[steps.len() - 2];
    for cut in last..log.len() {
        std::fs::write(d.join(LOG), &log[..cut]).expect("cut");
        assert_eq!(load(&d).expect("reads"), *before, "cut at byte {cut} of {}", log.len());
    }
    let mut zeros = log[..last].to_vec();
    zeros.extend(std::iter::repeat_n(0u8, 300));
    std::fs::write(d.join(LOG), &zeros).expect("zeros");
    assert_eq!(load(&d).expect("reads"), *before, "zeros past the last whole line");
    // a byte of the last line changed: its checksum fails
    let mut flipped = log.clone();
    flipped[last + 20] ^= 0x01;
    std::fs::write(d.join(LOG), &flipped).expect("flipped");
    assert_eq!(load(&d).expect("reads"), *before, "a line whose checksum fails ends the log");
    // and the next start goes on from it, the torn line dropped
    std::fs::write(d.join(LOG), &log[..log.len() - 3]).expect("cut");
    let mut next = open(&d);
    assert_eq!(log_lines(&d), 1, "written whole: the torn line gone");
    next.write(&steps[1]).expect("written");
    assert_eq!(load(&d).expect("reads"), steps[1]);
    let _ = std::fs::remove_dir_all(&d);
}

/// Goal (a save's copy): the platform copies `/data` file by file, so a
/// save may hold the snapshot as of one action and the log as of another.
/// Every such pair, across a compaction and the appends around it, reads
/// as a state the bridge wrote: never a log made to a snapshot it does not
/// follow.
#[test]
fn any_copy_a_save_takes_reads_as_a_state_written() {
    let steps = conversation();
    // the files after each action: appends, a compaction (held), appends
    let mut copies: Vec<Files> = Vec::new();
    let take = |d: &Path| (std::fs::read(d.join(SNAPSHOT)).ok(), std::fs::read(d.join(LOG)).ok());
    for fuel in 0.. {
        let d = dir("copies");
        let mut store = open(&d);
        store.fuel = Some(fuel);
        let ran = (|| {
            store.write(&steps[0])?;
            store.write(&steps[1])?;
            store.write(&steps[2])?;
            store.gate.holding.store(true, Ordering::SeqCst);
            store.write(&steps[3])?;
            store.gate.holding.store(false, Ordering::SeqCst);
            store.write(&steps[4])?;
            store.write(&steps[5])
        })();
        copies.push(take(&d));
        let _ = std::fs::remove_dir_all(&d);
        if ran.is_ok() {
            break;
        }
    }
    assert!(copies.len() > 10, "{} moments", copies.len());
    let written: Vec<State> = std::iter::once(State::default()).chain(steps.iter().cloned()).collect();
    for (i, (snapshot, _)) in copies.iter().enumerate() {
        for (j, (_, log)) in copies.iter().enumerate() {
            let d = dir("copy");
            std::fs::create_dir_all(&d).expect("a directory");
            if let Some(s) = snapshot {
                std::fs::write(d.join(SNAPSHOT), s).expect("written");
            }
            if let Some(l) = log {
                std::fs::write(d.join(LOG), l).expect("written");
            }
            let read = load(&d).expect("reads");
            assert!(written.contains(&read), "the snapshot after action {i} and the log after action {j}: {read:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

/// Goal (a rollback): under a hold the state is written whole into the
/// snapshot before the hold is answered, and so is every write while held
/// (the snapshot alone, two syncs, as before the log: the log, naming an
/// earlier snapshot, is read as none), so a save holds the state whole in
/// `state.json`, which is all an older bridge (before the log) reads. No
/// write while held says the log holds a change, even one cut short, so a
/// busy turn never keeps the answer back. Once the hold goes, the first
/// write is whole with a fresh log, and steps append after it.
#[test]
fn held_the_snapshot_holds_the_state_whole() {
    let d = dir("held");
    let gate = Arc::new(Gate::default());
    let (mut store, _) = Store::open(&d, gate.clone()).expect("opens");
    let steps = conversation();
    store.write(&steps[0]).expect("written");
    store.write(&steps[1]).expect("written");
    store.settle().expect("settles");
    assert!(gate.logged.load(Ordering::SeqCst), "not held: the log keeps its steps");
    assert_ne!(older_bridge_reads(&d), steps[1], "an older bridge would read less");
    gate.holding.store(true, Ordering::SeqCst);
    store.settle().expect("settles");
    assert!(!gate.logged.load(Ordering::SeqCst), "held: the hold may be answered");
    assert_eq!(older_bridge_reads(&d), steps[1], "held: whole in the snapshot");
    let log = std::fs::read(d.join(LOG)).expect("a log");
    for s in &steps[2..4] {
        store.write(s).expect("written");
        assert_eq!(older_bridge_reads(&d), *s, "every write while held is whole");
        assert_eq!(load(&d).expect("reads"), *s, "and the log read as none");
        assert_eq!(std::fs::read(d.join(LOG)).expect("a log"), log, "the log untouched while held");
        assert!(!gate.logged.load(Ordering::SeqCst));
    }
    // a held write cut short: still never said to be in the log
    store.fuel = Some(0);
    assert!(store.write(&steps[4]).is_err());
    assert!(!gate.logged.load(Ordering::SeqCst), "a held write never holds the answer back");
    store.fuel = None;
    gate.holding.store(false, Ordering::SeqCst);
    store.write(&steps[4]).expect("written");
    assert_eq!(log_lines(&d), 1, "the hold gone, written whole with a fresh log");
    assert_eq!(older_bridge_reads(&d), steps[4]);
    store.write(&steps[5]).expect("written");
    assert_eq!(log_lines(&d), 2, "then appended");
    assert_eq!(load(&d).expect("reads"), steps[5]);
    let _ = std::fs::remove_dir_all(&d);
}

/// Goal (a save under the hold): the platform copies `state.json` and
/// `state.log` once the hold is answered, at moments of its own. A log's
/// steps follow the snapshot it names; held, the state may change back to
/// that very snapshot's bytes (a helper started before the hold, another
/// during it, both gone). No copy taken after the answer may then read the
/// old steps over it: every pair reads as a state written since the answer.
/// So the first write held is whole with an empty log, before the answer,
/// and a crash at each of its actions leaves the step before or this one.
#[test]
fn a_save_under_a_hold_reads_no_older_than_its_answer() {
    let base = computer(1, 1, 3);
    let id = records::turn_id("agent0--k3x9", "chat0--k3x9", records::CHAT, 3);
    let helper = |child: &str| engine::Helper { agent: "agent0--k3x9".into(), fragment: "chat0--k3x9".into(), turn: id.clone(), child: child.into(), goal: "g".into(), since: 1 };
    let mut x = base.clone();
    x.children.push(helper("0123456789abcdef01234567"));
    let mut y = base.clone();
    y.children.push(helper("76543210fedcba9876543210"));
    let take = |d: &Path| (std::fs::read(d.join(SNAPSHOT)).ok(), std::fs::read(d.join(LOG)).ok());
    // the files after each action from the answer on: the hold comes as
    // y is written (its first held write), is answered, then the state
    // goes back to the snapshot's bytes
    let mut copies: Vec<Files> = Vec::new();
    let mut answered = None;
    for fuel in 0.. {
        let d = dir("save-held");
        std::fs::create_dir_all(&d).expect("a directory");
        std::fs::write(d.join(SNAPSHOT), serde_json::to_vec(&base).expect("serializes")).expect("written");
        let gate = Arc::new(Gate::default());
        let (mut store, _) = Store::open(&d, gate.clone()).expect("opens");
        store.write(&x).expect("a helper starts: appended");
        gate.holding.store(true, Ordering::SeqCst);
        store.fuel = Some(fuel);
        let first = store.write(&y);
        if first.is_err() {
            drop(store);
            let read = load(&d).expect("reads");
            assert!(read == x || read == y, "a crash at the first held write's action {fuel} leaves the step before or this one: {read:?}");
            let _ = std::fs::remove_dir_all(&d);
            continue;
        }
        assert!(!gate.logged.load(Ordering::SeqCst), "answered once written");
        answered.get_or_insert_with(|| take(&d));
        let back = store.write(&base);
        copies.push(take(&d));
        let _ = std::fs::remove_dir_all(&d);
        if back.is_ok() {
            break;
        }
    }
    copies.insert(0, answered.expect("answered"));
    let since = [y.clone(), base.clone()];
    for (i, (snapshot, _)) in copies.iter().enumerate() {
        for (j, (_, log)) in copies.iter().enumerate() {
            let d = dir("save-copy");
            std::fs::create_dir_all(&d).expect("a directory");
            std::fs::write(d.join(SNAPSHOT), snapshot.as_ref().expect("a snapshot")).expect("written");
            std::fs::write(d.join(LOG), log.as_ref().expect("a log")).expect("written");
            let read = load(&d).expect("reads");
            assert!(since.contains(&read), "the snapshot after action {i} and the log after action {j} of the hold read as a state from before its answer: {read:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

/// Restart: an append whose line was written and whose sync failed leaves
/// that line on disk. Held next, the state the store counts as written goes
/// whole into the snapshot (the same bytes the log follows): the failed
/// step is never read over it.
#[test]
fn a_failed_append_is_never_read_over_a_held_snapshot() {
    let d = dir("failed-append");
    let base = computer(1, 1, 3);
    std::fs::create_dir_all(&d).expect("a directory");
    std::fs::write(d.join(SNAPSHOT), serde_json::to_vec(&base).expect("serializes")).expect("written");
    let gate = Arc::new(Gate::default());
    let (mut store, _) = Store::open(&d, gate.clone()).expect("opens");
    let mut x = base.clone();
    x.said = 1;
    store.fuel = Some(1);
    assert!(store.write(&x).is_err(), "its sync failed");
    store.fuel = None;
    assert_eq!(log_lines(&d), 2, "its line is on disk");
    gate.holding.store(true, Ordering::SeqCst);
    store.settle().expect("settles");
    assert_eq!(load(&d).expect("reads"), base, "the state written, not the failed step");
    let _ = std::fs::remove_dir_all(&d);
}

/// Upgrade: a state an older bridge wrote (no log), as it wrote it, reads;
/// and this bridge's snapshot is in the format it wrote, byte for byte.
#[test]
fn an_older_bridges_state_reads() {
    let d = dir("older");
    std::fs::create_dir_all(&d).expect("a directory");
    let id = records::turn_id("juniper--k3x9", "talk--k3x9", "chat", 3);
    // as the bridge before the log wrote it (`driver::save`: serde_json's
    // compact form of the state)
    let older = format!(
        r#"{{"version":2,"boot":7,"cursors":{{"juniper--k3x9|juniper--k3x9|tasks":2,"juniper--k3x9|talk--k3x9|chat":3,"juniper--k3x9|talk--k3x9|work":12}},"turns":{{"{id}":{{"id":"{id}","agent":"juniper--k3x9","fragment":"talk--k3x9","cause":{{"fragment":"talk--k3x9","channel":"chat","seq":3}},"asker":"npub1paul","asker_name":"Paul","text":"hi","attachments":[],"routine":false,"hop":0,"order":4,"phase":"queued","last_ms":0,"steps":0,"replies":0,"last_part":0,"prompts":[],"stop_requested":false,"owed":[]}}}},"admitted":4,"said":1}}"#
    );
    std::fs::write(d.join(SNAPSHOT), &older).expect("written");
    let (mut store, state) = Store::open(&d, Arc::new(Gate::default())).expect("opens");
    assert_eq!(state.boot, 7);
    assert_eq!(state.turns[&id].text, "hi");
    assert_eq!(std::fs::read_to_string(d.join(SNAPSHOT)).expect("a snapshot"), older, "written again as it was");
    assert_eq!(older_bridge_reads(&d), state);
    let mut next = state.clone();
    next.boot += 1;
    store.write(&next).expect("written");
    assert_eq!(load(&d).expect("reads"), next);
    let _ = std::fs::remove_dir_all(&d);
}

/// Rollback and forward again: an older bridge writes `state.json` whole
/// and leaves the log as it was; this bridge, back, reads its snapshot and
/// not the log, which follows another one.
#[test]
fn a_log_an_older_bridge_left_is_read_as_none() {
    let d = dir("left");
    let mut store = open(&d);
    let steps = conversation();
    for s in &steps[..3] {
        store.write(s).expect("written");
    }
    drop(store);
    // the older bridge: its own state, written whole (`driver::save`)
    let mut theirs = older_bridge_reads(&d);
    theirs.boot += 1;
    std::fs::write(d.join(SNAPSHOT), serde_json::to_vec(&theirs).expect("serializes")).expect("written");
    assert_eq!(load(&d).expect("reads"), theirs, "its snapshot, the log set aside");
    let (_, read) = Store::open(&d, Arc::new(Gate::default())).expect("opens");
    assert_eq!(read, theirs);
    let _ = std::fs::remove_dir_all(&d);
}

/// Invalid: a log of another format, past its bound, with a line whose
/// checksum holds and is no change, or whose steps make the state another
/// format, is read up to there or set aside, never refused; a snapshot
/// that does not read still is.
#[test]
fn a_log_it_cannot_read_is_set_aside() {
    let d = dir("aside");
    let mut store = open(&d);
    let steps = conversation();
    store.write(&steps[0]).expect("written");
    store.write(&steps[1]).expect("written");
    drop(store);
    let snapshot = std::fs::read(d.join(SNAPSHOT)).expect("a snapshot");
    let log = std::fs::read(d.join(LOG)).expect("a log");
    let first = log.iter().position(|b| *b == b'\n').expect("a first line") + 1;
    let older = older_bridge_reads(&d);
    // another format
    let mut other = frame(json!({ "log": 2, "snapshot": snapshot_id(&snapshot) }).to_string().as_bytes());
    other.extend_from_slice(&log[first..]);
    std::fs::write(d.join(LOG), &other).expect("written");
    assert_eq!(load(&d).expect("reads"), older);
    // past its bound
    let mut big = log.clone();
    big.extend(std::iter::repeat_n(b'x', limits::STATE_FILE_MAX_BYTES));
    std::fs::write(d.join(LOG), &big).expect("written");
    assert_eq!(load(&d).expect("reads"), older);
    // a whole line that is no change: read up to it
    let mut odd = log.clone();
    odd.extend(frame(br#"{"turns":7}"#));
    odd.extend(frame(&serde_json::to_vec(&changed(&steps[1], &steps[2])).expect("serializes")));
    std::fs::write(d.join(LOG), &odd).expect("written");
    assert_eq!(load(&d).expect("reads"), steps[1], "up to the line that is no change");
    // a step to another format: set aside, as a snapshot of one is
    let mut newer = log.clone();
    newer.extend(frame(br#"{"version":99}"#));
    std::fs::write(d.join(LOG), &newer).expect("written");
    assert_eq!(load(&d).expect("reads"), State::default());
    // no log: the snapshot alone
    std::fs::remove_file(d.join(LOG)).expect("removed");
    assert_eq!(load(&d).expect("reads"), older);
    // a snapshot that does not read is refused, as it was
    std::fs::write(d.join(SNAPSHOT), b"{\"version\": 2, ").expect("written");
    assert!(matches!(load(&d), Err(BridgeError::Corrupt(_))));
    let _ = std::fs::remove_dir_all(&d);
}

/// Bounds: the log never passes its room, and is written whole into the
/// snapshot as it would; a state near its bound is written whole at each
/// step, so one past it is found at the step that passed it. Method: a
/// turn's text of 40 KB, changed at each step (a record's cursor is ~60
/// bytes: thousands of steps between compactions).
#[test]
fn the_log_stays_within_its_room() {
    let d = dir("room");
    let mut store = open(&d);
    let mut s = admitted(&computer(2, 5, 1), "agent0--k3x9", "chat0--k3x9", 2, "");
    let id = records::turn_id("agent0--k3x9", "chat0--k3x9", records::CHAT, 2);
    store.write(&s).expect("written");
    let (mut compactions, mut steps, mut since) = (0, 0, 0);
    // bounded: one step per pass, until the log was written whole twice
    while compactions < 2 {
        steps += 1;
        since += 1;
        s.turns.get_mut(&id).expect("its turn").text = format!("{steps:08}").repeat(5_000);
        let was = store.log_bytes;
        store.write(&s).expect("written");
        assert!(store.log_bytes <= room(store.snapshot_bytes), "{} bytes past its room", store.log_bytes);
        assert_eq!(std::fs::metadata(d.join(LOG)).expect("a log").len() as usize, store.log_bytes);
        if store.log_bytes < was {
            compactions += 1;
            assert_eq!(older_bridge_reads(&d), s, "written whole");
            assert!(since >= limits::STATE_LOG_FLOOR_BYTES / 41_000, "the log filled first: {since} steps");
            since = 0;
        }
    }
    assert_eq!(load(&d).expect("reads"), s);
    assert_eq!(room(limits::STATE_FILE_MAX_BYTES - 10), 10, "near its bound, little room: written whole");
    assert_eq!(room(10), limits::STATE_LOG_FLOOR_BYTES);
    assert_eq!(room(3 * 1024 * 1024), 1024 * 1024, "the bound, not the snapshot's size");
    let _ = std::fs::remove_dir_all(&d);
}

/// A state a step changes by a field the store does not know would not be
/// written: every field is named (`changed`, `apply`), and a state that
/// differs in each reads back.
#[test]
fn each_field_is_written() {
    let base = computer(1, 1, 3);
    let id = records::turn_id("agent0--k3x9", "chat0--k3x9", records::CHAT, 3);
    let mut variants: BTreeMap<&str, State> = BTreeMap::new();
    let mut v = base.clone();
    v.boot = 9;
    variants.insert("boot", v);
    let mut v = admitted(&base, "agent0--k3x9", "chat0--k3x9", 3, "q");
    v.turns.get_mut(&id).expect("a turn").ret = Some(serde_json::from_value(json!({ "agent": "agent1--k3x9", "chat": "chat0--k3x9", "name": "Rowan" })).expect("a return"));
    v.version = 3;
    variants.insert("version", v);
    let mut v = base.clone();
    v.said = 4;
    variants.insert("said", v);
    let mut v = base.clone();
    v.admitted = 4;
    variants.insert("admitted", v);
    let mut v = base.clone();
    v.agent_turns.insert("chat0--k3x9".into(), vec![1]);
    variants.insert("agent_turns", v);
    variants.insert("turns", admitted(&base, "agent0--k3x9", "chat0--k3x9", 3, "q"));
    let mut v = base.clone();
    v.cursors.insert("x|y|chat".into(), 1);
    variants.insert("cursors", v);
    let mut v = base.clone();
    v.children.push(engine::Helper { agent: "agent0--k3x9".into(), fragment: "chat0--k3x9".into(), turn: id.clone(), child: "0123456789abcdef01234567".into(), goal: "g".into(), since: 1 });
    variants.insert("children", v);
    for (field, v) in variants {
        let d = dir("field");
        let mut store = open(&d);
        store.write(&base).expect("written");
        store.write(&v).expect("written");
        assert_eq!(load(&d).expect("reads"), v, "{field}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
