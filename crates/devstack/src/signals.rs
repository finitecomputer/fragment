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

/// Runs `then` with the signal's number on a thread of its own when the
/// process gets one of the termination signals it does not ignore, then
/// exits as that signal would have it (128 + its number). A second signal
/// meanwhile ends the process at once, by that signal's own default.
/// Once per process.
pub fn on_termination(then: impl FnOnce(i32) + Send + 'static) -> Result<()> {
    ensure!(!HANDLED.swap(true, Ordering::SeqCst), "termination is handled once per process");
    let handled: Vec<libc::c_int> = TERMINATION.into_iter().filter(|&signal| !ignored(signal)).collect();
    // set as the first arrives: from then on each takes its default action
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
    fn spawn(test: &str, what: &str, ignoring_hup: bool) -> (Child, BufReader<std::process::ChildStdout>, String) {
        let mut cmd = Command::new(std::env::current_exe().expect("the test binary"));
        cmd.args(["--exact", test, "--nocapture", "--test-threads=1"]).env(CHILD, what).stdout(Stdio::piped());
        if ignoring_hup {
            // SAFETY: signal(2) between fork and exec, async-signal-safe
            unsafe { cmd.pre_exec(|| if libc::signal(libc::SIGHUP, libc::SIG_IGN) == libc::SIG_ERR { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
        }
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
}
