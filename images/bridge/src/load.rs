//! Work left running on the computer, outside any turn (docs/bridge.md,
//! "Work left running"): the computer's CPU time and network bytes, read
//! every `LOAD_SLICE_MS` from its own counters, whatever started the work
//! (a background command of any runtime's, `nohup … &`, a download). Each
//! slice goes to the engine, over a floor or not (`Input::Slice`): an
//! idle runtime, its desktop and an idle server stay under it.

use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::engine::Input;
use crate::limits::{LOAD_CPU_FLOOR_PERMILLE, LOAD_NET_FLOOR_BYTES_PER_S, LOAD_SLICE_MS};

/// The computer's own counters, each counting up from its start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Counters {
    /// CPU time, every process's, in microseconds.
    pub cpu_usec: u64,
    /// Bytes received and sent on every interface but loopback.
    pub net_bytes: u64,
}

/// Whether the slice from `was` to `now`, `ms` long, was over either
/// floor; none when a counter went back (a fallback's might) or no time
/// passed.
pub fn over(was: Counters, now: Counters, ms: u64) -> Option<bool> {
    let cpu = now.cpu_usec.checked_sub(was.cpu_usec)?;
    let net = now.net_bytes.checked_sub(was.net_bytes)?;
    if ms == 0 {
        return None;
    }
    // a microsecond of CPU per millisecond is a thousandth of a core
    Some(cpu / ms >= LOAD_CPU_FLOOR_PERMILLE || net.saturating_mul(1000) / ms >= LOAD_NET_FLOOR_BYTES_PER_S)
}

/// `usage_usec` of a cgroup v2 `cpu.stat`.
pub fn cgroup_cpu(stat: &str) -> Option<u64> {
    stat.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// The busy time of `/proc/stat`'s `cpu` line (all but idle and iowait),
/// in microseconds, at `hz` ticks a second: the machine's, which in a
/// computer's own VM is the computer's.
pub fn proc_cpu(stat: &str, hz: u64) -> Option<u64> {
    assert!(hz > 0, "the clock ticks");
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line.split_whitespace().skip(1).map(|v| v.parse().ok()).collect::<Option<_>>()?;
    // user nice system idle iowait irq softirq steal …
    if f.len() < 8 {
        return None;
    }
    let busy = f[0] + f[1] + f[2] + f[5] + f[6] + f[7];
    Some(busy.saturating_mul(1_000_000) / hz)
}

/// Bytes received and sent on every interface of `/proc/net/dev` but
/// loopback (a server the agent talks to on the computer is its own).
pub fn net_bytes(dev: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut any = false;
    for line in dev.lines().skip(2) {
        let (name, rest) = line.split_once(':')?;
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<u64> = rest.split_whitespace().map(|v| v.parse().ok()).collect::<Option<_>>()?;
        // receive: bytes and 7 more; transmit: bytes …
        total = total.saturating_add(*f.first()?).saturating_add(*f.get(8)?);
        any = true;
    }
    any.then_some(total)
}

/// Where the CPU time is read from: the container's own cgroup (v2), else
/// the machine's `/proc/stat`. Docker shows a container its own cgroup and
/// the host's `/proc/stat`, so the cgroup goes first.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CpuFrom {
    Cgroup,
    Proc,
}

fn read(from: CpuFrom) -> Option<Counters> {
    let cpu_usec = match from {
        CpuFrom::Cgroup => cgroup_cpu(&std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?)?,
        // Linux reports /proc/stat in USER_HZ, 100 on every architecture we run
        CpuFrom::Proc => proc_cpu(&std::fs::read_to_string("/proc/stat").ok()?, 100)?,
    };
    let net_bytes = net_bytes(&std::fs::read_to_string("/proc/net/dev").ok()?)?;
    Some(Counters { cpu_usec, net_bytes })
}

/// Reads the counters every `LOAD_SLICE_MS` for the bridge's life, and
/// gives the engine each slice (`Input::Slice`, as `wrap` puts it in
/// `inbox`). With no counters to read, it says so once and gives none, as
/// a bridge before it.
pub async fn watch<M>(inbox: mpsc::Sender<M>, wrap: fn(Input) -> M, mut stop: watch::Receiver<bool>) {
    let from = [CpuFrom::Cgroup, CpuFrom::Proc].into_iter().find(|f| read(*f).is_some());
    crate::ev!("load.counters", { "cpu": from.map(|f| format!("{f:?}")) });
    let Some(from) = from else { return };
    let mut last: Option<(Counters, u64)> = None;
    let mut every = tokio::time::interval(Duration::from_millis(LOAD_SLICE_MS));
    // bounded by the bridge's life: one read per slice, ended by `stop`
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = crate::net::stopped(&mut stop) => return,
        }
        let Some(c) = read(from) else { continue };
        let now = crate::log::now_ms();
        if let Some((was, at)) = last.replace((c, now)) {
            let Some(over) = over(was, c, now.saturating_sub(at)) else { continue };
            if inbox.send(wrap(Input::Slice { from: at, over })).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = LOAD_SLICE_MS;

    fn at(cpu_usec: u64, net_bytes: u64) -> Counters {
        Counters { cpu_usec, net_bytes }
    }

    /// Goal: a slice at either floor is over it, and one just under both
    /// is not. Invalid: a counter that went back, or no time passed, is no
    /// slice.
    #[test]
    fn a_slice_is_over_either_floor() {
        let zero = at(0, 0);
        assert_eq!(over(zero, at(LOAD_CPU_FLOOR_PERMILLE * S, 0), S), Some(true), "the CPU's floor");
        assert_eq!(over(zero, at(0, LOAD_NET_FLOOR_BYTES_PER_S * S / 1000), S), Some(true), "the network's floor");
        assert_eq!(over(zero, at(LOAD_CPU_FLOOR_PERMILLE * S - 1, LOAD_NET_FLOOR_BYTES_PER_S * S / 1000 - 1), S), Some(false), "just under both");
        assert_eq!(over(at(5, 0), at(4, 0), S), None, "went back");
        assert_eq!(over(at(0, 5), at(0, 4), S), None, "went back");
        assert_eq!(over(zero, at(1, 1), 0), None, "no time passed");
    }

    #[test]
    fn counters_parse() {
        assert_eq!(cgroup_cpu("usage_usec 631383441\nuser_usec 517364789\n"), Some(631_383_441));
        assert_eq!(cgroup_cpu("user_usec 1\n"), None);
        // user nice system idle iowait irq softirq steal guest guest_nice
        assert_eq!(proc_cpu("cpu  100 2 30 9999 50 4 5 6 0 0\ncpu0 1 1 1 1 1 1 1 1 0 0\n", 100), Some(147 * 10_000));
        assert_eq!(proc_cpu("cpu  1 2\n", 100), None);
        let dev = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo:  820992   13530    0    0    0     0          0         0   820992   13530    0    0    0     0       0          0\n  eth0: 7254812  120885    0    0    0     0          0         0  5787279   81328    0    0    0     0       0          0\n";
        assert_eq!(net_bytes(dev), Some(7_254_812 + 5_787_279), "loopback left out");
        assert_eq!(net_bytes("a\nb\n"), None, "no interface");
    }
}
