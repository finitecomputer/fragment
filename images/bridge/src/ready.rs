//! Which of its computer's agents the bridge runs, when the image says so
//! (`BRIDGE_AGENTS_FILE`, docs/bridge.md). A computer's agents may change
//! while it runs (docs/computers.md). An image whose runtime must prepare
//! an agent before its first turn (ours writes a Hermes profile) lists the
//! agents it has made ready in a file, and the bridge runs only those of
//! `GET /api/computer`'s, in the platform's order. Without the setting it
//! runs every agent the platform lists (the stub).
//!
//! The file is `{"agents": ["juniper--k3x9", …], "runtimes": {"rowan--k3x9":
//! "claude"}}`, the host's, written whole and renamed into place, so a read
//! never sees half of one. Each agent named is the bridge's to run, made
//! ready for the runtime `runtimes` names, else the default (Hermes'). One
//! whose `agent.json` names another runtime now stays the bridge's, its
//! turns waiting (`unready`, the engine's `Input::Unready`), until the file
//! says it is ready for that one (FIN-542: a Claude agent's files are
//! restored before its first turn). A missing file is no agent ready. One
//! that does not read keeps the set before it (a host's bug must not stop
//! the agents it had made ready), and says so.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::limits;
use crate::runtime::Agent;

/// The file is at most this many bytes: `AGENTS_MAX` names, each a
/// fragment's name (at most 128 bytes), with room for the JSON.
pub const READY_FILE_MAX_BYTES: usize = 16 * 1024;
/// A name in it is at most this long (a fragment's `<label>--<suffix>`).
pub const READY_NAME_MAX_BYTES: usize = 128;
/// A runtime's name in it is at most this long: any an `agent.json` names
/// (`runtime::named`), whatever it holds.
pub const READY_RUNTIME_MAX_BYTES: usize = crate::runtime::RUNTIME_NAMED_MAX_BYTES;

const _: () = assert!(limits::AGENTS_MAX * (2 * READY_NAME_MAX_BYTES + READY_RUNTIME_MAX_BYTES + 12) < READY_FILE_MAX_BYTES);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    agents: Vec<String>,
    #[serde(default)]
    runtimes: BTreeMap<String, String>,
}

/// The agents a ready file names, and the runtime each is ready for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub agents: BTreeSet<String>,
    runtimes: BTreeMap<String, String>,
}

impl Prepared {
    /// Whether `a` is ready for the runtime its `agent.json` names now.
    pub fn runs(&self, a: &Agent) -> bool {
        let for_runtime = self.runtimes.get(&a.fragment).map_or(crate::runtime::DEFAULT, String::as_str);
        self.agents.contains(&a.fragment) && for_runtime == a.runtime.as_deref().unwrap_or(crate::runtime::DEFAULT)
    }
}

/// The agents a ready file names.
pub fn parse(bytes: &[u8]) -> Result<Prepared, String> {
    if bytes.len() > READY_FILE_MAX_BYTES {
        return Err(format!("{} bytes, past {READY_FILE_MAX_BYTES}", bytes.len()));
    }
    let f: File = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if f.agents.len() > limits::AGENTS_MAX {
        return Err(format!("{} agents; a computer runs at most {}", f.agents.len(), limits::AGENTS_MAX));
    }
    if let Some(bad) = f.agents.iter().find(|a| a.is_empty() || a.len() > READY_NAME_MAX_BYTES || a.chars().any(char::is_whitespace)) {
        return Err(format!("{bad:?} is no agent fragment's name"));
    }
    let agents: BTreeSet<String> = f.agents.into_iter().collect();
    if let Some((a, r)) = f.runtimes.iter().find(|(a, r)| !agents.contains(*a) || r.len() > READY_RUNTIME_MAX_BYTES) {
        return Err(format!("{a:?} is ready for {r:?}: no agent it names, or past {READY_RUNTIME_MAX_BYTES} bytes"));
    }
    Ok(Prepared { agents, runtimes: f.runtimes })
}

/// The platform's agents the file names, in the platform's order.
pub fn gate(agents: Vec<Agent>, ready: &Prepared) -> Vec<Agent> {
    agents.into_iter().filter(|a| ready.agents.contains(&a.fragment)).collect()
}

/// Those of `agents` not ready yet for the runtime they name now: their
/// turns wait.
pub fn unready(agents: &[Agent], ready: &Prepared) -> BTreeSet<String> {
    agents.iter().filter(|a| !ready.runs(a)).map(|a| a.fragment.clone()).collect()
}

/// What tells one version of the file from the next: a rename makes a new
/// inode, and a write in place a new length or time.
type Signature = Option<(u64, u64, i64, i64)>;

/// The ready file, read again when it changes.
pub struct Ready {
    path: PathBuf,
    seen: Signature,
    read_once: bool,
    agents: Prepared,
}

impl Ready {
    pub fn new(path: PathBuf) -> Ready {
        Ready { path, seen: None, read_once: false, agents: Prepared::default() }
    }

    pub fn agents(&self) -> &Prepared {
        &self.agents
    }

    fn signature(&self) -> Signature {
        let m = std::fs::metadata(&self.path).ok()?;
        Some((m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
    }

    /// Reads the file again if it changed since the last look (always on
    /// the first): whether the set of ready agents changed.
    pub fn refresh(&mut self) -> bool {
        let sig = self.signature();
        if self.read_once && sig == self.seen {
            return false;
        }
        self.read_once = true;
        self.seen = sig;
        let next = match sig {
            None => Prepared::default(),
            Some(_) => match std::fs::read(&self.path).map_err(|e| e.to_string()).and_then(|b| parse(&b)) {
                Ok(set) => set,
                Err(e) => {
                    crate::ev!("agents_file.unread", { "path": self.path.display().to_string(), "error": e, "kept": self.agents.agents.len() });
                    return false;
                }
            },
        };
        if next == self.agents {
            return false;
        }
        crate::ev!("agents_file.read", { "ready": next.agents.iter().collect::<Vec<_>>(), "runtimes": next.runtimes });
        self.agents = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &str) -> Agent {
        Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: label.into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None, claude: Default::default() }
    }

    /// Valid: the file names the agents ready; the gate keeps the
    /// platform's order and drops an agent not ready yet, and one the
    /// platform no longer lists.
    #[test]
    fn the_gate_runs_the_ready_agents_in_the_platforms_order() {
        let ready = parse(br#"{"agents": ["maple--k3x9", "juniper--k3x9", "gone--k3x9"]}"#).unwrap();
        let got = gate(vec![agent("juniper"), agent("oak"), agent("maple")], &ready);
        assert_eq!(got.iter().map(|a| a.fragment.as_str()).collect::<Vec<_>>(), ["juniper--k3x9", "maple--k3x9"]);
        assert!(gate(vec![agent("juniper")], &Prepared::default()).is_empty(), "nothing ready, nothing run");
    }

    /// Goal (FIN-542): an agent is run only on the runtime it was made ready
    /// for; otherwise it stays the bridge's, its turns waiting. Invalid:
    /// switched to Claude, an entry made ready for Hermes does not run it on
    /// Claude (its files may not be restored yet), nor one made ready for
    /// Claude on Hermes. Valid: once the file says Claude, it runs. A
    /// runtime's name the file carries is any an `agent.json` gives, so one
    /// odd name never stops the others.
    #[test]
    fn an_agent_runs_only_on_the_runtime_it_was_made_ready_for() {
        let on = |label: &str, runtime: Option<&str>| Agent { runtime: runtime.map(str::to_string), ..agent(label) };
        let for_hermes = parse(br#"{"agents": ["juniper--k3x9"]}"#).unwrap();
        assert!(unready(&[on("juniper", None), on("juniper", Some("hermes"))], &for_hermes).is_empty());
        let switched = [on("juniper", Some("claude"))];
        assert_eq!(gate(switched.to_vec(), &for_hermes).len(), 1, "still the bridge's");
        assert_eq!(unready(&switched, &for_hermes).into_iter().collect::<Vec<_>>(), ["juniper--k3x9"], "switched to Claude: its turns wait");
        let for_claude = parse(br#"{"agents": ["juniper--k3x9"], "runtimes": {"juniper--k3x9": "claude"}}"#).unwrap();
        assert!(unready(&switched, &for_claude).is_empty(), "ready for Claude: it runs");
        assert_eq!(unready(&[on("juniper", None)], &for_claude).len(), 1, "back on Hermes: its profile first");
        let odd = parse(br#"{"agents": ["juniper--k3x9", "oak--k3x9"], "runtimes": {"oak--k3x9": "Claude Code! "}}"#).unwrap();
        assert!(unready(&[on("juniper", None), on("oak", Some("Claude Code! "))], &odd).is_empty(), "an odd runtime's name stops no one");
        let long = format!(r#"{{"agents": ["oak--k3x9"], "runtimes": {{"oak--k3x9": "{}"}}}}"#, "x".repeat(READY_RUNTIME_MAX_BYTES + 1));
        for bad in [&br#"{"agents": [], "runtimes": {"juniper--k3x9": "claude"}}"#[..], long.as_bytes()] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// Invalid: past its bounds, not the shape, or names that are no
    /// fragment's.
    #[test]
    fn a_file_out_of_shape_is_refused() {
        let many: Vec<String> = (0..=limits::AGENTS_MAX).map(|i| format!("a{i}--k3x9")).collect();
        let too_many = serde_json::to_vec(&serde_json::json!({ "agents": many })).unwrap();
        for bad in [&b"not json"[..], br#"{"agents": "juniper--k3x9"}"#, br#"{"agents": [], "more": 1}"#, br#"{"agents": [""]}"#, br#"{"agents": ["a b"]}"#, &too_many[..]] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(parse(&vec![b' '; READY_FILE_MAX_BYTES + 1]).is_err());
    }

    /// Replay and restart: the file is read again only when it changes; a
    /// rewrite with the same agents changes nothing; one that does not read
    /// keeps the set before it; a removed file is no agent ready; a new
    /// reader (a restarted bridge) reads it whole at once.
    #[test]
    fn the_file_is_read_again_when_it_changes() {
        crate::log::set_quiet(true);
        let dir = std::env::temp_dir().join(format!("bridge-ready-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agents.json");
        let write = |text: &str| {
            let tmp = dir.join("agents.json.tmp");
            std::fs::write(&tmp, text).unwrap();
            std::fs::rename(&tmp, &path).unwrap();
        };
        let mut r = Ready::new(path.clone());
        assert!(!r.refresh() && r.agents().agents.is_empty(), "no file: no agent ready");
        write(r#"{"agents": ["juniper--k3x9"]}"#);
        assert!(r.refresh());
        assert_eq!(r.agents().agents.iter().collect::<Vec<_>>(), ["juniper--k3x9"]);
        assert!(!r.refresh(), "unchanged: not read again");
        write(r#"{"agents": ["juniper--k3x9"]}"#);
        assert!(!r.refresh(), "the same agents again: no change");
        write(r#"{"agents": ["juniper--k3x9", "maple--k3x9"]}"#);
        assert!(r.refresh() && r.agents().agents.len() == 2);
        write("{half");
        assert!(!r.refresh() && r.agents().agents.len() == 2, "a file that does not read keeps the set before it");
        let mut restarted = Ready::new(path.clone());
        write(r#"{"agents": ["maple--k3x9"]}"#);
        assert!(restarted.refresh() && restarted.agents().agents.iter().collect::<Vec<_>>() == ["maple--k3x9"]);
        // the same agents, made ready for another runtime: a change
        write(r#"{"agents": ["maple--k3x9"], "runtimes": {"maple--k3x9": "claude"}}"#);
        assert!(restarted.refresh(), "made ready for Claude now");
        std::fs::remove_file(&path).unwrap();
        assert!(restarted.refresh() && restarted.agents().agents.is_empty(), "removed: no agent ready");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
