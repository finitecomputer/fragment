//! The board template (templates/board): an agent's designs side by side
//! on one canvas, scaffolded and deployed with the CLI, each frame one file
//! written as an agent writes it (at main by `fragment sync` and `fragment
//! write`, never deployed, then `put_frame`). Valid: the manifest lists each
//! frame; a page at each named width renders its content at that width, a
//! picture shows, and a page past the blob threshold (1 MiB, a picture
//! inline) runs its scripts as one under it does; when the agent changes one
//! frame, that frame alone loads again, in the owner's page and in a second
//! viewer's (by the share link), and each keeps its own pan and zoom; a
//! removed frame goes; Done leaves a page in use whose keys stay inside it,
//! on a desktop and a phone; Fit shows frames at the board's far corners; at
//! a phone's width it fits and pinches. Invalid: a frame's scripts reach
//! neither the board nor the viewer's session, cookies or storage, and can't
//! navigate the board (the sandbox); a frame is one file on every board, its
//! sibling stylesheet never loads; the operations' refusals: each field's
//! bounds (the schema), what the app checks, an added viewer's change, the
//! 49th frame. Failure: a load that fails (before its headers, after them,
//! or past 16 MiB, stopped as it streams) keeps the page shown, says why on
//! the frame and is tried again; a file not written yet loads at the same
//! rev once it is; a superseded or removed pending load shows nothing.
//! Replay and restart: a retried put_frame answers its first result and
//! redraws nothing, its id with another input refused; a page that missed a
//! removal (frozen meanwhile) loads the frame put again, with a changed file
//! and with the same; after a deploy drops the pages' sockets, they follow
//! the next change. With `FRAGMENT_E2E_KEEP`, the run keeps its screenshots
//! in `board-media/`.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use crate::api::Api;
use crate::browser::{Browser, Page};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(20);
/// The board's frames (templates/board/app.mjs `FRAMES_MAX`).
const FRAMES_MAX: usize = 48;
/// The size from which a file is a blob (`blobMinBytes`).
const BLOB_MIN_BYTES: usize = 1024 * 1024;
/// The screenshot frame's picture, and the width it is shown at.
const SHOT: (u32, u32) = (320, 200);
const SHOT_SHOWN: u32 = 640;

/// Frame `id`'s page: its title, an input to type in, and a probe of what
/// its scripts reach (a top navigation tried among them), sent to the
/// board's page at each load (the board's test listener keeps it: `HEARD`).
fn frame_page(id: &str, title: &str, extra_head: &str, extra_body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>body {{ margin: 0; font: 16px system-ui; }} h1 {{ margin: 40px; color: rgb(10, 20, 30); }} input {{ margin: 0 40px; }}</style>
{extra_head}
</head>
<body>
<h1>{title}</h1>
<input id="field" placeholder="Type here">
{extra_body}
<script>
addEventListener("load", async () => {{
  // its width as its scripts see it at load, before anything waits
  const width = innerWidth;
  const tried = (f) => {{ try {{ f(); return "reached"; }} catch {{ return "blocked"; }} }};
  let fetched;
  try {{
    const r = await fetch("/__op/frames", {{ method: "POST", credentials: "include", headers: {{ "content-type": "application/json" }}, body: '{{"id":"probe","input":{{}}}}' }});
    fetched = "status " + r.status;
  }} catch {{
    fetched = "blocked";
  }}
  let navigated;
  try {{
    top.location.href = "about:blank#escaped";
    navigated = "tried";
  }} catch {{
    navigated = "blocked";
  }}
  parent.postMessage({{
    board_e2e: {id:?},
    text: document.querySelector("h1").innerText,
    width,
    origin: self.origin,
    color: getComputedStyle(document.querySelector("h1")).color,
    inline: document.getElementById("inline")?.naturalWidth ?? null,
    parent: tried(() => parent.document.title),
    top: tried(() => top.location.href),
    navigated,
    cookie: tried(() => document.cookie),
    storage: tried(() => localStorage.length),
    fetched,
  }}, "*");
}});
</script>
</body>
</html>
"#
    )
}

/// The board page's own listener of its frames' probes, set before its
/// scripts run (`open_with_script`).
const HEARD: &str = "window.__heard = []; addEventListener('message', (e) => { if (e.data && e.data.board_e2e) window.__heard.push(e.data); });";

/// The probes heard from frame `id`, in order.
fn heard(chrome: &mut Browser, page: &Page, id: &str) -> Vec<Value> {
    let v = chrome.eval(page, &format!("JSON.stringify((window.__heard ?? []).filter((m) => m.board_e2e === {id:?}))")).unwrap_or(Value::Null);
    v.as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
}

/// Waits until frame `id` has sent `n` probes, then answers them.
fn heard_n(chrome: &mut Browser, page: &Page, id: &str, n: usize) -> Vec<Value> {
    chrome.until(page, &format!("(window.__heard ?? []).filter((m) => m.board_e2e === {id:?}).length >= {n}"), WAIT);
    heard(chrome, page, id)
}

/// A PNG of `w` × `h`, a gradient, stored without compression (deflate's
/// stored blocks): a screenshot's stand-in that needs no image crate.
fn png(w: u32, h: u32) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    let mut raw = vec![];
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, 160]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        let n = block.len() as u16;
        z.extend(n.to_le_bytes());
        z.extend((!n).to_le_bytes());
        z.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut ihdr = vec![];
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) -> Result<()> {
    let at = dir.join(rel);
    std::fs::create_dir_all(at.parent().expect("a file in a folder"))?;
    std::fs::write(&at, bytes).with_context(|| format!("writing {}", at.display()))
}

/// The board's camera, as its page set it.
fn camera(chrome: &mut Browser, page: &Page) -> String {
    chrome.eval(page, "document.getElementById('world').style.transform").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Every frame the page shows: its rev, the rev it shows, a failed rev and
/// why, whether its iframe is the element marked before (`kept`), and how
/// many iframes or pictures it holds (one, unless a load is under way).
fn shown(chrome: &mut Browser, page: &Page) -> Value {
    let v = chrome
        .eval(page, "JSON.stringify([...document.querySelectorAll('.bd-frame')].map((f) => ({ id: f.dataset.id, rev: f.dataset.rev, loaded: f.dataset.loaded ?? null, failed: f.dataset.failed ?? null, note: f.querySelector('.bd-note').hidden ? null : f.querySelector('.bd-note').textContent, label: f.querySelector('.bd-label b')?.textContent, kept: f.querySelector('iframe')?.dataset.e2e === 'kept', parts: f.querySelectorAll('iframe, img').length })))")
        .unwrap_or(Value::Null);
    v.as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null)
}

/// One frame's row of `shown`.
fn one(chrome: &mut Browser, page: &Page, id: &str) -> Value {
    shown(chrome, page).as_array().and_then(|f| f.iter().find(|f| f["id"] == id).cloned()).unwrap_or(Value::Null)
}

fn shot(chrome: &mut Browser, page: &Page, dir: &Path, name: &str) {
    let _ = chrome.screenshot(page, &dir.join(name));
}

/// Whether every frame in `ids` shows its rev.
fn loaded(ids: &[&str]) -> String {
    let ids = serde_json::to_string(ids).expect("ids encode");
    format!("{ids}.every((id) => {{ const f = document.querySelector(`.bd-frame[data-id=\"${{id}}\"]`); return !!f && f.dataset.loaded === f.dataset.rev; }})")
}

/// Whether frame `id` shows `rev`, from one iframe or picture, with no
/// failure noted.
fn shows(id: &str, rev: &Value) -> String {
    format!("(() => {{ const f = document.querySelector('.bd-frame[data-id=\"{id}\"]'); return !!f && f.dataset.rev === '{rev}' && f.dataset.loaded === '{rev}' && !f.dataset.failed && f.querySelectorAll('iframe, img').length === 1; }})()")
}

/// Whether a rev the page holds as text (`shown`'s) is `rev`.
fn same_rev(text: &Value, rev: &Value) -> bool {
    text.as_str().and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| rev.as_i64() == Some(t))
}

/// Marks every iframe the page shows (`kept` in `shown`).
const MARK: &str = "document.querySelectorAll('.bd-frame iframe').forEach((i) => { i.dataset.e2e = 'kept'; })";

/// Waits until `n` of the page's requests are held, then answers them.
fn held_n(chrome: &mut Browser, page: &Page, n: usize) -> Vec<String> {
    let until = Instant::now() + WAIT;
    loop {
        let held = chrome.held(page).unwrap_or_default();
        if held.len() >= n || Instant::now() > until {
            return held;
        }
        // a round trip to the page lets its requests reach the hold
        let _ = chrome.until(page, "true", WAIT);
    }
}

/// Taps the middle of `selector` with one finger.
fn tap(chrome: &mut Browser, page: &Page, selector: &str) -> Result<()> {
    let at = chrome.eval(page, &format!("(() => {{ const r = document.querySelector({selector:?}).getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()"))?;
    let (x, y) = (at[0].as_f64().unwrap_or(0.0), at[1].as_f64().unwrap_or(0.0));
    chrome.touches(page, "touchStart", &[(x, y)])?;
    chrome.touches(page, "touchEnd", &[])
}

pub fn board(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("board", &[crate::Need::Chrome, crate::Need::Levers]) {
        return Ok(());
    }
    let home = s.dir("board-home");
    s.login(api, &home);
    let owner = s.cli_keys(&home).context("the CLI logged in")?;
    // a board as made: shared by link, not public
    let made = s.cli_json(api, &home, &["create", &s.name("board"), "--json"])?;
    let name = made["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(!name.is_empty(), "no board to test: {made}");
    let dir = s.dir("board").join("designs");
    let dir_s = dir.to_str().expect("utf-8 path").to_string();
    let edits = s.dir("board-edits");
    let out = s.cli(api, &home, &["new", &dir_s, "--template", "board"]);
    s.ok(
        "fragment new scaffolds the board template: its page, its app and its manifest, and no frame",
        out.status.success() && dir.join("site/board.js").is_file() && dir.join("app.mjs").is_file() && dir.join("fragment.json").is_file() && !dir.join("frames").exists() && !dir.join("package.json").exists(),
        String::from_utf8_lossy(&out.stderr),
    );
    let out = s.cli(api, &home, &["deploy", &name, "--dir", &dir_s]);
    s.ok("it deploys as it is", out.status.success(), String::from_utf8_lossy(&out.stderr));
    let st = api.status(&owner, &name)?;
    let ops: Vec<&str> = st.body["code"]["operations"].as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    s.ok("its code installs the board's three operations", st.body["code"]["error"].is_null() && ops == ["frames", "put_frame", "remove_frame"], &st.body["code"]);
    let live = st.body["pins"]["live"].clone();
    let token = st.body["viewToken"].as_str().unwrap_or("").to_string();

    // frames, as an agent makes them: each one file, synced to main (a
    // picture, and pages either side of the blob threshold among them),
    // then put on the board; nothing deployed
    for (id, title) in [("home", "Home, first draft"), ("home-tablet", "Home on a tablet"), ("home-phone", "Home on a phone")] {
        write(&dir, &format!("frames/{id}/index.html"), frame_page(id, title, "", "").as_bytes())?;
    }
    write(&dir, "frames/kit/index.html", frame_page("kit", "A kit of parts", r#"<link rel="stylesheet" href="kit.css">"#, "").as_bytes())?;
    write(&dir, "frames/kit/kit.css", b"h1 { color: rgb(200, 30, 60); }\n")?;
    write(&dir, "frames/shot/shot.png", &png(SHOT.0, SHOT.1))?;
    let small = frame_page("under", "Just under a blob", "", "");
    let under = frame_page("under", "Just under a blob", "", &format!("<!-- {} -->", "x".repeat(BLOB_MIN_BYTES - 4096 - small.len())));
    let inline = base64::engine::general_purpose::STANDARD.encode(png(600, 600));
    let over = frame_page("over", "A picture inline", "", &format!(r#"<img id="inline" alt="" src="data:image/png;base64,{inline}">"#));
    write(&dir, "frames/under/index.html", under.as_bytes())?;
    write(&dir, "frames/over/index.html", over.as_bytes())?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let st = api.status(&owner, &name)?;
    s.ok(
        "the frames' files sync to main, one of them past the blob threshold, and nothing goes live",
        out.status.success() && st.body["pins"]["live"] == live && st.body["pins"]["main"] != live && under.len() < BLOB_MIN_BYTES && over.len() > BLOB_MIN_BYTES,
        json!({ "stderr": String::from_utf8_lossy(&out.stderr), "pins": st.body["pins"], "under": under.len(), "over": over.len() }),
    );
    let puts = [
        json!({ "id": "home", "kind": "html", "label": "Home", "size": "desktop" }),
        json!({ "id": "home-tablet", "kind": "html", "label": "Home", "size": "tablet" }),
        json!({ "id": "home-phone", "kind": "html", "label": "Home", "size": "mobile" }),
        json!({ "id": "kit", "kind": "html", "label": "Kit", "size": "custom", "width": 1000, "height": 600 }),
        json!({ "id": "shot", "kind": "image", "label": "Home today", "file": "shot.png", "width": SHOT_SHOWN }),
        json!({ "id": "under", "kind": "html", "label": "Under 1 MiB", "size": "custom", "width": 800, "height": 400 }),
        json!({ "id": "over", "kind": "html", "label": "Over 1 MiB", "size": "custom", "width": 800, "height": 800 }),
    ];
    let put = |s: &mut Suite, input: &Value| s.cli_json(api, &home, &["call", &name, "put_frame", "--input", &input.to_string(), "--json"]).unwrap_or_else(|e| json!({ "error": format!("{e:#}") }));
    let answers: Vec<Value> = puts.iter().map(|p| put(s, p)).collect();
    let revs: Vec<i64> = answers.iter().map(|a| a["result"]["rev"].as_i64().unwrap_or(0)).collect();
    s.ok(
        "put_frame puts each on the board, each at the board's next rev",
        answers.iter().zip(&puts).all(|(a, p)| a["result"]["id"] == p["id"] && a["result"]["created"] == true && a["replayed"] == false) && revs.windows(2).all(|w| w[1] > w[0]) && revs[0] > 0,
        json!(answers),
    );
    let ids: Vec<&str> = puts.iter().map(|p| p["id"].as_str().unwrap_or("")).collect();
    let listed = s.cli_json(api, &home, &["call", &name, "frames", "--json"])?;
    let rows: Vec<Value> = listed["result"]["frames"].as_array().cloned().unwrap_or_default();
    let row = |id: &str| rows.iter().find(|f| f["id"] == id).cloned().unwrap_or(Value::Null);
    s.ok(
        "the manifest lists them in the order put, each named size at its width",
        rows.iter().map(|f| f["id"].as_str().unwrap_or("")).collect::<Vec<_>>() == ids
            && [("home", 1440, 900), ("home-tablet", 768, 1024), ("home-phone", 390, 844), ("kit", 1000, 600)].iter().all(|(id, w, h)| row(id)["width"] == *w && row(id)["height"] == *h)
            && row("shot")["width"] == SHOT_SHOWN
            && row("shot")["height"].is_null()
            && listed["result"]["max"] == FRAMES_MAX,
        &listed,
    );

    // refusals: each field's bounds (the schema), what the app checks, who
    let refused = |input: Value| -> Result<(u16, String)> {
        let r = api.op(&owner, &name, "put_frame", &format!("bad-{}", crate::api::now_ms()), input)?;
        Ok((r.status, r.message().to_string()))
    };
    let schema: Vec<(u16, String)> = [
        json!({ "id": "a", "kind": "video", "label": "x" }),
        json!({ "id": "a", "kind": "html", "label": "x".repeat(81) }),
        json!({ "id": "a", "kind": "html", "label": "x", "size": "custom", "width": 100 }),
        json!({ "id": "a", "kind": "html", "label": "x", "height": 20000 }),
        json!({ "id": "a", "kind": "html", "label": "x", "colour": "red" }),
        json!({ "id": "a", "kind": "html" }),
    ]
    .into_iter()
    .map(refused)
    .collect::<Result<_>>()?;
    s.ok("each field's bounds are the input schema's: refused before the app runs", schema.iter().all(|(status, _)| *status == 400), json!(schema));
    let app: Vec<(u16, String)> = [
        json!({ "id": "Home", "kind": "html", "label": "x" }),
        json!({ "id": "a", "kind": "html", "label": "x", "file": "../app.mjs" }),
        json!({ "id": "a", "kind": "html", "label": "x", "file": "app.js" }),
        json!({ "id": "a", "kind": "image", "label": "x" }),
        json!({ "id": "a", "kind": "html", "label": "x", "x": 10 }),
        json!({ "id": "a", "kind": "html", "label": "x", "size": "tablet", "width": 800 }),
    ]
    .into_iter()
    .map(refused)
    .collect::<Result<_>>()?;
    let says = ["a-z, 0-9 and -", "none starting with a dot", "one .html file, everything it needs inline", "names its file", "both x and y", "768 wide"];
    s.ok(
        "what a schema can't say the app refuses, saying why: a frame is one HTML file or a picture",
        app.iter().zip(says).all(|((status, message), why)| *status == 422 && message.contains(why)),
        json!(app),
    );
    let viewer = api.person()?;
    let shared = api.share(&name, &fragment_core::npub::encode(viewer.pubkey_hex()), "viewer")?;
    let reads = api.op(&viewer, &name, "frames", "viewer-0", json!({}))?;
    let r = api.op(&viewer, &name, "put_frame", "viewer-1", json!({ "id": "mine", "kind": "html", "label": "Mine" }))?;
    let r2 = api.op(&viewer, &name, "remove_frame", "viewer-2", json!({ "id": "home" }))?;
    s.ok(
        "a viewer of the board reads its frames and changes none of them",
        shared.status == 200 && reads.status == 200 && reads.body["result"]["frames"].as_array().is_some_and(|f| f.len() == ids.len()) && r.status == 403 && r2.status == 403,
        format!("{shared} / {reads} / {r} / {r2}"),
    );

    // the 49th frame
    for i in 0..FRAMES_MAX - ids.len() {
        api.op(&owner, &name, "put_frame", &format!("fill-{i}"), json!({ "id": format!("fill-{i}"), "kind": "html", "label": "Filler" }))?;
    }
    let over_max = api.op(&owner, &name, "put_frame", "fill-over", json!({ "id": "one-more", "kind": "html", "label": "One more" }))?;
    s.ok(&format!("a board holds {FRAMES_MAX} frames: one more is refused, saying so"), over_max.status == 422 && over_max.message().contains(&format!("holds {FRAMES_MAX} frames")), &over_max);
    let mut removed = vec![];
    for i in 0..FRAMES_MAX - ids.len() {
        removed.push(api.op(&owner, &name, "remove_frame", &format!("unfill-{i}"), json!({ "id": format!("fill-{i}") }))?.body["result"]["removed"].clone());
    }
    let again = api.op(&owner, &name, "remove_frame", "unfill-again", json!({ "id": "fill-0" }))?;
    s.ok("removed, each goes; removed again, nothing changes", removed.iter().all(|r| r == 1) && again.body["result"]["removed"] == 0, json!({ "removed": removed, "again": again.body }));

    // the board in Chrome: the owner's page, signed in, and a second
    // viewer's, by the share link
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the board's pages (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let media = s.dir("board-media");
    let session = api.sign_in(&crate::cli_email(&fragment_core::npub::encode(owner.pubkey_hex()))?)?;
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let a = chrome.open_with_script(&api.site_url(&name, "__signin?return=%2F"), HEARD)?;
    chrome.viewport(&a, 1400, 900, false)?;
    let drawn = chrome.until(&a, &format!("location.pathname === '/' && document.querySelectorAll('.bd-frame').length === {} && {}", ids.len(), loaded(&ids)), WAIT);
    s.ok("the owner's board shows every frame of the manifest, each loaded", drawn, json!({ "at": chrome.eval(&a, "location.href")?, "frames": shown(&mut chrome, &a) }));
    let labels = chrome.eval(&a, "[...document.querySelectorAll('.bd-label')].map((l) => l.innerText.replace(/\\s+/g, ' ')).join(' | ')")?;
    s.ok(
        "each frame has its label, and its size",
        labels.as_str().is_some_and(|l| l.contains("Home Desktop 1440") && l.contains("Home Tablet 768") && l.contains("Home Mobile 390") && l.contains("Kit 1000 wide") && l.contains("Home today Image")),
        &labels,
    );
    shot(&mut chrome, &a, &media, "1-board.png");

    let probes: Vec<(&str, u64, &str, Vec<Value>)> = [("home", 1440, "Home, first draft"), ("home-tablet", 768, "Home on a tablet"), ("home-phone", 390, "Home on a phone")]
        .into_iter()
        .map(|(id, w, text)| (id, w, text, heard_n(&mut chrome, &a, id, 1)))
        .collect();
    s.ok(
        "a page at each named width renders its content at that width",
        probes.iter().all(|(_, w, text, h)| h.len() == 1 && h[0]["width"] == *w && h[0]["text"] == *text),
        json!(probes.iter().map(|(id, _, _, h)| json!({ id.to_string(): h })).collect::<Vec<_>>()),
    );
    let sandbox = &probes[0].3[0];
    let still = chrome.eval(&a, &format!("location.pathname === '/' && location.hash === '' && document.querySelectorAll('.bd-frame').length === {}", ids.len()))?;
    s.ok(
        "a frame's scripts run in an opaque origin: they reach neither the board, nor its top, nor this origin's cookies, storage or operations, and navigating the board fails",
        sandbox["origin"] == "null" && sandbox["parent"] == "blocked" && sandbox["top"] == "blocked" && sandbox["cookie"] == "blocked" && sandbox["storage"] == "blocked" && sandbox["fetched"] == "blocked" && still == true,
        json!({ "probe": sandbox, "board_unmoved": still }),
    );
    let picture = chrome.eval(&a, "(() => { const i = document.querySelector('.bd-frame[data-id=\"shot\"] img'); return i ? { complete: i.complete, w: i.naturalWidth, h: i.naturalHeight, shown: i.parentElement.parentElement.offsetWidth, tall: i.parentElement.parentElement.offsetHeight } : null; })()")?;
    s.ok(
        "the screenshot shows, at the width it was given and its own shape",
        picture["complete"] == true && picture["w"] == SHOT.0 && picture["h"] == SHOT.1 && picture["shown"] == SHOT_SHOWN && picture["tall"] == SHOT_SHOWN * SHOT.1 / SHOT.0,
        &picture,
    );
    let (small, big) = (heard_n(&mut chrome, &a, "under", 1), heard_n(&mut chrome, &a, "over", 1));
    s.ok(
        "pages either side of the blob threshold run their scripts alike, the larger's picture inline",
        small.len() == 1 && small[0]["text"] == "Just under a blob" && big.len() == 1 && big[0]["text"] == "A picture inline" && big[0]["inline"] == 600 && big[0]["origin"] == "null",
        json!({ "under": small, "over": big }),
    );
    let kit = heard_n(&mut chrome, &a, "kit", 1);
    s.ok("a frame is one file: its sibling stylesheet never loads", kit.len() == 1 && kit[0]["text"] == "A kit of parts" && kit[0]["color"] == "rgb(10, 20, 30)", json!(kit));

    // the second viewer, by the share link: a camera of its own
    let other = chrome.another_context()?;
    let b = chrome.open_in(&other, &api.site_url(&name, &format!("?view={token}")))?;
    chrome.viewport(&b, 1100, 760, false)?;
    let b_drawn = chrome.until(&b, &format!("location.search === '' && document.querySelectorAll('.bd-frame').length === {} && {}", ids.len(), loaded(&ids)), WAIT);
    s.ok("a viewer with the share link sees the same board", b_drawn, json!({ "at": chrome.eval(&b, "location.href")?, "frames": shown(&mut chrome, &b) }));

    // the owner moves and zooms; then the agent changes one frame
    chrome.eval(
        &a,
        "(() => { const v = document.getElementById('viewport'); v.dispatchEvent(new WheelEvent('wheel', { deltaY: -120, ctrlKey: true, clientX: 500, clientY: 400, bubbles: true, cancelable: true })); v.dispatchEvent(new WheelEvent('wheel', { deltaX: 80, deltaY: 40, clientX: 500, clientY: 400, bubbles: true, cancelable: true })); })()",
    )?;
    let a_camera = camera(&mut chrome, &a);
    let b_camera = camera(&mut chrome, &b);
    chrome.eval(&a, MARK)?;
    chrome.eval(&b, MARK)?;
    let (a_before, b_before) = (shown(&mut chrome, &a), shown(&mut chrome, &b));
    shot(&mut chrome, &b, &media, "2a-viewer-before.png");
    let changed = edits.join("home.html");
    std::fs::write(&changed, frame_page("home", "Home, second draft", "", ""))?;
    let changed_s = changed.to_str().expect("utf-8 path").to_string();
    let out = s.cli(api, &home, &["write", &name, "frames/home/index.html", "--from", &changed_s]);
    let again = put(s, &puts[0]);
    let rev = again["result"]["rev"].clone();
    s.ok("the agent writes the frame's page again and puts it again, at a new rev", out.status.success() && rev.as_i64() > revs.last().copied(), json!({ "write": String::from_utf8_lossy(&out.stderr), "put": again }));
    let a_redrawn = chrome.until(&a, &shows("home", &rev), WAIT);
    let b_redrawn = chrome.until(&b, &shows("home", &rev), WAIT);
    let second = heard_n(&mut chrome, &a, "home", 2);
    let others: Vec<(String, usize)> = ["home-tablet", "home-phone", "kit", "under", "over"].iter().map(|id| (id.to_string(), heard(&mut chrome, &a, id).len())).collect();
    let (a_frames, b_frames) = (shown(&mut chrome, &a), shown(&mut chrome, &b));
    let untouched = |before: &Value, after: &Value| {
        let pick = |v: &Value| v.as_array().map(|f| f.iter().filter(|f| f["id"] != "home").cloned().collect::<Vec<_>>()).unwrap_or_default();
        let (before, after) = (pick(before), pick(after));
        before == after && after.iter().filter(|f| f["id"] != "shot").all(|f| f["kept"] == true)
    };
    s.ok(
        "that frame alone loads again, in both pages, showing the new page",
        a_redrawn && b_redrawn && second.len() == 2 && second[1]["text"] == "Home, second draft" && others.iter().all(|(_, n)| *n == 1) && untouched(&a_before, &a_frames) && untouched(&b_before, &b_frames),
        json!({ "home": second, "others": others, "a": a_frames, "b": b_frames }),
    );
    s.ok(
        "and each page keeps its own pan and zoom",
        camera(&mut chrome, &a) == a_camera && camera(&mut chrome, &b) == b_camera && a_camera != b_camera && !a_camera.ends_with("scale(1)"),
        json!({ "a": [a_camera, camera(&mut chrome, &a)], "b": [b_camera, camera(&mut chrome, &b)] }),
    );
    shot(&mut chrome, &a, &media, "2-home-updated.png");
    shot(&mut chrome, &b, &media, "2b-viewer-after.png");

    // replay: the same call id answers its first result and redraws nothing
    let id = "replay-home";
    let first = s.cli_json(api, &home, &["call", &name, "put_frame", "--id", id, "--input", &puts[0].to_string(), "--json"])?;
    let replay = s.cli_json(api, &home, &["call", &name, "put_frame", "--id", id, "--input", &puts[0].to_string(), "--json"])?;
    let other_input = api.op(&owner, &name, "put_frame", id, json!({ "id": "home", "kind": "html", "label": "Elsewhere" }))?;
    let rev = first["result"]["rev"].clone();
    let stored = s.cli_json(api, &home, &["call", &name, "frames", "--json"])?["result"]["frames"].as_array().and_then(|f| f.iter().find(|f| f["id"] == "home").map(|f| f["rev"].clone()));
    let back = chrome.until(&a, &shows("home", &rev), WAIT);
    s.ok(
        "a retried put_frame is a replay: its first answer, no second write; its id with another input is refused",
        replay["replayed"] == true && replay["result"] == first["result"] && other_input.status == 409 && stored == Some(rev.clone()) && back,
        json!({ "first": first, "replay": replay, "other": other_input.body, "stored": stored }),
    );

    // a load that fails keeps the page shown, says so, and is tried again
    // (the board fitted first, so its screenshot shows the frame whole)
    chrome.click(&a, "#fit")?;
    chrome.eval(&a, MARK)?;
    chrome.hold(&a, &["*/frames/home?r=*"])?;
    let again = put(s, &puts[0]);
    let rev = again["result"]["rev"].clone();
    let held = held_n(&mut chrome, &a, 1);
    let waiting = one(&mut chrome, &a, "home");
    chrome.release(&a, true)?;
    let noted = chrome.until(&a, &format!("document.querySelector('.bd-frame[data-id=\"home\"]').dataset.failed === '{rev}'"), WAIT);
    let failed = one(&mut chrome, &a, "home");
    shot(&mut chrome, &a, &media, "5-failed-load.png");
    let recovered = chrome.until(&a, &shows("home", &rev), WAIT);
    s.ok(
        "a load that fails keeps the page shown, says so on the frame, and is tried again until it loads",
        held.len() == 1 && waiting["kept"] == true && same_rev(&waiting["rev"], &rev) && noted && failed["kept"] == true && failed["note"] == "The board couldn't reach it." && recovered,
        json!({ "held": held, "waiting": waiting, "failed": failed, "after": one(&mut chrome, &a, "home") }),
    );
    // a body that breaks after its headers, and one that never ends: the
    // page's next load of a frame answered so, once (the page's fetch
    // wrapped, as a connection that drops mid-body, or a file that grew
    // past the limit after the route measured it, would answer)
    chrome.eval(
        &a,
        "(() => { window.__pulled = 0; const real = window.fetch; window.fetch = async (url, init) => { const plan = window.__plan; const r = await real(url, init); if (!plan || !String(url).startsWith(`/frames/${plan.id}?`)) return r; window.__plan = null; if (plan.kind === 'drop') { const reader = r.body.getReader(); let sent = false; return new Response(new ReadableStream({ async pull(c) { if (sent) return c.error(new TypeError('the connection dropped')); const { value } = await reader.read(); c.enqueue(value.slice(0, 16)); sent = true; } }), { status: r.status, headers: r.headers }); } r.body.cancel(); return new Response(new ReadableStream({ pull(c) { window.__pulled += 1; c.enqueue(new Uint8Array(1 << 20)); } }), { status: 200, headers: r.headers }); }; })()",
    )?;
    let mut broken = vec![];
    for (kind, why) in [("drop", "The board couldn't reach it."), ("endless", "It is over 16 MiB, a frame's most.")] {
        chrome.eval(&a, MARK)?;
        chrome.eval(&a, &format!("window.__plan = {{ id: 'home', kind: '{kind}' }}"))?;
        let again = put(s, &puts[0]);
        let rev = again["result"]["rev"].clone();
        let noted = chrome.until(&a, &format!("(() => {{ const f = document.querySelector('.bd-frame[data-id=\"home\"]'); return f.dataset.failed === '{rev}' && f.querySelector('.bd-note').textContent === {why:?}; }})()"), WAIT);
        let failed = one(&mut chrome, &a, "home");
        let pulled = chrome.eval(&a, "window.__pulled")?;
        let recovered = chrome.until(&a, &shows("home", &rev), WAIT);
        broken.push(json!({ "kind": kind, "noted": noted, "kept": failed["kept"], "pulled": pulled, "recovered": recovered, "failed": failed }));
    }
    s.ok(
        "a body that breaks after its headers, or runs past 16 MiB (stopped as it streams), keeps the page shown, says why, and is tried again",
        broken.iter().all(|b| b["noted"] == true && b["kept"] == true && b["recovered"] == true) && broken[1]["pulled"].as_u64().is_some_and(|n| (16..=18).contains(&n)),
        json!(broken),
    );
    let later = json!({ "id": "later", "kind": "html", "label": "Later" });
    let later_put = put(s, &later);
    let later_rev = later_put["result"]["rev"].clone();
    let waits = chrome.until(&a, "document.querySelector('.bd-frame[data-id=\"later\"]')?.querySelector('.bd-note')?.textContent === 'No frames/later/index.html yet.'", WAIT);
    let before_file = one(&mut chrome, &a, "later");
    std::fs::write(edits.join("later.html"), frame_page("later", "Written after", "", ""))?;
    let out = s.cli(api, &home, &["write", &name, "frames/later/index.html", "--from", edits.join("later.html").to_str().expect("utf-8 path")]);
    let came = chrome.until(&a, &shows("later", &later_rev), WAIT);
    s.ok(
        "a frame put before its file says so, and loads at the same rev once the file is written",
        waits && same_rev(&before_file["failed"], &later_rev) && before_file["parts"] == 0 && out.status.success() && came && heard_n(&mut chrome, &a, "later", 1).first().is_some_and(|p| p["text"] == "Written after"),
        json!({ "before": before_file, "after": one(&mut chrome, &a, "later") }),
    );

    // public, still one file: the sibling stylesheet never loads
    let out = s.cli(api, &home, &["visibility", &name, "public"]);
    let kit_put = put(s, &puts[3]);
    let kit = heard_n(&mut chrome, &a, "kit", 2);
    s.ok(
        "on a public board too, a frame is one file",
        out.status.success() && kit_put["result"]["created"] == false && kit.len() == 2 && kit[1]["color"] == "rgb(10, 20, 30)",
        json!({ "visibility": String::from_utf8_lossy(&out.stderr), "kit": kit }),
    );

    // pending loads: a superseded one shows nothing, nor one whose frame went
    chrome.hold(&a, &["*/frames/kit?r=*"])?;
    let first = put(s, &puts[3]);
    let held_one = held_n(&mut chrome, &a, 1);
    let second = put(s, &puts[3]);
    let held_two = held_n(&mut chrome, &a, 2);
    chrome.release(&a, false)?;
    let newest = second["result"]["rev"].clone();
    let settled = chrome.until(&a, &format!("{} && !document.querySelector('.bd-frame[data-id=\"kit\"] .next')", shows("kit", &newest)), WAIT);
    s.ok(
        "of two pending loads of a frame, the newer alone is shown",
        held_one.len() == 1 && held_two.len() == 2 && first["result"]["rev"] != newest && settled,
        json!({ "held": held_two, "kit": one(&mut chrome, &a, "kit") }),
    );
    // the page's loads of kit, counted as each settles (before the board's
    // own handler of it runs)
    chrome.eval(&a, "(() => { window.__kitSettled = 0; const real = window.fetch; window.fetch = (url, init) => { const p = real(url, init); if (String(url).startsWith('/frames/kit?')) p.finally(() => { window.__kitSettled += 1; }).catch(() => {}); return p; }; })()")?;
    chrome.hold(&a, &["*/frames/kit?r=*"])?;
    put(s, &puts[3]);
    let held = held_n(&mut chrome, &a, 1);
    let gone = s.cli_json(api, &home, &["call", &name, "remove_frame", "--input", r#"{"id": "kit"}"#, "--json"])?;
    let left = chrome.until(&a, "!document.querySelector('.bd-frame[data-id=\"kit\"]')", WAIT);
    chrome.release(&a, false)?;
    // the released load answered, in full
    let answered = chrome.until(&a, "window.__kitSettled === 1", WAIT);
    let after = shown(&mut chrome, &a);
    let strays = chrome.eval(&a, "document.querySelectorAll('.bd-world > :not(.bd-frame)').length")?;
    s.ok(
        "a pending load of a frame removed meanwhile shows nothing",
        held.len() == 1 && gone["result"]["removed"] == 1 && left && answered && after.as_array().is_some_and(|f| f.iter().all(|f| f["id"] != "kit")) && strays == 0,
        &after,
    );

    // a page that missed a removal: frozen while a frame goes and comes
    // back, with a changed file and with the same; thawed, it loads both
    chrome.freeze(&b, true)?;
    let moved_to = json!({ "id": "home-tablet", "kind": "html", "label": "Home", "size": "tablet", "file": "v2.html" });
    s.cli_json(api, &home, &["call", &name, "remove_frame", "--input", r#"{"id": "home-tablet"}"#, "--json"])?;
    std::fs::write(edits.join("v2.html"), frame_page("home-tablet", "Tablet, version 2", "", ""))?;
    let w1 = s.cli(api, &home, &["write", &name, "frames/home-tablet/v2.html", "--from", edits.join("v2.html").to_str().expect("utf-8 path")]);
    let tablet = put(s, &moved_to);
    s.cli_json(api, &home, &["call", &name, "remove_frame", "--input", r#"{"id": "home-phone"}"#, "--json"])?;
    std::fs::write(edits.join("phone.html"), frame_page("home-phone", "Phone, version 2", "", ""))?;
    let w2 = s.cli(api, &home, &["write", &name, "frames/home-phone/index.html", "--from", edits.join("phone.html").to_str().expect("utf-8 path")]);
    let phone_put = put(s, &puts[2]);
    chrome.freeze(&b, false)?;
    let srcdoc = |id: &str, text: &str| format!("(document.querySelector('.bd-frame[data-id=\"{id}\"] iframe')?.srcdoc ?? '').includes({text:?})");
    let caught = chrome.until(&b, &format!("{} && {} && {} && {}", shows("home-tablet", &tablet["result"]["rev"]), srcdoc("home-tablet", "Tablet, version 2"), shows("home-phone", &phone_put["result"]["rev"]), srcdoc("home-phone", "Phone, version 2")), WAIT);
    s.ok(
        "a page that missed a frame's removal loads it put again, with a changed file and with the same",
        w1.status.success() && w2.status.success() && tablet["result"]["created"] == true && phone_put["result"]["created"] == true && caught,
        json!({ "b": shown(&mut chrome, &b) }),
    );

    // a removed frame goes, from both pages
    let gone = s.cli_json(api, &home, &["call", &name, "remove_frame", "--input", r#"{"id": "shot"}"#, "--json"])?;
    let none = "!document.querySelector('.bd-frame[data-id=\"shot\"]')";
    s.ok("a removed frame disappears from every open board", gone["result"]["removed"] == 1 && chrome.until(&a, none, WAIT) && chrome.until(&b, none, WAIT), &gone);

    // restart: a deploy drops every page's socket; the pages follow on
    std::fs::write(dir.join("README.md"), format!("{}\nDeployed again.\n", std::fs::read_to_string(dir.join("README.md"))?))?;
    let out = s.cli(api, &home, &["deploy", &name, "--dir", &dir_s]);
    let a_camera = camera(&mut chrome, &a);
    let renamed = json!({ "id": "home-phone", "kind": "html", "label": "Home, small", "size": "mobile" });
    let renamed_put = put(s, &renamed);
    let label = "document.querySelector('.bd-frame[data-id=\"home-phone\"] .bd-label b')?.textContent === 'Home, small'";
    s.ok(
        "after a deploy, open boards follow the next change, the camera where it was",
        out.status.success() && renamed_put["result"]["created"] == false && chrome.until(&a, label, WAIT) && chrome.until(&b, label, WAIT) && camera(&mut chrome, &a) == a_camera,
        json!({ "deploy": String::from_utf8_lossy(&out.stderr), "put": renamed_put, "frames": shown(&mut chrome, &a) }),
    );

    // Done: a page in use keeps its keys, Escape included; Done gives the
    // board back
    chrome.click(&a, "#fit")?;
    chrome.click(&a, ".bd-frame[data-id=\"home\"] .bd-shield")?;
    let using = chrome.until(&a, "document.querySelector('.bd-frame[data-id=\"home\"]').classList.contains('on') && !document.getElementById('done').hidden", WAIT);
    let inside = chrome.eval(&a, "(() => { const r = document.querySelector('.bd-frame[data-id=\"home\"] iframe').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()")?;
    chrome.click_at(&a, (inside[0].as_f64().unwrap_or(0.0), inside[1].as_f64().unwrap_or(0.0)))?;
    let focused = chrome.until(&a, "document.activeElement?.tagName === 'IFRAME' && document.activeElement.closest('.bd-frame')?.dataset.id === 'home'", WAIT);
    chrome.press(&a, "Escape", false)?;
    let kept = chrome.until(&a, "document.querySelector('.bd-frame[data-id=\"home\"]').classList.contains('on')", WAIT);
    shot(&mut chrome, &a, &media, "6-in-use-done.png");
    chrome.click(&a, "#done")?;
    let out_again = chrome.until(&a, "!document.querySelector('.bd-frame.on') && document.getElementById('done').hidden", WAIT);
    s.ok("a page in use keeps its keys, Escape among them; Done gives the board back", using && focused && kept && out_again, json!({ "using": using, "focused": focused, "kept": kept, "done": out_again }));

    // a phone: the board fits, nothing overflows, Done by touch, a pinch zooms it
    let phone = chrome.another_context()?;
    let c = chrome.open_in(&phone, &api.site_url(&name, ""))?;
    chrome.viewport(&c, 390, 844, true)?;
    chrome.touch(&c, true)?;
    let now: Vec<String> = s.cli_json(api, &home, &["call", &name, "frames", "--json"])?["result"]["frames"].as_array().map(|f| f.iter().filter_map(|f| f["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let now: Vec<&str> = now.iter().map(String::as_str).collect();
    let fitted = chrome.until(&c, &format!("document.querySelectorAll('.bd-frame').length === {} && {}", now.len(), loaded(&now)), WAIT);
    chrome.eval(&c, "document.getElementById('fit').click()")?;
    let fits_js = "(() => { const r = [...document.querySelectorAll('.bd-body')].map((b) => b.getBoundingClientRect()); const z = document.querySelector('.bd-zoom').getBoundingClientRect(); const labels = [...document.querySelectorAll('.bd-label')].every((l) => l.getBoundingClientRect().right <= l.parentElement.getBoundingClientRect().right + 0.5); return { labels, inside: r.every((b) => b.left >= 0 && b.right <= innerWidth && b.top >= 0 && b.bottom <= innerHeight), overflow: document.documentElement.scrollWidth > innerWidth || document.documentElement.scrollHeight > innerHeight, controls: z.left >= 0 && z.right <= innerWidth && z.bottom <= innerHeight }; })()";
    let fits = chrome.eval(&c, fits_js)?;
    shot(&mut chrome, &c, &media, "3-phone.png");
    s.ok("at a phone's width the board fits every frame on screen, each label within its frame, with nothing overflowing", fitted && fits["labels"] == true && fits["inside"] == true && fits["overflow"] == false && fits["controls"] == true, &fits);
    tap(&mut chrome, &c, ".bd-frame[data-id=\"home\"] .bd-shield")?;
    let using = chrome.until(&c, "document.querySelector('.bd-frame[data-id=\"home\"]').classList.contains('on') && !document.getElementById('done').hidden", WAIT);
    let done_box = chrome.eval(&c, "(() => { const r = document.getElementById('done').getBoundingClientRect(); return r.right <= innerWidth && r.top >= 0 && r.height >= 44; })()")?;
    shot(&mut chrome, &c, &media, "7-phone-done.png");
    tap(&mut chrome, &c, "#done")?;
    let back = chrome.until(&c, "!document.querySelector('.bd-frame.on') && document.getElementById('done').hidden", WAIT);
    s.ok("on a phone, a tapped page is in use, and a tap on Done (in reach, finger-sized) gives the board back", using && done_box == true && back, json!({ "using": using, "done": done_box, "back": back }));
    let before = camera(&mut chrome, &c);
    chrome.touches(&c, "touchStart", &[(150.0, 400.0), (240.0, 400.0)])?;
    for step in 1..=6 {
        let d = 15.0 * step as f64;
        chrome.touches(&c, "touchMove", &[(150.0 - d, 400.0), (240.0 + d, 400.0)])?;
    }
    chrome.touches(&c, "touchEnd", &[])?;
    let pinched = chrome.eval(&c, "Number(/scale\\(([\\d.e-]+)\\)/.exec(document.getElementById('world').style.transform)?.[1] ?? 0)")?;
    let z_before = before.rsplit("scale(").next().and_then(|z| z.trim_end_matches(')').parse::<f64>().ok()).unwrap_or(0.0);
    s.ok("two fingers apart zoom it in", pinched.as_f64().is_some_and(|z| z > z_before * 2.5), json!({ "before": before, "after": pinched }));
    shot(&mut chrome, &c, &media, "4-phone-pinched.png");

    // Fit shows the board's far corners, on a phone and a desktop
    for (id, at) in [("far-a", -100_000), ("far-b", 100_000)] {
        std::fs::write(edits.join(format!("{id}.html")), frame_page(id, "Far away", "", ""))?;
        s.cli(api, &home, &["write", &name, &format!("frames/{id}/index.html"), "--from", edits.join(format!("{id}.html")).to_str().expect("utf-8 path")]);
        put(s, &json!({ "id": id, "kind": "html", "label": "Far", "size": "desktop", "height": 16000, "x": at, "y": at }));
    }
    let mut far = vec![];
    for page in [&c, &a] {
        let both = chrome.until(page, &loaded(&["far-a", "far-b"]), WAIT);
        chrome.eval(page, "document.getElementById('fit').click()")?;
        far.push(json!({ "loaded": both, "fits": chrome.eval(page, fits_js)? }));
    }
    s.ok("Fit shows frames at the board's opposite corners, as tall as a frame may be", far.iter().all(|f| f["loaded"] == true && f["fits"]["inside"] == true && f["fits"]["overflow"] == false), json!(far));
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Goal: the screenshot stand-in is a PNG a browser decodes (checked by
    /// the lane in Chrome); here, its frame: the signature, the header's
    /// size, and every chunk's CRC.
    #[test]
    fn the_stand_in_screenshot_is_a_png() {
        let png = super::png(70, 3000);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[16..24], [0, 0, 0, 70, 0, 0, 0x0b, 0xb8]);
        let mut at = 8;
        let mut kinds = vec![];
        while at < png.len() {
            let n = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            kinds.push(String::from_utf8_lossy(&png[at + 4..at + 8]).to_string());
            at += 12 + n;
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert_eq!(at, png.len());
    }
}
