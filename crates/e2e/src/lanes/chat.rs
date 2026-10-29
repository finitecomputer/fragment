//! The blessed chat template (`templates/chat`, docs/chat-records.md) in a
//! browser, against the stub image's scripted agent (`images/bridge`'s
//! `script` runtime) on a computer under `wrangler dev` with Docker.
//!
//! A person's computer runs an agent fragment that is in a chat whose files
//! are the template's. The page's own routes answer as the API's rules
//! say: an agent's name (`__people`), the members (`__members`), and an
//! editor's upload (`PUT __blob/<sha256>`). Then the owner, signed in on
//! the chat's page, types and presses Enter, and sees the agent's draft
//! live, then its reply; a tool step as a card, saying what it does by
//! its icon, which its verb names; the runtime's notices as quiet lines of
//! the turn, the one after its end too; a question asked to be answered in
//! words, the page waiting for their answer until they give it; an
//! approval card they
//! answer with its button, which another member sees but may not press;
//! a card of choices whose last is answered in words, typed on the card
//! and sent with Enter;
//! Stop, which ends a slow turn; a picture they attach, in their message
//! and as the chat's blob; a reply's file; the mic's card before the
//! browser asks, and the one a refused microphone shows; a voice memo
//! recorded from Chrome's fake microphone, sent as an audio attachment and
//! shown as a player, which the agent receives; the mic in the shell's
//! frame, which names the chat's origin. At a phone's width nothing scrolls
//! sideways. Screenshots (desktop light and dark, phone) stay in the run's
//! scratch (`chat/`).
//!
//! The chat runs the template's code from the platform's release
//! (decision 40), which its own repo cannot override, and that code's push
//! (docs/chat-records.md, Push): every agent reply starts its job, no
//! person's record does; while the owner's page is on screen nothing is
//! pushed to them, and once it is closed a reply reaches each of the
//! chat's people's subscriptions once (the push fake decrypts it), never
//! a tag that names no one; no one subscribes for another identity. In
//! the shell's frame its bell turns on alerts the shell holds, which a
//! reply reaches, and turns them off.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::computers::{agent_replies, turn_of, work_of, AGENT_JSON};
use super::jobs::{records, settle, started};
use super::signin::site_cookie;
use crate::api::{Api, Call, Reply, Socket};
use fragment_nip98::Keys;
use crate::browser::{Browser, Page};
use crate::Suite;

#[path = "chat_polish.rs"]
mod polish;

/// A start of the stub and its bridge's first follow (as the computers lane's).
const WAKE: Duration = Duration::from_secs(90);
/// A turn of the scripted agent, once awake, as a page sees it.
const TURN: Duration = Duration::from_secs(30);

/// `cond`, as a page's JavaScript, true within `TURN`.
fn shows(chrome: &mut Browser, page: &Page, cond: &str) -> bool {
    chrome.until(page, cond, TURN)
}

/// A chat page whose first live socket is never answered (its upgrade held,
/// as a dropped connection or a phone's changing network leaves one: the
/// library dialled again only on a close, so the page waited for its hello
/// for good). Valid: the page closes it once it has waited past any healthy
/// open (`OPEN_WAIT_MS`, cell/client.mjs), dials again, has its hello, and
/// its composer is ready, its chat comes (the owner's newest message
/// shown), and every message of the owner's is shown once.
fn held_socket(s: &mut Suite, api: &Api, chrome: &mut Browser, owner: &Keys, chat: &str) -> Result<()> {
    // the open wait and its first backoff, jittered, and the new open
    let wait = Duration::from_secs(30);
    // The chat comes after the hello: the new socket's subscribe, from the
    // channel's cursor, then its page of records, drawn. Its newest message
    // of the owner's shown, that page is (a look as the hello came found
    // none drawn yet: runs 39667027692, 39679966640 and 39682770211).
    let me = api.identity(owner)?;
    let newest = records(api, owner, chat, "chat")
        .into_iter()
        .filter(|r| r["principal"] == me.as_str() && r["body"]["text"].is_string() && r["body"]["kind"].is_null())
        .filter_map(|r| r["seq"].as_i64())
        .max()
        .context("the owner has said something in the chat")?;
    let page = chrome.open_with_script(&api.site_url(chat, "__signin?return=/"), &crate::browser::hold_first_socket("/__live"))?;
    fit(chrome, &page, 1280, 860, false)?;
    let ready = chrome.until(
        &page,
        &format!("window.__heldSockets === 1 && window.__heldClosed === 1 && document.getElementById('say')?.dataset.ready === '1' && !!document.querySelector('.msg.user.mine[data-seq=\"{newest}\"]')"),
        wait,
    );
    let seen = chrome.eval(&page, "({ held: window.__heldSockets, closed: window.__heldClosed, ready: document.getElementById('say')?.dataset.ready ?? null, mine: [...document.querySelectorAll('.msg.user.mine[data-seq]')].map((m) => m.dataset.seq) })")?;
    let mine = strings_of(&seen["mine"]);
    let once = !mine.is_empty() && mine.iter().collect::<std::collections::BTreeSet<_>>().len() == mine.len();
    s.ok(
        "a page whose first live socket is never answered closes it once it has waited past any healthy open, dials again, and is ready, each of the owner's messages shown once",
        ready && once,
        &seen,
    );
    chrome.close(page)?;
    Ok(())
}

/// The chat's page whose script does not come, in the shell's frame (the
/// frame was blank, with nothing to press): after a while it says it is
/// still loading, with a reload of its own page, in its frame. Valid: a
/// chat that loads never says it (its script draws the page, and the line
/// goes with what was there). Its `chat.js` refused (the frame's own
/// requests) and the frame loaded again: nothing for a while, then "Still
/// loading… Reload", the link to the frame's own page; its Reload, the
/// script let through, loads the chat again in its frame, and the shell
/// around it stays as it was (not reloaded).
fn still_loading_in_frame(s: &mut Suite, api: &Api, chrome: &mut Browser, chat: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let line = "document.getElementById('still-loading')";
    let ready = format!("document.getElementById('say')?.dataset.ready === '1' && !{line}");
    let shell = chrome.open(&format!("{}/", api.base))?;
    chrome.viewport(&shell, 1280, 860, false)?;
    let row = format!("#chats [data-key={}]", js(&format!("chat:{chat}")));
    let listed = chrome.until(&shell, &format!("!document.getElementById('layout').hidden && !!document.querySelector({})", js(&row)), wait);
    chrome.eval(&shell, &format!("(document.querySelector({})?.click(), window.__shellStays = true, true)", js(&row)))?;
    let framed = s.eventually(wait, || chrome.eval_in_frame(&shell, chat, &ready).ok() == Some(Value::Bool(true)));
    s.ok("the chat in the shell's frame loads, and never says it is still loading (its script drew the page, the line gone with what was there)", listed && framed, "");

    let frame = chrome.frame(chat)?;
    // A page navigation during release skew must never strand this pane
    // on raw JSON. Refuse the frame session lookup, then let it recover.
    if !api.signs_in_by_levers() {
        let skew = |times: u64| -> Result<()> {
            let r = api.unsigned("POST", "/api/test/registry", Some(&json!({ "skewFor": { "path": "/session", "times": times } })))?;
            anyhow::ensure!(r.status == 200, "setting release skew: {r}");
            Ok(())
        };
        skew(100)?;
        chrome.eval(&frame, "(location.reload(), true)")?;
        let updating = chrome.until(&frame, "document.body?.innerText.trim() === 'Updating — back in a moment'", wait);
        let styled = chrome.until(&frame, "document.body?.innerText.trim() === 'Updating — back in a moment' && getComputedStyle(document.body).display === 'flex'", wait);
        skew(0)?;
        let recovered = chrome.until(&frame, &ready, wait);
        let stays = chrome.eval(&shell, "window.__shellStays === true")?;
        s.ok("a framed chat shows Updating during release skew, styled under CSP, then reloads itself and recovers without reloading the shell", updating && styled && recovered && stays == true,
            json!({ "updating": updating, "styled": styled, "recovered": recovered, "shellStayed": stays }));
    }
    chrome.block(&frame, &["*/chat.js"])?;
    chrome.eval(&frame, "(location.reload(), true)")?;
    // (styled: its stylesheet in; before it a line is unstyled, which a
    // person never sees, as the page draws nothing until it has come)
    let styled = format!("!!{line} && getComputedStyle({line}).position === 'fixed'");
    let quiet = chrome.until(&frame, &format!("{styled} && getComputedStyle({line}).visibility === 'hidden'"), wait);
    let t0 = std::time::Instant::now();
    let shown = quiet && chrome.until(&frame, &format!("{styled} && getComputedStyle({line}).visibility === 'visible'"), wait);
    let after = t0.elapsed();
    let seen = chrome.eval(&frame, &format!("({{ said: {line}?.textContent ?? null, link: {line}?.querySelector('a')?.href ?? null, here: location.href }})"))?;
    s.ok(
        "its script refused, the chat's frame says nothing for a while, then \"Still loading… Reload\", the link to its own page",
        shown && after >= Duration::from_secs(8) && seen["said"] == "Still loading… Reload" && seen["link"] == seen["here"],
        json!({ "seen": seen, "afterMs": after.as_millis() as u64 }),
    );
    chrome.block(&frame, &[])?;
    // (a press, as a script's: with no line, a FAIL here, the checks after it made)
    chrome.eval(&frame, "(document.querySelector('#still-loading a')?.click(), true)")?;
    let back = chrome.until(&frame, &ready, wait);
    let stays = chrome.eval(&shell, "window.__shellStays === true")?;
    s.ok("its Reload, the script let through, loads the chat again in its frame, and the shell stays as it was", back && stays == true, json!({ "back": back, "stays": stays }));
    chrome.detach(frame)?;
    chrome.close(shell)?;
    Ok(())
}

/// The chat's mic in the shell's frame, which the platform's mint
/// (`/auth/frame`) sends on to the chat's own origin. A bare
/// `allow="microphone"` names the frame's `src`'s origin, the platform's,
/// and the browser refused the chat's page the microphone without asking
/// ("The microphone is off for this chat", PR 582). Valid: the shell's
/// frame names the chat's origin, and there, the microphone allowed
/// (Chrome's fake, as a person's Allow), the chat is told it may record,
/// and a press records at once, with no card.
fn mic_in_frame(s: &mut Suite, api: &Api, chrome: &mut Browser, chat: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let shell = chrome.open(&format!("{}/", api.base))?;
    chrome.viewport(&shell, 1280, 860, false)?;
    let row = format!("#chats [data-key={}]", js(&format!("chat:{chat}")));
    let listed = chrome.until(&shell, &format!("!document.getElementById('layout').hidden && !!document.querySelector({})", js(&row)), wait);
    chrome.eval(&shell, &format!("(document.querySelector({})?.click(), true)", js(&row)))?;
    let ready = "document.getElementById('say')?.dataset.ready === '1'";
    let framed = listed && s.eventually(wait, || chrome.eval_in_frame(&shell, chat, ready).ok() == Some(Value::Bool(true)));
    let allow = chrome.eval(&shell, &format!("document.querySelector({})?.allow ?? null", js(&format!("iframe[data-fragment=\"{chat}\"]"))))?;
    if !framed {
        s.ok("in the shell's frame, which names the chat's own origin, the chat may use the microphone, and its mic records at once", false, json!({ "framed": false, "allow": allow }));
        return chrome.close(shell);
    }
    let frame = chrome.frame(chat)?;
    let told = chrome.eval(&frame, "navigator.permissions.query({ name: 'microphone' }).then((p) => p.state)")?;
    let mic = "document.getElementById('record')";
    chrome.eval(&frame, &format!("({mic}.click(), true)"))?;
    let on = chrome.until(&frame, &format!("{mic}.classList.contains('on') && document.getElementById('mic-card').hidden && !document.getElementById('recording').hidden"), wait);
    let card = chrome.eval(&frame, "document.getElementById('mic-card').hidden ? null : document.getElementById('mic-card-title').textContent")?;
    chrome.eval(&frame, "(document.getElementById('discard').click(), true)")?;
    let off = chrome.until(&frame, &format!("!{mic}.classList.contains('on')"), wait);
    s.ok(
        "in the shell's frame, which names the chat's own origin, the chat may use the microphone, and its mic records at once",
        framed && told == "granted" && on && off,
        json!({ "framed": framed, "allow": allow, "told": told, "on": on, "card": card, "off": off }),
    );
    chrome.detach(frame)?;
    chrome.close(shell)?;
    Ok(())
}

/// The mic once the chat's access ends (Alex's review of PR 582): a
/// person's page whose access is taken away (4003) while the mic's card is
/// open, and one whose chat is deleted (4004) while the browser is still
/// asked whether it may record. Valid: neither asks for the microphone nor
/// records after: the card goes, its Continue (pressed as a script would,
/// though hidden) starts nothing, and an answer that comes after starts
/// nothing.
fn mic_after_access_ends(s: &mut Suite, api: &Api, chrome: &mut Browser, owner: &Keys, member: &Keys, member_session: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let name = s.named(api, owner, "chat-mic-ends")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the chat whose access ends: {made}");
    s.owned(&made.body, owner);
    let member_id = api.identity(member)?;
    let shared = api.share(&name, &member_id, "editor")?;
    anyhow::ensure!(shared.status == 200, "sharing it with the member: {shared}");
    let ready = "document.getElementById('say')?.dataset.ready === '1' && !document.getElementById('record').hidden && !document.getElementById('record').disabled";
    // the page's asks for the microphone, counted
    let counted = "(() => { window.__asked = 0; const real = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices); navigator.mediaDevices.getUserMedia = (c) => { window.__asked += 1; return real(c); }; return true; })()";
    let after = "({ card: !document.getElementById('mic-card').hidden, asked: window.__asked, on: document.getElementById('record').classList.contains('on'), said: document.getElementById('banner').hidden ? null : document.getElementById('banner-text').textContent })";

    // the member's page, the card open, their access taken away
    let elsewhere = chrome.another_context()?;
    chrome.set_cookie_in(&elsewhere, &format!("{}/", api.base), "fragment_session", member_session)?;
    let theirs = chrome.open_in(&elsewhere, &api.site_url(&name, "__signin?return=/"))?;
    let up = chrome.until(&theirs, ready, wait);
    chrome.eval(&theirs, counted)?;
    chrome.eval(&theirs, "(navigator.permissions.query = async () => ({ state: 'prompt' }), true)")?;
    chrome.eval(&theirs, "(document.getElementById('record').click(), true)")?;
    let carded = chrome.until(&theirs, "!document.getElementById('mic-card').hidden && document.getElementById('mic-card').dataset.kind === 'ask'", wait);
    let gone = api.signed(owner, "DELETE", &format!("/api/f/{name}/members/{member_id}"), None)?;
    let closed = chrome.until(&theirs, "document.getElementById('banner-text')?.textContent === 'Your access to this chat changed: reload the page.'", wait);
    chrome.eval(&theirs, "(document.getElementById('mic-card-go').click(), true)")?;
    std::thread::sleep(Duration::from_secs(1));
    let seen = chrome.eval(&theirs, after)?;
    s.ok(
        "their access taken away (4003) while the mic's card is open, the card goes and its Continue asks for nothing: no microphone, no memo",
        up && carded && gone.status == 200 && closed && seen["card"] == false && seen["asked"] == 0 && seen["on"] == false,
        json!({ "up": up, "carded": carded, "removed": gone.status, "closed": closed, "seen": seen }),
    );
    chrome.close(theirs)?;

    // the owner's page, the browser still asked, the chat deleted
    let page = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    let up = chrome.until(&page, ready, wait);
    chrome.eval(&page, counted)?;
    chrome.eval(&page, "(navigator.permissions.query = () => new Promise((answer) => { window.__answer = answer; }), true)")?;
    chrome.eval(&page, "(document.getElementById('record').click(), true)")?;
    let asking = chrome.until(&page, "typeof window.__answer === 'function'", wait);
    let deleted = api.signed(owner, "DELETE", &format!("/api/f/{name}"), None)?;
    let closed = chrome.until(&page, "document.getElementById('banner-text')?.textContent === 'This chat was deleted.'", wait);
    chrome.eval(&page, "(window.__answer?.({ state: 'granted' }), true)")?;
    std::thread::sleep(Duration::from_secs(1));
    let seen = chrome.eval(&page, after)?;
    s.ok(
        "its chat deleted (4004) while the browser is asked whether it may record, the answer that comes after starts nothing: no microphone, no memo",
        up && asking && deleted.status == 200 && closed && seen["card"] == false && seen["asked"] == 0 && seen["on"] == false,
        json!({ "up": up, "asking": asking, "deleted": deleted.status, "closed": closed, "seen": seen }),
    );
    chrome.close(page)?;
    Ok(())
}

/// A chat page left open whose person signed out elsewhere (Austin,
/// 2026-10-25: in Firefox a new chat's page stayed empty for minutes, even
/// after a reload, while his phone opened it; a page in another tab whose
/// session had ended dialled its socket again every half minute, each
/// refused (401), and Firefox holds back every socket to the same address
/// and path, `/__live` for every fragment, for up to a minute after each
/// refusal). Its socket, asked again who it is, is closed to reconnect
/// (4001), and the one it dials then names no one who may see the chat.
/// Valid: that socket is opened and closed at once, for good (4003), not
/// refused by a status the page cannot read: the page says its access
/// changed and dials no more. A socket that names no page (the CLI's) is
/// still refused by its status (401).
fn signed_out_page(s: &mut Suite, api: &Api, chrome: &mut Browser, owner: &Keys) -> Result<()> {
    let wait = Duration::from_secs(30);
    let name = s.named(api, owner, "chat-signed-out")?;
    let made = api.create_with(owner, json!({ "name": name, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the chat whose page signs out: {made}");
    s.owned(&made.body, owner);
    let member = api.person()?;
    let shared = api.share(&name, &api.identity(&member)?, "editor")?;
    anyhow::ensure!(shared.status == 200, "sharing it with the member: {shared}");
    let session = api.sign_in(&Api::email_of(&member))?;
    let site = format!("fragment_site={}", site_cookie(api, &session, &name)?);

    // the member's page, open, its dials of `__live` counted from here
    let elsewhere = chrome.another_context()?;
    chrome.set_cookie_in(&elsewhere, &format!("{}/", api.base), "fragment_session", &session)?;
    let page = chrome.open_in(&elsewhere, &api.site_url(&name, "__signin?return=/"))?;
    let up = chrome.until(&page, "document.getElementById('say')?.dataset.ready === '1'", wait);
    let counted = "(() => { window.__dials = 0; const Real = WebSocket; window.WebSocket = class extends Real { constructor(url, ...rest) { super(url, ...rest); if (String(url).includes('/__live')) window.__dials += 1; } }; return true; })()";
    chrome.eval(&page, counted)?;

    // signed out elsewhere: the platform session ends, and every site session made from it
    let ended = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "expireSession": session } })))?;
    anyhow::ensure!(ended.status == 200, "ending the member's session: {ended}");
    let own_page = match Socket::on_host(api, &name, "__live", None, Some(&site), Some(&api.site_origin(&name))) {
        Ok(mut socket) => match socket.next() {
            Ok(frame) => format!("opened: {frame}"),
            Err(e) => format!("{e:#}"),
        },
        Err(e) => format!("refused: {e:#}"),
    };
    s.ok("a page's socket whose person signed out is opened and closed for good (4003), not refused by a status its page cannot read", own_page == "closed 4003", &own_page);
    let unnamed = match Socket::on_host(api, &name, "__live", None, Some(&site), None) {
        Ok(_) => "opened".to_string(),
        Err(e) => format!("refused: {e:#}"),
    };
    s.ok("a socket that names no page is still refused by its status (members only: 401)", unnamed.contains("401"), &unnamed);

    // the open page's socket asked again who it is (a test lever ages the
    // check), at its next frame: the member typing
    let aged = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "age-live", "ms": fragment_proto::limits::LIVE_IDENTITY_MS })))?;
    chrome.eval(&page, "(() => { const t = document.getElementById('text'); t.value = 'still here?'; t.dispatchEvent(new Event('input')); return true; })()")?;
    let told = chrome.until(&page, "document.getElementById('banner-text')?.textContent === 'Your access to this chat changed: reload the page.'", wait);
    // past the library's first waits (half to one and a half of 1 s, then 2 s): it would have dialled again
    std::thread::sleep(Duration::from_secs(5));
    let seen = chrome.eval(&page, "({ dials: window.__dials, said: document.getElementById('banner').hidden ? null : document.getElementById('banner-text').textContent })")?;
    s.ok(
        "the open page, asked again who it is, dials once more, is closed for good (4003), says its access changed, and dials no more",
        up && aged.status == 200 && told && seen["dials"] == 1,
        json!({ "up": up, "aged": aged.status, "told": told, "seen": seen }),
    );
    chrome.close(page)?;
    Ok(())
}

/// The mic's card as shown (`kind`, "ask" or "denied"), light and dark,
/// into the run's screenshots; the page left light, as it starts.
fn mic_card_shots(chrome: &mut Browser, page: &Page, shots: &Path, kind: &str) -> Result<()> {
    for scheme in ["light", "dark"] {
        chrome.color_scheme(page, scheme)?;
        chrome.screenshot(page, &shots.join(format!("mic-card-{kind}-{scheme}.png")))?;
    }
    chrome.color_scheme(page, "light")
}

/// A JSON array's strings.
fn strings_of(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

/// The page's viewport made `width`×`height` (a phone's when `mobile`),
/// and waited for until the chat fits it. The chat is as tall as the
/// visual viewport, from that viewport's `resize` (chat.js `fitViewport`,
/// `--viewport-height`), which comes a frame or more after the change;
/// until it does, the composer is where the old size put it, and a press
/// read from its box lands beside it (what was typed then went nowhere:
/// run 39542336296's "on a desktop Enter sends", a local run's "on a phone
/// too, Reply quotes").
fn fit(chrome: &mut Browser, page: &Page, width: u32, height: u32, mobile: bool) -> Result<()> {
    chrome.viewport(page, width, height, mobile)?;
    let fitted = format!(
        "innerWidth === {width} && Math.round(visualViewport.height) === {height} && document.documentElement.style.getPropertyValue('--viewport-height') === '{height}px'"
    );
    anyhow::ensure!(chrome.until(page, &fitted, TURN), "the chat fits {width}×{height}: {}", chrome.eval(page, "[innerWidth, visualViewport.height, document.documentElement.style.getPropertyValue('--viewport-height')]")?);
    Ok(())
}

/// A JS string literal.
fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string encodes")
}

pub fn chat(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("chat", &[crate::Need::Fakes, crate::Need::LocalDocker, crate::Need::Chrome, crate::Need::Levers]) {
        return Ok(());
    }
    let owner = api.person()?;
    let member = api.person()?;
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let computer = r.body["computer"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && computer.starts_with("computer:"), "making the computer: {r}");

    // an agent fragment on the computer, and a chat on the blessed template
    let agent_name = s.named(api, &owner, "juniper")?;
    let agent = s.create(api, &owner, &agent_name)?;
    s.commit(&agent, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&agent);
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{computer}/agents/{agent_name}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && fragment_core::npub::is_identity(&identity), "assigning the agent: {r}");
    let chat_name = s.named(api, &owner, "talk")?;
    let r = api.create_with(&owner, json!({ "name": chat_name, "template": "chat" }))?;
    anyhow::ensure!(r.status == 200, "making the chat on the template: {r}");
    s.owned(&r.body, &owner);
    let made = r.body.clone();
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    s.ok("the agent joins the chat on the template", r.status == 200, &r);
    // the platform tells the agent it joined (Paul, 2026-10-03), and wakes its computer
    let member_id = api.identity(&member)?;
    let r = api.share(&chat_name, &member_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "adding a viewer: {r}");

    // ---- the template's code runs from the platform's release (decision 40): its push job
    let code = |api: &Api| api.status(&owner, &chat_name).map(|r| r.body["code"].clone()).unwrap_or_default();
    let installed = s.eventually(Duration::from_secs(20), || code(api)["operations"]["notify_reply"]["kind"] == "job");
    let files = api.signed(&owner, "GET", &format!("/api/f/{chat_name}/files"), None)?;
    let own: Vec<String> = files.body["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(str::to_string)).collect();
    s.ok(
        "the chat runs the template's code from the release, its push job, with none in its own repo",
        installed && code(api)["error"].is_null() && !own.iter().any(|p| p == "app.mjs"),
        json!({ "code": code(api), "files": own }),
    );
    let triggers = api.signed(&owner, "GET", &format!("/api/f/{chat_name}/triggers"), None)?;
    s.ok(
        "its trigger starts the job for its agents' records alone",
        triggers.body["triggers"] == json!([{ "channel": "chat", "from": "agent", "run": "notify_reply", "paused": false }]),
        &triggers,
    );
    s.commit(&made, &[("app.mjs", Some(b"export class App { notify_reply() { return { pushed: 99 }; } }\n"))]);
    s.deploy(&made);
    let refused = s.eventually(Duration::from_secs(30), || code(api)["error"].as_str().is_some_and(|e| e.contains("carries no code of its own (app.mjs): fork it")));
    s.ok(
        "an app.mjs of the chat's own does not override the release's: it is refused, saying to fork, and the release's code stays",
        refused && code(api)["operations"]["notify_reply"]["kind"] == "job",
        code(api),
    );
    s.commit(&made, &[("app.mjs", None)]);
    s.deploy(&made);
    let back = s.eventually(Duration::from_secs(30), || code(api)["error"].is_null());
    s.ok("without it, the chat installs again from the release", back && code(api)["operations"]["notify_reply"].is_object(), code(api));

    // ---- the page's own routes, as a browser signed in there calls them
    let owner_session = api.sign_in(&Api::email_of(&owner))?;
    let member_session = api.sign_in(&Api::email_of(&member))?;
    let owner_site = format!("fragment_site={}", site_cookie(api, &owner_session, &chat_name)?);
    let member_site = format!("fragment_site={}", site_cookie(api, &member_session, &chat_name)?);
    let served = s.eventually(Duration::from_secs(20), || api.page(&chat_name, "", Some(&owner_site)).is_ok_and(|r| r.status == 200 && r.text.contains("./chat.js")));
    s.ok("the chat serves the template's page", served, "");
    let label = fragment_proto::split_fragment_name(&agent_name).map_or("", |(l, _)| l).to_string();
    let r = api.page(&chat_name, &format!("__people?id={identity}"), Some(&owner_site))?;
    let profile = &r.body["profiles"][identity.as_str()];
    s.ok(
        "__people names an agent made from an agent fragment: its label, and its fragment",
        r.status == 200 && profile["kind"] == "agent" && profile["name"] == label.as_str() && profile["fragment"] == agent_name.as_str(),
        &r,
    );
    let r = api.page(&chat_name, "__members", Some(&owner_site))?;
    let listed = r.body["members"].as_array().cloned().unwrap_or_default();
    s.ok(
        "__members lists the chat's members, its agent among them",
        r.status == 200 && listed.iter().any(|m| m["principal"] == identity.as_str() && m["kind"] == "agent" && m["role"] == "editor"),
        &r,
    );
    let r = api.page(&chat_name, "__members", Some(&member_site))?;
    s.ok("a viewer lists them too", r.status == 200, &r);
    let r = api.page(&chat_name, "__members", None)?;
    s.ok("and no one who may not see the chat", r.status == 401, &r);
    let bytes = b"a page's upload".to_vec();
    let sha = hex::encode(Sha256::digest(&bytes));
    let put = |sha: &str, cookie: Option<&str>, extra: Vec<(&'static str, String)>| {
        api.call(Call { method: "PUT", url: api.site_url(&chat_name, &format!("__blob/{sha}")), body: Some(bytes.clone()), content_type: Some("text/plain"), cookie: cookie.map(str::to_string), extra, ..Call::default() })
    };
    let r = put(&sha, Some(&owner_site), vec![])?;
    s.ok("an editor's page uploads a blob at __blob/<sha256>", r.status == 200 && r.body["sha"] == sha.as_str() && r.body["stored"] == true, &r);
    let again = put(&sha, Some(&owner_site), vec![])?;
    s.ok("the same upload again stores nothing new", again.status == 200 && again.body["stored"] == false, &again);
    let r = api.page(&chat_name, &format!("__blob/{sha}"), Some(&owner_site))?;
    s.ok("and the page reads it there", r.status == 200 && r.bytes == bytes, &r);
    let wrong = hex::encode(Sha256::digest(b"something else"));
    let r = put(&wrong, Some(&owner_site), vec![])?;
    s.ok("bytes that are not what their hash says are refused", r.status == 400 && r.text.contains("hash to"), &r);
    let r = put(&sha, Some(&member_site), vec![])?;
    s.ok("a viewer may not upload", r.status == 403, &r);
    let r = put(&sha, None, vec![])?;
    s.ok("nor anyone not signed in", r.status == 401, &r);
    let other_page = vec![("sec-fetch-site", "same-site".to_string()), ("sec-fetch-mode", "cors".to_string()), ("sec-fetch-dest", "empty".to_string())];
    let r = put(&sha, Some(&owner_site), other_page)?;
    s.ok("another fragment's page cannot upload with the owner's cookie", r.status == 401, &r);

    // ---- awake, the guest follows the chat as the agent
    let r = api.signed(&owner, "POST", &format!("/api/computers/{computer}/wake"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "waking the computer: {r}");
    let subscribed = s.eventually(WAKE, || {
        api.signed(&owner, "GET", &format!("/api/f/{chat_name}/subscriptions"), None)
            .ok()
            .is_some_and(|r| r.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["wake"] == true && x["channel"] == "chat")))
    });
    s.ok("the computer's guest follows the chat", subscribed, "");

    // ---- push: each of the chat's people subscribes their own browser
    let owner_id = api.identity(&owner)?;
    let owner_push = s.name("chat-owner-push");
    let r = subscribe(s, api, &chat_name, &owner_site, &owner_id, &owner_push, 17)?;
    s.ok("the owner's page subscribes their browser, tagged with their identity", r.status == 200 && r.body["who"] == owner_id.as_str(), &r);
    let another = subscribe(s, api, &chat_name, &owner_site, &member_id, &s.name("chat-not-theirs"), 19)?;
    let agents = subscribe(s, api, &chat_name, &owner_site, &identity, &s.name("chat-agents-push"), 23)?;
    s.ok("no one subscribes for another identity, a person's or an agent's (403)", another.status == 403 && agents.status == 403, format!("{another} | {agents}"));

    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the chat section (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("chat");
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &owner_session)?;
    let page = chrome.open(&api.site_url(&chat_name, "__signin?return=/"))?;
    fit(&mut chrome, &page, 1280, 860, false)?;
    let ready = chrome.until(&page, "document.getElementById('say')?.dataset.ready === '1'", TURN);
    let role = chrome.eval(&page, "import('./__fragment.js').then((f) => f.me()).then((m) => m.role)").unwrap_or_default();
    s.ok("the owner, signed in on the chat's page, may write", ready && role == "owner" && chrome.eval(&page, "!document.getElementById('text').disabled")? == true, &role);
    let named = format!("document.title === {} && document.getElementById('text').placeholder === {}", js(&capital(&label)), js(&format!("Message {}", capital(&label))));
    s.ok("it names the chat by its agent, and the composer says whom it messages", shows(&mut chrome, &page, &named), chrome.eval(&page, "[document.title, document.getElementById('text').placeholder]")?);
    s.ok("a new chat shows its empty start", chrome.eval(&page, "!!document.querySelector('.empty .agent-avatar.large')")? == true, "");

    // the page notes every draft it shows, as a person would see them
    chrome.eval(
        &page,
        "(() => { window.__drafts = []; new MutationObserver(() => { for (const d of document.querySelectorAll('.msg.agent.streaming .md')) { const t = d.textContent; if (t && window.__drafts[window.__drafts.length - 1] !== t) window.__drafts.push(t); } }).observe(document.getElementById('messages'), { childList: true, subtree: true, characterData: true }); return true; })()",
    )?;
    let say = |chrome: &mut Browser, text: &str| -> Result<()> {
        chrome.click(&page, "#text")?;
        chrome.type_text(&page, text)?;
        chrome.press(&page, "Enter", false)
    };
    let mine = |text: &str| format!("[...document.querySelectorAll('.msg.user.mine .bubble')].some((b) => b.textContent === {})", js(text));
    let replied = |text: &str| format!("[...document.querySelectorAll('.msg.agent:not(.streaming) .md')].some((m) => m.textContent.includes({}))", js(text));
    // The record of the owner's message `text`, waited for within TURN: the
    // page shows its own message as it posts it (`.sending`), before the
    // platform has answered the post and written its record, so a read
    // right after it shows may find none (as #456 found for a command's).
    let posted = |s: &Suite, text: &str| -> Value {
        let mut found = Value::Null;
        s.eventually(TURN, || {
            found = records(api, &owner, &chat_name, "chat").into_iter().find(|x| x["body"]["text"] == text).unwrap_or_default();
            !found.is_null()
        });
        found
    };

    // typed, Enter: the message, the agent's draft live, then its reply
    let first = "hello there, slowly";
    chrome.click(&page, "#text")?;
    chrome.type_text(&page, "hello there,")?;
    chrome.press(&page, "Enter", true)?;
    let lines = chrome.eval(&page, "document.getElementById('text').value")?;
    s.ok("Shift+Enter adds a line, and sends nothing", lines == "hello there,\n" && chrome.eval(&page, "document.querySelectorAll('.msg.user').length")? == 0, &lines);
    chrome.eval(&page, "(() => { const t = document.getElementById('text'); t.value = ''; t.dispatchEvent(new Event('input')); return true; })()")?;
    say(&mut chrome, first)?;
    s.ok("typed and sent with Enter, the message shows as the owner's", shows(&mut chrome, &page, &mine(first)), "");
    s.ok("and the composer is empty again", chrome.eval(&page, "document.getElementById('text').value")? == "", "");
    let sent = posted(s, first);
    s.ok("the page posted it to chat as the owner, `{text}`", sent["principal"] == api.identity(&owner).unwrap_or_default().as_str(), &sent);
    let answered = shows(&mut chrome, &page, &format!("{} && !document.querySelector('.msg.agent.streaming')", replied(first)));
    let reply = chrome.eval(&page, &format!("[...document.querySelectorAll('.msg.agent .md')].map((m) => m.textContent).find((t) => t.includes({})) ?? ''", js(first)))?;
    let reply = reply.as_str().unwrap_or("").to_string();
    let drafts = chrome.eval(&page, "window.__drafts")?;
    // each draft the whole text so far: a beginning of the reply
    let drafted = drafts.as_array().is_some_and(|d| !d.is_empty() && d.iter().all(|t| t.as_str().is_some_and(|t| reply.starts_with(t) && t.len() < reply.len())));
    s.ok("the agent's draft shows live, as it writes", drafted, json!({ "drafts": drafts, "reply": reply }));
    s.ok("then its reply replaces the draft", answered && reply.starts_with("echo:"), chrome.eval(&page, "document.getElementById('messages').innerText")?);
    s.ok("the agent's reply is under its name", chrome.eval(&page, &format!("[...document.querySelectorAll('.msg.agent .who')].every((w) => w.textContent === {})", js(&capital(&label))))? == true, "");

    // a tool step, as a card: what it does (its category's icon, its verb
    // the icon's name), then its arguments and what came of it; the words
    // the agent said before it are its reply, before the card and not in it
    say(&mut chrome, "tool please")?;
    let step = "[...document.querySelectorAll('details.tools .step[data-category=\"web\"]')].find((s) => s.querySelector('.step-args')?.textContent === 'tool please')";
    let words = "[...document.querySelectorAll('.msg.agent .md')].find((m) => m.textContent === 'Let me look.')?.closest('.msg')";
    let stepped = shows(&mut chrome, &page, &format!("!!{step} && !{step}.querySelector('.step-verb') && ['Searching the web', 'Searched the web'].includes({step}.querySelector('.step-name > svg[role=\"img\"]')?.getAttribute('aria-label')) && {step}.querySelector('.step-result')?.textContent.includes('3 results') && !!{words} && !{words}.closest('details') && !!({words}.compareDocumentPosition({step}) & Node.DOCUMENT_POSITION_FOLLOWING) && !{step}.closest('details').textContent.includes('Let me look.') && {}", replied("tool please")));
    s.ok("a tool step shows as a card before the reply, saying what it does by its icon (named \"Searched the web\") and its arguments (\"tool please\"), its result kept; the agent's words before it (\"Let me look.\") are its reply before the card, never in it", stepped, chrome.eval(&page, "document.getElementById('messages').innerText")?);
    // every card open, for the screenshots: what each step says
    let open_cards = "(() => { const cards = [...document.querySelectorAll('details.tools')]; cards.forEach((d) => (d.open = true)); cards.at(-1)?.scrollIntoView({ block: 'center' }); return true; })()";
    chrome.eval(&page, open_cards)?;
    let _ = chrome.screenshot(&page, &shots.join("desktop-step.png"));

    // a runtime's notices (as Hermes' inactivity warning or its review of
    // its memory, the one after the turn's end): quiet lines of the turn,
    // never a step nor the agent's words
    say(&mut chrome, "a notice please")?;
    let notice = |category: &str, text: &str| format!("[...document.querySelectorAll('.msg.notice.runtime.{category}')].some((n) => n.textContent === {} && n.querySelector('svg') && !n.closest('details'))", js(text));
    let noticed = shows(&mut chrome, &page, &format!("{} && {} && {}", notice("info", "Compressing context"), notice("memory", "Self-improvement review: Memory updated"), replied("a notice please")));
    let words = chrome.eval(&page, "[...document.querySelectorAll('.msg.agent .md, details.tools')].some((m) => /Self-improvement|Compressing/.test(m.textContent))")?;
    s.ok("a runtime's notices show as quiet lines of their turn, the one after its end too, never a step nor its words", noticed && words == false, chrome.eval(&page, "document.getElementById('messages').innerText")?);
    chrome.eval(&page, open_cards)?;
    chrome.eval(&page, "(() => { const s = document.getElementById('scroll'); s.scrollTop = s.scrollHeight; return true; })()")?;
    let _ = chrome.screenshot(&page, &shots.join("desktop-notices.png"));

    // a question the agent asks to be answered in words (as Hermes' clarify
    // asks one): no working line, but whose answer it waits for, and the
    // composer says what they type is it; their next message answers it
    say(&mut chrome, "name my plant, ask-me")?;
    let waits = "[...document.querySelectorAll('.msg.working.asked')].some((w) => w.textContent === 'Waiting for your answer') && !document.querySelector('.msg.working:not(.asked)')";
    let answer_here = format!("document.getElementById('text').placeholder === {}", js(&format!("Your answer for {}", capital(&label))));
    let asked = shows(&mut chrome, &page, &format!("{} && {waits} && {answer_here}", replied("What should I call it?")));
    s.ok(
        "asked in words, the page says it waits for the owner's answer, and the composer that what they type answers it",
        asked,
        chrome.eval(&page, "[document.getElementById('messages').innerText.slice(-300), document.getElementById('text').placeholder]")?,
    );
    let _ = chrome.screenshot(&page, &shots.join("desktop-asked.png"));
    say(&mut chrome, "Sprout")?;
    let answered = shows(&mut chrome, &page, &format!("{} && !document.querySelector('.msg.asked') && !({answer_here})", replied("(told: Sprout)")));
    s.ok("their next message is the answer: the turn goes on with it, and nothing waits", answered, chrome.eval(&page, "document.getElementById('messages').innerText.slice(-300)")?);

    // an @mention: the composer offers the chat's agents, and the message is `to` the one picked
    chrome.click(&page, "#text")?;
    chrome.type_text(&page, &format!("@{}", &label[..3]))?;
    let offered = shows(&mut chrome, &page, &format!("!document.getElementById('mentions').hidden && document.getElementById('mentions').textContent === {}", js(&capital(&label))));
    chrome.press(&page, "Tab", false)?;
    chrome.type_text(&page, "are you there")?;
    let typed = chrome.eval(&page, "document.getElementById('text').value")?;
    s.ok("typing @ offers the chat's agents, and Tab picks one", offered && typed == format!("@{label} are you there").as_str(), &typed);
    chrome.press(&page, "Enter", false)?;
    let addressed = format!("@{label} are you there");
    s.ok("the agent answers it", shows(&mut chrome, &page, &replied(&addressed)), "");
    let r = records(api, &owner, &chat_name, "chat");
    let to = r.iter().find(|x| x["body"]["text"] == addressed.as_str()).map(|x| x["body"]["to"].clone()).unwrap_or_default();
    s.ok("its record is `to` the agent its @mention named", to == json!([identity]), &to);

    // an approval: a card with buttons for the owner, which a member sees but may not press
    say(&mut chrome, "approve this")?;
    let card = "document.querySelector('.prompt:not(.closed) button[data-option=\"once\"]')";
    s.ok("an approval shows as a card with its buttons, enabled for the owner", shows(&mut chrome, &page, &format!("{card} && !{card}.disabled")), chrome.eval(&page, "document.getElementById('messages').innerText")?);
    let elsewhere = chrome.another_context()?;
    chrome.set_cookie_in(&elsewhere, &format!("{}/", api.base), "fragment_session", &member_session)?;
    let theirs = chrome.open_in(&elsewhere, &api.site_url(&chat_name, "__signin?return=/"))?;
    fit(&mut chrome, &theirs, 1280, 860, false)?;
    let seen = chrome.until(&theirs, "document.querySelectorAll('.prompt:not(.closed) .options button').length === 2", TURN);
    let pressable = chrome.eval(&theirs, "[...document.querySelectorAll('.prompt:not(.closed) .options button')].filter((b) => !b.disabled).length")?;
    let note = chrome.eval(&theirs, "document.querySelector('.prompt .prompt-note')?.textContent ?? ''")?;
    s.ok("another member sees the card, its buttons disabled, saying who may answer", seen && pressable == 0 && note.as_str().is_some_and(|n| n.contains("can answer")), json!({ "pressable": pressable, "note": note }));
    s.ok("and may not attach files (a blob is an editor's)", chrome.eval(&theirs, "document.getElementById('attach').disabled")? == true, "");
    s.ok("nor record a voice memo", chrome.eval(&theirs, "document.getElementById('record').disabled")? == true, "");
    chrome.click(&page, ".prompt:not(.closed) button[data-option=\"once\"]")?;
    let closed = shows(&mut chrome, &page, "document.querySelector('.prompt.closed .outcome.answered')?.textContent.startsWith('Allow once')");
    s.ok("its owner answers by clicking, and the card says how it closed", closed, chrome.eval(&page, "document.querySelector('.prompt')?.innerText")?);
    s.ok("the turn goes on approved", shows(&mut chrome, &page, &replied("(approved)")), "");
    let r = records(api, &owner, &chat_name, "chat");
    let answer = r.iter().find(|x| x["body"]["kind"] == "prompt_response").cloned().unwrap_or_default();
    let asked = records(api, &owner, &chat_name, "work").into_iter().find(|x| x["body"]["kind"] == "turn.prompt" && x["body"]["prompt"] == answer["body"]["prompt"]).unwrap_or_default();
    s.ok(
        "the page answered with a prompt_response for the card's option, naming the card's agent and turn",
        answer["body"]["option"] == "once" && answer["body"]["prompt"].is_string() && answer["body"]["agent"] == identity.as_str() && answer["body"]["turn"] == asked["body"]["turn"] && asked["principal"] == identity.as_str(),
        json!({ "answer": answer, "card": asked }),
    );
    s.ok("the member's card closes too", chrome.until(&theirs, "!!document.querySelector('.prompt.closed .outcome.answered')", TURN), "");
    chrome.close(theirs)?;

    // choices the agent asks (as Hermes' clarify asks them), the last
    // answered in words: typed on the card, sent with Enter (Paul on p5,
    // 2026-10-09: "'other' type your answer for questions doesn't work very well")
    say(&mut chrome, "choose a plant")?;
    let field = ".prompt:not(.closed) input[data-option=\"other\"]";
    let offered = shows(&mut chrome, &page, &format!("!!document.querySelector({0}) && !document.querySelector({0}).disabled", js(field)));
    let _ = chrome.screenshot(&page, &shots.join("desktop-choices.png"));
    s.ok("a card of choices offers the option answered in words as a field on the card", offered, chrome.eval(&page, "document.querySelector('.prompt:not(.closed)')?.innerText ?? null")?);
    if offered {
        chrome.click(&page, field)?;
        chrome.type_text(&page, "rosemary")?;
        chrome.press(&page, "Enter", false)?;
    }
    let closed = shows(&mut chrome, &page, "[...document.querySelectorAll('.prompt.closed .outcome.answered')].some((o) => o.textContent.startsWith('rosemary'))");
    s.ok("typed there and sent with Enter, the card closes saying what was answered", closed, chrome.eval(&page, "[...document.querySelectorAll('.prompt')].map((p) => p.innerText)")?);
    s.ok("and the agent has the words as the answer", shows(&mut chrome, &page, &replied("(chose: rosemary)")), chrome.eval(&page, "document.getElementById('messages').innerText")?);
    let r = records(api, &owner, &chat_name, "chat");
    let answer = r.iter().find(|x| x["body"]["kind"] == "prompt_response" && x["body"]["option"] == "other").cloned().unwrap_or_default();
    s.ok("the page answered with the option and its words, `{option, text}`", answer["body"]["text"] == "rosemary", &answer);
    let _ = chrome.screenshot(&page, &shots.join("desktop-choices-answered.png"));

    // the same card answered in the message box (Paul on production,
    // 2026-10-22: his typed reply stayed queued and the card open): the
    // composer says what they type answers it; sent, the card closes with
    // their words, the agent has them, and the message waits for nothing
    say(&mut chrome, "choose again")?;
    let open = shows(&mut chrome, &page, &format!("!!document.querySelector({}) && {answer_here}", js(field)));
    s.ok("a card that takes words open, the composer says what its owner types answers it", open, chrome.eval(&page, "document.getElementById('text').placeholder")?);
    say(&mut chrome, "thyme")?;
    let closed = shows(&mut chrome, &page, "[...document.querySelectorAll('.prompt.closed .outcome.answered')].some((o) => o.textContent.startsWith('thyme'))");
    s.ok("sent from the message box, it answers the card: the card closes saying the words", closed, chrome.eval(&page, "[...document.querySelectorAll('.prompt')].map((p) => p.innerText)")?);
    let ran_on = shows(&mut chrome, &page, &format!("{} && !document.querySelector('.prompt:not(.closed)') && !({answer_here})", replied("choose again (chose: thyme)")));
    let waits = chrome.eval(&page, "[...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.bubble')?.textContent === 'thyme' && !!m.querySelector('.queued'))")?;
    let thyme = posted(s, "thyme");
    let own_turns = records(api, &owner, &chat_name, "work").iter().filter(|r| r["body"]["kind"] == "turn.start" && r["body"]["cause"]["seq"] == thyme["seq"]).count();
    s.ok("the turn goes on with them, and the message waits for nothing: never marked queued, no turn of its own", ran_on && waits == false && own_turns == 0, json!({ "queued": waits, "turns": own_turns }));

    // Stop: the send circle becomes Stop while the owner's turn runs. The
    // turn is the message's own (docs/chat-records.md): the page may still
    // show Stop for the turn before, whose end it has not heard yet, and a
    // click then stops nothing
    let slow = "slow, then stopped";
    say(&mut chrome, slow)?;
    let sent_at = |r: &Value| (r["body"]["text"] == slow).then(|| r["seq"].as_i64()).flatten();
    let mut seq = None;
    s.eventually(TURN, || {
        seq = records(api, &owner, &chat_name, "chat").iter().find_map(sent_at);
        seq.is_some()
    });
    let turn = turn_of(&agent_name, &chat_name, "chat", seq.unwrap_or(0));
    let stoppable = chrome.until(&page, &format!("(() => {{ const b = document.getElementById('stop'); return !b.hidden && !b.disabled && b.dataset.turn === {}; }})()", js(&turn)), TURN);
    s.ok("while the owner's turn runs, the send circle is Stop", stoppable && document_hidden(&mut chrome, &page, "send")?, &turn);
    chrome.click(&page, "#stop")?;
    let notice = shows(&mut chrome, &page, &format!("document.querySelector('.msg.notice.stopped[data-turn={}]') && document.getElementById('stop').hidden", js(&turn)));
    let ended = work_of(&records(api, &owner, &chat_name, "work"), &turn).iter().any(|r| r["body"]["kind"] == "turn.end" && r["body"]["outcome"] == "stopped");
    let full = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).iter().any(|r| r["body"]["turn"] == turn.as_str());
    s.ok("clicking Stop stops the turn: it ends stopped, its answer never whole", notice && ended && !full, chrome.eval(&page, "document.getElementById('messages').innerText")?);
    let stop = records(api, &owner, &chat_name, "chat").into_iter().find(|r| r["body"]["kind"] == "stop").unwrap_or_default();
    s.ok("the page's Stop is a record naming the turn", stop["body"]["turn"] == turn.as_str(), &stop);

    // pictures the owner attaches: in their message, as the chat's blobs, and read by the agent
    let mut pictures = Vec::new();
    for (name, width, height) in [("picture.png", 960, 240), ("tall.png", 240, 960), ("square.png", 960, 960)] {
        let png = chrome.eval(
            &page,
            &format!("(() => {{ const c = document.createElement('canvas'); c.width = {width}; c.height = {height}; const g = c.getContext('2d'); const l = g.createLinearGradient(0, 0, {width}, {height}); l.addColorStop(0, '#a88bea'); l.addColorStop(1, '#62c8af'); g.fillStyle = l; g.fillRect(0, 0, {width}, {height}); return c.toDataURL('image/png').split(',')[1]; }})()"),
        )?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(png.as_str().context("the canvas answered a data URL")?)?;
        let sha = hex::encode(Sha256::digest(&bytes));
        let path = shots.join(name);
        std::fs::write(&path, &bytes)?;
        pictures.push((name, bytes, sha, path));
    }
    let paths: Vec<&Path> = pictures.iter().map(|(_, _, _, path)| path.as_path()).collect();
    chrome.choose_files(&page, "#attachment-picker", &paths)?;
    s.ok("chosen files show as removable chips", shows(&mut chrome, &page, "[...document.querySelectorAll('#attachments .attachment-chip')].length === 3 && [...document.querySelectorAll('#attachments .attachment-chip')].every((c) => c.querySelector('.attachment-remove'))"), "");
    say(&mut chrome, "here is a picture")?;
    let images = "[...document.querySelectorAll('.msg.user.mine .attachment-image img')].filter((i) => ['picture.png', 'tall.png', 'square.png'].includes(i.alt))";
    let shown = format!("{images}.length === 3 && {images}.every((i) => i.complete && i.naturalWidth > 0)");
    s.ok("the message carries them, shown inline", shows(&mut chrome, &page, &shown), chrome.eval(&page, "document.getElementById('messages').innerHTML.slice(-600)")?);
    let picture = &pictures[0].1;
    let picture_sha = &pictures[0].2;
    let fetched = chrome.eval(&page, &format!("fetch('./__blob/{picture_sha}').then(async (r) => [r.status, r.headers.get('content-type'), (await r.arrayBuffer()).byteLength])"))?;
    s.ok("the page reads one back as the chat's blob, as uploaded", fetched == json!([200, "image/png", picture.len()]), &fetched);
    let blob = api.signed(&owner, "GET", &format!("/api/f/{chat_name}/blobs/{picture_sha}"), None)?;
    s.ok("the same bytes the API serves", blob.status == 200 && blob.bytes == *picture, blob.status);
    let carried = posted(s, "here is a picture")["body"]["attachments"].clone();
    let expected: Vec<Value> = pictures.iter().map(|(name, bytes, sha, _)| json!({ "sha256": sha, "size": bytes.len(), "type": "image/png", "name": name })).collect();
    s.ok("their record names the files as docs/chat-records.md does", carried == Value::Array(expected), &carried);
    s.ok("the agent read them", shows(&mut chrome, &page, &replied("[got 3: picture.png, tall.png, square.png]")), "");

    // Large pictures keep their aspect ratio, capped at 240px and at the
    // message's available width; each wrapper's content box hugs its image.
    for (width, height, mobile) in [(1280, 860, false), (375, 812, true), (280, 812, true)] {
        fit(&mut chrome, &page, width, height, mobile)?;
        let geometry = chrome.eval(&page, &format!("(() => {{ const images = {images}; return {{ width: innerWidth, images: images.map((img) => {{ const box = img.closest('.attachment-image'); const message = img.closest('.msg'); const r = img.getBoundingClientRect(); const b = box.getBoundingClientRect(); const style = getComputedStyle(box); const left = parseFloat(style.borderLeftWidth); const right = parseFloat(style.borderRightWidth); const top = parseFloat(style.borderTopWidth); const bottom = parseFloat(style.borderBottomWidth); const available = message.clientWidth - left - right; const scale = Math.min(1, (240 - left - right) / img.naturalWidth, 240 / img.naturalHeight, available / img.naturalWidth); const expected = [img.naturalWidth * scale, img.naturalHeight * scale]; const hugs = Math.abs(box.clientWidth - r.width) <= 1 && Math.abs(box.clientHeight - r.height) <= 1; const inside = r.left >= b.left + left - 0.5 && r.right <= b.right - right + 0.5 && r.top >= b.top + top - 0.5 && r.bottom <= b.bottom - bottom + 0.5; return {{ name: img.alt, natural: [img.naturalWidth, img.naturalHeight], image: [r.width, r.height], wrapper: [b.width, b.height], expected, hugs, inside, sized: Math.abs(r.width - expected[0]) <= 1 && Math.abs(r.height - expected[1]) <= 1, large: img.naturalWidth > 240 || img.naturalHeight > 240 }}; }}) }}; }})()"))?;
        let fits = geometry["width"] == width && geometry["images"].as_array().is_some_and(|images| images.len() == 3 && images.iter().all(|image| image["hugs"] == true && image["inside"] == true && image["sized"] == true && image["large"] == true));
        s.ok(&format!("at {width}px, large wide, tall and square pictures fit their wrappers with no blank band or clipping"), fits, &geometry);
    }
    fit(&mut chrome, &page, 1280, 860, false)?;

    // a reply's file: a chip that downloads it
    say(&mut chrome, "draw me something")?;
    let chip = "[...document.querySelectorAll('.msg.agent a.attachment-name')].find((a) => a.textContent.includes('drawing.txt'))";
    s.ok("a reply's file shows as a chip", shows(&mut chrome, &page, chip), "");
    // no chip is no text: a FAIL below, the checks after it still made
    let drawing = chrome.eval(&page, &format!("(() => {{ const a = {chip}; return a ? fetch(a.getAttribute('href')).then((r) => r.text()) : null; }})()"))?;
    s.ok("which reads as the chat's blob", drawing.as_str().is_some_and(|t| t.contains("a drawing for")), &drawing);

    // a voice memo: the mic records (Chrome's fake microphone) and the message carries the clip
    let mic = "document.getElementById('record')";
    s.ok("an editor's composer has a mic", chrome.eval(&page, &format!("!{mic}.hidden && !{mic}.disabled"))? == true, "");
    let bubbles = |chrome: &mut Browser| chrome.eval(&page, "document.querySelectorAll('.msg.user.mine').length").ok().and_then(|v| v.as_u64()).unwrap_or(0);
    let before = bubbles(&mut chrome);
    // Chrome's fake microphone is allowed from its start (run 40070388501:
    // the page told `granted` recorded at once, and showed no card); a
    // person's browser would ask, so the page is told `prompt` (the fake
    // still answers the request yes, as a person's Allow)
    let origin = chrome.eval(&page, "location.origin")?.as_str().context("the chat's origin")?.to_string();
    chrome.set_permission(&origin, "microphone", "prompt")?;
    chrome.click(&page, "#record")?;
    // the first press says what the mic is for, and asks the browser only on Continue
    let card = "(() => { const c = document.getElementById('mic-card'); return !c.hidden && c.dataset.kind === 'ask' && !document.getElementById('record').classList.contains('on'); })()";
    s.ok("the first press shows the microphone's card, asking nothing of the browser yet", shows(&mut chrome, &page, card), "");
    mic_card_shots(&mut chrome, &page, &shots, "ask")?;
    chrome.click(&page, "#mic-card-go")?;
    let recording = format!("{mic}.classList.contains('on') && !document.getElementById('recording').hidden && !document.getElementById('discard').hidden");
    let on = shows(&mut chrome, &page, &recording);
    std::thread::sleep(Duration::from_millis(800));
    chrome.click(&page, "#discard")?;
    let off = shows(&mut chrome, &page, &format!("!{mic}.classList.contains('on') && document.getElementById('recording').hidden"));
    std::thread::sleep(Duration::from_millis(500));
    let why = if on && off {
        Value::Null
    } else {
        // what the page's microphone and composer said, for the failure
        chrome.eval(&page, "Promise.race([navigator.mediaDevices.getUserMedia({ audio: true }).then((s) => { s.getTracks().forEach((t) => t.stop()); return 'granted'; }, (e) => e.name), new Promise((r) => setTimeout(() => r('no answer in 5 s'), 5000))]).then((mic) => ({ mic, focused: document.hasFocus(), visible: document.visibilityState, banner: document.getElementById('banner').hidden ? null : document.getElementById('banner-text').textContent, record: document.getElementById('record').outerHTML.slice(0, 200) }))")?
    };
    s.ok("pressed, it records, quietly (the mic is its Stop, beside a clock); the x lets the memo go, sending nothing", on && off && bubbles(&mut chrome) == before, json!({ "on": on, "off": off, "why": why }));
    // a microphone the browser refused: the card says where to allow it,
    // and its Okay closes it, recording nothing
    chrome.set_permission(&origin, "microphone", "denied")?;
    chrome.click(&page, "#record")?;
    let refused = format!(
        "(() => {{ const c = document.getElementById('mic-card'); return !c.hidden && c.dataset.kind === 'denied' && document.getElementById('mic-card-title').textContent === 'Your browser needs microphone permissions' && !{mic}.classList.contains('on'); }})()"
    );
    let said = shows(&mut chrome, &page, &refused);
    mic_card_shots(&mut chrome, &page, &shots, "denied")?;
    chrome.click(&page, "#mic-card-go")?;
    let closed = shows(&mut chrome, &page, &format!("document.getElementById('mic-card').hidden && !{mic}.classList.contains('on')"));
    s.ok("a microphone the browser refused shows a card saying where to allow it, which Okay closes, recording nothing", said && closed, json!({ "said": said, "closed": closed }));
    // allowed, a press records at once
    chrome.set_permission(&origin, "microphone", "granted")?;
    chrome.click(&page, "#record")?;
    let ticking = shows(&mut chrome, &page, &format!("{recording} && document.getElementById('mic-card').hidden && document.getElementById('recording-time').textContent !== '0:00'"));
    s.ok("the microphone allowed, a press records at once, with no card", ticking, "");
    chrome.click(&page, "#text")?;
    chrome.type_text(&page, "a voice memo")?;
    chrome.click(&page, "#record")?;
    let memo_shown = "[...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.attachment-audio audio[src^=\"./__blob/\"]') && m.textContent.includes('a voice memo'))";
    s.ok("pressed again, it sends the clip with what was typed, shown as a player", ticking && shows(&mut chrome, &page, memo_shown), chrome.eval(&page, "document.getElementById('messages').innerHTML.slice(-600)")?);
    let memo = posted(s, "a voice memo")["body"]["attachments"].clone();
    let memo_sha = memo[0]["sha256"].as_str().unwrap_or("").to_string();
    s.ok(
        "its record names the clip as an audio attachment, as the recorder made it",
        memo.as_array().is_some_and(|a| a.len() == 1) && memo[0]["type"] == "audio/webm" && memo[0]["name"] == "voice-memo.webm" && memo[0]["size"].as_u64().is_some_and(|n| n > 0),
        &memo,
    );
    let played = chrome.eval(&page, &format!("fetch('./__blob/{memo_sha}').then(async (r) => [r.status, r.headers.get('content-type'), (await r.arrayBuffer()).byteLength])"))?;
    s.ok("the page plays it from the chat's blob, served as audio", played == json!([200, "audio/webm", memo[0]["size"]]), &played);
    s.ok("the agent received it", shows(&mut chrome, &page, &replied("[got 1: voice-memo.webm]")), "");

    // what it looks like: desktop light and dark, and a phone's width with nothing sideways
    chrome.color_scheme(&page, "light")?;
    chrome.screenshot(&page, &shots.join("desktop-light.png"))?;
    chrome.color_scheme(&page, "dark")?;
    chrome.screenshot(&page, &shots.join("desktop-dark.png"))?;
    // the cards, further up
    chrome.eval(&page, "(() => { document.querySelector('.prompt')?.scrollIntoView({ block: 'center' }); return true; })()")?;
    chrome.screenshot(&page, &shots.join("desktop-dark-cards.png"))?;
    chrome.color_scheme(&page, "light")?;
    chrome.screenshot(&page, &shots.join("desktop-light-cards.png"))?;
    chrome.color_scheme(&page, "dark")?;
    fit(&mut chrome, &page, 375, 812, true)?;
    chrome.eval(&page, "(() => { const s = document.getElementById('scroll'); s.scrollTop = s.scrollHeight; return true; })()")?;
    std::thread::sleep(Duration::from_millis(300));
    let widths = chrome.eval(
        &page,
        "(() => { const s = document.getElementById('scroll'); return { inner: innerWidth, page: document.documentElement.scrollWidth, scroll: s.scrollWidth, client: s.clientWidth, composer: document.getElementById('say').getBoundingClientRect().right }; })()",
    )?;
    // a width the page did not answer is NaN, which fits nothing
    let w = |k: &str| widths[k].as_f64().unwrap_or(f64::NAN);
    let fits = w("page") <= w("inner") && w("scroll") <= w("client") && w("composer") <= w("inner");
    s.ok("at 375×812, nothing scrolls sideways", w("inner") == 375.0 && fits, &widths);
    chrome.screenshot(&page, &shots.join("phone-dark.png"))?;
    chrome.color_scheme(&page, "light")?;
    chrome.screenshot(&page, &shots.join("phone-light.png"))?;
    println!("      (screenshots in {})", shots.display());

    // nothing answered twice, and the computer goes back to sleep
    let turns: Vec<String> = records(api, &owner, &chat_name, "work").iter().filter(|r| r["body"]["kind"] == "turn.start").filter_map(|r| r["body"]["turn"].as_str().map(str::to_string)).collect();
    let once: std::collections::BTreeSet<&String> = turns.iter().collect();
    s.ok("every message the page sent was one turn (the one that answered a card, none)", turns.len() == 12 && once.len() == turns.len(), json!(turns));
    let first_turn = turn_of(&agent_name, &chat_name, "chat", sent["seq"].as_i64().unwrap_or(0));
    s.ok("the first of them the turn of the page's first message", turns.first() == Some(&first_turn), json!({ "first": first_turn, "turns": turns }));

    commands_queued_and_quotes(s, api, &mut chrome, &page, Owned { owner: &owner, member_session: &member_session, chat: &chat_name, identity: &identity, label: &label }, &shots)?;

    // files a record names stay past the grace period; an upload none names goes
    let blob = |sha: &str| api.signed(&owner, "HEAD", &format!("/api/f/{chat_name}/blobs/{sha}"), None).map(|r| r.status).unwrap_or(0);
    // a quiet fragment's poll pass, which collects, is a day off: a test lever brings it in
    let poll_now = || api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": chat_name, "op": "poll-now" })));
    let collected = s.eventually(Duration::from_secs(30), || poll_now().is_ok_and(|r| r.status == 200) && blob(&sha) == 404);
    s.ok("an upload no record names is collected after the grace period", collected, blob(&sha));
    let named: Vec<String> = records(api, &owner, &chat_name, "chat")
        .iter()
        .flat_map(|r| r["body"]["attachments"].as_array().cloned().unwrap_or_default())
        .filter_map(|a| a["sha256"].as_str().map(str::to_string))
        .collect();
    let kept: Vec<(String, u16)> = named.iter().map(|sha| (sha.clone(), blob(sha))).collect();
    s.ok(
        "while the files the chat's records name stay (the picture, the agent's drawing, the voice memo)",
        collected && kept.len() >= 3 && kept.iter().all(|(_, status)| *status == 200),
        json!(kept),
    );

    // ---- push (docs/chat-records.md, Push): while the owner's page was on
    // screen, the agent's replies were not pushed to them
    let runs_ended = |s: &Suite| {
        let replies = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).len();
        s.eventually(TURN, || {
            let runs = notify_runs(api, &owner, &chat_name);
            runs.len() == replies && runs.iter().all(|r| r["status"] == "succeeded")
        })
    };
    let ended = runs_ended(s);
    let runs = notify_runs(api, &owner, &chat_name);
    let replies = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity);
    s.ok(
        "each agent reply started the chat's push job once, as the chat itself; no person's record started one",
        ended && runs.len() == replies.len() && runs.iter().all(|r| r["via"] == "channel" && r["trigger"] == "chat"),
        json!({ "runs": runs.len(), "replies": replies.len() }),
    );
    let looked = chrome.eval(&page, "document.visibilityState")?;
    let last = runs.first().and_then(|r| r["id"].as_i64()).unwrap_or(0);
    let last = settle(api, &owner, &chat_name, last, &["succeeded"], TURN);
    s.ok(
        "while the owner's page was on screen (`looking`), none was pushed to them: the job left them out",
        looked == "visible" && s.push.received(&owner_push).is_empty() && last["output"]["to"] == 1 && last["output"]["pushed"] == 0,
        json!({ "visibility": looked, "received": s.push.received(&owner_push), "last": last["output"] }),
    );

    // Native sharing is absent in headless Chrome; only the clipboard
    // boundary is captured so the actual reply button and fallback run.
    chrome.eval(&page, "(() => { Object.defineProperty(navigator, 'share', { configurable: true, value: undefined }); window.__copied = null; navigator.clipboard.writeText = async (text) => { window.__copied = text; }; const b = document.querySelector('.msg.agent .share'); window.__shareText = b.closest('.msg').querySelector('.md').textContent; b.click(); return true; })()")?;
    s.ok("sharing a reply falls back to copying its words in headless Chrome", shows(&mut chrome, &page, "typeof window.__copied === 'string' && window.__copied.length > 0 && document.querySelector('.msg.agent .share').getAttribute('aria-label') === 'Copied'"), chrome.eval(&page, "window.__copied")?);
    let cancelled = chrome.eval(&page, "import('./response-actions.js').then(async (a) => { window.__copied = null; Object.defineProperty(navigator, 'share', { configurable: true, value: async () => { throw new DOMException('Cancelled', 'AbortError'); } }); const result = await a.share({text: 'cancelled', title: 'Chat'}); return result === null && window.__copied === null; })")?;
    s.ok("cancelling a native share sheet is quiet and copies nothing", cancelled == true, "");
    let failed = chrome.eval(&page, "import('./response-actions.js').then(async (a) => { Object.defineProperty(navigator, 'share', { configurable: true, value: async () => { throw new Error('Unavailable'); } }); const result = await a.share({text: 'fallback', title: 'Chat'}); delete navigator.share; return result === 'Copied' && window.__copied === 'fallback'; })")?;
    s.ok("a failed native share falls back to copy too", failed == true, "");

    // renamed while its chat's page is open (its fragment's title, put
    // live), the agent is named so on that page, which no shell frames, as
    // its next turn ends (docs/chat-records.md, "Who is who")
    s.commit(&agent, &[("fragment.json", Some(br#"{ "channels": { "tasks": { "read": "editor", "post": "editor" } }, "meta": { "title": "Juniper Renamed" } }"#))]);
    s.deploy(&agent);
    chrome.front(&page)?;
    say(&mut chrome, "after your rename")?;
    let renamed = shows(
        &mut chrome,
        &page,
        &format!("{} && [...document.querySelectorAll('.msg.agent .who')].pop()?.textContent === 'Juniper Renamed' && document.getElementById('head-label').textContent === 'Juniper Renamed'", replied("after your rename")),
    );
    s.ok(
        "renamed while its chat's page is open, that page (no shell around it) names it so once its next turn ends",
        renamed,
        chrome.eval(&page, "({ by: [...document.querySelectorAll('.msg.agent .who')].pop()?.textContent, head: document.getElementById('head-label').textContent })")?,
    );

    // away from the chat (its page closed), a reply reaches each of its people once
    chrome.close(page)?;
    let member_push = s.name("chat-member-push");
    let anyone = s.name("chat-anyone-push");
    let mine = subscribe(s, api, &chat_name, &member_site, &member_id, &member_push, 29)?;
    let untagged = subscribe(s, api, &chat_name, &member_site, "everyone", &anyone, 31)?;
    s.ok("a viewer subscribes for themselves, and under a tag that names no one", mine.status == 200 && untagged.status == 200, format!("{mine} | {untagged}"));
    let said = "while I am away";
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": "away-1", "body": { "text": said } })))?;
    anyhow::ensure!(r.status == 200, "posting while away: {r}");
    let answered = s.eventually(TURN, || agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).iter().any(|x| x["body"]["text"].as_str().is_some_and(|t| t.contains(said))));
    let reply = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).into_iter().find(|x| x["body"]["text"].as_str().is_some_and(|t| t.contains(said))).unwrap_or_default();
    let pushed = s.eventually(TURN, || !s.push.received(&owner_push).is_empty() && !s.push.received(&member_push).is_empty());
    let ended = runs_ended(s);
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let text = reply["body"]["text"].as_str().unwrap_or("").to_string();
    // titled with its name as renamed above (job.people: its fragment's title)
    let want = json!({ "title": "Juniper Renamed", "body": text, "tag": chat_name, "url": "./" });
    s.ok(
        "away, the agent's reply is pushed once to each of the chat's people: its name, its words, the chat",
        answered && pushed && ended && text.chars().count() <= 120 && s.push.received(&owner_push) == [want.clone()] && s.push.received(&member_push) == [want.clone()],
        json!({ "want": want, "owner": s.push.received(&owner_push), "member": s.push.received(&member_push) }),
    );
    s.ok(
        "nothing for the person's own message or the reply's drafts, nor to a tag that names no person",
        s.push.received(&anyone).is_empty() && notify_runs(api, &owner, &chat_name).len() == agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).len(),
        json!({ "anyone": s.push.received(&anyone) }),
    );
    let r = api.op(&owner, &chat_name, "notify_reply", "by-hand", json!({ "channel": "chat", "record": reply }))?;
    let by_hand = settle(api, &owner, &chat_name, started(&r), &["succeeded", "held"], TURN);
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    s.ok(
        "a member who runs the push job by hand pushes nothing: only its trigger's runs push",
        by_hand["status"] == "succeeded" && by_hand["output"]["pushed"] == 0 && s.push.received(&owner_push).len() == 1,
        &by_hand,
    );
    // its words before a tool call, said while it works, are a reply that
    // is not pushed (`interim`); its answer is
    let said = "tool please, while I am away";
    let r = api.signed(&owner, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": "away-2", "body": { "text": said } })))?;
    anyhow::ensure!(r.status == 200, "posting a tool turn while away: {r}");
    let answered = s.eventually(TURN, || agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).iter().any(|x| x["body"]["text"].as_str().is_some_and(|t| t.contains(said))));
    // every reply's run done (and the one by hand), the interim one's too
    let settled = s.eventually(TURN, || {
        let runs = notify_runs(api, &owner, &chat_name);
        runs.len() == agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).len() + 1 && runs.iter().all(|r| r["status"] == "succeeded")
    });
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let replies = agent_replies(&records(api, &owner, &chat_name, "chat"), &identity);
    let interim = replies.iter().filter(|x| x["body"]["interim"] == true).count();
    let bodies = |to: &str| s.push.received(to).iter().map(|p| p["body"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    let (to_owner, to_member) = (bodies(&owner_push), bodies(&member_push));
    s.ok(
        "away, the agent's words before a tool call (an interim reply) are not pushed; its answer is, once to each of the chat's people",
        answered && settled && interim >= 1 && to_owner.len() == 2 && to_owner[1].contains(said) && to_member == to_owner,
        json!({ "owner": to_owner, "member": to_member, "interim": interim }),
    );
    // (a tab of its own, in front: after the push checks, which ask whether
    // the owner's page is on screen)
    held_socket(s, api, &mut chrome, &owner, &chat_name)?;
    still_loading_in_frame(s, api, &mut chrome, &chat_name)?;
    mic_in_frame(s, api, &mut chrome, &chat_name)?;
    mic_after_access_ends(s, api, &mut chrome, &owner, &member, &member_session)?;
    signed_out_page(s, api, &mut chrome, &owner)?;

    polish::check(s, api, &owner, &owner_session, &mut chrome, &label)?;
    polish::working_card(s, api, &owner, &mut chrome, &identity, &agent_name)?;
    polish::record_shapes(s, api, &owner, &owner_session, &mut chrome)?;
    polish::finished_card_times(s, api, &owner, &mut chrome)?;
    alerts_in_frame(s, api, &mut chrome, &owner, &owner_session, &member_session, &owner_site, &chat_name, &identity)?;

    std::thread::sleep(super::computers::QUEUE_DRAIN);
    api.signed(&owner, "POST", &format!("/api/computers/{computer}/sleep"), Some(&json!({})))?;
    Ok(())
}

/// The chat the commands' checks run in, and who is in it.
struct Owned<'a> {
    owner: &'a Keys,
    member_session: &'a str,
    chat: &'a str,
    identity: &'a str,
    label: &'a str,
}

/// The owner's commands, a message that waits its turn, Steer, and Reply
/// (docs/chat-records.md), on the owner's page against the scripted agent,
/// whose menu has one command of each way the bridge carries one
/// (`images/bridge`, `script::MENU`): `/` offers the agent's commands to its
/// owner alone; one typed whole is a `{kind: "command"}` record, answered
/// as the agent's turn; a message sent while the agent works is marked
/// queued until its turn starts; Steer sends the typed words into the
/// running turn as `/steer`, no turn of their own; Reply quotes a message,
/// above what is typed and above the message sent, its record naming it
/// (`reply_to`), and the agent is handed the quoted words, at a desktop's
/// width and a phone's; a phone's return adds a line and its send button
/// sends, a desktop's Enter sends. Screenshots in the run's scratch
/// (`chat/`).
fn commands_queued_and_quotes(s: &mut Suite, api: &Api, chrome: &mut Browser, page: &Page, o: Owned, shots: &Path) -> Result<()> {
    fit(chrome, page, 1280, 860, false)?;
    chrome.color_scheme(page, "light")?;
    let shows = |chrome: &mut Browser, cond: &str| chrome.until(page, cond, TURN);
    let replied = |text: &str| format!("[...document.querySelectorAll('.msg.agent:not(.streaming) .md')].some((m) => m.textContent.includes({}))", js(text));
    let typed = |chrome: &mut Browser, text: &str| -> Result<()> {
        chrome.click(page, "#text")?;
        chrome.type_text(page, text)
    };
    let chat_records = || records(api, o.owner, o.chat, "chat");
    let starts = |seq: i64| records(api, o.owner, o.chat, "work").iter().filter(|r| r["body"]["kind"] == "turn.start" && r["body"]["cause"]["seq"] == seq).count();

    // the agent's menu, as it posted it in the chat
    let menu = records(api, o.owner, o.chat, "work").into_iter().find(|r| r["body"]["kind"] == "commands").unwrap_or_default();
    s.ok(
        "the agent posted its runtime's commands in the chat, as itself (`commands` on work)",
        menu["principal"] == o.identity && menu["body"]["agent"] == o.identity && menu["body"]["commands"].as_array().is_some_and(|c| c.iter().any(|x| x["name"] == "usage" && x["description"].is_string())),
        &menu,
    );

    // `/` offers them to the agent's owner, as they type
    typed(chrome, "/us")?;
    let offered = shows(chrome, "!document.getElementById('commands').hidden && [...document.querySelectorAll('#commands button')].map((b) => b.dataset.command).join() === 'usage'");
    let _ = chrome.screenshot(page, &shots.join("commands-picker.png"));
    s.ok("typing / offers its owner the agent's commands whose names start so", offered, chrome.eval(page, "document.getElementById('commands').innerText")?);
    chrome.eval(page, "(() => { const t = document.getElementById('text'); t.value = '/'; t.setSelectionRange(1, 1); t.dispatchEvent(new Event('input')); return true; })()")?;
    let all = chrome.eval(page, "[...document.querySelectorAll('#commands button')].map((b) => b.dataset.command)")?;
    s.ok("`/` alone, all of them, each with what it does", all.as_array().is_some_and(|a| a.len() >= 6 && a.iter().any(|c| c == "steer") && a.iter().any(|c| c == "new")), &all);
    let _ = chrome.screenshot(page, &shots.join("commands-picker-all.png"));
    chrome.eval(page, "(() => { const t = document.getElementById('text'); t.value = '/us'; t.setSelectionRange(3, 3); t.dispatchEvent(new Event('input')); return true; })()")?;
    chrome.press(page, "Tab", false)?;
    let completed = chrome.eval(page, "document.getElementById('text').value")?;
    s.ok("Tab writes the one picked", completed == "/usage", &completed);
    chrome.press(page, "Enter", false)?;
    let bubble = "[...document.querySelectorAll('.msg.user.mine .bubble.command')].some((b) => b.textContent === '/usage')";
    s.ok("Enter sends it, shown as the command it is", shows(chrome, bubble), chrome.eval(page, "document.getElementById('messages').innerText.slice(-300)")?);
    // its bubble shows as it is posted (`.sending`), before its record is
    // written: the record is waited for, not read once (Alex's, from #430)
    let mut sent = Value::Null;
    s.eventually(TURN, || {
        sent = chat_records().into_iter().find(|r| r["body"]["kind"] == "command" && r["body"]["command"] == "usage").unwrap_or_default();
        !sent.is_null()
    });
    s.ok("its record is `{kind: \"command\", command, to}`, the agent named", sent["body"] == json!({ "kind": "command", "command": "usage", "to": [o.identity] }), &sent);
    s.ok("and the agent answers it as a turn of its own", shows(chrome, &replied("ran /usage")), "");

    // a viewer is offered none: their `/` is words
    let elsewhere = chrome.another_context()?;
    let base = format!("{}/", api.base);
    chrome.set_cookie_in(&elsewhere, &base, "fragment_session", o.member_session)?;
    let theirs = chrome.open_in(&elsewhere, &api.site_url(o.chat, "__signin?return=/"))?;
    chrome.until(&theirs, "document.getElementById('say')?.dataset.ready === '1' && !!document.querySelector('.bubble.command')", TURN);
    chrome.click(&theirs, "#text")?;
    chrome.type_text(&theirs, "/us")?;
    std::thread::sleep(Duration::from_millis(300));
    s.ok("someone who is not the agent's owner is offered no command", chrome.eval(&theirs, "document.getElementById('commands').hidden")? == true, "");
    chrome.close(theirs)?;

    // a message sent while the agent works waits its turn, marked queued
    typed(chrome, "steer-me now")?;
    chrome.press(page, "Enter", false)?;
    let working = format!("document.getElementById('text').placeholder === {}", js(&format!("{} is working: a message waits its turn, or Steer it now", capital(o.label))));
    s.ok("while its agent works, the composer says a message waits its turn, or Steer it", shows(chrome, &working), chrome.eval(page, "document.getElementById('text').placeholder")?);
    typed(chrome, "and after that")?;
    chrome.press(page, "Enter", false)?;
    let queued = "[...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.bubble')?.textContent === 'and after that' && !!m.querySelector('.queued'))";
    let marked = shows(chrome, queued);
    let _ = chrome.screenshot(page, &shots.join("queued.png"));
    s.ok("a message sent while its agent works shows as queued", marked, chrome.eval(page, "document.getElementById('messages').innerText.slice(-400)")?);

    // Steer: the typed words into the running turn, now
    typed(chrome, "use blue")?;
    let steerable = shows(chrome, "!document.getElementById('steer').hidden && !document.getElementById('steer').disabled");
    let _ = chrome.screenshot(page, &shots.join("steer.png"));
    s.ok("words typed while it works offer Steer beside Send", steerable, "");
    chrome.click(page, "#steer")?;
    s.ok("Steer tells the running turn: it goes on with them", shows(chrome, &replied("(steered: use blue)")), chrome.eval(page, "document.getElementById('messages').innerText.slice(-400)")?);
    let steer = chat_records().into_iter().find(|r| r["body"]["kind"] == "command" && r["body"]["command"] == "steer").unwrap_or_default();
    s.ok(
        "Steer is the owner's `/steer` command, no turn of its own",
        steer["body"]["args"] == "use blue" && steer["seq"].as_i64().is_some_and(|seq| starts(seq) == 0),
        &steer,
    );
    let ran = shows(chrome, &format!("{} && ![...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.bubble')?.textContent === 'and after that' && !!m.querySelector('.queued'))", replied("and after that")));
    s.ok("the queued message runs once the turn ends, and is queued no longer", ran, chrome.eval(page, "document.getElementById('messages').innerText.slice(-400)")?);

    // Reply: the composer quotes a message; the message sent shows it, and the agent is handed it
    let reply_seq = chat_records().into_iter().find(|r| r["principal"] == o.identity && r["body"]["text"] == "ran /usage").and_then(|r| r["seq"].as_i64()).unwrap_or(0);
    chrome.eval(page, &format!("(() => {{ document.querySelector('.msg.agent[data-seq=\"{reply_seq}\"] .actions .reply')?.click(); return true; }})()"))?;
    let quoting = format!("!document.getElementById('quoting').hidden && document.getElementById('quoting-who').textContent === {} && document.getElementById('quoting-text').textContent === 'ran /usage'", js(&format!("Replying to {}", capital(o.label))));
    let shown = shows(chrome, &quoting);
    typed(chrome, "what did that mean")?;
    let _ = chrome.screenshot(page, &shots.join("quote-composer.png"));
    s.ok("Reply on the agent's message quotes it above what is typed", shown, chrome.eval(page, "document.getElementById('quoting').innerText")?);
    chrome.press(page, "Enter", false)?;
    // one wait for both: the message can come back on the live feed before
    // its post answers, and the composer lets the quote go only after that
    let quoted = "document.getElementById('quoting').hidden && [...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.bubble')?.textContent === 'what did that mean' && m.querySelector('.quote .quote-text')?.textContent === 'ran /usage')";
    s.ok("the message sent shows what it quotes, and the composer quotes nothing now", shows(chrome, quoted), "");
    let sent = chat_records().into_iter().find(|r| r["body"]["text"] == "what did that mean").unwrap_or_default();
    s.ok("its record names the message it quotes (`reply_to`, its seq)", sent["body"]["reply_to"] == reply_seq, &sent);
    s.ok("and the agent is handed the quoted words", shows(chrome, &replied("(quoting itself: ran /usage)")), chrome.eval(page, "document.getElementById('messages').innerText.slice(-300)")?);
    let _ = chrome.screenshot(page, &shots.join("quote.png"));

    // a side question: said beside, answered as a message of its own
    typed(chrome, "/btw which file?")?;
    chrome.press(page, "Enter", false)?;
    s.ok("/btw is answered as a message of the agent's own", shows(chrome, &replied("aside: /btw which file?")), "");
    // and on a phone, the page fitted to its visual viewport, its fields 16px
    fit(chrome, page, 375, 812, true)?;
    chrome.eval(page, &format!("(() => {{ document.querySelector('.msg.agent[data-seq=\"{reply_seq}\"] .actions .reply')?.click(); return true; }})()"))?;
    let phone_shown = shows(chrome, &quoting);
    typed(chrome, "and on a phone")?;
    chrome.press(page, "Enter", false)?;
    let phone_quoted = "document.getElementById('quoting').hidden && [...document.querySelectorAll('.msg.user.mine')].some((m) => m.querySelector('.bubble')?.textContent === 'and on a phone' && m.querySelector('.quote .quote-text')?.textContent === 'ran /usage')";
    let phone_sent = shows(chrome, phone_quoted);
    let on_phone = chat_records().into_iter().find(|r| r["body"]["text"] == "and on a phone").unwrap_or_default();
    let phone_handed = shows(chrome, &replied("and on a phone (quoting itself: ran /usage)"));
    let _ = chrome.screenshot(page, &shots.join("quote-phone.png"));
    s.ok(
        "on a phone too, Reply quotes the message, the message sent shows it, its record names it (`reply_to`) and the agent is handed it",
        phone_shown && phone_sent && on_phone["body"]["reply_to"] == reply_seq && phone_handed,
        &on_phone,
    );
    fit(chrome, page, 1280, 860, false)?;

    // a phone's return is a newline, and the send button sends (Paul,
    // 2026-10-14): a touch screen with no finer pointer, at a phone's width
    let composer = "(() => { const t = document.getElementById('text'); return { value: t.value, hint: t.enterKeyHint }; })()";
    let said = |text: &str| chat_records().into_iter().filter(|r| r["body"]["text"] == text).count();
    fit(chrome, page, 375, 812, true)?;
    chrome.touch(page, true)?;
    let labelled = shows(chrome, "document.getElementById('text').enterKeyHint === 'enter'");
    let before = chat_records().len();
    typed(chrome, "a line on a phone")?;
    chrome.press(page, "Enter", false)?;
    chrome.type_text(page, "and another")?;
    std::thread::sleep(Duration::from_secs(2));
    let held = chrome.eval(page, composer)?;
    let unsent = chat_records().len() == before;
    chrome.click(page, "#send")?;
    let lines = "a line on a phone\nand another";
    let posted = shows(chrome, "document.getElementById('text').value === ''") && s.eventually(TURN, || said(lines) == 1);
    let _ = chrome.screenshot(page, &shots.join("phone-return.png"));
    s.ok(
        "on a phone (a touch screen, no finer pointer) the keyboard's return adds a line, labelled so (enterkeyhint enter), and sends nothing; the send button sends the lines",
        labelled && held["value"] == lines && unsent && posted,
        json!({ "held": held, "unsent": unsent, "posted": posted }),
    );
    let _ = shows(chrome, &replied("a line on a phone"));
    // a desktop's: Enter sends, Shift+Enter adds a line
    chrome.touch(page, false)?;
    fit(chrome, page, 1280, 860, false)?;
    let labelled = shows(chrome, "innerWidth === 1280 && document.getElementById('text').enterKeyHint === 'send'");
    typed(chrome, "first of two")?;
    chrome.press(page, "Enter", true)?;
    chrome.type_text(page, "second of two")?;
    let held = chrome.eval(page, composer)?;
    chrome.press(page, "Enter", false)?;
    let posted = shows(chrome, "document.getElementById('text').value === ''") && s.eventually(TURN, || said("first of two\nsecond of two") == 1);
    s.ok("on a desktop Enter sends (enterkeyhint send), and Shift+Enter adds a line", labelled && held["value"] == "first of two\nsecond of two" && posted, json!({ "held": held, "posted": posted }));
    let _ = shows(chrome, &replied("first of two"));
    Ok(())
}

/// The shell, the browser's push service's fake on each of its documents,
/// with `chat` open in its frame and ready: the shell, and its frame's own
/// target; `None` when it never was.
fn framing_shell(s: &mut Suite, api: &Api, chrome: &mut Browser, chat: &str, script: &str) -> Result<Option<(Page, Page)>> {
    let wait = Duration::from_secs(30);
    let shell = chrome.open_with_script(&format!("{}/", api.base), script)?;
    chrome.viewport(&shell, 1280, 860, false)?;
    let row = format!("#chats [data-key={}]", js(&format!("chat:{chat}")));
    let listed = chrome.until(&shell, &format!("!document.getElementById('layout').hidden && !!document.querySelector({})", js(&row)), wait);
    chrome.eval(&shell, &format!("(document.querySelector({})?.click(), true)", js(&row)))?;
    let ready = "document.getElementById('say')?.dataset.ready === '1'";
    if !(listed && s.eventually(wait, || chrome.eval_in_frame(&shell, chat, ready).ok() == Some(Value::Bool(true)))) {
        chrome.close(shell)?;
        return Ok(None);
    }
    let frame = chrome.frame(chat)?;
    Ok(Some((shell, frame)))
}

/// The browser's push service at the shell's subscribe (`PushManager`, the
/// vendor's boundary: headless Chrome reaches none), on each of the shell's
/// documents: a registration's subscription is the push fake's `sub`, kept
/// by its scope with the key it was made with, in the shell's storage
/// (`e2e-push`), so a page loaded again reads it.
fn push_service_fake(sub: &fragment_fakes::push::Subscribed) -> String {
    let sub = json!({ "endpoint": sub.endpoint, "p256dh": sub.p256dh, "auth": sub.auth });
    format!(
        "(() => {{ const SUB = {sub}; const kept = () => JSON.parse(localStorage.getItem('e2e-push') || '{{}}'); const keep = (all) => localStorage.setItem('e2e-push', JSON.stringify(all)); \
         const bytes = (s) => Uint8Array.from(atob(s.replaceAll('-', '+').replaceAll('_', '/')), (c) => c.charCodeAt(0)).buffer; \
         const made = (scope) => ({{ endpoint: SUB.endpoint, getKey: (k) => bytes(SUB[k]), unsubscribe: async () => {{ const all = kept(); delete all[scope]; keep(all); return true; }} }}); \
         Object.defineProperty(ServiceWorkerRegistration.prototype, 'pushManager', {{ configurable: true, get() {{ const scope = this.scope; return {{ \
           subscribe: async (o) => {{ const all = kept(); all[scope] = btoa(String.fromCharCode(...new Uint8Array(o.applicationServerKey))).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', ''); keep(all); return made(scope); }}, \
           getSubscription: async () => (kept()[scope] ? made(scope) : null) }}; }} }}); }})();"
    )
}

/// A chat's alerts, from its bell in the shell's frame (docs/api.md, The
/// shell, "A chat's alerts are the shell's"). A browser lets no
/// cross-origin frame ask for notifications, so the bell of a chat the
/// shell framed only said to open the chat in a tab of its own (Paul,
/// 2026-10-24: alerts should not take a tab). Valid: framed, the bell
/// reads off; pressed, with the browser's notifications allowed to the
/// platform, it says on, and the shell holds a registration of its alerts
/// worker for that person and chat alone, subscribed with the chat's own
/// key; a push delivered to it (DevTools, as a push service would) shows
/// under the chat, whatever its tag said; away, an agent's reply is pushed
/// to that subscription once; a shell opened again reads it on, and
/// pressed, off: the registration gone, and the next reply pushed there
/// nothing. Turned on again and the person gone from the browser (signed
/// out, or another signed in: however the session ended, its cookie alone
/// changed here; Alex's review), a push shows nothing and the worker lets
/// itself go.
#[allow(clippy::too_many_arguments)]
fn alerts_in_frame(s: &mut Suite, api: &Api, chrome: &mut Browser, owner: &Keys, owner_session: &str, member_session: &str, owner_site: &str, chat: &str, identity: &str) -> Result<()> {
    let wait = Duration::from_secs(30);
    let id = s.name("chat-shell-alerts");
    let fake = push_service_fake(&s.push.subscribe(&id, 37));
    let platform = api.base.trim_end_matches('/').to_string();
    let base = format!("{platform}/");
    chrome.set_permission(&platform, "notifications", "granted")?;
    let key = api.call(Call { method: "GET", url: api.site_url(chat, "__push-key"), cookie: Some(owner_site.to_string()), ..Call::default() })?.body["key"].clone();
    let scope = format!("{platform}/__shell/alerts/{}/{chat}/", api.identity(owner)?);
    let bell = "document.getElementById('notify')";
    let held = format!(
        "navigator.serviceWorker.getRegistration({scope}).then((r) => ({{ scope: r?.scope ?? null, worker: (r?.active ?? r?.waiting ?? r?.installing)?.scriptURL ?? null, key: JSON.parse(localStorage.getItem('e2e-push') || '{{}}')[{scope}] ?? null }}))",
        scope = js(&scope)
    );
    let gone_now = format!("navigator.serviceWorker.getRegistration({}).then((r) => !r || r.scope !== {})", js(&scope), js(&scope));
    let push = json!({ "title": "Juniper", "body": "a reply", "tag": "another-chat", "url": "./" }).to_string();
    let Some((shell, frame)) = framing_shell(s, api, chrome, chat, &fake)? else {
        s.ok("framed by the shell, the chat's bell reads off; pressed, on: the shell holds the chat's registration of its alerts worker, subscribed with the chat's own key", false, "the chat never framed");
        return Ok(());
    };
    let read = chrome.until(&frame, &format!("!{bell}.hidden && {bell}.dataset.state === 'off'"), wait);
    let was = chrome.eval(&frame, &format!("{bell}.dataset.state ?? null"))?;
    chrome.eval(&frame, &format!("({bell}.click(), true)"))?;
    let on = chrome.until(&frame, &format!("{bell}.dataset.state === 'on'"), wait);
    let holds = chrome.eval(&shell, &held)?;
    let banner = chrome.eval(&frame, "document.getElementById('banner').hidden ? null : document.getElementById('banner-text').textContent")?;
    s.ok(
        "framed by the shell, the chat's bell reads off; pressed, on: the shell holds the chat's registration of its alerts worker, subscribed with the chat's own key",
        read && on && holds == json!({ "scope": scope, "worker": format!("{platform}/__shell/alerts-sw.js"), "key": key }),
        json!({ "was": was, "holds": holds, "key": key, "banner": banner }),
    );
    // its worker, as a push service would reach it
    let notified = format!("navigator.serviceWorker.getRegistration({}).then((r) => (r ? r.getNotifications() : [])).then((l) => l.map((n) => ({{ title: n.title, body: n.body, tag: n.tag }})))", js(&scope));
    let registration = chrome.registration_id(&shell, &scope)?;
    chrome.deliver_push(&shell, &platform, &registration, &push)?;
    let shown = s.eventually(wait, || chrome.eval(&shell, &notified).ok().is_some_and(|v| v.as_array().is_some_and(|l| !l.is_empty())));
    let seen = chrome.eval(&shell, &notified)?;
    s.ok(
        "a push to it, its person signed in, shows under the chat: its title and words, its tag the chat's, whatever the push said",
        shown && seen == json!([{ "title": "Juniper", "body": "a reply", "tag": chat }]),
        &seen,
    );
    let closing = format!("navigator.serviceWorker.getRegistration({}).then((r) => r.getNotifications()).then((l) => (l.forEach((n) => n.close()), true))", js(&scope));
    chrome.eval(&shell, &closing)?;
    // the platform unable to say who is signed in (its answer refused to
    // the worker): only that something came, and the alerts stay
    let worker = chrome.worker("/__shell/alerts-sw.js")?;
    chrome.block(&worker, &["*/api/identities/me*"])?;
    chrome.deliver_push(&shell, &platform, &registration, &push)?;
    let unsure = json!([{ "title": "Finite", "body": "Something new in one of your chats", "tag": chat }]);
    let kept_back = s.eventually(wait, || chrome.eval(&shell, &notified).ok().as_ref() == Some(&unsure));
    let seen = chrome.eval(&shell, &notified)?;
    let stays = chrome.eval(&shell, &held)?["scope"] == scope.as_str();
    chrome.block(&worker, &[])?;
    chrome.detach(worker)?;
    chrome.eval(&shell, &closing)?;
    s.ok(
        "the platform unable to say who is signed in, a push shows only that something came in a chat, its title and words kept back, and the alerts stay",
        kept_back && stays,
        json!({ "seen": seen, "stays": stays }),
    );
    chrome.detach(frame)?;
    chrome.close(shell)?;

    // away (the shell closed): a reply comes to the shell's subscription
    let said = "alerts while the shell is closed";
    let reply = said_away(s, api, owner, chat, identity, said)?;
    let pushed = s.eventually(TURN, || !s.push.received(&id).is_empty());
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let got = s.push.received(&id);
    s.ok(
        "away, the agent's reply is pushed once to the subscription the shell holds: its words, the chat",
        pushed && got.len() == 1 && got[0]["body"] == reply && got[0]["tag"] == chat,
        json!({ "reply": reply, "received": got }),
    );

    // a shell opened again reads it on; pressed, off
    let Some((shell, frame)) = framing_shell(s, api, chrome, chat, &fake)? else {
        s.ok("a shell opened again reads it on; pressed, off: the registration gone, and the next reply pushed there nothing", false, "the chat never framed");
        return Ok(());
    };
    let again = chrome.until(&frame, &format!("{bell}.dataset.state === 'on'"), wait);
    chrome.eval(&frame, &format!("({bell}.click(), true)"))?;
    let off = chrome.until(&frame, &format!("{bell}.dataset.state === 'off'"), wait);
    let gone = chrome.eval(&shell, &held)?;
    chrome.detach(frame)?;
    chrome.close(shell)?;
    let reply = said_away(s, api, owner, chat, identity, "and once alerts are off")?;
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let got = s.push.received(&id);
    s.ok(
        "a shell opened again reads it on; pressed, off: the registration gone, and the next reply pushed there nothing",
        again && off && gone == json!({ "scope": null, "worker": null, "key": null }) && !reply.is_null() && got.len() == 1,
        json!({ "again": again, "off": off, "gone": gone, "received": got.len() }),
    );

    // on again, then the person gone from this browser (the shell closed
    // first, so nothing of its own answers the change): nothing shown
    let mut left = vec![];
    for (how, session) in [("signed out", ""), ("another person signed in", member_session)] {
        chrome.set_cookie(&base, "fragment_session", owner_session)?;
        let Some((shell, frame)) = framing_shell(s, api, chrome, chat, &fake)? else {
            left.push(json!({ "how": how, "framed": false }));
            continue;
        };
        let read = chrome.until(&frame, &format!("!{bell}.hidden && {bell}.dataset.state === 'off'"), wait);
        let before = chrome.eval(&frame, &format!("({{ state: {bell}.dataset.state ?? null, hidden: {bell}.hidden }})"))?;
        let held_before = chrome.eval(&shell, &held)?;
        if read {
            chrome.eval(&frame, &format!("({bell}.click(), true)"))?;
        }
        let on = read && chrome.until(&frame, &format!("{bell}.dataset.state === 'on'"), wait);
        chrome.detach(frame)?;
        chrome.close(shell)?;
        if !on {
            left.push(json!({ "how": how, "on": false, "read": read, "before": before, "held": held_before }));
            continue;
        }
        chrome.set_cookie(&base, "fragment_session", session)?;
        let quiet = chrome.open(&format!("{platform}/about"))?;
        let registration = chrome.registration_id(&quiet, &scope)?;
        chrome.deliver_push(&quiet, &platform, &registration, &push)?;
        let let_go = chrome.until(&quiet, &gone_now, wait);
        left.push(json!({ "how": how, "on": on, "letGo": let_go }));
        chrome.close(quiet)?;
        chrome.set_cookie(&base, "fragment_session", owner_session)?;
    }
    s.ok(
        "on again, and its person gone from the browser (signed out, or another signed in), a push shows nothing and the worker lets itself go",
        left.len() == 2 && left.iter().all(|l| l["on"] == true && l["letGo"] == true),
        json!(left),
    );

    // a chat that leaves the release's code (Alex's review): its alerts
    // on, then code of its own deployed in the release's place (its
    // manifest naming no template): a push shows nothing, and the worker
    // lets itself go
    let forked = s.named(api, owner, "chat-alerts-fork")?;
    let made = api.create_with(owner, json!({ "name": forked, "template": "chat" }))?;
    anyhow::ensure!(made.status == 200, "making the chat that leaves the release: {made}");
    s.owned(&made.body, owner);
    let fork_scope = format!("{platform}/__shell/alerts/{}/{forked}/", api.identity(owner)?);
    let fork_gone = format!("navigator.serviceWorker.getRegistration({}).then((r) => !r || r.scope !== {})", js(&fork_scope), js(&fork_scope));
    let mut on = false;
    if let Some((shell, frame)) = framing_shell(s, api, chrome, &forked, &fake)? {
        let read = chrome.until(&frame, &format!("!{bell}.hidden && {bell}.dataset.state === 'off'"), wait);
        if read {
            chrome.eval(&frame, &format!("({bell}.click(), true)"))?;
        }
        on = read && chrome.until(&frame, &format!("{bell}.dataset.state === 'on'"), wait);
        chrome.detach(frame)?;
        chrome.close(shell)?;
    }
    s.commit(&made.body, &[("fragment.json", Some(br#"{ "kind": "chat" }"#.as_slice())), ("app.mjs", Some(b"export class App {}\n".as_slice()))]);
    s.deploy(&made.body);
    let own = s.eventually(wait, || api.status(owner, &forked).ok().and_then(|r| r.body["code"]["id"].as_str().map(|id| id.starts_with("app:"))).unwrap_or(false));
    let mut let_go = false;
    if on && own {
        let quiet = chrome.open(&format!("{platform}/about"))?;
        let registration = chrome.registration_id(&quiet, &fork_scope)?;
        chrome.deliver_push(&quiet, &platform, &registration, &push)?;
        let_go = chrome.until(&quiet, &fork_gone, wait);
        chrome.close(quiet)?;
    }
    s.ok(
        "a chat that leaves the release's code (its own deployed in the release's place), a push to the alerts it had shows nothing, and the worker lets itself go",
        on && own && let_go,
        json!({ "on": on, "own": own, "letGo": let_go }),
    );
    Ok(())
}

/// The owner's message posted by the API (no page of theirs open), and
/// the agent's reply's words once it comes (null when none did).
fn said_away(s: &mut Suite, api: &Api, owner: &Keys, chat: &str, identity: &str, said: &str) -> Result<Value> {
    let id = format!("away-{}", Sha256::digest(said.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect::<String>());
    let r = api.signed(owner, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": id, "body": { "text": said } })))?;
    anyhow::ensure!(r.status == 200, "posting while away: {r}");
    let mine = |x: &Value| x["body"]["text"].as_str().is_some_and(|t| t.contains(said));
    let _ = s.eventually(WAKE, || agent_replies(&records(api, owner, chat, "chat"), identity).iter().any(mine));
    Ok(agent_replies(&records(api, owner, chat, "chat"), identity).into_iter().find(mine).map(|x| x["body"]["text"].clone()).unwrap_or(Value::Null))
}

/// A browser's push subscription at the push fake (`id`), stored by the
/// chat's page as the person whose site cookie it carries, tagged `who`.
fn subscribe(s: &Suite, api: &Api, chat: &str, cookie: &str, who: &str, id: &str, seed: u8) -> Result<Reply> {
    let sub = s.push.subscribe(id, seed);
    let body = json!({ "who": who, "endpoint": sub.endpoint, "p256dh": sub.p256dh, "auth": sub.auth });
    api.call(Call {
        method: "POST",
        url: api.site_url(chat, "__push-sub"),
        body: Some(body.to_string().into_bytes()),
        content_type: Some("application/json"),
        cookie: Some(cookie.to_string()),
        ..Call::default()
    })
}

/// The chat's push job's runs, the newest first.
fn notify_runs(api: &Api, owner: &Keys, chat: &str) -> Vec<Value> {
    api.signed(owner, "GET", &format!("/api/f/{chat}/runs?op=notify_reply&limit=200"), None).ok().and_then(|r| r.body["runs"].as_array().cloned()).unwrap_or_default()
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Whether the element `id` is hidden on the page.
fn document_hidden(chrome: &mut Browser, page: &Page, id: &str) -> Result<bool> {
    Ok(chrome.eval(page, &format!("document.getElementById({}).hidden", js(id)))? == Value::Bool(true))
}
