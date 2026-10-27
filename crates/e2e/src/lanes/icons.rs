//! A fragment's icon (docs/api.md, Icons; issue #587): made, kept,
//! uploaded, served and removed, end to end on workerd, its vendors the
//! Workers AI fake (a lower rung at the vendor boundary: its subject named
//! as asked, its drawings already cut out, since local Images cuts
//! nothing, and Clef's checks passed; fragment_fakes::workers_ai) and
//! local Images (miniflare's), which fits and flattens.
//!
//! A making offers `icon::CANDIDATES` drawings of one subject, each a
//! `icon::SIZE` PNG with clear corners, best first, every vendor call one
//! row on the owner's ledger; one kept is served as those bytes, by its
//! hash, to viewers and up, and is the fragment's picture in its members'
//! lists; an upload is fitted; a making whose drawings are all tiles offers
//! none and says why. Refused: a viewer's making, keeping and removing, a
//! hint too long or not text, an upload that is no image or past the body
//! limit, an agent fragment's icon, and the hour's eleventh making, also
//! when the makings come at once and the ledger is slow to answer. At its
//! cap a fragment's making is its owner's and their agents' alone, and
//! read-only (its owner past the overdraft) its icon is neither kept nor
//! removed, nothing fitted. That a node's restart keeps the icon is not
//! checked here: it needs the node (`Need::Node`), restart.rs's.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use fragment_core::icon;
use fragment_core::price::Usage;
use fragment_fakes::workers_ai::{icon_subject, Used, ICON_MODEL, ICON_SUBJECT};
use fragment_nip98::Keys;
use fragment_proto::ErrorCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::faces::row;
use super::ledger::{charge, charged, entries, forget_gate, ledger, lever, tokens_costing, usage, USD};
use crate::api::{Api, Call, Reply};
use crate::Suite;

const WAIT: Duration = Duration::from_secs(30);

/// A photograph's stand-in: a 40×24 JPEG of one colour, wider than tall,
/// so fitting it pads it.
const PHOTO: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCAAYACgDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAb/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAYH/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AngEi1YAAAAAAAAAB/9k=";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

pub(super) fn photo() -> Vec<u8> {
    b64().decode(PHOTO).expect("a fixture JPEG")
}

fn route(fragment: &str) -> String {
    format!("/api/f/{fragment}/icon")
}

fn make(api: &Api, keys: &Keys, fragment: &str, body: Value) -> Result<Reply> {
    api.signed(keys, "POST", &format!("{}/make", route(fragment)), Some(&body))
}

/// `PUT` an image's bytes as its icon, signed over them.
fn put(api: &Api, keys: &Keys, fragment: &str, bytes: &[u8]) -> Result<Reply> {
    let url = format!("{}{}", api.base, route(fragment));
    api.call(Call { method: "PUT", url, body: Some(bytes.to_vec()), content_type: Some("application/octet-stream"), keys: Some(keys), ..Call::default() })
}

/// `GET` its icon, as one holding `etag` (`if-none-match`) when one.
fn get(api: &Api, keys: &Keys, fragment: &str, etag: Option<&str>) -> Result<Reply> {
    let extra = etag.map(|e| vec![("if-none-match", e.to_string())]).unwrap_or_default();
    api.call(Call { method: "GET", url: format!("{}{}", api.base, route(fragment)), keys: Some(keys), extra, ..Call::default() })
}

/// A PNG read as the cell reads one: its sides, and whether its four
/// corners are clear (`None`: no PNG).
fn read(png: &[u8]) -> Option<(u32, u32, bool)> {
    let (w, h, alpha) = icon::alpha_of(png).ok()?;
    let corners = [0, w as usize - 1, (w * (h - 1)) as usize, (w * h) as usize - 1];
    Some((w, h, corners.iter().all(|&i| alpha[i] == 0)))
}

/// A making's candidates' PNGs, decoded.
fn pngs(r: &Reply) -> Vec<Vec<u8>> {
    r.body["candidates"].as_array().into_iter().flatten().filter_map(|c| c["png"].as_str().and_then(|p| b64().decode(p).ok())).collect()
}

/// A making's answer, told without its PNGs' base64.
fn told(r: &Reply) -> Value {
    let candidates: Vec<Value> = r.body["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let png = c["png"].as_str().and_then(|p| b64().decode(p).ok()).unwrap_or_default();
            json!({ "bytes": png.len(), "read": format!("{:?}", read(&png)), "score": c["score"] })
        })
        .collect();
    json!({ "status": r.status, "subject": r.body["subject"], "candidates": candidates, "flaws": r.body["flaws"], "error": r.body["error"], "message": r.body["message"] })
}

/// The call a meter row's reference names (`icon::meter_ref`'s last part).
fn call_of(entry: &Value) -> String {
    entry["ref"].as_str().and_then(|r| r.rsplit_once(':')).map_or(String::new(), |(_, call)| call.to_string())
}

/// Whether a ledger entry is a row of `want`'s usage, charged as the book
/// prices it.
fn priced(entry: &Value, want: impl Fn(&Usage) -> bool) -> bool {
    let used: Option<Usage> = serde_json::from_value(usage(entry).clone()).ok();
    used.is_some_and(|u| want(&u) && charged(entry) == Some(charge(&u)))
}

/// Whether a row is priced as its call's vendor bills it: a subject and a
/// check in their models' tokens, a drawing in the neurons Workers AI
/// said, and each Images step one transformation.
fn priced_as_its_call(entry: &Value) -> bool {
    let call = call_of(entry);
    match call.split('-').next().unwrap_or_default() {
        "subject" => priced(entry, |u| matches!(u, Usage::Tokens { model, .. } if model == icon::SUBJECT_MODEL)),
        "check" => priced(entry, |u| matches!(u, Usage::Tokens { model, .. } if model == icon::CHECK_MODEL.id())),
        "draw" => priced(entry, |u| *u == Usage::Neurons { milli: icon::DRAWN_MILLI_NEURONS }),
        "cut" | "fit" | "flat" | "upload" => priced(entry, |u| *u == Usage::Images { count: 1 }),
        _ => false,
    }
}

pub fn icons(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("icons", &[crate::Need::Fakes, crate::Need::Levers]) {
        return Ok(());
    }
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let name = s.named(api, &owner, "icons")?;
    s.create(api, &owner, &name)?;
    let n = icon::CANDIDATES;
    // the makings on `name` that answered, which the hour's limit counts
    let mut made = 0;

    // ---- valid: its owner makes icons
    let mark = s.ai.calls().len();
    let r = make(api, &owner, &name, json!({}))?;
    made += usize::from(r.status == 200);
    let offered = pngs(&r);
    let scores: Vec<f64> = r.body["candidates"].as_array().into_iter().flatten().filter_map(|c| c["score"].as_f64()).collect();
    s.ok(
        "its owner makes icons: 4 candidates, each a 256×256 PNG with clear corners, best first, of a subject named, nothing refused",
        r.status == 200
            && offered.len() == n
            && scores.len() == n
            && offered.iter().all(|p| read(p) == Some((icon::SIZE, icon::SIZE, true)))
            && scores.windows(2).all(|w| w[0] >= w[1])
            && scores.iter().all(|s| *s > 0.0 && *s <= 1.0)
            && r.body["subject"] == ICON_SUBJECT
            && r.body["flaws"] == json!({}),
        told(&r),
    );
    let calls = s.ai.calls().split_off(mark);
    let asked: Vec<_> = calls.iter().filter(|c| c.model == icon::SUBJECT_MODEL && icon_subject(&c.body).is_some()).collect();
    let drawn: Vec<_> = calls.iter().filter(|c| c.model == ICON_MODEL).collect();
    let checked: Vec<_> = calls.iter().filter(|c| c.model == icon::CHECK_MODEL.id()).collect();
    let seeds: BTreeSet<String> = drawn.iter().map(|c| c.body["seed"].to_string()).collect();
    s.ok(
        "at its vendors: one subject asked (unstreamed, its tokens bounded), 4 drawings of it at 512×512 each its own seed, and Clef's check of each with its picture",
        asked.len() == 1
            && asked[0].body.get("stream").is_none_or(|v| *v == json!(false))
            && asked[0].body["max_tokens"] == icon::SUBJECT_MAX_TOKENS
            && drawn.len() == n
            && seeds.len() == n
            && drawn.iter().all(|c| c.body["prompt"] == icon::prompt(ICON_SUBJECT) && c.body["width"] == icon::DRAWN_PX && c.body["height"] == icon::DRAWN_PX)
            && checked.len() == n
            && checked.iter().all(|c| c.body["images"].as_array().map(Vec::len) == Some(1)),
        json!(calls.iter().map(|c| json!({ "model": c.model, "seed": c.body["seed"], "prompt": c.body["prompt"], "max_tokens": c.body["max_tokens"] })).collect::<Vec<_>>()),
    );
    // each call a row on the owner's ledger, all of one making
    let prefix = format!("icon:{name}@");
    let wanted: BTreeSet<String> = std::iter::once("subject".to_string()).chain((0..n).flat_map(|i| ["draw", "cut", "fit", "flat", "check"].map(|step| format!("{step}-{i}")))).collect();
    let landed = s.eventually(WAIT, || {
        let _ = lever(api, &name, "meter-now", json!({ "sample": false }));
        entries(api, &owner_id, &prefix).len() >= wanted.len()
    });
    let rows = entries(api, &owner_id, &prefix);
    let named: BTreeSet<String> = rows.iter().map(call_of).collect();
    let starts: BTreeSet<&str> = rows.iter().filter_map(|e| e["ref"].as_str()?.strip_prefix(prefix.as_str())?.split(':').nth(1)).collect();
    s.ok(
        "each vendor call is one row on its owner's ledger (`icon:<name>@<life>:<started>.<nonce>:<call>`, the making named alike in each), priced by the book: the subject's and checks' tokens, the drawings' neurons as Workers AI said them, one Images transformation a step",
        landed && rows.len() == wanted.len() && named == wanted && starts.len() == 1 && starts.iter().all(|op| op.split_once('.').is_some_and(|(at, nonce)| at.parse::<i64>().is_ok() && nonce.len() == 8)) && rows.iter().all(priced_as_its_call),
        json!(rows),
    );

    // ---- one kept, as it is; served by its hash; the fragment's picture
    let chosen = offered.first().cloned().unwrap_or_default();
    let sha = hex::encode(Sha256::digest(&chosen));
    let path = icon::icon_path(&name, &sha);
    let kept = put(api, &owner, &name, &chosen)?;
    s.ok(
        "one kept (its bytes PUT) is kept as it is: the answer names its address by its SHA-256",
        kept.status == 200 && kept.body["icon"] == path.as_str() && kept.body["sha256"] == sha.as_str() && kept.body["size"] == chosen.len(),
        &kept,
    );
    let got = get(api, &owner, &name, None)?;
    let etag = format!("\"{sha}\"");
    let held = get(api, &owner, &name, Some(&etag))?;
    s.ok(
        "it is served as those very bytes, a PNG tagged by its SHA-256, and not again to one who holds it (304)",
        got.status == 200 && got.bytes == chosen && got.header("content-type") == "image/png" && got.header("etag") == etag && held.status == 304,
        format!("{} {} etag {} / {}", got.status, got.header("content-type"), got.header("etag"), held.status),
    );
    let again = put(api, &owner, &name, &chosen)?;
    s.ok("kept again, it answers the same (a replay)", again.status == 200 && again.body == kept.body, &again);
    let listed = s.eventually(WAIT, || row(api, &owner, &name)["picture"] == path.as_str());
    s.ok("its owner's list shows it as the fragment's picture", listed, row(api, &owner, &name));

    // ---- a viewer sees it and changes nothing; an outsider sees nothing
    let viewer = api.person()?;
    let r = api.share(&name, viewer.pubkey_hex(), "viewer")?;
    anyhow::ensure!(r.status == 200, "a viewer: {r}");
    let seen = get(api, &viewer, &name, None)?;
    let theirs = s.eventually(WAIT, || row(api, &viewer, &name)["picture"] == path.as_str());
    s.ok("a viewer is served it, and their list shows it too", seen.status == 200 && seen.bytes == chosen && theirs, format!("{} / {}", seen.status, row(api, &viewer, &name)));
    let before = s.ai.calls().len();
    let (m, p) = (make(api, &viewer, &name, json!({}))?, put(api, &viewer, &name, &photo())?);
    let d = api.signed(&viewer, "DELETE", &route(&name), None)?;
    let still = get(api, &owner, &name, None)?;
    s.ok(
        "a viewer makes, keeps and removes none (403 each), nothing is called, and the icon stays",
        [&m, &p, &d].iter().all(|r| r.status == 403 && r.code() == Some(ErrorCode::Forbidden)) && s.ai.calls().len() == before && still.bytes == chosen,
        format!("make {m} / put {p} / delete {d}"),
    );
    let stranger = api.person()?;
    let r = get(api, &stranger, &name, None)?;
    s.ok("someone not in it is served none (403)", r.status == 403, &r);

    // ---- an editor's upload is fitted, at its owner's charge
    let editor = api.person()?;
    let r = api.share(&name, editor.pubkey_hex(), "editor")?;
    anyhow::ensure!(r.status == 200, "an editor: {r}");
    let up = put(api, &editor, &name, &photo())?;
    let fitted = get(api, &owner, &name, None)?;
    let fitted_sha = hex::encode(Sha256::digest(&fitted.bytes));
    s.ok(
        "an editor uploads a photograph (a 40×24 JPEG): it is fitted into a 256×256 PNG padded clear, which becomes the icon",
        up.status == 200
            && fitted.status == 200
            && read(&fitted.bytes) == Some((icon::SIZE, icon::SIZE, true))
            && up.body["sha256"] == fitted_sha.as_str()
            && up.body["icon"] == icon::icon_path(&name, &fitted_sha).as_str()
            && up.body["size"] == fitted.bytes.len(),
        format!("{up} / {} {:?}", fitted.status, read(&fitted.bytes)),
    );
    let paid = s.eventually(WAIT, || {
        let _ = lever(api, &name, "meter-now", json!({ "sample": false }));
        entries(api, &owner_id, &prefix).iter().any(|e| call_of(e) == "upload" && priced_as_its_call(e))
    });
    s.ok("its fitting is one Images row on the owner's ledger (`upload`), whoever uploads", paid, json!(entries(api, &owner_id, &prefix).iter().filter(|e| call_of(e) == "upload").collect::<Vec<_>>()));

    favicon(s, api, &owner)?;

    // ---- a making whose drawings are all tiles offers none, saying why
    let r = make(api, &owner, &name, json!({ "hint": "a square" }))?;
    made += usize::from(r.status == 200);
    s.ok(
        "a making whose every drawing is a tile (its hint, a square) offers none, and counts why: {tile: 4}",
        r.status == 200 && r.body["subject"] == "a square" && r.body["candidates"] == json!([]) && r.body["flaws"] == json!({ "tile": n }),
        told(&r),
    );

    // ---- invalid hints and uploads
    let longest = "x".repeat(icon::HINT_MAX_CHARS);
    let r = make(api, &owner, &name, json!({ "hint": longest }))?;
    made += usize::from(r.status == 200);
    s.ok("a hint of 200 characters is taken", r.status == 200 && pngs(&r).len() == n, told(&r));
    let before = s.ai.calls().len();
    let long = make(api, &owner, &name, json!({ "hint": format!("{longest}x") }))?;
    let number = make(api, &owner, &name, json!({ "hint": 42 }))?;
    s.ok(
        "a hint past 200 characters, or not text, is refused (400 invalid_request) before anything is called",
        [&long, &number].iter().all(|r| r.status == 400 && r.code() == Some(ErrorCode::InvalidRequest)) && s.ai.calls().len() == before,
        format!("{long} / {number}"),
    );
    let text = put(api, &owner, &name, b"hello, I am no picture")?;
    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.resize(fragment_proto::limits::BODY_MAX_BYTES + 1, 0);
    let large = put(api, &owner, &name, &big)?;
    let unmoved = get(api, &owner, &name, None)?;
    s.ok(
        "an upload that is no image is refused (400), one past the body limit (2 MiB) 413, and the icon stays",
        text.status == 400 && text.code() == Some(ErrorCode::InvalidRequest) && large.status == 413 && unmoved.bytes == fitted.bytes,
        format!("{text} / {large}"),
    );

    // ---- an agent fragment's picture is its avatar
    let agent = s.named(api, &owner, "icons-agent")?;
    let r = api.create_with(&owner, json!({ "name": agent, "template": "agent", "title": "Juniper" }))?;
    anyhow::ensure!(r.status == 200, "the agent fragment: {r}");
    s.owned(&r.body, &owner);
    let known = s.eventually(WAIT, || row(api, &owner, &agent)["kind"] == "agent");
    let before = s.ai.calls().len();
    let (m, p) = (make(api, &owner, &agent, json!({}))?, put(api, &owner, &agent, &chosen)?);
    s.ok(
        "an agent fragment takes no icon (its picture is its avatar.png): its making and an upload are refused (400), nothing called",
        known && [&m, &p].iter().all(|r| r.status == 400 && r.code() == Some(ErrorCode::InvalidRequest)) && s.ai.calls().len() == before,
        format!("make {m} / put {p}"),
    );

    // ---- removed
    let gone = api.signed(&owner, "DELETE", &route(&name), None)?;
    let none = get(api, &owner, &name, None)?;
    let unlisted = s.eventually(WAIT, || {
        let r = row(api, &owner, &name);
        r["name"] == name.as_str() && r.get("picture").is_none()
    });
    let again = api.signed(&owner, "DELETE", &route(&name), None)?;
    s.ok(
        "removed (DELETE), none is served (404) and no list shows one; removed again, it answers the same",
        gone.status == 200 && gone.body == json!({ "icon": null }) && none.status == 404 && unlisted && again.status == 200 && again.body == gone.body,
        format!("{gone} / {none} / {} / {again}", row(api, &owner, &name)),
    );
    let events = api.signed(&owner, "GET", &format!("/api/f/{name}/events"), None)?;
    s.ok(
        "its event log tells each: icon.made, icon.set, icon.removed",
        events.status == 200 && ["icon.made", "icon.set", "icon.removed"].iter().all(|e| events.text.contains(e)),
        &events,
    );

    // ---- the hour's limit: the makings the refusals above counted none of
    let mut last = None;
    // bounded: at most MAKINGS_PER_HOUR makings
    while made < icon::MAKINGS_PER_HOUR {
        let r = make(api, &owner, &name, json!({ "hint": "a square" }))?;
        if r.status != 200 {
            last = Some(told(&r));
            break;
        }
        made += 1;
    }
    let r = make(api, &owner, &name, json!({}))?;
    let kept = put(api, &owner, &name, &chosen)?;
    s.ok(
        "the hour's 11th making of a fragment is refused (429 rate_limited), the refusals before it counting none; keeping one is no making",
        made == icon::MAKINGS_PER_HOUR && r.status == 429 && r.code() == Some(ErrorCode::RateLimited) && kept.status == 200,
        json!({ "made": made, "stopped": last, "then": told(&r), "kept": kept.to_string() }),
    );
    at_once(s, api)?;
    capped(s, api)?;
    read_only(s, api)?;
    Ok(())
}

/// The vendor calls since `mark` that asked for a subject: one a making.
fn subjects_since(s: &Suite, mark: usize) -> usize {
    s.ai.calls().get(mark..).unwrap_or_default().iter().filter(|c| c.model == icon::SUBJECT_MODEL && icon_subject(&c.body).is_some()).count()
}

/// Makings asked all at once, while the owner's ledger is slow (its gate,
/// cold, waits its 1.5 s for it): each is admitted against the others',
/// so the hour's limit holds, at most 10 started and the rest refused
/// (429) before anything is called.
fn at_once(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let name = s.named(api, &owner, "icons-race")?;
    s.create(api, &owner, &name)?;
    let slow = |ms: u64| api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": owner_id, "op": "slow", "ms": ms })));
    let slowed = slow(3_000)?;
    forget_gate(api, &owner_id)?;
    let mark = s.ai.calls().len();
    let asked = icon::MAKINGS_PER_HOUR + 2;
    let answers = std::thread::scope(|scope| {
        let each: Vec<_> = (0..asked).map(|_| scope.spawn(|| make(api, &owner, &name, json!({ "hint": "a square" })))).collect();
        each.into_iter().map(|h| h.join().expect("a making's thread")).collect::<Result<Vec<_>>>()
    });
    let restored = slow(0)?;
    let answers = answers?;
    let made = answers.iter().filter(|r| r.status == 200).count();
    let busy = answers.iter().filter(|r| r.status == 429 && r.code() == Some(ErrorCode::RateLimited)).count();
    let subjects = subjects_since(s, mark);
    s.ok(
        "12 makings asked at once, the owner's ledger slow to answer: 10 are made, 2 refused (429) before anything is called (one subject a making)",
        slowed.status == 200 && restored.status == 200 && made == icon::MAKINGS_PER_HOUR && busy == 2 && subjects == icon::MAKINGS_PER_HOUR,
        json!({ "made": made, "busy": busy, "subjects": subjects, "statuses": answers.iter().map(|r| r.status).collect::<Vec<_>>() }),
    );
    Ok(())
}

/// At its cap a fragment's making is its owner's, and their agents', alone
/// (docs/ledger.md, Caps): an editor's is refused (402) before anything is
/// called.
fn capped(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let name = s.named(api, &owner, "icons-cap")?;
    s.create(api, &owner, &name)?;
    let editor = api.person()?;
    let r = api.share(&name, editor.pubkey_hex(), "editor")?;
    anyhow::ensure!(r.status == 200, "an editor: {r}");
    let hand = Keys::generate();
    let reg = "/api/identities";
    let r = api.signed(&owner, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&hand, "POST", reg, &owner) })))?;
    anyhow::ensure!(r.status == 200, "an agent of the owner's: {r}");
    let r = api.share(&name, hand.pubkey_hex(), "editor")?;
    anyhow::ensure!(r.status == 200, "the owner's agent, an editor: {r}");
    let cap = api.signed(&owner, "PUT", &format!("/api/f/{name}/cap"), Some(&json!({ "id": "icons-cap", "micros": 0 })))?;
    let asked = |who: &Keys| -> Result<(Reply, usize)> {
        forget_gate(api, &owner_id)?;
        let mark = s.ai.calls().len();
        let r = make(api, who, &name, json!({ "hint": "a square" }))?;
        Ok((r, subjects_since(s, mark)))
    };
    let (theirs, their_calls) = asked(&editor)?;
    let (ours, our_calls) = asked(&owner)?;
    let (agents, agent_calls) = asked(&hand)?;
    s.ok(
        "at its cap ($0 here) an editor's making is refused (402, saying so) before anything is called; its owner's and their agent's are made",
        cap.status == 200
            && theirs.status == 402
            && theirs.code() == Some(ErrorCode::BudgetUsedUp)
            && theirs.message().contains("cap")
            && their_calls == 0
            && ours.status == 200
            && our_calls == 1
            && agents.status == 200
            && agent_calls == 1,
        format!("cap {cap} / editor {theirs} / owner {} / agent {}", told(&ours), told(&agents)),
    );
    Ok(())
}

/// Read-only (its owner past the overdraft), a fragment's icon stays as it
/// is: keeping one and removing it are writes, refused (402, saying why)
/// before anything is fitted, stored or called.
fn read_only(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let name = s.named(api, &owner, "icons-read-only")?;
    s.create(api, &owner, &name)?;
    let kept = put(api, &owner, &name, &photo())?;
    anyhow::ensure!(kept.status == 200, "an icon first: {kept}");
    let icon_was = get(api, &owner, &name, None)?.bytes;
    let prefix = format!("icon:{name}@");
    let uploads = || {
        let _ = lever(api, &name, "meter-now", json!({ "sample": false }));
        entries(api, &owner_id, &prefix).iter().filter(|e| call_of(e) == "upload").count()
    };
    let fitted_once = s.eventually(WAIT, || uploads() == 1);
    // past the overdraft: a making whose subject costs the balance and $3 more
    let balance = ledger(api, &owner)["balanceMicros"].as_i64().unwrap_or_default();
    s.ai.set_usage(&[Used { prompt: 10, cached: 0, completion: tokens_costing(icon::SUBJECT_MODEL, 10, balance + 3 * USD) }]);
    let r = make(api, &owner, &name, json!({ "hint": "a square" }))?;
    s.ai.clear_script();
    let over = s.eventually(WAIT, || {
        let _ = lever(api, &name, "meter-now", json!({ "sample": false }));
        ledger(api, &owner)["standing"]["standing"] == "read_only"
    });
    // what the fragment and its gate heard of the owner within the minute
    lever(api, &name, "forget-standing", json!({}))?;
    forget_gate(api, &owner_id)?;
    let calls = s.ai.calls().len();
    let p = put(api, &owner, &name, &photo())?;
    let d = api.signed(&owner, "DELETE", &route(&name), None)?;
    let still = get(api, &owner, &name, None)?;
    let refused = [&p, &d].iter().all(|r| r.status == 402 && r.code() == Some(ErrorCode::BudgetUsedUp) && r.message().contains("read-only"));
    s.ok(
        "read-only (its owner past the overdraft), keeping an icon and removing it are refused (402, saying why); nothing is called or fitted, and the icon stays",
        fitted_once && r.status == 200 && over && refused && s.ai.calls().len() == calls && still.status == 200 && still.bytes == icon_was && uploads() == 1 && row(api, &owner, &name)["picture"] == kept.body["icon"],
        format!("put {p} / delete {d} / {} / {} uploads / {}", still.status, uploads(), ledger(api, &owner)["standing"]),
    );
    Ok(())
}

/// A page that names no icon of its own is asked its favicon on the
/// fragment's origin (`favicon.ico`): the fragment's icon, revalidated by its
/// hash, to whoever may open its pages (here a share link's holder); a live
/// with its own favicon.ico serves that; with no icon, none (404).
fn favicon(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "favicon")?;
    let c = s.create(api, owner, &name)?;
    let cookie = format!("fragview={}", c["viewToken"].as_str().unwrap_or(""));
    s.commit(&c, &[("site/index.html", Some(b"<!doctype html><html><head></head><body>hi</body></html>"))]);
    s.deploy(&c);
    let ask = |etag: Option<&str>| {
        let extra = etag.map(|e| vec![("if-none-match", e.to_string())]).unwrap_or_default();
        api.call(Call { method: "GET", url: api.site_url(&name, "favicon.ico"), cookie: Some(cookie.clone()), extra, ..Call::default() })
    };
    let none = ask(None)?;
    let kept = put(api, owner, &name, &photo())?;
    let sha = kept.body["sha256"].as_str().unwrap_or("").to_string();
    let icon = get(api, owner, &name, None)?;
    let r = ask(None)?;
    let again = ask(Some(&format!("\"{sha}\"")))?;
    s.ok(
        "a page with no icon of its own is asked favicon.ico on its origin: none before, then the fragment's icon (a PNG, revalidated by its hash: 304)",
        none.status == 404
            && kept.status == 200
            && r.status == 200
            && r.header("content-type") == icon::MEDIA_TYPE
            && r.bytes == icon.bytes
            && r.header("etag") == format!("\"{sha}\"")
            && r.header("cache-control").contains("no-cache")
            && again.status == 304,
        format!("{none} / {r} / {again}"),
    );
    s.commit(&c, &[("site/favicon.ico", Some(b"\0\0\x01\0its own"))]);
    s.deploy(&c);
    let own = s.eventually(WAIT, || ask(None).is_ok_and(|r| r.status == 200 && r.bytes.ends_with(b"its own")));
    s.ok("a live with its own favicon.ico serves that, icon or not", own, ask(None)?);
    s.commit(&c, &[("site/favicon.ico", None)]);
    s.deploy(&c);
    let head = || api.call(Call { method: "HEAD", url: api.site_url(&name, "favicon.ico"), cookie: Some(cookie.clone()), ..Call::default() });
    let headed = s.eventually(WAIT, || {
        head().is_ok_and(|r| r.status == 200 && r.header("content-type") == icon::MEDIA_TYPE && r.header("etag") == format!("\"{sha}\"") && r.bytes.is_empty())
    });
    s.ok("its own favicon.ico gone, a HEAD says the icon's headers and sends no bytes", headed, head().map(|r| format!("{r} {:?}", r.headers))?);
    api.signed(owner, "DELETE", &route(&name), None)?;
    let gone = s.eventually(WAIT, || ask(None).is_ok_and(|r| r.status == 404));
    s.ok("its icon removed, and no favicon.ico of its own, none is served (404)", gone, ask(None)?);
    app_favicon(s, api, owner)
}

/// An app that answers `/favicon.ico` from its own route (App.fetch), its
/// page naming it, with no such file in its live.
const FAVICON_APP: &str = r#"import { DurableObject } from "cloudflare:workers";
export class App extends DurableObject {
  async fetch(request) {
    if (new URL(request.url).pathname !== "/favicon.ico") return new Response("no such route", { status: 404 });
    return new Response("the app's own icon", { headers: { "content-type": "image/x-icon", "etag": "\"the-app\"" } });
  }
}
"#;

/// An app's own favicon route wins over the fragment's icon: set, it
/// changes nothing there (a GET, a HEAD, a GET naming the icon's hash),
/// and removed, the app still answers.
fn app_favicon(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "favicon-app")?;
    let c = s.create(api, owner, &name)?;
    let cookie = format!("fragview={}", c["viewToken"].as_str().unwrap_or(""));
    let page: &[u8] = b"<!doctype html><html><head><link rel=\"icon\" href=\"/favicon.ico\"></head><body>hi</body></html>";
    s.commit(&c, &[("site/index.html", Some(page)), ("app.mjs", Some(FAVICON_APP.as_bytes())), ("fragment.json", Some(br#"{ "name": "favicon-app" }"#))]);
    s.deploy(&c);
    let ask = |method: &'static str, etag: Option<&str>| {
        let extra = etag.map(|e| vec![("if-none-match", e.to_string())]).unwrap_or_default();
        api.call(Call { method, url: api.site_url(&name, "favicon.ico"), cookie: Some(cookie.clone()), extra, ..Call::default() })
    };
    let the_apps = |r: &Reply, head: bool| r.status == 200 && r.header("content-type") == "image/x-icon" && (head || r.bytes == b"the app's own icon");
    let before = s.eventually(WAIT, || ask("GET", None).is_ok_and(|r| the_apps(&r, false)));
    let kept = put(api, owner, &name, &photo())?;
    let sha = kept.body["sha256"].as_str().unwrap_or("").to_string();
    let (got, head, held) = (ask("GET", None)?, ask("HEAD", None)?, ask("GET", Some(&format!("\"{sha}\"")))?);
    s.ok(
        "an app whose own route answers favicon.ico (its page naming it) keeps it with an icon set: a GET, a HEAD and a GET naming the icon's hash are the app's",
        before && kept.status == 200 && the_apps(&got, false) && the_apps(&head, true) && the_apps(&held, false),
        format!("{got} / {head} / {held}"),
    );
    let gone = api.signed(owner, "DELETE", &route(&name), None)?;
    let after = ask("GET", None)?;
    s.ok("its icon removed, the app's route still answers it", gone.status == 200 && the_apps(&after, false), &after);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The upload's fixture is a JPEG as its bytes say, wider than tall.
    #[test]
    fn the_photo_is_a_wide_jpeg() {
        assert_eq!(fragment_core::media::jpeg_size(&photo()), Some((40, 24)));
        assert_eq!(fragment_core::face::picture_type(&photo()), Some("image/jpeg"));
    }

    /// A PNG's corners are read clear or not, and what is no PNG is none.
    #[test]
    fn a_pngs_corners_are_read() {
        let drawn = fragment_fakes::workers_ai::icon_drawing(&icon::prompt(ICON_SUBJECT), 512, 512, 3);
        assert_eq!(read(&drawn), Some((512, 512, true)));
        let tile = fragment_fakes::workers_ai::icon_drawing("a square", 256, 256, 0);
        assert_eq!(read(&tile), Some((256, 256, true)));
        assert_eq!(read(&photo()), None);
        let entry = json!({ "ref": "icon:icons-k3x9@1:2:draw-3", "entry": { "entry": "row", "charge": charge(&Usage::Neurons { milli: icon::DRAWN_MILLI_NEURONS }), "row": { "usage": { "kind": "neurons", "milli": icon::DRAWN_MILLI_NEURONS } } } });
        assert_eq!(call_of(&entry), "draw-3");
        assert!(priced_as_its_call(&entry));
        let mut wrong = entry.clone();
        wrong["ref"] = json!("icon:icons-k3x9@1:2:cut-3");
        assert!(!priced_as_its_call(&wrong), "a cut is an Images step, not neurons");
    }
}
