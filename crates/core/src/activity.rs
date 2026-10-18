//! What a computer's agents are doing, as its guest says it on its
//! keepalive (docs/computers.md, "What its agents are doing"; decision 62,
//! proposed). The guest is less trusted than the cell: its report is read
//! as untrusted input, checked whole, and taken or refused whole. What it
//! says is shown to the computer's owner and grants nothing.

use std::collections::BTreeSet;

use fragment_proto::computer::{ActivityEntry, ActivityReport};

/// Entries a report holds at most (the guest counts the rest in `more`).
pub const ACTIVITY_MAX: usize = 64;
/// A report's frame, at most, read before it is parsed.
pub const ACTIVITY_REPORT_MAX_BYTES: usize = 16 * 1024;
/// The least time between two reports that are not empty, which a guest
/// that keeps time never breaks (it sends at most one a second).
pub const ACTIVITY_SPACING_MS: i64 = 1_000;
/// The report's version this cell reads.
pub const ACTIVITY_VERSION: u32 = 1;

/// Why a report is refused: its keepalive is closed (1008), and what the
/// computer keeps does not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityRefusal {
    /// Past `ACTIVITY_REPORT_MAX_BYTES`.
    TooLarge,
    /// Not a report this cell reads: no JSON, another `type` or `v`, a
    /// field it does not know.
    Malformed(String),
    /// Past `ACTIVITY_MAX` entries.
    TooMany,
    /// An agent this computer does not run.
    NotHere(String),
    /// A chat that is no fragment's name, or a `waitsOn` that is no
    /// identity, or one on a state that waits on no one.
    BadEntry(String),
    /// One agent and chat twice.
    Twice(String),
}

impl ActivityRefusal {
    pub fn message(&self) -> String {
        match self {
            ActivityRefusal::TooLarge => format!("an activity report is at most {ACTIVITY_REPORT_MAX_BYTES} bytes"),
            ActivityRefusal::Malformed(e) => format!("not an activity report (type \"activity\", v {ACTIVITY_VERSION}): {e}"),
            ActivityRefusal::TooMany => format!("an activity report holds at most {ACTIVITY_MAX} entries (count the rest in `more`)"),
            ActivityRefusal::NotHere(agent) => format!("this computer does not run {agent}"),
            ActivityRefusal::BadEntry(why) => format!("an activity entry: {why}"),
            ActivityRefusal::Twice(key) => format!("{key} is reported twice"),
        }
    }
}

/// Reads and checks a report: every agent one of `runs` (the agent
/// fragments this computer runs now), each agent and chat once. Its
/// entries come back ordered by agent and chat, so the same report said
/// twice is the same.
pub fn check(frame: &str, runs: &BTreeSet<String>) -> Result<(Vec<ActivityEntry>, u32), ActivityRefusal> {
    if frame.len() > ACTIVITY_REPORT_MAX_BYTES {
        return Err(ActivityRefusal::TooLarge);
    }
    let report: ActivityReport = serde_json::from_str(frame).map_err(|e| ActivityRefusal::Malformed(e.to_string()))?;
    if report.kind != "activity" || report.v != ACTIVITY_VERSION {
        return Err(ActivityRefusal::Malformed(format!("type {:?}, v {}", report.kind, report.v)));
    }
    if report.agents.len() > ACTIVITY_MAX {
        return Err(ActivityRefusal::TooMany);
    }
    let mut seen = BTreeSet::new();
    for e in &report.agents {
        if !runs.contains(&e.agent) {
            return Err(ActivityRefusal::NotHere(e.agent.clone()));
        }
        if !fragment_proto::valid_fragment_name(&e.chat) {
            return Err(ActivityRefusal::BadEntry(format!("{:?} is no fragment's name", e.chat)));
        }
        match (&e.waits_on, e.state) {
            (Some(who), fragment_proto::computer::ActivityState::Waiting) if !crate::npub::is_identity(who) => {
                return Err(ActivityRefusal::BadEntry(format!("waitsOn {who:?} is no identity")));
            }
            (Some(_), state) if state != fragment_proto::computer::ActivityState::Waiting => {
                return Err(ActivityRefusal::BadEntry(format!("a {} entry waits on no one", state.as_str())));
            }
            _ => {}
        }
        if !seen.insert((e.agent.clone(), e.chat.clone())) {
            return Err(ActivityRefusal::Twice(format!("{} in {}", e.agent, e.chat)));
        }
    }
    let mut entries = report.agents;
    entries.sort_by(|a, b| (&a.agent, &a.chat).cmp(&(&b.agent, &b.chat)));
    Ok((entries, report.more))
}

/// Whether a report that is not empty comes too soon after the last one
/// taken (`last_ms`), at `now_ms`. An empty one always comes in time: it
/// clears.
pub fn too_soon(entries: &[ActivityEntry], last_ms: Option<i64>, now_ms: i64) -> bool {
    !entries.is_empty() && last_ms.is_some_and(|last| now_ms - last < ACTIVITY_SPACING_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::computer::ActivityState::{Queued, Waiting, Working};
    use serde_json::json;

    const PAUL: &str = "npub1sg6plzptd64u62a878hep2kev88swjh3tw00gjsfl8f237lmu63q0uf63m";

    fn runs() -> BTreeSet<String> {
        ["juniper--k3x9", "maple--k3x9"].map(String::from).into()
    }

    fn frame(agents: serde_json::Value) -> String {
        json!({ "type": "activity", "v": 1, "agents": agents, "more": 0 }).to_string()
    }

    /// Valid: entries of the agents it runs, in any order, come back
    /// ordered; an empty report clears; replay: the same report, in
    /// another order, is the same.
    #[test]
    fn a_report_of_its_own_agents_is_taken() {
        assert!(crate::npub::is_identity(PAUL), "the test's identity is one");
        let said = frame(json!([
            { "agent": "maple--k3x9", "chat": "maple-chat--k3x9", "state": "queued" },
            { "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "waiting", "waitsOn": PAUL },
            { "agent": "juniper--k3x9", "chat": "notes--k3x9", "state": "working" },
        ]));
        let (entries, more) = check(&said, &runs()).expect("taken");
        assert_eq!(more, 0);
        let got: Vec<(&str, &str, _)> = entries.iter().map(|e| (e.agent.as_str(), e.chat.as_str(), e.state)).collect();
        assert_eq!(got, vec![("juniper--k3x9", "notes--k3x9", Working), ("juniper--k3x9", "talk--k3x9", Waiting), ("maple--k3x9", "maple-chat--k3x9", Queued)]);
        assert_eq!(entries[1].waits_on.as_deref(), Some(PAUL));
        let reordered = frame(json!([
            { "agent": "juniper--k3x9", "chat": "notes--k3x9", "state": "working" },
            { "agent": "maple--k3x9", "chat": "maple-chat--k3x9", "state": "queued" },
            { "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "waiting", "waitsOn": PAUL },
        ]));
        assert_eq!(check(&reordered, &runs()).expect("taken").0, entries, "the same report is the same");
        assert_eq!(check(&frame(json!([])), &runs()).expect("an empty report clears").0, vec![]);
        let more = json!({ "type": "activity", "v": 1, "agents": [], "more": 3 }).to_string();
        assert_eq!(check(&more, &runs()).expect("taken").1, 3);
    }

    /// Invalid: each refusal, the report refused whole.
    #[test]
    fn a_report_that_is_wrong_is_refused_whole() {
        let ok = json!({ "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "working" });
        let refused = |agents: serde_json::Value| check(&frame(agents), &runs()).unwrap_err();
        assert_eq!(check(&"x".repeat(ACTIVITY_REPORT_MAX_BYTES + 1), &runs()).unwrap_err(), ActivityRefusal::TooLarge);
        for bad in [
            "not json".to_string(),
            json!({ "type": "ping" }).to_string(),
            json!({ "type": "activity", "v": 2, "agents": [] }).to_string(),
            json!({ "type": "activity", "v": 1, "agents": [], "extra": true }).to_string(),
            json!({ "type": "activity", "v": 1, "agents": [{ "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "asleep" }] }).to_string(),
        ] {
            assert!(matches!(check(&bad, &runs()), Err(ActivityRefusal::Malformed(_))), "{bad}");
        }
        let many: Vec<_> = (0..=ACTIVITY_MAX).map(|i| json!({ "agent": "juniper--k3x9", "chat": format!("c{i}--k3x9"), "state": "working" })).collect();
        assert_eq!(refused(json!(many)), ActivityRefusal::TooMany);
        assert_eq!(refused(json!([ok, { "agent": "oak--k3x9", "chat": "talk--k3x9", "state": "working" }])), ActivityRefusal::NotHere("oak--k3x9".into()));
        assert!(matches!(refused(json!([{ "agent": "juniper--k3x9", "chat": "Not A Name", "state": "working" }])), ActivityRefusal::BadEntry(_)));
        assert!(matches!(refused(json!([{ "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "waiting", "waitsOn": "paul" }])), ActivityRefusal::BadEntry(_)));
        assert!(matches!(refused(json!([{ "agent": "juniper--k3x9", "chat": "talk--k3x9", "state": "working", "waitsOn": PAUL }])), ActivityRefusal::BadEntry(_)));
        assert_eq!(refused(json!([ok, ok])), ActivityRefusal::Twice("juniper--k3x9 in talk--k3x9".into()));
        for r in [ActivityRefusal::TooLarge, ActivityRefusal::TooMany, ActivityRefusal::NotHere("x".into())] {
            assert!(!r.message().is_empty());
        }
    }

    /// A report that is not empty within a second of the last is too soon;
    /// an empty one never is, and the first never is.
    #[test]
    fn reports_are_spaced_but_a_clear_is_not() {
        let one = vec![ActivityEntry { agent: "juniper--k3x9".into(), chat: "talk--k3x9".into(), state: Working, waits_on: None }];
        assert!(!too_soon(&one, None, 1_000));
        assert!(too_soon(&one, Some(1_000), 1_999));
        assert!(!too_soon(&one, Some(1_000), 2_000));
        assert!(!too_soon(&[], Some(1_000), 1_001));
    }
}
