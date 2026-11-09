//! SimpleX from Settings (FIN-393; Paul, 2026-11-07: "Go on doing simplex
//! like telegram"), from the computers lane (its stub agent, its `door`
//! commands), in Chrome, each step's screenshot at 1280 and 390 wide, light
//! and dark, in its scratch (`simplex-settings/`), for a person to see:
//! - a SimpleX link made before this release (its owner's ticket, redeemed
//!   on its computer with a door key, its SimpleX chat as its computer
//!   makes one) still relays its owner's words to the agent;
//! - Settings shows the agent's row with Open (its SimpleX chat) and Set
//!   up; Set up opens its computer's link page in the connector dialog, as
//!   Telegram's setup is: linked, then Unlink, made sure of in its own
//!   line, whose ticket closes the door at once; then Link again, its code
//!   to scan, redeemed with a new door key, and linked: the way to
//!   reconnect; Done, and Open is its SimpleX chat.
//!
//! The page is the stub's own (images/bridge `simplex/page.html`), as is
//! every ticket (the platform's, asked through Settings). The stub runs no
//! SimpleX core, so its answers to the page's `state`, `link` and `unlink`
//! are this module's in the frame (`STAND_IN`), as the bridge's would be
//! (images/bridge `simplex::run::View`); the bridge's own are proved
//! against real SimpleX servers (images/bridge tests/docker.rs). Each
//! ticket the page posts is kept there, and redeemed on the computer with
//! the stub's `door link`, as the bridge redeems one.

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::computers::{agent_replies, lever_with};
use super::jobs::records;
use super::telegram_settings::shoot;
use crate::api::Api;
use crate::browser::{Browser, Page};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(30);
/// The link page's frame, as its URL names it.
const AT: &str = "simplex/?agent=";
/// The invitation the stand-in shows, and its QR code as the bridge draws
/// it (`simplex::http::qr_svg` of this link).
const LINK: &str = "https://smp.lab.test/i#e2e-simplex-settings-invitation";
const QR: &str = include_str!("simplex-invitation.svg");
/// Whom the person's SimpleX app is, as the stand-in's linked view says.
const PERSON: &str = "Paul";

/// Who and what the checks are of: the computers lane's agent, `chat` the
/// chat its `door` commands are said in.
pub(super) struct Agent<'a> {
    pub owner: &'a Keys,
    pub computer: &'a str,
    pub agent: &'a str,
    pub chat: &'a str,
    pub identity: &'a str,
    pub wake: Duration,
}

/// The computer's answers to the page, in its frame's own world: `state`
/// as `__sx.view` says, `link` and `unlink` kept with their tickets
/// (`data-sx-tickets`) and answered as the bridge moves (an invitation
/// shown; off), its code's image the bridge's drawing.
fn stand_in(agent: &str) -> String {
    format!(
        r#"(() => {{
          if (window.__sx) return;
          const qr = "data:image/svg+xml," + encodeURIComponent({qr});
          const sx = window.__sx = {{ view: null, tickets: [] }};
          sx.set = (phase, more = {{}}) => {{ sx.view = {{ agent: {agent}, phase, alwaysOn: true, running: true, ...more }}; }};
          const answer = (v) => new Response(JSON.stringify(v), {{ status: 200, headers: {{ "content-type": "application/json" }} }});
          const real = window.fetch.bind(window);
          window.fetch = async (u, init = {{}}) => {{
            const what = new URL(String(u), location.href).pathname.split("/simplex/")[1];
            if (what === "state") return answer(sx.view);
            if (what === "link" || what === "unlink") {{
              sx.tickets.push({{ act: what, ticket: JSON.parse(init.body) }});
              document.documentElement.dataset.sxTickets = JSON.stringify(sx.tickets);
              if (what === "link") sx.set("inviting", {{ link: {link}, short: {link} }});
              else sx.set("off");
              return answer(sx.view);
            }}
            return real(u, init);
          }};
          const drawn = () => {{ for (const i of document.querySelectorAll("img.qr")) if (!i.src.startsWith("data:")) i.src = qr; }};
          new MutationObserver(drawn).observe(document.body, {{ childList: true, subtree: true }});
        }})();"#,
        qr = json!(QR),
        agent = json!(agent),
        link = json!(LINK),
    )
}

/// `code` run in the frame's page's own world (a script of its own: an
/// evaluation's world shares its DOM, not its globals).
fn in_page(b: &mut Browser, page: &Page, code: &str) -> bool {
    let expr = format!("(() => {{ const s = document.createElement('script'); s.textContent = {}; document.documentElement.append(s); s.remove(); return true; }})()", json!(code));
    b.eval_in_frame(page, AT, &expr).is_ok_and(|v| v == true)
}

/// The computer's answer now `phase` (and `more`), and the page's read of it.
fn answers(b: &mut Browser, page: &Page, phase: &str, more: Value) -> bool {
    in_page(b, page, &format!("window.__sx.set({}, {more}); refresh();", json!(phase)))
}

/// What the page says now: its line, its buttons and links, its tags, its
/// code's image drawn.
const SAID: &str = "(() => { const m = document.getElementById('main'); return { line: m?.querySelector(':scope > p')?.textContent ?? null, buttons: [...(m?.querySelectorAll('button, a.button') ?? [])].map((x) => x.textContent), tags: m?.querySelectorAll('.tag').length ?? 0, qr: (() => { const i = m?.querySelector('img.qr'); return !!i && i.complete && i.naturalWidth > 0; })(), box: m?.querySelector('.choice input')?.checked ?? null, tickets: JSON.parse(document.documentElement.dataset.sxTickets ?? '[]') }; })()";

fn now(b: &mut Browser, page: &Page) -> Value {
    b.eval_in_frame(page, AT, SAID).unwrap_or(Value::Null)
}

/// The page says `line`, its tags above it.
fn until_line(s: &Suite, b: &mut Browser, page: &Page, line: &str) -> bool {
    s.eventually(WAIT, || {
        let said = now(b, page);
        said["line"] == line && said["tags"] == 2
    })
}

/// Its button `text` pressed.
fn press(b: &mut Browser, page: &Page, text: &str) -> bool {
    b.eval_in_frame(page, AT, &format!("(() => {{ const x = [...document.querySelectorAll('#main button')].find((x) => x.textContent === {}); if (!x || x.disabled) return false; x.click(); return true; }})()", json!(text))).is_ok_and(|v| v == true)
}

/// Settings' SimpleX row of `agent`: its buttons, its Open's chat, the
/// section's tags and line; null while it is not drawn.
fn row(agent: &str) -> String {
    format!(
        "(() => {{ const s = document.getElementById('settings-simplex'); const go = s?.querySelector('[data-simplex-for={a}]'); const line = go?.closest('.settings-line'); if (!s || s.hidden || !line) return null; return {{ buttons: [...line.querySelectorAll('button')].map((x) => x.textContent), open: line.querySelector('[data-simplex-open]')?.dataset.simplexOpen ?? null, tags: [...s.querySelectorAll('.connector-tag')].map((t) => t.textContent), said: s.querySelector(':scope > p')?.textContent ?? null }}; }})()",
        a = json!(agent)
    )
}

/// Each shot light, then dark (the shell's dark, which it tells the frame).
fn shoot_both(b: &mut Browser, page: &Page, dir: &std::path::Path, name: &str, focus: Option<&str>) -> Result<()> {
    shoot(b, page, dir, name, focus)?;
    b.color_scheme(page, "dark")?;
    shoot(b, page, dir, &format!("{name}-dark"), focus)?;
    b.color_scheme(page, "light")?;
    Ok(())
}

/// The checks (see the module's); the agent's replies in `c.chat` they add.
pub(super) fn checks(s: &mut Suite, api: &Api, c: &Agent<'_>, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut replies = 0;
    let door = format!("/api/computers/{}/agents/{}/door", c.computer, c.agent);
    let ticket = |act: &str| api.signed(c.owner, "POST", &door, Some(&json!({ "door": "simplex", "act": act })));
    let echoed = |text: &str| agent_replies(&records(api, c.owner, c.chat, "chat"), c.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.starts_with("echo: ") && t.ends_with(&format!("] {text}"))));
    // its owner's words through the door (the stub's `door say`, as the
    // bridge relays the person's SimpleX message), answered by the agent
    let relays = |s: &Suite, n: u32, text: &str| -> Result<(String, bool)> {
        let said = fetched(s, n, &format!("door say {text}"))?;
        let answered = said.starts_with("door said ") && s.eventually(c.wake, || echoed(text));
        Ok((said, answered))
    };

    // a link made before this release, as its page made it then: its
    // owner's ticket, redeemed on the computer with a door key; and its
    // SimpleX chat, as the bridge makes one as it links
    let t = ticket("link")?;
    anyhow::ensure!(t.status == 200, "its owner's link ticket: {t}");
    let linked = fetched(s, 460, &format!("door link {}", t.body))?;
    replies += 1;
    anyhow::ensure!(linked == "door linked", "its link, made as before: {linked}");
    let label = c.agent.rsplit_once("--").map_or(c.agent, |(l, _)| l);
    let made = api.create_with(c.owner, json!({ "label": format!("{label}-simplex"), "template": "chat", "title": "SimpleX" }))?;
    anyhow::ensure!(made.status == 200, "its SimpleX chat: {made}");
    s.owned(&made.body, c.owner);
    let sx = made.body["name"].as_str().unwrap_or("").to_string();
    let joined = api.signed(c.owner, "PUT", &format!("/api/f/{sx}/members/{}", c.identity), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(joined.status == 200, "its agent in its SimpleX chat: {joined}");
    // its computer's image does SimpleX, and stays on (the levers, as its
    // bridge and its seat would say)
    for op in ["simplex", "always-on"] {
        let r = lever_with(api, c.computer, json!({ "op": op, "on": true }))?;
        anyhow::ensure!(r.status == 200, "the {op} lever: {r}");
    }
    let (said, answered) = relays(s, 461, "linked before the release")?;
    replies += 1 + usize::from(answered);
    s.ok("a SimpleX link made before this release (its owner's ticket, redeemed on its computer with a door key) still relays its owner's words, which its agent answers", answered, &said);

    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for Settings' SimpleX (set CHROME_BIN)", false, "no Chrome found");
        return Ok(replies);
    };
    let shots = s.dir("simplex-settings");
    let session = api.sign_in(&Api::email_of(c.owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/settings", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    let mut seen = Value::Null;
    s.eventually(WAIT, || {
        seen = b.eval(&page, &row(c.agent)).unwrap_or(Value::Null);
        seen["buttons"] == json!(["Open", "Set up"])
    });
    s.ok(
        "Settings shows the agent's SimpleX, a word each for what it is, with Open (its SimpleX chat) and Set up",
        seen == json!({ "buttons": ["Open", "Set up"], "open": sx, "tags": ["Experimental", "Secure"], "said": "Message your agents from SimpleX Chat." }),
        &seen,
    );
    shoot_both(&mut b, &page, &shots, "a1-settings", Some("#settings-simplex"))?;

    // Set up: its computer's link page in the dialog
    b.eval(&page, &format!("(document.querySelector('#settings-simplex [data-simplex-for={}]')?.click(), true)", json!(c.agent)))?;
    let mut dialog = Value::Null;
    s.eventually(WAIT, || {
        dialog = b
            .eval(&page, "(() => { const d = document.querySelector('dialog.connector-setup[open]'); const f = d?.querySelector('iframe'); return d && f && { title: d.querySelector('h2')?.textContent, agent: d.querySelector('.connector-setup-agent')?.textContent, simplex: f.dataset.simplex ?? null, tall: f.classList.contains('tall'), done: [...d.querySelectorAll('.rename-actions button')].map((x) => x.textContent) }; })()")
            .unwrap_or(Value::Null);
        !dialog.is_null()
    });
    let name = dialog["agent"].as_str().unwrap_or("").to_string();
    // the stub's own answer first: it runs no SimpleX core
    let stub = until_line(s, &mut b, &page, "SimpleX isn't on this computer.");
    let stood = in_page(&mut b, &page, &stand_in(c.agent)) && answers(&mut b, &page, "linked", json!({ "name": PERSON }));
    let shown = until_line(s, &mut b, &page, &format!("Linked to {PERSON}."));
    let shown_now = now(&mut b, &page);
    s.ok(
        "its Set up opens its computer's link page in Settings' dialog, titled, its agent named: the stub's own word that it runs no SimpleX, then, as a computer running SimpleX answers, linked, whom to, with Unlink",
        dialog["title"] == "SimpleX" && dialog["simplex"] == c.agent && dialog["tall"] == true && dialog["done"] == json!(["Done"]) && !name.is_empty() && stub && stood && shown && shown_now["buttons"] == json!(["Unlink"]),
        json!({ "dialog": dialog, "said": shown_now }),
    );
    shoot_both(&mut b, &page, &shots, "a2-dialog-linked", None)?;

    // reconnecting: Unlink, made sure of in its own line, then Link again
    let sure = press(&mut b, &page, "Unlink") && until_line(s, &mut b, &page, &format!("Unlink {name}? Your SimpleX app will show the chat as deleted."));
    let asked = now(&mut b, &page);
    shoot_both(&mut b, &page, &shots, "a3-unlink-sure", None)?;
    let unlinked = press(&mut b, &page, "Unlink") && until_line(s, &mut b, &page, &format!("Link {name} to SimpleX Chat on your phone."));
    let off = now(&mut b, &page);
    let given = &off["tickets"][0];
    let closed = fetched(s, 462, "door say after the unlink")?;
    replies += 1;
    s.ok(
        "Unlink asks again in its own line (Unlink, Keep it); then its unlink ticket, the platform's through Settings, is posted to its computer and closes its door at once, and it offers Link SimpleX",
        sure && asked["buttons"] == json!(["Unlink", "Keep it"]) && unlinked && given["act"] == "unlink" && given["ticket"]["act"] == "unlink" && given["ticket"]["agent"] == c.agent && given["ticket"]["computer"] == c.computer && closed.starts_with("door refused: 403") && off["buttons"] == json!(["Link SimpleX"]) && off["box"] == true,
        json!({ "asked": asked, "off": off, "closed": closed }),
    );
    shoot_both(&mut b, &page, &shots, "a4-link", None)?;
    // (its reminders left where they are: shell.rs proves the box's home)
    b.eval_in_frame(&page, AT, "(() => { const i = document.querySelector('#main .choice input'); i.checked = false; i.dispatchEvent(new Event('change')); return true; })()")?;
    let inviting = press(&mut b, &page, "Link SimpleX") && until_line(s, &mut b, &page, "Scan this in SimpleX Chat on your phone.") && s.eventually(WAIT, || now(&mut b, &page)["qr"] == true);
    let invite = now(&mut b, &page);
    let link = invite["tickets"].as_array().and_then(|t| t.last()).cloned().unwrap_or_default();
    shoot_both(&mut b, &page, &shots, "a5-scan", None)?;
    let redeemed = fetched(s, 463, &format!("door link {}", link["ticket"]))?;
    replies += 1;
    let opened = answers(&mut b, &page, "linked", json!({ "name": PERSON })) && until_line(s, &mut b, &page, &format!("Linked to {PERSON}."));
    shoot_both(&mut b, &page, &shots, "a6-linked-again", None)?;
    let (said_again, answered) = relays(s, 464, "linked again from settings")?;
    replies += 1 + usize::from(answered);
    s.ok(
        "Link SimpleX posts its link ticket, the platform's through Settings, and shows its code to scan, Open in SimpleX and Copy link; redeemed on its computer with a new door key and answered by the person's app, it says linked, and its owner's words relay again",
        inviting && invite["buttons"] == json!(["Open in SimpleX", "Copy link", "Cancel"]) && link["act"] == "link" && link["ticket"]["agent"] == c.agent && redeemed == "door linked" && opened && answered,
        json!({ "invite": invite, "redeemed": redeemed, "said": said_again }),
    );

    // Done, and Open: its SimpleX chat
    b.eval(&page, "([...document.querySelectorAll('dialog.connector-setup .rename-actions button')].find((x) => x.textContent === 'Done')?.click(), true)")?;
    let done = b.until(&page, "!document.querySelector('dialog.connector-setup')", WAIT);
    b.eval(&page, &format!("(document.querySelector('#settings-simplex [data-simplex-open={}]')?.click(), true)", json!(sx)))?;
    let at = b.until(&page, &format!("location.pathname !== '/settings' && !!document.querySelector('iframe[data-fragment={}]')", json!(sx)), WAIT);
    s.ok("Done closes it, and Open is its SimpleX chat", done && at, b.eval(&page, "location.pathname")?);
    b.close(page)?;

    // as the lane goes on: its door closed, no SimpleX chat, no SimpleX, its computer let sleep
    let r = ticket("unlink")?;
    anyhow::ensure!(r.status == 200, "its door closed: {r}");
    let gone = api.signed(c.owner, "DELETE", &format!("/api/f/{sx}"), None)?;
    anyhow::ensure!(matches!(gone.status, 200 | 204), "its SimpleX chat deleted: {gone}");
    for op in ["simplex", "always-on"] {
        let r = lever_with(api, c.computer, json!({ "op": op, "on": false }))?;
        anyhow::ensure!(r.status == 200, "the {op} lever: {r}");
    }
    Ok(replies)
}
