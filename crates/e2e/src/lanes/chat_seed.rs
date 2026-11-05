//! A chat's create answers before its repo is made (docs/api.md, Names;
//! cell publish.rs `seeds_later`): the chat runs its template from the
//! release until its first commit lands, which comes right after the
//! answer, else at the alarm's pass, else first by a writer of its repo.
//! Valid: a chat made while code.storage is down is usable at once, keeps
//! working across a restart of every object, and heals once code.storage
//! is back. Invalid: the other templates still make their repo before
//! answering, and a writer of the repo while code.storage is down fails
//! as code.storage does, leaving the chat as it was. Replay: the alarm's
//! pass again changes nothing. Races: a writer racing the seed after the
//! answer, and a delete racing it.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::signin::site_cookie;
use crate::api::{timing_parts, Api};
use crate::{Need, Suite};

/// code.storage's latency while a race is set up: long enough that a
/// request sent as the create answers lands inside the seed's first call.
const SLOW: Duration = Duration::from_millis(800);

pub fn chat_seed(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("chat-seed", &[Need::Fakes, Need::Node, Need::Levers]) {
        return Ok(());
    }
    let wait = Duration::from_secs(30);
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let session = api.sign_in(&Api::email_of(&owner))?;
    let poll_now = |name: &str| api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "poll-now" })));
    let status = |name: &str| api.status(&owner, name).map(|r| r.body).unwrap_or(Value::Null);
    let repo_name = |name: &str| fragment_core::codestorage::repo_name("", name, &owner_id).unwrap_or_default();
    let chat = |s: &Suite, label: &str| -> Result<(String, crate::api::Reply)> {
        let r = api.create_with(&owner, json!({ "label": s.name(label), "template": "chat", "title": "Seeded" }))?;
        s.owned(&r.body, &owner);
        Ok((r.body["name"].as_str().with_context(|| format!("a chat: {r}"))?.to_string(), r))
    };

    // ---- made while code.storage is down: answered, and usable at once
    s.fake.fail_all(true);
    let (name, made) = chat(s, "seed-down")?;
    let waited = timing_parts(&made.header("server-timing"));
    s.ok(
        "a chat made while code.storage is down is made: its answer names no repo yet, and its waits no repo nor seed, its template installed from the release",
        made.status == 200 && made.body["repo"] == "" && !waited.iter().any(|p| p == "repo" || p == "seed") && waited.iter().any(|p| p == "install"),
        format!("{made} {}", made.header("server-timing")),
    );
    let usable = |s: &mut Suite, when: &str, id: &str| -> Result<()> {
        let st = status(&name);
        let posted = api.signed(&owner, "POST", &format!("/api/f/{name}/channels/chat"), Some(&json!({ "id": id, "body": { "text": format!("said {when}") } })))?;
        let cookie = format!("fragment_site={}", site_cookie(api, &session, &name)?);
        let page = api.page(&name, "", Some(&cookie))?;
        let files = api.signed(&owner, "GET", &format!("/api/f/{name}/files"), None)?;
        s.ok(
            &format!("{when}, it is a chat: its status says its template's code and no repo, a message is posted, its page is the chat's, and its files are none"),
            st["repo"] == "" && st["pins"] == json!({ "main": null, "live": null }) && st["code"]["id"].as_str().is_some_and(|id| id.starts_with("blessed:chat@"))
                && st["code"]["operations"]["notify_reply"].is_object()
                && posted.status == 200
                && page.status == 200 && page.text.contains("id=\"chat-page\"")
                && files.status == 200 && files.body["files"].as_array().is_some_and(Vec::is_empty),
            format!("{st} / {posted} / {} / {files}", page.status),
        );
        Ok(())
    };
    usable(s, "before its repo is made", "m1")?;
    // a writer of its repo while code.storage is down fails as code.storage does
    let token = api.signed(&owner, "GET", &format!("/api/f/{name}/storage-token"), None)?;
    s.ok(
        "a writer of its repo then (a storage token) fails as code.storage does (502), and the chat is as it was",
        token.status == 502 && status(&name)["repo"] == "" && api.signed(&owner, "GET", &format!("/api/f/{name}/status"), None)?.status == 200,
        &token,
    );

    // ---- a restart of every object between the create and its seed
    s.reload()?;
    usable(s, "after a restart, its repo still not made", "m2")?;

    // ---- code.storage back: the alarm's pass makes its repo and lands its template
    s.fake.fail_all(false);
    poll_now(&name)?;
    let landed = s.eventually(wait, || {
        let st = status(&name);
        st["repo"].as_str().is_some_and(|r| !r.is_empty()) && st["pins"]["live"].is_string() && st["pins"]["main"] == st["pins"]["live"]
    });
    let st = status(&name);
    let repo = st["repo"].as_str().unwrap_or("").to_string();
    let manifest: Value = s.fake.file_at(&repo, "main", "fragment.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let said = api.signed(&owner, "GET", &format!("/api/f/{name}/channels/chat?after=0&limit=10"), None)?;
    s.ok(
        "code.storage back, the alarm's pass makes its repo (its owner's name) and lands its template: main and live at its one commit, the code installed from live, the messages said before kept",
        landed
            && s.fake.repo_url(&repo_name(&name)).as_deref() == Some(repo.as_str())
            && manifest == json!({ "template": "chat", "meta": { "title": "Seeded" } })
            && s.fake.paths(&repo, "main") == ["fragment.json"]
            && st["code"]["sha"] == st["pins"]["live"]
            && said.body["records"].as_array().is_some_and(|r| r.len() == 2),
        format!("{st} {manifest} {said}"),
    );
    // replay: another pass changes nothing
    let packs = s.fake.requests(&repo, "POST commit-pack");
    poll_now(&name)?;
    let again = s.eventually(wait, || api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "alarm" }))).is_ok_and(|r| r.body["pollAt"].as_i64() > Some(crate::api::now_ms())));
    s.ok(
        "the alarm's pass again commits nothing more: its pins and its one commit stay",
        again && status(&name)["pins"] == st["pins"] && s.fake.requests(&repo, "POST commit-pack") == packs,
        status(&name),
    );

    // ---- the other templates make their repo before answering
    let made: Vec<(String, Value)> = ["agent", "todo"]
        .into_iter()
        .map(|t| {
            let r = api.create_with(&owner, json!({ "label": s.name(&format!("seed-{t}")), "template": t }))?;
            s.owned(&r.body, &owner);
            Ok((t.to_string(), json!({ "status": r.status, "repo": r.body["repo"], "waited": timing_parts(&r.header("server-timing")) })))
        })
        .collect::<Result<_>>()?;
    s.ok(
        "an agent's and a template app's create still make their repo and land their template before answering",
        made.iter().all(|(_, m)| m["status"] == 200 && m["repo"].as_str().is_some_and(|r| !r.is_empty()) && m["waited"].as_array().is_some_and(|w| w.contains(&json!("repo")) && w.contains(&json!("seed")))),
        json!(made),
    );

    // ---- a writer racing the seed after the answer
    s.fake.set_latency(SLOW);
    let (raced, _) = chat(s, "seed-race")?;
    let token = api.signed(&owner, "GET", &format!("/api/f/{raced}/storage-token"), None)?;
    s.fake.set_latency(Duration::ZERO);
    let st = status(&raced);
    let repo = st["repo"].as_str().unwrap_or("").to_string();
    s.ok(
        "a writer as the create answers (a storage token) waits for the seed it races: its token names the repo made once, with the template's one commit under it",
        token.status == 200 && !repo.is_empty() && token.body["repo"] == repo.as_str() && st["pins"]["live"].is_string()
            && s.fake.requests(&repo, "POST commit-pack") == 1 && s.fake.paths(&repo, "main") == ["fragment.json"],
        format!("{token} {st}"),
    );

    // ---- a delete racing the seed, then the name made again: the new
    // life finds the repo (the same owner's, kept as a deleted life's is)
    s.fake.set_latency(SLOW);
    let label = s.name("seed-gone");
    let first = api.create_with(&owner, json!({ "label": label, "template": "chat", "title": "Seeded" }))?;
    let gone = first.body["name"].as_str().unwrap_or("").to_string();
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{gone}"), None)?;
    let missing = api.status(&owner, &gone)?.status;
    let again = api.create_with(&owner, json!({ "name": gone, "template": "chat", "title": "Seeded again" }))?;
    s.owned(&again.body, &owner);
    s.fake.set_latency(Duration::ZERO);
    poll_now(&gone)?;
    let landed = s.eventually(wait, || status(&gone)["pins"]["live"].is_string());
    let st = status(&gone);
    let manifest: Value = s.fake.file_at(st["repo"].as_str().unwrap_or(""), "main", "fragment.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    s.ok(
        "a chat deleted as its seed runs is gone at once (404), and the same name made again lands in the repo its owner's name finds, its own template on main",
        first.status == 200 && deleted.status == 200 && missing == 404 && again.status == 200 && landed
            && s.fake.repo_url(&repo_name(&gone)).as_deref() == st["repo"].as_str()
            && manifest == json!({ "template": "chat", "meta": { "title": "Seeded again" } }),
        format!("{deleted} {missing} {again} {st} {manifest}"),
    );
    Ok(())
}
