//! Placement (docs/self-host.md, seam 2; `fragment_core::placement`): on a
//! run with sandcastle nodes (`FRAGMENT_E2E_NODES=two`), one that listens
//! and one that dials in. A computer is placed at its first start, not
//! before; two placed one after the other go to two nodes (the rule: the
//! fewest computers for a node's capacity, every placement so far balanced
//! on two nodes of one capacity); each runs its image on its node, its
//! screen through its port; asleep and woken, it wakes where it was; its
//! container killed under it while awake, it is started again there by
//! itself and comes up (on either node: one listens, one dials in). Each
//! node in turn goes down: a wake of a computer on it answers 503
//! `node_down`, typed and within seconds, its view says why, and a new
//! computer goes to the node that is up; the node back, the computer wakes
//! on it again. A node down under an awake computer (`outage`, found 39):
//! its process restarted, the computer stays awake, its work kept; down
//! for good, the computer says its node is down, and goes on when it is
//! back (on each kind of node, and on a real engine's one). That pinning
//! outlives a restart of the platform is the restart section's.

use std::time::{Duration, Instant};

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Call};
use crate::Suite;

/// A start of the stub on a node and its restore: well under this.
const WAKE: Duration = Duration::from_secs(90);
/// A wake of a computer on a node that is down answers within this: a
/// node that listens refuses the connection at once; one that dials in is
/// waited for as long as its object waits for a dial (node.mjs's
/// HEALTH_MS, past uplink.mjs's UPLINK_WAIT_MS).
const DOWN_ANSWER: Duration = Duration::from_secs(15);
/// The stub's bridge has booted and made its state under `/data` by then.
const BOOTED: Duration = Duration::from_secs(2);
/// A node's process restarting under an awake computer is down this long.
const RESTARTING: Duration = Duration::from_secs(3);
/// The computer's watch has looked at a node back from a restart by then:
/// its looks' pauses (node.mjs: 1 s, doubling), each look's bound (6 s).
const LOOKED: Duration = Duration::from_secs(15);
/// A node silent past node.mjs's SILENT_MS (2 minutes) is said to be down
/// by then: that bound, the uplink's wait for a dial before a call it
/// holds is cut (uplink.mjs's AGAIN_WAIT_MS, 30 s), a look's longest pause
/// (30 s) and its bound (6 s).
const SAID_DOWN: Duration = Duration::from_secs(120 + 30 + 30 + 6 + 14);
/// The node back, the computer goes on by then: a look's longest pause and
/// its bound, and an uplink's dial.
const GOES_ON: Duration = Duration::from_secs(30 + 6 + 24);
/// The file each computer keeps across its node's outage.
const KEPT: &str = "/data/outage.txt";

/// A person's computer, made.
fn computer(api: &Api, owner: &Keys) -> Result<(String, Value)> {
    let r = api.signed(owner, "POST", "/api/computers", Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "making a computer: {r}");
    Ok((r.body["computer"].as_str().unwrap_or("").to_string(), r.body.clone()))
}

fn wake(api: &Api, owner: &Keys, id: &str) -> Result<crate::api::Reply> {
    api.signed(owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})))
}

fn sleep(api: &Api, owner: &Keys, id: &str) -> Result<crate::api::Reply> {
    api.signed(owner, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})))
}

fn view(api: &Api, owner: &Keys, id: &str) -> Value {
    api.signed(owner, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null)
}

/// The build its screen serves, through its port, as its owner signs.
fn version(api: &Api, owner: &Keys, origin: &str) -> String {
    api.call(Call { method: "GET", url: format!("{origin}/p/6080/version.txt"), keys: Some(owner), ..Call::default() }).map(|r| r.text.trim().to_string()).unwrap_or_default()
}

/// Kills the one container running on node `node` that started at or
/// after `since_ms` (computer `id`'s, awake there with a backup to restore)
/// under it, behind the platform's back, and waits up to WAKE for the
/// computer to start it again there by itself and come up, its screen
/// serving the stub's first build: whether it did, and its view and how
/// long it took.
pub(crate) fn killed_comes_back(s: &Suite, api: &Api, owner: &Keys, id: &str, node: &str, origin: &str, since_ms: u64) -> Result<(bool, String)> {
    let running = s.node_running_since(node, since_ms)?;
    anyhow::ensure!(running.len() == 1, "the node {node} runs one container started since {since_ms}: {running:?}");
    std::thread::sleep(BOOTED);
    let killed_at = fragment_devstack::sandcastle::now_ms();
    s.node_kill(node, &running[0])?;
    let t0 = Instant::now();
    let back = s.eventually(WAKE, || s.node_running_since(node, killed_at).is_ok_and(|c| c.len() == 1) && view(api, owner, id)["phase"] == "awake" && version(api, owner, origin) == "1");
    let v = view(api, owner, id);
    Ok((back && v["node"] == node && v["why"].is_null(), format!("{v} in {:.1?}", t0.elapsed())))
}

/// Runs `argv` in computer `id`'s container (the lever's `exec`): its
/// output when it exits 0, else what the lever answered.
fn exec(api: &Api, id: &str, argv: &[&str]) -> std::result::Result<String, String> {
    let r = super::computers::lever_with(api, id, json!({ "op": "exec", "argv": argv })).map_err(|e| e.to_string())?;
    match r.status == 200 && r.body["exitCode"] == 0 {
        true => Ok(r.body["output"].as_str().unwrap_or("").trim().to_string()),
        false => Err(r.to_string()),
    }
}

/// A node down under an awake computer (docs/self-host.md, found 39), on
/// each of `on` (`(owner, computer, node)`, each awake on its node, the
/// nodes all different) at once: a file written in it first. Its node's
/// process restarted (its engine, and the container, kept): the computer
/// stays awake on the same start, its file kept, no rollback. Its node
/// down past node.mjs's SILENT_MS: it says its node is down (its view, and
/// a wake's 503 `node_down`), still on the same start. The node back: it
/// goes on by itself, its file kept, its screen answering, and its
/// container is watched as before (killed under it, it is started again).
/// `since_ms`: before their containers started, the only ones on their
/// nodes since.
fn outage(s: &mut Suite, api: &Api, on: &[(&Keys, String, String)], since_ms: u64) -> Result<()> {
    let start_of = |owner: &Keys, id: &str| {
        let v = view(api, owner, id);
        (v["generation"].clone(), v["rollbacks"].clone(), v["restored"].clone())
    };
    let mut before = vec![];
    for (owner, id, node) in on {
        std::thread::sleep(BOOTED);
        let token = format!("{node} {}", fragment_devstack::sandcastle::now_ms());
        if let Err(e) = exec(api, id, &["sh", "-c", &format!("echo '{token}' > {KEPT}")]) {
            anyhow::bail!("writing {KEPT} in {id}: {e}");
        }
        before.push((token, start_of(owner, id)));
    }
    let same = |owner: &Keys, id: &str, i: usize| start_of(owner, id) == before[i].1 && exec(api, id, &["cat", KEPT]).as_deref() == Ok(before[i].0.as_str());

    // its process restarts, its engine keeping the container
    for (_, _, node) in on {
        s.node_down(node)?;
        std::thread::sleep(RESTARTING);
        s.node_up(node)?;
    }
    std::thread::sleep(LOOKED);
    for (i, (owner, id, node)) in on.iter().enumerate() {
        let v = view(api, owner, id);
        s.ok(
            &format!("its node's process restarted under it (one that {}), the computer stays awake on the same start: its file kept, no restart, no rollback", s.node_reach(node)),
            v["phase"] == "awake" && v["why"].is_null() && same(owner, id, i),
            format!("{v} / {:?}", exec(api, id, &["cat", KEPT])),
        );
    }

    // down past the bound: it says so, and runs on there
    for (_, _, node) in on {
        s.node_down(node)?;
    }
    let t0 = Instant::now();
    let said = |owner: &Keys, id: &str, node: &str| view(api, owner, id)["why"].as_str().is_some_and(|w| w.contains(node) && w.contains("has not answered"));
    s.eventually(SAID_DOWN, || on.iter().all(|(owner, id, node)| said(owner, id, node)));
    let took = t0.elapsed();
    for (i, (owner, id, node)) in on.iter().enumerate() {
        let r = wake(api, owner, id)?;
        let v = view(api, owner, id);
        s.ok(
            &format!("its node down past two minutes (one that {}), the computer says so, naming it, and a wake answers 503 node_down; the same start, no rollback", s.node_reach(node)),
            said(owner, id, node) && r.status == 503 && r.body["error"] == "node_down" && r.body["message"].as_str().is_some_and(|m| m.contains(node.as_str())) && start_of(owner, id) == before[i].1,
            format!("{v} / {r} after {took:.1?}"),
        );
    }

    // back, it goes on by itself
    for (_, _, node) in on {
        s.node_up(node)?;
    }
    let t0 = Instant::now();
    s.eventually(GOES_ON, || on.iter().all(|(owner, id, _)| view(api, owner, id)["why"].is_null()));
    let took = t0.elapsed();
    for (i, (owner, id, node)) in on.iter().enumerate() {
        let v = view(api, owner, id);
        let origin = v["origin"].as_str().unwrap_or("").to_string();
        s.ok(
            &format!("its node back (one that {}), the computer goes on by itself: the same start, its file kept, its screen answering, no rollback", s.node_reach(node)),
            v["phase"] == "awake" && v["why"].is_null() && same(owner, id, i) && version(api, owner, &origin) == "1",
            format!("{v} after {took:.1?}"),
        );
    }
    // still watched: its watch went on through the outage, not lost in it
    for (owner, id, node) in on {
        let origin = view(api, owner, id)["origin"].as_str().unwrap_or("").to_string();
        let (back, shown) = killed_comes_back(s, api, owner, id, node, &origin, since_ms)?;
        s.ok(&format!("after its node's outage (one that {}), its container is watched as before: killed under it, the computer starts it again by itself", s.node_reach(node)), back, shown);
    }
    Ok(())
}

pub fn placement(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("placement", &[crate::Need::Nodes, crate::Need::Computers, crate::Need::Node]) {
        return Ok(());
    }
    if s.node_ids().len() < 2 {
        // the real engine's one node: its outage under an awake computer
        let node = s.node_ids().first().cloned().unwrap_or_default();
        let a = api.person()?;
        let (c, _) = computer(api, &a)?;
        let since = fragment_devstack::sandcastle::now_ms();
        let r = wake(api, &a, &c)?;
        anyhow::ensure!(r.status == 200 && r.body["phase"] == "awake" && r.body["node"] == node.as_str(), "waking a computer on {node}: {r}");
        outage(s, api, &[(&a, c.clone(), node)], since)?;
        sleep(api, &a, &c)?;
        s.skip("computers spread across nodes, each pinned to its own", "it needs two nodes (FRAGMENT_E2E_NODES=two); this run's one is the real engine's");
        return Ok(());
    }
    let ids = s.node_ids();
    anyhow::ensure!(ids.len() == 2, "the run's nodes: {ids:?}");
    let (a, b) = (api.person()?, api.person()?);
    let (ca, made) = computer(api, &a)?;
    s.ok("a computer is placed at its first start, not before", made["node"].is_null() && made["phase"] == "asleep", &made);
    let r = wake(api, &a, &ca)?;
    let na = r.body["node"].as_str().unwrap_or("").to_string();
    s.ok("its first start places it on a node, and it wakes there", r.status == 200 && r.body["phase"] == "awake" && ids.contains(&na), &r);
    let (cb, _) = computer(api, &b)?;
    let r = wake(api, &b, &cb)?;
    let nb = r.body["node"].as_str().unwrap_or("").to_string();
    s.ok(
        "the next one goes to the other node: the fewest computers for its capacity",
        r.status == 200 && r.body["phase"] == "awake" && ids.contains(&nb) && nb != na,
        json!({ "first": na, "second": r.body }),
    );
    // each runs its image on its node: one reached directly, one over its uplink
    let (oa, ob) = (made["origin"].as_str().unwrap_or("").to_string(), view(api, &b, &cb)["origin"].as_str().unwrap_or("").to_string());
    let (va, vb) = (version(api, &a, &oa), version(api, &b, &ob));
    s.ok("both computers' screens answer through their ports, one on each node, whether it listens or dials in", va == "1" && vb == "1", format!("{na} ({}): {va:?}; {nb} ({}): {vb:?}", s.node_reach(&na), s.node_reach(&nb)));

    // asleep and woken, it wakes where it was: its /data saved on the way
    // down, restored on the way up (the stub's bridge makes /data as it
    // boots, within a second: a sleep sooner saves nothing to restore)
    std::thread::sleep(BOOTED);
    sleep(api, &a, &ca)?;
    sleep(api, &b, &cb)?;
    let woken_at = fragment_devstack::sandcastle::now_ms();
    let r = wake(api, &a, &ca)?;
    s.ok("asleep and woken, a computer wakes on the node it was placed on", r.status == 200 && r.body["phase"] == "awake" && r.body["node"] == na.as_str(), &r);
    let r = wake(api, &b, &cb)?;
    anyhow::ensure!(r.status == 200 && r.body["phase"] == "awake", "waking the second computer again: {r}");

    // its container killed under it while it is awake, the computer starts
    // it again on its node by itself (the start its exit's report makes,
    // with a backup to restore: an exec first), and it comes up
    for (id, owner, pinned, origin) in [(na.clone(), &a, ca.clone(), oa.clone()), (nb.clone(), &b, cb.clone(), ob.clone())] {
        let reach = s.node_reach(&id);
        let (back, shown) = killed_comes_back(s, api, owner, &pinned, &id, &origin, woken_at)?;
        s.ok(&format!("its container killed under it, awake (on the node that {reach}), the computer starts it again there by itself, and it comes up"), back, shown);
    }
    sleep(api, &a, &ca)?;
    sleep(api, &b, &cb)?;

    // each node in turn goes down
    for (id, owner, pinned, other) in [(nb.clone(), &b, cb.clone(), na.clone()), (na.clone(), &a, ca.clone(), nb.clone())] {
        let reach = s.node_reach(&id);
        s.node_down(&id)?;
        let t0 = Instant::now();
        let r = wake(api, owner, &pinned)?;
        let took = t0.elapsed();
        s.ok(
            &format!("its node down (one that {reach}), a wake answers 503 node_down, naming it, within {DOWN_ANSWER:?}"),
            r.status == 503 && r.body["error"] == "node_down" && r.body["message"].as_str().is_some_and(|m| m.contains(&id)) && took < DOWN_ANSWER,
            format!("{r} in {took:.1?}"),
        );
        let v = view(api, owner, &pinned);
        s.ok("its view says why, and it stays placed on its node", v["node"] == id.as_str() && v["phase"] != "awake" && v["why"].as_str().is_some_and(|w| w.contains(&id)), &v);
        let c = api.person()?;
        let (cc, _) = computer(api, &c)?;
        let r = wake(api, &c, &cc)?;
        s.ok("a new computer goes to the node that is up", r.status == 200 && r.body["phase"] == "awake" && r.body["node"] == other.as_str(), &r);
        sleep(api, &c, &cc)?;
        s.node_up(&id)?;
        // a node that dials in is back once its uplink is: a wake may find it dialing
        let back = s.eventually(WAKE, || wake(api, owner, &pinned).is_ok_and(|r| r.status == 200 && r.body["phase"] == "awake"));
        let v = view(api, owner, &pinned);
        let why = v["why"].as_str().unwrap_or("");
        s.ok("its node back, the computer wakes on it again, and its view no longer says its node is down", back && v["node"] == id.as_str() && !why.contains(&id), &v);
        sleep(api, owner, &pinned)?;
    }

    // each kind of node down under an awake computer, both at once
    let since = fragment_devstack::sandcastle::now_ms();
    for (owner, id) in [(&a, &ca), (&b, &cb)] {
        let r = wake(api, owner, id)?;
        anyhow::ensure!(r.status == 200 && r.body["phase"] == "awake", "waking {id} again: {r}");
    }
    outage(s, api, &[(&a, ca.clone(), na.clone()), (&b, cb.clone(), nb.clone())], since)?;
    sleep(api, &a, &ca)?;
    sleep(api, &b, &cb)?;
    Ok(())
}
