//! Work left running on the computer, outside any turn (docs/bridge.md,
//! "Work left running"): its own CPU time (its cgroup's) and network
//! bytes, read every `LOAD_SLICE_MS`, whatever started the work. Each
//! slice goes to the engine, over a floor or not (`Input::Slice`).

use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::engine::Input;
use crate::limits::{LOAD_CPU_FLOOR_PERMILLE, LOAD_NET_FLOOR_BYTES_PER_S};
#[cfg(test)]
use crate::limits::LOAD_SLICE_MS;

/// Whether a slice `ms` long, from counters `was` to `now` (CPU µs,
/// network bytes), was over either floor. A microsecond of CPU a
/// millisecond is a thousandth of a core.
pub fn over(was: (u64, u64), now: (u64, u64), ms: u64) -> bool {
    let ms = ms.max(1);
    now.0.saturating_sub(was.0) / ms >= LOAD_CPU_FLOOR_PERMILLE || now.1.saturating_sub(was.1).saturating_mul(1000) / ms >= LOAD_NET_FLOOR_BYTES_PER_S
}

/// `usage_usec` of a cgroup v2 `cpu.stat`.
pub fn cgroup_cpu(stat: &str) -> Option<u64> {
    stat.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// Bytes received and sent on every interface of `/proc/net/dev` but
/// loopback (a server the agent talks to on the computer is its own).
pub fn net_bytes(dev: &str) -> Option<u64> {
    let mut total = 0u64;
    for line in dev.lines().skip(2) {
        let (name, rest) = line.split_once(':')?;
        let f: Vec<u64> = rest.split_whitespace().map(|v| v.parse().ok()).collect::<Option<_>>()?;
        // receive: bytes and 7 more; transmit: bytes …
        if name.trim() != "lo" {
            total = total.saturating_add(*f.first()?).saturating_add(*f.get(8)?);
        }
    }
    Some(total)
}

fn read() -> Option<(u64, u64)> {
    let cpu = cgroup_cpu(&std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?)?;
    Some((cpu, net_bytes(&std::fs::read_to_string("/proc/net/dev").ok()?)?))
}

/// Reads the counters every `every` for the bridge's life, and gives the
/// engine each slice (`wrap` puts it in `inbox`). A slice the platform's
/// hold overlapped (`held`: it saves then) says nothing: else a save would
/// hold the computer it saves, and its end save it again. With no
/// counters to read, it says so once and gives none.
pub async fn watch<M>(inbox: mpsc::Sender<M>, wrap: fn(Input) -> M, every: Duration, mut held: watch::Receiver<bool>, mut stop: watch::Receiver<bool>) {
    // a slice's start as the engine's clock reads it, its length as a
    // monotonic one does (a guest clock set back would shrink it)
    let mut last = read().map(|c| (c, crate::log::now_ms(), std::time::Instant::now()));
    crate::ev!("load.counters", { "read": last.is_some() });
    let mut every = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    // bounded by the bridge's life: one read per slice, ended by `stop`
    while let Some((was, at, since)) = last {
        tokio::select! {
            _ = every.tick() => {}
            _ = crate::net::stopped(&mut stop) => return,
        }
        // held now, or at any moment since the last look (a change)
        let holding = held.has_changed().unwrap_or(false) | *held.borrow_and_update();
        let Some(c) = read() else { continue };
        last = Some((c, crate::log::now_ms(), std::time::Instant::now()));
        let ms = since.elapsed().as_millis() as u64;
        let slice = Input::Slice { from: at, over: (!holding).then(|| over(was, c, ms)) };
        if inbox.send(wrap(slice)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = LOAD_SLICE_MS;

    /// Goal: a slice at either floor is over it, and one just under both
    /// is not. Invalid: a counter that went back is no work, and a slice
    /// of no time is read as a millisecond.
    #[test]
    fn a_slice_is_over_either_floor() {
        let zero = (0, 0);
        assert!(over(zero, (LOAD_CPU_FLOOR_PERMILLE * S, 0), S), "the CPU's floor");
        assert!(over(zero, (0, LOAD_NET_FLOOR_BYTES_PER_S * S / 1000), S), "the network's floor");
        assert!(!over(zero, (LOAD_CPU_FLOOR_PERMILLE * S - 1, LOAD_NET_FLOOR_BYTES_PER_S * S / 1000 - 1), S), "just under both");
        assert!(!over((5, 5), (4, 4), S), "went back");
        assert!(!over(zero, (1, 1), 0), "no time");
    }

    /// Goal (Codex on #1349): a save's own work says nothing of the
    /// computer's. Valid: this machine's counters, a thread spinning, give
    /// slices over the floor. Invalid: a slice the platform's hold
    /// overlapped says nothing, however busy, nor one the hold ended in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slice_under_the_hold_is_no_work() {
        let spin = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let spinning = spin.clone();
        // bounded by the test: it stops it
        let spinner = std::thread::spawn(move || {
            while spinning.load(std::sync::atomic::Ordering::Relaxed) {
                std::hint::black_box(0u64);
            }
        });
        let (tx, mut rx) = mpsc::channel(16);
        let (held_tx, held_rx) = watch::channel(false);
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(watch(tx, |i| i, Duration::from_millis(200), held_rx, stop_rx));
        let over = |i: Option<Input>| match i {
            Some(Input::Slice { over, .. }) => over,
            other => panic!("a slice: {other:?}"),
        };
        assert_eq!(over(rx.recv().await), Some(true), "a spinning core is over the floor");
        held_tx.send_replace(true);
        rx.recv().await;
        assert_eq!(over(rx.recv().await), None, "under the hold, it says nothing");
        held_tx.send_replace(false);
        assert_eq!(over(rx.recv().await), None, "nor in the slice the hold ended in");
        assert_eq!(over(rx.recv().await), Some(true), "and after it, work again");
        stop_tx.send_replace(true);
        spin.store(false, std::sync::atomic::Ordering::Relaxed);
        spinner.join().unwrap();
    }

    #[test]
    fn counters_parse() {
        assert_eq!(cgroup_cpu("usage_usec 631383441\nuser_usec 517364789\n"), Some(631_383_441));
        assert_eq!(cgroup_cpu("user_usec 1\n"), None);
        let dev = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo:  820992   13530    0    0    0     0          0         0   820992   13530    0    0    0     0       0          0\n  eth0: 7254812  120885    0    0    0     0          0         0  5787279   81328    0    0    0     0       0          0\n";
        assert_eq!(net_bytes(dev), Some(7_254_812 + 5_787_279), "loopback left out");
        assert_eq!(net_bytes("a\nb\n eth0: 1 2\n"), None, "a line short of its fields");
    }
}
