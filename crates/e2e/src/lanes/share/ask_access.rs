//! Asking for access (FIN-502; docs/api.md, Asking for access): someone
//! signed in who opens a fragment they are not in is told so on its
//! origin, with Request access, the platform's page, where they ask its
//! owner. The owner sees the request among their notices, and is mailed;
//! sharing it is their own invite to the requester's email (which takes the
//! request away), and Dismiss takes it away alone, after which asking
//! again tells them nothing. The page's form is a sharing page's: armed
//! after a moment, the page's own token, from the platform's origin alone.
//! Its owner's identity never reaches the requester.

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

/// The page the requester opened, which their way in comes back to.
const ASKED: &str = "/?from=ask";

pub(super) fn requests(s: &mut Suite, api: &Api, owner: &Keys, owner_session: &str) -> Result<()> {
    let platform = api.base.clone();
    let name = api.qualified(owner, &s.name("askd"))?;
    let r = api.create_with(owner, json!({ "name": name, "template": "todo" }))?;
    anyhow::ensure!(r.status == 200, "making {name}: {r}");
    let title = api.status(owner, &name)?.body["title"].as_str().unwrap_or("").to_string();
    let owner_email = email_of(owner);
    let page = format!("/auth/fragment/ask?name={name}&return={}", url_enc(ASKED));
    // the requests the owner's notices show for this fragment
    let told = || -> Result<Vec<Value>> {
        let r = api.signed(owner, "GET", "/api/notices", None)?;
        anyhow::ensure!(r.status == 200, "the owner's notices: {r}");
        Ok(r.body["notices"].as_array().into_iter().flatten().filter(|n| n["kind"] == "access_asked" && n["id"].as_str().is_some_and(|id| id.starts_with(&format!("asked:{name}:")))).cloned().collect())
    };
    let mailed = |s: &Suite| s.mail.sent_to(&owner_email).into_iter().filter(|m| m.subject.contains("asked for access to")).count();

    // ---- a stranger who said yes to it is refused there, and offered Request access
    let (ann, ann_session) = person(api)?;
    let (ann_email, ann_id) = (email_of(&ann), api.identity(&ann)?);
    let cookie = site_cookie(api, &ann_session, &name)?;
    // a browser's top-level navigation, which a refusal answers as a page
    let navigation = vec![
        ("accept", "text/html".to_string()),
        ("sec-fetch-dest", "document".to_string()),
        ("sec-fetch-mode", "navigate".to_string()),
        ("sec-fetch-site", "none".to_string()),
    ];
    let r = api.call(Call { method: "GET", url: api.site_url(&name, ""), cookie: Some(format!("fragment_site={cookie}")), extra: navigation, ..Call::default() })?;
    s.ok(
        "a fragment refuses someone not in it (403) with Request access, a link to the platform's page for it",
        r.status == 403 && r.text.contains("You don't have access to") && r.text.contains(&format!("{platform}/auth/fragment/ask?name={name}&amp;return=")),
        &r,
    );
    let r = api.unsigned("GET", &page, None)?;
    s.ok(
        "that page signs someone signed out in first, and back to it",
        r.status == 302 && r.header("location").contains("/auth/login?return=") && r.header("location").contains(&url_enc(&format!("/auth/fragment/ask?name={name}"))),
        &r,
    );
    let r = with_session(api, "GET", &page, &ann_session)?;
    s.ok(
        "signed in, it says they have no access, that its owner sees their email, with Request access armed later; on the platform's own page (unframed, kept nowhere), naming no owner",
        r.status == 200
            && r.text.contains("You don't have access to")
            && r.text.contains(&ann_email)
            && r.text.contains("data-arm disabled>Request access")
            && !r.text.contains(&owner_email)
            && unframed(&r)
            && r.header("cache-control") == "no-store",
        &r,
    );
    let fresh = form_of(&r);
    let early = post(api, &page, &ann_session, &platform, &[("form", &fresh)])?;
    s.ok("a request sent before its button arms is refused (403), and tells no one", early.status == 403 && early.text.contains("too quick") && told()?.is_empty() && mailed(s) == 0, &early);
    std::thread::sleep(Duration::from_millis(form::DELAY_MS as u64 + 100));
    let from_page = post(api, &page, &ann_session, &api.site_origin(&name), &[("form", &fresh)])?;
    let consents = form::issue(&ann_session, &format!("consent:{name}"), now_ms() - form::DELAY_MS - 50);
    let other_purpose = post(api, &page, &ann_session, &platform, &[("form", &consents)])?;
    s.ok(
        "one from the fragment's own page is refused (403), as is one with another page's token (the consent page's); neither tells anyone",
        from_page.status == 403 && other_purpose.status == 403 && told()?.is_empty() && mailed(s) == 0,
        format!("{from_page} {other_purpose}"),
    );
    let sent = post(api, &page, &ann_session, &platform, &[("form", &fresh)])?;
    s.ok("their request is sent, and the page says so, by the email its owner sees", sent.status == 200 && sent.text.contains("Request sent") && sent.text.contains(&ann_email), &sent);
    let mail = s.mail.sent_to(&owner_email).into_iter().rev().find(|m| m.subject.contains("asked for access to"));
    s.ok(
        "its owner is mailed who asked for what, and the platform's page, where they answer it",
        mailed(s) == 1 && mail.as_ref().is_some_and(|m| m.subject == format!("{ann_email} asked for access to \u{201c}{title}\u{201d}") && m.text.contains(&format!(": {platform}/\n"))),
        mail.map(|m| format!("{} / {}", m.subject, m.text)).unwrap_or_default(),
    );
    let notices = told()?;
    let actions = |n: &Value| -> Vec<(String, String, Value)> {
        n["actions"].as_array().into_iter().flatten().map(|a| (a["label"].as_str().unwrap_or("").to_string(), a["post"].as_str().unwrap_or("").to_string(), a["body"].clone())).collect()
    };
    s.ok(
        "and sees it among their notices: its title, who asked, Share as viewer, Share as editor and Dismiss",
        notices.len() == 1
            && notices.first().is_some_and(|n| {
                n["id"] == format!("asked:{name}:{ann_id}").as_str()
                    && n["title"] == title.as_str()
                    && n["text"] == format!("{ann_email} asked for access to it.").as_str()
                    && actions(n).iter().map(|(l, _, _)| l.as_str()).eq(["Share as viewer", "Share as editor", "Dismiss"])
            }),
        json!(notices),
    );
    let again = post(api, &page, &ann_session, &platform, &[("form", &fresh)])?;
    s.ok(
        "asking again says they asked already, and tells its owner nothing more: no mail, one notice",
        again.status == 200 && again.text.contains("Asked already") && mailed(s) == 1 && told()?.len() == 1,
        &again,
    );

    // ---- the owner shares it: the notice's Share as editor is their own invite
    let (_, share_at, share_body) = notices.first().and_then(|n| actions(n).into_iter().nth(1)).context("the request's Share as editor")?;
    let shared = api.signed(owner, "POST", &share_at, Some(&share_body))?;
    s.ok(
        "its Share as editor is the owner's own invite to the requester's email, which waits for their yes, mailed to them; the request is gone from the owner's notices",
        shared.status == 200 && shared.body["invited"]["email"] == ann_email.as_str() && shared.body["invited"]["role"] == "editor" && !s.mail.sent_to(&ann_email).is_empty() && told()?.is_empty(),
        &shared,
    );
    let r = with_session(api, "GET", &page, &ann_session)?;
    s.ok("their request page, an invite waiting, sends them to it (the fragment's sign-in)", r.status == 302 && r.header("location").starts_with(&format!("/auth/fragment?name={name}&return=")), &r);
    let yes = api.signed(&ann, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    anyhow::ensure!(yes.status == 200, "{ann_email} accepts: {yes}");
    let r = with_session(api, "GET", &page, &ann_session)?;
    s.ok("in it now, the page says so, with the way in", r.status == 200 && r.text.contains("You're in") && r.text.contains(&format!("/auth/fragment?name={name}&amp;return=")), &r);
    let r = with_session(api, "GET", &page, owner_session)?;
    s.ok("its owner opening it is told they are in it", r.status == 200 && r.text.contains("You're in"), &r);

    // ---- dismissed: only by its owner, and asked again it tells them nothing
    let (bea, bea_session) = person(api)?;
    let bea_id = api.identity(&bea)?;
    let form = armed(api, &page, &bea_session)?;
    let sent = post(api, &page, &bea_session, &platform, &[("form", &form)])?;
    anyhow::ensure!(sent.status == 200 && sent.text.contains("Request sent"), "{bea_id} asks: {sent}");
    let mails = mailed(s);
    let dismiss = format!("/api/access-requests/{name}/dismiss");
    let theirs = api.signed(&bea, "POST", &dismiss, Some(&json!({ "requester": bea_id })))?;
    let unnamed = api.signed(owner, "POST", &dismiss, Some(&json!({ "requester": "bea" })))?;
    let more = api.signed(owner, "POST", &dismiss, Some(&json!({ "requester": bea_id, "role": "editor" })))?;
    s.ok(
        "only its owner dismisses it: the requester's own Dismiss is 404 (none waits on them), a body naming no identity, or more, 400; it is still told",
        theirs.status == 404 && theirs.body["reason"] == "gone" && unnamed.status == 400 && more.status == 400 && told()?.len() == 1,
        format!("{theirs} {unnamed} {more}"),
    );
    let (_, dismiss_at, dismiss_body) = told()?.first().and_then(|n| actions(n).into_iter().nth(2)).context("Dismiss")?;
    let dismissed = api.signed(owner, "POST", &dismiss_at, Some(&dismiss_body))?;
    let replayed = api.signed(owner, "POST", &dismiss_at, Some(&dismiss_body))?;
    s.ok(
        "its owner's Dismiss (the notice's) takes it from their notices; again, it is 404, gone",
        dismiss_at == dismiss && dismissed.status == 200 && told()?.is_empty() && replayed.status == 404 && replayed.body["reason"] == "gone",
        format!("{dismissed} {replayed}"),
    );
    let again = post(api, &page, &bea_session, &platform, &[("form", &form)])?;
    s.ok(
        "asked again a while after its owner dismissed it, it tells them nothing (no notice, no mail), and the page says asked already, never that it was dismissed",
        again.status == 200 && again.text.contains("Asked already") && !again.text.contains("dismiss") && told()?.is_empty() && mailed(s) == mails,
        &again,
    );

    // ---- a fragment that ends takes its requests with it
    let ending = api.qualified(owner, &s.name("asknd"))?;
    let r = api.create_with(owner, json!({ "name": ending, "template": "todo" }))?;
    anyhow::ensure!(r.status == 200, "making {ending}: {r}");
    let (_, cy_session) = person(api)?;
    let at = format!("/auth/fragment/ask?name={ending}&return=/");
    let form = armed(api, &at, &cy_session)?;
    let sent = post(api, &at, &cy_session, &platform, &[("form", &form)])?;
    let asked_there = |api: &Api| -> Result<bool> {
        let r = api.signed(owner, "GET", "/api/notices", None)?;
        Ok(r.body["notices"].as_array().is_some_and(|l| l.iter().any(|n| n["id"].as_str().is_some_and(|id| id.starts_with(&format!("asked:{ending}:"))))))
    };
    let before = asked_there(api)?;
    let r = api.signed(owner, "DELETE", &format!("/api/f/{ending}"), None)?;
    s.ok(
        "a fragment deleted takes its requests from its owner's notices",
        sent.status == 200 && before && r.status == 200 && s.eventually(Duration::from_secs(20), || asked_there(api).is_ok_and(|a| !a)),
        format!("{sent} {r}"),
    );
    in_chrome(s, api, owner_session, &name, &page)
}

/// In Chrome, as people see it: the request page and its answer, at a
/// desktop's width and a phone's, light and dark, and the owner's notice
/// in their shell (kept in the run's scratch folder).
fn in_chrome(s: &mut Suite, api: &Api, owner_session: &str, name: &str, page: &str) -> Result<()> {
    let wait = Duration::from_secs(20);
    let (_dee, dee_session) = person(api)?;
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the share lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &dee_session)?;
    let tab = chrome.open(&format!("{}{page}", api.base))?;
    let armed = chrome.until(&tab, "!!document.querySelector('form button') && !document.querySelector('form button').disabled", wait);
    shots(s, &mut chrome, &tab, "ask-access", wait)?;
    chrome.eval(&tab, "(document.querySelector('form button').click(), true)")?;
    let sent = chrome.until(&tab, "document.body.innerText.includes('Request sent')", wait);
    shots(s, &mut chrome, &tab, "ask-access-sent", wait)?;
    drop(chrome);
    // the owner's shell, in a browser of its own: the request among their notices
    let Some(mut chrome) = s.browser()? else { return Ok(()) };
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", owner_session)?;
    let shell = chrome.open(&format!("{}/", api.base))?;
    let told = chrome.until(&shell, "document.body.innerText.includes('asked for access to it.') && document.body.innerText.includes('Share as viewer')", wait);
    for (width, height, mobile) in [(1280, 800, false), (390, 844, true)] {
        chrome.viewport(&shell, width, height, mobile)?;
        std::thread::sleep(Duration::from_millis(300));
        chrome.screenshot(&shell, &s.scratch.join(format!("ask-access-owner-{width}.png")))?;
    }
    s.ok(
        "in Chrome: the request page arms its Request access, a click on it says the request is sent, and the owner's shell shows it among their notices, with its answers",
        armed && sent && told,
        format!("{name}: armed {armed} sent {sent} told {told}"),
    );
    Ok(())
}

/// The page in `tab` at 1280 and 390 px, light and dark, each once the
/// scheme's landscape (platform-pages.css) is read and painted.
fn shots(s: &mut Suite, chrome: &mut crate::browser::Browser, tab: &crate::browser::Page, stem: &str, wait: Duration) -> Result<()> {
    for (width, height, mobile) in [(1280, 800, false), (390, 844, true)] {
        chrome.viewport(tab, width, height, mobile)?;
        for (scheme, landscape) in [("light", "platform-landscape.jpg"), ("dark", "platform-landscape-dark.jpg")] {
            chrome.color_scheme(tab, scheme)?;
            chrome.until(tab, &format!("performance.getEntriesByType('resource').some(e => e.name.includes({landscape:?}))"), wait);
            std::thread::sleep(Duration::from_millis(300));
            chrome.screenshot(tab, &s.scratch.join(format!("{stem}-{width}-{scheme}.png")))?;
        }
    }
    chrome.viewport(tab, 1280, 800, false)?;
    chrome.color_scheme(tab, "light")?;
    Ok(())
}
