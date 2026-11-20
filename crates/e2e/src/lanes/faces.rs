//! An agent's name, role and picture (docs/api.md, "An agent's name and
//! picture"): its title and role its cell's own (`PUT /api/f/{name}/face`,
//! no commit and no deploy, FIN-541), its picture its `avatar.png` at main
//! (a commit, no deploy), the one source everything shows it by. Renamed
//! and given a picture through the routes Settings and `fragment agent`
//! use, it reaches its owner's list (what the shell draws), a chat's
//! `__people` (what its page draws), the share sheet, the access view and
//! its picture's own address. The same change again commits and records
//! nothing new (replay); a picture too large or that is no picture, a title
//! blank or too long, a role too long or a chat's, and someone else's agent
//! are refused, the face kept; taken away, the picture is gone everywhere.
//! A rename says its name in its job in the same request: refused there,
//! neither changes; an answer lost, it is looked for at main. An older CLI,
//! renaming by its `fragment.json`, still renames it (the debt ledger). A
//! node's restart keeps it all (restart.rs).

use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use fragment_nip98::Keys;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::signin::{site_cookie, with_session};
use super::templates::person;
use crate::api::{Api, Call, Reply};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(30);

/// Two pictures, 16×16 PNGs: orange and blue.
const ORANGE: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAFklEQVR42mO46a9NEmIY1TCqYfhqAADneVMQLy52+gAAAABJRU5ErkJggg==";
const BLUE: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAFklEQVR42mPQrrpJEmIY1TCqYfhqAABnSH4QybTRQgAAAABJRU5ErkJggg==";

/// A picture's bytes from its base64.
pub(super) fn picture(b64: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(b64).expect("a fixture PNG")
}

/// The orange picture, for another lane's agent.
pub(super) fn orange() -> Vec<u8> {
    picture(ORANGE)
}

/// The blue picture, distinct from the icon lane's orange photograph.
pub(super) fn blue() -> Vec<u8> {
    picture(BLUE)
}

/// Where an identity's picture of `bytes` is served (fragment_core::face).
pub(super) fn picture_path(identity: &str, bytes: &[u8]) -> String {
    fragment_core::face::picture_path(identity, &hex::encode(Sha256::digest(bytes)))
}

/// An agent of `owner`'s: a fragment on the agent template titled `title`,
/// run by their computer (which gives it its identity, and is never woken
/// here). Its fragment and its identity.
pub(super) fn agent_of(s: &Suite, api: &Api, owner: &Keys, base: &str, title: &str) -> Result<(Value, String)> {
    let name = s.named(api, owner, base)?;
    let r = api.create_with(owner, json!({ "name": name, "template": "agent", "title": title }))?;
    anyhow::ensure!(r.status == 200, "the agent fragment: {r}");
    s.owned(&r.body, owner);
    let r = api.signed(owner, "POST", "/api/computers", Some(&json!({})))?;
    let computer = r.body["computer"].as_str().with_context(|| format!("a computer: {r}"))?.to_string();
    let r = api.signed(owner, "PUT", &format!("/api/computers/{computer}/agents/{name}"), Some(&json!({})))?;
    let id = r.body["agents"].as_array().into_iter().flatten().find(|a| a["fragment"] == name.as_str()).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string();
    anyhow::ensure!(fragment_core::npub::is_identity(&id), "its computer took it: {r}");
    let made = api.status(owner, &name)?.body;
    Ok((json!({ "name": name, "repo": made["repo"] }), id))
}

/// An agent's name and picture as Settings and `fragment agent` set them:
/// its title through its face (when one), its `avatar.png` (`Some` bytes,
/// or `None` to take it away; untouched when not asked) a commit to main
/// (`key`, when one), with no deploy. The first answer (its face's, else
/// its files'), and its files' when both were asked.
pub(super) fn set_face(api: &Api, keys: &Keys, agent: &str, title: Option<&str>, avatar: Option<Option<&[u8]>>, key: Option<&str>) -> Result<(Reply, Option<Reply>)> {
    let named = title.map(|t| api.signed(keys, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": t })))).transpose()?;
    let file = match avatar {
        Some(Some(bytes)) => Some(json!({ "path": "avatar.png", "base64": base64::engine::general_purpose::STANDARD.encode(bytes) })),
        Some(None) => Some(json!({ "path": "avatar.png", "delete": true })),
        None => None,
    };
    let pictured = match file {
        Some(file) => {
            let mut body = json!({ "message": "its picture", "files": [file] });
            if let Some(key) = key {
                body["key"] = json!(key);
            }
            Some(api.signed(keys, "POST", &format!("/api/f/{agent}/files"), Some(&body))?)
        }
        None => None,
    };
    match (named, pictured) {
        (Some(named), pictured) => Ok((named, pictured)),
        (None, Some(pictured)) => Ok((pictured, None)),
        (None, None) => anyhow::bail!("set_face asked for nothing"),
    }
}

/// `name`'s row in `keys`' list (`GET /api/fragments`): what the shell draws.
pub(super) fn row(api: &Api, keys: &Keys, name: &str) -> Value {
    let r = api.signed(keys, "GET", "/api/fragments", None).ok();
    r.and_then(|r| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name).cloned())).unwrap_or(Value::Null)
}

/// `id`'s profile as `chat`'s page asks it (`__people`, with a site cookie).
pub(super) fn people(api: &Api, chat: &str, cookie: &str, id: &str) -> Value {
    let r = api.page(chat, &format!("__people?id={id}"), Some(&format!("fragment_site={cookie}"))).ok();
    r.map(|r| r.body["profiles"][id].clone()).unwrap_or(Value::Null)
}

/// The picture at `path` (on the platform), as anyone fetches it.
pub(super) fn fetched(api: &Api, path: &str) -> Result<Reply> {
    api.call(Call { method: "GET", url: format!("{}{path}", api.base), ..Call::default() })
}

/// How many of `kind` a fragment's newest events hold.
fn events(api: &Api, keys: &Keys, name: &str, kind: &str) -> usize {
    super::site::event_kinds(api, keys, name).iter().filter(|k| *k == kind).count()
}

/// A JPEG of more than `bytes`, its picture the icon lane's 40×24
/// photograph: comment segments after its start (the shell lane's photo).
pub(super) fn heavy_jpeg(bytes: usize) -> Vec<u8> {
    let photo = super::icons::photo();
    let mut out = photo[..2].to_vec();
    let comment = vec![b'x'; 60_000];
    while out.len() <= bytes {
        out.extend_from_slice(&[0xFF, 0xFE]);
        out.extend_from_slice(&((comment.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&comment);
    }
    out.extend_from_slice(&photo[2..]);
    out
}

pub fn faces(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("faces", &[crate::Need::Computers]) {
        return Ok(());
    }
    let (orange, blue) = (picture(ORANGE), picture(BLUE));
    let (paul, session) = person(api)?;
    let (created, agent_id) = agent_of(s, api, &paul, "faces-juniper", "Juniper")?;
    let agent = created["name"].as_str().unwrap_or("").to_string();
    let label = fragment_proto::split_fragment_name(&agent).map(|(l, _)| l.to_string()).unwrap_or_default();
    // a chat of Paul's, where its page asks who is who
    let chat = s.named(api, &paul, "faces-chat")?;
    let r = api.create_with(&paul, json!({ "name": chat, "template": "chat", "title": "Garden" }))?;
    anyhow::ensure!(r.status == 200, "the chat: {r}");
    let cookie = site_cookie(api, &session, &chat)?;
    let path_of = |bytes: &[u8]| picture_path(&agent_id, bytes);

    // made: its title, no picture
    let made = row(api, &paul, &agent);
    let asked = people(api, &chat, &cookie, &agent_id);
    let none = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
    s.ok(
        "an agent fresh from the agent template shows by its fragment's title, with no picture (404 at its address)",
        made["title"] == "Juniper" && made.get("picture").is_none() && asked["title"] == "Juniper" && asked["name"] == label.as_str() && asked["picture"].is_null() && none.status == 404,
        json!({ "row": made, "people": asked, "picture": none.status }),
    );

    // renamed, and given a picture, as Settings and `fragment agent` do:
    // its face, and its avatar.png on main, with no deploy (FIN-541): live
    // never holds that commit (a fragment with no repo yet lands its
    // template's own deploy first)
    let (named, wrote) = set_face(api, &paul, &agent, Some("Frank"), Some(Some(&orange)), Some("face-1"))?;
    let wrote = wrote.with_context(|| format!("the face: {named}"))?;
    let shown = path_of(&orange);
    let listed = s.eventually(WAIT, || {
        let r = row(api, &paul, &agent);
        r["title"] == "Frank" && r["picture"] == shown.as_str()
    });
    let live = api.status(&paul, &agent)?.body["pins"]["live"].clone();
    s.ok(
        "renamed through its face and given a picture by its avatar.png, at once: its owner's list (what the shell draws) names it and shows its picture's address, and its picture's commit was never deployed",
        named.status == 200 && named.body["title"] == "Frank" && wrote.status == 200 && listed && wrote.body["commit"].is_string() && live != wrote.body["commit"],
        json!({ "face": named.body, "files": wrote.body, "row": row(api, &paul, &agent), "live": live }),
    );
    let asked = people(api, &chat, &cookie, &agent_id);
    s.ok(
        "a chat's page is told so too (`__people`): its title and its picture, whole; its @name stays its label",
        asked["title"] == "Frank" && asked["picture"].as_str().is_some_and(|p| p.starts_with("http") && p.ends_with(&shown)) && asked["name"] == label.as_str(),
        &asked,
    );
    let got = fetched(api, &shown)?;
    s.ok(
        "its picture is served at its identity's address to anyone, the bytes main's read checked, as an image",
        got.status == 200 && got.bytes == orange && got.header("content-type") == "image/png" && got.header("cache-control").contains("public"),
        json!({ "status": got.status, "type": got.header("content-type"), "bytes": got.bytes.len() }),
    );
    let sheet = with_session(api, "GET", &format!("/share/{agent}"), &session)?;
    s.ok(
        "the share sheet names it so, with its picture",
        sheet.status == 200 && sheet.text.contains("Frank") && sheet.text.contains(&format!("src=\"{}\"", shown.replace('&', "&amp;"))),
        &sheet,
    );
    // who has access to what asks the members of what someone else is in:
    // the agent in Paul's chat, and Bob a viewer there for a moment (its own
    // fragment is Paul's alone: FIN-544)
    let (bob, _) = person(api)?;
    let bob_id = api.identity(&bob)?;
    let r = api.signed(&paul, "PUT", &format!("/api/f/{chat}/members/{agent_id}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent in the chat: {r}");
    let r = api.share(&chat, &bob_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "Bob, a viewer: {r}");
    let access = api.signed(&paul, "GET", "/api/access", None)?;
    let member = access.body["fragments"].as_array().into_iter().flatten().find(|f| f["name"] == chat.as_str()).and_then(|f| f["members"].as_array()?.iter().find(|m| m["principal"] == agent_id.as_str()).cloned());
    s.ok("who has access to what names it so too (its title, beside its label)", member.as_ref().is_some_and(|m| m["title"] == "Frank" && m["name"] == label.as_str()), &access);
    let r = api.signed(&paul, "DELETE", &format!("/api/f/{chat}/members/{bob_id}"), None)?;
    anyhow::ensure!(r.status == 200, "Bob, out again: {r}");

    // replay: the same change again, by its key or by its files, commits
    // nothing new, and its face records no second change
    let tip = wrote.body["commit"].clone();
    let changes = events(api, &paul, &agent, "face.changed");
    let (renamed, again) = set_face(api, &paul, &agent, Some(" Frank "), Some(Some(&orange)), Some("face-1"))?;
    let (same, _) = set_face(api, &paul, &agent, None, Some(Some(&orange)), None)?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "the same change again (padded), under its key or as the same files, commits nothing new and writes no face.changed, and its name and picture's address stay",
        renamed.status == 200 && again.is_some_and(|a| a.body["commit"] == tip) && same.body["commit"] == tip && changes == 1 && events(api, &paul, &agent, "face.changed") == 1 && kept["title"] == "Frank" && kept["picture"] == shown.as_str(),
        json!({ "first": tip, "same": same.body, "events": super::site::event_kinds(api, &paul, &agent), "row": kept }),
    );

    // invalid: written through the platform, refused before anything is committed
    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.resize(fragment_proto::limits::PICTURE_MAX_BYTES + 1, 0);
    let (large, _) = set_face(api, &paul, &agent, None, Some(Some(&big)), None)?;
    let (text, _) = set_face(api, &paul, &agent, None, Some(Some(b"hello, I am no picture")), None)?;
    let unmoved = api.signed(&paul, "GET", &format!("/api/f/{agent}/status"), None)?.body["pins"]["main"].clone();
    s.ok(
        "a picture past 256 KiB is 413 and one that is no picture 400, each naming avatar.png and why, and nothing is committed",
        large.status == 413
            && large.text.contains("avatar.png")
            && large.text.contains("262144")
            && text.status == 400
            && text.text.contains("avatar.png is not a picture")
            && unmoved == tip,
        json!({ "large": large.to_string(), "text": text.to_string(), "main": unmoved, "tip": tip }),
    );
    // invalid through its face: refused, nothing changed
    let face = |body: Value, keys: &Keys| api.signed(keys, "PUT", &format!("/api/f/{agent}/face"), Some(&body));
    let long = face(json!({ "title": "F".repeat(fragment_proto::limits::TITLE_MAX_CHARS + 1) }), &paul)?;
    let blank = face(json!({ "title": "   " }), &paul)?;
    let nothing = face(json!({}), &paul)?;
    let pictured = face(json!({ "picture": shown }), &paul)?;
    let stale = face(json!({ "title": "Fred", "was": "Juniper" }), &paul)?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "through its face, a title past 120 characters, one of only spaces, a change naming nothing or a picture are refused (400) saying why, and a rename whose `was` is no longer its title (409); its name and picture stay",
        [&long, &blank, &nothing, &pictured].iter().all(|r| r.status == 400) && stale.status == 409 && long.text.contains("1 to 120 characters") && kept["title"] == "Frank" && kept["picture"] == shown.as_str(),
        json!({ "long": long.to_string(), "blank": blank.to_string(), "nothing": nothing.to_string(), "picture": pictured.to_string(), "stale": stale.to_string(), "row": kept }),
    );
    if s.hosted() {
        s.skip("a picture pushed through git that is no picture shows none", "a preview's git is written through the platform's files route, which refuses it first (above)");
    } else {
        s.commit(&created, &[("avatar.png", Some(b"<svg/>"))]);
        let none = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
        let served = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
        s.ok(
            "a picture pushed through git that is no picture shows none, saying why (`picture.refused`), and its name stays",
            none && served.status == 404 && events(api, &paul, &agent, "picture.refused") == 1 && row(api, &paul, &agent)["title"] == "Frank",
            json!({ "served": served.status, "row": row(api, &paul, &agent) }),
        );
        set_face(api, &paul, &agent, None, Some(Some(&orange)), None)?;
        anyhow::ensure!(s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == shown.as_str()), "its picture again");
    }
    // someone else's agent: Bob, no member of it now, changes nothing
    let (theirs, _) = set_face(api, &bob, &agent, None, Some(Some(&blue)), None)?;
    let (renamed, _) = set_face(api, &bob, &agent, Some("Bobbed"), None, None)?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "someone else's agent: another person renames it or writes its picture, refused (403 or 404), its name and picture unchanged",
        matches!(theirs.status, 403 | 404) && matches!(renamed.status, 403 | 404) && kept["title"] == "Frank" && kept["picture"] == shown.as_str(),
        json!({ "files": theirs.to_string(), "face": renamed.to_string(), "row": kept }),
    );
    let nobody = fetched(api, "/api/identities/npub1nobody/picture")?;
    s.ok("an address that names no identity serves no picture (404)", nobody.status == 404, &nobody);
    // an app's avatar.png is a file like any other: only an agent's is its picture
    let app = s.named(api, &paul, "faces-app")?;
    s.create(api, &paul, &app)?;
    let r = api.signed(&paul, "POST", &format!("/api/f/{app}/files"), Some(&json!({ "files": [{ "path": "avatar.png", "text": "not a picture, and no one's face" }] })))?;
    s.ok("an app's avatar.png is a file like any other (only an agent fragment's is a picture)", r.status == 200, &r);

    // a new picture is a new address; taken away, it is gone everywhere
    let (fixed, _) = set_face(api, &paul, &agent, None, Some(Some(&blue)), None)?;
    let renewed = path_of(&blue);
    let moved = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == renewed.as_str());
    let got = fetched(api, &renewed)?;
    s.ok(
        "a new picture is a new address, serving it",
        fixed.status == 200 && moved && renewed != shown && got.status == 200 && got.bytes == blue,
        json!({ "row": row(api, &paul, &agent), "status": got.status }),
    );
    let (gone, _) = set_face(api, &paul, &agent, None, Some(None), None)?;
    let removed = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
    let asked = people(api, &chat, &cookie, &agent_id);
    let none = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
    s.ok(
        "its picture taken away (avatar.png deleted), no list, page or address shows one, and its name stays",
        gone.status == 200 && removed && asked["picture"].is_null() && asked["title"] == "Frank" && none.status == 404,
        json!({ "row": row(api, &paul, &agent), "people": asked, "picture": none.status }),
    );

    // an older CLI's rename (`fragment agent --name`, `fragment rename`; the
    // one a computer's image carries): `GET manifest` (its title its
    // face's, and a mark of the read), its fragment.json written back with
    // the new title, then a deploy. It renames it, back to a title git holds
    // already too; the same write again (its key) after a rename through
    // its face, a deploy writing no fragment.json, or the file written as
    // git holds it (its mark taken), rename nothing
    let old_rename = |name: &str, title: &str, key: Option<&str>| -> Result<(Value, bool, Value)> {
        let m = api.signed(&paul, "GET", &format!("/api/f/{name}/manifest"), None)?;
        // (an app with no fragment.json: one of its own)
        let mut body = if m.status == 200 { m.body.clone() } else { json!({}) };
        body["meta"]["title"] = json!(title);
        let mut files = json!({ "message": "its name", "files": [{ "path": "fragment.json", "text": format!("{}\n", serde_json::to_string_pretty(&body)?) }] });
        if let Some(key) = key {
            files["key"] = json!(key);
        }
        let wrote = api.signed(&paul, "POST", &format!("/api/f/{name}/files"), Some(&files))?.status == 200;
        let done = wrote && api.signed(&paul, "POST", &format!("/api/f/{name}/deploy"), Some(&json!({})))?.status == 200;
        Ok((m.body["meta"]["title"].clone(), done, files))
    };
    let reface = |name: &str, title: &str| -> Result<bool> { Ok(api.signed(&paul, "PUT", &format!("/api/f/{name}/face"), Some(&json!({ "title": title })))?.status == 200) };
    let title_of = |name: &str| row(api, &paul, name)["title"].clone();
    let (read, done, sent) = old_rename(&agent, "Gus", Some("legacy-rename-1"))?;
    let first = read == "Frank" && done && title_of(&agent) == "Gus";
    let refaced = reface(&agent, "Frank")?;
    let (_, again, _) = old_rename(&agent, "Gus", None)?;
    let again = refaced && again && title_of(&agent) == "Gus";
    let replayed = reface(&agent, "Frank")?
        && api.signed(&paul, "POST", &format!("/api/f/{agent}/files"), Some(&sent))?.status == 200
        && api.signed(&paul, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})))?.status == 200
        && title_of(&agent) == "Frank";
    let unrelated = api.signed(&paul, "POST", &format!("/api/f/{agent}/files"), Some(&json!({ "message": "a note", "files": [{ "path": "notes.md", "text": "a note\n" }] })))?.status == 200
        && api.signed(&paul, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})))?.status == 200
        && title_of(&agent) == "Frank";
    let app_first = reface(&app, "Faces app")? && old_rename(&app, "Garden app", None)?.1 && title_of(&app) == "Garden app";
    let app_back = reface(&app, "Faces app")? && old_rename(&app, "Garden app", None)?.1 && title_of(&app) == "Garden app";
    let raw = api.signed(&paul, "GET", &format!("/api/f/{app}/file?path=fragment.json"), None)?.text;
    let mut rewritten: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    rewritten["meta"]["description"] = json!("an app");
    let files = json!({ "message": "its manifest, as git holds it", "files": [{ "path": "fragment.json", "text": rewritten.to_string() }] });
    let raw_kept = reface(&app, "Faces app")?
        && api.signed(&paul, "POST", &format!("/api/f/{app}/files"), Some(&files))?.status == 200
        && api.signed(&paul, "POST", &format!("/api/f/{app}/deploy"), Some(&json!({})))?.status == 200
        && title_of(&app) == "Faces app";
    s.ok(
        "an older CLI's rename (its manifest read, fragment.json written back with the new title, deployed) renames an agent and an app, back to the title git holds after a rename through its face too; its same write again after a newer rename, a deploy writing no fragment.json, and the file written as git holds it rename nothing",
        first && again && replayed && unrelated && app_first && app_back && raw_kept,
        json!({ "read": read, "first": first, "again": again, "replayed": replayed, "unrelated": unrelated, "app": [app_first, app_back, raw_kept], "row": row(api, &paul, &app), "events": super::site::event_kinds(api, &paul, &agent) }),
    );

    // its rename says its name in its job in the same request, before its
    // face: a job commit refused changes neither; one whose answer is lost
    // is looked for at main, and goes on; asked again after a lost answer
    // (its `was` no longer its title, and its title the one asked) it is
    // done, its role left as it is
    let repo = api.status(&paul, &agent)?.body["repo"].as_str().unwrap_or("").to_string();
    let rename = |title: &str, was: &str| api.signed(&paul, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": title, "was": was })));
    let job_at = |s: &Suite| s.fake.file_at(&repo, "main", "SOUL.md").map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let wrote = api.signed(&paul, "POST", &format!("/api/f/{agent}/files"), Some(&json!({ "message": "its job", "files": [{ "path": "SOUL.md", "text": "Frank helps with research.\n" }] })))?.status == 200;
    s.fake.fail_commit_packs(true);
    let refused = rename("Gus", "Frank");
    s.fake.fail_commit_packs(false);
    let refused = refused?;
    let after_refused = (row(api, &paul, &agent)["title"].clone(), job_at(s));
    let retried = rename("Gus", "Frank")?;
    let after_retry = (row(api, &paul, &agent)["title"].clone(), job_at(s));
    s.ok(
        "a rename whose job commit is refused fails (5xx), its face and its job as they were; asked again, both say the new name",
        wrote && refused.status >= 500 && after_refused == (json!("Frank"), "Frank helps with research.\n".to_string()) && retried.status == 200 && after_retry == (json!("Gus"), "Gus helps with research.\n".to_string()),
        json!({ "refused": refused.to_string(), "after": after_refused, "retried": retried.to_string(), "then": after_retry }),
    );
    s.fake.drop_commit_answers(&repo, 1);
    let lost = rename("Hal", "Gus")?;
    let served = api.signed(&paul, "GET", &format!("/api/f/{agent}/file?path=SOUL.md"), None)?.text;
    let tip = s.fake.branch(&repo, "main");
    let replay = api.signed(&paul, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": "Hal", "role": "scribe", "was": "Gus" })))?;
    let stale = rename("Ivy", "Gus")?;
    let after = (row(api, &paul, &agent)["title"].clone(), job_at(s), s.fake.branch(&repo, "main"));
    s.ok(
        "a rename whose job commit landed and lost its answer is found at main and done: its name and job say it, its job served so; asked again with that `was` (and a role), done with nothing committed and its role as it was; another name with that `was` is 409",
        lost.status == 200 && served == "Hal helps with research.\n" && replay.status == 200 && stale.status == 409
            && after == (json!("Hal"), "Hal helps with research.\n".to_string(), tip.clone()) && tip.is_some() && row(api, &paul, &agent).get("agentRole").is_none(),
        json!({ "lost": lost.to_string(), "served": served, "replay": replay.to_string(), "stale": stale.to_string(), "after": after }),
    );
    let back = api.signed(&paul, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": "Frank" })))?;
    anyhow::ensure!(back.status == 200 && job_at(s) == "Frank helps with research.\n", "renamed Frank again: {back}");
    // its commit landed, its answer lost, and its job unread after: the
    // rename fails, its job renamed and its face not; the next rename, to
    // another name, finds the name that commit wrote and renames it whole
    s.fake.drop_commit_answers(&repo, 1);
    s.fake.fail_read_after_lost_answer("SOUL.md");
    let lost = rename("Frank Prime", "Frank");
    s.fake.fail_file_read(None);
    let lost = lost?;
    let between = (row(api, &paul, &agent)["title"].clone(), job_at(s));
    let other = rename("Rover", "Frank")?;
    s.ok(
        "a rename whose commit landed while its answer and the read after it were lost fails, its job renamed and its name not; the next rename, to another name, says it in its job whole (never \"Rover Prime\")",
        lost.status >= 500 && between == (json!("Frank"), "Frank Prime helps with research.\n".to_string()) && other.status == 200 && job_at(s) == "Rover helps with research.\n" && row(api, &paul, &agent)["title"] == "Rover",
        json!({ "lost": lost.to_string(), "between": between, "other": other.to_string(), "job": job_at(s) }),
    );
    let back = api.signed(&paul, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": "Frank" })))?;
    anyhow::ensure!(back.status == 200 && job_at(s) == "Frank helps with research.\n", "renamed Frank once more: {back}");

    // the CLI, as its person uses it (cli/GUIDE.md): `fragment agent
    // --name` renames it and says its new name; every reader follows, its
    // status's title too (what its computer's image reads it by), and
    // `fragment access` names it by its name, never its label
    let home = super::shell::cli_home(s, api, &session, "faces-cli")?;
    // its job named it (one an owner or the agent wrote): the rename says
    // the new name there too, in its own commit (Paul, 2026-10-27)
    let job = |text: &str| api.signed(&paul, "POST", &format!("/api/f/{agent}/files"), Some(&json!({ "message": "its job", "files": [{ "path": "SOUL.md", "text": text }] })));
    let job_now = || api.signed(&paul, "GET", &format!("/api/f/{agent}/file?path=SOUL.md"), None).map(|r| r.text).unwrap_or_default();
    anyhow::ensure!(job("Frank keeps the garden; Frankly, Frank_ and frank are other words.\n")?.status == 200, "its job written");
    let said = s.cli_json(api, &home, &["agent", &agent, "--name", "Frances", "--json"]);
    let rewritten = job_now();
    s.ok(
        "`fragment agent --name` says the new name where its job said the old one as a word, and nowhere else",
        rewritten == "Frances keeps the garden; Frankly, Frank_ and frank are other words.\n",
        &rewritten,
    );
    let followed = s.eventually(WAIT, || row(api, &paul, &agent)["title"] == "Frances");
    let status = api.status(&paul, &agent)?.body["title"].clone();
    let asked = people(api, &chat, &cookie, &agent_id);
    s.ok(
        "`fragment agent --name` renames it, saying its new name; its list row, its status (what its image reads) and a chat's page follow",
        said.as_ref().is_ok_and(|v| v["name"] == "Frances" && v["changed"] == true) && followed && status == "Frances" && asked["title"] == "Frances" && asked["name"] == label.as_str(),
        json!({ "cli": said.map_err(|e| e.to_string()), "status": status, "people": asked }),
    );
    // Bob a viewer of the chat it is in again (its own fragment is Paul's alone)
    let r = api.share(&chat, &bob_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "Bob, a viewer again: {r}");
    let out = s.cli(api, &home, &["access"]);
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let r = api.signed(&paul, "DELETE", &format!("/api/f/{chat}/members/{bob_id}"), None)?;
    anyhow::ensure!(r.status == 200, "Bob, out again: {r}");
    let line = printed.lines().find(|l| l.contains("Frances") || l.split('\t').any(|w| w.trim() == label));
    s.ok("`fragment access` names it by its name, never its label", out.status.success() && line.is_some_and(|l| l.contains("Frances")), &printed);
    // renamed before a rename said so in its job (Skyler's XP, once Bliss):
    // the shell's first job of then ("You are <name>, <owner>'s default
    // agent: …") still names it. Its owner's rename again, to the name it
    // has, says it there; asked once more, nothing changes (replay)
    let first = "You are Bliss, paul@example.com's default agent: the first one they talk to.\nBliss keeps the garden.\n";
    anyhow::ensure!(job(first)?.status == 200, "its old job written");
    let again = s.cli_json(api, &home, &["agent", &agent, "--name", "Frances", "--json"]);
    let recovered = job_now();
    let tip = api.status(&paul, &agent)?.body["pins"]["main"].clone();
    let replay = s.cli_json(api, &home, &["agent", &agent, "--name", "Frances", "--json"]);
    let replayed = (job_now(), api.status(&paul, &agent)?.body["pins"]["main"].clone());
    s.ok(
        "an agent renamed before: its owner's rename again says its name where the shell's first job named it, and once more changes nothing",
        again.is_ok() && recovered == "You are Frances, paul@example.com's default agent: the first one they talk to.\nFrances keeps the garden.\n" && replay.is_ok() && replayed == (recovered.clone(), tip.clone()),
        json!({ "recovered": recovered, "replayed": replayed.0, "tips": [tip, replayed.1] }),
    );
    // `fragment agent --picture` writes its avatar.png, and `--no-picture`
    // deletes it, each shown on its list with no deploy
    let file = s.dir("faces-cli-picture").join("me.png");
    std::fs::write(&file, &orange)?;
    let live_was = api.status(&paul, &agent)?.body["pins"]["live"].clone();
    let set = s.cli_json(api, &home, &["agent", &agent, "--picture", &file.display().to_string(), "--json"]);
    let set_shows = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == shown.as_str());
    let unset = s.cli_json(api, &home, &["agent", &agent, "--no-picture", "--json"]);
    let unset_shows = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
    s.ok(
        "`fragment agent --picture` gives it a picture, and `--no-picture` takes it away, each shown on its list, nothing deployed",
        set.is_ok_and(|v| v["changed"] == true) && set_shows && unset.is_ok_and(|v| v["changed"] == true) && unset_shows && api.status(&paul, &agent)?.body["pins"]["live"] == live_was,
        json!({ "row": row(api, &paul, &agent) }),
    );
    // its job as its computer's sync leaves it: on main, never deployed
    // (images/hermes/boot/src/sync.rs). Its page reads it there (`__file`,
    // main first for an agent), with no deploy; an app's file stays live's
    // until it deploys (Paul, 2026-10-27: his agent's page kept its first job)
    let write = |name: &str, path: &str, text: &str| api.signed(&paul, "POST", &format!("/api/f/{name}/files"), Some(&json!({ "message": "a change", "files": [{ "path": path, "text": text }] })));
    let deploy = |name: &str| api.signed(&paul, "POST", &format!("/api/f/{name}/deploy"), Some(&json!({})));
    let read = |name: &str, path: &str| -> Result<String> {
        let cookie = site_cookie(api, &session, name)?;
        Ok(api.page(name, &format!("__file?path={path}"), Some(&format!("fragment_site={cookie}")))?.text)
    };
    let (first, second) = ("You keep the garden.\n", "You keep the garden and the bees.\n");
    let staged = write(&agent, "SOUL.md", first)?.status == 200 && deploy(&agent)?.status == 200 && write(&agent, "SOUL.md", second)?.status == 200;
    let its_job = read(&agent, "SOUL.md")?;
    let (one, two) = ("the app's notes, deployed\n", "the app's notes, not yet\n");
    let app_staged = write(&app, "notes.md", one)?.status == 200 && deploy(&app)?.status == 200 && write(&app, "notes.md", two)?.status == 200;
    let apps = read(&app, "notes.md")?;
    s.ok(
        "an agent's page reads its job as its computer leaves it, on main, with no deploy; an app's page reads its file from live until it deploys",
        staged && its_job == second && app_staged && apps == one,
        json!({ "job": its_job, "app": apps }),
    );
    let bare = s.named(api, &paul, "faces-bare")?;
    let r = api.create_with(&paul, json!({ "name": bare, "template": "agent" }))?;
    anyhow::ensure!(r.status == 200, "an agent with no title: {r}");
    s.owned(&r.body, &paul);
    let made = s.eventually(WAIT, || row(api, &paul, &bare)["kind"] == "agent");
    let shown = row(api, &paul, &bare);
    let status = api.status(&paul, &bare)?.body["title"].clone();
    let said = s.cli_json(api, &home, &["agent", &bare, "--json"]);
    let bare_label = fragment_proto::split_fragment_name(&bare).map_or(bare.as_str(), |(l, _)| l).to_string();
    s.ok(
        "an agent made with no title is untitled (its label shows, and `fragment agent` says it), never called by the template's word",
        made && shown.get("title").is_none_or(Value::is_null) && status.is_null() && said.is_ok_and(|v| v["name"] == bare_label.as_str()),
        json!({ "row": shown, "status": status }),
    );
    // an app whose main holds an avatar.png, become an agent: its install
    // shows that picture; an app again, none; an agent again, it again
    let became = s.named(api, &paul, "faces-became")?;
    let r = api.create_with(&paul, json!({ "name": became, "template": "blank", "visibility": "members" }))?;
    anyhow::ensure!(r.status == 200, "an app to become an agent: {r}");
    s.owned(&r.body, &paul);
    let kind_is = |kind: &str| -> Result<bool> {
        let manifest = if kind == "agent" { json!({ "kind": "agent", "meta": { "title": "Became" } }) } else { json!({ "meta": { "title": "Became" } }) };
        let wrote = write(&became, "fragment.json", &manifest.to_string())?.status == 200;
        Ok(wrote && deploy(&became)?.status == 200 && s.eventually(WAIT, || row(api, &paul, &became)["kind"] == kind))
    };
    let pictured = api.signed(&paul, "POST", &format!("/api/f/{became}/files"), Some(&json!({ "message": "a picture", "files": [{ "path": "avatar.png", "base64": base64::engine::general_purpose::STANDARD.encode(&orange) }] })))?.status == 200
        && deploy(&became)?.status == 200;
    let as_app = row(api, &paul, &became).get("picture").is_none();
    let orange_v = hex::encode(Sha256::digest(&orange))[..12].to_string();
    let shows = || row(api, &paul, &became)["picture"].as_str().is_some_and(|p| p.ends_with(&format!("?v={orange_v}")));
    let as_agent = kind_is("agent")? && shows();
    let app_again = kind_is("app")? && row(api, &paul, &became).get("picture").is_none();
    let agent_again = kind_is("agent")? && shows();
    s.ok(
        "an app whose main holds an avatar.png, become an agent, shows it at that install; an app again shows none; an agent again shows it again",
        pictured && as_app && as_agent && app_again && agent_again,
        json!({ "steps": [pictured, as_app, as_agent, app_again, agent_again], "row": row(api, &paul, &became) }),
    );
    // its page's Open Graph title is its name, its fragment.json naming none
    let page_wrote = write(&app, "site/index.html", "<!doctype html><html><head></head><body>notes</body></html>")?.status == 200 && deploy(&app)?.status == 200;
    let renamed = api.signed(&paul, "PUT", &format!("/api/f/{app}/face"), Some(&json!({ "title": "Garden notes" })))?;
    let cookie = site_cookie(api, &session, &app)?;
    let page = api.page(&app, "", Some(&format!("fragment_site={cookie}")))?;
    s.ok(
        "an app renamed through its face: its page's Open Graph title and its tab's title are its new name",
        page_wrote && renamed.status == 200 && page.status == 200 && page.text.contains(r#"og:title" content="Garden notes""#) && page.text.contains("<title>Garden notes</title>"),
        &page,
    );
    // its role: the tag beside its name, on its owner's list as its name is
    // (Paul, 2026-10-30: one source, live everywhere), set through its face;
    // one of spaces takes it away, one too long is refused and the last
    // kept, and a role is an agent's alone
    let role = |name: &str, role: &str| api.signed(&paul, "PUT", &format!("/api/f/{name}/face"), Some(&json!({ "role": role })));
    let given = role(&agent, " project manager ")?.status == 200
        && s.eventually(WAIT, || row(api, &paul, &agent)["agentRole"] == "project manager")
        && api.status(&paul, &agent)?.body["agentRole"] == "project manager";
    let too_long = role(&agent, &"r".repeat(fragment_proto::limits::ROLE_MAX_CHARS + 1))?;
    let kept = row(api, &paul, &agent);
    let blank = role(&agent, "   ")?.status == 200 && s.eventually(WAIT, || row(api, &paul, &agent).get("agentRole").is_none());
    let chats = role(&chat, "a chat")?;
    s.ok(
        "its role, set through its face, shows on its owner's list beside its name, and on its status, trimmed; one past 48 characters is refused (400), the last kept; one of spaces takes it away; a chat's is refused (400)",
        given && too_long.status == 400 && too_long.text.contains("48") && kept["agentRole"] == "project manager" && blank && chats.status == 400 && row(api, &paul, &chat).get("agentRole").is_none(),
        json!({ "long": too_long.to_string(), "kept": kept, "row": row(api, &paul, &agent), "chat": chats.to_string() }),
    );
    Ok(())
}
