//! Sharing (docs/api.md, Sharing): the
//! share sheet on the platform's origin, and invites by email, on a node
//! shaped as fragment.club is (the platform cross-site from every fragment,
//! so its session reaches a fragment's page only on a top-level visit), in
//! Chrome where the browser is the point.
//!
//! The owner shares a todo list by email with someone who has never signed
//! in: they are mailed the platform's page, their sign-in as that email
//! shows them the invite and makes them no member, and their yes does
//! (decision 63: valid, someone else's, an agent's, with a body, replayed);
//! its address then opens it for them; they see it live, and add to it;
//! the owner removes them, and their socket closes and the list answers
//! them 403. Someone who signs in already is invited the same way, and
//! accepts it in their shell's sidebar; a revoked invite admits no one,
//! and a declined one waits no more. A member sees who is in and changes
//! nothing. A fragment's page (its author's code, or an agent's)
//! cannot share: not by fetching the sheet or the API, not by framing the
//! sheet, not with a signed call (it holds no key), and not by scripting
//! the window it opens on the sheet (only the platform's own page frames
//! it, in a dialog). Every form needs the page's own token, bound to the
//! session, and arms only after a moment.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use fragment_core::form;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::signin::{site_cookie, unframed, with_session};
use super::templates::{email_of, person};
use crate::api::{url_enc, Api, Call, Reply};
use crate::browser::{Browser, Page};
use crate::Suite;

/// Runs on a node restarted with the platform and the fragments on two domains
/// (as fragment.club is), then restarts it as it was for the lanes after.
pub fn share(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("share", &[crate::Need::Node, crate::Need::Chrome, crate::Need::Levers, crate::Need::Fakes]) {
        return Ok(());
    }
    s.stop()?;
    let api = s.start_as_browsers_see_it()?;
    let result = run(s, &api);
    drop(api);
    s.stop()?;
    s.start(false)?;
    result
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after 1970").as_millis() as i64
}

/// A page's value for an input named or with the id `key`.
fn value_of(r: &Reply, attr: &str) -> String {
    let at = r.text.find(attr).map(|i| i + attr.len());
    at.and_then(|i| r.text[i..].split_once("value=\"")).and_then(|(_, rest)| rest.split('"').next()).unwrap_or("").replace("&amp;", "&")
}

/// The form token a sharing page holds.
pub(super) fn form_of(r: &Reply) -> String {
    value_of(r, "name=\"form\"")
}

/// Whether only the platform's own pages may frame a page: `'self'` and
/// nothing else, so no fragment's origin (another origin, one site with the
/// platform or not) may.
fn framed_by_platform(r: &Reply) -> bool {
    let csp = r.header("content-security-policy");
    csp.split(';').map(str::trim).filter(|d| d.starts_with("frame-ancestors")).eq(["frame-ancestors 'self'"]) && r.header("x-frame-options") == "SAMEORIGIN"
}

/// The shell's share sheet is open and has sized its frame: the sheet's own
/// script told the shell its height, so its buttons are wired too. Its
/// frame's DOM comes before that: the script waits on the platform's
/// stylesheet, which the page links ahead of its content (share.rs
/// `platform_html`; kept once read, shell.rs `versioned`, but a first open
/// fetches it), and a Done clicked meanwhile does nothing.
const SHEET_SIZED: &str = "document.getElementById('sheet').open && !!document.querySelector('#sheet iframe')?.style.height";

/// Whether a page severs a window that opened it.
fn unopened(r: &Reply) -> bool {
    r.header("cross-origin-opener-policy") == "same-origin"
}

fn headers(r: &Reply) -> String {
    format!(
        "{} csp {:?} xfo {:?} coop {:?}",
        r.status,
        r.header("content-security-policy"),
        r.header("x-frame-options"),
        r.header("cross-origin-opener-policy")
    )
}

/// A form a sharing page posts, from `origin`, with `session`'s cookie.
fn post(api: &Api, path: &str, session: &str, origin: &str, fields: &[(&str, &str)]) -> Result<Reply> {
    let body = fields.iter().map(|(k, v)| format!("{k}={}", url_enc(v))).collect::<Vec<_>>().join("&");
    api.call(Call {
        method: "POST",
        url: format!("{}{path}", api.base),
        body: Some(body.into_bytes()),
        content_type: Some("application/x-www-form-urlencoded"),
        cookie: Some(format!("fragment_session={session}")),
        extra: vec![("origin", origin.to_string())],
        ..Call::default()
    })
}

/// A page's form token once its buttons have armed.
fn armed(api: &Api, path: &str, session: &str) -> Result<String> {
    let r = with_session(api, "GET", path, session)?;
    anyhow::ensure!(r.status == 200, "GET {path}: {r}");
    std::thread::sleep(Duration::from_millis(form::DELAY_MS as u64 + 100));
    Ok(form_of(&r))
}

fn run(s: &mut Suite, api: &Api) -> Result<()> {
    let wait = Duration::from_secs(20);
    let platform = api.base.clone();
    let (owner, owner_session) = person(api)?;
    // the guest has never signed in: the owner's invite waits on their email
    let guest = Keys::generate();
    let (stranger, stranger_session) = person(api)?;
    let (member, member_session) = person(api)?;
    // people are shown, and invited, by email (decisions 45 and 48)
    let (owner_name, guest_name, stranger_name, member_name) = (email_of(&owner), email_of(&guest), email_of(&stranger), email_of(&member));
    let stranger_id = api.identity(&stranger)?;
    let make = |label: &str, template: &str| -> Result<String> {
        let name = api.qualified(&owner, label)?;
        let r = api.create_with(&owner, json!({ "name": name, "template": template }))?;
        anyhow::ensure!(r.status == 200, "making {name}: {r}");
        Ok(name)
    };
    let chat = make(&s.name("list"), "todo")?;
    // an agent of the owner's is in it too: it counts as no guest
    let hand = Keys::generate();
    let reg = "/api/identities";
    let r = api.signed(&owner, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hand, "POST", reg, &owner) })))?;
    anyhow::ensure!(r.status == 200, "registering the owner's agent: {r}");
    let agent = r.body["id"].as_str().unwrap_or("").to_string();
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the owner's agent joins {chat}: {r}");
    let sheet = format!("/share/{chat}");
    let invites_for = |email: &str| -> Result<Vec<Value>> {
        let r = api.signed(&owner, "GET", &format!("/api/f/{chat}/invites"), None)?;
        Ok(r.body["invites"].as_array().into_iter().flatten().filter(|i| i["email"] == email).cloned().collect())
    };
    let role_of = |id: &str| -> Result<Option<String>> {
        let r = api.signed(&owner, "GET", &format!("/api/f/{chat}/members"), None)?;
        Ok(r.body["members"].as_array().into_iter().flatten().find(|m| m["principal"] == id).and_then(|m| m["role"].as_str().map(str::to_string)))
    };

    // ---- the sheet
    let r = api.unsigned("GET", &sheet, None)?;
    s.ok("the share sheet sends a signed-out browser to sign in first, and back", r.status == 302 && r.header("location").contains(&format!("/auth/login?return={}", url_enc(&sheet))), &r);
    let r = with_session(api, "GET", &sheet, &owner_session)?;
    let title = api.status(&owner, &chat)?.body["title"].as_str().unwrap_or("").to_string();
    s.ok(
        "the owner's sheet, titled with the fragment's title, shows who is in, by email (their agent too, as theirs), and the owner's controls",
        r.status == 200
            && !title.is_empty()
            && r.text.contains(&format!("<h1>Share “{title}”</h1>"))
            && r.text.contains(&owner_name)
            && r.text.contains("Your agent")
            && r.text.contains("name=\"email\"")
            && r.text.contains("General access"),
        &r,
    );
    // whoever the owner lets into a chat can talk to their agent in it: the
    // sheet says so by the invite, on a chat their agent is in and nowhere
    // else (Paul, 2026-10-23 and 10-24): not on an app it is in (no agent
    // hears one: this list), nor an app or a chat without it
    let warning = fragment_core::access::TRUST_WARNING;
    let plain = make(&s.name("plain"), "blank")?;
    let without = with_session(api, "GET", &format!("/share/{plain}"), &owner_session)?;
    let talk = make(&s.name("talk"), "chat")?;
    let bare = with_session(api, "GET", &format!("/share/{talk}"), &owner_session)?;
    let joined = api.signed(&owner, "PUT", &format!("/api/f/{talk}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    let warned = with_session(api, "GET", &format!("/share/{talk}"), &owner_session)?;
    let invites = |r: &Reply| r.status == 200 && r.text.contains("name=\"email\"");
    s.ok(
        "beside its invite, the sheet of a chat the owner's agent is in says people in it can talk to their agent; an app it is in, an app without it, and a chat without it do not",
        joined.status == 200
            && invites(&warned)
            && warned.text.contains(&format!("<p class=\"hint trust\">{warning}</p>"))
            && [&r, &without, &bare].iter().all(|r| invites(r) && !r.text.contains(warning)),
        json!([joined.status, warned.status, bare.status, without.status]),
    );
    s.ok(
        "only the platform's own pages may frame it (`frame-ancestors 'self'` alone: every fragment is another origin), and it severs a window that opened it",
        framed_by_platform(&r) && unopened(&r),
        headers(&r),
    );
    // its look and its script are the release's files, linked by their
    // versions, which a browser keeps: opening the sheet again waits on no
    // round trip for them (shell.rs `versioned`); none is inline
    let linked = |prefix: &str| r.text.split(['"', '\'']).find(|u| u.starts_with(prefix)).unwrap_or("").to_string();
    let files = [linked("/__shell/sheet.css?v="), linked("/__shell/platform-pages.css?v="), linked("/__shell/platform-pages.js?v=")];
    let kept = files.iter().map(|f| api.unsigned("GET", f, None)).collect::<Result<Vec<_>>>()?;
    let other = api.unsigned("GET", "/__shell/platform-pages.css?v=0000", None)?;
    let named = api.unsigned("GET", "/__shell/platform-pages.css", None)?;
    s.ok(
        "the sheet links its stylesheets and its script (a module) by their versions, each kept for good (immutable), and holds no style of its own; by another version, or by name alone, a file is revalidated",
        files.iter().all(|f| !f.is_empty())
            && r.text.contains(&format!("<script type=\"module\" src=\"{}\">", files[2]))
            && !r.text.contains("<style")
            && r.header("content-security-policy").contains("script-src 'self' 'unsafe-inline'")
            && kept.iter().all(|k| k.status == 200 && k.header("cache-control") == "public, max-age=31536000, immutable")
            && [&other, &named].iter().all(|r| r.status == 200 && r.header("cache-control") == "no-cache"),
        json!([files, kept.iter().map(|k| k.header("cache-control")).collect::<Vec<_>>(), other.header("cache-control"), named.header("cache-control")]),
    );
    s.ok(
        "every choice on it is a button, none a select (which an arrow key changes in passing), and they come disabled: they arm a moment after the page shows",
        r.text.contains("data-arm disabled") && !r.text.contains("data-arm>") && !r.text.contains("<select"),
        "",
    );
    s.ok("and it offers the share link to copy", r.text.contains(&format!("{}?view=", api.site_url(&chat, ""))), "");
    let r = with_session(api, "GET", &format!("{sheet}?email={}&role=editor&action=invite&visibility=public&form=1", url_enc(&guest_name)), &owner_session)?;
    s.ok(
        "it takes nothing from its URL: a link cannot prefill what a click would approve",
        r.status == 200 && !r.text.contains(&guest_name) && r.text.contains("value=\"link\" aria-pressed=\"true\"") && !r.text.contains("value=\"public\" aria-pressed=\"true\""),
        &r,
    );
    let r = with_session(api, "GET", &sheet, &stranger_session)?;
    s.ok("someone who is not in it gets a 403 page", r.status == 403 && r.text.contains("Not yours to share") && !r.text.contains("name=\"form\"") && unframed(&r), &r);
    // ---- its forms: the page's own token, from the platform's origin, after a moment
    let invite = |form: &str, session: &str, origin: &str| {
        post(api, &sheet, session, origin, &[("form", form), ("action", "invite"), ("email", guest_name.as_str()), ("role", "editor")])
    };
    let fresh = form_of(&with_session(api, "GET", &sheet, &owner_session)?);
    let r = invite(&fresh, &owner_session, &platform)?;
    s.ok(
        "a form sent before its buttons arm is refused (403): the click that opened the sheet cannot confirm in it",
        r.status == 403 && r.text.contains("too quick") && invites_for(&guest_name)?.is_empty() && s.mail.sent_to(&guest_name).is_empty(),
        &r,
    );
    std::thread::sleep(Duration::from_millis(form::DELAY_MS as u64 + 100));
    let r = invite("", &owner_session, &platform)?;
    s.ok("a POST without the form's token is refused (403)", r.status == 403 && invites_for(&guest_name)?.is_empty(), &r);
    let other_session = api.sign_in(&format!("t-{}@e2e.test", &owner.pubkey_hex()[..12]))?;
    let others = armed(api, &sheet, &other_session)?;
    let r = invite(&others, &owner_session, &platform)?;
    s.ok("and one with another session's token (the same person's other browser)", r.status == 403 && invites_for(&guest_name)?.is_empty(), &r);
    let r = invite(&fresh, &owner_session, &api.site_origin(&chat))?;
    s.ok("a POST from a fragment's page is refused (403), with a good token too", r.status == 403 && invites_for(&guest_name)?.is_empty(), &r);
    let mut portless = reqwest::Url::parse(&platform)?;
    let _ = portless.set_port(None);
    let r = invite(&fresh, &owner_session, &portless.origin().ascii_serialization())?;
    s.ok("and one from the platform's host on another port (another origin, whose name begins the same)", r.status == 403 && invites_for(&guest_name)?.is_empty(), &r);
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &fresh), ("action", "invite"), ("email", "not an email"), ("role", "viewer")])?;
    s.ok("an invite to what is not an email says so (400)", r.status == 400 && r.text.contains("is not an email"), &r);

    // ---- the owner shares the chat by email with someone who has never signed in
    let r = invite(&fresh, &owner_session, &platform)?;
    let pending = invites_for(&guest_name)?;
    s.ok(
        "the owner invites a person by email who has never signed in: the sheet says they were mailed, and the invite waits on their email",
        r.status == 200 && r.text.contains("we mailed them a link") && pending.len() == 1 && pending[0]["role"] == "editor",
        &r,
    );
    let mailed = s.mail.sent_to(&guest_name);
    let link = api.site_url(&chat, "");
    s.ok(
        "they are mailed, from the deployment's address: who invites them to what, on the platform the shell names, and the platform's own page to accept it on (never the fragment's address), which holds no secret",
        mailed.len() == 1
            && mailed[0].from == json!({ "email": "mail@fragment.localhost", "name": "fragment" })
            && mailed[0].subject.starts_with(&format!("{owner_name} invited you to "))
            && mailed[0].text.contains(&format!(" on {}.\n", fragment_core::mail::PRODUCT))
            && mailed[0].text.contains(&format!("Accept it: {platform}/\n"))
            && mailed[0].text.contains(&format!("Sign in as {guest_name}, then accept or decline it"))
            && !mailed[0].text.contains(&link)
            && !mailed[0].text.contains("?view=")
            && !mailed[0].text.contains("token"),
        mailed.first().map(|m| format!("{} / {}", m.subject, m.text)).unwrap_or_default(),
    );
    let r = with_session(api, "GET", &sheet, &owner_session)?;
    s.ok("it waits on the owner's sheet, by email, with its role", r.text.contains(&guest_name) && r.text.contains("Invited as an editor"), &r);
    let listed = |name: &str| -> Result<Value> {
        let r = api.signed(&owner, "GET", "/api/fragments", None)?;
        Ok(r.body["fragments"].as_array().into_iter().flatten().find(|f| f["name"] == name).cloned().unwrap_or_default())
    };
    s.ok("and the owner's list counts it, beside its members", listed(&chat)?["sharing"]["invites"] == 1, listed(&chat)?);
    let form = armed(api, &sheet, &owner_session)?;
    let r = invite(&form, &owner_session, &platform)?;
    s.ok(
        "the same invite again renews it: still one waiting, mailed again, and the sheet says so",
        r.status == 200 && r.text.contains("again") && invites_for(&guest_name)?.len() == 1 && s.mail.sent_to(&guest_name).len() == 2,
        &r,
    );
    // a sign-in as another email meets nothing addressed to this one
    api.sign_in(&stranger_name)?;
    s.ok("someone else's sign-in meets none of it", role_of(&stranger_id)?.is_none() && invites_for(&guest_name)?.len() == 1, "");

    // ---- their sign-in shows them the invite; their yes makes them a member (decision 63)
    let guest_session = api.sign_in(&guest_name)?;
    api.approve(&guest_session, &guest)?;
    let guest_id = api.identity(&guest)?;
    s.ok(
        "their first sign-in as that email makes them no member: the invite waits for their yes",
        role_of(&guest_id)?.is_none() && invites_for(&guest_name)?.len() == 1,
        json!({ "role": role_of(&guest_id)?, "waiting": invites_for(&guest_name)? }),
    );
    let theirs = api.signed(&guest, "GET", "/api/invites", None)?;
    let shown = theirs.body["invites"].as_array().cloned().unwrap_or_default();
    // the registry answers its rows as read: the wire is proto's type
    let typed = serde_json::from_value::<fragment_proto::InvitesWaiting>(theirs.body.clone());
    s.ok(
        "they see it waiting (GET /api/invites, proto's InvitesWaiting): what it shares, as its title, as what, from whom, on which email",
        theirs.status == 200
            && typed.is_ok_and(|t| t.invites.len() == 1 && t.invites[0].role == fragment_proto::Role::Editor)
            && shown.len() == 1
            && shown[0]["fragment"] == chat.as_str()
            && shown[0]["title"] == title.as_str()
            && shown[0]["role"] == "editor"
            && shown[0]["from"] == owner_name.as_str()
            && shown[0]["email"] == guest_name.as_str()
            && shown[0]["expiresAt"].as_i64().is_some_and(|t| t > now_ms()),
        &theirs,
    );
    let others = api.signed(&stranger, "GET", "/api/invites", None)?;
    let agents = api.signed(&hand, "GET", "/api/invites", None)?;
    let said_for = api.signed(&guest, "GET", &format!("/api/invites?for={guest_id}"), None)?;
    s.ok(
        "no one else sees it: another person's are theirs (none), an agent's is 403 (it has no email), and `for` is 400",
        others.status == 200 && others.body["invites"] == json!([]) && agents.status == 403 && said_for.status == 400,
        format!("{others} {agents} {said_for}"),
    );
    let accept = format!("/api/invites/{chat}/accept");
    let foreign = api.signed(&stranger, "POST", &accept, Some(&json!({})))?;
    let bodied = api.signed(&guest, "POST", &accept, Some(&json!({ "role": "owner" })))?;
    let by_agent = api.signed(&hand, "POST", &accept, Some(&json!({})))?;
    s.ok(
        "someone else's yes is 404 (it waits on no email of theirs), a yes with a body is 400 (it says nothing but yes), and an agent's 403; none makes anyone a member",
        foreign.status == 404 && foreign.body["reason"] == "gone" && bodied.status == 400 && by_agent.status == 403 && role_of(&stranger_id)?.is_none() && role_of(&guest_id)?.is_none() && invites_for(&guest_name)?.len() == 1,
        format!("{foreign} {bodied} {by_agent}"),
    );
    let r = api.signed(&guest, "POST", &accept, Some(&json!({})))?;
    let listed_theirs = api.signed(&guest, "GET", "/api/fragments", None)?;
    let in_list = listed_theirs.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == chat.as_str() && f["role"] == "editor"));
    s.ok(
        "their yes makes them a member, at the role it said: the invite is spent, and it is in their list as it answers",
        r.status == 200 && r.body == json!({ "fragment": chat, "role": "editor" }) && role_of(&guest_id)?.as_deref() == Some("editor") && invites_for(&guest_name)?.is_empty() && in_list,
        format!("{r} {listed_theirs}"),
    );
    let members_before = api.signed(&owner, "GET", &format!("/api/f/{chat}/members"), None)?.body["members"].as_array().map_or(0, Vec::len);
    let again = api.signed(&guest, "POST", &accept, Some(&json!({})))?;
    let after = api.signed(&guest, "GET", "/api/invites", None)?;
    api.sign_in(&guest_name)?;
    let members_after = api.signed(&owner, "GET", &format!("/api/f/{chat}/members"), None)?.body["members"].as_array().map_or(0, Vec::len);
    s.ok(
        "their yes again (a replay) is 404, gone, and changes nothing; nothing waits for them now, and signing in again meets nothing",
        again.status == 404 && again.body["reason"] == "gone" && after.body["invites"] == json!([]) && members_after == members_before && role_of(&guest_id)?.as_deref() == Some("editor"),
        format!("{again} {after} {members_before} → {members_after}"),
    );
    let r = api.signed(&owner, "GET", &format!("/api/f/{chat}/events?tail=50"), None)?;
    s.ok(
        "the events say an invite was made and they joined, by npub, and name no email (a link's viewers read them)",
        r.text.contains("invite.created") && r.text.contains("member.joined") && r.text.contains(&guest_id) && !r.text.contains(&guest_name),
        &r,
    );
    let on_chat = site_cookie(api, &owner_session, &chat)?;
    let r = api.page(&chat, "__join?invite=x", Some(&format!("fragment_site={on_chat}")))?;
    s.ok("a fragment's origin accepts no invites (there is no page to accept one on)", r.status == 404 && !r.text.contains("Join"), &r);
    let r = api.unsigned("GET", &format!("/join/{chat}?token={}", "0".repeat(48)), None)?;
    s.ok("nor does the platform: invites by link are gone (an unguessable link is `link` visibility)", r.status == 404, &r);

    // ---- in Chrome: its address opens the chat; they see it live, and post in it
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the share lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    chrome.set_cookie(&format!("{platform}/"), "fragment_session", &guest_session)?;
    let page = chrome.open(&link)?;
    let landed = chrome.until(&page, &format!("location.host.startsWith({chat:?}) && document.title === 'Todo'"), wait);
    chrome.screenshot(&page, &s.scratch.join("share-accepted-link.png"))?;
    s.ok(
        "its address opens the chat for them, signed in on its origin, asking nothing: it is shared with them, accepted",
        landed,
        chrome.eval(&page, "location.href + ' ' + document.title").unwrap_or_default(),
    );
    // the list's page is live once it knows who is here
    let connected = chrome.until(&page, "document.getElementById('here')?.textContent.includes('here')", wait);
    let r = api.op(&owner, &chat, "add", "share-owner-1", json!({ "text": "from the owner" }))?;
    let live = chrome.until(&page, "document.getElementById('todos').textContent.includes('from the owner')", wait);
    s.ok("the guest sees the owner's change arrive live", connected && r.status == 200 && live, &r);
    chrome.eval(&page, "document.getElementById('text').value = 'from the guest'; document.getElementById('add').requestSubmit(); true")?;
    let ops = || api.signed(&owner, "GET", &format!("/api/f/{chat}/channels/ops"), None);
    let added = s.eventually(wait, || {
        ops().is_ok_and(|r| r.body["records"].as_array().is_some_and(|a| a.iter().any(|x| x["body"]["op"] == "add" && x["principal"] == guest_id.as_str())))
    });
    s.ok("and adds to it, as themselves", added, ops()?);

    // ---- the owner's list: who else is in each of their fragments (the
    // fragment sent it as the guest joined), as the platform's page shows
    let entry = listed(&chat)?;
    let members = api.signed(&owner, "GET", &format!("/api/f/{chat}/members"), None)?.body["members"].as_array().map_or(0, Vec::len);
    s.ok(
        "the owner's list reports each fragment's member count, guests (not the owner or their agents) and invites waiting, and its visibility",
        entry["sharing"] == json!({ "visibility": "link", "members": members, "guests": 1, "invites": 0 }) && members == 3,
        &entry,
    );
    // Goal: the list reads no fragment. Method: a test hook adds members
    // to a fragment without it telling anyone (its index is not touched):
    // the list still says what it was sent, not what the fragment would;
    // the fragment's next real change updates it.
    let quiet = make(&s.name("squiet"), "blank")?;
    // its create told the list who is in it, and its template's install
    // its face (sending the sharing as it was then): real changes, so both
    // land before the fill
    let installed = s.eventually(wait, || listed(&quiet).is_ok_and(|f| f.get("sharing").is_some() && f["title"] == "Blank"));
    anyhow::ensure!(installed, "{quiet}'s create and its template's install never reached its owner's list");
    let before = listed(&quiet)?;
    let r = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": quiet, "op": "members", "fill": 4 })))?;
    let filled = api.status(&owner, &quiet)?;
    let after = listed(&quiet)?;
    s.ok(
        "the owner's list wakes no fragment: its counts are the list's, not the fragment's own",
        before["sharing"] == json!({ "visibility": "link", "members": 1, "guests": 0, "invites": 0 })
            && r.status == 200
            && filled.body["counts"]["members"] == 4
            && after["sharing"] == before["sharing"],
        format!("{before} / {after} / {}", filled.body["counts"]),
    );
    let r = api.signed(&owner, "PUT", &format!("/api/f/{quiet}/visibility"), Some(&json!({ "visibility": "public" })))?;
    let after = listed(&quiet)?;
    s.ok(
        "and a change to who may open it sends the fragment's sharing to the list",
        r.status == 200 && after["sharing"] == json!({ "visibility": "public", "members": 4, "guests": 3, "invites": 0 }),
        &after,
    );

    // ---- the owner removes the guest: their socket closes, and the chat is 403 to them
    let form = armed(api, &sheet, &owner_session)?;
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "role"), ("member", &guest_id), ("role", "remove")])?;
    s.ok("a role that is no role removes no one (400): removing is its own button", r.status == 400 && role_of(&guest_id)?.as_deref() == Some("editor"), &r);
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "remove"), ("member", &guest_id)])?;
    s.ok("the owner removes the guest from the sheet (its Remove)", r.status == 303 && r.header("location") == sheet && role_of(&guest_id)?.is_none(), &r);
    let said = "document.getElementById('here')?.textContent ?? ''";
    let closed = chrome.until(&page, &format!("({said}).includes('access changed')"), wait);
    s.ok("the guest's socket closes (their page says its access changed)", closed, chrome.eval(&page, said).unwrap_or_default());
    let again = site_cookie(api, &guest_session, &chat)?;
    let r = api.page(&chat, "", Some(&format!("fragment_site={again}")))?;
    chrome.reload(&page)?;
    let refused = chrome.until(&page, "document.contentType === 'text/html' && document.body?.innerText.includes(\"You don't have access to\")", wait);
    s.ok("and the chat answers them 403: their reload, a page saying they have no access", r.status == 403 && refused, &r);
    drop(chrome);

    // ---- someone who signs in already is invited too: mailed, and in once they accept (decision 63)
    let member_id = api.identity(&member)?;
    let form = armed(api, &sheet, &owner_session)?;
    let mails_before = s.mail.sent_to(&member_name).len();
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "invite"), ("email", &member_name.to_uppercase()), ("role", "viewer")])?;
    let theirs = api.signed(&member, "GET", "/api/invites", None)?;
    s.ok(
        "an email someone signs in as already (as typed, in any case) makes them no member: it waits for their yes, mailed, and they see it",
        r.status == 200
            && r.text.contains(&format!("Invited <b>{member_name}</b>"))
            && role_of(&member_id)?.is_none()
            && invites_for(&member_name)?.len() == 1
            && s.mail.sent_to(&member_name).len() == mails_before + 1
            && theirs.body["invites"].as_array().is_some_and(|l| l.iter().any(|i| i["fragment"] == chat.as_str() && i["role"] == "viewer")),
        format!("{r} {theirs}"),
    );
    let r = api.signed(&member, "POST", &format!("/api/invites/{chat}/accept"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200 && role_of(&member_id)?.as_deref() == Some("viewer"), "{member_name} accepts: {r}");
    // Add is for adding: someone in already keeps their role (an Add at
    // the default, Viewer, once lowered an editor without a word)
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "invite"), ("email", &member_name), ("role", "editor")])?;
    s.ok(
        "adding someone in already changes nothing, and says they are in, as what: their role changes in the list",
        r.status == 200 && r.text.contains(&format!("<b>{member_name}</b> is in already, as a viewer")) && role_of(&member_id)?.as_deref() == Some("viewer"),
        &r,
    );
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "invite"), ("email", &owner_name.to_uppercase()), ("role", "viewer")])?;
    s.ok("adding the owner's own email says it is them, and changes nothing", r.status == 200 && r.text.contains(&format!("<b>{owner_name}</b> is you")), &r);
    // the API refuses it as the sheet does: an invite adds, a role changes with PUT members
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": member_name, "role": "editor" })))?;
    let own = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": owner_name, "role": "viewer" })))?;
    s.ok(
        "an invite to someone in already is refused (400), saying they are in and how a role changes; the owner's own email, that they own it",
        r.status == 400 && r.message().contains("is in it already, as a viewer") && r.message().contains("PUT members") && role_of(&member_id)?.as_deref() == Some("viewer")
            && own.status == 400 && own.message().contains("owns it"),
        format!("{r} {own}"),
    );
    // a person's membership is their agents' (decision 64): one they make
    // now comes in beside them, and the sheet names the person, one entry
    // "with their agents", never the agent (decision 63: "I invite Skyler
    // and his agents, not Skyler and his agent Agent123")
    let member_agent = Keys::generate();
    let reg = "/api/identities";
    let made = api.signed(&member, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&member_agent, "POST", reg, &member) })))?;
    let theirs_id = made.body["id"].as_str().unwrap_or("-").to_string();
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "role"), ("member", &member_id), ("role", "editor")])?;
    let shown = with_session(api, "GET", &sheet, &owner_session)?;
    s.ok(
        "an agent the member makes comes in beside them; the owner changes their role on the sheet, and it follows; the person is one entry there, \"with their agents\", the agent never named (no name, no npub)",
        made.status == 200
            && r.status == 303
            && role_of(&member_id)?.as_deref() == Some("editor")
            && role_of(&theirs_id)?.as_deref() == Some("editor")
            && shown.text.contains(&member_name)
            && shown.text.contains("With their agents")
            && !shown.text.contains(&theirs_id),
        format!("{r} {made}"),
    );
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat}/members/{member_id}"), Some(&json!({ "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200 && r.body["role"] == "viewer", "{member_name} a viewer again: {r}");

    // ---- a revoked invite admits no one
    let late = Keys::generate();
    let late_name = email_of(&late);
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "invite"), ("email", &late_name), ("role", "viewer")])?;
    let waited = r.status == 200 && invites_for(&late_name)?.len() == 1;
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "uninvite"), ("email", &late_name)])?;
    let late_session = api.sign_in(&late_name)?;
    api.approve(&late_session, &late)?;
    let late_id = api.identity(&late)?;
    let theirs = api.signed(&late, "GET", "/api/invites", None)?;
    let yes = api.signed(&late, "POST", &format!("/api/invites/{chat}/accept"), Some(&json!({})))?;
    s.ok(
        "the owner revokes an invite from the sheet: it waits no more, they see none after their sign-in, and their yes is 404, making them no member",
        waited && r.status == 303 && invites_for(&late_name)?.is_empty() && theirs.body["invites"] == json!([]) && yes.status == 404 && role_of(&late_id)?.is_none(),
        format!("{r} {theirs} {yes}"),
    );

    // ---- their no: the invite goes, there and in the owner's list, and a yes after it is 404
    let (decliner, _) = person(api)?;
    let decliner_name = email_of(&decliner);
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": decliner_name, "role": "editor" })))?;
    let counted = listed(&chat)?["sharing"]["invites"].clone();
    let decline = format!("/api/invites/{chat}/decline");
    let no = api.signed(&decliner, "POST", &decline, Some(&json!({})))?;
    let decliner_id = api.identity(&decliner)?;
    let yes = api.signed(&decliner, "POST", &format!("/api/invites/{chat}/accept"), Some(&json!({})))?;
    let no_again = api.signed(&decliner, "POST", &decline, Some(&json!({})))?;
    let events = api.signed(&owner, "GET", &format!("/api/f/{chat}/events?tail=20"), None)?;
    s.ok(
        "they decline it: it waits no more (the owner's list counts it no more), their yes after is 404 and their no again 404, and its event names no one",
        r.status == 200
            && !r.body["invited"].is_null()
            && counted == 1
            && no.status == 200
            && no.body == json!({ "ok": true, "declined": chat })
            && invites_for(&decliner_name)?.is_empty()
            && listed(&chat)?["sharing"]["invites"] == 0
            && yes.status == 404
            && no_again.status == 404
            && role_of(&decliner_id)?.is_none()
            && events.text.contains("invite.declined")
            && !events.text.contains(&decliner_id)
            && !events.text.contains(&decliner_name),
        format!("{r} {no} {yes} {no_again}"),
    );

    // ---- a member sees who is in, and changes nothing
    let r = with_session(api, "GET", &sheet, &member_session)?;
    let actions: Vec<&str> = r.text.match_indices("name=\"action\" value=\"").map(|(i, m)| r.text[i + m.len()..].split('"').next().unwrap_or("")).collect();
    s.ok(
        "a member sees who has access, and no link, and their one control is Leave",
        r.status == 200
            && r.text.contains(&format!("<h1>Who has access to “{title}”</h1>"))
            && r.text.contains(&owner_name)
            && actions == ["leave"]
            && !r.text.contains("name=\"email\"")
            && !r.text.contains("?view="),
        format!("{actions:?} {r}"),
    );
    // a good token for the member's own session (the harness holds the
    // cookie a browser would), so only the fragment's rule is left to refuse
    let token_of_member = form::issue(&member_session, &format!("share:{chat}"), now_ms() - form::DELAY_MS - 50);
    let waiting = format!("w-{}@e2e.test", &Keys::generate().pubkey_hex()[..12]);
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": waiting, "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200 && !r.body["invited"].is_null(), "an invite waiting on {waiting}: {r}");
    let before = api.status(&owner, &chat)?;
    let mut answers = vec![];
    for fields in [
        vec![("action", "invite"), ("email", stranger_name.as_str()), ("role", "editor")],
        vec![("action", "visibility"), ("visibility", "public")],
        vec![("action", "rotate")],
        vec![("action", "role"), ("member", agent.as_str()), ("role", "viewer")],
        vec![("action", "remove"), ("member", agent.as_str())],
        vec![("action", "uninvite"), ("email", waiting.as_str())],
    ] {
        let action = fields[0].1;
        let mut all = vec![("form", token_of_member.as_str())];
        all.extend(fields);
        answers.push((action, post(api, &sheet, &member_session, &platform, &all)?.status));
    }
    let after = api.status(&owner, &chat)?;
    s.ok(
        "a member's every change is refused (403), their own good form token and all, and nothing changes",
        answers.iter().all(|(_, status)| *status == 403)
            && after.body["visibility"] == before.body["visibility"]
            && after.body["viewToken"] == before.body["viewToken"]
            && role_of(&agent)?.as_deref() == Some("editor")
            && role_of(&stranger_id)?.is_none()
            && invites_for(&stranger_name)?.is_empty()
            && invites_for(&waiting)?.len() == 1,
        format!("{answers:?}"),
    );

    // ---- a chat: made members only, and its own agent keeps its place on the sheet
    let talk = api.qualified(&owner, &s.name("stalk"))?;
    let r = api.create_with(&owner, json!({ "name": talk, "template": "chat" }))?;
    anyhow::ensure!(r.status == 200, "the owner's chat: {r}");
    let is_chat = s.eventually(wait, || api.status(&owner, &talk).is_ok_and(|r| r.body["kind"] == "chat"));
    let r = api.signed(&owner, "PUT", &format!("/api/f/{talk}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(is_chat && r.status == 200, "the owner's agent joins {talk}: {r}");
    let r = with_session(api, "GET", &format!("/share/{talk}"), &owner_session)?;
    let controls = r.text.matches(&format!("name=\"member\" value=\"{agent}\"")).count();
    s.ok(
        "a chat is made members only, and its sheet keeps its own agent's place: no role buttons, no Remove, saying why",
        r.status == 200 && r.text.contains("This chat's own agent") && controls == 0 && r.text.contains("value=\"members\" aria-pressed=\"true\""),
        &r,
    );
    // a person who accepts brings their agents into its roster, after its
    // own agent, its lead (decision 64): each takes part as itself
    let r = api.signed(&owner, "POST", &format!("/api/f/{talk}/invites"), Some(&json!({ "email": member_name, "role": "editor" })))?;
    let yes = api.signed(&member, "POST", &format!("/api/invites/{talk}/accept"), Some(&json!({})))?;
    let roster = api.signed(&member, "GET", &format!("/api/f/{talk}/members"), None)?;
    let agents: Vec<&str> = roster.body["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").filter_map(|m| m["principal"].as_str()).collect();
    let its_list = api.signed(&member_agent, "GET", "/api/fragments", None)?;
    let listed_there = its_list.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == talk.as_str()));
    let said = api.signed(&member_agent, "POST", &format!("/api/f/{talk}/channels/chat"), Some(&json!({ "id": "theirs-1", "body": { "text": "hello from the member's agent" } })))?;
    let sheet_now = with_session(api, "GET", &format!("/share/{talk}"), &owner_session)?;
    s.ok(
        "a member who accepts a chat's invite brings their agent into its roster after its own (the lead stays first), in the agent's own list; it speaks there as itself; the sheet shows the member once, \"with their agents\", never naming it",
        r.status == 200
            && yes.status == 200
            && agents == vec![agent.as_str(), theirs_id.as_str()]
            && listed_there
            && said.status == 200
            && sheet_now.text.contains("With their agents")
            && !sheet_now.text.contains(&theirs_id),
        format!("{yes} {roster} {said}"),
    );

    // ---- the owner sets who can open it, and makes a new link
    let form = armed(api, &sheet, &owner_session)?;
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "rotate")])?;
    let rotated = api.status(&owner, &chat)?;
    let r2 = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "visibility"), ("visibility", "members")])?;
    let now = api.status(&owner, &chat)?;
    s.ok(
        "the owner makes a new link (the sheet says the old one opens nothing now), and sets who can open it, from the sheet",
        r.status == 200
            && r.text.contains("A new share link")
            && rotated.body["viewToken"] != before.body["viewToken"]
            && r.text.contains(&format!("?view={}", rotated.body["viewToken"].as_str().unwrap_or("-")))
            && r2.status == 303
            && now.body["visibility"] == "members",
        format!("{r} {r2} {now}"),
    );
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "visibility"), ("visibility", "public")])?;
    let public = with_session(api, "GET", &sheet, &owner_session)?;
    s.ok(
        "a public fragment's Copy link is its address alone: no share link's token rides along",
        r.status == 303 && public.text.contains(&format!("data-copy=\"{}\"", api.site_url(&chat, ""))) && !public.text.contains("?view="),
        &public,
    );
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "visibility"), ("visibility", "link")])?;
    anyhow::ensure!(r.status == 303, "visibility back to link: {r}");

    // ---- leaving: a member's own (a viewer of the owner's, here)
    let r = post(api, &sheet, &owner_session, &platform, &[("form", &form), ("action", "leave")])?;
    s.ok("the owner cannot leave (400, the sheet saying why)", r.status == 400 && r.text.contains("flash error") && api.status(&owner, &chat)?.body["role"] == "owner", &r);
    let leaving = armed(api, &sheet, &member_session)?;
    let r = post(api, &sheet, &member_session, &platform, &[("form", &leaving), ("action", "leave")])?;
    s.ok(
        "a member leaves from the sheet: a page says so, its Done closes it, and they are in it no more",
        r.status == 200 && r.text.contains(&format!("You left “{title}”")) && r.text.contains("data-done") && role_of(&member_id)?.is_none(),
        &r,
    );
    let r = post(api, &sheet, &member_session, &platform, &[("form", &leaving), ("action", "leave")])?;
    s.ok("leaving again (the same form sent twice) changes nothing: the sheet is not theirs now (403)", r.status == 403 && r.text.contains("Not yours to share") && role_of(&member_id)?.is_none(), &r);
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": member_name, "role": "viewer" })))?;
    let yes = api.signed(&member, "POST", &format!("/api/invites/{chat}/accept"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200 && yes.status == 200 && role_of(&member_id)?.as_deref() == Some("viewer"), "{member_name} shared with again: {r} {yes}");

    // ---- a fragment's page cannot share: its code (its author's, or an
    // agent's) runs in the owner's browser, with the platform's session
    let Some(mut chrome) = s.browser()? else { return Ok(()) };
    chrome.set_cookie(&format!("{platform}/"), "fragment_session", &owner_session)?;
    let page = chrome.open(&api.site_url(&chat, "__signin?return=/"))?;
    anyhow::ensure!(chrome.until(&page, "document.title === 'Todo'", wait), "the owner's list did not open: {}", chrome.eval(&page, "location.href")?);
    let attempt = |chrome: &mut Browser, page: &Page, js: &str| chrome.eval(page, js).unwrap_or_else(|e| json!(e.to_string()));
    let read = attempt(
        &mut chrome,
        &page,
        &format!("fetch({:?}, {{ credentials: 'include' }}).then(r => r.text()).then(t => 'read ' + t.length, e => 'refused ' + e.name)", format!("{platform}{sheet}")),
    );
    let forged = format!("action=invite&email={}&role=editor&form={}", url_enc(&stranger_name), url_enc(&token_of_member));
    let posted = attempt(
        &mut chrome,
        &page,
        &format!(
            "fetch({:?}, {{ method: 'POST', mode: 'no-cors', credentials: 'include', headers: {{ 'content-type': 'application/x-www-form-urlencoded' }}, body: {forged:?} }}).then(() => 'sent', e => 'refused ' + e.name)",
            format!("{platform}{sheet}")
        ),
    );
    std::thread::sleep(Duration::from_millis(300));
    s.ok(
        "a fragment's page cannot read the sheet (no CORS: it never holds the form's token), and what it posts to it invites no one",
        read.as_str().is_some_and(|r| r.starts_with("refused")) && role_of(&stranger_id)?.is_none(),
        format!("{read} / {posted}"),
    );
    let members_api = format!("{platform}/api/f/{chat}/members");
    let listed = attempt(&mut chrome, &page, &format!("fetch({members_api:?}, {{ credentials: 'include' }}).then(r => r.text()).then(t => 'read ' + t, e => 'refused ' + e.name)"));
    let put = attempt(
        &mut chrome,
        &page,
        &format!(
            "fetch({:?}, {{ method: 'PUT', credentials: 'include', headers: {{ 'content-type': 'application/json' }}, body: '{{\"role\":\"editor\"}}' }}).then(r => 'status ' + r.status, e => 'refused ' + e.name)",
            format!("{members_api}/{stranger_id}")
        ),
    );
    let invited = attempt(
        &mut chrome,
        &page,
        &format!(
            "fetch({:?}, {{ method: 'POST', mode: 'no-cors', credentials: 'include', headers: {{ 'content-type': 'text/plain' }}, body: '{{\"role\":\"editor\",\"email\":\"{stranger_name}\"}}' }}).then(() => 'sent', e => 'refused ' + e.name)",
            format!("{platform}/api/f/{chat}/invites")
        ),
    );
    std::thread::sleep(Duration::from_millis(300));
    s.ok(
        "nor fetch the signed API: it cannot read who is in, and its calls change nothing",
        listed.as_str().is_some_and(|r| r.starts_with("refused")) && role_of(&stranger_id)?.is_none() && invites_for(&stranger_name)?.is_empty(),
        format!("{listed} / {put} / {invited}"),
    );
    let r = api.call(Call {
        method: "PUT",
        url: format!("{members_api}/{stranger_id}"),
        body: Some(br#"{"role":"editor"}"#.to_vec()),
        content_type: Some("application/json"),
        cookie: Some(format!("fragment_session={owner_session}")),
        extra: vec![("origin", api.site_origin(&chat))],
        ..Call::default()
    })?;
    s.ok("(the signed API takes no session: a call it sends with the cookie and no key's signature is 401)", r.status == 401 && role_of(&stranger_id)?.is_none(), &r);

    // framing the sheet: the owner's session rides into the frame, but the
    // sheet refuses to show there, so there is nothing to lay a click under
    chrome.eval(&page, &format!("(() => {{ const f = document.createElement('iframe'); f.src = {:?}; document.body.append(f); return true; }})()", format!("{platform}{sheet}")))?;
    let framed = |chrome: &mut Browser| chrome.eval_in_frame(&page, "/share/", "document.body?.innerText ?? ''").ok().and_then(|v| v.as_str().map(str::to_string));
    let shown = s.eventually(Duration::from_secs(5), || framed(&mut chrome).is_some_and(|t| t.contains("General access")));
    s.ok("a fragment's page that frames the sheet gets a frame without it", !shown, format!("{:?}", framed(&mut chrome)));

    // scripting the window it opens on the sheet (on a click of the
    // owner's, as a popup must be): the sheet severs it
    chrome.eval(
        &page,
        &format!(
            "(() => {{ const b = document.createElement('button'); b.id = 'taker'; b.textContent = 'x'; b.style.cssText = 'position:fixed;right:0;bottom:0;width:60px;height:40px;z-index:99'; b.onclick = () => {{ window.__sheet = window.open({:?}, 'sheet', 'popup,width=480,height=640'); }}; document.body.append(b); return true; }})()",
            format!("{platform}{sheet}")
        ),
    )?;
    chrome.click(&page, "#taker")?;
    let mut popup = None;
    let loaded = s.eventually(wait, || {
        popup = chrome.pages().ok().and_then(|p| p.into_iter().find(|(_, url)| url.ends_with(&sheet)));
        popup.is_some()
    });
    let severed = loaded && chrome.until(&page, "window.__sheet.closed === true", Duration::from_secs(5));
    chrome.eval(&page, "try { window.__sheet.location.href = location.origin + '/#taken'; } catch (e) {} true")?;
    std::thread::sleep(Duration::from_millis(500));
    let still = popup.as_ref().is_some_and(|(id, _)| chrome.pages().is_ok_and(|p| p.iter().any(|(t, url)| t == id && url.ends_with(&sheet))));
    s.ok(
        "a window it opens on the sheet is severed from it: its handle reads closed, and cannot navigate it away",
        severed && still,
        format!("{:?} {:?}", chrome.eval(&page, "window.__sheet.closed"), chrome.pages()?),
    );

    // ---- the shell, the one page that frames the sheet: an app's Share
    // opens it in a dialog, and Done closes it (at /settings, which the
    // shell opens whether or not the person has a chat yet; the launcher
    // finds the app there and opens its window on the desktop)
    let home = chrome.open(&format!("{platform}/settings"))?;
    chrome.viewport(&home, 1280, 800, false)?;
    let row = format!("#apps [data-key={:?}]", format!("app:{chat}"));
    let found = format!("#search-results .search-result[data-fragment={chat:?}]");
    let share = "#stack button[aria-label='Share…']";
    // A hand's click lands where the button is drawn: only once the page has
    // loaded, its fonts are in and its panels and windows have stopped
    // moving (the desktop springs open), or a layout still moving puts the
    // click elsewhere.
    let settled = |selector: &str| {
        format!(
            "document.readyState === 'complete' && document.fonts.status === 'loaded' && !!document.querySelector({selector:?}) \
             && document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)"
        )
    };
    // (the sidebar laid out open, as a desktop's is, and the app in the list)
    let mut listed = chrome.until(&home, &format!("{} && {} && document.getElementById('layout').classList.contains('left-open')", settled("#sidebar-new-chat"), settled(&row)), wait);
    if listed {
        chrome.click(&home, "#sidebar-new-chat")?;
        chrome.eval(&home, "(() => { const i = document.getElementById('workspace-search'); i.value = 'Todo'; i.dispatchEvent(new Event('input')); return true; })()")?;
        listed = chrome.until(&home, &settled(&found), wait);
    }
    if listed {
        chrome.click(&home, &found)?;
    }
    let windowed = listed && chrome.until(&home, &settled(share), wait);
    if windowed {
        chrome.click(&home, share)?;
    }
    let inside = |chrome: &mut Browser| chrome.eval_in_frame(&home, "/share/", "document.body?.innerText ?? ''").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let shown = windowed && chrome.until(&home, SHEET_SIZED, wait) && s.eventually(wait, || inside(&mut chrome).contains("General access"));
    chrome.screenshot(&home, &s.scratch.join("share-dialog.png"))?;
    let done = shown && chrome.eval_in_frame(&home, "/share/", "(document.querySelector('button[data-done]').click(), true)").is_ok();
    let closed = done && chrome.until(&home, &format!("!document.getElementById('sheet').open && !document.querySelector('#sheet iframe') && !!document.querySelector({row:?})"), wait);
    s.ok(
        "the shell opens an app's share sheet in a dialog (a frame on its own origin), and its Done closes it (the shell reads the list again)",
        shown && closed,
        format!("listed {listed} windowed {windowed} shown {shown} done {done}: {}", inside(&mut chrome)),
    );

    // ---- at a short window and a tall one: the dialog is as tall as the
    // sheet says, all of it in view when it fits and scrolling when it does
    // not, and its Done, clicked where it is drawn, closes it
    let (mut all, mut seen) = (true, vec![]);
    for (height, fits) in [(700, false), (1440, true)] {
        chrome.viewport(&home, 1280, height, false)?;
        // (two frames for the desktop to lay itself out at the new size)
        chrome.eval(&home, "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))")?;
        let ready = chrome.until(&home, &format!("innerHeight === {height} && {}", settled(share)), wait);
        if ready {
            chrome.click(&home, share)?;
        }
        let sized = ready && chrome.until(&home, SHEET_SIZED, wait);
        // the sheet's own height, and the frame and dialog the shell gave it
        let mut laid = Value::Null;
        let fitted = sized
            && s.eventually(wait, || {
                let own = chrome.eval_in_frame(&home, "/share/", "document.body.offsetHeight").ok().and_then(|v| v.as_f64());
                laid = chrome
                    .eval(
                        &home,
                        "(() => { const d = document.getElementById('sheet'), r = d.getBoundingClientRect(); \
                         return { frame: d.querySelector('iframe').getBoundingClientRect().height, dialog: r.height, inView: r.top >= 0 && r.bottom <= innerHeight, scrolls: d.scrollHeight > d.clientHeight }; })()",
                    )
                    .unwrap_or_default();
                laid["own"] = json!(own);
                own.is_some()
                    && laid["frame"].as_f64() == own
                    && laid["inView"] == json!(true)
                    && laid["scrolls"] == json!(!fits)
                    && (!fits || laid["dialog"].as_f64() == own)
            });
        chrome.screenshot(&home, &s.scratch.join(format!("share-dialog-{height}.png")))?;
        // Done where it is drawn: the dialog scrolled to it, and the point
        // the frame's place in the page plus the button's in the frame,
        // the button what is there
        chrome.eval(&home, "(() => { const d = document.getElementById('sheet'); d.scrollTop = d.scrollHeight; return true; })()")?;
        let frame = chrome.eval(&home, "(() => { const r = document.querySelector('#sheet iframe')?.getBoundingClientRect(); return r ? [r.left, r.top] : []; })()")?;
        let done = chrome
            .eval_in_frame(&home, "/share/", "(() => { const r = document.querySelector('button[data-done]').getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height / 2; return document.elementFromPoint(x, y)?.closest('button[data-done]') ? [x, y] : []; })()")
            .unwrap_or_default();
        let at = match (frame.as_array().map(Vec::as_slice), done.as_array().map(Vec::as_slice)) {
            (Some([fx, fy]), Some([x, y])) => fx.as_f64().zip(fy.as_f64()).zip(x.as_f64().zip(y.as_f64())).map(|((fx, fy), (x, y))| (fx + x, fy + y)),
            _ => None,
        };
        if let Some(at) = at {
            chrome.click_at(&home, at)?;
        }
        let closed = at.is_some() && chrome.until(&home, "!document.getElementById('sheet').open && !document.querySelector('#sheet iframe')", wait);
        seen.push(format!("{height}: sized {sized} {laid} Done at {at:?} closed {closed}"));
        all &= sized && fitted && closed;
        if !all {
            break;
        }
    }
    chrome.viewport(&home, 1280, 800, false)?;
    s.ok(
        "at a short window and a tall one the dialog is as tall as the sheet says, in view (scrolling only when the sheet does not fit), and its Done, clicked where it is drawn, closes it",
        all,
        seen.join("; "),
    );

    // ---- the Apps page: who has access to each of the owner's (GET
    // /api/access) at a glance; its Share… opens the sheet
    let r = with_session(api, "GET", "/api/access", &owner_session)?;
    s.ok("(the access view is the shell's API, not a page's: a session alone, without the shell's header, is 401)", r.status == 401, &r);
    chrome.eval(&home, "(document.getElementById('apps-link').click(), true)")?;
    let access = format!("#apps-page .app-row[data-fragment={chat:?}]");
    let drawn = chrome.until(&home, &format!("location.pathname === '/apps' && !!document.querySelector({access:?})?.querySelector('.app-row-access')?.textContent.includes('1 invited')"), wait);
    let text = chrome.eval(&home, &format!("document.querySelector({access:?})?.innerText ?? ''"))?;
    let text = text.as_str().unwrap_or_default();
    s.ok(
        "the Apps page lists the owner's app: its title, who can open it, who is in by email, and the invite waiting",
        drawn && text.contains(&title) && text.contains("Anyone with the link") && text.contains(&member_name) && text.contains("1 invited"),
        text,
    );
    chrome.click(&home, &format!("{access} button[data-action=sharing]"))?;
    let opened = chrome.until(&home, &format!("!!document.querySelector({access:?})?.querySelector('.app-sharing-frame')?.style.height"), wait) && s.eventually(wait, || inside(&mut chrome).contains("General access"));
    let _ = chrome.eval_in_frame(&home, "/share/", "(document.querySelector('button[data-done]').click(), true)");
    let closed = opened && chrome.until(&home, &format!("!document.querySelector({access:?})?.querySelector('.app-sharing-frame') && !!document.querySelector({access:?})"), wait);
    s.ok("Sharing expands inline, and Done closes it back to the page", opened && closed, inside(&mut chrome));
    chrome.close(home)?;

    // the sheet as the owner sees it, at a popup's size and a phone's, light
    // and dark (evidence for a person): no sideways scroll
    let shown = chrome.open(&format!("{platform}{sheet}"))?;
    let mut fits = vec![];
    for (width, mobile, scheme, shot) in [(480, false, "light", "share-sheet.png"), (390, true, "dark", "share-sheet-phone.png")] {
        chrome.viewport(&shown, width, 900, mobile)?;
        chrome.color_scheme(&shown, scheme)?;
        let ready = chrome.until(&shown, "document.readyState === 'complete' && !!document.querySelector('footer')", wait);
        fits.push((width, ready && chrome.eval(&shown, "document.documentElement.scrollWidth <= innerWidth")? == json!(true)));
        chrome.screenshot(&shown, &s.scratch.join(shot))?;
    }
    s.ok("the sheet fits a popup's width and a phone's, with no sideways scroll", fits.iter().all(|(_, fit)| *fit), format!("{fits:?}"));

    // ---- a keyboard: arrow keys over its choices send nothing (a closed
    // select's did, on Linux and Windows: an ArrowDown on a member's role
    // removed them, on General access made it public)
    chrome.viewport(&shown, 480, 900, false)?;
    chrome.reload(&shown)?;
    let armed_now = chrome.until(&shown, "!!document.querySelector('footer') && ![...document.querySelectorAll('[data-arm]')].some(b => b.disabled)", wait);
    let before = (api.status(&owner, &chat)?.body["visibility"].clone(), role_of(&member_id)?);
    for focus in [format!("form:has(input[value={member_id:?}]) button[value=editor]"), "button[value=public]".to_string()] {
        chrome.eval(&shown, &format!("(document.querySelector({focus:?}).focus(), true)"))?;
        for key in ["ArrowDown", "ArrowDown", "ArrowUp", "ArrowDown"] {
            chrome.press(&shown, key, false)?;
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let after = (api.status(&owner, &chat)?.body["visibility"].clone(), role_of(&member_id)?);
    s.ok("arrow keys over a member's role and over General access change nothing", armed_now && before == after && after.0 == "link", format!("{before:?} → {after:?}"));
    // Remove asks a second click: the first only says what it does
    let remove = format!("form:has(input[name=action][value=remove]):has(input[value={member_id:?}]) button");
    chrome.click(&shown, &remove)?;
    std::thread::sleep(Duration::from_millis(300));
    let asked = chrome.eval(&shown, &format!("document.querySelector({remove:?}).textContent"))?;
    s.ok("one click on Remove removes no one: it asks again", asked == "Remove?" && role_of(&member_id)?.as_deref() == Some("viewer"), &asked);
    chrome.click(&shown, &remove)?;
    let gone = s.eventually(wait, || role_of(&member_id).is_ok_and(|r| r.is_none()));
    s.ok("and the second removes them", gone, "");
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat}/invites"), Some(&json!({ "email": member_name, "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200 && !r.body["invited"].is_null(), "{member_name} invited again: {r}");
    chrome.close(shown)?;

    // ---- the member's shell: the invite waits in their sidebar, with
    // Accept and Decline (decision 63); their Accept opens it
    chrome.set_cookie(&format!("{platform}/"), "fragment_session", &member_session)?;
    let theirs = chrome.open(&format!("{platform}/settings"))?;
    chrome.viewport(&theirs, 1280, 800, false)?;
    let invite = format!("#invites .invite[data-invite={chat:?}]");
    let waits = chrome.until(&theirs, &format!("{} && !document.getElementById('invites').hidden && !document.querySelector({row:?})", settled(&format!("{invite} .invite-accept"))), wait);
    let said = chrome.eval(&theirs, &format!("document.querySelector({invite:?})?.innerText ?? ''"))?;
    chrome.screenshot(&theirs, &s.scratch.join("share-invite-sidebar.png"))?;
    s.ok(
        "their shell shows the invite in the sidebar, above their chats: its title, who invited them and as what, with Accept and Decline; it is in no list yet",
        waits && said.as_str().is_some_and(|t| t.contains(&title) && t.contains(&format!("{owner_name} invited you to view it")) && t.contains("Accept") && t.contains("Decline")),
        &said,
    );
    if waits {
        chrome.click(&theirs, &format!("{invite} .invite-accept"))?;
    }
    // (their Accept opens its window on the desktop)
    let more = "#stack button[aria-label='More']";
    let listed = waits && chrome.until(&theirs, &format!("{} && {} && !document.querySelector({invite:?})", settled(&row), settled(more)), wait);
    s.ok(
        "their Accept makes them a member, and opens it: it is in their dock, its window open, and the invite gone from the sidebar",
        listed && role_of(&member_id)?.as_deref() == Some("viewer") && invites_for(&member_name)?.is_empty(),
        chrome.eval(&theirs, "document.getElementById('sidebar').innerText")?,
    );
    // ---- someone else's app offers People…, its sheet read-only, with Leave
    let windowed = listed;
    if windowed {
        chrome.click(&theirs, more)?;
    }
    let offered = windowed && chrome.until(&theirs, "[...document.querySelectorAll('#menu button')].some(b => b.textContent === 'People…')", wait);
    let no_share = chrome.eval(&theirs, "!document.querySelector(\"#stack button[aria-label='Share…']\")")? == json!(true);
    if offered {
        chrome.eval(&theirs, "([...document.querySelectorAll('#menu button')].find(b => b.textContent === 'People…').click(), true)")?;
    }
    let inside = |chrome: &mut Browser| chrome.eval_in_frame(&theirs, "/share/", "document.body?.innerText ?? ''").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let shown = offered && chrome.until(&theirs, SHEET_SIZED, wait) && s.eventually(wait, || inside(&mut chrome).contains("Who has access to"));
    chrome.screenshot(&theirs, &s.scratch.join("share-people-member.png"))?;
    let text = inside(&mut chrome);
    s.ok(
        "a member's window of someone else's app offers People… (no Share…): the sheet, read-only, with Leave",
        no_share && shown && text.contains("Leave") && text.contains("Only its owner changes who has access"),
        format!("listed {listed} windowed {windowed} offered {offered} shown {shown}: {text}"),
    );
    Ok(())
}
