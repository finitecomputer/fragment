//! Computers on disks (docs/computers.md, "A disk"), on a node whose stub's
//! new computers go on them (`Shape::Disks`) and a runtime that gives a
//! container FUSE (krun's: `Need::Disks`). A computer made there is on a
//! disk for good: each start mounts `/data` from FPAC through its route,
//! nothing restored and no gate; each sleep is a signal and a wait, its
//! daemon's final flush posting a clean head; awake, nothing is saved.
//!
//! Valid: a new disk, a write read after each of `cycles()` sleeps.
//! Invalid: a head that will not open, fallen back past to the one before;
//! a new disk's first life killed before anything of it shipped, its device
//! blank, begun again as a new disk. Replay: a crash while it runs, its
//! acknowledged writes replayed by the next mount; a first life killed as
//! its flushes ship, mounted as it shipped. Restart: every wake is a new
//! life on the same disk, each sleep's head that life's and clean.

use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};

use super::computers::{agent_replies, lever, lever_with, members_only, phase, put_to_sleep, told, AGENT_JSON, CHAT_JSON};
use crate::api::Api;
use crate::{Need, Suite};

/// A start on a disk: its container, its mount, its guest's first read.
const WAKE: Duration = Duration::from_secs(120);
/// The writes of a life that runs on land in its disk's bucket about a
/// second after they are synced (its daemon's flush): waited out before a
/// crash, so a write the guest acknowledged is one the store has.
const SHIPPED_WITHIN: Duration = Duration::from_secs(5);

/// Sleeps and wakes, each with a write before it, beside the first's
/// (`FRAGMENT_E2E_DISK_CYCLES`: a longer run's, at most `CYCLES_MAX`).
fn cycles() -> u32 {
    std::env::var("FRAGMENT_E2E_DISK_CYCLES").ok().and_then(|n| n.parse().ok()).unwrap_or(3).clamp(1, CYCLES_MAX)
}
const CYCLES_MAX: u32 = 1_000;
/// Each `CRASH_EVERY`th cycle crashes its computer instead of sleeping it,
/// a fill of `FILL_MIB` still shipping, its write before shipped.
const CRASH_EVERY: u32 = 10;
const FILL_MIB: u32 = 64;

const DISK_CHECKS: [&str; 9] = [
    "a computer made while its image goes on disks is on one: a new disk of 20,000 MiB, no head yet",
    "its first start mounts the new disk: the start line's path is disk, saying how long its mount took",
    "a sleep is a signal and a wait: its daemon's final flush posts that life's head, clean, its sleep line saying so, and no save is taken",
    "a wake mounts the disk as it slept: what it wrote is there, nothing restored, its line saying what its last sleep left unshipped",
    "every write is read after the wake that follows it, each sleep clean, each tenth a crash while a fill ships",
    "a crash while it runs loses no write it acknowledged: the start after it mounts the disk as that life left it",
    "a head that will not open is fallen back past: that start fails as unusable, the next mounts the head before it, no strike",
    "awake or asleep, no save of a disk is taken for itself",
    "its owner's export of it asleep wakes it and copies a save taken for that export, its files whole",
];

pub fn disks(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_using("disks", &[Need::Disks, Need::Computers, Need::Levers], &[Need::Node]) {
        return Ok(());
    }
    // hosted, the deployment's new computers are on disks already (its
    // config's `computers.disks`); locally, a node of that shape
    if s.hosted() {
        return disk_checks(s, api);
    }
    s.stop()?;
    let on = s.start_on_disks()?;
    let checked = disk_checks(s, &on);
    s.stop()?;
    s.start(false)?;
    checked
}

fn disk_checks(s: &mut Suite, api: &Api) -> Result<()> {
    let owner = api.person()?;
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let id = r.body["computer"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && id.starts_with("computer:"), "a computer: {r}");
    // its checks talk to the stub's script: hosted, a branch whose new
    // computers are the stub's; its lines are the node's logs, local only
    if s.hosted() && r.body["image"] != "stub" {
        for check in DISK_CHECKS {
            s.skip(check, "it needs the stub's scripted runtime, and this deployment's new computers run another image");
        }
        return image_on_disk(s, api);
    }
    let local = !s.hosted();
    let row = || lever(api, &id, "fpac").map(|r| r.body).unwrap_or(Value::Null);
    let made = row();
    s.ok(DISK_CHECKS[0], made["disk"] == true && made["volume"] == 20_000u64 << 20 && made["head"].is_null() && made["back"].is_null(), &made);
    first_lives(s, api)?;

    // its agent, and a chat it is in: its join wakes the computer
    let (agent_name, chat, identity) = agent_in_chat(s, api, &owner, &id, &[])?;
    let awake = |s: &Suite| s.eventually(WAKE, || phase(api, &owner, &id) == "awake");
    let woke = awake(s);
    let view = || api.signed(&owner, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let first = view();
    let lines = |s: &Suite| lines_of(s, &id);
    // what its agent says to `write` and `read` (the stub's script)
    let talk = Talk { api, owner: &owner, chat: &chat, agent: &agent_name, identity: &identity, n: std::cell::Cell::new(0), bound: WAKE };
    let wrote = talk.said(s, "write disk/first.txt one")?;
    // its start's line is written at its first turn (that write's)
    let line1 = || lines_of(s, &id).0.into_iter().find(|l| l["generation"] == 1).unwrap_or(Value::Null);
    let _ = !local || s.eventually(Duration::from_secs(10), || !line1().is_null());
    let line1 = line1();
    s.ok(
        DISK_CHECKS[1],
        woke && first["restored"]["from"] == "disk" && (!local || (line1["path"] == "disk" && line1["end"] == "ready" && line1["disk"]["mountMs"].is_u64() && line1.get("bytes").is_none())),
        json!({ "view": first["restored"], "line": line1 }),
    );
    let (slept, _) = put_to_sleep(s, api, &owner, &id);
    let after = row();
    let sleep1 = lines(s).1.into_iter().find(|l| l["sleep"]["generation"] == 1).unwrap_or(Value::Null);
    let saves = lever(api, &id, "saves")?.body["saves"].clone();
    s.ok(
        DISK_CHECKS[2],
        wrote == "wrote disk/first.txt" && slept && head_of(&after["head"]).is_some_and(|(epoch, clean)| epoch == 1 && clean) && (!local || (sleep1["sleep"]["clean"] == true && sleep1["sleep"]["exited"] == true)) && saves.as_array().is_some_and(Vec::is_empty),
        json!({ "wrote": wrote, "row": after, "line": sleep1, "saves": saves }),
    );

    let _ = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let woke = awake(s);
    let read = talk.said(s, "read disk/first.txt")?;
    let again = view();
    let generation = again["restored"]["generation"].as_u64().unwrap_or(0);
    let line = lines(s).0.into_iter().find(|l| l["generation"] == generation).unwrap_or(Value::Null);
    s.ok(
        DISK_CHECKS[3],
        woke && first_line(&read) == "read disk/first.txt: one" && again["restored"]["from"] == "disk" && (!local || (line["path"] == "disk" && line["disk"]["sleptUnshipped"].is_u64())),
        json!({ "read": read, "restored": again["restored"], "line": line }),
    );

    // every write read after the wake that follows it, each sleep clean;
    // every `CRASH_EVERY`th a crash instead, while a fill still ships
    let (mut failed, mut crashes, t0) = (vec![], 0u32, std::time::Instant::now());
    for i in 0..cycles() {
        let token = format!("cycle-{i}");
        let wrote = talk.said(s, &format!("write disk/cycle.txt {token}"))?;
        let crash = i % CRASH_EVERY == CRASH_EVERY - 1;
        let (ended, head) = if crash {
            std::thread::sleep(SHIPPED_WITHIN);
            let before = view()["restored"]["generation"].as_u64().unwrap_or(0);
            let filled = talk.said(s, &format!("fill-data disk/fill.bin {FILL_MIB}"))?;
            let killed = lever(api, &id, "kill").is_ok_and(|r| r.status == 200);
            crashes += 1;
            let back = s.eventually(WAKE + WAKE, || view()["restored"]["generation"].as_u64().is_some_and(|g| g > before) && phase(api, &owner, &id) == "awake");
            if !(filled.starts_with("filled ") && killed && back) {
                println!("      (crash {i}: filled {filled:?}, killed {killed}, back {back})");
            }
            (filled.starts_with("filled ") && killed && back, None)
        } else {
            let (slept, _) = put_to_sleep(s, api, &owner, &id);
            let head = head_of(&row()["head"]);
            let _ = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
            (slept && head.is_some_and(|(_, clean)| clean) && awake(s), head)
        };
        let read = talk.said(s, "read disk/cycle.txt")?;
        if !(wrote == "wrote disk/cycle.txt" && ended && first_line(&read) == format!("read disk/cycle.txt: {token}")) {
            let f = json!({ "i": i, "crash": crash, "wrote": wrote, "ended": ended, "read": read, "head": head.map(|(e, c)| json!([e, c])) });
            println!("      (cycle {i} failed: {f})");
            failed.push(f);
        }
        if (i + 1) % 10 == 0 {
            println!("      ({} of {} cycles, {crashes} crashes, {} failed, {:.0?})", i + 1, cycles(), failed.len(), t0.elapsed());
        }
    }
    s.ok(DISK_CHECKS[4], failed.is_empty(), json!({ "cycles": cycles(), "crashes": crashes, "failed": failed, "took": format!("{:.0?}", t0.elapsed()) }));

    // a crash while it runs: what it acknowledged, shipped, is mounted again
    let wrote = talk.said(s, "write disk/crash.txt synced")?;
    std::thread::sleep(SHIPPED_WITHIN);
    let before = view()["restored"]["generation"].as_u64().unwrap_or(0);
    let killed = lever(api, &id, "kill")?;
    let came_back = s.eventually(WAKE + WAKE, || view()["restored"]["generation"].as_u64().is_some_and(|g| g > before) && phase(api, &owner, &id) == "awake");
    let read = talk.said(s, "read disk/crash.txt")?;
    s.ok(DISK_CHECKS[5], wrote == "wrote disk/crash.txt" && killed.status == 200 && came_back && first_line(&read) == "read disk/crash.txt: synced", json!({ "read": read, "killed": killed.body, "view": view()["restored"] }));

    // a head that will not open: one naming a checkpoint never written
    let (slept, _) = put_to_sleep(s, api, &owner, &id);
    let good = row()["head"].clone();
    let epoch = head_of(&good).map(|(e, _)| e).unwrap_or(0);
    let broken = format!("{epoch} {epoch} 999999 {}", "0".repeat(64));
    let set = lever_with(api, &id, json!({ "op": "fpac-head", "head": broken }))?;
    let _ = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let woke = awake(s);
    let read = talk.said(s, "read disk/crash.txt")?;
    let fell = row();
    let started = lines(s).0;
    let unusable = started.iter().find(|l| l["end"] == "failed" && l["failed"] == "unusable" && l["path"] == "disk").cloned();
    let failures = lever(api, &id, "saves")?.body["failures"].clone();
    s.ok(
        DISK_CHECKS[6],
        slept && set.status == 200 && woke && first_line(&read) == "read disk/crash.txt: synced" && (!local || unusable.is_some()) && head_of(&fell["head"]).is_some_and(|(e, _)| e > epoch) && failures == 0,
        json!({ "set": set.body, "read": read, "unusable": unusable, "row": fell, "failures": failures }),
    );

    let saves = lever(api, &id, "saves")?.body;
    s.ok(DISK_CHECKS[7], saves["saves"].as_array().is_some_and(Vec::is_empty) && saves["aside"].as_array().is_none_or(Vec::is_empty), &saves["saves"]);

    // its export, asleep: a save taken for it, awake
    let (slept, _) = put_to_sleep(s, api, &owner, &id);
    let exported = disk_exported(s, api, &owner, &id)?;
    s.ok(DISK_CHECKS[8], slept && exported["ok"] == true, &exported);
    Ok(())
}

/// A new disk's first life, killed this long after its daemon's claim (the
/// head its open posts): at once, before its first flush (a second after
/// its daemon starts) can ship anything, or later, as its flushes ship.
const FIRST_KILLS_MS: [u64; 14] = [0, 0, 250, 500, 600, 700, 800, 900, 1_000, 1_200, 1_500, 2_000, 4_000, 8_000];
const FIRST_LIVES: &str = "a new disk's first life killed at once after its claim, or as its flushes ship: its next start mounts the disk, its head that start's own, each sleep after clean, and a write it acknowledged and shipped is there";

/// Each kill of `FIRST_KILLS_MS`, on a new computer of its own, then a
/// first life that wrote, its write shipped, killed. Which kills shipped
/// nothing is in the bucket (that life's objects,
/// `disks/<hex>/e<generation>/`), listed after a hosted run beside the
/// platform's lines of the starts that fell back (a blank device begun
/// again as a new disk, and no other); a local run's logs say here which
/// fell back.
fn first_lives(s: &mut Suite, api: &Api) -> Result<()> {
    let local = !s.hosted();
    let mut lives = vec![];
    let row = |id: &str| lever(api, id, "fpac").map(|r| r.body).unwrap_or(Value::Null);
    for (n, after) in FIRST_KILLS_MS.into_iter().enumerate() {
        let owner = api.person()?;
        let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
        let id = r.body["computer"].as_str().unwrap_or("").to_string();
        anyhow::ensure!(r.status == 200 && id.starts_with("computer:"), "a computer: {r}");
        let t0 = Instant::now();
        let _ = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
        let mut claim = Value::Null;
        let claimed = s.eventually(WAKE, || {
            claim = row(&id)["head"].clone();
            !claim.is_null()
        });
        let claimed_ms = t0.elapsed().as_millis() as u64;
        std::thread::sleep(Duration::from_millis(after));
        let mut life = killed_and_back(s, api, &owner, &id);
        let (slept, _) = put_to_sleep(s, api, &owner, &id);
        let generation = life["generation"].as_u64().unwrap_or(0);
        let clean = head_of(&row(&id)["head"]).is_some_and(|(e, c)| e == generation && c);
        life["n"] = json!(n);
        life["afterMs"] = json!(after);
        life["claimed"] = json!(claimed);
        life["claimedMs"] = json!(claimed_ms);
        life["claim"] = claim;
        life["slept"] = json!(slept);
        life["clean"] = json!(clean);
        if local {
            life["fellBack"] = json!(unusable_of(s, &id));
        }
        life["ok"] = json!(claimed && life["sent"] == true && life["woke"] == true && life["from"] == "disk" && life["own"] == true && slept && clean);
        println!("      FIRSTLIFE {life}");
        lives.push(life);
    }
    // a first life that acknowledged a write, shipped before its kill
    let owner = api.person()?;
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let id = r.body["computer"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && id.starts_with("computer:"), "a computer: {r}");
    let (agent, chat, identity) = agent_in_chat(s, api, &owner, &id, &[])?;
    let talk = Talk { api, owner: &owner, chat: &chat, agent: &agent, identity: &identity, n: std::cell::Cell::new(0), bound: WAKE };
    let wrote = talk.said(s, "write disk/first-life.txt shipped")?;
    std::thread::sleep(SHIPPED_WITHIN);
    let mut life = killed_and_back(s, api, &owner, &id);
    let read = talk.said(s, "read disk/first-life.txt")?;
    let (slept, _) = put_to_sleep(s, api, &owner, &id);
    let generation = life["generation"].as_u64().unwrap_or(0);
    let clean = head_of(&row(&id)["head"]).is_some_and(|(e, c)| e == generation && c);
    life["n"] = json!("wrote");
    life["wrote"] = json!(wrote);
    life["read"] = json!(read);
    life["slept"] = json!(slept);
    life["clean"] = json!(clean);
    if local {
        life["fellBack"] = json!(unusable_of(s, &id));
    }
    life["ok"] = json!(wrote == "wrote disk/first-life.txt" && first_line(&read) == "read disk/first-life.txt: shipped" && life["sent"] == true && life["woke"] == true && life["own"] == true && slept && clean);
    println!("      FIRSTLIFE {life}");
    lives.push(life);
    s.ok(FIRST_LIVES, lives.iter().all(|l| l["ok"] == true), json!(lives));
    Ok(())
}

/// Kills computer `id`'s life (again while its container is not yet one
/// its object signals), lets nothing more of it ship, and wakes it: what
/// its next start did.
fn killed_and_back(s: &Suite, api: &Api, owner: &crate::Keys, id: &str) -> Value {
    let row = || lever(api, id, "fpac").map(|r| r.body).unwrap_or(Value::Null);
    let view = || api.signed(owner, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let t0 = Instant::now();
    let mut killed = Value::Null;
    let sent = s.eventually(Duration::from_secs(30), || {
        killed = lever(api, id, "kill").ok().filter(|r| r.status == 200).map(|r| r.body).unwrap_or(Value::Null);
        killed["killed"].is_u64()
    });
    let kill_ms = t0.elapsed().as_millis() as u64;
    let generation = killed["killed"].as_u64().unwrap_or(0);
    // its container gone, nothing more of that life ships
    std::thread::sleep(SHIPPED_WITHIN);
    let left = row()["head"].clone();
    let _ = api.signed(owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let woke = s.eventually(WAKE * 3, || view()["restored"]["generation"].as_u64().is_some_and(|g| g > generation) && phase(api, owner, id) == "awake");
    let up = view();
    let now = up["restored"]["generation"].as_u64().unwrap_or(0);
    let head = row()["head"].clone();
    let own = head_of(&head).is_some_and(|(e, _)| e == now);
    json!({ "computer": id, "sent": sent, "killMs": kill_ms, "killed": generation, "left": left, "woke": woke, "from": up["restored"]["from"], "generation": now, "head": head, "own": own })
}

/// The platform's lines of the starts that found computer `id`'s head
/// unusable (`{"disk": "unusable", …}`), as the node logs them.
fn unusable_of(s: &Suite, id: &str) -> Vec<Value> {
    super::vault::logs(s)
        .lines()
        .filter_map(|l| l.find(r#"{"computer":"#).map(|at| l[at..].to_string()))
        .filter_map(|l| serde_json::Deserializer::from_str(&l).into_iter::<Value>().next().and_then(Result::ok))
        .filter(|v| v["computer"] == id && v["disk"] == "unusable")
        .collect()
}

/// An agent on computer `id` (a plain fragment made members only, `files`
/// beside its fragment.json), and a chat it is in: its join wakes the
/// computer. Its name, its chat's, and its identity.
fn agent_in_chat(s: &Suite, api: &Api, owner: &crate::Keys, id: &str, files: &[(&str, Option<&[u8]>)]) -> Result<(String, String, String)> {
    let agent_name = s.named(api, owner, "aspen")?;
    let agent = s.create(api, owner, &agent_name)?;
    let mut changes = vec![("fragment.json", Some(AGENT_JSON))];
    changes.extend_from_slice(files);
    s.commit(&agent, &changes);
    s.deploy(&agent);
    members_only(api, owner, &agent_name)?;
    let r = api.signed(owner, "PUT", &format!("/api/computers/{id}/agents/{agent_name}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && !identity.is_empty(), "its agent assigned: {r}");
    let chat = s.named(api, owner, "chat")?;
    let c = s.create(api, owner, &chat)?;
    s.commit(&c, &[("fragment.json", Some(CHAT_JSON))]);
    s.deploy(&c);
    let r = api.signed(owner, "PUT", &format!("/api/f/{chat}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "its agent joins the chat: {r}");
    let _ = s.eventually(Duration::from_secs(10), || told(api, owner, &agent_name, &chat).len() == 1);
    Ok((agent_name, chat, identity))
}

/// The deployment's own image's lives on a disk (hosted, ours: Hermes).
const LIVES: usize = 10;
/// The lives (from 0) that export its owner's data, then say a message to
/// its agent (a turn of the cheap tier's: a paid call, and Hermes' title
/// one more), and the life that crashes after both instead of sleeping.
/// Three exports, a person's most a day (`limits::PER_DAY`).
const EXPORTS_IN: [usize; 3] = [1, 4, 9];
const SAID_IN: [usize; 4] = [0, 1, 4, 7];
const CRASHED_IN: usize = 4;
/// What the run lends its person: its turns' calls, with room.
const IMAGE_PAID_CALLS: u64 = 14;
/// A turn of the cheap tier's, on a computer that just woke.
const TURN: Duration = Duration::from_secs(6 * 60);
const IMAGE_ON_DISK: &str = "the deployment's own image on a disk: each of ten wakes mounts it, each sleep's final flush posts that life's head, clean, and the start after a crash mounts it";
const IMAGE_HELD: &str = "its databases stay live on a disk: each export's copy of Hermes' state.db (its hold's, the live file left out) holds every message said before it, those said after an earlier export's hold included, so no boot put a hold's copies back";

/// The deployment's own image on a disk (hosted, ours: Hermes): a person
/// lent `IMAGE_PAID_CALLS`, their computer, and its agent on the cheap
/// tier. `LIVES` lives, each start mounting the disk and each sleep's final
/// flush posting that life's head, clean, one a crash instead. An export
/// holds the computer (Hermes' boot copies each SQLite database into
/// `/data/held-copies` for that save); on a disk no boot puts those copies
/// back, its live databases being newer, so each export's copy of Hermes'
/// `state.db` holds every message said before it.
fn image_on_disk(s: &mut Suite, api: &Api) -> Result<()> {
    let keys = crate::Keys::generate();
    let (session, _) = api.e2e_sign_in(&Api::email_of(&keys), IMAGE_PAID_CALLS)?;
    let me = api.approve(&session, &keys)?;
    anyhow::ensure!(me.status == 200 && me.body["id"].is_string(), "an approved key works: {me}");
    let made = api.signed(&keys, "POST", "/api/computers", Some(&json!({})))?;
    let id = made.body["computer"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(made.status == 200 && id.starts_with("computer:"), "a computer: {made}");
    let view = || api.signed(&keys, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let (agent, chat, identity) = agent_in_chat(s, api, &keys, &id, &[("agent.json", Some(br#"{ "tier": "cheap" }"#))])?;
    let talk = Talk { api, owner: &keys, chat: &chat, agent: &agent, identity: &identity, n: std::cell::Cell::new(0), bound: TURN };
    let evidence = s.dir("disks-held");
    let (mut lives, mut exports, mut said) = (vec![], vec![], Vec::<String>::new());
    let mut generation = 0u64;
    for life in 0..LIVES {
        // its join woke the first
        if life > 0 {
            let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
        }
        let woke = s.eventually(WAKE * 5, || view()["restored"]["generation"].as_u64().is_some_and(|g| g > generation) && phase(api, &keys, &id) == "awake");
        let up = view();
        generation = up["restored"]["generation"].as_u64().unwrap_or(generation);
        if life == 0 {
            // its agent follows its chat (the shell's ready) before it is told anything
            let following = || {
                api.signed(&keys, "GET", &format!("/api/f/{chat}/subscriptions"), None)
                    .ok()
                    .is_some_and(|r| r.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["wake"] == true && x["principal"] == identity.as_str() && x["channel"] == "chat")))
            };
            let _ = s.eventually(TURN, following);
        }
        let mut l = json!({ "life": life + 1, "woke": woke, "from": up["restored"]["from"], "generation": generation });
        if EXPORTS_IN.contains(&life) {
            let e = held_state(s, api, &keys, &id, &said, &evidence.join(format!("state-{}.db", life + 1)))?;
            println!("      (life {}'s export: {e})", life + 1);
            l["export"] = e["ok"].clone();
            exports.push(e);
        }
        if SAID_IN.contains(&life) {
            let token = format!("{}-{}", s.name("held"), life + 1);
            let reply = talk.said(s, &format!("Please answer with the single word ok. ({token})"))?;
            l["said"] = json!(token);
            l["answered"] = json!(!reply.is_empty());
            said.push(token);
        }
        if life == CRASHED_IN {
            // its turn's writes on its disk, and shipped
            std::thread::sleep(SHIPPED_WITHIN * 3);
            l["killed"] = json!(lever(api, &id, "kill").is_ok_and(|r| r.status == 200));
        } else {
            let (slept, _) = put_to_sleep(s, api, &keys, &id);
            let head = head_of(&lever(api, &id, "fpac").map(|r| r.body["head"].clone()).unwrap_or(Value::Null));
            l["slept"] = json!(slept);
            l["head"] = json!(head.map(|(e, c)| json!([e, c])));
        }
        println!("      (life {}: {l})", life + 1);
        lives.push(l);
    }
    let ok = lives.iter().all(|l| l["woke"] == true && l["from"] == "disk" && (l["killed"] == true || (l["slept"] == true && l["head"][1] == true && l["head"][0] == l["generation"])));
    s.ok(IMAGE_ON_DISK, ok, json!({ "image": view()["image"], "lives": lives }));
    let counts: Vec<i64> = exports.iter().map(|e| e["messages"].as_i64().unwrap_or(-1)).collect();
    let answered = lives.iter().filter(|l| l["said"].is_string()).all(|l| l["answered"] == true);
    let held = exports.len() == EXPORTS_IN.len() && exports.iter().all(|e| e["ok"] == true) && counts.windows(2).all(|w| w[0] <= w[1]) && answered;
    s.ok(IMAGE_HELD, held, json!({ "exports": exports, "said": said, "answered": answered }));
    Ok(())
}

/// Its owner's export, and Hermes' `state.db` as it holds it: the copy its
/// hold made (`/data/held-copies`, as its manifest names it, kept at
/// `keep`), the live file left out of the save, and each message of `said`
/// in that copy.
fn held_state(s: &Suite, api: &Api, keys: &crate::Keys, id: &str, said: &[String], keep: &std::path::Path) -> Result<Value> {
    use std::io::Read;
    let (summary, files) = exported(s, api, keys, id)?;
    let record = files.get(&fragment_core::export::save_path(id, "/data")).cloned().unwrap_or_default();
    let tar = unzstd(&record)?;
    let (mut held, mut names) = (std::collections::BTreeMap::new(), vec![]);
    for entry in tar::Archive::new(tar.as_slice()).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let path = path.trim_start_matches("./").trim_start_matches('/');
        let path = path.strip_prefix("data/").unwrap_or(path).to_string();
        if path.starts_with("held-copies/") || path.starts_with("hermes/state.db") {
            let mut bytes = vec![];
            entry.read_to_end(&mut bytes)?;
            held.insert(path.clone(), bytes);
        }
        if names.len() < 12 {
            names.push(path);
        }
    }
    let manifest: Value = held.get("held-copies/manifest.json").and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
    let copy = manifest["copies"].as_array().and_then(|c| c.iter().find(|c| c["path"] == "/data/hermes/state.db")).and_then(|c| c["file"].as_str()).map(|f| format!("held-copies/{f}"));
    let (mut messages, mut found) = (-1i64, vec![]);
    if let Some(bytes) = copy.as_ref().and_then(|c| held.get(c)) {
        std::fs::write(keep, bytes)?;
        let db = rusqlite::Connection::open_with_flags(keep, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        messages = db.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
        for token in said {
            let n: i64 = db.query_row("SELECT count(*) FROM messages WHERE role = 'user' AND content LIKE ?1", [format!("%{token}%")], |r| r.get(0))?;
            found.push(n > 0);
        }
    }
    let live_left_out = !held.contains_key("hermes/state.db");
    let ok = summary["ok"] == true && messages >= 0 && found.len() == said.len() && found.iter().all(|f| *f) && live_left_out;
    Ok(json!({ "ok": ok, "built": summary["ok"], "save": summary["save"]["id"], "copy": copy, "copies": manifest["copies"].as_array().map(Vec::len), "messages": messages, "said": said, "found": found, "liveLeftOut": live_left_out, "first": names }))
}

/// A save's record, its zstd undone (the box's `zstd`).
fn unzstd(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("zstd").arg("-dcq").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("zstd's stdin"))?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output()?;
    writer.join().map_err(|_| anyhow::anyhow!("the write to zstd panicked"))??;
    anyhow::ensure!(out.status.success(), "zstd -dc: {}", out.status);
    Ok(out.stdout)
}

/// Its owner's export, pressed with the computer asleep: built (its disk's
/// save taken awake for it on the way), its meta naming that save, taken
/// after the press, each record's file whole in the archive.
fn disk_exported(s: &Suite, api: &Api, keys: &crate::Keys, id: &str) -> Result<Value> {
    Ok(exported(s, api, keys, id)?.0)
}

/// Its owner's export, as `disk_exported` checks it, and its archive's files.
fn exported(s: &Suite, api: &Api, keys: &crate::Keys, id: &str) -> Result<(Value, std::collections::BTreeMap<String, Vec<u8>>)> {
    use std::io::Read;
    let pressed_at = super::computers::now_ms();
    let pressed = api.signed(keys, "POST", "/api/export", None)?;
    let mut export = Value::Null;
    let built = s.eventually(WAKE * 4, || {
        export = api.signed(keys, "GET", "/api/export", None).map(|r| r.body["export"].clone()).unwrap_or(Value::Null);
        export["state"] != "building"
    });
    let download = api.signed(keys, "GET", &format!("/api/export/{}/download", export["id"].as_str().unwrap_or("")), None)?;
    let mut files = std::collections::BTreeMap::new();
    for entry in tar::Archive::new(download.bytes.as_slice()).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        files.insert(path, bytes);
    }
    let hex = id.strip_prefix("computer:").unwrap_or("");
    let meta: Value = files.get(&format!("{}/computers/{hex}/meta.json", fragment_core::export::ROOT)).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
    let save = &meta["save"];
    let records = save["records"].as_array().cloned().unwrap_or_default();
    let whole = !records.is_empty() && records.iter().all(|r| files.get(&fragment_core::export::save_path(id, r["dir"].as_str().unwrap_or("/data"))).is_some_and(|b| fragment_core::blob::sha256_hex(b) == r["sha256"].as_str().unwrap_or("-")));
    let kept = lever(api, id, "saves")?.body["saves"].clone();
    let ok = pressed.status == 200 && built && export["state"] == "ready" && save["at"].as_i64().is_some_and(|at| at >= pressed_at) && whole && kept[0]["id"] == save["id"];
    Ok((json!({ "ok": ok, "export": export, "save": save, "whole": whole, "skipped": meta["skipped"], "kept": kept }), files))
}

/// Its agent's chat, as the stub's script answers it.
struct Talk<'a> {
    api: &'a Api,
    owner: &'a crate::Keys,
    chat: &'a str,
    agent: &'a str,
    identity: &'a str,
    n: std::cell::Cell<u32>,
    /// How long its answer is waited for: the stub's script, or a model's.
    bound: Duration,
}

impl Talk<'_> {
    /// Its agent's answer to `text`, said to it in the chat.
    fn said(&self, s: &Suite, text: &str) -> Result<String> {
        self.n.set(self.n.get() + 1);
        let body = json!({ "id": format!("d{}", self.n.get()), "body": { "text": text } });
        let r = self.api.signed(self.owner, "POST", &format!("/api/f/{}/channels/chat", self.chat), Some(&body))?;
        let seq = r.body["record"]["seq"].as_i64().unwrap_or(0);
        let turn = super::computers::turn_of(self.agent, self.chat, "chat", seq);
        // the records after its message: a chat of a thousand cycles is more
        // than a page from its start
        let after = || self.api.signed(self.owner, "GET", &format!("/api/f/{}/channels/chat?after={seq}", self.chat), None).ok().and_then(|r| r.body["records"].as_array().cloned()).unwrap_or_default();
        let reply = || agent_replies(&after(), self.identity).into_iter().find(|x| x["body"]["turn"] == turn.as_str());
        s.eventually(self.bound, || reply().is_some());
        Ok(reply().and_then(|x| x["body"]["text"].as_str().map(str::to_string)).unwrap_or_default())
    }
}

/// A reply's first line: what the stub's script answered. After a crash its
/// bridge may add a note that its memory of the chat is behind (its memory
/// is a file, and the crash took its last unsynced write).
fn first_line(reply: &str) -> &str {
    reply.lines().next().unwrap_or_default()
}

/// A head's line's epoch, and whether it marks a clean sleep
/// (`fragment_core::computer::fpac::Head`'s spelling).
fn head_of(v: &Value) -> Option<(u64, bool)> {
    let head = fragment_core::computer::fpac::Head::decode(v.as_str()?)?;
    Some((head.epoch, head.clean))
}

/// Its start lines' `start`s and its sleep lines, as the node logs them
/// (docs/computers.md, "The start line" and "The sleep line").
fn lines_of(s: &Suite, id: &str) -> (Vec<Value>, Vec<Value>) {
    let logs = super::vault::logs(s);
    let of = |marker: &str| -> Vec<Value> {
        logs.lines()
            .filter_map(|l| l.find(marker).map(|at| &l[at..]))
            .filter_map(|l| serde_json::Deserializer::from_str(l).into_iter::<Value>().next().and_then(Result::ok))
            .filter(|v| v["computer"] == id)
            .collect()
    };
    (of(r#"{"start":{"generation":"#).into_iter().map(|v| v["start"].clone()).collect(), of(r#"{"sleep":{"generation":"#))
}
