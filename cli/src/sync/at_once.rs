//! How much a pass does at once: the scoped pool, and the gate that bounds blobs.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// Files a pass reads from code.storage at once: the pull's, and the
/// bootstrap's content checks (`adopt_identical`). A read is a round trip
/// of about 40 ms from a laptop (preview and prod alike, 2026-11-02), so
/// one at a time a 400-file pull took 17 s; 16 at once is about a second,
/// and still a handful of connections for one person's sync. A blob's
/// bytes, up to `BLOB_MAX_BYTES` each, keep a bound of their own
/// (`BLOB_TRANSFERS_AT_ONCE`).
pub(super) const READS_AT_ONCE: usize = 16;

/// Blobs (files of 1 MiB or more, up to `BLOB_MAX_BYTES` each) a pass moves
/// at once, up or down. A blob's PUT took about 450 ms before its bytes and
/// 150 ms a MB after, its HEAD about 120 ms (preview, 2026-11-02): four at
/// a time overlap those waits, and hold at most four in memory (1 GiB at
/// the largest). A push reads its files this many at a time too, each a
/// blob's upload when it is one.
pub(super) const BLOB_TRANSFERS_AT_ONCE: usize = 4;

/// At most `max` holders at once: a counting gate (std has none). A
/// holder's `Pass` lets it go when dropped.
pub(super) struct Gate {
    max: usize,
    held: Mutex<usize>,
    freed: Condvar,
}

pub(super) struct Pass<'a>(&'a Gate);

impl Gate {
    pub(super) fn new(max: usize) -> Gate {
        assert!(max >= 1, "a gate lets one through at least");
        Gate { max, held: Mutex::new(0), freed: Condvar::new() }
    }

    /// Waits for a place, and holds it until the pass is dropped.
    pub(super) fn enter(&self) -> Pass<'_> {
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let mut held = self.freed.wait_while(held, |n| *n >= self.max).unwrap_or_else(|e| e.into_inner());
        *held += 1;
        Pass(self)
    }

    /// How many hold a place now.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        *self.0.held.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

/// `f` over `items`, at most `at_once` at a time on scoped threads: each
/// item's answer in order, or `None` for one never started because one
/// before it failed (the pass is failing, so nothing more is read).
pub(super) fn at_once<T: Sync, R: Send, E: Send>(items: &[T], at_once: usize, f: impl Fn(&T) -> Result<R, E> + Sync) -> Vec<Option<Result<R, E>>> {
    assert!(at_once >= 1, "at least one at a time");
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let slots: Vec<Mutex<Option<Result<R, E>>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..at_once.min(items.len()) {
            scope.spawn(|| {
                while !failed.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let answer = f(item);
                    if answer.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
                }
            });
        }
    });
    slots.into_iter().map(|slot| slot.into_inner().unwrap_or_else(|e| e.into_inner())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: `at_once` runs that many together, never more, and answers
    /// in order. Method: the first `max` tasks each wait until `max` are in
    /// flight, which a pool running fewer at once never reaches (the wait
    /// is bounded so such a pool fails rather than hangs).
    #[test]
    fn at_once_runs_that_many_together_and_answers_in_order() {
        use std::sync::Condvar;
        let (n, max) = (40, READS_AT_ONCE);
        // (in flight now, the most at once, whether `max` were)
        let flight = Mutex::new((0usize, 0usize, false));
        let all_in = Condvar::new();
        let items: Vec<usize> = (0..n).collect();
        let answers = at_once(&items, max, |&i| -> Result<usize, ()> {
            let mut f = flight.lock().unwrap();
            f.0 += 1;
            f.1 = f.1.max(f.0);
            if f.0 == max {
                f.2 = true;
                all_in.notify_all();
            }
            let (mut f, waited) = all_in.wait_timeout_while(f, std::time::Duration::from_secs(10), |f| !f.2).unwrap();
            assert!(!waited.timed_out(), "{max} were never in flight at once");
            f.0 -= 1;
            Ok(i * 2)
        });
        assert_eq!(flight.lock().unwrap().1, max, "{max} at once, never more");
        assert_eq!(answers, (0..n).map(|i| Some(Ok(i * 2))).collect::<Vec<_>>());
    }

    /// Goal: once a task fails, nothing more starts. Method: one at a time,
    /// so which tasks started is certain.
    #[test]
    fn at_once_starts_nothing_after_a_failure() {
        let answers = at_once(&[1, 2, 3, 4, 5], 1, |&i| if i == 3 { Err(i) } else { Ok(i) });
        assert_eq!(answers, vec![Some(Ok(1)), Some(Ok(2)), Some(Err(3)), None, None]);
        assert!(at_once(&[] as &[u8], 4, |_| Ok::<_, ()>(())).is_empty());
    }

    /// Goal: the gate lets `max` through together and never more. Method:
    /// each of twice that many waits (bounded) until `max` hold a place,
    /// which a gate letting fewer through never reaches.
    #[test]
    fn a_gate_lets_that_many_through_together() {
        let gate = Gate::new(BLOB_TRANSFERS_AT_ONCE);
        let peak = Mutex::new(0usize);
        std::thread::scope(|scope| {
            for _ in 0..BLOB_TRANSFERS_AT_ONCE * 2 {
                scope.spawn(|| {
                    let _place = gate.enter();
                    let now = gate.held();
                    let mut p = peak.lock().unwrap();
                    *p = (*p).max(now);
                    drop(p);
                    let t0 = std::time::Instant::now();
                    while *peak.lock().unwrap() < BLOB_TRANSFERS_AT_ONCE {
                        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{BLOB_TRANSFERS_AT_ONCE} never held places at once");
                        std::thread::yield_now();
                    }
                });
            }
        });
        assert_eq!(*peak.lock().unwrap(), BLOB_TRANSFERS_AT_ONCE);
        assert_eq!(gate.held(), 0);
    }
}
