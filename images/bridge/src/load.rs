//! Work left running on the computer, outside any turn (docs/bridge.md,
//! "Work left running"): the computer's CPU time and network bytes, read
//! every `LOAD_SLICE_MS` from its own counters, whatever started the work
//! (a background command of any runtime's, `nohup … &`, a download). It is
//! working while a slice of the last `LOAD_SLICES` was over a floor: an
//! idle runtime, its desktop and an idle server stay under it. The engine
//! holds the computer for it only within `LEFT_RUNNING_HOLD_MS` of its
//! agents' last turn (`Input::Load`).

use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::engine::Input;
use crate::limits::{LOAD_CPU_FLOOR_PERMILLE, LOAD_NET_FLOOR_BYTES_PER_S, LOAD_SLICES, LOAD_SLICE_MS};

/// The computer's own counters, each counting up from its start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Counters {
    /// CPU time, every process's, in microseconds.
    pub cpu_usec: u64,
    /// Bytes received and sent on every interface but loopback.
    pub net_bytes: u64,
}

/// The last `LOAD_SLICES` slices, each over a floor or not.
#[derive(Debug, Default)]
pub struct Window {
    last: Option<(Counters, u64)>,
    slices: VecDeque<bool>,
}

impl Window {
    /// The counters read at `now_ms`: whether the computer is working, as
    /// the slice since the read before and the slices before it say. A
    /// counter that went back (a new container under the same bridge
    /// cannot, but a fallback's might) starts over from it.
    pub fn read(&mut self, c: Counters, now_ms: u64) -> bool {
        if let Some((was, at)) = self.last.replace((c, now_ms)) {
            if let (Some(cpu), Some(net), true) = (c.cpu_usec.checked_sub(was.cpu_usec), c.net_bytes.checked_sub(was.net_bytes), now_ms > at) {
                let ms = now_ms - at;
                // a microsecond of CPU per millisecond is a thousandth of a core
                let over = cpu / ms >= LOAD_CPU_FLOOR_PERMILLE || net.saturating_mul(1000) / ms >= LOAD_NET_FLOOR_BYTES_PER_S;
                if self.slices.len() == LOAD_SLICES {
                    self.slices.pop_front();
                }
                self.slices.push_back(over);
            }
        }
        assert!(self.slices.len() <= LOAD_SLICES, "a window keeps its slices");
        self.working()
    }

    pub fn working(&self) -> bool {
        self.slices.iter().any(|&s| s)
    }
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
        // receive: bytes …(8 fields); transmit: bytes …
        total = total.saturating_add(*f.first()?).saturating_add(*f.get(8)?);
        any = true;
    }
    any.then_some(total)
}

/// Where the counters are read from: the container's own cgroup (v2), else
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

/// Reads the counters every `LOAD_SLICE_MS` for the bridge's life, and tells
/// the engine each time the computer starts or stops working
/// (`Input::Load`, as `wrap` puts it in `inbox`). With no counters to
/// read, it says so once and holds nothing, as a bridge before it.
pub async fn watch<M>(inbox: mpsc::Sender<M>, wrap: fn(Input) -> M, mut stop: watch::Receiver<bool>) {
    let from = [CpuFrom::Cgroup, CpuFrom::Proc].into_iter().find(|f| read(*f).is_some());
    crate::ev!("load.counters", { "cpu": format!("{from:?}") });
    let Some(from) = from else { return };
    let mut window = Window::default();
    let mut said = false;
    let mut every = tokio::time::interval(Duration::from_millis(LOAD_SLICE_MS));
    // bounded by the bridge's life: one read per slice, ended by `stop`
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = crate::net::stopped(&mut stop) => return,
        }
        let Some(c) = read(from) else { continue };
        let working = window.read(c, crate::log::now_ms());
        if working != said {
            said = working;
            if inbox.send(wrap(Input::Load(working))).await.is_err() {
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

    /// Goal: the computer is working while a slice of the window was over
    /// either floor, and not once the window holds none. Invalid: a first
    /// read says nothing yet; a counter that went back, or a clock that did
    /// not move, makes no slice.
    #[test]
    fn working_is_a_slice_over_a_floor_in_the_window() {
        let mut w = Window::default();
        assert!(!w.read(at(0, 0), 0), "one read is no slice");
        // just under each floor, every slice
        let under_cpu = LOAD_CPU_FLOOR_PERMILLE * S - S;
        let under_net = LOAD_NET_FLOOR_BYTES_PER_S * S / 1000 - 1;
        let (mut cpu, mut net) = (0, 0);
        for i in 1..=LOAD_SLICES as u64 + 2 {
            cpu += under_cpu;
            net += under_net;
            assert!(!w.read(at(cpu, net), i * S), "under both floors");
        }
        let mut t = (LOAD_SLICES as u64 + 2) * S;
        // a slice at the CPU floor works, and the window holds it
        t += S;
        cpu += LOAD_CPU_FLOOR_PERMILLE * S;
        assert!(w.read(at(cpu, net), t));
        for _ in 1..LOAD_SLICES {
            t += S;
            assert!(w.read(at(cpu, net), t), "held by the window");
        }
        t += S;
        assert!(!w.read(at(cpu, net), t), "the window holds no slice over");
        // the network's floor alone
        t += S;
        net += LOAD_NET_FLOOR_BYTES_PER_S * S / 1000;
        assert!(w.read(at(cpu, net), t));
        // invalid: a counter that went back and a clock that stood still
        // make no slice, and the window keeps what it held
        assert!(w.read(at(0, 0), t + S));
        assert!(w.read(at(u64::MAX / 2, 0), t + S), "no time passed");
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
