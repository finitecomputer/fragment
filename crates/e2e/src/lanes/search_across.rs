//! Search across fragments (docs/api.md, "Search across fragments"): one
//! question asked of several fragments' indexes as one, as the signer.
//! Two fragments written through the upsert API, a third with no index:
//! the owner's question finds both indexes' sections merged, embedded
//! once, and says the third has none; a viewer of one finds that one's
//! alone, the other denied; a member removed finds nothing more; a source
//! a push deleted never shows; words alone embed nothing; a question past
//! its limits is refused before anything is embedded; and `fragment
//! search --in` and `fragment mcp`'s `fragment-search` tool ask it as the
//! signed-in person.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::Api;
use crate::Suite;

const MODEL: &str = fragment_core::embed::MODEL_DEFAULT;

/// How many texts the embeddings model was asked for since `from`.
fn embedded(s: &Suite, from: usize) -> usize {
    s.ai.calls()[from..].iter().filter(|c| c.model == MODEL).map(|c| c.body["text"].as_array().map_or(0, Vec::len)).sum()
}

fn upsert(source: &str, title: &str, text: &str) -> Value {
    json!({ "source": source, "revision": "r1", "title": title, "sections": [{ "id": "s0", "text": text }] })
}

/// Each fragment's state in an answer, by name.
fn states(answer: &Value) -> Vec<(String, String)> {
    answer["fragments"].as_array().into_iter().flatten().map(|f| (f["name"].as_str().unwrap_or("").to_string(), f["state"].as_str().unwrap_or("").to_string())).collect()
}

/// The fragments the hits of an answer are from, best first.
fn from(answer: &Value) -> Vec<String> {
    answer["results"].as_array().into_iter().flatten().map(|h| h["fragment"].as_str().unwrap_or("").to_string()).collect()
}

pub fn search_across(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("search-across", &[crate::Need::Node, crate::Need::Levers, crate::Need::Fakes]) {
        return Ok(());
    }
    let wait = Duration::from_secs(60);
    let owner = api.person()?;
    let reader = api.person()?;
    let made = |label: &str| -> Result<(Value, String)> {
        let created = s.create(api, &owner, label)?;
        let name = created["name"].as_str().context("its name")?.to_string();
        Ok((created, name))
    };
    let (research, alpha) = made("research")?;
    let (_, beta) = made("delivery")?;
    let (_, gamma) = made("unindexed")?;
    s.ai.clear_script();
    for (name, body) in [
        (&alpha, upsert("wiki/launch.md", "Launch", "The harbour lantern launch moves to November, research says")),
        (&beta, upsert("wiki/launch.md", "Launch", "Delivery plans the harbour lantern launch for the second week of November")),
    ] {
        let w = api.signed(&owner, "POST", &format!("/api/f/{name}/search/upsert"), Some(&body))?;
        anyhow::ensure!(w.status == 200, "{name}'s write: {w}");
    }
    let r = api.share(&alpha, &api.identity(&reader)?, "viewer")?;
    anyhow::ensure!(r.status == 200, "a viewer: {r}");
    let ask = |who, fragments: &[&str], extra: Value| {
        let mut body = json!({ "q": "harbour lantern launch", "in": fragments });
        body.as_object_mut().expect("an object").extend(extra.as_object().cloned().unwrap_or_default());
        api.signed(who, "POST", "/api/search", Some(&body))
    };

    // ---- the owner's question: both indexes merged, the question embedded once
    let calls = s.ai.calls().len();
    let both = ask(&owner, &[&alpha, &beta, &gamma], json!({}))?;
    let asked = embedded(s, calls);
    let ranks: Vec<u64> = both.body["results"].as_array().into_iter().flatten().filter_map(|h| h["rank"].as_u64()).collect();
    let scores: Vec<f64> = both.body["results"].as_array().into_iter().flatten().filter_map(|h| h["score"].as_f64()).collect();
    s.ok(
        "a question across fragments finds each index's sections, merged best first and named by their fragment; the question is embedded once for all of them; a fragment with no index says so",
        both.status == 200 && both.body["semantic"] == "on" && asked == 1
            && from(&both.body).contains(&alpha) && from(&both.body).contains(&beta)
            && ranks == (1..=ranks.len() as u64).collect::<Vec<_>>() && scores.windows(2).all(|w| w[0] >= w[1])
            && states(&both.body) == [(alpha.clone(), "searched".into()), (beta.clone(), "searched".into()), (gamma.clone(), "no_index".into())],
        json!({ "answer": both.body, "embedded": asked }),
    );

    // ---- by meaning alone: cosines compare across fragments, so the merge is by cosine
    // nearer the second fragment's section than the first's, so an
    // interleave by rank (cosines that did not compare) puts them wrong
    let meaning = api.signed(&owner, "POST", "/api/search", Some(&json!({ "q": "delivery plans the second week", "in": [&alpha, &beta], "mode": "semantic" })))?;
    let cosines: Vec<f64> = meaning.body["results"].as_array().into_iter().flatten().filter_map(|h| h["semantic"]["cosine"].as_f64()).collect();
    s.ok(
        "by meaning alone, the hits of both fragments merge by their cosines, nearest first: one space, one scheme",
        meaning.status == 200 && from(&meaning.body).contains(&alpha) && from(&meaning.body).contains(&beta)
            && cosines.len() == from(&meaning.body).len() && cosines.windows(2).all(|w| w[0] >= w[1]),
        &meaning.body,
    );

    // ---- a viewer of one: that one's alone, the others denied, a name that is no fragment missing
    let missing = "nothere--k3x9";
    let viewer = ask(&reader, &[&alpha, &beta, missing], json!({}))?;
    s.ok(
        "a reader of one fragment finds its sections alone: the one they may not read is denied, a name that is no fragment missing, and none of their hits shows",
        viewer.status == 200 && !from(&viewer.body).is_empty() && from(&viewer.body).iter().all(|f| *f == alpha)
            && states(&viewer.body) == [(alpha.clone(), "searched".into()), (beta.clone(), "denied".into()), (missing.to_string(), "missing".into())],
        &viewer.body,
    );

    // ---- words alone embed nothing; a question past its limits is refused first
    let calls = s.ai.calls().len();
    let words = ask(&owner, &[&alpha, &beta], json!({ "mode": "lexical" }))?;
    let many: Vec<String> = (0..=fragment_proto::search::IN_MAX).map(|i| format!("b{i}--k3x9")).collect();
    let too_many = api.signed(&owner, "POST", "/api/search", Some(&json!({ "q": "launch", "in": many })))?;
    let bare = api.signed(&owner, "POST", "/api/search", Some(&json!({ "q": "launch", "in": ["research"] })))?;
    let twice = ask(&owner, &[&alpha, &alpha], json!({}))?;
    let unknown = ask(&owner, &[&alpha], json!({ "order": "newest" }))?;
    s.ok(
        "words alone embed nothing (semantic off); more than 32 fragments, a bare label, a fragment named twice and a field the route does not know are refused (400), nothing embedded",
        words.status == 200 && words.body["semantic"] == "off" && from(&words.body).len() >= 2
            && [&too_many, &bare, &twice, &unknown].iter().all(|r| r.status == 400) && embedded(s, calls) == 0,
        json!({ "words": words.body, "refused": [too_many.status, bare.status, twice.status, unknown.status] }),
    );

    // ---- a source a push deleted never shows
    s.commit(&research, &[("wiki/old.md", Some(b"# old".as_slice()))]);
    let w = api.signed(&owner, "POST", &format!("/api/f/{alpha}/search/upsert"), Some(&upsert("wiki/old.md", "Old", "The zebra crossing permit")))?;
    let held = api.signed(&owner, "POST", "/api/search", Some(&json!({ "q": "zebra permit", "in": [&alpha, &beta], "mode": "lexical" })))?;
    s.commit(&research, &[("wiki/old.md", None)]);
    let gone = s.eventually(wait, || {
        api.signed(&owner, "POST", "/api/search", Some(&json!({ "q": "zebra permit", "in": [&alpha, &beta], "mode": "lexical" })))
            .is_ok_and(|r| r.status == 200 && r.body["results"].as_array().is_some_and(|h| h.iter().all(|h| h["source"] != "wiki/old.md")))
    });
    s.ok(
        "a source whose file a push deleted is left out of a search across fragments, as out of its own",
        w.status == 200 && held.body["results"][0]["source"] == "wiki/old.md" && gone,
        &held.body,
    );

    // ---- the CLI, and an MCP client through it, ask it as the signed-in person
    let home = s.dir("search-across-cli");
    s.login(api, &home);
    let cli = s.cli_keys(&home).context("the CLI's key")?;
    let shared = api.share(&beta, &api.identity(&cli)?, "viewer")?;
    let found = s.cli_json(api, &home, &["search", "harbour lantern launch", "--in", &format!("{alpha},{beta}"), "--json"])?;
    let printed = String::from_utf8_lossy(&s.cli(api, &home, &["search", "harbour lantern launch", "--in", &beta, "--limit", "1"]).stdout).into_owned();
    s.ok(
        "fragment search --in asks as the signed-in person: a fragment they may read searched, the other denied; printed, a hit a line with its fragment, its snippet under it",
        shared.status == 200 && from(&found).iter().all(|f| *f == beta) && !from(&found).is_empty()
            && states(&found) == [(alpha.clone(), "denied".into()), (beta.clone(), "searched".into())]
            && printed.starts_with(&format!("1. {beta} wiki/launch.md #s0")) && printed.contains("November"),
        json!({ "json": found, "printed": printed }),
    );
    let mut mcp = s.bare_cli().args(["mcp", &beta]).env("HOME", &home).env("FRAGMENT_HOST", &api.base).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    {
        let mut stdin = mcp.stdin.take().context("its stdin is piped")?;
        let init = json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } });
        for line in [
            json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": init }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "fragment-search", "arguments": { "q": "second week of November" } } }),
        ] {
            writeln!(stdin, "{line}")?;
        }
    }
    let out = mcp.wait_with_output()?;
    let answers: Vec<Value> = String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap_or_else(|_| json!({ "not JSON-RPC": l }))).collect();
    let listed = answers.get(1).and_then(|a| a["result"]["tools"].as_array().cloned()).unwrap_or_default();
    let tool = answers.get(2).map(|a| a["result"]["structuredContent"].clone()).unwrap_or(Value::Null);
    s.ok(
        "fragment mcp serves fragment-search to a reader, asking its own fragment's index unless told others",
        listed.iter().any(|t| t["name"] == "fragment-search") && from(&tool).first() == Some(&beta) && states(&tool) == [(beta.clone(), "searched".into())],
        json!({ "mcp": answers }),
    );

    // ---- a member removed finds nothing more
    let gone = api.signed(&owner, "DELETE", &format!("/api/f/{alpha}/members/{}", api.identity(&reader)?), None)?;
    let after = ask(&reader, &[&alpha], json!({}))?;
    s.ok(
        "a member removed finds nothing more there: denied, no hits",
        gone.status == 200 && after.status == 200 && from(&after.body).is_empty() && states(&after.body) == [(alpha.clone(), "denied".into())],
        &after.body,
    );
    Ok(())
}
