//! A fragment's search (docs/api.md, "A fragment's search"): each life's
//! index a `SearchIndex` object of its own, the Workers AI fake embedding.
//! A fragment that wrote nothing answers `indexed: false`, embedding
//! nothing; an editor writes a source whole, chunked and embedded once; a
//! reader finds it by its words and its meaning, each hit placed in each
//! leg; a replay and a grown source embed nothing again; a stale write is
//! refused; limits are checked before anything is sent; a viewer only
//! reads, a stranger neither, and a member removed reads nothing more; a
//! push that deletes a source's file forgets it, and a forget the index's
//! outage held back, or one past a pass's hundred, never lands after the
//! source's next write; a write held at its embedding, or at the index
//! itself, while its fragment is deleted and made again writes nothing to
//! either life; a deleted fragment's index is ended, its rows gone; the
//! node restarted still holds what was written; and a deployment whose
//! tiers are private, with no private embeddings slot, indexes by words
//! alone.

use std::time::Duration;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::ledger::lever;
use crate::api::Api;
use crate::Suite;

const MODEL: &str = fragment_core::embed::MODEL_DEFAULT;

/// How many texts the embeddings model was asked for since `from`.
fn embedded(s: &Suite, from: usize) -> usize {
    s.ai.calls()[from..].iter().filter(|c| c.model == MODEL).map(|c| c.body["text"].as_array().map_or(0, Vec::len)).sum()
}

fn upsert(source: &str, revision: &str, sections: Value) -> Value {
    json!({ "source": source, "revision": revision, "title": "Trip planning", "fields": { "kind": "chat" }, "sections": sections })
}

fn turns() -> Value {
    json!([
        { "id": "t0", "text": "Person: the harbour lantern audit moved to the north annex\nAgent: noted", "at": "2026-10-29T10:00:00.000Z", "location": { "from": 0, "to": 1 } },
        { "id": "t1", "text": "Person: taxes are due in april\nAgent: I will remind you", "location": { "from": 2, "to": 3 } },
    ])
}

/// The index of `name`'s life `incarnation` (its live one's, none named),
/// as it holds: `{incarnation, index: {exists, ended, sources, chunks, held, …}}`.
fn index_of(api: &Api, name: &str, incarnation: Option<i64>, hold: Option<bool>) -> Result<Value> {
    let mut extra = json!({});
    if let Some(i) = incarnation {
        extra["incarnation"] = json!(i);
    }
    if let Some(h) = hold {
        extra["hold"] = json!(h);
    }
    let r = lever(api, name, "search-index", extra)?;
    anyhow::ensure!(r.status == 200, "{name}'s search index: {r}");
    Ok(r.body)
}

/// Whether a life's index ended with nothing left in it.
fn ended_empty(index: &Value) -> bool {
    index["index"]["ended"] == true && index["index"]["sources"] == 0 && index["index"]["chunks"] == 0
}

pub fn search(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("search", &[crate::Need::Node, crate::Need::Levers, crate::Need::Fakes]) {
        return Ok(());
    }
    let (owner, name) = checks(s, api)?;
    // ---- the node restarted holds what was written
    s.stop()?;
    let api = s.start(false)?;
    let found = api.signed(&owner, "POST", &format!("/api/f/{name}/search"), Some(&json!({ "q": "ferry friday" })))?;
    s.ok("the node restarted, what was written is searched still", found.status == 200 && found.body["results"][0]["section"] == "t2", &found);
    s.stop()?;
    let private = match s.start_as_private_tier() {
        Ok(api) => private_checks(s, &api),
        Err(e) => Err(e),
    };
    s.stop()?;
    s.start(false)?;
    private
}

/// A fragment labelled `label`, made by `owner`, its index made by a
/// first write: its name, and its life.
fn life(s: &Suite, api: &Api, owner: &Keys, label: &str) -> Result<(String, i64)> {
    let created = s.create(api, owner, label)?;
    let name = created["name"].as_str().context("its name")?;
    let w = api.signed(owner, "POST", &format!("/api/f/{name}/search/upsert"), Some(&upsert("notes/a.md", "r1", json!([{ "id": "t0", "text": "Person: the first life's note" }]))))?;
    anyhow::ensure!(w.status == 200, "{name}'s first write: {w}");
    let index = index_of(api, name, None, None)?;
    anyhow::ensure!(index["index"]["chunks"] == 1, "{name}'s index: {index}");
    Ok((name.to_string(), index["incarnation"].as_i64().context("its life")?))
}

fn checks(s: &mut Suite, api: &Api) -> Result<(Keys, String)> {
    let wait = Duration::from_secs(60);
    let owner = api.person()?;
    let reader = api.person()?;
    let stranger = api.person()?;
    let created = s.create(api, &owner, "searched")?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let path = |route: &str| format!("/api/f/{name}/search{route}");
    let r = api.share(&name, &api.identity(&reader)?, "viewer")?;
    anyhow::ensure!(r.status == 200, "a viewer: {r}");
    s.ai.clear_script();

    // ---- a fragment that wrote nothing has no index: nothing found, nothing embedded
    let calls = s.ai.calls().len();
    let none = api.signed(&reader, "POST", &path(""), Some(&json!({ "q": "harbour lantern audit" })))?;
    let status = api.signed(&reader, "GET", &path("/status"), None)?;
    s.ok(
        "a fragment that never wrote a source answers a search empty, indexed false, embedding nothing; its status says it holds nothing",
        none.status == 200 && none.body["indexed"] == false && none.body["results"] == json!([]) && embedded(s, calls) == 0 && status.status == 200 && status.body["exists"] == false,
        json!({ "found": none.body, "status": status.body }),
    );

    // ---- an editor writes a source whole; a reader finds it, by words and by meaning
    let calls = s.ai.calls().len();
    let w = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let wrote = embedded(s, calls);
    let calls = s.ai.calls().len();
    let found = api.signed(&reader, "POST", &path(""), Some(&json!({ "q": "harbour lantern audit" })))?;
    let asked = s.ai.calls()[calls..].iter().any(|c| c.model == MODEL && c.body["text"] == json!(["harbour lantern audit"]));
    let hit = &found.body["results"][0];
    let cosine = hit["semantic"]["cosine"].as_f64().unwrap_or(f64::NAN);
    let status = api.signed(&reader, "GET", &path("/status"), None)?;
    s.ok(
        "an editor's source is written whole, its sections embedded on the deployment's slot; a reader finds its section by its words and its meaning, the question embedded as theirs, with its location and its place in each leg; the answer names its space and scheme; its status counts it",
        w.status == 200 && w.body["written"] == true && wrote == 2
            && found.status == 200 && found.body["semantic"] == "on" && found.body["indexed"] == true
            && found.body["scheme"] == fragment_proto::search::SCHEME && found.body["space"]["model"] == MODEL
            && hit["source"] == "sessions/a.json" && hit["section"] == "t0" && hit["signals"] == json!(["lexical", "semantic"]) && hit["location"] == json!({ "from": 0, "to": 1 })
            && hit["lexical"]["rank"] == 1 && hit["lexical"]["bm25"].as_f64().is_some_and(|b| b > 0.0) && hit["semantic"]["rank"].as_u64().is_some_and(|r| r >= 1) && (-1.01..=1.01).contains(&cosine)
            && asked && status.body["sources"] == 1 && status.body["chunks"] == 2 && status.body["semantic"] == "on" && status.body.get("ended").is_none(),
        json!({ "write": w.body, "embedded": wrote, "found": found.body, "status": status.body }),
    );

    // ---- a replay writes and embeds nothing; a grown source embeds only its new turn
    let calls = s.ai.calls().len();
    let again = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let mut grown = turns();
    grown.as_array_mut().context("turns")?.push(json!({ "id": "t2", "text": "Person: book the ferry for friday" }));
    let more = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/a.json", "r2", grown)))?;
    s.ok(
        "the same revision again writes nothing and embeds nothing; a grown source embeds its new turn alone, its others' vectors the index's own",
        again.status == 200 && again.body["written"] == false && more.status == 200 && more.body["written"] == true && embedded(s, calls) == 1,
        json!({ "again": again.body, "more": more.body, "embedded": embedded(s, calls) }),
    );

    // ---- a stale write is refused, naming the revision it is not
    let mut stale = upsert("sessions/a.json", "r1", turns());
    stale["replaces"] = json!("r0");
    let r = api.signed(&owner, "POST", &path("/upsert"), Some(&stale))?;
    s.ok("a write that replaces a revision the source has not is 409 conflict, naming the one it has", r.status == 409 && r.body["error"] == "conflict" && r.body["current"] == "r2", &r);

    // ---- limits, before anything is embedded or sent
    let calls = s.ai.calls().len();
    let mut refused = Vec::new();
    let many: Vec<Value> = (0..=fragment_proto::search::SECTIONS_MAX).map(|i| json!({ "id": format!("t{i}"), "text": "x" })).collect();
    for (body, status, why) in [
        (upsert("sessions/b.json", "r1", json!(many)), 400, "2000 sections"),
        (upsert("sessions/b.json", "r1", json!([{ "id": "t0", "text": "x".repeat(8001) }])), 400, "past 8000"),
        (upsert("", "r1", turns()), 400, "source is named"),
        (json!({ "source": "s", "revision": "r", "sections": [], "order": 1 }), 400, "unknown field"),
        (upsert("sessions/b.json", "r1", json!([{ "id": "t0", "text": "x".repeat(4 * 1024 * 1024) }])), 413, ""),
    ] {
        let r = api.signed(&owner, "POST", &path("/upsert"), Some(&body))?;
        refused.push((r.status == status && r.message().contains(why), r.status, r.message().chars().take(120).collect::<String>()));
    }
    s.ok(
        "too many sections, a section past 8,000 characters, no source, a field the API does not know, and a body past 4 MiB are refused, saying why, nothing embedded",
        refused.iter().all(|(ok, _, _)| *ok) && embedded(s, calls) == 0,
        format!("{refused:?}"),
    );

    // ---- who may: a viewer reads only, a stranger nothing, a member removed nothing more
    let viewer_writes = api.signed(&reader, "POST", &path("/upsert"), Some(&upsert("sessions/c.json", "r1", turns())))?;
    let stranger_reads = api.signed(&stranger, "POST", &path(""), Some(&json!({ "q": "lantern" })))?;
    let gone = api.signed(&owner, "DELETE", &format!("/api/f/{name}/members/{}", api.identity(&reader)?), None)?;
    let removed_reads = api.signed(&reader, "POST", &path(""), Some(&json!({ "q": "lantern" })))?;
    s.ok(
        "a viewer may search but not write (403); a stranger may not search (403); a member removed searches no more",
        viewer_writes.status == 403 && stranger_reads.status == 403 && gone.status == 200 && removed_reads.status == 403,
        json!({ "viewer_writes": viewer_writes.status, "stranger_reads": stranger_reads.status, "removed": gone.status, "removed_reads": removed_reads.status }),
    );

    // ---- a push that deletes a source's file forgets the source, there and in its sources
    s.commit(&created, &[("sessions/d.json", Some(b"{}".as_slice()))]);
    let d = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert("sessions/d.json", "r1", json!([{ "id": "t0", "text": "Person: the zebra crossing permit" }]))))?;
    let held = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "zebra permit" })))?;
    s.commit(&created, &[("sessions/d.json", None)]);
    let forgotten = s.eventually(wait, || {
        let found = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "zebra permit", "mode": "lexical" }))).ok();
        let sources = api.signed(&owner, "GET", &path("/sources?limit=1000"), None).ok();
        let status = api.signed(&owner, "GET", &path("/status"), None).ok();
        found.is_some_and(|f| f.body["results"].as_array().is_some_and(|r| r.iter().all(|h| h["source"] != "sessions/d.json")))
            && sources.is_some_and(|x| x.body["sources"].as_array().is_some_and(|r| r.iter().all(|h| h["source"] != "sessions/d.json")))
            && status.is_some_and(|x| x.body["forgetting"] == 0 && x.body["sources"] == 1)
    });
    s.ok(
        "a push that deletes a source's file forgets the source: gone from search, from its sources and from the index, with no write of the app's",
        d.status == 200 && held.body["results"][0]["source"] == "sessions/d.json" && forgotten,
        json!({ "held": held.body }),
    );

    // ---- a forget the index's outage held back goes before its source's next write
    let e = upsert("sessions/e.json", "r1", json!([{ "id": "t0", "text": "Person: the walrus ledger" }]));
    s.commit(&created, &[("sessions/e.json", Some(b"{}".as_slice()))]);
    let first = api.signed(&owner, "POST", &path("/upsert"), Some(&e))?;
    // its next two calls into the index fail: the forget's, and the read's
    let out = lever(api, &name, "fail-search-index", json!({ "times": 2 }))?;
    s.commit(&created, &[("sessions/e.json", None)]);
    let down = api.signed(&owner, "GET", &path("/sources"), None)?;
    let restored = upsert("sessions/e.json", "r2", json!([{ "id": "t0", "text": "Person: the walrus ledger, restored" }]));
    let back = api.signed(&owner, "POST", &path("/upsert"), Some(&restored))?;
    let found = api.signed(&owner, "POST", &path(""), Some(&json!({ "q": "walrus ledger", "mode": "lexical" })))?;
    let status = api.signed(&owner, "GET", &path("/status"), None)?;
    s.ok(
        "a forget the index's outage held back is sent before its source's next write: written again during its retry's wait, the source is found at once, and nothing is left to forget it",
        first.status == 200 && out.status == 200 && down.status != 200 && back.status == 200 && back.body["written"] == true
            && found.body["results"][0]["source"] == "sessions/e.json" && found.body["results"][0]["revision"] == "r2" && status.body["forgetting"] == 0,
        json!({ "down": down.status, "back": back.body, "found": found.body, "status": status.body }),
    );

    // ---- more forgets than a pass sends: the last source's next write stays
    let notes: Vec<String> = (0..=100).map(|i| format!("notes/n{i:03}.md")).collect();
    s.commit(&created, &notes.iter().map(|p| (p.as_str(), Some(b"x".as_slice()))).collect::<Vec<_>>());
    for p in &notes {
        let r = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert(p, "r1", json!([{ "id": "t0", "text": "Person: a numbered note" }]))))?;
        anyhow::ensure!(r.status == 200, "{p}: {r}");
    }
    s.commit(&created, &notes.iter().map(|p| (p.as_str(), None)).collect::<Vec<_>>());
    let last = &notes[100];
    let again = api.signed(&owner, "POST", &path("/upsert"), Some(&upsert(last, "r2", json!([{ "id": "t0", "text": "Person: the last note, written again" }]))))?;
    let settled = s.eventually(wait, || api.signed(&owner, "GET", &path("/status"), None).is_ok_and(|r| r.body["forgetting"] == 0));
    let listed = api.signed(&owner, "GET", &path("/sources?limit=1000"), None)?;
    let kept: Vec<&str> = listed.body["sources"].as_array().into_iter().flatten().filter_map(|x| x["source"].as_str()).filter(|x| x.starts_with("notes/")).collect();
    s.ok(
        "101 files deleted in one push, more than a pass forgets: the last one's source, written again after, stays; the other 100 are forgotten",
        again.status == 200 && settled && kept == [last.as_str()],
        json!({ "again": again.body, "kept": kept }),
    );

    // ---- a write held at its embedding while its fragment is deleted and made again
    let late = |text: &str| upsert("notes/b.md", "r1", json!([{ "id": "t0", "text": text }]));
    let (a, life_a) = life(s, api, &owner, "search-life-a")?;
    s.ai.hold_embeddings();
    let calls = s.ai.calls().len();
    let (wrote, waiting, deleted, ended, remade) = std::thread::scope(|scope| {
        let wrote = scope.spawn(|| api.signed(&owner, "POST", &format!("/api/f/{a}/search/upsert"), Some(&late("Person: the late wombat"))));
        let waiting = s.eventually(wait, || embedded(s, calls) > 0);
        let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{a}"), None);
        let ended = s.eventually(wait, || index_of(api, &a, Some(life_a), None).is_ok_and(|i| ended_empty(&i)));
        let remade = s.create(api, &owner, &a);
        s.ai.release_embeddings();
        (wrote.join().expect("the write's thread"), waiting, deleted, ended, remade)
    });
    s.ai.release_embeddings();
    let (wrote, deleted, remade) = (wrote?, deleted?, remade?);
    let old = index_of(api, &a, Some(life_a), None)?;
    let now = api.signed(&owner, "GET", &format!("/api/f/{a}/search/status"), None)?;
    s.ok(
        "a write held at its embedding while its fragment is deleted and made again under its name writes nothing: refused 404, the ended life's index ended and empty for good, the new life's empty",
        waiting && deleted.status == 200 && ended && remade["name"] == a.as_str() && wrote.status == 404 && ended_empty(&old) && now.status == 200 && now.body["exists"] == false,
        json!({ "wrote": wrote.body, "old": old, "now": now.body }),
    );

    // ---- a write held at the index itself while its fragment is deleted and made again
    let (b, life_b) = life(s, api, &owner, "search-life-b")?;
    index_of(api, &b, None, Some(true))?;
    let (wrote, held, deleted, ended, remade, released) = std::thread::scope(|scope| {
        let wrote = scope.spawn(|| api.signed(&owner, "POST", &format!("/api/f/{b}/search/upsert"), Some(&late("Person: the held pangolin"))));
        let held = s.eventually(wait, || index_of(api, &b, Some(life_b), None).is_ok_and(|i| i["index"]["held"] == 1));
        let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{b}"), None);
        let ended = s.eventually(wait, || index_of(api, &b, Some(life_b), None).is_ok_and(|i| ended_empty(&i)));
        let remade = s.create(api, &owner, &b);
        let released = index_of(api, &b, Some(life_b), Some(false));
        (wrote.join().expect("the write's thread"), held, deleted, ended, remade, released)
    });
    let (wrote, deleted, remade, released) = (wrote?, deleted?, remade?, released?);
    let old = index_of(api, &b, Some(life_b), None)?;
    let now = api.signed(&owner, "GET", &format!("/api/f/{b}/search/status"), None)?;
    let found = api.signed(&owner, "POST", &format!("/api/f/{b}/search"), Some(&json!({ "q": "pangolin" })))?;
    s.ok(
        "a write the cell sent that its index takes only after the fragment was deleted and made again lands on an ended index: refused, nothing written to it, the new life's index empty",
        held && deleted.status == 200 && ended && remade["name"] == b.as_str() && released["index"]["held"] == 1 && wrote.status == 404
            && ended_empty(&old) && now.body["exists"] == false && found.body["results"] == json!([]) && found.body["indexed"] == false,
        json!({ "wrote": wrote.body, "old": old, "now": now.body, "found": found.body }),
    );

    // ---- a deleted fragment's index is ended with it, its rows gone
    let (doomed, life_d) = life(s, api, &owner, "doomed")?;
    let w = api.signed(&owner, "POST", &format!("/api/f/{doomed}/search/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{doomed}"), None)?;
    let dropped = s.eventually(wait, || index_of(api, &doomed, Some(life_d), None).is_ok_and(|i| ended_empty(&i)));
    s.ok("a deleted fragment's index is ended with its other data, its sources and chunks gone", w.status == 200 && deleted.status == 200 && dropped, json!({ "write": w.body }));
    Ok((owner, name))
}

/// On a node whose tiers are private, with no private embeddings slot: by
/// words alone, nothing sent to a model.
fn private_checks(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let created = s.create(api, &owner, "private-search")?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let calls = s.ai.calls().len();
    let w = api.signed(&owner, "POST", &format!("/api/f/{name}/search/upsert"), Some(&upsert("sessions/a.json", "r1", turns())))?;
    let found = api.signed(&owner, "POST", &format!("/api/f/{name}/search"), Some(&json!({ "q": "harbour lantern" })))?;
    let embeds = s.ai.calls()[calls..].iter().filter(|c| c.model == MODEL).count();
    let hit = &found.body["results"][0];
    s.ok(
        "a deployment whose tiers are private and whose embeddings slot is not indexes and searches by words alone: written, found, semantic off, no space or scheme, nothing sent to embed",
        w.status == 200 && found.status == 200 && found.body["semantic"] == "off" && found.body.get("space").is_none() && found.body.get("scheme").is_none()
            && hit["signals"] == json!(["lexical"]) && hit.get("semantic").is_none() && hit["lexical"]["rank"] == 1 && embeds == 0,
        json!({ "write": w.body, "found": found.body, "embeds": embeds }),
    );
    Ok(())
}
