//! The slides template (templates/slides): a deck built with open-slide,
//! scaffolded and deployed with the CLI as an agent does (`fragment new
//! --template slides`, then `fragment deploy`), its committed build served
//! as the site, so no npm runs here. Valid: the deck opens at its own
//! address and at open-slide's (`/s/deck`, its presenter's view), its
//! hashed assets are served; an editor presents live on the `present`
//! channel and a second, anonymous page follows page by page, also in
//! open-slide's present mode, stops following and follows again; a note
//! left on a page reaches the other page live, and one typed while the
//! page moves keeps the page it was started on. Invalid: a reader's post to
//! `present` (editors and up present), and an anonymous one (sign in). With
//! `FRAGMENT_E2E_KEEP`, the run keeps the two pages' screenshots in
//! `slides-media/`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::{Api, Call};
use crate::browser::{Browser, Page};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(20);
const NOTE: &str = "Could this page say who follows?";
const KEPT: &str = "Started on page 4, sent from page 2.";

/// The bar's text on a page (empty before it is drawn).
fn bar(chrome: &mut Browser, page: &Page) -> String {
    chrome.eval(page, "document.querySelector('.sl-bar')?.innerText ?? ''").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Clicks the bar's button whose text starts with `label`.
fn press_button(chrome: &mut Browser, page: &Page, label: &str) -> Result<()> {
    let marked = chrome.eval(
        page,
        &format!("(() => {{ const b = [...document.querySelectorAll('.sl-bar button, .sl-panel button')].find((b) => b.innerText.startsWith({label:?})); if (!b) return false; b.setAttribute('data-e2e', 'press'); return true; }})()"),
    )?;
    anyhow::ensure!(marked == true, "no button {label:?}: {}", bar(chrome, page));
    chrome.click(page, "[data-e2e=press]")?;
    chrome.eval(page, "document.querySelector('[data-e2e=press]')?.removeAttribute('data-e2e')")?;
    Ok(())
}

/// Whether the page shows open-slide's page `p` (1 for the first).
fn at_page(p: u32) -> String {
    format!("new URLSearchParams(location.search).get('p') === '{p}' && location.pathname === '/s/deck'")
}

fn shot(chrome: &mut Browser, page: &Page, dir: &Path, name: &str) {
    let _ = chrome.screenshot(page, &dir.join(name));
}

pub fn slides(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("slides", &[crate::Need::Chrome]) {
        return Ok(());
    }
    let home = s.dir("slides-home");
    s.login(api, &home);
    let owner = s.cli_keys(&home).context("the CLI logged in")?;
    let made = s.cli_json(api, &home, &["create", &s.name("slides"), "--visibility", "public", "--json"])?;
    let name = made["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(!name.is_empty(), "no deck to test: {made}");
    let dir = s.dir("slides").join("deck");
    let dir_s = dir.to_str().expect("utf-8 path").to_string();
    let out = s.cli(api, &home, &["new", &dir_s, "--template", "slides"]);
    s.ok(
        "fragment new scaffolds the slides template: the deck, its pins, its build script and its built site",
        out.status.success()
            && dir.join("slides/deck/index.tsx").is_file()
            && dir.join("package.json").is_file()
            && dir.join("scripts/build.mjs").is_file()
            && dir.join("site/index.html").is_file()
            && dir.join("site/build.json").is_file(),
        String::from_utf8_lossy(&out.stderr),
    );
    let out = s.cli(api, &home, &["deploy", &name, "--dir", &dir_s]);
    s.ok("it deploys as it is", out.status.success(), String::from_utf8_lossy(&out.stderr));
    let st = api.status(&owner, &name)?;
    let ops: Vec<&str> = st.body["code"]["operations"].as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    s.ok("its code installs the notes' operations", st.body["code"]["error"].is_null() && ops == ["annotate", "notes", "remove_note", "resolve"], &st.body["code"]);

    // the deck at every address a site has for it, and its assets
    let root = api.page(&name, "", None)?;
    let deep = api.page(&name, "s/deck", None)?;
    let presenter = api.page(&name, "s/deck/presenter", None)?;
    s.ok(
        "the deck's page is served at /, at /s/deck and at its presenter's view, with the live layer",
        [&root, &deep, &presenter].iter().all(|r| r.status == 200 && r.text.contains("/slides-route.js") && r.text.contains("/slides-live.js")),
        format!("{root} / {deep} / {presenter}"),
    );
    let assets: Vec<String> = root.text.match_indices("/assets/").filter_map(|(at, _)| root.text[at + 1..].split('"').next().map(str::to_string)).collect();
    let mut served = vec![];
    for a in &assets {
        let r = api.call(Call { method: "GET", url: api.site_url(&name, a), ..Call::default() })?;
        served.push(json!({ "path": a, "status": r.status, "type": r.header("content-type"), "bytes": r.bytes.len() }));
    }
    s.ok(
        "the page's own hashed assets are served, the 1.4 MB app among them",
        assets.len() >= 3 && served.iter().all(|r| r["status"] == 200) && served.iter().any(|r| r["bytes"].as_u64().unwrap_or(0) > 1024 * 1024),
        json!(served),
    );

    // who presents: editors and up post to `present`
    let reader = api.person()?;
    let r = api.signed(&reader, "POST", &format!("/api/f/{name}/channels/present"), Some(&json!({ "id": "r1", "body": { "page": 2, "live": true, "from": "x" } })))?;
    let r2 = api.browser_op(&name, "channels/present", "a1", json!({ "page": 2, "live": true, "from": "x" }), None)?;
    s.ok("a reader's post to present is refused, and an anonymous visitor is asked to sign in", r.status == 403 && r2.status == 401, format!("{r} / {r2}"));
    let records = api.signed(&owner, "GET", &format!("/api/f/{name}/channels/present"), None)?;
    s.ok("and nothing is presented", records.status == 200 && records.body["records"].as_array().is_some_and(Vec::is_empty), &records);

    // Chrome: the owner presents, and an anonymous page follows
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the deck's pages (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let media = s.dir("slides-media");
    let session = api.sign_in(&crate::cli_email(&fragment_core::npub::encode(owner.pubkey_hex()))?)?;
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let url = api.site_url(&name, "");
    let a = chrome.open(&url)?;
    chrome.viewport(&a, 1280, 800, false)?;
    let other = chrome.another_context()?;
    let b = chrome.open_in(&other, &url)?;
    chrome.viewport(&b, 1280, 800, false)?;
    let drawn = "location.pathname === '/s/deck' && !!document.querySelector('.sl-bar') && document.body.innerText.includes('Your deck')";
    let opened = chrome.until(&a, drawn, WAIT) && chrome.until(&b, drawn, WAIT);
    s.ok("each page opens the deck at its own address, with the live bar", opened, json!({ "a": bar(&mut chrome, &a), "b": bar(&mut chrome, &b) }));

    // the owner's page signs in on the deck (a public deck's visitor is anonymous until then)
    chrome.eval(&a, "location.href = '/__signin?return=' + encodeURIComponent(location.pathname)")?;
    let consent = chrome.until(&a, "location.pathname === '/auth/fragment' && document.body.innerText.includes('Continue to')", WAIT);
    if consent && chrome.until(&a, "document.querySelector('button[data-arm]')?.disabled === false", Duration::from_secs(5)) {
        chrome.click(&a, "button[data-arm]")?;
    }
    let editor = chrome.until(&a, &format!("{drawn} && document.querySelector('.sl-bar').innerText.includes('Present live')"), WAIT);
    let reader_bar = bar(&mut chrome, &b);
    s.ok("signed in, the owner may present live; the anonymous page may not", editor && !reader_bar.contains("Present live"), json!({ "a": bar(&mut chrome, &a), "b": reader_bar }));

    press_button(&mut chrome, &a, "Present live")?;
    let live = chrome.until(&a, "document.querySelector('.sl-bar').innerText.includes('Live')", WAIT);
    let follows = chrome.until(&b, "document.querySelector('.sl-bar').innerText.includes('Following')", WAIT);
    s.ok("presenting live, the other page follows", live && follows, json!({ "a": bar(&mut chrome, &a), "b": bar(&mut chrome, &b) }));
    chrome.press(&a, "ArrowRight", false)?;
    let moved = chrome.until(&a, &at_page(2), WAIT);
    let followed = chrome.until(&b, &format!("{} && document.body.innerText.includes('Everyone with the link moves with you')", at_page(2)), WAIT);
    shot(&mut chrome, &a, &media, "1-presenter.png");
    shot(&mut chrome, &b, &media, "2-follower.png");
    s.ok("the presenter's next page is the follower's", moved && followed, json!({ "a": chrome.eval(&a, "location.href")?, "b": chrome.eval(&b, "location.href")? }));

    // in open-slide's present mode, the follower's player follows too
    chrome.eval(&b, "dispatchEvent(new KeyboardEvent('keydown', { key: 'p' }))")?;
    // open-slide's player: its own controls' layer (`data-osd-chrome`)
    let playing = chrome.until(&b, "!!document.querySelector('[data-osd-chrome]')", WAIT);
    chrome.press(&a, "ArrowRight", false)?;
    let in_player = chrome.until(&b, &format!("{} && document.body.innerText.includes('Your agent reads them')", at_page(3)), WAIT);
    shot(&mut chrome, &b, &media, "3-follower-present-mode.png");
    s.ok("a follower in present mode moves with the presenter", playing && in_player, json!({ "b": chrome.eval(&b, "location.href")? }));
    chrome.press(&b, "Escape", false)?;

    // a follower may stop following, and follow again
    press_button(&mut chrome, &b, "Stop following")?;
    chrome.press(&a, "ArrowRight", false)?;
    let a_on = chrome.until(&a, &at_page(4), WAIT);
    let stayed = !chrome.until(&b, &at_page(4), Duration::from_secs(3)) && chrome.until(&b, &at_page(3), WAIT);
    press_button(&mut chrome, &b, "Follow")?;
    let back = chrome.until(&b, &at_page(4), WAIT);
    s.ok("a follower stops following, stays put, and catches up when it follows again", a_on && stayed && back, json!({ "b": chrome.eval(&b, "location.href")?, "bar": bar(&mut chrome, &b) }));

    // a note on page 4, from the presenter's page, reaches the follower live
    press_button(&mut chrome, &a, "Notes")?;
    let writing = chrome.until(&a, "!!document.querySelector('.sl-panel textarea')", WAIT);
    chrome.click(&a, ".sl-panel textarea")?;
    chrome.type_text(&a, NOTE)?;
    chrome.click(&a, ".sl-panel button[type=submit]")?;
    let posted = writing && chrome.until(&a, &format!("document.querySelector('.sl-panel').innerText.includes({NOTE:?})"), WAIT);
    let stored = api.op(&owner, &name, "notes", "q", json!({}))?;
    let theirs: Vec<Value> = stored.body["result"]["notes"].as_array().cloned().unwrap_or_default();
    s.ok("the owner leaves a note on the page they are on", posted && theirs.iter().any(|n| n["anchor"] == "p4" && n["text"] == NOTE), json!(theirs));
    let counted = chrome.until(&b, "document.querySelector('.sl-bar').innerText.includes('Notes · 1')", WAIT);
    press_button(&mut chrome, &b, "Notes")?;
    let seen = chrome.until(&b, &format!("document.querySelector('.sl-panel').innerText.includes({NOTE:?}) && !!document.querySelector('.sl-panel a.sl-signin')"), WAIT);
    shot(&mut chrome, &a, &media, "4-presenter-note.png");
    shot(&mut chrome, &b, &media, "5-follower-sees-note.png");
    s.ok("the follower sees it live, and is asked to sign in to leave one", counted && seen, chrome.eval(&b, "document.querySelector('.sl-panel')?.innerText ?? ''")?);

    // the presenter stops: the follower no longer follows
    press_button(&mut chrome, &a, "Stop")?;
    let ended = chrome.until(&b, "!document.querySelector('.sl-bar').innerText.includes('Following')", WAIT);
    let last = api.signed(&owner, "GET", &format!("/api/f/{name}/channels/present"), None)?;
    let bodies: Vec<Value> = last.body["records"].as_array().map(|r| r.iter().map(|r| r["body"].clone()).collect()).unwrap_or_default();
    s.ok(
        "stopping ends it for everyone, and the channel holds each page presented",
        ended && bodies.last().is_some_and(|b| b["live"] == false) && [1, 2, 3, 4].iter().all(|p| bodies.iter().any(|b| b["page"] == *p && b["live"] == true)),
        json!(bodies),
    );

    // a note keeps the page it was started on: typed on page 4, sent after
    // the page moved to 2 (as following moves it), it is page 4's
    let typing = chrome.until(&a, "document.querySelector('.sl-panel textarea')?.dataset.anchor === 'p4'", WAIT);
    chrome.click(&a, ".sl-panel textarea")?;
    chrome.type_text(&a, KEPT)?;
    chrome.eval(&a, "(() => { const u = new URL(location.href); u.searchParams.set('p', '2'); history.replaceState(history.state, '', u); dispatchEvent(new PopStateEvent('popstate', { state: history.state })); })()")?;
    let moved = chrome.until(&a, &at_page(2), WAIT);
    chrome.click(&a, ".sl-panel button[type=submit]")?;
    let sent = chrome.until(&a, &format!("document.querySelector('.sl-panel').innerText.includes({KEPT:?}) || document.querySelector('.sl-panel').innerText.includes('Page 2')"), WAIT);
    let kept = api.op(&owner, &name, "notes", "q2", json!({}))?;
    let kept: Vec<Value> = kept.body["result"]["notes"].as_array().cloned().unwrap_or_default();
    s.ok(
        "a note typed on page 4 and sent after the page moved stays page 4's",
        typing && moved && sent && kept.iter().any(|n| n["text"] == KEPT && n["anchor"] == "p4") && !kept.iter().any(|n| n["text"] == KEPT && n["anchor"] != "p4"),
        json!(kept),
    );
    Ok(())
}
