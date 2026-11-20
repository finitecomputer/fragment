//! The computer's agents while it is awake (docs/computers.md: a computer's
//! agents may change while it runs), as pure functions of what the boot
//! runs and what `GET /api/computer` says now. A change is carried out with
//! no restart of anything (main.rs, `follow_agents`): Hermes v0.21.5's
//! multiplexed gateway serves a profile written after it started (its
//! `rescan-profiles` control verb, and every relayed turn resolving its
//! profile's directory as it arrives), and the bridge runs an agent once
//! the ready file names it (the bridge's `ready.rs`). So no turn of
//! another agent is ever cut by a change, and none waits for one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fragment_bridge::runtime::relay::wire;
use fragment_bridge::runtime::{claude, Agent};
use serde_json::{json, Value};

use crate::sync::Place;

/// What changed between the agents the boot runs and the agents the
/// platform lists now, each by its fragment (its profile's name).
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    /// New to this computer: each needs its profile before the bridge runs it.
    pub added: Vec<Agent>,
    /// No longer on this computer: the bridge stops running it at once; its
    /// profile is retired at the computer's next start, as a boot retires
    /// one (moving it while Hermes winds down a turn of its could leave a
    /// half profile behind).
    pub removed: Vec<Agent>,
    /// Still here, with other credentials (a connection made or lost, an
    /// operator key offered, its owner's narrowing): its `.env` and its
    /// credentials file are written again, nothing restarted.
    pub credentials: Vec<Agent>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.credentials.is_empty()
    }
}

/// The change from `running` to `read`. The same fragment is the same
/// agent: a profile is named by its fragment, and what else the platform
/// says of it (its identity, its name) the bridge reads for itself, but
/// for its credentials, which are its profile's.
pub fn diff(running: &[Agent], read: &[Agent]) -> Change {
    let had: BTreeSet<&str> = running.iter().map(|a| a.fragment.as_str()).collect();
    let has: BTreeSet<&str> = read.iter().map(|a| a.fragment.as_str()).collect();
    assert_eq!(has.len(), read.len(), "the platform lists an agent once");
    Change {
        added: read.iter().filter(|a| !had.contains(a.fragment.as_str())).cloned().collect(),
        removed: running.iter().filter(|a| !has.contains(a.fragment.as_str())).cloned().collect(),
        credentials: read.iter().filter(|a| running.iter().any(|r| r.fragment == a.fragment && r.credentials != a.credentials)).cloned().collect(),
    }
}

/// The agents Hermes runs: each whose `agent.json` names no runtime or
/// Hermes' (`runtime::DEFAULT`). Only these have a profile, a desktop and a
/// route; another's turns go to the runtime it names, and a runtime this
/// bridge does not run refuses them (docs/computers.md).
pub fn hermes(agents: &[Agent]) -> Vec<Agent> {
    agents.iter().filter(|a| of_hermes(a)).cloned().collect()
}

/// One of the agents Hermes runs (`hermes`).
pub fn of_hermes(a: &Agent) -> bool {
    a.runtime.as_deref().is_none_or(|r| r == fragment_bridge::runtime::DEFAULT)
}

/// Where an agent's own files are synced with its agent fragment (sync.rs):
/// a Hermes agent's profile; a Claude agent's working folder (the bridge's,
/// `<work>/<its fragment>`) and Claude Code's memory for it, in the
/// person's `.claude` in `home` (FIN-542); another runtime's, nowhere.
pub fn places(a: &Agent, home: &Path) -> Vec<Place> {
    if of_hermes(a) {
        return vec![Place::hermes(&crate::hermes::profile_dir(home, &a.fragment))];
    }
    if a.runtime.as_deref() != Some(claude::NAME) {
        return vec![];
    }
    let work = Path::new(crate::hermes::WORK).join(&a.fragment);
    Place::claude(&work, &claude::memory_dir(&claude::config_dir(home), &work), home).into()
}

/// The agents to run now, as `read`, but for one newly Hermes' (switched
/// back, or assigned again) while a restore of its files is still running
/// (`running`, by fragment): it stays as it was `before` (another
/// runtime's, its files not restored for it, so ready for none and its
/// turns held), or not here yet if it was not, until a read after that
/// restore ended. Its profile, written and synced as it becomes Hermes',
/// waits so (one sync of an agent's files at a time); every other agent
/// goes on.
pub fn until_restored(before: &[Agent], read: Vec<Agent>, running: &BTreeSet<String>) -> Vec<Agent> {
    read.into_iter()
        .filter_map(|a| {
            let waits = of_hermes(&a) && running.contains(&a.fragment) && !before.iter().any(|b| b.fragment == a.fragment && of_hermes(b));
            if !waits {
                return Some(a);
            }
            before.iter().find(|b| b.fragment == a.fragment).cloned()
        })
        .collect()
}

/// The runtime an agent's `agent.json` names, as the bridge reads it.
pub fn runtime_of(a: &Agent) -> &str {
    a.runtime.as_deref().unwrap_or(fragment_bridge::runtime::DEFAULT)
}

/// The agents ready for a runtime, each by its fragment and that runtime:
/// one of Hermes' whose profile the gateway serves, any other whose own
/// files were restored for it (`places`).
pub type Restored = BTreeSet<(String, String)>;

/// The gateway's served profiles the boot reads, at most: one an agent of
/// the computer's (at most `AGENTS_MAX`), and its default.
pub const SERVED_MAX: usize = fragment_bridge::limits::AGENTS_MAX + 1;

pub fn restored_key(a: &Agent) -> (String, String) {
    (a.fragment.clone(), runtime_of(a).to_string())
}

/// The bridge's ready file (its `ready.rs`): every agent it may run, in the
/// platform's order, each named with the runtime it is ready for
/// (`restored`): one of Hermes' once the gateway serves its profile, any
/// other once its own files are restored for the runtime it names
/// (FIN-542). Until then it is ready for none, and the bridge holds its
/// turns, never dropping them.
pub fn ready_file(agents: &[Agent], restored: &Restored) -> String {
    let runtimes: BTreeMap<&str, &str> = agents.iter().filter(|a| restored.contains(&restored_key(a))).map(|a| (a.fragment.as_str(), runtime_of(a))).collect();
    json!({ "agents": agents.iter().map(|a| a.fragment.as_str()).collect::<Vec<_>>(), "runtimes": runtimes }).to_string()
}

/// The profiles of `agents` the gateway's answer to `rescan-profiles` does
/// not list as served (`served_profiles`). An answer that is pending (the
/// gateway still starting, or a rescan that outlasted its five seconds),
/// or has no list, names none: every relayed turn finds its profile's
/// directory as it arrives anyway, and Hermes rescans every 30 s.
pub fn unserved(agents: &[Agent], answer: &Value) -> Vec<String> {
    if answer["pending"] == true {
        return vec![];
    }
    let Some(served) = answer["served_profiles"].as_array() else { return vec![] };
    agents.iter().map(|a| wire::profile(&a.fragment)).filter(|p| !served.iter().any(|s| s == p.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &str) -> Agent {
        Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: label.into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None, claude: Default::default() }
    }

    fn names(agents: &[Agent]) -> Vec<&str> {
        agents.iter().map(|a| a.fragment.as_str()).collect()
    }

    /// Valid: an agent assigned while it runs is added, one unassigned is
    /// removed (each agent's screen comes and goes with it: the screens
    /// file is written with the ready file).
    #[test]
    fn a_change_is_what_was_added_and_removed() {
        let c = diff(&[agent("juniper")], &[agent("juniper"), agent("maple")]);
        assert_eq!((names(&c.added), names(&c.removed)), (vec!["maple--k3x9"], vec![]));
        let c = diff(&[agent("juniper"), agent("maple")], &[agent("maple")]);
        assert_eq!((names(&c.added), names(&c.removed)), (vec![], vec!["juniper--k3x9"]));
        // the first run: a computer started before its first agent was made
        let c = diff(&[], &[agent("juniper")]);
        assert_eq!(names(&c.added), vec!["juniper--k3x9"]);
        let c = diff(&[agent("juniper")], &[]);
        assert_eq!(names(&c.removed), vec!["juniper--k3x9"]);
    }

    /// Goal (FIN-258): Hermes runs an agent naming no runtime or its own,
    /// and only those have a profile; every agent is in the ready file, each
    /// ready for its runtime once its own files are restored for it
    /// (FIN-542). Valid: one switched to Claude leaves Hermes as a removed
    /// agent does, and is ready for Claude only once its files are restored
    /// for it; one switched back is added. Invalid: a runtime no one runs is not
    /// Hermes'. Replay: the same runtimes again are no change.
    #[test]
    fn hermes_runs_only_the_agents_that_name_it() {
        let named = |label: &str, runtime: Option<&str>| Agent { runtime: runtime.map(str::to_string), ..agent(label) };
        let set = [named("juniper", None), named("maple", Some("hermes")), named("rowan", Some("claude")), named("oak", Some("nobody-runs-this"))];
        assert_eq!(names(&hermes(&set)), vec!["juniper--k3x9", "maple--k3x9"]);
        let restored: Restored = set.iter().map(restored_key).collect();
        assert_eq!(ready_file(&set, &restored), r#"{"agents":["juniper--k3x9","maple--k3x9","rowan--k3x9","oak--k3x9"],"runtimes":{"juniper--k3x9":"hermes","maple--k3x9":"hermes","oak--k3x9":"nobody-runs-this","rowan--k3x9":"claude"}}"#, "every agent the bridge may run, on its runtime");

        let switched = [named("juniper", Some("claude")), named("maple", Some("hermes"))];
        let c = diff(&hermes(&set[..2]), &hermes(&switched));
        assert_eq!((names(&c.added), names(&c.removed)), (vec![], vec!["juniper--k3x9"]), "to Claude: its profile goes");
        assert_eq!(ready_file(&switched, &restored), r#"{"agents":["juniper--k3x9","maple--k3x9"],"runtimes":{"maple--k3x9":"hermes"}}"#, "to Claude: ready for none until its files are restored");
        let ready = Restored::from([("juniper--k3x9".to_string(), "claude".to_string()), ("maple--k3x9".to_string(), "hermes".to_string())]);
        assert_eq!(ready_file(&switched, &ready), r#"{"agents":["juniper--k3x9","maple--k3x9"],"runtimes":{"juniper--k3x9":"claude","maple--k3x9":"hermes"}}"#, "then ready for Claude");
        let c = diff(&hermes(&switched), &hermes(&set[..2]));
        assert_eq!((names(&c.added), names(&c.removed)), (vec!["juniper--k3x9"], vec![]), "back to Hermes: its profile again");
        assert!(diff(&hermes(&set), &hermes(&set)).is_empty(), "replay");
    }

    /// Replay: the same set read again is no change, nor is an agent the
    /// platform describes anew (its fragment is its profile), nor another
    /// order (each agent's screen is its own, whichever is first).
    #[test]
    fn the_same_agents_again_are_no_change() {
        let set = [agent("juniper"), agent("maple")];
        assert!(diff(&set, &set).is_empty());
        assert!(diff(&[], &[]).is_empty());
        let mut renamed = agent("juniper");
        renamed.name = "Juniper".into();
        assert!(diff(&[agent("juniper")], &[renamed]).is_empty());
        let c = diff(&[agent("juniper"), agent("maple"), agent("oak")], &[agent("juniper"), agent("oak"), agent("maple")]);
        assert!(c.is_empty(), "{c:?}");
        let c = diff(&[agent("juniper"), agent("maple")], &[agent("maple"), agent("juniper")]);
        assert!(c.is_empty(), "a new first agent changes nothing: {c:?}");
    }

    /// Valid and replay: an agent whose credentials changed is named so,
    /// added and removed ones are not; the same credentials again are no
    /// change.
    #[test]
    fn a_change_of_credentials_rewrites_the_profile_alone() {
        let with = |label: &str, placeholder: &str| {
            let mut a = agent(label);
            a.credentials = vec![fragment_bridge::runtime::Credential { provider: "google".into(), kind: "connection".into(), env: vec!["GOOGLE_OAUTH_ACCESS_TOKEN".into()], placeholder: placeholder.into(), hosts: vec!["www.googleapis.com".into()], model_base: None }];
            a
        };
        let c = diff(&[agent("juniper"), agent("maple")], &[with("juniper", "fcx_google_a"), agent("maple")]);
        assert_eq!((names(&c.credentials), c.added.is_empty(), c.removed.is_empty()), (vec!["juniper--k3x9"], true, true), "connected");
        let c = diff(&[with("juniper", "fcx_google_a")], &[agent("juniper")]);
        assert_eq!(names(&c.credentials), vec!["juniper--k3x9"], "disconnected");
        assert!(diff(&[with("juniper", "fcx_google_a")], &[with("juniper", "fcx_google_a")]).is_empty());
        let c = diff(&[agent("juniper")], &[agent("juniper"), with("oak", "fcx_google_b")]);
        assert!(c.credentials.is_empty() && names(&c.added) == vec!["oak--k3x9"], "a new agent's profile is written whole anyway");
    }

    /// Goal (FIN-542): a Claude agent's own files sync too, where its
    /// runtime keeps them: its working folder and Claude Code's memory for
    /// it, never a profile; a Hermes agent's in its profile; a runtime no
    /// one runs, nowhere.
    #[test]
    fn each_runtime_keeps_its_own_files_where_it_reads_them() {
        let named = |label: &str, runtime: Option<&str>| Agent { runtime: runtime.map(str::to_string), ..agent(label) };
        let home = Path::new("/data/hermes");
        assert_eq!(places(&named("juniper", None), home), vec![Place::hermes(Path::new("/data/hermes/profiles/juniper--k3x9"))]);
        let rowan = places(&named("rowan", Some("claude")), home);
        let roots: Vec<_> = rowan.iter().map(|p| p.root.to_string_lossy().into_owned()).collect();
        assert_eq!(roots, ["/data/work/rowan--k3x9", "/data/hermes/.claude/projects/-data-work-rowan--k3x9/memory"]);
        assert!(places(&named("oak", Some("nobody-runs-this")), home).is_empty());
    }

    /// Goal (FIN-542): an agent switched back to Hermes while a restore of
    /// its Claude files runs waits alone: it stays Claude's (ready for none,
    /// its turns held), and no profile of it is written, until that restore
    /// ended; an agent assigned meanwhile, and one switched to Claude, go
    /// on. Valid: once the restore ended, it is Hermes' and its profile is
    /// written. Invalid: one coming back as Hermes' while a stale restore
    /// of its runs is not here yet.
    #[test]
    fn a_switch_back_to_hermes_waits_alone_for_its_restore() {
        let named = |label: &str, runtime: Option<&str>| Agent { runtime: runtime.map(str::to_string), ..agent(label) };
        let before = [named("rowan", Some("claude")), named("maple", None), named("hazel", Some("claude"))];
        let read = vec![named("rowan", None), named("maple", Some("claude")), named("hazel", Some("claude")), named("oak", None)];
        let running = BTreeSet::from(["rowan--k3x9".to_string(), "maple--k3x9".to_string()]);
        let now = until_restored(&before, read.clone(), &running);
        assert_eq!(now.iter().map(|a| (a.fragment.as_str(), runtime_of(a))).collect::<Vec<_>>(), [("rowan--k3x9", "claude"), ("maple--k3x9", "claude"), ("hazel--k3x9", "claude"), ("oak--k3x9", "hermes")]);
        let c = diff(&hermes(&before), &hermes(&now));
        assert_eq!((names(&c.added), names(&c.removed)), (vec!["oak--k3x9"], vec!["maple--k3x9"]), "oak's profile now, maple leaves Hermes; rowan's waits");
        let restored = Restored::from([("hazel--k3x9".to_string(), "claude".to_string()), ("oak--k3x9".to_string(), "hermes".to_string())]);
        let file = fragment_bridge::ready::parse(ready_file(&now, &restored).as_bytes()).unwrap();
        assert_eq!(fragment_bridge::ready::unready(&read, &file).into_iter().collect::<Vec<_>>(), ["maple--k3x9", "rowan--k3x9"], "rowan held as Hermes', maple until restored; hazel and oak (its profile served) run");
        // its restore ended: Hermes' now, its profile written
        let later = until_restored(&now, read.clone(), &BTreeSet::from(["maple--k3x9".to_string()]));
        assert_eq!(names(&diff(&hermes(&now), &hermes(&later)).added), vec!["rowan--k3x9"]);
        // a stale restore of one not here before: not here yet
        assert!(until_restored(&[], vec![named("rowan", None)], &BTreeSet::from(["rowan--k3x9".to_string()])).is_empty());
    }

    #[test]
    #[should_panic(expected = "the platform lists an agent once")]
    fn an_agent_listed_twice_is_a_platform_bug() {
        diff(&[], &[agent("juniper"), agent("juniper")]);
    }

    /// Goal (FIN-542): every agent is named, each ready for its runtime only
    /// once it is: a Hermes agent once the gateway serves its profile (a
    /// turn relayed before is refused, and lost), another once its files
    /// are restored.
    #[test]
    fn the_ready_file_names_every_agent_in_order() {
        let served = Restored::from([restored_key(&agent("maple"))]);
        assert_eq!(ready_file(&[agent("maple"), agent("juniper")], &served), r#"{"agents":["maple--k3x9","juniper--k3x9"],"runtimes":{"maple--k3x9":"hermes"}}"#, "juniper's profile not served yet: ready for none");
        assert_eq!(ready_file(&[], &Restored::new()), r#"{"agents":[],"runtimes":{}}"#);
        // what the bridge reads of it: each agent ready for its runtime
        let rowan = Agent { runtime: Some("claude".into()), ..agent("rowan") };
        let file = ready_file(&[agent("maple"), rowan.clone()], &Restored::from([restored_key(&rowan), restored_key(&agent("maple"))]));
        let read = fragment_bridge::ready::parse(file.as_bytes()).unwrap();
        assert!(read.runs(&agent("maple")) && read.runs(&rowan) && !read.runs(&agent("rowan")));
        let file = ready_file(&[agent("maple"), rowan.clone()], &served);
        let read = fragment_bridge::ready::parse(file.as_bytes()).unwrap();
        assert_eq!(fragment_bridge::ready::unready(&[agent("maple"), rowan.clone()], &read).into_iter().collect::<Vec<_>>(), ["rowan--k3x9"], "not restored: its turns wait");
        // switched to Hermes before it was ever restored: not ready for
        // Hermes either until a file written after its profile says so
        assert!(!read.runs(&agent("rowan")), "no entry is ready for no runtime");
    }

    /// The gateway's answer says whether it serves the new profiles; an
    /// answer without the list (pending, or not multiplexed) says nothing.
    #[test]
    fn a_rescan_answer_names_what_it_does_not_serve() {
        let answer = json!({ "multiplex": true, "added": ["maple--k3x9"], "removed": [], "served_profiles": ["default", "juniper--k3x9", "maple--k3x9"] });
        assert!(unserved(&[agent("juniper"), agent("maple")], &answer).is_empty());
        let answer = json!({ "multiplex": true, "served_profiles": ["default", "juniper--k3x9"] });
        assert_eq!(unserved(&[agent("juniper"), agent("maple")], &answer), vec!["maple--k3x9"]);
        assert!(unserved(&[agent("maple")], &json!({ "multiplex": true, "pending": true, "served_profiles": ["default"] })).is_empty());
        assert!(unserved(&[agent("maple")], &json!({ "multiplex": true })).is_empty());
    }
}
