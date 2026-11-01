//! An ask answered back (docs/chat-records.md, "An ask"), run whole: the
//! bridge in process against the fake API, its runtime scripted, through
//! crashes, a lost answer, a rollback, refusals and a loop. One agent
//! (juniper) asks another of its computer's (rowan) in their pair chat, as
//! `fragment ask --return-to` posts it, and is answered in its chat with
//! its owner (home) as a turn of its own there.
mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use fragment_bridge::api::Api;
use fragment_bridge::engine::LOST;
use fragment_bridge::records;
use fragment_bridge::runtime::{Command, Runtime, RuntimeFuture, RuntimeIo};
use support::fake::{person, Fake, World};
use support::Runs;

const WAIT: u64 = 12_000;
const JUNIPER: &str = "juniper--k3x9";

/// juniper's question, answered back in `home`.
fn question(home: &str, text: &str) -> Value {
    json!({ "text": text, "to": [person("rowan")], "return_to": home })
}

/// rowan's turn of the record at `seq` in `pair`.
fn asked_turn(pair: &str, said: &Value) -> String {
    records::turn_id("rowan--k3x9", pair, "chat", said["seq"].as_u64().expect("a seq"))
}

/// The answers on juniper's `tasks`, as records.
fn answers(w: &World) -> Vec<Value> {
    w.records(JUNIPER, "tasks").into_iter().filter(|r| r["body"]["kind"] == "answer").collect()
}

/// juniper's turn at home woken by the answer `a`.
fn woke(a: &Value) -> String {
    records::turn_id(JUNIPER, JUNIPER, "tasks", a["seq"].as_u64().expect("a seq"))
}

/// `turn`'s records of `kind` on `chat`'s work.
fn of(w: &World, chat: &str, kind: &str, turn: &str) -> usize {
    w.bodies(chat, "work", kind).iter().filter(|b| b["turn"] == turn).count()
}

/// Both agents follow their own `tasks` and each chat they are in.
async fn following(fake: &Fake, sockets: usize) {
    fake.until(WAIT, "the bridge to follow its channels", |w| w.live_sockets() >= sockets).await;
}

fn posts_to(w: &World, path: &str) -> usize {
    w.calls.iter().filter(|c| c.starts_with("POST ") && c.contains(path)).count()
}

/// Goal: juniper's question is rowan's turn in their chat, like any
/// message; once that turn is over, juniper (whose own turn, and CLI, are
/// long gone) is woken at home by an answer on its own `tasks`: a turn of
/// its owner's business, told what rowan said. One run each.
#[tokio::test]
async fn an_answer_wakes_its_asker_after_it_has_gone() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let runs = Runs::default();
    let dir = support::dir("ask-answer");
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), support::counting(support::script(), &runs));
    following(&fake, 5).await;
    let said = fake.say(&pair, &person("juniper"), question(&home, "What is the weather?"));
    let asked = asked_turn(&pair, &said);
    fake.until(WAIT, "juniper woken at home", |w| !w.bodies(&home, "work", "turn.end").is_empty()).await;
    fake.with(|w| {
        assert_eq!(w.bodies(&pair, "chat", "reply"), vec![json!({ "text": "echo: [Juniper] What is the weather?", "turn": asked })], "rowan answers where it was asked");
        let a = answers(w);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0]["principal"], person("juniper"), "posted by juniper's own bridge");
        let text = format!("Rowan answered your question in {pair} (\"What is the weather?\"):\n\necho: [Juniper] What is the weather?");
        assert_eq!(a[0]["body"], json!({ "kind": "answer", "chat": home, "text": text, "by": person("rowan"), "name": "Rowan", "asked": { "fragment": pair, "seq": said["seq"] }, "turn": asked, "outcome": "idle", "hop": 3 }), "rowan called by its name");
        let woke = woke(&a[0]);
        let start = &w.bodies(&home, "work", "turn.start")[0];
        assert_eq!((&start["turn"], &start["asker"], &start["cause"]), (&json!(woke), &json!(person("rowan")), &json!({ "fragment": JUNIPER, "channel": "tasks", "seq": a[0]["seq"] })), "asked as the agent that answered");
        assert_eq!(w.bodies(&home, "chat", "reply"), vec![json!({ "text": format!("echo: [Rowan] {text}"), "turn": woke })], "told by rowan");
        assert!(w.bodies(&pair, "chat", "reply").iter().all(|r| r.get("to").is_none()), "an answer hands nothing off");
    });
    let a = fake.with(|w| answers(w)[0].clone());
    assert_eq!((runs.of(&asked), runs.of(&woke(&a)), runs.all().len()), (1, 1, 2));
    bridge.stop().await;
}

/// Goal (#721): the files the asked agent sends with its answer go with
/// it: blobs of the chat it was asked in, named on the answer, which its
/// asker's turn at home reads as the asker (the platform's rule on who
/// reads that chat) and has on its computer. A question answered with no
/// files names none (the test above).
#[tokio::test]
async fn an_answer_carries_the_files_the_asked_agent_sent() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let dir = support::dir("ask-files");
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), support::script());
    following(&fake, 5).await;
    let said = fake.say(&pair, &person("juniper"), question(&home, "draw"));
    let asked = asked_turn(&pair, &said);
    fake.until(WAIT, "juniper woken at home", |w| !w.bodies(&home, "work", "turn.end").is_empty()).await;
    fake.with(|w| {
        let reply = w.bodies(&pair, "chat", "reply").into_iter().find(|r| r["turn"] == asked.as_str()).expect("rowan's reply");
        let sent = reply["attachments"].clone();
        assert_eq!(sent.as_array().map(|a| a.iter().map(|f| f["name"].clone()).collect::<Vec<_>>()), Some(vec![json!("drawing.txt")]), "rowan drew: {reply}");
        let a = &answers(w)[0]["body"];
        assert_eq!((&a["files"], &a["asked"]["fragment"]), (&sent, &json!(pair)), "the answer names them, in the chat it was asked in: {a}");
        let told = w.bodies(&home, "chat", "reply");
        assert!(told.iter().any(|r| r["text"].as_str().is_some_and(|t| t.contains("[got 1: drawing.txt]"))), "juniper had the file at home: {told:?}");
        assert!(w.calls.iter().any(|c| c.starts_with("GET ") && c.contains(&format!("/f/{pair}/blobs/"))), "read from the asked chat");
    });
    bridge.stop().await;
}

/// Goal: a crash before the answer is posted, and one after it was posted
/// but before its answer came back: the next life posts it again under the
/// same id and body (a replay when it had landed), so there is one answer,
/// rowan ran once, and juniper is woken once at most, its turn started and
/// ended once.
#[tokio::test]
async fn a_crash_before_or_after_the_answer_is_posted() {
    for after in [false, true] {
        let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
        let pair = fake.chat("pair", &["juniper", "rowan"]);
        let home = fake.chat("home", &["juniper"]);
        let runs = Runs::default();
        let dir = support::dir("ask-crash");
        let cfg = support::config(&fake.url(), &dir, support::settings());
        let tasks = format!("/api/f/{JUNIPER}/channels/tasks");
        fake.with(|w| w.fail_channel = Some(("tasks".into(), if after { 1 } else { u32::MAX }, after)));
        let bridge = support::start_killable(cfg.clone(), support::counting(support::script(), &runs));
        following(&fake, 5).await;
        let said = fake.say(&pair, &person("juniper"), question(&home, "q"));
        let asked = asked_turn(&pair, &said);
        if after {
            fake.until(WAIT, "the answer landed, its answer lost", |w| answers(w).len() == 1).await;
        } else {
            fake.until(WAIT, "the answer refused once", |w| posts_to(w, &tasks) >= 1).await;
            assert!(fake.with(|w| answers(w).is_empty()));
        }
        bridge.kill().await;
        fake.until(WAIT, "its sockets closed", |w| w.live_sockets() == 0).await;
        fake.with(|w| w.fail_channel = None);
        let bridge = support::start_killable(cfg, support::counting(support::script(), &runs));
        fake.until(WAIT, "juniper woken, its turn over", |w| answers(w).len() == 1 && of(w, &home, "turn.end", &woke(&answers(w)[0])) == 1).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        fake.with(|w| {
            let a = answers(w);
            assert_eq!(a.len(), 1, "after: {after}");
            assert_eq!(a[0]["body"]["outcome"], "idle", "rowan's turn had ended before the crash");
            assert_eq!((of(w, &home, "turn.start", &woke(&a[0])), of(w, &home, "turn.end", &woke(&a[0]))), (1, 1));
            assert_eq!((of(w, &pair, "turn.start", &asked), of(w, &pair, "turn.end", &asked)), (1, 1));
        });
        let a = fake.with(|w| answers(w)[0].clone());
        assert_eq!(runs.of(&asked), 1, "after: {after}");
        assert!(runs.of(&woke(&a)) <= 1, "after: {after}");
        bridge.kill().await;
    }
}

/// Goal: `/data` put back to before the question was read, after it was
/// answered and juniper woken: the next life reads both again and runs
/// neither (another life's claims), posts no second answer, and is told it
/// forgot them.
#[tokio::test]
async fn a_rollback_answers_once() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let runs = Runs::default();
    let dir = support::dir("ask-rollback");
    let cfg = support::config(&fake.url(), &dir, support::settings());
    let bridge = support::start_killable(cfg.clone(), support::counting(support::script(), &runs));
    following(&fake, 5).await;
    let saved = support::save_state(&dir);
    let said = fake.say(&pair, &person("juniper"), question(&home, "q"));
    let asked = asked_turn(&pair, &said);
    fake.until(WAIT, "juniper woken, its turn over", |w| answers(w).len() == 1 && of(w, &home, "turn.end", &woke(&answers(w)[0])) == 1).await;
    bridge.kill().await;
    fake.until(WAIT, "its sockets closed", |w| w.live_sockets() == 0).await;
    support::restore_state(&dir, &saved);
    let (pair_work, home_work, tasks) = (format!("/api/f/{pair}/channels/work"), format!("/api/f/{home}/channels/work"), format!("/api/f/{JUNIPER}/channels/tasks"));
    let before = fake.with(|w| (posts_to(w, &pair_work), posts_to(w, &home_work), posts_to(w, &tasks)));
    let bridge = support::start_killable(cfg, support::counting(support::script(), &runs));
    // rowan's claim and its end (as lost), its answer (lost), juniper's
    // claim and its end: each 409, another life's
    fake.until(WAIT, "both read again", |w| posts_to(w, &pair_work) >= before.0 + 2 && posts_to(w, &tasks) > before.2 && posts_to(w, &home_work) >= before.1 + 2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let a = fake.with(|w| {
        let a = answers(w);
        assert_eq!(a.len(), 1, "one answer");
        assert_eq!(a[0]["body"]["outcome"], "idle", "the first life's");
        assert_eq!((of(w, &pair, "turn.start", &asked), of(w, &pair, "turn.end", &asked)), (1, 1));
        assert_eq!((of(w, &home, "turn.start", &woke(&a[0])), of(w, &home, "turn.end", &woke(&a[0]))), (1, 1));
        a[0].clone()
    });
    assert_eq!((runs.of(&asked), runs.of(&woke(&a)), runs.all().len()), (1, 1, 2));
    bridge.kill().await;
}

/// Goal: rowan's turn cut by a crash (waiting on its owner's card) is
/// answered lost, never run again for a better answer.
#[tokio::test]
async fn a_cut_turn_is_answered_lost() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let runs = Runs::default();
    let dir = support::dir("ask-cut");
    let cfg = support::config(&fake.url(), &dir, support::settings());
    let bridge = support::start_killable(cfg.clone(), support::counting(support::script(), &runs));
    following(&fake, 5).await;
    let said = fake.say(&pair, &person("juniper"), question(&home, "risky"));
    let asked = asked_turn(&pair, &said);
    fake.until(WAIT, "rowan waiting on its owner", |w| of(w, &pair, "turn.prompt", &asked) == 1).await;
    bridge.kill().await;
    fake.until(WAIT, "its sockets closed", |w| w.live_sockets() == 0).await;
    let bridge = support::start_killable(cfg, support::counting(support::script(), &runs));
    fake.until(WAIT, "juniper woken", |w| answers(w).len() == 1 && of(w, &home, "turn.start", &woke(&answers(w)[0])) == 1).await;
    fake.with(|w| {
        let a = &answers(w)[0]["body"];
        assert_eq!((&a["outcome"], &a["error"], &a["name"]), (&json!("error"), &json!(LOST), &json!("Rowan")), "called by its name though no view is read yet");
        assert_eq!(a["text"], format!("Rowan could not answer your question in {pair} (\"risky\"): {LOST}."));
    });
    assert_eq!(runs.of(&asked), 1, "never run again");
    bridge.kill().await;
}

/// Goal (Paul, 2026-10-30): juniper's answer turn is asked as rowan, and
/// is its owner's business at home still: paul's Stop stops one, and his
/// words answer one that asks him something (never a turn of their own).
/// Rowan's own turn, asked by juniper, that asks back in words is told no
/// answer is coming, and goes on.
#[tokio::test]
async fn an_answers_owner_stops_it_and_answers_its_question() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let runs = Runs::default();
    let dir = support::dir("ask-owner");
    // slow turns take 4 s: time for a Stop
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), support::counting(support::script_paced(Duration::from_millis(200)), &runs));
    following(&fake, 5).await;
    let started = |w: &World, turn: &str| of(w, &home, "turn.start", turn) == 1;

    fake.say(&pair, &person("juniper"), question(&home, "slow"));
    fake.until(WAIT * 2, "juniper's slow answer turn", |w| answers(w).first().is_some_and(|a| started(w, &woke(a)))).await;
    let stopped = fake.with(|w| woke(&answers(w)[0]));
    fake.with(|w| assert_eq!(w.bodies(&home, "work", "turn.start")[0]["asker"], person("rowan")));
    fake.say(&home, &person("paul"), json!({ "kind": "stop", "turn": stopped }));
    fake.until(WAIT, "paul's Stop", |w| w.bodies(&home, "work", "turn.end").iter().any(|e| e["turn"] == stopped.as_str() && e["outcome"] == "stopped")).await;

    fake.say(&pair, &person("juniper"), question(&home, "x ask-me x"));
    fake.until(WAIT, "juniper's answer turn asking paul", |w| answers(w).len() == 2 && w.bodies(&home, "work", "turn.asked").iter().any(|q| q["turn"] == woke(&answers(w)[1]).as_str())).await;
    let (asking, rowans) = fake.with(|w| {
        let a = answers(w)[1].clone();
        assert!(a["body"]["text"].as_str().is_some_and(|t| t.contains("What should I call it?") && t.contains(&format!("(told: {})", fragment_bridge::engine::ASKED_BACK))), "rowan, asked by an agent, was told no answer is coming and went on: {a}");
        assert_eq!(w.bodies(&home, "work", "turn.asked").iter().find(|q| q["turn"] == woke(&a).as_str()).unwrap()["asks"], person("paul"), "it waits for paul");
        (woke(&a), a["body"]["turn"].as_str().unwrap().to_string())
    });
    let basil = fake.say(&home, &person("paul"), json!({ "text": "Basil" }));
    fake.until(WAIT, "paul's words told to it", |w| w.bodies(&home, "chat", "reply").iter().any(|r| r["turn"] == asking.as_str() && r["text"].as_str().is_some_and(|t| t.ends_with("(told: Basil)")))).await;
    let own = fragment_bridge::records::turn_id(JUNIPER, &home, "chat", basil["seq"].as_u64().unwrap());
    assert_eq!((runs.of(&asking), runs.of(&rowans), runs.of(&own)), (1, 1, 0), "each ran once; paul's words started no turn");
    bridge.stop().await;
}

/// Goal: rowan may not answer where it was asked (held below editor
/// there): juniper is told so. Invalid: an answer back to a chat juniper is
/// not in runs nothing there.
#[tokio::test]
async fn refusals_on_either_side() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper"]);
    fake.with(|w| w.fragments.get_mut(&pair).unwrap().members.push(support::fake::Member { principal: person("rowan"), role: "viewer".into(), kind: "agent".into(), added_at: 20 }));
    let home = fake.chat("home", &["juniper"]);
    let theirs = fake.chat("theirs", &[]);
    let runs = Runs::default();
    let dir = support::dir("ask-refused");
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), support::counting(support::script(), &runs));
    following(&fake, 5).await;
    let said = fake.say(&pair, &person("juniper"), question(&home, "q"));
    fake.until(WAIT, "juniper told", |w| !w.bodies(&home, "chat", "reply").is_empty()).await;
    fake.with(|w| {
        let a = answers(w);
        assert_eq!((&a[0]["body"]["outcome"], &a[0]["body"]["error"]), (&json!("error"), &json!(format!("it may not answer in {pair}"))));
        assert!(w.records(&pair, "work").is_empty(), "nothing where rowan may not post");
    });
    assert_eq!(runs.of(&asked_turn(&pair, &said)), 0);

    let said = fake.say(&pair, &person("juniper"), question(&theirs, "q"));
    fake.until(WAIT, "the second answer", |w| answers(w).len() == 2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    fake.with(|w| assert!(w.records(&theirs, "work").is_empty() && w.records(&theirs, "chat").is_empty(), "nothing in a chat juniper is not in"));
    let a = fake.with(|w| answers(w)[1].clone());
    assert_eq!(a["body"]["asked"]["seq"], said["seq"]);
    assert_eq!(runs.of(&woke(&a)), 0);
    bridge.stop().await;
}

/// `inner`, but each turn of juniper's first asks rowan again, in `pair`,
/// to be answered back in that turn's chat (as `fragment ask --return-to`
/// does from a tool call), and holds the turn until the bridge has read
/// it: so the question is read while the turn that asked it runs.
struct AskingAgain {
    inner: Box<dyn Runtime>,
    world: Arc<Mutex<World>>,
    api: String,
    pair: String,
}

impl Runtime for AskingAgain {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn run(self: Box<Self>, io: RuntimeIo) -> RuntimeFuture {
        let RuntimeIo { mut commands, events, shutdown } = io;
        let (to_inner, inner_commands) = mpsc::channel::<Command>(256);
        let AskingAgain { inner, world, api, pair } = *self;
        let inner = inner.run(RuntimeIo { commands: inner_commands, events, shutdown });
        Box::pin(async move {
            tokio::spawn(async move {
                let api = Api::new(&api).expect("the fake's address");
                // bounded by the bridge: ends when it drops its sender
                while let Some(c) = commands.recv().await {
                    if let Command::Start(ts) = &c {
                        if ts.agent.fragment == JUNIPER {
                            let posted = api.post(JUNIPER, &pair, "chat", &format!("ask-{}", ts.turn), &question(&ts.fragment, "again?")).await.expect("asked");
                            let seq = posted.seq.expect("its seq");
                            let read = |w: &World| w.bodies(&pair, "work", "turn.start").iter().any(|b| b["cause"]["seq"] == seq);
                            let t = Instant::now();
                            // bounded: a question past the chain starts nothing, so at most 1.5 s
                            while !read(&world.lock().expect("the world")) && t.elapsed() < Duration::from_millis(1_500) {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        }
                    }
                    if to_inner.send(c).await.is_err() {
                        return;
                    }
                }
            });
            inner.await
        })
    }
}

/// Goal: an answer cannot restart a loop. juniper asks rowan again from
/// every turn, its answers' turns too: each answer is one hop past the
/// turn that answered, so after paul's message (hop 0) rowan answers at
/// hop 1 and 3, juniper's answers run at 2 and 3, and the question asked
/// from the last is past the chain: refused, and answered so, as a turn
/// of juniper's at home that never runs (paul sees why where he asked),
/// so nothing asks again.
#[tokio::test]
async fn an_answer_cannot_restart_a_loop() {
    let fake = Fake::start("127.0.0.1:0", &["juniper", "rowan"]).await;
    let pair = fake.chat("pair", &["juniper", "rowan"]);
    let home = fake.chat("home", &["juniper"]);
    let runs = Runs::default();
    let dir = support::dir("ask-loop");
    let asking = Box::new(AskingAgain { inner: support::script(), world: fake.world.clone(), api: fake.url(), pair: pair.clone() });
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), support::counting(asking, &runs));
    following(&fake, 5).await;
    fake.say(&home, &person("paul"), json!({ "text": "start" }));
    fake.until(WAIT, "three questions, three answers", |w| w.records(&pair, "chat").iter().filter(|r| r["principal"] == person("juniper")).count() == 3 && w.bodies(&home, "work", "turn.end").len() == 4).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    fake.with(|w| {
        assert_eq!(w.records(&pair, "chat").iter().filter(|r| r["principal"] == person("juniper")).count(), 3, "and no more");
        let ends = w.bodies(&pair, "work", "turn.end");
        assert_eq!(ends.iter().filter(|e| e["outcome"] == "idle").count(), 2, "rowan answered two");
        assert!(ends.iter().any(|e| e["error"] == fragment_bridge::engine::refused_deep()), "and refused the third: {ends:?}");
        let a = answers(w);
        assert_eq!(a.iter().map(|a| (a["body"]["hop"].clone(), a["body"]["refused"].clone())).collect::<Vec<_>>(), vec![(json!(2), json!(null)), (json!(3), json!(null)), (json!(3), json!(true))]);
        let refused = woke(&a[2]);
        let home_ends = w.bodies(&home, "work", "turn.end");
        assert_eq!(home_ends.len(), 4, "paul's turn, two answers, and the refusal");
        assert!(home_ends.iter().any(|e| e["turn"] == refused.as_str() && e["error"].as_str().is_some_and(|x| x.contains("agents handed this on"))), "said where paul asked: {home_ends:?}");
    });
    assert_eq!(runs.all().len(), 5, "the refusal ran nothing");
    bridge.stop().await;
}
