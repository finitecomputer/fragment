//! Posts (decision R18): a channel fragment.json declares with a
//! `post` role takes records the platform appends for the poster, with no
//! app code at all. Checked in the `channels` section (the rules: the role,
//! the replay, the size, a channel without one, the deploy check, the
//! public call budget, who hears a post, a channel's posters' kind,
//! `from`, and what its members' lists show of it, `listed`) and the
//! `browser` section
//! (`fragment.post` from a page).

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use fragment_proto::{limits, ErrorCode};
use serde_json::{json, Value};

use super::app::ship;
use crate::api::{now_s, Api, Call, Reply, Socket};
use crate::browser::Lease;
use crate::Suite;

/// Channels and nothing else: no app.mjs.
const ROOM_JSON: &[u8] = include_bytes!("../../fixtures/room.json");
/// Channels an app hears through a trigger, beside a public query.
const HEARD_APP: &[u8] = include_bytes!("../../fixtures/heard.mjs");
const HEARD_JSON: &[u8] = include_bytes!("../../fixtures/heard.json");
/// A postable channel, and a mutation that fills it 64 records a call.
const WALL_APP: &[u8] = include_bytes!("../../fixtures/wall.mjs");
const WALL_JSON: &[u8] = include_bytes!("../../fixtures/wall.json");

fn post(api: &Api, keys: &Keys, name: &str, channel: &str, id: &str, body: Value) -> Result<Reply> {
    api.signed(keys, "POST", &format!("/api/f/{name}/channels/{channel}"), Some(&json!({ "id": id, "body": body })))
}

/// A browser's post, as `fragment.post` sends it: the call's door on the site.
fn site_post(api: &Api, name: &str, channel: &str, id: &str, body: Value, cookie: Option<&str>, content_type: &'static str) -> Result<Reply> {
    api.call(Call {
        method: "POST",
        url: api.site_url(name, &format!("__op/channels/{channel}")),
        body: Some(json!({ "id": id, "input": body }).to_string().into_bytes()),
        content_type: Some(content_type),
        cookie: cookie.map(str::to_string),
        ..Call::default()
    })
}

/// The records of a channel, read by `keys`.
fn records(api: &Api, keys: &Keys, name: &str, channel: &str) -> Vec<Value> {
    api.signed(keys, "GET", &format!("/api/f/{name}/channels/{channel}?after=0"), None)
        .ok()
        .and_then(|r| r.body["records"].as_array().cloned())
        .unwrap_or_default()
}

/// A public fragment with no app code: `room.json`'s channels (and, with
/// `page`, a page to open them from). Answers its name and its creation.
fn room(s: &Suite, api: &Api, owner: &Keys, base: &str, page: bool) -> Result<(String, Value)> {
    let name = s.named(api, owner, base)?;
    let c = s.create(api, owner, &name)?;
    api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "public" })))?;
    let mut files: Vec<(&str, Option<&[u8]>)> = vec![("fragment.json", Some(ROOM_JSON))];
    if page {
        files.push(("site/index.html", Some(b"<!doctype html><meta charset=\"utf-8\"><title>room</title><p>a room</p>")));
    }
    s.commit(&c, &files);
    s.deploy(&c);
    Ok((name, c))
}

/// The `channels` section's posts.
pub fn posts(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let editor = api.person()?;
    let viewer = api.person()?;
    let stranger = api.person()?;
    let (name, c) = room(s, api, &owner, "room", false)?;
    for (who, role) in [(&editor, "editor"), (&viewer, "viewer")] {
        api.share(&name, who.pubkey_hex(), role)?;
    }
    let (owner_id, editor_id) = (api.identity(&owner)?, api.identity(&editor)?);

    // the role
    let r = post(api, &editor, &name, "notes", "n1", json!({ "text": "from the editor" }))?;
    let record = &r.body["record"];
    s.ok(
        "a post at the channel's role is appended by the platform, naming its poster",
        r.status == 200
            && r.body["replayed"] == false
            && record["seq"] == 1
            && record["principal"] == editor_id.as_str()
            && record["kind"] == "message"
            && record["body"] == json!({ "text": "from the editor" }),
        &r,
    );
    let status = api.status(&owner, &name)?;
    let builds = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "code-builds" })))?;
    s.ok(
        "with no app code: the fragment has none, and loads no worker",
        status.body["code"]["sha"].is_null() && builds.body["builds"] == 0,
        format!("{} {}", status.body["code"], builds.body),
    );
    let held = records(api, &viewer, &name, "notes");
    s.ok("the channel holds the record, for its readers", held.len() == 1 && held[0]["principal"] == editor_id.as_str(), json!(held));
    let r = post(api, &viewer, &name, "notes", "n2", json!({ "text": "from a viewer" }))?;
    s.ok(
        "below the post role is 403, and appends nothing",
        r.status == 403 && r.message().contains("editor") && records(api, &owner, &name, "notes").len() == 1,
        &r,
    );
    let r = api.signed(&owner, "PUT", &format!("/api/f/{name}/members/{}", viewer.pubkey_hex()), Some(&json!({ "role": "editor" })))?;
    let promoted = post(api, &viewer, &name, "notes", "n2", json!({ "text": "from a viewer" }))?;
    s.ok("and the same post at the role goes through", r.status == 200 && promoted.status == 200 && promoted.body["record"]["seq"] == 2, &promoted);

    // the replay
    let r = post(api, &editor, &name, "notes", "n1", json!({ "text": "from the editor" }))?;
    s.ok(
        "the same id and body again answer the record, and append nothing",
        r.status == 200 && r.body["replayed"] == true && r.body["record"]["seq"] == 1 && records(api, &owner, &name, "notes").len() == 2,
        &r,
    );
    let r = post(api, &editor, &name, "notes", "n1", json!({ "text": "changed my mind" }))?;
    s.ok(
        "the same id with another body is 409, and appends nothing",
        r.code() == Some(ErrorCode::ConflictingBody) && records(api, &owner, &name, "notes").len() == 2,
        &r,
    );
    let r = post(api, &editor, &name, "chat", "n1", json!({ "text": "from the editor" }))?;
    s.ok("and so is the same id on another channel (a post is keyed by its poster and id)", r.code() == Some(ErrorCode::ConflictingBody) && records(api, &owner, &name, "chat").is_empty(), &r);
    let r = post(api, &owner, &name, "notes", "n1", json!({ "text": "from the editor" }))?;
    s.ok(
        "another poster's same id is their own: a new record",
        r.status == 200 && r.body["replayed"] == false && r.body["record"]["seq"] == 3 && r.body["record"]["principal"] == owner_id.as_str(),
        &r,
    );

    // the size: a string of n characters is n + 2 bytes of JSON
    let edge = limits::RECORD_BODY_MAX_BYTES - 2;
    let r = post(api, &editor, &name, "chat", "edge", json!("x".repeat(edge)))?;
    s.ok(
        "a body of exactly 64 KiB of JSON is appended",
        r.status == 200 && r.body["record"]["body"].to_string().len() == limits::RECORD_BODY_MAX_BYTES,
        format!("status {}", r.status),
    );
    let r = post(api, &editor, &name, "chat", "over", json!("x".repeat(edge + 1)))?;
    s.ok(
        "a byte over it is 413, and appends nothing",
        r.status == 413 && r.code() == Some(ErrorCode::TooLarge) && records(api, &owner, &name, "chat").len() == 1,
        &r,
    );

    // a channel that takes no posts, and ones that are not the app's
    let r = post(api, &owner, &name, "quiet", "q1", json!({ "text": "hush" }))?;
    s.ok(
        "a channel without a post role refuses every post, the owner's too (403)",
        r.status == 403 && r.message().contains("takes no posts") && records(api, &owner, &name, "quiet").is_empty(),
        &r,
    );
    let r = post(api, &owner, &name, "events", "e1", json!({ "summary": "forged" }))?;
    let unknown = post(api, &owner, &name, "nowhere", "u1", json!({}))?;
    s.ok("a platform channel refuses posts (403), and an undeclared one is 404", r.status == 403 && unknown.status == 404, format!("{r} {unknown}"));
    let r = api.signed(&owner, "GET", &format!("/api/f/{name}/channels"), None)?;
    let list = r.body["channels"].as_array().cloned().unwrap_or_default();
    let post_of = |n: &str| list.iter().find(|c| c["name"] == n).map(|c| (c["post"].clone(), c["signedIn"].clone())).unwrap_or_default();
    s.ok(
        "the channel list names who may post to each, and whether they must be signed in",
        post_of("chat") == (json!("public"), json!(false))
            && post_of("notes").0 == "editor"
            && post_of("signed") == (json!("public"), json!(true))
            && post_of("quiet").0.is_null()
            && post_of("events").0.is_null(),
        &r,
    );

    // the site's door: a visitor posts as their anonymous principal
    let r = site_post(api, &name, "chat", "v1", json!({ "text": "from a visitor" }), None, "application/json")?;
    let cookie = r.cookies().into_iter().find(|c| c.starts_with("fragment_anon=")).unwrap_or_default();
    s.ok(
        "a visitor posts from the site, as their anonymous principal (and gets its cookie)",
        r.status == 200 && r.body["result"]["principal"].as_str().is_some_and(|p| p.starts_with("anon:")) && !cookie.is_empty(),
        &r,
    );
    let again = site_post(api, &name, "chat", "v1", json!({ "text": "from a visitor" }), Some(&cookie), "application/json")?;
    s.ok("the visitor's retry is the same record", again.body["replayed"] == true && again.body["result"]["seq"] == r.body["result"]["seq"], &again);
    let refused = site_post(api, &name, "signed", "v3", json!({ "text": "from a visitor" }), Some(&cookie), "application/json")?;
    let signed = post(api, &stranger, &name, "signed", "s0", json!({ "text": "signed in" }))?;
    s.ok(
        "a channel that says signedIn refuses the visitor's post (401), though their role may post, and a signed-in poster's lands",
        refused.code() == Some(ErrorCode::Unauthenticated) && signed.status == 200 && records(api, &owner, &name, "signed").len() == 1,
        format!("{refused} {signed}"),
    );
    let form = site_post(api, &name, "chat", "v2", json!({ "text": "a form" }), Some(&cookie), "application/x-www-form-urlencoded")?;
    let signed = api.signed(&owner, "POST", &format!("/api/f/{name}/ops/channels%2Fchat"), Some(&json!({ "id": "o1", "input": {} })))?;
    s.ok(
        "a site post that is not JSON is refused (no cross-site forms), and the door is the site's alone",
        form.status == 400 && signed.code() == Some(ErrorCode::NoCode) && records(api, &owner, &name, "chat").len() == 2,
        format!("{form} {signed}"),
    );

    // the deploy check
    s.commit(&c, &[("fragment.json", Some(br#"{"channels":{"chat":{"read":"viewer","post":"public"}}}"#))]);
    s.deploy(&c);
    let status = api.status(&owner, &name)?;
    s.ok(
        "a post role looser than the channel's read is refused at deploy, saying why",
        status.body["code"]["error"].as_str().is_some_and(|e| e.contains("channels.chat.post (public) is looser than its read (viewer)")),
        &status.body["code"],
    );
    let r = post(api, &stranger, &name, "chat", "s1", json!({ "text": "still open" }))?;
    s.ok("and the channels deployed before it stand", r.status == 200, &r);

    heard(s, api, &owner, &stranger)?;
    posters_kind(s, api, &owner)?;
    draft_refusals(s, api, &owner)?;
    listed(s, api, &owner)?;
    newest(s, api, &owner)?;
    retention(s, api, &owner)
}

/// Drafts use the real PUT route and live socket: refusing a caller or the
/// fragment's pace must not broadcast, and successful drafts are never records.
fn draft_refusals(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let (name, _) = room(s, api, owner, "draft-refusals", false)?;
    let editor = api.person()?;
    let viewer = api.person()?;
    let stranger = api.person()?;
    for (keys, role) in [(&editor, "editor"), (&viewer, "viewer")] {
        let r = api.signed(owner, "PUT", &format!("/api/f/{name}/members/{}", keys.pubkey_hex()), Some(&json!({ "role": role })))?;
        anyhow::ensure!(r.status == 200, "adding the draft's {role}: {r}");
    }
    let editor_id = api.identity(&editor)?;
    let path = format!("/api/f/{name}/channels/notes/draft");
    let mut reader = Socket::open(api, &name, "__live", Some(&viewer), None)?;
    reader.until("hello", 5)?;
    reader.send(&json!({ "type": "subscribe", "channel": "notes", "after": 0 }))?;
    reader.until("subscribed", 5)?;

    let body = json!({ "turn": "refused", "text": "must not be broadcast" });
    let unsigned = api.unsigned("PUT", &path, Some(&body))?;
    let outsider = api.signed(&stranger, "PUT", &path, Some(&body))?;
    let below = api.signed(&viewer, "PUT", &path, Some(&body))?;
    s.ok(
        "the draft PUT refuses an unsigned caller (401), a signed stranger and a viewer below its post role (403)",
        unsigned.status == 401 && unsigned.code() == Some(ErrorCode::Unauthenticated)
            && outsider.status == 403 && outsider.code() == Some(ErrorCode::Forbidden)
            && below.status == 403 && below.code() == Some(ErrorCode::Forbidden),
        format!("{unsigned} {outsider} {below}"),
    );
    let accepted = api.signed(&editor, "PUT", &path, Some(&json!({ "turn": "accepted", "text": "the editor's draft" })))?;
    let heard = reader.until("draft", 10)?;
    let stored = api.signed(owner, "GET", &format!("/api/f/{name}/channels/notes"), None)?;
    s.ok(
        "the first draft a reader receives is the allowed editor's, never a refused caller's",
        accepted.status == 200 && heard["channel"] == "notes" && heard["principal"] == editor_id.as_str()
            && heard["turn"] == "accepted" && heard["text"] == "the editor's draft"
            && stored.status == 200 && stored.body["records"] == json!([]),
        json!({ "status": accepted.status, "heard": heard }),
    );

    // One bounded simultaneous burst, no retrying PUTs. The cell's clock
    // remains real: exact boundary/refill arithmetic is covered in core/live.rs.
    // If a runner is too slow to reach the limiter, fail with the elapsed time;
    // do not silently skip the 429 assertion or turn a transport error into one.
    const REQUESTS: usize = 64;
    let barrier = std::sync::Barrier::new(REQUESTS);
    let started = std::time::Instant::now();
    let replies: Vec<(String, Reply)> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..REQUESTS).map(|i| {
            let (barrier, path, editor) = (&barrier, &path, &editor);
            scope.spawn(move || {
                let turn = format!("burst-{i}");
                barrier.wait();
                api.signed(editor, "PUT", path, Some(&json!({ "turn": turn, "text": "latest text" }))).map(|r| (turn, r))
            })
        }).collect();
        threads.into_iter().map(|t| t.join().expect("draft request thread")).collect::<Result<_>>()
    })?;
    let elapsed = started.elapsed();
    let allowed: std::collections::BTreeSet<_> = replies.iter().filter(|(_, r)| r.status == 200).map(|(turn, _)| turn.clone()).collect();
    let limited: Vec<_> = replies.iter().filter(|(_, r)| r.status == 429 && r.code() == Some(ErrorCode::RateLimited)
        && r.message() == format!("drafts past {} a second are dropped", limits::PRESENCE_PER_S)).map(|(turn, _)| turn.clone()).collect();
    s.ok(
        "a bounded draft PUT burst reaches the fragment's own 429 limiter, with no other errors",
        !allowed.is_empty() && !limited.is_empty() && allowed.len() + limited.len() == REQUESTS,
        json!({ "allowed": allowed.len(), "limited": limited.len(), "elapsedMs": elapsed.as_millis(), "statuses": replies.iter().map(|(_, r)| (r.status, r.code())).collect::<Vec<_>>() }),
    );

    // More than a full burst's debt can drain, regardless of which concurrent
    // request was last admitted. Reuse a refused turn to show it was not stored.
    std::thread::sleep(Duration::from_millis(((limits::PRESENCE_BURST + 1) * fragment_core::live::PRESENCE_EVERY_MS) as u64));
    let retry_turn = limited.first().map(String::as_str).unwrap_or("recovery-without-429");
    let recovered = api.signed(&editor, "PUT", &path, Some(&json!({ "turn": retry_turn, "text": "after the pace recovers" })))?;
    anyhow::ensure!(recovered.status == 200, "the draft after refill: {recovered}");
    let mut delivered = Vec::new();
    let mut recovery = None;
    for _ in 0..REQUESTS + 10 {
        let frame = reader.next()?;
        if frame["type"] != "draft" { continue; }
        if frame["turn"] == retry_turn && frame["text"] == "after the pace recovers" {
            recovery = Some(frame);
            break;
        }
        delivered.push(frame);
    }
    let turns: std::collections::BTreeSet<_> = delivered.iter().filter_map(|f| f["turn"].as_str().map(str::to_string)).collect();
    let stored = api.signed(owner, "GET", &format!("/api/f/{name}/channels/notes"), None)?;
    s.ok(
        "only accepted burst drafts arrive once; a refused turn can be sent after refill, and drafts append no records",
        !limited.is_empty() && turns == allowed && delivered.len() == allowed.len()
            && delivered.iter().all(|f| f["channel"] == "notes" && f["principal"] == editor_id.as_str() && f["text"] == "latest text")
            && recovery.as_ref().is_some_and(|f| f["channel"] == "notes" && f["principal"] == editor_id.as_str())
            && stored.status == 200 && stored.body["records"] == json!([]),
        json!({ "allowed": allowed, "delivered": delivered, "recovery": recovery }),
    );
    reader.close();
    Ok(())
}

/// A fragment with a channel its agents post to that any member reads, and
/// a desk only its editors read, both `listed`.
pub(super) const LISTED_JSON: &str = r#"{"channels":{
  "work":{"read":"viewer","post":"editor","from":"agent","listed":{"kinds":["turn.start","turn.asked","turn.end"],"fields":["asks"]}},
  "desk":{"read":"editor","post":"editor","listed":{"kinds":["note"],"fields":["text"]}}}}"#;

/// `keys`' row of `name` in their list, or null when it lists none.
pub(super) fn row_of(api: &Api, keys: &Keys, name: &str) -> Value {
    let r = api.signed(keys, "GET", "/api/fragments", None).ok();
    r.and_then(|r| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name).cloned())).unwrap_or(Value::Null)
}

/// What `keys`' row of `name` lists: `[(channel, principal, kind, fields)]`,
/// the newest first.
pub(super) fn listed_of(api: &Api, keys: &Keys, name: &str) -> Vec<(String, String, String, Value)> {
    let row = row_of(api, keys, name);
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    row["listed"].as_array().map(|l| l.iter().map(|r| (text(&r["channel"]), text(&r["principal"]), text(&r["kind"]), r["fields"].clone())).collect()).unwrap_or_default()
}

/// `json` committed as the fragment's manifest, and deployed.
/// Deploys `json` as `name`'s fragment.json, then waits for the deploy's
/// card shot to land: the `cards` lever read, its state there, with
/// `wanted` none. The shot runs in the fragment's alarm, which sends the
/// listed rows that wait for their gap: a shot out (the first ones wait on
/// the browser's cold start, 20 to 35 s on a busy runner) holds those
/// sends, and the checks below time them. A read that fails is read again
/// within the bound; a shot that never lands fails here, saying its state.
fn redeploy(s: &Suite, api: &Api, c: &Value, name: &str, json: &str) -> Result<()> {
    s.commit(c, &[("fragment.json", Some(json.as_bytes()))]);
    s.deploy(c);
    let mut last = Value::Null;
    let landed = s.eventually(Duration::from_secs(120), || {
        let read = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "cards" })));
        let (shown, landed) = match read {
            Ok(r) if r.status == 200 && r.body["cards"].is_object() => (r.body["cards"].clone(), r.body["cards"].get("wanted") == Some(&Value::Null)),
            Ok(r) => (json!({ "status": r.status, "body": r.body }), false),
            Err(e) => (json!({ "error": e.to_string() }), false),
        };
        last = shown;
        landed
    });
    anyhow::ensure!(landed, "{name}'s deploy's card shot did not land within 120 s: {last}");
    Ok(())
}

/// The next frame a list's socket is told within `wait`, or none.
fn told_within(w: &mut Socket, wait: Duration) -> Option<Value> {
    w.patience(wait).ok()?;
    let frame = w.next().ok();
    let _ = w.patience(Duration::from_secs(5));
    frame
}

/// A channel's `listed` (docs/api.md, Listed records): each member's newest
/// record of a listed kind rides the rows of the members whose role reads
/// its channel. Goal: a list follows what a fragment's members are doing,
/// told to their open page as it changes and a few times a burst, never
/// one a record; it never shows a member what their role may not read,
/// nor what a member who left did, nor what is listed no more. Method: a fragment with an agents'
/// channel any member reads and a desk only editors read; its owner, a
/// viewer, an editor, an outsider and the owner's agent; each one's list
/// read signed, and the viewer's watched with their key, as the CLI does.
/// Each check is one only the mechanism it names makes pass: a re-send the
/// change would cause anyway is not counted as the proof.
fn listed(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "listed")?;
    let c = s.create(api, owner, &name)?;
    let (viewer, editor, outsider) = (api.person()?, api.person()?, api.person()?);
    let (viewer_id, editor_id) = (api.identity(&viewer)?, api.identity(&editor)?);
    let (agent, agent_id) = super::delegation::agent_of(api, owner)?;
    let set_role = |who: &str, role: &str| api.share(&name, who, role);
    for (who, role) in [(viewer_id.as_str(), "viewer"), (editor_id.as_str(), "editor"), (agent_id.as_str(), "editor")] {
        let r = set_role(who, role)?;
        anyhow::ensure!(r.status == 200, "adding {who} as {role}: {r}");
    }
    redeploy(s, api, &c, &name, LISTED_JSON)?;
    let r = api.signed(owner, "GET", &format!("/api/f/{name}/channels"), None)?;
    let declared = r.body["channels"].as_array().and_then(|l| l.iter().find(|c| c["name"] == "work").map(|c| c["listed"].clone())).unwrap_or_default();
    s.ok(
        "a channel's listed is installed, and the channel list names it",
        declared == json!({ "kinds": ["turn.start", "turn.asked", "turn.end"], "fields": ["asks"] }),
        &r,
    );

    // the viewer's open page (a key's socket), past what setting up told it
    let url = format!("{}/api/fragments/watch", api.base);
    let (mut tab, _) = Socket::connect(api, &url, Some(&viewer), None, None)?;
    tab.expect("hello")?;
    while told_within(&mut tab, Duration::from_secs(2)).is_some() {}
    let work = |id: &str, body: Value| post(api, &agent, &name, "work", id, body);
    let start = work("w1", json!({ "kind": "turn.start", "turn": "t1", "agent": agent_id, "asker": viewer_id }))?;
    let changed = told_within(&mut tab, Duration::from_secs(10));
    let mine = row_of(api, &viewer, &name);
    let at = start.body["record"]["at"].clone();
    s.ok(
        "an agent's listed record reaches a member's row, its kind and when, and their open page is told at once",
        start.status == 200
            && changed == Some(json!({ "type": "changed" }))
            && mine["listed"] == json!([{ "channel": "work", "principal": agent_id, "kind": "turn.start", "at": at }]),
        json!({ "told": changed, "row": mine }),
    );

    let step = work("w2", json!({ "kind": "turn.step", "turn": "t1", "step": 1 }))?;
    let same = work("w3", json!({ "kind": "turn.start", "turn": "t2", "agent": agent_id, "asker": viewer_id }))?;
    let again = work("w1", json!({ "kind": "turn.start", "turn": "t1", "agent": agent_id, "asker": viewer_id }))?;
    let quiet = told_within(&mut tab, Duration::from_millis(limits::LISTED_MIN_GAP_MS as u64 + 2000));
    let mine = row_of(api, &viewer, &name);
    s.ok(
        "records that show nothing new move nothing: a kind not listed, the same kind again, a post replayed (the page is told nothing; the row keeps the first's time)",
        step.status == 200 && same.status == 200 && again.body["replayed"] == true && quiet.is_none() && mine["listed"][0]["at"] == at && mine["listed"].as_array().map(Vec::len) == Some(1),
        json!({ "told": quiet, "row": mine }),
    );

    let asked = work("w4", json!({ "kind": "turn.asked", "turn": "t2", "asks": viewer_id, "text": "never listed" }))?;
    let changed = told_within(&mut tab, Duration::from_secs(10));
    let shown = listed_of(api, &viewer, &name);
    s.ok(
        "another kind moves it, with its listed fields and no others",
        asked.status == 200 && changed.is_some() && shown == [("work".to_string(), agent_id.clone(), "turn.asked".to_string(), json!({ "asks": viewer_id }))],
        json!({ "told": changed, "listed": shown }),
    );

    // a burst of moves is a send or two, the last with the newest record
    let t0 = std::time::Instant::now();
    let kinds = ["turn.start", "turn.end", "turn.start", "turn.end", "turn.start", "turn.end"];
    let mut final_at = Value::Null;
    for (i, kind) in kinds.iter().enumerate() {
        let r = work(&format!("b{i}"), json!({ "kind": kind, "turn": format!("b{i}") }))?;
        anyhow::ensure!(r.status == 200, "the burst's post {i}: {r}");
        final_at = r.body["record"]["at"].clone();
    }
    let burst = t0.elapsed();
    // the send that waits for the gap's end goes when the fragment's alarm
    // runs, however late a loaded runner runs it (a fixed window after the
    // gap failed while the alarm ran seconds late): the row ending on the
    // burst's last record (its own time: b1 and b3 end turns too), each
    // frame counted as it comes
    let (mut frames, mut last) = (0, Value::Null);
    let ended = s.eventually(Duration::from_secs(60), || {
        while told_within(&mut tab, Duration::from_millis(250)).is_some() {
            frames += 1;
        }
        last = row_of(api, &viewer, &name)["listed"][0].clone();
        last["kind"] == "turn.end" && last["at"] == final_at
    });
    // and the frame that told the page of that row: sent as the row was
    // applied, before the read that saw it was answered, so it is on its
    // way or here; read now, so none is left for the checks after
    while told_within(&mut tab, Duration::from_secs(2)).is_some() {
        frames += 1;
    }
    // a burst that outlasts a gap may take one send more a gap
    let allowed = 2 + burst.as_millis() as i64 / limits::LISTED_MIN_GAP_MS;
    s.ok(
        &format!("a burst of {} moves within a gap is told at most twice, not once a record, and the row ends on the newest", kinds.len()),
        ended && frames >= 1 && frames <= allowed,
        json!({ "frames": frames, "allowed": allowed, "burstMs": burst.as_millis() as u64, "newest": last, "lastAt": final_at }),
    );

    // the desk is the editors': the viewer's row never carries it
    let note = post(api, &editor, &name, "desk", "d1", json!({ "kind": "note", "text": "the budget is 4k" }))?;
    let owners = s.eventually(Duration::from_secs(10), || listed_of(api, owner, &name).iter().any(|r| r.0 == "desk"));
    let quiet = told_within(&mut tab, Duration::from_millis(limits::LISTED_MIN_GAP_MS as u64 + 2000));
    let (theirs, editors) = (listed_of(api, &viewer, &name), listed_of(api, &editor, &name));
    let outside = row_of(api, &outsider, &name);
    s.ok(
        "a channel only editors read lists to the owner's and the editor's rows alone: the viewer's row carries none of it and their page is told nothing; an outsider's list has no row",
        note.status == 200
            && owners
            && editors.iter().any(|r| r.0 == "desk" && r.1 == editor_id && r.3 == json!({ "text": "the budget is 4k" }))
            && theirs.iter().all(|r| r.0 == "work")
            && quiet.is_none()
            && outside.is_null(),
        json!({ "viewer": theirs, "editor": editors, "told": quiet, "outsider": outside }),
    );

    // a role changed sends the row again: given, then taken
    let up = set_role(&viewer_id, "editor")?;
    let told_up = told_within(&mut tab, Duration::from_secs(10));
    let raised = listed_of(api, &viewer, &name);
    let down = set_role(&editor_id, "viewer")?;
    let lowered = s.eventually(Duration::from_secs(10), || listed_of(api, &editor, &name).iter().all(|r| r.0 != "desk"));
    s.ok(
        "a role changed sends the member's row again: raised to editor it carries the desk; lowered to viewer it no longer does",
        up.status == 200 && told_up.is_some() && raised.iter().any(|r| r.0 == "desk") && down.status == 200 && lowered,
        json!({ "raised": raised, "lowered": listed_of(api, &editor, &name) }),
    );
    tab.close();

    // a deploy that tightens a channel's read takes it from the rows that may no longer read it
    let tightened = LISTED_JSON.replacen(r#""work":{"read":"viewer""#, r#""work":{"read":"editor""#, 1);
    redeploy(s, api, &c, &name, &tightened)?;
    let gone = s.eventually(Duration::from_secs(20), || listed_of(api, &editor, &name).is_empty());
    let (kept, owners) = (listed_of(api, &viewer, &name), listed_of(api, owner, &name));
    s.ok(
        "a deploy that tightens work's read to editors takes its record from the row of the member now a viewer (none left), and the editors' rows keep it",
        gone && kept.iter().any(|r| r.0 == "work") && owners.iter().any(|r| r.0 == "work"),
        json!({ "now a viewer": listed_of(api, &editor, &name), "now an editor": kept, "owner": owners }),
    );

    // an invalid listed is refused at deploy, and what was installed stands
    let bad = tightened.replacen(r#""kinds":["note"]"#, r#""kinds":["a note"]"#, 1);
    redeploy(s, api, &c, &name, &bad)?;
    let status = api.status(owner, &name)?;
    s.ok(
        "a listed kind that is no kind is refused at deploy, saying why, and the rows keep what they carried",
        status.body["code"]["error"].as_str().is_some_and(|e| e.contains("channels.desk.listed.kinds has \"a note\", which is no kind")) && listed_of(api, owner, &name) == owners,
        &status.body["code"],
    );

    // a person who leaves takes their record from every other row: the
    // other editor's row is sent again only because of it
    let r = api.signed(owner, "DELETE", &format!("/api/f/{name}/members/{editor_id}"), None)?;
    let left = s.eventually(Duration::from_secs(20), || listed_of(api, &viewer, &name).iter().all(|r| r.1 != editor_id));
    s.ok(
        "a person removed takes their listed record from another member's row (the desk's note), which nothing else sends again",
        r.status == 200 && left,
        json!({ "other": listed_of(api, &viewer, &name) }),
    );
    // a kind listed no more leaves every row, and is dropped from what the
    // fragment keeps: listed again, it starts from its next record, never
    // the old one (which only the drop keeps from coming back)
    let ends = |json: &str| json.replacen(r#""kinds":["turn.start","turn.asked","turn.end"]"#, r#""kinds":["turn.start","turn.asked"]"#, 1);
    redeploy(s, api, &c, &name, &ends(&tightened))?;
    let dropped = s.eventually(Duration::from_secs(20), || listed_of(api, owner, &name).iter().all(|r| r.0 != "work"));
    redeploy(s, api, &c, &name, &tightened)?;
    let relisted = s.eventually(Duration::from_secs(20), || {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/channels"), None).ok();
        r.is_some_and(|r| r.body["channels"].as_array().is_some_and(|l| l.iter().any(|c| c["name"] == "work" && c["listed"]["kinds"].as_array().is_some_and(|k| k.len() == 3))))
    });
    // the install sent every row again: an old turn.end kept would show now
    std::thread::sleep(Duration::from_millis(limits::LISTED_MIN_GAP_MS as u64));
    let after = listed_of(api, owner, &name);
    let next = work("n1", json!({ "kind": "turn.end", "turn": "n1" }))?;
    let fresh = s.eventually(Duration::from_secs(20), || listed_of(api, owner, &name).iter().any(|r| r.0 == "work" && r.2 == "turn.end"));
    s.ok(
        "a kind listed no more leaves every row; listed again, the old record does not come back, and the next one shows",
        dropped && relisted && after.iter().all(|r| r.0 != "work") && next.status == 200 && fresh,
        json!({ "afterRelisting": after, "now": listed_of(api, owner, &name) }),
    );

    // a listing dropped, and a channel removed, take theirs from every row
    let mine = post(api, owner, &name, "desk", "d2", json!({ "kind": "note", "text": "mine" }))?;
    let noted = s.eventually(Duration::from_secs(10), || listed_of(api, owner, &name).iter().any(|r| r.0 == "desk"));
    anyhow::ensure!(mine.status == 200 && noted, "the owner's note: {mine}");
    let without_desk = r#"{"channels":{"work":{"read":"editor","post":"editor","from":"agent","listed":{"kinds":["turn.start","turn.asked","turn.end"],"fields":["asks"]}}}}"#;
    redeploy(s, api, &c, &name, without_desk)?;
    let no_desk = s.eventually(Duration::from_secs(20), || !listed_of(api, owner, &name).iter().any(|r| r.0 == "desk"));
    s.ok("a deploy that removes a listed channel takes its records from every row", no_desk && listed_of(api, owner, &name).iter().any(|r| r.0 == "work"), json!(listed_of(api, owner, &name)));
    redeploy(s, api, &c, &name, r#"{"channels":{"work":{"read":"editor","post":"editor","from":"agent"}}}"#)?;
    let none = s.eventually(Duration::from_secs(20), || listed_of(api, owner, &name).is_empty() && listed_of(api, &viewer, &name).is_empty());
    s.ok("one that drops a channel's listed (the channel kept) takes its records from every row", none, json!([listed_of(api, owner, &name), listed_of(api, &viewer, &name)]));
    listed_posters(s, api)
}

/// A listed channel keeps its `LISTED_POSTERS_MAX` posters seen latest.
/// Goal: past the bound the poster seen longest ago goes, and one who
/// posts the same again (moving nothing) still counts as seen. Method: one
/// more agent member than the bound, each posting once in turn, the first
/// posting the same again before the last.
fn listed_posters(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let name = s.named(api, &owner, "listed-many")?;
    let c = s.create(api, &owner, &name)?;
    s.commit(&c, &[("fragment.json", Some(br#"{"channels":{"wall":{"post":"editor","listed":{"kinds":["mood"]}}}}"#))]);
    s.deploy(&c);
    let mut posters = vec![];
    for _ in 0..=limits::LISTED_POSTERS_MAX {
        let (keys, id) = super::delegation::agent_of(api, &owner)?;
        let r = api.signed(&owner, "PUT", &format!("/api/f/{name}/members/{id}"), Some(&json!({ "role": "editor" })))?;
        anyhow::ensure!(r.status == 200, "adding a poster: {r}");
        posters.push((keys, id));
    }
    let mood = |i: usize, id: &str| post(api, &posters[i].0, &name, "wall", id, json!({ "kind": "mood", "n": i }));
    for i in 0..limits::LISTED_POSTERS_MAX {
        let r = mood(i, "m1")?;
        anyhow::ensure!(r.status == 200, "poster {i}: {r}");
    }
    let again = mood(0, "m2")?;
    let last = mood(limits::LISTED_POSTERS_MAX, "m1")?;
    let ids = |l: Vec<(String, String, String, Value)>| l.into_iter().map(|r| r.1).collect::<Vec<_>>();
    let full = s.eventually(Duration::from_secs(20), || ids(listed_of(api, &owner, &name)).contains(&posters[limits::LISTED_POSTERS_MAX].1));
    let shown = ids(listed_of(api, &owner, &name));
    s.ok(
        &format!("past {} posters the one seen longest ago goes: the second (the first posted the same again, and stays)", limits::LISTED_POSTERS_MAX),
        again.status == 200 && last.status == 200 && full && shown.len() == limits::LISTED_POSTERS_MAX && shown.contains(&posters[0].1) && !shown.contains(&posters[1].1),
        json!({ "shown": shown.len(), "first": shown.contains(&posters[0].1), "second": shown.contains(&posters[1].1) }),
    );
    Ok(())
}

/// A fragment whose public wall keeps each poster's newest `mood` (and
/// fills 64 records a call, `fill`), whose desk only editors read keeps
/// each one's newest `note`, a channel that keeps four kinds, and one
/// that keeps none.
fn newest_json(wall_keeps: bool) -> String {
    let keeps = if wall_keeps { r#","newest":["mood"]"# } else { "" };
    format!(
        r#"{{"operations":{{"fill":{{"kind":"mutation","input":{{"type":"object","required":["n"],"properties":{{"n":{{"type":"integer","minimum":1,"maximum":64}}}}}}}}}},
  "channels":{{"wall":{{"read":"public","post":"editor"{keeps}}},"desk":{{"read":"editor","post":"editor","newest":["note"]}},
  "many":{{"read":"viewer","post":"editor","newest":["k0","k1","k2","k3"]}},"quiet":{{"read":"viewer","post":"editor"}}}}}}"#
    )
}

/// A channel's `newest` (docs/api.md, Newest records): each poster's
/// newest record of a kind it keeps, in one read, however far back it was
/// posted. Goal: a page asks for the latest state each poster said (a
/// chat's agents' helpers working) instead of reading history back, and
/// learns nothing its role may not read. Method: a public fragment
/// (`newest_json`); two editors post on the wall, a fill puts 1,024
/// records past them, then a viewer reads by the API and a visitor by the
/// page's door; nine editors fill the bound; a deploy stops and starts
/// keeping, and a lowered retention drops what the newest held.
fn newest(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "newest")?;
    let c = s.create(api, owner, &name)?;
    api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "public" })))?;
    let viewer = api.person()?;
    let mut posters = vec![];
    for _ in 0..9 {
        let keys = api.person()?;
        let r = api.share(&name, &api.identity(&keys)?, "editor")?;
        anyhow::ensure!(r.status == 200, "adding an editor: {r}");
        posters.push(keys);
    }
    let r = api.share(&name, &api.identity(&viewer)?, "viewer")?;
    anyhow::ensure!(r.status == 200, "adding the viewer: {r}");
    ship(s, &c, WALL_APP, newest_json(true).as_bytes());
    let (ada, bo) = (&posters[0], &posters[1]);
    let (ada_id, bo_id) = (api.identity(ada)?, api.identity(bo)?);
    // the deploy lands: the first post again until it has
    s.eventually(Duration::from_secs(15), || post(api, ada, &name, "wall", "m1", json!({ "kind": "mood", "n": 1 })).is_ok_and(|r| r.status == 200));
    let read = |keys: &Keys, channel: &str| api.signed(keys, "GET", &format!("/api/f/{name}/channels/{channel}/newest"), None);
    // each record's (poster, n), newest first
    let said = |r: &Reply| -> Vec<(String, i64)> {
        r.body["records"].as_array().map(|l| l.iter().map(|x| (x["principal"].as_str().unwrap_or_default().to_string(), x["body"]["n"].as_i64().unwrap_or(-1))).collect()).unwrap_or_default()
    };

    let chatter = post(api, ada, &name, "wall", "x1", json!({ "kind": "chatter", "n": 9 }))?;
    let b1 = post(api, bo, &name, "wall", "m1", json!({ "kind": "mood", "n": 2 }))?;
    let a2 = post(api, ada, &name, "wall", "m2", json!({ "kind": "mood", "n": 3 }))?;
    let fill = |id: &str| api.op(owner, &name, "fill", id, json!({ "n": 64 }));
    let mut filled = 0;
    for i in 0..16 {
        filled += i64::from(fill(&format!("f{i}"))?.status == 200) * 64;
    }
    let past = s.eventually(Duration::from_secs(60), || {
        let page = api.signed(owner, "GET", &format!("/api/f/{name}/channels/wall?before=1000000&limit=1000"), None);
        page.is_ok_and(|p| p.body["records"][0]["seq"].as_i64().is_some_and(|first| first > a2.body["record"]["seq"].as_i64().unwrap_or(i64::MAX)))
    });
    let r = read(&viewer, "wall")?;
    s.ok(
        "a channel's newest are each poster's newest record of a kind it keeps, the newest first, found past a page of others (a viewer, by the API)",
        chatter.status == 200 && b1.status == 200 && a2.status == 200 && filled == 1024 && past && r.status == 200
            && said(&r) == vec![(ada_id.clone(), 3), (bo_id.clone(), 2)]
            && r.body["records"][0]["seq"] == a2.body["record"]["seq"] && r.body["records"][0]["kind"] == "message",
        &r,
    );
    let door = api.call(Call { method: "GET", url: api.site_url(&name, "__channel/wall/newest"), ..Call::default() })?;
    s.ok("a visitor's page reads the same through its door (`__channel/wall/newest`), as the wall's public read allows", door.status == 200 && said(&door) == said(&r), &door);

    let again = post(api, ada, &name, "wall", "m2", json!({ "kind": "mood", "n": 3 }))?;
    let older = post(api, ada, &name, "wall", "m1", json!({ "kind": "mood", "n": 1 }))?;
    let r = read(&viewer, "wall")?;
    s.ok(
        "a post replayed moves nothing: neither its newest again nor an older one",
        again.body["replayed"] == true && older.body["replayed"] == true && said(&r) == vec![(ada_id.clone(), 3), (bo_id.clone(), 2)],
        &r,
    );

    let note = post(api, ada, &name, "desk", "n1", json!({ "kind": "note", "n": 4 }))?;
    let (desk_viewer, desk_editor) = (read(&viewer, "desk")?, read(bo, "desk")?);
    let quiet = post(api, ada, &name, "quiet", "q1", json!({ "kind": "mood", "n": 5 }))?;
    let (none, missing, bad) = (read(&viewer, "quiet")?, read(&viewer, "nowhere")?, read(&viewer, "Bad%20Name")?);
    let door_desk = api.call(Call { method: "GET", url: api.site_url(&name, "__channel/desk/newest"), ..Call::default() })?;
    s.ok(
        "its read is the channel's: a viewer may not read an editors' desk's (403, its door 401 or 403 to a visitor) while an editor does; a channel that keeps none answers none; no such channel is 404, a bad name 400",
        note.status == 200 && desk_viewer.status == 403 && desk_editor.status == 200 && said(&desk_editor) == vec![(ada_id.clone(), 4)]
            && (door_desk.status == 401 || door_desk.status == 403)
            && quiet.status == 200 && none.status == 200 && said(&none).is_empty()
            && missing.status == 404 && bad.status == 400,
        format!("{desk_viewer} {desk_editor} {door_desk} {none} {missing} {bad}"),
    );

    // nine posters, four kinds: 36 newest, past the bound
    for (i, keys) in posters.iter().enumerate() {
        for k in 0..4 {
            let r = post(api, keys, &name, "many", &format!("k{k}"), json!({ "kind": format!("k{k}"), "n": i * 4 + k }))?;
            anyhow::ensure!(r.status == 200, "poster {i} kind {k}: {r}");
        }
    }
    let r = read(&viewer, "many")?;
    let ns: Vec<i64> = said(&r).into_iter().map(|(_, n)| n).collect();
    s.ok(
        &format!("a channel keeps its {} newest across posters and kinds: the oldest go", limits::NEWEST_KEPT_MAX),
        ns == (4..36).rev().collect::<Vec<i64>>(),
        json!(ns),
    );

    // a deploy that stops keeping a kind drops it; one that keeps it again starts from its next record
    s.commit(&c, &[("fragment.json", Some(newest_json(false).as_bytes()))]);
    s.deploy(&c);
    let dropped = s.eventually(Duration::from_secs(30), || read(&viewer, "wall").is_ok_and(|r| r.status == 200 && said(&r).is_empty()));
    let kept_desk = read(bo, "desk")?;
    s.commit(&c, &[("fragment.json", Some(newest_json(true).as_bytes()))]);
    s.deploy(&c);
    // each try a new record: one posted before the install landed is not kept, and a replay moves nothing
    let mut tries = 0;
    let again = s.eventually(Duration::from_secs(30), || {
        tries += 1;
        post(api, bo, &name, "wall", &format!("m2-{tries}"), json!({ "kind": "mood", "n": 6 })).is_ok_and(|p| p.status == 200)
            && read(&viewer, "wall").is_ok_and(|r| !said(&r).is_empty())
    });
    let r = read(&viewer, "wall")?;
    s.ok(
        "a deploy that stops keeping a kind drops what was kept (others stay), and keeping it again starts from its next record",
        dropped && said(&kept_desk) == vec![(ada_id.clone(), 4)] && again && said(&r) == vec![(bo_id.clone(), 6)],
        format!("{kept_desk} {r}"),
    );

    // what retention drops is none
    let lever = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "posted-kept", "n": 2 })))?;
    let a3 = post(api, ada, &name, "wall", "m3", json!({ "kind": "mood", "n": 7 }))?;
    let x2 = post(api, ada, &name, "wall", "x2", json!({ "kind": "chatter" }))?;
    let r = read(&viewer, "wall")?;
    s.ok(
        "a newest record the channel's retention dropped is none: the wall keeps 2, so bo's mood is gone and ada's stays",
        lever.status == 200 && a3.status == 200 && x2.status == 200 && said(&r) == vec![(ada_id, 7)],
        &r,
    );

    // live deleted (no code: no channel keeps anything), then deployed again
    // as it was: keeping starts from its next record (Codex on this read)
    if !s.hosted() {
        s.undeploy(&c);
        let gone = s.eventually(Duration::from_secs(30), || read(&viewer, "wall").is_ok_and(|r| r.status == 404));
        s.deploy(&c);
        let back = s.eventually(Duration::from_secs(30), || read(&viewer, "wall").is_ok_and(|r| r.status == 200));
        let r = read(&viewer, "wall")?;
        s.ok("with live deleted and deployed again, what was kept before is gone: keeping starts from the next record", gone && back && said(&r).is_empty(), &r);
    }
    Ok(())
}

/// A channel that says `"from": "agent"` (a chat's `work`) takes posts and
/// drafts from the fragment's agent members only, each posting as itself:
/// an editor person holds the role it asks for and is refused all the
/// same, as is the agent acting for its owner. Goal: no person's record
/// reads as an agent's (audit M7). Method: a fragment of channels alone,
/// an editor person and an editor agent, each posting and drafting there.
fn posters_kind(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "agents-only")?;
    let c = s.create(api, owner, &name)?;
    let editor = api.person()?;
    let (agent, agent_id) = super::delegation::agent_of(api, owner)?;
    let owner_id = api.identity(owner)?;
    for who in [api.identity(&editor)?, agent_id.clone()] {
        let r = api.share(&name, &who, "editor")?;
        anyhow::ensure!(r.status == 200, "adding {who} as an editor: {r}");
    }
    s.commit(&c, &[("fragment.json", Some(br#"{"channels":{"work":{"read":"viewer","post":"editor","from":"agent"},"notes":{"post":"editor"}}}"#))]);
    s.deploy(&c);
    let work = || records(api, owner, &name, "work");

    let person = post(api, &editor, &name, "work", "w1", json!({ "kind": "turn.step", "turn": "t1", "text": "forged" }))?;
    let owners = post(api, owner, &name, "work", "w1", json!({ "kind": "turn.step", "turn": "t1", "text": "forged" }))?;
    s.ok(
        "an agents' channel refuses an editor person's post, and its owner's (403, saying who posts there), and appends nothing",
        person.status == 403
            && person.code() == Some(ErrorCode::Forbidden)
            && person.message().contains("agent members only")
            && owners.status == 403
            && work().is_empty(),
        format!("{person} {owners}"),
    );
    let notes = post(api, &editor, &name, "notes", "n1", json!({ "text": "a person's note" }))?;
    s.ok("while a channel without from takes the same person's post", notes.status == 200, &notes);

    let step = json!({ "kind": "turn.step", "turn": "t1", "text": "the agent's step" });
    let r = post(api, &agent, &name, "work", "w1", step.clone())?;
    s.ok(
        "an agent member's post is appended, naming the agent itself",
        r.status == 200 && r.body["replayed"] == false && r.body["record"]["principal"] == agent_id.as_str() && work().len() == 1,
        &r,
    );
    let acting = api.signed(&agent, "POST", &format!("/api/f/{name}/channels/work?for={owner_id}"), Some(&json!({ "id": "w2", "body": step })))?;
    s.ok(
        "the same agent acting for its owner is refused (403): the post would be the owner's say",
        acting.status == 403 && acting.message().contains("agent members only") && work().len() == 1,
        &acting,
    );
    let again = post(api, &agent, &name, "work", "w1", step)?;
    s.ok(
        "the agent's same id and body again answer its record, replayed",
        again.status == 200 && again.body["replayed"] == true && again.body["record"]["seq"] == r.body["record"]["seq"] && work().len() == 1,
        &again,
    );

    let draft = |keys: &Keys| api.signed(keys, "PUT", &format!("/api/f/{name}/channels/work/draft"), Some(&json!({ "turn": "t2", "text": "writing" })));
    let (person, agents) = (draft(&editor)?, draft(&agent)?);
    s.ok(
        "a draft there is a post in progress: the person's is 403, the agent's goes through",
        person.status == 403 && person.message().contains("agent members only") && agents.status == 200,
        format!("{person} {agents}"),
    );

    let r = api.signed(owner, "GET", &format!("/api/f/{name}/channels"), None)?;
    let list = r.body["channels"].as_array().cloned().unwrap_or_default();
    let from_of = |n: &str| list.iter().find(|c| c["name"] == n).map(|c| c["from"].clone());
    s.ok(
        "the channel list names each channel's posters' kind (null without one)",
        from_of("work") == Some(json!("agent")) && from_of("notes") == Some(Value::Null) && from_of("events") == Some(Value::Null),
        &r,
    );

    // A chat whose agent joined before member rows named their kind (its
    // row's kind NULL) still takes its agent's posts: the row takes the
    // kind of the identity the registry resolved, and keeps it.
    let kind_of = |who: &str| -> Result<Value> {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/members"), None)?;
        Ok(r.body["members"].as_array().and_then(|l| l.iter().find(|m| m["principal"] == who).map(|m| m["kind"].clone())).unwrap_or(Value::Null))
    };
    let lever = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "legacy-member-kind", "principal": agent_id })))?;
    let legacy = kind_of(&agent_id)?;
    let r = post(api, &agent, &name, "work", "w-legacy", json!({ "kind": "turn.step", "turn": "t3", "step": 1 }))?;
    let filled = kind_of(&agent_id)?;
    s.ok(
        "an agent member whose row predates its kind posts there all the same, and its row takes the agent's kind",
        lever.status == 200 && lever.body["members"] == 1 && legacy.is_null() && r.status == 200 && r.body["record"]["principal"] == agent_id.as_str() && filled == "agent",
        json!({ "lever": lever.body, "before": legacy, "post": r.body, "after": filled }),
    );
    let editor_id = api.identity(&editor)?;
    api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "legacy-member-kind", "principal": editor_id })))?;
    let r = post(api, &editor, &name, "work", "p-legacy", json!({ "kind": "turn.step", "turn": "t3", "step": 2 }))?;
    s.ok(
        "a person whose row predates its kind is still refused there, its row taking the person's kind",
        r.status == 403 && kind_of(&editor_id)? == "person",
        &r,
    );
    Ok(())
}

/// A channel people may post to keeps its newest `limits::POSTED_KEPT`
/// records (the oldest go, whoever appended them), as `events` does: a
/// public chat would otherwise grow for good. Filled by a mutation, 64
/// records a call. The trim is reached through the `posted-kept` lever
/// (100,000 posts would take this lane most of an hour): a lower cap, as
/// an older release's after a rollback, trims everything past it at the
/// next append, then one record a post.
fn retention(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "wall")?;
    let c = s.create(api, owner, &name)?;
    ship(s, &c, WALL_APP, WALL_JSON);
    let fill = |id: &str, n: i64| api.op(owner, &name, "fill", id, json!({ "n": n }));
    // the deploy lands: the first fill again until it has
    s.eventually(Duration::from_secs(15), || fill("f0", 64).is_ok_and(|r| r.status == 200));
    let calls = 11;
    let mut failed = None;
    for i in 1..calls {
        let r = fill(&format!("f{i}"), 64)?;
        if r.status != 200 {
            failed = Some(r.to_string());
            break;
        }
    }
    let appended = calls * 64;
    assert!(appended < limits::POSTED_KEPT, "the fill stays under the real cap");
    let first = |api: &Api| records(api, owner, &name, "wall").first().and_then(|r| r["seq"].as_i64());
    let newest = || -> Option<i64> {
        let r = api.signed(owner, "GET", &format!("/api/f/{name}/channels"), None).ok()?;
        r.body["channels"].as_array()?.iter().find(|c| c["name"] == "wall")?["seq"].as_i64()
    };
    // the fills' effects are applied after their commits; the channel's
    // listing names the newest, read in the same wait (a read of it once,
    // after, saw a moment before the last fill had landed)
    let all = s.eventually(Duration::from_secs(60), || newest() == Some(appended) && first(api) == Some(1));
    s.ok(
        &format!("under its cap ({}) a postable channel keeps every record", limits::POSTED_KEPT),
        failed.is_none() && all,
        format!("{failed:?}; newest {:?}, oldest kept {:?}", newest(), first(api)),
    );
    let kept = 640;
    let r = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "posted-kept", "n": kept })))?;
    s.ok("(a test lever lowers the cap, as an older release's)", r.status == 200 && r.body["kept"] == kept, &r);
    let r = post(api, owner, &name, "wall", "past", json!({ "text": "past the cap" }))?;
    let trimmed = s.eventually(Duration::from_secs(30), || first(api) == Some(appended + 1 - kept + 1));
    s.ok(
        &format!("a lower cap trims every record past its newest {kept} at the next append"),
        r.status == 200 && r.body["record"]["seq"] == appended + 1 && trimmed,
        format!("{} oldest kept {:?}", r.status, first(api)),
    );
    let r = post(api, owner, &name, "wall", "last", json!({ "text": "one more" }))?;
    let trimmed = s.eventually(Duration::from_secs(30), || first(api) == Some(appended + 2 - kept + 1));
    s.ok(
        "a post past them drops one more of the oldest",
        r.status == 200 && r.body["record"]["seq"] == appended + 2 && trimmed,
        format!("{} oldest kept {:?}", r.status, first(api)),
    );
    let r = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "posted-kept", "n": 0 })))?;
    s.ok("(a cap of none is refused)", r.status == 400, &r);
    retention_bytes(s, api, owner)
}

/// Such a channel keeps its newest records by bytes too, whichever bound
/// comes first (`limits::POSTED_BYTES_MAX`, a safety bound under a Durable
/// Object's storage): an append past it first lets the oldest go, so it
/// always makes room, and a record that nearly fills the bound alone is
/// kept alone. Reached through the `posted-bytes` lever (100,000 bytes),
/// with posts of known sizes: after each, the channel holds the newest
/// records whose bodies fit, numbered on (one that empties the channel to
/// fit is not numbered from 1 again).
fn retention_bytes(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let name = s.named(api, owner, "wall-bytes")?;
    let c = s.create(api, owner, &name)?;
    ship(s, &c, WALL_APP, WALL_JSON);
    let cap = 100_000usize;
    let r = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "posted-bytes", "n": cap })))?;
    s.ok("(a test lever lowers the byte bound)", r.status == 200 && r.body["bytes"] == cap, &r);
    // each body `{"text":"x…"}`, `n` bytes in all
    let body = |n: usize| json!({ "text": "x".repeat(n - r#"{"text":""}"#.len()) });
    let sizes = [20_000, 20_000, 20_000, 20_000, 20_000, 20_000, 60_000, 60_000];
    // the deploy lands: the first post again until it has
    s.eventually(Duration::from_secs(15), || post(api, owner, &name, "wall", "b0", body(sizes[0])).is_ok_and(|r| r.status == 200));
    let mut steps = vec![];
    for (i, n) in sizes.iter().enumerate() {
        let r = post(api, owner, &name, "wall", &format!("b{i}"), body(*n))?;
        let held: Vec<(i64, usize)> = records(api, owner, &name, "wall").iter().map(|r| (r["seq"].as_i64().unwrap_or(0), r["body"].to_string().len())).collect();
        // the newest that fit, newest first, as the bound keeps them
        let (mut want, mut sum) = (vec![], 0);
        for (j, m) in sizes[..=i].iter().enumerate().rev() {
            if sum + m > cap {
                break;
            }
            sum += m;
            want.insert(0, (j as i64 + 1, *m));
        }
        steps.push(json!({ "status": r.status, "held": held, "want": want }));
        if r.status != 200 || held != want {
            break;
        }
    }
    let kept_alone = steps.last().is_some_and(|l| l["held"].as_array().map(Vec::len) == Some(1));
    s.ok(
        &format!("past {cap} bytes of bodies, the oldest go before each append (whichever bound comes first): the newest that fit are kept, and one that nearly fills the bound alone is kept alone"),
        steps.len() == sizes.len() && steps.iter().all(|x| x["status"] == 200 && x["held"] == x["want"]) && kept_alone,
        json!(steps),
    );
    let r = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "posted-bytes", "n": 0 })))?;
    s.ok("(a byte bound of none is refused)", r.status == 400, &r);
    Ok(())
}

/// Who hears a post (a subscriber's socket, a subscribed URL, the triggers
/// on its channel), and the public call budget a visitor's posts spend.
fn heard(s: &mut Suite, api: &Api, owner: &Keys, stranger: &Keys) -> Result<()> {
    let name = s.named(api, owner, "heard")?;
    let c = s.create(api, owner, &name)?;
    api.signed(owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "public" })))?;
    ship(s, &c, HEARD_APP, HEARD_JSON);
    let stranger_id = api.identity(stranger)?;
    let mut socket = Socket::open(api, &name, "__live", None, None)?;
    socket.until("hello", 5)?;
    socket.send(&json!({ "type": "subscribe", "channel": "chat", "after": 0 }))?;
    // (on to the checks, which say what is missing, even if it never subscribes)
    let _ = socket.until("subscribed", 5);
    let r = api.signed(owner, "POST", &format!("/api/f/{name}/subscriptions"), Some(&json!({ "channel": "chat", "url": format!("{}/notify", s.push.url) })))?;
    s.ok("(the owner subscribes a URL to the channel)", r.status == 200, &r);

    let r = post(api, stranger, &name, "chat", "hello", json!({ "text": "hello, room" }))?;
    let live = socket.until("record", 5).unwrap_or_else(|e| json!({ "error": e.to_string() }));
    s.ok(
        "a posted record reaches a subscriber's socket live, naming its poster",
        r.status == 200 && live["channel"] == "chat" && live["body"]["text"] == "hello, room" && live["principal"] == stranger_id.as_str(),
        &live,
    );
    let delivered = s.eventually(Duration::from_secs(15), || s.push.notified().iter().any(|f| f["type"] == "record" && f["record"]["body"]["text"] == "hello, room"));
    s.ok("and a URL subscribed to the channel (as an agent's is)", delivered, "");
    let log = || api.op(owner, &name, "log", "q", json!({})).map(|r| r.body["result"]["heard"].clone()).unwrap_or_default();
    let triggered = s.eventually(Duration::from_secs(15), || {
        let heard = log();
        heard.as_array().is_some_and(|h| h.len() == 1 && h[0]["body"]["text"] == "hello, room" && h[0]["principal"] == stranger_id.as_str())
    });
    s.ok("and starts the run its channel's trigger names, with the record", triggered, log());
    let runs = || api.signed(owner, "GET", &format!("/api/f/{name}/runs?op=heard"), None).map(|r| r.body["runs"].clone()).unwrap_or_default();
    let r = post(api, stranger, &name, "chat", "hello", json!({ "text": "hello, room" }))?;
    std::thread::sleep(Duration::from_secs(1));
    let started = runs();
    s.ok(
        "a replayed post starts no run again",
        r.body["replayed"] == true && started.as_array().is_some_and(|a| a.len() == 1 && a[0]["via"] == "channel" && a[0]["depth"] == 1),
        &started,
    );

    // A visitor's posts spend the public call budget with their calls:
    // exactly PUBLIC_CALLS_PER_MIN a clock minute, then 429. A burst that
    // crosses a minute's boundary counts across two budgets and proves
    // nothing exact, so it runs once more with another visitor.
    let limit = limits::PUBLIC_CALLS_PER_MIN as usize;
    let mut burst = None;
    for attempt in 0..2 {
        let minute = now_s() / 60;
        let visitor = api.person()?;
        let mut allowed = 0;
        let mut refusal = None;
        for i in 0..=limit {
            // posts and calls by turns: one budget
            let r = if i % 2 == 0 {
                post(api, &visitor, &name, "lobby", &format!("b{attempt}-{i}"), json!({ "i": i }))?
            } else {
                api.op(&visitor, &name, "log", &format!("b{attempt}-{i}"), json!({}))?
            };
            if r.status != 200 {
                refusal = Some((i % 2 == 0, r));
                break;
            }
            allowed += 1;
        }
        if now_s() / 60 == minute {
            burst = Some((allowed, refusal));
            break;
        }
    }
    let exact = burst.as_ref().is_some_and(|(allowed, refusal)| {
        *allowed == limit && refusal.as_ref().is_some_and(|(posted, r)| *posted && r.code() == Some(ErrorCode::RateLimited))
    });
    let detail = match &burst {
        None => "both bursts crossed a minute's boundary".to_string(),
        Some((allowed, refusal)) => format!("{allowed} allowed within one minute, then {:?}", refusal.as_ref().map(|(posted, r)| (posted, r.to_string()))),
    };
    s.ok(&format!("a visitor's posts spend the public call budget with their calls: {limit} a minute, then a post is 429"), exact, detail);
    let mut member = 0;
    for i in 0..=limit {
        if post(api, owner, &name, "notes", &format!("m{i}"), json!({ "i": i }))?.status == 200 {
            member += 1;
        }
    }
    s.ok(&format!("a member's posts spend none of it ({} in a row)", limit + 1), member == limit + 1, member);
    Ok(())
}

/// The `browser` section's posts: `fragment.post` from a page of a room
/// with no app code, heard live by another page.
pub fn browser(s: &mut Suite, api: &Api, chrome: &mut Lease) -> Result<()> {
    let owner = api.person()?;
    let (name, _) = room(s, api, &owner, "room-page", true)?;
    let wait = Duration::from_secs(15);
    let url = api.site_url(&name, "");
    let a = chrome.open(&url)?;
    let b = chrome.open(&url)?;
    // the room's page itself: a new tab is `about:blank`, complete at once,
    // until its navigation lands (slower on a preview than locally)
    let here = format!("location.href.startsWith({}) && document.readyState === 'complete'", serde_json::to_string(&url)?);
    let loaded = chrome.until(&a, &here, wait) && chrome.until(&b, &here, wait);
    s.ok("(two pages of a room with no app code open)", loaded, "");
    let lib = "import(new URL('./__fragment.js', location.href).href)";
    chrome.eval(&b, &format!("{lib}.then((f) => {{ window.__heard = []; f.subscribe('chat', (r) => window.__heard.push(r)); return f.me(); }})"))?;
    let record = chrome.eval(&a, &format!("{lib}.then((f) => f.post('chat', {{ text: 'from a page' }}, {{ id: 'page-1' }}))"))?;
    s.ok(
        "a page posts with fragment.post, as its visitor, and gets the record",
        record["channel"] == "chat" && record["body"]["text"] == "from a page" && record["principal"].as_str().is_some_and(|p| p.starts_with("anon:")),
        &record,
    );
    s.ok("another page following the channel gets it live", chrome.until(&b, "window.__heard.some((r) => r.body.text === 'from a page')", wait), "");
    let again = chrome.eval(&a, &format!("{lib}.then((f) => f.post('chat', {{ text: 'from a page' }}, {{ id: 'page-1' }}))"))?;
    let held = records(api, &owner, &name, "chat");
    s.ok("the same id again is the same record, appended once", again["seq"] == record["seq"] && held.len() == 1, json!(held));
    let refused = chrome.eval(&a, &format!("{lib}.then((f) => f.post('quiet', {{ text: 'hush' }}).then(() => 'posted', (e) => `${{e.status}} ${{e.code}}`))"))?;
    s.ok("a post to a channel without a post role rejects with the refusal", refused == "403 forbidden", &refused);
    chrome.close(a)?;
    chrome.close(b)?;
    Ok(())
}
