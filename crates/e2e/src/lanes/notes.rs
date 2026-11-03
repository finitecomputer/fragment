//! The notes template (slice E): a folder of markdown scaffolded, synced,
//! and deployed with the CLI; its page is the platform's files view
//! (`files.page`, FIN-372), which reads the notes at `main`, and open pages
//! follow new notes without a deploy.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::api::{Api, Call};
use crate::Suite;

pub fn notes(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("notes", &[crate::Need::Chrome]) {
        return Ok(());
    }
    let home = s.dir("notes-home");
    s.login(api, &home);
    s.cli_keys(&home).expect("the CLI logged in");
    let c = s.cli_json(api, &home, &["create", &s.name("notes"), "--show-tokens", "--json"])?;
    let name = c["name"].as_str().unwrap_or("").to_string();
    let root = s.dir("notes");
    let dir = root.join("vault");
    let dir_s = dir.to_str().expect("utf-8 path").to_string();
    let out = s.cli(api, &home, &["new", &dir_s, "--template", "notes"]);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("fragment.json")).unwrap_or_default()).unwrap_or_default();
    s.ok(
        "fragment new scaffolds the notes template: its notes and a manifest whose page is the files view, no page or code of its own",
        out.status.success() && dir.join("notes/welcome.md").is_file() && manifest["files"]["page"] == true && !dir.join("site").exists() && !dir.join("app.mjs").exists(),
        String::from_utf8_lossy(&out.stderr),
    );
    // two notes live as deployed, then changed on main alone (below)
    std::fs::write(dir.join("notes/kept.md"), "# kept\n\nfirst words\n")?;
    std::fs::write(dir.join("notes/gone.md"), "# gone\n")?;
    let out = s.cli(api, &home, &["deploy", &name, "--dir", &dir_s]);
    s.ok("it deploys", out.status.success(), String::from_utf8_lossy(&out.stderr));

    let view = c["viewToken"].as_str().unwrap_or("").to_string();
    let get = |path: &str| {
        api.call(Call { method: "GET", url: format!("{}{}view={view}", api.site_url(&name, path), if path.contains('?') { "&" } else { "?" }), ..Call::default() })
    };
    let listing = || {
        api.call(Call {
            method: "GET",
            url: format!("{}?view={view}", api.site_url(&name, "__files")),
            extra: vec![("accept", "application/json".into())],
            ..Call::default()
        })
    };
    let commit = |r: &crate::api::Reply, path: &str| -> Option<String> {
        r.body["files"].as_array()?.iter().find(|f| f["path"] == path).and_then(|f| f["commit"].as_str()).map(str::to_string)
    };
    let r = listing()?;
    let paths: Vec<&str> = r.body["files"].as_array().map(|a| a.iter().filter_map(|f| f["path"].as_str()).collect()).unwrap_or_default();
    s.ok(
        "its files view lists the notes, not the machinery",
        r.status == 200 && paths.contains(&"notes/welcome.md") && paths.contains(&"README.md") && !paths.contains(&"fragment.json"),
        &r,
    );
    let r = get("__file?path=notes/welcome.md")?;
    s.ok("the platform serves a note", r.status == 200 && r.text.contains("# welcome to your notes"), &r);

    // a new note needs no deploy: the files view reads main
    std::fs::write(dir.join("notes/second.md"), "# second\n")?;
    s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let r = get("__file?path=notes/second.md")?;
    s.ok("a synced note is served without a deploy", r.status == 200 && r.text == "# second\n", &r);

    // main is its files' authority (FIN-372, as its search and sync read
    // them): a deployed note edited or deleted on main alone is as main
    // says, at a version too, and nothing main deleted is listed or read
    // from live's older tree
    let deployed = listing()?;
    let (was_kept, was_gone) = (commit(&deployed, "notes/kept.md").unwrap_or_default(), commit(&deployed, "notes/gone.md").unwrap_or_default());
    std::fs::write(dir.join("notes/kept.md"), "# kept\n\nnew words\n")?;
    std::fs::remove_file(dir.join("notes/gone.md"))?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let after = listing()?;
    let now_kept = commit(&after, "notes/kept.md").unwrap_or_default();
    let kept = get("__file?path=notes/kept.md")?;
    let kept_then = get(&format!("__file?path=notes/kept.md&v={was_kept}"))?;
    let gone = get("__file?path=notes/gone.md")?;
    let gone_then = get(&format!("__file?path=notes/gone.md&v={was_gone}"))?;
    s.ok(
        "a deployed note edited on main alone reads as main has it, its old version sent on to the new; one deleted on main alone is listed nowhere and read nowhere, its old version neither",
        out.status.success()
            && !was_kept.is_empty() && !was_gone.is_empty() && !now_kept.is_empty() && now_kept != was_kept
            && kept.status == 200 && kept.text.contains("new words")
            && kept_then.status == 302 && kept_then.header("location").contains(&format!("v={now_kept}"))
            && commit(&after, "notes/gone.md").is_none() && gone.status == 404 && gone_then.status == 404,
        json!({ "kept": [kept.status, kept_then.status], "location": kept_then.header("location"), "gone": [gone.status, gone_then.status], "commits": [was_kept, now_kept] }),
    );

    // its page, in a browser: the files view, which opens its README and follows new notes
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the notes' page (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let page = chrome.open(&api.site_url(&name, &format!("?view={view}")))?;
    let wait = Duration::from_secs(20);
    let has = |n: &str| format!("document.getElementById('tree')?.textContent.includes('{n}')");
    // titled as its manifest names it (`fragment new` names a vault after its folder)
    let titled = manifest["meta"]["title"].as_str().unwrap_or("").to_string();
    let first = format!("document.querySelector('#reader .doc h1')?.textContent === 'notes' && document.getElementById('brand')?.textContent === {}", serde_json::to_string(&titled)?);
    let shown = chrome.until(&page, &format!("{} && {first}", has("second")), wait);
    s.ok(
        "its page is the files view: the notes tree, its README first, titled by it",
        shown,
        chrome.eval(&page, "JSON.stringify({ brand: document.getElementById('brand')?.textContent, h1: document.querySelector('#reader .doc h1')?.textContent ?? null, tree: document.getElementById('tree')?.textContent, hash: location.hash })")?,
    );
    std::fs::write(dir.join("notes/third.md"), "# third\n")?;
    s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    s.ok("an open page shows a new note without a reload", chrome.until(&page, &has("third"), wait), "");
    chrome.eval(&page, "location.hash = '#/notes/third.md'; true")?;
    s.ok("and opens it", chrome.until(&page, "document.getElementById('reader')?.textContent.includes('third')", wait), "");
    let vault = Vault { home: &home, name: &name, dir: &dir, view: &view };
    hostile(s, api, &vault, &mut chrome, &page)?;
    links(s, api, &vault, &mut chrome, &page)?;
    late(s, api, &vault, &mut chrome, &page)?;
    listed_late(s, api, &vault, &mut chrome, &page)
}

/// A note opened while the page's list is read again, before the list has
/// it (a link to a page an agent just wrote, followed at once). Goal: once
/// the list has it, it is drawn, not left as "no such file". Method: the
/// list's read held (`HOLD_FETCH`) as main's move reaches the page, the
/// note named meanwhile, the read let go, and the page waited on its body
/// read (hosted e2e e90d7c4619 found it: preview's latency left the note
/// "not in" its vault for good).
fn listed_late(s: &mut Suite, api: &Api, vault: &Vault, chrome: &mut crate::browser::Browser, page: &crate::browser::Page) -> Result<()> {
    let (home, name, dir) = (vault.home, vault.name, vault.dir);
    let wait = Duration::from_secs(20);
    chrome.eval(page, crate::browser::HOLD_FETCH)?;
    chrome.eval(page, "(window.__hold = /^__files$/, true)")?;
    std::fs::write(dir.join("notes/fresh.md"), "# fresh\n\nJust written.\n")?;
    s.cli(api, home, &["sync", name, "--dir", dir.to_str().expect("utf-8 path")]);
    let held = chrome.until(page, "window.__held.length === 1", wait);
    chrome.eval(page, "(window.__hold = null, location.hash = '#/notes/fresh.md', true)")?;
    let missing = chrome.until(page, "document.getElementById('reader')?.textContent.includes('notes/fresh.md is not in')", wait);
    chrome.eval(page, "(window.__held.splice(0).forEach((go) => go()), true)")?;
    let listed = chrome.until(page, "window.__consumed === 1", wait);
    let drawn = chrome.until(page, "document.querySelector('#reader .doc h1')?.textContent === 'fresh'", wait);
    s.ok(
        "a note opened before the page's list has it is drawn once the list has it, not left as no such file",
        held && missing && listed && drawn,
        json!({ "held": held, "missing": missing, "listed": listed, "drawn": drawn, "reader": chrome.eval(page, "document.getElementById('reader')?.innerText.slice(0, 120) ?? ''")? }),
    );
    Ok(())
}

/// The notes fragment the lane made: its CLI's home, its name, the synced
/// folder, and its share link.
struct Vault<'a> {
    home: &'a std::path::Path,
    name: &'a str,
    dir: &'a std::path::Path,
    view: &'a str,
}

/// The files view's hooks into the renderer, in Chrome: a [[wikilink]]
/// opens its note there, an ![[embed]] is the vault's picture through the
/// platform's `__file` (with the page's share link), a relative link and a
/// reference link resolve to notes, and front matter is left out.
fn links(s: &mut Suite, api: &Api, vault: &Vault, chrome: &mut crate::browser::Browser, page: &crate::browser::Page) -> Result<()> {
    let (home, name, dir) = (vault.home, vault.name, vault.dir);
    let wait = Duration::from_secs(20);
    std::fs::write(dir.join("notes/linked.md"), "---\ntitle: Linked\nsecret: in the front matter\n---\n# Linked note\n\nGo [[welcome|home]], to [the second](second.md), or to [the third][3].\n\n![[pic.svg|40]]\n\n[3]: third.md\n")?;
    s.cli(api, home, &["sync", name, "--dir", dir.to_str().expect("utf-8 path")]);
    chrome.eval(page, "location.hash = '#/notes/linked.md'; true")?;
    // The note is read at main as it opens, but its embed resolves against
    // the files view's list, which follows main by `__watch`: until main's
    // move reaches the page (seconds, on a hosted run) the picture an
    // earlier sync added is not in it, and the note is drawn again when the
    // list next moves. So it is read once its picture is loaded.
    let embed = "document.querySelector('#reader .doc img.md-image')";
    let shown = chrome.until(page, &format!("document.querySelector('#reader .doc h1')?.textContent === 'Linked note' && !!{embed}?.getAttribute('src') && {embed}.complete && {embed}.naturalWidth > 0"), wait);
    let read = chrome.eval(page, "(() => { const c = document.querySelector('#reader .doc'); const img = c.querySelector('img.md-image'); const a = (t) => [...c.querySelectorAll('a')].find((x) => x.textContent === t); return { front: c.textContent.includes('front matter'), src: img?.getAttribute('src') ?? null, width: img?.getAttribute('width') ?? null, loaded: !!img && img.complete && img.naturalWidth > 0, home: a('home')?.getAttribute('href') ?? null, second: a('the second')?.dataset.path ?? null, third: a('the third')?.dataset.path ?? null }; })()")?;
    s.ok(
        "the files view leaves out front matter, shows an embed through __file (the share link rides its cookie), and resolves wikilinks, relative and reference links to notes",
        shown
            && read["front"] == false
            && read["src"].as_str().is_some_and(|u| u.starts_with("__file?path=notes%2Fpic.svg"))
            && read["loaded"] == true
            && read["width"] == "40"
            && read["home"] == "#/notes/welcome.md"
            && read["second"] == "notes/second.md"
            && read["third"] == "notes/third.md",
        &read,
    );
    chrome.click(page, "#reader .doc a.md-wikilink")?;
    let opened = chrome.until(page, "location.hash === '#/notes/welcome.md' && document.getElementById('reader')?.textContent.includes('welcome to your notes')", wait);
    s.ok("a click on a wikilink opens its note in the files view", opened, chrome.eval(page, "location.hash + ' ' + (document.getElementById('reader')?.innerText.slice(0, 120) ?? '')")?);
    Ok(())
}

/// A note names a picture and a note the vault does not hold yet. Goal:
/// when they come in a later sync, the open note, itself unchanged, shows
/// the picture and links the note in place; a file it does not name, coming,
/// leaves it as drawn. Method: the note opened once the page's tree has it
/// (so its own sync's trigger has come and gone), then the files synced;
/// then a note it does not name, with the page's reads of it counted.
fn late(s: &mut Suite, api: &Api, vault: &Vault, chrome: &mut crate::browser::Browser, page: &crate::browser::Page) -> Result<()> {
    let (home, name, dir) = (vault.home, vault.name, vault.dir);
    let dir_s = dir.to_str().expect("utf-8 path");
    let wait = Duration::from_secs(20);
    let text = "JSON.stringify({ hash: location.hash, stayed: window.__stayed ?? null, text: document.getElementById('reader')?.innerText.slice(0, 200) ?? null, img: document.querySelector('#reader .doc img')?.getAttribute('src') ?? null })";
    std::fs::write(dir.join("notes/waiting.md"), "# Waiting\n\n![[late.png]]\n\nThen [[later]].\n")?;
    s.cli(api, home, &["sync", name, "--dir", dir_s]);
    chrome.eval(page, "location.hash = '#/notes/waiting.md'; true")?;
    let missing = chrome.until(
        page,
        "document.querySelector('#reader .doc h1')?.textContent === 'Waiting' && !document.querySelector('#reader .doc img') && [...document.querySelectorAll('#reader .doc .md-missing')].map((x) => x.textContent).join() === 'late.png,later' && !!document.querySelector('#tree a.file[data-path=\"notes/waiting.md\"]')",
        wait,
    );
    s.ok("a note that names a picture and a note the vault lacks shows both as missing", missing, chrome.eval(page, text)?);

    // a navigation or a reload would drop the mark
    chrome.eval(page, "window.__stayed = true; true")?;
    std::fs::write(dir.join("notes/late.png"), DOT_PNG)?;
    std::fs::write(dir.join("notes/later.md"), "# later\n")?;
    s.cli(api, home, &["sync", name, "--dir", dir_s]);
    let embed = "document.querySelector('#reader .doc img.md-image')";
    let drawn = chrome.until(
        page,
        &format!("window.__stayed === true && location.hash === '#/notes/waiting.md' && ({embed}?.getAttribute('src') ?? '').startsWith('__file?path=notes%2Flate.png') && {embed}.complete && {embed}.naturalWidth > 0 && document.querySelector('#reader .doc a.md-wikilink')?.dataset.path === 'notes/later.md'"),
        wait,
    );
    s.ok("when they come in a later sync, the open note, unchanged, shows the picture and links the note, with no navigation", drawn, chrome.eval(page, text)?);

    // the page's reads of the open note, from here
    chrome.eval(page, "(() => { window.__reads = 0; const f = window.fetch; window.fetch = (u, o) => { if (String(u).includes('waiting.md')) window.__reads++; return f(u, o); }; return true; })()")?;
    std::fs::write(dir.join("notes/aside.md"), "# aside\n")?;
    s.cli(api, home, &["sync", name, "--dir", dir_s]);
    // the refresh that lists it decides, in the same turn, whether to read the note again
    let listed = chrome.until(page, "!!document.querySelector('#tree a.file[data-path=\"notes/aside.md\"]')", wait);
    let reads = chrome.eval(page, "window.__reads")?;
    s.ok("a note it does not name, coming, leaves it as drawn: the page does not read it again", listed && reads == 0, &reads);
    Ok(())
}

/// What a note, an editor's or an agent's, holds is data (audit H2: the
/// viewer once put `marked`'s output in `innerHTML`, and a note's `<img
/// onerror>` ran as each of its viewers). Goal: the files view renders a
/// note's markup as its text and runs none of it; the platform's renderer
/// is served and holds to that in a real browser; a note's SVG served by
/// the platform is sandboxed. Method: a committed hostile note opened in
/// the files view; the renderer imported on the page with hooks that allow any
/// URL; an SVG that scripts itself, read and opened directly.
fn hostile(s: &mut Suite, api: &Api, vault: &Vault, chrome: &mut crate::browser::Browser, page: &crate::browser::Page) -> Result<()> {
    let (home, name, dir, view) = (vault.home, vault.name, vault.dir, vault.view);
    let wait = Duration::from_secs(20);
    // a fetch with the share link, as the lane's own reads are
    let get = |path: &str| api.call(Call { method: "GET", url: format!("{}{}view={view}", api.site_url(name, path), if path.contains('?') { "&" } else { "?" }), ..Call::default() });
    let dir_s = dir.to_str().expect("utf-8 path");
    std::fs::write(dir.join("notes/hostile.md"), HOSTILE_NOTE)?;
    std::fs::write(dir.join("notes/pic.svg"), SCRIPTED_SVG)?;
    s.cli(api, home, &["sync", name, "--dir", dir_s]);
    chrome.eval(page, "location.hash = '#/notes/hostile.md'; true")?;
    let shown = chrome.until(page, "document.querySelector('#reader .doc h1')?.textContent === 'A hostile note'", wait);
    std::thread::sleep(Duration::from_millis(500));
    s.ok(
        "a note's <img onerror>, <script>, <svg onload> and javascript: links render as their text, and none of it runs",
        shown && chrome.eval(page, "(() => { const c = document.getElementById('reader'); return window.__ran === undefined && !c.querySelector('img,script,svg,iframe,a[href^=\"javascript\" i]') && c.textContent.includes('<img src=x onerror=') && c.textContent.includes('<script>'); })()")? == true,
        chrome.eval(page, "JSON.stringify({ ran: window.__ran ?? null, text: document.getElementById('reader')?.innerText.slice(0, 400) })")?,
    );
    let r = get("__markdown.js")?;
    s.ok("the platform's renderer is served to every fragment, tagged with its build-time hash", r.status == 200 && r.header("content-type").starts_with("text/javascript") && r.header("etag").len() == 18 && r.text.contains("export function markdown"), &r);
    let rendered = chrome.eval(page, "import('./__markdown.js').then((m) => { const box = document.createElement('div'); box.append(m.markdown('<img src=x onerror=\"window.__ran2 = 1\">\\n\\n[x](javascript:window.__ran2=2) [y](\\u0001javascript:window.__ran2=3) ![z](javascript:1) [[w]]', { resolve: (t) => ({ href: t }), image: (src) => ({ src, href: src }) })); document.body.append(box); return new Promise((done) => setTimeout(() => done(window.__ran2 === undefined && !box.querySelector('img,script,a[href^=\"javascript\" i]') && box.textContent.startsWith('<img')), 300)); })")?;
    s.ok("in Chrome the renderer makes no element of markup and no script link, even when its hooks allow every URL", rendered == true, &rendered);
    let r = get("__file?path=notes/pic.svg")?;
    s.ok(
        "an SVG the platform serves is sandboxed, and never sniffed",
        r.status == 200 && r.header("content-type").starts_with("image/svg+xml") && r.header("content-security-policy").contains("sandbox") && r.header("x-content-type-options") == "nosniff",
        &r,
    );
    // the page's browser holds the share link's cookie
    let svg = chrome.open(&api.site_url(name, "__file?path=notes/pic.svg"))?;
    // its document whole, its script's turn come and gone (an inline
    // script runs as its document parses)
    let loaded = chrome.until(&svg, "document.readyState === 'complete' && document.documentElement.localName === 'svg'", Duration::from_secs(10));
    let ran = chrome.eval(&svg, "document.documentElement.getAttribute('data-ran')")?;
    s.ok("opened directly, the SVG's script does not run", loaded && ran.is_null(), &ran);
    chrome.close(svg)?;
    Ok(())
}

const HOSTILE_NOTE: &str = include_str!("../../fixtures/hostile-note.md");
/// A 1×1 PNG: the picture a note names before the vault holds it.
const DOT_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15,
    0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00,
    0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const SCRIPTED_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"document.documentElement.setAttribute('data-ran', '1')\"><circle cx=\"4\" cy=\"4\" r=\"4\"/></svg>\n";
