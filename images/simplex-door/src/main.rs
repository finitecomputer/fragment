//! A SimpleX door's runner (docs/computers.md, "F: messengers outside the
//! computer"): one per door container, beside SimpleX's own core. The core
//! holds one link to its owner's SimpleX app, by the bridge's own rules
//! (`fragment_bridge::simplex::link`, pure and tested there); the runner
//! is the I/O around them, with one peer, its door (door.rs):
//!
//! 1. **A start** restores the door's newest save (or none: a new door),
//!    checked whole before the core sees it, sets both databases to WAL
//!    mode (save.rs `wal` says why), starts the core, and tells the link
//!    whether the save is behind (its door says so), so a rollback forces
//!    one ratchet sync before anything is sent.
//! 2. **Its owner's messages** are read on from the link's cursor
//!    (`delivered`) a page at a time and go to its door one at a time, in
//!    order, each until it is taken; the cursor moves after. The core's own
//!    database is the queue: nothing waits in memory but one page, and a
//!    start reads on from the cursor what any earlier life took.
//! 3. **What the door asks** (`DOOR_HTTP`): `GET /state`; `POST /link` and
//!    `POST /unlink`; `POST /reply {key, text}`, sent once a key. Each ask
//!    but the state carries its mark (door.rs, "an effect").
//! 4. **Saves** follow changes (save.rs `Due`), the core never paused; a
//!    planned stop (SIGTERM, or another client taking its queues) stops the
//!    core first, then saves last.
//!
//! **Marks.** An ask is taken only under a mark newer than every mark the
//! runner has seen; an older one (its request overtaken by a later mark's)
//! is refused, and its door asks it again under a new mark. So no ask ever
//! acts after a save that claims a later mark. A save's mark is the newest
//! mark seen when its link was last quiet: no command asked of the core
//! unanswered, no reply held, no sync under way; everything it numbers is
//! then committed in the databases the save copies after it (an effect not
//! yet counted only makes the door think a save behind: one sync).
//!
//! Env: `DOOR_GENERATION` (this start's, from 1), `DOOR_NAME` (the agent's),
//! `DOOR_API`, `DOOR_DIR`, `DOOR_CORE_BIN`, `DOOR_HTTP`, and `DOOR_SMP` (a
//! test's server; else the core's presets).

mod core;
mod door;
mod save;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use fragment_bridge::ev;
use fragment_bridge::limits::REPLY_TEXT_MAX_BYTES;
use fragment_bridge::net::{self, Body};
use fragment_bridge::simplex::link::{self, Action, Inbound, Input, Link, Notice, Phase};
use fragment_bridge::simplex::wire::{self, Cmd, Content, Resp};
use fragment_bridge::simplex::run;
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
/// A reply's body, at most: its text at the bridge's bound with every byte
/// escaped as JSON's longest (`\u00XX`, 6 bytes), its key and framing.
const REPLY_BODY_MAX_BYTES: usize = 6 * REPLY_TEXT_MAX_BYTES + 1024;
const NOTICES_KEPT: usize = 16;
/// A read on from the cursor: the link's own catch-up page.
const PAGE: u16 = link::CATCHUP_ITEMS;
/// A door that will not give its save for this long: the runner ends (its
/// door starts it again).
const RESTORE_MS_MAX: u64 = 10 * 60_000;
/// A planned stop waits this long for the link to be quiet before it stops
/// the core, and twice this for its last save to be taken.
const STOP_MS_MAX: u64 = 8_000;
/// The exit code of a start whose save would not restore: its door starts
/// it from an older one.
const UNREADABLE_EXIT: i32 = 2;

/// What each save holds beside the databases, rolled back with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    link: Link,
    /// The newest of the door's marks committed in the databases (Marks).
    #[serde(default)]
    mark: u64,
    /// The newest replies' keys whose every part the core took or the link
    /// holds (`REPLIED_KEPT`).
    #[serde(default)]
    replied: VecDeque<String>,
}

pub enum Msg {
    Frame(String),
    CoreGone(String),
    Asked(Asked, oneshot::Sender<Response<Body>>),
    Delivered { item: i64, mark: u64 },
    DeliveryFailed,
    SaveEnded { seq: u64, mark: u64, result: Result<door::Saved, String> },
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
    /// The newest mark seen (an ask's, a delivery's answer, or the door's
    /// newest at the start): `kept.mark` once the link is quiet.
    mark_seen: u64,
    core: core::Core,
    corr: u64,
    pending: BTreeMap<u64, Cmd>,
    /// Replies taken whose parts the core has not all answered: each key,
    /// and its sends still out. A key asked again meanwhile waits.
    replying: Vec<(String, BTreeSet<u64>)>,
    /// `Input::Started` applied: asks are taken.
    up: bool,
    stopping: Option<(&'static str, Instant)>,
    /// When a planned stop asked the core to end, whether it was killed,
    /// and whether it has ended.
    core_stop: Option<Instant>,
    killed: bool,
    core_gone: bool,
    /// The seq of the save a planned stop began: its answer ends the runner.
    last: Option<u64>,
    /// The deliverable messages of the last read on from the cursor, oldest
    /// first (at most `PAGE`).
    page: VecDeque<Inbound>,
    /// That read (its corr), while the core has not answered it: on from
    /// `anchor` (an item this life read, which the core held then), or,
    /// with none (a new link, or the anchor expired), a seek back from the
    /// newest page, `seek` the oldest item it has passed, to the cursor.
    reading: Option<u64>,
    anchor: Option<i64>,
    seek: Option<i64>,
    /// A message forwarded since the last read began, or there may be more
    /// past the last read: read on.
    poked: bool,
    full: bool,
    delivering: bool,
    delivery_wait: net::Backoff,
    delivery_at: Option<Instant>,
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
    // the cursor is the item its door took last, which the core holds unless
    // it expired (a read on from it then seeks)
    let anchor = Some(kept.link.delivered).filter(|d| *d > 0);
    let mut r = Runner {
        door,
        dir,
        generation,
        kept,
        mark_seen: door_mark,
        core,
        corr: 0,
        pending: BTreeMap::new(),
        replying: vec![],
        up: false,
        stopping: None,
        core_stop: None,
        killed: false,
        core_gone: false,
        last: None,
        page: VecDeque::new(),
        reading: None,
        anchor,
        seek: None,
        // each start reads on from its cursor once: what an earlier life took
        poked: true,
        full: false,
        delivering: false,
        delivery_wait: net::Backoff::default(),
        delivery_at: None,
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
    r.ask(Cmd::Profile { name });
    r.ask(Cmd::ExpireDay);
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

/// The newest save, checked whole and made the core's; or none, and
/// nothing left of an earlier life's files (the door's save is the one
/// truth). A save that will not restore never reaches the core.
async fn restore(door: &Door, dir: &Path) -> (Kept, u64, bool) {
    let t0 = Instant::now();
    let mut wait = net::Backoff::default();
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
    match restored {
        Restore::None => {
            for db in save::DBS {
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(dir.join(format!("{db}{suffix}")));
                }
            }
            (Kept::default(), 0, false)
        }
        Restore::Save { packed, mark, behind } => {
            let state = save::stage(&packed, dir).unwrap_or_else(|e| unreadable(&e.to_string()));
            let kept: Kept = serde_json::from_slice(&state).unwrap_or_else(|e| unreadable(&e.to_string()));
            assert!(behind || kept.mark >= mark, "a save its door says is not behind holds the door's newest mark");
            save::place(dir).expect("the staged databases placed");
            ev!("door.restored", { "bytes": packed.len(), "mark": kept.mark, "doorMark": mark, "behind": behind });
            let newest = mark.max(kept.mark);
            (kept, newest, behind)
        }
    }
}

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

fn answer(status: StatusCode, body: &'static str) -> Response<Body> {
    net::respond(status, "application/json", body)
}

fn refuse(status: StatusCode, why: &str) -> Response<Body> {
    net::refusal(status, "door", why)
}

impl Runner {
    fn now(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    fn on(&mut self, m: Msg) {
        match m {
            Msg::Frame(text) => self.frame(&text),
            Msg::CoreGone(why) => {
                ev!("door.core_gone", { "why": why, "stopping": self.stopping.is_some() });
                if self.stopping.is_none() {
                    std::process::exit(1);
                }
                self.core_gone = true;
                self.pending.clear();
                self.replying.clear();
                self.reading = None;
            }
            Msg::Asked(a, answer) => {
                let _ = answer.send(self.asked(a));
            }
            Msg::Delivered { item, mark } => {
                let done = self.page.pop_front().expect("a delivery is the oldest read");
                assert_eq!(done.item, item, "deliveries end in order");
                self.delivering = false;
                self.delivery_wait.reset();
                self.mark_seen = self.mark_seen.max(mark);
                self.apply(Input::Delivered(item));
                assert!(self.kept.link.contact().is_none() || self.kept.link.delivered >= item, "the cursor moves to what was delivered");
                self.settle();
            }
            Msg::DeliveryFailed => {
                self.delivering = false;
                self.delivery_at = Some(Instant::now() + Duration::from_millis(self.delivery_wait.next_ms()));
            }
            Msg::SaveEnded { seq, mark, result } => self.save_ended(seq, mark, result),
            Msg::Stop(why) => self.begin_stop(why),
        }
    }

    fn begin_stop(&mut self, why: &'static str) {
        if self.stopping.is_none() {
            ev!("door.stopping", { "why": why });
            self.stopping = Some((why, Instant::now()));
        }
    }

    /// A command to the core, answered by its `corrId`. A send its writer
    /// has no room for is the link's to hold (as one the core refused).
    fn ask(&mut self, cmd: Cmd) {
        self.corr += 1;
        if matches!(cmd, Cmd::Sync { force: true, .. }) {
            self.forced_syncs += 1;
        }
        match self.core.writer.try_send(cmd.frame(self.corr)) {
            Ok(()) => {
                self.pending.insert(self.corr, cmd);
            }
            Err(_) => {
                ev!("door.unsent", { "corr": self.corr });
                match &cmd {
                    Cmd::React { item, on, .. } => {
                        self.kept.link.apply(Input::NotReacted { item: *item, on: *on });
                    }
                    Cmd::Send { .. } => self.apply(Input::Failed { cmd: &cmd, what: "the core's writer was full" }),
                    _ => {}
                }
            }
        }
    }

    /// The link's rules on `input`, and what they ask. A message forwarded
    /// is read on from the cursor (`read`): the forward itself only says
    /// there is one.
    fn apply(&mut self, input: Input<'_>) {
        let before = self.kept.clone();
        let actions = self.kept.link.apply(input);
        for a in actions {
            match a {
                Action::Ask(cmd) => self.ask(cmd),
                Action::Forward(_) => self.poked = true,
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

    /// The mark moves once the link is quiet (Marks).
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
        let corr = frame.corr;
        let asked = corr.and_then(|c| self.pending.remove(&c));
        // the runner's own read, never the link's (its catch-up reads too)
        if corr.is_some() && corr == self.reading {
            self.reading = None;
            match (&asked, &frame.resp) {
                (Some(cmd), Resp::Page(items)) => self.read(cmd, items),
                (_, resp) => self.unread(&asked, resp),
            }
            return self.settle();
        }
        if let Some(wire::Cmd::React { item, on, .. }) = &asked {
            self.apply(Input::Reacted { item: *item, on: *on });
        }
        match (&asked, &frame.resp) {
            (Some(Cmd::Conns { again }), Resp::Conns(conns)) => {
                let since = self.core.spawned_ms;
                self.apply(Input::Conns { conns, since, again: *again });
            }
            (Some(cmd), Resp::Error { what }) => {
                ev!("door.refused", { "what": wire::cut(what, 200) });
                self.apply(Input::Failed { cmd, what });
            }
            (Some(_), resp) => self.apply(Input::Said(resp)),
            (None, resp) => {
                self.events += 1;
                self.apply(Input::Said(resp));
            }
        }
        // a reply whose last part the core answered is done (the link holds
        // any it refused, in the same save)
        if let Some(c) = corr {
            let mut done = vec![];
            self.replying.retain_mut(|(key, corrs)| {
                corrs.remove(&c);
                let left = !corrs.is_empty();
                if !left {
                    done.push(std::mem::take(key));
                }
                left
            });
            for key in done {
                self.replied(key);
            }
        }
        self.settle();
    }

    fn replied(&mut self, key: String) {
        if self.kept.replied.len() == REPLIED_KEPT {
            self.kept.replied.pop_front();
        }
        self.kept.replied.push_back(key);
        self.due.changed(self.now());
    }

    /// A read answered. On from the anchor: its owner's messages in it, to
    /// deliver, and its newest item the next anchor. A seek's page: while
    /// it is full and all of it newer than the cursor, older ones may be
    /// past it, so the seek goes on back from its oldest; else its messages
    /// past the cursor are the oldest to deliver, and the reads go on
    /// forward from its newest (re-reading what the seek passed, in order).
    fn read(&mut self, cmd: &Cmd, items: &[wire::Item]) {
        let mut items: Vec<&wire::Item> = items.iter().collect();
        items.sort_by_key(|i| i.id);
        let delivered = self.kept.link.delivered;
        let newest = items.last().map(|i| i.id);
        if let Cmd::Chat { .. } = cmd {
            let oldest = items.first().map(|i| i.id);
            if items.len() >= usize::from(PAGE) && oldest.is_some_and(|o| o > delivered) {
                self.seek = oldest;
                self.full = true;
                return;
            }
            self.seek = None;
            self.full = newest.is_some();
        } else {
            self.full = items.len() >= usize::from(PAGE);
        }
        self.anchor = newest.or(self.anchor);
        let contact = self.kept.link.contact();
        for item in items {
            let theirs = item.received && Some(item.contact) == contact && item.id > delivered;
            // a message has its sender's id; one without is no message to carry
            if let (true, Content::Message { kind, text }, Some(id)) = (theirs, &item.content, &item.shared) {
                self.page.push_back(Inbound { id: id.clone(), kind: kind.clone(), text: text.clone(), at: item.at.clone(), item: item.id, file: item.file.clone() });
            }
        }
        assert!(self.page.len() <= usize::from(PAGE), "a page holds one read");
    }

    /// A read refused: an anchor the core no longer holds (expired, or
    /// deleted) seeks from the newest; anything else is read again later.
    fn unread(&mut self, asked: &Option<Cmd>, resp: &Resp) {
        ev!("door.read_failed", { "answer": wire::cut(&format!("{resp:?}"), 200) });
        self.poked = true;
        let gone = matches!(resp, Resp::Error { what } if what.contains("chatItemNotFound"));
        if gone && matches!(asked, Some(Cmd::ChatAfter { .. })) {
            self.anchor = None;
            self.seek = None;
        } else {
            self.delivery_at = Some(Instant::now() + Duration::from_millis(self.delivery_wait.next_ms()));
        }
    }

    fn asked(&mut self, a: Asked) -> Response<Body> {
        let mark = match &a {
            Asked::State => return self.view(),
            Asked::Link { mark } | Asked::Unlink { mark } | Asked::Reply { mark, .. } => *mark,
        };
        // every mark shown is taken, whatever the ask's end: one numbering
        // what never happened is covered by any save, and one left out
        // would leave every later save behind (one sync at each start)
        let stale = mark <= self.mark_seen;
        self.mark_seen = self.mark_seen.max(mark);
        if let Asked::Reply { key, .. } = &a {
            if self.kept.replied.contains(key) {
                self.settle();
                return answer(StatusCode::OK, r#"{"replayed":true}"#);
            }
            if self.replying.iter().any(|(k, _)| k == key) {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "that reply is still being sent: ask again");
            }
        }
        // overtaken by a later mark: acted on now, it could act after a save
        // that claims that later mark (Marks); its door asks again
        if stale {
            return refuse(StatusCode::CONFLICT, "a stale mark: ask again under a new one");
        }
        if !self.up || self.stopping.is_some() {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the door is starting or stopping");
        }
        match a {
            Asked::State => unreachable!("answered above"),
            Asked::Link { .. } => self.apply(Input::Want),
            Asked::Unlink { .. } => self.apply(Input::Unlink),
            Asked::Reply { key, text, .. } => {
                if self.kept.link.contact().is_none() {
                    return refuse(StatusCode::CONFLICT, "not linked");
                }
                // room for every part before any is taken: in the outbox
                // while the link syncs, else in the core's writer
                let parts = wire::parts(&text).len();
                let l = &self.kept.link;
                let room = if l.syncing { link::OUTBOX_MAX.saturating_sub(l.outbox.len()) } else { self.core.writer.capacity() };
                if parts > room {
                    return refuse(StatusCode::SERVICE_UNAVAILABLE, "no room for that reply now: ask again");
                }
                let first = self.corr + 1;
                self.apply(Input::Reply(&text));
                let corrs: BTreeSet<u64> = (first..=self.corr).filter(|c| self.pending.contains_key(c)).collect();
                if corrs.is_empty() {
                    self.replied(key);
                } else {
                    self.replying.push((key, corrs));
                }
                self.settle();
                return answer(StatusCode::OK, r#"{"replayed":false}"#);
            }
        }
        self.settle();
        answer(StatusCode::ACCEPTED, "{}")
    }

    fn view(&self) -> Response<Body> {
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
            waiting: self.page.len(),
            notices: &self.notices,
        };
        net::respond(StatusCode::OK, "application/json", serde_json::to_vec(&view).expect("a view is JSON"))
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
        if self.kept.link.syncing && !self.core_gone {
            let due = *self.resync_at.get_or_insert(at + Duration::from_millis(run::RESYNC_MS));
            if at >= due {
                self.resync_at = None;
                self.apply(Input::Resync);
            }
        } else {
            self.resync_at = None;
        }
        if self.recheck_at.is_some_and(|t| at >= t) && !self.core_gone {
            self.recheck_at = None;
            self.apply(Input::Recheck);
        }
        self.settle();
        if let Some((why, since)) = self.stopping {
            return self.stop(why, since);
        }
        self.deliver(at);
        if !self.saving && self.due.take(now) {
            self.save();
        }
    }

    /// The oldest message read to its door, one at a time; with none in
    /// hand, the next page read on from the cursor, when there may be one.
    fn deliver(&mut self, at: Instant) {
        // a start's catch-up first: the link moves no cursor while it reads
        if self.delivering || self.reading.is_some() || self.kept.link.scan.running || self.delivery_at.is_some_and(|t| at < t) {
            return;
        }
        let Some(contact) = self.kept.link.contact() else {
            self.page.clear();
            return;
        };
        let Some(i) = self.page.front().cloned() else {
            if self.poked || self.full {
                self.poked = false;
                self.full = false;
                let read = match self.anchor {
                    Some(after) => Cmd::ChatAfter { contact, after, count: PAGE },
                    None => Cmd::Chat { contact, count: PAGE, before: self.seek },
                };
                self.ask(read);
                if self.pending.contains_key(&self.corr) {
                    self.reading = Some(self.corr);
                } else {
                    self.poked = true;
                }
            }
            return;
        };
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

    /// A planned stop: asks refused; once the link is quiet and what is
    /// under way has ended (at most `STOP_MS_MAX`), the core is asked to
    /// end, then killed past the bridge's own bound; once it has ended, its
    /// databases still, one last save. A message the core took before it
    /// ended is in that save, read on at the next start.
    fn stop(&mut self, why: &'static str, since: Instant) {
        if self.saving {
            return;
        }
        if !self.core_gone {
            let quiet = self.pending.is_empty() && !self.delivering;
            if !quiet && since.elapsed() < Duration::from_millis(STOP_MS_MAX) {
                return;
            }
            match self.core_stop {
                None => {
                    ev!("door.core_stopping", { "why": why, "quiet": quiet });
                    self.core.signal(false);
                    self.core_stop = Some(Instant::now());
                }
                Some(t) if !self.killed && t.elapsed() > Duration::from_millis(run::STOP_MS_MAX) => {
                    self.core.signal(true);
                    self.killed = true;
                }
                Some(_) => {}
            }
            return;
        }
        // a last save that will not be taken ends it all the same: its door
        // knows what the save behind it misses (Marks)
        if since.elapsed() > Duration::from_millis(2 * STOP_MS_MAX) {
            ev!("door.stop_unsaved", { "why": why });
            std::process::exit(1);
        }
        if self.last.is_none() {
            ev!("door.last_save", { "why": why });
            self.save();
            self.last = Some(self.seq);
        }
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
            let result = match made {
                Ok(packed) => door.save(seq, mark, packed).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = to.send(Msg::SaveEnded { seq, mark, result }).await;
        });
    }

    fn save_ended(&mut self, seq: u64, mark: u64, result: Result<door::Saved, String>) {
        self.saving = false;
        match result {
            Ok(door::Saved::Taken) => {
                self.saves += 1;
                ev!("door.saved", { "seq": seq, "mark": mark });
                if let (Some((why, _)), true) = (self.stopping, self.last == Some(seq)) {
                    ev!("door.stopped", { "why": why });
                    std::process::exit(0);
                }
            }
            Ok(door::Saved::Superseded) => {
                ev!("door.superseded", { "generation": self.generation });
                std::process::exit(1);
            }
            Err(e) => {
                ev!("door.save_failed", { "seq": seq, "error": e });
                self.due.failed(self.now());
                if self.last == Some(seq) {
                    self.last = None;
                }
            }
        }
    }
}

/// The door's calls, to the loop.
async fn handle(to: mpsc::Sender<Msg>, req: Request<Incoming>) -> Response<Body> {
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
    if r.text.trim().is_empty() || r.text.len() > REPLY_TEXT_MAX_BYTES {
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

    /// Goal: a reply's body is taken only as `{key, text}` within bounds,
    /// and a text at its bound fits the body's bound however it is escaped
    /// (review of #1430: 52,891 bytes of JSON-heavy text was refused 413).
    /// Method: valid, each way to be invalid, and the worst escaping.
    #[test]
    fn a_reply_is_its_key_and_text() {
        assert_eq!(reply(&Bytes::from_static(br#"{"key":"chat:12","text":"hi"}"#)), Ok(("chat:12".into(), "hi".into())));
        for bad in [r#"{"key":"","text":"hi"}"#, r#"{"key":"a b","text":"hi"}"#, r#"{"key":"k","text":"  "}"#, r#"{"key":"k"}"#, r#"{"key":"k","text":"hi","to":"x"}"#, "nope"] {
            assert!(reply(&Bytes::from(bad)).is_err(), "{bad}");
        }
        let long = format!(r#"{{"key":"k","text":"{}"}}"#, "x".repeat(REPLY_TEXT_MAX_BYTES + 1));
        assert!(reply(&Bytes::from(long)).is_err());
        let key = format!(r#"{{"key":"{}","text":"hi"}}"#, "k".repeat(KEY_MAX_BYTES + 1));
        assert!(reply(&Bytes::from(key)).is_err());
        for worst in ["\u{1}", "\"", "\\"] {
            let text = format!("a{}", worst.repeat(REPLY_TEXT_MAX_BYTES - 1));
            let body = serde_json::to_vec(&serde_json::json!({ "key": "k".repeat(KEY_MAX_BYTES), "text": text })).unwrap();
            assert!(body.len() <= REPLY_BODY_MAX_BYTES, "{} bytes of {worst:?}", body.len());
            assert!(reply(&Bytes::from(body)).is_ok());
        }
    }
}
