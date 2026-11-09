//! Real agents asking each other (docs/cloudflare-v1.md, decision 8;
//! docs/computers.md, "Teammates in our Hermes image"; docs/chat-records.md,
//! "An ask"): on a branch preview, a Hermes computer running three agents
//! of one person, their real model, and Chrome. The hosted lane runs it by
//! name (`cargo xtask e2e --hosted … --only agent-ask`); a local run never
//! has a real agent (needs.rs `RealAgent`): the images' Docker lane runs
//! the same paths on the scripted model (`teammates_ask_each_other`), and
//! shell-ui runs `fragment ask` as a person.
//!
//! An e2e person gets three agents, each made as the shell makes one (its
//! fragment, its SOUL, its own chat `<label>-chat`): Juniper, who talks
//! with them; Fred, who keeps a secret word in his SOUL; and Maple, who
//! builds apps. Juniper and Maple are each said hello to in their own chat.
//! Both ways one agent asks another are asked of Juniper, in turn:
//!
//! - **The CLI, waiting** (`fragment ask … --wait`): asked to get the word
//!   from Fred, Juniper's turn runs it in its terminal; the CLI, as Juniper,
//!   makes a chat of the two agents and their owner, adds Fred, and asks him
//!   there; Fred's bridge takes the question as a turn, and he answers in
//!   that chat; the CLI prints his answer, and Juniper tells the person the
//!   word, all in the one turn.
//! - **The tool, answered later** (`fragment_ask`): asked to have Maple make
//!   and publish a todo app and show it in her browser, Juniper's turn asks
//!   Maple in a chat of the two and their owner, made for it; Maple's turn
//!   there makes the app, which goes live, and opens it on her desktop; and
//!   her answer comes back to Juniper as a new turn of hers, who tells the
//!   person in her own chat, once: what either says after (a second relay,
//!   an acknowledgement) is counted as redundant.
//!
//! The agents are on the tier the shell makes one on. Juniper's and Maple's
//! chats and screens are shot for the evidence (Maple's screen checked to
//! connect). Then the computer sleeps (`--sweep` deletes the run's
//! `e2e-<run>-` fragments, the agents' chats among them).
//!
//! A check that depends on what the model chooses to do says so in its text.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::agent_smoke::{live_app, open_screen, ran};
use super::computers::{agent_replies, phase, turn_of, work_of, QUEUE_DRAIN};
use super::jobs::records;
use super::ledger::entries;
use crate::api::{now_s, Api};
use crate::{Need, Suite};

pub const SECTION: &str = "agent-ask";

/// The paid calls the run lends the section's person: two hellos; the CLI
/// ask, Juniper's turn (a model call before and after its terminal step)
/// and Fred's (a call or two); the tool's, Juniper's turn (its message, and
/// its answer once woken) and Maple's (the app made and deployed, her
/// browser opened: a dozen tool calls or so); Hermes' titles and guardian
/// among them. Measured: the CLI ask spent 5, and the rest 27 in all
/// (Maple's turn between 4 and 21); the run's default `--max-paid-calls`,
/// so the section runs by name alone.
const PAID_CALLS: u64 = crate::hosted::MAX_PAID_CALLS_DEFAULT;
/// The tier the shell makes a new agent on (cell/shell/shell.js,
/// `DEFAULT_AGENT_TIER`), so the agents answer as a person's would.
const AGENT_TIER: &str = "cheap";
/// How long the chats are watched after Juniper tells the person, for
/// what either says again (a second relay, an acknowledgement).
const SETTLE: Duration = Duration::from_secs(90);
/// A new computer's first start on a preview, to its agents following.
const FIRST_START: Duration = Duration::from_secs(10 * 60);
/// A turn that answers in words.
const REPLY: Duration = Duration::from_secs(5 * 60);
/// A turn that does things: an app made and deployed, a browser opened,
/// or another agent's turn waited on.
const WORK: Duration = Duration::from_secs(15 * 60);
/// Its owner's sleep, asked until it is asleep.
const SLEEP: Duration = Duration::from_secs(3 * 60);
const POLL: Duration = Duration::from_secs(3);

/// Asks `f` every `POLL` until it answers, or `bound` passes.
#[track_caller]
fn within<T>(bound: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let at = std::panic::Location::caller();
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if t0.elapsed() >= bound {
            println!("      (a wait ran out its {:?} at {}:{})", bound, at.file(), at.line());
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// An agent of the person's on `tier`: its fragment, made with its SOUL,
/// and assigned to the computer; its identity.
pub(super) fn agent(api: &Api, keys: &Keys, computer: &str, label: &str, title: &str, soul: &str, tier: &str) -> Result<(String, String)> {
    let made = api.create_with(keys, json!({ "label": label, "template": "agent", "title": title }))?;
    anyhow::ensure!(made.status == 200, "the agent {label}: {made}");
    let name = made.body["name"].as_str().context("a fragment's name")?.to_string();
    let assigned = api.signed(keys, "PUT", &format!("/api/computers/{computer}/agents/{name}"), Some(&json!({})))?;
    let identity = assigned.body["agents"].as_array().and_then(|a| a.iter().find(|x| x["fragment"] == name.as_str())).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string();
    anyhow::ensure!(fragment_core::npub::is_identity(&identity), "{name} assigned: {assigned}");
    let files = json!({ "key": "agent-ask", "message": "its job", "files": [{ "path": "SOUL.md", "text": soul }, { "path": "agent.json", "text": format!("{{\n  \"tier\": \"{tier}\"\n}}\n") }] });
    let wrote = api.signed(keys, "POST", &format!("/api/f/{name}/files"), Some(&files))?;
    let deployed = api.signed(keys, "POST", &format!("/api/f/{name}/deploy"), Some(&json!({})))?;
    anyhow::ensure!(wrote.status == 200 && deployed.status == 200, "{name}'s SOUL: {wrote} {deployed}");
    Ok((name, identity))
}

/// An agent of the person's, made as the shell makes one: `agent` on the
/// shell's tier, and its own chat with it in it. Its fragment, identity
/// and chat.
fn agent_with_chat(api: &Api, keys: &Keys, computer: &str, label: &str, title: &str, soul: &str) -> Result<(String, String, String)> {
    let (name, identity) = agent(api, keys, computer, label, title, soul, AGENT_TIER)?;
    let chat = api.create_with(keys, json!({ "label": format!("{label}-chat"), "template": "chat", "title": title }))?;
    let chat_name = chat.body["name"].as_str().context("its chat's name")?.to_string();
    let joined = api.signed(keys, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(chat.status == 200 && joined.status == 200, "{name}'s chat: {chat} {joined}");
    Ok((name, identity, chat_name))
}

/// What the person's paid calls were, from their ledger.
fn paid(api: &Api, identity: &str) -> usize {
    entries(api, identity, "aig:").len() + entries(api, identity, "step:").len()
}

/// The owner's message in `chat`: the turn it starts for `agent`, and its seq.
fn say(api: &Api, keys: &Keys, chat: &str, agent: &str, id: &str, text: &str) -> Result<(String, i64)> {
    let r = api.signed(keys, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": id, "body": { "text": text } })))?;
    anyhow::ensure!(r.status == 200, "the owner's message {id}: {r}");
    let seq = r.body["record"]["seq"].as_i64().context("a post answers its record's seq")?;
    Ok((turn_of(agent, chat, "chat", seq), seq))
}

/// The turn's end on `chat`'s `work`.
fn ended(api: &Api, keys: &Keys, chat: &str, turn: &str) -> Option<Value> {
    work_of(&records(api, keys, chat, "work"), turn).into_iter().map(|r| r["body"].clone()).find(|b| b["kind"] == "turn.end")
}

/// A chat the owner's list names with both agents in it (by the list's
/// `agents`), other than each one's own: its name.
fn their_chat(api: &Api, keys: &Keys, a: &str, b: &str, not: &[&str]) -> Option<String> {
    let list = api.signed(keys, "GET", "/api/fragments", None).ok()?;
    list.body["fragments"].as_array()?.iter().find_map(|f| {
        let agents = f["agents"].as_array()?;
        let both = agents.iter().any(|x| x == a) && agents.iter().any(|x| x == b);
        let name = f["name"].as_str()?;
        (f["kind"] == "chat" && !not.contains(&name) && both).then(|| name.to_string())
    })
}

pub fn agent_ask(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it runs three real agents on a Hermes computer for about half an hour, and spends up to {PAID_CALLS} of the run's paid calls");
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Computers, Need::Models, Need::RealAgent], &why) {
        return Ok(());
    }
    anyhow::ensure!(api.signs_in_by_levers(), "a real agent's person signs in through a preview's levers");
    let started_s = now_s();
    let evidence = s.dir(SECTION);
    let keys = Keys::generate();
    let (session, owner_id) = api.e2e_sign_in(&Api::email_of(&keys), PAID_CALLS)?;
    let me = api.approve(&session, &keys)?;
    let email = me.body["email"].as_str().context("the person has an email")?.to_string();
    println!("      ({email} as {owner_id}, lent {PAID_CALLS} paid calls)");
    let skills = api.create_with(&keys, json!({ "label": s.name("skills"), "template": "skills" }))?;
    let made = api.signed(&keys, "POST", "/api/computers", Some(&json!({})))?;
    let id = made.body["computer"].as_str().unwrap_or("").to_string();
    let word = format!("marmalade{}", now_s() % 1000);
    let (juniper, juniper_id, jchat) = agent_with_chat(api, &keys, &id, &s.name("juniper"), "Juniper", &format!("You are Juniper, {email}'s agent. You talk with {email} and hand building work to your teammates.\n"))?;
    let (fred, fred_id, fchat) = agent_with_chat(api, &keys, &id, &s.name("fred"), "Fred", &format!("You are Fred, {email}'s agent. You keep their secret word, which is {word}. When anyone asks you for the secret word, answer with just that word.\n"))?;
    let (maple, maple_id, mchat) = agent_with_chat(api, &keys, &id, &s.name("maple"), "Maple", &format!("You are Maple, {email}'s builder. You make and publish small web apps (fragments) with the fragment CLI, and show them in the browser on your desktop.\n"))?;
    let own = [jchat.as_str(), fchat.as_str(), mchat.as_str()];
    let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let set_up = made.status == 200 && skills.status == 200;
    s.ok("three agents of one person on one computer, each with its own chat, as the shell makes them", set_up, json!({ "computer": made.body, "juniper": juniper, "fred": fred, "maple": maple }));
    if !set_up {
        return Ok(());
    }
    let following = |chat: &str, agent: &str| {
        api.signed(&keys, "GET", &format!("/api/f/{chat}/subscriptions"), None)
            .ok()
            .is_some_and(|r| r.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["wake"] == true && x["principal"] == agent && x["channel"] == "chat")))
    };
    let t0 = Instant::now();
    let ready = within(FIRST_START, || {
        (phase(api, &keys, &id) == "awake" && following(&jchat, &juniper_id) && following(&fchat, &fred_id) && following(&mchat, &maple_id)).then_some(())
    })
    .is_some();
    println!("      (asleep to each following its chat: {:.1?})", t0.elapsed());
    // its whole view when it did not come up: `why` says what its starts met
    let view = api.signed(&keys, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    s.ok("its computer wakes, and each agent follows its own chat", ready, &view);
    if !ready {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
        return Ok(());
    }

    // each agent's first turn in its own chat
    for (chat, agent, n) in [(&mchat, &maple, "1"), (&jchat, &juniper, "2")] {
        let (hello, _) = say(api, &keys, chat, agent, &format!("ask-hello-{n}"), "hello")?;
        let end = within(REPLY, || ended(api, &keys, chat, &hello));
        s.ok(&format!("\"hello\" in {agent}'s own chat is answered (model-dependent)"), end.as_ref().is_some_and(|e| e["outcome"] == "idle"), end.unwrap_or(Value::Null));
    }

    // the CLI, waiting: Juniper asks Fred, and tells the person his answer
    // in the same turn
    let calls = paid(api, &owner_id);
    let ask = format!("Get my secret word from Fred: run `fragment ask {} \"What is the secret word?\" --wait` in your terminal, then tell me the word he answered.", fragment_proto::split_fragment_name(&fred).map_or(fred.as_str(), |(label, _)| label));
    let (word_turn, _) = say(api, &keys, &jchat, &juniper, "ask-word", &ask)?;
    let t1 = Instant::now();
    let word_end = within(WORK, || ended(api, &keys, &jchat, &word_turn));
    let took = t1.elapsed();
    let word_work: Vec<Value> = work_of(&records(api, &keys, &jchat, "work"), &word_turn).into_iter().map(|r| r["body"].clone()).collect();
    let asked = word_work.iter().any(|w| w["kind"] == "turn.step" && ran(w, |cmd| cmd.contains("fragment ask")));
    let told_word: Vec<String> = agent_replies(&records(api, &keys, &jchat, "chat"), &juniper_id).into_iter().filter(|r| r["body"]["turn"] == word_turn.as_str()).filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect();
    let fred_pair = their_chat(api, &keys, &juniper_id, &fred_id, &own);
    let (question, answer, fred_turns) = match &fred_pair {
        Some(p) => {
            let said = records(api, &keys, p, "chat");
            let question = said.iter().find(|r| r["principal"] == juniper_id.as_str() && r["body"]["to"] == json!([fred_id])).cloned();
            let answer: Vec<String> = agent_replies(&said, &fred_id).into_iter().filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect();
            let starts: Vec<Value> = records(api, &keys, p, "work").into_iter().filter(|r| r["body"]["kind"] == "turn.start" && r["body"]["agent"] == fred_id.as_str()).map(|r| r["body"].clone()).collect();
            (question, answer, starts)
        }
        None => (None, vec![], vec![]),
    };
    println!("      (Juniper's turn asking Fred: {:.1?}, {} paid calls; their chat: {})", took, paid(api, &owner_id).saturating_sub(calls), fred_pair.as_deref().unwrap_or("none"));
    let line = |t: &str| t.chars().take(200).collect::<String>().replace('\n', " ");
    let terminal: Vec<String> = word_work.iter().filter(|w| w["kind"] == "turn.step").map(|w| format!("{} {}", w["tool"].as_str().unwrap_or(""), line(w["args"].as_str().unwrap_or("")))).collect();
    println!("      (Juniper's steps: {terminal:?})");
    println!("      (asked, in their chat: {}; Fred answered: {:?})", question.as_ref().map(|q| q["body"].to_string()).unwrap_or_default(), answer.iter().map(|a| line(a)).collect::<Vec<_>>());
    println!("      (Fred's turns there: {:?}; Juniper told its person: {:?})", fred_turns.iter().map(|t| t["cause"].clone()).collect::<Vec<_>>(), told_word.iter().map(|t| line(t)).collect::<Vec<_>>());
    let word_detail = json!({ "work": word_work, "told": told_word, "pair": fred_pair, "question": question, "answer": answer, "fredTurns": fred_turns });
    s.ok("asked to, Juniper's turn runs `fragment ask` in its terminal (model-dependent)", asked && word_end.is_some(), &word_detail);
    s.ok(
        "the CLI made a chat of the two agents and their owner, asked Fred there as Juniper, naming him in `to`, and Fred took it as a turn",
        question.is_some() && fred_turns.iter().any(|t| t["asker"] == juniper_id.as_str() && t["cause"]["seq"] == question.as_ref().map(|q| q["seq"].clone()).unwrap_or(Value::Null)),
        &word_detail,
    );
    s.ok("Fred answers in that chat with the word (model-dependent)", answer.iter().any(|a| a.contains(&word)), &word_detail);
    s.ok("and Juniper tells the person the word, its answer having come back (model-dependent)", told_word.iter().any(|t| t.contains(&word)), &word_detail);

    // the tool, answered later: Juniper has Maple make and publish an app,
    // and show it on her desktop
    let calls = paid(api, &owner_id);
    let app_label = s.name("todo");
    let ask = format!(
        "Use fragment_ask to ask Maple to make me a todo app from the todo template called {app_label}, deploy it, and open it in the browser on her desktop. Then tell me what she says when she answers."
    );
    let (turn, asked_at) = say(api, &keys, &jchat, &juniper, "ask-build", &ask)?;
    let t2 = Instant::now();
    let jend = within(REPLY, || ended(api, &keys, &jchat, &turn));
    let jwork: Vec<Value> = work_of(&records(api, &keys, &jchat, "work"), &turn).into_iter().map(|r| r["body"].clone()).collect();
    // a step's tool is as Hermes' progress line names it (`fragment_ask...`)
    let messaged = jwork.iter().any(|w| w["kind"] == "turn.step" && w["tool"].as_str().is_some_and(|t| t.starts_with("fragment_ask")));
    s.ok("asked to, Juniper's turn runs fragment_ask (model-dependent)", messaged && jend.is_some(), json!({ "work": jwork }));

    // the question, in a chat of the two made for it, as Juniper's, naming
    // Maple, to come back to Juniper's chat
    let handed = within(REPLY, || {
        let pair = their_chat(api, &keys, &juniper_id, &maple_id, &own)?;
        records(api, &keys, &pair, "chat").into_iter().find(|r| r["principal"] == juniper_id.as_str() && r["body"]["to"] == json!([maple_id])).map(|r| (pair, r))
    });
    println!("      (asked to Juniper's question in their chat: {:.1?})", t2.elapsed());
    s.ok(
        "the question is in a chat of the two, posted as Juniper and naming Maple, to come back to Juniper's chat",
        handed.as_ref().is_some_and(|(_, r)| r["body"]["return_to"] == jchat.as_str()),
        handed.as_ref().map(|(_, r)| r.clone()).unwrap_or(Value::Null),
    );
    let Some((pair, handed)) = handed else {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
        return Ok(());
    };
    let mturn = turn_of(&maple, &pair, "chat", handed["seq"].as_i64().unwrap_or(0));
    let mend = within(WORK, || ended(api, &keys, &pair, &mturn));
    let mwork: Vec<Value> = work_of(&records(api, &keys, &pair, "work"), &mturn).into_iter().map(|r| r["body"].clone()).collect();
    let manswer: Vec<String> = agent_replies(&records(api, &keys, &pair, "chat"), &maple_id).into_iter().filter(|r| r["body"]["turn"] == mturn.as_str()).filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect();
    println!("      (Maple's turn: {:.1?}, steps {:?})", t2.elapsed(), mwork.iter().filter(|w| w["kind"] == "turn.step").map(|w| w["tool"].clone()).collect::<Vec<_>>());
    s.ok("Maple takes it as a turn of hers in their chat, and answers there (model-dependent)", mend.is_some() && !manswer.is_empty(), json!({ "end": mend, "answer": manswer }));
    let live = within(REPLY, || live_app(api, &keys, &app_label));
    s.ok(&format!("the app {app_label} is theirs and live (model-dependent)"), live.is_some(), live.clone().unwrap_or(Value::Null));

    // Juniper says Maple's answer in her own chat, after her turn
    let told = within(REPLY, || {
        let said: Vec<String> = agent_replies(&records(api, &keys, &jchat, "chat"), &juniper_id).into_iter().filter(|r| r["body"]["turn"] != turn.as_str() && r["seq"].as_i64().is_some_and(|q| q > asked_at)).filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect();
        said.into_iter().find(|t| t.to_lowercase().contains("maple") || t.contains(&app_label))
    });
    println!("      (asked to Juniper telling the person: {:.1?}; Maple's turn spent {} paid calls with Juniper's)", t2.elapsed(), paid(api, &owner_id).saturating_sub(calls));
    s.ok("Maple's answer comes back to Juniper, who tells the person in her own chat (model-dependent)", told.is_some(), told.clone().unwrap_or_default());

    // each says the answer once: Juniper relays it once in her chat, and
    // Maple says nothing more in theirs after answering (what they said
    // after it is counted, once both chats have been quiet a while)
    if told.is_some() {
        std::thread::sleep(SETTLE);
    }
    let after = |chat: &str, agent: &str, since: i64, not: &str| -> Vec<String> {
        agent_replies(&records(api, &keys, chat, "chat"), agent).into_iter().filter(|r| r["body"]["turn"] != not && r["seq"].as_i64().is_some_and(|q| q > since)).filter_map(|r| r["body"]["text"].as_str().map(str::to_string)).collect()
    };
    let relays = after(&jchat, &juniper_id, asked_at, &turn);
    let more = after(&pair, &maple_id, handed["seq"].as_i64().unwrap_or(0), &mturn);
    let back = mwork.iter().filter(|w| w["kind"] == "turn.step" && w["tool"].as_str().is_some_and(|t| t.starts_with("fragment_ask"))).count();
    let redundant = relays.len().saturating_sub(1) + more.len();
    println!("      (after the ask: Juniper said {} in her chat, 1 wanted; Maple said {} more in theirs, 0 wanted; Maple asked Juniper back {back} times; {redundant} redundant)", relays.len(), more.len());
    let short = |t: &String| t.chars().take(160).collect::<String>().replace('\n', " ");
    println!("      (Juniper's: {:?}; Maple's more: {:?})", relays.iter().map(short).collect::<Vec<_>>(), more.iter().map(short).collect::<Vec<_>>());
    s.ok("each says the answer once: Juniper relays it once, and Maple says nothing more (model-dependent)", relays.len() == 1 && more.is_empty(), json!({ "juniper": relays, "maple": more, "mapleAskedBack": back }));

    // both chats, and both screens, as the person sees them
    let mut shots = vec![];
    if let Some(mut b) = s.browser()? {
        // the shell, each agent's chat opened from its sidebar as a person opens it
        b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
        let shell = b.open(&format!("{}/", api.base))?;
        b.viewport(&shell, 1280, 900, false)?;
        let up = b.until(&shell, "!document.getElementById('layout').hidden && document.querySelectorAll('#chats .row').length > 1", Duration::from_secs(90));
        for (chat, title, file) in [(&jchat, "Juniper", "juniper-chat.png"), (&mchat, "Maple", "maple-chat.png")] {
            // its row, by the key the shell gives it (shell.js: `chat:<name>`)
            let row = format!("#chats .row[data-key=\"chat:{chat}\"]");
            let open = format!("(() => {{ const r = document.querySelector({row:?}); if (r) r.click(); return !!r; }})()");
            let opened = up && b.eval(&shell, &open).is_ok_and(|v| v == json!(true));
            std::thread::sleep(Duration::from_secs(10));
            let _ = b.screenshot(&shell, &evidence.join(file));
            s.ok(&format!("the shell opens {title}'s chat from its sidebar"), opened, file);
            shots.push(file.to_string());
        }
        // Maple's screen, where she opened the app, is checked; Juniper's,
        // a desktop she never used (started for its first viewer), only shot
        for (agent, file, checked) in [(&maple, "maple-screen.png", true), (&juniper, "juniper-screen.png", false)] {
            let (page, connected, loaded) = open_screen(&mut b, api, &keys, &id, agent)?;
            std::thread::sleep(Duration::from_secs(5));
            let _ = b.screenshot(&page, &evidence.join(file));
            if checked {
                s.ok(&format!("{agent}'s screen connects (its browser shows the app: the evidence's shot)"), connected, json!({ "shot": file, "loading": loaded }));
            } else {
                println!("      ({agent}'s screen connected: {connected})");
            }
            shots.push(file.to_string());
        }
    } else {
        s.skip("both chats and both screens shot", "no Chrome is installed here (set CHROME_BIN)");
    }
    let detail = json!({
        "word": word_detail,
        "build": { "juniperWork": jwork, "handed": handed, "mapleWork": mwork, "mapleAnswer": manswer, "told": told, "relays": relays, "mapleMore": more, "redundant": redundant, "live": live },
        "shots": shots,
    });
    std::fs::write(evidence.join("agent-ask.json"), serde_json::to_vec_pretty(&detail)?)?;
    let spent = paid(api, &owner_id);
    s.ok(&format!("its paid calls stay within the {PAID_CALLS} lent"), spent as u64 <= PAID_CALLS, spent);

    // asleep at the end
    std::thread::sleep(QUEUE_DRAIN);
    let slept = within(SLEEP, || {
        let r = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({}))).ok()?;
        (r.body["phase"] == "asleep").then_some(())
    });
    s.ok("its computer sleeps at the end", slept.is_some(), phase(api, &keys, &id));
    println!("      (the section took {} s; evidence in {})", now_s() - started_s, evidence.display());
    Ok(())
}
