//! The team folder (docs/computers.md, "The team folder in our Hermes
//! image"): since a computer per agent, no two of a person's agents share a
//! disk, so each keeps `~/shared` in its work in step with its owner's team
//! folder (their fragment of kind `shared`), through the CLI's own `fragment
//! sync --watch`, one process an agent, as that agent, for its owner. The
//! sync does the rest by its own rules (a change sent within moments, a
//! teammate's arriving through the fragment's change feed, a conflict kept
//! as a copy, a mass delete refused). Hermes sees a folder: no habit to
//! learn (Paul, 2026-11-19).
//!
//! A hold stops every sync (SIGSTOP) before the boot answers it, and lets
//! them go on after (SIGCONT): nothing under `/data` changes while a save
//! is taken (docs/computers.md, "What changes under the hold").

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use fragment_bridge::api::FragmentEntry;
use fragment_bridge::ev;

/// The folder in an agent's work its team folder is kept in.
pub const DIR: &str = "shared";
/// A sync that ended (its fragment unread, a mass delete it refused) starts
/// again at most this often: never a loop of starts.
pub const RESTART_MS: u64 = 60_000;

/// The owner's team folder among the fragments an agent reaches acting for
/// them: kind `shared`, theirs (one someone else shared is not), one
/// labelled `shared` first (the shell makes it so), else the first by name,
/// as their skills fragment is found (skills.rs, `pick`).
pub fn pick(fragments: &[FragmentEntry]) -> Option<String> {
    let mut theirs: Vec<&str> = fragments.iter().filter(|f| f.kind == "shared" && f.owned).map(|f| f.name.as_str()).collect();
    theirs.sort_unstable();
    let labelled = theirs.iter().find(|n| n.rsplit_once("--").is_some_and(|(label, _)| label == DIR));
    labelled.or(theirs.first()).map(|n| n.to_string())
}

/// An agent's `~/shared`.
pub fn dir_of(agent_fragment: &str) -> PathBuf {
    crate::hermes::work_dir(agent_fragment).join(DIR)
}

/// One agent's sync: of which fragment, its process, and when it started.
struct Running {
    fragment: String,
    child: Option<Child>,
    started: Instant,
}

/// The syncs this computer runs, one an agent, and whether a hold stopped
/// them.
#[derive(Default)]
pub struct Syncs {
    running: BTreeMap<String, Running>,
    held: bool,
}

impl Syncs {
    /// Runs `want` (agent → team folder) and nothing else: a sync of an agent
    /// gone, or of another fragment, ends; one missing starts (`spawn`), as
    /// does one that ended, at most once a `RESTART_MS`. Started while held,
    /// it is stopped at once, as the others are.
    pub fn keep(&mut self, want: &BTreeMap<String, String>, spawn: &dyn Fn(&str, &str) -> std::io::Result<Child>) {
        let gone: Vec<String> = self.running.iter().filter(|(a, r)| want.get(*a) != Some(&r.fragment)).map(|(a, _)| a.clone()).collect();
        for agent in gone {
            if let Some(mut r) = self.running.remove(&agent) {
                end(&agent, r.child.as_mut());
            }
        }
        for (agent, fragment) in want {
            let ended = match self.running.get_mut(agent) {
                Some(r) => r.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) && r.started.elapsed() >= Duration::from_millis(RESTART_MS),
                None => true,
            };
            if !ended {
                continue;
            }
            let child = match spawn(agent, fragment) {
                Ok(c) => {
                    ev!("shared.started", { "agent": agent, "fragment": fragment, "pid": c.id() });
                    Some(c)
                }
                Err(e) => {
                    ev!("shared.failed", { "agent": agent, "fragment": fragment, "error": e.to_string() });
                    None
                }
            };
            if let (true, Some(c)) = (self.held, &child) {
                signal(c.id(), libc::SIGSTOP);
            }
            self.running.insert(agent.clone(), Running { fragment: fragment.clone(), child, started: Instant::now() });
        }
        assert!(self.running.len() <= want.len(), "one sync an agent, and none of an agent gone");
    }

    /// Whether a hold stopped them.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Stops every sync for a hold (`true`), or lets them go on after it.
    pub fn hold(&mut self, held: bool) {
        if self.held == held {
            return;
        }
        self.held = held;
        let sig = if held { libc::SIGSTOP } else { libc::SIGCONT };
        for r in self.running.values() {
            if let Some(c) = &r.child {
                signal(c.id(), sig);
            }
        }
        ev!("shared.held", { "held": held, "syncs": self.running.len() });
    }
}

fn signal(pid: u32, sig: libc::c_int) {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return };
    // SAFETY: kill(2) on a child of this process, by its pid
    unsafe {
        libc::kill(pid, sig);
    }
}

/// Ends an agent's sync: its folder stays as the sync left it.
fn end(agent: &str, child: Option<&mut Child>) {
    if let Some(c) = child {
        // a stopped process takes no SIGKILL's notice until it runs again
        signal(c.id(), libc::SIGCONT);
        let _ = c.kill();
        let _ = c.wait();
    }
    ev!("shared.ended", { "agent": agent });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: &str, owned: bool) -> FragmentEntry {
        FragmentEntry { name: name.into(), role: "editor".into(), kind: kind.into(), owned, title: String::new(), identity: None, agent_role: None }
    }

    /// Goal: the owner's own team folder is found as their skills are: theirs,
    /// of kind `shared`, labelled `shared` first. Invalid: one shared with
    /// them, another kind, none at all.
    #[test]
    fn the_owners_team_folder_is_found() {
        let list = [entry("notes--k3x9", "app", true), entry("work--p2m4", "shared", true), entry("shared--z9z9", "shared", true), entry("shared--a1a1", "shared", false)];
        assert_eq!(pick(&list).as_deref(), Some("shared--z9z9"), "labelled shared first, theirs");
        assert_eq!(pick(&list[..2]).as_deref(), Some("work--p2m4"), "else the first by name");
        assert_eq!(pick(&[entry("shared--a1a1", "shared", false), entry("shared--b2b2", "skills", true)]), None);
        assert_eq!(dir_of("juniper--k3x9"), PathBuf::from("/data/work/juniper--k3x9/shared"));
    }

    /// Goal: one sync an agent, of the folder wanted; an agent gone, or a
    /// folder changed, ends its sync; one that ended starts again no sooner
    /// than `RESTART_MS`; a hold stops each, one started while held too.
    /// Method: `sleep` stands in for the CLI.
    #[test]
    fn one_sync_an_agent_and_a_hold_stops_them() {
        let spawned = std::cell::RefCell::new(vec![]);
        let spawn = |agent: &str, fragment: &str| {
            spawned.borrow_mut().push((agent.to_string(), fragment.to_string()));
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        let mut want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string()), ("maple--p2m4".to_string(), "shared--z9z9".to_string())]);
        syncs.keep(&want, &spawn);
        syncs.keep(&want, &spawn);
        assert_eq!(spawned.borrow().len(), 2, "one each, kept");
        let state = |pid: u32| std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().split_whitespace().nth(2).unwrap_or("").to_string();
        let pids: Vec<u32> = syncs.running.values().filter_map(|r| r.child.as_ref().map(Child::id)).collect();
        syncs.hold(true);
        std::thread::sleep(Duration::from_millis(100));
        assert!(pids.iter().all(|p| state(*p) == "T"), "stopped under the hold");
        // an agent made meanwhile starts stopped
        want.insert("rowan--q3q3".into(), "shared--z9z9".into());
        syncs.keep(&want, &spawn);
        let rowan = syncs.running["rowan--q3q3"].child.as_ref().map(Child::id).expect("started");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(state(rowan), "T", "started under a hold, stopped at once");
        syncs.hold(false);
        std::thread::sleep(Duration::from_millis(100));
        assert!(pids.iter().chain([&rowan]).all(|p| state(*p) == "S"), "going on after it");
        // maple leaves; juniper's folder changes
        want.remove("maple--p2m4");
        want.insert("juniper--k3x9".into(), "team--w4w4".into());
        syncs.keep(&want, &spawn);
        assert_eq!(syncs.running.len(), 2);
        assert_eq!(syncs.running["juniper--k3x9"].fragment, "team--w4w4");
        assert_eq!(spawned.borrow().last(), Some(&("juniper--k3x9".to_string(), "team--w4w4".to_string())));
        // one that ended waits RESTART_MS before it starts again
        let before = spawned.borrow().len();
        if let Some(c) = syncs.running.get_mut("rowan--q3q3").and_then(|r| r.child.as_mut()) {
            let _ = c.kill();
            let _ = c.wait();
        }
        syncs.keep(&want, &spawn);
        assert_eq!(spawned.borrow().len(), before, "not again at once");
        syncs.running.get_mut("rowan--q3q3").expect("kept").started -= Duration::from_millis(RESTART_MS);
        syncs.keep(&want, &spawn);
        assert_eq!(spawned.borrow().len(), before + 1, "again after RESTART_MS");
        syncs.keep(&BTreeMap::new(), &spawn);
        assert!(syncs.running.is_empty());
    }
}
