//! "Only you" or "Shared" (FIN-615 PR 18): each of a person's fragments
//! says which, derived as their list is read (`Sharing::only_you`, the
//! list's `onlyYou`), never kept; and its link and
//! public stay shut for one reason the fragment gives (its status's
//! `linkShut`, reach.rs `link_shut`), which its visibility refuses with and
//! its share sheet shows.

use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use super::super::shell::shell;
use super::super::templates::person;
use crate::api::Api;
use crate::Suite;

/// Valid: the owner's app no one else may open reads "Only you" on their
/// list, and one shared by name or by link reads nothing there (so
/// "Shared"), nor does a member's row of someone else's; one
/// holding a key says why its link stays shut (`linkShut: reach`), and its
/// link is refused for that reason, while one without says nothing. In the
/// shell, the Apps page's row leads with "Only you" or "Shared" (then "Anyone
/// with the link" for a linked one), and each window's header says the same
/// word, with the globe only on the linked one; its link opened while both
/// are up, each says Shared at once (the list's change). Shot at a
/// desktop's width and a phone's, light and dark.
pub(super) fn only_you_label(s: &mut Suite, api: &Api) -> Result<()> {
    let wait = Duration::from_secs(20);
    let (owner, owner_session) = person(api)?;
    let (member, member_session) = person(api)?;
    let member_id = api.identity(&member)?;
    // members only (an app is made open to its link)
    let make = |label: &str, title: &str| -> Result<String> {
        let name = api.qualified(&owner, label)?;
        let r = api.create_with(&owner, json!({ "name": name, "template": "todo", "title": title, "visibility": "members" }))?;
        anyhow::ensure!(r.status == 200, "making {name}: {r}");
        Ok(name)
    };
    let alone = make(&s.name("alone"), "Notes")?;
    let named = make(&s.name("named"), "Recipes")?;
    let linked = make(&s.name("linked"), "Trips")?;
    let keyed = make(&s.name("keyed"), "Budget")?;
    let r = api.share(&named, &member_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "sharing {named}: {r}");
    let r = api.signed(&owner, "PUT", &format!("/api/f/{linked}/visibility"), Some(&json!({ "visibility": "link" })))?;
    anyhow::ensure!(r.status == 200, "{linked} by link: {r}");
    let key = json!({ "value": "sk-e2e-label-never-shown", "hosts": ["api.example.com"] });
    let r = api.signed(&owner, "PUT", &format!("/api/f/{keyed}/secrets/API_KEY"), Some(&key))?;
    anyhow::ensure!(r.status == 200, "a key on {keyed}: {r}");

    // the list's word, as it is read
    let rows = |session: &str, path: &str, key: &str| -> Vec<Value> {
        shell(api, session, "GET", path, None, &[]).ok().and_then(|r| r.body[key].as_array().cloned()).unwrap_or_default()
    };
    let word = |rows: &[Value], name: &str| -> Value { rows.iter().find(|f| f["name"] == name).map(|f| f["onlyYou"].clone()).unwrap_or(json!("missing")) };
    let settled = s.eventually(wait, || {
        let list = rows(&owner_session, "/api/fragments", "fragments");
        word(&list, &alone) == json!(true) && word(&list, &keyed) == json!(true) && word(&list, &named).is_null() && word(&list, &linked).is_null()
    });
    let list = rows(&owner_session, "/api/fragments", "fragments");
    let theirs = rows(&member_session, "/api/fragments", "fragments");
    let said = |rows: &[Value]| json!({ "alone": word(rows, &alone), "keyed": word(rows, &keyed), "named": word(rows, &named), "linked": word(rows, &linked) });
    s.ok(
        "the owner's list says Only you (`onlyYou`) of theirs no one else may open, a key or not, and nothing of one shared by name or by link; a member's row of someone else's says nothing",
        settled && word(&theirs, &named).is_null(),
        json!({ "list": said(&list), "member's": word(&theirs, &named) }),
    );

    // one reason its link stays shut, which its visibility refuses with
    let shut = |name: &str| api.status(&owner, name).ok().map(|r| r.body["linkShut"].clone()).unwrap_or(json!("unread"));
    let refused = api.signed(&owner, "PUT", &format!("/api/f/{keyed}/visibility"), Some(&json!({ "visibility": "link" })))?;
    s.ok(
        "one holding a key says why its link stays shut (`linkShut: reach`), and its link is refused for that reason (403 private_while_reaching); one without says nothing",
        shut(&keyed) == json!("reach") && refused.status == 403 && refused.body["reason"] == "private_while_reaching" && shut(&alone).is_null(),
        json!({ "keyed": shut(&keyed), "refused": refused.status, "reason": refused.body["reason"], "alone": shut(&alone) }),
    );

    // the shell: the Apps page's rows, and each window's header
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the share lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &owner_session)?;
    let page = chrome.open(&format!("{}/apps", api.base))?;
    chrome.viewport(&page, 1280, 860, false)?;
    let row = |name: &str| format!("document.querySelector({})", serde_json::to_string(&format!("#apps-page .app-row[data-fragment=\"{name}\"]")).unwrap_or_default());
    let line = |name: &str| format!("({}?.querySelector('.app-row-access')?.textContent ?? '')", row(name));
    let drawn = chrome.until(&page, &format!("location.pathname === '/apps' && {}.startsWith('Only you') && {}.startsWith('Shared') && {}.includes('Anyone with the link')", line(&alone), line(&named), line(&linked)), wait);
    let lines = chrome.eval(&page, &format!("[{}, {}, {}]", line(&alone), line(&named), line(&linked)))?;
    s.ok(
        "the Apps page's row leads with Only you or Shared, then what shares it (Anyone with the link)",
        drawn && lines[0] == json!("Only you") && lines[1].as_str().is_some_and(|t| t.starts_with("Shared ·")) && lines[2].as_str().is_some_and(|t| t.starts_with("Shared · Anyone with the link")),
        &lines,
    );
    let shots = s.dir("only-you");
    super::super::chat::shell_shots(&mut chrome, &page, &shots, "apps-rows", (1280, 860))?;
    // each window, opened from its row
    for name in [&alone, &named, &linked] {
        chrome.eval(&page, &format!("({}?.querySelector('.app-row-art')?.click(), true)", row(name)))?;
        chrome.until(&page, &format!("!!document.querySelector({})", serde_json::to_string(&format!("#stack .pane[data-key=\"app:{name}\"]"))?), wait);
    }
    let head = |name: &str| {
        let pane = serde_json::to_string(&format!("#stack .pane[data-key=\"app:{name}\"] .pane-sharing")).unwrap_or_default();
        format!("(() => {{ const s = document.querySelector({pane}); return s ? [s.textContent, !!s.querySelector('svg')] : null; }})()")
    };
    let heads = chrome.eval(&page, &format!("[{}, {}, {}]", head(&alone), head(&named), head(&linked)))?;
    s.ok(
        "each window's header says the same word, Only you or Shared, with the globe only while its link opens it",
        heads == json!([["Only you", false], ["Shared", false], ["Shared", true]]),
        &heads,
    );
    // the windows: at a desktop's width beside the page; at a phone's, its
    // desktop column shown (its toggle)
    for (width, height, phone) in [(1280, 860, false), (390, 844, true)] {
        chrome.viewport(&page, width, height, phone)?;
        if phone {
            chrome.eval(&page, "(document.getElementById('toggle-right')?.click(), true)")?;
        }
        for scheme in ["light", "dark"] {
            chrome.color_scheme(&page, scheme)?;
            std::thread::sleep(Duration::from_millis(400));
            chrome.screenshot(&page, &shots.join(format!("windows-{width}-{scheme}.png")))?;
        }
    }
    chrome.color_scheme(&page, "light")?;
    chrome.viewport(&page, 1280, 860, false)?;
    // its link opened while its window and its row are up: both say so at once
    let r = api.signed(&owner, "PUT", &format!("/api/f/{alone}/visibility"), Some(&json!({ "visibility": "link" })))?;
    let now = chrome.until(&page, &format!("JSON.stringify({}) === JSON.stringify(['Shared', true]) && {}.startsWith('Shared · Anyone with the link')", head(&alone), line(&alone)), wait);
    let seen = chrome.eval(&page, &format!("[{}, {}]", head(&alone), line(&alone)))?;
    s.ok(
        "its link opened while its window and its Apps row are up, each says Shared at once (the globe too): neither keeps an old word",
        r.status == 200 && now,
        json!({ "opened": r.status, "seen": seen }),
    );
    chrome.close(page)?;
    Ok(())
}
