//! A computer per agent, with real agents (docs/computers.md, "What a
//! computer is" and "The team folder in our Hermes image"): on a branch
//! preview, two Hermes agents of one person, each on a computer of its own,
//! their real model. The hosted lane runs it by name (`cargo xtask e2e
//! --hosted … --only agent-own`); a local run never has a real agent
//! (needs.rs `RealAgent`): `computers` runs the same platform paths on the
//! stub, and the images' tests the bridge's and the boot's.
//!
//! What it measures, for the evidence:
//!
//! - **Add agent, end to end**: the calls the shell's Add makes, in its
//!   order (cell/shell/onboarding.js `makeAgent`: the agent fragment and its
//!   job, its own computer, the assign, the deploy, its chat, the hello
//!   routine, the wake), timed from the first to its hello in its chat, on a
//!   computer made for it.
//! - **An ask to a sleeping teammate, a file passed through `~/shared`**:
//!   Maple's computer asleep, the person asks Juniper to put a word in
//!   `~/shared` and ask Maple what it says; timed from the person's message
//!   to Maple's answer back on Juniper's `tasks`, and when the file reached
//!   the team folder against when Maple's turn began.
//!
//! A check that depends on what the model chooses to do says so in its text.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::agent_ask::{say, within};
use super::computers::{agent_replies, phase};
use super::jobs::records;
use crate::api::Api;
use crate::{Need, Suite};

pub const SECTION: &str = "agent-own";

/// Paid calls lent the section's person: two hellos, Juniper's turn (a
/// write to ~/shared and an ask: a few tool calls), Maple's (a read and an
/// answer), and Juniper's turn of the answer; Hermes' titles among them.
const PAID_CALLS: u64 = 40;
/// A new computer's first start on a preview, to its agent's hello.
const FIRST_START: Duration = Duration::from_secs(10 * 60);
/// An ask's whole round: the asker's turn, the asked computer's wake and
/// turn, the answer's turn.
const ROUND: Duration = Duration::from_secs(12 * 60);
const SLEEP: Duration = Duration::from_secs(3 * 60);

/// An agent made as the shell's Add makes one, on a computer of its own:
/// its fragment, identity, chat and computer, and how long from the first
/// call to its hello in its chat (none: no hello within `FIRST_START`).
fn added(api: &Api, keys: &Keys, label: &str, title: &str, soul: &str) -> Result<(String, String, String, String, Option<Duration>)> {
    let t0 = Instant::now();
    let made = api.create_with(keys, json!({ "label": label, "template": "agent", "title": title }))?;
    let name = made.body["name"].as_str().context("an agent fragment's name")?.to_string();
    let files = json!({ "key": "agent-default", "message": "its job", "files": [{ "path": "SOUL.md", "text": soul }, { "path": "agent.json", "text": "{\n  \"tier\": \"cheap\"\n}\n" }] });
    let wrote = api.signed(keys, "POST", &format!("/api/f/{name}/files"), Some(&files))?;
    let computer = api.signed(keys, "POST", "/api/computers", Some(&json!({ "agent": name })))?;
    let id = computer.body["computer"].as_str().unwrap_or("").to_string();
    let assigned = api.signed(keys, "PUT", &format!("/api/computers/{id}/agents/{name}"), Some(&json!({})))?;
    let identity = assigned.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    let deployed = api.signed(keys, "POST", &format!("/api/f/{name}/deploy"), Some(&json!({})))?;
    let chat = api.create_with(keys, json!({ "label": format!("{label}-chat"), "template": "chat", "title": title }))?;
    let chat_name = chat.body["name"].as_str().context("its chat's name")?.to_string();
    let joined = api.signed(keys, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    let hello = json!({ "id": "hello", "body": { "kind": "routine", "chat": chat_name, "text": "You were just made, and this is your chat with the person you work for. Say hello in a sentence or two: who you are, and what you will do for them (your SOUL.md). Start no work until they answer. Write in the language their browser is set to (en) until they write to you in another." } });
    let routine = api.signed(keys, "POST", &format!("/api/f/{name}/channels/tasks"), Some(&hello))?;
    let _ = api.signed(keys, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    anyhow::ensure!(
        [made.status, wrote.status, computer.status, assigned.status, deployed.status, chat.status, joined.status, routine.status].iter().all(|s| *s == 200) && fragment_core::npub::is_identity(&identity),
        "{label} made as Add makes it: {made} {wrote} {computer} {assigned} {deployed} {chat} {joined} {routine}"
    );
    let said = within(FIRST_START, || (!agent_replies(&records(api, keys, &chat_name, "chat"), &identity).is_empty()).then_some(()));
    Ok((name, identity, chat_name, id, said.map(|()| t0.elapsed())))
}

pub fn agent_own(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it runs two real agents on Hermes computers of their own for about twenty minutes, and spends up to {PAID_CALLS} of the run's paid calls");
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Computers, Need::Models, Need::RealAgent], &why) {
        return Ok(());
    }
    anyhow::ensure!(api.signs_in_by_levers(), "a real agent's person signs in through a preview's levers");
    let keys = Keys::generate();
    let (session, owner_id) = api.e2e_sign_in(&Api::email_of(&keys), PAID_CALLS)?;
    let me = api.approve(&session, &keys)?;
    let email = me.body["email"].as_str().context("the person has an email")?.to_string();
    println!("      ({email} as {owner_id}, lent {PAID_CALLS} paid calls)");
    // their skills and their team folder, as the shell makes them beside an agent
    let skills = api.create_with(&keys, json!({ "label": s.name("skills"), "template": "skills" }))?;
    let shared = api.create_with(&keys, json!({ "label": s.name("shared"), "template": "shared" }))?;
    let folder = shared.body["name"].as_str().unwrap_or("").to_string();
    s.ok("their skills and their team folder are made, as the shell makes them", skills.status == 200 && shared.status == 200, json!({ "skills": skills.status, "shared": shared.body }));

    let (juniper, juniper_id, jchat, jcomputer, jsaid) = added(api, &keys, &s.name("juniper"), "Juniper", &format!("You are Juniper, {email}'s agent. You talk with {email} and hand work to your teammates.\n"))?;
    if let Some(took) = jsaid {
        println!("      (Add agent, from its first call to its hello on its own computer: {took:.1?})");
    }
    s.ok("Add agent: made on a computer of its own, it says hello there", jsaid.is_some(), json!({ "juniper": juniper, "computer": jcomputer, "took_ms": jsaid.map(|d| d.as_millis()) }));
    let (maple, maple_id, mchat, mcomputer, msaid) = added(api, &keys, &s.name("maple"), "Maple", &format!("You are Maple, {email}'s researcher. When a teammate asks you about a file, read it and answer with what it says.\n"))?;
    if let Some(took) = msaid {
        println!("      (a second agent, Add to hello on its own computer: {took:.1?})");
    }
    let two = jcomputer != mcomputer && msaid.is_some();
    s.ok("a second agent: its own computer, another than the first's, and its hello there", two, json!({ "maple": maple, "computer": mcomputer, "took_ms": msaid.map(|d| d.as_millis()) }));
    if jsaid.is_none() || !two {
        for id in [&jcomputer, &mcomputer] {
            let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
        }
        return Ok(());
    }

    // Maple's computer asleep; Juniper's awake
    let asleep = within(SLEEP, || {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{mcomputer}/sleep"), Some(&json!({})));
        (phase(api, &keys, &mcomputer) == "asleep").then_some(())
    })
    .is_some();
    s.ok("the teammate's computer sleeps", asleep, phase(api, &keys, &mcomputer));
    let word = format!("quince{}", crate::api::now_s() % 10_000);
    let file = "handover.txt";
    let ask = format!("Please write the word {word} into a new file ~/shared/{file}, then ask Maple with fragment_ask what word is in ~/shared/{file}, and tell me what she answers.");
    let t0 = Instant::now();
    let (_, asked_seq) = say(api, &keys, &jchat, &juniper, "own-1", &ask)?;
    let _ = asked_seq;
    // when the file reached the team folder, as its listing says
    let mut landed: Option<Duration> = None;
    let back = within(ROUND, || {
        if landed.is_none() {
            let files = api.signed(&keys, "GET", &format!("/api/f/{folder}/files"), None).ok().map(|r| r.body).unwrap_or(Value::Null);
            if files["files"].as_array().is_some_and(|l| l.iter().any(|f| f["path"] == file)) {
                landed = Some(t0.elapsed());
            }
        }
        let answers: Vec<Value> = records(api, &keys, &juniper, "tasks").into_iter().filter(|r| r["body"]["kind"] == "answer" && r["principal"] == maple_id.as_str()).collect();
        answers.into_iter().next().map(|a| (a, t0.elapsed()))
    });
    let (answer, answered_in) = match back {
        Some((a, d)) => (a, Some(d)),
        None => (Value::Null, None),
    };
    // Maple's turn of the question, in the chat it was asked in
    let pair = answer["body"]["asked"]["fragment"].as_str().unwrap_or("").to_string();
    let started = if pair.is_empty() { None } else { records(api, &keys, &pair, "work").into_iter().find(|r| r["principal"] == maple_id.as_str() && r["body"]["kind"] == "turn.start") };
    let began_ms = started.as_ref().and_then(|r| r["at"].as_i64());
    let asked_at = records(api, &keys, &jchat, "chat").into_iter().find(|r| r["body"]["text"] == ask.as_str()).and_then(|r| r["at"].as_i64());
    let read_word = answer["body"]["text"].as_str().is_some_and(|t| t.contains(&word));
    println!(
        "      (an ask to a sleeping teammate: answered in {:?}; the file in the team folder after {:?}; Maple's turn began {:?} ms after the message)",
        answered_in,
        landed,
        began_ms.zip(asked_at).map(|(b, a)| b - a)
    );
    s.ok(
        "an ask to a teammate asleep on its own computer: it wakes, answers, and its answer, signed by it, comes back to the asker's tasks",
        answered_in.is_some() && answer["body"]["by"] == maple_id.as_str() && phase(api, &keys, &mcomputer) != "asleep",
        json!({ "answer": answer, "answered_ms": answered_in.map(|d| d.as_millis()) }),
    );
    s.ok(
        "the file the asker put in ~/shared reached the team folder, and the teammate read the word in it (the model's to do)",
        landed.is_some() && read_word,
        json!({ "landed_ms": landed.map(|d| d.as_millis()), "began_ms_after_message": began_ms.zip(asked_at).map(|(b, a)| b - a), "said": answer["body"]["text"] }),
    );
    let told = within(ROUND, || agent_replies(&records(api, &keys, &jchat, "chat"), &juniper_id).iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.contains(&word))).then_some(()));
    s.ok("and the asker tells the person the word, in its own chat (the model's to do)", told.is_some(), json!(agent_replies(&records(api, &keys, &jchat, "chat"), &juniper_id).iter().map(|r| &r["body"]["text"]).collect::<Vec<_>>()));
    let _ = mchat;
    for id in [&jcomputer, &mcomputer] {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
    }
    Ok(())
}
