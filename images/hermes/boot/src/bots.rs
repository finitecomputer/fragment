//! What Hermes knows of this image's agents beside their configs
//! (docs/computers.md, "Teammates in our Hermes image"): each one's name
//! and role, in its `profile.yaml` (`profile_yaml`) and its teammates'
//! configs (hermes.rs, `told_team`), and the route that makes each profile
//! one the gateway reaches (`profile_routes`).

use std::collections::BTreeMap;

use fragment_bridge::api::FragmentEntry;
use fragment_bridge::runtime::relay::wire;
use fragment_bridge::runtime::Agent;

/// An agent's role, in characters at most (each teammate is one line).
pub const DESCRIPTION_MAX_CHARS: usize = 120;

fn q(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// A fragment name's label: the bridge's (`fragment_bridge::api`).
use fragment_bridge::api::label;

/// What an agent does, from its job (its `SOUL.md`): the first line that says
/// anything, a heading's or a list's marks taken off, its spaces collapsed,
/// at most `DESCRIPTION_MAX_CHARS`.
pub fn description(soul: &str) -> String {
    let line = soul.lines().map(|l| l.trim().trim_start_matches(['#', '>', '-', '*']).trim()).find(|l| !l.is_empty()).unwrap_or("");
    let words = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() <= DESCRIPTION_MAX_CHARS {
        return words;
    }
    let cut: String = words.chars().take(DESCRIPTION_MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// An agent's name (docs/api.md, "An agent's name and picture"): its agent
/// fragment's title, else its name on the computer, else its fragment's
/// label. What its `profile.yaml` (`profile_yaml`), its config's
/// `agent.system_prompt` (hermes.rs, `told_name`) and its teammates'
/// (`told_team`) say.
pub fn name<'a>(agent: &'a Agent, title: Option<&'a str>) -> &'a str {
    [title.unwrap_or(""), agent.name.as_str()].into_iter().map(str::trim).find(|n| !n.is_empty()).unwrap_or_else(|| label(&agent.fragment))
}

/// An agent's `profile.yaml`: Hermes' profile metadata, its name (its
/// agent fragment's title, "Juniper", as the shell titles it; else its
/// name on the computer, else its fragment's label) and its role (its
/// job's first line, `description`). It marks no profile a Bot Mode bot
/// (`ui_meta.hermes-bots`): Hermes' Bot Mode protocol is off (hermes.rs,
/// `managed_config`), and the mark gone changes the capability fingerprint
/// of each Bot Chat's stored prompt, so Hermes rebuilds that prompt once,
/// without the protocol, keeping its history. Written whole at every boot,
/// for an agent assigned while the computer runs, and when a sync pulls its
/// job.
pub fn profile_yaml(agent: &Agent, title: Option<&str>, soul: Option<&str>) -> String {
    let name = name(agent, title);
    let about = soul.map(description).unwrap_or_default();
    let mut y = format!(
        "# Written by hermes-boot for {}, from its agent fragment, and written again as that changes (images/hermes/boot/src/bots.rs). Never edit it: rename the agent with `fragment agent --name`.\n",
        agent.fragment
    );
    y.push_str(&format!("display_name: {}\n", q(name)));
    if !about.is_empty() {
        y.push_str(&format!("description: {}\n", q(&about)));
    }
    y
}

/// Hermes' `profile_routes`, for the gateway's config (the managed
/// overlay, main.rs): one route a profile, each agent's `routed` chat (a
/// chat it is in: `route_chat`) to its profile. No route
/// picks a message's profile (the relay names it on each, which wins); a
/// route makes its profile one the gateway reaches through the default
/// profile's relay after a restart (v0.21.6: `gateway/authz_mixin.py`,
/// `_is_shared_bot_satellite`, which asks only whether any route names the
/// profile), so one reaches its sessions in every chat. A session the
/// gateway reads back from disk holds no live adapter, and a profile has
/// none of its own, so without a route a background process's completion
/// in it was retried unsent for ever
/// (p5, 2026-10-09). The gateway reads them as it starts, and only then
/// (no verb of Hermes' reloads them): an agent come since has its route at
/// the next start, and its sessions until then are this gateway's own,
/// which need none.
pub fn profile_routes(agents: &[Agent], routed: &BTreeMap<String, String>) -> String {
    let routes: Vec<String> = agents
        .iter()
        .filter_map(|a| {
            let profile = wire::profile(&a.fragment);
            let chat = wire::chat_id(routed.get(&a.fragment)?, &a.fragment);
            Some(format!("  - name: {}\n    platform: \"relay\"\n    profile: {}\n    chat_id: {}\n", q(&profile), q(&profile), q(&chat)))
        })
        .collect();
    if routes.is_empty() {
        return String::new();
    }
    format!("profile_routes:\n{}", routes.concat())
}

/// The chat a route names for an agent: of the fragments it is in
/// (`fragments`, its own list), the chat first by name,
/// not its own fragment; None when it is in no chat (then it has no
/// session a completion could be lost in).
pub fn route_chat(agent_fragment: &str, fragments: &[FragmentEntry]) -> Option<String> {
    fragments.iter().filter(|f| f.kind == "chat" && f.name != agent_fragment).map(|f| f.name.clone()).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(label: &str, name: &str) -> Agent {
        Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: name.into(), owner: "npub1paul".into(), credentials: vec![], key: None }
    }

    fn listed(name: &str, kind: &str, owned: bool) -> FragmentEntry {
        FragmentEntry { name: name.into(), role: "editor".into(), kind: kind.into(), owned, title: String::new() }
    }

    /// Valid: the job's first line that says anything; invalid: none, or
    /// only marks; a long one cut, whitespace collapsed.
    #[test]
    fn a_description_is_the_jobs_first_line() {
        assert_eq!(description("You research plants.\nAnd more."), "You research plants.");
        assert_eq!(description("\n\n# Juniper\n\nGardener."), "Juniper");
        assert_eq!(description("  - keeps   the   books  "), "keeps the books");
        assert_eq!(description(""), "");
        assert_eq!(description("#\n> \n"), "");
        let long = "word ".repeat(100);
        let d = description(&long);
        assert_eq!(d.chars().count(), DESCRIPTION_MAX_CHARS);
        assert!(d.ends_with('…') && !d.contains("  "), "{d}");
        let wide = "é".repeat(500);
        assert_eq!(description(&wide).chars().count(), DESCRIPTION_MAX_CHARS, "cut on a character, never inside one");
    }

    /// Valid: name (its fragment's title) and description, each a YAML
    /// double-quoted scalar (a JSON string) whatever it holds; with no job,
    /// no description; with no title, its name on the computer; with
    /// neither, the label. Invalid: no Bot Mode mark (its protocol is off).
    /// Replay: the same agent writes the same bytes.
    #[test]
    fn a_profile_says_who_the_agent_is() {
        let y = profile_yaml(&agent("juniper", "juniper"), Some("Juniper"), Some("You tend the garden: \"weeds\" first.\n"));
        assert_eq!(
            y,
            "# Written by hermes-boot for juniper--k3x9, from its agent fragment, and written again as that changes (images/hermes/boot/src/bots.rs). Never edit it: rename the agent with `fragment agent --name`.\n\
             display_name: \"Juniper\"\n\
             description: \"You tend the garden: \\\"weeds\\\" first.\"\n"
        );
        assert_eq!(y, profile_yaml(&agent("juniper", "juniper"), Some("Juniper"), Some("You tend the garden: \"weeds\" first.\n")));
        let untitled = profile_yaml(&agent("juniper", "juniper"), Some(" "), None);
        assert!(untitled.contains("display_name: \"juniper\"\n"), "no title: its name on the computer: {untitled}");
        let bare = profile_yaml(&agent("fred", " "), None, None);
        assert!(bare.contains("display_name: \"fred\"\n") && !bare.contains("description") && !bare.contains("hermes-bots"), "{bare}");
        let odd = profile_yaml(&agent("x", "x"), Some("a: b\n# c"), Some("line: one"));
        assert!(odd.contains("display_name: \"a: b\\n# c\"\n"), "a name is one scalar, never more YAML: {odd}");
        // renamed (its fragment's title), the agent is its new name alone
        let renamed = profile_yaml(&agent("juniper", "juniper"), Some("Frank"), Some("You tend the garden.\n"));
        assert!(renamed.contains("display_name: \"Frank\"\n") && !renamed.contains("\"Juniper\""), "{renamed}");
    }

    /// What every agent is told of its name and picture (the platform
    /// skill's page, computer.md): the one way to change them, never by
    /// the profile's files, which the boot writes again, and never its
    /// name in its job, which a rename would leave behind.
    #[test]
    fn an_agent_is_told_how_to_rename_itself() {
        let page = include_str!("computer.md");
        for said in ["fragment agent --name", "fragment agent --picture", "fragment agent --no-picture", "`profile.yaml`", "256 KiB", "never put your name in it"] {
            assert!(page.contains(said), "computer.md says {said}");
        }
    }

    /// Valid: one route a profile, each agent's routed chat (as the relay
    /// names it to Hermes) to its profile. An agent with none has no route;
    /// with none at all, no key.
    #[test]
    fn each_profile_is_routed_by_one_chat() {
        let chats: BTreeMap<String, String> = [("juniper--k3x9".to_string(), "juniper-chat--h6j7".to_string()), ("fred--k3x9".into(), "fred-chat--b3c4".into())].into();
        let agents = [agent("juniper", "Juniper"), agent("maple", "Maple"), agent("fred", "Fred")];
        assert_eq!(
            profile_routes(&agents, &chats),
            "profile_routes:\n  - name: \"juniper--k3x9\"\n    platform: \"relay\"\n    profile: \"juniper--k3x9\"\n    chat_id: \"juniper-chat--h6j7/juniper--k3x9\"\n  - name: \"fred--k3x9\"\n    platform: \"relay\"\n    profile: \"fred--k3x9\"\n    chat_id: \"fred-chat--b3c4/fred--k3x9\"\n"
        );
        assert_eq!(profile_routes(&agents[1..2], &chats), "", "maple, with no chat found, has none");
        assert_eq!(profile_routes(&[], &chats), "");
    }

    /// Valid: of the fragments an agent is in, the chat first by name.
    /// Invalid: an app, its own fragment, or none: no chat.
    #[test]
    fn an_agent_is_routed_by_a_chat_it_is_in() {
        let list = [listed("todo--p2m4", "app", false), listed("garden--r2d5", "chat", false), listed("allotment--a1b2", "chat", false), listed("fern--k3x9", "chat", false)];
        assert_eq!(route_chat("fern--k3x9", &list).as_deref(), Some("allotment--a1b2"));
        assert_eq!(route_chat("fern--k3x9", &list[..1]), None, "an app is no chat");
        assert_eq!(route_chat("fern--k3x9", &list[3..]), None, "nor its own fragment");
        assert_eq!(route_chat("fern--k3x9", &[]), None);
    }
}
