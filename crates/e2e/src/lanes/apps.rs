//! An app as a member of another fragment (decision 72), for now its
//! owner's alone: an owner adds an app of theirs to a fragment of theirs,
//! the app stays members only and invites no one while it is in, and
//! removing it, deleting it or deleting the fragment it is in lets it go.

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::{Need, Suite};

pub fn apps(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("apps", &[Need::Node]) {
        return Ok(());
    }
    let owner = api.person()?;
    let bob = api.person()?;
    let made = |s: &Suite, keys: &Keys, base: &str| -> Result<(String, String)> {
        let name = s.named(api, keys, base)?;
        let body = s.create(api, keys, &name)?;
        Ok((name, body["npub"].as_str().unwrap_or("").to_string()))
    };
    let (tracker, _) = made(s, &owner, "apps-tracker")?;
    let (board, board_npub) = made(s, &owner, "apps-board")?;
    let put = |keys: &Keys, target: &str, who: &str, role: &str| api.signed(keys, "PUT", &format!("/api/f/{target}/members/{who}"), Some(&json!({ "role": role })));
    let visibility = |keys: &Keys, name: &str, v: &str| api.signed(keys, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": v })));
    let members = |name: &str| -> Result<Vec<Value>> { Ok(api.signed(&owner, "GET", &format!("/api/f/{name}/members"), None)?.body["members"].as_array().cloned().unwrap_or_default()) };
    let app_row = |name: &str| -> Result<Option<Value>> { Ok(members(name)?.into_iter().find(|m| m["app"]["name"] == board.as_str())) };
    let label = |name: &str| name.rsplit_once("--").map_or(name, |(l, _)| l).to_string();

    // ---- refused: shared by link (a new app's default), someone else's, owning
    let linked = put(&owner, &tracker, &board, "viewer")?;
    s.ok(
        "an app shared by link is refused (403), saying to make it members only first, and is no member",
        linked.status == 403 && linked.message() == format!("{} is shared by link. Make it members only first.", label(&board)) && app_row(&tracker)?.is_none(),
        &linked,
    );
    let (bobs, _) = made(s, &bob, "apps-bobs")?;
    visibility(&bob, &bobs, "members")?;
    let theirs = put(&owner, &tracker, &bobs, "viewer")?;
    s.ok("someone else's app is refused (403): only its owner adds it", theirs.status == 403 && theirs.message().contains("only its owner adds it"), &theirs);
    visibility(&owner, &board, "members")?;
    let owning = put(&owner, &tracker, &board, "owner")?;
    s.ok("an app made an owner is refused (400): it views or edits", owning.status == 400 && owning.message().contains("never own it"), &owning);

    // ---- refused while anyone else may come in: an invite waiting
    let invited = api.signed(&owner, "POST", &format!("/api/f/{board}/invites"), Some(&json!({ "email": "apps-carol@e2e.test", "role": "viewer" })))?;
    let shared = put(&owner, &tracker, &board, "viewer")?;
    s.ok(
        "an app with an invite waiting is refused (403): for now an app joins only while it is its owner's alone",
        invited.status == 200 && shared.status == 403 && shared.message().contains("only while it's yours alone"),
        format!("{invited} {shared}"),
    );
    api.signed(&owner, "DELETE", &format!("/api/f/{board}/invites/apps-carol@e2e.test"), None)?;

    // ---- in, as a viewer: its row names the app and its owner, no kind
    let joined = put(&owner, &tracker, &board, "viewer")?;
    let again = put(&owner, &tracker, &board, "viewer")?;
    let owner_id = api.identity(&owner)?;
    let row = app_row(&tracker)?;
    s.ok(
        "the owner adds their own members-only app: its row names the app and its owner, leaves `kind` out, and is the app's own key; asked again, the same",
        joined.status == 200
            && joined.body["app"]["name"] == board.as_str()
            && joined.body["principal"] == board_npub.as_str()
            && joined.body["owner"] == owner_id.as_str()
            && joined.body.get("kind").is_none()
            && joined.body["role"] == "viewer"
            && again.status == 200
            && again.body == joined.body
            && row.as_ref() == Some(&joined.body)
            && members(&tracker)?.len() == 2,
        format!("{joined} {again} {row:?}"),
    );
    let listed = api.signed(&owner, "GET", "/api/fragments", None)?;
    let sharing = listed.body["fragments"].as_array().and_then(|f| f.iter().find(|f| f["name"] == tracker.as_str()).map(|f| f["sharing"].clone()));
    s.ok("its owner's row counts the app as a member, never a guest", sharing.as_ref().is_some_and(|s| s["members"] == 2 && s["guests"] == 0), format!("{sharing:?}"));

    // ---- while it is in: members only, its owner's alone
    let open = visibility(&owner, &board, "link")?;
    let invite = api.signed(&owner, "POST", &format!("/api/f/{board}/invites"), Some(&json!({ "email": "apps-carol@e2e.test", "role": "viewer" })))?;
    let in_line = |r: &Reply, tail: &str| r.status == 403 && r.message() == format!("{} is in {}, {tail}", label(&board), label(&tracker));
    s.ok(
        "while in, the app's link and its invites are refused (403), each naming where it is",
        in_line(&open, "so it stays members only.") && in_line(&invite, "so for now it's yours alone."),
        format!("{open} {invite}"),
    );
    let editor = put(&owner, &tracker, &board, "editor")?;
    s.ok("its role there changes, asked again", editor.status == 200 && editor.body["role"] == "editor" && app_row(&tracker)?.is_some_and(|m| m["role"] == "editor"), &editor);

    // ---- a restart of every object: still in, still held
    s.reload()?;
    let open = visibility(&owner, &board, "link")?;
    s.ok("after a restart the app is still in, and its link still refused", app_row(&tracker)?.is_some() && in_line(&open, "so it stays members only."), &open);

    // ---- removed: told in the removal's own turn, its link its own again
    let removed = api.signed(&owner, "DELETE", &format!("/api/f/{tracker}/members/{board}"), None)?;
    let open = visibility(&owner, &board, "link")?;
    s.ok(
        "removed by name, the app is out, and its link opens again",
        removed.status == 200 && app_row(&tracker)?.is_none() && open.status == 200,
        format!("{removed} {open}"),
    );
    let stale = put(&owner, &tracker, &board, "viewer")?;
    s.ok("(open again, it is refused again)", stale.status == 403, &stale);
    visibility(&owner, &board, "members")?;

    // ---- the app deleted: out of the fragment it was in
    let rejoined = put(&owner, &tracker, &board, "viewer")?;
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{board}"), None)?;
    s.ok(
        "an app deleted is out of the fragment it was in",
        rejoined.status == 200 && deleted.status == 200 && app_row(&tracker)?.is_none() && members(&tracker)?.len() == 1,
        format!("{rejoined} {deleted}"),
    );

    // ---- the fragment it is in deleted: the app is let go
    let (other, _) = made(s, &owner, "apps-other")?;
    visibility(&owner, &other, "members")?;
    let joined = put(&owner, &tracker, &other, "viewer")?;
    let gone = api.signed(&owner, "DELETE", &format!("/api/f/{tracker}"), None)?;
    let open = visibility(&owner, &other, "link")?;
    s.ok(
        "a fragment deleted lets each app in it go: its link opens again",
        joined.status == 200 && gone.status == 200 && open.status == 200,
        format!("{joined} {gone} {open}"),
    );
    Ok(())
}
