//! A local run's disposable state, in RAM when `FRAGMENT_E2E_SCRATCH`
//! names a directory on a RAM-backed file system (a tmpfs; this box's slot
//! script names /dev/shm/fragment-e2e). Up to four runs share one SSD with
//! a person's desktop, whose apps stalled on the runs' writes: their nodes'
//! SQLite commits (2026-11-19: a desktop app's commits on the SSD took up
//! to seconds; on a tmpfs, under a millisecond).
//!
//! What goes there is what no one reads once a run has passed: its node's
//! project (the staged cell and its `.wrangler/state`, the Durable Objects'
//! SQLite, R2 and Workflows) and its Chromes' profiles. The run's scratch
//! on disk (`target/e2e/<run>`) keeps the rest: each node boot's log, the
//! lanes' directories and screenshots. A run that does not pass (a FAIL, an
//! error, a panic, a signal, its cap) has its state copied there before its
//! directory in RAM goes.
//!
//! Limits. A run takes RAM only while the box keeps `RESERVE` available
//! with every run in RAM at `PEAK`, and while the file system has room for
//! every run at `CAP`; otherwise its state goes in its scratch on disk, and
//! it says why. A run past `CAP` stops itself, as Ctrl-C stops it (`watch`).
//! Each run's directory is locked (flock) while its process lives, so the
//! next run's start sweeps the directories of runs that died outright
//! (SIGKILL, the OOM killer), their state copied to their scratch first.

use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};

/// The variable naming the directory runs keep their disposable state in.
pub const VAR: &str = "FRAGMENT_E2E_SCRATCH";

const GIB: u64 = 1 << 30;
/// A run's disposable state at its largest, with room: a whole run's
/// project held 0.95 GiB at its end (2026-11-19, without the trace store,
/// `devstack::NodeOptions::trace_store`) and its Chromes' profiles up to
/// 0.16 GiB.
pub const PEAK: u64 = 3 * GIB / 2;
/// The most one run may hold in RAM: past it, the run stops (`watch`).
pub const CAP: u64 = 4 * GIB;
/// RAM the box keeps available (`MemAvailable`) with every run in RAM at
/// its `PEAK`: the desktop's, the builds' and the computers' containers'.
pub const RESERVE: u64 = 12 * GIB;
/// How often a run measures its directory in RAM against `CAP`.
const WATCH_EVERY: Duration = Duration::from_secs(10);

const _: () = assert!(PEAK < CAP, "a run's peak is under its cap");

/// In a run's directory: the lock its process holds while it lives.
const LOCK: &str = ".lock";
/// In a run's directory: its scratch on disk, where its state is kept.
const SCRATCH: &str = ".scratch";
/// What a run's state kept on disk leaves out: the release's copy (the
/// cell's build) and wrangler's bundles, both made again by any run, and
/// the directory's own files.
const NOT_KEPT: [&str; 4] = ["cell/build", "cell/.wrangler/tmp", LOCK, SCRATCH];

/// The box's room for one more run in RAM.
#[derive(Debug, Clone, Copy)]
struct Room {
    /// `MemAvailable`.
    available: u64,
    /// Free bytes on the directory's file system.
    free: u64,
    /// The runs in RAM with this one.
    runs: u64,
}

impl Room {
    /// Whether one more run fits, or why it does not.
    fn admits(self) -> std::result::Result<(), String> {
        assert!(self.runs >= 1, "the runs in RAM count this one");
        let need = RESERVE + self.runs * PEAK;
        if self.available < need {
            return Err(format!(
                "{} of RAM available, under the {} that {} runs in RAM at {} each and {} kept for the box need",
                gib(self.available),
                gib(need),
                self.runs,
                gib(PEAK),
                gib(RESERVE)
            ));
        }
        if self.free < self.runs * CAP {
            return Err(format!("{} free on its file system, under {} runs at their cap of {}", gib(self.free), self.runs, gib(CAP)));
        }
        Ok(())
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// A run's directory in RAM, locked while the run lives. Ended once: by the
/// run's end, its drop, a termination signal, whichever comes first.
#[derive(Clone)]
pub struct Ram(Arc<Mutex<Option<Held>>>, PathBuf);

struct Held {
    dir: PathBuf,
    scratch: PathBuf,
    _lock: File,
}

/// Where the run `run` (its scratch on disk: `scratch`) keeps its
/// disposable state: `Some`, a directory in RAM under `FRAGMENT_E2E_SCRATCH`;
/// `None`, its scratch (no `FRAGMENT_E2E_SCRATCH`, or no room, said why).
pub fn place(run: &str, scratch: &Path) -> Result<Option<Ram>> {
    let Some(root) = std::env::var_os(VAR).filter(|v| !v.is_empty()).map(PathBuf::from) else { return Ok(None) };
    let room = |runs| Ok(Room { available: mem_available()?, free: fs_free(&root)?, runs });
    place_in(&root, run, scratch, room)
}

fn place_in(root: &Path, run: &str, scratch: &Path, room: impl FnOnce(u64) -> Result<Room>) -> Result<Option<Ram>> {
    ensure!(root.is_absolute(), "{VAR} names a directory absolutely, not {}", root.display());
    ensure!(crate::hosted::sweep::is_run(run), "{run:?} is a run's id");
    fs::create_dir_all(root).with_context(|| format!("make {VAR}={}", root.display()))?;
    sweep(root);
    let room = room(live(root) + 1)?;
    if let Err(why) = room.admits() {
        println!("scratch: on disk, {}: {why}", scratch.display());
        return Ok(None);
    }
    // made under a name no sweep takes, locked, then moved into place: a
    // sweep never finds it unlocked
    let making = root.join(format!(".{run}"));
    fs::create_dir(&making).with_context(|| format!("make {}", making.display()))?;
    fs::set_permissions(&making, fs::Permissions::from_mode(0o700))?;
    let lock = File::create(making.join(LOCK))?;
    lock.try_lock().with_context(|| format!("lock {}", making.display()))?;
    fs::write(making.join(SCRATCH), scratch.as_os_str().as_bytes())?;
    let dir = root.join(run);
    ensure!(!dir.exists(), "{}: another run's directory", dir.display());
    fs::rename(&making, &dir)?;
    println!("scratch: the node's state and Chrome's profiles in RAM, {} ({} runs there; {} of RAM available)", dir.display(), room.runs, gib(room.available));
    Ok(Some(Ram(Arc::new(Mutex::new(Some(Held { dir: dir.clone(), scratch: scratch.to_path_buf(), _lock: lock }))), dir)))
}

impl Ram {
    pub fn dir(&self) -> &Path {
        &self.1
    }

    /// Ends the run's directory in RAM, its state first copied to its
    /// scratch on disk when `keep`; after the first, a no-op. The node and
    /// the Chromes writing there are stopped first.
    pub fn end(&self, keep: bool) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(h) = held.take() else { return };
        if keep {
            match copy_kept(&h.dir, &h.scratch) {
                Ok(bytes) => println!("      (the node's state, {} MiB, copied from RAM to {})", bytes >> 20, h.scratch.display()),
                Err(e) => println!("      could not copy the node's state from {} to {}: {e:#}", h.dir.display(), h.scratch.display()),
            }
        }
        if let Err(e) = fs::remove_dir_all(&h.dir) {
            println!("      could not remove {}: {e}", h.dir.display());
        }
    }

    /// Measures the directory every `WATCH_EVERY` while it is there; past
    /// `CAP`, stops the run as a SIGTERM does (its node and Chromes killed,
    /// its state kept: `devstack::signals::on_termination`).
    pub fn watch(&self) -> Result<()> {
        let dir = self.1.clone();
        std::thread::Builder::new().name("scratch-cap".into()).spawn(move || loop {
            std::thread::sleep(WATCH_EVERY);
            if !dir.exists() {
                return;
            }
            let held = size(&dir);
            if held > CAP {
                println!("\nscratch: {} holds {}, past a run's cap of {}: the run stops", dir.display(), gib(held), gib(CAP));
                // SAFETY: kill(2) on this process's own id
                unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
                return;
            }
        })?;
        Ok(())
    }
}

/// `MemAvailable`, from /proc/meminfo.
fn mem_available() -> Result<u64> {
    let text = fs::read_to_string("/proc/meminfo")?;
    let kib = text.lines().find_map(|l| l.strip_prefix("MemAvailable:")).context("/proc/meminfo has MemAvailable")?;
    Ok(kib.trim().trim_end_matches("kB").trim().parse::<u64>().context("MemAvailable in kB")? * 1024)
}

/// The bytes an unprivileged writer may still use on `dir`'s file system.
fn fs_free(dir: &Path) -> Result<u64> {
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes())?;
    // SAFETY: a zeroed statvfs is a valid out-parameter; `path` is a C string
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        bail!("statvfs {}: {}", dir.display(), std::io::Error::last_os_error());
    }
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

/// The runs' directories under `root`: (path, its lock file), each named as
/// a run (or `.`run, one being made). Nothing else there is touched: the
/// root may be shared (/dev/shm itself).
fn runs(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let Ok(entries) = fs::read_dir(root) else { return vec![] };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            crate::hosted::sweep::is_run(name.strip_prefix('.').unwrap_or(&name)) && e.file_type().is_ok_and(|t| t.is_dir())
        })
        .map(|e| (e.path(), e.path().join(LOCK)))
        .filter(|(_, lock)| lock.is_file())
        .collect()
}

/// The runs in RAM now: their locks held.
fn live(root: &Path) -> u64 {
    runs(root).iter().filter(|(_, lock)| File::open(lock).is_ok_and(|f| f.try_lock().is_err())).count() as u64
}

/// Removes the directories of runs whose process is gone (their lock free),
/// each one's state copied to its scratch on disk first, when that is there.
fn sweep(root: &Path) {
    for (dir, lock) in runs(root) {
        let Ok(file) = File::open(&lock) else { continue };
        if file.try_lock().is_err() {
            continue;
        }
        let scratch = fs::read(dir.join(SCRATCH)).ok().map(|b| PathBuf::from(std::ffi::OsStr::from_bytes(&b)));
        let kept = match scratch.filter(|s| s.is_dir()) {
            Some(s) => match copy_kept(&dir, &s) {
                Ok(bytes) => format!("its state ({} MiB) copied to {}", bytes >> 20, s.display()),
                Err(e) => format!("its state could not be copied to {}: {e:#}", s.display()),
            },
            None => "its scratch on disk is gone".into(),
        };
        match fs::remove_dir_all(&dir) {
            Ok(()) => println!("scratch: swept {}, a run's that is gone; {kept}", dir.display()),
            Err(e) => println!("scratch: could not sweep {}: {e}", dir.display()),
        }
    }
}

/// Copies what a run's directory keeps (all but `NOT_KEPT`) into `to`,
/// beside what is there; the bytes copied. Symlinks are copied as links
/// (Chrome's Singleton*), and sockets and the like left out.
fn copy_kept(from: &Path, to: &Path) -> Result<u64> {
    fn walk(root: &Path, from: &Path, to: &Path, bytes: &mut u64) -> Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let path = entry.path();
            if NOT_KEPT.iter().any(|n| path.strip_prefix(root).is_ok_and(|rel| rel == Path::new(n))) {
                continue;
            }
            let target = to.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(root, &path, &target, bytes)?;
            } else if kind.is_file() {
                *bytes += fs::copy(&path, &target).with_context(|| format!("copy {}", path.display()))?;
            } else if kind.is_symlink() {
                let _ = std::os::unix::fs::symlink(fs::read_link(&path)?, &target);
            }
        }
        Ok(())
    }
    let mut bytes = 0;
    walk(from, from, to, &mut bytes)?;
    Ok(bytes)
}

/// The bytes the files under `dir` take.
fn size(dir: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => size(&e.path()),
            _ => e.metadata().map_or(0, |m| m.blocks() * 512),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "0123456789";

    /// A directory of its own under the test's temp dir, removed on drop.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Tmp {
            let d = std::env::temp_dir().join(format!("e2e-scratch-test-{}", fragment_devstack::random_hex(6)));
            fs::create_dir_all(&d).unwrap();
            Tmp(d)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn roomy(runs: u64) -> Result<Room> {
        Ok(Room { available: 64 * GIB, free: 64 * GIB, runs })
    }

    /// A run's directory as a node and a Chrome leave it.
    fn fill(dir: &Path) {
        for (path, body) in [("cell/.wrangler/state/v3/do/x.sqlite", "rows"), ("cell/build/worker.wasm", "built"), ("cell/.wrangler/tmp/bundle.js", "bundled"), ("chrome-ab12/chrome.log", "said")] {
            fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            fs::write(dir.join(path), body).unwrap();
        }
        std::os::unix::fs::symlink("host-123", dir.join("chrome-ab12/SingletonLock")).unwrap();
    }

    #[test]
    fn a_run_fits_while_the_box_keeps_its_reserve_and_its_file_system_has_room() {
        let need = RESERVE + 4 * PEAK;
        assert!(Room { available: need, free: 4 * CAP, runs: 4 }.admits().is_ok());
        let short = Room { available: need - 1, free: 4 * CAP, runs: 4 }.admits().unwrap_err();
        assert!(short.contains("4 runs in RAM"), "{short}");
        let full = Room { available: need, free: 4 * CAP - 1, runs: 4 }.admits().unwrap_err();
        assert!(full.contains("free on its file system"), "{full}");
    }

    #[test]
    fn a_run_without_room_keeps_its_state_on_disk_and_makes_nothing_in_ram() {
        let tmp = Tmp::new();
        let root = tmp.0.join("ram");
        let placed = place_in(&root, RUN, &tmp.0.join("disk"), |runs| Ok(Room { available: RESERVE, free: 64 * GIB, runs })).unwrap();
        assert!(placed.is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[test]
    fn a_passing_run_leaves_nothing_and_a_failing_one_keeps_its_state_on_disk() {
        let tmp = Tmp::new();
        let (root, disk) = (tmp.0.join("ram"), tmp.0.join("disk"));
        fs::create_dir_all(&disk).unwrap();
        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        assert_eq!(ram.dir(), root.join(RUN));
        assert_eq!(fs::metadata(ram.dir()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(live(&root), 1);
        fill(ram.dir());
        ram.end(false);
        assert!(!root.join(RUN).exists() && fs::read_dir(&disk).unwrap().count() == 0);

        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        fill(ram.dir());
        ram.end(true);
        // replayed (a signal after the run's own end): nothing more
        ram.end(true);
        assert!(!root.join(RUN).exists());
        assert_eq!(fs::read_to_string(disk.join("cell/.wrangler/state/v3/do/x.sqlite")).unwrap(), "rows");
        assert_eq!(fs::read_to_string(disk.join("chrome-ab12/chrome.log")).unwrap(), "said");
        assert_eq!(fs::read_link(disk.join("chrome-ab12/SingletonLock")).unwrap(), Path::new("host-123"));
        for gone in ["cell/build", "cell/.wrangler/tmp", LOCK, SCRATCH] {
            assert!(!disk.join(gone).exists(), "{gone} is not kept");
        }
    }

    #[test]
    fn the_next_start_sweeps_a_dead_runs_directory_its_state_kept_and_spares_a_live_one_and_all_else() {
        let tmp = Tmp::new();
        let root = tmp.0.join("ram");
        let (dead_disk, live_disk) = (tmp.0.join("dead"), tmp.0.join("live"));
        fs::create_dir_all(&dead_disk).unwrap();
        // a run killed outright: its lock released with its process, its
        // directory left (a clone kept past the drop stands in for nothing)
        let dead = place_in(&root, "aaaaaaaaaa", &dead_disk, roomy).unwrap().expect("room");
        fill(dead.dir());
        let held = dead.0.lock().unwrap().take().expect("held");
        drop(held);
        let alive = place_in(&root, "bbbbbbbbbb", &live_disk, roomy).unwrap().expect("room");
        // what is not a run's: never touched
        fs::create_dir_all(root.join("not-a-run")).unwrap();
        fs::create_dir_all(root.join("cccccccccc")).unwrap();
        assert_eq!(live(&root), 1);

        let next = place_in(&root, RUN, &tmp.0.join("next"), |runs| {
            assert_eq!(runs, 2, "the live run and this one");
            roomy(runs)
        })
        .unwrap()
        .expect("room");
        assert!(!root.join("aaaaaaaaaa").exists(), "the dead run's directory is swept");
        assert_eq!(fs::read_to_string(dead_disk.join("cell/.wrangler/state/v3/do/x.sqlite")).unwrap(), "rows");
        assert!(alive.dir().is_dir() && root.join("not-a-run").is_dir() && root.join("cccccccccc").is_dir());
        alive.end(false);
        next.end(false);
    }

    #[test]
    fn a_directory_named_relatively_or_a_bad_run_id_is_refused() {
        assert!(place_in(Path::new("relative/ram"), RUN, Path::new("/disk"), roomy).is_err());
        let tmp = Tmp::new();
        assert!(place_in(&tmp.0, "../escape", &tmp.0, roomy).is_err());
    }

    #[test]
    fn the_box_reads_its_available_ram_and_free_space() {
        assert!(mem_available().unwrap() > 0);
        assert!(fs_free(&std::env::temp_dir()).unwrap() > 0);
        let tmp = Tmp::new();
        fill(&tmp.0);
        assert!(size(&tmp.0) > 0);
    }
}
