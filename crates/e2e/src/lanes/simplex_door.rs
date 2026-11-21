//! A SimpleX door on the platform (cell/door.mjs, docs/computers.md "F"):
//! its object starts its container from the door's image; the runner
//! restores (none: a new door), saves to the bucket and names each save to
//! its object; a crash starts it again from its newest save, in step; a
//! save that will not restore is passed over for the one before it; its
//! saves are kept to the newest three; and a planned stop saves last and
//! is not started again until asked.
//!
//! Lower rung, labeled so: local workerd and Docker, its containers
//! reaching no SimpleX server (the e2e's reach none), so no link here:
//! SimpleX itself is the lab's (images/bridge/tests/door.rs) and the
//! hosted check's. Run by name: its stack builds the door's image
//! (devstack::Fleet::doors).

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

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

pub fn door(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name(NAME, &[Need::Node, Need::Levers, Need::LocalDocker], "its stack builds the door's image (SimpleX's core and its runner)") {
        return Ok(());
    }
    let door = "e2e-door-simplex--k3x9";
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
    Ok(())
}
