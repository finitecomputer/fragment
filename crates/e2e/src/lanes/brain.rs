//! The brain template (docs/cloudflare-v1.md, decision 30): a fragment app
//! on the blessed `brain` template, made as the CLI makes it, whose repo
//! holds only its face and its files. Its files are wikis (raw/ wiki/ …,
//! index.md, log.md); its search is the fragment's own index, by words and
//! by meaning, written through the app's `this.searchIndex` and kept by
//! its triggers (each move of main, each install of a new release). An
//! agent's path through it: the CLI syncs a folder, writes a source into
//! raw/ and a page into wiki/, and searches. Then: search
//! syntax is plain words, an over-long query is refused, a viewer searches
//! and a stranger may not, and a viewer's start of the index's job is safe;
//! the same files again change nothing; the index survives a restart and
//! follows the next change; an asset's blob, named by its source note, is
//! served; an index behind its files says so, and a member's page starts
//! its job; the page (the notes viewer and the brain's search) loads for a
//! member; and a page grown past 1 MiB leaves the index by the brain's own
//! fenced delete, while a delete a newer writer fenced is stale, no failure.

#[path = "brain_parent.rs"]
mod parent;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use fragment_core::blob::{self, sha256_hex};
use fragment_core::npub;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::jobs;
use super::ledger::lever;
use super::shell::shell;
use super::signin::site_cookie;
use super::templates::person;
use crate::api::{url_enc, Api, Call, Reply};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(40);

const PAGE: &str = "---
title: Solar panels
summary: What the shed's roof could carry.
tags: [energy, roof]
---

# Solar panels

Notes on panels for the shed, from [[garden/raw/2026-10-03-cafe-solaire.md|the café article]].

## Photovoltaic cells

Photovoltaic cells turn light into current. Photovoltaic output falls in shade.

## Inverters

An inverter turns the panels' direct current into household current. A string
inverter serves the whole array; a micro inverter serves one panel.

## Caveats

NOT every roof suits them: a north face gets too little light.
";
const SOURCE: &str = "---
type: source
title: Café solaire
source: https://example.com/cafe-solaire
captured: 2026-10-03
---

# Café solaire

A café in Lyon runs its espresso machine from photovoltaic panels on its roof.
";
const INDEX: &str = "# garden\n\n- [[garden/wiki/solar-panels.md|Solar panels]]: panels for the shed\n";
const LOG: &str = "# Log\n\n## 2026-10-03 ingest\n\nTook in the café article; wrote [[garden/wiki/solar-panels.md|Solar panels]].\n";
/// A wiki's own conventions: read by agents, never searched (a word only here).
const AGENTS: &str = "# garden\n\nSources here are dated. Zeppelin.\n";
const BATTERIES: &str = "\n## Batteries\n\nA lithium battery keeps the day's surplus for the evening.\n";

fn bytes_of(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(29).wrapping_add(seed)).collect()
}

fn write(dir: &Path, path: &str, bytes: &[u8]) -> Result<()> {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().context("a file in a folder")?)?;
    Ok(std::fs::write(p, bytes)?)
}

fn search(api: &Api, keys: &Keys, name: &str, input: Value) -> Result<Reply> {
    api.op(keys, name, "search", "q", input)
}

/// A search's ranked sections.
fn results(r: &Reply) -> Vec<Value> {
    r.body["result"]["results"].as_array().cloned().unwrap_or_default()
}

/// Where a search's results are: path and heading, best first.
fn found(r: &Reply) -> Vec<(String, String)> {
    results(r).iter().map(|x| at(x["path"].as_str().unwrap_or(""), x["heading"].as_str().unwrap_or(""))).collect()
}

/// A section: its page's path and its heading.
fn at(path: &str, heading: &str) -> (String, String) {
    (path.to_string(), heading.to_string())
}

fn index(api: &Api, keys: &Keys, name: &str) -> Value {
    api.op(keys, name, "index_status", "st", json!({})).map(|r| r.body["result"].clone()).unwrap_or_default()
}

/// The index has taken in a sync: nothing pending, `pages` pages.
fn settled(api: &Api, keys: &Keys, name: &str, pages: u64) -> bool {
    let st = index(api, keys, name);
    st["pending"] == 0 && st["pages"] == pages
}

const SOLAR: &str = "garden/wiki/solar-panels.md";
const CAFE: &str = "garden/raw/2026-10-03-cafe-solaire.md";
const NOTE: &str = "garden/raw/roof-survey.md";
const ASSET: &str = "garden/raw/assets/roof-survey.bin";
const GROWS: &str = "garden/wiki/anemometer.md";
const GUST: &str = "garden/wiki/gust.md";

pub fn brain(s: &mut Suite, api: &Api) -> Result<()> {
    // the code.storage fake (a pointer read back), a restart, Chrome
    if !s.section("brain", &[crate::Need::Fakes, crate::Need::Node, crate::Need::Chrome]) {
        return Ok(());
    }
    // an agent's CLI, and its person on the platform (the same person)
    let home = s.dir("brain-home");
    s.login(api, &home);
    let keys = s.cli_keys(&home).context("the CLI logged in")?;
    let session = api.sign_in(&crate::cli_email(&npub::encode(keys.pubkey_hex()))?)?;
    let label = s.name("garden");

    // made as the CLI makes it (`fragment create --template brain`): blessed, named, with a title
    let made = s.cli_json(api, &home, &["create", &label, "--template", "brain", "--title", "Garden", "--json"]).unwrap_or_else(|e| json!({ "error": format!("{e:#}") }));
    let name = made["name"].as_str().unwrap_or("").to_string();
    s.ok("the CLI makes a brain on the blessed brain template", fragment_proto::split_fragment_name(&name).is_some_and(|(l, _)| l == label), &made);
    anyhow::ensure!(!name.is_empty(), "no brain to test: {made}");
    let repo = made["repo"].as_str().unwrap_or("").to_string();
    let listed = s.eventually(WAIT, || {
        shell(api, &session, "GET", "/api/fragments", None, &[]).is_ok_and(|r| {
            r.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == name.as_str() && f["kind"] == "brain" && f["title"] == "Garden"))
        })
    });
    s.ok("the person's list says it is a brain, and its title", listed, "");
    let m = api.signed(&keys, "GET", &format!("/api/f/{name}/manifest"), None)?;
    s.ok("its repo names the template and its face, nothing else", m.body == json!({ "template": "brain", "meta": { "title": "Garden" } }), &m);
    let st = api.status(&keys, &name)?;
    let ops = &st.body["code"]["operations"];
    let release = fragment_templates::blessed::release("brain").map(|r| format!("blessed:brain@{r}"));
    s.ok(
        "the release's code runs on it, as its status names it: search, its index, and the job its file trigger starts",
        st.body["code"]["error"].is_null()
            && st.body["code"]["id"].as_str() == release.as_deref()
            && ops["search"]["kind"] == "query"
            && ops["changed"]["kind"] == "job"
            && ops["changed"]["role"] == "viewer",
        &st.body["code"],
    );
    let t = api.signed(&keys, "GET", &format!("/api/f/{name}/triggers"), None)?;
    let listed = |want: Value| t.body["triggers"].as_array().is_some_and(|l| l.iter().any(|x| x["run"] == "changed" && want.as_object().is_some_and(|w| w.iter().all(|(k, v)| x[k] == *v))));
    s.ok("a file trigger keeps the index, and an installed trigger takes each new release", listed(json!({ "files": "**" })) && listed(json!({ "installed": true })), &t);
    // each install of the release runs `changed` once, as the brain: a new
    // release (an index of an older shape among them) is taken in with no
    // one acting, at the brain's next request
    let own = st.body["npub"].clone();
    let installs = || jobs::runs(api, &keys, &name, "&op=changed").into_iter().filter(|r| r["via"] == "installed").collect::<Vec<Value>>();
    let first = s.eventually(WAIT, || installs().first().is_some_and(|r| r["status"] == "succeeded"));
    let stale = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "blessed-stale" })))?;
    let asked = index(api, &keys, &name);
    let again = s.eventually(WAIT, || {
        let l = installs();
        l.len() == 2 && l.iter().all(|r| r["status"] == "succeeded" && r["principal"] == own)
    });
    s.ok(
        "its install ran changed once, as the brain; a newer release of the template runs it once more at the brain's next request, no one starting it",
        first && stale.status == 200 && stale.body["stale"] == true && asked["format"].is_string() && again,
        json!({ "stale": stale.body, "installs": installs() }),
    );

    // ingest, as an agent does: sync a folder, write a source and a page, sync
    let dir = s.dir("brain");
    let dir_s = dir.to_str().context("a utf-8 path")?.to_string();
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    s.ok("an agent syncs a folder with the brain", out.status.success() && dir.join("fragment.json").is_file(), String::from_utf8_lossy(&out.stderr));
    let asset = bytes_of(1536 * 1024 + 7, 3);
    let asset_sha = sha256_hex(&asset);
    let asset_note = format!(
        "---\ntype: asset\ntitle: Roof survey\nresource: raw/assets/roof-survey.bin\ndescription: The installer's survey of the south face.\nfinite_asset:\n  content_type: application/octet-stream\n  size: {}\n  content_hash: sha256:{asset_sha}\n---\n\n# Roof survey\n\nThe installer's survey of the shed's south face, captured whole.\n",
        asset.len()
    );
    for (path, bytes) in [
        ("garden/index.md", INDEX.as_bytes()),
        ("garden/log.md", LOG.as_bytes()),
        ("garden/AGENTS.md", AGENTS.as_bytes()),
        (CAFE, SOURCE.as_bytes()),
        (SOLAR, PAGE.as_bytes()),
        (NOTE, asset_note.as_bytes()),
        (ASSET, &asset),
    ] {
        write(&dir, path, bytes)?;
    }
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    s.ok("its sources and pages sync to main, with no deploy", out.status.success(), String::from_utf8_lossy(&out.stderr));
    let files = api.signed(&keys, "GET", &format!("/api/f/{name}/files"), None)?;
    let paths: Vec<&str> = files.body["files"].as_array().map(|l| l.iter().filter_map(|f| f["path"].as_str()).collect()).unwrap_or_default();
    s.ok(
        "its repo holds its face and its files, no code or site",
        paths.contains(&"fragment.json") && paths.contains(&SOLAR) && !paths.iter().any(|p| *p == "app.mjs" || p.starts_with("site/") || p.starts_with("applib/")),
        format!("{paths:?}"),
    );
    // five pages: the index, the log, the source, the page, the asset's note (a wiki's AGENTS.md is no page)
    let took = s.eventually(WAIT, || settled(api, &keys, &name, 5));
    s.ok("the file trigger indexes what moved: five pages, nothing pending", took, index(api, &keys, &name));

    let out = s.cli_json(api, &home, &["call", &name, "search", "--input", r#"{"q": "inverter"}"#, "--json"]);
    let r = out.as_ref().map(|d| d["result"].clone()).unwrap_or_default();
    let first = r["results"][0].clone();
    s.ok(
        "a search from the CLI finds the page's section: path, heading, the headings above it, a snippet",
        first["rank"] == 1 && first["path"] == SOLAR && first["heading"] == "Inverters" && first["ancestry"] == json!(["Solar panels"]) && first["title"] == "Solar panels"
            && first["wiki"] == "garden" && first["snippet"].as_str().is_some_and(|t| t.to_lowercase().contains("inverter")) && r["pending"] == 0 && r["indexing"] == false,
        &r,
    );
    let r = search(api, &keys, &name, json!({ "q": "photovoltaic" }))?;
    s.ok(
        "ranked: the page whose section is about it first, the source that mentions it after, before what only means something like it",
        found(&r).get(..2) == Some(&[at(SOLAR, "Photovoltaic cells"), at(CAFE, "Café solaire")][..]),
        &r,
    );
    let r = search(api, &keys, &name, json!({ "q": "PHOTOVOLTAIC" }))?;
    s.ok("a word finds it, whatever its case", found(&r).first().is_some_and(|f| f.0 == SOLAR), &r);
    let r = search(api, &keys, &name, json!({ "q": "cafe" }))?;
    s.ok("accents are ignored: cafe finds the café source, and the page that cites it, first", found(&r).iter().take(2).any(|f| f.0 == CAFE) && found(&r).iter().take(2).any(|f| f.0 == SOLAR), &r);
    let r = search(api, &keys, &name, json!({ "q": "photovoltaic", "wiki": "kitchen" }))?;
    let mine = search(api, &keys, &name, json!({ "q": "photovoltaic", "wiki": "garden", "limit": 1 }))?;
    s.ok("a search narrows to one wiki, and to a limit", r.status == 200 && results(&r).is_empty() && found(&mine) == vec![at(SOLAR, "Photovoltaic cells")], format!("{r} | {mine}"));
    let r = search(api, &keys, &name, json!({ "q": "zeppelin" }))?;
    s.ok("a wiki's AGENTS.md is read by agents, not searched", r.status == 200 && found(&r).iter().all(|f| f.0 != "garden/AGENTS.md"), &r);

    // invalid: search syntax is words; a query too long is refused
    let mut tried = vec![];
    for q in ["inverter OR zebra", "heading:inverter", "\"inverter", "inverter*", "^inverter", "inverter)", "NEAR(inverter", "inverter AND"] {
        let r = search(api, &keys, &name, json!({ "q": q }))?;
        tried.push((q, r.status, found(&r).iter().take(3).any(|f| *f == at(SOLAR, "Inverters"))));
    }
    s.ok(
        "OR, a column filter, quotes, stars, carets, brackets, NEAR and a dangling AND are words, never a syntax error: each finds the Inverters section",
        tried.iter().all(|(_, status, found)| *status == 200 && *found),
        format!("{tried:?}"),
    );
    let r = search(api, &keys, &name, json!({ "q": "?! …" }))?;
    s.ok("a query with no word is no error", r.status == 200, &r);
    let r = search(api, &keys, &name, json!({ "q": "x".repeat(257) }))?;
    s.ok("a query over 256 characters is refused (400), naming it", r.status == 400 && r.message().contains("/q"), &r);
    let r = search(api, &keys, &name, json!({ "q": "" }))?;
    let r2 = search(api, &keys, &name, json!({ "q": "inverter", "limit": 51 }))?;
    let r3 = search(api, &keys, &name, json!({ "q": "inverter", "sql": "1" }))?;
    s.ok("an empty query, a limit over 50, and an unknown field are refused (400)", r.status == 400 && r2.status == 400 && r3.status == 400, format!("{r} | {r2} | {r3}"));

    // access is the brain's members: a viewer searches, a stranger may not
    let (viewer, viewer_session) = person(api)?;
    let r = api.share(&name, &npub::encode(viewer.pubkey_hex()), "viewer")?;
    anyhow::ensure!(r.status == 200, "adding a viewer: {r}");
    let r = search(api, &viewer, &name, json!({ "q": "inverter" }))?;
    s.ok("a viewer may search", r.status == 200 && found(&r).first().is_some_and(|f| f.0 == SOLAR), &r);
    let stranger = api.person()?;
    let r = search(api, &stranger, &name, json!({ "q": "inverter" }))?;
    let anon = api.browser_op(&name, "search", "q", json!({ "q": "inverter" }), None)?;
    s.ok("a stranger may not (403), nor anyone without the link (401)", r.status == 403 && anon.status == 401, format!("{r} | {anon}"));
    // a viewer may start the index's job, which costs nothing with nothing to do: no full pass is theirs
    let writes = index(api, &keys, &name)["writes"].clone();
    let r = api.op(&viewer, &name, "reindex", "r", json!({ "full": true }))?;
    let r2 = api.op(&viewer, &name, "mark", "m", json!({ "paths": [], "full": true }))?;
    let started = api.op(&viewer, &name, "changed", "v-changed", json!({ "full": true }))?;
    let run = jobs::settle(api, &keys, &name, jobs::started(&started), &["succeeded", "held"], WAIT);
    s.ok(
        "a viewer may start the index's job: with nothing to do it takes two steps and writes nothing, and a full pass is an editor's alone",
        r.status == 200 && r.body["result"]["claimed"] == 0 && r2.status == 200 && r2.body["result"]["full"] == false
            && run["status"] == "succeeded" && run["output"]["steps"] == 1 && run["output"]["full"] == false && index(api, &keys, &name)["writes"] == writes,
        json!({ "reindex": r.body, "mark": r2.body, "run": run }),
    );

    // an asset: its bytes a blob, named by its source note, served
    let pointer = s.fake.file_at(&repo, "main", ASSET).unwrap_or_default();
    s.ok(
        "an asset of 1 MiB or more is a blob: a pointer in git",
        blob::parse(&pointer).is_some_and(|p| p.sha256 == asset_sha && p.size == asset.len() as u64),
        String::from_utf8_lossy(&pointer),
    );
    let r = search(api, &keys, &name, json!({ "q": "roof survey" }))?;
    s.ok("its source note is searched", found(&r).first().is_some_and(|f| f.0 == NOTE), &r);
    let viewer_site = format!("fragment_site={}", site_cookie(api, &viewer_session, &name)?);
    let get = |url: String| api.call(Call { method: "GET", url, cookie: Some(viewer_site.clone()), ..Call::default() });
    let note = get(api.site_url(&name, &format!("api/file?path={}", url_enc(NOTE))))?;
    let resource = note.text.lines().find_map(|l| l.strip_prefix("resource: ")).unwrap_or("").to_string();
    let hash = note.text.lines().find_map(|l| l.trim().strip_prefix("content_hash: sha256:")).unwrap_or("").to_string();
    let to = get(api.site_url(&name, &format!("api/file?path={}", url_enc(&format!("garden/{resource}")))))?;
    let location = to.header("location");
    let bytes = if location.starts_with("http") { Some(get(location.clone())?) } else { None };
    s.ok(
        "the note names its asset, whose bytes the brain serves to a member: the blob it hashes to",
        note.status == 200 && to.status == 302 && location.contains("__file?path=") && bytes.as_ref().is_some_and(|b| b.status == 200 && b.bytes == asset) && hash == asset_sha,
        format!("note {} ({resource}), {to} → {location}, {:?} bytes", note.status, bytes.map(|b| (b.status, b.bytes.len()))),
    );
    let landing = get(api.site_url(&name, "api/file?path=_index.md"))?;
    let guide = get(api.site_url(&name, "api/file?path=AGENTS.md"))?;
    let tree = get(api.site_url(&name, "api/tree"))?;
    let listed: Vec<&str> = tree.body["files"].as_array().map(|l| l.iter().filter_map(|f| f["path"].as_str()).collect()).unwrap_or_default();
    s.ok(
        "the viewer's tree is the brain's files and the two it makes: its landing (its wikis) and the guide",
        landing.text.contains("# Garden") && landing.text.contains("[[garden/index.md|garden]]") && guide.text.starts_with("# This brain")
            && listed.contains(&"_index.md") && listed.contains(&"AGENTS.md") && listed.contains(&SOLAR) && !listed.contains(&"fragment.json"),
        format!("{landing} | {listed:?}"),
    );
    let out = s.cli_json(api, &home, &["call", &name, "guide", "--input", "{}", "--json"]);
    s.ok("an agent reads the guide from the CLI", out.as_ref().is_ok_and(|d| d["result"]["text"] == guide.text.as_str()), out.map(|d| d["result"]["text"].to_string().chars().take(80).collect::<String>()).unwrap_or_else(|e| format!("{e:#}")));

    // replay: the same files again change nothing in the index
    let before = index(api, &keys, &name);
    let ranked = found(&search(api, &keys, &name, json!({ "q": "photovoltaic" }))?);
    let all: Vec<&str> = vec!["garden/index.md", "garden/log.md", "garden/AGENTS.md", CAFE, SOLAR, NOTE, ASSET];
    for (id, input) in [("again", json!({ "paths": all })), ("again-full", json!({ "paths": [], "full": true }))] {
        let r = api.op(&keys, &name, "changed", id, input)?;
        let run = jobs::settle(api, &keys, &name, jobs::started(&r), &["succeeded", "held"], WAIT);
        anyhow::ensure!(run["status"] == "succeeded", "the replayed run: {run}");
    }
    let after = index(api, &keys, &name);
    let again = found(&search(api, &keys, &name, json!({ "q": "photovoltaic" }))?);
    s.ok(
        "the same paths again, and every file read again, write nothing: the same pages, sections, and ranks",
        after["writes"] == before["writes"] && after["sections"] == before["sections"] && after["pages"] == 5 && again == ranked,
        json!({ "before": before, "after": after }),
    );
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    s.ok("and a sync with nothing new leaves it as it was", out.status.success() && index(api, &keys, &name)["writes"] == before["writes"], index(api, &keys, &name));

    // restart: the index survives, and follows the next change
    s.stop()?;
    let api = &s.start(false)?;
    let st = index(api, &keys, &name);
    let r = search(api, &keys, &name, json!({ "q": "inverter" }))?;
    s.ok(
        "after a restart the index is as it was, and answers at once",
        st["writes"] == before["writes"] && st["sections"] == before["sections"] && st["pending"] == 0 && found(&r).first() == Some(&at(SOLAR, "Inverters")),
        json!({ "index": st, "search": r.body }),
    );
    let mut page = PAGE.to_string();
    page.push_str(BATTERIES);
    write(&dir, SOLAR, page.as_bytes())?;
    std::fs::remove_file(dir.join(CAFE))?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let followed = s.eventually(WAIT, || {
        settled(api, &keys, &name, 4) && search(api, &keys, &name, json!({ "q": "lithium" })).is_ok_and(|r| found(&r).first() == Some(&at(SOLAR, "Batteries")))
    });
    let st = index(api, &keys, &name);
    let r = search(api, &keys, &name, json!({ "q": "cafe" }))?;
    s.ok(
        "then a change is indexed and a removed page leaves it: only those two written",
        out.status.success() && followed && st["writes"].as_i64() == before["writes"].as_i64().map(|w| w + 2) && found(&r).iter().all(|f| f.0 != CAFE),
        json!({ "index": st, "cafe": r.body }),
    );

    // a page the index has no room for is left out, saying why, and taken once there is room
    // (the index's 507 crosses the capability whole)
    let held_chunks = index(api, &keys, &name)["index"]["chunks"].as_u64().unwrap_or(0);
    let capped = lever(api, &name, "search-index", json!({ "chunks_max": held_chunks }))?;
    write(&dir, "garden/wiki/wind.md", b"# Wind\n\nA small turbine on the ridge.\n")?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let left_out = s.eventually(WAIT, || {
        let st = index(api, &keys, &name);
        st["pending"] == 0 && st["skipped"].as_array().is_some_and(|l| l.iter().any(|x| x["path"] == "garden/wiki/wind.md" && x["why"].as_str().is_some_and(|w| w.contains("full"))))
    });
    let roomy = lever(api, &name, "search-index", json!({ "chunks_max": 50_000 }))?;
    let r = api.op(&keys, &name, "changed", "after-room", json!({ "paths": ["garden/wiki/wind.md"] }))?;
    jobs::settle(api, &keys, &name, jobs::started(&r), &["succeeded", "held"], WAIT);
    let taken = search(api, &keys, &name, json!({ "q": "turbine ridge" }))?;
    s.ok(
        "a page past the index's room is left out (\"the index is full\", in index_status), and taken at its next pass once there is room",
        capped.status == 200 && out.status.success() && left_out && roomy.status == 200 && found(&taken).first().is_some_and(|f| f.0 == "garden/wiki/wind.md"),
        json!({ "index": index(api, &keys, &name), "taken": taken.body }),
    );

    // embeddings moved to another space: the first page written empties the index (`cleared`,
    // across the capability), and the brain writes every page again
    s.stop()?;
    let api = &s.start_repinned()?;
    let before = index(api, &keys, &name);
    let mut page = PAGE.to_string();
    page.push_str(BATTERIES);
    page.push_str("\n## Tilt\n\nThe panels lean thirty degrees.\n");
    write(&dir, SOLAR, page.as_bytes())?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let rebuilt = s.eventually(WAIT, || {
        let st = index(api, &keys, &name);
        st["indexing"] == false && st["index"]["semantic"] == "on"
            && search(api, &keys, &name, json!({ "q": "roof survey" })).is_ok_and(|r| results(&r).first().is_some_and(|h| h["path"] == NOTE && h["signals"].as_array().is_some_and(|s| s.iter().any(|x| x == "semantic"))))
    });
    let after = index(api, &keys, &name);
    s.ok(
        "embeddings moved to another space: one page written empties the index, and the brain writes every page again (an unchanged page found by meaning again)",
        out.status.success() && rebuilt && after["writes"].as_u64().unwrap_or(0) >= before["writes"].as_u64().unwrap_or(0) + after["pages"].as_u64().unwrap_or(0),
        json!({ "before": before, "after": after }),
    );
    s.stop()?;
    let api = &s.start(false)?;

    // behind its files (a page marked, no run on it), a search says so, and how to finish it
    let r = api.op(&keys, &name, "mark", "behind", json!({ "paths": [SOLAR], "full": false }))?;
    let behind = search(api, &keys, &name, json!({ "q": "inverter" }))?;
    s.ok(
        "an index behind its files says so: indexing, results may be incomplete, and the job that finishes it",
        r.status == 200 && behind.body["result"]["indexing"] == true && behind.body["result"]["hint"].as_str().is_some_and(|h| h.contains("changed")) && found(&behind).first() == Some(&at(SOLAR, "Inverters")),
        &behind,
    );

    // the page: the notes viewer and the brain's search, for a member, who starts the index's job
    let Some(mut chrome) = s.browser()? else {
        s.ok("Chrome is installed for the brain's page (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", &viewer_session)?;
    let tab = chrome.open(&api.site_url(&name, "__signin?return=/"))?;
    let wait = Duration::from_secs(20);
    let caught_up = s.eventually(WAIT, || index(api, &keys, &name)["indexing"] == false);
    s.ok("a member's page starts the index's job when it is behind, and the index catches up", caught_up, index(api, &keys, &name));
    let shown = chrome.until(&tab, "document.getElementById('tree')?.textContent.includes('solar-panels') && document.getElementById('content')?.textContent.includes('Wikis')", wait);
    let brand = chrome.eval(&tab, "document.getElementById('brand')?.textContent ?? ''")?;
    s.ok("the viewer shows a member the brain's tree and its landing, titled by the brain", shown && brand == "Garden", chrome.eval(&tab, "document.body.innerText.slice(0, 300)")?);
    chrome.eval(&tab, "(() => { const q = document.getElementById('q'); q.value = 'inverter'; q.dispatchEvent(new Event('input', { bubbles: true })); return true; })()")?;
    let listed = chrome.until(&tab, "document.querySelector('.brain-result .brain-title')?.textContent === 'Solar panels'", wait);
    let said = chrome.eval(&tab, "document.querySelector('.brain-result')?.innerText ?? ''")?;
    s.ok("its search box lists the ranked sections", listed && said.as_str().is_some_and(|t| t.contains("Inverters")), &said);
    // the page as a person sees it (kept with the run's scratch: FRAGMENT_E2E_KEEP)
    let _ = chrome.screenshot(&tab, &s.dir("brain-page").join("search.png"));
    chrome.click(&tab, ".brain-result .brain-title")?;
    let opened = chrome.until(&tab, "document.querySelector('#content .md')?.textContent.includes('Photovoltaic cells')", wait);
    s.ok("and a result opens its page in the viewer", opened, chrome.eval(&tab, "document.getElementById('content')?.innerText.slice(0, 200) ?? ''")?);

    // a page an agent wrote is data, whatever a model put in it (audit H2:
    // the viewer's markdown once went into innerHTML): its markup is text
    write(&dir, "garden/raw/hostile.md", include_bytes!("../../fixtures/hostile-note.md"))?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    chrome.eval(&tab, "location.hash = '#/garden/raw/hostile.md'; true")?;
    let shown = out.status.success() && chrome.until(&tab, "document.querySelector('#content .md h1')?.textContent === 'A hostile note'", wait);
    std::thread::sleep(Duration::from_millis(500));
    s.ok(
        "a brain's page shows its <img onerror>, <script>, <svg onload> and javascript: links as their text, and runs none of it",
        shown && chrome.eval(&tab, "(() => { const c = document.getElementById('content'); return window.__ran === undefined && !c.querySelector('img,script,svg,iframe,a[href^=\"javascript\" i]') && c.textContent.includes('<img src=x onerror='); })()")? == true,
        chrome.eval(&tab, "JSON.stringify({ ran: window.__ran ?? null, text: document.getElementById('content')?.innerText.slice(0, 400) })")?,
    );

    // a page that grows past 1 MiB leaves the index: no file left main, so the
    // platform forgets nothing, and only the brain's fenced delete takes it out
    // (across the capability, its source whole). Last here: a newer writer's
    // fence, set through the owner's API as a batch that took the page after
    // this one's claim lapsed would, stays for the life.
    write(&dir, GROWS, b"# Anemometer\n\nAn anemometer on the ridge counts the wind.\n")?;
    write(&dir, GUST, b"# Gust\n\nThe strongest gust this winter.\n")?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let small = s.eventually(WAIT, || {
        index(api, &keys, &name)["pending"] == 0
            && search(api, &keys, &name, json!({ "q": "anemometer" })).is_ok_and(|r| found(&r).first().is_some_and(|f| f.0 == GROWS))
            && search(api, &keys, &name, json!({ "q": "strongest gust" })).is_ok_and(|r| found(&r).first().is_some_and(|f| f.0 == GUST))
    });
    anyhow::ensure!(out.status.success() && small, "the two small pages indexed: {}", index(api, &keys, &name));
    let newer = json!({
        "source": GUST, "revision": "newer", "title": "Gust", "fields": { "wiki": "garden" },
        "sections": [{ "id": "s0", "text": "A newer writer's gale." }], "fence": 1u64 << 40,
    });
    let newer = api.signed(&keys, "POST", &format!("/api/f/{name}/search/upsert"), Some(&newer))?;
    anyhow::ensure!(newer.status == 200, "the newer writer's write: {newer}");
    write(&dir, GROWS, "An anemometer on the ridge counts the wind.\n".repeat(25_000).as_bytes())?;
    write(&dir, GUST, "The strongest gust this winter.\n".repeat(35_000).as_bytes())?;
    let out = s.cli(api, &home, &["sync", &name, "--dir", &dir_s]);
    let skipped = s.eventually(WAIT, || {
        index(api, &keys, &name)["skipped"].as_array().is_some_and(|l| l.iter().any(|x| x["path"] == GROWS && x["why"].as_str().is_some_and(|w| w.contains("1 MiB"))))
    });
    let gone = search(api, &keys, &name, json!({ "q": "anemometer" }))?;
    s.ok(
        "a page grown past 1 MiB (a blob, still at main) is taken out of the index by the brain's fenced delete, and listed as skipped",
        out.status.success() && skipped && gone.status == 200 && found(&gone).iter().all(|f| f.0 != GROWS),
        json!({ "index": index(api, &keys, &name), "anemometer": gone.body }),
    );
    let done = s.eventually(WAIT, || index(api, &keys, &name)["pending"] == 0);
    let stands = search(api, &keys, &name, json!({ "q": "gale" }))?;
    s.ok(
        "a delete below a newer writer's fence is refused across the capability whole: the brain counts it stale, fails nothing, and the newer write stands",
        done && found(&stands).first().is_some_and(|f| f.0 == GUST),
        json!({ "index": index(api, &keys, &name), "gale": stands.body }),
    );
    parent::run(s, api, &keys)?;
    Ok(())
}
