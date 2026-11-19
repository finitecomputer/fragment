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
//! directory in RAM goes; a copy that fails leaves it in RAM, and says so.
//!
//! Limits. A run takes RAM only while the box keeps `RESERVE` available
//! with every run in RAM at `PEAK`, and while the file system has room for
//! every run at `CAP`; otherwise its state goes in its scratch on disk, and
//! it says why. A run past `CAP` stops itself, as SIGTERM stops it
//! (`watch`). The directory is this user's alone (0700), each run's
//! directory in it locked (flock) while its process lives, and runs take
//! their places one at a time (`PLACING`), so the next run's start sweeps
//! the directories of runs that died outright (SIGKILL, the OOM killer):
//! what they left running killed, their containers removed, their state
//! copied to their scratch first.

use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use fragment_devstack as devstack;

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
/// How long a sweep waits for a dead run's processes to go once killed.
const LEFTOVERS_WAIT: Duration = Duration::from_secs(5);

const _: () = assert!(PEAK < CAP, "a run's peak is under its cap");

/// In the directory: the lock a run holds while it takes its place, so
/// that sweeping, counting the runs, and making its own are one step.
const PLACING: &str = ".placing";
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

/// A run's directory in RAM, locked while the run lives. Ended once: by
/// `end`, by the termination signal's work (`ending`), or as it drops (its
/// state kept: an error or a panic before the run's own end).
pub struct Ram {
    held: Arc<Mutex<Option<Held>>>,
    dir: PathBuf,
}

/// A run's directory as the termination signal's work holds it: never
/// keeping it from ending as the run's own `Ram` drops.
pub struct Ending(Weak<Mutex<Option<Held>>>);

struct Held {
    dir: PathBuf,
    scratch: PathBuf,
    /// Whether its state is copied to its scratch before it goes.
    keep: bool,
    _lock: File,
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.keep {
            match copy_kept(&self.dir, &self.scratch) {
                Ok(bytes) => println!("      (the node's state, {} MiB, copied from RAM to {})", bytes >> 20, self.scratch.display()),
                Err(e) => {
                    println!("      could not copy the node's state to {}: {e:#}; it stays in RAM, at {}", self.scratch.display(), self.dir.display());
                    return;
                }
            }
        }
        if let Err(e) = fs::remove_dir_all(&self.dir) {
            println!("      could not remove {}: {e}", self.dir.display());
        }
    }
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
    own_dir(root)?;
    let placing = File::create(root.join(PLACING))?;
    placing.lock().with_context(|| format!("lock {}", root.join(PLACING).display()))?;
    sweep(root);
    let room = room(live(root) + 1)?;
    if let Err(why) = room.admits() {
        println!("scratch: on disk, {}: {why}", scratch.display());
        return Ok(None);
    }
    let dir = root.join(run);
    fs::create_dir(&dir).with_context(|| format!("make {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let lock = File::create(dir.join(LOCK))?;
    lock.try_lock().with_context(|| format!("lock {}", dir.display()))?;
    let held = Held { dir: dir.clone(), scratch: scratch.to_path_buf(), keep: true, _lock: lock };
    fs::write(dir.join(SCRATCH), scratch.as_os_str().as_bytes())?;
    drop(placing);
    println!("scratch: the node's state and Chrome's profiles in RAM, {} ({} runs there; {} of RAM available)", dir.display(), room.runs, gib(room.available));
    Ok(Some(Ram { held: Arc::new(Mutex::new(Some(held))), dir }))
}

impl Ram {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Ends the run's directory in RAM, its state first copied to its
    /// scratch on disk when `keep`; after the first end, a no-op. Its
    /// writers (the node, the Chromes) are stopped first.
    pub fn end(&self, keep: bool) {
        end(&self.held, keep);
    }

    /// The directory, for the termination signal's work to end.
    pub fn ending(&self) -> Ending {
        Ending(Arc::downgrade(&self.held))
    }

    /// Measures the directory every `WATCH_EVERY` while it is there; past
    /// `CAP`, stops the run as a SIGTERM does (its node and Chromes killed,
    /// its state kept: `devstack::signals::on_termination`), unless a
    /// termination signal is already being handled: a second would end the
    /// process halfway through keeping its state.
    pub fn watch(&self) -> Result<()> {
        let dir = self.dir.clone();
        std::thread::Builder::new().name("scratch-cap".into()).spawn(move || loop {
            std::thread::sleep(WATCH_EVERY);
            if !dir.exists() || devstack::signals::received() {
                return;
            }
            let held = size(&dir);
            if held > CAP && !devstack::signals::received() {
                println!("\nscratch: {} holds {}, past a run's cap of {}: the run stops", dir.display(), gib(held), gib(CAP));
                // SAFETY: kill(2) on this process's own id
                unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
                return;
            }
        })?;
        Ok(())
    }
}

impl Ending {
    /// As `Ram::end`, if the run's `Ram` has not ended it already.
    pub fn end(&self, keep: bool) {
        if let Some(held) = self.0.upgrade() {
            end(&held, keep);
        }
    }
}

fn end(held: &Mutex<Option<Held>>, keep: bool) {
    let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(mut h) = held.take() {
        h.keep = keep;
    }
}

/// `root`, made if need be, as this user's alone (0700): what is in it is
/// this user's runs', which a sweep kills, copies and removes.
fn own_dir(root: &Path) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("make {VAR}={}", root.display()))?;
    let meta = fs::symlink_metadata(root)?;
    // SAFETY: geteuid(2) has no failure
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me {
        bail!("{VAR}={} is not a directory of this user's own: name one it makes (/dev/shm/fragment-e2e), not a shared one", root.display());
    }
    if meta.mode() & 0o777 != 0o700 {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
    #[allow(clippy::unnecessary_cast)]
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

/// The runs' directories under `root`: those named as a run.
fn runs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else { return vec![] };
    entries
        .flatten()
        .filter(|e| crate::hosted::sweep::is_run(&e.file_name().to_string_lossy()) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// The runs in RAM now: their locks held.
fn live(root: &Path) -> u64 {
    runs(root).iter().filter(|dir| File::open(dir.join(LOCK)).is_ok_and(|f| f.try_lock().is_err())).count() as u64
}

/// Removes the directories of runs whose process is gone (their lock free,
/// or never made: a start that died taking its place), each one's leftover
/// processes killed and containers removed first, and its state copied to
/// its scratch on disk when that is there (a copy that fails leaves it).
/// Called with `PLACING` held, so no run is taking its place meanwhile.
fn sweep(root: &Path) {
    for dir in runs(root) {
        let lock = File::open(dir.join(LOCK));
        if lock.as_ref().is_ok_and(|f| f.try_lock().is_err()) {
            continue;
        }
        let killed = kill_leftovers(&dir);
        let project = dir.join("cell");
        let containers = match project.join("wrangler.jsonc").is_file() {
            true => devstack::containers::remove(&project).map(|r| r.containers).unwrap_or_else(|e| {
                println!("scratch: could not remove the containers of {}: {e:#}", project.display());
                0
            }),
            false => 0,
        };
        let kept = match scratch_of(&dir) {
            Some(s) => match copy_kept(&dir, &s) {
                Ok(bytes) => format!("its state ({} MiB) copied to {}", bytes >> 20, s.display()),
                Err(e) => {
                    println!("scratch: could not copy {}'s state to {}: {e:#}; it stays", dir.display(), s.display());
                    continue;
                }
            },
            None => "no scratch on disk to keep its state in".into(),
        };
        match fs::remove_dir_all(&dir) {
            Ok(()) => println!("scratch: swept {}, a run's that is gone ({killed} processes it left killed, {containers} containers removed); {kept}", dir.display()),
            Err(e) => println!("scratch: could not sweep {}: {e}", dir.display()),
        }
    }
}

/// A dead run's scratch on disk, as its directory names it: only a
/// `target/e2e/<run>` of the same run, and only while it is there.
fn scratch_of(dir: &Path) -> Option<PathBuf> {
    let named = PathBuf::from(std::ffi::OsStr::from_bytes(&fs::read(dir.join(SCRATCH)).ok()?));
    let ours = named.is_absolute() && named.file_name() == dir.file_name() && named.parent().is_some_and(|p| p.ends_with("target/e2e"));
    (ours && named.is_dir()).then_some(named)
}

/// Kills what a dead run left running in `dir` (its node's wrangler and
/// workerd, its Chromes: a process working in it or naming it), then waits
/// for them to go, so its state is copied still and its pages freed. The
/// number killed.
fn kill_leftovers(dir: &Path) -> usize {
    let mut inside = dir.as_os_str().as_bytes().to_vec();
    inside.push(b'/');
    let me = std::process::id();
    let mut killed = vec![];
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|p| p.parse::<u32>().ok()).filter(|&p| p != me) else { continue };
        let cwd = fs::read_link(entry.path().join("cwd")).ok();
        let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let works_in = cwd.is_some_and(|c| c.starts_with(dir));
        let names = cmdline.windows(inside.len()).any(|w| w == inside.as_slice());
        // SAFETY: kill(2) on a process of this user's, which works in or names the dead run's directory
        if (works_in || names) && unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0 {
            killed.push(pid);
        }
    }
    // gone: no process, or a zombie its parent has yet to reap
    let running = |p: &u32| fs::read_to_string(format!("/proc/{p}/stat")).is_ok_and(|st| st.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z')));
    let t0 = Instant::now();
    while killed.iter().any(running) && t0.elapsed() < LEFTOVERS_WAIT {
        std::thread::sleep(Duration::from_millis(50));
    }
    killed.len()
}

/// Copies what a run's directory keeps (all but `NOT_KEPT`) into `to`,
/// beside what is there; the bytes copied. Symlinks are copied as links
/// (Chrome's Singleton*), and sockets and the like left out.
fn copy_kept(from: &Path, to: &Path) -> Result<u64> {
    fn walk(root: &Path, from: &Path, to: &Path, bytes: &mut u64) -> Result<()> {
        fs::create_dir_all(to).with_context(|| format!("make {}", to.display()))?;
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
            let d = std::env::temp_dir().join(format!("e2e-scratch-test-{}", devstack::random_hex(6)));
            fs::create_dir_all(&d).unwrap();
            Tmp(d)
        }
        /// A run's scratch on disk, `target/e2e/<run>`.
        fn disk(&self, run: &str) -> PathBuf {
            let d = self.0.join("target/e2e").join(run);
            fs::create_dir_all(&d).unwrap();
            d
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

    /// The directory a run killed outright leaves: its lock file free, its
    /// scratch named.
    fn dead(root: &Path, run: &str, scratch: &Path) -> PathBuf {
        let dir = root.join(run);
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join(LOCK)).unwrap();
        fs::write(dir.join(SCRATCH), scratch.as_os_str().as_bytes()).unwrap();
        fill(&dir);
        dir
    }

    fn kept(disk: &Path) -> bool {
        fs::read_to_string(disk.join("cell/.wrangler/state/v3/do/x.sqlite")).is_ok_and(|t| t == "rows")
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
        let placed = place_in(&root, RUN, &tmp.disk(RUN), |runs| Ok(Room { available: RESERVE, free: 64 * GIB, runs })).unwrap();
        assert!(placed.is_none());
        assert!(runs(&root).is_empty());
    }

    #[test]
    fn a_passing_run_leaves_nothing_and_a_failing_one_keeps_its_state_on_disk() {
        let tmp = Tmp::new();
        let (root, disk) = (tmp.0.join("ram"), tmp.disk(RUN));
        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        assert_eq!(ram.dir(), root.join(RUN));
        for d in [&root, &root.join(RUN)] {
            assert_eq!(fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700, "{} is this user's alone", d.display());
        }
        assert_eq!(live(&root), 1);
        fill(ram.dir());
        ram.end(false);
        assert!(!root.join(RUN).exists() && fs::read_dir(&disk).unwrap().count() == 0);

        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        fill(ram.dir());
        let ending = ram.ending();
        ram.end(true);
        // replayed (a signal after the run's own end): nothing more
        ending.end(false);
        ram.end(false);
        assert!(!root.join(RUN).exists());
        assert!(kept(&disk));
        assert_eq!(fs::read_to_string(disk.join("chrome-ab12/chrome.log")).unwrap(), "said");
        assert_eq!(fs::read_link(disk.join("chrome-ab12/SingletonLock")).unwrap(), Path::new("host-123"));
        for gone in ["cell/build", "cell/.wrangler/tmp", LOCK, SCRATCH] {
            assert!(!disk.join(gone).exists(), "{gone} is not kept");
        }
    }

    #[test]
    fn a_run_that_ends_early_keeps_its_state_as_it_drops_and_a_signal_after_finds_nothing() {
        let tmp = Tmp::new();
        let (root, disk) = (tmp.0.join("ram"), tmp.disk(RUN));
        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        fill(ram.dir());
        let ending = ram.ending();
        drop(ram);
        assert!(!root.join(RUN).exists() && kept(&disk));
        ending.end(true);
    }

    #[test]
    fn a_copy_that_fails_leaves_the_state_in_ram() {
        let tmp = Tmp::new();
        let root = tmp.0.join("ram");
        // its scratch is a file: nothing can be made under it
        let disk = tmp.0.join("target/e2e").join(RUN);
        fs::create_dir_all(disk.parent().unwrap()).unwrap();
        fs::write(&disk, "in the way").unwrap();
        let ram = place_in(&root, RUN, &disk, roomy).unwrap().expect("room");
        fill(ram.dir());
        ram.end(true);
        assert_eq!(fs::read_to_string(root.join(RUN).join("cell/.wrangler/state/v3/do/x.sqlite")).unwrap(), "rows");
    }

    #[test]
    fn the_next_start_sweeps_a_dead_runs_directory_its_leftovers_killed_and_its_state_kept() {
        let tmp = Tmp::new();
        let root = tmp.0.join("ram");
        own_dir(&root).unwrap();
        let dead_disk = tmp.disk("aaaaaaaaaa");
        let dead_dir = dead(&root, "aaaaaaaaaa", &dead_disk);
        // what it left running there
        let mut leftover = std::process::Command::new("sleep").arg("600").current_dir(&dead_dir).spawn().unwrap();
        // a start that died taking its place: no lock made
        fs::create_dir_all(root.join("dddddddddd")).unwrap();
        let alive = place_in(&root, "bbbbbbbbbb", &tmp.disk("bbbbbbbbbb"), roomy).unwrap().expect("room");
        // what is not a run's: never touched
        fs::create_dir_all(root.join("not-a-run")).unwrap();

        let next = place_in(&root, RUN, &tmp.disk(RUN), |runs| {
            assert_eq!(runs, 2, "the live run and this one");
            roomy(runs)
        })
        .unwrap()
        .expect("room");
        assert!(!dead_dir.exists() && !root.join("dddddddddd").exists(), "the dead runs' directories are swept");
        assert!(kept(&dead_disk));
        // the sweep waited for it to go: reaped now, or it was never killed
        let status = leftover.try_wait().unwrap();
        let _ = leftover.kill();
        assert_eq!(status.and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s)), Some(libc::SIGKILL), "its leftover is killed");
        assert!(alive.dir().is_dir() && root.join("not-a-run").is_dir());
        alive.end(false);
        next.end(false);
    }

    #[test]
    fn a_dead_runs_state_goes_only_to_its_own_scratch() {
        let tmp = Tmp::new();
        let root = tmp.0.join("ram");
        own_dir(&root).unwrap();
        // names another run's scratch, and a directory that is no scratch
        let elsewhere = tmp.disk("ffffffffff");
        let dir = dead(&root, "aaaaaaaaaa", &elsewhere);
        assert_eq!(scratch_of(&dir), None);
        let notes = tmp.0.join("notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(dir.join(SCRATCH), notes.as_os_str().as_bytes()).unwrap();
        assert_eq!(scratch_of(&dir), None);
        sweep(&root);
        assert!(!dir.exists());
        assert!(fs::read_dir(&elsewhere).unwrap().count() == 0 && fs::read_dir(&notes).unwrap().count() == 0);
    }

    #[test]
    fn a_directory_named_relatively_or_a_bad_run_id_is_refused() {
        assert!(place_in(Path::new("relative/ram"), RUN, Path::new("/disk"), roomy).is_err());
        let tmp = Tmp::new();
        assert!(place_in(&tmp.0, "../escape", &tmp.0, roomy).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_box_reads_its_available_ram_and_free_space() {
        assert!(mem_available().unwrap() > 0);
        assert!(fs_free(&std::env::temp_dir()).unwrap() > 0);
        let tmp = Tmp::new();
        fill(&tmp.0);
        assert!(size(&tmp.0) > 0);
    }
}
