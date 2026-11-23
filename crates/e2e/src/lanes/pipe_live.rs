//! Each operator vendor's own number, read live (FIN-616): on a branch
//! preview, a real Hermes agent calls each of its shared-key tools once
//! (Grok's X search, ElevenLabs' speech, Firecrawl's page read, its web
//! search, fal's image), and its owner's ledger has each call's row in its
//! vendor's own unit (`catalog::Charge`), charged above nothing: proof that
//! each answer names its number where `price::DEFAULT_CHARGES` reads it.
//! The hosted lane runs it by name (`cargo xtask e2e --hosted … --only
//! pipe-live`); a local run never has a real agent (needs.rs `RealAgent`):
//! the computers lane proves each row against the vendors' fakes.
//!
//! Each tool is one turn, asked by name. Which tool the model calls is its
//! choice, so a check says so; every row the run made is printed for the
//! evidence, with what it was charged. Then the computer sleeps (`--sweep`
//! deletes the run's `e2e-<run>-` fragments).
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::json;

use super::computers::{phase, turn_of, work_of};
use super::jobs::records;
use super::ledger::{charged, entries, usage};
use crate::api::Api;
use crate::{Need, Suite};

pub const SECTION: &str = "pipe-live";
/// The paid calls the run lends the section's person: five turns, each a
/// model call or two around its tool's, and its hello. Each tool's own call
/// is a vendor's, paid by the platform's key and charged to the person.
const PAID_CALLS: u64 = 20;
/// A new computer's first start on a preview, to its agent following.
const FIRST_START: Duration = Duration::from_secs(10 * 60);
/// A turn that calls a tool.
const TURN: Duration = Duration::from_secs(5 * 60);
const POLL: Duration = Duration::from_secs(3);

/// Each tool asked of the agent, and the unit its call's row is in: none
/// for fal's queued image, whose answer names no units yet (the debt
/// ledger), printed for the evidence alone.
const TOOLS: [(&str, &str, Option<&str>); 5] = [
    ("x-search", "Use your x_search tool once to find two recent posts about Cloudflare Workers, then answer in one sentence.", Some("xai:tick")),
    ("speech", "Use your text_to_speech tool once to say exactly: Hello from the pipe. Then answer done.", Some("elevenlabs:character")),
    ("extract", "Use your web_extract tool once on https://example.com and tell me its title in one sentence.", Some("firecrawl:credit")),
    ("search", "Use your web_search tool once to look up Cloudflare Durable Objects, then answer in one sentence.", Some("search")),
    ("image", "Use your image_generate tool once to make a small plain blue square, then answer done.", None),
];

/// Asks `f` every `POLL` until it answers, or `bound` passes.
fn within<T>(bound: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if t0.elapsed() >= bound {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// A key row as the person's ledger holds it: `(key, units, charge)`.
type KeyRow = (String, u64, i64);

/// The key rows the person's ledger holds.
fn rows(api: &Api, identity: &str) -> Vec<KeyRow> {
    entries(api, identity, "key:").iter().map(|e| (usage(e)["key"].as_str().unwrap_or("").to_string(), usage(e)["units"].as_u64().unwrap_or(0), charged(e).unwrap_or(0))).collect()
}

pub fn pipe_live(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it runs a real agent on a Hermes computer for about ten minutes, calls five vendors on the platform's keys, and spends up to {PAID_CALLS} of the run's paid calls");
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Computers, Need::Models, Need::RealAgent], &why) {
        return Ok(());
    }
    anyhow::ensure!(api.signs_in_by_levers(), "a real agent's person signs in through a preview's levers");
    let keys = Keys::generate();
    let (session, owner_id) = api.e2e_sign_in(&Api::email_of(&keys), PAID_CALLS)?;
    let me = api.approve(&session, &keys)?;
    let email = me.body["email"].as_str().context("the person has an email")?.to_string();
    println!("      ({email} as {owner_id}, lent {PAID_CALLS} paid calls)");
    let made = api.signed(&keys, "POST", "/api/computers", Some(&json!({})))?;
    let id = made.body["computer"].as_str().unwrap_or("").to_string();
    let (agent, identity) = super::agent_ask::agent(api, &keys, &id, &s.name("pipe"), "Pip", &format!("You are Pip, {email}'s agent. When asked to use a tool, you use exactly that tool once, then answer briefly.\n"), "cheap")?;
    let chat = api.create_with(&keys, json!({ "label": s.name("pipe-chat"), "template": "chat", "title": "Pip" }))?;
    let chat_name = chat.body["name"].as_str().context("its chat's name")?.to_string();
    let joined = api.signed(&keys, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let following = || {
        api.signed(&keys, "GET", &format!("/api/f/{chat_name}/subscriptions"), None)
            .ok()
            .is_some_and(|r| r.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["wake"] == true && x["principal"] == identity.as_str() && x["channel"] == "chat")))
    };
    let ready = made.status == 200 && chat.status == 200 && joined.status == 200 && within(FIRST_START, || (phase(api, &keys, &id) == "awake" && following()).then_some(())).is_some();
    s.ok("a real agent of one person, on its computer, following its chat", ready, json!({ "computer": made.body, "agent": agent }));
    if !ready {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
        return Ok(());
    }
    let mut made_rows: Vec<(&str, Vec<KeyRow>)> = vec![];
    for (n, (name, ask, _)) in TOOLS.iter().enumerate() {
        let before = rows(api, &owner_id);
        let r = api.signed(&keys, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": format!("pipe-{n}"), "body": { "text": ask } })))?;
        let seq = r.body["record"]["seq"].as_i64().unwrap_or(0);
        let turn = turn_of(&agent, &chat_name, "chat", seq);
        let ended = within(TURN, || work_of(&records(api, &keys, &chat_name, "work"), &turn).into_iter().find(|w| w["body"]["kind"] == "turn.end"));
        // its charge goes on after its answer: a moment for the queue; the
        // rows new since (each once, a row like an earlier one too)
        let new = within(Duration::from_secs(30), || {
            let mut now = rows(api, &owner_id);
            (now.len() > before.len()).then(|| {
                for r in &before {
                    if let Some(i) = now.iter().position(|n| n == r) {
                        now.swap_remove(i);
                    }
                }
                now
            })
        })
        .unwrap_or_default();
        println!("      ({name}: {} new key rows {new:?}; turn ended {})", new.len(), ended.is_some());
        made_rows.push((name, new));
    }
    let of = |name: &str| made_rows.iter().find(|(n, _)| *n == name).map(|(_, r)| r.clone()).unwrap_or_default();
    for (name, unit) in TOOLS.iter().filter_map(|(name, _, unit)| Some((*name, (*unit)?))) {
        // a web search is Perplexity's call or Firecrawl's credits, as Hermes' provider is
        let units: &[&str] = if unit == "search" { &["perplexity:call", "firecrawl:credit"] } else { std::slice::from_ref(&unit) };
        let found = of(name).into_iter().find(|(key, n, charge)| units.contains(&key.as_str()) && *n > 0 && *charge > 0);
        s.ok(
            &format!("(a model's choice) {name}: its call is charged its vendor's own number, in {unit}"),
            found.is_some(),
            json!({ "rows": of(name) }),
        );
    }
    let all = rows(api, &owner_id);
    let total: i64 = all.iter().map(|(_, _, c)| c).sum();
    println!("      (every key row: {all:?}; {} charged in all)", fragment_core::price::dollars(total));
    let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
    Ok(())
}
