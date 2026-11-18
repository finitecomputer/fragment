//! A fragment's name and an agent's picture (docs/api.md, "A fragment's
//! name and an agent's picture"): its face, kept by its cell, with no
//! commit and no deploy: its title and an agent's role set through `PUT
//! /api/f/{name}/face`, its picture its icon (`PUT /api/f/{name}/icon`),
//! the routes Settings and `fragment agent` use. It reaches its owner's
//! list (what the shell draws), a chat's `__people` (what its page draws),
//! the share sheet, the access view and its picture's own address. The same
//! change again changes nothing (replay); a title blank or too long, a role
//! too long or a chat's, a file that is no picture, and someone else's
//! agent are refused, the face kept; a `fragment.json` names no face once
//! it has one, and refuses no install; taken away, the picture is gone
//! everywhere. A picture kept by the release before (its `avatar.png` at
//! the commit an install checked) is taken once, by its first fetch or the
//! alarm's pass, and one git no longer holds is let go (valid, invalid,
//! replay); a node's restart keeps it all (restart.rs).

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
/// its title through its face (when one), its picture as its icon (`Some`
/// bytes, or `None` to take it away; untouched when not asked). Each
/// answer, when asked.
pub(super) fn set_face(api: &Api, keys: &Keys, agent: &str, title: Option<&str>, picture: Option<Option<&[u8]>>) -> Result<(Option<Reply>, Option<Reply>)> {
    let named = title.map(|t| api.signed(keys, "PUT", &format!("/api/f/{agent}/face"), Some(&json!({ "title": t })))).transpose()?;
    let url = format!("{}/api/f/{agent}/icon", api.base);
    let pictured = match picture {
        Some(Some(bytes)) => Some(api.call(Call { method: "PUT", url, body: Some(bytes.to_vec()), content_type: Some("application/octet-stream"), keys: Some(keys), ..Call::default() })?),
        Some(None) => Some(api.signed(keys, "DELETE", &format!("/api/f/{agent}/icon"), None)?),
        None => None,
    };
    Ok((named, pictured))
}

/// An agent's picture as the release before kept it (`fragment_core::face::
/// AGENT_PICTURE` at the commit an install checked, and the row naming
/// it): `bytes` committed there and deployed, then the row written by the
/// test lever, no icon. The commit.
pub(super) fn legacy_picture(api: &Api, keys: &Keys, agent: &str, bytes: &[u8]) -> Result<String> {
    legacy_picture_beside(api, keys, agent, bytes, false)
}

/// [`legacy_picture`], keeping the icon it has (`icon`): one a release
/// before left hidden beside its picture, from before it was an agent.
fn legacy_picture_beside(api: &Api, keys: &Keys, agent: &str, bytes: &[u8], icon: bool) -> Result<String> {
    let files = json!({ "message": "its picture, as a release before kept it", "files": [{ "path": "avatar.png", "base64": base64::engine::general_purpose::STANDARD.encode(bytes) }] });
    let wrote = api.signed(keys, "POST", &format!("/api/f/{agent}/files"), Some(&files))?;
    let deployed = api.signed(keys, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})))?;
    let live = deployed.body["live"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(wrote.status == 200 && deployed.status == 200 && !live.is_empty(), "its avatar.png committed and deployed: {wrote} {deployed}");
    let row = format!("{} image/png {live}", hex::encode(Sha256::digest(bytes)));
    let r = super::ledger::lever(api, agent, "legacy-picture", json!({ "row": row, "icon": icon }))?;
    anyhow::ensure!(r.status == 200, "its row, as a release before kept it: {r}");
    Ok(live)
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

/// A JPEG's width and height, from its first frame's header (SOF0 to
/// SOF2), or none.
fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    let mut at = 2;
    // bounded by the bytes: each segment moves past its own length
    while at + 9 < bytes.len() && bytes[at] == 0xFF {
        let (marker, len) = (bytes[at + 1], u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize);
        if (0xC0..=0xC2).contains(&marker) {
            return Some((u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]), u16::from_be_bytes([bytes[at + 5], bytes[at + 6]])));
        }
        at += 2 + len;
    }
    None
}

/// A JPEG of more than `bytes`, its picture the icon lane's 40×24
/// photograph: comment segments after its start.
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
    let pins = |name: &str| api.status(&paul, name).map(|r| r.body["pins"].clone()).unwrap_or(Value::Null);

    // made: its title, no picture
    let made = row(api, &paul, &agent);
    let asked = people(api, &chat, &cookie, &agent_id);
    let none = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
    s.ok(
        "an agent fresh from the agent template shows by the title it was made with, with no picture (404 at its address)",
        made["title"] == "Juniper" && made.get("picture").is_none() && asked["title"] == "Juniper" && asked["name"] == label.as_str() && asked["picture"].is_null() && none.status == 404,
        json!({ "row": made, "people": asked, "picture": none.status }),
    );

    // renamed, and given a picture, as Settings and `fragment agent` do:
    // its face, with no commit and no deploy
    let before = pins(&agent);
    let (named, pictured) = set_face(api, &paul, &agent, Some("Frank"), Some(Some(&orange)))?;
    let (named, pictured) = (named.context("named")?, pictured.context("pictured")?);
    let shown = path_of(&orange);
    let listed = s.eventually(WAIT, || {
        let r = row(api, &paul, &agent);
        r["title"] == "Frank" && r["picture"] == shown.as_str()
    });
    s.ok(
        "renamed and given a picture through its face and its icon, at once: its owner's list (what the shell draws) names it and shows its picture's address, and nothing was committed or deployed",
        named.status == 200 && named.body["title"] == "Frank" && pictured.status == 200 && pictured.body["icon"] == shown.as_str() && listed && pins(&agent) == before,
        json!({ "face": named.body, "icon": pictured.body, "row": row(api, &paul, &agent), "pins": [before, pins(&agent)] }),
    );
    let asked = people(api, &chat, &cookie, &agent_id);
    s.ok(
        "a chat's page is told so too (`__people`): its title and its picture, whole; its @name stays its label",
        asked["title"] == "Frank" && asked["picture"].as_str().is_some_and(|p| p.starts_with("http") && p.ends_with(&shown)) && asked["name"] == label.as_str(),
        &asked,
    );
    let got = fetched(api, &shown)?;
    s.ok(
        "its picture is served at its identity's address to anyone, the bytes it was given (a picture is kept as it is), as an image",
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
    // Bob, a viewer of it for a moment
    let (bob, _) = person(api)?;
    let bob_id = api.identity(&bob)?;
    let r = api.share(&agent, &bob_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "Bob, a viewer: {r}");
    let access = api.signed(&paul, "GET", "/api/access", None)?;
    let member = access.body["fragments"].as_array().into_iter().flatten().find(|f| f["name"] == agent.as_str()).and_then(|f| f["members"].as_array()?.iter().find(|m| m["principal"] == agent_id.as_str()).cloned());
    s.ok("who has access to what names it so too (its title, beside its label)", member.as_ref().is_some_and(|m| m["title"] == "Frank" && m["name"] == label.as_str()), &access);
    // a viewer is no editor: Bob renames nothing
    let (theirs, _) = set_face(api, &bob, &agent, Some("Bobbed"), None)?;
    let r = api.signed(&paul, "DELETE", &format!("/api/f/{agent}/members/{bob_id}"), None)?;
    anyhow::ensure!(r.status == 200, "Bob, out again: {r}");

    // its history: each change an `events` row; the same change again
    // changes nothing, and writes none
    let changes = events(api, &paul, &agent, "face.changed");
    let (again, same) = set_face(api, &paul, &agent, Some(" Frank "), Some(Some(&orange)))?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "each change is a `face.changed` event, its history; the same change again (padded, the same picture) answers the same face, writes no event, and its name and picture's address stay",
        changes == 1 && again.is_some_and(|r| r.status == 200 && r.body["title"] == "Frank") && same.is_some_and(|r| r.status == 200) && events(api, &paul, &agent, "face.changed") == 1 && kept["title"] == "Frank" && kept["picture"] == shown.as_str(),
        json!({ "events": super::site::event_kinds(api, &paul, &agent), "row": kept }),
    );

    // invalid: refused, nothing changed
    let face = |body: Value, keys: &Keys| api.signed(keys, "PUT", &format!("/api/f/{agent}/face"), Some(&body));
    let long = face(json!({ "title": "F".repeat(fragment_proto::limits::TITLE_MAX_CHARS + 1) }), &paul)?;
    let blank = face(json!({ "title": "   " }), &paul)?;
    let nothing = face(json!({}), &paul)?;
    let pictured = face(json!({ "picture": shown }), &paul)?;
    let stale = face(json!({ "title": "Fred", "was": "Juniper" }), &paul)?;
    let (_, text) = set_face(api, &paul, &agent, None, Some(Some(b"hello, I am no picture")))?;
    let text = text.context("its picture")?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "a title past 120 characters, one of only spaces, a change naming nothing or a picture, are refused (400) saying why; a file that is no picture is refused (400); a rename whose `was` is no longer its title is refused (409); its name and picture stay",
        [&long, &blank, &nothing, &pictured, &text].iter().all(|r| r.status == 400)
            && stale.status == 409
            && long.text.contains("1 to 120 characters")
            && pictured.text.contains("icon")
            && kept["title"] == "Frank"
            && kept["picture"] == shown.as_str()
            && events(api, &paul, &agent, "face.changed") == 1,
        json!({ "long": long.to_string(), "blank": blank.to_string(), "nothing": nothing.to_string(), "picture": pictured.to_string(), "text": text.to_string(), "stale": stale.to_string(), "row": kept }),
    );
    // someone else's agent: Bob, a viewer then, and no member now, changes nothing
    let (bobs, bobs_picture) = set_face(api, &bob, &agent, Some("Bobbed"), Some(Some(&blue)))?;
    let kept = row(api, &paul, &agent);
    s.ok(
        "someone else's agent: a viewer renames it (403), and someone not in it renames it and sets its picture (403 or 404), its name and picture unchanged",
        theirs.is_some_and(|r| r.status == 403) && bobs.is_some_and(|r| matches!(r.status, 403 | 404)) && bobs_picture.is_some_and(|r| matches!(r.status, 403 | 404)) && kept["title"] == "Frank" && kept["picture"] == shown.as_str(),
        json!({ "row": kept }),
    );
    let nobody = fetched(api, "/api/identities/npub1nobody/picture")?;
    s.ok("an address that names no identity serves no picture (404)", nobody.status == 404, &nobody);

    // a `fragment.json` names no face once it has one, and refuses no
    // install: a title or a role there (even one past a face's limits),
    // and an `avatar.png`, committed and deployed, change nothing, and say
    // nothing in `code.error`
    let manifest = json!({ "template": "agent", "meta": { "title": "From git", "role": "r".repeat(fragment_proto::limits::ROLE_MAX_CHARS + 1) } });
    let files = json!({ "message": "a face in git", "files": [{ "path": "fragment.json", "text": manifest.to_string() }, { "path": "avatar.png", "base64": base64::engine::general_purpose::STANDARD.encode(&blue) }] });
    let wrote = api.signed(&paul, "POST", &format!("/api/f/{agent}/files"), Some(&files))?;
    let deployed = api.signed(&paul, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})))?;
    let status = api.status(&paul, &agent)?.body;
    let kept = row(api, &paul, &agent);
    s.ok(
        "a fragment.json's title and role, and an avatar.png, committed and deployed, name no face: its name and picture stay, and its install refuses nothing (no code.error)",
        wrote.status == 200 && deployed.status == 200 && status["code"]["error"].is_null() && status["title"] == "Frank" && kept["title"] == "Frank" && kept["picture"] == shown.as_str() && kept.get("agentRole").is_none(),
        json!({ "status": status, "row": kept }),
    );

    // a picture past what one is kept as (256 KiB) is cropped by Images to
    // fill the icon's square (it is shown round), a JPEG, at its owner's charge
    if s.makes_icons() {
        let heavy = heavy_jpeg(fragment_proto::limits::PICTURE_MAX_BYTES);
        let (_, put) = set_face(api, &paul, &agent, None, Some(Some(&heavy)))?;
        let put = put.context("its heavy picture")?;
        let at = put.body["icon"].as_str().unwrap_or("").to_string();
        let got = fetched(api, &at)?;
        let moved = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == at.as_str());
        s.ok(
            "a picture past 256 KiB is cropped to fill a 256×256 square, a JPEG within 256 KiB, served at its identity's new address",
            heavy.len() > fragment_proto::limits::PICTURE_MAX_BYTES
                && put.status == 200
                && at.starts_with(&format!("/api/identities/{agent_id}/picture?v="))
                && got.status == 200
                && got.header("content-type") == "image/jpeg"
                && got.bytes.len() <= fragment_proto::limits::PICTURE_MAX_BYTES
                && jpeg_size(&got.bytes) == Some((256, 256))
                && moved,
            json!({ "put": put.to_string(), "type": got.header("content-type"), "bytes": got.bytes.len(), "size": jpeg_size(&got.bytes) }),
        );
    } else {
        s.skip_icon("a picture past 256 KiB is cropped to fill a 256×256 square");
    }

    // a new picture is a new address; taken away, it is gone everywhere
    let (_, fixed) = set_face(api, &paul, &agent, None, Some(Some(&blue)))?;
    let renewed = path_of(&blue);
    let moved = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == renewed.as_str());
    let got = fetched(api, &renewed)?;
    s.ok(
        "a new picture is a new address, serving it",
        fixed.is_some_and(|d| d.status == 200) && moved && renewed != shown && got.status == 200 && got.bytes == blue,
        json!({ "row": row(api, &paul, &agent), "status": got.status }),
    );
    let (_, gone) = set_face(api, &paul, &agent, None, Some(None))?;
    let removed = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
    let asked = people(api, &chat, &cookie, &agent_id);
    let none = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
    s.ok(
        "its picture taken away (its icon removed), no list, page or address shows one, and its name stays",
        gone.is_some_and(|r| r.status == 200) && removed && asked["picture"].is_null() && asked["title"] == "Frank" && none.status == 404,
        json!({ "row": row(api, &paul, &agent), "people": asked, "picture": none.status }),
    );

    // the release before's picture, taken once: an agent whose row names
    // its avatar.png at the commit an install checked shows it at once (the
    // same address), and its first fetch takes it into its icon, writing
    // `picture.taken`; fetched again, it is served the same and taken no
    // more (replay)
    let taken_before = events(api, &paul, &agent, "picture.taken");
    legacy_picture(api, &paul, &agent, &orange)?;
    let shows = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == shown.as_str());
    let first = fetched(api, &shown)?;
    let second = fetched(api, &shown)?;
    let icon = api.signed(&paul, "GET", &format!("/api/f/{agent}/icon"), None)?;
    s.ok(
        "a picture the release before kept (its avatar.png at a checked commit) shows at once, is served by its first fetch, the same bytes, and is its icon from then on; fetched again, it is taken no more",
        shows && first.status == 200 && first.bytes == orange && first.header("content-type") == "image/png" && second.bytes == orange && icon.bytes == orange && events(api, &paul, &agent, "picture.taken") == taken_before + 1,
        json!({ "row": row(api, &paul, &agent), "first": first.status, "icon": icon.status, "events": super::site::event_kinds(api, &paul, &agent) }),
    );
    // taken by the alarm's pass, unasked: within a day of the release,
    // every row is taken, fetched or not
    let live = legacy_picture(api, &paul, &agent, &blue)?;
    let r = super::ledger::lever(api, &agent, "poll-now", json!({}))?;
    let passed = r.status == 200 && s.eventually(WAIT, || events(api, &paul, &agent, "picture.taken") == taken_before + 2);
    let icon = api.signed(&paul, "GET", &format!("/api/f/{agent}/icon"), None)?;
    s.ok(
        "one never fetched is taken by the alarm's pass, the bytes its install checked",
        passed && icon.bytes == blue && row(api, &paul, &agent)["picture"] == renewed.as_str(),
        json!({ "events": super::site::event_kinds(api, &paul, &agent), "row": row(api, &paul, &agent) }),
    );
    // invalid: a row whose commit holds other bytes than it names is let
    // go, said, and it shows no picture
    let r = super::ledger::lever(api, &agent, "legacy-picture", json!({ "row": format!("{} image/png {live}", "a".repeat(64)) }))?;
    let lost = fetched(api, &format!("/api/identities/{agent_id}/picture"))?;
    let dropped = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
    s.ok(
        "a row whose commit no longer holds the bytes it names is let go (`picture.lost`): no picture is served or shown",
        r.status == 200 && lost.status == 404 && dropped && events(api, &paul, &agent, "picture.lost") == 1,
        json!({ "lost": lost.to_string(), "row": row(api, &paul, &agent) }),
    );
    // a row beside an icon it kept from before it was an agent: its
    // picture is the row's, shown and served, and taken over that icon
    let (_, hidden) = set_face(api, &paul, &agent, None, Some(Some(&blue)))?;
    legacy_picture_beside(api, &paul, &agent, &orange, true)?;
    let shows = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == shown.as_str());
    let served = fetched(api, &shown)?;
    let icon = api.signed(&paul, "GET", &format!("/api/f/{agent}/icon"), None)?;
    s.ok(
        "a row beside an icon kept from before it was an agent: the row's picture shows and is served, and is its icon once taken, never that old icon",
        hidden.is_some_and(|r| r.status == 200) && shows && served.bytes == orange && icon.bytes == orange && row(api, &paul, &agent)["picture"] == shown.as_str(),
        json!({ "row": row(api, &paul, &agent), "served": served.status }),
    );

    // the CLI, as its person uses it (cli/GUIDE.md): `fragment agent
    // --name` renames it and says its new name; every reader follows, its
    // status's title too (what its computer's image reads it by), and
    // `fragment access` names it by its name, never its label
    let home = super::shell::cli_home(s, api, &session, "faces-cli")?;
    // its job named it (one an owner or the agent wrote): the rename says
    // the new name there too, in a commit of its own (Paul, 2026-10-27)
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
    let r = api.share(&agent, &bob_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "Bob, a viewer again: {r}");
    let out = s.cli(api, &home, &["access"]);
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let r = api.signed(&paul, "DELETE", &format!("/api/f/{agent}/members/{bob_id}"), None)?;
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
    // renamed back and forth (A, B, A, B): each rename says its name in its
    // job; none is taken for an earlier rename to the same name
    let mut named = vec![];
    for name in ["Frank", "Frances", "Frank", "Frances"] {
        let said = s.cli_json(api, &home, &["agent", &agent, "--name", name, "--json"]);
        named.push(json!({ "name": name, "cli": said.is_ok(), "job": job_now().lines().next().unwrap_or("").to_string() }));
    }
    s.ok(
        "renamed back and forth with `fragment agent --name` (A, B, A, B), its job says each new name in turn",
        named.iter().all(|n| n["cli"] == true && n["job"].as_str().is_some_and(|j| j.starts_with(&format!("You are {},", n["name"].as_str().unwrap_or(""))))),
        json!(named),
    );
    // `fragment agent --picture` sets its icon, and `--no-picture` takes it away
    let file = s.dir("faces-cli-picture").join("me.png");
    std::fs::write(&file, &orange)?;
    let set = s.cli_json(api, &home, &["agent", &agent, "--picture", &file.display().to_string(), "--json"]);
    let set_shows = s.eventually(WAIT, || row(api, &paul, &agent)["picture"] == shown.as_str());
    let unset = s.cli_json(api, &home, &["agent", &agent, "--no-picture", "--json"]);
    let unset_shows = s.eventually(WAIT, || row(api, &paul, &agent).get("picture").is_none());
    s.ok(
        "`fragment agent --picture` gives it a picture, and `--no-picture` takes it away, each shown on its list",
        set.is_ok_and(|v| v["changed"] == true) && set_shows && unset.is_ok_and(|v| v["changed"] == true) && unset_shows,
        json!({ "row": row(api, &paul, &agent) }),
    );
    // its job as its computer's sync leaves it: on main, never deployed
    // (images/hermes/boot/src/sync.rs). Its page reads it there (`__file`,
    // main first for an agent), with no deploy; an app's file stays live's
    // until it deploys (Paul, 2026-10-27: his agent's page kept its first job)
    let app = s.named(api, &paul, "faces-app")?;
    s.create(api, &paul, &app)?;
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
    // an app has no identity's picture, even run by a computer as an agent
    // would be: its icon stays its viewers' alone
    if s.makes_icons() {
        let (_, iconed) = set_face(api, &paul, &app, None, Some(Some(&orange)))?;
        let computer = api.signed(&paul, "POST", "/api/computers", Some(&json!({})))?.body["computer"].as_str().unwrap_or("").to_string();
        let r = api.signed(&paul, "PUT", &format!("/api/computers/{computer}/agents/{app}"), Some(&json!({})))?;
        let id = r.body["agents"].as_array().into_iter().flatten().find(|a| a["fragment"] == app.as_str()).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string();
        let public = fetched(api, &format!("/api/identities/{id}/picture"))?;
        let own = api.signed(&paul, "GET", &format!("/api/f/{app}/icon"), None)?;
        s.ok(
            "an app a computer runs has no public picture (404 at its identity's address), its icon still its viewers'",
            iconed.is_some_and(|r| r.status == 200) && !id.is_empty() && public.status == 404 && own.status == 200,
            json!({ "assigned": r.status, "identity": id, "public": public.status, "own": own.status }),
        );
    } else {
        s.skip_icon("an app a computer runs has no public picture");
    }
    // its page's Open Graph title is its name, its fragment.json naming none
    let page_wrote = write(&app, "site/index.html", "<!doctype html><html><head></head><body>notes</body></html>")?.status == 200 && deploy(&app)?.status == 200;
    let renamed = api.signed(&paul, "PUT", &format!("/api/f/{app}/face"), Some(&json!({ "title": "Garden notes" })))?;
    let cookie = site_cookie(api, &session, &app)?;
    let page = api.page(&app, "", Some(&format!("fragment_site={cookie}")))?;
    s.ok(
        "an app whose fragment.json has no meta, renamed: its page's Open Graph title and its tab's title are its new name",
        page_wrote && renamed.status == 200 && page.status == 200 && page.text.contains(r#"og:title" content="Garden notes""#) && page.text.contains("<title>Garden notes</title>"),
        &page,
    );
    let bare = s.named(api, &paul, "faces-bare")?;
    let r = api.create_with(&paul, json!({ "name": bare, "template": "agent" }))?;
    anyhow::ensure!(r.status == 200, "an agent with no title: {r}");
    s.owned(&r.body, &paul);
    let made = s.eventually(WAIT, || row(api, &paul, &bare)["kind"] == "agent");
    let shown_bare = row(api, &paul, &bare);
    let status = api.status(&paul, &bare)?.body["title"].clone();
    let said = s.cli_json(api, &home, &["agent", &bare, "--json"]);
    s.ok(
        "an agent made with no title is untitled (its label shows, and `fragment agent` says it), never called by the template's word",
        made && shown_bare.get("title").is_none_or(Value::is_null) && status.is_null() && said.is_ok_and(|v| v["name"] == label_of(&bare)),
        json!({ "row": shown_bare, "status": status }),
    );
    // an agent with a picture the release before kept, beside an icon of
    // its own, deployed as an app (forked): its row goes at its pass, and
    // its icon, an app's, is the icon it had, never that avatar
    let (_, own_icon) = set_face(api, &paul, &bare, None, Some(Some(&blue)))?;
    legacy_picture_beside(api, &paul, &bare, &orange, true)?;
    let forked = write(&bare, "fragment.json", "{}\n")?.status == 200 && deploy(&bare)?.status == 200;
    let app_now = s.eventually(WAIT, || row(api, &paul, &bare)["kind"] == "app");
    let r = super::ledger::lever(api, &bare, "poll-now", json!({}))?;
    let passed = r.status == 200 && s.eventually(WAIT, || api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": bare, "op": "alarm" }))).is_ok_and(|r| r.body["pollAt"].as_i64() > Some(crate::api::now_ms())));
    let icon = api.signed(&paul, "GET", &format!("/api/f/{bare}/icon"), None)?;
    let blue_at = fragment_core::icon::icon_path(&bare, &hex::encode(Sha256::digest(&blue)));
    s.ok(
        "an agent with a picture the release before kept beside an icon of its own, forked into an app: its pass lets the row go, never taking that avatar, and its icon is the one it had",
        own_icon.is_some_and(|r| r.status == 200) && forked && app_now && passed && icon.bytes == blue && row(api, &paul, &bare)["picture"] == blue_at.as_str() && events(api, &paul, &bare, "picture.taken") == 0,
        json!({ "row": row(api, &paul, &bare), "icon": icon.status, "events": super::site::event_kinds(api, &paul, &bare) }),
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

/// A fragment's label (`juniper` of `juniper--k3x9`).
fn label_of(name: &str) -> &str {
    fragment_proto::split_fragment_name(name).map_or(name, |(label, _)| label)
}
