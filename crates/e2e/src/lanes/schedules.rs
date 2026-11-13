//! The Schedules page (cell/shell/schedules-page.js; Paul, 2026-11-10):
//! every scheduled job across the person's own fragments, read-only. A
//! person of its own, so no other section's fragments are in their list.

use anyhow::{Context, Result};
use serde_json::json;

use super::jobs::jobs_fragment;
use crate::api::Api;
use crate::Suite;

/// An agent fragment, titled, with its routines' channel (templates/agent).
const AGENT_JSON: &[u8] = br#"{ "kind": "agent", "meta": { "title": "Maple" }, "channels": { "tasks": { "read": "editor", "post": "editor" } } }"#;

/// Valid: an agent's routine and an app's cron trigger are both listed,
/// the soonest first, each named (the routine by its text, the cron by its
/// job), with its schedule in words, its next run, and its fragment by its
/// title; a row opens its fragment. Replay: paused, the routine says so and
/// comes last, read again as the page opens again. Restart: a reload at
/// `/schedules` stays there. Screenshots at 1280 and 390, light and dark.
pub fn schedules_ui(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("schedules-ui", &[crate::Need::Fakes, crate::Need::Chrome]) {
        return Ok(());
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the Schedules lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let wait = std::time::Duration::from_secs(30);
    let shots = s.dir("schedules-ui");
    let home = s.dir("schedules-ui-home");
    s.login(api, &home);
    let keys = s.cli_keys(&home).context("the CLI logged in")?;

    // an agent's routine, due next new year
    let agent = s.named(api, &keys, "maple")?;
    let made = s.create(api, &keys, &agent)?;
    s.commit(&made, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&made);
    let routine = json!({ "kind": "routine", "text": "Water the plants\nand the beans", "chat": "none" });
    let r = api.signed(&keys, "POST", &format!("/api/f/{agent}/schedules"), Some(&json!({ "cron": "0 0 1 1 *", "channel": "tasks", "record": routine })))?;
    let id = r.body["schedule"]["id"].as_i64().with_context(|| format!("the routine scheduled: {r}"))?;
    // an app's cron trigger, due tomorrow morning at the latest
    let (app, _) = jobs_fragment(s, api, &keys, "garden", |m| {
        m["meta"] = json!({ "title": "Garden log" });
        m["triggers"] = json!([{ "cron": "0 9 * * *", "run": "tick" }]);
    })?;

    let session = api.sign_in(&crate::cli_email(&fragment_core::npub::encode(keys.pubkey_hex()))?)?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/settings", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    b.until(&page, "!document.getElementById('layout').hidden && !!document.getElementById('schedules-link')?.offsetParent", wait);
    let order = b.eval(&page, "[...document.querySelectorAll('#history-link, #apps-link, #schedules-link, #settings')].map((e) => e.id)")?;
    s.ok("the sidebar's Schedules is below Apps and above Settings", order == json!(["history-link", "apps-link", "schedules-link", "settings"]), &order);
    b.click(&page, "#schedules-link")?;
    let rows = "[...document.querySelectorAll('.schedules-row')].map((r) => ({ name: r.querySelector('.label').textContent, said: r.querySelector('.schedules-said').textContent, fragment: r.querySelector('.schedules-fragment').textContent, next: !!r.querySelector('time')?.dateTime, paused: !!r.querySelector('.schedules-chip') }))";
    let listed = b.until(&page, "location.pathname === '/schedules' && !document.getElementById('schedules-page').hidden && document.querySelectorAll('.schedules-row').length === 2", wait);
    let seen = b.eval(&page, rows)?;
    s.ok(
        "Schedules lists an app's cron and an agent's routine, the soonest first: each its name, its schedule in words, its next run and its fragment's title",
        listed
            && seen
                == json!([
                    { "name": "tick", "said": "Every day at 09:00 UTC", "fragment": "Garden log", "next": true, "paused": false },
                    { "name": "Water the plants", "said": "Every year on January 1 at 00:00 UTC", "fragment": "Maple", "next": true, "paused": false },
                ]),
        &seen,
    );
    for (scheme, name) in [("light", "desktop-1280-light.png"), ("dark", "desktop-1280-dark.png")] {
        b.color_scheme(&page, scheme)?;
        b.screenshot(&page, &shots.join(name))?;
    }
    b.viewport(&page, 390, 844, true)?;
    let fits = b.until(&page, "document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('.schedules-fragment')].every((f) => f.scrollWidth <= f.clientWidth)", wait);
    for (scheme, name) in [("light", "phone-390-light.png"), ("dark", "phone-390-dark.png")] {
        b.color_scheme(&page, scheme)?;
        b.screenshot(&page, &shots.join(name))?;
    }
    s.ok("at a phone's width nothing runs off the page, and each fragment's title is whole", fits, b.eval(&page, rows)?);
    b.color_scheme(&page, "light")?;
    b.viewport(&page, 1280, 900, false)?;
    // the sidebar drawn again at the desktop's width before it is clicked
    // (on a fast node the call below answers before it is)
    b.until(&page, "!!document.getElementById('settings')?.offsetParent", wait);

    // paused, the routine says so and comes last, read as the page opens again
    let r = api.signed(&keys, "PUT", &format!("/api/f/{agent}/schedules/{id}"), Some(&json!({ "paused": true })))?;
    b.click(&page, "#settings")?;
    b.until(&page, "location.pathname === '/settings'", wait);
    b.click(&page, "#schedules-link")?;
    let paused = b.until(&page, "(() => { const r = [...document.querySelectorAll('.schedules-row')]; return r.length === 2 && r[1].querySelector('.schedules-chip')?.textContent === 'Paused' && !r[1].querySelector('time'); })()", wait);
    s.ok("a paused routine says Paused, with no next run, and comes last", r.status == 200 && paused, b.eval(&page, rows)?);

    // a reload at /schedules stays there
    b.reload(&page)?;
    let stays = b.until(&page, "location.pathname === '/schedules' && !document.getElementById('schedules-page').hidden && document.querySelectorAll('.schedules-row').length === 2", wait);
    s.ok("a reload at /schedules opens Schedules again", stays, b.eval(&page, "location.pathname")?);

    // a row opens its fragment
    b.click(&page, ".schedules-row")?;
    let opened = b.until(&page, &format!("!!document.querySelector('#stack .pane[data-key=\"app:{app}\"]')"), wait);
    s.ok("a row opens its fragment", opened, b.eval(&page, "[...document.querySelectorAll('#stack .pane')].map((p) => p.dataset.key)")?);
    Ok(())
}
