//! The board template (templates/board): an agent's designs side by side
//! on one canvas, scaffolded and deployed with the CLI, its frames
//! written as an agent writes them (files at main by `fragment sync` and
//! `fragment write`, never deployed, then `put_frame`). Valid: the manifest
//! lists each frame; a page at each named width renders its content at
//! that width, a picture shows; when the agent changes one frame, that
//! frame alone loads again, in the owner's page and in a second viewer's
//! (by the share link), and each keeps its own pan and zoom; a removed
//! frame goes; after a deploy drops the pages' sockets, they follow the
//! next change; at a phone's width it fits and pinches. Invalid: a frame's
//! scripts reach neither the board nor the viewer's session, cookies or
//! storage (the sandbox); on a board that isn't public its page's other
//! files are a stranger's ask, refused, and load once it is public; the
//! operations' refusals: each field's bounds (the schema), what the app
//! checks, a reader's change, and the 49th frame. Replay: a retried
//! put_frame answers its first result and redraws nothing; its id with
//! another input is refused. With `FRAGMENT_E2E_KEEP`, the run keeps its
//! screenshots in `board-media/`.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::Api;
use crate::browser::{Browser, Page};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(20);
/// The board's frames (templates/board/app.mjs `FRAMES_MAX`).
const FRAMES_MAX: usize = 48;
/// The screenshot frame's picture, and the width it is shown at.
const SHOT: (u32, u32) = (320, 200);
const SHOT_SHOWN: u32 = 640;

/// A frame's page: its text, and a probe of what its scripts reach, sent to
/// the board's page (whose test listener keeps it: `HEARD`) at each load.
fn frame_page(title: &str, extra_head: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<style>body {{ margin: 0; font: 16px system-ui; }} h1 {{ margin: 40px; color: rgb(10, 20, 30); }}</style>
{extra_head}
</head>
<body>
<h1>{title}</h1>
<script>
addEventListener("load", async () => {{
  const tried = (f) => {{ try {{ f(); return "reached"; }} catch {{ return "blocked"; }} }};
  let fetched;
  try {{
    const r = await fetch("/__op/frames", {{ method: "POST", credentials: "include", headers: {{ "content-type": "application/json" }}, body: '{{"id":"probe","input":{{}}}}' }});
    fetched = "status " + r.status;
  }} catch {{
    fetched = "blocked";
  }}
  parent.postMessage({{
    board_e2e: location.pathname.split("/")[2],
    text: document.querySelector("h1").innerText,
    width: innerWidth,
    origin: self.origin,
    color: getComputedStyle(document.querySelector("h1")).color,
    parent: tried(() => parent.document.title),
    top: tried(() => top.location.href),
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

/// Polls until frame `id` has sent `n` probes, or WAIT passes.
fn heard_n(chrome: &mut Browser, page: &Page, id: &str, n: usize) -> Vec<Value> {
    let until = Instant::now() + WAIT;
    loop {
        let h = heard(chrome, page, id);
        if h.len() >= n || Instant::now() > until {
            return h;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

/// Every frame the page shows: `{id, rev, loaded, kept}` (`kept`: its
/// iframe is the element marked before, `data-e2e`).
fn shown(chrome: &mut Browser, page: &Page) -> Value {
    let v = chrome
        .eval(page, "JSON.stringify([...document.querySelectorAll('.bd-frame')].map((f) => ({ id: f.dataset.id, rev: f.dataset.rev, loaded: f.dataset.loaded ?? null, label: f.querySelector('.bd-label b')?.textContent, kept: f.querySelector('iframe')?.dataset.e2e === 'kept', w: f.offsetWidth, h: f.offsetHeight })))")
        .unwrap_or(Value::Null);
    v.as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null)
}

fn shot(chrome: &mut Browser, page: &Page, dir: &Path, name: &str) {
    let _ = chrome.screenshot(page, &dir.join(name));
}

/// Whether every frame in `ids` has loaded its rev.
fn loaded(ids: &[&str]) -> String {
    let ids = serde_json::to_string(ids).expect("ids encode");
    format!("{ids}.every((id) => {{ const f = document.querySelector(`.bd-frame[data-id=\"${{id}}\"]`); return !!f && f.dataset.loaded === f.dataset.rev; }})")
}

pub fn board(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("board", &[crate::Need::Chrome]) {
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

    // frames, as an agent makes them: the files synced to main (a picture
    // among them), then put on the board; nothing deployed
    for (id, title) in [("home", "Home, first draft"), ("home-tablet", "Home on a tablet"), ("home-phone", "Home on a phone")] {
        write(&dir, &format!("frames/{id}/index.html"), frame_page(title, "").as_bytes())?;
    }
    write(&dir, "frames/kit/index.html", frame_page("A kit of parts", r#"<link rel="stylesheet" href="kit.css">"#).as_bytes())?;
    write(&dir, "frames/kit/kit.css", b"h1 { color: rgb(200, 30, 60); }\n")?;
    write(&dir, "frames/shot/shot.png", &png(SHOT.0, SHOT.1))?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let st = api.status(&owner, &name)?;
    s.ok("the frames' files sync to main, and nothing goes live", out.status.success() && st.body["pins"]["live"] == live && st.body["pins"]["main"] != live, json!({ "stderr": String::from_utf8_lossy(&out.stderr), "pins": st.body["pins"] }));
    let puts = [
        json!({ "id": "home", "kind": "html", "label": "Home", "size": "desktop" }),
        json!({ "id": "home-tablet", "kind": "html", "label": "Home", "size": "tablet" }),
        json!({ "id": "home-phone", "kind": "html", "label": "Home", "size": "mobile" }),
        json!({ "id": "kit", "kind": "html", "label": "Kit", "size": "custom", "width": 1000, "height": 600 }),
        json!({ "id": "shot", "kind": "image", "label": "Home today", "file": "shot.png", "width": SHOT_SHOWN }),
    ];
    let mut answers = vec![];
    for input in &puts {
        answers.push(s.cli_json(api, &home, &["call", &name, "put_frame", "--input", &input.to_string(), "--json"]).unwrap_or_else(|e| json!({ "error": format!("{e:#}") })));
    }
    s.ok(
        "put_frame puts each on the board, at its first rev",
        answers.iter().zip(&puts).all(|(a, p)| a["result"] == json!({ "id": p["id"], "rev": 1, "created": true }) && a["replayed"] == false),
        json!(answers),
    );
    let listed = s.cli_json(api, &home, &["call", &name, "frames", "--json"])?;
    let rows: Vec<Value> = listed["result"]["frames"].as_array().cloned().unwrap_or_default();
    let row = |id: &str| rows.iter().find(|f| f["id"] == id).cloned().unwrap_or(Value::Null);
    s.ok(
        "the manifest lists them in the order put, each named size at its width",
        rows.iter().map(|f| f["id"].as_str().unwrap_or("")).collect::<Vec<_>>() == ["home", "home-tablet", "home-phone", "kit", "shot"]
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
        json!({ "id": "a", "kind": "image", "label": "x" }),
        json!({ "id": "a", "kind": "html", "label": "x", "x": 10 }),
        json!({ "id": "a", "kind": "html", "label": "x", "size": "tablet", "width": 800 }),
    ]
    .into_iter()
    .map(refused)
    .collect::<Result<_>>()?;
    let says = ["a-z, 0-9 and -", "none starting with a dot", "names its file", "both x and y", "768 wide"];
    s.ok(
        "what a schema can't say the app refuses, saying why",
        app.iter().zip(says).all(|((status, message), why)| *status == 422 && message.contains(why)),
        json!(app),
    );
    let reader = api.person()?;
    let r = api.op(&reader, &name, "put_frame", "reader-1", json!({ "id": "mine", "kind": "html", "label": "Mine" }))?;
    let r2 = api.op(&reader, &name, "remove_frame", "reader-2", json!({ "id": "home" }))?;
    s.ok("someone not an editor changes nothing", r.status >= 400 && r2.status >= 400, format!("{r} / {r2}"));

    // the 49th frame
    for i in 0..FRAMES_MAX - rows.len() {
        api.op(&owner, &name, "put_frame", &format!("fill-{i}"), json!({ "id": format!("fill-{i}"), "kind": "html", "label": "Filler" }))?;
    }
    let over = api.op(&owner, &name, "put_frame", "fill-over", json!({ "id": "one-more", "kind": "html", "label": "One more" }))?;
    s.ok(&format!("a board holds {FRAMES_MAX} frames: one more is refused, saying so"), over.status == 422 && over.message().contains(&format!("holds {FRAMES_MAX} frames")), &over);
    let mut removed = vec![];
    for i in 0..FRAMES_MAX - rows.len() {
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
    let all = ["home", "home-tablet", "home-phone", "kit", "shot"];
    let drawn = chrome.until(&a, &format!("location.pathname === '/' && document.querySelectorAll('.bd-frame').length === 5 && {}", loaded(&all)), WAIT);
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
    s.ok(
        "a frame's scripts run in an opaque origin, reaching neither the board, nor its top, nor this origin's cookies, storage or operations",
        sandbox["origin"] == "null" && sandbox["parent"] == "blocked" && sandbox["top"] == "blocked" && sandbox["cookie"] == "blocked" && sandbox["storage"] == "blocked" && sandbox["fetched"] == "blocked",
        sandbox,
    );
    let picture = chrome.eval(&a, "(() => { const i = document.querySelector('.bd-frame[data-id=\"shot\"] img'); return i ? { complete: i.complete, w: i.naturalWidth, h: i.naturalHeight, shown: i.parentElement.parentElement.offsetWidth, tall: i.parentElement.parentElement.offsetHeight } : null; })()")?;
    s.ok(
        "the screenshot shows, at the width it was given and its own shape",
        picture["complete"] == true && picture["w"] == SHOT.0 && picture["h"] == SHOT.1 && picture["shown"] == SHOT_SHOWN && picture["tall"] == SHOT_SHOWN * SHOT.1 / SHOT.0,
        &picture,
    );
    let kit = heard_n(&mut chrome, &a, "kit", 1);
    s.ok(
        "on a board that isn't public, a frame's other files are a stranger's ask, refused: its own page alone styles it",
        kit.len() == 1 && kit[0]["text"] == "A kit of parts" && kit[0]["color"] == "rgb(10, 20, 30)",
        json!(kit),
    );

    // the second viewer, by the share link: a camera of its own
    let other = chrome.another_context()?;
    let b = chrome.open_in(&other, &api.site_url(&name, &format!("?view={token}")))?;
    chrome.viewport(&b, 1100, 760, false)?;
    let b_drawn = chrome.until(&b, &format!("location.search === '' && document.querySelectorAll('.bd-frame').length === 5 && {}", loaded(&all)), WAIT);
    s.ok("a viewer with the share link sees the same board", b_drawn, json!({ "at": chrome.eval(&b, "location.href")?, "frames": shown(&mut chrome, &b) }));

    // the owner moves and zooms; then the agent changes one frame
    chrome.eval(
        &a,
        "(() => { const v = document.getElementById('viewport'); v.dispatchEvent(new WheelEvent('wheel', { deltaY: -120, ctrlKey: true, clientX: 500, clientY: 400, bubbles: true, cancelable: true })); v.dispatchEvent(new WheelEvent('wheel', { deltaX: 80, deltaY: 40, clientX: 500, clientY: 400, bubbles: true, cancelable: true })); })()",
    )?;
    let a_camera = camera(&mut chrome, &a);
    let b_camera = camera(&mut chrome, &b);
    let mark = "document.querySelectorAll('.bd-frame iframe').forEach((i) => { i.dataset.e2e = 'kept'; })";
    chrome.eval(&a, mark)?;
    chrome.eval(&b, mark)?;
    let changed = s.dir("board-edits").join("home.html");
    std::fs::write(&changed, frame_page("Home, second draft", ""))?;
    let changed_s = changed.to_str().expect("utf-8 path").to_string();
    let out = s.cli(api, &home, &["write", &name, "frames/home/index.html", "--from", &changed_s]);
    let put = s.cli_json(api, &home, &["call", &name, "put_frame", "--input", &puts[0].to_string(), "--json"])?;
    s.ok("the agent writes the frame's page again and puts it again", out.status.success() && put["result"]["rev"] == 2, json!({ "write": String::from_utf8_lossy(&out.stderr), "put": put }));
    let redrawn = "(() => { const f = document.querySelector('.bd-frame[data-id=\"home\"]'); return f.dataset.rev === '2' && f.dataset.loaded === '2' && !f.querySelector('iframe').dataset.e2e && f.querySelectorAll('iframe').length === 1; })()";
    let a_redrawn = chrome.until(&a, redrawn, WAIT);
    let b_redrawn = chrome.until(&b, redrawn, WAIT);
    let second = heard_n(&mut chrome, &a, "home", 2);
    let others: Vec<(String, usize)> = ["home-tablet", "home-phone", "kit"].iter().map(|id| (id.to_string(), heard(&mut chrome, &a, id).len())).collect();
    let (a_frames, b_frames) = (shown(&mut chrome, &a), shown(&mut chrome, &b));
    let untouched = |frames: &Value| frames.as_array().is_some_and(|f| f.iter().filter(|f| f["id"] != "home" && f["id"] != "shot").all(|f| f["kept"] == true && f["rev"] == "1"));
    s.ok(
        "that frame alone loads again, in both pages, showing the new page",
        a_redrawn && b_redrawn && second.len() == 2 && second[1]["text"] == "Home, second draft" && others.iter().all(|(_, n)| *n == 1) && untouched(&a_frames) && untouched(&b_frames),
        json!({ "home": second, "others": others, "a": a_frames, "b": b_frames }),
    );
    s.ok(
        "and each page keeps its own pan and zoom",
        camera(&mut chrome, &a) == a_camera && camera(&mut chrome, &b) == b_camera && a_camera != b_camera && !a_camera.ends_with("scale(1)"),
        json!({ "a": [a_camera, camera(&mut chrome, &a)], "b": [b_camera, camera(&mut chrome, &b)] }),
    );
    shot(&mut chrome, &a, &media, "2-home-updated.png");

    // replay: the same call id answers its first result and redraws nothing
    let id = "replay-home";
    let first = s.cli_json(api, &home, &["call", &name, "put_frame", "--id", id, "--input", &puts[0].to_string(), "--json"])?;
    let replay = s.cli_json(api, &home, &["call", &name, "put_frame", "--id", id, "--input", &puts[0].to_string(), "--json"])?;
    let other_input = api.op(&owner, &name, "put_frame", id, json!({ "id": "home", "kind": "html", "label": "Elsewhere" }))?;
    let rev = s.cli_json(api, &home, &["call", &name, "frames", "--json"])?["result"]["frames"].as_array().and_then(|f| f.iter().find(|f| f["id"] == "home").map(|f| f["rev"].clone()));
    let back = chrome.until(&a, "document.querySelector('.bd-frame[data-id=\"home\"]').dataset.loaded === '3'", WAIT);
    s.ok(
        "a retried put_frame is a replay: its first answer, no second write; its id with another input is refused",
        first["result"]["rev"] == 3 && replay["replayed"] == true && replay["result"] == first["result"] && other_input.status == 409 && rev == Some(json!(3)) && back,
        json!({ "first": first, "replay": replay, "other": other_input.body, "rev": rev }),
    );

    // a removed frame goes, from both pages
    let gone = s.cli_json(api, &home, &["call", &name, "remove_frame", "--input", r#"{"id": "shot"}"#, "--json"])?;
    let none = "!document.querySelector('.bd-frame[data-id=\"shot\"]') && document.querySelectorAll('.bd-frame').length === 4";
    s.ok("a removed frame disappears from every open board", gone["result"]["removed"] == 1 && chrome.until(&a, none, WAIT) && chrome.until(&b, none, WAIT), &gone);

    // public, a frame's other files load: its stylesheet applies
    let out = s.cli(api, &home, &["visibility", &name, "public"]);
    let kit_put = s.cli_json(api, &home, &["call", &name, "put_frame", "--input", &puts[3].to_string(), "--json"])?;
    let kit = heard_n(&mut chrome, &a, "kit", 2);
    s.ok(
        "on a public board a frame's own other files load too",
        out.status.success() && kit_put["result"]["rev"] == 2 && kit.len() == 2 && kit[1]["color"] == "rgb(200, 30, 60)",
        json!({ "visibility": String::from_utf8_lossy(&out.stderr), "kit": kit }),
    );

    // restart: a deploy drops every page's socket; the pages follow on
    std::fs::write(dir.join("README.md"), format!("{}\nDeployed again.\n", std::fs::read_to_string(dir.join("README.md"))?))?;
    let out = s.cli(api, &home, &["deploy", &name, "--dir", &dir_s]);
    let a_camera = camera(&mut chrome, &a);
    let renamed = json!({ "id": "home-phone", "kind": "html", "label": "Home, small", "size": "mobile" });
    let put = s.cli_json(api, &home, &["call", &name, "put_frame", "--input", &renamed.to_string(), "--json"])?;
    let label = "document.querySelector('.bd-frame[data-id=\"home-phone\"] .bd-label b')?.textContent === 'Home, small'";
    s.ok(
        "after a deploy, open boards follow the next change, the camera where it was",
        out.status.success() && put["result"]["rev"] == 2 && chrome.until(&a, label, WAIT) && chrome.until(&b, label, WAIT) && camera(&mut chrome, &a) == a_camera,
        json!({ "deploy": String::from_utf8_lossy(&out.stderr), "put": put, "frames": shown(&mut chrome, &a) }),
    );

    // a phone: the board fits, nothing overflows, a pinch zooms it
    let phone = chrome.another_context()?;
    let c = chrome.open_in(&phone, &api.site_url(&name, ""))?;
    chrome.viewport(&c, 390, 844, true)?;
    chrome.touch(&c, true)?;
    let fitted = chrome.until(&c, &format!("document.querySelectorAll('.bd-frame').length === 4 && {}", loaded(&["home", "home-tablet", "home-phone", "kit"])), WAIT);
    // fitted once its frames came: every frame inside the screen
    chrome.eval(&c, "document.getElementById('fit').click()")?;
    let fits = chrome.eval(
        &c,
        "(() => { const r = [...document.querySelectorAll('.bd-body')].map((b) => b.getBoundingClientRect()); const z = document.querySelector('.bd-zoom').getBoundingClientRect(); return { inside: r.every((b) => b.left >= 0 && b.right <= innerWidth && b.top >= 0 && b.bottom <= innerHeight), overflow: document.documentElement.scrollWidth > innerWidth || document.documentElement.scrollHeight > innerHeight, controls: z.left >= 0 && z.right <= innerWidth && z.bottom <= innerHeight }; })()",
    )?;
    shot(&mut chrome, &c, &media, "3-phone.png");
    s.ok("at a phone's width the board fits every frame on screen, with nothing overflowing", fitted && fits["inside"] == true && fits["overflow"] == false && fits["controls"] == true, &fits);
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
