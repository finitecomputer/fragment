//! An app as a member of another fragment (decision 72): an owner adds an
//! app of theirs to a fragment of theirs while everyone it shows is held
//! there; a removal or a lowered role there pauses it until they are held
//! again; while it is in, it stays members only and its audience grows
//! only by that fragment's yes; and removing it, deleting it or deleting
//! the fragment it is in lets it go.

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::{Need, Suite};

pub fn apps(s: &mut Suite, api: &Api) -> Result<()> {
    // Chrome is for its sheets' screenshots alone: without one, the rest runs
    if !s.section("apps", &[Need::Node]) {
        return Ok(());
    }
    // the owner signs in on the platform too: their sheets are shown
    let (owner, owner_session) = super::templates::person(api)?;
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

    // ---- a fragment with no room: refused, and the app is let go at once
    let (full, _) = made(s, &owner, "apps-full")?;
    api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": full, "op": "members", "fill": fragment_proto::limits::MEMBERS_MAX })))?;
    let no_room = put(&owner, &full, &board, "viewer")?;
    let open = visibility(&owner, &board, "link")?;
    s.ok(
        "a fragment at its member cap refuses the app (400), and the app, told at once, is not held to it: its link opens",
        no_room.status == 400 && no_room.message().contains("at most") && open.status == 200,
        format!("{no_room} {open}"),
    );
    visibility(&owner, &board, "members")?;

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
        "while in, the app's link is refused (403), naming where it is; an invite to an email no one signs in as is refused, naming the fix",
        in_line(&open, "so it stays members only.")
            && invite.status == 403
            && invite.message() == format!("{} can see {}. Add apps-carol@e2e.test to {} first.", label(&board), label(&tracker), label(&tracker)),
        format!("{open} {invite}"),
    );
    let editor = put(&owner, &tracker, &board, "editor")?;
    s.ok("its role there changes, asked again", editor.status == 200 && editor.body["role"] == "editor" && app_row(&tracker)?.is_some_and(|m| m["role"] == "editor"), &editor);

    // ---- its people: Carol edits the board, Dana views it; held to them here
    let (carol, dana) = (api.person()?, api.person()?);
    // each by their identity, as a member row names them (not their key's npub)
    let (carol_id, dana_id) = (api.identity(&carol)?, api.identity(&dana)?);
    api.signed(&owner, "DELETE", &format!("/api/f/{tracker}/members/{board}"), None)?;
    api.share(&board, &carol_id, "editor")?;
    api.share(&board, &dana_id, "viewer")?;
    let short = put(&owner, &tracker, &board, "editor")?;
    let named = [&carol, &dana].iter().any(|k| short.message() == format!("{} can see {} but isn't in {}. Add them here first.", Api::email_of(k), label(&board), label(&tracker)));
    s.ok("an app whose people are not all here is refused (403), naming one by their email", short.status == 403 && named && app_row(&tracker)?.is_none(), &short);
    api.share(&tracker, &carol_id, "editor")?;
    api.share(&tracker, &dana_id, "viewer")?;
    let held = put(&owner, &tracker, &board, "editor")?;
    s.ok("held here, its editor an editor, it joins as an editor, not paused", held.status == 200 && held.body["app"].get("paused").is_none(), &held);
    let raised = api.share(&board, &dana_id, "editor")?;
    s.ok(
        "while in, raising one of its people to edit it is asked of the fragment, which says no (403): they only view there",
        raised.status == 403 && raised.message() == format!("{} can edit {}. Make {} an editor there first.", label(&board), label(&tracker), Api::email_of(&dana)),
        &raised,
    );
    let paused = |who: &str| -> Result<bool> { Ok(app_row(&tracker)?.is_some_and(|m| m["app"]["paused"] == who)) };

    // ---- a narrowing: never refused, it pauses the app in the same turn
    let lowered = api.share(&tracker, &carol_id, "viewer")?;
    s.ok("lowering its editor to a viewer here works, and pauses it, naming them", lowered.status == 200 && paused(&carol_id)?, &lowered);
    let again = api.share(&tracker, &carol_id, "viewer")?;
    s.ok("(the same lowering again: still paused, on the same person)", again.status == 200 && paused(&carol_id)?, &again);

    // ---- the two sheets as their owner sees them, at a desktop's width and
    // a phone's, light and dark (evidence for a person): no sideways scroll
    if let Some(mut chrome) = s.browser()? {
        let (platform, wait) = (api.base.clone(), std::time::Duration::from_secs(20));
        chrome.set_cookie(&format!("{platform}/"), "fragment_session", &owner_session)?;
        let (mut fits, mut texts) = (vec![], vec![]);
        for (sheet, base) in [(&tracker, "apps-sheet-fragment"), (&board, "apps-sheet-app")] {
            let page = chrome.open(&format!("{platform}/share/{sheet}"))?;
            for (width, mobile, scheme) in [(1280, false, "light"), (1280, false, "dark"), (390, true, "light"), (390, true, "dark")] {
                chrome.viewport(&page, width, 900, mobile)?;
                chrome.color_scheme(&page, scheme)?;
                let ready = chrome.until(&page, "document.readyState === 'complete' && !!document.querySelector('main.share .people')", wait);
                fits.push(ready && chrome.eval(&page, "document.documentElement.scrollWidth <= innerWidth")? == json!(true));
                chrome.screenshot(&page, &s.scratch.join(format!("{base}-{width}-{scheme}.png")))?;
            }
            texts.push(chrome.eval(&page, "document.querySelector('main.share')?.innerText ?? ''")?.as_str().unwrap_or_default().to_string());
            chrome.close(page)?;
        }
        let paused_line = format!("Paused. {} can edit {} but only views here.", Api::email_of(&carol), label(&board));
        s.ok(
            "the fragment's sheet shows the app as its owner's, paused, naming whom it waits on; the app's says where it is in; each fits, light and dark",
            fits.iter().all(|f| *f) && texts[0].contains(&label(&board)) && texts[0].contains("Your app") && texts[0].contains(&paused_line) && texts[1].contains(&format!("Can see: {}", label(&tracker))),
            format!("{fits:?} {texts:?}"),
        );
    }
    let gone = api.signed(&owner, "DELETE", &format!("/api/f/{tracker}/members/{dana_id}"), None)?;
    let row = app_row(&tracker)?;
    s.ok(
        "removing someone who sees it works too, and it stays paused, still a member at its role",
        gone.status == 200 && row.as_ref().is_some_and(|m| m["role"] == "editor" && (m["app"]["paused"] == carol_id.as_str() || m["app"]["paused"] == dana_id.as_str())),
        format!("{gone} {row:?}"),
    );
    let raise = put(&owner, &tracker, &board, "editor")?;
    s.ok("asked again while its list is short, it is refused (403), and stays paused", raise.status == 403 && app_row(&tracker)?.is_some_and(|m| !m["app"]["paused"].is_null()), &raise);

    // ---- a growth asked before a restart: Erin, a member there, invited here
    let erin = api.person()?;
    let erin_id = api.identity(&erin)?;
    let invite_erin = || api.signed(&owner, "POST", &format!("/api/f/{board}/invites"), Some(&json!({ "email": Api::email_of(&erin), "role": "viewer" })));
    let no = invite_erin()?;
    s.ok(
        "inviting someone not in the fragment it is in is refused (403), naming the fix",
        no.status == 403 && no.message() == format!("{} can see {}. Add {} to {} first.", label(&board), label(&tracker), Api::email_of(&erin), label(&tracker)),
        &no,
    );
    api.share(&tracker, &erin_id, "viewer")?;
    let (yes, again) = (invite_erin()?, invite_erin()?);
    s.ok("once they are in it, the fragment says yes and the invite waits; asked again, yes again", yes.status == 200 && again.status == 200, format!("{yes} {again}"));

    // ---- a restart of every object: still in, still held, still paused
    s.reload()?;
    let open = visibility(&owner, &board, "link")?;
    s.ok(
        "after a restart the app is still in and still paused, and its link still refused",
        app_row(&tracker)?.is_some_and(|m| !m["app"]["paused"].is_null()) && in_line(&open, "so it stays members only."),
        &open,
    );
    api.share(&tracker, &carol_id, "editor")?;
    let one = app_row(&tracker)?;
    let out = api.signed(&owner, "DELETE", &format!("/api/f/{board}/members/{dana_id}"), None)?;
    let both = app_row(&tracker)?;
    s.ok(
        "one held again here, it waits on the other; the other taken out of the app, the fragment looks again and it resumes",
        one.as_ref().is_some_and(|m| m["app"]["paused"] == dana_id.as_str()) && out.status == 200 && both.as_ref().is_some_and(|m| m["app"].get("paused").is_none()),
        format!("{one:?} {out} {both:?}"),
    );

    // ---- the invite's yes asks again, after the restart; then the app narrows
    let accepted = api.signed(&erin, "POST", &format!("/api/invites/{board}/accept"), Some(&json!({})))?;
    let board_people = api.signed(&owner, "GET", &format!("/api/f/{board}/members"), None)?;
    s.ok(
        "their yes, asked of the fragment again, makes them a member of the app",
        accepted.status == 200 && board_people.body["members"].as_array().is_some_and(|m| m.iter().any(|m| m["principal"] == erin_id.as_str())),
        &accepted,
    );
    api.signed(&owner, "DELETE", &format!("/api/f/{tracker}/members/{erin_id}"), None)?;
    let waits = paused(&erin_id)?;
    let left = api.signed(&owner, "DELETE", &format!("/api/f/{board}/members/{erin_id}"), None)?;
    s.ok(
        "removed from the fragment it is in, they pause it; removed from the app too, the fragment looks again and it resumes",
        waits && left.status == 200 && app_row(&tracker)?.is_some_and(|m| m["app"].get("paused").is_none()),
        &left,
    );

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
        rejoined.status == 200 && deleted.status == 200 && app_row(&tracker)?.is_none(),
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
    races(s, api, &owner)
}

/// The races the reviews found, each interleaved by the hold lever
/// (ops.rs `test_hold`): a request held at a named step while another
/// request, or a restart, goes between its turns.
fn races(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let wait = std::time::Duration::from_secs(20);
    let lever = |name: &str, op: &str, step: Option<&str>| api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": op, "step": step })));
    let hold = |name: &str, step: Option<&str>| lever(name, "hold", step);
    let held_at = |name: &str| lever(name, "hold-at", None).ok().and_then(|r| r.body["at"].as_str().map(str::to_string)).unwrap_or_default();
    let made = |s: &Suite, keys: &Keys, base: &str| -> Result<String> {
        let name = s.named(api, keys, base)?;
        s.create(api, keys, &name)?;
        Ok(name)
    };
    let members_only = |keys: &Keys, name: &str| api.signed(keys, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })));
    let members = |keys: &Keys, name: &str| -> Vec<Value> { api.signed(keys, "GET", &format!("/api/f/{name}/members"), None).ok().and_then(|r| r.body["members"].as_array().cloned()).unwrap_or_default() };
    let row = |keys: &Keys, target: &str, app: &str| members(keys, target).into_iter().find(|m| m["app"]["name"] == app);

    // ---- a join's look outlives a restart: Robin leaves the app between its
    // answer and the target's commit (the app's word to look finds no row
    // yet), and the node restarts before the target looks again
    let (target, app) = (made(s, owner, "apps-recon-t")?, made(s, owner, "apps-recon-a")?);
    members_only(owner, &app)?;
    let robin = api.person()?;
    let robin_id = api.identity(&robin)?;
    api.share(&target, &robin_id, "viewer")?;
    api.share(&app, &robin_id, "viewer")?;
    hold(&target, Some("app-answer"))?;
    let reached = std::thread::scope(|scope| -> Result<bool> {
        let join = scope.spawn(|| api.signed(owner, "PUT", &format!("/api/f/{target}/members/{app}"), Some(&json!({ "role": "viewer" }))));
        let answered = s.eventually(wait, || held_at(&target) == "app-answer");
        let left = api.signed(owner, "DELETE", &format!("/api/f/{app}/members/{robin_id}"), None)?.status == 200;
        hold(&target, Some("app-committed"))?;
        let committed = s.eventually(wait, || held_at(&target) == "app-committed");
        s.reload()?;
        let _ = join.join();
        Ok(answered && left && committed)
    })?;
    hold(&target, None)?;
    lever(&target, "poll-now", None)?;
    let joined = row(owner, &target, &app).is_some();
    let out = api.signed(owner, "DELETE", &format!("/api/f/{target}/members/{robin_id}"), None)?;
    let resumed = s.eventually(wait, || row(owner, &target, &app).is_some_and(|m| m["app"].get("paused").is_none()));
    s.ok(
        "a join taken with someone who left the app meanwhile is looked at again, a restart between: their leaving the target pauses nothing",
        reached && joined && out.status == 200 && resumed,
        format!("reached {reached} joined {joined} {out} {:?}", row(owner, &target, &app)),
    );

    // ---- a failed resend restores only what its own write replaced: the
    // invite revoked while its growth was asked stays revoked
    let ann = api.person()?;
    api.share(&target, &api.identity(&ann)?, "editor")?;
    let invite_ann = |role: &str| api.signed(owner, "POST", &format!("/api/f/{app}/invites"), Some(&json!({ "email": Api::email_of(&ann), "role": role })));
    let first = invite_ann("editor")?;
    hold(&app, Some("invite-asked"))?;
    let (revoked, resent) = std::thread::scope(|scope| -> Result<(u16, u16)> {
        let resend = scope.spawn(|| invite_ann("viewer"));
        s.eventually(wait, || held_at(&app) == "invite-asked");
        let revoked = api.signed(owner, "DELETE", &format!("/api/f/{app}/invites/{}", Api::email_of(&ann)), None)?.status;
        s.mail.fail_next("E_SEND_FAILED");
        hold(&app, None)?;
        Ok((revoked, resend.join().expect("the resend's thread")?.status))
    })?;
    let waiting = api.signed(owner, "GET", &format!("/api/f/{app}/invites"), None)?;
    s.ok(
        "a resend whose mail fails, the invite revoked while it was asked, brings back no invite",
        first.status == 200 && revoked == 200 && resent != 200 && waiting.body["invites"].as_array().is_some_and(|i| i.is_empty()),
        format!("{first} revoked {revoked} resent {resent} {waiting}"),
    );
    Ok(())
}
