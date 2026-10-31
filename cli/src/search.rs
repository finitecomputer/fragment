//! `fragment search <fragment> <query>`: a fragment's index searched by its
//! words and its meaning (docs/api.md, "A fragment's search"), through the
//! platform's search route, as whoever the CLI signs as (this machine's
//! key, or on a computer its agent), with their role there: a reader's
//! (a viewer's) at least. `fragment mcp` serves the same as its
//! `fragment-search` tool.
//!
//! Each hit names its fragment, so a search of several (the meta-brain's
//! `--in <a,b,…>`, FIN-238) is hits of one shape, merged: one fragment's
//! answer is the case of one.

use std::collections::BTreeMap;

use anyhow::Result;
use fragment_proto::search::{Found, Hit, Mode, Query};
use serde::Serialize;
use serde_json::{json, Value};

use crate::api::Client;

/// `fragment mcp`'s tool for a fragment's index: a name no operation can
/// have (an operation's is `a-z`, `0-9` and `_`: `valid_op_name`).
pub const TOOL: &str = "fragment-search";

/// A search's answer: whether the question was searched by its meaning
/// too (`semantic`: `off` on a deployment that embeds nothing, or for an
/// anonymous reader), and each hit, named by its fragment, best first.
#[derive(Debug, Serialize)]
pub struct Answer {
    pub semantic: String,
    pub results: Vec<Named>,
}

#[derive(Debug, Serialize)]
pub struct Named {
    pub fragment: String,
    #[serde(flatten)]
    pub hit: Hit,
}

/// The question `fragment search` asks: `fields` as `name=value`, each.
pub fn query(q: String, limit: Option<usize>, mode: Option<Mode>, fields: &[String], after: Option<String>, before: Option<String>) -> Result<Query, String> {
    let mut named = BTreeMap::new();
    for f in fields {
        let (name, value) = f.split_once('=').filter(|(n, v)| !n.is_empty() && !v.is_empty()).ok_or_else(|| format!("--field is name=value (kind=chat), not {f:?}"))?;
        if named.insert(name.to_string(), value.to_string()).is_some() {
            return Err(format!("--field {name} is named twice"));
        }
    }
    Ok(Query { q, limit, mode: mode.unwrap_or_default(), fields: named, after, before })
}

/// `fragment`'s index, asked `query`: its answer, each hit named by the
/// fragment in full (a bare label resolved).
pub fn search(client: &Client, fragment: &str, query: &Query) -> Result<Answer> {
    let full = client.fragment(fragment)?;
    let found: Found = client.call_as(client.post_json(&path(&full), query)?)?;
    Ok(named(&full, found))
}

/// The search route of `fragment` (named in full).
pub fn path(fragment: &str) -> String {
    format!("/api/f/{fragment}/search")
}

/// `found`, each hit named by `fragment`.
pub fn named(fragment: &str, found: Found) -> Answer {
    Answer { semantic: found.semantic, results: found.results.into_iter().map(|hit| Named { fragment: fragment.to_string(), hit }).collect() }
}

/// The answer as a person reads it: a hit a line, its snippet under it.
pub fn print(answer: &Answer) {
    for r in &answer.results {
        let h = &r.hit;
        let mut trail: Vec<&str> = vec![h.title.as_str()];
        trail.extend(h.context.iter().map(String::as_str));
        trail.extend(h.heading.as_deref());
        let trail: Vec<&str> = trail.into_iter().filter(|t| !t.is_empty()).collect();
        println!("{}. {} #{}  {}  ({})", h.rank, h.source, h.section, trail.join(" › "), h.signals.join("+"));
        println!("   {}", h.snippet.replace('\n', " "));
    }
    if answer.results.is_empty() {
        eprintln!("nothing found");
    }
    if answer.semantic != "on" {
        eprintln!("(by its words alone: this deployment embeds nothing, or you asked unsigned)");
    }
}

/// The tool's definition: a reader's, and a read.
pub fn tool() -> Value {
    json!({
        "name": TOOL,
        "description": "Search this fragment's index by words and meaning. Each result is a section of one of its sources (a conversation, a document), ranked, with a snippet of its best part, its source, section, title and heading. Your question is embedded on the platform's model, paid as you.",
        "inputSchema": {
            "type": "object",
            "required": ["q"],
            "properties": {
                "q": { "type": "string", "description": "What to find, in words" },
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

    #[test]
    fn a_question_is_checked_as_it_is_named() {
        let q = query("ferry".into(), Some(5), Some(Mode::Lexical), &["kind=chat".into()], None, None).unwrap();
        assert_eq!(serde_json::to_value(&q).unwrap(), json!({ "q": "ferry", "limit": 5, "mode": "lexical", "fields": { "kind": "chat" } }));
        assert!(query("x".into(), None, None, &["kind".into()], None, None).unwrap_err().contains("name=value"));
        assert!(query("x".into(), None, None, &["kind=a".into(), "kind=b".into()], None, None).unwrap_err().contains("twice"));
        assert_eq!(query("x".into(), None, None, &[], None, None).unwrap().mode, Mode::Hybrid);
    }

    #[test]
    fn the_tool_is_one_no_operation_can_be() {
        assert!(!fragment_proto::valid_op_name(TOOL));
        assert_eq!(tool()["inputSchema"]["required"], json!(["q"]));
        // its arguments are the route's question, as they are
        let args = json!({ "q": "ferry", "mode": "semantic", "limit": 3 });
        assert!(serde_json::from_value::<Query>(args).is_ok());
    }

    #[test]
    fn each_hit_names_its_fragment() {
        let hit: Hit = serde_json::from_value(json!({ "rank": 1, "source": "s", "section": "t0", "revision": "r", "title": "T", "snippet": "x", "score": 0.1, "signals": ["lexical"] })).unwrap();
        let answer = Answer { semantic: "on".into(), results: vec![Named { fragment: "notes--k3x9".into(), hit }] };
        let v = serde_json::to_value(&answer).unwrap();
        assert_eq!((v["results"][0]["fragment"].clone(), v["results"][0]["section"].clone()), (json!("notes--k3x9"), json!("t0")));
    }
}
