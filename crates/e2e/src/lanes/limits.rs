//! A tenant's reach: an app's database stops at 16 MiB and the rest of the
//! fragment keeps working, and an app cannot run code from strings, block
//! its thread, or use the parts of its Durable Object the platform takes
//! away. Its CPU and memory limits are the runtime's, which local workerd
//! does not enforce (spike S1): the hosted lane checks those.

use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value};

use super::app::ship;
use super::jobs;
use crate::api::Api;
use crate::Suite;

const HOARD_APP: &[u8] = include_bytes!("../../fixtures/hoard.mjs");
const HOARD_JSON: &[u8] = include_bytes!("../../fixtures/hoard.json");
const GUESTBOOK_APP: &[u8] = include_bytes!("../../fixtures/guestbook.mjs");
const GUESTBOOK_JSON: &[u8] = include_bytes!("../../fixtures/guestbook.json");
const MIB: i64 = 1024 * 1024;

pub fn facet_cap(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("facet-cap", &[]) {
        return Ok(());
    }
    let owner = api.person()?;
    let name = s.named(api, &owner, "hoard")?;
    let c = s.create(api, &owner, &name)?;
    ship(s, &c, HOARD_APP, HOARD_JSON);
    let neighbor = s.named(api, &owner, "hoard-neighbor")?;
    let n = s.create(api, &owner, &neighbor)?;
    ship(s, &n, GUESTBOOK_APP, GUESTBOOK_JSON);
    let size = |api: &Api| -> Value { api.op(&owner, &name, "size", "q", json!({})).map(|r| r.body["result"].clone()).unwrap_or(Value::Null) };
    // 4 MiB at a time until the platform refuses
    let mut stored = 0;
    let mut refusal = None;
    for i in 0..8 {
        let r = api.op(&owner, &name, "add", &format!("a{i}"), json!({ "mib": 4 }))?;
        if r.status == 200 {
            stored = r.body["result"]["rows"].as_i64().unwrap_or(0);
        } else {
            refusal = Some(r);
            break;
        }
    }
    let refused = refusal.as_ref().is_some_and(|r| r.status == 507 && r.error() == "storage_full");
    s.ok("an app's database stops at its 16 MiB cap (507 storage_full)", refused && (12..16).contains(&stored), format!("{stored} MiB stored; {}", refusal.as_ref().map_or("never refused".into(), |r| r.to_string())));
    let now = size(api);
    s.ok(
        "the refused mutation rolled back, and the app still answers",
        now["rows"].as_i64() == Some(stored) && now["bytes"].as_i64().is_some_and(|b| b <= 16 * MIB),
        &now,
    );
    let r = api.op(&owner, &name, "empty", "e1", json!({}))?;
    s.ok("emptying it works at the cap", r.status == 200 && r.body["result"]["rows"] == 0, &r);
    let r = api.op(&owner, &name, "add", "after", json!({ "mib": 4 }))?;
    s.ok("then it takes writes again", r.status == 200 && r.body["result"]["rows"] == 4, &r);
    let r = api.op(&owner, &neighbor, "sign", "n1", json!({ "text": "next door" }))?;
    s.ok("the other apps carry on", r.status == 200, &r);
    Ok(())
}

const PROBE_APP: &[u8] = include_bytes!("../../fixtures/probe.mjs");
const PROBE_JSON: &[u8] = include_bytes!("../../fixtures/probe.json");

/// What an app may not do: code from strings and `Atomics.wait` (the
/// runtime's), and an alarm, an async transaction, a KV write, or a facet
/// of its own (platform.mjs: S1 found an app's alarm wedges its facet).
pub fn lockdown(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("app-lockdown", &[]) {
        return Ok(());
    }
    let owner = api.person()?;
    let name = s.named(api, &owner, "probe")?;
    let c = s.create(api, &owner, &name)?;
    ship(s, &c, PROBE_APP, PROBE_JSON);
    let neighbor = s.named(api, &owner, "probe-neighbor")?;
    let n = s.create(api, &owner, &neighbor)?;
    ship(s, &n, GUESTBOOK_APP, GUESTBOOK_JSON);
    let r = api.status(&owner, &name)?;
    s.ok("the probe app installs", r.body["code"]["error"].is_null() && r.body["code"]["sha"].is_string(), &r.body["code"]);
    let tried = |op: &str| -> Result<(u16, Value)> {
        let r = api.op(&owner, &name, op, op, json!({}))?;
        println!("      ({op}: {r})");
        Ok((r.status, r.body["result"].clone()))
    };
    let refused = |(status, v): &(u16, Value), says: &str| *status == 200 && v["ok"] == false && v["error"].as_str().is_some_and(|e| e.contains(says));
    let r = tried("try_eval")?;
    s.ok("an app cannot eval", refused(&r, "Code generation from strings disallowed"), &r.1);
    let r = tried("try_function")?;
    s.ok("nor build a Function from a string", refused(&r, "Code generation from strings disallowed"), &r.1);
    let r = tried("try_wait")?;
    s.ok("nor block its thread in Atomics.wait", refused(&r, "Atomics.wait cannot be called"), &r.1);
    let r = tried("try_alarm")?;
    s.ok("nor set an alarm (its triggers are its schedule)", refused(&r, "an app has no alarm"), &r.1);
    let r = tried("try_alarm_proto")?;
    s.ok("nor find the original on the prototype", refused(&r, "an app has no alarm"), &r.1);
    let r = tried("try_transaction")?;
    s.ok("nor open a transaction of its own", refused(&r, "a mutation is the app's transaction"), &r.1);
    let r = tried("try_put")?;
    s.ok("nor write outside its SQLite", refused(&r, "an app writes its SQLite"), &r.1);
    let r = tried("try_facets")?;
    s.ok("nor start a facet of its own", refused(&r, "it starts none of its own"), &r.1);
    let r = api.op(&owner, &neighbor, "sign", "n1", json!({ "text": "next door" }))?;
    s.ok("its neighbors carry on", r.status == 200, &r);
    let r = api.op(&owner, &name, "hello", "h", json!({}))?;
    s.ok("and the app itself answers again", r.status == 200 && r.body["result"]["hello"] == "still here", &r);
    Ok(())
}

/// An app that makes `n` calls of one capability in one call into it, `par`
/// at a time: how many it made, or the runtime's refusal of one thrown on
/// (naming how many it made first), as an app that does not catch it
/// fails; and one that spins its CPU.
const BURN_APP: &[u8] = br#"import { DurableObject } from "cloudflare:workers";
export class App extends DurableObject {
  async burn({ op, n, par = 1 }) {
    const call = {
      "files.list": () => this.files.list("none/"),
      "files.read": () => this.files.readBytes("note.md"),
      "files.stat": () => this.files.stat("note.md"),
      "search.status": () => this.searchIndex.status(),
    }[op];
    let made = 0;
    while (made < n) {
      const round = await Promise.allSettled(Array.from({ length: Math.min(par, n - made) }, call));
      made += round.filter((r) => r.status === "fulfilled").length;
      const refused = round.find((r) => r.status === "rejected");
      if (refused) throw new Error(`after ${made}: ${refused.reason?.message ?? refused.reason}`);
    }
    return { made };
  }
  // the same calls in a job: a throw the platform code answers as the run's failure
  async burn_job(input) {
    return this.burn(input);
  }
  // `iters` turns of arithmetic in one call: CPU time and no I/O
  spin({ iters }) {
    let x = 0x9e3779b9 | 0;
    for (let i = 0; i < iters; i++) x = (Math.imul(x ^ (x >>> 15), 0x2c1b3c6d) + i) | 0;
    return { x };
  }
}
"#;
const BURN_JSON: &[u8] = br#"{ "name": "burn", "operations": { "burn": { "kind": "query", "role": "viewer" }, "burn_job": { "kind": "job", "role": "editor" }, "spin": { "kind": "query", "role": "viewer" } } }"#;

/// Calls past the 50 subrequests an app facet was once held to, in one call.
const ASKED: u64 = 100;
/// Calls on one facet past the account's 10,000 subrequests a life, in
/// `FAR_CALLS` calls (each well inside a request's time).
const FAR: u64 = 10_050;
const FAR_CALLS: u64 = 10;

/// What an app's facet may spend (docs/technical-debt-ledger.md, "An app
/// facet's subrequests are the account's, for the facet's life"): more
/// than 50 capability calls in one call, of each kind, and across calls on
/// one facet; past the account's 10,000 on one facet, its next call makes
/// all it asks (a facet that ran out is started again); and CPU each
/// call's, the runaway guard (`limits::APP_CPU_MS`: five calls of two
/// fifths of it on one facet answer, one past it is stopped). Run by name.
/// Only Cloudflare enforces these limits (a local node makes every call
/// asked, and checks no CPU), so it is a hosted lane's proof.
pub fn capability_cost(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name("capability-cost", &[], "it proves what an app's facet may spend, which only Cloudflare enforces") {
        return Ok(());
    }
    let owner = api.person()?;
    let fresh = |s: &Suite, label: &str| -> Result<String> {
        let name = s.named(api, &owner, label)?;
        let c = s.create(api, &owner, &name)?;
        s.commit(&c, &[("app.mjs", Some(BURN_APP)), ("fragment.json", Some(BURN_JSON)), ("note.md", Some(b"# Note\n\na line\n"))]);
        s.deploy(&c);
        Ok(name)
    };
    let burn = |name: &str, op: &str, n: u64, par: u64, id: &str| -> Result<Value> {
        let t0 = Instant::now();
        let r = api.op(&owner, name, "burn", id, json!({ "op": op, "n": n, "par": par }))?;
        let error = if r.status == 200 { Value::Null } else { json!(r.message()) };
        Ok(json!({ "op": op, "status": r.status, "asked": n, "made": r.body["result"]["made"], "error": error, "ms": t0.elapsed().as_millis() as u64 }))
    };
    let made = |row: &Value| row["status"] == 200 && row["made"] == row["asked"];
    // each kind, past the old 50 in one call, on a facet of its own
    let mut kinds = vec![];
    for op in ["files.list", "files.stat", "files.read", "search.status"] {
        let name = fresh(s, &format!("burn-{}", op.replace('.', "-")))?;
        kinds.push(burn(&name, op, ASKED, 1, "burn")?);
    }
    // past it across calls on one facet
    let name = fresh(s, "burn-shared")?;
    let across = [burn(&name, "files.list", 60, 1, "first")?, burn(&name, "files.list", 60, 1, "second")?];
    println!("      in one call: {}; across two calls on one facet: {} and {}", kinds.iter().map(|k| format!("{} {}", k["op"], k["made"])).collect::<Vec<_>>().join(", "), across[0]["made"], across[1]["made"]);
    s.ok(
        "an app makes more than 50 capability calls in one call, of each kind, and across calls on one facet",
        kinds.iter().all(made) && across.iter().all(made),
        json!({ "kinds": kinds, "across": across }),
    );
    // past the account's limit on one facet (a call refused there fails,
    // and the facet is stopped), then its next call
    let name = fresh(s, "burn-far")?;
    let mut far = vec![];
    for i in 0..FAR_CALLS {
        let row = burn(&name, "files.list", FAR / FAR_CALLS, 10, &format!("far{i}"))?;
        let stop = row["status"] != 200;
        far.push(row);
        if stop {
            break;
        }
    }
    let next = burn(&name, "files.list", ASKED, 1, "next")?;
    let refused = far.iter().find(|r| r["status"] != 200);
    println!(
        "      {FAR} calls in {FAR_CALLS} on one facet: {} ms; {}; its next call made {}",
        far.iter().map(|r| r["ms"].as_u64().unwrap_or(0)).sum::<u64>(),
        refused.map_or("none refused".into(), |r| format!("refused: {}", r["error"])),
        next["made"]
    );
    s.ok(
        &format!("past {FAR} capability calls on one facet, its next call makes all it asks (a facet that ran out is started again)"),
        made(&next) && refused.is_none_or(|r| r["error"].as_str().is_some_and(|e| e.contains("Too many subrequests"))),
        json!({ "far": far, "next": next }),
    );
    let mut report = json!({ "deployment": api.base, "kinds": kinds, "across": across, "far": far, "next": next });
    // CPU, each call's: Cloudflare's alone. A Worker's clock stands still
    // through computation, so the client times it: the turns a millisecond
    // of CPU takes (its wall time, less a call's floor), then five calls of
    // two fifths of the limit on a fresh facet (the third refused were the
    // limit the facet's), and one call past it on another
    if s.hosted() {
        let cpu_ms = fragment_proto::limits::APP_CPU_MS as f64;
        let spin = |name: &str, iters: u64, id: &str| -> Result<Value> {
            let t0 = Instant::now();
            let r = api.op(&owner, name, "spin", id, json!({ "iters": iters }))?;
            Ok(json!({ "iters": iters, "status": r.status, "ms": t0.elapsed().as_millis() as u64, "error": if r.status == 200 { Value::Null } else { json!(r.message()) } }))
        };
        let calibrate = fresh(s, "spin-calibrate")?;
        spin(&calibrate, 1_000_000, "warm")?;
        let floor = spin(&calibrate, 1, "floor")?["ms"].as_f64().unwrap_or(0.0);
        let probe = 200_000_000u64;
        let took = spin(&calibrate, probe, "calibrate")?;
        let per_ms = probe as f64 / (took["ms"].as_f64().unwrap_or(1.0) - floor).max(1.0);
        let at = |ms: f64| ((per_ms * ms) as u64).clamp(1, 1 << 52);
        let life = fresh(s, "spin-life")?;
        let parts: Vec<Value> = (0..5).map(|i| spin(&life, at(cpu_ms * 0.4), &format!("part{i}"))).collect::<Result<_>>()?;
        let guard = fresh(s, "spin-guard")?;
        let past = spin(&guard, at(cpu_ms * 1.5), "past")?;
        println!("      CPU: five calls of ~{:.0} s on one facet: {}; one of ~{:.0} s: {} in {} ms", cpu_ms * 0.4 / 1000.0, parts.iter().map(|p| format!("{} in {} ms", p["status"], p["ms"])).collect::<Vec<_>>().join(", "), cpu_ms * 1.5 / 1000.0, past["status"], past["ms"]);
        s.ok(
            "CPU is each call's: five calls of two fifths of the limit on one facet answer, and one past it is stopped",
            parts.iter().all(|p| p["status"] == 200) && past["status"] != 200,
            json!({ "turns_per_ms": per_ms, "floor_ms": floor, "parts": parts, "past": past }),
        );
        report["cpu"] = json!({ "parts": parts, "past": past });
    } else {
        s.skip("CPU is each call's", "only Cloudflare checks an app's CPU");
    }
    let out = s.dir("capability-cost");
    std::fs::write(out.join("cost.json"), serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}

/// A facet that meets a runtime limit is started again
/// (docs/technical-debt-ledger.md, "An app facet's subrequests are the
/// account's"). The `facet-calls` lever gives each start of the facet 5
/// capability calls, refused past them in the runtime's words, as
/// Cloudflare refuses a facet's subrequests past its life's limit. A query
/// that runs out fails, and the next call makes all it asks; so does a job
/// that runs out, whose throw the platform code answers as its run's failure.
pub fn facet_restart(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("facet-restart", &[crate::Need::Levers]) {
        return Ok(());
    }
    let wait = std::time::Duration::from_secs(40);
    let owner = api.person()?;
    let name = s.named(api, &owner, "restart")?;
    let c = s.create(api, &owner, &name)?;
    s.commit(&c, &[("app.mjs", Some(BURN_APP)), ("fragment.json", Some(BURN_JSON)), ("note.md", Some(b"# Note\n\na line\n"))]);
    s.deploy(&c);
    let lever = |calls: Value| api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": name, "op": "facet-calls", "calls": calls })));
    let burn = |n: u64, id: &str| api.op(&owner, &name, "burn", id, json!({ "op": "files.list", "n": n }));
    let made = |r: &crate::api::Reply, n: u64| r.status == 200 && r.body["result"]["made"] == n;
    let set = lever(json!(5))?;
    anyhow::ensure!(set.status == 200, "the facet-calls lever: {set}");
    let out = burn(10, "query-out")?;
    let next = burn(5, "query-next")?;
    s.ok(
        "a query that runs out of its facet's calls fails in the runtime's words, and the next call makes all it asks: the facet started again",
        out.status != 200 && out.message().contains("Too many subrequests") && made(&next, 5),
        format!("{out} | {next}"),
    );
    let started = api.op(&owner, &name, "burn_job", "job-out", json!({ "op": "files.list", "n": 10 }))?;
    let run = jobs::settle(api, &owner, &name, jobs::started(&started), &["succeeded", "held", "failed"], wait);
    let next = burn(5, "job-next")?;
    s.ok(
        "a job that runs out fails its run in the runtime's words (the platform code answers its throw), and the next call makes all it asks: the facet started again",
        run["status"] != "succeeded" && run["error"].as_str().is_some_and(|e| e.contains("Too many subrequests")) && made(&next, 5),
        json!({ "run": run, "next": next.body }),
    );
    lever(Value::Null)?;
    Ok(())
}
