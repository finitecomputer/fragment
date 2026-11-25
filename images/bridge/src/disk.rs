//! A computer's disk (docs/computers.md, "A disk"): `/data` mounted from
//! FPAC when its start says so (`FRAGMENT_DISK=fpac`), by whichever boot
//! owns the container's life: the stub's bridge as its PID 1
//! (`BRIDGE_MOUNTS_DISK`), the Hermes boot's pre-init. The daemon (`fpac`,
//! each image's Dockerfile pins its release by its SHA-256) runs from the
//! mount to the stop, and makes its final flush on SIGTERM, which the boot
//! waits for before it exits (`flush`). Its state, its log and status, is
//! on a tmpfs of its own at `/run/fpac`, never on the disk its cache fills
//! (FPAC's docs/release.md).
//!
//! What the mount did, the boot says with its first read of its view
//! (`query`): the start line's numbers (cell `computer/guest`).

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// What the disk is mounted on.
pub const DATA: &str = "/data";
/// Its disk's route (the platform's, by FPAC's contract and its own
/// `POST /unusable`): what the daemon reaches, and where a boot says its
/// head will not open (`Api::disk_unusable`).
pub const ROUTE: &str = "http://fpac.fragment.internal";
/// The daemon, as the images put it.
pub const FPAC: &str = "/usr/local/bin/fpac";
/// Its state (the daemon's default `--state`): a tmpfs of `STATE_BYTES`.
pub const STATE: &str = "/run/fpac";
const STATE_BYTES: u64 = 16 << 20;
/// Its log (its stderr: one JSON event a line), its pid, and what its
/// mount did, as the boot that started it saw.
const LOG: &str = "/run/fpac/log";
const PID: &str = "/run/fpac/daemon.pid";
const MOUNTED: &str = "/run/fpac/mounted.json";
/// What a read of its log takes at most: a mount says a few lines.
const LOG_BYTES_MAX: usize = 1 << 20;
/// A mount is waited for this long: inside the cell's wait for the guest's
/// first read (`DISK_ALIVE_MS`, 120 s), which its boot makes after.
pub const MOUNT_MS_MAX: u64 = 100_000;
/// Its final flush is waited for this long: inside the cell's wait for the
/// container's exit (`DISK_EXIT_WAIT_MS`, 150 s), less the boot's own stop.
pub const FLUSH_MS_MAX: u64 = 140_000;
/// A boot that told the platform its head is unusable waits this long to
/// be destroyed by that start, rather than exit: an exit heard first would
/// be a failed start of its own, a strike, and the same head again.
pub const TOLD_WAIT_MS: u64 = 30_000;
/// How an open that found the disk's head unusable begins its error (FPAC
/// v0.1.0, `open_store`): a head that will never mount, told from a remote
/// that did not answer.
const UNUSABLE: &str = "the disk does not open";

/// Its start says it is on a disk.
pub fn wanted() -> bool {
    std::env::var("FRAGMENT_DISK").is_ok_and(|v| v == "fpac")
}

/// Its daemon, started, its mount not yet seen.
pub struct Mounting {
    child: Child,
    t0: Instant,
}

/// What its mount did: how long the boot waited for it (from the daemon's
/// start), and the extents a read fetched meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mounted {
    pub ms: u64,
    pub misses: u64,
}

/// Why it did not mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmounted {
    /// Its head will not open (FPAC's words): its start falls back past it
    /// (`Api::disk_unusable`).
    Unusable(String),
    /// Anything else: the remote, the device, the time.
    Failed(String),
}

/// Starts its daemon on `DATA`, its state on a fresh tmpfs. Nothing runs
/// yet that reads `/data`.
pub fn start() -> Result<Mounting, String> {
    std::fs::create_dir_all(STATE).map_err(|e| format!("{STATE}: {e}"))?;
    tmpfs(STATE)?;
    let log = std::fs::File::create(LOG).map_err(|e| format!("{LOG}: {e}"))?;
    let t0 = Instant::now();
    let child = Command::new(FPAC).args(["mount", "--dir", DATA]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(log).spawn().map_err(|e| format!("{FPAC}: {e}"))?;
    std::fs::write(PID, child.id().to_string()).map_err(|e| format!("{PID}: {e}"))?;
    Ok(Mounting { child, t0 })
}

impl Mounting {
    /// The daemon's pid.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Waits, at most `MOUNT_MS_MAX`, for its mount, and keeps what it did
    /// for its first read (`query`). The daemon goes on as the child of
    /// whatever its starter becomes (an exec keeps the pid); one past its
    /// bound is killed.
    pub fn wait(mut self) -> Result<Mounted, Unmounted> {
        // bounded by MOUNT_MS_MAX
        loop {
            let events = log_events();
            if let Some(m) = events.iter().find(|e| e["fpac"] == "mounted") {
                let mounted = Mounted { ms: self.t0.elapsed().as_millis() as u64, misses: m["misses"].as_u64().unwrap_or(0) };
                let _ = std::fs::write(MOUNTED, json!({ "mountMs": mounted.ms, "misses": mounted.misses }).to_string());
                return Ok(mounted);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(unmounted(&log_events(), &status.to_string()));
            }
            if self.t0.elapsed() > Duration::from_millis(MOUNT_MS_MAX) {
                let _ = self.child.kill();
                return Err(Unmounted::Failed(format!("no mount within {} s", MOUNT_MS_MAX / 1000)));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Why a daemon that exited (`status`) did not mount, from its log's
/// `events`: its error, if it said one. A disk it opened as not new with
/// nothing mapped or replayed is a blank device (a new disk's first life
/// ended before its filesystem shipped: its head a claim alone), which no
/// retry mounts: unusable, so the platform starts it as a new disk again.
fn unmounted(events: &[Value], status: &str) -> Unmounted {
    let why = events.iter().rev().find(|e| e["fpac"] == "error").and_then(|e| e["why"].as_str().map(str::to_string));
    let blank = events.iter().rev().find(|e| e["fpac"] == "opened").is_some_and(|o| o["new"] == false && o["extents_mapped"] == 0 && o["replayed"] == 0);
    match why {
        Some(why) if why.starts_with(UNUSABLE) => Unmounted::Unusable(why),
        Some(why) if blank => Unmounted::Unusable(format!("a blank disk (nothing of it shipped): {why}")),
        Some(why) => Unmounted::Failed(why),
        None => Unmounted::Failed(format!("fpac exited ({status}) saying nothing")),
    }
}

/// Ends its life's disk: SIGTERM to its daemon, which unmounts (or
/// freezes) `/data`, ships what is left and posts a clean head, then waits
/// for it to be gone, at most `FLUSH_MS_MAX`. Answers whether it ended
/// within that, and how long it took; a computer on no disk, or one whose
/// daemon is gone already, `None`. Its own child or not: a daemon that
/// exited is gone once it is a zombie.
pub fn flush() -> Option<(bool, u64)> {
    let pid: libc::pid_t = std::fs::read_to_string(PID).ok()?.trim().parse().ok()?;
    if !alive(pid) {
        return None;
    }
    let t0 = Instant::now();
    // SAFETY: a signal to a pid this boot started.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    // bounded by FLUSH_MS_MAX
    while alive(pid) && t0.elapsed() < Duration::from_millis(FLUSH_MS_MAX) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let flushed = events_of(LOG).iter().any(|e| e["fpac"] == "flushed" && e["error"].is_null());
    Some((!alive(pid) && flushed, t0.elapsed().as_millis() as u64))
}

fn alive(pid: libc::pid_t) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit(')').next().and_then(|rest| rest.split_whitespace().next()) != Some("Z"))
}

static SAID: AtomicBool = AtomicBool::new(false);

/// What its mount did, for its view's read to say (`?diskMountMs=…`):
/// empty on no disk, and once a read said it (`said`). Misses and the
/// fetches made again are the daemon's as it says them now.
pub fn query() -> String {
    if SAID.load(Ordering::Relaxed) {
        return String::new();
    }
    let Some(mounted) = std::fs::read_to_string(MOUNTED).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return String::new() };
    let status = std::fs::read_to_string(Path::new(STATE).join("status.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    numbers_query(&mounted, &status)
}

/// A read said what `query` gave it.
pub fn said() {
    SAID.store(true, Ordering::Relaxed);
}

fn numbers_query(mounted: &Value, status: &Value) -> String {
    let mut q = String::new();
    let misses = status["misses"].as_u64().or(mounted["misses"].as_u64());
    for (key, value) in [("diskMountMs", mounted["mountMs"].as_u64()), ("diskMisses", misses), ("diskRefetches", status["fetch_retries"].as_u64())] {
        if let Some(v) = value {
            q.push_str(&format!("{}{key}={v}", if q.is_empty() { "" } else { "&" }));
        }
    }
    q
}

/// Its log's events, each a JSON line, the first `LOG_BYTES_MAX` of it.
fn log_events() -> Vec<Value> {
    events_of(LOG)
}

fn events_of(path: &str) -> Vec<Value> {
    let Ok(f) = std::fs::File::open(path) else { return vec![] };
    let mut read = 0usize;
    let mut events = vec![];
    // bounded by LOG_BYTES_MAX
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        read += line.len() + 1;
        if read > LOG_BYTES_MAX {
            break;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            events.push(v);
        }
    }
    events
}

/// A tmpfs of `STATE_BYTES` at `dir`, root's alone.
fn tmpfs(dir: &str) -> Result<(), String> {
    let c = |s: &str| std::ffi::CString::new(s).expect("no NUL in a mount's names");
    let (source, target, fstype, data) = (c("tmpfs"), c(dir), c("tmpfs"), c(&format!("size={STATE_BYTES},mode=0700")));
    // SAFETY: mount with valid C strings.
    let r = unsafe { libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, data.as_ptr().cast()) };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("a tmpfs at {dir}: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_says_what_the_mount_did_and_what_the_daemon_counts_now() {
        let mounted = json!({ "mountMs": 412, "misses": 3 });
        assert_eq!(numbers_query(&mounted, &json!({ "misses": 9, "fetch_retries": 1, "unshipped_extents": 0 })), "diskMountMs=412&diskMisses=9&diskRefetches=1");
        assert_eq!(numbers_query(&mounted, &Value::Null), "diskMountMs=412&diskMisses=3", "no status yet: the mount's own");
        assert_eq!(numbers_query(&Value::Null, &Value::Null), "", "nothing known, nothing said");
    }

    #[test]
    fn an_unusable_head_is_told_from_every_other_failure_by_fpacs_words() {
        // FPAC v0.1.0's two errors at an open (daemon.rs, `open_store`)
        assert!("the disk does not open: a head that names a checkpoint not found".starts_with(UNUSABLE));
        assert!(!"the remote is unavailable: connection refused".starts_with(UNUSABLE));
    }

    #[test]
    fn a_disk_whose_first_life_ended_before_its_filesystem_shipped_is_unusable() {
        let error = |why: &str| json!({ "fpac": "error", "why": why });
        let opened = |new: bool, mapped: u64, replayed: u64| json!({ "fpac": "opened", "new": new, "extents_mapped": mapped, "replayed": replayed });
        let mount = "mount ext4 at /data: Invalid argument (os error 22)";
        // its head a claim and nothing replayed: a blank device, which no
        // retry mounts; the platform starts it again as a new disk
        assert!(matches!(unmounted(&[opened(false, 0, 0), error(mount)], "exit status: 1"), Unmounted::Unusable(why) if why.contains("blank")));
        // anything mapped or replayed: its files are there, and a mount that
        // fails may pass (a failed start, retried)
        assert_eq!(unmounted(&[opened(false, 12, 0), error(mount)], "exit status: 1"), Unmounted::Failed(mount.into()));
        assert_eq!(unmounted(&[opened(false, 0, 3), error(mount)], "exit status: 1"), Unmounted::Failed(mount.into()));
        // a new disk it made itself, failing: a failed start too
        assert_eq!(unmounted(&[opened(true, 0, 0), error(mount)], "exit status: 1"), Unmounted::Failed(mount.into()));
        // an open that refused its head, as before; and one that said nothing
        assert!(matches!(unmounted(&[error("the disk does not open: no such checkpoint")], "exit status: 1"), Unmounted::Unusable(_)));
        assert!(matches!(unmounted(&[], "exit status: 9"), Unmounted::Failed(why) if why.contains("saying nothing")));
    }

    #[test]
    fn a_zombie_is_gone() {
        let mut child = Command::new("true").spawn().expect("true runs");
        let pid = child.id() as libc::pid_t;
        // bounded: `true` exits at once
        let t0 = Instant::now();
        while alive(pid) && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!alive(pid), "exited and unreaped, it is gone");
        let _ = child.wait();
        assert!(!alive(pid));
        assert!(alive(std::process::id() as libc::pid_t));
    }
}
