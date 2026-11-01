//! The bridge's I/O around its pure engine (engine.rs):
//!
//! - the restore gate (`RESTORE_PENDING=1`: wait for
//!   `/run/computer/restored` before reading `/data`);
//! - the life: 128 random bits of this process's, never written to `/data`;
//! - the state file (`<state>/state.json`), written whole and renamed into
//!   place after every step that changed it, before the step's effects;
//! - followers: for each agent and each fragment it follows (its chats'
//!   `chat`, its own `tasks`), a wake subscription, a catch-up from the
//!   cursor over `GET …/channels/{c}?after=`, then `__live`, reconnecting
//!   with jitter (lesson 5);
//! - lanes: each fragment's posts, claims and drafts in order, retried by
//!   id; a claim is sent only while the computer is not held
//!   (`/run/computer/hold`, a sleep's mark before its save), and its answer
//!   goes back to the engine;
//! - the runtime's commands (a turn's attachments downloaded first, and its
//!   note read from its chat's journal: what a restart cut of the agent's
//!   turn before it, note.rs) and events;
//! - the keepalive socket, held while the engine says so (decision 39);
//! - the screen (screen.rs), told which agents it runs;
//! - SimpleX, when the image names its core (simplex/run.rs): told each
//!   read of the computer, and waited for by the answer to a hold.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::api::{self, Api, ApiError, HomeChat};
use crate::engine::{self, ChatView, ClaimAnswer, Effect, Engine, Input, Settings, State};
use crate::history::{Budget, Unavailable};
use crate::limits;
use crate::net::Backoff;
use crate::note;
use crate::ready::Ready;
use crate::records::{self, AttachmentRef, Cause, Record};
use crate::runtime::{Agent, Command, Event, LocalFile, Quote, RuntimeIo, Runtimes, TurnStart};
use crate::screen::{self, Named, ScreenConfig};

/// What the bridge is given.
#[derive(Debug, Clone)]
pub struct Config {
    /// `FRAGMENT_API`.
    pub api: String,
    /// Where its state lives (`/data/bridge`).
    pub state_dir: PathBuf,
    /// Scratch for attachments, never kept across a sleep.
    pub media_dir: PathBuf,
    /// `RESTORE_PENDING=1`: wait for `restored` before reading `/data`.
    pub restore_pending: bool,
    pub restored: PathBuf,
    /// `BRIDGE_HOLD`: while this file exists the platform holds the
    /// computer (a save, a sleep's included), and the bridge claims nothing.
    pub hold: PathBuf,
    /// `BRIDGE_HELD`: the bridge's answer to a hold, which it writes once the
    /// hold exists and no claim of its is in flight, and removes otherwise
    /// (docs/computers.md, "The hold"). An image with more to quiet than the
    /// bridge names another file, and answers the platform itself.
    pub held: PathBuf,
    /// `BRIDGE_HELD_LEAVE_OUT`: what the save may leave out, written into
    /// the answer one per line (gitignore patterns relative to `/data`):
    /// files the image keeps another way. None by default.
    pub left_out: Vec<String>,
    pub settings: Settings,
    /// `BRIDGE_AGENTS_FILE`: the agents the image has made ready (ready.rs);
    /// none, every agent the platform lists.
    pub agents_file: Option<PathBuf>,
    /// `BRIDGE_SCREEN_LISTEN` and the rest: the screen of each agent it runs
    /// (screen.rs); none, no screen.
    pub screen: Option<ScreenConfig>,
    /// `BRIDGE_KEEPALIVE_PING_MS`: how often the keepalive pings; silent
    /// twice that, it is opened again (`limits::KEEPALIVE_PING_MS`).
    pub keepalive_ping: Duration,
    /// `BRIDGE_SIMPLEX_BIN` and the rest: each linked agent's SimpleX core
    /// (simplex/run.rs); none, no SimpleX, and nothing of it runs.
    pub simplex: Option<crate::simplex::run::Config>,
}

/// Why the bridge stopped.
#[derive(Debug)]
pub enum BridgeError {
    /// Its state file contradicts itself: refused, never repaired in place.
    Corrupt(String),
    /// Its state could not be read or written.
    Disk(String),
    /// Its runtime failed for good, or it was misconfigured.
    Runtime(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Corrupt(m) => write!(f, "corrupt state: {m}"),
            BridgeError::Disk(m) => write!(f, "state: {m}"),
            BridgeError::Runtime(m) => write!(f, "{m}"),
        }
    }
}

// ---- state on disk ----

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

/// The state as last written, or a fresh one. A state of another format (an
/// older bridge's) is set aside for a fresh one: the state is a cache of
/// the journal (one life per turn), so starting from none reads the chats
/// again and runs nothing twice. A state that contradicts itself is refused.
pub fn load(dir: &Path) -> Result<State, BridgeError> {
    let path = state_path(dir);
    let corrupt = |why: String| BridgeError::Corrupt(format!("{}: {why}", path.display()));
    match std::fs::read(&path) {
        Ok(bytes) => {
            if bytes.len() > limits::STATE_FILE_MAX_BYTES {
                return Err(corrupt(format!("{} bytes, past the bound", bytes.len())));
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| corrupt(e.to_string()))?;
            let Some(version) = value.get("version").and_then(Value::as_u64) else { return Err(corrupt("no version".into())) };
            if !engine::STATE_VERSIONS.iter().any(|v| u64::from(*v) == version) {
                crate::ev!("state.set_aside", { "path": path.display().to_string(), "version": version, "why": "another bridge's format: the journal says which turns ran" });
                return Ok(State::default());
            }
            let state: State = serde_json::from_value(value).map_err(|e| corrupt(e.to_string()))?;
            engine::check(&state).map_err(|c| BridgeError::Corrupt(c.0))?;
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(BridgeError::Disk(format!("{}: {e}", path.display()))),
    }
}

/// This process's life: 128 bits from the kernel's random source, as 32
/// lowercase hex. Never written to `/data`, so no save carries it into
/// another life (docs/chat-records.md, `turn.start`).
pub fn new_life() -> Result<String, BridgeError> {
    use std::io::Read;
    let mut bits = [0u8; 16];
    let mut random = std::fs::File::open("/dev/urandom").map_err(|e| BridgeError::Runtime(format!("/dev/urandom: {e}")))?;
    random.read_exact(&mut bits).map_err(|e| BridgeError::Runtime(format!("/dev/urandom: {e}")))?;
    let life = records::hex(&bits);
    assert!(records::valid_life(&life), "a life is 32 lowercase hex");
    Ok(life)
}

/// Writes the state whole: a temporary file, synced, renamed over the old,
/// and the directory synced, so a crash leaves the old state or the new.
pub fn save(dir: &Path, state: &State) -> Result<(), BridgeError> {
    let bytes = serde_json::to_vec(state).map_err(|e| BridgeError::Disk(e.to_string()))?;
    assert!(bytes.len() <= limits::STATE_FILE_MAX_BYTES, "the engine bounds its state: {} bytes", bytes.len());
    let path = state_path(dir);
    let tmp = dir.join("state.json.tmp");
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(dir)?;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    };
    write().map_err(|e| BridgeError::Disk(format!("{}: {e}", path.display())))?;
    Ok(())
}

/// Whether `pattern` is one a hold's answer may name (docs/computers.md,
/// "The hold"): a gitignore pattern of letters, digits and `._-/*?[]`, at
/// most `HELD_PATTERN_MAX_BYTES`. The platform refuses an answer with any
/// other, and saves the guest whole.
pub fn left_out_ok(pattern: &str) -> bool {
    (1..=limits::HELD_PATTERN_MAX_BYTES).contains(&pattern.len()) && pattern.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/*?[]".contains(&b))
}

/// Writes `bytes` to `path` whole: a temporary file beside it, renamed over
/// it, so a reader never sees half.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Waits for the platform's restore of `/data` (docs/computers.md, the
/// restore gate). False when told to stop first.
pub async fn restore_gate(cfg: &Config, mut stop: watch::Receiver<bool>) -> bool {
    if !cfg.restore_pending {
        return true;
    }
    crate::ev!("gate.waiting", { "marker": cfg.restored.display().to_string() });
    let started = Instant::now();
    // bounded by the platform: it touches the marker once /data is back, or
    // stops the container
    loop {
        if cfg.restored.exists() {
            crate::ev!("gate.open", { "waitedMs": started.elapsed().as_millis() as u64 });
            return true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            _ = crate::net::stopped(&mut stop) => return false,
        }
    }
}

// ---- the engine's inbox ----

enum Msg {
    Input(Input),
    /// `GET /api/computer` as read (in a task of its own).
    Computer(api::Computer),
}

/// What followers read of the engine without asking it: the cursors, as of
/// the last step that was persisted; and when an agent of this computer
/// last joined each fragment, which makes every view of it read before
/// stale (a follower of another agent there would miss the new one, and
/// its lead answer an `@mention` of it).
#[derive(Default)]
struct Shared {
    cursors: Mutex<BTreeMap<String, u64>>,
    /// The prompts this life's running turns hold open (`Engine::held`).
    held: Mutex<BTreeSet<(String, String, String)>>,
    joins: Mutex<HashMap<String, Instant>>,
    /// Each linked agent's SimpleX chat, as the engine is told it
    /// (`Input::Relaying`): the one a routine at home runs in.
    mirrors: Mutex<BTreeMap<String, Option<String>>>,
}

impl Shared {
    fn cursor(&self, agent: &str, fragment: &str, channel: &str) -> u64 {
        self.cursors.lock().expect("cursors").get(&engine::cursor_key(agent, fragment, channel)).copied().unwrap_or(0)
    }

    fn holds(&self, agent: &str, fragment: &str, prompt: &str) -> bool {
        self.held.lock().expect("held").contains(&(agent.to_string(), fragment.to_string(), prompt.to_string()))
    }

    /// An agent of this computer joined `fragment` now.
    fn joined(&self, fragment: &str) {
        let mut joins = self.joins.lock().expect("joins");
        // a join older than a view's life makes no view stale that is not already
        joins.retain(|_, at| at.elapsed() < Duration::from_millis(limits::VIEW_TTL_MS));
        if joins.len() >= limits::JOINS_HELD_MAX {
            crate::ev!("joins.bounded", { "max": limits::JOINS_HELD_MAX });
            joins.clear();
        }
        joins.insert(fragment.to_string(), Instant::now());
    }

    fn joined_at(&self, fragment: &str) -> Option<Instant> {
        self.joins.lock().expect("joins").get(fragment).copied()
    }
}

/// The agents the screen names, from those the bridge runs.
fn named(agents: &[Agent]) -> Vec<Named> {
    agents.iter().map(|a| Named { fragment: a.fragment.clone() }).collect()
}

/// Runs the bridge until `stop` turns true.
pub async fn run(cfg: Config, runtimes: Runtimes, stop: watch::Receiver<bool>) -> Result<(), BridgeError> {
    // The screen's page answers from the start (a viewer's port wakes the
    // computer, and waits for it); its sockets wait for the agents, which
    // are told only past the restore gate, so no lease under `/data` is
    // touched before the restore.
    let (screen_agents, screen_rx) = watch::channel::<Option<Vec<Named>>>(None);
    let simplex_page = crate::simplex::http::Simplex { configured: cfg.simplex.is_some(), ..Default::default() };
    if let Some(sc) = cfg.screen.clone() {
        let (stop, simplex_page) = (stop.clone(), simplex_page.clone());
        tokio::spawn(async move {
            if let Err(e) = screen::serve(sc, screen_rx, simplex_page, stop).await {
                crate::ev!("screen.failed", { "error": e });
            }
        });
    }
    if !restore_gate(&cfg, stop.clone()).await {
        return Ok(());
    }
    // its state's directory at once, and so `/data`, which on a first start
    // the image makes (docs/computers.md, "Data and the restore gate"): a
    // computer that has run no turn yet still has a `/data` to save, so its
    // sleep saves rather than failing (and keeping it awake). Past the gate,
    // never before: a restore swaps `/data` into place.
    std::fs::create_dir_all(&cfg.state_dir).map_err(|e| BridgeError::Disk(format!("{}: {e}", cfg.state_dir.display())))?;
    let api = Api::new(&cfg.api).map_err(BridgeError::Runtime)?;
    std::fs::create_dir_all(&cfg.media_dir).map_err(|e| BridgeError::Disk(format!("{}: {e}", cfg.media_dir.display())))?;
    // its answer to the platform's holds, for its whole life (none claims
    // yet, and no SimpleX core runs: `quiet` is false only while one does)
    let claims_in_flight = Arc::new(AtomicUsize::new(0));
    let quiet = Arc::new(std::sync::atomic::AtomicBool::new(true));
    // the hold as the runtimes see it, and what of theirs it waits on
    let (held_tx, held_rx) = watch::channel(false);
    let runtimes_unquiet = Arc::new(AtomicUsize::new(0));
    let answer: String = cfg.left_out.iter().map(|p| format!("{p}\n")).collect();
    // a core a bridge before this one left running is gone before any hold is answered
    if let Some(sc) = &cfg.simplex {
        crate::simplex::run::kill_strays(sc).await;
    }
    tokio::spawn(answer_holds(cfg.hold.clone(), cfg.held.clone(), answer, claims_in_flight.clone(), quiet.clone(), held_tx, runtimes_unquiet.clone(), stop.clone()));
    // SimpleX, when the image names its core: told of the computer at each
    // read; it says which agents' links are bound (`relaying`), whose
    // relayed records alone the engine honors, each with its SimpleX chat
    // once found, which is its owner's phone. The sender lives as long as
    // the bridge, so the engine's watch of it never ends on its own.
    let (relaying_tx, mut relaying_rx) = watch::channel(std::collections::BTreeMap::<String, Option<String>>::new());
    let simplex = cfg.simplex.clone().map(|sc| crate::simplex::run::start(sc, api.clone(), quiet.clone(), cfg.hold.clone(), relaying_tx.clone(), stop.clone()));
    if let Some(h) = &simplex {
        let _ = simplex_page.handle.set(h.clone());
    }
    let state = load(&cfg.state_dir)?;
    let life = new_life()?;
    let mut engine = Engine::new(state, cfg.settings, &life).map_err(|c| BridgeError::Corrupt(c.0))?;

    let (inbox_tx, mut inbox) = mpsc::channel::<Msg>(limits::INBOX_MAX);
    let (keep_tx, keep_rx) = watch::channel(false);
    let shared = Arc::new(Shared::default());

    // Each runtime starts at once (Hermes dials it while the bridge reads
    // the platform), and its events go to the inbox, named by it. The
    // bridge lives as long as every one of them: one that ends ends it.
    let mut to_runtime = BTreeMap::new();
    let mut runtime_tasks = Vec::new();
    let mut runs = Vec::new();
    for (serves, runtime) in runtimes.into_inner() {
        engine = engine.with_runtime(&serves, runtime.menu());
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(256);
        let (event_tx, mut event_rx) = mpsc::channel::<Event>(1024);
        let name = runtime.name();
        runs.push(json!({ "serves": serves, "runtime": name }));
        let task = tokio::spawn(runtime.run(RuntimeIo { commands: cmd_rx, events: event_tx, shutdown: stop.clone(), held: held_rx.clone(), unquiet: runtimes_unquiet.clone() }));
        runtime_tasks.push(Box::pin(async move { (name, task.await) }));
        to_runtime.insert(serves.clone(), cmd_tx);
        let inbox_tx = inbox_tx.clone();
        tokio::spawn(async move {
            // bounded by the runtime: ends when it drops its sender
            while let Some(event) = event_rx.recv().await {
                if inbox_tx.send(Msg::Input(Input::Runtime { runtime: serves.clone(), event })).await.is_err() {
                    return;
                }
            }
        });
    }
    let mut runtime_ended = futures_util::future::select_all(runtime_tasks);

    // The agents, before any record: a turn is handed to an agent it names.
    // With a ready file, only those the image made ready (ready.rs).
    let mut ready = cfg.agents_file.clone().map(Ready::new);
    if let Some(r) = ready.as_mut() {
        r.refresh();
    }
    let gated = |agents: Vec<Agent>, ready: &Option<Ready>| match ready {
        Some(r) => crate::ready::gate(agents, r.agents()),
        None => agents,
    };
    // each agent's runtime as its `agent.json` named it last (`with_runtimes`)
    let runtime_names: RuntimeNames = Arc::new(Mutex::new(HashMap::new()));
    let computer = tokio::select! {
        c = ask_until(&api, cfg.simplex.is_some(), &runtime_names, stop.clone()) => c,
        ((name, r), _, _) = &mut runtime_ended => return runtime_result(r, name),
    };
    let Some(mut computer) = computer else { return Ok(()) };
    let listed = computer.agents.len();
    computer.agents = gated(computer.agents, &ready);
    screen_agents.send_replace(Some(named(&computer.agents)));
    if let Some(h) = &simplex {
        h.computer(computer.clone(), computer.agents.clone());
    }
    crate::ev!("bridge.start", { "computer": computer.computer, "agents": computer.agents.len(), "listed": listed, "gated": ready.is_some(), "runtimes": runs, "boot": engine.state().boot + 1, "life": life });

    let lanes = Lanes::new(api.clone(), stop.clone(), Claims { inbox: inbox_tx.clone(), hold: cfg.hold.clone(), in_flight: claims_in_flight, simplex: simplex.clone() });
    let runtime_lanes: BTreeMap<String, RuntimeLane> = to_runtime.into_iter().map(|(serves, tx)| (serves, RuntimeLane::spawn(api.clone(), tx, cfg.media_dir.clone()))).collect();
    tokio::spawn(keepalive(api.clone(), keep_rx, cfg.keepalive_ping, stop.clone()));
    let follows = Follows { api: api.clone(), inbox: inbox_tx.clone(), shared: shared.clone(), stop: stop.clone(), running: Arc::new(Mutex::new(HashMap::new())), discovering: Mutex::new(HashMap::new()), last: Mutex::new(HashMap::new()) };

    let act = |engine: &Engine, step: engine::Step| -> Result<(), BridgeError> {
        if step.dirty {
            save(&cfg.state_dir, engine.state())?;
            *shared.cursors.lock().expect("cursors") = engine.state().cursors.clone();
            *shared.held.lock().expect("held") = engine.held();
        }
        for e in step.effects {
            match e {
                Effect::Post { agent, fragment, channel, id, body, files } => {
                    // what an agent says in its owner's SimpleX chat goes to
                    // their phone too, once posted, whatever started its turn
                    let mirrored = simplex.is_some() && engine.mirror(&agent) == Some(fragment.as_str());
                    let relay = mirrored.then(|| for_simplex(channel, &body, files.len())).flatten();
                    lanes.push(&fragment, Job::Post { agent, channel, id, body, files, relay })
                }
                Effect::Claim { agent, fragment, turn, id, body } => lanes.push(&fragment, Job::Claim { agent, turn, id, body }),
                Effect::Fence { agent, fragment, turn, prompt, id, body } => lanes.push(&fragment, Job::Fence { agent, turn, prompt, id, body }),
                Effect::Owed { agent, fragment, channel, turn, id, body } => lanes.push(&fragment, Job::Owed { agent, channel, turn, id, body }),
                Effect::Draft { agent, fragment, turn, text } => lanes.push(&fragment, Job::Draft { agent, turn, text }),
                Effect::Runtime { runtime, command } => runtime_lanes.get(&runtime).expect("the engine names only the runtimes it was given").push(command),
                Effect::Keepalive(on) => {
                    crate::ev!("keepalive", { "hold": on });
                    let _ = keep_tx.send(on);
                }
                Effect::Discover { agent, joined } => {
                    if let Some(fragment) = joined {
                        shared.joined(&fragment);
                    }
                    // the agent as the engine runs it: a read of the computer
                    // first would be one more call whose failure loses the join
                    let a = engine.agents().iter().find(|a| a.fragment == agent).expect("the engine asks to list only an agent it runs");
                    follows.discover(a.clone());
                }
            }
        }
        Ok(())
    };

    let s = engine.step(Input::Agents(computer.agents.clone()), crate::log::now_ms());
    act(&engine, s)?;
    // the agents whose SimpleX link is bound, and their chats, as their files
    // say, before any record: a relayed record a cut life posted and never
    // ran is honored, and what is said in their chat goes to the phone
    if let Some(sc) = &cfg.simplex {
        let linked = crate::simplex::run::linked_on_disk(&sc.dir, &computer.agents);
        relaying_tx.send_modify(|now| {
            for (a, chat) in linked {
                let known = now.entry(a).or_insert(None);
                if known.is_none() {
                    *known = chat;
                }
            }
        });
        let linked = relaying_tx.borrow().clone();
        *shared.mirrors.lock().expect("mirrors") = linked.clone();
        let s = engine.step(Input::Relaying(linked), crate::log::now_ms());
        act(&engine, s)?;
    }
    let s = engine.recover(crate::log::now_ms());
    act(&engine, s)?;
    for a in &computer.agents {
        follows.discover(a.clone());
    }

    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut computer_read = Instant::now();
    let mut stop_rx = stop.clone();
    // bounded by the bridge's life: one input per pass, ended by `stop`
    let result = loop {
        tokio::select! {
            biased;
            _ = crate::net::stopped(&mut stop_rx) => break Ok(()),
            m = inbox.recv() => {
                let Some(m) = m else { break Ok(()) };
                let step = match m {
                    Msg::Input(input) => engine.step(input, crate::log::now_ms()),
                    Msg::Computer(c) => {
                        let agents = gated(c.agents.clone(), &ready);
                        if let Some(h) = &simplex {
                            h.computer(c, agents.clone());
                        }
                        let now = Some(named(&agents));
                        screen_agents.send_if_modified(|was| if *was == now { false } else { *was = now; true });
                        let s = engine.step(Input::Agents(agents.clone()), crate::log::now_ms());
                        // An agent that left this computer (or is not ready) is followed no more.
                        follows.keep_only(&agents);
                        follows.rediscover_due(&agents);
                        s
                    }
                };
                if let Err(e) = act(&engine, step) {
                    break Err(e);
                }
            }
            r = relaying_rx.changed() => {
                if r.is_err() {
                    break Ok(());
                }
                let linked = relaying_rx.borrow_and_update().clone();
                *shared.mirrors.lock().expect("mirrors") = linked.clone();
                let s = engine.step(Input::Relaying(linked), crate::log::now_ms());
                if let Err(e) = act(&engine, s) {
                    break Err(e);
                }
            }
            _ = tick.tick() => {
                let s = engine.step(Input::Tick, crate::log::now_ms());
                if let Err(e) = act(&engine, s) {
                    break Err(e);
                }
                // The computer's agents are read again this often: one assigned
                // to it while it is awake is followed from then. A ready file
                // that changed is read at once: an agent the image just made
                // ready is followed within a tick.
                let ready_changed = ready.as_mut().is_some_and(Ready::refresh);
                if ready_changed || computer_read.elapsed() >= Duration::from_millis(limits::COMPUTER_EVERY_MS) {
                    computer_read = Instant::now();
                    reread(&api, cfg.simplex.is_some(), &runtime_names, &inbox_tx);
                }
            }
            ((name, r), _, _) = &mut runtime_ended => break runtime_result(r, name),
        }
    };
    crate::ev!("bridge.stop", { "ok": result.is_ok() });
    result
}

fn runtime_result(r: Result<Result<(), crate::runtime::RuntimeError>, tokio::task::JoinError>, name: &str) -> Result<(), BridgeError> {
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(BridgeError::Runtime(e.to_string())),
        Err(e) => Err(BridgeError::Runtime(format!("the {name} runtime panicked: {e}"))),
    }
}

/// Reads `GET /api/computer` in a task of its own (a slow platform never
/// holds the engine), into the inbox.
fn reread(api: &Api, simplex: bool, names: &RuntimeNames, inbox: &mpsc::Sender<Msg>) {
    let (api, names, inbox) = (api.clone(), names.clone(), inbox.clone());
    tokio::spawn(async move {
        if let Some(c) = ask_once(&api, simplex).await {
            let (c, _) = with_runtimes(&api, c, &names).await;
            let _ = inbox.send(Msg::Computer(c)).await;
        }
    });
}

/// Each agent's runtime as its `agent.json` named it last (`None`: it names
/// none), by agent fragment: at most the agents the computer lists.
type RuntimeNames = Arc<Mutex<HashMap<String, Option<String>>>>;

/// The computer's agents, each with the runtime its `agent.json` names
/// (`runtime::named`; no file names none), read as the computer is: at
/// start, every minute, and as the ready file changes. One whose file does
/// not read keeps what it named last; one never read is left out (counted
/// in the second value), as an agent not ready is, so no turn of it goes to
/// a runtime it does not name. An agent's runtime changes within a minute.
async fn with_runtimes(api: &Api, mut c: api::Computer, names: &RuntimeNames) -> (api::Computer, usize) {
    let reads = c.agents.iter().map(|a| async move { (a.fragment.clone(), api.file(&a.fragment, &a.fragment, "agent.json", api::AGENT_JSON_MAX_BYTES).await) });
    let read = futures_util::future::join_all(reads).await;
    let mut named = names.lock().expect("runtime names");
    for (agent, r) in read {
        let now = match r {
            Ok(b) => crate::runtime::named(Some(&b)),
            Err(e) if e.status() == Some(404) => None,
            Err(e) => {
                crate::ev!("runtime.unread", { "agent": agent, "error": e.to_string() });
                continue;
            }
        };
        if named.get(&agent) != Some(&now) {
            crate::ev!("agent.runtime", { "agent": agent, "runtime": now.as_deref().unwrap_or(crate::runtime::DEFAULT) });
        }
        named.insert(agent, now);
    }
    named.retain(|agent, _| c.agents.iter().any(|a| &a.fragment == agent));
    let listed = c.agents.len();
    c.agents.retain(|a| named.contains_key(&a.fragment));
    for a in &mut c.agents {
        a.runtime = named[&a.fragment].clone();
    }
    let unread = listed - c.agents.len();
    (c, unread)
}

async fn ask_once(api: &Api, simplex: bool) -> Option<api::Computer> {
    match api.computer(simplex).await {
        Ok(c) => Some(c),
        Err(e) => {
            crate::ev!("computer.unread", { "error": e.to_string() });
            None
        }
    }
}

/// `GET /api/computer`, and each agent's runtime, until all answer: at
/// start no agent is left out for a runtime not read (its turns the state
/// holds would be let go as if it had left).
async fn ask_until(api: &Api, simplex: bool, names: &RuntimeNames, stop: watch::Receiver<bool>) -> Option<api::Computer> {
    let mut backoff = Backoff::default();
    // bounded by the platform's answer, or `stop`
    loop {
        if let Some(c) = ask_once(api, simplex).await {
            let (c, unread) = with_runtimes(api, c, names).await;
            if unread == 0 {
                return Some(c);
            }
        }
        if !backoff.wait(stop.clone()).await {
            return None;
        }
    }
}

// ---- followers ----

type FollowKey = (String, String, String);

/// The fragments each agent follows: one task per `(agent, fragment,
/// channel)`, and at most one discovery per agent under way.
struct Follows {
    api: Api,
    inbox: mpsc::Sender<Msg>,
    shared: Arc<Shared>,
    stop: watch::Receiver<bool>,
    running: Arc<Mutex<HashMap<FollowKey, tokio::task::JoinHandle<()>>>>,
    discovering: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    last: Mutex<HashMap<String, Instant>>,
}

impl Follows {
    /// Stops following for agents no longer on this computer, and listing
    /// their fragments.
    fn keep_only(&self, agents: &[Agent]) {
        let mut running = self.running.lock().expect("followers");
        running.retain(|(agent, fragment, channel), h| {
            let keep = agents.iter().any(|a| &a.fragment == agent);
            if !keep {
                crate::ev!("follow.dropped", { "agent": agent, "fragment": fragment, "channel": channel, "why": "the agent left this computer" });
                h.abort();
            }
            keep
        });
        self.discovering.lock().expect("discoveries").retain(|agent, h| {
            let keep = agents.iter().any(|a| &a.fragment == agent);
            if !keep {
                h.abort();
            }
            keep
        });
        self.last.lock().expect("last").retain(|agent, _| agents.iter().any(|a| &a.fragment == agent));
    }

    fn rediscover_due(&self, agents: &[Agent]) {
        let every = Duration::from_millis(limits::DISCOVER_EVERY_MS);
        let due: Vec<Agent> = {
            let last = self.last.lock().expect("last");
            agents.iter().filter(|a| last.get(&a.fragment).is_none_or(|t| t.elapsed() >= every)).cloned().collect()
        };
        for a in due {
            self.discover(a);
        }
    }

    /// Lists the agent's fragments, and follows each chat and its tasks it
    /// does not follow yet (in a task of its own: a slow listing never
    /// holds the engine). What a read failed in a way asking again may pass
    /// (no answer, 429, 5xx) is asked again after a jittered backoff until
    /// it answers: the list, or only the fragments whose channels did not
    /// read, what was found meanwhile followed. Never left to the next
    /// cadence (`DISCOVER_EVERY_MS`): until a start's first listing its
    /// agent follows nothing, its own `tasks` (and so a `joined`) included.
    /// One discovery an agent at a time: a newer one (a join's, the
    /// cadence's) lists after whatever asked for it, and takes the place of
    /// one still under way.
    fn discover(&self, agent: Agent) {
        self.last.lock().expect("last").insert(agent.fragment.clone(), Instant::now());
        let (api, inbox, shared, stop, running) = (self.api.clone(), self.inbox.clone(), self.shared.clone(), self.stop.clone(), self.running.clone());
        let name = agent.fragment.clone();
        let task = tokio::spawn(async move {
            let mut backoff = Backoff::default();
            let mut missed: Option<Vec<String>> = None;
            // bounded by the platform's answers, or `stop`: one listing a pass
            loop {
                match list_follows(&api, &agent, missed.take()).await {
                    Ok(listed) => {
                        crate::ev!("discover", { "agent": agent.fragment, "follows": listed.follows.len(), "missed": listed.missed.len() });
                        let mut running = running.lock().expect("followers");
                        running.retain(|_, h| !h.is_finished());
                        for (fragment, channel) in listed.follows {
                            let key = (agent.fragment.clone(), fragment.clone(), channel.clone());
                            if running.contains_key(&key) {
                                continue;
                            }
                            let h = tokio::spawn(follow(api.clone(), agent.clone(), fragment, channel, inbox.clone(), shared.clone(), stop.clone()));
                            running.insert(key, h);
                        }
                        if listed.missed.is_empty() {
                            return;
                        }
                        missed = Some(listed.missed);
                    }
                    Err(e) => {
                        crate::ev!("discover.failed", { "agent": agent.fragment, "error": e.to_string(), "again": e.retryable() });
                        if !e.retryable() {
                            return;
                        }
                    }
                }
                if !backoff.wait(stop.clone()).await {
                    return;
                }
            }
        });
        if let Some(before) = self.discovering.lock().expect("discoveries").insert(name, task) {
            before.abort();
        }
    }
}

/// What one listing found for an agent to follow, and the fragments whose
/// channels did not read in a way asking again may pass.
struct Listed {
    follows: Vec<(String, String)>,
    missed: Vec<String>,
}

/// What an agent follows: `chat` of every fragment it is in that has a
/// postable `chat` channel, and `tasks` of its own fragment if it has one;
/// of `only` those fragments, when named (a listing's misses, asked again).
/// A fragment whose channels do not read is passed over (the agent left it,
/// or asking again changes nothing) or missed, and keeps the agent from no
/// other.
async fn list_follows(api: &Api, agent: &Agent, only: Option<Vec<String>>) -> Result<Listed, ApiError> {
    let names = match only {
        Some(names) => names,
        None => {
            let mut names: Vec<String> = api.fragments(&agent.fragment).await?.into_iter().map(|f| f.name).collect();
            names.truncate(limits::DISCOVER_FRAGMENTS_MAX);
            if !names.contains(&agent.fragment) {
                names.push(agent.fragment.clone());
            }
            names
        }
    };
    assert!(names.len() <= limits::DISCOVER_FRAGMENTS_MAX + 1, "a listing reads a bounded set of fragments");
    let mut out = Listed { follows: Vec::new(), missed: Vec::new() };
    for name in names {
        // a miss holds a follow's place: one asked again stays within the bound
        if out.follows.len() + out.missed.len() >= limits::FOLLOWS_PER_AGENT_MAX {
            crate::ev!("discover.bounded", { "agent": agent.fragment, "max": limits::FOLLOWS_PER_AGENT_MAX });
            break;
        }
        let channels = match api.channels(&agent.fragment, &name).await {
            Ok(c) => c,
            Err(e) if e.gone() => continue,
            Err(e) => {
                crate::ev!("discover.unread", { "agent": agent.fragment, "fragment": name, "error": e.to_string(), "again": e.retryable() });
                if e.retryable() {
                    out.missed.push(name);
                }
                continue;
            }
        };
        let own = name == agent.fragment;
        for c in channels {
            let chat = c.name == records::CHAT && c.post.is_some() && !own;
            let tasks = c.name == records::TASKS && own;
            if chat || tasks {
                out.follows.push((name.clone(), c.name.clone()));
            }
        }
    }
    Ok(out)
}

/// A chat's members and names, read again after `VIEW_TTL_MS`.
struct ViewCache {
    view: ChatView,
    since: i64,
    read_at: Option<Instant>,
}

impl ViewCache {
    /// Read within its life, and after the last join of an agent of this
    /// computer to the chat (`Shared::joined`).
    fn fresh(&self, joined: Option<Instant>) -> bool {
        self.read_at.is_some_and(|t| t.elapsed() < Duration::from_millis(limits::VIEW_TTL_MS) && joined.is_none_or(|j| j < t))
    }

    /// Reads the chat's agents (in the order they were added), its writers'
    /// names, and when this agent joined: records from before are history.
    async fn read(&mut self, api: &Api, agent: &Agent, fragment: &str, writer: Option<&str>) -> Result<(), ApiError> {
        let mut members = api.members(&agent.fragment, fragment).await?;
        members.sort_by(|a, b| (a.added_at, &a.principal).cmp(&(b.added_at, &b.principal)));
        self.view.agents = members.iter().filter(|m| m.kind == "agent").map(|m| m.principal.clone()).collect();
        self.view.people = members.iter().filter(|m| m.kind != "agent").map(|m| m.principal.clone()).collect();
        self.since = members.iter().find(|m| m.principal == agent.identity).map(|m| m.added_at).unwrap_or(0);
        // Who to name, at most `PEOPLE_MAX` (one `__people`): the writer whose
        // record asked first, then its people, then its other agents (an
        // agent's message, a hand-off, is told by its name: its title, as
        // `__people` says it now).
        let mut ids: Vec<String> = writer.filter(|w| records::is_identity(w) && *w != agent.identity).map(str::to_string).into_iter().collect();
        // the agent itself too: an answer names the agent that gave it
        // (engine.rs, `owe_answer`) from whichever agent's view it holds
        let (agents, people): (Vec<&api::Member>, Vec<&api::Member>) = members.iter().filter(|m| records::is_identity(&m.principal)).partition(|m| m.kind == "agent");
        for m in people.into_iter().chain(agents) {
            if ids.len() >= api::PEOPLE_MAX {
                break;
            }
            if !ids.contains(&m.principal) {
                ids.push(m.principal.clone());
            }
        }
        match api.people(&agent.fragment, fragment, &ids).await {
            Ok(names) => self.view.names = names,
            Err(e) => crate::ev!("people.unread", { "fragment": fragment, "error": e.to_string() }),
        }
        // A writer asked of and not named is "someone" until the next read,
        // not a reason to read again at each of its records. One past the
        // bound was not asked: left unnamed, its first record reads the view
        // again, with it first.
        for id in ids {
            self.view.names.entry(id).or_insert_with(|| "someone".into());
        }
        self.read_at = Some(Instant::now());
        Ok(())
    }
}

/// Why a follower stopped following.
enum Ended {
    /// The agent is no longer in the fragment.
    Gone,
    /// The bridge is stopping.
    Stopped,
    /// The connection ended: follow again after a wait.
    Again(String),
}

/// Follows one channel of one fragment as one agent until the bridge stops
/// or the agent leaves it.
async fn follow(api: Api, agent: Agent, fragment: String, channel: String, inbox: mpsc::Sender<Msg>, shared: Arc<Shared>, stop: watch::Receiver<bool>) {
    let mut backoff = Backoff::default();
    let mut view = ViewCache { view: ChatView::default(), since: 0, read_at: None };
    let tasks = channel == records::TASKS;
    let mut subscribed = false;
    // bounded by the bridge's life (or the agent's membership): one
    // connection per pass, each pass waiting a jittered backoff
    loop {
        if *stop.borrow() {
            return;
        }
        let ended = follow_once(&api, &agent, &fragment, &channel, &inbox, &shared, &stop, &mut view, &mut subscribed, tasks, &mut backoff).await;
        match ended {
            Ended::Stopped => return,
            Ended::Gone => {
                crate::ev!("follow.gone", { "agent": agent.fragment, "fragment": fragment, "channel": channel });
                let _ = inbox.send(Msg::Input(Input::Gone { agent: agent.fragment.clone(), fragment: fragment.clone() })).await;
                return;
            }
            Ended::Again(why) => {
                crate::ev!("follow.reconnect", { "agent": agent.fragment, "fragment": fragment, "channel": channel, "why": why });
                if !backoff.wait(stop.clone()).await {
                    return;
                }
            }
        }
    }
}

fn ended_by(e: ApiError) -> Ended {
    if e.gone() {
        Ended::Gone
    } else {
        Ended::Again(e.to_string())
    }
}

#[allow(clippy::too_many_arguments)]
async fn follow_once(
    api: &Api,
    agent: &Agent,
    fragment: &str,
    channel: &str,
    inbox: &mpsc::Sender<Msg>,
    shared: &Shared,
    stop: &watch::Receiver<bool>,
    view: &mut ViewCache,
    subscribed: &mut bool,
    tasks: bool,
    backoff: &mut Backoff,
) -> Ended {
    // A record on the channel wakes this computer from now on.
    if !*subscribed {
        match ensure_wake(api, agent, fragment, channel).await {
            Ok(()) => *subscribed = true,
            Err(e) => return ended_by(e),
        }
    }
    if !tasks && !view.fresh(shared.joined_at(fragment)) {
        if let Err(e) = view.read(api, agent, fragment, None).await {
            return ended_by(e);
        }
    }
    // Catch up from the cursor, then follow live from where that left off.
    let mut last = shared.cursor(&agent.fragment, fragment, channel);
    for _ in 0..limits::CATCHUP_PAGES_MAX {
        let page = match api.records(&agent.fragment, fragment, channel, last, limits::CATCHUP_PAGE_RECORDS).await {
            Ok(p) => p,
            Err(e) => return ended_by(e),
        };
        let full = page.records.len() as u32 >= limits::CATCHUP_PAGE_RECORDS;
        for r in page.records {
            last = last.max(r.seq);
            if let Some(ended) = feed(api, agent, fragment, r, inbox, shared, view, tasks).await {
                return ended;
            }
        }
        if !full {
            break;
        }
    }
    let ws = match api.live(&agent.fragment, fragment).await {
        Ok(ws) => ws,
        Err(e) => return Ended::Again(e),
    };
    let (mut sink, mut stream) = ws.split();
    let subscribe = |after: u64| Message::text(json!({ "type": "subscribe", "channel": channel, "after": after }).to_string());
    if sink.send(subscribe(last)).await.is_err() {
        return Ended::Again("the live socket closed".into());
    }
    crate::ev!("follow.live", { "agent": agent.fragment, "fragment": fragment, "channel": channel, "after": last });
    let mut ping = tokio::time::interval(Duration::from_millis(limits::LIVE_PING_MS));
    ping.tick().await;
    let mut heard = Instant::now();
    let mut stop = stop.clone();
    // bounded by the socket: ends when it closes, goes quiet, or `stop`
    loop {
        tokio::select! {
            _ = crate::net::stopped(&mut stop) => {
                let _ = sink.close().await;
                return Ended::Stopped;
            }
            _ = ping.tick() => {
                if heard.elapsed() > Duration::from_millis(2 * limits::LIVE_PING_MS) {
                    return Ended::Again("the live socket went quiet".into());
                }
                if sink.send(Message::text(json!({ "type": "ping" }).to_string())).await.is_err() {
                    return Ended::Again("the live socket closed".into());
                }
            }
            m = stream.next() => {
                heard = Instant::now();
                let text = match m {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(frame))) => {
                        let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(1005);
                        // 4003: access revoked; 4004: the fragment was deleted.
                        if matches!(code, 4003 | 4004) {
                            return Ended::Gone;
                        }
                        return Ended::Again(format!("closed {code}"));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Ended::Again(e.to_string()),
                    None => return Ended::Again("the live socket ended".into()),
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                match v["type"].as_str() {
                    Some("hello") => backoff.reset(),
                    Some("record") if v["channel"] == channel => {
                        let Ok(r) = serde_json::from_value::<Record>(v.clone()) else { continue };
                        if r.seq <= last {
                            continue;
                        }
                        last = r.seq;
                        if let Some(ended) = feed(api, agent, fragment, r, inbox, shared, view, tasks).await {
                            return ended;
                        }
                    }
                    Some("subscribed") if v["more"] == json!(true) => {
                        let next = v["next"].as_u64().unwrap_or(last);
                        if sink.send(subscribe(next.max(last))).await.is_err() {
                            return Ended::Again("the live socket closed".into());
                        }
                    }
                    Some("error") => crate::ev!("follow.refused_frame", { "fragment": fragment, "message": v["message"].as_str().unwrap_or("") }),
                    _ => {}
                }
            }
        }
    }
}

/// One record into the engine, with the chat's view (fresh enough, and
/// knowing its writer). A routine that names no chat is given the chat its
/// agent's home names now (`home_of`; docs/api.md, "An agent's home"); one
/// whose home could not be read, or names a SimpleX chat not made yet,
/// ends the follow, which reads it again after a wait from the engine's
/// cursor, so no fire is lost to a failed read or a chat being made
/// (until it is past the hour a missed routine is skipped after). What
/// ends the follow, if anything: `Stopped` when the engine is gone.
#[allow(clippy::too_many_arguments)]
async fn feed(api: &Api, agent: &Agent, fragment: &str, record: Record, inbox: &mpsc::Sender<Msg>, shared: &Shared, view: &mut ViewCache, tasks: bool) -> Option<Ended> {
    let (input_view, since) = if tasks {
        // A routine that fired while the computer was gone longer than this
        // is skipped, as cron skips a missed run; an answer never is (its
        // claim keeps it to one run however late it is read).
        let answer = matches!(records::task(&record.body), records::Task::Answer { .. });
        let since = if answer { 0 } else { i64::try_from(crate::log::now_ms().saturating_sub(limits::TASKS_BACKLOG_MS)).expect("ms fit") };
        (None, since)
    } else {
        // a writer the view has no name for (a person or another agent past
        // the last read's bound, or new): read again, with it first
        let unknown = records::is_identity(&record.principal) && record.principal != agent.identity && !view.view.names.contains_key(&record.principal);
        if !view.fresh(shared.joined_at(fragment)) || unknown {
            if let Err(e) = view.read(api, agent, fragment, Some(&record.principal)).await {
                crate::ev!("view.unread", { "fragment": fragment, "error": e.to_string() });
            }
        }
        (Some(view.view.clone()), view.since)
    };
    let record = if tasks && record.at >= since && records::homeless(&record.body) {
        let simplex = shared.mirrors.lock().expect("mirrors").get(&agent.fragment).cloned().flatten();
        let now = i64::try_from(crate::log::now_ms()).expect("ms fit");
        let waited = now.saturating_sub(record.at) >= i64::try_from(limits::HOME_CHAT_WAIT_MS).expect("ms fit");
        match home_of(api, agent, simplex.as_deref(), waited).await {
            Ok(HomeChat::In(chat)) => {
                crate::ev!("routine.home", { "agent": agent.fragment, "seq": record.seq, "chat": chat });
                Record { body: records::homed(&record.body, &chat), ..record }
            }
            // no chat with its owner at all: passed over, said so
            Ok(HomeChat::Nowhere) => {
                crate::ev!("routine.homeless", { "agent": agent.fragment, "seq": record.seq });
                record
            }
            // its SimpleX chat named and not made yet: read again from the cursor, as a failed read is
            Ok(HomeChat::Unmade) => return Some(Ended::Again("its SimpleX chat is not made yet".into())),
            Err(e) => return Some(Ended::Again(format!("its home was not read: {e}"))),
        }
    } else {
        record
    };
    // an owner's answer to a card of the agent's that no running turn of
    // this life holds is a late one, read against the journal first
    // (cards.rs): a read that failed (a deploy's 503, a timeout) ends the
    // follow, which reads it again from the cursor after a wait, so a card
    // is never closed for a blip. (A turn that ends between this look and
    // the engine's is a race of a few ms; its answer is passed over.)
    let late = !tasks && record.at >= since && record.seq > shared.cursor(&agent.fragment, fragment, &record.channel) && record.principal == agent.owner && matches!(records::said(&record.body), records::Said::PromptResponse { prompt, card, .. } if card.as_ref().is_none_or(|c| c.agent == agent.identity) && !shared.holds(&agent.fragment, fragment, &prompt));
    let input = if late {
        let read = tokio::time::timeout(Duration::from_millis(limits::CARD_READ_MS_MAX), card_history(api, agent, fragment, &record)).await;
        let history = read.unwrap_or(Err(Unavailable::Read));
        if let Err(Unavailable::Read) = history {
            return Some(Ended::Again(format!("a late card answer's journal did not read (seq {})", record.seq)));
        }
        Input::CardRecord { agent: agent.fragment.clone(), fragment: fragment.to_string(), record, view: input_view, since, history }
    } else {
        Input::Record { agent: agent.fragment.clone(), fragment: fragment.to_string(), record, view: input_view, since }
    };
    let fed = inbox.send(Msg::Input(input)).await;
    fed.is_err().then_some(Ended::Stopped)
}

/// The chat an agent's home names now (`api::home_chat`): its `agent.json`,
/// its SimpleX chat while linked (the one its link mirrors: `simplex`),
/// its owner's list and its own, read as a routine that names no chat
/// fires (at most every 15 minutes a schedule); `waited`: the routine fired
/// `limits::HOME_CHAT_WAIT_MS` ago or more.
async fn home_of(api: &Api, agent: &Agent, simplex: Option<&str>, waited: bool) -> Result<HomeChat, ApiError> {
    let home = api.home(&agent.fragment).await?;
    let owners = api.fragments_for(&agent.fragment, &agent.owner).await?;
    let mine = api.fragments(&agent.fragment).await?;
    Ok(api::home_chat(&agent.fragment, home, simplex, &owners, &mine, waited))
}

/// The agent's wake subscription on the channel: made unless it has one.
async fn ensure_wake(api: &Api, agent: &Agent, fragment: &str, channel: &str) -> Result<(), ApiError> {
    let subs = api.subscriptions(&agent.fragment, fragment).await?;
    let have = subs.iter().any(|s| s.channel == channel && s.wake && s.principal.as_deref().is_none_or(|p| p == agent.identity));
    if have {
        return Ok(());
    }
    let s = api.subscribe_wake(&agent.fragment, fragment, channel).await?;
    crate::ev!("subscribed", { "agent": agent.fragment, "fragment": fragment, "channel": channel, "id": s.id });
    Ok(())
}

// ---- lanes: each fragment's posts, claims and drafts, in order ----

/// What of a record an agent posts in its owner's SimpleX chat goes to
/// their SimpleX app (the caller's to say it is that chat), its files beside
/// it (simplex/run.rs, `Handle::reply`): a message of its own (a reply;
/// something it says with no turn running), its text and its files, unless
/// it is interim (the model's words before a tool call stay in the chat, as
/// its push leaves them out: docs/chat-records.md, "Push"; a turn may
/// narrate 200 of them, each a notification on the phone); and a card's
/// question, its choices named, since a card shows only in the chat.
/// Nothing else (a relayed message, the owner's own, is the phone's
/// already).
fn for_simplex(channel: &str, body: &Value, files: usize) -> Option<String> {
    let text = body["text"].as_str().unwrap_or("");
    match channel {
        records::CHAT if body["interim"] != true && body.get("relayed").is_none() => (!text.trim().is_empty() || files > 0).then(|| text.to_string()),
        records::WORK if body["kind"] == "turn.prompt" => {
            let options = body["options"].as_array()?;
            let words = options.iter().any(|o| o["words"] == true);
            let labels: Vec<&str> = options.iter().filter(|o| o["words"] != true).filter_map(|o| o["label"].as_str()).collect();
            // the owner's next message answers a card that takes words (engine.rs `told`)
            let how = match (words, labels.is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!("\n\n(Answer with one of: {}, or in your own words.)", labels.join(", ")),
                (false, _) => "\n\n(This one needs a button: answer it in our chat.)".to_string(),
            };
            Some(format!("{text}{how}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum Job {
    /// `relay`: what of it goes to the owner's SimpleX app once posted, for
    /// a post in its owner's SimpleX chat (`for_simplex`), with its files.
    Post { agent: String, channel: &'static str, id: String, body: Value, files: Vec<LocalFile>, relay: Option<String> },
    /// A turn's `turn.start` on `work`, whose answer the engine waits for.
    Claim { agent: String, turn: String, id: String, body: Value },
    /// A live card answer's closure on `work`, whose answer the engine
    /// waits for before the runtime has the answer.
    Fence { agent: String, turn: String, prompt: String, id: String, body: Value },
    /// One of a turn's last records (on `work`, or an answer on `tasks`):
    /// the engine hears when the lane is done with it.
    Owed { agent: String, channel: &'static str, turn: String, id: String, body: Value },
    Draft { agent: String, turn: String, text: Option<String> },
}

impl Job {
    /// The turn a job is of (a post's, by its body: a reply's, a step's).
    fn turn(&self) -> Option<&str> {
        match self {
            Job::Post { body, .. } => body["turn"].as_str(),
            Job::Claim { turn, .. } | Job::Fence { turn, .. } | Job::Owed { turn, .. } | Job::Draft { turn, .. } => Some(turn),
        }
    }
}

struct Lane {
    queue: Mutex<VecDeque<Job>>,
    ready: Notify,
}

/// What a lane needs beside its jobs: where claims' answers go, the hold
/// that stops them, and the count of claims in flight (a try between its
/// look at the hold and its answer), which the bridge's answer to a hold
/// waits on.
#[derive(Clone)]
struct Claims {
    inbox: mpsc::Sender<Msg>,
    hold: PathBuf,
    in_flight: Arc<AtomicUsize>,
    /// Where a SimpleX turn's posted reply goes (none without SimpleX).
    simplex: Option<crate::simplex::run::Handle>,
}

/// One claim's try in flight, counted while it lives.
struct Flight<'a>(&'a AtomicUsize);

impl<'a> Flight<'a> {
    fn new(count: &'a AtomicUsize) -> Flight<'a> {
        let before = count.fetch_add(1, Ordering::SeqCst);
        assert!(before < limits::TURNS_OPEN_MAX, "a claim in flight is one of an open turn");
        Flight(count)
    }
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        let before = self.0.fetch_sub(1, Ordering::SeqCst);
        assert!(before > 0, "a claim in flight is counted once");
    }
}

#[derive(Clone)]
struct Lanes {
    api: Api,
    stop: watch::Receiver<bool>,
    claims: Claims,
    lanes: Arc<Mutex<HashMap<String, Arc<Lane>>>>,
}

impl Lanes {
    fn new(api: Api, stop: watch::Receiver<bool>, claims: Claims) -> Lanes {
        Lanes { api, stop, claims, lanes: Arc::new(Mutex::new(HashMap::new())) }
    }

    fn push(&self, fragment: &str, job: Job) {
        let lane = {
            let mut lanes = self.lanes.lock().expect("lanes");
            match lanes.get(fragment) {
                Some(l) => l.clone(),
                None => {
                    let l = Arc::new(Lane { queue: Mutex::new(VecDeque::new()), ready: Notify::new() });
                    lanes.insert(fragment.to_string(), l.clone());
                    tokio::spawn(work(self.api.clone(), fragment.to_string(), l.clone(), self.claims.clone(), self.stop.clone()));
                    l
                }
            }
        };
        {
            let mut q = lane.queue.lock().expect("lane");
            // A lane this long is a platform that stopped answering; the
            // oldest drafts go first (the next carries the whole text).
            if q.len() >= limits::LANE_MAX {
                if let Some(i) = q.iter().position(|j| matches!(j, Job::Draft { .. })) {
                    q.remove(i);
                }
            }
            assert!(q.len() < limits::LANE_MAX, "a fragment's lane holds at most {} posts", limits::LANE_MAX);
            q.push_back(job);
        }
        lane.ready.notify_one();
    }
}

/// A fragment's lane: its jobs in order, each post retried until the
/// platform takes it (its id makes a retry the same record).
async fn work(api: Api, fragment: String, lane: Arc<Lane>, claims: Claims, mut stop: watch::Receiver<bool>) {
    let mut drafted: HashMap<String, Instant> = HashMap::new();
    // bounded by the bridge's life: one job per pass
    loop {
        let job = {
            let mut q = lane.queue.lock().expect("lane");
            let job = q.pop_front();
            // A draft a later draft of the same turn replaces is skipped.
            match job {
                Some(Job::Draft { ref turn, text: Some(_), .. }) if q.iter().any(|j| matches!(j, Job::Draft { turn: t, .. } if t == turn)) => continue,
                j => j,
            }
        };
        let Some(job) = job else {
            tokio::select! {
                _ = lane.ready.notified() => continue,
                _ = crate::net::stopped(&mut stop) => return,
            }
        };
        match job {
            Job::Draft { agent, turn, text } => {
                if text.is_some() {
                    // Within its interval it waits, unless its turn has more
                    // behind it (a newer draft, or its reply or its end):
                    // then it is moot, and waiting would hold that behind it
                    // (a reply waited up to the interval so, and an ask's
                    // answer, in its asker's lane, overtook it).
                    let until = drafted.get(&turn).map(|t| *t + Duration::from_millis(limits::DRAFT_INTERVAL_MS));
                    let mut moot = false;
                    // bounded: each pass ends at the interval's end, or as a
                    // job comes (a lane holds at most LANE_MAX)
                    while let Some(left) = until.map(|u| u.saturating_duration_since(Instant::now())).filter(|l| !l.is_zero()) {
                        if lane.queue.lock().expect("lane").iter().any(|j| j.turn() == Some(turn.as_str())) {
                            moot = true;
                            break;
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(left) => {}
                            _ = lane.ready.notified() => {}
                        }
                    }
                    // the interval out, a newer draft of its turn still goes first
                    if moot || (until.is_some() && lane.queue.lock().expect("lane").iter().any(|j| matches!(j, Job::Draft { turn: t, .. } if *t == turn))) {
                        continue;
                    }
                    drafted.insert(turn.clone(), Instant::now());
                } else {
                    drafted.remove(&turn);
                }
                match api.draft(&agent, &fragment, records::CHAT, &turn, text.as_deref()).await {
                    Ok(()) => {}
                    Err(e) if api::rate_limited(&e) => {}
                    Err(e) => crate::ev!("draft.failed", { "fragment": fragment, "turn": turn, "error": e.to_string() }),
                }
            }
            Job::Post { agent, channel, id, body, files, relay } => {
                // what goes back over SimpleX goes only once it is on the chat
                let sent = post(&api, &fragment, &agent, channel, &id, body, &files, None, limits::POST_TRIES_MAX, stop.clone()).await;
                if let (Some(text), true, Some(sx)) = (relay, sent.on_chat(), claims.simplex.as_ref()) {
                    sx.reply(&agent, &text, &files).await;
                }
            }
            Job::Owed { agent, channel, turn, id, body } => {
                // Done (answered, or given up after its tries) unless the
                // bridge is stopping: then the turn keeps owing it, and the
                // next life posts it again. An answer (on `tasks`) is never
                // given up: it is its question's only delivery, so it is
                // tried until the platform answers it.
                let tries = if channel == records::TASKS { u32::MAX } else { limits::POST_TRIES_MAX };
                let sent = post(&api, &fragment, &agent, channel, &id, body, &[], None, tries, stop.clone()).await;
                if sent.ended() {
                    let _ = claims.inbox.send(Msg::Input(Input::Posted { turn, id, kept: sent == Sent::Posted })).await;
                }
            }
            Job::Claim { agent, turn, id, body } => {
                let answer = claim(&api, &fragment, &agent, &id, &body, Some(&claims), stop.clone()).await;
                // A closed inbox is an engine that has stopped.
                let _ = claims.inbox.send(Msg::Input(Input::Claimed { turn, answer })).await;
            }
            Job::Fence { agent, turn, prompt, id, body } => {
                // an answer's record, not a turn's claim: posted whatever
                // the hold, so a sleep under way loses no answer given
                let answer = claim(&api, &fragment, &agent, &id, &body, None, stop.clone()).await;
                let _ = claims.inbox.send(Msg::Input(Input::Fenced { turn, prompt, answer })).await;
            }
        }
    }
}

/// A turn's claim, posted unless the computer is held (looked at before
/// each try, so a hold made while a try waits stops the next): the
/// platform's answer, or none. Each try is in flight from its look at the
/// hold to its answer, so the bridge never says `held` with one out. With
/// no `claims`, a post whose answer the engine waits for all the same, the
/// hold aside (a live answer's closure: `Job::Fence`).
#[allow(clippy::too_many_arguments)]
async fn claim(api: &Api, fragment: &str, agent: &str, id: &str, body: &Value, claims: Option<&Claims>, stop: watch::Receiver<bool>) -> ClaimAnswer {
    let mut backoff = Backoff::default();
    for attempt in 1..=limits::POST_TRIES_MAX {
        let answered = {
            let _flight = claims.map(|c| Flight::new(&c.in_flight));
            if let Some(claims) = claims.filter(|c| c.hold.exists()) {
                crate::ev!("claim.held", { "fragment": fragment, "id": id, "hold": claims.hold.display().to_string() });
                return ClaimAnswer::Unanswered;
            }
            api.post(agent, fragment, records::WORK, id, body).await
        };
        match answered {
            Ok(posted) => {
                crate::ev!("claimed", { "fragment": fragment, "id": id, "replayed": posted.replayed, "seq": posted.seq, "attempt": attempt });
                return ClaimAnswer::Ours { seq: posted.seq };
            }
            Err(e) if e.status() == Some(409) => {
                crate::ev!("claim.taken", { "fragment": fragment, "id": id, "why": "another life's claim holds the id" });
                return ClaimAnswer::Theirs;
            }
            Err(e) if e.gone() => {
                // Not the agent's to post in (it left, or is held below
                // editor): an answer, and asking again changes nothing.
                crate::ev!("claim.refused", { "fragment": fragment, "id": id, "error": e.to_string() });
                return ClaimAnswer::Refused;
            }
            Err(e) if e.retryable() && attempt < limits::POST_TRIES_MAX => {
                crate::ev!("claim.retry", { "fragment": fragment, "id": id, "attempt": attempt, "error": e.to_string() });
                if !backoff.wait(stop.clone()).await {
                    return ClaimAnswer::Unanswered;
                }
            }
            Err(e) => {
                // Out of tries, or an answer that is none of the above:
                // nothing is known, so it fails closed. The engine claims it
                // again at its next tick.
                crate::ev!("claim.unanswered", { "fragment": fragment, "id": id, "error": e.to_string() });
                return ClaimAnswer::Unanswered;
            }
        }
    }
    unreachable!("the last try returns")
}

/// How a post ended: on the chat (this try's, or an earlier life's under
/// its id), refused for good (out of tries, or no longer the agent's to
/// post in), or not ended, the bridge stopping first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sent {
    /// Appended, or a replay of this body: the platform holds it as ours.
    Posted,
    /// Another life's under the same id (409): on the chat, not this life's.
    Taken,
    Refused,
    Stopped,
}

impl Sent {
    /// Done with, either way: what a turn's owed records wait for.
    pub(crate) fn ended(self) -> bool {
        self != Sent::Stopped
    }

    /// The id is on the chat, this life's body or another's.
    pub(crate) fn on_chat(self) -> bool {
        matches!(self, Sent::Posted | Sent::Taken)
    }
}

/// A post, tried until the platform answers it, or `tries` times on a
/// transport error, 429 or 5xx (a jittered backoff between: lesson 5), or
/// refused at once. A relayed record is signed by its `door` at each try,
/// its files' refs in it (simplex/door.rs).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn post(api: &Api, fragment: &str, agent: &str, channel: &str, id: &str, mut body: Value, files: &[LocalFile], door: Option<&crate::simplex::door::Key>, tries: u32, stop: watch::Receiver<bool>) -> Sent {
    assert!(tries >= 1, "a post is tried");
    let mut backoff = Backoff::default();
    // its files refused here (an agent that is a viewer of a connector's
    // chat may post there, not upload): its words go alone, saying so
    let mut files_refused = false;
    for attempt in 1..=tries {
        let uploaded = match if files_refused { Ok(Vec::new()) } else { upload(api, fragment, agent, files).await } {
            Err(e) if e.status() == Some(403) && !files.is_empty() => {
                crate::ev!("upload.refused", { "fragment": fragment, "channel": channel, "id": id, "files": files.len() });
                files_refused = true;
                let said = body["text"].as_str().unwrap_or("").trim_end().to_string();
                body["text"] = json!(format!("{said}{}({} could not be attached here.)", if said.is_empty() { "" } else { "\n\n" }, if files.len() == 1 { "Its file" } else { "Its files" }));
                Ok(Vec::new())
            }
            other => other,
        };
        let result = match uploaded {
            Ok(refs) => {
                if !refs.is_empty() {
                    body["attachments"] = json!(refs);
                }
                if let Some(door) = door {
                    door.seal(&mut body, fragment, channel, id, crate::log::now_ms() as i64);
                }
                api.post(agent, fragment, channel, id, &body).await
            }
            Err(e) => Err(e),
        };
        match result {
            Ok(posted) => {
                crate::ev!("posted", { "fragment": fragment, "channel": channel, "id": id, "replayed": posted.replayed, "attempt": attempt });
                return Sent::Posted;
            }
            Err(e) if e.retryable() && attempt < tries => {
                crate::ev!("post.retry", { "fragment": fragment, "id": id, "attempt": attempt, "error": e.to_string() });
                if !backoff.wait(stop.clone()).await {
                    return Sent::Stopped;
                }
            }
            Err(e) if e.status() == Some(409) => {
                // Another life posted the id with another body: the end of a
                // turn that life ended, or the records of one it claimed
                // (one life per turn). Never retried.
                crate::ev!("post.taken", { "fragment": fragment, "channel": channel, "id": id });
                return Sent::Taken;
            }
            Err(e) => {
                // Out of tries, or a 403/404: it is no longer the agent's to
                // post in.
                crate::ev!("post.failed", { "fragment": fragment, "channel": channel, "id": id, "error": e.to_string() });
                return Sent::Refused;
            }
        }
    }
    unreachable!("the last try returns")
}

/// Uploads a reply's files as the fragment's blobs: their refs.
async fn upload(api: &Api, fragment: &str, agent: &str, files: &[LocalFile]) -> Result<Vec<AttachmentRef>, ApiError> {
    let mut refs = Vec::new();
    for f in files.iter().take(limits::ATTACHMENTS_MAX) {
        let bytes = match tokio::fs::read(&f.path).await {
            Ok(b) if !b.is_empty() && b.len() as u64 <= limits::ATTACHMENT_MAX_BYTES => Bytes::from(b),
            Ok(_) => {
                crate::ev!("attachment.skipped", { "name": f.name, "why": "empty, or past the bound" });
                continue;
            }
            Err(e) => {
                crate::ev!("attachment.skipped", { "name": f.name, "why": e.to_string() });
                continue;
            }
        };
        let sha = records::hex(&Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        api.put_blob(agent, fragment, &sha, &f.media_type, bytes).await?;
        refs.push(AttachmentRef { sha256: sha, size, media_type: f.media_type.clone(), name: f.name.clone() });
    }
    Ok(refs)
}

// ---- the runtime's commands ----

/// Commands to the runtime in order; a turn's attachments are downloaded,
/// and its note read from its chat's journal, before it is handed (so a Stop
/// never overtakes its Start), as an answer's are before it is.
struct RuntimeLane {
    tx: mpsc::UnboundedSender<Command>,
}

impl RuntimeLane {
    /// One runtime's lane: a turn's commands all go to one runtime, so they
    /// keep the engine's order, and a runtime slow to read its commands
    /// holds no other's.
    fn spawn(api: Api, to_runtime: mpsc::Sender<Command>, media_dir: PathBuf) -> RuntimeLane {
        let (tx, mut rx) = mpsc::unbounded_channel::<Command>();
        tokio::spawn(async move {
            // bounded by the engine: ends when it drops its sender
            while let Some(mut c) = rx.recv().await {
                // an answer's files, downloaded as a turn's are, then named
                // in its words with where each is (`with_files`)
                match &mut c {
                    Command::Tell { text, attached, .. } => {
                        if let Some(a) = attached.take() {
                            let files = download(&api, &a.agent, &a.fragment, &a.refs, &media_dir).await;
                            *text = crate::runtime::with_files(text, &a.by_name, a.refs.len(), &files);
                        }
                    }
                    Command::Answer { words, attached, .. } => {
                        if let Some(a) = attached.take() {
                            let files = download(&api, &a.agent, &a.fragment, &a.refs, &media_dir).await;
                            *words = Some(crate::runtime::with_files(words.as_deref().unwrap_or(""), &a.by_name, a.refs.len(), &files));
                        }
                    }
                    _ => {}
                }
                if let Command::Start(ts) = &mut c {
                    let note = async {
                        let read = tokio::time::timeout(Duration::from_millis(limits::NOTE_READ_MS_MAX), note_for(&api, ts)).await;
                        read.unwrap_or_else(|_| {
                            crate::ev!("note.unread", { "turn": ts.turn, "why": "the journal answered too slowly" });
                            None
                        })
                    };
                    let quote = async {
                        let seq = ts.reply_to?;
                        let read = tokio::time::timeout(Duration::from_millis(limits::NOTE_READ_MS_MAX), quote_of(&api, ts, seq)).await;
                        read.unwrap_or_else(|_| {
                            crate::ev!("quote.unread", { "turn": ts.turn, "seq": seq, "why": "the chat answered too slowly" });
                            None
                        })
                    };
                    let (files, note, quote) = tokio::join!(download(&api, &ts.agent.fragment, &ts.fragment, &ts.attachments, &media_dir), note, quote);
                    ts.files = files;
                    // its age first (a late message's), then a late card
                    // answer's turn's own note (engine.rs `hand`)
                    let notes: Vec<String> = [ts.age.take(), ts.note.take(), note].into_iter().flatten().collect();
                    ts.note = (!notes.is_empty()).then(|| notes.join("\n\n"));
                    // (its own, a card's turn, in the cut note's place: `note_for`)
                    assert!(ts.note.as_ref().is_none_or(|n| n.len() <= limits::AGE_NOTE_MAX_BYTES + limits::HISTORY_NOTE_MAX_BYTES + 2 * limits::NOTE_MAX_BYTES + 6), "a turn's notes are bounded");
                    ts.quote = quote;
                }
                if to_runtime.send(c).await.is_err() {
                    return;
                }
            }
        });
        RuntimeLane { tx }
    }

    fn push(&self, c: Command) {
        // The engine bounds its open turns, and so the commands in flight.
        let _ = self.tx.send(c);
    }
}

/// The message a turn's message quotes (`reply_to`, a seq on the chat's
/// `chat`), read as the agent: its text (a command as typed; a file with no
/// words, its names), cut to `QUOTE_TEXT_MAX_BYTES`, and whether it is the
/// agent's own. `None` when it is not there, is no message, or the chat did
/// not answer: a turn is never held back for its quote.
async fn quote_of(api: &Api, ts: &TurnStart, seq: u64) -> Option<Quote> {
    let page = api.records(&ts.agent.fragment, &ts.fragment, records::CHAT, seq.saturating_sub(1), 1).await.map_err(|e| crate::ev!("quote.unread", { "turn": ts.turn, "seq": seq, "error": e.to_string() })).ok()?;
    let r = page.records.into_iter().find(|r| r.seq == seq)?;
    let text = match records::said(&r.body) {
        records::Said::Message(m) if !m.text.trim().is_empty() => m.text,
        records::Said::Message(m) => m.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "),
        records::Said::Command(c) if c.args.is_empty() => format!("/{}", c.name),
        records::Said::Command(c) => format!("/{} {}", c.name, c.args),
        _ => String::new(),
    };
    if text.trim().is_empty() {
        crate::ev!("quote.none", { "turn": ts.turn, "seq": seq, "why": "it quotes no message" });
        return None;
    }
    let id = records::turn_id(&ts.agent.fragment, &ts.fragment, records::CHAT, seq);
    Some(Quote { id, text: records::cut_bytes(&text, limits::QUOTE_TEXT_MAX_BYTES), own: own_words(&r.principal, &r.body, &ts.agent) })
}

/// Whether a record `principal` posted is `agent`'s own words (a quote of
/// it is `reply_to.is_own`): what it posted, but for a message it relays
/// from its owner (docs/chat-records.md, "A relayed message"), which are
/// its owner's.
fn own_words(principal: &str, body: &Value, agent: &crate::runtime::Agent) -> bool {
    principal == agent.identity && records::relayed(body).is_none_or(|r| r.for_ != agent.owner)
}

/// A message's attachments, from the chat's blobs to scratch files.
async fn download(api: &Api, agent: &str, fragment: &str, refs: &[AttachmentRef], dir: &Path) -> Vec<LocalFile> {
    let mut out = Vec::new();
    for r in refs.iter().take(limits::ATTACHMENTS_MAX) {
        let path = dir.join(&r.sha256);
        let have = tokio::fs::metadata(&path).await.is_ok_and(|m| m.len() == r.size);
        if !have {
            match api.get_blob(agent, fragment, &r.sha256).await {
                Ok(bytes) if records::hex(&Sha256::digest(&bytes)) == r.sha256 => {
                    if let Err(e) = tokio::fs::write(&path, &bytes).await {
                        crate::ev!("attachment.unsaved", { "sha256": r.sha256, "error": e.to_string() });
                        continue;
                    }
                }
                Ok(_) => {
                    crate::ev!("attachment.refused", { "sha256": r.sha256, "why": "its bytes are not its hash" });
                    continue;
                }
                Err(e) => {
                    crate::ev!("attachment.unread", { "sha256": r.sha256, "error": e.to_string() });
                    continue;
                }
            }
        }
        out.push(LocalFile { path, media_type: r.media_type.clone(), name: r.name.clone(), size: r.size });
    }
    out
}

// ---- the journal read back: a turn's note (note.rs), a late card answer (cards.rs) ----

/// Records of `fragment`'s `channel` with a seq below `before`, oldest
/// first: read back a page (`NOTE_PAGE_RECORDS`) at a time, each whole and
/// within `budget` (history.rs), until `enough` says the records read
/// answer, the channel's first record is read, or `NOTE_SCAN_RECORDS_MAX`
/// are. With whether they reach back to its start.
async fn read_back(api: &Api, agent: &str, fragment: &str, channel: &str, before: u64, enough: impl Fn(&[Record]) -> bool, budget: &mut Budget) -> Result<(Vec<Record>, bool), Unavailable> {
    let mut got: Vec<Record> = Vec::new();
    // the newest seq not read yet
    let mut upto = before.saturating_sub(1);
    // bounded by NOTE_SCAN_RECORDS_MAX: each pass reads a page further back
    loop {
        if upto == 0 {
            return Ok((got, true));
        }
        if enough(&got) || got.len() >= limits::NOTE_SCAN_RECORDS_MAX {
            return Ok((got, false));
        }
        let after = upto.saturating_sub(u64::from(limits::NOTE_PAGE_RECORDS));
        let (page, size) = api.history_records(agent, fragment, channel, after, limits::NOTE_PAGE_RECORDS, budget.request()?).await?;
        budget.bytes(size)?;
        budget.page(&page.records, channel, after, upto)?;
        let mut older: Vec<Record> = page.records.into_iter().filter(|r| r.seq <= upto).collect();
        older.append(&mut got);
        got = older;
        upto = after;
    }
}

/// The journal a late card answer is checked against (cards.rs), at most
/// one `Budget`: `work` back from its tail to the card's turn's start;
/// `chat` back from the answer to before the card (and to the turn's
/// cause, when it is this chat's); and that cause, read alone when it is
/// further back or elsewhere (context: none when it does not read). A card
/// further back than a read goes is `Budget`, final; a call that failed
/// is `Read`, read again by the follower.
async fn card_history(api: &Api, agent: &Agent, fragment: &str, record: &Record) -> Result<crate::history::Journal, Unavailable> {
    let records::Said::PromptResponse { prompt, card, .. } = records::said(&record.body) else { unreachable!("feed reads a card's answer alone") };
    let me = agent.identity.as_str();
    let mut budget = Budget::default();
    budget.request()?;
    let tail = api.channels(&agent.fragment, fragment).await?.into_iter().find(|c| c.name == records::WORK).map_or(0, |c| c.seq);
    let card_of = |r: &Record| r.principal == me && r.body["kind"] == "turn.prompt" && r.body["prompt"] == prompt.as_str() && card.as_ref().is_none_or(|c| r.body["turn"] == c.turn.as_str());
    let start_of = |rs: &[Record], turn: &Value| rs.iter().position(|s| s.principal == me && s.body["kind"] == "turn.start" && &s.body["turn"] == turn);
    let started = |rs: &[Record]| rs.iter().filter(|r| card_of(r)).any(|p| start_of(rs, &p.body["turn"]).is_some());
    let (work, from_start) = read_back(api, &agent.fragment, fragment, records::WORK, tail.saturating_add(1), started, &mut budget).await?;
    let Some(asked) = work.iter().find(|r| card_of(r) && start_of(&work, &r.body["turn"]).is_some()) else {
        return Err(if from_start { Unavailable::Missing } else { Unavailable::Budget });
    };
    let start = &work[start_of(&work, &asked.body["turn"]).expect("found")];
    let cause = serde_json::from_value::<Cause>(start.body["cause"].clone()).ok();
    let in_chat = cause.as_ref().filter(|c| c.fragment == fragment && c.channel == records::CHAT).map(|c| c.seq);
    let reached = |r: &Record| r.at < asked.at;
    let (chat, from_start) = read_back(api, &agent.fragment, fragment, records::CHAT, record.seq.saturating_add(1), |rs| rs.first().is_some_and(|r| reached(r) && in_chat.is_none_or(|seq| r.seq <= seq)), &mut budget).await?;
    if !from_start && !chat.first().is_some_and(reached) {
        return Err(Unavailable::Budget);
    }
    let mut said = None;
    if let Some(c) = &cause {
        said = chat.iter().find(|r| in_chat == Some(r.seq)).cloned();
        if said.is_none() {
            let one = async {
                let (page, _) = api.history_records(&agent.fragment, &c.fragment, &c.channel, c.seq.saturating_sub(1), 1, budget.request()?).await?;
                Ok::<_, Unavailable>(page.records.into_iter().find(|r| r.seq == c.seq))
            };
            said = one.await.unwrap_or_else(|e| {
                crate::ev!("card.cause_unread", { "agent": agent.fragment, "fragment": fragment, "seq": record.seq, "why": e.why() });
                None
            });
        }
    }
    Ok(crate::history::Journal { work, chat, cause: said })
}

/// The note a turn carries (note.rs): at the agent's first turn in a chat
/// continued from its owner's Session Archive, what was said there
/// (`history_note`, at most `HISTORY_NOTE_MAX_BYTES`); what a restart cut
/// of the agent's turn before it in the chat (`cut_note`); and what its
/// runtime does not remember of the chat (`forgotten_note`), each from the
/// chat's journal, the last two together at most twice a note's bound. `None` when there is neither, or
/// the journal did not answer: a turn is never held back for its note. A
/// late card answer's turn carries its card's turn as its note already
/// (`TurnStart::note`), in the cut note's place: that turn is the one a
/// restart cut, most often, and is told once.
async fn note_for(api: &Api, ts: &TurnStart) -> Option<String> {
    let cut = async { if ts.note.is_some() { None } else { cut_note(api, ts).await } };
    let (history, cut, forgot) = tokio::join!(history_note(api, ts), cut, forgotten_note(api, ts));
    let note = [history, cut, forgot].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
    assert!(note.len() <= limits::HISTORY_NOTE_MAX_BYTES + 2 * limits::NOTE_MAX_BYTES + 4, "a turn's notes are bounded");
    (!note.is_empty()).then_some(note)
}

/// What a chat continued from its owner's Session Archive was (note.rs,
/// `history`), at the agent's first turn there: no turn of its own started
/// in the chat before this one's claim (its `work` read back to the
/// channel's start), and the chat's first records hold its owner's
/// `history`. `None` otherwise, or when the chat did not answer.
async fn history_note(api: &Api, ts: &TurnStart) -> Option<String> {
    let claim = ts.claim_seq?;
    let (agent, chat, me) = (ts.agent.fragment.as_str(), ts.fragment.as_str(), ts.agent.identity.as_str());
    let unread = |e: Unavailable| crate::ev!("note.unread", { "turn": ts.turn, "history": true, "error": e.why() });
    let started = |rs: &[Record]| rs.iter().any(|r| r.principal == me && r.body["kind"] == "turn.start");
    let (work, from_start) = read_back(api, agent, chat, records::WORK, claim, started, &mut Budget::default()).await.map_err(unread).ok()?;
    if started(&work) || !from_start {
        return None;
    }
    let first = api.records(agent, chat, records::CHAT, 0, limits::HISTORY_SCAN_RECORDS).await.map_err(|e| unread(e.into())).ok()?;
    let text = first.records.iter().find_map(|r| note::history(r, &ts.agent.owner))?;
    crate::ev!("note", { "turn": ts.turn, "history": true, "bytes": text.len() });
    Some(text)
}

/// What the agent no longer remembers of the chat (note.rs, "forgotten"):
/// its turns there that another life ran since this life's `/data` was
/// saved (`TurnStart::forgotten`), read from `work` back from the turn's
/// claim to the oldest of their starts, and from `chat` back from its tail
/// to the oldest of their causes. `None` when there are none, or the
/// journal did not answer.
async fn forgotten_note(api: &Api, ts: &TurnStart) -> Option<String> {
    if ts.forgotten.is_empty() {
        return None;
    }
    let claim = ts.claim_seq?;
    let (agent, chat, me) = (ts.agent.fragment.as_str(), ts.fragment.as_str(), ts.agent.identity.as_str());
    let unread = |e: Unavailable| crate::ev!("note.unread", { "turn": ts.turn, "forgotten": ts.forgotten.len(), "error": e.why() });
    let started = |rs: &[Record]| ts.forgotten.iter().all(|t| rs.iter().any(|r| r.principal == me && r.body["kind"] == "turn.start" && r.body["turn"] == t.as_str()));
    let (work, _) = read_back(api, agent, chat, records::WORK, claim, started, &mut Budget::default()).await.map_err(unread).ok()?;
    // their causes in this chat: the chat read back to the oldest of them
    let causes: Vec<u64> = work
        .iter()
        .filter(|r| r.principal == me && r.body["kind"] == "turn.start" && ts.forgotten.iter().any(|t| r.body["turn"] == t.as_str()))
        .filter(|r| r.body["cause"]["fragment"] == chat && r.body["cause"]["channel"] == records::CHAT)
        .filter_map(|r| r.body["cause"]["seq"].as_u64())
        .collect();
    let oldest = causes.iter().min().copied().unwrap_or(0);
    let said = async {
        let tail = api.channels(agent, chat).await?.into_iter().find(|c| c.name == records::CHAT).map_or(0, |c| c.seq);
        read_back(api, agent, chat, records::CHAT, tail + 1, |rs| rs.first().is_some_and(|r| r.seq <= oldest), &mut Budget::default()).await
    };
    let said: Vec<Record> = said.await.map_err(unread).map(|(rs, _)| rs).unwrap_or_default();
    let items = note::forgotten(&work, &said, me, &ts.forgotten);
    if items.is_empty() {
        crate::ev!("note.unread", { "turn": ts.turn, "why": "no forgotten turn's start among the records read", "forgotten": ts.forgotten.len() });
        return None;
    }
    let text = note::forgotten_text(&items, ts.forgotten_more);
    crate::ev!("note", { "turn": ts.turn, "forgotten": items.len(), "more": ts.forgotten_more, "bytes": text.len() });
    Some(text)
}

/// The note of a cut turn (note.rs), from its chat's journal alone: `work`
/// back from the turn's claim to the agent's turn before it there; when a
/// restart cut that turn, the record it answered and what it had replied
/// (`chat` back from its tail to that turn's cause, or its start). `None`
/// when nothing was cut, or the journal did not answer.
async fn cut_note(api: &Api, ts: &TurnStart) -> Option<String> {
    let claim = ts.claim_seq?;
    let (agent, chat, me) = (ts.agent.fragment.as_str(), ts.fragment.as_str(), ts.agent.identity.as_str());
    let unread = |e: Unavailable| crate::ev!("note.unread", { "turn": ts.turn, "error": e.why() });
    // a cut turn's continuation is told of that turn, whatever ran between
    // (engine.rs, `continue_turn`); any other turn, of the agent's turn
    // before it when that one was cut
    let cut = match ts.continues.as_deref() {
        Some(continues) => {
            let started = |rs: &[Record]| rs.iter().any(|r| r.principal == me && r.body["kind"] == "turn.start" && r.body["turn"] == continues);
            let (work, _) = read_back(api, agent, chat, records::WORK, claim, started, &mut Budget::default()).await.map_err(unread).ok()?;
            let Some(c) = note::turn(&work, me, continues) else {
                crate::ev!("note.unread", { "turn": ts.turn, "why": "the cut turn it continues is further back than a note reads", "read": work.len() });
                return None;
            };
            c
        }
        None => {
            let (work, from_start) = read_back(api, agent, chat, records::WORK, claim, |rs| note::previous(rs, me, false) != note::Before::Unread, &mut Budget::default()).await.map_err(unread).ok()?;
            match note::previous(&work, me, from_start) {
                note::Before::Cut(c) => c,
                note::Before::Kept => return None,
                note::Before::Unread => {
                    crate::ev!("note.unread", { "turn": ts.turn, "why": "the agent's turn before is further back than a note reads", "read": work.len() });
                    return None;
                }
            }
        }
    };
    // the cut turn's replies, from the chat's tail back to its cause (or,
    // for a routine, to its start: a reply comes after its turn's claim);
    // what it asked and said are the note's detail, so one that does not
    // read leaves the note without it
    let in_chat = cut.cause.fragment == chat && cut.cause.channel == records::CHAT;
    let reached = |r: &Record| r.at < cut.started_at || (in_chat && r.seq <= cut.cause.seq);
    let said = async {
        let tail = api.channels(agent, chat).await?.into_iter().find(|c| c.name == records::CHAT).map_or(0, |c| c.seq);
        read_back(api, agent, chat, records::CHAT, tail + 1, |rs| rs.first().is_some_and(reached), &mut Budget::default()).await
    };
    let said: Vec<Record> = said.await.map_err(unread).map(|(rs, _)| rs).unwrap_or_default();
    let mut cause = said.iter().find(|r| in_chat && r.seq == cut.cause.seq).cloned();
    if cause.is_none() {
        cause = api.records(agent, &cut.cause.fragment, &cut.cause.channel, cut.cause.seq.saturating_sub(1), 1).await.map_err(|e| unread(e.into())).ok().and_then(|p| p.records.into_iter().find(|r| r.seq == cut.cause.seq));
    }
    let asked = cause.as_ref().and_then(note::asked);
    let text = note::text(&cut, asked.as_deref(), &note::replies(&said, me, &cut.turn));
    crate::ev!("note", { "turn": ts.turn, "cut": cut.turn, "steps": cut.steps_total, "bytes": text.len() });
    Some(text)
}

// ---- the hold ----
// ---- the hold ----

/// The bridge's answer to the platform's hold (docs/computers.md, "The
/// hold"): `held` exists while `hold` does, no claim is in flight (a claim
/// looks at the hold before each try, so none starts once it holds), and no
/// SimpleX core runs (`cores_quiet`: SimpleX stops them all as it sees the
/// hold, and starts none while it lasts; always true with no SimpleX), and
/// nothing of a runtime's own runs outside its turns (`runtimes_unquiet`:
/// each runtime is told the hold, `runtimes_held`, and quiets its own), and
/// not otherwise. The platform clears `held` before each hold, so an
/// answer is always to the hold it finds. The answer is `answer`: what the
/// save may leave out, one per line (none, empty). Looked at every
/// `HOLD_POLL_MS`, for the bridge's life.
#[allow(clippy::too_many_arguments)]
async fn answer_holds(hold: PathBuf, held: PathBuf, answer: String, in_flight: Arc<AtomicUsize>, cores_quiet: Arc<std::sync::atomic::AtomicBool>, runtimes_held: watch::Sender<bool>, runtimes_unquiet: Arc<AtomicUsize>, mut stop: watch::Receiver<bool>) {
    assert!(hold != held, "a hold and its answer are two files");
    assert!(answer.len() <= limits::HELD_ANSWER_MAX_BYTES, "an answer the platform reads whole");
    // bounded by the bridge's life: one look per `HOLD_POLL_MS`, ended by `stop`
    loop {
        let holding = hold.exists();
        runtimes_held.send_if_modified(|was| std::mem::replace(was, holding) != holding);
        let quiet = holding && in_flight.load(Ordering::SeqCst) == 0 && cores_quiet.load(Ordering::SeqCst) && runtimes_unquiet.load(Ordering::SeqCst) == 0;
        match (quiet, held.exists()) {
            // written whole, then renamed: the platform never reads half
            (true, false) => match write_whole(&held, answer.as_bytes()) {
                Ok(()) => crate::ev!("held", { "held": held.display().to_string() }),
                Err(e) => crate::ev!("held.failed", { "held": held.display().to_string(), "error": e.to_string() }),
            },
            (false, true) => {
                let _ = std::fs::remove_file(&held);
            }
            _ => {}
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(limits::HOLD_POLL_MS)) => {}
            _ = crate::net::stopped(&mut stop) => return,
        }
    }
}

// ---- keepalive ----

/// Holds `GET /api/computer/keepalive` open while `hold` is true, and only
/// then (decision 39: the computer stays awake while it is open). It pings
/// every `every`, as a `__live` socket does: one silent twice that is
/// opened again, since a deploy can cut the platform's side of it with no
/// close reaching here (#557), and the platform, holding no socket, counts
/// it closed.
pub(crate) async fn keepalive(api: Api, mut hold: watch::Receiver<bool>, every: Duration, stop: watch::Receiver<bool>) {
    assert!(!every.is_zero(), "the keepalive pings at an interval");
    let mut backoff = Backoff::default();
    // bounded by the bridge's life: one connection per pass
    loop {
        if *stop.borrow() {
            return;
        }
        if !*hold.borrow() {
            let mut stop2 = stop.clone();
            tokio::select! {
                r = hold.changed() => if r.is_err() { return },
                _ = crate::net::stopped(&mut stop2) => return,
            }
            continue;
        }
        let ws = match api.keepalive().await {
            Ok(ws) => ws,
            Err(e) => {
                crate::ev!("keepalive.failed", { "error": e });
                if !backoff.wait(stop.clone()).await {
                    return;
                }
                continue;
            }
        };
        backoff.reset();
        crate::ev!("keepalive.held");
        let (mut sink, mut stream) = ws.split();
        let mut stop2 = stop.clone();
        let mut ping = tokio::time::interval(every);
        ping.tick().await;
        let mut heard = Instant::now();
        // bounded by the socket, its silence, `hold`, or `stop`
        loop {
            tokio::select! {
                r = hold.changed() => {
                    if r.is_err() || !*hold.borrow() {
                        let _ = sink.close().await;
                        crate::ev!("keepalive.dropped");
                        break;
                    }
                }
                _ = ping.tick() => {
                    // dropped, not closed: its far side may hear nothing
                    if heard.elapsed() > 2 * every {
                        crate::ev!("keepalive.quiet", { "ms": heard.elapsed().as_millis() as u64 });
                        break;
                    }
                    if sink.send(Message::text(json!({ "type": "ping" }).to_string())).await.is_err() {
                        crate::ev!("keepalive.lost");
                        break;
                    }
                }
                m = stream.next() => match m {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        crate::ev!("keepalive.lost");
                        break;
                    }
                    Some(Ok(_)) => heard = Instant::now(),
                },
                _ = crate::net::stopped(&mut stop2) => {
                    let _ = sink.close().await;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Goal (SimpleX, after #584's mid-turn replies; per chat since Paul's
    /// 2026-10-26): of what an agent posts in its owner's SimpleX chat, their
    /// app gets every reply that is not interim (its answer, an open
    /// question, something said unasked), its files with it, and a card's
    /// question with its choices; the model's words before a tool call stay
    /// in the chat, as the chat's push leaves them out, and so do a turn's
    /// work and its owner's own words from the phone.
    #[test]
    fn what_goes_back_over_simplex() {
        let reply = |interim: bool| records::reply("It'll be sunny.", "t1", &[], 0, interim);
        assert_eq!(for_simplex(records::CHAT, &reply(false), 0).as_deref(), Some("It'll be sunny."));
        assert_eq!(for_simplex(records::CHAT, &reply(false), 2).as_deref(), Some("It'll be sunny."), "its files go as files, not noted");
        assert_eq!(for_simplex(records::CHAT, &reply(true), 0), None, "interim: the chat's alone");
        assert_eq!(for_simplex(records::CHAT, &records::reply("", "t1", &[], 0, false), 1).as_deref(), Some(""), "a file alone goes");
        assert_eq!(for_simplex(records::CHAT, &records::reply(" ", "t1", &[], 0, false), 0), None, "nothing said");
        let agent = crate::runtime::Agent { fragment: "juniper--k3x9".into(), identity: "npub1juniper".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None };
        assert_eq!(for_simplex(records::CHAT, &records::relayed_message("hi from my phone", &agent, records::VIA_SIMPLEX, &[]), 0), None, "its owner's own, from the phone");
        let option = |id: &str, label: &str, words: bool| records::PromptOption { id: id.into(), label: label.into(), style: None, words };
        let card = records::turn_prompt("t1", "c-1", "What should I plant?", &[option("c0", "Basil", false), option("c1", "Mint", false), option("other", "Something else", true)], "npub1paul", 1);
        assert_eq!(for_simplex(records::WORK, &card, 0).as_deref(), Some("What should I plant?\n\n(Answer with one of: Basil, Mint, or in your own words.)"));
        let buttons = records::turn_prompt("t1", "p-1", "Run it?", &[option("once", "Allow once", false)], "npub1paul", 1);
        assert!(for_simplex(records::WORK, &buttons, 0).is_some_and(|t| t.ends_with("answer it in our chat.)")));
        assert_eq!(for_simplex(records::WORK, &json!({ "kind": "turn.start", "turn": "t1" }), 0), None, "a turn's work records stay");
    }

    /// Goal (Alex's review of #548): a post says how it ended, so what goes
    /// back over SimpleX, and an inbound file's deletion, wait for one on
    /// the chat: a refusal (403, 404) is no post; another life's under the
    /// same id (409) is, as another life's (a cut turn's continuation
    /// waits for its lost end to be this life's own).
    #[tokio::test]
    async fn a_post_says_how_it_ended() {
        for (status, want) in [(200u16, Sent::Posted), (409, Sent::Taken), (403, Sent::Refused), (404, Sent::Refused)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
            let addr = listener.local_addr().expect("its address");
            let (_stop_tx, stop) = watch::channel(false);
            let answer = move |_req: hyper::Request<hyper::body::Incoming>, _peer: std::net::SocketAddr| async move {
                let code = hyper::StatusCode::from_u16(status).expect("a status");
                if status == 200 { crate::net::json_answer(code, &json!({ "record": { "seq": 1 }, "replayed": false })) } else { crate::net::refusal(code, "refused", "as asked") }
            };
            tokio::spawn(crate::net::serve(listener, answer, stop.clone()));
            let api = Api::new(&format!("http://{addr}")).expect("an address");
            let sent = post(&api, "chat--a1b2", "juniper--k3x9", records::CHAT, "sx:1", json!({ "text": "hi" }), &[], None, limits::POST_TRIES_MAX, stop).await;
            assert_eq!(sent, want, "{status}");
            assert!(sent.ended());
            assert_eq!(sent.on_chat(), status == 200 || status == 409, "on the chat, ours or another life's: {status}");
        }
    }

    /// Goal (Alex's review of #548): a message its agent relays from its
    /// owner, quoted, is its owner's words to the runtime, not the agent's
    /// own; anything else the agent posted is its own.
    #[test]
    fn a_relayed_message_quoted_is_its_owners() {
        let agent = crate::runtime::Agent { fragment: "juniper--k3x9".into(), identity: "npub1juniper".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None };
        let relayed = records::relayed_message("hi from my phone", &agent, records::VIA_SIMPLEX, &[]);
        assert!(!own_words("npub1juniper", &relayed, &agent), "its owner's, relayed");
        assert!(own_words("npub1juniper", &json!({ "text": "a reply", "turn": "t1" }), &agent));
        assert!(!own_words("npub1paul", &json!({ "text": "hi" }), &agent));
        let for_someone_else = records::relayed_message("x", &crate::runtime::Agent { owner: "npub1skyler".into(), ..agent.clone() }, records::VIA_SIMPLEX, &[]);
        assert!(own_words("npub1juniper", &for_someone_else, &agent), "a relay for anyone else is no owner's");
    }
}
