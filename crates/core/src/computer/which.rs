//! Which computer: a person's own, or an agent's own.
//!
//! Since a computer per agent (2026-11-24), Add agent makes each new agent
//! a computer of its own, its id derived from the agent's identity by the
//! one function that derives a person's (`default_computer_of`): an
//! identity names one computer, and an agent's identity is its agent
//! fragment's key, so a name deleted and made again gets another. A
//! person's own computer is from before: it runs the agents put on it then
//! (at most 16), and runs on as it is until they move (the debt ledger,
//! "Computers from before a computer per agent run several agents").

use sha2::{Digest, Sha256};

/// The agents an agent's own computer runs: that agent alone, a named limit
/// checked as an agent is assigned (`may_run`) and asserted where a
/// computer keeps its agents.
pub const AGENTS_ON_OWN_MAX: u64 = 1;

/// The id of the computer an identity owns: a person's own (one each from
/// before a computer per agent: decision 13), or an agent's own. Derived,
/// so making it twice makes one.
pub fn default_computer_of(owner: &str) -> String {
    let digest = Sha256::digest(format!("fragment computer\0{owner}").as_bytes());
    format!("computer:{}", &hex::encode(digest)[..24])
}

/// Whose computer `computer` is, to an agent fragment of `owner`'s whose
/// agent is `agent` (its identity, its fragment's npub): the person's own,
/// the agent's own, or neither, which may not run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// The person's own computer, from before: it runs several agents.
    Owners,
    /// The agent's own computer: it runs that agent alone.
    AgentsOwn,
    /// Another computer: refused.
    Neither,
}

/// Whether `computer` may run `owner`'s agent `agent`: on their own
/// computer from before, or its own. An agent's own computer runs no other
/// agent, whoever's, since its id names that agent alone.
pub fn may_run(computer: &str, owner: &str, agent: &str) -> Runs {
    assert_ne!(owner, agent, "an agent is not its own owner");
    if computer == default_computer_of(agent) {
        Runs::AgentsOwn
    } else if computer == default_computer_of(owner) {
        Runs::Owners
    } else {
        Runs::Neither
    }
}

/// The computers a person's agents may run on, the order a list shows
/// them: their own from before, then each agent's own, in their agents'
/// order (`agents`, the registry's: the first made first). Which of them
/// were made is each computer's to say.
pub fn computers_of(person: &str, agents: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(agents.len() + 1);
    out.push(default_computer_of(person));
    out.extend(agents.iter().map(|a| default_computer_of(a)));
    assert_eq!(out.len(), agents.len() + 1, "one computer each, and the person's own");
    out
}

/// `GET /api/computers`'s answer: each computer's view as it answered
/// (`ComputerView`, written by its own object), in their order, and where
/// its person gets help, as one list (`{computers, support}`). Its views
/// are joined as bytes, never read: the router decodes and encodes none.
pub fn views_joined(views: &[&[u8]], support: Option<&str>) -> Vec<u8> {
    let mut out = br#"{"computers":["#.to_vec();
    for (i, view) in views.iter().enumerate() {
        assert!(view.first() == Some(&b'{') && view.last() == Some(&b'}'), "a computer's view is an object");
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(view);
    }
    out.extend_from_slice(br#"],"support":"#);
    out.extend_from_slice(serde_json::to_string(&support).expect("a string serializes").as_bytes());
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_persons_computer_has_one_stable_id() {
        let a = default_computer_of("id:aaaa");
        assert_eq!(a, default_computer_of("id:aaaa"));
        assert_ne!(a, default_computer_of("id:aaab"));
        assert!(a.starts_with("computer:") && a.len() == "computer:".len() + 24 && a[9..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// Goal: an agent runs on its owner's computer from before, or on its
    /// own; nowhere else. Invalid: another person's computer, another
    /// agent's own (its owner's or anyone's), a made-up id.
    #[test]
    fn an_agent_runs_on_its_owners_computer_or_its_own() {
        let (paul, juniper, fred) = ("npub1paul", "npub1juniper", "npub1fred");
        assert_eq!(may_run(&default_computer_of(paul), paul, juniper), Runs::Owners);
        assert_eq!(may_run(&default_computer_of(juniper), paul, juniper), Runs::AgentsOwn);
        assert_eq!(may_run(&default_computer_of(fred), paul, juniper), Runs::Neither, "another agent's own, even its owner's");
        assert_eq!(may_run(&default_computer_of("npub1ann"), paul, juniper), Runs::Neither, "another person's");
        assert_eq!(may_run("computer:000000000000000000000000", paul, juniper), Runs::Neither);
        assert_eq!(computers_of(paul, &[juniper.into(), fred.into()]), [default_computer_of(paul), default_computer_of(juniper), default_computer_of(fred)]);
    }

    /// Goal: the list is each view as it came, in order, with the help
    /// address, whatever a view holds (`]}` in a string included); none is
    /// an empty list.
    #[test]
    fn computers_are_listed_as_their_views_came() {
        let (a, b) = (br#"{"computer":"computer:a","why":"]}"}"#.as_slice(), br#"{"computer":"computer:b"}"#.as_slice());
        let help = "https://help.example/?q=\"x\"";
        let list: serde_json::Value = serde_json::from_slice(&views_joined(&[a, b], Some(help))).expect("a list");
        assert_eq!(list, serde_json::json!({ "computers": [{ "computer": "computer:a", "why": "]}" }, { "computer": "computer:b" }], "support": help }));
        let none: serde_json::Value = serde_json::from_slice(&views_joined(&[], None)).expect("a list");
        assert_eq!(none, serde_json::json!({ "computers": [], "support": null }));
    }
}
