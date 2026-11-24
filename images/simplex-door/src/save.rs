//! When a door's save is due (`Due`, pure) and what moved since (`Watch`):
//! its runner's own. The save's format (pack, unpack, stage, place) is
//! the bridge's (`fragment_bridge::simplex::save`), which a computer's
//! bridge shares to make a door's first save of a link it hands over.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub use fragment_bridge::simplex::save::*;

fn io(e: impl std::fmt::Display) -> SaveError {
    SaveError::Io(e.to_string())
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
