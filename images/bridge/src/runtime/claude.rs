//! The `claude` runtime: Claude Code, the person's own unmodified `claude`,
//! signed in with their own plan (docs/bridge.md, "The Claude runtime").
//! The bridge drives it over its stream-json wire, as Anthropic's Agent SDK
//! does (`-p --input-format stream-json --output-format stream-json`), and
//! never sits between it and Anthropic: no proxy, no base URL, no key, and
//! no `ANTHROPIC_*` or `CLAUDE_CODE_*` setting of anyone's reaches it.
//!
//! - **A session per agent and chat.** Its id is derived from the two and
//!   the chat's `/new` count (`session_id`), so every life names the same
//!   one. A process is started for its first turn, resuming the session
//!   when its transcript is on disk (under `CLAUDE_CONFIG_DIR`, in `/data`,
//!   so a save carries it), and stops once idle. A process that dies leaves
//!   the session to the next turn, which Claude Code itself tells that the
//!   call it cut ended unknown (the spike: durable-agent-runtimes, "The
//!   Claude spike").
//! - **A turn** is one user message. Its text streams as drafts, each text
//!   block is a reply part, each tool call's result a step with its call's
//!   id, and its `result` its end.
//! - **No technical approvals** (Paul, 2026-10-14): tools run with
//!   permissions bypassed. `AskUserQuestion` alone still asks its host
//!   (`can_use_tool`): each of its questions is a card, its options and
//!   "Other" in words; the answers go back as the tool's `answers`.
//! - **Stop** is Claude Code's own interrupt, which stops the running tool
//!   too; never a lone kill (its tools run in process groups of their own,
//!   and would outlive it).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, watch};

use super::{Agent, Command, Event, How, MenuItem, Runtime, RuntimeError, RuntimeFuture, RuntimeIo, TurnStart};
use crate::records::{self, Call, Category, Outcome, PromptOption, Step, ToolEvidence, ToolStatus};

/// Its owners' commands: a new conversation, and Stop.
pub const MENU: &[MenuItem] = &[
    MenuItem { name: "new", description: "Start a new conversation", args: None, how: How::Restart, refuse: &[] },
    MenuItem { name: "stop", description: "Stop the current reply", args: None, how: How::Stop, refuse: &[] },
];

/// The sessions a computer keeps a `claude` running for at once, at most:
/// each is a process of a few hundred MB; past this a turn is refused
/// saying so, and an idle one frees its place within `ClaudeConfig::idle`.
pub const SESSIONS_MAX: usize = 8;

/// What a line of `claude`'s stderr keeps, at most, for a turn's error.
const STDERR_TAIL_MAX_BYTES: usize = 2 * 1024;

/// How long a session's process is given to exit once its input closes,
/// before it is killed (its tools are stopped by its interrupt first).
const EXIT_GRACE_MS: u64 = 1_000;

/// How long a turn the bridge let go of is given to end on its process
/// once interrupted (`retire_turn`), before the process is replaced: an
/// interrupt answers within a second or two (the spike), and the next turn
/// waits on it.
pub const DRAIN_MS_DEFAULT: u64 = 10_000;

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// The unmodified `claude`, as Anthropic publishes it.
    pub bin: PathBuf,
    /// Its `CLAUDE_CONFIG_DIR`: the person's login and the sessions'
    /// transcripts, under `/data`.
    pub config_dir: PathBuf,
    /// Each agent's working directory is `<work_dir>/<agent fragment>`.
    pub work_dir: PathBuf,
    /// Where the runtime keeps each chat's `/new` count (`sessions.json`).
    pub state_dir: PathBuf,
    /// `--model`; none: the account's own default.
    pub model: Option<String>,
    /// A session's process stops after this long with no turn.
    pub idle: Duration,
    /// How long a turn let go of is given to end on its process before the
    /// process is replaced (`retire_turn`; `DRAIN_MS_DEFAULT`).
    pub drain: Duration,
    /// The user and group it runs as (`uid`, `gid`): the agents' own, never
    /// root, which Claude Code refuses to bypass permissions as. None: the
    /// bridge's.
    pub run_as: Option<(u32, u32)>,
}

pub struct Claude {
    pub config: ClaudeConfig,
}

impl Runtime for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn menu(&self) -> &'static [MenuItem] {
        MENU
    }

    fn run(self: Box<Self>, io: RuntimeIo) -> RuntimeFuture {
        Box::pin(run(self.config, io))
    }
}

/// A session's id: a UUID (version 4's layout) of the agent, its chat and
/// the chat's `/new` count, so every life of the bridge names the same one.
pub fn session_id(agent: &str, chat: &str, generation: u64) -> String {
    let d = Sha256::digest(format!("fragment claude session\0{agent}\0{chat}\0{generation}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = records::hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Where Claude Code keeps a session's transcript: its project directory is
/// the working directory with every character but letters and digits a
/// `-` (Claude Code's sessions docs; ours are well under its 200).
pub fn transcript(config_dir: &Path, cwd: &Path, id: &str) -> PathBuf {
    let project: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    assert!(project.len() <= 200, "a working directory Claude Code names whole: {project}");
    config_dir.join("projects").join(project).join(format!("{id}.jsonl"))
}

/// Each chat's `/new` count, by `agent|chat` (none: 0), kept in
/// `<state_dir>/sessions.json`, written whole and renamed into place. A
/// rollback takes it back with the transcripts it names.
struct Generations {
    path: PathBuf,
    of: BTreeMap<String, u64>,
}

impl Generations {
    fn load(state_dir: &Path) -> Generations {
        let path = state_dir.join("sessions.json");
        let of = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Generations { path, of }
    }

    fn of(&self, key: &str) -> u64 {
        self.of.get(key).copied().unwrap_or(0)
    }

    fn bump(&mut self, key: &str) -> std::io::Result<u64> {
        let n = self.of(key) + 1;
        self.of.insert(key.to_string(), n);
        let tmp = self.path.with_extension("json.tmp");
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&tmp, serde_json::to_vec(&self.of).expect("a map of counts"))?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(n)
    }
}

/// What the bridge tells a session's task.
enum ToSession {
    Start(Box<TurnStart>),
    Stop(String),
    Answer { turn: String, prompt: String, option: Option<String>, words: Option<String> },
    Forget(String),
}

async fn run(cfg: ClaudeConfig, mut io: RuntimeIo) -> Result<(), RuntimeError> {
    let generations = Arc::new(Mutex::new(Generations::load(&cfg.state_dir)));
    let mut sessions: HashMap<(String, String), mpsc::Sender<ToSession>> = HashMap::new();
    let mut turns: HashMap<String, (String, String)> = HashMap::new();
    let mut shutdown = io.shutdown.clone();
    // It can take a turn from its start: one it cannot run (no `claude`,
    // not signed in) ends saying why, never waits unclaimed.
    let _ = io.events.send(Event::Connected(true)).await;
    // bounded by the bridge's life: one command per pass
    loop {
        let c = tokio::select! {
            c = io.commands.recv() => c,
            _ = crate::net::stopped(&mut shutdown) => return Ok(()),
        };
        let Some(c) = c else { return Ok(()) };
        sessions.retain(|_, tx| !tx.is_closed());
        turns.retain(|_, key| sessions.contains_key(key));
        let (key, msg) = match c {
            Command::Start(ts) => {
                let key = (ts.agent.fragment.clone(), ts.fragment.clone());
                if !sessions.contains_key(&key) && sessions.len() >= SESSIONS_MAX {
                    let turn = ts.turn.clone();
                    let _ = io.events.send(Event::End { turn, outcome: Outcome::Error("too many Claude conversations are open on this computer; try again in a few minutes".into()) }).await;
                    continue;
                }
                turns.insert(ts.turn.clone(), key.clone());
                (key, ToSession::Start(ts))
            }
            Command::Stop { turn } => match turns.get(&turn) {
                Some(key) => (key.clone(), ToSession::Stop(turn)),
                None => continue,
            },
            Command::Answer { turn, prompt, option, words, .. } => match turns.get(&turn) {
                Some(key) => (key.clone(), ToSession::Answer { turn, prompt, option, words }),
                None => continue,
            },
            Command::Forget { turn } => match turns.remove(&turn) {
                Some(key) => (key, ToSession::Forget(turn)),
                None => continue,
            },
            // Claude Code never waits on words mid-turn (it asks in a reply,
            // and the answer is the next turn), and its menu has no aside.
            Command::Tell { .. } | Command::Aside { .. } => continue,
        };
        let tx = sessions.entry(key.clone()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(session(cfg.clone(), key.clone(), generations.clone(), rx, io.events.clone(), io.held.clone(), io.unquiet.clone()));
            tx
        });
        let _ = tx.send(msg).await;
    }
}

/// A session's `claude`, while it runs.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
    /// It was started resuming its session (else naming it new).
    resumed: bool,
    /// Its `system` `init` came: it started.
    started: bool,
}

/// One question of an `AskUserQuestion` call, shown as a card.
struct Asked {
    request: String,
    tool_use_id: String,
    input: Value,
    /// Per question: its card, its text, its options' labels, its answer.
    cards: Vec<(String, String, Vec<String>, Option<String>)>,
}

/// A turn of the session's, while it runs.
struct Turn {
    id: String,
    /// Its agent, as the bridge handed it (its credentials).
    agent: Agent,
    /// Its message, kept until its process has started, so a start that
    /// failed for its session's flag is tried once the other way.
    message: Option<String>,
    retried: bool,
    part: u32,
    draft: String,
    /// Tool calls made and not answered: name, input, when, and the model
    /// response (its assistant message) that asked for it.
    calls: HashMap<String, (String, Value, Instant, String)>,
    /// The response whose calls were said last, and the place its next
    /// takes: Claude Code says each of a message's blocks on a line of its
    /// own, its message's id on each.
    said: Option<(String, u32)>,
    asked: Vec<Asked>,
    stopping: bool,
}

/// A session counted while its process runs with no turn on it: what a
/// hold waits on (`RuntimeIo::unquiet`), uncounted as it stops or the
/// session ends.
struct Idle {
    counted: bool,
    unquiet: Arc<AtomicUsize>,
}

impl Idle {
    fn set(&mut self, idle: bool) {
        if idle != self.counted {
            if idle {
                self.unquiet.fetch_add(1, Ordering::SeqCst);
            } else {
                self.unquiet.fetch_sub(1, Ordering::SeqCst);
            }
            self.counted = idle;
        }
    }
}

impl Drop for Idle {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(cfg: ClaudeConfig, key: (String, String), generations: Arc<Mutex<Generations>>, mut rx: mpsc::Receiver<ToSession>, events: mpsc::Sender<Event>, mut held: watch::Receiver<bool>, unquiet: Arc<AtomicUsize>) {
    let (agent, chat) = key;
    let gen_key = format!("{agent}|{chat}");
    let cwd = cfg.work_dir.join(&agent);
    let mut proc: Option<Proc> = None;
    let mut turn: Option<Turn> = None;
    let mut idle_since = Instant::now();
    let mut requests = 0u64;
    let mut idle = Idle { counted: false, unquiet };
    // bounded by its idle stop, or the bridge dropping its sender
    loop {
        idle.set(proc.is_some() && turn.is_none());
        let idle_left = cfg.idle.saturating_sub(idle_since.elapsed());
        tokio::select! {
            m = rx.recv() => {
                let Some(m) = m else { break };
                match m {
                    ToSession::Start(ts) => {
                        // the bridge runs one turn of an agent in a chat at a
                        // time, so one still here was let go of: it ends on its
                        // process before this one starts there
                        if let Some(t) = turn.take() {
                            crate::ev!("claude.overlap", { "turn": ts.turn, "running": t.id });
                            retire_turn(&mut proc, &mut requests, cfg.drain, &t.id).await;
                        }
                        if ts.command {
                            let outcome = command(&ts, &gen_key, &generations, &mut proc, &events).await;
                            let _ = events.send(Event::End { turn: ts.turn.clone(), outcome }).await;
                            idle_since = Instant::now();
                            continue;
                        }
                        let message = message(&ts);
                        if proc.is_none() {
                            let id = session_id(&agent, &chat, generations.lock().expect("generations").of(&gen_key));
                            let resume = transcript(&cfg.config_dir, &cwd, &id).is_file();
                            match spawn(&cfg, &ts.agent, &cwd, &id, resume) {
                                Ok(p) => proc = Some(p),
                                Err(why) => {
                                    let _ = events.send(Event::End { turn: ts.turn.clone(), outcome: Outcome::Error(why) }).await;
                                    continue;
                                }
                            }
                        }
                        let p = proc.as_mut().expect("started");
                        let started = p.started;
                        if let Err(e) = write(&mut p.stdin, &user_message(&message)).await {
                            crate::ev!("claude.write", { "turn": ts.turn, "error": e.to_string() });
                        }
                        turn = Some(Turn { id: ts.turn.clone(), agent: ts.agent.clone(), message: (!started).then_some(message), retried: false, part: 1, draft: String::new(), calls: HashMap::new(), said: None, asked: Vec::new(), stopping: false });
                    }
                    ToSession::Stop(id) if turn.as_ref().is_some_and(|t| t.id == id) => {
                        // interrupted, it ends stopped at its own `result`
                        turn.as_mut().expect("checked").stopping = true;
                        if let Some(p) = proc.as_mut() {
                            requests += 1;
                            let _ = write(&mut p.stdin, &interrupt(requests)).await;
                        }
                    }
                    ToSession::Forget(id) if turn.as_ref().is_some_and(|t| t.id == id) => {
                        // the bridge ended it: it says nothing more, and ends
                        // on its process before the next turn starts there
                        let t = turn.take().expect("checked");
                        retire_turn(&mut proc, &mut requests, cfg.drain, &t.id).await;
                        idle_since = Instant::now();
                    }
                    ToSession::Answer { turn: id, prompt, option, words } if turn.as_ref().is_some_and(|t| t.id == id) => {
                        let t = turn.as_mut().expect("checked");
                        if let Some(p) = proc.as_mut() {
                            answer(t, p, &prompt, option, words).await;
                        }
                    }
                    ToSession::Stop(_) | ToSession::Forget(_) | ToSession::Answer { .. } => {}
                }
            }
            line = next_line(&mut proc) => {
                match line {
                    Some(line) => {
                        let Ok(v) = serde_json::from_str::<Value>(&line) else {
                            crate::ev!("claude.unread", { "agent": agent, "line": records::cut(&line, 200) });
                            continue;
                        };
                        if let Some(p) = proc.as_mut() {
                            if v["type"] == "system" && v["subtype"] == "init" {
                                p.started = true;
                                if let Some(t) = turn.as_mut() {
                                    t.message = None;
                                }
                            }
                        }
                        let ended = match turn.as_mut() {
                            Some(t) => on_line(t, &v, proc.as_mut().expect("a line is a process's"), &events).await,
                            None => false,
                        };
                        if ended {
                            turn = None;
                            idle_since = Instant::now();
                        }
                    }
                    None => {
                        // its process ended: a start that failed for its
                        // session's flag is tried once the other way
                        let p = proc.take().expect("a process ended");
                        let stderr = p.stderr.lock().expect("stderr").clone();
                        let resumed = p.resumed;
                        let started = p.started;
                        drop(p);
                        if let Some(t) = turn.as_mut() {
                            if !started && !t.retried && t.message.is_some() {
                                t.retried = true;
                                let id = session_id(&agent, &chat, generations.lock().expect("generations").of(&gen_key));
                                crate::ev!("claude.restart", { "turn": t.id, "resumed": !resumed, "stderr": records::cut(&stderr, 300) });
                                if let Ok(mut q) = spawn(&cfg, &t.agent, &cwd, &id, !resumed) {
                                    let m = t.message.clone().expect("checked");
                                    let _ = write(&mut q.stdin, &user_message(&m)).await;
                                    proc = Some(q);
                                    continue;
                                }
                            }
                            let t = turn.take().expect("checked");
                            let why = if stderr.trim().is_empty() { "Claude Code stopped unexpectedly".to_string() } else { format!("Claude Code stopped: {}", records::cut(stderr.trim(), 300)) };
                            let _ = events.send(Event::End { turn: t.id, outcome: if t.stopping { Outcome::Stopped } else { Outcome::Error(why) } }).await;
                        }
                        idle_since = Instant::now();
                    }
                }
            }
            _ = async { let _ = held.wait_for(|h| *h).await; }, if turn.is_none() && proc.is_some() => {
                // held for a save: its process stops now, as at its idle
                // bound, so nothing of it (an update Claude Code makes on its
                // own, among others) writes under /data while it is copied; a
                // turn running at the hold runs on, its process stopped as it
                // ends. The session stays, for a turn handed as the hold came,
                // and its next turn resumes it.
                crate::ev!("claude.held", { "agent": agent, "chat": chat });
                if let Some(p) = proc.take() {
                    stop(p).await;
                }
            }
            _ = tokio::time::sleep(idle_left), if turn.is_none() => {
                // idle: its process stops, and so does this task
                break;
            }
        }
    }
    if let Some(p) = proc.take() {
        stop(p).await;
    }
}

/// A command of the menu said as a turn: `/new` starts the chat's next
/// session (its count one on, its process stopped), and says so.
async fn command(ts: &TurnStart, gen_key: &str, generations: &Arc<Mutex<Generations>>, proc: &mut Option<Proc>, events: &mpsc::Sender<Event>) -> Outcome {
    let name = ts.text.trim().trim_start_matches('/').split_whitespace().next().unwrap_or("");
    match name {
        "new" => {
            if let Some(p) = proc.take() {
                stop(p).await;
            }
            let bumped = generations.lock().expect("generations").bump(gen_key);
            match bumped {
                Ok(_) => {
                    let _ = events.send(Event::Reply { turn: ts.turn.clone(), part: 1, text: "Started a new conversation.".into() }).await;
                    Outcome::Idle
                }
                Err(e) => Outcome::Error(format!("could not start a new conversation: {e}")),
            }
        }
        other => Outcome::Error(format!("/{other} is not one of this agent's commands")),
    }
}

/// What a turn's message says to Claude: what it should know first (the
/// note), the message it quotes, who said it, and the files sent with it.
fn message(ts: &TurnStart) -> String {
    let mut out = String::new();
    if let Some(note) = &ts.note {
        out.push_str(&format!("[Recent channel messages]\n{note}\n\n[New message]\n"));
    }
    if let Some(q) = &ts.quote {
        let whose = if q.own { "your previous message" } else { "" };
        if whose.is_empty() {
            out.push_str(&format!("[Replying to: \"{}\"]\n", q.text));
        } else {
            out.push_str(&format!("[Replying to {whose}: \"{}\"]\n", q.text));
        }
    }
    out.push_str(&format!("[{}] {}", ts.asker_name, ts.text));
    for f in &ts.files {
        out.push_str(&format!("\n[{} sent a file: '{}' ({}). It is on your computer at: {}]", ts.asker_name, f.name, f.media_type, f.path.display()));
    }
    let unread = ts.attachments.len().saturating_sub(ts.files.len());
    if unread > 0 {
        out.push_str(&format!("\n[{} also sent {unread} file{} that could not be read]", ts.asker_name, if unread == 1 { "" } else { "s" }));
    }
    out
}

fn user_message(text: &str) -> Value {
    json!({ "type": "user", "message": { "role": "user", "content": text }, "parent_tool_use_id": null })
}

/// The variables of the bridge's own environment `claude` never gets: any
/// that would change whom it signs in as, where it calls, or what it does
/// (the person's own plan, unmodified), and the bridge's.
fn withheld(name: &str) -> bool {
    name.starts_with("ANTHROPIC_") || name.starts_with("CLAUDE_CODE_") || name.starts_with("BRIDGE_") || matches!(name, "CLAUDECODE" | "CLAUDE_CONFIG_DIR" | "MCP_TOOL_TIMEOUT")
}

/// Starts the session's `claude`: resuming it, or naming it new.
fn spawn(cfg: &ClaudeConfig, agent: &Agent, cwd: &Path, id: &str, resume: bool) -> Result<Proc, String> {
    std::fs::create_dir_all(cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut c = tokio::process::Command::new(&cfg.bin);
    c.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    c.args(["--permission-mode", "bypassPermissions", "--allow-dangerously-skip-permissions", "--permission-prompt-tool", "stdio"]);
    if let Some(m) = &cfg.model {
        c.args(["--model", m]);
    }
    c.arg(if resume { "--resume" } else { "--session-id" }).arg(id);
    c.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_none_or(withheld) {
            c.env_remove(&name);
        }
    }
    c.env("CLAUDE_CONFIG_DIR", &cfg.config_dir);
    // the agent's connections, as every runtime's tools get them: a
    // placeholder its computer's egress swaps (never a model's key)
    for cred in &agent.credentials {
        for e in cred.env.iter().filter(|e| !withheld(e)) {
            c.env(e, &cred.placeholder);
        }
    }
    if let Some((uid, gid)) = cfg.run_as {
        c.uid(uid).gid(gid);
    }
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "Claude Code isn't installed on this computer".to_string(),
        _ => format!("Claude Code did not start: {e}"),
    })?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let (tx, lines) = mpsc::channel(256);
    tokio::spawn(async move {
        let mut r = BufReader::new(stdout).lines();
        // bounded by the process: ends with its output
        while let Ok(Some(l)) = r.next_line().await {
            if tx.send(l).await.is_err() {
                return;
            }
        }
    });
    let stderr = Arc::new(Mutex::new(String::new()));
    let tail = stderr.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        // bounded by the process: ends with its output
        while let Ok(n) = err.read(&mut buf).await {
            if n == 0 {
                return;
            }
            let mut t = tail.lock().expect("stderr");
            t.push_str(&String::from_utf8_lossy(&buf[..n]));
            if t.len() > STDERR_TAIL_MAX_BYTES {
                let cut = t.len() - STDERR_TAIL_MAX_BYTES;
                let at = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                t.drain(..at);
            }
        }
    });
    crate::ev!("claude.start", { "agent": agent.fragment, "session": id, "resume": resume });
    Ok(Proc { child, stdin, lines, stderr, resumed: resume, started: false })
}

async fn write(stdin: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(v).expect("a JSON value");
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await
}

/// The next line of the running process's output; none once it ended. With
/// no process, never.
async fn next_line(proc: &mut Option<Proc>) -> Option<String> {
    match proc.as_mut() {
        Some(p) => p.lines.recv().await,
        None => std::future::pending().await,
    }
}

/// Claude Code's interrupt, the `n`th this session asked for.
fn interrupt(n: u64) -> Value {
    json!({ "type": "control_request", "request_id": format!("stop-{n}"), "request": { "subtype": "interrupt" } })
}

/// A turn the bridge let go of (`Forget`: it went quiet past its bound, or
/// its agent left), or one a Start found still running, ends on its
/// process before any other turn starts there: interrupted, its remaining
/// output read and dropped until its `result`. One that gives no `result`
/// within `drain` (an interrupt unanswered, a process hung) has its process
/// replaced, stopped and killed past a grace, and the next turn resumes the
/// session in a new one. So one turn's words and end are never another's
/// (Alex's review of #812). Holds the session's other commands meanwhile,
/// as the next turn's start must wait.
async fn retire_turn(proc: &mut Option<Proc>, requests: &mut u64, drain: Duration, turn: &str) {
    let Some(p) = proc.as_mut() else { return };
    *requests += 1;
    let _ = write(&mut p.stdin, &interrupt(*requests)).await;
    let lines = &mut p.lines;
    let ended = tokio::time::timeout(drain, async {
        // bounded by the drain's timeout, or the process's output ending
        while let Some(line) = lines.recv().await {
            if serde_json::from_str::<Value>(&line).is_ok_and(|v| v["type"] == "result") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    crate::ev!("claude.retired", { "turn": turn, "ended": ended });
    if !ended {
        if let Some(p) = proc.take() {
            stop(p).await;
        }
    }
}

/// A session's process stops: its input closes (it finishes and exits),
/// and past a grace it is killed.
async fn stop(mut p: Proc) {
    drop(p.stdin);
    if tokio::time::timeout(Duration::from_millis(EXIT_GRACE_MS), p.child.wait()).await.is_err() {
        let _ = p.child.start_kill();
        let _ = p.child.wait().await;
    }
}

/// One line of `claude`'s output, for its running turn: what it means to
/// the bridge, sent; true once the turn ended.
async fn on_line(t: &mut Turn, v: &Value, p: &mut Proc, events: &mpsc::Sender<Event>) -> bool {
    // a subagent's own stream is its tool call's, never the turn's words
    let top = v["parent_tool_use_id"].is_null();
    match v["type"].as_str().unwrap_or("") {
        "stream_event" if top => {
            if let Some(text) = drafted(t, &v["event"]) {
                let _ = events.send(Event::Draft { turn: t.id.clone(), text }).await;
            }
        }
        "assistant" if top => {
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str().unwrap_or("") {
                    "text" => {
                        let text = b["text"].as_str().unwrap_or("").to_string();
                        t.draft.clear();
                        if !text.trim().is_empty() {
                            let _ = events.send(Event::Reply { turn: t.id.clone(), part: t.part, text }).await;
                            t.part += 1;
                        }
                    }
                    "tool_use" => {
                        let id = b["id"].as_str().unwrap_or("").to_string();
                        if t.calls.contains_key(&id) {
                            continue; // said again: one call
                        }
                        let name = b["name"].as_str().unwrap_or("").to_string();
                        let response = v["message"]["id"].as_str().unwrap_or("").to_string();
                        let position = place(&mut t.said, &response);
                        // its intent (`turn.call`), as Claude Code says it,
                        // before its result: the line comes before the tool
                        // runs, though the tool may start before the bridge
                        // reads it (docs/bridge.md). A question is its card.
                        if name != "AskUserQuestion" && records::valid_post_id(&response) && records::valid_post_id(&id) {
                            let (args, category) = shown(&name, &b["input"]);
                            let call = Call { response: response.clone(), position, tool_call_id: id.clone(), tool: name.clone(), args, category };
                            let _ = events.send(Event::Called { turn: t.id.clone(), call }).await;
                        }
                        t.calls.insert(id, (name, b["input"].clone(), Instant::now(), response));
                    }
                    _ => {}
                }
            }
        }
        "user" if top => {
            for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                let id = b["tool_use_id"].as_str().unwrap_or("").to_string();
                let Some((name, input, at, response)) = t.calls.remove(&id) else { continue };
                if name == "AskUserQuestion" {
                    continue;
                }
                let failed = b["is_error"] == true;
                let status = if failed && t.stopping { ToolStatus::Cancelled } else if failed { ToolStatus::Failed } else { ToolStatus::Completed };
                let (args, category) = shown(&name, &input);
                let step = Step { tool: name, args, ok: !failed, excerpt: String::new(), category, evidence: Some(ToolEvidence { tool_call_id: id, status, duration_ms: u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX), response: records::valid_post_id(&response).then_some(response) }) };
                let _ = events.send(Event::Step { turn: t.id.clone(), step }).await;
            }
        }
        "control_request" => {
            let r = &v["request"];
            let request = v["request_id"].as_str().unwrap_or("").to_string();
            if r["subtype"] == "can_use_tool" && r["tool_name"] == "AskUserQuestion" {
                ask(t, &request, r, events).await;
            } else if r["subtype"] == "can_use_tool" {
                // nothing else asks a person here (no technical approvals)
                let _ = write(&mut p.stdin, &json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request, "response": { "behavior": "deny", "message": "Not available here.", "toolUseID": r["tool_use_id"] } } })).await;
            } else {
                let _ = write(&mut p.stdin, &json!({ "type": "control_response", "response": { "subtype": "error", "request_id": request, "error": "not supported" } })).await;
            }
        }
        "result" => {
            let outcome = if t.stopping {
                Outcome::Stopped
            } else if v["subtype"] == "success" && v["is_error"] != true {
                Outcome::Idle
            } else {
                Outcome::Error(error_of(v))
            };
            let _ = events.send(Event::End { turn: t.id.clone(), outcome }).await;
            return true;
        }
        _ => {}
    }
    false
}

/// A streamed event of the model's answer: a text block's start clears the
/// draft, and each piece of its text grows it (the draft so far, to show).
fn drafted(t: &mut Turn, e: &Value) -> Option<String> {
    if e["type"] == "content_block_start" && e["content_block"]["type"] == "text" {
        t.draft.clear();
    }
    if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta" {
        t.draft.push_str(e["delta"]["text"].as_str().unwrap_or(""));
        return Some(t.draft.clone());
    }
    None
}

/// What a failed turn's `result` says, in a person's words where it has
/// some (a plan's limit, a sign-in needed), else its kind.
fn error_of(v: &Value) -> String {
    let said = v["result"].as_str().map(str::trim).filter(|s| !s.is_empty());
    match said {
        Some(s) => records::cut(s, 300),
        None => format!("Claude Code ended the turn: {}", v["subtype"].as_str().unwrap_or("an error")),
    }
}

/// A call's place in its response: one after the last call said, when that
/// was the same response's, else the first.
fn place(said: &mut Option<(String, u32)>, response: &str) -> u32 {
    match said {
        Some((r, next)) if r == response => std::mem::replace(next, *next + 1),
        said => {
            *said = Some((response.to_string(), 1));
            0
        }
    }
}

/// A tool call as a step shows it: what it was called with, and what kind
/// of step it is.
fn shown(name: &str, input: &Value) -> (String, Category) {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    match name {
        "Bash" => (s("command"), Category::Shell),
        "Read" | "NotebookRead" => (s("file_path"), Category::Read),
        "Glob" | "Grep" => (s("pattern"), Category::Read),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => (s("file_path"), Category::Write),
        "WebSearch" => (s("query"), Category::Web),
        "WebFetch" => (s("url"), Category::Web),
        "Task" | "Agent" => (s("description"), Category::Delegate),
        _ => (records::cut(&input.to_string(), 140), Category::Other),
    }
}

/// `AskUserQuestion` asks: each of its questions a card (by its call's id
/// and its place, so a deferred call asked again is the same card), its
/// options, and "Other" in words.
async fn ask(t: &mut Turn, request: &str, r: &Value, events: &mpsc::Sender<Event>) {
    let tool_use_id = r["tool_use_id"].as_str().unwrap_or("").to_string();
    let input = r["input"].clone();
    let mut cards = Vec::new();
    for (i, q) in input["questions"].as_array().into_iter().flatten().enumerate() {
        let card = format!("{}-q{i}", records::cut(&tool_use_id, 56));
        let text = q["question"].as_str().unwrap_or("").to_string();
        let labels: Vec<String> = q["options"].as_array().into_iter().flatten().filter_map(|o| o["label"].as_str().map(str::to_string)).collect();
        let mut options: Vec<PromptOption> = labels.iter().enumerate().map(|(j, l)| PromptOption { id: format!("o{j}"), label: l.clone(), style: None, words: false }).collect();
        options.push(PromptOption { id: "other".into(), label: "Other".into(), style: None, words: true });
        let _ = events.send(Event::Prompt { turn: t.id.clone(), prompt: card.clone(), text: text.clone(), options, ttl_ms: None }).await;
        cards.push((card, text, labels, None));
    }
    t.asked.push(Asked { request: request.to_string(), tool_use_id, input, cards });
}

/// A card's answer (none: it expired): once every question of its call is
/// answered, the call goes on with them; one unanswered refuses it, saying
/// no one answered.
async fn answer(t: &mut Turn, p: &mut Proc, prompt: &str, option: Option<String>, words: Option<String>) {
    let Some(i) = t.asked.iter().position(|a| a.cards.iter().any(|c| c.0 == prompt)) else { return };
    let a = &mut t.asked[i];
    let card = a.cards.iter_mut().find(|c| c.0 == prompt).expect("found");
    let said = match option.as_deref() {
        None => None,
        Some("other") => Some(words.unwrap_or_default()),
        Some(o) => o.strip_prefix('o').and_then(|n| n.parse::<usize>().ok()).and_then(|n| card.2.get(n).cloned()).or(words),
    };
    let Some(said) = said else {
        let a = t.asked.remove(i);
        let _ = write(&mut p.stdin, &json!({ "type": "control_response", "response": { "subtype": "success", "request_id": a.request, "response": { "behavior": "deny", "message": "No one answered in time.", "toolUseID": a.tool_use_id } } })).await;
        return;
    };
    card.3 = Some(said);
    if a.cards.iter().all(|c| c.3.is_some()) {
        let a = t.asked.remove(i);
        let answers: serde_json::Map<String, Value> = a.cards.iter().map(|c| (c.1.clone(), Value::String(c.3.clone().expect("all answered")))).collect();
        let mut input = a.input.clone();
        input["answers"] = Value::Object(answers);
        let _ = write(&mut p.stdin, &json!({ "type": "control_response", "response": { "subtype": "success", "request_id": a.request, "response": { "behavior": "allow", "updatedInput": input, "toolUseID": a.tool_use_id } } })).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a session's id is a UUID of its agent, chat and `/new` count:
    /// the same in every life, another for another chat or count.
    #[test]
    fn a_sessions_id_is_its_chats() {
        let a = session_id("juniper--k3x9", "talk--k3x9", 0);
        assert_eq!(a, session_id("juniper--k3x9", "talk--k3x9", 0));
        assert_ne!(a, session_id("juniper--k3x9", "talk--k3x9", 1));
        assert_ne!(a, session_id("juniper--k3x9", "other--k3x9", 0));
        assert_ne!(a, session_id("rowan--k3x9", "talk--k3x9", 0));
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4'), "version 4's layout: {a}");
    }

    /// Goal: a transcript is where Claude Code keeps it: its project named
    /// for the working directory, letters and digits kept.
    #[test]
    fn a_transcript_is_where_claude_code_keeps_it() {
        let p = transcript(Path::new("/data/claude"), Path::new("/data/work/juniper--k3x9"), "abc");
        assert_eq!(p, PathBuf::from("/data/claude/projects/-data-work-juniper--k3x9/abc.jsonl"));
    }

    /// Goal: nothing that would change whom `claude` signs in as, where it
    /// calls, or what it does reaches it; the bridge's own settings neither.
    #[test]
    fn what_claude_never_gets() {
        for n in ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_SIMPLE", "CLAUDECODE", "BRIDGE_RELAY_SECRET_FILE", "CLAUDE_CONFIG_DIR", "MCP_TOOL_TIMEOUT"] {
            assert!(withheld(n), "{n}");
        }
        for n in ["PATH", "HOME", "GOOGLE_OAUTH_ACCESS_TOKEN", "FRAGMENT_API"] {
            assert!(!withheld(n), "{n}");
        }
    }

    /// Goal: the model's words stream as the draft: each text block starts
    /// one, each piece of its text grows it, and nothing else touches it.
    #[test]
    fn its_words_stream_as_the_draft() {
        let agent = Agent { fragment: "juniper--k3x9".into(), identity: "npub1j".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: Some("claude".into()) };
        let mut t = Turn { id: "t".into(), agent, message: None, retried: false, part: 1, draft: "stale".into(), calls: HashMap::new(), said: None, asked: Vec::new(), stopping: false };
        assert_eq!(drafted(&mut t, &json!({ "type": "content_block_start", "content_block": { "type": "text" } })), None);
        assert_eq!(drafted(&mut t, &json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "hel" } })).as_deref(), Some("hel"));
        assert_eq!(drafted(&mut t, &json!({ "type": "content_block_delta", "delta": { "type": "thinking_delta", "thinking": "hm" } })), None);
        assert_eq!(drafted(&mut t, &json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "lo" } })).as_deref(), Some("hello"));
        assert_eq!(drafted(&mut t, &json!({ "type": "message_stop" })), None);
    }

    /// Goal: a tool call shows what it was called with, and its kind.
    #[test]
    fn a_tool_call_as_a_step() {
        assert_eq!(shown("Bash", &json!({ "command": "ls -la" })), ("ls -la".into(), Category::Shell));
        assert_eq!(shown("Edit", &json!({ "file_path": "/a/b.rs" })), ("/a/b.rs".into(), Category::Write));
        assert_eq!(shown("WebFetch", &json!({ "url": "https://x.y" })).1, Category::Web);
        assert_eq!(shown("Other", &json!({ "k": 1 })).1, Category::Other);
    }

    /// Goal (phase 4): each call Claude Code says is placed in its response
    /// (its assistant message), whose blocks come a line each: the next of
    /// the same message one further, another message's first at 0.
    #[test]
    fn a_calls_place_in_its_response() {
        let mut said = None;
        let places: Vec<u32> = ["msg_a", "msg_a", "msg_a", "msg_b", "msg_c", "msg_c"].iter().map(|r| place(&mut said, r)).collect();
        assert_eq!(places, [0, 1, 2, 0, 0, 1]);
    }

    /// Goal: a turn's message carries its note, its quote, who said it and
    /// where each file sent with it is.
    #[test]
    fn a_turns_message() {
        let agent = Agent { fragment: "juniper--k3x9".into(), identity: "npub1j".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: Some("claude".into()) };
        let ts = TurnStart {
            turn: "t".into(),
            agent,
            fragment: "talk--k3x9".into(),
            chat_name: "talk".into(),
            seq: 1,
            asker: "npub1paul".into(),
            asker_name: "paul".into(),
            text: "hi".into(),
            attachments: vec![],
            files: vec![super::super::LocalFile { path: "/tmp/m/a.png".into(), media_type: "image/png".into(), name: "a.png".into(), size: 1 }],
            routine: false,
            claim_seq: None,
            note: Some("the turn before was cut".into()),
            forgotten: vec![],
            forgotten_more: 0,
            command: false,
            reply_to: None,
            quote: Some(super::super::Quote { id: "q".into(), text: "earlier".into(), own: true }),
            simplex: false,
            continues: None,
            age: None,
        };
        assert_eq!(message(&ts), "[Recent channel messages]\nthe turn before was cut\n\n[New message]\n[Replying to your previous message: \"earlier\"]\n[paul] hi\n[paul sent a file: 'a.png' (image/png). It is on your computer at: /tmp/m/a.png]");
    }
}
