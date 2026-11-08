//! The Telegram connector from Settings (FIN-393; Paul, 2026-11-07: the
//! setup flow in Settings, "just try not to break telegram … for anyone who
//! has it set up. at least make it easy to reconnect by going to
//! settings"), from the computers lane (`telegram_checks`: its stub agent,
//! Telegram's fake), in Chrome, each step's screenshot at 1280 and 390 wide
//! in its scratch (`telegram-settings/`), for a person to see:
//! - `after_a_release`: a connector set up as its page did before its setup
//!   moved (its owner's secrets, `connect`, `pair`, `confirm`, its door
//!   bound), its fragment then at an older release, is the current
//!   release's at its next request and relays, and answers, as before;
//!   Settings shows it its agent's (Open, Set up); its Set up opens its
//!   setup page in the dialog, connected and paired; a token pasted again
//!   there reconnects it, its pairing and its webhook's secret kept; Open
//!   is its chat, just a chat; and where the frame can't sign in (a browser
//!   that keeps no third-party cookie, as Safari 18.5–26.1), the dialog
//!   opens that setup page in a tab.
//! - `from_settings`: an agent with no Telegram chat, Set up makes one,
//!   bound as its door, and its setup in the dialog, step by step: the
//!   token, connected, the pairing link, the account that answered it
//!   confirmed, done; its row then Open; its owner's message from Telegram
//!   reaches the agent. The chat lane runs it again for a Claude Code agent
//!   (`chat::claude_telegram`): a door is any runtime's.
//!
//! The setup page's calls are its own but for where Telegram is: the fake's
//! address in place of api.telegram.org (its secrets' host, `connect`'s
//! `api`), set in its frame's `fetch` before Connect (`to_fake`).

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use fragment_fakes::telegram::{Telegram, BOT_USERNAME};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::computers::agent_replies;
use super::jobs::records;
use crate::api::{Api, Call, Reply};
use crate::browser::{Browser, Page};
use crate::Suite;

/// Its owner's Telegram account, as the fake's updates name it.
const PAUL: i64 = 424_242;
/// The setup page's frame, as its URL names it.
const SETUP: &str = "setup.html";
const WAIT: Duration = Duration::from_secs(30);

/// Who and what the checks are of.
pub(super) struct Connector<'a> {
    pub owner: &'a Keys,
    pub owner_id: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    pub tg: &'a Telegram,
    pub token: &'a str,
    pub wake: Duration,
}

/// Telegram's delivery of `update` to `name`'s inbox, its webhook's secret in its header.
fn deliver(api: &Api, name: &str, update: Value, secret: &str) -> Result<Reply> {
    let url = format!("{}/api/f/{name}/inbox", api.base);
    api.call(Call { method: "POST", url, body: Some(update.to_string().into_bytes()), content_type: Some("application/json"), extra: vec![("x-telegram-bot-api-secret-token", secret.to_string())], ..Call::default() })
}

/// The run a delivery started, once it ends.
fn ran(api: &Api, c: &Connector<'_>, name: &str, r: &Reply) -> Value {
    super::jobs::settle(api, c.owner, name, r.body["runs"][0].as_i64().unwrap_or(0), &["succeeded", "held"], c.wake)
}

/// A message from its owner's account through `name`'s inbox: published
/// there as theirs to the agent, through its door, and the agent's reply,
/// with its words, sent back to Telegram.
fn relays(s: &Suite, api: &Api, c: &Connector<'_>, name: &str, update: i64, text: &str, secret: &str) -> Result<Value> {
    let run = ran(api, c, name, &deliver(api, name, Telegram::message(update, PAUL, "Paul", text), secret)?);
    let record = records(api, c.owner, name, "chat").into_iter().find(|r| r["body"]["text"] == text).unwrap_or_default();
    let sent = s.eventually(c.wake, || c.tg.sent().iter().any(|m| m["chat_id"] == PAUL && m["text"].as_str().is_some_and(|t| t.contains(text))));
    let answered = agent_replies(&records(api, c.owner, name, "chat"), c.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains(text)));
    let ok = run["output"]["relayed"] == true && record["body"]["relayed"] == json!({ "via": "telegram", "for": c.owner_id, "agent": c.agent }) && sent && answered;
    Ok(json!({ "ok": ok, "run": run, "record": record, "sent": sent, "answered": answered }))
}

/// The connector's state, as its owner's call reads it.
fn state(api: &Api, c: &Connector<'_>, name: &str) -> Value {
    api.op(c.owner, name, "state", "state", json!({})).map(|r| r.body["result"].clone()).unwrap_or_default()
}

/// `expr` in the setup page's frame, its own world (Null while it is not there).
fn framed(b: &mut Browser, expr: &str) -> Value {
    b.eval_in_isolated_frame(SETUP, expr).unwrap_or(Value::Null)
}

/// What the setup page says now: its line and its buttons.
const SAID: &str = "({ line: document.querySelector('#setup > p')?.textContent ?? null, buttons: [...document.querySelectorAll('#setup button, #setup .setup-row a')].map((x) => x.textContent), tags: document.querySelectorAll('#setup .setup-tag').length })";

/// The setup page in its frame says `line`, its tags above it.
fn until_line(s: &Suite, b: &mut Browser, line: &str) -> bool {
    s.eventually(WAIT, || {
        let said = framed(b, SAID);
        said["line"] == line && said["tags"] == 2
    })
}

/// Its button (or link) `text` pressed, in its frame.
fn press(b: &mut Browser, text: &str) -> bool {
    framed(b, &format!("(() => {{ const x = [...document.querySelectorAll('#setup button')].find((x) => x.textContent === {text:?}); if (!x) return false; x.click(); return true; }})()")) == true
}

/// The setup page's `fetch`, in its frame's own world, sending to Telegram's
/// fake (`url`) where it would send to api.telegram.org.
fn to_fake(b: &mut Browser, url: &str) -> bool {
    let patch = format!(
        r#"(() => {{
          if (window.__toFake) return true;
          const send = window.fetch.bind(window);
          const body = (init, f) => {{ const b = JSON.parse(init.body); f(b); return {{ ...init, body: JSON.stringify(b) }}; }};
          window.fetch = (u, init) => {{
            const at = String(u);
            if (init?.body && /\/__secrets\//.test(at)) init = body(init, (b) => {{ b.hosts = ["127.0.0.1"]; }});
            if (init?.body && /\/__op\/connect$/.test(at)) init = body(init, (b) => {{ b.input.api = {url:?}; }});
            return send(u, init);
          }};
          window.__toFake = true;
          return true;
        }})()"#
    );
    b.eval_in_isolated_frame(SETUP, &patch).is_ok_and(|v| v == true)
}

/// The bot's token pasted and Connect pressed, in its frame.
fn paste(b: &mut Browser, token: &str) -> bool {
    framed(b, &format!("(() => {{ const i = document.querySelector('#setup input[type=password]'); if (!i) return false; i.value = {token:?}; i.dispatchEvent(new Event('input')); return true; }})()")) == true && press(b, "Connect")
}

/// The page as it is, at 1280 wide and at a phone's 390 (`NN-<name>.png`),
/// `focus` (a selector) in view at each.
fn shoot(b: &mut Browser, page: &Page, dir: &Path, name: &str, focus: Option<&str>) -> Result<()> {
    let center = |b: &mut Browser| {
        if let Some(sel) = focus {
            let _ = b.eval(page, &format!("(document.querySelector({sel:?})?.scrollIntoView({{ block: 'center' }}), true)"));
        }
    };
    center(b);
    std::thread::sleep(Duration::from_millis(400));
    let _ = b.screenshot(page, &dir.join(format!("1280-{name}.png")));
    b.viewport(page, 390, 844, true)?;
    std::thread::sleep(Duration::from_millis(500));
    center(b);
    std::thread::sleep(Duration::from_millis(300));
    let _ = b.screenshot(page, &dir.join(format!("390-{name}.png")));
    b.viewport(page, 1280, 900, false)?;
    std::thread::sleep(Duration::from_millis(300));
    center(b);
    Ok(())
}

/// Settings' Telegram row of `agent`: whether its section and nav link
/// show, its tags and its buttons; null while it is not drawn.
fn row(agent: &str) -> String {
    format!(
        "(() => {{ const s = document.getElementById('settings-telegram'); const go = s?.querySelector('[data-telegram-for={agent:?}]'); const line = go?.closest('.settings-line'); if (!s || s.hidden || !line || line.hidden) return null; return {{ buttons: [...line.querySelectorAll('button')].map((x) => x.textContent), open: line.querySelector('[data-telegram-open]')?.dataset.telegramOpen ?? null, tags: [...s.querySelectorAll('.connector-tag')].map((t) => t.textContent), nav: !document.querySelector('.settings-nav-link[data-section=\"settings-telegram\"]')?.hidden }}; }})()"
    )
}

/// The shell's Settings, signed in as the connector's owner, its Telegram
/// section in view once `agent`'s row is `want` (its buttons).
fn settings(s: &Suite, b: &mut Browser, api: &Api, agent: &str, want: &[&str]) -> Result<(Page, Value)> {
    let page = b.open(&format!("{}/settings", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    let mut seen = Value::Null;
    s.eventually(WAIT, || {
        seen = b.eval(&page, &row(agent)).unwrap_or(Value::Null);
        seen["buttons"] == json!(want)
    });
    let _ = b.eval(&page, "(document.getElementById('settings-telegram')?.scrollIntoView({ block: 'center' }), true)");
    Ok((page, seen))
}

/// Its Set up pressed: the dialog open, its frame its setup page.
fn set_up(s: &Suite, b: &mut Browser, page: &Page, agent: &str) -> Result<Value> {
    b.eval(page, &format!("(document.querySelector('#settings-telegram [data-telegram-for={agent:?}]')?.click(), true)"))?;
    let mut seen = Value::Null;
    s.eventually(WAIT, || {
        seen = b
            .eval(page, "(() => { const d = document.querySelector('dialog.connector-setup[open]'); return d && { title: d.querySelector('h2')?.textContent, agent: d.querySelector('.connector-setup-agent')?.textContent, fragment: d.querySelector('iframe')?.dataset.fragment ?? null, done: [...d.querySelectorAll('.rename-actions button')].map((x) => x.textContent) }; })()")
            .unwrap_or(Value::Null);
        !seen.is_null()
    });
    Ok(seen)
}

/// The checks of a connector set up as its page did before its setup moved
/// to Settings (`name`, bound as `c.agent`'s door, `hook` its webhook's
/// secret): see the module's.
pub(super) fn after_a_release(s: &mut Suite, api: &Api, c: &Connector<'_>, name: &str, hook: &str) -> Result<()> {
    // the platform deployed a release since: its next request installs it
    let stale = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "blessed-stale" })))?;
    anyhow::ensure!(stale.status == 200 && stale.body["stale"] == true, "at an older release: {stale}");
    let relayed = relays(s, api, c, name, 50, "after a release", hook)?;
    let session = api.sign_in(&Api::email_of(c.owner))?;
    let site = format!("fragment_site={}", super::signin::site_cookie(api, &session, name)?);
    let setup = api.page(name, SETUP, Some(&site))?;
    let release = fragment_templates::blessed::release("telegram").unwrap_or("");
    let bound = api.signed(c.owner, "GET", &format!("/api/f/{name}/door"), None)?;
    s.ok(
        "a Telegram connector set up as its page did before (its secrets, connect, pair, confirm, its door bound), a release later, serves that release's setup page, and its owner's message from Telegram reaches its agent through its door, the reply going back",
        relayed["ok"] == true && setup.status == 200 && setup.header("etag").starts_with(&format!("\"b-{release}")) && bound.body["bound"] == json!({ "door": "telegram", "agent": c.agent }),
        json!({ "relayed": relayed, "setup": setup.status, "etag": setup.header("etag"), "release": release, "bound": bound.body }),
    );

    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for Settings' Telegram (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("telegram-settings");
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let (page, seen) = settings(s, &mut b, api, c.agent, &["Open", "Set up"])?;
    s.ok(
        "Settings shows it its agent's Telegram, a word each for what it is, with Open and Set up",
        seen == json!({ "buttons": ["Open", "Set up"], "open": name, "tags": ["Experimental", "Less secure"], "nav": true }),
        &seen,
    );
    shoot(&mut b, &page, &shots, "a1-settings-connected", Some("#settings-telegram"))?;
    let dialog = set_up(s, &mut b, &page, c.agent)?;
    let connected = format!("Connected as @{BOT_USERNAME}, paired with Paul.");
    let shown = until_line(s, &mut b, &connected);
    let said = framed(&mut b, SAID);
    s.ok(
        "its Set up opens its setup page in Settings' dialog, titled, its agent named: connected and paired, with New token and Unpair",
        dialog["title"] == "Telegram" && dialog["fragment"] == name && dialog["done"] == json!(["Done"]) && dialog["agent"].as_str().is_some_and(|a| !a.is_empty()) && shown && said["buttons"] == json!(["New token", "Unpair"]),
        json!({ "dialog": dialog, "said": said }),
    );
    shoot(&mut b, &page, &shots, "a2-dialog-connected", None)?;

    // reconnecting: the token pasted again (the bot stopped, say)
    let before = state(api, c, name);
    let asks = press(&mut b, "New token") && until_line(s, &mut b, "Make a bot with @BotFather, then paste its token.");
    shoot(&mut b, &page, &shots, "a3-reconnect-token", None)?;
    let pasted = to_fake(&mut b, &c.tg.url) && paste(&mut b, c.token);
    let back = until_line(s, &mut b, &connected);
    let after = state(api, c, name);
    let inbox = format!("{}/api/f/{name}/inbox", api.base);
    s.ok(
        "a token pasted there again reconnects it: Telegram's answer comes, its account stays paired, and its webhook is set again with the same secret",
        asks && pasted && back && after["answered"].as_array().is_some_and(|a| a.len() == before["answered"].as_array().map_or(0, Vec::len) + 1 && a.last().and_then(Value::as_str).is_some_and(|x| x.len() == 32)) && after["paired"]["name"] == "Paul" && c.tg.webhook() == Some((inbox.clone(), hook.to_string())),
        json!({ "before": before, "after": after, "webhook": c.tg.webhook() }),
    );
    shoot(&mut b, &page, &shots, "a4-reconnected", None)?;
    let relayed = relays(s, api, c, name, 51, "after reconnecting", hook)?;
    s.ok("reconnected, its owner's message from Telegram still reaches its agent, and the reply goes back", relayed["ok"] == true, &relayed);

    // Done, and Open: its chat, just a chat
    b.eval(&page, "([...document.querySelectorAll('dialog.connector-setup .rename-actions button')].find((x) => x.textContent === 'Done')?.click(), true)")?;
    let closed = b.until(&page, "!document.querySelector('dialog.connector-setup')", WAIT);
    b.eval(&page, &format!("(document.querySelector('#settings-telegram [data-telegram-open={name:?}]')?.click(), true)"))?;
    let site_url = api.site_url(name, "");
    let mut chat = Value::Null;
    s.eventually(WAIT, || {
        chat = b
            .eval_in_isolated_frame(&site_url, "({ ready: document.getElementById('say')?.dataset.ready === '1', setup: !!document.querySelector('.telegram, .setup, .setup-tag, input[type=password]') })")
            .unwrap_or(Value::Null);
        chat["ready"] == true
    });
    let at = b.eval(&page, "location.pathname")?;
    s.ok("Done closes it; Open is its chat, just a chat, no setup in it", closed && chat == json!({ "ready": true, "setup": false }) && at != "/settings", json!({ "chat": chat, "at": at }));
    shoot(&mut b, &page, &shots, "a5-chat", None)?;
    b.close(page)?;

    // where its frame can't sign in (Safari 18.5–26.1 keeps no cookie from
    // it, partitioned or not), its page is refused and says so to the shell
    // (auth.rs `refused`: frames.rs proves a browser that drops it gets
    // that): the dialog says it can't sign in there, and opens that same
    // setup page in a tab
    let (page, _) = settings(s, &mut b, api, c.agent, &["Open", "Set up"])?;
    set_up(s, &mut b, &page, c.agent)?;
    let shown = until_line(s, &mut b, &connected);
    let refused = b.eval_in_isolated_frame(SETUP, &format!("(parent.postMessage({{ fragment: 'signin-blocked', name: {name:?}, why: 'refused' }}, {:?}), true)", api.base)).unwrap_or_default();
    let note = "(() => { const n = document.querySelector('dialog.connector-setup .frame-note'); const a = n?.querySelector('a.allow'); return n && a ? { text: n.innerText.split('\\n')[0], href: a.getAttribute('href'), target: a.target, frame: getComputedStyle(document.querySelector('dialog.connector-setup iframe')).display } : null; })()";
    let mut said = Value::Null;
    s.eventually(WAIT, || {
        said = b.eval(&page, note).unwrap_or(Value::Null);
        !said.is_null()
    });
    s.ok(
        "where its frame can't sign in (its page refused, as Safari 18.5–26.1 leaves it), the dialog says so in its place, and opens its setup page in a tab",
        shown && refused == true && said == json!({ "text": "Telegram can't sign in here", "href": format!("/auth/fragment?name={name}&return=%2Fsetup.html"), "target": "_blank", "frame": "none" }),
        json!({ "refused": refused, "said": said }),
    );
    shoot(&mut b, &page, &shots, "a6-dialog-cannot-sign-in", None)?;
    b.close(page)?;
    Ok(())
}

/// The checks of an agent with no Telegram chat, set up from Settings: see
/// the module's. Its screenshots go in `dir`, in the run's scratch.
pub(super) fn from_settings(s: &mut Suite, api: &Api, c: &Connector<'_>, dir: &str) -> Result<()> {
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for Settings' Telegram (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.scratch.join(dir);
    std::fs::create_dir_all(&shots)?;
    let session = api.sign_in(&Api::email_of(c.owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let (page, seen) = settings(s, &mut b, api, c.agent, &["Set up"])?;
    // no Telegram chat of its at all (the lane's earlier one deleted), so
    // the sidebar shows what Set up makes, and only that
    let telegrams = || -> usize {
        api.signed(c.owner, "GET", "/api/fragments", None).ok().and_then(|r| r.body["fragments"].as_array().map(|l| l.iter().filter(|f| f["title"] == "Telegram").count())).unwrap_or(usize::MAX)
    };
    let none = telegrams();
    s.ok("an agent with no Telegram chat has Set up alone in Settings", seen["buttons"] == json!(["Set up"]) && seen["open"].is_null() && none == 0, json!({ "row": seen, "telegram_chats": none }));
    shoot(&mut b, &page, &shots, "b1-settings-new", Some("#settings-telegram"))?;

    // Set up: a connector made, bound as its door, its setup in the dialog
    let dialog = set_up(s, &mut b, &page, c.agent)?;
    let name = dialog["fragment"].as_str().unwrap_or("").to_string();
    let asks = until_line(s, &mut b, "Make a bot with @BotFather, then paste its token.");
    let bound = api.signed(c.owner, "GET", &format!("/api/f/{name}/door"), None)?;
    let listed = api.signed(c.owner, "GET", "/api/fragments", None)?.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name.as_str()).cloned()).unwrap_or_default();
    let one = telegrams();
    s.ok(
        "its Set up makes one Telegram chat, bound as its agent's door, and opens its setup page in the dialog, asking for the bot's token",
        !name.is_empty() && asks && bound.body["bound"] == json!({ "door": "telegram", "agent": c.agent }) && listed["title"] == "Telegram" && listed["kind"] == "chat" && one == 1,
        json!({ "dialog": dialog, "bound": bound.body, "listed": listed, "telegram_chats": one }),
    );
    shoot(&mut b, &page, &shots, "b2-token", None)?;

    // the token: connected
    let pasted = to_fake(&mut b, &c.tg.url) && paste(&mut b, c.token);
    let connected = until_line(s, &mut b, &format!("Connected as @{BOT_USERNAME}."));
    let said = framed(&mut b, SAID);
    let inbox = format!("{}/api/f/{name}/inbox", api.base);
    let webhook = c.tg.webhook();
    let secret = webhook.as_ref().filter(|(url, _)| *url == inbox).map(|(_, secret)| secret.clone()).unwrap_or_default();
    s.ok(
        "its token pasted there connects the bot: its webhook is this chat's inbox, with a secret of the page's making",
        pasted && connected && said["buttons"] == json!(["Pair your Telegram", "New token"]) && secret.len() == 64,
        json!({ "said": said, "webhook": webhook }),
    );
    shoot(&mut b, &page, &shots, "b3-connected", None)?;

    // pairing: its link, answered in Telegram, confirmed
    let linked = press(&mut b, "Pair your Telegram") && until_line(s, &mut b, &format!("Open @{BOT_USERNAME} in Telegram and tap Start."));
    let link = framed(&mut b, "document.querySelector('#setup .setup-row a')?.href ?? ''").as_str().unwrap_or("").to_string();
    s.ok("Pair shows its link, which opens the bot in Telegram", linked && link.starts_with(&format!("https://t.me/{BOT_USERNAME}?start=")), &link);
    shoot(&mut b, &page, &shots, "b4-pair-link", None)?;
    let nonce = link.rsplit("start=").next().unwrap_or("").to_string();
    let answered = ran(api, c, &name, &deliver(api, &name, Telegram::message(60, PAUL, "Paul", &format!("/start {nonce}")), &secret)?);
    let asked = until_line(s, &mut b, "Is Paul your Telegram?");
    s.ok("the account that answers its link is shown there for its owner to confirm", answered["output"]["proposed"] == true && asked && framed(&mut b, SAID)["buttons"] == json!(["Yes, it's me", "No"]), &answered);
    shoot(&mut b, &page, &shots, "b5-confirm", None)?;
    let done = press(&mut b, "Yes, it's me") && until_line(s, &mut b, &format!("Connected as @{BOT_USERNAME}, paired with Paul."));
    s.ok("confirmed, it says the bot and the account, with New token and Unpair", done && framed(&mut b, SAID)["buttons"] == json!(["New token", "Unpair"]) && state(api, c, &name)["paired"]["name"] == "Paul", framed(&mut b, SAID));
    shoot(&mut b, &page, &shots, "b6-done", None)?;
    // and in dark, as the shell is
    b.color_scheme(&page, "dark")?;
    shoot(&mut b, &page, &shots, "b6-done-dark", None)?;
    b.color_scheme(&page, "light")?;

    // Done: its row Open and Set up; and its owner's word from Telegram reaches the agent
    b.eval(&page, "([...document.querySelectorAll('dialog.connector-setup .rename-actions button')].find((x) => x.textContent === 'Done')?.click(), true)")?;
    let mut seen = Value::Null;
    let opens = s.eventually(WAIT, || {
        seen = b.eval(&page, &row(c.agent)).unwrap_or(Value::Null);
        seen["buttons"] == json!(["Open", "Set up"]) && seen["open"] == name.as_str()
    });
    let closed = b.until(&page, "!document.querySelector('dialog.connector-setup')", WAIT);
    s.ok("Done closes it, and its agent's row is Open and Set up", opens && closed, &seen);
    shoot(&mut b, &page, &shots, "b7-settings-done", Some("#settings-telegram"))?;
    let relayed = relays(s, api, c, &name, 61, "set up from settings", &secret)?;
    s.ok("its owner's message from Telegram reaches its agent through the chat Settings made, and the reply goes back", relayed["ok"] == true, &relayed);
    b.close(page)?;
    Ok(())
}
