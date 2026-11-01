//! A tenant's reach: an app's database stops at 16 MiB and the rest of the
//! fragment keeps working, and an app cannot run code from strings, block
//! its thread, or use the parts of its Durable Object the platform takes
//! away. Its CPU and memory limits are the runtime's, which local workerd
//! does not enforce (spike S1): the hosted lane checks those.

use anyhow::Result;
use serde_json::{json, Value};

use super::app::ship;
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

/// An app that makes `n` calls of one capability in one call into it,
/// until the runtime refuses one: how many it made, and the refusal.
const BURN_APP: &[u8] = br#"import { DurableObject } from "cloudflare:workers";
export class App extends DurableObject {
  async burn({ op, n }) {
    const call = {
      "files.list": () => this.files.list("none/"),
      "files.read": () => this.files.readBytes("note.md"),
      "files.stat": () => this.files.stat("note.md"),
      "search.status": () => this.searchIndex.status(),
    }[op];
    let made = 0;
    try {
      while (made < n) { await call(); made++; }
      return { made, error: null };
    } catch (e) {
      return { made, error: String(e?.message ?? e) };
    }
  }
}
"#;
const BURN_JSON: &[u8] = br#"{ "name": "burn", "operations": { "burn": { "kind": "query", "role": "viewer" } } }"#;

/// What one capability call costs an app's facet, in subrequests
/// (`limits::APP_SUBREQUESTS`): each kind on a fragment of its own, made
/// in one call until the runtime refuses one; and whether the budget is
/// each call's or the running facet's (two calls of 30 in a row on a fresh
/// one: the second makes 20 when the 50 are shared). Run by name; it
/// measures and gates nothing. Only Cloudflare enforces the limit (a local
/// node makes every call asked), so it is a hosted lane's measure.
pub fn capability_cost(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name("capability-cost", &[], "it measures what a capability call costs an app's facet in subrequests, which only Cloudflare enforces") {
        return Ok(());
    }
    let owner = api.person()?;
    let budget = fragment_proto::limits::APP_SUBREQUESTS as u64;
    let fresh = |label: &str| -> Result<String> {
        let name = s.named(api, &owner, label)?;
        let c = s.create(api, &owner, &name)?;
        s.commit(&c, &[("app.mjs", Some(BURN_APP)), ("fragment.json", Some(BURN_JSON)), ("note.md", Some(b"# Note\n\na line\n"))]);
        s.deploy(&c);
        Ok(name)
    };
    let burn = |name: &str, op: &str, n: u64, id: &str| -> Result<Value> {
        let r = api.op(&owner, name, "burn", id, json!({ "op": op, "n": n }))?;
        Ok(json!({ "op": op, "status": r.status, "asked": n, "made": r.body["result"]["made"], "error": r.body["result"]["error"] }))
    };
    let mut rows = vec![];
    for op in ["files.list", "files.stat", "files.read", "search.status"] {
        let name = fresh(&format!("burn-{}", op.replace('.', "-")))?;
        let mut row = burn(&name, op, budget * 2, "burn")?;
        // refused after `made`: each call costs between budget / (made + 1) and budget / made
        let cost = row["made"].as_u64().filter(|m| *m > 0 && *m < budget * 2).map(|m| format!("{:.2}..{:.2}", budget as f64 / (m + 1) as f64, budget as f64 / m as f64));
        println!("      {op}: {} of {} made in one call on a fresh facet, {}", row["made"], budget * 2, cost.as_ref().map_or("none refused".into(), |c| format!("so each costs {c} of its {budget} subrequests")));
        row["cost"] = json!(cost);
        rows.push(row);
    }
    // the budget: each call's, or the running facet's
    let name = fresh("burn-shared")?;
    let first = burn(&name, "files.list", 30, "first")?;
    let second = burn(&name, "files.list", 30, "second")?;
    let third = burn(&name, "files.list", 1, "third")?;
    println!("      two calls of 30 on one facet: the first made {}, the second {}, a third of 1 then {}", first["made"], second["made"], third["made"]);
    let out = s.dir("capability-cost");
    let report = json!({ "deployment": api.base, "budget": budget, "fresh": rows, "shared": [first, second, third] });
    std::fs::write(out.join("cost.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("      (cost: {})", out.join("cost.json").display());
    s.ok("each capability kind was measured on a facet of its own, and the budget's scope probed", rows.iter().all(|r| r["status"] == 200) && first["status"] == 200, &report);
    Ok(())
}
