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
//! answer, and a delete racing it; and the name made again over its kept
//! repo, whose old live a refresh never takes for the new life's.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::signin::site_cookie;
use crate::api::{timing_parts, Api};
use crate::{Need, Suite};

/// code.storage's latency while a race is set up: long enough that a
/// request sent as the create answers lands inside the seed's first call.
const SLOW: Duration = Duration::from_millis(800);

/// code.storage's latency while a delete and the name made again land
/// inside one call of a seed's deploy: each takes a few dozen ms here.
const DEPLOY_SLOW: Duration = Duration::from_millis(1500);

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

    unfinished_seeds(s, api, &owner, &session)?;
    Ok(())
}

/// The seed's boundaries once a chat's create answers before code.storage:
/// its repo made and its first commit failed, its first commit in flight,
/// and its deploy in flight when its life ends. Each is set up from the
/// fake's side (its repos, its requests, its branches), so no request of
/// the test's reaches the chat before the step it checks.
fn unfinished_seeds(s: &mut Suite, api: &Api, owner: &crate::Keys, session: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let owner_id = api.identity(owner)?;
    let repo_url = |s: &Suite, name: &str| s.fake.repo_url(&fragment_core::codestorage::repo_name("", name, &owner_id).unwrap_or_default());
    let status = |name: &str| api.status(owner, name).map(|r| r.body).unwrap_or(Value::Null);
    let poll_now = |name: &str| api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "poll-now" })));
    let events = |name: &str, kind: &str| -> Vec<i64> {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/events?tail=200"), None);
        r.map(|r| r.body["events"].as_array().into_iter().flatten().filter(|e| e["kind"] == kind).filter_map(|e| e["id"].as_i64()).collect()).unwrap_or_default()
    };
    let chat = |s: &Suite, label: &str, title: &str| -> Result<String> {
        let r = api.create_with(owner, json!({ "label": s.name(label), "template": "chat", "title": title }))?;
        anyhow::ensure!(r.status == 200, "a chat: {r}");
        s.owned(&r.body, owner);
        Ok(r.body["name"].as_str().unwrap_or("").to_string())
    };
    let manifest_at = |s: &Suite, repo: &str| -> Value { s.fake.file_at(repo, "main", "fragment.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null) };

    // ---- its repo made, its first commit failed (Alex, #1042: 1 and 3)
    s.fake.fail_commit_packs(true);
    let half = match chat(s, "seed-half", "Half") {
        Ok(n) => n,
        Err(e) => {
            s.fake.fail_commit_packs(false);
            return Err(e);
        }
    };
    let failed = s.eventually(wait, || repo_url(s, &half).is_some_and(|url| s.fake.requests(&url, "POST commit-pack") >= 1));
    let url = repo_url(s, &half).unwrap_or_default();
    // then code.storage down altogether: the first request of the chat since
    s.fake.fail_all(true);
    let st = status(&half);
    let cookie = format!("fragment_site={}", site_cookie(api, session, &half)?);
    let page = api.page(&half, "", Some(&cookie))?;
    let posted = api.signed(owner, "POST", &format!("/api/f/{half}/channels/chat"), Some(&json!({ "id": "h1", "body": { "text": "said with its repo made, its template not" } })))?;
    s.fake.fail_all(false);
    s.ok(
        "its repo made and its first commit failed, then code.storage down: the chat still serves its template from the release, its status names its repo and no pins, and a message is posted",
        failed && page.status == 200 && page.text.contains("id=\"chat-page\"") && st["repo"] == url.as_str() && st["pins"] == json!({ "main": null, "live": null }) && posted.status == 200,
        format!("{st} / {} / {posted}", page.status),
    );
    // its writers, commits still refused: none goes ahead of its template
    let token = api.signed(owner, "GET", &format!("/api/f/{half}/storage-token"), None)?;
    let deploy = api.signed(owner, "POST", &format!("/api/f/{half}/deploy"), Some(&json!({})))?;
    let written = json!({ "template": "chat", "meta": { "title": "Written" } });
    let write = |key: &str| {
        let text = format!("{}\n", serde_json::to_string_pretty(&written).expect("a manifest"));
        api.signed(owner, "POST", &format!("/api/f/{half}/files"), Some(&json!({ "files": [{ "path": "fragment.json", "text": text }], "key": key })))
    };
    let early = write("half-early")?;
    s.ok(
        "its repo made, its template not landed: a storage token, a deploy and a manifest write each fail as code.storage fails its commits (502), none minted, deployed (no \"nothing to deploy\") or committed",
        [&token, &deploy, &early].iter().all(|r| r.status == 502) && s.fake.branch(&url, "main").is_none() && events(&half, "storage-token.minted").is_empty(),
        format!("{token} / {deploy} / {early}"),
    );
    // commits back: the first writer lands the template, then its own write on top
    s.fake.fail_commit_packs(false);
    let late = write("half-late")?;
    let main_after = manifest_at(s, &url);
    poll_now(&half)?;
    let settled = s.eventually(wait, || api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": half, "op": "alarm" }))).is_ok_and(|r| r.body["pollAt"].as_i64() > Some(crate::api::now_ms())));
    let token = api.signed(owner, "GET", &format!("/api/f/{half}/storage-token"), None)?;
    let (landed, minted) = (events(&half, "template"), events(&half, "storage-token.minted"));
    s.ok(
        "commits back, a manifest write lands its template first and itself on top, which no later pass overwrites; a storage token then is minted after its template",
        late.status == 200 && main_after == written && settled && manifest_at(s, &url) == written
            && landed.len() == 1 && token.status == 200 && minted.first().is_some_and(|m| landed[0] < *m),
        format!("{late} / {main_after} / {} / {token} / {landed:?} {minted:?}", manifest_at(s, &url)),
    );

    // ---- a writer while its first commit is in flight, its repo made (Alex, #1042: 1)
    s.fake.set_latency(SLOW);
    let flight = chat(s, "seed-flight", "Flight")?;
    // its seed read main in the repo it made: its commit is on its way
    let committing = s.eventually(wait, || repo_url(s, &flight).is_some_and(|url| s.fake.requests(&url, "GET branch") >= 1));
    let token = api.signed(owner, "GET", &format!("/api/f/{flight}/storage-token"), None)?;
    s.fake.set_latency(Duration::ZERO);
    let (landed, minted) = (events(&flight, "template"), events(&flight, "storage-token.minted"));
    s.ok(
        "a storage token asked once its repo is made and its first commit is in flight waits for its template: minted after it, on its one commit",
        committing && token.status == 200 && matches!((landed.first(), minted.first()), (Some(l), Some(m)) if l < m)
            && repo_url(s, &flight).is_some_and(|url| s.fake.paths(&url, "main") == ["fragment.json"]),
        format!("{token} {landed:?} {minted:?}"),
    );

    // ---- its life ends while its deploy's first live refresh is in
    // flight, the name made again (Alex, #1042: 2, at b6472432 and
    // 6e9635a3): a 90-character title makes its fragment.json a possible
    // blob pointer, which that refresh reads after listing live's files and
    // before it writes live's pin and tree; then the install reads it again
    s.fake.set_latency(DEPLOY_SLOW);
    let label = s.name("seed-deploy-gone");
    let long = format!("Seeded {}", "a".repeat(83));
    assert_eq!(long.chars().count(), 90, "a 90-character title");
    let first = api.create_with(owner, json!({ "label": label, "template": "chat", "title": long }))?;
    let gone = first.body["name"].as_str().unwrap_or("").to_string();
    // its deploy made live, and its refresh listed live's files: its read
    // of the manifest as a possible pointer is on its way
    let live_made = s.eventually(wait, || repo_url(s, &gone).is_some_and(|url| s.fake.branch(&url, "live").is_some()));
    let url = repo_url(s, &gone).unwrap_or_default();
    let listed = s.fake.requests(&url, "GET files/metadata");
    let deploying = live_made && s.eventually(wait, || s.fake.requests(&url, "GET files/metadata") > listed);
    let reads = s.fake.requests(&url, "GET file");
    let deleted = api.signed(owner, "DELETE", &format!("/api/f/{gone}"), None)?;
    // that read answered after the delete: whatever its refresh writes
    // next (live's pin and tree) it has written before the name is made again
    let read = s.eventually(wait, || s.fake.requests(&url, "GET file") > reads);
    // (a stale live pin left in the next life broke its create: it never answered)
    let again = api.create_with(owner, json!({ "name": gone, "template": "chat", "title": "Seeded after its deploy" }));
    let again = match again {
        Ok(r) => {
            s.owned(&r.body, owner);
            r.status.to_string()
        }
        Err(e) => format!("{e:#}"),
    };
    s.fake.set_latency(Duration::ZERO);
    poll_now(&gone)?;
    let healed = s.eventually(wait, || {
        let st = status(&gone);
        st["repo"].as_str().is_some_and(|r| !r.is_empty()) && st["pins"]["live"].is_string() && st["pins"]["main"] == st["pins"]["live"]
    });
    let st = status(&gone);
    let manifest = manifest_at(s, st["repo"].as_str().unwrap_or(""));
    s.ok(
        "a chat deleted while its first deploy's live refresh is in flight, the name made again: the delete waits for that step, the old seed writes nothing into the new life, which is made (no stale pin) and lands its own template, its title on main",
        first.status == 200 && deploying && deleted.status == 200 && read && again == "200" && healed
            && manifest == json!({ "template": "chat", "meta": { "title": "Seeded after its deploy" } }) && events(&gone, "template").len() == 1,
        format!("{deleted} {again} {st} {manifest}"),
    );
    // ---- a deleted chat's repo kept with its live, the name made again by
    // its owner, its template's commit failing (#1072): a refresh lands its
    // template first, as any writer does, and never pins the deleted life's
    // live for this one's, so no later pass takes that live for its template
    // landed; with commits back, the alarm's pass lands its own
    let kept = chat(s, "seed-kept", "The first life")?;
    let first_landed = s.eventually(wait, || status(&kept)["pins"]["live"].is_string());
    let old_live = status(&kept)["pins"]["live"].as_str().unwrap_or("").to_string();
    let url = repo_url(s, &kept).unwrap_or_default();
    let packs = s.fake.requests(&url, "POST commit-pack");
    let deleted = api.signed(owner, "DELETE", &format!("/api/f/{kept}"), None)?;
    s.fake.fail_commit_packs(true);
    let again = api.create_with(owner, json!({ "name": kept, "template": "chat", "title": "The second life" }));
    let again = match again {
        Ok(r) => r,
        Err(e) => {
            s.fake.fail_commit_packs(false);
            return Err(e);
        }
    };
    s.owned(&again.body, owner);
    // its seed found the kept repo, and its commit was refused
    let tried = s.eventually(wait, || s.fake.requests(&url, "POST commit-pack") > packs);
    let refreshed = api.signed(owner, "POST", &format!("/api/f/{kept}/refresh"), Some(&json!({})))?;
    poll_now(&kept)?;
    let pending_st = status(&kept);
    let landed_while_down = events(&kept, "template");
    s.fake.fail_commit_packs(false);
    s.ok(
        "a deleted chat's name made again by its owner, its repo kept with its live and its own template's commit refused: a refresh fails as that commit does (502), and nothing pins the old live as this life's or lands its template",
        first_landed && deleted.status == 200 && again.status == 200 && tried && refreshed.status == 502
            && pending_st["pins"]["live"].is_null() && landed_while_down.is_empty(),
        format!("{refreshed} / {pending_st} / {landed_while_down:?}"),
    );
    poll_now(&kept)?;
    let healed = s.eventually(wait, || {
        let st = status(&kept);
        st["pins"]["live"].as_str().is_some_and(|live| live != old_live) && st["code"]["sha"] == st["pins"]["live"]
    });
    let st = status(&kept);
    s.ok(
        "commits back, its own template lands: committed and deployed in this life, its title on main, live past the old one and installed from it",
        healed && manifest_at(s, &url) == json!({ "template": "chat", "meta": { "title": "The second life" } }) && events(&kept, "template").len() == 1,
        format!("{st} {}", manifest_at(s, &url)),
    );
    pending_install_held(s, api, owner)
}

/// A chat's create installs its template from the release (plane.rs
/// `install_pending`) holding the plane's lock, as every install does
/// (Codex, #1042 at cd21e345). Its install held where its release's reads
/// are (`hold-install`): a delete, and the name made again as an app,
/// waits for it, never landing its own life to have the chat's install
/// written over it; and a writer's seed, its install from live, comes after
/// it, never overwritten by it. Each sequence's other side is let run for
/// `LOCKED` before the hold is let go: with the lock it cannot finish.
fn pending_install_held(s: &mut Suite, api: &Api, owner: &crate::Keys) -> Result<()> {
    const LOCKED: Duration = Duration::from_secs(5);
    let wait = Duration::from_secs(30);
    let lever = |name: &str, on: Option<bool>| -> Value {
        let mut body = json!({ "fragment": name, "op": "hold-install" });
        if let Some(on) = on {
            body["on"] = json!(on);
        }
        api.unsigned("POST", "/api/test/fragment", Some(&body)).map(|r| r.body).unwrap_or(Value::Null)
    };
    let status = |name: &str| api.status(owner, name).map(|r| r.body).unwrap_or(Value::Null);
    let events = |name: &str, kind: &str| -> usize {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/events?tail=200"), None);
        r.map(|r| r.body["events"].as_array().into_iter().flatten().filter(|e| e["kind"] == kind).count()).unwrap_or(0)
    };

    // ---- held, its life deleted and the name made again as an app
    let gone = s.named(api, owner, "seed-install-gone")?;
    lever(&gone, Some(true));
    let (made, others, ran, held) = std::thread::scope(|scope| {
        let made = scope.spawn(|| api.create_with(owner, json!({ "name": gone, "template": "chat", "title": "Held install" })));
        let held = s.eventually(wait, || lever(&gone, None)["waiting"] == true);
        let others = scope.spawn(|| -> Result<(u16, u16)> {
            let deleted = api.signed(owner, "DELETE", &format!("/api/f/{gone}"), None)?.status;
            let again = api.create_with(owner, json!({ "name": gone, "template": "todo" }))?.status;
            Ok((deleted, again))
        });
        // with the lock, the delete waits for the install: they cannot end
        let ran = s.eventually(LOCKED, || others.is_finished());
        lever(&gone, Some(false));
        (made.join().expect("the create's thread"), others.join().expect("the delete's thread"), ran, held)
    });
    let made = made?;
    let (deleted, again) = others?;
    if again == 200 {
        s.owned(&json!({ "name": gone }), owner);
    }
    let landed = s.eventually(wait, || events(&gone, "template") >= 1);
    let st = status(&gone);
    s.ok(
        "a chat's create, its install from the release held, the name deleted and made again as an app: the delete waits for the install, the app's life lands its own template, and nothing of the chat is installed in it",
        held && !ran && made.status == 200 && deleted == 200 && again == 200 && landed
            && st["code"]["id"].as_str().is_some_and(|id| !id.starts_with("blessed:")) && st["code"]["sha"] == st["pins"]["live"] && st["kind"] == "app",
        format!("held {held} ran {ran} create {} delete {deleted} again {again} landed {landed} {st}", made.status),
    );

    // ---- held, a writer's seed beside it
    let beside = s.named(api, owner, "seed-install-beside")?;
    lever(&beside, Some(true));
    let (made, token, ran, held) = std::thread::scope(|scope| {
        let made = scope.spawn(|| api.create_with(owner, json!({ "name": beside, "template": "chat", "title": "Held beside" })));
        let held = s.eventually(wait, || lever(&beside, None)["waiting"] == true);
        let token = scope.spawn(|| api.signed(owner, "GET", &format!("/api/f/{beside}/storage-token"), None));
        // with the lock, the seed's commit and install wait for it
        let ran = s.eventually(LOCKED, || token.is_finished());
        lever(&beside, Some(false));
        (made.join().expect("the create's thread"), token.join().expect("the token's thread"), ran, held)
    });
    let made = made?;
    s.owned(&made.body, owner);
    let token = token?;
    let st = status(&beside);
    s.ok(
        "a chat's create, its install from the release held, a writer's seed beside it: the seed waits, then installs live over it, never under it (its code is live's)",
        held && !ran && made.status == 200 && token.status == 200
            && st["pins"]["live"].is_string() && st["code"]["sha"] == st["pins"]["live"] && st["code"]["id"].as_str().is_some_and(|id| id.starts_with("blessed:chat@")),
        format!("held {held} ran {ran} create {} {token} {st}", made.status),
    );
    Ok(())
}
