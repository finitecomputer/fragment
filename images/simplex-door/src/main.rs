//! A SimpleX door's runner (docs/computers.md, "F: messengers outside the
//! computer"): one per door container, beside SimpleX's own core. The core
//! holds one link to its owner's SimpleX app, by the bridge's own rules
//! (`fragment_bridge::simplex::link`, pure and tested there); the runner
//! is the I/O around them, with one peer, its door (door.rs):
//!
//! 1. **A start** restores the door's newest save (or none: a new door),
//!    sets both databases to WAL mode (save.rs `wal` says why), starts the
//!    core, and tells the link whether the save is behind (its door says
//!    so), so a rollback forces one ratchet sync before anything is sent.
//! 2. **Its owner's messages** go to its door one at a time, in order, each
//!    until it is taken; the link's cursor (`delivered`) moves after, and
//!    a start reads back to it (a core killed mid-burst committed messages
//!    whose events died with it).
//! 3. **What the door asks** (`DOOR_HTTP`): `GET /state`; `POST /link` and
//!    `POST /unlink`; `POST /reply {key, text}`, sent once a key. Each ask
//!    but the state carries its mark (door.rs, "an effect").
//! 4. **Saves** follow changes (save.rs `Due`), the core never paused, and
//!    a planned stop (SIGTERM, or a gap in its deliveries) saves last.
//!
//! **Marks.** A save's mark is the door's newest mark this runner had seen
//! when its link was last quiet: no command asked of the core unanswered,
//! no reply held, no sync under way. So everything a mark numbers is
//! committed in the databases the save copies after it (an effect not yet
//! counted only makes the door think a save behind, which costs one sync).
//!
//! Env: `DOOR_GENERATION` (this start's, from 1), `DOOR_NAME` (the agent's),
//! `DOOR_API`, `DOOR_DIR`, `DOOR_CORE_BIN`, `DOOR_HTTP`, and `DOOR_SMP` (a
//! test's server; else the core's presets).

mod core;
mod door;
mod save;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use fragment_bridge::ev;
use fragment_bridge::net::{self, Body};
use fragment_bridge::simplex::link::{Action, Inbound, Input, Link, Notice, Phase};
use fragment_bridge::simplex::{run, wire};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::door::{Delivery, Door, Restore};

/// Messages to the loop waiting, at most: past it a sender waits (the
/// core's socket is read no faster than the loop takes its frames).
const MSGS_MAX: usize = 256;
/// The loop's clock: databases watched, timers and deliveries.
const TICK_MS: u64 = 250;
/// Replies' keys kept, the newest: one asked again within them is not sent
/// again (the door asks again when its answer was lost).
const REPLIED_KEPT: usize = 64;
const KEY_MAX_BYTES: usize = 128;
/// A reply's body, at most: the bridge's own bound on a reply's text, and
/// its key and framing.
const REPLY_BODY_MAX_BYTES: usize = fragment_bridge::limits::REPLY_TEXT_MAX_BYTES + 1024;
const NOTICES_KEPT: usize = 16;
/// Messages waiting to be delivered, at most: a start's catch-up's and live
/// ones. Past it one is dropped and the runner stops once the rest are
/// delivered (a planned stop), so its next start reads back to it.
const DELIVERIES_MAX: usize = 2 * fragment_bridge::simplex::link::SCAN_MAX;
/// A door that will not give its save for this long: the runner ends (its
/// door starts it again).
const RESTORE_MS_MAX: u64 = 10 * 60_000;
/// A planned stop waits this long for the link to be quiet and its last
/// save to be taken.
const STOP_MS_MAX: u64 = 8_000;

/// What each save holds beside the databases, rolled back with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    link: Link,
    /// The newest of the door's marks committed in the databases (`Marks`).
    #[serde(default)]
    mark: u64,
    /// The newest replies' keys (`REPLIED_KEPT`).
    #[serde(default)]
    replied: VecDeque<String>,
}

pub enum Msg {
    Frame(String),
    CoreGone(String),
    Asked(Asked, oneshot::Sender<Response<Body>>),
    Delivered { item: i64, mark: u64 },
    DeliveryFailed,
    SaveEnded(Result<door::Saved, String>),
    Stop(&'static str),
}

pub enum Asked {
    State,
    Link { mark: u64 },
    Unlink { mark: u64 },
    Reply { key: String, text: String, mark: u64 },
}

/// What `GET /state` says.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct View<'a> {
    generation: u64,
    ready: bool,
    phase: &'static str,
    link: Option<&'a str>,
    short: Option<&'a str>,
    name: Option<&'a str>,
    syncing: bool,
    delivered: i64,
    mark: u64,
    saves: u64,
    forced_syncs: u32,
    waiting: usize,
    notices: &'a VecDeque<String>,
}

struct Runner {
    door: Door,
    dir: PathBuf,
    generation: u64,
    kept: Kept,
    /// The newest mark seen (an ask taken, a delivery's answer, or the
    /// door's newest at the start): `kept.mark` once the link is quiet.
    mark_seen: u64,
    core: core::Core,
    corr: u64,
    pending: BTreeMap<u64, wire::Cmd>,
    /// `Input::Started` applied: asks are taken.
    up: bool,
    stopping: Option<(&'static str, Instant)>,
    /// The seq of the save a planned stop began: its answer ends the runner.
    last: Option<u64>,
    deliveries: VecDeque<Inbound>,
    delivering: bool,
    delivery_wait: fragment_bridge::net::Backoff,
    delivery_at: Option<Instant>,
    gap: bool,
    resync_at: Option<Instant>,
    recheck_at: Option<Instant>,
    due: save::Due,
    watch: save::Watch,
    versions: [i64; 2],
    /// Frames the core said unasked since the databases were last read.
    events: u64,
    saving: bool,
    seq: u64,
    saves: u64,
    forced_syncs: u32,
    notices: VecDeque<String>,
    born: Instant,
    to: mpsc::Sender<Msg>,
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let generation: u64 = std::env::var("DOOR_GENERATION").ok().and_then(|g| g.parse().ok()).filter(|g| *g > 0).expect("DOOR_GENERATION: this start's, from 1");
    let dir = PathBuf::from(env("DOOR_DIR", "/data/door"));
    let cfg = run::Config { bin: env("DOOR_CORE_BIN", "/opt/door/simplex-chat").into(), dir: dir.clone(), smp: std::env::var("DOOR_SMP").ok(), xftp: None };
    let name = run::display_name(std::env::var("DOOR_NAME").ok().as_deref(), "agent");
    let door = Door::new(&env("DOOR_API", "http://door.fragment.internal"), generation).expect("DOOR_API: http://host[:port]");
    ev!("door.start", { "generation": generation });
    for sub in ["files", "tmp"] {
        std::fs::create_dir_all(dir.join(sub)).expect("the door's folders");
    }

    let (to, mut rx) = mpsc::channel::<Msg>(MSGS_MAX);
    // its door's calls first, so it sees the runner come up (each answered
    // 503 until the link is up); held for the runner's life
    let (_stop_tx, stop_rx) = watch::channel(false);
    let listener = tokio::net::TcpListener::bind(env("DOOR_HTTP", "0.0.0.0:8080")).await.expect("the runner's port");
    let asks = to.clone();
    tokio::spawn(net::serve(listener, move |req, _| handle(asks.clone(), req), stop_rx));

    let (kept, door_mark, behind) = restore(&door, &dir).await;
    save::wal(&dir).expect("the databases in WAL mode");
    let watch = save::Watch::open(&dir).expect("the databases watched");
    let core = match core::start(&cfg, &dir, &name, to.clone()).await {
        Ok(c) => c,
        Err(e) => {
            ev!("door.core_failed", { "why": e });
            std::process::exit(1);
        }
    };
    let mut r = Runner {
        door,
        dir,
        generation,
        kept,
        mark_seen: door_mark,
        core,
        corr: 0,
        pending: BTreeMap::new(),
        up: false,
        stopping: None,
        last: None,
        deliveries: VecDeque::new(),
        delivering: false,
        delivery_wait: fragment_bridge::net::Backoff::default(),
        delivery_at: None,
        gap: false,
        resync_at: None,
        recheck_at: None,
        due: save::Due::default(),
        versions: watch.versions(),
        watch,
        events: 0,
        saving: false,
        seq: 0,
        saves: 0,
        forced_syncs: 0,
        notices: VecDeque::new(),
        born: Instant::now(),
        to: to.clone(),
    };
    // its name (a profile its database made earlier keeps the earlier one),
    // its messages' expiry, then the link's start
    r.ask(wire::Cmd::Profile { name });
    r.ask(wire::Cmd::ExpireDay);
    r.up = true;
    r.apply(Input::Started { rollback: behind });
    // each start's first save: what it restored, or the new profile
    r.due.changed(0);
    ev!("door.up", { "generation": generation, "behind": behind, "phase": phase(&r.kept.link.phase) });

    let term = to.clone();
    tokio::spawn(async move {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
        sig.recv().await;
        let _ = term.send(Msg::Stop("sigterm")).await;
    });
    let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
    // bounded by the runner's life: it ends by `exit`
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(m) => r.on(m),
                None => unreachable!("the loop holds a sender of its own"),
            },
            _ = tick.tick() => r.tick(),
        }
    }
}

/// The newest save, unpacked in place; or none, and nothing left of an
/// earlier life's files (the door's save is the one truth).
async fn restore(door: &Door, dir: &Path) -> (Kept, u64, bool) {
    let t0 = Instant::now();
    let mut wait = fragment_bridge::net::Backoff::default();
    // bounded by RESTORE_MS_MAX
    let restored = loop {
        match door.restore().await {
            Ok(r) => break r,
            Err(e) => {
                ev!("door.restore_failed", { "error": e.to_string() });
                if t0.elapsed() > Duration::from_millis(RESTORE_MS_MAX) {
                    std::process::exit(1);
                }
                tokio::time::sleep(Duration::from_millis(wait.next_ms())).await;
            }
        }
    };
    for db in save::DBS {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{db}{suffix}")));
        }
    }
    match restored {
        Restore::None => (Kept::default(), 0, false),
        Restore::Save { packed, mark, behind } => {
            // one that will not restore: a code of its own, so its door
            // starts it from an older one
            let files = save::unpack(&packed).unwrap_or_else(|e| unreadable(&e.to_string()));
            let mut kept = None;
            for (name, bytes) in files {
                if name == save::STATE {
                    kept = Some(serde_json::from_slice::<Kept>(&bytes).unwrap_or_else(|e| unreadable(&e.to_string())));
                } else {
                    let tmp = dir.join(format!("{name}.restoring"));
                    std::fs::write(&tmp, &bytes).expect("a database restored");
                    std::fs::rename(&tmp, dir.join(&name)).expect("a database restored");
                }
            }
            let kept = kept.unwrap_or_else(|| unreadable("no runner's state"));
            assert!(behind || kept.mark >= mark, "a save the door says is not behind holds its newest mark");
            ev!("door.restored", { "bytes": packed.len(), "mark": kept.mark, "doorMark": mark, "behind": behind });
            let newest = mark.max(kept.mark);
            (kept, newest, behind)
        }
    }
}

/// The exit code of a start whose save would not restore.
const UNREADABLE_EXIT: i32 = 2;

fn unreadable(why: &str) -> ! {
    ev!("door.save_unreadable", { "why": why });
    std::process::exit(UNREADABLE_EXIT);
}

fn phase(p: &Phase) -> &'static str {
    match p {
        Phase::Off => "off",
        Phase::Asking => "asking",
        Phase::Inviting { .. } => "inviting",
        Phase::Linked { .. } => "linked",
    }
}

impl Runner {
    fn now(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    fn on(&mut self, m: Msg) {
        match m {
            Msg::Frame(text) => self.frame(&text),
            Msg::CoreGone(why) => {
                ev!("door.core_gone", { "why": why });
                std::process::exit(1);
            }
            Msg::Asked(a, answer) => {
                let _ = answer.send(self.asked(a));
            }
            Msg::Delivered { item, mark } => {
                let done = self.deliveries.pop_front().expect("a delivery is the oldest waiting");
                assert_eq!(done.item, item, "deliveries end in order");
                self.delivering = false;
                self.delivery_wait.reset();
                self.mark_seen = self.mark_seen.max(mark);
                if !self.gap && item > 0 {
                    self.apply(Input::Delivered(item));
                }
                self.settle();
            }
            Msg::DeliveryFailed => {
                self.delivering = false;
                self.delivery_at = Some(Instant::now() + Duration::from_millis(self.delivery_wait.next_ms()));
            }
            Msg::SaveEnded(r) => self.save_ended(r),
            Msg::Stop(why) => self.begin_stop(why),
        }
    }

    fn begin_stop(&mut self, why: &'static str) {
        if self.stopping.is_none() {
            ev!("door.stopping", { "why": why });
            self.stopping = Some((why, Instant::now()));
        }
    }

    /// A command to the core, answered by its `corrId`.
    fn ask(&mut self, cmd: wire::Cmd) {
        self.corr += 1;
        if matches!(cmd, wire::Cmd::Sync { force: true, .. }) {
            self.forced_syncs += 1;
        }
        match self.core.writer.try_send(cmd.frame(self.corr)) {
            Ok(()) => {
                self.pending.insert(self.corr, cmd);
            }
            Err(_) => {
                ev!("door.unsent", { "corr": self.corr });
                if let wire::Cmd::React { item, on, .. } = cmd {
                    self.kept.link.apply(Input::NotReacted { item, on });
                }
            }
        }
    }

    /// The link's rules on `input`, and what they ask.
    fn apply(&mut self, input: Input<'_>) {
        let before = self.kept.clone();
        let actions = self.kept.link.apply(input);
        for a in actions {
            match a {
                Action::Ask(cmd) => self.ask(cmd),
                Action::Forward(i) => {
                    if self.deliveries.len() < DELIVERIES_MAX {
                        queue(&mut self.deliveries, i, self.delivering);
                    } else {
                        // the next start reads back to it: no cursor moves past it
                        ev!("door.delivery_dropped", { "item": i.item });
                        self.gap = true;
                    }
                }
                Action::Tell(n) => self.told(n),
                Action::Save => self.due.changed(self.now()),
            }
        }
        if self.kept != before {
            self.due.changed(self.now());
        }
    }

    fn told(&mut self, n: Notice) {
        ev!("door.told", { "notice": format!("{n:?}") });
        if self.notices.len() == NOTICES_KEPT {
            self.notices.pop_front();
        }
        self.notices.push_back(format!("{n:?}"));
        match n {
            // deaf to its contact until it subscribes again: a planned restart
            Notice::Taken => self.begin_stop("taken"),
            Notice::Stalled => self.recheck_at = Some(Instant::now() + Duration::from_millis(run::RECHECK_MS)),
            _ => {}
        }
    }

    /// The mark moves once the link is quiet (main.rs, "Marks").
    fn settle(&mut self) {
        let l = &self.kept.link;
        let quiet = self.up && self.pending.is_empty() && l.outbox.is_empty() && !l.syncing && !l.forced;
        if quiet && self.mark_seen > self.kept.mark {
            self.kept.mark = self.mark_seen;
            self.due.changed(self.now());
        }
    }

    fn frame(&mut self, text: &str) {
        let frame = match wire::parse(text) {
            Ok(f) => f,
            Err(e) => {
                ev!("door.frame_unread", { "error": wire::cut(&e.to_string(), 200) });
                return;
            }
        };
        let asked = frame.corr.and_then(|c| self.pending.remove(&c));
        if let Some(wire::Cmd::React { item, on, .. }) = &asked {
            self.apply(Input::Reacted { item: *item, on: *on });
        }
        match (&asked, &frame.resp) {
            (Some(wire::Cmd::Conns { again }), wire::Resp::Conns(conns)) => {
                let since = self.core.spawned_ms;
                self.apply(Input::Conns { conns, since, again: *again });
            }
            (Some(cmd), wire::Resp::Error { what }) => {
                ev!("door.refused", { "what": wire::cut(what, 200) });
                self.apply(Input::Failed { cmd, what });
            }
            (Some(_), resp) => self.apply(Input::Said(resp)),
            (None, resp) => {
                self.events += 1;
                self.apply(Input::Said(resp));
            }
        }
        self.settle();
    }

    fn asked(&mut self, a: Asked) -> Response<Body> {
        let refuse = |status: StatusCode, why: &str| net::refusal(status, "door", why);
        if let Asked::State = a {
            let (link, short) = match &self.kept.link.phase {
                Phase::Inviting { link, short, .. } => (Some(link.as_str()), short.as_deref()),
                _ => (None, None),
            };
            let name = match &self.kept.link.phase {
                Phase::Linked { name, .. } => Some(name.as_str()),
                _ => None,
            };
            let view = View {
                generation: self.generation,
                ready: self.up && self.stopping.is_none(),
                phase: phase(&self.kept.link.phase),
                link,
                short,
                name,
                syncing: self.kept.link.syncing,
                delivered: self.kept.link.delivered,
                mark: self.kept.mark,
                saves: self.saves,
                forced_syncs: self.forced_syncs,
                waiting: self.deliveries.len(),
                notices: &self.notices,
            };
            return net::respond(StatusCode::OK, "application/json", serde_json::to_vec(&view).expect("a view is JSON"));
        }
        // every mark shown is taken, whatever the ask's end: one numbering
        // what never happened is covered by any save, and one not taken would
        // leave every later save behind (a sync at each start)
        let (Asked::Link { mark } | Asked::Unlink { mark } | Asked::Reply { mark, .. }) = &a else { unreachable!("the state is answered above") };
        self.mark_seen = self.mark_seen.max(*mark);
        if !self.up || self.stopping.is_some() {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the door is starting or stopping");
        }
        match a {
            Asked::State => unreachable!("answered above"),
            Asked::Link { .. } => self.apply(Input::Want),
            Asked::Unlink { .. } => self.apply(Input::Unlink),
            Asked::Reply { key, text, .. } => {
                if self.kept.replied.contains(&key) {
                    self.settle();
                    return net::respond(StatusCode::OK, "application/json", r#"{"replayed":true}"#);
                }
                if self.kept.link.contact().is_none() {
                    return refuse(StatusCode::CONFLICT, "not linked");
                }
                if self.kept.replied.len() == REPLIED_KEPT {
                    self.kept.replied.pop_front();
                }
                self.kept.replied.push_back(key);
                self.apply(Input::Reply(&text));
                self.settle();
                return net::respond(StatusCode::OK, "application/json", r#"{"replayed":false}"#);
            }
        }
        self.settle();
        net::respond(StatusCode::ACCEPTED, "application/json", "{}")
    }

    fn tick(&mut self) {
        let now = self.now();
        let versions = self.watch.versions();
        if versions != self.versions {
            // the agent database alone, the core silent: its idle statistics
            if versions[0] != self.versions[0] || self.events > 0 {
                self.due.changed(now);
            } else {
                self.due.idle_stats(now);
            }
            self.versions = versions;
            self.events = 0;
        }
        let at = Instant::now();
        if self.kept.link.syncing {
            let due = *self.resync_at.get_or_insert(at + Duration::from_millis(run::RESYNC_MS));
            if at >= due {
                self.resync_at = None;
                self.apply(Input::Resync);
            }
        } else {
            self.resync_at = None;
        }
        if self.recheck_at.is_some_and(|t| at >= t) {
            self.recheck_at = None;
            self.apply(Input::Recheck);
        }
        self.deliver(at);
        self.settle();
        if self.gap && self.deliveries.is_empty() && !self.delivering {
            self.begin_stop("gap");
        }
        if let Some((why, since)) = self.stopping {
            return self.stop(why, since);
        }
        if !self.saving && self.due.take(now) {
            self.save();
        }
    }

    /// The oldest waiting message to its door, one at a time.
    fn deliver(&mut self, at: Instant) {
        // a start's catch-up first: a live message forwarded meanwhile waits
        // for the older ones it reads back, so the door takes them in order
        if self.delivering || self.kept.link.scan.running || self.delivery_at.is_some_and(|t| at < t) {
            return;
        }
        // it stays the oldest waiting until its door takes it
        let Some(i) = self.deliveries.front().cloned() else { return };
        self.delivering = true;
        self.delivery_at = None;
        let (door, to) = (self.door.clone(), self.to.clone());
        tokio::spawn(async move {
            let msg = match door.deliver(&Delivery::of(&i)).await {
                Ok(mark) => Msg::Delivered { item: i.item, mark },
                Err(e) => {
                    ev!("door.deliver_failed", { "item": i.item, "error": e.to_string() });
                    Msg::DeliveryFailed
                }
            };
            let _ = to.send(msg).await;
        });
    }

    /// A planned stop: asks refused, the link left to go quiet and what is
    /// under way to end (at most `STOP_MS_MAX`), then one last save.
    fn stop(&mut self, why: &'static str, since: Instant) {
        // a save under way ends first: it may have begun before the stop
        if self.saving {
            return;
        }
        let quiet = self.pending.is_empty() && !self.delivering;
        if !quiet && since.elapsed() < Duration::from_millis(STOP_MS_MAX) {
            return;
        }
        ev!("door.last_save", { "why": why, "quiet": quiet });
        self.save();
        self.last = Some(self.seq);
    }

    fn save(&mut self) {
        assert!(!self.saving, "one save at a time");
        self.saving = true;
        self.seq += 1;
        let (seq, mark) = (self.seq, self.kept.mark);
        let state = serde_json::to_vec(&self.kept).expect("the runner's state is JSON");
        let (dir, door, to) = (self.dir.clone(), self.door.clone(), self.to.clone());
        tokio::spawn(async move {
            let made = tokio::task::spawn_blocking(move || save::make(&dir, &dir.join("save"), &state)).await.expect("a save's copy ran");
            let r = match made {
                Ok(packed) => door.save(seq, mark, packed).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = to.send(Msg::SaveEnded(r)).await;
        });
    }

    fn save_ended(&mut self, r: Result<door::Saved, String>) {
        self.saving = false;
        match r {
            Ok(door::Saved::Taken) => {
                self.saves += 1;
                ev!("door.saved", { "seq": self.seq, "mark": self.kept.mark });
                if let (Some((why, _)), true) = (self.stopping, self.last == Some(self.seq)) {
                    ev!("door.stopped", { "why": why });
                    std::process::exit(0);
                }
            }
            Ok(door::Saved::Superseded) => {
                ev!("door.superseded", { "generation": self.generation });
                std::process::exit(1);
            }
            Err(e) => {
                ev!("door.save_failed", { "seq": self.seq, "error": e });
                self.due.failed(self.now());
                self.last = None;
                if self.stopping.is_some_and(|(_, since)| since.elapsed() > Duration::from_millis(2 * STOP_MS_MAX)) {
                    std::process::exit(1);
                }
            }
        }
    }
}

/// A message forwarded, in its place among those waiting: by the core's
/// own item order (one database's, so it is the order they arrived in),
/// the one being delivered kept first.
fn queue(deliveries: &mut VecDeque<Inbound>, i: Inbound, delivering: bool) {
    let from = usize::from(delivering).min(deliveries.len());
    let at = from + deliveries.range(from..).take_while(|q| q.item <= i.item).count();
    deliveries.insert(at, i);
    assert!(deliveries.range(from..).zip(deliveries.range(from + 1..)).all(|(a, b)| a.item <= b.item), "deliveries wait oldest first");
}

/// The door's calls, to the loop.
async fn handle(to: mpsc::Sender<Msg>, req: Request<Incoming>) -> Response<Body> {
    let refuse = |status: StatusCode, why: &str| net::refusal(status, "door", why);
    let mark = req.headers().get(door::MARK_HEADER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let asked = match (req.method().as_str(), req.uri().path(), mark) {
        ("GET", "/state", _) => Asked::State,
        ("POST", "/link", Some(mark)) => Asked::Link { mark },
        ("POST", "/unlink", Some(mark)) => Asked::Unlink { mark },
        ("POST", "/reply", Some(mark)) => {
            let Ok(body) = Limited::new(req.into_body(), REPLY_BODY_MAX_BYTES).collect().await.map(|b| b.to_bytes()) else {
                return refuse(StatusCode::PAYLOAD_TOO_LARGE, "a reply past its bound");
            };
            match reply(&body) {
                Ok((key, text)) => Asked::Reply { key, text, mark },
                Err(why) => return refuse(StatusCode::BAD_REQUEST, why),
            }
        }
        ("POST", "/link" | "/unlink" | "/reply", None) => return refuse(StatusCode::BAD_REQUEST, "an ask carries its mark"),
        _ => return refuse(StatusCode::NOT_FOUND, "no such route"),
    };
    let (tx, rx) = oneshot::channel();
    if to.send(Msg::Asked(asked, tx)).await.is_err() {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the runner is ending");
    }
    rx.await.unwrap_or_else(|_| refuse(StatusCode::SERVICE_UNAVAILABLE, "the runner is ending"))
}

/// A reply's body: `{key, text}`, a key of letters, digits, `-`, `_`, `:`
/// and `.`, and something to say.
fn reply(body: &Bytes) -> Result<(String, String), &'static str> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        key: String,
        text: String,
    }
    let r: Reply = serde_json::from_slice(body).map_err(|_| "a reply is {key, text}")?;
    let key_ok = !r.key.is_empty() && r.key.len() <= KEY_MAX_BYTES && r.key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'));
    if !key_ok {
        return Err("a reply's key is 1-128 of letters, digits, -, _, : and .");
    }
    if r.text.trim().is_empty() || r.text.len() > fragment_bridge::limits::REPLY_TEXT_MAX_BYTES {
        return Err("a reply says something, within its bound");
    }
    Ok((r.key, r.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: what a save holds beside the databases reads back as it was
    /// written (the link's own file shape inside), and an old save's
    /// missing fields read as nothing yet. Method: round trips.
    #[test]
    fn the_kept_state_round_trips() {
        let mut kept = Kept { mark: 9, ..Default::default() };
        kept.link.phase = Phase::Linked { contact: 3, name: "paul".into() };
        kept.link.delivered = 41;
        kept.replied.push_back("chat:12".into());
        let bytes = serde_json::to_vec(&kept).unwrap();
        assert_eq!(serde_json::from_slice::<Kept>(&bytes).unwrap(), kept);
        let old: Kept = serde_json::from_str(r#"{"link":{"phase":"off"}}"#).unwrap();
        assert_eq!(old, Kept::default());
    }

    /// Goal: deliveries go oldest first by the core's item order, however
    /// they were forwarded (a live message during a start's catch-up comes
    /// before the older ones it reads back: seen in the lab, 1 run in 6),
    /// and the one being delivered stays first. Method: out of order in,
    /// with and without one in flight.
    #[test]
    fn deliveries_wait_oldest_first() {
        let m = |item: i64| Inbound { id: format!("s{item}"), kind: "text".into(), text: String::new(), at: String::new(), item, file: None };
        let items = |q: &VecDeque<Inbound>| q.iter().map(|i| i.item).collect::<Vec<_>>();
        let mut q = VecDeque::new();
        for item in [30, 23, 24, 27, 25, 26] {
            queue(&mut q, m(item), false);
        }
        assert_eq!(items(&q), vec![23, 24, 25, 26, 27, 30]);
        let mut q = VecDeque::from([m(40)]);
        queue(&mut q, m(38), true);
        queue(&mut q, m(39), true);
        assert_eq!(items(&q), vec![40, 38, 39], "the one in flight stays first");
    }

    /// Goal: a reply's body is taken only as `{key, text}` within bounds.
    /// Method: valid, and each way to be invalid.
    #[test]
    fn a_reply_is_its_key_and_text() {
        assert_eq!(reply(&Bytes::from_static(br#"{"key":"chat:12","text":"hi"}"#)), Ok(("chat:12".into(), "hi".into())));
        for bad in [r#"{"key":"","text":"hi"}"#, r#"{"key":"a b","text":"hi"}"#, r#"{"key":"k","text":"  "}"#, r#"{"key":"k"}"#, r#"{"key":"k","text":"hi","to":"x"}"#, "nope"] {
            assert!(reply(&Bytes::from(bad)).is_err(), "{bad}");
        }
        let long = format!(r#"{{"key":"k","text":"{}"}}"#, "x".repeat(fragment_bridge::limits::REPLY_TEXT_MAX_BYTES + 1));
        assert!(reply(&Bytes::from(long)).is_err());
        let key = format!(r#"{{"key":"{}","text":"hi"}}"#, "k".repeat(KEY_MAX_BYTES + 1));
        assert!(reply(&Bytes::from(key)).is_err());
    }
}
