//! A process and every process descended from it, killed as one (FIN-457).
//!
//! A process group does not hold everything a node starts: miniflare's
//! Browser Rendering binding starts Chrome detached (`setsid`, the default
//! of @puppeteer/browsers off Windows), in a session and group of its own.
//! A SIGKILL to the node's group leaves that Chrome, its zygotes and
//! renderers running, handed to init, and only a graceful wrangler exit
//! (its `exit` hook) ever closed it. Descent holds where groups do not:
//! each process found is stopped (SIGSTOP) before its children are looked
//! for, so none starts one unseen, then all of them are SIGKILLed.

use std::collections::BTreeSet;
use std::io;
use std::process::{Command, Stdio};

/// Processes one tree may hold: a node (wrangler, workerd, a few Chromes
/// with their zygotes and renderers) is a few dozen, a run a few hundred.
const TREE_MAX: usize = 4096;

/// Each running process and its parent, as `ps` lists them: the `ps` that
/// lists them left out.
fn parents() -> io::Result<Vec<(u32, u32)>> {
    let child = Command::new("ps").args(["-A", "-o", "pid=", "-o", "ppid="]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let lister = child.id();
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("ps exited {}", out.status)));
    }
    let pair = |line: &str| {
        let mut words = line.split_whitespace();
        Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?))
    };
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(pair).filter(|&(pid, _)| pid != lister).collect())
}

fn signal(pid: u32, signal: libc::c_int) {
    let pid = libc::pid_t::try_from(pid).expect("a pid fits a pid_t");
    // SAFETY: kill(2) on a positive pid signals that process alone; one
    // already gone answers ESRCH, which is what was wanted
    unsafe { libc::kill(pid, signal) };
}

/// SIGKILLs every process descended from `root`, and `root` too unless it
/// is this process (which kills what it started, and goes on). The caller
/// vouches that `root` is still its own: this process, or a child of it not
/// yet reaped, whose pid the kernel cannot have given another. Returns how
/// many it killed (`root` among them).
pub fn kill(root: u32) -> io::Result<usize> {
    let me = std::process::id();
    let mut tree = BTreeSet::from([root]);
    let mut stopped = BTreeSet::new();
    // bounded: each round finds a process not yet in the tree, or ends
    loop {
        for &pid in &tree {
            if pid != me && stopped.insert(pid) {
                signal(pid, libc::SIGSTOP);
            }
        }
        let found: Vec<u32> = parents()?.into_iter().filter(|(pid, parent)| tree.contains(parent) && !tree.contains(pid)).map(|(pid, _)| pid).collect();
        if found.is_empty() {
            break;
        }
        tree.extend(found);
        assert!(tree.len() <= TREE_MAX, "a tree holds at most {TREE_MAX} processes");
    }
    for &pid in &stopped {
        signal(pid, libc::SIGKILL);
    }
    Ok(stopped.len())
}
