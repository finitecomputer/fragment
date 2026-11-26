//! A SimpleX door on the platform (cell/door.mjs, docs/computers.md "F"):
//! its object starts its container from the door's image; the runner
//! restores (none: a new door), saves to the bucket and names each save to
//! its object; a crash starts it again from its newest save, in step; a
//! save that will not restore is passed over for the one before it; its
//! saves are kept to the newest three; and a planned stop saves last and
//! is not started again until asked. Then its connector (templates/simplex)
//! on a door of its own: its connect starts it, and its owner's message
//! goes from the door to the connector's inbox and its trigger.
//!
//! Locally on workerd and Docker, its containers reaching no SimpleX server
//! (the e2e's reach none), so no link here: SimpleX itself is the lab's
//! (images/bridge/tests/door.rs). Hosted, on a deployment whose config
//! names `doors`. Run by name: a local stack builds the door's image for
//! it alone (devstack::Fleet::doors).
//!
//! With `simplex-link` named too (`--only simplex-door,simplex-link`; hosted,
//! a door on Cloudflare's containers), its files against the real thing
//! (`files_for_real`): a door of the run's linked to a SimpleX app of the
//! run's on SimpleX's preset servers; the app's voice message, past what
//! goes inline, reaches the door whole through its XFTP relay, and the
//! door sends the same bytes back, which the app receives whole. Its
//! levers alone: no connector, no agent, no model call.
//!
//! `connector` (in the computers lane, whose stub agent it binds, on a
//! stack that runs doors: `--only computers,simplex-door`): the SimpleX connector
//! (templates/simplex) end to end on a door. Its owner's `connect` starts
//! its life's door; no other fragment's job reaches a
//! door; bound to the agent, its owner's message as the door's runner
//! delivers it reaches the connector's inbox, is published there as its
//! owner's to the agent, whose computer, asleep, wakes and answers; the
//! answer goes to the door's runner (which has no SimpleX link here, and
//! says so); its owner's voice message (a file its runner gave the door)
//! is kept as one of the connector's blobs and published as its
//! attachment; its agent's reply's file (uploaded there by its agent, a
//! viewer) is given to the door, which holds those very bytes (hashed
//! again there); its question to its owner (a card on `work`) starts the
//! mirror, and no other work of its turns does, and their next message
//! through the door answers it, once (FIN-747); a delivery without the
//! inbox's secret is refused, and one once its owner unbound it reaches no
//! one.

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use crate::api::{Api, Call};
use crate::{Keys, Need, Suite};

pub const NAME: &str = "simplex-door";

fn lever(api: &Api, door: &str, op: &str, more: Value) -> Result<Value> {
    let mut body = json!({ "door": door, "op": op });
    body.as_object_mut().expect("an object").extend(more.as_object().cloned().unwrap_or_default());
    let r = api.unsigned("POST", "/api/test/door", Some(&body))?;
    ensure!(r.status == 200, "the door's {op}: {r}");
    Ok(r.body)
}

/// Its state, once `done` holds (within `within`).
fn until(api: &Api, door: &str, within: Duration, done: impl Fn(&Value) -> bool) -> Result<Value> {
    let t0 = Instant::now();
    // bounded by `within`
    loop {
        let s = lever(api, door, "state", json!({}))?;
        if done(&s) {
            return Ok(s);
        }
        ensure!(t0.elapsed() < within, "the door is not there within {within:?}: {s}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Whether it has ended (cell/door.mjs `/door/end`): its storage gone,
/// so its every ask but a start is refused (410); and its files left
/// under its prefix in the bucket.
fn end_of(api: &Api, door: &str) -> Result<(bool, Value)> {
    let r = api.unsigned("POST", "/api/test/door", Some(&json!({ "door": door, "op": "state" })))?;
    ensure!(r.status == 200 || r.status == 410, "the door's state: {r}");
    Ok((r.status == 410, lever(api, door, "files", json!({}))?["files"].clone()))
}

/// Its end, once it has and its files are gone (within `within`): whether
/// it ended, and its files left. A door refuses its asks (410) from the
/// moment its end begins, before its files are deleted.
pub(super) fn ended(api: &Api, door: &str, within: Duration) -> Result<(bool, Value)> {
    let t0 = Instant::now();
    // bounded by `within`
    loop {
        let (gone, files) = end_of(api, door)?;
        if (gone && files.as_array().is_some_and(Vec::is_empty)) || t0.elapsed() >= within {
            return Ok((gone, files));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Its runner up, at start `generation` or a later one: an exit Docker
/// restarts in place (a code of the runner's own) keeps its generation.
fn ready_from(generation: u64) -> impl Fn(&Value) -> bool {
    move |s| s["runner"]["ready"] == true && s["runner"]["generation"].as_u64().is_some_and(|g| g >= generation) && s["runner"]["generation"] == s["door"]["generation"]
}

fn generation(s: &Value) -> u64 {
    s["door"]["generation"].as_u64().unwrap_or(0)
}

/// The newest save's start and seq.
fn newest(s: &Value) -> (u64, u64) {
    s["saves"].as_array().into_iter().flatten().filter(|x| x["unreadable"] == 0).map(|x| (x["generation"].as_u64().unwrap_or(0), x["seq"].as_u64().unwrap_or(0))).max().unwrap_or((0, 0))
}

/// A run's door, named as a fragment is (`<label>--<suffix>`), from the
/// run's label for it (`Suite::name`: hosted, `e2e-<run>-door`, as a
/// preview's door lever takes it).
fn door_name(label: &str) -> String {
    format!("{label}--k3x9")
}

pub fn door(s: &mut Suite, api: &Api) -> Result<()> {
    // named before the section's own check, so a plan knows the name
    let real = s.names(super::simplex_setup::LINK);
    if !s.section_by_name(NAME, &[Need::Levers, Need::Doors], "its stack builds the door's image (SimpleX's core and its runner)") {
        return Ok(());
    }
    // the run's own door: it ends exhausted (its saves lost), so another
    // run's on the same deployment, after it or beside it, is never this one
    let door = door_name(&s.name("door"));
    let door = door.as_str();
    // a new door: no save, a new profile, its first save in the bucket
    lever(api, door, "start", json!({ "agent": "Juniper" }))?;
    let st = until(api, door, Duration::from_secs(300), |x| ready_from(1)(x) && newest(x).0 == generation(x))?;
    s.ok("a new door starts with no save, and its first save is in the bucket", st["door"]["started"] == 1 && newest(&st) == (generation(&st), 1), &st);

    // a crash: started again (a new start) from its newest save, in step
    lever(api, door, "kill", json!({}))?;
    let st = until(api, door, Duration::from_secs(120), |x| ready_from(2)(x) && newest(x).0 == generation(x))?;
    s.ok("a crash starts it again from its newest save, in step (no sync)", st["runner"]["forcedSyncs"] == 0, &st);
    let crashed = generation(&st);

    // a save that will not restore: its runner says so and ends; it comes
    // up again on the one before it, and the bad one is never given again
    let corrupted = lever(api, door, "corrupt", json!({}))?;
    let bad = corrupted["corrupted"].as_str().unwrap_or_default().to_string();
    let st = until(api, door, Duration::from_secs(120), |x| ready_from(crashed + 1)(x) && newest(x).0 == generation(x))?;
    let given = st["saves"].as_array().into_iter().flatten().any(|x| format!("{}-{}", x["generation"], x["seq"]) == bad && x["unreadable"] == 0);
    s.ok("a save that will not restore is never given again: it comes up on the one before", !bad.is_empty() && !given, &st);

    // its saves kept to the newest three readable
    let readable = st["saves"].as_array().map_or(0, |a| a.iter().filter(|x| x["unreadable"] == 0).count());
    s.ok("its saves are kept to the newest three readable", (1..=3).contains(&readable), &st);

    // a planned stop: its last save, and it stays stopped until asked
    let before = newest(&st);
    lever(api, door, "stop", json!({}))?;
    let st = until(api, door, Duration::from_secs(60), |x| x["running"] == false)?;
    s.ok("a planned stop saves last", newest(&st) > before, &st);
    std::thread::sleep(Duration::from_secs(3));
    let later = lever(api, door, "state", json!({}))?;
    s.ok("a stopped door is not started again", later["running"] == false, &later);
    let stopped = generation(&st);
    lever(api, door, "start", json!({ "agent": "Juniper" }))?;
    let st = until(api, door, Duration::from_secs(120), ready_from(stopped + 1))?;
    s.ok("started again after its stop, from its last save, in step", st["runner"]["forcedSyncs"] == 0 && newest(&st).0 >= stopped, &st);

    // every save's file lost: a door that saved finds none to restore and
    // stops, never a new profile over them (review of #1435)
    lever(api, door, "lose", json!({}))?;
    let st = until(api, door, Duration::from_secs(120), |x| x["door"]["stopped"].is_string() && x["running"] == false)?;
    s.ok("a door whose saves are all lost stops: never a new profile over them", st["door"]["stopped"] == "no save of this door restores" && st["saves"].as_array().is_some_and(|a| a.iter().all(|x| x["unreadable"] == 1)), &st);

    // its connector (templates/simplex), on a door of its own: its owner's
    // message goes from the door to the connector's inbox, as a webhook's
    // does, and its trigger takes it (bound to no agent here, it reaches no
    // one: `connector`, in the computers lane, binds one)
    let owner = api.person()?;
    // its own name, never `<agent>-simplex` (the computer's SimpleX chat's)
    let named = s.named(api, &owner, "sxlane")?;
    let (name, door, _) = connected(s, api, &owner, named, SECRET)?;
    deliver(api, &door, "sx-lane-1", "hello from simplex")?;
    let kept = lever(api, &door, "deliveries", json!({}))?;
    let intake = newest_run(api, &owner, &name, "intake", Duration::from_secs(60));
    s.ok(
        "its owner's message reaches the connector's inbox from the door, and its inbox's trigger takes it (bound to no agent, it reaches no one)",
        kept.as_array().is_some_and(|k| k.iter().any(|d| d["id"] == "sx-lane-1" && d["sent"] == 1)) && intake["output"]["ignored"] == "not linked",
        json!({ "deliveries": kept, "intake": intake }),
    );
    // its connector deleted, its door stopped already (FIN-614): the
    // alarm ends it all the same, its saves and storage gone
    let saved = until(api, &door, Duration::from_secs(120), |x| x["saves"].as_array().is_some_and(|a| !a.is_empty()))?;
    lever(api, &door, "stop", json!({}))?;
    let before = end_of(api, &door)?.1;
    let deleted = api.signed(&owner, "DELETE", &format!("/api/f/{name}"), None)?;
    ensure!(deleted.status == 200, "deleting the connector: {deleted}");
    let (gone, files) = ended(api, &door, Duration::from_secs(60))?;
    s.ok(
        "its connector deleted, its door (stopped already) ends: its saves' files and its storage gone",
        before.as_array().is_some_and(|f| !f.is_empty()) && gone && files.as_array().is_some_and(Vec::is_empty),
        json!({ "saves": saved["saves"], "filesBefore": before, "ended": gone, "filesAfter": files }),
    );
    // its name made again by someone else (the review of #1450): their
    // connect starts a door of their own, new, and the earlier door's
    // files come back no more
    let other = api.person()?;
    let (_, theirs, fresh) = connected(s, api, &other, name.clone(), "e2e-simplex-inbox-of-the-next-life-0a1b2c3d4e5f6a7b")?;
    let (earlier, left) = end_of(api, &door)?;
    s.ok(
        "its name made again by someone else: their connect starts a new door of their own, and the earlier one's stays ended, its files gone",
        theirs != door && generation(&fresh) <= 2 && fresh["door"]["saved"].as_u64().is_some_and(|n| n <= 1) && earlier && left.as_array().is_some_and(Vec::is_empty),
        json!({ "theirs": theirs, "earlier": door, "fresh": fresh["door"], "earlierEnded": earlier, "earlierFiles": left }),
    );
    // theirs ends with their connector, deleted while it runs
    let deleted = api.signed(&other, "DELETE", &format!("/api/f/{name}"), None)?;
    ensure!(deleted.status == 200, "deleting their connector: {deleted}");
    let (gone, files) = ended(api, &theirs, Duration::from_secs(60))?;
    s.ok("their connector deleted while its door runs: its door ends, its files gone", gone && files.as_array().is_some_and(Vec::is_empty), json!({ "ended": gone, "files": files }));
    if real {
        files_for_real(s, api)?;
    }
    Ok(())
}

/// A lever's ask whose door answers other than 200 (a reply taken, 202):
/// its status and body.
fn ask(api: &Api, door: &str, op: &str, more: Value) -> Result<(u16, Value)> {
    let mut body = json!({ "door": door, "op": op });
    body.as_object_mut().expect("an object").extend(more.as_object().cloned().unwrap_or_default());
    let r = api.unsigned("POST", "/api/test/door", Some(&body))?;
    Ok((r.status, r.body))
}

/// How long SimpleX's own servers, a door's start, or a file over XFTP
/// take, at most.
const REAL: Duration = Duration::from_secs(300);

/// S1 against the real thing (module doc): a door of the run's, linked
/// by its invitation to a SimpleX app of the run's (the lab's client on
/// SimpleX's preset servers, as a phone's app is), carries a voice message
/// in and the same bytes back out, each whole, its core's file traffic
/// through its relay alone (on Cloudflare, a container's own TLS hello to
/// an XFTP server is reset).
fn files_for_real(s: &mut Suite, api: &Api) -> Result<()> {
    use super::simplex_setup::{tag, Phone};
    let door = door_name(&s.name("door-files"));
    let door = door.as_str();
    lever(api, door, "start", json!({ "agent": "Juniper" }))?;
    until(api, door, REAL, ready_from(1))?;
    let (asked, _) = ask(api, door, "link", json!({}))?;
    ensure!(asked == 202, "the door's link: {asked}");
    let st = until(api, door, REAL, |x| x["runner"]["phase"] == "inviting" && x["runner"]["link"].is_string())?;
    let link = st["runner"]["short"].as_str().or(st["runner"]["link"].as_str()).unwrap_or_default().to_string();
    let dir = s.dir("simplex-door-phone");
    let mut phone = Phone::start(&dir, &s.name("door-phone"))?;
    let r = phone.cmd(&format!("/c {link}"))?;
    ensure!(!tag(&r).contains("rror"), "the phone's connect: {r}");
    let contact = phone.wait(REAL, |e| tag(e) == "contactConnected")?["contact"]["contactId"].as_i64().unwrap_or(0);
    until(api, door, REAL, |x| x["runner"]["phase"] == "linked")?;

    // its voice message, past what goes inline (over XFTP), whole at the door
    let memo: Vec<u8> = (0..400_000u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let sha = fragment_core::blob::sha256_hex(&memo);
    std::fs::write(dir.join("files").join("memo.m4a"), &memo)?;
    let body = json!([{ "filePath": "/data/files/memo.m4a", "msgContent": { "type": "voice", "text": "", "duration": 5 } }]);
    let sent = phone.cmd(&format!("/_send @{contact} json {body}"))?;
    let t0 = Instant::now();
    let delivered = s.eventually(REAL, || lever(api, door, "deliveries", json!({})).is_ok_and(|d| d.as_array().is_some_and(|d| d.iter().any(|m| m["file"]["sha256"] == sha.as_str()))));
    let came_ms = t0.elapsed().as_millis();
    let held = lever(api, door, "file", json!({ "sha": sha })).unwrap_or_default();
    s.ok(
        "a SimpleX app's voice message (over XFTP, SimpleX's own servers) reaches the door whole through its relay, and is delivered naming it",
        tag(&sent) == "newChatItems" && delivered && held["sha256"] == sha.as_str() && held["size"] == memo.len(),
        json!({ "sent": tag(&sent), "delivered": delivered, "ms": came_ms, "held": held, "logs": if delivered { String::new() } else { phone.logs() } }),
    );

    // the same bytes back, as a reply's file, whole at the app
    let files = json!([{ "sha256": sha, "name": "echo.bin", "type": "application/octet-stream", "size": memo.len() }]);
    let t0 = Instant::now();
    let done = s.eventually(REAL, || ask(api, door, "reply", json!({ "key": "files-echo", "text": "your memo, back", "files": files })).is_ok_and(|(status, _)| status == 200));
    let back = (|| -> Result<Vec<u8>> {
        let is = |ci: &Value| ci["chatItem"]["chatDir"]["type"] == "directRcv" && ci["chatItem"]["file"]["fileId"].is_i64();
        let e = phone.wait(REAL, |e| tag(e) == "newChatItems" && e["chatItems"].as_array().is_some_and(|cs| cs.iter().any(is)))?;
        let id = e["chatItems"].as_array().and_then(|cs| cs.iter().find(|c| is(c))).map(|c| c["chatItem"]["file"]["fileId"].clone()).unwrap_or_default();
        phone.wait(REAL, |e| tag(e) == "rcvFileDescrReady" && e["chatItem"]["chatItem"]["file"]["fileId"] == id)?;
        let r = phone.cmd(&format!("/freceive {id}"))?;
        ensure!(!tag(&r).contains("rror"), "/freceive {id}: {r}");
        let done = phone.wait(REAL, |e| tag(e) == "rcvFileComplete" && e["chatItem"]["chatItem"]["file"]["fileId"] == id)?;
        let path = done["chatItem"]["chatItem"]["file"]["fileSource"]["filePath"].as_str().unwrap_or_default().to_string();
        Ok(std::fs::read(dir.join("files").join(path.rsplit('/').next().unwrap_or(&path)))?)
    })();
    let back_ms = t0.elapsed().as_millis();
    s.ok(
        "the door sends the same bytes back as a reply's file (over XFTP, through its relay), whole at the app",
        done && back.as_ref().is_ok_and(|b| *b == memo),
        json!({ "done": done, "ms": back_ms, "received": back.as_ref().map(Vec::len).map_err(|e| format!("{e:#}")), "logs": if back.is_ok() { String::new() } else { phone.logs() } }),
    );
    lever(api, door, "stop", json!({}))?;
    Ok(())
}

/// Whose connector it is, and the agent it binds (the computers lane's).
pub(super) struct Agent<'a> {
    pub owner: &'a Keys,
    pub owner_id: &'a str,
    pub computer: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    pub wake: Duration,
}

/// An app of its author's own: a job that asks what URL it is given, as
/// a door's connector would, naming a door of its choosing.
const REACHER_APP: &str = r#"import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  async reach({ url, door }, job) {
    const r = await job.fetch(url, { method: "POST", headers: { "content-type": "application/json", "x-fragment-door": door }, body: "{}" });
    return { status: r.status };
  }
}
"#;
const REACHER_JSON: &str = r#"{ "operations": { "reach": { "kind": "job" } } }"#;

/// The newest run of `op` on `name`, once it ends (within `within`).
fn newest_run(api: &Api, owner: &Keys, name: &str, op: &str, within: Duration) -> Value {
    let id = api.signed(owner, "GET", &format!("/api/f/{name}/runs?op={op}&limit=1"), None).ok().and_then(|r| r.body["runs"][0]["id"].as_i64()).unwrap_or(0);
    super::jobs::settle(api, owner, name, id, &["succeeded", "failed", "held"], within)
}

/// One of its owner's messages, as the door's runner delivers it.
fn deliver(api: &Api, door: &str, id: &str, text: &str) -> Result<Value> {
    lever(api, door, "deliver", json!({ "message": { "id": id, "kind": "text", "text": text, "at": "2026-11-21T00:00:00Z", "file": null } }))
}

/// A file as the door's runner gives one (`bytes`, typed `type`): its
/// SHA-256, which a delivery then names.
fn give(api: &Api, door: &str, bytes: &[u8], kind: &str) -> Result<String> {
    use base64::Engine as _;
    let r = lever(api, door, "file", json!({ "bytes": base64::engine::general_purpose::STANDARD.encode(bytes), "type": kind }))?;
    r["sha256"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("the door's file: {r}"))
}

/// One of its owner's messages carrying file `sha256`, as its runner delivers it.
fn deliver_file(api: &Api, door: &str, id: &str, kind: &str, file: Value) -> Result<Value> {
    lever(api, door, "deliver", json!({ "message": { "id": id, "kind": kind, "text": "", "at": "2026-11-24T00:00:00Z", "file": file } }))
}

/// A SimpleX connector of `owner`'s named `name`, its inbox's `secret`
/// set, connected: its own job's connect starts its life's door
/// (`fragment_core::door::door_id`) and gives it that secret. Its name,
/// its door's and the door's state.
fn connected(s: &mut Suite, api: &Api, owner: &Keys, name: String, secret: &str) -> Result<(String, String, Value)> {
    let made = api.create_with(owner, json!({ "name": name, "template": "simplex" }))?;
    ensure!(made.status == 200, "making the SimpleX connector: {made}");
    s.owned(&made.body, owner);
    let door = fragment_core::door::door_id(&name, made.body["npub"].as_str().unwrap_or_default()).ok_or_else(|| anyhow::anyhow!("no door for {name}: {made}"))?;
    let r = api.signed(owner, "PUT", &format!("/api/f/{name}/secrets/SIMPLEX_DOOR_SECRET"), Some(&json!({ "value": secret, "hosts": [fragment_core::door::DOOR_HOST] })))?;
    ensure!(r.status == 200, "its inbox's secret: {r}");
    let r = api.op(owner, &name, "connect", "connect", json!({ "name": "Juniper" }))?;
    let run = super::jobs::settle(api, owner, &name, r.body["result"]["run"].as_i64().unwrap_or(0), &["succeeded", "failed", "held"], Duration::from_secs(300));
    let st = until(api, &door, Duration::from_secs(300), ready_from(1))?;
    s.ok(
        "its owner's connect, the connector's own job, starts its life's own door and gives it the inbox's secret",
        run["output"]["connected"] == true && st["door"]["door"] == door.as_str() && st["door"]["agent"] == "Juniper" && st["inbox"] == true,
        json!({ "run": run, "door": st }),
    );
    Ok((name, door, st))
}

/// The inbox's secret a run's connector is set with.
const SECRET: &str = "e2e-simplex-inbox-5ec4e7a9f0b1c2d3e4f5061728394a5b";

pub(super) fn connector(s: &mut Suite, api: &Api, a: &Agent<'_>) -> Result<()> {
    if !s.runs_doors() {
        s.skip("the SimpleX connector on a door", &format!("the stack runs no doors: a local one builds their image for a run that names their lane (cargo xtask e2e --only computers,{NAME})"));
        return Ok(());
    }
    let owner = a.owner;
    let named = s.named(api, owner, "sxdoor")?;
    let (name, door, st) = connected(s, api, owner, named, SECRET)?;
    let newest = |op: &str| newest_run(api, owner, &name, op, a.wake);

    // no other fragment's job reaches a door, whichever it names
    let reacher = s.named(api, owner, "reacher")?;
    let c = s.create(api, owner, &reacher)?;
    super::app::ship(s, &c, REACHER_APP.as_bytes(), REACHER_JSON.as_bytes());
    let marks = st["door"]["marks"].clone();
    let r = api.op(owner, &reacher, "reach", "reach", json!({ "url": format!("https://{}/state", fragment_core::door::DOOR_HOST), "door": door }))?;
    let refused = super::jobs::settle(api, owner, &reacher, r.body["result"]["run"].as_i64().unwrap_or(0), &["succeeded", "failed", "held"], a.wake);
    let after = lever(api, &door, "state", json!({}))?;
    s.ok(
        "another fragment's job reaches no door: its fetch of the door's host fails unsent",
        refused["status"] != "succeeded" && refused["error"].as_str().is_some_and(|e| e.contains("a SimpleX connector's own door")) && after["door"]["marks"] == marks,
        json!({ "run": refused, "door": after }),
    );

    // bound to its owner's agent, whose computer then sleeps
    let r = api.signed(owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "link", "agent": a.agent })))?;
    ensure!(r.status == 200 && r.body["door"] == "simplex", "binding the connector as the agent's simplex door: {r}");
    let slept = api.signed(owner, "POST", &format!("/api/computers/{}/sleep", a.computer), Some(&json!({})))?;
    ensure!(slept.body["phase"] == "asleep", "the agent's computer asleep first: {slept}");

    // its owner's message, as the door's runner delivers it: on to the
    // connector's inbox, published there as theirs to their agent, which
    // wakes and answers; the answer goes to the door's runner
    deliver(api, &door, "sx-e2e-1", "hello from simplex")?;
    let kept = lever(api, &door, "deliveries", json!({}))?;
    let intake = newest("intake");
    let chat = || super::jobs::records(api, owner, &name, "chat");
    let relayed = chat().into_iter().find(|r| r["body"]["text"] == "hello from simplex").unwrap_or_default();
    s.ok(
        "its owner's message reaches the connector's inbox from the door, and is published there as theirs, to their agent, through its simplex door",
        kept.as_array().is_some_and(|k| k.iter().any(|d| d["id"] == "sx-e2e-1" && d["sent"] == 1))
            && intake["output"]["relayed"] == true
            && relayed["body"]["relayed"] == json!({ "via": "simplex", "for": a.owner_id, "agent": a.agent })
            && relayed["body"]["to"] == json!([a.identity]),
        json!({ "deliveries": kept, "intake": intake, "record": relayed }),
    );
    let answered = s.eventually(a.wake, || super::computers::agent_replies(&chat(), a.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains("hello from simplex"))));
    let mirror = newest("mirror");
    s.ok(
        "its agent's computer, asleep, wakes for it and answers; the answer goes to the door's runner, which has no SimpleX link here and says so",
        answered && mirror["output"] == json!({ "sent": 0, "why": "not linked" }),
        json!({ "replies": super::computers::agent_replies(&chat(), a.identity), "mirror": mirror }),
    );

    // its owner's voice message: its file, which its runner gave the door,
    // kept as one of the connector's blobs and published as its attachment
    // within a lever's body as base64 (cell/door.mjs `BODY_MAX`)
    let memo: Vec<u8> = (0..150_000u32).map(|n| (n % 253) as u8).collect();
    let sha = give(api, &door, &memo, "audio/mp4")?;
    let named = json!({ "sha256": sha, "name": "memo.m4a", "type": "audio/mp4", "size": memo.len() });
    deliver_file(api, &door, "sx-e2e-voice", "voice", named)?;
    let intake = newest("intake");
    let voiced = chat().into_iter().find(|r| r["body"]["attachments"][0]["sha256"] == sha.as_str()).unwrap_or_default();
    let blob = api.signed(owner, "GET", &format!("/api/f/{name}/blobs/{sha}"), None)?;
    s.ok(
        "its owner's voice message reaches their agent with its file, kept from the door as one of the connector's blobs, whole",
        intake["output"] == json!({ "relayed": true, "file": true })
            && voiced["body"]["attachments"] == json!([{ "sha256": sha, "size": memo.len(), "type": "audio/mp4", "name": "memo.m4a" }])
            && voiced["body"]["relayed"]["via"] == "simplex"
            && blob.status == 200
            && blob.bytes == memo,
        json!({ "intake": intake, "record": voiced, "blob": blob.status, "bytes": blob.bytes.len() }),
    );

    // its agent's reply's file (a drawing), uploaded there by its agent, a
    // viewer: given to the door, which holds those very bytes
    deliver(api, &door, "sx-e2e-draw", "draw me something")?;
    let drew = s.eventually(a.wake, || super::computers::agent_replies(&chat(), a.identity).iter().any(|r| r["body"]["attachments"][0]["sha256"].is_string()));
    let drawn = super::computers::agent_replies(&chat(), a.identity).into_iter().find(|r| r["body"]["attachments"][0]["sha256"].is_string()).unwrap_or_default();
    let file = drawn["body"]["attachments"][0].clone();
    let fsha = file["sha256"].as_str().unwrap_or("").to_string();
    let look = || api.unsigned("POST", "/api/test/door", Some(&json!({ "door": door, "op": "file", "sha": fsha }))).map(|r| r.body).unwrap_or_default();
    let held = s.eventually(a.wake, || look()["sha256"] == fsha.as_str());
    let mirror = newest("mirror");
    s.ok(
        "its agent's reply's file is uploaded to the connector's chat by its agent, given to the door as the reply goes, and the door holds those very bytes",
        drew && held && look()["size"] == file["size"] && mirror["output"] == json!({ "sent": 0, "why": "not linked" }) && !drawn["body"]["text"].as_str().unwrap_or("").contains("could not be attached"),
        json!({ "reply": drawn, "held": look(), "mirror": mirror }),
    );

    // its agent's question (FIN-747): its card on `work` starts the mirror,
    // and no other work of its turns does; the mirror sends it to the door
    // (which has no SimpleX link here, and says so), as the computer's own
    // SimpleX words it (applib/question.mjs: its words are Telegram's
    // lane's to see); its owner's next message through the door answers
    // it, once, and its turn goes on
    let work = || super::jobs::records(api, owner, &name, "work");
    let on_work = || -> Vec<Value> {
        let runs = api.signed(owner, "GET", &format!("/api/f/{name}/runs?op=mirror&limit=50"), None).ok().and_then(|r| r.body["runs"].as_array().cloned()).unwrap_or_default();
        runs.into_iter().filter(|r| r["trigger"] == "work").collect()
    };
    deliver(api, &door, "sx-e2e-2", "choose a plant")?;
    let carded = s.eventually(a.wake, || work().iter().any(|r| r["body"]["kind"] == "turn.prompt" && r["body"]["text"] == "What should I plant?"));
    let card = work().into_iter().find(|r| r["body"]["kind"] == "turn.prompt" && r["body"]["text"] == "What should I plant?").unwrap_or_default();
    let started = s.eventually(a.wake, || !on_work().is_empty());
    let asked = on_work().first().map(|r| super::jobs::settle(api, owner, &name, r["id"].as_i64().unwrap_or(0), &["succeeded", "failed", "held"], a.wake)).unwrap_or_default();
    s.ok(
        "its agent's question to its owner, its card on work, starts the mirror, which sends it to the door (no SimpleX link here, which it says)",
        carded && card["body"]["asks"] == a.owner_id && started && asked["output"] == json!({ "sent": 0, "why": "not linked" }) && asked["input"]["record"]["seq"] == card["seq"],
        json!({ "card": card, "mirror": asked }),
    );
    deliver(api, &door, "sx-e2e-3", "Mint")?;
    let chose = s.eventually(a.wake, || super::computers::agent_replies(&chat(), a.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains("choose a plant (chose: Mint)"))));
    let closed: Vec<Value> = work().into_iter().filter(|r| r["body"]["kind"] == "turn.prompt.closed" && r["body"]["prompt"] == card["body"]["prompt"]).collect();
    let own_turn = super::computers::agent_replies(&chat(), a.identity).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.ends_with("] Mint")));
    let turns = work().iter().filter(|r| r["body"]["kind"] == "turn.start").count();
    let mirrored = on_work();
    s.ok(
        "its owner's next message through the door answers it, once: the card closed with their words, its turn going on with them, no turn of its own; and no other work of its turns started the mirror",
        chose && closed.len() == 1 && closed[0]["body"]["outcome"] == "answered" && closed[0]["body"]["by"] == a.owner_id && closed[0]["body"]["text"] == "Mint" && !own_turn && turns >= 2 && mirrored.len() == 1,
        json!({ "closed": closed, "turns": turns, "mirrorsOnWork": mirrored }),
    );

    // a delivery without the inbox's secret, from anyone, starts nothing
    let url = format!("{}/api/f/{name}/inbox", api.base);
    let forged = api.call(Call { method: "POST", url, body: Some(json!({ "payload": { "id": "sx-forged", "kind": "text", "text": "forged", "at": "2026-11-21T00:00:00Z" } }).to_string().into_bytes()), content_type: Some("application/json"), extra: vec![("x-simplex-door-secret", "not-the-secret".into())], ..Call::default() })?;
    s.ok("a delivery without the inbox's secret is refused", forged.status == 403, &forged);

    // unbound by its owner (FIN-614): its door ends with the unlink, its
    // saves and storage gone, and its binding with it
    let before = end_of(api, &door)?.1;
    let r = api.signed(owner, "POST", &format!("/api/f/{name}/door"), Some(&json!({ "act": "unlink" })))?;
    ensure!(r.status == 200, "unbinding: {r}");
    let (gone, files) = end_of(api, &door)?;
    let bound = api.signed(owner, "GET", &format!("/api/f/{name}/door"), None)?;
    s.ok(
        "unbound by its owner, its door ends with the unlink: its saves' files and its storage gone, and it is no one's door",
        before.as_array().is_some_and(|f| !f.is_empty()) && gone && files.as_array().is_some_and(Vec::is_empty) && bound.status == 200 && bound.body["bound"].is_null(),
        json!({ "filesBefore": before, "ended": gone, "filesAfter": files, "bound": bound.body }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: each run its own door (the review of #1435: the lane ends with
    /// its door exhausted, so a second run on one deployment, with its
    /// storage kept, waited on a door that never starts again, and two at
    /// once shared one), and a hosted one a door the preview's lever takes.
    /// Method: two runs' labels, as a hosted suite makes them.
    #[test]
    fn each_run_has_a_door_of_its_own() {
        let (one, two) = (crate::hosted::sweep::label("0149355cab", "door"), crate::hosted::sweep::label("0149355cac", "door"));
        let (a, b) = (door_name(&one), door_name(&two));
        assert_ne!(a, b, "two runs, two doors");
        for d in [&a, &b] {
            assert!(fragment_core::levers::is_e2e_fragment(d), "a preview's lever takes {d}");
        }
    }
}
