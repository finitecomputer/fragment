//! `fragment search "<question>" --in a,b`: fragments' own indexes searched
//! by their words and their meaning as one (docs/api.md, "Search across
//! fragments"), through the platform's `POST /api/search`, as whoever the
//! CLI signs as (this machine's key, or on a computer its agent). Each
//! fragment asked decides alone, as if asked by itself: a reader's role
//! there at least. One fragment is `--in a`. `fragment mcp` serves the same
//! as its `fragment-search` tool.

use std::collections::BTreeMap;

use anyhow::Result;
use fragment_proto::search::{Across, FoundAcross, Mode, Reached, IN_MAX};
use fragment_proto::{FragmentKind, FragmentList};
use serde_json::{json, Value};

use crate::api::Client;

/// `fragment mcp`'s search tool: a name no operation can have (an
/// operation's is `a-z`, `0-9` and `_`: `valid_op_name`).
pub const TOOL: &str = "fragment-search";

/// The question `fragment search` asks: `fields` as `name=value`, each.
pub fn across(q: String, fragments: Vec<String>, limit: Option<usize>, mode: Option<Mode>, fields: &[String], after: Option<String>, before: Option<String>) -> Result<Across, String> {
    let mut named = BTreeMap::new();
    for f in fields {
        let (name, value) = f.split_once('=').filter(|(n, v)| !n.is_empty() && !v.is_empty()).ok_or_else(|| format!("--field is name=value (kind=chat), not {f:?}"))?;
        if named.insert(name.to_string(), value.to_string()).is_some() {
            return Err(format!("--field {name} is named twice"));
        }
    }
    Ok(Across { q, fragments, limit, mode: mode.unwrap_or_default(), fields: named, after, before })
}

/// What `--all` asks: the caller's brains and Session Archive (not one
/// they archived), at most `IN_MAX` of them by name, and how many more
/// there were.
pub fn all(list: &FragmentList) -> (Vec<String>, usize) {
    let mut names: Vec<String> = list
        .fragments
        .iter()
        .filter(|f| matches!(f.kind, FragmentKind::Brain | FragmentKind::Sessions) && !f.archived)
        .map(|f| f.name.clone())
        .collect();
    names.sort();
    let left = names.len().saturating_sub(IN_MAX);
    names.truncate(IN_MAX);
    (names, left)
}

/// `asked`, each fragment named in full (a bare label resolved), asked.
pub fn search(client: &Client, mut asked: Across) -> Result<FoundAcross> {
    asked.fragments = asked.fragments.iter().map(|f| client.fragment(f)).collect::<Result<_>>()?;
    client.call_as(client.post_json("/api/search", &asked)?)
}

/// The answer as a person reads it: a hit a line, its snippet under it,
/// then each fragment that was not searched, and why.
pub fn print(answer: &FoundAcross) {
    for r in &answer.results {
        let h = &r.hit;
        let mut trail: Vec<&str> = vec![h.title.as_str()];
        trail.extend(h.context.iter().map(String::as_str));
        trail.extend(h.heading.as_deref());
        let trail: Vec<&str> = trail.into_iter().filter(|t| !t.is_empty()).collect();
        println!("{}. {} {} #{}  {}  ({})", h.rank, r.fragment, h.source, h.section, trail.join(" › "), h.signals.join("+"));
        println!("   {}", h.snippet.replace('\n', " "));
    }
    if answer.results.is_empty() {
        eprintln!("nothing found");
    }
    for f in answer.fragments.iter().filter(|f| f.state != Reached::Searched) {
        let state = serde_json::to_value(f.state).expect("a state serializes");
        let why = f.message.as_deref().map(|m| format!(": {m}")).unwrap_or_default();
        eprintln!("{} not searched ({}){why}", f.name, state.as_str().unwrap_or_default());
    }
    if answer.semantic != "on" {
        eprintln!("(by its words alone: you asked for words alone, or this deployment embeds nothing)");
    }
}

/// The tool's definition: a reader's, and a read. `in` is this fragment
/// unless named.
pub fn tool() -> Value {
    json!({
        "name": TOOL,
        "description": "Search fragments' indexes by words and meaning, as one: this fragment's, or each named in `in` (a parent brain and its sources, say). Each result is a section of one of their sources (a page, a conversation), best first, named by its fragment, with a snippet of its best part. Each fragment searched is one you may read; `fragments` says how each answered. Your question is embedded once on the platform's model, paid as you.",
        "inputSchema": {
            "type": "object",
            "required": ["q"],
            "properties": {
                "q": { "type": "string", "description": "What to find, in words" },
                "in": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": IN_MAX, "description": "The fragments to search, each named in full (this one's, unless named)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": fragment_proto::search::RESULTS_MAX, "description": "How many results, at most" },
                "mode": { "enum": ["hybrid", "lexical", "semantic"], "description": "By words and meaning (hybrid, the default), words alone, or meaning alone" },
                "fields": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Only sources with each of these fields (\"kind\": \"chat\")" },
                "after": { "type": "string", "description": "Only sections at or after this time (RFC 3339)" },
                "before": { "type": "string", "description": "Only sections before this time" }
            },
            "additionalProperties": false
        },
        "annotations": { "readOnlyHint": true, "openWorldHint": false }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::{ListedFragment, Role};

    #[test]
    fn a_question_is_checked_as_it_is_named() {
        let a = across("ferry".into(), vec!["notes--k3x9".into()], Some(5), Some(Mode::Lexical), &["kind=chat".into()], None, None).unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap(), json!({ "q": "ferry", "in": ["notes--k3x9"], "limit": 5, "mode": "lexical", "fields": { "kind": "chat" } }));
        assert!(across("x".into(), vec![], None, None, &["kind".into()], None, None).unwrap_err().contains("name=value"));
        assert!(across("x".into(), vec![], None, None, &["kind=a".into(), "kind=b".into()], None, None).unwrap_err().contains("twice"));
        assert_eq!(across("x".into(), vec![], None, None, &[], None, None).unwrap().mode, Mode::Hybrid);
    }

    #[test]
    fn the_tool_is_one_no_operation_can_be() {
        assert!(!fragment_proto::valid_op_name(TOOL));
        assert_eq!(tool()["inputSchema"]["required"], json!(["q"]));
        // its arguments, with `in`, are the route's question as they are
        let args = json!({ "q": "ferry", "in": ["notes--k3x9"], "mode": "semantic", "limit": 3 });
        assert!(serde_json::from_value::<Across>(args).is_ok());
    }

    /// Goal (Alex's review of #848): an agent acting for someone searches
    /// as them, so each fragment caps it by their role: the question names
    /// `for`; one acting for no one, or a key, names none.
    #[test]
    fn an_agent_acting_for_someone_searches_as_them() {
        use crate::api::{AgentMode, Signer};
        use fragment_fakes::http::{Request, Response, Server};
        use std::sync::{Arc, Mutex};
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
        let log = Arc::clone(&seen);
        let host = Server::start(
            0,
            Arc::new(move |r: &Request| {
                log.lock().unwrap().push(r.pairs.iter().find(|(k, _)| k == "for").map(|(_, v)| v.clone()));
                Response::json(200, &json!({ "results": [], "fragments": [{ "name": "notes--k3x9", "state": "denied" }], "semantic": "on" }))
            }),
        )
        .expect("start the fake host");
        let asked = || across("ferry".into(), vec!["notes--k3x9".into()], None, None, &[], None, None).unwrap();
        let for_paul = Client::new(&host.url, Signer::Agent(AgentMode { agent: "juniper--k3x9".into(), acting_for: Some("id:paul".into()) }));
        let alone = Client::new(&host.url, Signer::Agent(AgentMode { agent: "juniper--k3x9".into(), acting_for: None }));
        let key = Client::new(&host.url, crate::auth::fixed(7));
        let answer = search(&for_paul, asked()).unwrap();
        assert_eq!(answer.fragments[0].state, Reached::Denied);
        search(&alone, asked()).unwrap();
        search(&key, asked()).unwrap();
        assert_eq!(*seen.lock().unwrap(), [Some("id:paul".to_string()), None, None]);
    }

    #[test]
    fn all_is_your_brains_and_archive_by_name() {
        let row = |name: &str, kind: FragmentKind, archived: bool| {
            let mut f: ListedFragment = serde_json::from_value(json!({ "name": name, "role": "owner" })).unwrap();
            f.kind = kind;
            f.role = Role::Owner;
            f.archived = archived;
            f
        };
        let list = FragmentList {
            fragments: vec![
                row("zeta--k3x9", FragmentKind::Brain, false),
                row("chat--k3x9", FragmentKind::Chat, false),
                row("archive--k3x9", FragmentKind::Sessions, false),
                row("old--k3x9", FragmentKind::Brain, true),
                row("todo--k3x9", FragmentKind::App, false),
            ],
        };
        assert_eq!(all(&list), (vec!["archive--k3x9".to_string(), "zeta--k3x9".to_string()], 0));
        let many = FragmentList { fragments: (0..IN_MAX + 3).map(|i| row(&format!("b{i:02}--k3x9"), FragmentKind::Brain, false)).collect() };
        let (names, left) = all(&many);
        assert_eq!((names.len(), left, names[0].as_str()), (IN_MAX, 3, "b00--k3x9"));
    }
}
