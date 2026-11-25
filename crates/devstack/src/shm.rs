//! A celld e2e run's directory in RAM (`/dev/shm`): the run stages its
//! project there, and with it `celld dev`'s state, which celld keeps beside
//! the project (`PROJECT/.celld/dev`, celld::state_dir) and the e2e clears
//! between the node's lives (`clear_state`).
//!
//! On the home disk that state shared the disk with every other job on the
//! box. A job writing gigabytes held every SQLite call of the node for
//! seconds, its lease renewal and the read-back after it among them, and
//! the node fenced itself (FIN-728, 2026-11-24). In RAM it waits on no disk.
//!
//! RAM is not given back when a process dies, so each run's directory is
//! held by a lock its run keeps open (`<run>.lock` beside it): the run
//! removes the directory as it ends (`RunDir::remove`, or its drop), its
//! termination handler as it is stopped, and the next run removes every
//! directory whose lock is free, the run that held it having been killed
//! outright. A run starts only with room to spare in RAM (`ROOM_BYTES`).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// What a run needs free in RAM before it starts. A celld run of every
/// section (no engine) ended with 0.93 GB of state on 2026-11-24, and one
/// of the container sections with 0.3 GB: this is three times the larger.
pub const ROOM_BYTES: u64 = 3 << 30;

/// This user's runs' directories: in `/dev/shm`, the RAM every Linux box
/// mounts there, under a name of the user's own.
pub fn root() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/dev/shm/fragment-e2e-{uid}"))
}

/// One run's directory, held until the run ends.
#[must_use = "the run's directory is removed when it is dropped"]
pub struct RunDir {
    dir: PathBuf,
    lock: PathBuf,
    _held: File,
}

impl RunDir {
    /// `run`'s directory under `root`, after removing every directory there
    /// whose run is gone. Refused, saying why, when `root`'s file system
    /// has less than `room` free.
    pub fn claim(root: &Path, run: &str, room: u64) -> Result<RunDir> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(root).with_context(|| format!("make {}", root.display()))?;
        // A claim and a sweep hold this alike: a sweep never sees a run's
        // lock file before its run holds it.
        let _claiming = hold(&root.join(".claims"), true)?.context("a claim's hold is waited for")?;
        sweep(root)?;
        let free = free_bytes(root).with_context(|| format!("read the room in {}", root.display()))?;
        if free < room {
            bail!(
                "{} has {} free, and a celld e2e run keeps its node's state there, in RAM, until it ends: it needs {} free to start. \
                 Wait for another run to end, or free some of it",
                root.display(),
                gib(free),
                gib(room)
            );
        }
        let lock = root.join(format!("{run}.lock"));
        let held = OpenOptions::new().write(true).create_new(true).open(&lock).with_context(|| format!("{}: this run's lock, made by another run first", lock.display()))?;
        held.try_lock().map_err(|e| anyhow::anyhow!("lock {}: {e}", lock.display()))?;
        let dir = root.join(run);
        fs::DirBuilder::new().mode(0o700).create(&dir).with_context(|| format!("{}: this run's directory, made by another run first", dir.display()))?;
        Ok(RunDir { dir, lock, _held: held })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// What removes the directory from elsewhere: a termination handler,
    /// which the run's own value does not reach.
    pub fn remover(&self) -> Remover {
        Remover { dir: self.dir.clone(), lock: self.lock.clone() }
    }

    /// Removes the directory now, as the run ends. The end of a run calls
    /// this itself: `std::process::exit` runs no drop.
    pub fn remove(&self) -> Result<()> {
        self.remover().remove()
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

/// A run's directory and lock, by path.
#[derive(Clone)]
pub struct Remover {
    dir: PathBuf,
    lock: PathBuf,
}

impl Remover {
    /// The directory, then its lock: a removal cut short between the two
    /// leaves a lock with nothing to hold, which the next sweep removes.
    pub fn remove(&self) -> Result<()> {
        remove_dir(&self.dir)?;
        match fs::remove_file(&self.lock) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", self.lock.display())),
            _ => Ok(()),
        }
    }
}

/// Removes each run's directory under `root` whose run is gone: its lock
/// is free, or it has none. Answers how many. Its caller holds `.claims`.
fn sweep(root: &Path) -> Result<u32> {
    let mut removed = 0;
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with('.') {
            continue;
        }
        if let Some(run) = name.strip_suffix(".lock") {
            // held while its run lives, by the kernel's count: free once
            // the run is gone, however it went
            let Some(_free) = hold(&path, false)? else { continue };
            Remover { dir: root.join(run), lock: path.clone() }.remove()?;
            removed += 1;
        } else if path.is_dir() && !root.join(format!("{name}.lock")).exists() {
            // its lock went first: a removal cut short
            remove_dir(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// `path`'s lock, held exclusively; with `wait`, once its holder lets go,
/// else `None` while another holds it.
fn hold(path: &Path, wait: bool) -> Result<Option<File>> {
    let file = OpenOptions::new().write(true).create(true).truncate(false).open(path).with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) if wait => {
            file.lock().with_context(|| format!("lock {}", path.display()))?;
            Ok(Some(file))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e).with_context(|| format!("lock {}", path.display())),
    }
}

fn remove_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", dir.display())),
        _ => Ok(()),
    }
}

/// The bytes an unprivileged writer may still use on `path`'s file system.
fn free_bytes(path: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: a zeroed statvfs is a valid out-parameter, and `path` is a
    // NUL-terminated string that outlives the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Set for the child `a_killed_run_leaves_nothing` starts: the root its
    /// run is claimed in.
    const CHILD: &str = "FRAGMENT_SHM_TEST_CHILD";

    /// Not a test of its own: the run `a_killed_run_leaves_nothing` kills.
    /// It claims a directory, fills it, says so, and waits to be killed.
    #[test]
    fn helper_a_run_to_kill() {
        let Some(root) = std::env::var_os(CHILD) else { return };
        let run = RunDir::claim(Path::new(&root), "killed", 0).unwrap();
        fs::write(run.path().join("objects.sqlite3"), vec![0u8; 1 << 20]).unwrap();
        println!("claimed");
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// A fresh root for one test, removed as it ends.
    struct Root(PathBuf);

    impl Root {
        fn new(what: &str) -> Root {
            let path = std::env::temp_dir().join(format!("devstack-shm-{what}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Root(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !done() {
            assert!(t0.elapsed() < Duration::from_secs(30), "{what} within 30 s");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Goal: a run killed outright (SIGKILL: no drop, no handler) leaves
    /// no RAM behind past the next run's start, and a live run's directory
    /// is never taken. Method: a child process claims and fills a
    /// directory; another run's claim leaves it while the child lives; the
    /// child is killed; the next claim removes its directory and lock.
    #[test]
    fn a_killed_run_leaves_nothing() {
        let root = Root::new("killed");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shm::tests::helper_a_run_to_kill", "--nocapture"])
            .env(CHILD, root.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let killed = root.path().join("killed");
        wait_for("the child's run is claimed and filled", || killed.join("objects.sqlite3").metadata().is_ok_and(|m| m.len() == 1 << 20));

        let alive = RunDir::claim(root.path(), "while-it-lives", 0).unwrap();
        assert!(killed.is_dir(), "a live run's directory was removed");
        drop(alive);

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(killed.is_dir(), "the killed run left its directory (as a SIGKILL does)");

        let next = RunDir::claim(root.path(), "next", 0).unwrap();
        assert!(!killed.exists(), "the next run left the killed run's directory in RAM");
        assert!(!root.path().join("killed.lock").exists(), "the next run left the killed run's lock");
        assert!(next.path().is_dir());
    }

    /// Goal: a run's directory goes as the run ends, by its end's own call
    /// or its drop, and its lock with it.
    #[test]
    fn a_run_removes_its_directory_as_it_ends() {
        let root = Root::new("ends");
        let run = RunDir::claim(root.path(), "ends", 0).unwrap();
        fs::write(run.path().join("state"), b"x").unwrap();
        run.remove().unwrap();
        assert!(!root.path().join("ends").exists() && !root.path().join("ends.lock").exists());

        let dropped = RunDir::claim(root.path(), "dropped", 0).unwrap();
        let remover = dropped.remover();
        drop(dropped);
        assert!(!root.path().join("dropped").exists() && !root.path().join("dropped.lock").exists());
        remover.remove().unwrap();
    }

    /// Goal: removing a run's directory takes a link in it (the repo's
    /// node_modules, which the e2e links there), never what it links to.
    #[test]
    fn removing_a_run_leaves_what_it_links_to() {
        let root = Root::new("links");
        let outside = Root::new("links-outside");
        fs::create_dir_all(outside.path().join("pkg")).unwrap();
        fs::write(outside.path().join("pkg/index.js"), b"x").unwrap();
        let run = RunDir::claim(root.path(), "linked", 0).unwrap();
        std::os::unix::fs::symlink(outside.path(), run.path().join("node_modules")).unwrap();
        run.remove().unwrap();
        assert!(!root.path().join("linked").exists());
        assert!(outside.path().join("pkg/index.js").is_file(), "the removal followed the link");

        // and so does the sweep, for a killed run's directory
        fs::create_dir(root.path().join("swept")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("swept/node_modules")).unwrap();
        drop(RunDir::claim(root.path(), "next", 0).unwrap());
        assert!(!root.path().join("swept").exists());
        assert!(outside.path().join("pkg/index.js").is_file(), "the sweep followed the link");
    }

    /// Goal: a run with no room for its state in RAM does not start, and
    /// says where and how much.
    #[test]
    fn a_run_without_room_is_refused() {
        let root = Root::new("room");
        let refused = RunDir::claim(root.path(), "big", u64::MAX).err().expect("refused");
        let said = format!("{refused:#}");
        assert!(said.contains(&root.path().display().to_string()) && said.contains("free"), "{said}");
        assert!(!root.path().join("big").exists());
    }
}
