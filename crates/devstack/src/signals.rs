//! Termination signals (a terminal's Ctrl-C, SIGTERM, a closed terminal's
//! SIGHUP), turned into work a thread does. signal-hook does the part a
//! signal handler is in: its handler only writes to a pipe, which its
//! iterator reads on a thread of ours. What it has no say in is here:
//! that a signal the process was started ignoring stays ignored, and that
//! the processes it starts ignore Ctrl-C (`outlive_interrupt`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{ensure, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// The signals that end a run: Ctrl-C (SIGINT), SIGTERM, SIGHUP.
const TERMINATION: [libc::c_int; 3] = [SIGINT, SIGTERM, SIGHUP];

/// Whether `on_termination` has run: it runs once per process.
static HANDLED: AtomicBool = AtomicBool::new(false);

/// A signal's disposition now: whether this process was started with it
/// ignored (a background job's SIGINT, nohup's SIGHUP), which it keeps.
fn ignored(signal: libc::c_int) -> bool {
    // SAFETY: a zeroed sigaction is a valid out-parameter, and a null new
    // action only reads the current one
    let mut now: libc::sigaction = unsafe { std::mem::zeroed() };
    let read = unsafe { libc::sigaction(signal, std::ptr::null(), &mut now) };
    assert_eq!(read, 0, "sigaction reads signal {signal}'s disposition");
    now.sa_sigaction == libc::SIG_IGN
}

/// When the process gets one of the termination signals it does not
/// ignore, on a thread of its own: kills everything the process started
/// (`kill_started`), deaf to further signals until that is done, then runs
/// `then` with the signal's number and exits as that signal would have it
/// (128 + its number). A second signal during `then` ends the process at
/// once, by that signal's own default; one during the kill could not, as
/// the kill stops each process before it kills them all (tree.rs), and an
/// exit between the two would leave them stopped for good. Once per process.
pub fn on_termination(then: impl FnOnce(i32) + Send + 'static) -> Result<()> {
    ensure!(!HANDLED.swap(true, Ordering::SeqCst), "termination is handled once per process");
    let handled: Vec<libc::c_int> = TERMINATION.into_iter().filter(|&signal| !ignored(signal)).collect();
    // set once what the process started is killed: from then on each takes
    // its default action
    let ending = Arc::new(AtomicBool::new(false));
    for &signal in &handled {
        signal_hook::flag::register_conditional_default(signal, Arc::clone(&ending))?;
    }
    // its pipe is close-on-exec: the processes it starts (wrangler,
    // Chrome) inherit neither end
    let mut signals = Signals::new(&handled)?;
    std::thread::Builder::new().name("termination".into()).spawn(move || {
        // nothing closes the iterator: this returns on a signal
        let Some(signal) = signals.forever().next() else { return };
        crate::kill_started();
        ending.store(true, Ordering::SeqCst);
        then(signal);
        std::process::exit(128 + signal);
    })?;
    Ok(())
}

/// This process ignores Ctrl-C from here on, so that it outlives a node in
/// the terminal's process group (which the same Ctrl-C stops) to clean up
/// after it. A process it starts afterwards inherits the ignore: call it
/// once the node has started.
pub fn outlive_interrupt() -> Result<()> {
    // SAFETY: SIG_IGN for SIGINT
    let previous = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
    ensure!(previous != libc::SIG_ERR, "ignore SIGINT: {}", std::io::Error::last_os_error());
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Each test re-runs this test binary as a child (`CHILD` names what it
    //! does there) and signals it: a handler is the process's, and the work
    //! ends the process.
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// What the child does: its value names it.
    const CHILD: &str = "FRAGMENT_SIGNALS_TEST_CHILD";
    /// How long a child waits for its signal, and the parent for the child.
    const WAIT: Duration = Duration::from_secs(20);

    /// The child's half: handles termination, says so, then waits to be
    /// signalled. `slow`: its work takes a while, so a second signal comes
    /// while it runs.
    fn child(slow: bool) {
        on_termination(move |signal| {
            println!("work ran for {signal}");
            if slow {
                std::thread::sleep(WAIT);
            }
        })
        .expect("handled");
        let again = on_termination(|_| unreachable!("the second handling never runs"));
        println!("ready; again: {}", again.map_or_else(|e| e.to_string(), |()| "handled twice".into()));
        std::thread::sleep(WAIT);
        println!("no signal came");
    }

    /// This test run again as a child doing `what`, `ignoring` SIGHUP as
    /// nohup would start it, once it says it is ready, and the line saying so.
    /// Its termination signals start at their defaults and unblocked,
    /// whatever this test started with: an ignored disposition and a
    /// blocked mask are both inherited across exec, and a background job of
    /// a non-interactive shell starts with SIGINT ignored (nohup, SIGHUP),
    /// so a `cargo xtask check` run detached made a child that kept them
    /// ignored and never ended.
    fn spawn(test: &str, what: &str, ignoring_hup: bool) -> (Child, BufReader<std::process::ChildStdout>, String) {
        let mut cmd = Command::new(std::env::current_exe().expect("the test binary"));
        cmd.args(["--exact", test, "--nocapture", "--test-threads=1"]).env(CHILD, what).stdout(Stdio::piped());
        // SAFETY: signal(2), sigemptyset(3), sigaddset(3) and sigprocmask(2)
        // between fork and exec, each async-signal-safe, on a set of its own
        unsafe {
            cmd.pre_exec(move || {
                let mut unblock: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut unblock);
                for signal in TERMINATION {
                    let disposition = if ignoring_hup && signal == libc::SIGHUP { libc::SIG_IGN } else { libc::SIG_DFL };
                    if libc::signal(signal, disposition) == libc::SIG_ERR || libc::sigaddset(&mut unblock, signal) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if libc::sigprocmask(libc::SIG_UNBLOCK, &unblock, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
        };
        let mut child = cmd.spawn().expect("the child starts");
        let mut out = BufReader::new(child.stdout.take().expect("its stdout"));
        let (mut line, mut ready) = (String::new(), None);
        // bounded: the harness prints a line or two before the child's own,
        // and the test's name before its first, on the same line
        for _ in 0..16 {
            line.clear();
            if out.read_line(&mut line).expect("a line") == 0 {
                break;
            }
            if let Some(at) = line.find("ready; ") {
                ready = Some(line[at..].to_string());
                break;
            }
        }
        match ready {
            Some(ready) => (child, out, ready),
            None => {
                let _ = child.kill();
                panic!("the child never said it was ready: {:?}", child.wait());
            }
        }
    }

    fn signal(child: &Child, signal: libc::c_int) {
        let pid = libc::pid_t::try_from(child.id()).expect("a pid");
        // SAFETY: kill(2) on our own child
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0, "signal {signal} sent");
    }

    /// The child's exit, within `WAIT`, and the rest of what it printed.
    fn ended(mut child: Child, mut out: BufReader<std::process::ChildStdout>) -> (std::process::ExitStatus, String) {
        let t0 = Instant::now();
        // bounded: WAIT, polled every 20 ms
        let status = loop {
            if let Some(status) = child.try_wait().expect("its status") {
                break status;
            }
            assert!(t0.elapsed() < WAIT, "the child did not end");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut out, &mut rest).expect("the rest of its output");
        (status, rest)
    }

    /// Goal: each termination signal runs the work once, with its number,
    /// then ends the process as the signal would (128 + it), and the
    /// handling is once per process. Method: a child per signal, signalled
    /// once it is ready.
    #[test]
    fn a_termination_signal_runs_the_work_then_exits_as_it() {
        const TEST: &str = "signals::tests::a_termination_signal_runs_the_work_then_exits_as_it";
        if std::env::var(CHILD).as_deref() == Ok("plain") {
            return child(false);
        }
        for sig in TERMINATION {
            let (c, out, ready) = spawn(TEST, "plain", false);
            assert!(ready.contains("termination is handled once per process"), "{ready}");
            signal(&c, sig);
            let (status, rest) = ended(c, out);
            assert_eq!(status.code(), Some(128 + sig), "signal {sig}: {status:?}");
            assert_eq!(rest.matches("work ran for").count(), 1, "{rest}");
            assert!(rest.contains(&format!("work ran for {sig}\n")), "{rest}");
        }
    }

    /// Goal: a signal the process was started ignoring stays ignored (nohup's
    /// SIGHUP), and a second signal while the work runs ends the process at
    /// once, by that signal. Method: a child started with SIGHUP ignored is
    /// sent it, then SIGTERM twice, its work slow.
    #[test]
    fn an_ignored_signal_stays_ignored_and_a_second_ends_at_once() {
        const TEST: &str = "signals::tests::an_ignored_signal_stays_ignored_and_a_second_ends_at_once";
        if std::env::var(CHILD).as_deref() == Ok("slow") {
            return child(true);
        }
        let (c, mut out, _) = spawn(TEST, "slow", true);
        signal(&c, libc::SIGHUP);
        std::thread::sleep(Duration::from_millis(300));
        signal(&c, libc::SIGTERM);
        let mut line = String::new();
        out.read_line(&mut line).expect("a line");
        assert_eq!(line, format!("work ran for {}\n", libc::SIGTERM), "SIGHUP was ignored, SIGTERM ran the work");
        let t0 = Instant::now();
        signal(&c, libc::SIGTERM);
        let (status, rest) = ended(c, out);
        assert_eq!(status.signal(), Some(libc::SIGTERM), "the second ended it by the signal itself: {status:?} {rest}");
        assert!(t0.elapsed() < WAIT / 2, "at once, not after the work");
    }

    /// A process's state letter from /proc (`T` stopped, `Z` a zombie), or
    /// `None` once it is gone.
    #[cfg(target_os = "linux")]
    fn state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat[stat.rfind(')')? + 2..].chars().next()
    }

    /// Goal (FIN-457): a second signal while what the process started is
    /// being killed (each stopped, then SIGKILLed: tree.rs) does not end
    /// the process early, which would leave them stopped for good, holding
    /// their memory and ports, deaf to another SIGTERM. Method: a child that
    /// started a tree (a process that detaches another, as miniflare starts
    /// Chrome) is sent SIGTERM; a `ps` of its own sends it the second from
    /// inside the walk, once the tree's root is stopped (its second call:
    /// the first found the root). None of the tree is left, stopped or not,
    /// and the child ends as the first signal would have it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_second_signal_while_the_started_are_killed_leaves_none() {
        const TEST: &str = "signals::tests::a_second_signal_while_the_started_are_killed_leaves_none";
        // the child's `ps`, which counts its calls: named for the parent
        let shim = |parent: u32| std::env::temp_dir().join(format!("devstack-second-signal-{parent}"));
        if std::env::var(CHILD).as_deref() == Ok("tree") {
            let script = "setsid sh -c 'sleep 60 & echo $$ $!; wait' & wait";
            let mut tree = Command::new("sh").args(["-c", script]).stdout(Stdio::piped()).stdin(Stdio::null()).spawn().expect("the tree starts");
            let mut line = String::new();
            BufReader::new(tree.stdout.take().expect("its stdout")).read_line(&mut line).expect("its pids");
            let path = std::env::var_os("PATH").expect("a PATH");
            let real = std::env::split_paths(&path).map(|d| d.join("ps")).find(|p| p.is_file()).expect("a ps");
            let dir = shim(std::os::unix::process::parent_id());
            std::fs::create_dir_all(&dir).expect("the shim's directory");
            let ps = format!("#!/bin/sh\nn=$(($(cat '{d}/calls' 2>/dev/null || echo 0) + 1))\necho $n > '{d}/calls'\n[ $n = 2 ] && kill -TERM $PPID\nexec '{real}' \"$@\"\n", d = dir.display(), real = real.display());
            std::fs::write(dir.join("ps"), ps).expect("the shim");
            std::fs::set_permissions(dir.join("ps"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("the shim runs");
            // the walk's `ps` (tree.rs) is the shim from here on
            std::env::set_var("PATH", std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&path))).expect("a PATH"));
            on_termination(|_| {}).expect("handled");
            println!("ready; {} {}", tree.id(), line.trim());
            std::thread::sleep(WAIT);
            // no signal came: the tree is this child's to end
            let _ = tree.kill();
            let _ = tree.wait();
            return;
        }
        let (c, out, ready) = spawn(TEST, "tree", false);
        let tree: Vec<u32> = ready.trim_start_matches("ready; ").split_whitespace().map(|p| p.parse().expect("a pid")).collect();
        assert_eq!(tree.len(), 3, "{ready}");
        signal(&c, libc::SIGTERM);
        let (status, rest) = ended(c, out);
        let calls = std::fs::read_to_string(shim(std::process::id()).join("calls")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(shim(std::process::id()));
        let left: Vec<(u32, char)> = tree.iter().filter_map(|&pid| state(pid).filter(|s| *s != 'Z').map(|s| (pid, s))).collect();
        for &(pid, _) in &left {
            // SAFETY: kill(2) on a process this test's child started, still there
            unsafe { libc::kill(libc::pid_t::try_from(pid).expect("a pid"), libc::SIGKILL) };
        }
        assert!(left.is_empty(), "left behind (pid, state): {left:?}; the child: {status:?} {rest}");
        assert!(calls.trim().parse::<u32>().is_ok_and(|n| n >= 2), "the second signal was sent from inside the walk: {calls:?}");
        assert_eq!(status.code(), Some(128 + libc::SIGTERM), "it ended as the first signal, after the kill: {rest}");
    }
}
