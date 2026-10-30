//! The engine's rules, each a valid path and its invalid, replay, and
//! restart paths. Method: drive the pure engine with records and runtime
//! events at chosen times, and read the effects it asks for. The rules of
//! one turn answer each claim as this life's (`answered`); the rules across
//! lives drive the engine inside a model world (below, "across lives").

use std::collections::VecDeque;

use serde_json::{json, Value};

use super::*;
use crate::records::Step as ToolStep;

const T0: u64 = 1_000_000;

/// The life of a test's engine.
const LIFE: &str = "0123456789abcdef0123456789abcdef";

fn agent(label: &str) -> Agent {
    Agent { fragment: format!("{label}--k3x9"), identity: format!("npub1{label}"), name: label.to_string(), owner: "npub1paul".into(), credentials: vec![], key: None }
}

/// A fresh engine whose runtime can take turns.
fn engine(agents: &[Agent]) -> Engine {
    let mut e = Engine::new(State::default(), Settings { prompt_ttl_ms: 60_000, turn_idle_ms: 600_000 }, LIFE).expect("a fresh state");
    e.step(Input::Agents(agents.to_vec()), T0);
    e.recover(T0);
    e.step(Input::Runtime(Event::Connected(true)), T0);
    e
}

/// Live runtime work holds even with no chat turn. Repeated snapshots do
/// nothing, and a restore inherits none of the previous process's work.
#[test]
fn runtime_work_holds_until_it_ends_and_is_never_restored() {
    let mut e = engine(&[agent("juniper")]);
    assert_eq!(keepalive(&e.step(Input::Runtime(Event::Working(true)), T0 + 1)), Some(true));
    assert_eq!(keepalive(&e.step(Input::Runtime(Event::Working(true)), T0 + 2)), None);
    assert_eq!(keepalive(&e.step(Input::Runtime(Event::Connected(false)), T0 + 3)), None, "the gateway's chat dial can reconnect while its children run");
    let mut restored = Engine::new(e.state().clone(), Settings { prompt_ttl_ms: 60_000, turn_idle_ms: 600_000 }, LIFE).unwrap();
    restored.recover(T0 + 4);
    assert!(!restored.keepalive());
    assert_eq!(keepalive(&e.step(Input::Runtime(Event::Working(false)), T0 + 5)), Some(false));
    assert_eq!(keepalive(&e.step(Input::Runtime(Event::Working(false)), T0 + 6)), None);
}

/// A background file needs no active turn or caption. It is posted as
/// the registered agent, with its bytes carried to the driver's uploader.
#[test]
fn a_background_file_is_posted_without_a_caption() {
    let a = agent("juniper");
    let mut e = engine(std::slice::from_ref(&a));
    let file = LocalFile { path: "/data/result.txt".into(), media_type: "text/plain".into(), name: "result.txt".into(), size: 27 };
    let event = |agent: &str, files: Vec<LocalFile>| Event::Say { agent: agent.into(), fragment: "talk--k3x9".into(), text: String::new(), files };
    assert!(posts(&ev(&mut e, event("unknown--k3x9", vec![file.clone()]), T0)).is_empty());
    assert!(posts(&ev(&mut e, event(&a.fragment, Vec::new()), T0)).is_empty());
    let s = ev(&mut e, event(&a.fragment, vec![file.clone()]), T0 + 1);
    let sent: Vec<_> = s.effects.iter().filter_map(|x| match x {
        Effect::Post { agent, fragment, body, files, .. } => Some((agent, fragment, body, files)),
        _ => None,
    }).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0.as_str(), sent[0].1.as_str(), sent[0].2["text"].as_str()), (a.fragment.as_str(), "talk--k3x9", Some("")));
    assert_eq!(sent[0].3, &vec![file]);
    assert_eq!(e.state().said, 1);
}

/// `s`, with each claim it makes answered as this life's, each live
/// answer's closure as on `work`, and each record it owes answered, and
/// what each answer does (the turn handed to the runtime, the answer given
/// it, an ended turn let go) added to it.
fn answered(e: &mut Engine, mut s: Step, now: u64) -> Step {
    let fences: Vec<(String, String)> = s.effects.iter().filter_map(|x| match x { Effect::Fence { turn, prompt, .. } => Some((turn.clone(), prompt.clone())), _ => None }).collect();
    for (turn, prompt) in fences {
        let more = e.step(Input::Fenced { turn, prompt, answer: ClaimAnswer::Ours { seq: None } }, now);
        s.dirty |= more.dirty;
        s.effects.extend(more.effects);
    }
    let claims: Vec<String> = s.effects.iter().filter_map(|x| match x { Effect::Claim { turn, .. } => Some(turn.clone()), _ => None }).collect();
    for turn in claims {
        let more = e.step(Input::Claimed { turn, answer: ClaimAnswer::Ours { seq: None } }, now);
        s.dirty |= more.dirty;
        s.effects.extend(more.effects);
    }
    let owed: Vec<(String, String)> = s.effects.iter().filter_map(|x| match x { Effect::Owed { turn, id, .. } => Some((turn.clone(), id.clone())), _ => None }).collect();
    for (turn, id) in owed {
        let more = e.step(Input::Posted { turn, id }, now);
        s.dirty |= more.dirty;
        s.effects.extend(more.effects);
    }
    s
}

fn rec(seq: u64, principal: &str, body: Value) -> Record {
    Record { channel: "chat".into(), seq, at: 0, principal: principal.into(), kind: "message".into(), body }
}

fn view(agents: &[&Agent]) -> ChatView {
    let mut v = ChatView { agents: agents.iter().map(|a| a.identity.clone()).collect(), ..ChatView::default() };
    v.names.insert("npub1paul".into(), "paul".into());
    v.names.insert("npub1skyler".into(), "skyler".into());
    v
}

fn said(e: &mut Engine, a: &Agent, v: &ChatView, seq: u64, principal: &str, body: Value, now: u64) -> Step {
    let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: rec(seq, principal, body), view: Some(v.clone()), since: 0 }, now);
    answered(e, s, now)
}

/// What a step posts, claims, live answers' closures and owed records included.
fn posts(s: &Step) -> Vec<(String, Value)> {
    s.effects.iter().filter_map(|e| match e { Effect::Post { id, body, .. } | Effect::Claim { id, body, .. } | Effect::Fence { id, body, .. } | Effect::Owed { id, body, .. } => Some((id.clone(), body.clone())), _ => None }).collect()
}

fn kinds(s: &Step) -> Vec<String> {
    posts(s).iter().map(|(_, b)| b.get("kind").and_then(Value::as_str).unwrap_or("reply").to_string()).collect()
}

fn started(s: &Step) -> Option<TurnStart> {
    s.effects.iter().find_map(|e| match e { Effect::Runtime(Command::Start(t)) => Some((**t).clone()), _ => None })
}

fn commands(s: &Step) -> Vec<Command> {
    s.effects.iter().filter_map(|e| match e { Effect::Runtime(c) => Some(c.clone()), _ => None }).collect()
}

fn keepalive(s: &Step) -> Option<bool> {
    s.effects.iter().find_map(|e| match e { Effect::Keepalive(k) => Some(*k), _ => None })
}

fn ev(e: &mut Engine, event: Event, now: u64) -> Step {
    let s = e.step(Input::Runtime(event), now);
    answered(e, s, now)
}

/// Goal: a person's message to the lead starts exactly one turn, which is
/// handed to the runtime with who asked, and keeps the computer awake.
#[test]
fn a_message_starts_one_turn() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let s = said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0);
    assert!(s.dirty, "admission is persisted before it is acted on");
    let t = started(&s).expect("handed to the runtime");
    assert_eq!((t.text.as_str(), t.asker.as_str(), t.asker_name.as_str(), t.seq), ("hi", "npub1paul", "paul", 1));
    assert_eq!(t.turn, records::turn_id(&a.fragment, "talk--k3x9", "chat", 1));
    assert_eq!(kinds(&s), vec!["turn.start"]);
    assert_eq!(posts(&s)[0].0, records::work_id(&t.turn, "start"));
    assert_eq!(posts(&s)[0].1["life"], LIFE, "claimed as this life");
    assert_eq!(keepalive(&s), Some(true));
    assert_eq!(e.cursor(&a.fragment, "talk--k3x9", "chat"), 1);

    // replay: the same record again (a catch-up, a reconnect) does nothing
    let again = said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0 + 1);
    assert_eq!(again, Step::default());
    // and an older one is behind the cursor too
    assert_eq!(said(&mut e, &a, &v, 0, "npub1paul", json!({ "text": "old" }), T0 + 2).effects, vec![]);
}

/// Invalid: the agent's own records, anonymous visitors, page-only kinds,
/// and broken bodies start nothing (but still move the cursor past them).
#[test]
fn what_starts_nothing() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let cases = [
        ("npub1juniper", json!({ "text": "my own reply", "turn": "abc" })),
        ("anon:7", json!({ "text": "hello?" })),
        ("npub1paul", json!({ "kind": "typing" })),
        ("npub1paul", json!({ "text": 12 })),
        ("npub1paul", json!({ "kind": "prompt_response", "prompt": "nope", "option": "once" })),
        ("npub1paul", json!({ "kind": "stop" })),
    ];
    for (i, (who, body)) in cases.into_iter().enumerate() {
        let seq = i as u64 + 1;
        let s = said(&mut e, &a, &v, seq, who, body.clone(), T0);
        assert!(started(&s).is_none(), "{who} {body}");
        assert!(posts(&s).is_empty(), "{who} {body}");
        assert_eq!(e.cursor(&a.fragment, "talk--k3x9", "chat"), seq, "the cursor passes it anyway");
    }
    // a record for an agent this computer does not run is refused whole
    let stranger = agent("rowan");
    let s = said(&mut e, &stranger, &v, 99, "npub1paul", json!({ "text": "hi" }), T0);
    assert_eq!(s, Step::default());
}

/// Goal: a streamed reply shows as drafts, then posts as one record naming
/// its turn (which replaces the draft), then the turn ends and the draft is
/// stopped, and the computer may sleep again.
#[test]
fn a_reply_streams_then_posts() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0)).expect("started").turn;
    let d = ev(&mut e, Event::Draft { turn: turn.clone(), text: "He".into() }, T0 + 2);
    assert_eq!(d.effects, vec![Effect::Draft { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), turn: turn.clone(), text: Some("He".into()) }]);
    assert!(!d.dirty, "drafts are never stored");
    let r = ev(&mut e, Event::Reply { turn: turn.clone(), part: 1, text: "Hello".into() }, T0 + 3);
    assert!(posts(&r).is_empty(), "a reply waits for its turn's end, or a later part");
    let end = e.step(Input::Runtime(Event::End { turn: turn.clone(), outcome: Outcome::Idle }), T0 + 4);
    let p = posts(&end);
    assert_eq!(p[0], (records::reply_id(&turn, 1), json!({ "text": "Hello", "turn": turn })));
    assert_eq!(p[1], (records::work_id(&turn, "end"), json!({ "kind": "turn.end", "turn": turn, "outcome": "idle" })));
    assert!(end.effects.contains(&Effect::Draft { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), turn: turn.clone(), text: None }));
    assert_eq!(keepalive(&end), Some(false));
    // its end is owed: the turn is kept, ended, until the lane is done with it
    let t = &e.state().turns[&turn];
    assert_eq!((t.phase, t.owed.len()), (Phase::Ended, 1));
    assert!(e.step(Input::Runtime(Event::Draft { turn: turn.clone(), text: "late".into() }), T0 + 4).effects.is_empty(), "an ended turn hears its runtime no more");
    let done = e.step(Input::Posted { turn: turn.clone(), id: records::work_id(&turn, "end") }, T0 + 4);
    assert!(done.dirty && done.effects.is_empty());
    assert!(e.state().turns.is_empty(), "let go once its end is answered");
    // replay: a late event for the ended turn is dropped
    assert_eq!(ev(&mut e, Event::Reply { turn: turn.clone(), part: 2, text: "late".into() }, T0 + 5), Step::default());
}

/// Goal: a runtime's notice (neither a step nor a reply) is a `turn.notice`
/// of the agent's turn running in its chat, numbered from 1; one after the
/// turn's end (a review its runtime finishes once the turn is done) is
/// still that turn's; the next turn there takes those after it. Invalid: a
/// notice before any turn ran there, an agent not of this computer, no
/// words, another chat: nothing. Past the bound, nothing. Restart: a new
/// life knows no turn of the last one's, so a late notice goes unposted.
#[test]
fn notices_belong_to_the_chats_turn() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let notice = |agent: &str, fragment: &str, text: &str| Event::Notice { agent: agent.into(), fragment: fragment.into(), category: NoticeCategory::Warning, text: text.into() };
    let mine = |text: &str| notice("juniper--k3x9", "talk--k3x9", text);
    assert!(posts(&ev(&mut e, mine("before any turn"), T0)).is_empty(), "no turn ran there");
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0)).expect("started").turn;
    let s = ev(&mut e, mine("I seem to be stuck"), T0 + 1);
    assert_eq!(posts(&s), vec![(records::work_id(&turn, "n1"), json!({ "kind": "turn.notice", "turn": turn, "category": "warning", "text": "I seem to be stuck" }))]);
    assert!(!s.dirty, "this life's alone");
    for bad in [notice("rowan--k3x9", "talk--k3x9", "not ours"), mine("  "), notice("juniper--k3x9", "notes--k3x9", "no turn there")] {
        assert!(posts(&ev(&mut e, bad.clone(), T0 + 1)).is_empty(), "{bad:?}");
    }
    // the turn ends; its review's notice after is still its own
    ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 2);
    let late = ev(&mut e, Event::Notice { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), category: NoticeCategory::Memory, text: "Memory updated".into() }, T0 + 3);
    assert_eq!(posts(&late), vec![(records::work_id(&turn, "n2"), json!({ "kind": "turn.notice", "turn": turn, "category": "memory", "text": "Memory updated" }))]);
    // the next turn there takes the next
    let t2 = started(&said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "again" }), T0 + 4)).expect("started").turn;
    assert_eq!(posts(&ev(&mut e, mine("Compressing context"), T0 + 5))[0].0, records::work_id(&t2, "n1"));
    // the bound
    for _ in 1..limits::NOTICES_PER_TURN_MAX {
        assert_eq!(posts(&ev(&mut e, mine("again"), T0 + 6)).len(), 1);
    }
    assert!(posts(&ev(&mut e, mine("one too many"), T0 + 7)).is_empty(), "at most {} a turn", limits::NOTICES_PER_TURN_MAX);
    // restart: the next life knows no turn of this one's
    let saved: State = serde_json::from_str(&serde_json::to_string(e.state()).expect("serializes")).expect("deserializes");
    let mut e2 = Engine::new(saved, Settings::default(), "fedcba9876543210fedcba9876543210").expect("whole");
    e2.step(Input::Agents(vec![a.clone()]), T0 + 8);
    e2.recover(T0 + 8);
    assert!(posts(&e2.step(Input::Runtime(mine("late")), T0 + 9)).is_empty(), "no turn of this life's there");
}

/// Goal (Paul, 2026-10-24: everything the agent says is a reply in the
/// chat): text, then a tool call, then text: the first reply posts before
/// the step, whole (a mid-turn answer past a step's old 300 characters is
/// never cut), marked `interim` (the turn worked on after it), the chat's
/// draft of it put away; steps number from 1, carry no words, and a late
/// edit of a posted part is dropped. The last reply is no interim one, nor
/// is an answer said beside a call that only updates memory.
#[test]
fn steps_split_replies() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "look it up" }), T0)).expect("started").turn;
    let looking = format!("Looking. {}", "I will read every page of the docs first, then the code, so nothing I change breaks what works. ".repeat(20));
    assert!(looking.chars().count() > 1000);
    ev(&mut e, Event::Reply { turn: turn.clone(), part: 1, text: looking.clone() }, T0 + 1);
    let step = ToolStep { tool: "web_search".into(), args: "{\"q\":\"x\"}".into(), ok: true, excerpt: "3 results".into(), category: crate::records::Category::Web, evidence: None };
    let s = ev(&mut e, Event::Step { turn: turn.clone(), step: step.clone() }, T0 + 2);
    assert_eq!(kinds(&s), vec!["reply", "turn.step"]);
    assert_eq!(posts(&s)[0], (records::reply_id(&turn, 1), json!({ "text": looking, "turn": turn, "interim": true })), "whole, before the step, interim");
    assert!(s.effects.iter().any(|x| matches!(x, Effect::Draft { turn: t, text: None, .. } if *t == turn)), "its draft put away as it posts: {:?}", s.effects);
    assert_eq!(posts(&s)[1].0, records::work_id(&turn, "1"));
    assert_eq!(posts(&s)[1].1["step"], 1);
    assert!(posts(&s)[1].1.get("text").is_none(), "a step carries no words: {}", posts(&s)[1].1);
    let late = ev(&mut e, Event::Reply { turn: turn.clone(), part: 1, text: "Looking!".into() }, T0 + 3);
    assert!(posts(&late).is_empty() && late.effects.is_empty(), "part 1 was posted");
    ev(&mut e, Event::Step { turn: turn.clone(), step }, T0 + 4);
    ev(&mut e, Event::Reply { turn: turn.clone(), part: 2, text: "Found it.".into() }, T0 + 5);
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 6);
    assert_eq!(posts(&end)[0], (records::reply_id(&turn, 2), json!({ "text": "Found it.", "turn": turn })), "the answer, no interim one");
    // an answer said beside a call that only updates memory (Hermes sends
    // nothing after it) is the turn's answer, not interim
    let t3 = started(&said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "remember this" }), T0 + 7)).expect("started").turn;
    ev(&mut e, Event::Reply { turn: t3.clone(), part: 1, text: "Noted: you like tomatoes.".into() }, T0 + 8);
    let memory = ToolStep { tool: "memory".into(), args: "tomatoes".into(), ok: true, excerpt: String::new(), category: crate::records::Category::Memory, evidence: None };
    let m = ev(&mut e, Event::Step { turn: t3.clone(), step: memory }, T0 + 9);
    assert_eq!(posts(&m)[0], (records::reply_id(&t3, 1), json!({ "text": "Noted: you like tomatoes.", "turn": t3 })));
    let end3 = ev(&mut e, Event::End { turn: t3.clone(), outcome: Outcome::Idle }, T0 + 10);
    assert_eq!(kinds(&end3), vec!["turn.end"], "nothing said again");
    // a retracted part never posts
    let t2 = started(&said(&mut e, &a, &v, 4, "npub1paul", json!({ "text": "again" }), T0 + 11)).expect("started").turn;
    ev(&mut e, Event::Reply { turn: t2.clone(), part: 1, text: "oops".into() }, T0 + 12);
    ev(&mut e, Event::Retract { turn: t2.clone(), part: 1 }, T0 + 13);
    let end2 = ev(&mut e, Event::End { turn: t2.clone(), outcome: Outcome::Stopped }, T0 + 14);
    assert_eq!(kinds(&end2), vec!["turn.end"]);
}

/// Goal (Alex's review of #584: 64 narrated tool calls spent the turn's
/// replies, and its answer was dropped): the words before a tool call have
/// a bound of their own (`INTERIM_REPLIES_PER_TURN_MAX`, one a posted
/// step), so narration never costs the answer. A turn that narrates past
/// `REPLIES_PER_TURN_MAX`, and past its own bound, still posts its answer,
/// whole and not interim (so it is pushed), numbered after the words; the
/// words past their bound go unposted. The answer's own bound still holds
/// for a turn that replies without end. Restart: the counts are the state's.
#[test]
fn narration_never_costs_the_answer() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "fix everything" }), T0)).expect("started").turn;
    let step = || ToolStep { tool: "terminal".into(), args: "ls".into(), ok: true, excerpt: String::new(), category: crate::records::Category::Shell, evidence: None };
    let narrated = limits::INTERIM_REPLIES_PER_TURN_MAX + 5;
    assert!(narrated > limits::REPLIES_PER_TURN_MAX && limits::REPLIES_PER_TURN_MAX >= 64);
    let mut posted = Vec::new();
    for part in 1..=narrated {
        ev(&mut e, Event::Reply { turn: turn.clone(), part, text: format!("Checking part {part}.") }, T0 + 1);
        let s = ev(&mut e, Event::Step { turn: turn.clone(), step: step() }, T0 + 1);
        posted.extend(posts(&s).into_iter().filter(|(id, _)| id.starts_with("rp:")));
    }
    assert_eq!(posted.len(), limits::INTERIM_REPLIES_PER_TURN_MAX as usize, "the words, up to their own bound, past the replies' 64");
    assert!(posted.iter().all(|(_, b)| b["interim"] == true));
    assert_eq!(posted[64], (records::reply_id(&turn, 65), json!({ "text": "Checking part 65.", "turn": turn, "interim": true })), "the 65th words post");
    // a restart in the middle keeps the counts (the state's)
    let saved: State = serde_json::from_str(&serde_json::to_string(e.state()).expect("serializes")).expect("deserializes");
    assert_eq!(saved.turns[&turn].interim, limits::INTERIM_REPLIES_PER_TURN_MAX);
    ev(&mut e, Event::Reply { turn: turn.clone(), part: narrated + 1, text: "Final answer: all fixed.".into() }, T0 + 2);
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 3);
    let n = limits::INTERIM_REPLIES_PER_TURN_MAX + 1;
    assert_eq!(posts(&end)[0], (records::reply_id(&turn, n), json!({ "text": "Final answer: all fixed.", "turn": turn })), "the answer, whole and not interim");
    // the answer's own bound: a runtime that replies without end
    let t2 = started(&said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "talk" }), T0 + 4)).expect("started").turn;
    let mut said_parts = 0;
    for part in 1..=limits::REPLIES_PER_TURN_MAX + 3 {
        let s = ev(&mut e, Event::Reply { turn: t2.clone(), part, text: format!("part {part}") }, T0 + 5);
        said_parts += posts(&s).len();
    }
    let end2 = ev(&mut e, Event::End { turn: t2.clone(), outcome: Outcome::Idle }, T0 + 6);
    said_parts += posts(&end2).iter().filter(|(id, _)| id.starts_with("rp:")).count();
    assert_eq!(said_parts, limits::REPLIES_PER_TURN_MAX as usize, "at most so many, interim ones apart");
}

/// Goal: one turn of an agent runs in a chat at a time, the rest in order;
/// past the queue's bound a message is told so, not queued: its claim and
/// its end, both records as every turn has, and nothing run.
#[test]
fn one_turn_at_a_time() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let first = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "one" }), T0)).expect("started").turn;
    let s2 = said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "two" }), T0);
    assert!(started(&s2).is_none() && posts(&s2).is_empty(), "queued, silently");
    said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "three" }), T0);
    let next = started(&ev(&mut e, Event::End { turn: first, outcome: Outcome::Idle }, T0 + 1)).expect("the next one");
    assert_eq!(next.text, "two");
    // the bound: the running one plus QUEUED_PER_CHAT_MAX waiting
    for seq in 4..(3 + limits::QUEUED_PER_CHAT_MAX as u64) {
        let s = said(&mut e, &a, &v, seq, "npub1paul", json!({ "text": "more" }), T0);
        assert!(posts(&s).is_empty());
    }
    let over = said(&mut e, &a, &v, 100, "npub1paul", json!({ "text": "too many" }), T0);
    assert_eq!(kinds(&over), vec!["turn.start", "turn.end"]);
    let refused = records::turn_id(&a.fragment, "talk--k3x9", "chat", 100);
    assert_eq!(posts(&over)[0], (records::work_id(&refused, "start"), json!({ "kind": "turn.start", "turn": refused, "asker": "npub1paul", "agent": "npub1juniper", "cause": { "fragment": "talk--k3x9", "channel": "chat", "seq": 100 }, "life": LIFE })));
    assert_eq!(posts(&over)[1].1["outcome"], "error");
    assert!(started(&over).is_none());
    assert!(!e.state().turns.contains_key(&refused), "held only until both its records were answered");
}

/// Goal: only the turn's asker stops it. A Stop from anyone else is
/// ignored; a Stop naming a queued turn removes it, with its claim and its
/// end (both records, nothing run).
#[test]
fn stop_is_the_askers() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "long" }), T0)).expect("started").turn;
    let other = said(&mut e, &a, &v, 2, "npub1skyler", json!({ "kind": "stop", "turn": turn }), T0 + 1);
    assert!(commands(&other).is_empty(), "skyler did not ask it");
    let s = said(&mut e, &a, &v, 3, "npub1paul", json!({ "kind": "stop" }), T0 + 2);
    assert_eq!(commands(&s), vec![Command::Stop { turn: turn.clone() }]);
    // replay: stopping again asks nothing more
    let again = said(&mut e, &a, &v, 4, "npub1paul", json!({ "kind": "stop", "turn": turn }), T0 + 3);
    assert!(commands(&again).is_empty());
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Stopped }, T0 + 4);
    assert_eq!(posts(&end).last().expect("its end").1["outcome"], "stopped");

    // a queued turn, stopped by name
    let running = started(&said(&mut e, &a, &v, 5, "npub1paul", json!({ "text": "a" }), T0 + 5)).expect("started").turn;
    said(&mut e, &a, &v, 6, "npub1paul", json!({ "text": "b" }), T0 + 5);
    let queued = records::turn_id(&a.fragment, "talk--k3x9", "chat", 6);
    let s = said(&mut e, &a, &v, 7, "npub1paul", json!({ "kind": "stop", "turn": queued }), T0 + 6);
    assert_eq!(kinds(&s), vec!["turn.start", "turn.end"]);
    assert_eq!((posts(&s)[0].0.as_str(), &posts(&s)[0].1["life"]), (records::work_id(&queued, "start").as_str(), &json!(LIFE)));
    assert_eq!(posts(&s)[1], (records::work_id(&queued, "end"), json!({ "kind": "turn.end", "turn": queued, "outcome": "stopped" })));
    assert!(commands(&s).is_empty(), "the runtime never had it");
    let after = ev(&mut e, Event::End { turn: running, outcome: Outcome::Idle }, T0 + 7);
    assert!(started(&after).is_none(), "the stopped one never runs");
}

/// Goal: a prompt is a card for the agent's owner; while it waits the
/// computer is kept awake; the owner's answer (the first) resumes the turn.
/// Invalid: someone else's answer, an unknown option, a second answer.
#[test]
fn an_approval_answered() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "risky" }), T0)).expect("started").turn;
    let options = vec![PromptOption { id: "once".into(), label: "Allow once".into(), style: None, words: false }, PromptOption { id: "deny".into(), label: "Deny".into(), style: Some("danger".into()), words: false }];
    let p = ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "ab12.0011".into(), text: "Run rm?".into(), options: options.clone(), ttl_ms: None }, T0 + 10);
    let (id, body) = posts(&p)[0].clone();
    assert_eq!(id, records::work_id(&turn, "p:ab12.0011"));
    assert_eq!(body["kind"], "turn.prompt");
    assert_eq!(body["asks"], "npub1paul");
    assert_eq!(body["expiresAt"], T0 + 10 + 60_000);
    assert_eq!(keepalive(&p), Some(false), "waiting on a person releases the hold");
    assert!(!e.keepalive());
    // a repeat of the same prompt asks nothing
    assert_eq!(ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "ab12.0011".into(), text: "again".into(), options, ttl_ms: None }, T0 + 11).effects, vec![]);

    let skyler = said(&mut e, &a, &v, 2, "npub1skyler", json!({ "kind": "prompt_response", "prompt": "ab12.0011", "option": "once" }), T0 + 20);
    assert!(commands(&skyler).is_empty() && posts(&skyler).is_empty(), "only the owner answers");
    let bad = said(&mut e, &a, &v, 3, "npub1paul", json!({ "kind": "prompt_response", "prompt": "ab12.0011", "option": "always" }), T0 + 21);
    assert!(commands(&bad).is_empty(), "not an option of this prompt");
    let yes = said(&mut e, &a, &v, 4, "npub1paul", json!({ "kind": "prompt_response", "prompt": "ab12.0011", "option": "once" }), T0 + 22);
    assert_eq!(commands(&yes), vec![Command::Answer { turn: turn.clone(), prompt: "ab12.0011".into(), option: Some("once".into()), seq: 4, by: "npub1paul".into(), words: None, attached: None }]);
    assert_eq!(posts(&yes)[0].1, json!({ "kind": "turn.prompt.closed", "turn": turn, "prompt": "ab12.0011", "outcome": "answered", "option": "once", "by": "npub1paul" }));
    assert_eq!(keepalive(&yes), Some(true), "running again");
    let second = said(&mut e, &a, &v, 5, "npub1paul", json!({ "kind": "prompt_response", "prompt": "ab12.0011", "option": "deny" }), T0 + 23);
    assert!(commands(&second).is_empty(), "the first answer won");
}

/// Goal (audit M7, residual): prompt ids are each agent's own, so another
/// agent of the chat may show a card of the same id; the owner's click on
/// that card names its agent and turn (the chat's page does), and must
/// never answer this agent's card. Method: answers naming another agent,
/// and this agent's other turn, are ignored and leave the card open; one
/// naming this agent's own turn answers it, as a bare one (the API's) does.
#[test]
fn an_answer_to_another_agents_card_of_the_same_prompt_id_answers_nothing() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "risky" }), T0)).expect("started").turn;
    let options = vec![PromptOption { id: "once".into(), label: "Allow once".into(), style: None, words: false }];
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p1".into(), text: "Run rm?".into(), options, ttl_ms: None }, T0 + 10);
    let answer = |agent: &str, turn: &str| json!({ "kind": "prompt_response", "prompt": "p1", "option": "once", "agent": agent, "turn": turn });
    let other_agent = said(&mut e, &a, &v, 2, "npub1paul", answer("npub1forger", &turn), T0 + 20);
    assert!(commands(&other_agent).is_empty() && posts(&other_agent).is_empty(), "an answer to another agent's card of that id is not this card's");
    let other_turn = said(&mut e, &a, &v, 3, "npub1paul", answer(&a.identity, "another-turn"), T0 + 21);
    assert!(commands(&other_turn).is_empty() && posts(&other_turn).is_empty(), "nor is one to another turn of its own");
    let mine = said(&mut e, &a, &v, 4, "npub1paul", answer(&a.identity, &turn), T0 + 22);
    assert_eq!(commands(&mine), vec![Command::Answer { turn: turn.clone(), prompt: "p1".into(), option: Some("once".into()), seq: 4, by: "npub1paul".into(), words: None, attached: None }], "its own card, named, is answered");
}

/// Goal (Paul on p5, 2026-10-09: "'other' type your answer for questions
/// doesn't work very well"): an option answered in words (a clarify's
/// "Other") is answered on its card: the words go to the runtime with the
/// answer, and the card's close says them, so nothing asks for them after
/// and the person's next message is a turn of its own. Invalid: words with
/// an option that takes none go nowhere.
#[test]
fn other_answered_in_words_on_its_card() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "pick for me" }), T0)).expect("started").turn;
    let options = vec![PromptOption { id: "c0".into(), label: "red".into(), style: None, words: false }, PromptOption { id: "other".into(), label: "Other".into(), style: None, words: true }];
    let p = ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "q.1".into(), text: "Which color?".into(), options: options.clone(), ttl_ms: None }, T0 + 10);
    assert_eq!(posts(&p)[0].1["options"][1], json!({ "id": "other", "label": "Other", "words": true }), "the card says which option takes words");
    let other = said(&mut e, &a, &v, 2, "npub1paul", json!({ "kind": "prompt_response", "prompt": "q.1", "option": "other", "text": "purple" }), T0 + 20);
    assert_eq!(commands(&other), vec![Command::Answer { turn: turn.clone(), prompt: "q.1".into(), option: Some("other".into()), seq: 2, by: "npub1paul".into(), words: Some("purple".into()), attached: None }]);
    assert_eq!(posts(&other)[0].1, json!({ "kind": "turn.prompt.closed", "turn": turn, "prompt": "q.1", "outcome": "answered", "option": "other", "by": "npub1paul", "text": "purple" }));
    // the turn asks nothing after: the next message waits as a turn of its own
    let next = said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "and now?" }), T0 + 30);
    assert!(commands(&next).is_empty() && started(&next).is_none(), "queued behind the running turn, told to it never");

    // words with an option that takes none go nowhere
    let p2 = ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "q.2".into(), text: "Again?".into(), options, ttl_ms: None }, T0 + 40);
    assert_eq!(posts(&p2).len(), 1);
    let red = said(&mut e, &a, &v, 4, "npub1paul", json!({ "kind": "prompt_response", "prompt": "q.2", "option": "c0", "text": "sneaky" }), T0 + 50);
    assert_eq!(commands(&red), vec![Command::Answer { turn: turn.clone(), prompt: "q.2".into(), option: Some("c0".into()), seq: 4, by: "npub1paul".into(), words: None, attached: None }]);
    assert!(posts(&red)[0].1.get("text").is_none(), "{:?}", posts(&red));
}

/// Goal: an unanswered prompt expires: its card says so, and the runtime is
/// told it got no answer. An answer after that is ignored.
#[test]
fn an_approval_expires() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "risky" }), T0)).expect("started").turn;
    let options = vec![PromptOption { id: "once".into(), label: "Allow".into(), style: None, words: false }];
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p1".into(), text: "ok?".into(), options, ttl_ms: Some(30_000) }, T0);
    assert!(e.step(Input::Tick, T0 + 29_999).effects.is_empty(), "not yet");
    let t = e.step(Input::Tick, T0 + 30_000);
    assert_eq!(posts(&t)[0].1["outcome"], "expired");
    assert_eq!(commands(&t), vec![Command::Answer { turn: turn.clone(), prompt: "p1".into(), option: None, seq: 0, by: String::new(), words: None, attached: None }]);
    let late = said(&mut e, &a, &v, 2, "npub1paul", json!({ "kind": "prompt_response", "prompt": "p1", "option": "once" }), T0 + 40_000);
    assert!(commands(&late).is_empty());
    // a prompt no card can show is expired at once
    let bad = ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "has space".into(), text: "?".into(), options: vec![], ttl_ms: None }, T0 + 41_000);
    assert!(posts(&bad).is_empty());
    assert_eq!(commands(&bad).len(), 1);
}

/// Goal (Paul on p5, 2026-10-05: a card missed, then no answers): an open
/// card releases its computer while it waits. A queued message still
/// holds it; expiry lets the original runtime continue when still alive.
#[test]
fn an_open_card_releases_its_computer_until_it_expires() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "risky" }), T0)).expect("started").turn;
    let options = vec![PromptOption { id: "once".into(), label: "Allow".into(), style: None, words: false }];
    let p = ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p1".into(), text: "ok?".into(), options, ttl_ms: None }, T0 + 10);
    assert_eq!(posts(&p)[0].1["expiresAt"], T0 + 10 + 60_000);
    assert_eq!(keepalive(&p), Some(false));
    assert!(!e.keepalive(), "only waiting on a person");
    let hello = said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "hello?" }), T0 + 20_000);
    assert!(started(&hello).is_none(), "a message meanwhile waits behind the card's turn");
    let quiet = e.step(Input::Tick, T0 + 10 + 59_999);
    assert!(quiet.effects.is_empty() && e.keepalive(), "held to the card's last moment");
    let x = e.step(Input::Tick, T0 + 10 + 60_000);
    assert_eq!(posts(&x), vec![(records::work_id(&turn, "pc:p1"), json!({ "kind": "turn.prompt.closed", "turn": turn, "prompt": "p1", "outcome": "expired" }))]);
    assert_eq!(commands(&x), vec![Command::Answer { turn: turn.clone(), prompt: "p1".into(), option: None, seq: 0, by: String::new(), words: None, attached: None }]);
    assert_eq!((e.state().turns[&turn].phase, keepalive(&x), e.keepalive()), (Phase::Running, None, true), "running again, as its runtime goes on without the answer");
    // the runtime ends it (Hermes: its approval timed out with the card, the
    // command BLOCKED, then its reply): the message that waited is run
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 61_000);
    assert_eq!(posts(&end).iter().filter(|(id, _)| id == &records::work_id(&turn, "end")).count(), 1, "the card's turn ends once");
    let next = started(&end).expect("the message that waited is claimed and run");
    assert_eq!(next.text, "hello?");
    assert_eq!(keepalive(&end), None, "awake for it");
    let done = ev(&mut e, Event::End { turn: next.turn.clone(), outcome: Outcome::Idle }, T0 + 62_000);
    assert_eq!(keepalive(&done), Some(false), "nothing open: let go");
}

/// Goal: a restart keeps every admission: a turn the runtime held is ended
/// (one life per turn: its runtime died with the life before), a queued one
/// is claimed by the new life once its runtime can take it, and runs, and
/// catching up on the same records starts nothing. (A turn handed and never
/// taken is ended too: `a_restart_ends_what_was_handed`.)
#[test]
fn a_restart_starts_nothing_twice() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let first = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "one" }), T0)).expect("started").turn;
    said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "two" }), T0);
    let b = agent("rowan");
    // the state as persisted, read back
    let saved = serde_json::to_string(e.state()).expect("serializes");
    let state: State = serde_json::from_str(&saved).expect("deserializes");
    assert_eq!(&state, e.state(), "a round trip changes nothing");
    assert!(!saved.contains(LIFE), "the life is never in the state");

    let next_life = "fedcba9876543210fedcba9876543210";
    let mut e2 = Engine::new(state, Settings::default(), next_life).expect("whole");
    e2.step(Input::Agents(vec![a.clone(), b]), T0 + 10);
    let r = e2.recover(T0 + 10);
    let p = posts(&r);
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!(p[0].1, json!({ "kind": "turn.end", "turn": first, "outcome": "error", "error": LOST }));
    assert!(started(&r).is_none(), "nothing runs before its runtime can take it");
    let c = e2.step(Input::Runtime(Event::Connected(true)), T0 + 10);
    assert_eq!(posts(&c)[0].1["life"], next_life, "claimed by the new life");
    let next = started(&answered(&mut e2, c, T0 + 10)).expect("the queued one runs");
    assert_eq!(next.text, "two");
    // the backlog again: nothing new
    for seq in 1..=2 {
        assert_eq!(said(&mut e2, &a, &v, seq, "npub1paul", json!({ "text": "again" }), T0 + 11), Step::default());
    }
    assert_eq!(e2.state().boot, 2);
}

/// Goal (a rollback of `/data`): the turns a life reads again that another
/// life ran (their claims answered 409) are ones its runtime does not
/// remember: the agent's next turn in that chat carries them
/// (`TurnStart::forgotten`, the newest `NOTE_FORGOTTEN_MAX` and how many
/// more), said once; another chat's turn, or another agent's, carries none.
/// Method: a life's claims answered as another's, then its own.
#[test]
fn turns_another_life_ran_are_told_once_to_the_next_turn() {
    let a = agent("juniper");
    let b = agent("rowan");
    let v = view(&[&a, &b]);
    let mut e = engine(&[a.clone(), b.clone()]);
    let theirs = |e: &mut Engine, s: Step| -> Vec<String> {
        let claims: Vec<String> = s.effects.iter().filter_map(|x| match x { Effect::Claim { turn, .. } => Some(turn.clone()), _ => None }).collect();
        for turn in &claims {
            let r = e.step(Input::Claimed { turn: turn.clone(), answer: ClaimAnswer::Theirs }, T0 + 1);
            assert!(started(&r).is_none(), "another life's turn never runs here");
            let owed: Vec<(String, String)> = r.effects.iter().filter_map(|x| match x { Effect::Owed { turn, id, .. } => Some((turn.clone(), id.clone())), _ => None }).collect();
            for (turn, id) in owed {
                // a 409 too: that life ended it
                e.step(Input::Posted { turn, id }, T0 + 1);
            }
        }
        claims
    };
    // the backlog a rollback sent the cursor back over: six of juniper's turns
    let mut ran = vec![];
    for seq in 1..=(limits::NOTE_FORGOTTEN_MAX as u64 + 2) {
        let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: rec(seq, "id:paul", json!({ "text": format!("old {seq}") })), view: Some(v.clone()), since: 0 }, T0);
        ran.extend(theirs(&mut e, s));
    }
    assert_eq!(ran.len(), limits::NOTE_FORGOTTEN_MAX + 2);
    // rowan's own turn there is told nothing of juniper's
    let r = said(&mut e, &b, &v, 20, "id:paul", json!({ "text": "@rowan hi", "to": [b.identity] }), T0 + 2);
    let rowans = started(&r).expect("rowan's runs");
    assert!(rowans.forgotten.is_empty() && rowans.forgotten_more == 0, "{rowans:?}");
    // juniper's next turn there is told of the newest four, and two more
    let r = said(&mut e, &a, &v, 21, "id:paul", json!({ "text": "and now?" }), T0 + 3);
    let next = started(&r).expect("juniper's runs");
    assert_eq!(next.forgotten, ran[2..].to_vec(), "the newest, oldest first");
    assert_eq!(next.forgotten_more, 2);
    ev(&mut e, Event::Reply { turn: next.turn.clone(), part: 1, text: "ok".into() }, T0 + 4);
    ev(&mut e, Event::End { turn: next.turn.clone(), outcome: Outcome::Idle }, T0 + 4);
    // said once: the turn after it is told nothing
    let r = said(&mut e, &a, &v, 22, "id:paul", json!({ "text": "again" }), T0 + 5);
    let after = started(&r).expect("runs");
    assert!(after.forgotten.is_empty() && after.forgotten_more == 0, "{after:?}");
}

/// Invalid state: a turn past its channel's cursor, or a wrong id, is
/// refused at load rather than trusted.
#[test]
fn a_corrupt_state_is_refused() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    said(&mut e, &a, &v, 5, "npub1paul", json!({ "text": "x" }), T0);
    let mut behind = e.state().clone();
    behind.cursors.insert(cursor_key(&a.fragment, "talk--k3x9", "chat"), 4);
    assert!(Engine::new(behind, Settings::default(), LIFE).is_err());
    let mut renamed = e.state().clone();
    let (id, mut t) = renamed.turns.pop_first().expect("one");
    t.id = "0".repeat(24);
    renamed.turns.insert(id, t);
    assert!(Engine::new(renamed, Settings::default(), LIFE).is_err());
    let mut old = e.state().clone();
    old.version = 0;
    assert!(Engine::new(old, Settings::default(), LIFE).is_err());
}

/// Goal (decision 8): in a group, an `@mention` (or `to`) picks who answers;
/// otherwise the lead, the first agent added, does. A reply that names
/// another agent hands off to it, a bounded number of hops deep.
#[test]
fn a_group_picks_who_answers() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let both = |e: &mut Engine, seq: u64, who: &str, body: Value| -> (bool, bool) {
        let sj = said(e, &j, &v, seq, who, body.clone(), T0);
        let sr = said(e, &r, &v, seq, who, body, T0);
        (started(&sj).is_some(), started(&sr).is_some())
    };
    assert_eq!(both(&mut e, 1, "npub1paul", json!({ "text": "hello all" })), (true, false), "the lead answers");
    e.step(Input::Runtime(Event::End { turn: records::turn_id(&j.fragment, "talk--k3x9", "chat", 1), outcome: Outcome::Idle }), T0);
    assert_eq!(both(&mut e, 2, "npub1paul", json!({ "text": "@Rowan what do you think?" })), (false, true), "a mention picks rowan");
    e.step(Input::Runtime(Event::End { turn: records::turn_id(&r.fragment, "talk--k3x9", "chat", 2), outcome: Outcome::Idle }), T0);
    assert_eq!(both(&mut e, 3, "npub1paul", json!({ "text": "you two", "to": ["npub1juniper", "npub1rowan"] })), (true, true), "to names both");

    // juniper hands off to rowan
    let jt = records::turn_id(&j.fragment, "talk--k3x9", "chat", 3);
    e.step(Input::Runtime(Event::End { turn: records::turn_id(&r.fragment, "talk--k3x9", "chat", 3), outcome: Outcome::Idle }), T0);
    e.step(Input::Runtime(Event::Reply { turn: jt.clone(), part: 1, text: "@rowan can take this".into() }), T0);
    let end = e.step(Input::Runtime(Event::End { turn: jt.clone(), outcome: Outcome::Idle }), T0);
    let reply = posts(&end)[0].1.clone();
    assert_eq!(reply["to"], json!(["npub1rowan"]));
    assert_eq!(reply["hop"], 1);
    let (byj, byr) = both(&mut e, 4, "npub1juniper", reply);
    assert_eq!((byj, byr), (false, true), "rowan takes the hand-off; juniper skips its own reply");
    assert_eq!(e.state().turns.values().find(|t| t.agent == r.fragment && t.phase != Phase::Ended).expect("rowan's").hop, 1);
    // too deep: an agent's message past HOPS_MAX starts nothing
    let deep = json!({ "text": "@juniper again", "turn": "x", "to": ["npub1juniper"], "hop": limits::HOPS_MAX + 1 });
    assert!(started(&said(&mut e, &j, &v, 5, "npub1rowan", deep, T0)).is_none());
    // an agent's message that names no one starts nothing, even for the lead
    assert!(started(&said(&mut e, &j, &v, 6, "npub1rowan", json!({ "text": "done", "turn": "y" }), T0)).is_none());
}

/// Goal: two agents on one computer, in two chats, run at once.
#[test]
fn two_agents_run_at_once() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let mut e = engine(&[j.clone(), r.clone()]);
    let s1 = said(&mut e, &j, &view(&[&j]), 1, "npub1paul", json!({ "text": "a" }), T0);
    let s2 = e.step(Input::Record { agent: r.fragment.clone(), fragment: "notes--k3x9".into(), record: rec(1, "npub1paul", json!({ "text": "b" })), view: Some(view(&[&r])), since: 0 }, T0);
    let s2 = answered(&mut e, s2, T0);
    let (t1, t2) = (started(&s1).expect("juniper"), started(&s2).expect("rowan"));
    assert_ne!(t1.turn, t2.turn);
    assert_eq!((t1.agent.fragment.as_str(), t2.agent.fragment.as_str()), ("juniper--k3x9", "rowan--k3x9"));
}

/// Goal: a routine on the agent's `tasks` channel is a turn in its chat, as
/// its owner; `joined` asks for a new listing.
#[test]
fn tasks_start_routines() {
    let a = agent("juniper");
    let mut e = engine(std::slice::from_ref(&a));
    let task = |seq, body| Record { channel: "tasks".into(), seq, at: 0, principal: "npub1paul".into(), kind: "message".into(), body };
    let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: a.fragment.clone(), record: task(1, json!({ "kind": "routine", "text": "water the plants", "chat": "talk--k3x9" })), view: None, since: 0 }, T0);
    let s = answered(&mut e, s, T0);
    let t = started(&s).expect("a routine turn");
    assert!(t.routine);
    assert_eq!((t.fragment.as_str(), t.asker.as_str()), ("talk--k3x9", "npub1paul"));
    assert_eq!(t.turn, records::turn_id(&a.fragment, &a.fragment, "tasks", 1));
    let j = e.step(Input::Record { agent: a.fragment.clone(), fragment: a.fragment.clone(), record: task(2, json!({ "kind": "joined", "fragment": "new--k3x9" })), view: None, since: 0 }, T0);
    assert!(j.effects.contains(&Effect::Discover { agent: a.fragment.clone(), joined: Some("new--k3x9".into()) }));
    // a `tasks` record on another fragment is no task
    let other = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: task(3, json!({ "kind": "joined" })), view: None, since: 0 }, T0);
    assert!(other.effects.is_empty());
}

/// Goal: a turn the runtime goes quiet on ends as an error; one that waits
/// on a person does not.
#[test]
fn a_quiet_turn_ends() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "x" }), T0)).expect("started").turn;
    assert!(e.step(Input::Tick, T0 + 600_000).effects.is_empty());
    let t = e.step(Input::Tick, T0 + 600_001);
    assert_eq!(commands(&t)[0], Command::Forget { turn: turn.clone() });
    assert_eq!(posts(&t).last().expect("its end").1["outcome"], "error");
}

/// Goal: an agent removed from a chat drops its turns there.
#[test]
fn gone_drops_turns() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "x" }), T0)).expect("started").turn;
    let g = e.step(Input::Gone { agent: a.fragment.clone(), fragment: "talk--k3x9".into() }, T0);
    assert_eq!(commands(&g), vec![Command::Forget { turn }]);
    assert!(posts(&g).is_empty(), "it can no longer post there");
    assert!(e.state().turns.is_empty());
}

/// Goal: what was said before the agent joined is history: it passes the
/// cursor and starts nothing; what was said since starts a turn.
#[test]
fn history_is_not_for_a_new_agent() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let at = |seq: u64, at: i64| Record { at, ..rec(seq, "npub1paul", json!({ "text": "hi @juniper" })) };
    let old = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: at(1, 500), view: Some(v.clone()), since: 1000 }, T0);
    assert!(started(&old).is_none());
    assert!(old.dirty, "its cursor still moves");
    assert_eq!(e.cursor(&a.fragment, "talk--k3x9", "chat"), 1);
    let new = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: at(2, 1000), view: Some(v), since: 1000 }, T0);
    let new = answered(&mut e, new, T0);
    assert!(started(&new).is_some());
}

/// Goal: an attachment rides on its reply part; a message's attachments are
/// handed with its turn.
#[test]
fn attachments_ride_along() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let sha = "b".repeat(64);
    let s = said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "see", "attachments": [{ "sha256": sha, "size": 4, "type": "image/png", "name": "a.png" }] }), T0);
    let t = started(&s).expect("started");
    assert_eq!(t.attachments.len(), 1);
    let file = LocalFile { path: "/tmp/x.png".into(), media_type: "image/png".into(), name: "x.png".into(), size: 4 };
    ev(&mut e, Event::Attachment { turn: t.turn.clone(), part: 1, file: file.clone() }, T0);
    let end = ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0);
    let files = end.effects.iter().find_map(|e| match e { Effect::Post { files, .. } if !files.is_empty() => Some(files.clone()), _ => None });
    assert_eq!(files, Some(vec![file]));
}

/// Goal: a turn that asks its asker something in words (Hermes' open
/// clarify) shows the question at once, and the asker's next message is
/// handed to that turn as its answer, never queued behind it (where the
/// turn would wait for it forever). Invalid: someone else's message, an
/// empty one, or one to another agent is not the answer. Replay: the
/// message again (behind the cursor) tells nothing twice. Restart: the
/// turn ends as lost, as any running one does (P1; across lives,
/// `a_question_cut_by_a_restart_is_lost_and_its_answer_runs_once`).
#[test]
fn a_question_is_answered_by_the_next_message() {
    let a = agent("juniper");
    let b = agent("rowan");
    let v = view(&[&a, &b]);
    let mut e = engine(&[a.clone(), b.clone()]);
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "keep my garden notes" }), T0)).expect("started").turn;
    ev(&mut e, Event::Reply { turn: turn.clone(), part: 1, text: "What do you plant?".into() }, T0 + 1);
    let asked = ev(&mut e, Event::Asked { turn: turn.clone() }, T0 + 2);
    assert_eq!(
        posts(&asked),
        vec![(records::reply_id(&turn, 1), json!({ "text": "What do you plant?", "turn": turn })), (records::work_id(&turn, "q1"), json!({ "kind": "turn.asked", "turn": turn, "asks": "npub1paul" }))],
        "the question shows at once, then whose answer it waits for"
    );
    assert!(asked.dirty);
    assert!(e.state().turns[&turn].asking);
    assert_eq!(keepalive(&asked), Some(false), "waiting on words releases the hold");
    // replay: the runtime saying it asks again, before an answer, posts nothing more
    assert!(posts(&ev(&mut e, Event::Asked { turn: turn.clone() }, T0 + 2)).is_empty());

    // invalid: another person's message, an empty one, one to the other agent
    let skyler = said(&mut e, &a, &v, 2, "npub1skyler", json!({ "text": "tomatoes?" }), T0 + 3);
    assert!(commands(&skyler).is_empty(), "queued behind it, as any message is");
    let empty = said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": " " }), T0 + 4);
    assert!(!commands(&empty).iter().any(|c| matches!(c, Command::Tell { .. })), "no words and no files, no answer");
    let other = said(&mut e, &b, &v, 4, "npub1paul", json!({ "text": "@rowan hi" }), T0 + 5);
    assert!(!commands(&other).iter().any(|c| matches!(c, Command::Tell { .. })));
    let to_rowan = said(&mut e, &a, &v, 4, "npub1paul", json!({ "text": "@rowan hi" }), T0 + 5);
    assert!(commands(&to_rowan).is_empty(), "not addressed to juniper");

    // valid: the asker's next message is the answer
    let told = said(&mut e, &a, &v, 5, "npub1paul", json!({ "text": "Tomatoes, at dawn" }), T0 + 6);
    assert_eq!(commands(&told), vec![Command::Tell { turn: turn.clone(), seq: 5, by: "npub1paul".into(), by_name: "paul".into(), text: "Tomatoes, at dawn".into(), attached: None }]);
    assert!(posts(&told).is_empty(), "no turn of its own");
    assert!(told.dirty && !e.state().turns[&turn].asking);
    // replay: behind the cursor, nothing
    assert_eq!(said(&mut e, &a, &v, 5, "npub1paul", json!({ "text": "Tomatoes, at dawn" }), T0 + 7), Step::default());
    // asked no longer, the next message queues as before
    let next = said(&mut e, &a, &v, 6, "npub1paul", json!({ "text": "and basil" }), T0 + 8);
    assert!(commands(&next).is_empty() && started(&next).is_none(), "queued");
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 9);
    let queued: Vec<String> = commands(&end).iter().filter_map(|c| match c { Command::Start(t) => Some(t.text.clone()), _ => None }).collect();
    assert_eq!(queued, vec!["tomatoes?".to_string()], "skyler's message runs next; then paul's empty one, then 'and basil'");

    // the state round-trips with a turn asking, and a restart ends it as
    // lost: a new life never tells a turn an earlier life ran
    let t2 = started(&said(&mut e, &a, &v, 7, "npub1paul", json!({ "text": "x" }), T0 + 10));
    assert!(t2.is_none(), "behind the others");
    let skylers = records::turn_id(&a.fragment, "talk--k3x9", "chat", 2);
    ev(&mut e, Event::Reply { turn: skylers.clone(), part: 1, text: "Which tomatoes?".into() }, T0 + 11);
    ev(&mut e, Event::Asked { turn: skylers.clone() }, T0 + 11);
    let saved: State = serde_json::from_str(&serde_json::to_string(e.state()).expect("serializes")).expect("deserializes");
    assert_eq!(&saved, e.state());
    assert!(saved.turns[&skylers].asking, "kept asking");
    let mut e2 = Engine::new(saved, Settings::default(), "fedcba9876543210fedcba9876543210").expect("whole");
    e2.step(Input::Agents(vec![a.clone(), b.clone()]), T0 + 12);
    let r = e2.recover(T0 + 12);
    assert!(posts(&r).contains(&(records::work_id(&skylers, "end"), json!({ "kind": "turn.end", "turn": skylers, "outcome": "error", "error": LOST }))), "ended as lost: {:?}", posts(&r));
    assert!(!e2.state().turns[&skylers].asking, "over, it asks nothing");
    e2.step(Input::Runtime(Event::Connected(true)), T0 + 12);
    let answer = said(&mut e2, &a, &v, 8, "npub1skyler", json!({ "text": "cherry ones" }), T0 + 13);
    assert!(!commands(&answer).iter().any(|c| matches!(c, Command::Tell { .. })), "never told to a turn of another life");
}

/// Goal: a turn waiting on its asker's words outlasts the idle bound up to
/// a prompt's lifetime, then ends as quiet; Stop still reaches it.
#[test]
fn a_question_waits_as_long_as_a_prompt() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = Engine::new(State::default(), Settings { prompt_ttl_ms: 3_600_000, turn_idle_ms: 600_000 }, LIFE).expect("a fresh state");
    e.step(Input::Agents(vec![a.clone()]), T0);
    e.recover(T0);
    e.step(Input::Runtime(Event::Connected(true)), T0);
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0)).expect("started").turn;
    ev(&mut e, Event::Reply { turn: turn.clone(), part: 1, text: "Which one?".into() }, T0);
    ev(&mut e, Event::Asked { turn: turn.clone() }, T0);
    assert!(commands(&e.step(Input::Tick, T0 + 600_001)).is_empty(), "past the idle bound, still asking");
    let quiet = e.step(Input::Tick, T0 + 3_600_001);
    assert_eq!(commands(&quiet), vec![Command::Forget { turn: turn.clone() }]);
    assert_eq!(posts(&quiet).last().expect("its end").1["outcome"], "error");

    let t2 = started(&said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "again" }), T0 + 3_600_002)).expect("started").turn;
    ev(&mut e, Event::Asked { turn: t2.clone() }, T0 + 3_600_003);
    let stop = said(&mut e, &a, &v, 3, "npub1paul", json!({ "kind": "stop" }), T0 + 3_600_004);
    assert_eq!(commands(&stop), vec![Command::Stop { turn: t2 }]);
    // the Stop answered the question (Relay says `skip` to it): the
    // asker's next message is a turn of its own, queued behind it
    let after = said(&mut e, &a, &v, 4, "npub1paul", json!({ "text": "never mind" }), T0 + 3_600_005);
    assert!(commands(&after).is_empty() && !e.state().turns.values().any(|t| t.asking), "queued, not told");
}

/// A photo, as a message's record names it.
fn photo(name: &str) -> Value {
    json!({ "sha256": "c".repeat(64), "size": 1, "type": "image/png", "name": name })
}

/// Files `paul` sent juniper in `talk`, as the engine hands them on.
fn attached(a: &Agent, names: &[&str]) -> Option<Attached> {
    Some(Attached { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), by_name: "paul".into(), refs: names.iter().map(|n| serde_json::from_value(photo(n)).expect("a ref")).collect() })
}

/// Goal: a question in words is answered with files too (photos sent as
/// the answer), with words or none: the asker's message is the answer, its
/// files with it for the driver to download and name in its words, never
/// a turn queued behind the question (where it waited out the question's
/// hour). Replay: the message again tells nothing; asked no longer, a
/// message of files is queued as any message is.
#[test]
fn a_question_is_answered_with_files() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "which photo?" }), T0)).expect("started").turn;
    ev(&mut e, Event::Asked { turn: turn.clone() }, T0 + 1);
    let only = said(&mut e, &a, &v, 2, "npub1paul", json!({ "attachments": [photo("a.png"), photo("b.png")] }), T0 + 2);
    assert_eq!(commands(&only), vec![Command::Tell { turn: turn.clone(), seq: 2, by: "npub1paul".into(), by_name: "paul".into(), text: String::new(), attached: attached(&a, &["a.png", "b.png"]) }], "files alone answer it");
    assert!(posts(&only).is_empty() && only.dirty && !e.state().turns[&turn].asking, "no turn of its own");
    assert_eq!(said(&mut e, &a, &v, 2, "npub1paul", json!({ "attachments": [photo("a.png"), photo("b.png")] }), T0 + 3), Step::default(), "replay: nothing");
    ev(&mut e, Event::Asked { turn: turn.clone() }, T0 + 4);
    let both = said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "this one", "attachments": [photo("c.png")] }), T0 + 5);
    assert_eq!(commands(&both), vec![Command::Tell { turn: turn.clone(), seq: 3, by: "npub1paul".into(), by_name: "paul".into(), text: "this one".into(), attached: attached(&a, &["c.png"]) }], "words and files");
    let next = said(&mut e, &a, &v, 4, "npub1paul", json!({ "attachments": [photo("d.png")] }), T0 + 6);
    assert!(commands(&next).is_empty() && started(&next).is_none(), "asked no longer: queued");
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 7);
    assert_eq!(started(&end).map(|t| t.attachments.len()), Some(1), "then run as a turn of its own, its file with it");
}

/// Goal (Paul on production, 2026-10-22: he typed his answer to a card in
/// the message box, and it stayed queued, the card open, for up to the
/// card's hour): while the agent's turn waits on a card that takes words
/// (a clarify's "Other"), its owner's message in the chat answers it, as
/// if typed on the card: the card closes answered with their words (or the
/// names of the files they sent), the turn runs on, and the runtime is
/// given that option and the words, files and all; no turn is queued.
/// Invalid: someone else's message is queued (only the owner answers a
/// card); a card that takes no words (an approval's Allow and Deny) takes a
/// press alone, its owner's message queued behind it. Replay: the message
/// again answers nothing; answered, the card takes no more.
#[test]
fn a_message_answers_a_card_that_takes_words() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "what should I plant?" }), T0)).expect("started").turn;
    let options = || vec![PromptOption { id: "c0".into(), label: "Basil".into(), style: None, words: false }, PromptOption { id: "other".into(), label: "Other".into(), style: None, words: true }];
    let card = |e: &mut Engine, prompt: &str, now: u64| ev(e, Event::Prompt { turn: turn.clone(), prompt: prompt.into(), text: "What should I plant?".into(), options: options(), ttl_ms: None }, now);
    card(&mut e, "q.1", T0 + 1);
    let open = |e: &Engine, prompt: &str| e.state().turns[&turn].prompts.iter().any(|p| p.id == prompt && !p.closed);

    // invalid: someone else's message waits its turn, the card open
    let skyler = said(&mut e, &a, &v, 2, "npub1skyler", json!({ "text": "tomatoes!" }), T0 + 2);
    assert!(commands(&skyler).is_empty() && open(&e, "q.1") && e.state().turns[&turn].phase == Phase::Waiting, "queued, as any message is");

    // valid: its owner's words answer it
    let typed = said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "Tomatoes" }), T0 + 3);
    let closed = |prompt: &str, text: &str| (records::work_id(&turn, &format!("pc:{prompt}")), json!({ "kind": "turn.prompt.closed", "turn": turn, "prompt": prompt, "outcome": "answered", "option": "other", "by": "npub1paul", "text": text }));
    assert_eq!(posts(&typed), vec![closed("q.1", "Tomatoes")], "the card closes with their words; no turn of its own");
    assert_eq!(commands(&typed), vec![Command::Answer { turn: turn.clone(), prompt: "q.1".into(), option: Some("other".into()), seq: 3, by: "npub1paul".into(), words: Some("Tomatoes".into()), attached: None }]);
    assert!(typed.dirty && !open(&e, "q.1") && e.state().turns[&turn].phase == Phase::Running, "the turn runs on");
    assert_eq!(said(&mut e, &a, &v, 3, "npub1paul", json!({ "text": "Tomatoes" }), T0 + 4), Step::default(), "replay: nothing");
    let more = said(&mut e, &a, &v, 4, "npub1paul", json!({ "text": "and basil" }), T0 + 5);
    assert!(commands(&more).is_empty() && posts(&more).is_empty(), "answered, the card takes no more: queued");

    // words and files; then files alone, the card showing their names
    card(&mut e, "q.2", T0 + 6);
    let both = said(&mut e, &a, &v, 5, "npub1paul", json!({ "text": "these", "attachments": [photo("a.png")] }), T0 + 7);
    assert_eq!(posts(&both), vec![closed("q.2", "these")]);
    assert_eq!(commands(&both), vec![Command::Answer { turn: turn.clone(), prompt: "q.2".into(), option: Some("other".into()), seq: 5, by: "npub1paul".into(), words: Some("these".into()), attached: attached(&a, &["a.png"]) }]);
    card(&mut e, "q.3", T0 + 8);
    let only = said(&mut e, &a, &v, 6, "npub1paul", json!({ "attachments": [photo("a.png"), photo("b.png")] }), T0 + 9);
    assert_eq!(posts(&only), vec![closed("q.3", "a.png, b.png")]);
    assert_eq!(commands(&only), vec![Command::Answer { turn: turn.clone(), prompt: "q.3".into(), option: Some("other".into()), seq: 6, by: "npub1paul".into(), words: Some(String::new()), attached: attached(&a, &["a.png", "b.png"]) }]);

    // a card that takes no words takes a press alone
    let approval = vec![PromptOption { id: "once".into(), label: "Allow".into(), style: None, words: false }, PromptOption { id: "deny".into(), label: "Deny".into(), style: None, words: false }];
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p.4".into(), text: "Fill your card?".into(), options: approval, ttl_ms: None }, T0 + 10);
    let yes = said(&mut e, &a, &v, 7, "npub1paul", json!({ "text": "yes go ahead" }), T0 + 11);
    assert!(commands(&yes).is_empty() && posts(&yes).is_empty() && open(&e, "p.4"), "queued, the card open for its press");

    // its end: what waited runs, in order, skyler's first
    let end = ev(&mut e, Event::End { turn: turn.clone(), outcome: Outcome::Idle }, T0 + 12);
    assert_eq!(started(&end).map(|t| t.text), Some("tomatoes!".to_string()));
}

// ---- commands: the runtime's menu, from the agent's owner ----

/// An engine whose runtime takes the scripted agent's commands (one of
/// each way the bridge carries one).
fn commanded(agents: &[Agent], life: &str) -> Engine {
    let mut e = Engine::new(State::default(), Settings { prompt_ttl_ms: 60_000, turn_idle_ms: 600_000 }, life).expect("a fresh state").with_menu(crate::runtime::script::MENU);
    e.step(Input::Agents(agents.to_vec()), T0);
    e.recover(T0);
    e.step(Input::Runtime(Event::Connected(true)), T0);
    e
}

fn cmd(name: &str, args: &str) -> Value {
    json!({ "kind": "command", "command": name, "args": args })
}

/// Goal: the agent's owner's command of the menu is a turn of its own,
/// claimed as a message's is, its text said to the runtime as it is
/// (`/<name> <args>`, `command`); the agent's first turn in a chat in a
/// life posts its menu there once (`cm:<life>` on `work`), before the
/// runtime hears of the turn. Replay: the same record again does nothing.
/// Restart: a command waiting its turn stays one, and the next life's
/// first turn there posts the menu again, as a record of its own.
#[test]
fn a_command_from_the_owner_is_a_turn_of_its_own() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = commanded(std::slice::from_ref(&a), LIFE);
    let s = said(&mut e, &a, &v, 1, "npub1paul", cmd("model", "glm-5"), T0);
    let t = started(&s).expect("handed");
    assert_eq!((t.text.as_str(), t.command, t.asker.as_str()), ("/model glm-5", true, "npub1paul"));
    let menu = records::commands(&a.identity, crate::runtime::script::MENU);
    assert_eq!(posts(&s), vec![(records::work_id(&t.turn, "start"), posts(&s)[0].1.clone()), (records::menu_id(LIFE), menu.clone())], "its claim, then the menu");
    assert_eq!(kinds(&s), vec!["turn.start", "commands"]);
    assert_eq!(said(&mut e, &a, &v, 1, "npub1paul", cmd("model", "glm-5"), T0 + 1), Step::default(), "a replay does nothing");
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 1, text: "ran /model glm-5".into() }, T0 + 2);
    ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0 + 2);
    // the menu once a life in a chat: the next turn posts none
    let usage = said(&mut e, &a, &v, 2, "npub1paul", cmd("usage", ""), T0 + 3);
    assert_eq!((started(&usage).expect("handed").text.as_str(), kinds(&usage)), ("/usage", vec!["turn.start".to_string()]));
    // waiting behind it, a command stays one across a restart
    said(&mut e, &a, &v, 3, "npub1paul", cmd("usage", "again"), T0 + 4);
    let state: State = serde_json::from_str(&serde_json::to_string(e.state()).expect("serializes")).expect("deserializes");
    assert_eq!(&state, e.state());
    let next_life = "fedcba9876543210fedcba9876543210";
    let mut e2 = Engine::new(state, Settings::default(), next_life).expect("whole").with_menu(crate::runtime::script::MENU);
    e2.step(Input::Agents(vec![a.clone()]), T0 + 10);
    e2.recover(T0 + 10);
    let c = e2.step(Input::Runtime(Event::Connected(true)), T0 + 10);
    let s = answered(&mut e2, c, T0 + 10);
    let next = started(&s).expect("the waiting command runs");
    assert_eq!((next.text.as_str(), next.command), ("/usage again", true));
    assert_eq!(posts(&s).iter().filter(|(id, _)| id.starts_with("cm:")).cloned().collect::<Vec<_>>(), vec![(records::menu_id(next_life), menu)], "a new life posts it again");
}

/// Invalid: a command from anyone but the agent's owner, one not on its
/// runtime's menu, one `to` another agent, and any command to an engine
/// whose runtime takes none, start nothing (the cursor passes them).
#[test]
fn a_command_from_anyone_else_or_off_the_menu_starts_nothing() {
    let a = agent("juniper");
    let b = agent("rowan");
    let v = view(&[&a, &b]);
    let mut e = commanded(&[a.clone(), b.clone()], LIFE);
    let cases = [
        ("npub1skyler", cmd("usage", "")),
        ("npub1paul", cmd("update", "")),
        ("npub1paul", cmd("restart", "")),
        ("npub1paul", json!({ "kind": "command", "command": "usage", "to": [b.identity] })),
        ("anon:7", cmd("usage", "")),
        ("npub1juniper", cmd("usage", "")),
    ];
    for (i, (who, body)) in cases.into_iter().enumerate() {
        let seq = i as u64 + 1;
        let s = said(&mut e, &a, &v, seq, who, body.clone(), T0);
        assert!(started(&s).is_none() && posts(&s).is_empty() && commands(&s).is_empty(), "{who} {body}");
        assert_eq!(e.cursor(&a.fragment, "talk--k3x9", "chat"), seq);
    }
    let mut plain = engine(std::slice::from_ref(&a));
    let s = said(&mut plain, &a, &v, 1, "npub1paul", cmd("usage", ""), T0);
    assert!(started(&s).is_none() && posts(&s).is_empty(), "a runtime with no menu takes no command");
    // a message that starts with `/` is a message, never a command
    let s = said(&mut plain, &a, &v, 2, "npub1paul", json!({ "text": "/usage" }), T0);
    assert_eq!(started(&s).map(|t| (t.text, t.command)), Some(("/usage".to_string(), false)));
}

/// Invalid: a command its menu refuses (`/model … --global`) gets both its
/// records, its end saying why, and the runtime never hears it.
#[test]
fn a_refused_command_ends_saying_why() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = commanded(std::slice::from_ref(&a), LIFE);
    let s = said(&mut e, &a, &v, 1, "npub1paul", cmd("model", "glm-5 --GLOBAL"), T0);
    let turn = records::turn_id(&a.fragment, "talk--k3x9", "chat", 1);
    assert!(started(&s).is_none(), "never handed");
    assert_eq!(kinds(&s), vec!["turn.start", "turn.end"]);
    assert_eq!(posts(&s)[1].1, json!({ "kind": "turn.end", "turn": turn, "outcome": "error", "error": "this chat's alone" }));
    // the word alone refuses it, not a word that holds it
    let s = said(&mut e, &a, &v, 2, "npub1paul", cmd("model", "my--globalish-model"), T0 + 1);
    assert_eq!(started(&s).map(|t| t.text), Some("/model my--globalish-model".to_string()));
}

/// Goal: `stop` (How::Stop) stops the agent's running turn in the chat,
/// whoever asked it, and ends its waiting ones there unrun, each with both
/// its records; no turn of its own. Another chat's, and another agent's,
/// are left alone.
#[test]
fn stop_stops_the_running_turn_and_ends_the_waiting() {
    let a = agent("juniper");
    let b = agent("rowan");
    let v = view(&[&a]);
    let mut e = commanded(&[a.clone(), b.clone()], LIFE);
    let running = started(&said(&mut e, &a, &v, 1, "npub1skyler", json!({ "text": "long" }), T0)).expect("started").turn;
    said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "next" }), T0);
    said(&mut e, &a, &v, 3, "npub1skyler", json!({ "text": "after" }), T0);
    let elsewhere = {
        let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: "other--k3x9".into(), record: rec(1, "npub1paul", json!({ "text": "there" })), view: Some(v.clone()), since: 0 }, T0);
        started(&answered(&mut e, s, T0)).expect("started there").turn
    };
    let s = said(&mut e, &a, &v, 4, "npub1paul", cmd("stop", ""), T0 + 1);
    assert_eq!(commands(&s), vec![Command::Stop { turn: running.clone() }], "the running turn, though skyler asked it");
    let waiting: Vec<String> = [2, 3].iter().map(|seq| records::turn_id(&a.fragment, "talk--k3x9", "chat", *seq)).collect();
    let ended: Vec<(String, Value)> = posts(&s).into_iter().filter(|(_, b)| b["kind"] == "turn.end").collect();
    assert_eq!(ended, waiting.iter().map(|t| (records::work_id(t, "end"), json!({ "kind": "turn.end", "turn": t, "outcome": "stopped" }))).collect::<Vec<_>>());
    assert_eq!(kinds(&s), vec!["turn.start", "turn.end", "turn.start", "turn.end"], "each waiting one has both its records, and the command none");
    assert!(e.state().turns.get(&elsewhere).is_some_and(|t| t.phase == Phase::Running), "another chat's turn runs on");
    let after = ev(&mut e, Event::End { turn: running, outcome: Outcome::Stopped }, T0 + 2);
    assert!(started(&after).is_none(), "nothing waited");
    // with nothing running, it stops nothing
    let s = said(&mut e, &a, &v, 5, "npub1paul", cmd("stop", ""), T0 + 3);
    assert!(commands(&s).is_empty() && posts(&s).is_empty());
}

/// Goal: `new` (How::Restart) stops as `stop` does, then is a turn of its
/// own, which runs once the stopped turn ends (a new session for the
/// chat, the runtime's); with nothing running, at once.
#[test]
fn new_stops_then_runs_its_own_turn() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = commanded(std::slice::from_ref(&a), LIFE);
    let running = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "long" }), T0)).expect("started").turn;
    said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "next" }), T0);
    let s = said(&mut e, &a, &v, 3, "npub1paul", cmd("new", ""), T0 + 1);
    assert_eq!(commands(&s), vec![Command::Stop { turn: running.clone() }]);
    assert!(started(&s).is_none(), "it waits for the stopped turn");
    let after = ev(&mut e, Event::End { turn: running, outcome: Outcome::Stopped }, T0 + 2);
    let fresh = started(&after).expect("then it runs");
    assert_eq!((fresh.text.as_str(), fresh.command, fresh.seq), ("/new", true, 3), "the waiting message never runs");
    ev(&mut e, Event::End { turn: fresh.turn, outcome: Outcome::Idle }, T0 + 3);
    let s = said(&mut e, &a, &v, 4, "npub1paul", cmd("new", ""), T0 + 4);
    assert_eq!(started(&s).map(|t| t.text), Some("/new".to_string()), "nothing running: at once");
}

/// Goal: `steer` (How::Steer) goes into the agent's running turn in the
/// chat at once, said beside it (`Command::Aside`), no turn of its own and
/// no record; with none running, its words are a message. `queue`
/// (How::Message) is its words as a message, waiting its turn. `btw`
/// (How::Aside) is said beside whatever runs, running or not. Each with no
/// words is a turn of its own (the runtime says how to use it).
#[test]
fn steer_queue_and_btw() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = commanded(std::slice::from_ref(&a), LIFE);
    let running = started(&said(&mut e, &a, &v, 1, "npub1skyler", json!({ "text": "long" }), T0)).expect("started").turn;
    let s = said(&mut e, &a, &v, 2, "npub1paul", cmd("steer", "use blue"), T0 + 1);
    let id = records::turn_id(&a.fragment, "talk--k3x9", "chat", 2);
    let aside = |turn: Option<String>, id: &str, text: &str| Command::Aside { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), chat_name: "talk--k3x9".into(), simplex: false, turn, id: id.to_string(), by: "npub1paul".into(), by_name: "paul".into(), text: text.into() };
    assert_eq!(commands(&s), vec![aside(Some(running.clone()), &id, "/steer use blue")]);
    assert!(posts(&s).is_empty() && !e.state().turns.contains_key(&id), "no turn of its own");
    let s = said(&mut e, &a, &v, 3, "npub1paul", cmd("btw", "which file?"), T0 + 2);
    assert_eq!(commands(&s), vec![aside(None, &records::turn_id(&a.fragment, "talk--k3x9", "chat", 3), "/btw which file?")]);
    let s = said(&mut e, &a, &v, 4, "npub1paul", cmd("queue", "then tests"), T0 + 3);
    assert!(commands(&s).is_empty(), "it waits behind the running turn");
    let after = ev(&mut e, Event::End { turn: running, outcome: Outcome::Idle }, T0 + 4);
    let queued = started(&after).expect("then it runs");
    assert_eq!((queued.text.as_str(), queued.command, queued.seq), ("then tests", false, 4), "a message, its words alone");
    ev(&mut e, Event::End { turn: queued.turn, outcome: Outcome::Idle }, T0 + 5);
    // nothing running: a steer's words are a message, a btw is said beside
    let s = said(&mut e, &a, &v, 5, "npub1paul", cmd("steer", "use red"), T0 + 6);
    let t = started(&s).expect("a turn of its own");
    assert_eq!((t.text.as_str(), t.command), ("use red", false));
    ev(&mut e, Event::End { turn: t.turn, outcome: Outcome::Idle }, T0 + 7);
    let s = said(&mut e, &a, &v, 6, "npub1paul", cmd("btw", "and now?"), T0 + 8);
    assert!(matches!(&commands(&s)[..], [Command::Aside { turn: None, .. }]) && posts(&s).is_empty());
    // no words: a turn of its own
    for (seq, name) in [(7, "steer"), (8, "queue"), (9, "btw")] {
        let s = said(&mut e, &a, &v, seq, "npub1paul", cmd(name, " "), T0 + 9 + seq);
        let t = started(&s).unwrap_or_else(|| panic!("{name} with no words is a turn"));
        assert_eq!((t.text, t.command), (format!("/{name}"), true));
        ev(&mut e, Event::End { turn: t.turn, outcome: Outcome::Idle }, T0 + 9 + seq);
    }
}

/// Goal: a person's message that quotes another (`reply_to`) hands its
/// turn the seq it quotes (the driver reads the message), kept across a
/// restart while it waits.
#[test]
fn a_quote_rides_along() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let t = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "first" }), T0)).expect("started");
    assert_eq!((t.reply_to, t.quote.clone()), (None, None));
    said(&mut e, &a, &v, 2, "npub1paul", json!({ "text": "that one", "reply_to": 1 }), T0 + 1);
    let saved = serde_json::to_string(e.state()).expect("serializes");
    assert!(saved.contains("\"reply_to\":1"), "{saved}");
    let mut e2 = Engine::new(serde_json::from_str(&saved).expect("deserializes"), Settings::default(), "fedcba9876543210fedcba9876543210").expect("whole");
    e2.step(Input::Agents(vec![a.clone()]), T0 + 2);
    e2.recover(T0 + 2);
    let c = e2.step(Input::Runtime(Event::Connected(true)), T0 + 2);
    let next = started(&answered(&mut e2, c, T0 + 2)).expect("the waiting one runs");
    assert_eq!((next.text.as_str(), next.reply_to, next.quote), ("that one", Some(1), None), "the driver reads the quote");
}

// ---- agents asking each other: the hop the bridge counts, the budget ----

fn rec_at(seq: u64, at: i64, principal: &str, body: Value) -> Record {
    Record { at, ..rec(seq, principal, body) }
}

/// One record of a chat read by each agent's follower there, as the driver
/// feeds it: the turns it started, with their hops.
fn to_all(e: &mut Engine, agents: &[&Agent], v: &ChatView, chat: &str, record: Record, now: u64) -> Vec<(TurnStart, u32)> {
    let mut out = Vec::new();
    for a in agents {
        let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: chat.into(), record: record.clone(), view: Some(v.clone()), since: 0 }, now);
        if let Some(t) = started(&answered(e, s, now)) {
            let hop = e.state().turns[&t.turn].hop;
            out.push((t, hop));
        }
    }
    out
}

/// The turn says `text` and ends: the reply it posts, its end answered (so
/// the turn is let go before anyone reads the reply).
fn reply_and_end(e: &mut Engine, turn: &str, text: &str, now: u64) -> Value {
    ev(e, Event::Reply { turn: turn.into(), part: 1, text: text.into() }, now);
    let end = ev(e, Event::End { turn: turn.into(), outcome: Outcome::Idle }, now);
    assert!(!e.state().turns.contains_key(turn), "let go once its end was answered");
    posts(&end).into_iter().find(|(id, _)| id.starts_with("rp:")).expect("its reply").1
}

/// Two agents told to hand off to each other until something stops them:
/// the turns they ran, with their hops. Through the bridge, each turn's
/// reply names the other (read after its turn was let go); `around`, each
/// turn posts its hand-off itself while it runs, as the CLI or the API
/// does: no `hop`, no `turn`.
fn ping_pong(around: bool) -> Vec<(String, u32)> {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let mut ran = Vec::new();
    let mut next = to_all(&mut e, &[&j, &r], &v, "talk--k3x9", rec_at(1, 10, "npub1paul", json!({ "text": "keep handing off", "to": ["npub1juniper"] })), T0);
    // bounded: each pass reads one more record, and the hop cap ends it
    for seq in 2..20u64 {
        let Some((t, hop)) = next.pop() else { break };
        assert!(next.is_empty(), "one turn a record");
        ran.push((t.agent.name.clone(), hop));
        let other = if t.agent.name == "juniper" { "rowan" } else { "juniper" };
        let by = format!("npub1{}", t.agent.name);
        let text = format!("over to you @{other}");
        next = if around {
            let posted = json!({ "text": text, "to": [format!("npub1{other}")] });
            let started = to_all(&mut e, &[&j, &r], &v, "talk--k3x9", rec_at(seq, 10 + seq as i64, &by, posted), T0 + seq);
            ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0 + seq);
            started
        } else {
            let reply = reply_and_end(&mut e, &t.turn, &text, T0 + seq);
            assert_eq!(reply["to"], json!([format!("npub1{other}")]), "a mention of the other agent hands off");
            to_all(&mut e, &[&j, &r], &v, "talk--k3x9", rec_at(seq, 10 + seq as i64, &by, reply), T0 + seq)
        };
    }
    ran
}

/// A turn just ended counts only for the reply that names it: an agent
/// handed back to in a chat of two, its turn there ended, then asked again
/// by its person elsewhere, asks the other with the CLI from that new turn
/// at one hop, not one past the turn it finished. Invalid: a post naming
/// the ended turn (a reply read late, or a forgery while it runs no turn)
/// counts from it.
#[test]
fn an_ended_turn_counts_only_for_the_reply_that_names_it() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let (talk, pair) = ("talk--k3x9", "juniper-rowan--k3x9");
    // in the pair chat: the person to juniper, juniper to rowan, rowan back to juniper
    let j0 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(1, 10, "npub1paul", json!({ "text": "start", "to": ["npub1juniper"] })), T0);
    let to_r = reply_and_end(&mut e, &j0[0].0.turn, "@rowan yours", T0 + 1);
    let r1 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(2, 11, "npub1juniper", to_r), T0 + 2);
    let to_j = reply_and_end(&mut e, &r1[0].0.turn, "@juniper back to you", T0 + 3);
    let j2 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(3, 12, "npub1rowan", to_j), T0 + 4);
    assert_eq!(j2[0].1, 2);
    let j2_turn = j2[0].0.turn.clone();
    reply_and_end(&mut e, &j2_turn, "thanks", T0 + 5);
    // its person asks it again in talk: a turn at hop 0 there
    let t0 = to_all(&mut e, &[&j, &r], &v, talk, rec_at(1, 13, "npub1paul", json!({ "text": "ask rowan again", "to": ["npub1juniper"] })), T0 + 6);
    assert_eq!(t0[0].1, 0);
    // from it, `fragment ask` into the pair chat: one hop, not three
    let asked = to_all(&mut e, &[&j, &r], &v, pair, rec_at(4, 14, "npub1juniper", json!({ "text": "and now?", "to": ["npub1rowan"] })), T0 + 7);
    assert_eq!(asked.iter().map(|(t, h)| (t.agent.name.as_str(), *h)).collect::<Vec<_>>(), vec![("rowan", 1)]);
    e.step(Input::Runtime(Event::End { turn: asked[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + 8);
    e.step(Input::Runtime(Event::End { turn: t0[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + 8);
    // naming the ended hop-2 turn (in no turn now) counts from it
    let named = to_all(&mut e, &[&j, &r], &v, pair, rec_at(5, 15, "npub1juniper", json!({ "text": "late", "to": ["npub1rowan"], "turn": j2_turn })), T0 + 9);
    assert_eq!(named[0].1, limits::HOPS_MAX, "one past the turn it names");
}

/// Goal (decision 8): two agents that hand off to each other stop at the
/// hop cap: A, B, A, B, and the fourth hand-off starts nothing. Invalid:
/// the same loop posted around the bridge (no `hop`, no `turn`: the CLI,
/// the API) stops at the same place, because the answering bridge counts
/// the hops from the turns it runs, not from what a record claims.
#[test]
fn a_hand_off_loop_stops_at_the_cap() {
    let want: Vec<(String, u32)> = vec![("juniper".into(), 0), ("rowan".into(), 1), ("juniper".into(), 2), ("rowan".into(), limits::HOPS_MAX)];
    assert_eq!(ping_pong(false), want, "through the bridge");
    assert_eq!(ping_pong(true), want, "around it: a reset that no longer resets");
}

/// Goal: a post an agent of this computer makes around the bridge counts
/// from the turn it is in. Valid: from its turn here, one past it; from a
/// turn in another chat (`fragment ask` into a chat of two agents), one past
/// that. Invalid: a claimed `hop: 0` from deep in a chain changes nothing,
/// and a claim deeper than the bridge's count is kept. Out of any turn
/// (something it left running), it is the last hop: answered once, and the
/// answer hands on nothing.
#[test]
fn a_post_around_the_bridge_counts_from_its_turn() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let talk = "talk--k3x9";
    let pair = "juniper-rowan--k3x9";
    // juniper runs a turn in talk at hop 0 (its person asked)
    let t = to_all(&mut e, &[&j, &r], &v, talk, rec_at(1, 10, "npub1paul", json!({ "text": "ask rowan for me", "to": ["npub1juniper"] })), T0);
    let jt = t[0].0.turn.clone();
    // ... and asks rowan in their own chat with the CLI: no hop, no turn
    let asked = to_all(&mut e, &[&j, &r], &v, pair, rec_at(1, 11, "npub1juniper", json!({ "text": "what's the weather?", "to": ["npub1rowan"] })), T0 + 1);
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].0.agent.name.as_str(), asked[0].1), ("rowan", 1), "one past juniper's turn in talk");
    // a claim of hop 0 from a deep turn resets nothing
    e.step(Input::Runtime(Event::End { turn: jt, outcome: Outcome::Idle }), T0 + 2);
    let rt = asked[0].0.turn.clone();
    reply_and_end(&mut e, &rt, "sunny", T0 + 3);
    let deep = to_all(&mut e, &[&j, &r], &v, pair, rec_at(2, 12, "npub1paul", json!({ "text": "@juniper and @rowan, chat", "to": ["npub1juniper", "npub1rowan"] })), T0 + 4);
    assert_eq!(deep.iter().map(|(t, h)| (t.agent.name.as_str(), *h)).collect::<Vec<_>>(), vec![("juniper", 0), ("rowan", 0)], "a person's message is hop 0");
    // drive juniper to hop 3 in the pair chat: rowan's turn hands to juniper twice
    let (jt0, rt0) = (deep[0].0.turn.clone(), deep[1].0.turn.clone());
    e.step(Input::Runtime(Event::End { turn: jt0, outcome: Outcome::Idle }), T0 + 5);
    let to_j = reply_and_end(&mut e, &rt0, "@juniper yours", T0 + 6);
    let j1 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(3, 13, "npub1rowan", to_j), T0 + 7);
    assert_eq!(j1[0].1, 1);
    let to_r = reply_and_end(&mut e, &j1[0].0.turn, "@rowan back", T0 + 8);
    let r2 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(4, 14, "npub1juniper", to_r), T0 + 9);
    assert_eq!(r2[0].1, 2);
    ev(&mut e, Event::Reply { turn: r2[0].0.turn.clone(), part: 1, text: "@juniper last".into() }, T0 + 10);
    let end = ev(&mut e, Event::End { turn: r2[0].0.turn.clone(), outcome: Outcome::Idle }, T0 + 10);
    let to_j = posts(&end).into_iter().find(|(id, _)| id.starts_with("rp:")).expect("its reply").1;
    let j3 = to_all(&mut e, &[&j, &r], &v, pair, rec_at(5, 15, "npub1rowan", to_j), T0 + 11);
    assert_eq!(j3[0].1, limits::HOPS_MAX);
    // juniper, in its hop-3 turn, posts to rowan with the API, claiming 0
    let reset = to_all(&mut e, &[&j, &r], &v, pair, rec_at(6, 16, "npub1juniper", json!({ "text": "again?", "to": ["npub1rowan"], "hop": 0 })), T0 + 12);
    assert!(reset.is_empty(), "past the cap whatever it claims: {reset:?}");
    // a claim deeper than the count is kept
    let claimed = to_all(&mut e, &[&j, &r], &v, pair, rec_at(7, 17, "npub1juniper", json!({ "text": "deep", "to": ["npub1rowan"], "hop": 9 })), T0 + 13);
    assert!(claimed.is_empty());
    e.step(Input::Runtime(Event::End { turn: j3[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + 14);

    // in no turn, long after its last one here: the last hop, once
    let later = T0 + 14 + limits::ENDED_HOPS_MS + 1;
    let stray = to_all(&mut e, &[&j, &r], &v, pair, rec_at(8, 18, "npub1juniper", json!({ "text": "from a loop I left running", "to": ["npub1rowan"] })), later);
    assert_eq!(stray.len(), 1);
    assert_eq!(stray[0].1, limits::HOPS_MAX, "answered once");
    let back = reply_and_end(&mut e, &stray[0].0.turn, "@juniper ok", later + 1);
    assert!(to_all(&mut e, &[&j, &r], &v, pair, rec_at(9, 19, "npub1rowan", back), later + 2).is_empty(), "its answer hands on nothing");
}

/// Restart: what the bridge knew of its ended turns goes with its life, so
/// a reply the next life reads first is the last hop (answered, handing on
/// nothing), never a reset to the first.
#[test]
fn a_reply_read_by_the_next_life_resets_nothing() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let t = to_all(&mut e, &[&j, &r], &v, "talk--k3x9", rec_at(1, 10, "npub1paul", json!({ "text": "hi", "to": ["npub1juniper"] })), T0);
    let reply = reply_and_end(&mut e, &t[0].0.turn, "@rowan over to you", T0 + 1);
    // juniper's follower read it (its own); the life ends before rowan's did
    e.step(Input::Record { agent: j.fragment.clone(), fragment: "talk--k3x9".into(), record: rec_at(2, 11, "npub1juniper", reply.clone()), view: Some(v.clone()), since: 0 }, T0 + 2);
    let saved = e.state().clone();
    let mut e2 = Engine::new(saved, Settings::default(), "fedcba9876543210fedcba9876543210").expect("whole");
    e2.step(Input::Agents(vec![j.clone(), r.clone()]), T0 + 3);
    e2.recover(T0 + 3);
    e2.step(Input::Runtime(Event::Connected(true)), T0 + 3);
    let s = e2.step(Input::Record { agent: r.fragment.clone(), fragment: "talk--k3x9".into(), record: rec_at(2, 11, "npub1juniper", reply), view: Some(v.clone()), since: 0 }, T0 + 4);
    let rt = started(&answered(&mut e2, s, T0 + 4)).expect("rowan answers it");
    assert_eq!(e2.state().turns[&rt.turn].hop, limits::HOPS_MAX);
}

/// Goal: a chat's agents start at most AGENT_TURNS_PER_CHAT_MAX turns of
/// each other in the window, and the one past it is refused with both its
/// records, its end saying why (the chat shows it). A person is never
/// counted, nor refused. Replay: a record read again spends nothing.
/// Restart: the count is kept. The window slides by the records' times.
#[test]
fn a_chats_agents_have_a_budget() {
    let (j, r) = (agent("juniper"), agent("rowan"));
    let v = view(&[&j, &r]);
    let mut e = engine(&[j.clone(), r.clone()]);
    let max = limits::AGENT_TURNS_PER_CHAT_MAX as u64;
    let at0 = 1_000_000i64;
    // rowan, in no turn, asks juniper again and again (each the last hop)
    let ask = |seq: u64, at: i64| rec_at(seq, at, "npub1rowan", json!({ "text": "again", "to": ["npub1juniper"] }));
    for seq in 1..=max {
        let s = to_all(&mut e, &[&j], &v, "talk--k3x9", ask(seq, at0 + seq as i64), T0 + seq);
        assert_eq!(s.len(), 1, "within the budget: {seq}");
        e.step(Input::Runtime(Event::End { turn: s[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + seq);
    }
    assert_eq!(e.state().agent_turns["talk--k3x9"].len() as u64, max);
    // replay: the last one read again spends nothing
    assert!(to_all(&mut e, &[&j], &v, "talk--k3x9", ask(max, at0 + max as i64), T0 + max).is_empty());
    assert_eq!(e.state().agent_turns["talk--k3x9"].len() as u64, max);
    // restart: the count is kept
    let saved: State = serde_json::from_str(&serde_json::to_string(e.state()).expect("serializes")).expect("deserializes");
    let mut e = Engine::new(saved, Settings::default(), "fedcba9876543210fedcba9876543210").expect("whole");
    e.step(Input::Agents(vec![j.clone(), r.clone()]), T0 + 100);
    e.recover(T0 + 100);
    e.step(Input::Runtime(Event::Connected(true)), T0 + 100);
    // past it: refused, with both records, saying why
    let over = e.step(Input::Record { agent: j.fragment.clone(), fragment: "talk--k3x9".into(), record: ask(max + 1, at0 + max as i64 + 1), view: Some(v.clone()), since: 0 }, T0 + 101);
    let over = answered(&mut e, over, T0 + 101);
    assert!(started(&over).is_none());
    assert_eq!(kinds(&over), vec!["turn.start", "turn.end"]);
    assert_eq!(posts(&over)[1].1["error"], refused_budget());
    // a person is never counted, nor refused
    let person = to_all(&mut e, &[&j], &v, "talk--k3x9", rec_at(max + 2, at0 + max as i64 + 2, "npub1paul", json!({ "text": "still there?" })), T0 + 102);
    assert_eq!(person.len(), 1);
    assert_eq!(e.state().agent_turns["talk--k3x9"].len() as u64, max);
    e.step(Input::Runtime(Event::End { turn: person[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + 102);
    // another chat has a budget of its own
    let other = to_all(&mut e, &[&j], &v, "other--k3x9", ask(1, at0 + max as i64 + 3), T0 + 103);
    assert_eq!(other.len(), 1);
    e.step(Input::Runtime(Event::End { turn: other[0].0.turn.clone(), outcome: Outcome::Idle }), T0 + 103);
    // the window slides: once the first is older than it, one more runs
    let later = at0 + 1 + limits::AGENT_TURNS_WINDOW_MS;
    let slid = to_all(&mut e, &[&j], &v, "talk--k3x9", ask(max + 3, later), T0 + 104);
    assert_eq!(slid.len(), 1, "one ran out of the window");
    assert_eq!(e.state().agent_turns["talk--k3x9"].len() as u64, max, "and is let go");
    assert!(e.state().agent_turns["talk--k3x9"].iter().all(|t| *t > later - limits::AGENT_TURNS_WINDOW_MS));
}

/// Invalid state: a budget out of order or past its bound, or a routine at
/// a hop, is refused at load.
#[test]
fn a_corrupt_budget_is_refused() {
    let mut s = State::default();
    s.agent_turns.insert("talk--k3x9".into(), vec![3, 2]);
    assert!(Engine::new(s, Settings::default(), LIFE).is_err(), "out of order");
    let mut s = State::default();
    s.agent_turns.insert("talk--k3x9".into(), (0..=limits::AGENT_TURNS_PER_CHAT_MAX as i64).collect());
    assert!(Engine::new(s, Settings::default(), LIFE).is_err(), "past the bound");
    let mut s = State::default();
    s.agent_turns.insert("talk--k3x9".into(), vec![]);
    assert!(Engine::new(s, Settings::default(), LIFE).is_err(), "an empty one is never kept");
    // a state from before the budget loads, with none
    let mut old = serde_json::to_value(State::default()).expect("serializes");
    old.as_object_mut().expect("an object").remove("agent_turns");
    let old: State = serde_json::from_value(old).expect("an earlier bridge's state loads");
    assert!(old.agent_turns.is_empty());
}

/// Goal: an agent's `tasks` hears its own fragment (its cron, by the
/// fragment's key, whose npub is the agent's identity) and its owner.
/// Invalid: another agent, acting for the owner (who may post there),
/// starts no routine and announces no join.
#[test]
fn tasks_hear_only_the_owner_and_the_fragment() {
    let a = agent("juniper");
    let mut e = engine(std::slice::from_ref(&a));
    let task = |seq, principal: &str, body| Record { channel: "tasks".into(), seq, at: 0, principal: principal.into(), kind: "message".into(), body };
    let routine = json!({ "kind": "routine", "text": "water the plants", "chat": "talk--k3x9" });
    let feed = |e: &mut Engine, r: Record| {
        let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: a.fragment.clone(), record: r, view: None, since: 0 }, T0);
        answered(e, s, T0)
    };
    let cron = feed(&mut e, task(1, &a.identity, routine.clone()));
    let t = started(&cron).expect("its cron's routine runs");
    assert_eq!(t.asker, "npub1paul", "asked by its owner");
    e.step(Input::Runtime(Event::End { turn: t.turn, outcome: Outcome::Idle }), T0);
    let rowan = feed(&mut e, task(2, "npub1rowan", routine.clone()));
    assert!(rowan.effects.is_empty(), "another agent starts no routine: {:?}", rowan.effects);
    assert_eq!(e.cursor(&a.fragment, &a.fragment, "tasks"), 2, "the cursor passes it");
    let joined = feed(&mut e, task(3, "npub1rowan", json!({ "kind": "joined", "fragment": "x--k3x9" })));
    assert!(joined.effects.is_empty());
    let owner = feed(&mut e, task(4, "npub1paul", routine));
    assert!(started(&owner).is_some(), "its owner's runs");
}

// ---- asks (docs/chat-records.md, "An ask") ----

/// The chat juniper asks from, where its answer's turn runs.
const HOME: &str = "home--k3x9";

/// A record of `fragment`'s `channel`, read as `a`, its turns' claims and
/// owed records answered.
fn read(e: &mut Engine, a: &Agent, fragment: &str, record: Record, v: Option<&ChatView>, now: u64) -> Step {
    let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: fragment.into(), record, view: v.cloned(), since: 0 }, now);
    answered(e, s, now)
}

/// juniper's question to rowan in talk, answered in `HOME`.
fn question(seq: u64, text: &str) -> Record {
    rec(seq, "npub1juniper", json!({ "text": text, "to": ["npub1rowan"], "return_to": HOME }))
}

/// The answers a step owes: as whom, on which fragment, and the body
/// (each under its `answer_id`, on `tasks`).
fn answers(s: &Step) -> Vec<(String, String, Value)> {
    s.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Owed { agent, fragment, channel, id, body, .. } if *channel == records::TASKS => {
                assert_eq!(id, &records::answer_id(body["turn"].as_str().expect("an answer names its turn")));
                Some((agent.clone(), fragment.clone(), body.clone()))
            }
            _ => None,
        })
        .collect()
}

/// An answer, as its asking agent's `tasks` holds it.
fn answer_task(seq: u64, principal: &str, body: Value) -> Record {
    Record { channel: "tasks".into(), seq, at: 0, principal: principal.into(), kind: "message".into(), body }
}

/// Goal: an agent's question to another, with a chat to return to, is a
/// turn of the asked agent like any message; once it is over, the asked
/// agent's turn owes its answer as the asking agent, on that agent's own
/// `tasks`: what it said (not its narration), the files it sent, how it
/// ended, one hop past it. That record is a turn of the asking agent in the
/// chat it asked from, its owner's business there (Stop and questions go to
/// them), told by the agent that answered. Its own answers carry it no
/// deeper than the chain allows.
#[test]
fn a_question_is_answered_back_where_it_was_asked() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let mut talk = view(&[&rowan, &juniper]);
    talk.names.insert("npub1rowan".into(), "Rowan".into());
    let home = view(&[&juniper]);
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    // paul asks juniper at home (hop 0); while it runs, it asks rowan
    let asking = started(&read(&mut e, &juniper, HOME, rec(1, "npub1paul", json!({ "text": "ask rowan" })), Some(&home), T0)).expect("juniper's turn").turn;
    let s = read(&mut e, &rowan, "talk--k3x9", question(1, "What is the weather?"), Some(&talk), T0 + 1);
    let t = started(&s).expect("rowan's turn");
    assert_eq!((t.asker.as_str(), t.text.as_str()), ("npub1juniper", "What is the weather?"), "a message like any");
    assert_eq!(e.state().turns[&t.turn].hop, 1);
    assert_eq!(e.state().turns[&t.turn].ret, Some(Return { agent: juniper.fragment.clone(), chat: HOME.into(), name: "Rowan".into(), said: String::new(), files: 0 }));
    // juniper's turn ends: the answer needs no one waiting
    ev(&mut e, Event::End { turn: asking, outcome: Outcome::Idle }, T0 + 2);

    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 1, text: "Let me look.".into() }, T0 + 3);
    let step = ToolStep { tool: "search".into(), args: "weather".into(), ok: true, excerpt: String::new(), category: crate::records::Category::Web, evidence: None };
    ev(&mut e, Event::Step { turn: t.turn.clone(), step }, T0 + 4);
    let file = LocalFile { path: "/data/map.png".into(), media_type: "image/png".into(), name: "map.png".into(), size: 9 };
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 2, text: "Sunny, 21°C.".into() }, T0 + 5);
    ev(&mut e, Event::Attachment { turn: t.turn.clone(), part: 2, file }, T0 + 5);
    let end = ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0 + 6);
    let owed = answers(&end);
    assert_eq!(owed.len(), 1, "one answer: {:?}", end.effects);
    let (by, on, body) = &owed[0];
    assert_eq!((by.as_str(), on.as_str()), (juniper.fragment.as_str(), juniper.fragment.as_str()), "posted as juniper, on its own tasks");
    let text = "Rowan answered your question in talk--k3x9 (\"What is the weather?\"):\n\nSunny, 21°C.\n\n(Rowan sent 1 file there.)";
    assert_eq!(*body, json!({ "kind": "answer", "chat": HOME, "text": text, "by": "npub1rowan", "name": "Rowan", "asked": { "fragment": "talk--k3x9", "seq": 1 }, "turn": t.turn, "outcome": "idle", "hop": 2 }), "called by its name, as its chat shows it");
    assert!(e.state().turns.is_empty(), "let go once its end and its answer are posted");

    // juniper's bridge reads it: a turn at home, its owner's, told by rowan
    let a = read(&mut e, &juniper, &juniper.fragment, answer_task(1, &juniper.identity, body.clone()), None, T0 + 7);
    let woke = started(&a).expect("juniper's answer turn");
    assert_eq!((woke.fragment.as_str(), woke.text.as_str(), woke.asker.as_str(), woke.asker_name.as_str()), (HOME, text, "npub1paul", "Rowan"));
    assert_eq!(woke.turn, records::turn_id(&juniper.fragment, &juniper.fragment, "tasks", 1));
    assert_eq!(e.state().turns[&woke.turn].hop, 2, "no shallower than the turn that answered");
    assert!(e.state().agent_turns.contains_key(HOME), "it spends its chat's budget");

    // asked again from that turn: one hop deeper, still within the chain
    let s = read(&mut e, &rowan, "talk--k3x9", question(2, "And tomorrow?"), Some(&talk), T0 + 8);
    let t2 = started(&s).expect("rowan answers a follow-up");
    assert_eq!(e.state().turns[&t2.turn].hop, 3);
    ev(&mut e, Event::End { turn: woke.turn, outcome: Outcome::Idle }, T0 + 9);
    let end = ev(&mut e, Event::Reply { turn: t2.turn.clone(), part: 1, text: "Rain.".into() }, T0 + 10);
    assert!(answers(&end).is_empty(), "an answer waits for its turn's end");
    let end = ev(&mut e, Event::End { turn: t2.turn.clone(), outcome: Outcome::Idle }, T0 + 11);
    let deep = answers(&end).remove(0).2;
    assert_eq!(deep["hop"], limits::HOPS_MAX, "held at the last hop");
    let a = read(&mut e, &juniper, &juniper.fragment, answer_task(2, &juniper.identity, deep), None, T0 + 12);
    started(&a).expect("its answer still wakes juniper");
    // and from there, no further: a question past the chain is refused,
    // and answered so, as a turn at home that never runs, so nothing asks
    // again, and its owner sees why where it asked
    let s = read(&mut e, &rowan, "talk--k3x9", question(3, "And the day after?"), Some(&talk), T0 + 13);
    assert!(started(&s).is_none(), "no turn of rowan's runs: {:?}", s.effects);
    assert_eq!(kinds(&s), vec!["turn.start", "turn.end", "answer"]);
    let refused = answers(&s).remove(0).2;
    assert_eq!((&refused["refused"], &refused["error"]), (&json!(true), &json!(refused_deep())));
    let a = read(&mut e, &juniper, &juniper.fragment, answer_task(3, &juniper.identity, refused), None, T0 + 14);
    assert!(started(&a).is_none() && commands(&a).is_empty(), "juniper's runtime hears nothing: {:?}", a.effects);
    assert_eq!(kinds(&a), vec!["turn.start", "turn.end"], "a turn at home, refused");
    let home: Vec<&str> = a.effects.iter().filter_map(|x| match x { Effect::Owed { fragment, .. } => Some(fragment.as_str()), _ => None }).collect();
    assert_eq!(home, vec![HOME, HOME]);
    assert!(posts(&a)[1].1["error"].as_str().is_some_and(|e| e.starts_with(&format!("Rowan wasn't asked: {}. (Your question in talk--k3x9: \"And the day after?\")", refused_deep()))), "the reason first: {:?}", posts(&a));
}

/// Goal: an answer says how the asked turn ended, and a turn is never run
/// again for a better one: one another life claimed is answered lost; one
/// cut by a restart is answered lost; one stopped while it waited, stopped;
/// one its agent may not run there (its claim refused, or it left the
/// chat), that it may not answer. A question refused at admission (its
/// chat's queue full, past the budget, past the hops) is answered refused:
/// a turn of its asker's that never runs, so no answer re-asks.
#[test]
fn every_end_of_an_asked_turn_answers() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let talk = view(&[&rowan, &juniper]);
    let outcome = |s: &Step| answers(s).first().map(|(_, _, b)| (b["outcome"].clone(), b["error"].clone()));

    // another life's claim
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let s = e.step(Input::Record { agent: rowan.fragment.clone(), fragment: "talk--k3x9".into(), record: question(1, "q"), view: Some(talk.clone()), since: 0 }, T0);
    let turn = s.effects.iter().find_map(|x| match x { Effect::Claim { turn, .. } => Some(turn.clone()), _ => None }).expect("a claim");
    let s = e.step(Input::Claimed { turn: turn.clone(), answer: ClaimAnswer::Theirs }, T0);
    assert_eq!(outcome(&s), Some((json!("error"), json!(LOST))));

    // a restart's cut
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let t = started(&read(&mut e, &rowan, "talk--k3x9", question(1, "q"), Some(&talk), T0)).expect("running");
    let mut again = Engine::new(e.state().clone(), Settings::default(), LIFE).expect("its state is whole");
    again.step(Input::Agents(vec![rowan.clone(), juniper.clone()]), T0 + 1);
    let r = again.recover(T0 + 1);
    assert_eq!(outcome(&r), Some((json!("error"), json!(LOST))), "{t:?}");

    // stopped while it waited; its claim refused; its agent gone from the chat
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    read(&mut e, &rowan, "talk--k3x9", rec(1, "npub1paul", json!({ "text": "busy" })), Some(&talk), T0);
    let waiting = records::turn_id(&rowan.fragment, "talk--k3x9", "chat", 2);
    read(&mut e, &rowan, "talk--k3x9", question(2, "q"), Some(&talk), T0);
    let s = read(&mut e, &rowan, "talk--k3x9", rec(3, "npub1juniper", json!({ "kind": "stop", "turn": waiting })), Some(&talk), T0);
    assert_eq!(outcome(&s), Some((json!("stopped"), Value::Null)));
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let s = e.step(Input::Record { agent: rowan.fragment.clone(), fragment: "talk--k3x9".into(), record: question(1, "q"), view: Some(talk.clone()), since: 0 }, T0);
    let turn = s.effects.iter().find_map(|x| match x { Effect::Claim { turn, .. } => Some(turn.clone()), _ => None }).expect("a claim");
    let s = e.step(Input::Claimed { turn, answer: ClaimAnswer::Refused }, T0);
    assert_eq!(outcome(&s), Some((json!("error"), json!("it may not answer in talk--k3x9"))));
    assert!(posts(&s).iter().all(|(id, _)| id.starts_with("an:")), "nothing on a chat it may not post in");
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    read(&mut e, &rowan, "talk--k3x9", question(1, "q"), Some(&talk), T0);
    let s = e.step(Input::Gone { agent: rowan.fragment.clone(), fragment: "talk--k3x9".into() }, T0);
    assert_eq!(outcome(&s), Some((json!("error"), json!("it is no longer in talk--k3x9"))));
    let s = answered(&mut e, s, T0);
    assert!(e.state().turns.is_empty(), "{:?}", s.effects);

    // refused at admission: its records, and an answer that says so
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    read(&mut e, &rowan, "talk--k3x9", rec(1, "npub1paul", json!({ "text": "busy" })), Some(&talk), T0);
    for seq in 2..2 + limits::QUEUED_PER_CHAT_MAX as u64 {
        read(&mut e, &rowan, "talk--k3x9", rec(seq, "npub1paul", json!({ "text": "waiting" })), Some(&talk), T0);
    }
    let s = read(&mut e, &rowan, "talk--k3x9", question(100, "q"), Some(&talk), T0);
    assert_eq!(kinds(&s), vec!["turn.start", "turn.end", "answer"], "refused, and said so");
    assert_eq!((&answers(&s)[0].2["refused"], &answers(&s)[0].2["error"]), (&json!(true), &json!(REFUSED_QUEUED)));
}

/// Goal (#717's re-review): an ask's turn (one with a return) that asks
/// back in words is told at once, in words its runtime's question takes,
/// that no answer is coming, and goes on: it is not ended (a runtime held
/// in a clarify lets go of words alone), and its answer is what it then
/// says. Invalid: a plain hand-off's turn (no return) waits as before.
#[test]
fn an_asks_turn_that_asks_back_is_told_to_go_on() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let mut talk = view(&[&rowan, &juniper]);
    talk.names.insert("npub1juniper".into(), "Juniper".into());
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let t = started(&read(&mut e, &rowan, "talk--k3x9", question(1, "Which garden?"), Some(&talk), T0)).expect("rowan's turn");
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 1, text: "Which one do you mean?".into() }, T0 + 1);
    let s = ev(&mut e, Event::Asked { turn: t.turn.clone() }, T0 + 2);
    assert_eq!(commands(&s), vec![Command::Tell { turn: t.turn.clone(), seq: ASKED_BACK_SEQ, by: "npub1juniper".into(), by_name: "Juniper".into(), text: ASKED_BACK.into(), attached: None }], "told to go on, in words");
    assert!(!kinds(&s).contains(&"turn.asked".to_string()), "waiting for no one: {:?}", s.effects);
    assert!(e.state().turns[&t.turn].active() && !e.state().turns[&t.turn].asking, "still running");
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 2, text: "The back one, I think.".into() }, T0 + 3);
    let end = ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0 + 4);
    assert!(answers(&end)[0].2["text"].as_str().is_some_and(|a| a.ends_with("Which one do you mean?\n\nThe back one, I think.")), "its answer is what it said: {:?}", answers(&end));

    // a plain hand-off (no return): it waits for its asker, as before
    let p = started(&read(&mut e, &rowan, "talk--k3x9", rec(2, "npub1juniper", json!({ "text": "Which garden?", "to": ["npub1rowan"] })), Some(&talk), T0 + 5)).expect("the hand-off's turn");
    ev(&mut e, Event::Reply { turn: p.turn.clone(), part: 1, text: "Which one?".into() }, T0 + 6);
    let s = ev(&mut e, Event::Asked { turn: p.turn.clone() }, T0 + 7);
    assert!(commands(&s).is_empty() && e.state().turns[&p.turn].asking, "{:?}", s.effects);
    assert!(posts(&s).iter().any(|(_, b)| b["kind"] == "turn.asked" && b["asks"] == "npub1juniper"));
}

/// Goal: a state holding an ask's return is format 3, which a bridge
/// before asks sets aside rather than refusing (its `load` sets aside any
/// version but 2, and its `check` would refuse an owed answer: a crash
/// loop); one holding none is format 2, as an older bridge wrote and
/// reads, so a rollback past asks keeps it. Both load here.
#[test]
fn a_state_owing_an_answer_is_one_an_older_bridge_sets_aside() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let talk = view(&[&rowan, &juniper]);
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    assert_eq!(e.state().version, 2, "no return: the older format");
    let t = started(&read(&mut e, &rowan, "talk--k3x9", question(1, "q"), Some(&talk), T0)).expect("running");
    assert_eq!(e.state().version, STATE_VERSION, "a return held: format 3");
    let end = e.step(Input::Runtime(Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }), T0);
    let owing = serde_json::to_value(e.state()).unwrap();
    assert!(owing["turns"][&t.turn]["owed"].as_array().unwrap().iter().any(|o| o["id"].as_str().is_some_and(|i| i.starts_with("an:"))), "it owes its answer");
    // the older bridge's load: a version but its own (2) is set aside
    assert_ne!(owing["version"], json!(2), "an older bridge sets it aside, never checks it");
    let dir = std::env::temp_dir().join(format!("bridge-state-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(crate::driver::state_path(&dir), owing.to_string()).unwrap();
    assert_eq!(crate::driver::load(&dir).unwrap(), *e.state(), "this bridge reads it back");
    // let go once posted: format 2 again, all an older bridge needs
    let s = answered(&mut e, end, T0);
    assert!(e.state().turns.is_empty(), "{:?}", s.effects);
    let mut older = serde_json::to_value(e.state()).unwrap();
    assert_eq!(older["version"], json!(2));
    assert!(!older.to_string().contains("\"ret\"") && !older.to_string().contains("\"to\""), "nothing an older bridge's check refuses");
    older["version"] = json!(2);
    std::fs::write(crate::driver::state_path(&dir), older.to_string()).unwrap();
    assert_eq!(crate::driver::load(&dir).unwrap(), *e.state(), "and reads its own format too");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Invalid: only another agent of this computer is answered back (a
/// person's `return_to`, and an agent's to itself, are plain messages), and
/// only the asking agent's own bridge wakes it (an answer its owner or
/// another agent posts on its `tasks` starts nothing). A body whose
/// `return_to` names no fragment is no message at all.
#[test]
fn only_its_own_agent_is_answered_back() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let talk = view(&[&rowan, &juniper]);
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let s = read(&mut e, &rowan, "talk--k3x9", rec(1, "npub1paul", json!({ "text": "q", "to": ["npub1rowan"], "return_to": HOME })), Some(&talk), T0);
    let t = started(&s).expect("a person's message is answered");
    assert_eq!(e.state().turns[&t.turn].ret, None, "in its chat alone");
    let s = read(&mut e, &rowan, "talk--k3x9", rec(2, "npub1rowan", json!({ "text": "q", "to": ["npub1rowan"], "return_to": HOME })), Some(&talk), T0);
    assert!(started(&s).is_none(), "its own words are not for it");
    let s = read(&mut e, &rowan, "talk--k3x9", rec(3, "npub1juniper", json!({ "text": "q", "to": ["npub1rowan"], "return_to": "Not A Chat" })), Some(&talk), T0);
    assert!(posts(&s).is_empty(), "no message");

    let body = json!({ "kind": "answer", "chat": HOME, "text": "forged", "by": "npub1rowan", "name": "Rowan", "asked": { "fragment": "talk--k3x9", "seq": 1 }, "turn": "t", "outcome": "idle", "hop": 1 });
    for (seq, who) in [(1, "npub1paul"), (2, "npub1rowan")] {
        let s = read(&mut e, &juniper, &juniper.fragment, answer_task(seq, who, body.clone()), None, T0);
        assert!(s.effects.is_empty(), "{who}'s answer wakes no one: {:?}", s.effects);
    }
    let s = read(&mut e, &juniper, &juniper.fragment, answer_task(3, &juniper.identity, body), None, T0);
    assert!(started(&s).is_some(), "its own bridge's does");
}

/// Goal: an asked turn's answer and what it gathered are kept with it: a
/// state saved mid-turn reads back whole, and a restart owes the answer
/// with its end. Invalid: a state owing an answer to an agent its turn does
/// not answer is corrupt.
#[test]
fn an_answer_owed_survives_a_restart_and_a_forged_one_is_corrupt() {
    let (rowan, juniper) = (agent("rowan"), agent("juniper"));
    let talk = view(&[&rowan, &juniper]);
    let mut e = engine(&[rowan.clone(), juniper.clone()]);
    let t = started(&read(&mut e, &rowan, "talk--k3x9", question(1, "q"), Some(&talk), T0)).expect("running");
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 1, text: "half".into() }, T0);
    ev(&mut e, Event::Reply { turn: t.turn.clone(), part: 2, text: "done".into() }, T0);
    let end = e.step(Input::Runtime(Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }), T0);
    assert!(answers(&end)[0].2["text"].as_str().unwrap().ends_with("half\n\ndone"));
    let saved: State = serde_json::from_str(&serde_json::to_string(e.state()).unwrap()).unwrap();
    assert_eq!(&saved, e.state(), "kept whole");
    let mut again = Engine::new(saved.clone(), Settings::default(), LIFE).expect("whole");
    again.step(Input::Agents(vec![rowan.clone(), juniper.clone()]), T0 + 1);
    let r = again.recover(T0 + 1);
    assert_eq!(answers(&r).len(), 1, "owed again, under the same id and body");
    assert_eq!(answers(&r)[0].2, answers(&end)[0].2);

    let mut forged = saved;
    let turn = forged.turns.get_mut(&t.turn).unwrap();
    turn.owed.iter_mut().find(|o| o.to.is_some()).unwrap().to = Some("maple--k3x9".into());
    assert!(check(&forged).is_err());
}

// ---- across lives (docs/durable-computers.md) ----
//
// The engine inside a model of what is around it: the platform's channels
// with their replay and 409 rule (the journal), the lane that posts to them
// in order, the runtime, and every state the engine ever persisted (a save
// may be any of them). A crash drops the engine, its lane and its runtime;
// a new life starts from any state saved, or from none. Runs
// (`Command::Start`) are counted, never records: a second run's records
// are replays of the first's.

const CHAT: &str = "talk--k3x9";

/// The platform's answer to a post (docs/api.md: the same id and body again
/// is a replay; another body, or another channel, is 409).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Answer {
    Appended,
    Replayed,
    Conflict,
}

/// The platform's channels: each one's records in order, and posts by id
/// (with the seq of the record each one made).
#[derive(Debug, Default)]
struct Journal {
    records: BTreeMap<(String, String), Vec<Record>>,
    ids: HashMap<(String, String), (String, Value, u64)>,
}

impl Journal {
    fn append(&mut self, fragment: &str, channel: &str, principal: &str, body: Value) -> u64 {
        let list = self.records.entry((fragment.to_string(), channel.to_string())).or_default();
        let seq = list.len() as u64 + 1;
        list.push(Record { channel: channel.to_string(), seq, at: 0, principal: principal.to_string(), kind: "message".into(), body });
        seq
    }

    fn post(&mut self, fragment: &str, channel: &str, principal: &str, id: &str, body: &Value) -> Answer {
        let key = (fragment.to_string(), id.to_string());
        match self.ids.get(&key) {
            Some((c, b, _)) if c == channel && b == body => Answer::Replayed,
            Some(_) => Answer::Conflict,
            None => {
                let seq = self.append(fragment, channel, principal, body.clone());
                self.ids.insert(key, (channel.to_string(), body.clone(), seq));
                Answer::Appended
            }
        }
    }

    /// The seq of the record a post made (the platform's answer names it).
    fn seq_of(&self, fragment: &str, id: &str) -> Option<u64> {
        self.ids.get(&(fragment.to_string(), id.to_string())).map(|(_, _, seq)| *seq)
    }

    /// The note a turn claimed at `claim` (a seq on `work`) carries, read
    /// from the journal as the driver reads it (driver.rs, `note_for`): the
    /// cut turn it tells of, and its text.
    fn note(&self, agent: &str, claim: Option<u64>) -> Option<(String, String)> {
        let work = self.channel(CHAT, records::WORK);
        let before = &work[..usize::try_from(claim? - 1).expect("a seq")];
        let crate::note::Before::Cut(cut) = crate::note::previous(before, agent, true) else { return None };
        let chat = self.channel(CHAT, records::CHAT);
        let asked = chat.iter().find(|r| r.seq == cut.cause.seq).and_then(crate::note::asked);
        Some((cut.turn.clone(), crate::note::text(&cut, asked.as_deref(), &crate::note::replies(chat, agent, &cut.turn))))
    }

    fn channel(&self, fragment: &str, channel: &str) -> &[Record] {
        self.records.get(&(fragment.to_string(), channel.to_string())).map(Vec::as_slice).unwrap_or(&[])
    }

    /// A turn's records of `kind` on its chat's `work` (an answer's turn
    /// runs in the chat its agent asked from).
    fn work(&self, turn: &str, kind: &str) -> Vec<Value> {
        self.works().flat_map(|w| w.iter()).map(|r| r.body.clone()).filter(|b| b["turn"] == turn && b["kind"] == kind).collect()
    }

    /// Every chat's `work`.
    fn works(&self) -> impl Iterator<Item = &Vec<Record>> {
        self.records.iter().filter(|((_, c), _)| c == records::WORK).map(|(_, w)| w)
    }
}

/// A turn the model runtime holds: the prompt it waits on, whether it asked
/// its asker in words and waits for their answer (`Event::Asked`), whether
/// its asker pressed Stop, and the reply parts it has said.
#[derive(Debug, Default, Clone)]
struct Held {
    prompted: Option<String>,
    asking: bool,
    stop: bool,
    parts: u32,
}

/// A message the engine told a turn as its answer (`Command::Tell`): the
/// life that told it, the turn, the message's seq, and, at that moment, the
/// turn's claim on `work` and whether this life's runtime held the turn.
#[derive(Debug, Clone)]
struct Told {
    life: u64,
    turn: String,
    seq: u64,
    claim: Option<Value>,
    held: bool,
}

struct World {
    agents: Vec<Agent>,
    view: ChatView,
    journal: Journal,
    engine: Option<Engine>,
    lives: u64,
    /// Every state the engine persisted, in order.
    saves: Vec<State>,
    /// The lane: posts not yet answered, in order (one chat, one lane).
    lane: VecDeque<Effect>,
    /// Each run: the life that ran it, and its turn.
    runs: Vec<(u64, String)>,
    /// At each run, the turn's `turn.start` on `work` as it stood then.
    claims_at_run: Vec<(u64, String, Option<Value>)>,
    /// At each run, the note it was handed (P5), as the driver reads it from
    /// the journal before the claim the engine names: the cut turn it tells
    /// of, and its text.
    notes: Vec<(String, Option<(String, String)>)>,
    held: BTreeMap<String, Held>,
    /// Each message told to a turn as its answer, in order.
    tells: Vec<Told>,
    /// Posts on `chat` the platform answered 409: one id, two bodies.
    conflicts_on_chat: Vec<String>,
    /// The ids of the posts a crash took from the lane before they were sent.
    dropped: Vec<String>,
    /// The ids of the posts the lane gave up after its tries (never an
    /// answer: the driver's lane tries one until the platform answers it).
    given_up: Vec<String>,
    now: u64,
}

impl World {
    fn new(labels: &[&str]) -> World {
        let agents: Vec<Agent> = labels.iter().map(|l| agent(l)).collect();
        let view = view(&agents.iter().collect::<Vec<&Agent>>());
        World {
            agents,
            view,
            journal: Journal::default(),
            engine: None,
            lives: 0,
            saves: Vec::new(),
            lane: VecDeque::new(),
            runs: Vec::new(),
            claims_at_run: Vec::new(),
            notes: Vec::new(),
            held: BTreeMap::new(),
            tells: Vec::new(),
            conflicts_on_chat: Vec::new(),
            dropped: Vec::new(),
            given_up: Vec::new(),
            now: T0,
        }
    }

    fn lead(&self) -> &Agent {
        &self.agents[0]
    }

    /// Life `n`'s id, as a bridge process makes its own (128 bits, hex).
    fn life_of(n: u64) -> String {
        format!("{n:032x}")
    }

    /// This life's id.
    fn life(&self) -> String {
        World::life_of(self.lives)
    }

    fn alive(&self) -> bool {
        self.engine.is_some()
    }

    fn engine(&self) -> &Engine {
        self.engine.as_ref().expect("a live engine")
    }

    /// The state as the engine last persisted it.
    fn latest(&self) -> Option<State> {
        self.saves.last().cloned()
    }

    /// A new life from `state` (a save, or none), its runtime connected.
    fn start(&mut self, state: Option<State>) {
        self.start_unconnected(state);
        self.connect();
    }

    /// A new life whose runtime cannot take a turn yet.
    fn start_unconnected(&mut self, state: Option<State>) {
        assert!(!self.alive(), "one life at a time");
        self.lives += 1;
        let settings = Settings { prompt_ttl_ms: 60_000, turn_idle_ms: 600_000 };
        let life = self.life();
        self.engine = Some(Engine::new(state.unwrap_or_default(), settings, &life).expect("a saved state is whole"));
        self.step(Input::Agents(self.agents.clone()));
        let now = self.now;
        let s = self.engine.as_mut().expect("alive").recover(now);
        self.absorb(s);
        self.deliver();
    }

    /// Its runtime can take turns now.
    fn connect(&mut self) {
        self.step(Input::Runtime(Event::Connected(true)));
    }

    /// Its runtime can take no turn now (Hermes gone from its socket).
    fn disconnect(&mut self) {
        self.step(Input::Runtime(Event::Connected(false)));
    }

    /// The life ends where it is: the post in flight lands (`landed`) or
    /// not, and the rest of its lane and its runtime are gone with it.
    fn crash(&mut self, landed: bool) {
        assert!(self.engine.take().is_some(), "a live engine crashes");
        if landed {
            if let Some(e) = self.lane.pop_front() {
                self.land(&e);
            }
        }
        for e in self.lane.drain(..) {
            if let Effect::Post { id, .. } | Effect::Claim { id, .. } | Effect::Fence { id, .. } = e {
                self.dropped.push(id);
            }
        }
        self.held.clear();
    }

    fn step(&mut self, input: Input) {
        let now = self.now;
        let s = self.engine.as_mut().expect("a live engine").step(input, now);
        self.absorb(s);
    }

    /// A step's effects, carried out as the driver does: its state saved
    /// first, its posts onto the lane, its commands to the runtime.
    fn absorb(&mut self, s: Step) {
        if s.dirty {
            self.saves.push(self.engine().state().clone());
        }
        for e in s.effects {
            match e {
                Effect::Post { .. } | Effect::Claim { .. } | Effect::Fence { .. } | Effect::Owed { .. } => self.lane.push_back(e),
                Effect::Runtime(Command::Start(ts)) => {
                    let claim = self.journal.work(&ts.turn, "turn.start").into_iter().next();
                    self.runs.push((self.lives, ts.turn.clone()));
                    self.claims_at_run.push((self.lives, ts.turn.clone(), claim));
                    assert!(ts.note.is_none(), "the engine names the claim; the driver reads the note");
                    let note = if ts.fragment == CHAT { self.journal.note(&ts.agent.identity, ts.claim_seq) } else { None };
                    self.notes.push((ts.turn.clone(), note));
                    self.held.insert(ts.turn.clone(), Held::default());
                }
                Effect::Runtime(Command::Stop { turn }) => {
                    if let Some(h) = self.held.get_mut(&turn) {
                        h.stop = true;
                    }
                }
                Effect::Runtime(Command::Answer { turn, prompt, option, .. }) => {
                    // the runtime acts on no owner's answer the journal does
                    // not hold: a life rolled back past it rebuilds nothing
                    if option.is_some() {
                        let closed = self.journal.work(&turn, "turn.prompt.closed");
                        assert!(closed.iter().any(|c| c["prompt"] == prompt.as_str() && c["option"] == json!(option) && c.get("answerSeq").is_none()), "{turn}/{prompt}: answered before its closure was on work: {closed:?}");
                    }
                    if let Some(h) = self.held.get_mut(&turn) {
                        h.prompted = None;
                    }
                }
                Effect::Runtime(Command::Forget { turn }) => {
                    self.held.remove(&turn);
                }
                // the bridge's own words to an ask's turn that asked back
                Effect::Runtime(Command::Tell { turn, seq: ASKED_BACK_SEQ, .. }) => {
                    if let Some(h) = self.held.get_mut(&turn) {
                        h.asking = false;
                    }
                }
                Effect::Runtime(Command::Tell { turn, seq, .. }) => {
                    let claim = self.journal.work(&turn, "turn.start").into_iter().next();
                    let held = match self.held.get_mut(&turn) {
                        Some(h) => {
                            h.asking = false;
                            true
                        }
                        None => false,
                    };
                    self.tells.push(Told { life: self.lives, turn, seq, claim, held });
                }
                Effect::Draft { .. } | Effect::Keepalive(_) | Effect::Discover { .. } | Effect::Runtime(Command::Aside { .. }) => {}
            }
        }
    }

    /// Every record of the chat the engine has not read, as its followers
    /// feed them (from its cursor).
    fn deliver(&mut self) {
        if !self.alive() {
            return;
        }
        for a in self.agents.clone() {
            let cursor = self.engine().cursor(&a.fragment, CHAT, records::CHAT);
            let unread: Vec<Record> = self.journal.channel(CHAT, records::CHAT).iter().filter(|r| r.seq > cursor).cloned().collect();
            for r in unread {
                self.step(Input::Record { agent: a.fragment.clone(), fragment: CHAT.into(), record: r, view: Some(self.view.clone()), since: 0 });
            }
            // its own `tasks` (where its answers come)
            let cursor = self.engine().cursor(&a.fragment, &a.fragment, records::TASKS);
            let unread: Vec<Record> = self.journal.channel(&a.fragment, records::TASKS).iter().filter(|r| r.seq > cursor).cloned().collect();
            for r in unread {
                self.step(Input::Record { agent: a.fragment.clone(), fragment: a.fragment.clone(), record: r, view: None, since: 0 });
            }
        }
    }

    /// A lane's post or claim, landed: the platform's answer.
    fn land(&mut self, e: &Effect) -> Answer {
        let (agent, fragment, channel, id, body) = match e {
            Effect::Post { agent, fragment, channel, id, body, .. } => (agent, fragment, *channel, id, body),
            Effect::Claim { agent, fragment, id, body, .. } | Effect::Fence { agent, fragment, id, body, .. } => (agent, fragment, records::WORK, id, body),
            Effect::Owed { agent, fragment, channel, id, body, .. } => (agent, fragment, *channel, id, body),
            _ => panic!("the lane holds posts, claims and owed records: {e:?}"),
        };
        let principal = self.agents.iter().find(|a| &a.fragment == agent).map(|a| a.identity.clone()).expect("an agent's post");
        let answer = self.journal.post(fragment, channel, &principal, id, body);
        if channel == records::CHAT && answer == Answer::Conflict {
            self.conflicts_on_chat.push(id.clone());
        }
        answer
    }

    /// The engine is told a claim's answer (the driver's `Input::Claimed`),
    /// or that the lane is done with an owed record (`Input::Posted`).
    fn tell(&mut self, e: &Effect, answer: ClaimAnswer) {
        match e {
            Effect::Claim { turn, .. } => self.step(Input::Claimed { turn: turn.clone(), answer }),
            Effect::Fence { turn, prompt, .. } => self.step(Input::Fenced { turn: turn.clone(), prompt: prompt.clone(), answer }),
            Effect::Owed { turn, id, .. } => self.step(Input::Posted { turn: turn.clone(), id: id.clone() }),
            _ => {}
        }
    }

    /// The lane's next post, answered; false when it holds none.
    fn answer_one(&mut self) -> bool {
        let Some(e) = self.lane.pop_front() else { return false };
        let seq = match &e {
            Effect::Claim { fragment, id, .. } => Some((fragment.clone(), id.clone())),
            _ => None,
        };
        let answer = match self.land(&e) {
            Answer::Appended | Answer::Replayed => ClaimAnswer::Ours { seq: seq.and_then(|(f, id)| self.journal.seq_of(&f, &id)) },
            Answer::Conflict => ClaimAnswer::Theirs,
        };
        self.tell(&e, answer);
        self.deliver();
        true
    }

    /// Every post answered, and every record read.
    fn answer_all(&mut self) {
        // bounded: each pass answers one post, and a world makes few
        for _ in 0..10_000 {
            if !self.answer_one() {
                return;
            }
        }
        panic!("a lane that never empties");
    }

    /// The lane gives up on its next claim (or a live answer's closure): it
    /// never reaches the platform, and the engine hears it unanswered. Any
    /// other post the model's lane
    /// tries until it lands (the lane's own bound of tries is a property of
    /// every post, out of this model; a crash is in it).
    fn lose_one(&mut self) {
        match self.lane.front() {
            Some(Effect::Claim { .. } | Effect::Fence { .. }) => {
                let e = self.lane.pop_front().expect("a claim");
                self.tell(&e, ClaimAnswer::Unanswered);
            }
            Some(_) => {
                self.answer_one();
            }
            None => {}
        }
    }

    /// The platform keeps failing the lane's next post past its tries: a
    /// claim is heard unanswered (claimed again at a tick); an owed record
    /// on `work` is given up (the lane says it is done, and it never
    /// lands), as is any other post; an answer is never given up, so it
    /// stays at the lane's head for its retry (driver.rs, `Job::Owed`).
    fn give_up_one(&mut self) {
        let Some(e) = self.lane.front().cloned() else { return };
        match &e {
            Effect::Owed { channel, .. } if *channel == records::TASKS => return,
            Effect::Claim { .. } => {
                self.lane.pop_front();
                self.tell(&e, ClaimAnswer::Unanswered);
            }
            Effect::Owed { turn, id, .. } => {
                self.lane.pop_front();
                self.given_up.push(id.clone());
                self.step(Input::Posted { turn: turn.clone(), id: id.clone() });
            }
            Effect::Post { id, .. } => {
                self.lane.pop_front();
                self.given_up.push(id.clone());
            }
            _ => {}
        }
        self.deliver();
    }

    /// The lane's next post reaches the platform, and its answer is lost: a
    /// claim is heard unanswered (the next tick's claim will find it a
    /// replay); any other post stays at the lane's head, for its retry to
    /// find it a replay.
    fn land_unanswered_one(&mut self) {
        let Some(e) = self.lane.front().cloned() else { return };
        self.land(&e);
        if matches!(e, Effect::Claim { .. } | Effect::Fence { .. }) {
            self.lane.pop_front();
            self.tell(&e, ClaimAnswer::Unanswered);
        }
        self.deliver();
    }

    /// `who` says `body` in the chat, read at once: its seq.
    fn say_body(&mut self, who: &str, body: Value) -> u64 {
        let seq = self.journal.append(CHAT, records::CHAT, who, body);
        self.deliver();
        seq
    }

    /// `who` says `text`: the lead's turn of it.
    fn say(&mut self, who: &str, text: &str) -> String {
        let seq = self.say_body(who, json!({ "text": text }));
        self.turn_of(seq)
    }

    fn turn_of(&self, seq: u64) -> String {
        records::turn_id(&self.lead().fragment, CHAT, records::CHAT, seq)
    }

    /// The runtime ends a turn it holds: with an answer (idle), or without.
    fn end(&mut self, turn: &str, outcome: Outcome) {
        let h = self.held.remove(turn).unwrap_or_else(|| panic!("the runtime holds {turn}"));
        if outcome == Outcome::Idle {
            self.step(Input::Runtime(Event::Reply { turn: turn.into(), part: h.parts + 1, text: format!("an answer to {turn}") }));
        }
        self.step(Input::Runtime(Event::End { turn: turn.into(), outcome }));
    }

    /// The runtime asks its asker something to answer in words (Hermes'
    /// open clarify): the question is a reply part, and the turn waits,
    /// running, for their next message (`Command::Tell`).
    fn ask_in_words(&mut self, turn: &str) {
        let h = self.held.get_mut(turn).expect("held");
        h.asking = true;
        h.parts += 1;
        let part = h.parts;
        self.step(Input::Runtime(Event::Reply { turn: turn.into(), part, text: format!("a question of {turn}") }));
        self.step(Input::Runtime(Event::Asked { turn: turn.into() }));
    }

    /// Whether the message whose turn (were it one) is `turn` was told to a
    /// turn as its answer, in any life.
    fn was_told(&self, turn: &str) -> bool {
        self.tells.iter().any(|t| self.turn_of(t.seq) == turn)
    }

    /// The runtime asks its owner something; the turn waits.
    fn ask(&mut self, turn: &str) {
        let prompt = format!("p-{}", &turn[..12]);
        self.held.get_mut(turn).expect("held").prompted = Some(prompt.clone());
        let options = vec![PromptOption { id: "once".into(), label: "Allow once".into(), style: None, words: false }, PromptOption { id: "deny".into(), label: "Deny".into(), style: None, words: false }];
        self.step(Input::Runtime(Event::Prompt { turn: turn.into(), prompt, text: "ok?".into(), options, ttl_ms: None }));
    }

    fn tick(&mut self, ms: u64) {
        self.now += ms;
        if self.alive() {
            self.step(Input::Tick);
        }
    }

    /// The lead's runtime says something with no turn running.
    fn say_unasked(&mut self, text: &str) {
        let agent = self.lead().fragment.clone();
        self.step(Input::Runtime(Event::Say { agent, fragment: CHAT.into(), text: text.into(), files: Vec::new() }));
    }

    fn runs_of(&self, turn: &str) -> usize {
        self.runs.iter().filter(|(_, t)| t == turn).count()
    }

    fn starts(&self, turn: &str) -> Vec<Value> {
        self.journal.work(turn, "turn.start")
    }

    fn ends(&self, turn: &str) -> Vec<Value> {
        self.journal.work(turn, "turn.end")
    }

    /// The note `turn` was handed at its run (it ran once).
    fn note_of(&self, turn: &str) -> Option<String> {
        let at: Vec<&Option<(String, String)>> = self.notes.iter().filter(|(t, _)| t == turn).map(|(_, n)| n).collect();
        assert_eq!(at.len(), 1, "{turn} ran once");
        at[0].as_ref().map(|(_, text)| text.clone())
    }

    /// A step of a turn the runtime holds, as the runtime reports it.
    fn did(&mut self, turn: &str, tool: &str, args: &str) {
        self.step(Input::Runtime(Event::Step { turn: turn.into(), step: ToolStep { tool: tool.into(), args: args.into(), ok: true, excerpt: String::new(), category: crate::records::Category::Shell, evidence: None } }));
    }

    /// The turns whose `turn.start` the lane holds, in order.
    fn pending_starts(&self) -> Vec<String> {
        self.lane
            .iter()
            .filter_map(|e| match e {
                Effect::Claim { turn, .. } => Some(turn.clone()),
                Effect::Post { body, .. } | Effect::Owed { body, .. } if body["kind"] == "turn.start" => body["turn"].as_str().map(str::to_string),
                _ => None,
            })
            .collect()
    }

    /// Another life's claim of `turn` (the turn of chat record `seq`),
    /// already on `work`.
    fn claimed_by_another_life(&mut self, turn: &str, seq: u64) {
        let lead = self.lead().clone();
        let body = json!({ "kind": "turn.start", "turn": turn, "asker": "npub1paul", "agent": lead.identity,
            "cause": { "fragment": CHAT, "channel": "chat", "seq": seq }, "life": "f".repeat(32) });
        assert_eq!(self.journal.post(CHAT, records::WORK, &lead.identity, &records::work_id(turn, "start"), &body), Answer::Appended);
    }

    /// What may be checked at any moment (I1, I2, F11's ids, and where an
    /// answer in words goes).
    fn check(&self) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for (_, t) in &self.runs {
            if !seen.insert(t) {
                return Err(format!("I1: {t} ran twice: {:?}", self.runs));
            }
        }
        for (life, t, claim) in &self.claims_at_run {
            let ours = claim.as_ref().is_some_and(|c| c["life"] == json!(World::life_of(*life)));
            if !ours {
                return Err(format!("I1: life {life} ran {t} without its claim answered as its own (work held {claim:?})"));
            }
        }
        let work: Vec<&Record> = self.journal.works().flatten().collect();
        let started: BTreeSet<&str> = work.iter().filter(|r| r.body["kind"] == "turn.start").filter_map(|r| r.body["turn"].as_str()).collect();
        for end in work.iter().filter(|r| r.body["kind"] == "turn.end") {
            let t = end.body["turn"].as_str().unwrap_or("");
            if !started.contains(t) && !self.gave_up(t, "start") {
                return Err(format!("I2: {t} ended with no start"));
            }
        }
        if !self.conflicts_on_chat.is_empty() {
            return Err(format!("an id on chat posted with two bodies: {:?}", self.conflicts_on_chat));
        }
        // An answer in words goes only to a turn this life claimed and its
        // runtime holds; a message is told at most once in a life, and never
        // also run there as a turn of its own.
        let mut told_in: BTreeSet<(u64, u64)> = BTreeSet::new();
        for t in &self.tells {
            if !t.held {
                return Err(format!("Tell: life {} told {} message {}, which its runtime does not hold", t.life, t.turn, t.seq));
            }
            if !self.runs.contains(&(t.life, t.turn.clone())) {
                return Err(format!("Tell: life {} told {} message {}, a turn it never ran", t.life, t.turn, t.seq));
            }
            if !t.claim.as_ref().is_some_and(|c| c["life"] == json!(World::life_of(t.life))) {
                return Err(format!("Tell: life {} told {} message {}, a turn not claimed as its own (work held {:?})", t.life, t.turn, t.seq, t.claim));
            }
            if !told_in.insert((t.life, t.seq)) {
                return Err(format!("Tell: life {} told message {} twice", t.life, t.seq));
            }
            let own = self.turn_of(t.seq);
            if self.runs.contains(&(t.life, own.clone())) {
                return Err(format!("Tell: life {} told message {} to {} and ran it as {own} too", t.life, t.seq, t.turn));
            }
        }
        // P5: a cut turn is told of once at most, by a turn after it, and only
        // a turn the journal ended as lost is told of
        let mut told_of: BTreeSet<&str> = BTreeSet::new();
        for (turn, note) in &self.notes {
            let Some((cut, _)) = note else { continue };
            if cut == turn || !told_of.insert(cut.as_str()) {
                return Err(format!("P5: {turn} was told of {cut}, already told or itself: {:?}", self.notes.iter().filter(|(_, n)| n.is_some()).collect::<Vec<_>>()));
            }
            if !self.ends(cut).iter().any(|e| e["error"] == crate::engine::LOST) {
                return Err(format!("P5: {turn} was told of {cut}, which did not end as lost: {:?}", self.ends(cut)));
            }
        }
        Ok(())
    }

    /// Everything settles: a life (the latest state's, if none runs), its
    /// runtime connected, every post answered, every turn it holds ended.
    fn settle(&mut self) {
        if !self.alive() {
            let s = self.latest();
            self.start(s);
        }
        self.connect();
        // bounded: each pass ends what the runtime holds, so the queues drain
        for _ in 0..1_000 {
            self.deliver();
            self.answer_all();
            let held: Vec<(String, Held)> = self.held.iter().map(|(t, h)| (t.clone(), h.clone())).collect();
            if held.is_empty() && self.lane.is_empty() && self.engine().state().turns.is_empty() {
                return;
            }
            for (t, h) in held {
                self.end(&t, if h.stop { Outcome::Stopped } else { Outcome::Idle });
            }
            // a claim left unanswered is claimed again at a tick
            self.tick(1_000);
        }
        panic!("a world that never settles");
    }

    /// Settled, every message said has one start and one end , and
    /// ran at most once , or it was the answer a turn asked for in words
    /// (told to that turn, in the life that ran it) and started no turn of
    /// its own; and no turn is held any more.
    /// Whether the lane gave up `turn`'s `part` (`start`, `end`).
    fn gave_up(&self, turn: &str, part: &str) -> bool {
        self.given_up.contains(&records::work_id(turn, part))
    }

    /// One start and one end, but for what the lane gave up.
    fn whole(&self, turn: &str) -> bool {
        let (starts, ends) = (self.starts(turn).len(), self.ends(turn).len());
        (starts == 1 || (starts == 0 && self.gave_up(turn, "start"))) && (ends == 1 || (ends == 0 && self.gave_up(turn, "end")))
    }

    fn check_settled(&self, said: &[String]) -> Result<(), String> {
        self.check()?;
        for t in said {
            let (starts, ends) = (self.starts(t).len(), self.ends(t).len());
            if (starts, ends) == (0, 0) && self.was_told(t) {
                continue;
            }
            if !self.whole(t) {
                let dropped: Vec<&String> = self.dropped.iter().filter(|id| id.contains(t.as_str())).collect();
                return Err(format!("I2/I4: {t} has {starts} starts and {ends} ends; crashes took its posts {dropped:?} from the lane"));
            }
        }
        let held = &self.engine().state().turns;
        if !held.is_empty() {
            return Err(format!("settled, and still holding {:?}", held.keys().collect::<Vec<_>>()));
        }
        Ok(())
    }

    /// Settled, each of juniper's questions (`asked`: rowan's turns of
    /// them) is one turn of rowan's (one start and one end, but for what
    /// the lane gave up), and has exactly one answer, on juniper's `tasks`
    /// under its turn's `answer_id`, whatever ended it (a refusal, past the
    /// hops or the budget, too, answered `refused`); each answer is a turn
    /// of juniper's in `HOME`, likewise whole, and a refused one's never
    /// ran.
    fn check_answers(&self, asked: &[String]) -> Result<(), String> {
        let Some(juniper) = self.agents.iter().find(|a| a.name == "juniper") else { return Ok(()) };
        let answers = self.journal.channel(&juniper.fragment, records::TASKS);
        for t in asked {
            if !self.whole(t) {
                return Err(format!("I2/I4: question {t} has {} starts and {} ends", self.starts(t).len(), self.ends(t).len()));
            }
            let mine = answers.iter().filter(|r| r.body["turn"] == t.as_str()).count();
            if mine != 1 {
                return Err(format!("A1: {t} (ends {:?}) has {mine} answers", self.ends(t)));
            }
        }
        for r in answers {
            if r.principal != juniper.identity || r.body["chat"] != HOME {
                return Err(format!("A2: an answer by {} for {}", r.principal, r.body["chat"]));
            }
            let woke = records::turn_id(&juniper.fragment, &juniper.fragment, records::TASKS, r.seq);
            if !self.whole(&woke) {
                return Err(format!("A2: answer {} woke {woke} with {} starts and {} ends", r.seq, self.starts(&woke).len(), self.ends(&woke).len()));
            }
            if r.body["refused"] == true && self.runs_of(&woke) > 0 {
                return Err(format!("A3: refused answer {} ran {woke}", r.seq));
            }
        }
        Ok(())
    }
}

/// Goal (I1, P1): a turn is handed to its runtime only once its claim (its
/// `turn.start`, as this life) is answered as this life's, and only while
/// its runtime can take it. A replay of the claim (the lane's own retry,
/// after an answer was lost) is this life's too, and runs it once.
#[test]
fn a_turn_starts_only_once_its_claim_is_answered() {
    let mut w = World::new(&["juniper"]);
    w.start_unconnected(None);
    let t = w.say("npub1paul", "one");
    assert_eq!((w.runs_of(&t), w.pending_starts()), (0, vec![]), "its runtime cannot take it: neither claimed nor run");
    w.connect();
    assert_eq!(w.pending_starts(), vec![t.clone()], "claimed once its runtime can take it");
    assert_eq!(w.runs_of(&t), 0, "not run before its claim is answered");
    w.answer_one();
    assert_eq!(w.runs_of(&t), 1, "run once its claim is answered");
    assert_eq!(w.starts(&t)[0]["life"], json!(w.life()), "the claim names its life");

    // replay: the claim landed and its answer was lost; claimed again at the
    // next tick, its replay is this life's own: run once
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t = w.say("npub1paul", "one");
    w.land_unanswered_one();
    assert_eq!(w.runs_of(&t), 0, "an unanswered claim starts nothing");
    w.tick(1_000);
    assert_eq!(w.pending_starts(), vec![t.clone()], "claimed again");
    w.answer_one();
    assert_eq!((w.runs_of(&t), w.starts(&t).len()), (1, 1), "its own claim, replayed: run once");
}

/// Goal : a claim another life holds (409) is never run here; the
/// turn gets its one end, and the next message runs.
#[test]
fn a_claim_another_life_holds_ends_the_turn() {
    let mut w = World::new(&["juniper"]);
    let t = w.turn_of(1);
    w.claimed_by_another_life(&t, 1);
    w.start(None);
    assert_eq!(w.say("npub1paul", "one"), t);
    w.answer_one();
    assert_eq!(w.runs_of(&t), 0, "another life's turn is never run here");
    w.answer_all();
    let ends = w.ends(&t);
    assert_eq!(ends.len(), 1, "one end: {ends:?}");
    assert_eq!((ends[0]["outcome"].as_str(), ends[0]["error"].as_str()), (Some("error"), Some("lost when the computer restarted")));
    assert!(w.engine().state().turns.is_empty(), "forgotten");
    let next = w.say("npub1paul", "two");
    w.answer_all();
    assert_eq!(w.runs_of(&next), 1, "the next message runs");
}

/// Goal (P1): a claim the platform refuses (403 or 404: the agent left the
/// chat, or its owner holds it below editor) runs nothing and is not asked
/// again: the turn is dropped, the chat's next turn is claimed, and the
/// computer is let go once none is left (never held awake for a claim that
/// cannot land).
#[test]
fn a_claim_the_agent_may_not_post_drops_the_turn() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let say = |e: &mut Engine, seq: u64, text: &str| e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: rec(seq, "npub1paul", json!({ "text": text })), view: Some(v.clone()), since: 0 }, T0);
    let one = say(&mut e, 1, "one");
    let t1 = records::turn_id(&a.fragment, "talk--k3x9", "chat", 1);
    assert_eq!(kinds(&one), vec!["turn.start"]);
    assert!(say(&mut e, 2, "two").effects.iter().all(|x| !matches!(x, Effect::Claim { .. })), "two waits behind one's claim");
    let r = e.step(Input::Claimed { turn: t1.clone(), answer: ClaimAnswer::Refused }, T0 + 1);
    assert!(started(&r).is_none(), "refused: not run");
    let t2 = records::turn_id(&a.fragment, "talk--k3x9", "chat", 2);
    assert_eq!(posts(&r), vec![(records::work_id(&t2, "start"), posts(&r)[0].1.clone())], "no end for one (it cannot be posted), and two is claimed");
    assert!(r.dirty && !e.state().turns.contains_key(&t1), "one is dropped");
    let r2 = e.step(Input::Claimed { turn: t2.clone(), answer: ClaimAnswer::Refused }, T0 + 2);
    assert_eq!(keepalive(&r2), Some(false), "nothing left: the computer may sleep");
    assert!(e.state().turns.is_empty());
    assert!(e.step(Input::Tick, T0 + 3).effects.is_empty(), "and nothing is asked again");
}

/// Goal (P1): a claim the lane gave up on (no answer) starts nothing; the
/// turn stays queued, unclaimed, and is claimed again at the next tick.
#[test]
fn a_claim_with_no_answer_starts_nothing() {
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t = w.say("npub1paul", "one");
    w.lose_one();
    assert_eq!(w.runs_of(&t), 0, "no answer: not run");
    assert!(w.starts(&t).is_empty(), "nor claimed");
    assert_eq!(w.engine().state().turns[&t].phase, Phase::Queued, "kept queued");
    w.tick(1_000);
    assert_eq!(w.pending_starts(), vec![t.clone()], "claimed again at a tick");
    w.answer_one();
    assert_eq!(w.runs_of(&t), 1);
}

/// Goal (P1, one life per turn): a turn whose life ended after it was handed
/// to its runtime is ended as lost by the next life, never handed again,
/// whether or not the runtime had taken it.
#[test]
fn a_restart_ends_what_was_handed() {
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t = w.say("npub1paul", "one");
    w.answer_all();
    assert_eq!(w.runs_of(&t), 1, "handed");
    let state = w.latest();
    w.crash(false);
    w.start(state);
    w.answer_all();
    assert_eq!(w.runs_of(&t), 1, "never handed again");
    let ends = w.ends(&t);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["error"], "lost when the computer restarted");
}

/// Goal : a rollback starts nothing twice. Method: keep the state after
/// turn one, run turn two to its end, start a new life from the kept state,
/// feed it the records again against the journal, and count runs.
#[test]
fn a_rollback_starts_nothing_twice() {
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t1 = w.say("npub1paul", "one");
    w.answer_all();
    w.end(&t1, Outcome::Idle);
    w.answer_all();
    let kept = w.latest();
    let t2 = w.say("npub1paul", "two");
    w.answer_all();
    w.end(&t2, Outcome::Idle);
    w.answer_all();
    w.crash(false);

    w.start(kept);
    w.answer_all();
    let t3 = w.say("npub1paul", "three");
    w.answer_all();
    w.end(&t3, Outcome::Idle);
    w.answer_all();
    let runs: Vec<&str> = w.runs.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(runs, vec![t1.as_str(), t2.as_str(), t3.as_str()], "each turn ran once");
    for t in [&t1, &t2, &t3] {
        let ends = w.ends(t);
        assert_eq!(ends.len(), 1, "{t}: one end");
        assert_eq!(ends[0]["outcome"], "idle", "{t}: its own life's end stands");
    }
    w.check().unwrap();
}

/// Goal : a message admitted and never started before a crash is run by
/// the next life, whether that life wakes with the state the crash left (it
/// queued there), a state from before it was said, or none.
#[test]
fn said_before_a_crash_and_never_started_runs_in_the_next_life() {
    for wake in ["latest", "before", "none"] {
        let mut w = World::new(&["juniper"]);
        w.start(None);
        let t1 = w.say("npub1paul", "one");
        w.answer_all();
        let before = w.latest();
        let t2 = w.say("npub1paul", "two");
        w.answer_all();
        assert_eq!(w.runs_of(&t2), 0, "{wake}: two waits behind one");
        let state = match wake {
            "latest" => w.latest(),
            "before" => before,
            _ => None,
        };
        w.crash(false);
        w.start(state);
        w.answer_all();
        assert_eq!(w.runs_of(&t2), 1, "{wake}: the next life runs it");
        w.end(&t2, Outcome::Idle);
        w.answer_all();
        assert_eq!(w.ends(&t2)[0]["outcome"], "idle", "{wake}");
        assert_eq!(w.ends(&t1).len(), 1, "{wake}: one ended once");
    }
}

/// Goal : what an agent says with no turn running survives a
/// rollback: its id holds the life, so a counter that went back collides
/// with nothing, and both messages are posted.
#[test]
fn what_an_agent_says_unasked_survives_a_rollback() {
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let kept = w.latest();
    w.say_unasked("remember the milk");
    w.answer_all();
    w.crash(false);
    w.start(kept);
    w.say_unasked("water the plants");
    w.answer_all();
    let said: Vec<Value> = w.journal.channel(CHAT, records::CHAT).iter().map(|r| r.body["text"].clone()).collect();
    assert_eq!(said, vec![json!("remember the milk"), json!("water the plants")], "both posted");
    w.check().unwrap();
}

/// Goal (P1, with a question in flight): a turn that asks its asker in
/// words is a running turn of its life. A restart ends it as lost, whatever
/// state the next life wakes with (the one the crash left, one from before
/// the turn was said, or none), and the answer said after is never told to
/// a turn the new life does not own: it is a turn of its own, claimed and
/// run once. Rollback: an answer told in its turn's life, read again by a
/// life a rollback sent back before it, finds no turn of that life asking,
/// and is that life's own turn, run once; the asking turn's end stands.
#[test]
fn a_question_cut_by_a_restart_is_lost_and_its_answer_runs_once() {
    for wake in ["latest", "before", "none"] {
        let mut w = World::new(&["juniper"]);
        w.start(None);
        let before = w.latest();
        let asked = w.say("npub1paul", "name my plant");
        w.answer_all();
        w.ask_in_words(&asked);
        w.answer_all();
        assert!(w.engine().state().turns[&asked].asking, "{wake}: it asks");
        let state = match wake {
            "latest" => w.latest(),
            "before" => before,
            _ => None,
        };
        w.crash(false);
        w.start(state);
        w.answer_all();
        assert_eq!(w.runs_of(&asked), 1, "{wake}: never run again");
        assert_eq!(w.ends(&asked), vec![json!({ "kind": "turn.end", "turn": asked, "outcome": "error", "error": LOST })], "{wake}: ended once, as lost");
        let answer = w.say("npub1paul", "Fernando");
        w.answer_all();
        assert!(w.tells.is_empty(), "{wake}: never told to a turn of another life");
        assert_eq!(w.runs_of(&answer), 1, "{wake}: the answer runs as a turn of its own");
        w.end(&answer, Outcome::Idle);
        w.settle();
        assert_eq!((w.runs_of(&answer), w.starts(&answer).len(), w.ends(&answer)[0]["outcome"].clone()), (1, 1, json!("idle")), "{wake}: once");
        w.check_settled(&[asked, answer]).unwrap_or_else(|e| panic!("{wake}: {e}"));
    }

    // told in its turn's life, then a rollback to before it was read
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let asked = w.say("npub1paul", "name my plant");
    w.answer_all();
    w.ask_in_words(&asked);
    w.answer_all();
    let asking = w.latest();
    let answer = w.say("npub1paul", "Fernando");
    assert_eq!(w.tells.iter().map(|t| (t.life, t.turn.clone(), w.turn_of(t.seq))).collect::<Vec<_>>(), vec![(1, asked.clone(), answer.clone())], "told to the asking turn");
    w.end(&asked, Outcome::Idle);
    w.answer_all();
    assert!(w.starts(&answer).is_empty(), "the answer started no turn of its own");
    w.crash(false);
    w.start(asking);
    w.answer_all();
    assert_eq!(w.ends(&asked), vec![json!({ "kind": "turn.end", "turn": asked, "outcome": "idle" })], "its own end stands (the new life's, as lost, is a 409)");
    assert_eq!(w.runs_of(&asked), 1, "never run again");
    assert_eq!(w.runs_of(&answer), 1, "read again, the answer is the new life's own turn");
    assert_eq!(w.tells.len(), 1, "told once, in the life that ran the asking turn");
    w.settle();
    w.check_settled(&[asked, answer]).unwrap();
}

/// A small deterministic generator (splitmix64): one seed, one history.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "a choice among some");
        self.next() % n
    }
}

/// Goal (P5): the first turn the agent runs in the chat after one of its
/// turns there ended as lost carries a note, built from the journal alone:
/// what the cut turn was asked, the steps it recorded, what it had replied,
/// and that a restart cut it. The turn after carries none (said once), also
/// after a rollback that reads the noted turn's message again; and the note
/// is the same whatever state the next life wakes with: the one the crash
/// left (the cut turn running), one from before the cut turn was said, or
/// none.
#[test]
fn the_turn_after_a_lost_one_carries_the_note_once() {
    let mut notes: Vec<String> = Vec::new();
    for wake in ["latest", "before", "none"] {
        let mut w = World::new(&["juniper"]);
        w.start(None);
        let one = w.say("npub1paul", "one");
        w.answer_all();
        w.end(&one, Outcome::Idle);
        w.answer_all();
        let before = w.latest();
        let cut = w.say("npub1paul", "do the risky thing");
        w.answer_all();
        assert_eq!(w.note_of(&cut), None, "{wake}: the turn before it was answered");
        w.did(&cut, "terminal", "ls /tmp");
        w.step(Input::Runtime(Event::Reply { turn: cut.clone(), part: 1, text: "Checking first.".into() }));
        w.did(&cut, "terminal", "rm -rf /tmp/x");
        w.answer_all();
        let state = match wake {
            "latest" => w.latest(),
            "before" => before,
            _ => None,
        };
        w.crash(false);
        w.start(state);
        w.answer_all();
        assert_eq!(w.ends(&cut).len(), 1, "{wake}: the cut turn ended once");
        assert_eq!(w.ends(&cut)[0]["error"], LOST, "{wake}");
        let kept = w.latest();
        let next = w.say("npub1paul", "good morning");
        w.answer_all();
        let note = w.note_of(&next).unwrap_or_else(|| panic!("{wake}: the turn after the lost one carries its note"));
        for said in ["cut short", "computer restarted", "Check what it already did", "“do the risky thing”", "terminal ls /tmp (ok); terminal rm -rf /tmp/x (ok)", "It had replied: “Checking first.”"] {
            assert!(note.contains(said), "{wake}: {said:?} in {note}");
        }
        w.end(&next, Outcome::Idle);
        w.answer_all();
        let after = w.say("npub1paul", "and after that");
        w.answer_all();
        assert_eq!(w.note_of(&after), None, "{wake}: said once");
        w.end(&after, Outcome::Idle);
        w.answer_all();
        // a rollback to before the noted turn's message: read again, it is
        // another life's claim and runs nothing; the next turn is told nothing
        w.crash(false);
        w.start(kept);
        w.answer_all();
        let last = w.say("npub1paul", "once more");
        w.answer_all();
        assert_eq!(w.runs_of(&next), 1, "{wake}: the noted turn ran once");
        assert_eq!(w.note_of(&last), None, "{wake}: said once, after a rollback too");
        w.check().unwrap();
        notes.push(note);
    }
    assert!(notes.windows(2).all(|p| p[0] == p[1]), "the journal's note, whatever state woke: {notes:#?}");
}

/// Seeds the simulation runs, events in each, and messages said in each.
const SIM_SEEDS: u64 = 120;
const SIM_EVENTS: usize = 400;
const SIM_SAID_MAX: usize = 40;

/// What a history reached, beyond its invariants: answers told to a turn
/// asking in words, and those a later life read again and ran as a turn of
/// their own (a rollback to before the life that told them).
#[derive(Debug, Default, Clone, Copy)]
struct Reached {
    told: usize,
    told_then_run: usize,
    answered: usize,
    /// Answers that say their turn was lost, and answers' turns a restart cut.
    answered_lost: usize,
    answers_cut: usize,
    /// Answers of questions refused at admission, and posts the lane gave up.
    answered_refused: usize,
    given_up: usize,
}

/// One history: events chosen by `seed`, the invariants checked after each,
/// and every message's records checked once everything settles. `asking`:
/// juniper asks rowan, the chat's lead, as often as paul says something,
/// to be answered back in `HOME`, and asks again from some of those
/// answers' turns, while they run.
fn simulate(seed: u64, asking: bool) -> Result<Reached, String> {
    // Its events would be millions of log lines.
    crate::log::set_quiet(true);
    let mut rng = Rng(seed);
    let mut w = World::new(if asking { &["rowan", "juniper"] } else { &["juniper"] });
    w.start(None);
    let ask = |w: &mut World, n: usize| w.journal.append(CHAT, records::CHAT, "npub1juniper", json!({ "text": format!("question {n}"), "to": ["npub1rowan"], "return_to": HOME }));
    let mut asked: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    let mut unasked = 0;
    for n in 0..SIM_EVENTS {
        let what = if !w.alive() && rng.below(3) == 0 {
            // a new life: from the latest state, an earlier one, or none
            let state = match rng.below(4) {
                0 => None,
                1 => w.latest(),
                _ if w.saves.is_empty() => None,
                _ => Some(w.saves[rng.below(w.saves.len() as u64) as usize].clone()),
            };
            w.start(state);
            "start"
        } else {
            match rng.below(100) {
                0..=14 if said.len() < SIM_SAID_MAX => {
                    // said (or asked), and read when the followers next deliver
                    if asking && rng.below(2) == 0 {
                        let seq = ask(&mut w, asked.len());
                        asked.push(w.turn_of(seq));
                        "ask"
                    } else {
                        let seq = w.journal.append(CHAT, records::CHAT, "npub1paul", json!({ "text": format!("message {}", said.len()) }));
                        said.push(w.turn_of(seq));
                        "say"
                    }
                }
                15..=27 => {
                    w.deliver();
                    "deliver"
                }
                28..=47 => {
                    w.answer_one();
                    "answer"
                }
                48..=51 => {
                    w.lose_one();
                    "lose"
                }
                52..=55 => {
                    w.land_unanswered_one();
                    "land unanswered"
                }
                56..=70 if !w.held.is_empty() => {
                    let held: Vec<(String, Held)> = w.held.iter().map(|(t, h)| (t.clone(), h.clone())).collect();
                    let (t, h) = held[rng.below(held.len() as u64) as usize].clone();
                    // a turn waits on its card's answer, or on its asker's
                    // words (the next message paul says is them)
                    let answering = w.alive() && w.engine().state().turns.get(&t).is_some_and(|x| x.fragment == HOME);
                    match (h.stop, h.prompted.is_some() || h.asking, rng.below(10)) {
                        (true, _, _) => w.end(&t, Outcome::Stopped),
                        // an answer's turn asks again, read while it runs
                        (false, false, 5) if answering && asked.len() < SIM_SAID_MAX => {
                            let seq = ask(&mut w, asked.len());
                            asked.push(w.turn_of(seq));
                            w.deliver();
                        }
                        (false, true, _) => {}
                        (false, false, 0..=1) => w.ask(&t),
                        (false, false, 2) => w.end(&t, Outcome::Error("it failed".into())),
                        (false, false, 3..=4) => w.ask_in_words(&t),
                        (false, false, _) => w.end(&t, Outcome::Idle),
                    }
                    "the runtime"
                }
                71..=75 => {
                    // paul answers a card the journal shows open
                    let asked: Vec<Value> = w.journal.channel(CHAT, records::WORK).iter().map(|r| r.body.clone()).filter(|b| b["kind"] == "turn.prompt").collect();
                    if !asked.is_empty() {
                        let p = &asked[rng.below(asked.len() as u64) as usize];
                        let option = if rng.below(2) == 0 { "once" } else { "deny" };
                        w.journal.append(CHAT, records::CHAT, "npub1paul", json!({ "kind": "prompt_response", "prompt": p["prompt"], "option": option }));
                    }
                    "answer a card"
                }
                76..=79 => {
                    let ms = [1_000, 30_000, 120_000, 1_800_000][rng.below(4) as usize];
                    w.tick(ms);
                    "tick"
                }
                80..=83 if !said.is_empty() => {
                    let t = said[rng.below(said.len() as u64) as usize].clone();
                    w.journal.append(CHAT, records::CHAT, "npub1paul", json!({ "kind": "stop", "turn": t }));
                    "stop"
                }
                84..=88 if w.alive() => {
                    w.crash(rng.below(2) == 0);
                    "crash"
                }
                89..=92 if w.alive() => {
                    unasked += 1;
                    w.say_unasked(&format!("unasked {unasked}"));
                    "say unasked"
                }
                93..=94 if w.alive() => {
                    w.disconnect();
                    "the runtime goes"
                }
                95..=97 if w.alive() => {
                    w.connect();
                    "the runtime comes back"
                }
                98..=99 if w.alive() => {
                    w.give_up_one();
                    "give up"
                }
                _ => "nothing",
            }
        };
        w.check().map_err(|e| format!("seed {seed}, event {n} ({what}): {e}"))?;
    }
    w.settle();
    w.check_settled(&said).map_err(|e| format!("seed {seed}, settled: {e}"))?;
    w.check_answers(&asked).map_err(|e| format!("seed {seed}, settled: {e}"))?;
    let told_then_run = w.tells.iter().filter(|t| w.runs.iter().any(|(life, run)| *life > t.life && *run == w.turn_of(t.seq))).count();
    let answers = w.journal.channel("juniper--k3x9", records::TASKS);
    let answered_lost = answers.iter().filter(|r| r.body["error"] == LOST).count();
    let answers_cut = answers.iter().filter(|r| w.ends(&records::turn_id("juniper--k3x9", "juniper--k3x9", records::TASKS, r.seq)).iter().any(|e| e["error"] == LOST)).count();
    let answered_refused = answers.iter().filter(|r| r.body["refused"] == true).count();
    Ok(Reached { told: w.tells.len(), told_then_run, answered: answers.len(), answered_lost, answers_cut, answered_refused, given_up: w.given_up.len() })
}

/// Goal : any history of messages, answers lost and late, runs,
/// cards, questions in words and their answers (`Asked`, `Tell`), Stops,
/// crashes (each taking the lane's unsent posts with it), and lives started
/// from any saved state or none keeps the invariants: checked after every
/// event (an answer in words told only to a turn its life claimed and
/// runs), and once it settles every message has one start and one end, ran
/// at most once, or was an answer told to a turn and started none of its
/// own; and no turn is held. No case is let off. The test that finds the
/// cases nobody listed.
#[test]
fn any_history_of_crashes_and_rollbacks_keeps_the_invariants() {
    let histories: Vec<Result<Reached, String>> = (0..SIM_SEEDS).map(|seed| simulate(seed, false)).collect();
    let failures: Vec<String> = histories.iter().filter_map(|h| h.clone().err()).collect();
    assert!(failures.is_empty(), "{} of {SIM_SEEDS} histories broke an invariant; the first:\n{}", failures.len(), failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n"));
    let reached = histories.iter().filter_map(|h| h.as_ref().ok()).fold(Reached::default(), |a, r| Reached { told: a.told + r.told, told_then_run: a.told_then_run + r.told_then_run, ..Reached::default() });
    assert!(reached.told > 0 && reached.told_then_run > 0, "the histories answer questions in words, and a rollback reads some answers again: {reached:?}");
}

/// Goal: the same histories with one agent asking another to be answered
/// back (and asking again from those answers, past the hops too): besides
/// every invariant above, each question is answered exactly once, however
/// it ended (refused too), and each answer is one turn of the asking agent
/// where it asked from, run once at most, never a refused one's. Crashes
/// before and after each post (an answer's included), lost answers, posts
/// the lane gives up (never an answer: the driver's lane tries one until
/// the platform answers it) and rollbacks to any save: no asked turn runs
/// twice to answer, and no answer wakes its agent twice.
#[test]
fn any_history_of_asks_answers_each_once() {
    let histories: Vec<Result<Reached, String>> = (0..SIM_SEEDS).map(|seed| simulate(seed, true)).collect();
    let failures: Vec<String> = histories.iter().filter_map(|h| h.clone().err()).collect();
    assert!(failures.is_empty(), "{} of {SIM_SEEDS} histories broke an invariant; the first:\n{}", failures.len(), failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n"));
    let reached = histories.iter().filter_map(|h| h.as_ref().ok()).fold(Reached::default(), |a, r| Reached { answered: a.answered + r.answered, answered_lost: a.answered_lost + r.answered_lost, answers_cut: a.answers_cut + r.answers_cut, answered_refused: a.answered_refused + r.answered_refused, given_up: a.given_up + r.given_up, ..a });
    assert!(reached.answered > SIM_SEEDS as usize && reached.answered_lost > 0 && reached.answers_cut > 0 && reached.answered_refused > 0 && reached.given_up > 0, "the histories answer back, some answers say lost or refused, restarts cut some answers' turns, and the lane gives up some posts: {reached:?}");
}

/// Goal : the case the simulation found, each way it happens: a
/// crash takes a turn's last records from the lane after the state was
/// written without them, and the next life (from the state the crash
/// left) posts them again, under the same ids and bodies, so the turn ends
/// once and as itself, never as lost. A turn that ran (its end taken; or
/// its end landed and its answer lost, so the next life's is a replay), a
/// refused one, and one stopped while it waited (their start and end
/// taken). Was `a_crash_that_drops_a_turns_end_leaves_it_open`, ignored
/// while P1 left it so.
#[test]
fn a_crash_that_drops_a_turns_end_leaves_nothing_open() {
    // a turn that ran: its end taken by the crash
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t = w.say("npub1paul", "one");
    w.answer_all();
    w.end(&t, Outcome::Idle);
    assert_eq!(w.engine().state().turns[&t].phase, Phase::Ended, "ended, owing its end");
    let state = w.latest();
    w.crash(false);
    assert!(w.ends(&t).is_empty(), "the crash took its end");
    w.start(state);
    w.answer_all();
    assert_eq!(w.ends(&t), vec![json!({ "kind": "turn.end", "turn": t, "outcome": "idle" })], "its own end, posted again");
    assert_eq!(w.runs_of(&t), 1);
    assert!(w.engine().state().turns.is_empty(), "let go once answered");

    // its end landed, and the answer was lost with the life: a replay
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let t = w.say("npub1paul", "one");
    w.answer_all();
    w.end(&t, Outcome::Idle);
    w.answer_one(); // its reply
    let state = w.latest();
    w.crash(true); // its end lands; no one hears
    assert_eq!(w.ends(&t).len(), 1);
    w.start(state);
    w.answer_all();
    assert_eq!(w.ends(&t).len(), 1, "posted again: a replay, so one end");
    assert!(w.engine().state().turns.is_empty());

    // a refused turn, and one stopped while it waited: their two records taken
    let mut w = World::new(&["juniper"]);
    w.start(None);
    let running = w.say("npub1paul", "running");
    w.answer_all();
    let stopped = w.say("npub1paul", "stopped");
    // the queue full: `stopped` and these wait, the next is refused
    for n in 1..limits::QUEUED_PER_CHAT_MAX {
        w.say("npub1paul", &format!("waiting {n}"));
    }
    let refused = w.say("npub1paul", "refused");
    w.say_body("npub1paul", json!({ "kind": "stop", "turn": stopped }));
    assert_eq!(w.pending_starts(), vec![refused.clone(), stopped.clone()], "both owe their start and end");
    let state = w.latest();
    w.crash(false);
    w.start(state);
    w.answer_all();
    for (t, outcome) in [(&refused, "error"), (&stopped, "stopped")] {
        assert_eq!((w.starts(t).len(), w.ends(t).len()), (1, 1), "{t}: both records, once");
        assert_eq!(w.ends(t)[0]["outcome"], outcome);
        assert_eq!(w.runs_of(t), 0, "{t}: never run");
    }
    assert_eq!(w.ends(&running)[0]["error"], LOST, "the one the crash cut is lost");
    w.check().unwrap();
}

/// The agents linked, each with `talk--k3x9` (the chat `said` posts in) as
/// its SimpleX chat.
fn relaying(e: &mut Engine, agents: &[&Agent]) {
    e.step(Input::Relaying(agents.iter().map(|a| (a.fragment.clone(), Some("talk--k3x9".to_string()))).collect()), T0);
}

/// Goal (SimpleX, valid): a linked agent's own relayed record is its
/// owner's message: one turn, asked as its owner and by their name, in its
/// SimpleX chat (`simplex`, so its runtime is told what you say there
/// reaches their phone); read again, nothing; its state, written and read
/// back (a restart), holds nothing of SimpleX; their Stop stops it.
#[test]
fn a_relayed_message_is_its_owners() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    relaying(&mut e, &[&a]);
    let body = records::relayed_message("hi from my phone", &a, records::VIA_SIMPLEX, &[]);
    let s = said(&mut e, &a, &v, 1, &a.identity, body.clone(), T0);
    let t = started(&s).expect("a turn, handed to the runtime");
    assert_eq!((t.text.as_str(), t.asker.as_str(), t.asker_name.as_str(), t.simplex), ("hi from my phone", "npub1paul", "paul", true));
    assert_eq!(said(&mut e, &a, &v, 1, &a.identity, body, T0 + 1), Step::default(), "read again: nothing");
    let written = serde_json::to_string(e.state()).expect("a state serializes");
    let back: State = serde_json::from_str(&written).expect("and reads back");
    assert!(back.turns.contains_key(&t.turn) && !written.contains("simplex") && !written.contains("via"), "a restart reads it back, nothing of SimpleX in it: {written}");
    // its owner's Stop stops it, as a turn they asked
    let s = said(&mut e, &a, &v, 2, "npub1paul", json!({ "kind": "stop", "turn": t.turn }), T0 + 2);
    assert!(commands(&s).iter().any(|c| matches!(c, Command::Stop { turn } if *turn == t.turn)), "{:?}", commands(&s));
}

/// Goal (a door, valid and invalid): a relayed record for an agent's
/// owner, through that agent's own door, is its owner's message, whichever
/// door (`simplex`, `telegram`) and whether or not a SimpleX link is bound
/// here: the platform took it only through a door its owner opened.
/// Carried by the agent itself or by a key that is none of the chat's
/// agents (a hosted connector's fragment), it is theirs; for someone else,
/// through another agent's door, or carried by another agent of the chat,
/// it is what it would be without the mark (from that agent, a hand-off at
/// most, never its owner's).
#[test]
fn a_relayed_record_is_its_owners_through_any_door_never_another_agents() {
    let (a, b) = (agent("juniper"), agent("maple"));
    let v = view(&[&a, &b]);
    let mut e = engine(&[a.clone(), b.clone()]);
    let mine = |text: &str| records::relayed_message(text, &a, records::VIA_SIMPLEX, &[]);
    let mut now = T0;
    let mut owners = |e: &mut Engine, seq: u64, principal: &str, body: Value| {
        now += 1;
        let t = started(&said(e, &a, &v, seq, principal, body, now)).expect("its owner's message, a turn");
        assert_eq!(t.asker, "npub1paul", "asked as its owner");
        ev(e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, now);
    };
    // no SimpleX link bound here: its owner's all the same
    owners(&mut e, 1, &a.identity, mine("not linked here"));
    // through another door
    let mut telegram = mine("from telegram");
    telegram["relayed"]["via"] = json!("telegram");
    owners(&mut e, 2, &a.identity, telegram.clone());
    // carried by a key that is none of the chat's agents: a hosted connector
    owners(&mut e, 3, "npub1telegramconnector", telegram);
    // posted by its owner with the mark: their own message
    owners(&mut e, 4, "npub1paul", mine("paul typed this"));
    // linked, then let go: what the platform took stays theirs
    relaying(&mut e, &[&a]);
    owners(&mut e, 5, &a.identity, mine("linked"));
    e.step(Input::Relaying(BTreeMap::new()), T0 + 20);
    owners(&mut e, 6, &a.identity, mine("unlinked since"));
    // for someone not its owner, or through another agent's door: nothing
    let skyler = records::relayed_message("for skyler", &Agent { owner: "npub1skyler".into(), ..a.clone() }, records::VIA_SIMPLEX, &[]);
    assert!(started(&said(&mut e, &a, &v, 7, &a.identity, skyler, T0 + 30)).is_none());
    let maples = records::relayed_message("maple's door", &Agent { fragment: b.fragment.clone(), ..a.clone() }, records::VIA_SIMPLEX, &[]);
    assert!(started(&said(&mut e, &a, &v, 8, &a.identity, maples, T0 + 30)).is_none());
    // a `via` that is no door's id: the agent's own words, as ever
    let mut bad = mine("bad door");
    bad["relayed"]["via"] = json!("../simplex");
    assert!(started(&said(&mut e, &a, &v, 9, &a.identity, bad, T0 + 30)).is_none());
    // carried by another agent of the chat: a hand-off at most, never its owner's
    assert!(started(&said(&mut e, &b, &v, 10, &a.identity, mine("juniper's, not maple's"), T0 + 31)).is_none(), "not to maple at all");
    let s = said(&mut e, &a, &v, 11, &b.identity, mine("a hand-off"), T0 + 32);
    let t = started(&s).expect("from maple to juniper: a hand-off");
    assert_eq!(t.asker, b.identity, "asked by maple, never as its owner");
}

/// Goal (Paul, 2026-10-26: SimpleX is a chat of its own): every turn in
/// an agent's SimpleX chat while linked is handed as that chat's
/// (`TurnStart::simplex`), whatever started it (its owner's phone, their
/// words typed there, a routine), and so is an aside said there; a turn in
/// any other chat, or while its chat is not found yet, or once unlinked, is
/// not; `mirror` names the chat exactly then.
#[test]
fn turns_in_its_simplex_chat_are_its_owners_phone() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a)).with_menu(crate::runtime::script::MENU);
    let in_chat = |e: &mut Engine, chat: &str, seq: u64, principal: &str, body: Value, now: u64| {
        let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: chat.into(), record: rec(seq, principal, body), view: Some(v.clone()), since: 0 }, now);
        let s = answered(e, s, now);
        let t = started(&s);
        if let Some(t) = &t {
            ev(e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, now);
        }
        t
    };
    // linked, its chat not found yet: no chat is its phone
    e.step(Input::Relaying(BTreeMap::from([(a.fragment.clone(), None)])), T0);
    assert_eq!(e.mirror(&a.fragment), None);
    assert!(!in_chat(&mut e, "talk--k3x9", 1, "npub1paul", json!({ "text": "early" }), T0).expect("a turn").simplex);
    // found: there, every turn, whatever started it; elsewhere, none
    relaying(&mut e, &[&a]);
    assert_eq!(e.mirror(&a.fragment), Some("talk--k3x9"));
    assert!(in_chat(&mut e, "talk--k3x9", 2, &a.identity, records::relayed_message("from my phone", &a, records::VIA_SIMPLEX, &[]), T0 + 1).expect("a turn").simplex);
    assert!(in_chat(&mut e, "talk--k3x9", 3, "npub1paul", json!({ "text": "typed on the web" }), T0 + 2).expect("a turn").simplex);
    let routine = rec(1, &a.identity, json!({ "kind": "routine", "text": "water the plants", "chat": "talk--k3x9" }));
    let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: a.fragment.clone(), record: Record { channel: records::TASKS.into(), ..routine }, view: None, since: 0 }, T0 + 3);
    let t = started(&answered(&mut e, s, T0 + 3)).expect("its routine, a turn");
    assert!(t.simplex && t.routine, "a routine there too");
    ev(&mut e, Event::End { turn: t.turn.clone(), outcome: Outcome::Idle }, T0 + 3);
    assert!(!in_chat(&mut e, "other--k3x9", 1, "npub1paul", json!({ "text": "another chat" }), T0 + 4).expect("a turn").simplex);
    // an aside said there is that chat's too
    let s = e.step(Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: rec(4, "npub1paul", json!({ "kind": "command", "command": "btw", "args": "how is it going?" })), view: Some(v.clone()), since: 0 }, T0 + 5);
    assert!(commands(&s).iter().any(|c| matches!(c, Command::Aside { simplex: true, .. })), "{:?}", commands(&s));
    // unlinked: no more
    e.step(Input::Relaying(BTreeMap::new()), T0 + 6);
    assert_eq!(e.mirror(&a.fragment), None);
    assert!(!in_chat(&mut e, "talk--k3x9", 5, "npub1paul", json!({ "text": "after the unlink" }), T0 + 6).expect("a turn").simplex);
}

/// Goal: a turn writes nothing of SimpleX into the state: a computer
/// without SimpleX keeps its `state.json` as it was, and so does one with.
#[test]
fn a_plain_turn_writes_no_via() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0)).expect("a turn");
    let written = serde_json::to_string(e.state()).expect("a state serializes");
    assert!(!written.contains("via") && !written.contains("simplex"), "{written}");
}

/// Goal (never break agents): a state an older bridge wrote, a turn in it
/// marked `via` (the per-turn rule, gone), reads as before, the mark let go.
#[test]
fn an_older_states_via_is_let_go() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let t = started(&said(&mut e, &a, &v, 1, "npub1paul", json!({ "text": "hi" }), T0)).expect("a turn");
    let mut older: Value = serde_json::to_value(e.state()).expect("a state serializes");
    older["turns"][&t.turn]["via"] = json!("simplex");
    let back: State = serde_json::from_value(older).expect("an older state reads");
    assert!(back.turns.contains_key(&t.turn));
    assert!(!serde_json::to_string(&back).expect("serializes").contains("via"), "written again without it");
}

/// Goal (P6): a card outlives its turn. Its answer read by a later life
/// (or after a runtime's own end) is a fresh turn, caused by the answer,
/// told its card's turn as its note, never the old turn resumed nor a
/// Tell to another turn's question; replayed, nothing; another life's
/// claim of it, no run. Method: the journal the driver reads, from the
/// records this engine posted, through each kind of life after it: the
/// saved state, the state after a normal end, none (a lost `/data`), and
/// one whose own turn asks a question meanwhile.
#[test]
fn a_late_answer_is_a_fresh_claim_even_without_the_cache() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let initial = said(&mut e, &a, &v, 1, &a.owner, json!({"text":"publish after asking"}), T0);
    let old = started(&initial).unwrap().turn;
    let card = ev(&mut e, Event::Prompt { turn: old.clone(), prompt: "p".into(), text: "Publish?".into(), options: vec![PromptOption { id:"yes".into(), label:"Publish".into(), style:None, words:false }], ttl_ms:None }, T0);
    assert!(!e.keepalive());
    let saved = e.state().clone();
    let ended = ev(&mut e, Event::End {turn:old.clone(),outcome:Outcome::Idle}, T0);
    assert!(!kinds(&ended).contains(&"turn.prompt.closed".into()), "the card survives a normal runtime end too");
    let work: Vec<Record> = posts(&initial).into_iter().chain(posts(&card)).enumerate().map(|(i, (_, body))| Record { channel: records::WORK.into(), seq: i as u64 + 1, at: T0 as i64, principal: a.identity.clone(), kind:"message".into(), body }).collect();
    let chat = vec![Record { at:T0 as i64, ..rec(1, &a.owner, json!({"text":"publish after asking"})) }, Record { at:T0 as i64 + 1, ..rec(2, &a.owner, json!({"kind":"prompt_response","prompt":"p","option":"yes","agent":a.identity,"turn":old})) }];
    for (state, questioning) in [(saved, false), (e.state().clone(), false), (State::default(), false), (State::default(), true)] {
        let mut next = Engine::new(state, Settings::default(), "11111111111111111111111111111111").unwrap();
        next.step(Input::Agents(vec![a.clone()]), T0 + 1);
        let recovery = next.recover(T0 + 1);
        assert!(!kinds(&recovery).contains(&"turn.prompt.closed".into()), "the card outlives its turn");
        assert!(next.held().is_empty(), "no turn of this life holds it");
        next.step(Input::Runtime(Event::Connected(true)), T0 + 1);
        if questioning {
            let routine = Record { channel:records::TASKS.into(), ..rec(1, &a.owner, json!({"kind":"routine","text":"another question","chat":"talk--k3x9"})) };
            let step = next.step(Input::Record {agent:a.fragment.clone(), fragment:a.fragment.clone(),record:routine,view:None,since:0}, T0 + 1);
            let handed = answered(&mut next, step, T0 + 1);
            ev(&mut next, Event::Asked {turn:started(&handed).unwrap().turn}, T0 + 1);
        }
        let input = Input::CardRecord { agent:a.fragment.clone(), fragment:"talk--k3x9".into(), record:chat[1].clone(), view:Some(v.clone()), since:0, history: Ok(crate::history::Journal { work: work.clone(), chat: chat.clone(), cause: Some(chat[0].clone()) }) };
        let step = next.step(input.clone(), T0 + 1);
        let fresh = records::turn_id(&a.fragment, "talk--k3x9", "chat", 2);
        assert_ne!(fresh, old);
        let closed: Vec<Value> = posts(&step).into_iter().filter(|(_, b)| b["kind"] == "turn.prompt.closed").map(|(_, b)| b).collect();
        assert_eq!((closed.len(), closed[0]["turn"].as_str(), closed[0]["answerSeq"].as_u64()), (1, Some(old.as_str()), Some(2)), "the old card closed, naming the answer that rebuilds it");
        if questioning {
            assert!(commands(&step).is_empty(), "never Tell/Other into another live question");
            assert_eq!(next.state().turns[&fresh].phase, Phase::Queued);
            continue;
        }
        assert!(step.effects.iter().any(|x| matches!(x, Effect::Claim {turn,..} if turn == &fresh)));
        assert!(started(&step).is_none());
        assert!(next.step(input.clone(), T0 + 1).effects.is_empty(), "cursor replay");
        let handed = next.step(Input::Claimed { turn: fresh.clone(), answer: ClaimAnswer::Ours { seq: Some(3) } }, T0 + 1);
        let ts = started(&handed).expect("handed");
        assert_eq!(ts.text, "Answer to your earlier question “Publish?”: Publish", "the owner's answer alone");
        assert!(ts.note.as_deref().is_some_and(|n| n.contains("It was answering paul: “publish after asking”")), "its card's turn as its note: {:?}", ts.note);
        let mut other = Engine::new(State::default(), Settings::default(), "22222222222222222222222222222222").unwrap();
        other.step(Input::Agents(vec![a.clone()]), T0 + 1);
        other.recover(T0 + 1);
        other.step(input, T0 + 1);
        let denied = other.step(Input::Claimed { turn:fresh, answer:ClaimAnswer::Theirs }, T0 + 1);
        assert!(started(&denied).is_none(), "journal claim prevents another run after rollback");
    }
}

/// Goal (P6, the rollback's double run): a card answered live, its turn
/// acting on it, then a `/data` rolled back to before the answer: the
/// next life reads the answer again, and the card's closure (written live,
/// naming no answer) starts nothing. A history that could not be read is
/// closed on the card's own turn, once, and starts nothing either.
#[test]
fn a_card_answered_live_starts_nothing_after_a_rollback() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let initial = said(&mut e, &a, &v, 1, &a.owner, json!({"text":"publish after asking"}), T0);
    let old = started(&initial).unwrap().turn;
    let options = vec![PromptOption { id:"yes".into(), label:"Publish".into(), style:None, words:false }];
    let card = ev(&mut e, Event::Prompt { turn: old.clone(), prompt: "p".into(), text: "Publish?".into(), options, ttl_ms:None }, T0);
    let saved = e.state().clone();
    assert!(e.held().contains(&(a.fragment.clone(), "talk--k3x9".into(), "p".into())), "its answer is the running turn's");
    let response = json!({"kind":"prompt_response","prompt":"p","option":"yes","agent":a.identity,"turn":old});
    let live = said(&mut e, &a, &v, 2, &a.owner, response.clone(), T0 + 1);
    assert_eq!(commands(&live).len(), 1, "the running turn has its answer");
    let journal: Vec<Value> = posts(&initial).into_iter().chain(posts(&card)).chain(posts(&live)).map(|(_, b)| b).collect();
    assert!(journal.iter().any(|b| b["kind"] == "turn.prompt.closed" && b.get("answerSeq").is_none()), "a live closure names no answer");
    let work: Vec<Record> = journal.into_iter().enumerate().map(|(i, body)| Record { channel: records::WORK.into(), seq: i as u64 + 1, at: T0 as i64, principal: a.identity.clone(), kind:"message".into(), body }).collect();
    let chat = vec![Record { at:T0 as i64, ..rec(1, &a.owner, json!({"text":"publish after asking"})) }, Record { at:T0 as i64 + 1, ..rec(2, &a.owner, response) }];
    let mut back = Engine::new(saved, Settings::default(), "11111111111111111111111111111111").unwrap();
    back.step(Input::Agents(vec![a.clone()]), T0 + 2);
    back.recover(T0 + 2);
    let again = back.step(Input::CardRecord { agent:a.fragment.clone(), fragment:"talk--k3x9".into(), record:chat[1].clone(), view:Some(v.clone()), since:0, history: Ok(crate::history::Journal { work, chat: chat.clone(), cause: Some(chat[0].clone()) }) }, T0 + 2);
    assert!(posts(&again).is_empty() && commands(&again).is_empty() && !again.effects.iter().any(|x| matches!(x, Effect::Claim {..})), "nothing again: {:?}", again.effects);
    assert!(again.dirty, "its cursor passes it");

    let mut lost = engine(std::slice::from_ref(&a));
    lost.recover(T0 + 2);
    let input = |history| Input::CardRecord { agent:a.fragment.clone(), fragment:"talk--k3x9".into(), record:chat[1].clone(), view:Some(v.clone()), since:0, history };
    let unread = lost.step(input(Err(crate::history::Unavailable::Budget)), T0 + 2);
    assert_eq!(posts(&unread), vec![(records::work_id(&old, "pc:p"), json!({"kind":"turn.prompt.closed","turn":old,"prompt":"p","outcome":"history_unavailable"}))], "closed on its own turn, no notice");
    assert!(!unread.effects.iter().any(|x| matches!(x, Effect::Claim {..})));
    assert!(lost.step(input(Err(crate::history::Unavailable::Budget)), T0 + 2).effects.is_empty(), "replayed: nothing");
}

/// Goal (Alex on #660: a live answer must be in the journal before the
/// runtime acts on it): an owner's answer to a running turn's card is held
/// from the runtime until its closure's post is answered. Unanswered (a
/// deploy's 503s past the lane's tries), it is posted again at the next
/// tick and still held; on `work`, the runtime has it; a closure another
/// body holds (409) ends the runtime's wait with no answer; a restart while
/// held ends the turn, the runtime never having had it.
#[test]
fn a_live_answer_reaches_the_runtime_only_once_its_closure_is_on_work() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, &a.owner, json!({"text":"ask me"}), T0)).unwrap().turn;
    let options = || vec![PromptOption { id:"yes".into(), label:"Publish".into(), style:None, words:false }];
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p".into(), text: "Publish?".into(), options: options(), ttl_ms: None }, T0);
    let record = |seq: u64, prompt: &str| Input::Record { agent: a.fragment.clone(), fragment: "talk--k3x9".into(), record: rec(seq, &a.owner, json!({"kind":"prompt_response","prompt":prompt,"option":"yes"})), view: Some(v.clone()), since: 0 };
    let live = e.step(record(2, "p"), T0 + 1);
    let fences: Vec<&Value> = live.effects.iter().filter_map(|x| match x { Effect::Fence { prompt, body, .. } if prompt == "p" => Some(body), _ => None }).collect();
    assert_eq!(fences.len(), 1, "its closure, posted to be answered: {:?}", live.effects);
    assert!(fences[0].get("answerSeq").is_none() && fences[0]["outcome"] == "answered");
    assert!(commands(&live).is_empty(), "the runtime has nothing before the journal holds it");
    assert!(live.dirty && keepalive(&live) == Some(true), "the answer is taken, and the computer held while it lands");
    let fenced = |answer| Input::Fenced { turn: turn.clone(), prompt: "p".into(), answer };
    assert!(commands(&e.step(fenced(ClaimAnswer::Unanswered), T0 + 2)).is_empty(), "no answer from the platform: still held");
    let tick = e.step(Input::Tick, T0 + 3);
    assert!(tick.effects.iter().any(|x| matches!(x, Effect::Fence { prompt, .. } if prompt == "p")) && commands(&tick).is_empty(), "posted again at the tick");
    assert!(!e.step(Input::Tick, T0 + 4).effects.iter().any(|x| matches!(x, Effect::Fence { .. })), "once in flight, not again");
    let on_work = e.step(fenced(ClaimAnswer::Ours { seq: Some(9) }), T0 + 5);
    assert_eq!(commands(&on_work), vec![Command::Answer { turn: turn.clone(), prompt: "p".into(), option: Some("yes".into()), seq: 2, by: a.owner.clone(), words: None, attached: None }]);
    assert!(e.step(fenced(ClaimAnswer::Ours { seq: Some(9) }), T0 + 6).effects.is_empty(), "a stale answer: nothing");

    // another body under its id: the runtime's wait ends unanswered
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "q".into(), text: "Again?".into(), options: options(), ttl_ms: None }, T0 + 7);
    e.step(record(3, "q"), T0 + 8);
    let theirs = e.step(Input::Fenced { turn: turn.clone(), prompt: "q".into(), answer: ClaimAnswer::Theirs }, T0 + 9);
    assert_eq!(commands(&theirs), vec![Command::Answer { turn: turn.clone(), prompt: "q".into(), option: None, seq: 0, by: String::new(), words: None, attached: None }]);

    // a restart while held: the turn ends, its answer never given
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "r".into(), text: "Last?".into(), options: options(), ttl_ms: None }, T0 + 10);
    e.step(record(4, "r"), T0 + 11);
    let mut next = Engine::new(e.state().clone(), Settings::default(), "11111111111111111111111111111111").unwrap();
    next.step(Input::Agents(vec![a.clone()]), T0 + 12);
    let recovered = next.recover(T0 + 12);
    assert!(!recovered.effects.iter().any(|x| matches!(x, Effect::Runtime(Command::Answer { .. }) | Effect::Fence { .. })), "{:?}", recovered.effects);
    assert!(next.step(Input::Fenced { turn: turn.clone(), prompt: "r".into(), answer: ClaimAnswer::Ours { seq: None } }, T0 + 13).effects.is_empty(), "the old life's answer: nothing");

    // a platform that answers no closure for a turn's idle bound: the turn
    // ends, its answer never given
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, &a.owner, json!({"text":"ask me"}), T0)).unwrap().turn;
    ev(&mut e, Event::Prompt { turn: turn.clone(), prompt: "p".into(), text: "Publish?".into(), options: options(), ttl_ms: None }, T0);
    e.step(record(2, "p"), T0 + 1);
    let idle = 600_000; // engine()'s bound
    for at in [T0 + 2, T0 + idle / 2, T0 + idle] {
        assert!(commands(&e.step(Input::Fenced { turn: turn.clone(), prompt: "p".into(), answer: ClaimAnswer::Unanswered }, at)).is_empty());
        e.step(Input::Tick, at);
    }
    let over = e.step(Input::Tick, T0 + 2 + idle);
    assert_eq!(commands(&over), vec![Command::Forget { turn: turn.clone() }], "ended, never answered");
    assert!(kinds(&over).contains(&"turn.end".to_string()));
}

#[test]
fn a_child_holds_even_when_its_parent_waits_on_a_person() {
    let a = agent("juniper");
    let v = view(&[&a]);
    let mut e = engine(std::slice::from_ref(&a));
    let turn = started(&said(&mut e, &a, &v, 1, &a.owner, json!({"text":"ask me"}), T0)).unwrap().turn;
    ev(&mut e, Event::Prompt { turn, prompt:"p".into(), text:"Publish?".into(), options:vec![PromptOption {id:"yes".into(),label:"Publish".into(),style:None,words:false}], ttl_ms:None }, T0);
    assert!(!e.keepalive());
    assert_eq!(keepalive(&ev(&mut e, Event::Working(true), T0)), Some(true));
    assert_eq!(keepalive(&ev(&mut e, Event::Working(false), T0)), Some(false));
}
