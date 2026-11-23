//! `for` names only the agent's own owner (access::may_act_for), shown on
//! a doc's notes, whose rule ("only a note's author or the page's owner",
//! applib/notes.mjs `mayChange`) trusts `call.principal`. A, a signed-in
//! reader, leaves a thread on a public doc; B, a stranger to both, has an
//! agent of their own. Invalid: B's agent naming A resolves, writes or
//! deletes nothing of A's (403), on a public doc and on a members doc they
//! both edit. Controls: B, and B's agent as itself, are refused by the
//! app's own rule (422), and a person naming `for` is 403.

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::delegation::agent_of;
use crate::api::{Api, Reply};
use crate::Suite;

fn notes(api: &Api, keys: &Keys, name: &str) -> Vec<Value> {
    api.op(keys, name, "notes", "q", json!({})).ok().and_then(|r| r.body["result"]["notes"].as_array().cloned()).unwrap_or_default()
}

fn ids(notes: &[Value]) -> Vec<i64> {
    notes.iter().filter_map(|n| n["id"].as_i64()).collect()
}

fn id_of(r: &Reply) -> i64 {
    r.body["result"]["id"].as_i64().unwrap_or(-1)
}

/// `keys` signs op `op` on `name`, for `asker`.
fn op_for(api: &Api, keys: &Keys, asker: &str, name: &str, op: &str, id: &str, input: Value) -> Result<Reply> {
    api.signed(keys, "POST", &format!("/api/f/{name}/ops/{op}?for={asker}"), Some(&json!({ "id": id, "input": input })))
}

pub fn doc_for(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("doc-for", &[]) {
        return Ok(());
    }
    // the doc's owner works from the CLI, as in the doc lane
    let home = s.dir("doc-for-home");
    s.login(api, &home);
    let owner = s.cli_keys(&home).context("the CLI logged in")?;
    let label = s.name("docfor");
    let made = s.cli_json(api, &home, &["create", &label, "--template", "doc", "--visibility", "public", "--json"]).unwrap_or_else(|e| json!({ "error": format!("{e:#}") }));
    let name = made["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(!name.is_empty() && made["visibility"] == "public", "no public doc to test: {made}");

    // A, a signed-in reader, leaves a thread; B is a stranger to A and the doc
    let (a, b) = (api.person()?, api.person()?);
    let a_id = api.identity(&a)?;
    let r = api.op(&a, &name, "annotate", "a1", json!({ "anchor": "lede", "text": "A's question" }))?;
    let thread = id_of(&r);
    let r2 = api.op(&a, &name, "annotate", "a2", json!({ "reply_to": thread, "text": "A's follow-up" }))?;
    let reply = id_of(&r2);
    anyhow::ensure!(r.status == 200 && r2.status == 200 && thread > 0 && reply > thread, "A's notes: {r} / {r2}");

    // controls: B as themselves, and B naming A with their own key, are refused today
    let as_b = api.op(&b, &name, "remove_note", "b0", json!({ "id": thread }))?;
    let b_names = op_for(api, &b, &a_id, &name, "remove_note", "b0b", json!({ "id": thread }))?;
    s.ok(
        "control: B's own delete of A's thread is refused (the app's rule), and a person naming `for` is 403",
        as_b.status == 422 && b_names.status == 403 && ids(&notes(api, &owner, &name)) == vec![thread, reply],
        format!("{as_b} / {b_names}"),
    );

    // B registers an agent of their own: nothing more is needed
    let (b_agent, b_agent_id) = agent_of(api, &b)?;
    let as_itself = api.op(&b_agent, &name, "remove_note", "g0", json!({ "id": thread }))?;
    s.ok("control: B's agent as itself is refused by the app's rule", as_itself.status == 422, &as_itself);

    // the attack: B's agent names A
    let resolved = op_for(api, &b_agent, &a_id, &name, "resolve", "g1", json!({ "id": thread, "resolved": true }))?;
    let after = notes(api, &owner, &name);
    let still_open = after.iter().find(|n| n["id"] == thread).is_some_and(|n| n["resolved"] == false);
    s.ok(
        "a stranger's agent naming A (`resolve?for=<A>`) is refused, and A's thread stays open",
        resolved.status != 200 && still_open,
        format!("{resolved} / {}", json!(after)),
    );
    let posed = op_for(api, &b_agent, &a_id, &name, "annotate", "g2", json!({ "anchor": "lede", "text": "written as A by B's agent" }))?;
    let as_a = notes(api, &owner, &name).into_iter().find(|n| n["id"] == id_of(&posed)).unwrap_or_default();
    s.ok(
        "a stranger's agent naming A (`annotate?for=<A>`) does not leave a note authored as A",
        posed.status != 200 || as_a["author"] != a_id.as_str(),
        format!("{posed} / {as_a}"),
    );
    let removed = op_for(api, &b_agent, &a_id, &name, "remove_note", "g3", json!({ "id": thread }))?;
    let left = ids(&notes(api, &owner, &name));
    s.ok(
        "a stranger's agent naming A (`remove_note?for=<A>`) is refused, and A's thread and its reply stay",
        removed.status != 200 && left.contains(&thread) && left.contains(&reply),
        format!("{removed} / left {left:?} (agent {b_agent_id})"),
    );

    // the same on a members doc: A and B both members, B's agent names A
    let r = api.signed(&owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    let b_id = api.identity(&b)?;
    let (sa, sb) = (api.share(&name, &a_id, "editor")?, api.share(&name, &b_id, "editor")?);
    anyhow::ensure!(r.status == 200 && sa.status == 200 && sb.status == 200, "members doc: {r} / {sa} / {sb}");
    let r = api.op(&a, &name, "annotate", "a3", json!({ "anchor": "lede", "text": "A's members-only note" }))?;
    let members_note = id_of(&r);
    anyhow::ensure!(r.status == 200 && members_note > 0, "A's members-only note: {r}");
    let removed = op_for(api, &b_agent, &a_id, &name, "remove_note", "g4", json!({ "id": members_note }))?;
    s.ok(
        "on a members doc, a fellow member's agent naming A (`remove_note?for=<A>`) is refused, and A's note stays",
        removed.status != 200 && ids(&notes(api, &owner, &name)).contains(&members_note),
        &removed,
    );
    Ok(())
}
