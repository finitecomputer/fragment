//! Computers on disks (docs/computers.md, "A disk"), on a node whose stub's
//! new computers go on them (`Shape::Disks`) and a runtime that gives a
//! container FUSE (krun's: `Need::Disks`). A computer made there is on a
//! disk for good: each start mounts `/data` from FPAC through its route,
//! nothing restored and no gate; each sleep is a signal and a wait, its
//! daemon's final flush posting a clean head; awake, nothing is saved.
//!
//! Valid: a new disk, a write read after each of `cycles()` sleeps.
//! Invalid: a head that will not open, fallen back past to the one before.
//! Replay: a crash while it runs, its acknowledged writes replayed by the
//! next mount. Restart: every wake is a new life on the same disk, each
//! sleep's head that life's and clean.

use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use super::computers::{agent_replies, lever, lever_with, members_only, phase, put_to_sleep, told, AGENT_JSON, CHAT_JSON};
use super::jobs::records;
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

const DISK_CHECKS: [&str; 8] = [
    "a computer made while its image goes on disks is on one: a new disk of 20,000 MiB, no head yet",
    "its first start mounts the new disk: the start line's path is disk, saying how long its mount took",
    "a sleep is a signal and a wait: its daemon's final flush posts that life's head, clean, its sleep line saying so, and no save is taken",
    "a wake mounts the disk as it slept: what it wrote is there, nothing restored, its line saying what its last sleep left unshipped",
    "every write is read after the wake that follows it, each of its sleeps clean",
    "a crash while it runs loses no write it acknowledged: the start after it mounts the disk as that life left it",
    "a head that will not open is fallen back past: that start fails as unusable, the next mounts the head before it, no strike",
    "awake or asleep, no save of a disk is ever taken",
];

pub fn disks(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("disks", &[Need::Disks, Need::Computers, Need::Levers, Need::Node]) {
        return Ok(());
    }
    let _ = api;
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
    let row = || lever(api, &id, "fpac").map(|r| r.body).unwrap_or(Value::Null);
    let made = row();
    s.ok(DISK_CHECKS[0], made["disk"] == true && made["volume"] == 20_000u64 << 20 && made["head"].is_null() && made["back"].is_null(), &made);

    // its agent, and a chat it is in: its join wakes the computer
    let agent_name = s.named(api, &owner, "aspen")?;
    let agent = s.create(api, &owner, &agent_name)?;
    s.commit(&agent, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&agent);
    members_only(api, &owner, &agent_name)?;
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{id}/agents/{agent_name}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && !identity.is_empty(), "its agent assigned: {r}");
    let chat = s.named(api, &owner, "chat")?;
    let c = s.create(api, &owner, &chat)?;
    s.commit(&c, &[("fragment.json", Some(CHAT_JSON))]);
    s.deploy(&c);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "its agent joins the chat: {r}");
    let _ = s.eventually(Duration::from_secs(10), || told(api, &owner, &agent_name, &chat).len() == 1);
    let awake = |s: &Suite| s.eventually(WAKE, || phase(api, &owner, &id) == "awake");
    let woke = awake(s);
    let view = || api.signed(&owner, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let first = view();
    let lines = |s: &Suite| lines_of(s, &id);
    let started = lines(s).0;
    let line1 = started.iter().find(|l| l["generation"] == 1).cloned().unwrap_or(Value::Null);
    s.ok(
        DISK_CHECKS[1],
        woke && first["restored"]["from"] == "disk" && line1["path"] == "disk" && line1["end"] == "ready" && line1["disk"]["mountMs"].is_u64() && line1.get("bytes").is_none(),
        json!({ "view": first["restored"], "line": line1 }),
    );

    // what its agent says to `write` and `read` (the stub's script)
    let talk = Talk { api, owner: &owner, chat: &chat, agent: &agent_name, identity: &identity, n: std::cell::Cell::new(0) };
    let wrote = talk.said(s, "write disk/first.txt one")?;
    let (slept, _) = put_to_sleep(s, api, &owner, &id);
    let after = row();
    let sleep1 = lines(s).1.into_iter().find(|l| l["sleep"]["generation"] == 1).unwrap_or(Value::Null);
    let saves = lever(api, &id, "saves")?.body["saves"].clone();
    s.ok(
        DISK_CHECKS[2],
        wrote == "wrote disk/first.txt" && slept && head_of(&after["head"]).is_some_and(|(epoch, clean)| epoch == 1 && clean) && sleep1["sleep"]["clean"] == true && sleep1["sleep"]["exited"] == true && saves.as_array().is_some_and(Vec::is_empty),
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
        woke && read == "read disk/first.txt: one" && again["restored"]["from"] == "disk" && line["path"] == "disk" && line["disk"]["sleptUnshipped"].is_u64(),
        json!({ "read": read, "restored": again["restored"], "line": line }),
    );

    // every write read after the wake that follows it, each sleep clean
    let mut cycled = vec![];
    for i in 0..cycles() {
        let token = format!("cycle-{i}");
        let wrote = talk.said(s, &format!("write disk/cycle.txt {token}"))?;
        let (slept, _) = put_to_sleep(s, api, &owner, &id);
        let head = head_of(&row()["head"]);
        let _ = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
        let woke = awake(s);
        let read = talk.said(s, "read disk/cycle.txt")?;
        let ok = wrote == "wrote disk/cycle.txt" && slept && head.is_some_and(|(_, clean)| clean) && woke && read == format!("read disk/cycle.txt: {token}");
        cycled.push(json!({ "i": i, "ok": ok, "read": read, "head": head.map(|(e, c)| json!([e, c])) }));
    }
    s.ok(DISK_CHECKS[4], cycled.iter().all(|c| c["ok"] == true), json!(cycled));

    // a crash while it runs: what it acknowledged, shipped, is mounted again
    let wrote = talk.said(s, "write disk/crash.txt synced")?;
    std::thread::sleep(SHIPPED_WITHIN);
    let before = view()["restored"]["generation"].as_u64().unwrap_or(0);
    let killed = lever(api, &id, "kill")?;
    let came_back = s.eventually(WAKE + WAKE, || view()["restored"]["generation"].as_u64().is_some_and(|g| g > before) && phase(api, &owner, &id) == "awake");
    let read = talk.said(s, "read disk/crash.txt")?;
    s.ok(DISK_CHECKS[5], wrote == "wrote disk/crash.txt" && killed.status == 200 && came_back && read == "read disk/crash.txt: synced", json!({ "read": read, "killed": killed.body, "view": view()["restored"] }));

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
        slept && set.status == 200 && woke && read == "read disk/crash.txt: synced" && unusable.is_some() && head_of(&fell["head"]).is_some_and(|(e, _)| e > epoch) && failures == 0,
        json!({ "set": set.body, "read": read, "unusable": unusable, "row": fell, "failures": failures }),
    );

    let saves = lever(api, &id, "saves")?.body;
    s.ok(DISK_CHECKS[7], saves["saves"].as_array().is_some_and(Vec::is_empty) && saves["aside"].as_array().is_none_or(Vec::is_empty), &saves["saves"]);
    Ok(())
}

/// Its agent's chat, as the stub's script answers it.
struct Talk<'a> {
    api: &'a Api,
    owner: &'a crate::Keys,
    chat: &'a str,
    agent: &'a str,
    identity: &'a str,
    n: std::cell::Cell<u32>,
}

impl Talk<'_> {
    /// Its agent's answer to `text`, said to it in the chat.
    fn said(&self, s: &Suite, text: &str) -> Result<String> {
        self.n.set(self.n.get() + 1);
        let body = json!({ "id": format!("d{}", self.n.get()), "body": { "text": text } });
        let r = self.api.signed(self.owner, "POST", &format!("/api/f/{}/channels/chat", self.chat), Some(&body))?;
        let seq = r.body["record"]["seq"].as_i64().unwrap_or(0);
        let turn = super::computers::turn_of(self.agent, self.chat, "chat", seq);
        let reply = || agent_replies(&records(self.api, self.owner, self.chat, "chat"), self.identity).into_iter().find(|x| x["body"]["turn"] == turn.as_str());
        s.eventually(WAKE, || reply().is_some());
        Ok(reply().and_then(|x| x["body"]["text"].as_str().map(str::to_string)).unwrap_or_default())
    }
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
