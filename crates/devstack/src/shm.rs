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
    /// whose run is gone. `is_run` tells a run's name (the e2e's own run
    /// ids), and the sweep touches nothing else. Refused, saying why, when
    /// `root`'s file system has less than `room` free.
    pub fn claim(root: &Path, run: &str, room: u64, is_run: fn(&str) -> bool) -> Result<RunDir> {
        anyhow::ensure!(is_run(run), "{run:?} is not a run's name");
        trusted_root(root)?;
        // A claim and a sweep hold this alike: a sweep never sees a run's
        // lock file before its run holds it.
        let _claiming = hold(&root.join(".claims"), true)?.context("a claim's hold is waited for")?;
        sweep(root, is_run)?;
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
        // remove_dir_all takes a link itself, never what it names
        remove_dir(&self.dir)?;
        match fs::remove_file(&self.lock) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", self.lock.display())),
            _ => Ok(()),
        }
    }
}

/// `root`, made if it is not there, and only if it is this user's own: a
/// directory, not a link, owned by this user, that no one else may enter.
/// `/dev/shm` is shared, and the root's name is known, so another user
/// could make it first, as a link into this user's files or a directory
/// they can write in, and the sweep would remove what they put there.
fn trusted_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    match fs::DirBuilder::new().mode(0o700).create(root) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e).with_context(|| format!("make {}", root.display())),
        _ => {}
    }
    let meta = fs::symlink_metadata(root).with_context(|| format!("read {}", root.display()))?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.file_type().is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        bail!(
            "{} is not this user's private directory (a directory, not a link, owned by uid {uid}, mode 0700): \
             the e2e keeps and removes run state there only when it is. Remove it, or see who made it",
            root.display()
        );
    }
    Ok(())
}

/// Removes each run's directory under `root` whose run is gone: its lock
/// is free, or it has none. Answers how many. Its caller holds `.claims`.
/// Only a run's own entries count: a real directory named as a run
/// (`is_run`), or a regular file named as its lock. Anything else, a link
/// among them, is left as it is.
fn sweep(root: &Path, is_run: fn(&str) -> bool) -> Result<u32> {
    let mut removed = 0;
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        // the entry itself, never what a link names
        let kind = entry.file_type()?;
        if let Some(run) = name.strip_suffix(".lock").filter(|run| is_run(run)) {
            if !kind.is_file() {
                continue;
            }
            // held while its run lives, by the kernel's count: free once
            // the run is gone, however it went
            let Some(_free) = hold(&entry.path(), false)? else { continue };
            Remover { dir: root.join(run), lock: entry.path() }.remove()?;
            removed += 1;
        } else if is_run(&name) && kind.is_dir() && !root.join(format!("{name}.lock")).exists() {
            // its lock went first: a removal cut short
            remove_dir(&entry.path())?;
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

    /// These tests' runs' names, as the e2e's are its run ids.
    fn is_run(name: &str) -> bool {
        name.strip_prefix("run-").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
    }

    /// Not a test of its own: the run `a_killed_run_leaves_nothing` kills.
    /// It claims a directory, fills it, says so, and waits to be killed.
    #[test]
    fn helper_a_run_to_kill() {
        let Some(root) = std::env::var_os(CHILD) else { return };
        let run = RunDir::claim(Path::new(&root), "run-killed", 0, is_run).unwrap();
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
        let killed = root.path().join("run-killed");
        wait_for("the child's run is claimed and filled", || killed.join("objects.sqlite3").metadata().is_ok_and(|m| m.len() == 1 << 20));

        let alive = RunDir::claim(root.path(), "run-while-it-lives", 0, is_run).unwrap();
        assert!(killed.is_dir(), "a live run's directory was removed");
        drop(alive);

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(killed.is_dir(), "the killed run left its directory (as a SIGKILL does)");

        let next = RunDir::claim(root.path(), "run-next", 0, is_run).unwrap();
        assert!(!killed.exists(), "the next run left the killed run's directory in RAM");
        assert!(!root.path().join("run-killed.lock").exists(), "the next run left the killed run's lock");
        assert!(next.path().is_dir());
    }

    /// Goal: a run's directory goes as the run ends, by its end's own call
    /// or its drop, and its lock with it.
    #[test]
    fn a_run_removes_its_directory_as_it_ends() {
        let root = Root::new("ends");
        let run = RunDir::claim(root.path(), "run-ends", 0, is_run).unwrap();
        fs::write(run.path().join("state"), b"x").unwrap();
        run.remove().unwrap();
        assert!(!root.path().join("run-ends").exists() && !root.path().join("run-ends.lock").exists());

        let dropped = RunDir::claim(root.path(), "run-dropped", 0, is_run).unwrap();
        let remover = dropped.remover();
        drop(dropped);
        assert!(!root.path().join("run-dropped").exists() && !root.path().join("run-dropped.lock").exists());
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
        let run = RunDir::claim(root.path(), "run-linked", 0, is_run).unwrap();
        std::os::unix::fs::symlink(outside.path(), run.path().join("node_modules")).unwrap();
        run.remove().unwrap();
        assert!(!root.path().join("run-linked").exists());
        assert!(outside.path().join("pkg/index.js").is_file(), "the removal followed the link");

        // and so does the sweep, for a killed run's directory
        fs::create_dir(root.path().join("run-swept")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("run-swept/node_modules")).unwrap();
        drop(RunDir::claim(root.path(), "run-next", 0, is_run).unwrap());
        assert!(!root.path().join("run-swept").exists());
        assert!(outside.path().join("pkg/index.js").is_file(), "the sweep followed the link");
    }

    /// Goal: a root this user does not own outright is refused before
    /// anything in it is read or removed: a link (here into a directory
    /// holding what a sweep would take), or a directory others may write.
    #[test]
    fn a_root_that_is_not_this_users_own_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let target = Root::new("untrusted-target");
        fs::create_dir_all(target.path().join("victim")).unwrap();
        let link = Root::new("untrusted-link");
        std::os::unix::fs::symlink(target.path(), link.path()).unwrap();
        let refused = RunDir::claim(link.path(), "run-run", 0, is_run).err().expect("a linked root is refused");
        assert!(format!("{refused:#}").contains("not this user's private directory"), "{refused:#}");
        assert!(target.path().join("victim").is_dir(), "a sweep through the link removed what it names");
        fs::remove_file(link.path()).unwrap();

        let open = Root::new("untrusted-open");
        fs::create_dir(open.path()).unwrap();
        fs::set_permissions(open.path(), fs::Permissions::from_mode(0o777)).unwrap();
        fs::create_dir(open.path().join("planted")).unwrap();
        assert!(RunDir::claim(open.path(), "run-run", 0, is_run).is_err(), "a root others may write is refused");
        assert!(open.path().join("planted").is_dir());
    }

    /// Goal: the sweep takes only a gone run's own entries. A link named as
    /// a run (and what it names), a file that is not a lock, and a name that
    /// is not a run's stay.
    #[test]
    fn the_sweep_takes_only_runs() {
        let root = Root::new("only-runs");
        let outside = Root::new("only-runs-outside");
        fs::create_dir_all(outside.path().join("kept")).unwrap();
        drop(RunDir::claim(root.path(), "run-first", 0, is_run).unwrap());
        std::os::unix::fs::symlink(outside.path(), root.path().join("run-link")).unwrap();
        fs::write(root.path().join("notes.txt"), b"x").unwrap();
        fs::create_dir(root.path().join("not.a.run")).unwrap();
        fs::create_dir(root.path().join("saved-state")).unwrap();
        drop(RunDir::claim(root.path(), "run-next", 0, is_run).unwrap());
        assert!(fs::symlink_metadata(root.path().join("run-link")).is_ok(), "the sweep took a link");
        assert!(outside.path().join("kept").is_dir(), "the sweep followed a link");
        assert!(root.path().join("notes.txt").is_file() && root.path().join("not.a.run").is_dir());
        assert!(root.path().join("saved-state").is_dir(), "the sweep took a directory no run is named");
    }

    /// Goal: a run with no room for its state in RAM does not start, and
    /// says where and how much.
    #[test]
    fn a_run_without_room_is_refused() {
        let root = Root::new("room");
        let refused = RunDir::claim(root.path(), "run-big", u64::MAX, is_run).err().expect("refused");
        let said = format!("{refused:#}");
        assert!(said.contains(&root.path().display().to_string()) && said.contains("free"), "{said}");
        assert!(!root.path().join("run-big").exists());
    }
}
