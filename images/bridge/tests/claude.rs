//! The bridge's `claude` runtime against the fake platform and `fake-claude`
//! replaying Claude Code's wire as the real `claude` spoke it
//! (tests/support/fake_claude.rs; the recordings are tests/claude/<scenario>/).
//! Method: an agent whose `agent.json` names `claude`, a chat, things said
//! there as a person says them, and the chat's records read back. Each
//! scenario's steps are the same recording and replaying.
//!
//! To record them again, with the real `claude` signed in (Haiku 5.5 alone:
//! the fake refuses any other model), one at a time:
//!
//! ```text
//! FRAGMENT_CLAUDE_RECORD=$(which claude) cargo test -p fragment-bridge --test claude -- --test-threads 1
//! ```
//!
//! Recording uses the signed-in user's own `CLAUDE_CONFIG_DIR` (default
//! `~/.claude`, or `FRAGMENT_CLAUDE_RECORD_CONFIG`), never a copy of their
//! login, and isolated from their settings and MCP servers.
//!
//! An interleaving no live run makes on cue (a turn's late output after the
//! bridge let it go) is a composed script instead, written by hand from
//! recorded lines' shapes (tests/claude-composed/<scenario>/): replayed
//! alike, never recorded.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use fragment_bridge::runtime::claude::{Claude, ClaudeConfig};
use fragment_bridge::runtime::{Runtimes, DEFAULT};
use support::fake::{person, Fake, World};

/// The recordings, one directory a scenario.
const RECORDINGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/claude");

/// The composed scripts, one directory a scenario.
const COMPOSED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/claude-composed");

/// The real `claude` to record with, when recording.
fn recording() -> Option<PathBuf> {
    std::env::var_os("FRAGMENT_CLAUDE_RECORD").map(PathBuf::from)
}

/// How long a step may take: a real model's answer, or a replay's.
fn wait() -> u64 {
    if recording().is_some() {
        120_000
    } else {
        8_000
    }
}

/// A scenario's world: the fake platform, an agent on `claude`, its chat,
/// and the directory its bridges run in.
struct Scenario {
    name: &'static str,
    fake: Fake,
    chat: String,
    dir: PathBuf,
    bin: PathBuf,
    config_dir: PathBuf,
    settings: fragment_bridge::engine::Settings,
    drain: Duration,
}

impl Scenario {
    /// A recorded scenario: recorded again when recording.
    async fn new(name: &'static str) -> Scenario {
        Scenario::open(name, Path::new(RECORDINGS).join(name), recording(), support::settings(), Duration::from_millis(fragment_bridge::runtime::claude::DRAIN_MS_DEFAULT)).await
    }

    /// A composed scenario: always replayed, its bridge's turns let go of
    /// after `turn_idle_ms` quiet, and a let-go turn given `drain` to end.
    async fn composed(name: &'static str, turn_idle_ms: u64, drain: Duration) -> Scenario {
        let settings = fragment_bridge::engine::Settings { turn_idle_ms, ..support::settings() };
        Scenario::open(name, Path::new(COMPOSED).join(name), None, settings, drain).await
    }

    async fn open(name: &'static str, script: PathBuf, recording: Option<PathBuf>, settings: fragment_bridge::engine::Settings, drain: Duration) -> Scenario {
        let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
        fake.with(|w| {
            w.fragments.get_mut("juniper--k3x9").expect("its fragment").files.insert("agent.json".into(), bytes::Bytes::from(r#"{"runtime":"claude"}"#));
        });
        let chat = fake.chat("talk", &["juniper"]);
        let dir = support::dir(&format!("claude-{name}"));
        let (mode, config_dir) = match recording {
            Some(real) => {
                let _ = std::fs::remove_dir_all(&script);
                let config = std::env::var_os("FRAGMENT_CLAUDE_RECORD_CONFIG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").expect("HOME")).join(".claude"));
                (json!({ "mode": "record", "real": real, "out": script }), config)
            }
            None => {
                // a replay takes each recording once: its markers go
                let copy = dir.join("script");
                std::fs::create_dir_all(&copy).expect("a script dir");
                for e in std::fs::read_dir(&script).unwrap_or_else(|e| panic!("{}: {e}", script.display())) {
                    let p = e.expect("an entry").path();
                    if p.extension().is_some_and(|x| x == "jsonl") {
                        std::fs::copy(&p, copy.join(p.file_name().expect("a name"))).expect("copied");
                    }
                }
                (json!({ "mode": "replay", "script": copy }), dir.join("claude-config"))
            }
        };
        let bin_dir = dir.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("its bin");
        let bin = bin_dir.join("claude");
        std::fs::copy(env!("CARGO_BIN_EXE_fake-claude"), &bin).expect("fake-claude copied");
        std::fs::write(bin_dir.join("fake-claude.json"), mode.to_string()).expect("its mode");
        Scenario { name, fake, chat, dir, bin, config_dir, settings, drain }
    }

    fn runtimes(&self) -> Runtimes {
        let claude = Claude {
            config: ClaudeConfig {
                bin: self.bin.clone(),
                config_dir: self.config_dir.clone(),
                work_dir: self.dir.join("work"),
                state_dir: self.dir.join("claude-state"),
                model: Some("claude-haiku-5-5".into()),
                idle: Duration::from_secs(60),
                drain: self.drain,
                run_as: None,
            },
        };
        Runtimes::new().with(DEFAULT, support::script()).with("claude", Box::new(claude))
    }

    fn start(&self) -> support::Running {
        support::start_runtimes(support::config(&self.fake.url(), &self.dir, self.settings), self.runtimes())
    }

    /// The agent's working directory, where its tools run.
    fn work(&self) -> PathBuf {
        let w = self.dir.join("work").join("juniper--k3x9");
        std::fs::create_dir_all(&w).expect("its work");
        w
    }

    fn say(&self, body: Value) -> Value {
        self.fake.say(&self.chat, &person("paul"), body)
    }

    async fn ended(&self, n: usize, what: &str) {
        self.fake.until(wait(), what, |w| w.bodies(&self.chat, "work", "turn.end").len() >= n).await;
    }

    fn with<T>(&self, f: impl FnOnce(&World) -> T) -> T {
        self.fake.with(|w| f(w))
    }

    fn replies(&self) -> Vec<String> {
        self.with(|w| w.bodies(&self.chat, "chat", "reply").iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect())
    }

    fn ends(&self) -> Vec<Value> {
        self.with(|w| w.bodies(&self.chat, "work", "turn.end"))
    }

    async fn following(&self) {
        self.fake.until(wait(), "the bridge to follow its channels", |w| w.live_sockets() >= 2).await;
    }

    fn done(&self) {
        eprintln!("claude scenario {} done", self.name);
    }
}

/// Goal: a message is answered by Claude Code: its words post as the
/// turn's reply, and the turn ends idle. (That they stream as the draft is
/// claude.rs's `its_words_stream_as_the_draft`: a replay comes at once, and
/// the driver paces drafts and drops one a reply overtakes.)
#[tokio::test]
async fn a_reply() {
    let s = Scenario::new("reply").await;
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Reply with exactly the word: hello" }));
    s.ended(1, "the turn's end").await;
    assert!(s.replies().iter().any(|r| r.to_lowercase().contains("hello")), "{:?}", s.replies());
    assert_eq!(s.ends()[0]["outcome"], "idle");
    bridge.stop().await;
    s.done();
}

/// Goal: a tool call is a step: its tool, what it ran, that it completed,
/// and its call's id; then the reply.
#[tokio::test]
async fn a_tool_call_is_a_step() {
    let s = Scenario::new("tool").await;
    s.work();
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Use the Bash tool to run exactly: echo fragment-ok — then reply with just: done" }));
    s.ended(1, "the turn's end").await;
    let steps = s.with(|w| w.bodies(&s.chat, "work", "turn.step"));
    let bash = steps.iter().find(|st| st["tool"] == "Bash").unwrap_or_else(|| panic!("a Bash step: {steps:?}"));
    assert_eq!((bash["args"].as_str(), bash["category"].as_str(), bash["ok"].as_bool(), bash["status"].as_str()), (Some("echo fragment-ok"), Some("shell"), Some(true), Some("completed")));
    assert!(bash["tool_call_id"].as_str().is_some_and(|id| id.starts_with("toolu_")), "{bash}");
    assert!(!s.replies().is_empty());
    assert_eq!(s.ends()[0]["outcome"], "idle");
    bridge.stop().await;
    s.done();
}

/// Goal: `AskUserQuestion` is a card: its question, its options and
/// "Other" in words, by its call's id; its owner's answer goes back to
/// Claude Code as the tool's answer, and the turn goes on with it.
#[tokio::test]
async fn a_question_is_a_card() {
    let s = Scenario::new("ask").await;
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Use the AskUserQuestion tool to ask me one question: do I prefer red or blue, with the options Red and Blue. Then reply with just the colour I chose." }));
    s.fake.until(wait(), "the card", |w| !w.bodies(&s.chat, "work", "turn.prompt").is_empty()).await;
    let card = s.with(|w| w.bodies(&s.chat, "work", "turn.prompt")[0].clone());
    let labels: Vec<&str> = card["options"].as_array().expect("its options").iter().filter_map(|o| o["label"].as_str()).collect();
    assert_eq!(labels, vec!["Red", "Blue", "Other"], "{card}");
    assert!(card["prompt"].as_str().is_some_and(|p| p.starts_with("toolu_") && p.ends_with("-q0")), "{card}");
    s.say(json!({ "kind": "prompt_response", "prompt": card["prompt"], "option": "o1" }));
    s.ended(1, "the turn's end").await;
    assert!(s.replies().iter().any(|r| r.to_lowercase().contains("blue")), "{:?}", s.replies());
    assert_eq!(s.ends()[0]["outcome"], "idle");
    bridge.stop().await;
    s.done();
}

/// A script that runs 25 seconds, writing a line a second.
const SLOW: &str = "import time\nfor i in range(25):\n    open('progress.txt', 'a').write(f'{i}\\n')\n    time.sleep(1)\nopen('marker.txt', 'w').write('landed')\n";

const RUN_SLOW: &str = "Run exactly this one bash command in the foreground (not in the background), then say done: python3 slow.py";

/// Goal: Stop is Claude Code's interrupt: the running tool stops (its
/// script never finishes), the turn ends stopped, and the same session
/// answers the next message.
#[tokio::test]
async fn stop_interrupts_the_running_tool() {
    let s = Scenario::new("stop").await;
    std::fs::write(s.work().join("slow.py"), SLOW).expect("slow.py");
    let bridge = s.start();
    s.following().await;
    let said = s.say(json!({ "text": RUN_SLOW }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &s.chat, "chat", said["seq"].as_u64().expect("a seq"));
    if recording().is_some() {
        support::until(wait(), "the tool to run", || s.work().join("progress.txt").is_file()).await;
    } else {
        s.fake.until(wait(), "the turn's start", |w| !w.bodies(&s.chat, "work", "turn.start").is_empty()).await;
    }
    s.say(json!({ "kind": "stop", "turn": turn }));
    s.ended(1, "the stopped turn's end").await;
    assert_eq!(s.ends()[0]["outcome"], "stopped");
    s.say(json!({ "text": "Reply with exactly the word: ok" }));
    s.ended(2, "the next turn's end").await;
    assert_eq!(s.ends()[1]["outcome"], "idle");
    assert!(s.replies().iter().any(|r| r.to_lowercase().contains("ok")), "{:?}", s.replies());
    if recording().is_some() {
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!s.work().join("marker.txt").exists(), "the stopped script never finished");
    }
    bridge.stop().await;
    s.done();
}

/// Goal: a chat's session outlives its process: a bridge started again
/// resumes it (the model remembers), and `/new` starts the next one (it
/// does not).
#[tokio::test]
async fn a_session_outlives_its_process_and_new_starts_another() {
    let s = Scenario::new("sessions").await;
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Remember the word maple. Reply with exactly: noted" }));
    s.ended(1, "the first turn's end").await;
    bridge.stop().await;

    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "What word did I ask you to remember? Reply with just that word." }));
    s.ended(2, "the resumed turn's end").await;
    assert!(s.replies().last().is_some_and(|r| r.to_lowercase().contains("maple")), "{:?}", s.replies());

    s.say(json!({ "kind": "command", "command": "new" }));
    s.ended(3, "/new's end").await;
    assert_eq!(s.replies().last().map(String::as_str), Some("Started a new conversation."));
    s.say(json!({ "text": "Did I ask you to remember a word in this conversation? If you don't know of one, reply with exactly: unknown" }));
    s.ended(4, "the new session's turn's end").await;
    assert!(s.replies().last().is_some_and(|r| r.to_lowercase().contains("unknown")), "{:?}", s.replies());
    bridge.stop().await;
    s.done();
}

/// Goal: the computer dying while a tool runs (the bridge and `claude`
/// killed whole) cuts the turn, which ends lost; the next message resumes
/// the session, Claude Code tells its model the cut call's outcome is
/// unknown, and the bridge's note says what was cut. Nothing reruns it.
#[tokio::test]
async fn a_crash_mid_tool_is_told_never_rerun() {
    let s = Scenario::new("crash").await;
    std::fs::write(s.work().join("slow.py"), SLOW).expect("slow.py");
    let bridge = support::start_killable_runtimes(support::config(&s.fake.url(), &s.dir, s.settings), s.runtimes());
    s.following().await;
    s.say(json!({ "text": RUN_SLOW }));
    if recording().is_some() {
        support::until(wait(), "the tool to run", || s.work().join("progress.txt").is_file()).await;
    } else {
        s.fake.until(wait(), "the turn's start", |w| !w.bodies(&s.chat, "work", "turn.start").is_empty()).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    bridge.kill().await;

    let bridge = s.start();
    s.following().await;
    s.fake.until(wait(), "the cut turn ended lost", |w| w.bodies(&s.chat, "work", "turn.end").iter().any(|e| e["error"] == "lost when the computer restarted")).await;
    s.say(json!({ "text": "Did your last command finish? Answer in one short sentence, and do not run anything." }));
    s.ended(2, "the next turn's end").await;
    let ends = s.ends();
    assert_eq!(ends.last().expect("its end")["outcome"], "idle", "{ends:?}");
    let steps = s.with(|w| w.bodies(&s.chat, "work", "turn.step"));
    assert!(steps.iter().all(|st| st["args"] != "python3 slow.py"), "the cut call is never run again: {steps:?}");
    bridge.stop().await;
    s.done();
}

/// The bridge lets a turn go (here: it went quiet past its bound) while
/// the next message waits, then hands that one at once. Goal: the let-go
/// turn's late output, still to come from its process, is never the next
/// turn's: it says nothing, and the next turn gets its own words and end
/// alone. Script: Claude Code is slow to answer the interrupt (0.5 s, while
/// the next message is handed at once; under the turns' 1.5 s bound, so the
/// next turn's wait on it is no quiet past it), then sends the let-go turn's tool
/// result, words and `result` before it reads the next message (Alex's
/// review of #812).
#[tokio::test]
async fn a_let_go_turns_late_output_is_never_the_next_turns() {
    let s = Scenario::composed("late-output", 1_500, Duration::from_secs(5)).await;
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Run a long command" }));
    s.say(json!({ "text": "And this, next" }));
    s.ended(2, "both turns' ends").await;
    let ends = s.ends();
    assert_eq!((ends[0]["outcome"].as_str(), ends[0]["error"].as_str()), (Some("error"), Some("the agent stopped answering")), "{ends:?}");
    assert_eq!(ends[1]["outcome"], "idle", "{ends:?}");
    assert_eq!(s.replies(), vec!["The next turn's own answer".to_string()], "the let-go turn's late words are no one's");
    assert!(s.with(|w| w.bodies(&s.chat, "work", "turn.step")).is_empty(), "the let-go turn's tool is no step");
    bridge.stop().await;
    s.done();
}

/// As above, but the interrupt is never answered (a hung process): past
/// `drain` the process is replaced, and the next turn resumes the session
/// in a new one, answering with its own words alone.
#[tokio::test]
async fn a_let_go_turn_whose_interrupt_goes_unanswered_has_its_process_replaced() {
    let s = Scenario::composed("unanswered-interrupt", 1_500, Duration::from_millis(500)).await;
    let bridge = s.start();
    s.following().await;
    s.say(json!({ "text": "Run a long command" }));
    s.say(json!({ "text": "And this, next" }));
    s.ended(2, "both turns' ends").await;
    let ends = s.ends();
    assert_eq!(ends[0]["error"], "the agent stopped answering", "{ends:?}");
    assert_eq!(ends[1]["outcome"], "idle", "{ends:?}");
    assert_eq!(s.replies(), vec!["The next turn's own answer".to_string()]);
    bridge.stop().await;
    s.done();
}
