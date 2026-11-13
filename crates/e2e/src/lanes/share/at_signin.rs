//! An invite asked first where a fragment's sign-in begins (FIN-502;
//! docs/api.md, `/auth/fragment`): someone invited to a fragment who opens
//! it, or the link their invite mailed, is shown the invite on the
//! platform's page, whatever they said yes to before, and their yes there
//! makes them a member and signs them in on to the page they asked for.
//! Their no declines it. The page's forms are a sharing page's: armed after
//! a moment, the page's own token, from the platform's origin alone.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_core::form;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::super::signin::{site_cookie, unframed, with_session};
use super::super::templates::{email_of, person};
use super::{armed, form_of, now_ms, post};
use crate::api::{url_enc, Api, Call};
use crate::Suite;

/// The page the person asked for, which their sign-in comes back to.
const ASKED: &str = "/?from=invite";

pub(super) fn invite_asked_first(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let platform = api.base.clone();
    let name = api.qualified(owner, &s.name("asked"))?;
    let r = api.create_with(owner, json!({ "name": name, "template": "todo" }))?;
    anyhow::ensure!(r.status == 200, "making {name}: {r}");
    let title = api.status(owner, &name)?.body["title"].as_str().unwrap_or("").to_string();
    let invite = |email: &str, role: &str| -> Result<()> {
        let r = api.signed(owner, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": email, "role": role })))?;
        anyhow::ensure!(r.status == 200, "inviting {email} to {name}: {r}");
        Ok(())
    };
    let waits = |email: &str| -> Result<bool> {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/invites"), None)?;
        Ok(r.body["invites"].as_array().is_some_and(|a| a.iter().any(|i| i["email"] == email)))
    };
    let role_of = |keys: &Keys| -> Result<Option<String>> {
        let id = api.identity(keys)?;
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/members"), None)?;
        Ok(r.body["members"].as_array().into_iter().flatten().find(|m| m["principal"] == id.as_str()).and_then(|m| m["role"].as_str().map(str::to_string)))
    };
    let signin = format!("/auth/fragment?name={name}&return={}", url_enc(ASKED));
    let answer = format!("/auth/fragment/invite?name={name}&return={}", url_enc(ASKED));

    // ---- someone who said yes to it before, a stranger there, is invited
    let (ann, ann_session) = person(api)?;
    let ann_email = email_of(&ann);
    let stranger = site_cookie(api, &ann_session, &name)?;
    let r = api.page(&name, "", Some(&format!("fragment_site={stranger}")))?;
    anyhow::ensure!(r.status == 403, "{name} refuses a stranger who said yes to it: {r}");
    invite(&ann_email, "editor")?;
    let mailed = s.mail.sent_to(&ann_email);
    s.ok(
        "their invite's mail links to the fragment's sign-in on the platform's page (no fragment's address, nothing secret)",
        mailed.last().is_some_and(|m| m.text.contains(&format!("Accept it: {platform}/auth/fragment?name={name}\n")) && !m.text.contains(&api.site_url(&name, ""))),
        mailed.last().map(|m| m.text.clone()).unwrap_or_default(),
    );
    let r = api.unsigned("GET", &format!("/auth/fragment?name={name}"), None)?;
    s.ok(
        "that link, signed out, signs them in first, and back to it",
        r.status == 302 && r.header("location").contains("/auth/login?return=") && r.header("location").contains(&url_enc(&format!("/auth/fragment?name={name}"))),
        &r,
    );
    let r = with_session(api, "GET", &signin, &ann_session)?;
    s.ok(
        "signed in, its sign-in shows them the invite first, before the yes they gave it before: who shared what, to edit, with Decline and Accept armed later, on the platform's own page (unframed, kept nowhere)",
        r.status == 200
            && r.text.contains(&format!("<b>{}</b> invited you to edit <b>{title}</b>", email_of(owner)))
            && r.text.contains(&format!("action=\"/auth/fragment/invite?name={name}&amp;return={}\"", url_enc(ASKED)))
            && r.text.contains("value=\"decline\" data-arm disabled")
            && r.text.contains("value=\"accept\" data-arm disabled")
            && r.text.contains(&ann_email)
            && unframed(&r)
            && r.header("cache-control") == "no-store"
            && role_of(&ann)?.is_none(),
        &r,
    );
    let fresh = form_of(&r);
    let early = post(api, &answer, &ann_session, &platform, &[("form", &fresh), ("answer", "accept")])?;
    s.ok(
        "a yes sent before its buttons arm is refused (403): the click that opened it cannot answer it",
        early.status == 403 && early.text.contains("too quick") && role_of(&ann)?.is_none() && waits(&ann_email)?,
        &early,
    );
    std::thread::sleep(Duration::from_millis(form::DELAY_MS as u64 + 100));
    let from_page = post(api, &answer, &ann_session, &api.site_origin(&name), &[("form", &fresh), ("answer", "accept")])?;
    let consents = form::issue(&ann_session, &format!("consent:{name}"), now_ms() - form::DELAY_MS - 50);
    let other_purpose = post(api, &answer, &ann_session, &platform, &[("form", &consents), ("answer", "accept")])?;
    let neither = post(api, &answer, &ann_session, &platform, &[("form", &fresh), ("answer", "maybe")])?;
    s.ok(
        "a yes from the fragment's own page is refused (403), as is one with another page's token (the consent page's); an answer that is neither is 400; none makes them a member",
        from_page.status == 403 && other_purpose.status == 403 && neither.status == 400 && role_of(&ann)?.is_none() && waits(&ann_email)?,
        format!("{from_page} {other_purpose} {neither}"),
    );
    let yes = post(api, &answer, &ann_session, &platform, &[("form", &fresh), ("answer", "accept")])?;
    let to = yes.header("location");
    s.ok(
        "their yes makes them a member at its role, the invite spent, and goes on to the fragment's sign-in (303)",
        yes.status == 303 && to.starts_with(&api.site_url(&name, "__signin?token=")) && role_of(&ann)?.as_deref() == Some("editor") && !waits(&ann_email)?,
        &yes,
    );
    let signed = api.call(Call { method: "GET", url: to, ..Call::default() })?;
    let cookie = signed.cookies().into_iter().find(|c| c.starts_with("fragment_site=")).context("a site cookie")?;
    let page = api.page(&name, ASKED.trim_start_matches('/'), Some(&cookie))?;
    s.ok(
        "which signs them in there, back to the page they asked for, which opens for them now",
        signed.status == 302 && signed.header("location").ends_with(ASKED) && page.status == 200,
        format!("{signed} {page}"),
    );
    let again = with_session(api, "GET", &signin, &ann_session)?;
    s.ok("their next sign-in there asks nothing: a member's, straight on", again.status == 302 && again.header("location").contains("__signin?token="), &again);

    // ---- someone new to it declines
    let (bea, bea_session) = person(api)?;
    let bea_email = email_of(&bea);
    invite(&bea_email, "viewer")?;
    let r = with_session(api, "GET", &signin, &bea_session)?;
    s.ok(
        "someone invited who never opened it is shown the invite too, to view it, never asked to continue as a stranger",
        r.status == 200 && r.text.contains("invited you to view") && !r.text.contains("Continue as"),
        &r,
    );
    let form = armed(api, &signin, &bea_session)?;
    let no = post(api, &answer, &bea_session, &platform, &[("form", &form), ("answer", "decline")])?;
    s.ok(
        "their no declines it, then the platform's page (303 to /): no member, nothing waiting",
        no.status == 303 && no.header("location") == "/" && role_of(&bea)?.is_none() && !waits(&bea_email)?,
        &no,
    );
    let after = with_session(api, "GET", &signin, &bea_session)?;
    s.ok(
        "and its sign-in asks first as on any fragment not shared with them",
        after.status == 200 && after.text.contains("Continue as") && !after.text.contains("invited you"),
        &after,
    );

    // ---- an invite taken back while its page is open
    let (cy, cy_session) = person(api)?;
    let cy_email = email_of(&cy);
    invite(&cy_email, "viewer")?;
    let form = armed(api, &signin, &cy_session)?;
    let r = api.signed(owner, "DELETE", &format!("/api/f/{name}/invites/{cy_email}"), None)?;
    anyhow::ensure!(r.status == 200, "taking back {cy_email}'s invite: {r}");
    let late = post(api, &answer, &cy_session, &platform, &[("form", &form), ("answer", "accept")])?;
    s.ok(
        "a yes to an invite taken back meanwhile is a page saying it is gone (404), with the way on to the fragment's sign-in; no member",
        late.status == 404 && late.text.contains("This invite is gone") && late.text.contains(&format!("href=\"{}\"", signin.replace('&', "&amp;"))) && role_of(&cy)?.is_none(),
        &late,
    );
    let listed: Value = api.signed(&cy, "GET", "/api/invites", None)?.body;
    anyhow::ensure!(listed["invites"] == json!([]), "nothing waits on {cy_email} now: {listed}");
    in_chrome(s, api, owner, &name, &signin, &invite)
}

/// In Chrome, as a person sees it: the invite page, at a desktop's width
/// and a phone's, light and dark (kept in the run's scratch folder), and a
/// click on its armed Accept that opens the fragment as a member.
fn in_chrome(s: &mut Suite, api: &Api, owner: &Keys, name: &str, signin: &str, invite: &dyn Fn(&str, &str) -> Result<()>) -> Result<()> {
    let wait = Duration::from_secs(20);
    let (dee, dee_session) = person(api)?;
    invite(&email_of(&dee), "editor")?;
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the share lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &dee_session)?;
    let page = chrome.open(&format!("{}{signin}", api.base))?;
    let armed = chrome.until(&page, "!!document.querySelector('button[value=accept]') && !document.querySelector('button[value=accept]').disabled", wait);
    for (width, height, mobile) in [(1280, 800, false), (390, 844, true)] {
        chrome.viewport(&page, width, height, mobile)?;
        for (scheme, landscape) in [("light", "platform-landscape.jpg"), ("dark", "platform-landscape-dark.jpg")] {
            chrome.color_scheme(&page, scheme)?;
            // the scheme's landscape (platform-pages.css) read, then painted
            chrome.until(&page, &format!("performance.getEntriesByType('resource').some(e => e.name.includes({landscape:?}))"), wait);
            std::thread::sleep(Duration::from_millis(300));
            chrome.screenshot(&page, &s.scratch.join(format!("invite-at-signin-{width}-{scheme}.png")))?;
        }
    }
    chrome.viewport(&page, 1280, 800, false)?;
    chrome.color_scheme(&page, "light")?;
    chrome.eval(&page, "(document.querySelector('button[value=accept]').click(), true)")?;
    let landed = chrome.until(&page, &format!("location.host.startsWith({name:?}) && location.search === '?from=invite' && document.title === 'Todo'"), wait);
    chrome.screenshot(&page, &s.scratch.join("invite-at-signin-accepted.png"))?;
    let id = api.identity(&dee)?;
    let members = api.signed(owner, "GET", &format!("/api/f/{name}/members"), None)?;
    let role = members.body["members"].as_array().into_iter().flatten().find(|m| m["principal"] == id.as_str()).and_then(|m| m["role"].as_str().map(str::to_string));
    s.ok(
        "in Chrome: the invite page arms its Accept, and a click on it opens the fragment at the page asked for, signed in as a member at the invite's role",
        armed && landed && role.as_deref() == Some("editor"),
        chrome.eval(&page, "location.href + ' ' + document.title").unwrap_or_default(),
    );
    Ok(())
}
