//! BENCH (selfhost-perf-bench, not for merge): the same operations timed on
//! whatever this run is (wrangler or celld; local Docker, two nodes, or the
//! real engine), printed as a table. Run by name: `--only perf`.
//! `FRAGMENT_PERF_N` reps (default 20) for the short operations, and
//! `FRAGMENT_PERF_CYCLES` (default 3) sleep/wake/restart cycles.

use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};

use super::computers::{lever_with, phase, AGENT_JSON, CHAT_JSON as AGENT_CHAT_JSON};
use crate::api::{Api, Socket};
use crate::{Need, Suite};

fn ms_since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// One row: median, p90, min, max of `ms`.
fn row(rows: &mut Vec<String>, label: &str, ms: &[f64]) {
    let mut v: Vec<f64> = ms.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        rows.push(format!("{label:<44} (none)"));
        return;
    }
    v.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let at = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    rows.push(format!("{label:<44} n={:<3} median {:>8.1} p90 {:>8.1} min {:>8.1} max {:>8.1}", v.len(), at(0.5), at(0.9), v[0], v[v.len() - 1]));
}

fn env_n(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn perf(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section_by_name("perf", &[Need::Computers, Need::Levers], "a benchmark, not a check") {
        return Ok(());
    }
    let n = env_n("FRAGMENT_PERF_N", 20);
    let cycles = env_n("FRAGMENT_PERF_CYCLES", 3);
    let mut rows = vec![];
    let owner = api.person()?;

    // the platform alone: a fragment op, a read, a live update, a computer's view
    let (name, _) = super::author::chat(s, api, &owner, "perf")?;
    for i in 0..3 {
        api.op(&owner, &name, "say", &format!("w{i}"), json!({ "text": "warm" }))?;
    }
    let mut ops = vec![];
    for i in 0..n {
        let t0 = Instant::now();
        let r = api.op(&owner, &name, "say", &format!("o{i}"), json!({ "text": "op" }))?;
        ops.push(ms_since(t0));
        anyhow::ensure!(r.status == 200, "op: {r}");
    }
    row(&mut rows, "platform: fragment op (POST ops/say)", &ops);
    let mut reads = vec![];
    for _ in 0..n {
        let t0 = Instant::now();
        let r = api.signed(&owner, "GET", &format!("/api/f/{name}/channels/room?after=0"), None)?;
        reads.push(ms_since(t0));
        anyhow::ensure!(r.status == 200, "read: {r}");
    }
    row(&mut rows, "platform: channel read (GET channels/room)", &reads);
    let mut page = Socket::open(api, &name, "__live", Some(&owner), None)?;
    page.until("hello", 5)?;
    page.send(&json!({ "type": "subscribe", "channel": "room", "after": 1_000_000 }))?;
    page.until("subscribed", 2000)?;
    let mut live = vec![];
    let mut live_after_answer = vec![];
    for i in 0..n {
        let id = format!("l{i}");
        let t0 = Instant::now();
        api.op(&owner, &name, "say", &id, json!({ "text": id }))?;
        let answered = ms_since(t0);
        page.until_where("record", 50, |r| r["body"]["text"] == id.as_str())?;
        let heard = ms_since(t0);
        live.push(heard);
        live_after_answer.push(heard - answered);
    }
    page.close();
    row(&mut rows, "platform: live update (op to its frame)", &live);
    row(&mut rows, "platform: live frame after the op's answer", &live_after_answer);

    // a computer: cold start, exec, intercepted calls, sleep, warm wake, restart
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let id = r.body["computer"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200, "made: {r}");
    let mut views = vec![];
    for _ in 0..n {
        let t0 = Instant::now();
        api.signed(&owner, "GET", &format!("/api/computers/{id}"), None)?;
        views.push(ms_since(t0));
    }
    row(&mut rows, "platform: computer's view (GET, asleep)", &views);
    // an agent on it, for the guest's model calls
    let agent_name = s.named(api, &owner, "perfagent")?;
    let agent = s.create(api, &owner, &agent_name)?;
    s.commit(&agent, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&agent);
    let _ = AGENT_CHAT_JSON;
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{id}/agents/{agent_name}"), Some(&json!({})))?;
    anyhow::ensure!(r.status == 200, "assigned: {r}");
    let patient = api.patient(Duration::from_secs(300));
    let wake = |api: &Api| -> Result<(f64, Value)> {
        let t0 = Instant::now();
        let r = api.signed(&owner, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})))?;
        Ok((ms_since(t0), r.body))
    };
    let (cold, v) = wake(&patient)?;
    println!("      cold wake answered {} on node {:?} in {cold:.0} ms", v["phase"], v["node"]);
    s.eventually(Duration::from_secs(120), || phase(api, &owner, &id) == "awake");
    rows.push(format!("{:<44} {cold:.1} ms (answered {})", "computer: cold start (first wake, POST)", v["phase"]));

    let bench = |script: &str, times: usize| -> Result<Value> {
        let r = lever_with(&patient, &id, json!({ "op": "bench-exec", "times": times, "script": script }))?;
        anyhow::ensure!(r.status == 200, "bench-exec {script}: {r}");
        Ok(r.body)
    };
    let ms_of = |v: &Value| -> Vec<f64> { v["ms"].as_array().map(|l| l.iter().filter_map(Value::as_f64).collect()).unwrap_or_default() };
    bench("true", 3)?;
    let exec = ms_of(&bench("true", n)?);
    row(&mut rows, "computer: exec round trip (in the DO)", &exec);
    let t0 = Instant::now();
    bench("true", 1)?;
    rows.push(format!("{:<44} {:.1} ms", "computer: one exec through the lever (outside)", ms_since(t0)));
    // the guest's intercepted calls: a loop of `k` in one exec, less the empty loop
    let k = 20;
    let looped = |cmd: &str| format!("i=0; while [ $i -lt {k} ]; do {cmd} || exit 9; i=$((i+1)); done");
    let empty = ms_of(&bench(&looped("true"), 5)?);
    let base = {
        let mut e = empty.clone();
        e.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        e[e.len() / 2]
    };
    let per = |v: &Value| -> Vec<f64> { ms_of(v).iter().map(|t| (t - base) / k as f64).collect() };
    let api_get = bench(&looped("wget -q -O /dev/null http://api.fragment.internal/api/computer"), 5)?;
    anyhow::ensure!(api_get["last"]["exitCode"] == 0, "the guest's API call: {api_get}");
    row(&mut rows, "computer: guest -> platform API (GET /api/computer)", &per(&api_get));
    let models = bench(&looped("wget -q -O /dev/null http://model.fragment.internal/v1/models"), 5)?;
    anyhow::ensure!(models["last"]["exitCode"] == 0, "the guest's models list: {models}");
    row(&mut rows, "computer: guest -> model intercept (GET /v1/models)", &per(&models));
    let call = format!(
        "wget -q -O /dev/null --header 'x-fragment-agent: {agent_name}' --header 'content-type: application/json' --post-data '{{\"model\":\"cheap\",\"messages\":[{{\"role\":\"user\",\"content\":\"hi\"}}]}}' http://model.fragment.internal/v1/chat/completions"
    );
    let chat = bench(&looped(&call), 3)?;
    if chat["last"]["exitCode"] == 0 {
        row(&mut rows, "computer: guest -> model call (POST completions)", &per(&chat));
    } else {
        rows.push(format!("computer: guest -> model call failed: {}", chat["last"]));
    }
    let mut awake_views = vec![];
    for _ in 0..n {
        let t0 = Instant::now();
        api.signed(&owner, "GET", &format!("/api/computers/{id}"), None)?;
        awake_views.push(ms_since(t0));
    }
    row(&mut rows, "platform: computer's view (GET, awake)", &awake_views);

    // sleep (hold, save, snapshot, stop), warm wake (from the snapshot), restart (image + restore)
    let (mut sleeps, mut warm, mut restarts) = (vec![], vec![], vec![]);
    for c in 0..cycles {
        // something new in /data for the save to carry
        bench(&format!("mkdir -p /data/perf && head -c 1048576 /dev/urandom > /data/perf/f{c}"), 1)?;
        let t0 = Instant::now();
        let r = patient.signed(&owner, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})))?;
        let answered = r.body["phase"].clone();
        s.eventually(Duration::from_secs(120), || phase(api, &owner, &id) == "asleep");
        sleeps.push(ms_since(t0));
        println!("      sleep {c}: answered {answered}, asleep in {:.0} ms", sleeps[c]);
        let (w, v) = wake(&patient)?;
        s.eventually(Duration::from_secs(120), || phase(api, &owner, &id) == "awake");
        println!("      warm wake {c}: answered {} in {w:.0} ms, restored {}", v["phase"], v["restored"]["from"]);
        warm.push(w);
        let g = v["generation"].as_u64().unwrap_or(0);
        let t0 = Instant::now();
        let r = patient.signed(&owner, "POST", &format!("/api/computers/{id}/restart"), Some(&json!({ "generation": g })))?;
        let answered = r.body["phase"].clone();
        s.eventually(Duration::from_secs(120), || phase(api, &owner, &id) == "awake");
        restarts.push(ms_since(t0));
        println!("      restart {c}: answered {answered} in {:.0} ms (awake by {:.0} ms), restored {}", 0.0, restarts[c], r.body["restored"]["from"]);
    }
    row(&mut rows, "computer: sleep, to asleep (POST sleep)", &sleeps);
    row(&mut rows, "computer: warm wake (from its snapshot)", &warm);
    row(&mut rows, "computer: restart, to awake (save, stop, start)", &restarts);
    let r = patient.signed(&owner, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})))?;
    let _ = r;

    println!("\n      perf ({}; {} reps, {} cycles):", if s.has_nodes() { "nodes" } else { "this runtime's containers" }, n, cycles);
    for r in &rows {
        println!("      {r}");
    }
    s.ok("the benchmark ran", true, "");
    Ok(())
}
