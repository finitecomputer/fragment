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
//!    `POST /unlink`; `POST /reply {key, text, files?}`, sent once a key. Each ask
//!    but the state carries its mark (door.rs, "an effect"). A reply is
//!    the runner's once taken (202, `{"done": false}`): its text waits in
//!    the runner's state, in every save, until the core has taken each of
//!    its parts, and only then is its key done (200, `{"done": true}`). Its
//!    door asks again until it is. A reply is its link's: one whose link
//!    ends first (unlinked, or its contact gone) is never sent, to that
//!    contact or to the next, and its key is told so (410,
//!    `{"done": false, "ended": true}`).
//! 4. **Files** both ways, by the bridge's rules (files.rs): its owner's
//!    received and given to its door for their message's delivery; a
//!    reply's read from its door and sent after its text; the core's XFTP
//!    traffic through its door's relay.
//! 5. **Saves** follow changes (save.rs `Due`), the core never paused; a
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
//! `DOOR_API`, `DOOR_DIR`, `DOOR_CORE_BIN`, `DOOR_HTTP`, and `DOOR_SMP` and
//! `DOOR_XFTP` (a test's servers; else the core's presets).

mod core;
mod door;
mod files;
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
use fragment_bridge::simplex::{run, socks};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::door::{Delivery, Door, Restore};
use crate::files::{Fetched, Got, NotGiven, Receipt};

/// Messages to the loop waiting, at most: past it a sender waits (the
/// core's socket is read no faster than the loop takes its frames).
const MSGS_MAX: usize = 256;
/// The loop's clock: databases watched, timers and deliveries.
const TICK_MS: u64 = 250;
/// Replies' keys kept, the newest: one asked again within them is not sent
/// again (the door asks again when its answer was lost).
const REPLIED_KEPT: usize = 64;
/// Replies taken and not yet sent whole, at most: past it one is refused
/// (503, ask again), never one taken let go.
const REPLIES_MAX: usize = 32;
const KEY_MAX_BYTES: usize = 128;
/// A reply's files, at most: a message's (the bridge's `ATTACHMENTS_MAX`),
/// and what one says of a file (`door::File`) at most, framing included.
const REPLY_FILES_MAX: usize = fragment_bridge::limits::ATTACHMENTS_MAX;
const REPLY_FILE_JSON_MAX_BYTES: usize = 2 * 1024;
/// A reply's body, at most: its text at the bridge's bound with every byte
/// escaped as JSON's longest (`\u00XX`, 6 bytes), its key, its files and
/// framing.
const REPLY_BODY_MAX_BYTES: usize = 6 * REPLY_TEXT_MAX_BYTES + REPLY_FILES_MAX * REPLY_FILE_JSON_MAX_BYTES + 1024;
/// Its files' sends under way at once, at most (asked of the core, or
/// uploading): a reply's most, so each reply's files go once those before
/// them end (`run::SEND_WAIT_MS` at most).
const SENDING_MAX: usize = REPLY_FILES_MAX;
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
/// The exit code of a start of a door that saved and none of whose saves
/// restores: its door stopped itself (it starts no new profile over them).
const EXHAUSTED_EXIT: i32 = 3;

/// What each save holds beside the databases, rolled back with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    link: Link,
    /// The newest of the door's marks committed in the databases (Marks).
    #[serde(default)]
    mark: u64,
    /// The replies taken and not yet sent whole, oldest first: each its key
    /// and its text, sent one at a time while the link is in step.
    #[serde(default)]
    replies: VecDeque<Reply>,
    /// The newest replies' keys whose every part the core took
    /// (`REPLIED_KEPT`).
    #[serde(default)]
    replied: VecDeque<String>,
    /// The newest replies' keys whose link ended before they were sent
    /// whole (`REPLIED_KEPT`): never sent, to any contact.
    #[serde(default)]
    ended: VecDeque<String>,
    /// Its replies' files whose upload the core has under way, by the
    /// core's id (`SENDING_MAX`).
    #[serde(default)]
    sending: BTreeMap<wire::FileId, Upload>,
    /// The file its door holds of the message at the front, by its item,
    /// until that message is delivered: a start on a new disk, which finds
    /// the core's received copy gone, delivers it as its door holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    given: Option<(i64, door::File)>,
}

/// A reply's file the core is uploading: its copy's folder (under
/// `run::OUT_DIR`), what it went as, and the contact it is for (a note of
/// its failure goes to that contact alone).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Upload {
    folder: String,
    kind: wire::Kind,
    contact: wire::ContactId,
}

/// A reply taken, for the contact its link was bound to as it was taken:
/// sent to that contact alone. Its text first (`said` once the core took
/// each part), then its files, which its door holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Reply {
    key: String,
    text: String,
    contact: wire::ContactId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<door::File>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    said: bool,
}

pub enum Msg {
    Frame(String),
    /// The core's process ended.
    CoreExited(String),
    /// Its bot API closed (the process may live on).
    CoreLost,
    Asked(Asked, oneshot::Sender<Response<Body>>),
    Delivered { item: i64, mark: u64 },
    DeliveryFailed,
    /// The file of message `item`, given to its door, or not.
    Given { item: i64, result: Result<door::File, NotGiven> },
    /// Reply `key`'s files, read from its door.
    Fetched { key: String, result: Result<Vec<Fetched>, String> },
    SaveEnded { seq: u64, mark: u64, result: Result<door::Saved, String> },
    Stop(&'static str),
}

pub enum Asked {
    State,
    Link { mark: u64 },
    Unlink { mark: u64 },
    Reply { key: String, text: String, files: Vec<door::File>, mark: u64 },
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
    replies: usize,
    notices: &'a VecDeque<String>,
    /// While inviting, its invitation's QR code (`qr`).
    qr: Option<Qr>,
    /// It carries files both ways (its door sends a reply's only to a
    /// runner that does).
    files: bool,
    /// Its replies' files whose upload is under way.
    sending: usize,
}

/// An invitation's QR code, for its owner's page to draw (templates/simplex
/// site/setup.js): `size` modules a side, row by row in `dark`, a `1` each
/// dark one. Of the short link where there is one, at error correction M,
/// as the bridge's page draws its own (simplex/http.rs `qr_svg`); none
/// past what a QR code holds.
#[derive(Debug, Serialize)]
struct Qr {
    size: usize,
    dark: String,
}

fn qr(text: &str) -> Option<Qr> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let dark: String = code.to_colors().iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect();
    assert_eq!(dark.len(), code.width() * code.width(), "a QR code is square");
    Some(Qr { size: code.width(), dark })
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
    /// The oldest reply being sent: its sends the core has not answered,
    /// and whether one was refused (then it is sent again, whole, once the
    /// link is in step).
    sending: Option<(BTreeSet<u64>, bool)>,
    /// The file of the message at the front of `page`, on its way.
    receipt: Option<Receipt>,
    /// The files of the oldest reply, once its text is sent: being read
    /// from its door, then asked of the core (each send's corr, and its
    /// copy), the reply done once each is answered.
    outgoing: Option<Outgoing>,
    /// Copies made this life (a copy's folder is `<generation>-<n>`).
    copies: u64,
    /// When each of `Kept::sending` was asked (this life's clock, ms).
    sends: BTreeMap<wire::FileId, u64>,
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
    /// A seek's last full page, all past the cursor: the oldest, should the
    /// page before it be empty.
    held: Vec<wire::Item>,
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

/// The files of the reply being sent, once its text is.
#[derive(Debug)]
struct Outgoing {
    key: String,
    /// Asked again after a door that did not answer.
    at: Option<Instant>,
    /// Read, and asked of the core: each send's corr, its copy, what it
    /// goes as, and the file it is. None while they are read.
    asked: Option<BTreeMap<u64, (PathBuf, wire::Kind, door::File)>>,
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let generation: u64 = std::env::var("DOOR_GENERATION").ok().and_then(|g| g.parse().ok()).filter(|g| *g > 0).expect("DOOR_GENERATION: this start's, from 1");
    let dir = PathBuf::from(env("DOOR_DIR", "/data/door"));
    let cfg = run::Config { bin: env("DOOR_CORE_BIN", "/opt/door/simplex-chat").into(), dir: dir.clone(), smp: std::env::var("DOOR_SMP").ok(), xftp: std::env::var("DOOR_XFTP").ok() };
    let name = run::display_name(std::env::var("DOOR_NAME").ok().as_deref(), "agent");
    let door = Door::new(&env("DOOR_API", "http://door.fragment.internal"), generation).expect("DOOR_API: http://host[:port]");
    ev!("door.start", { "generation": generation });
    for sub in ["files", "tmp", run::OUT_DIR] {
        std::fs::create_dir_all(dir.join(sub)).expect("the door's folders");
    }

    let (to, mut rx) = mpsc::channel::<Msg>(MSGS_MAX);
    // its door's calls first, so it sees the runner come up (each answered
    // 503 until the link is up); held for the runner's life
    let (_stop_tx, stop_rx) = watch::channel(false);
    let listener = tokio::net::TcpListener::bind(env("DOOR_HTTP", "0.0.0.0:8080")).await.expect("the runner's port");
    let asks = to.clone();
    tokio::spawn(net::serve(listener, move |req, _| handle(asks.clone(), req), stop_rx));

    let (kept, door_mark, behind, life) = restore(&door, &dir).await;
    // every call of this life carries its generation
    let door = door.living(life);
    let generation = life;
    save::wal(&dir).expect("the databases in WAL mode");
    let watch = save::Watch::open(&dir).expect("the databases watched");
    // the copies a send under way does not name are no one's
    sweep_out(&dir, &kept);
    // the core's way out: SimpleX's file servers through its door's relay
    let socks = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("the core's proxy");
    let socks_port = socks.local_addr().expect("the core's proxy's port").port();
    let relay = files::Relay { door: door.clone(), open: std::sync::Arc::new(tokio::sync::Semaphore::new(files::RELAYS_MAX)) };
    tokio::spawn(socks::serve_with(socks, relay, _stop_tx.subscribe()));
    let core = match core::start(&cfg, &dir, &name, Some(socks_port), to.clone()).await {
        Ok(c) => c,
        Err(e) => {
            ev!("door.core_failed", { "why": e });
            std::process::exit(1);
        }
    };
    let mut r = Runner::new(door, dir, generation, kept, door_mark, core, watch, to.clone());
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

/// Each copy under `run::OUT_DIR` that no send under way names: let go
/// (another life's, its send ended or never asked).
fn sweep_out(dir: &Path, kept: &Kept) {
    let named: BTreeSet<&str> = kept.sending.values().map(|s| s.folder.as_str()).collect();
    let Ok(entries) = std::fs::read_dir(dir.join(run::OUT_DIR)) else { return };
    // bounded by the folder's entries: a door's replies' files, a few at once
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !named.contains(name.as_str()) {
            run::remove_copy(dir, &name);
        }
    }
}

/// The newest save, checked whole and made the core's; or none, and
/// nothing left of an earlier life's files (the door's save is the one
/// truth). A save that will not restore never reaches the core. Its door
/// gives this life its generation: the answer's last item.
async fn restore(door: &Door, dir: &Path) -> (Kept, u64, bool, u64) {
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
        Restore::None { life } => {
            for db in save::DBS {
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(dir.join(format!("{db}{suffix}")));
                }
            }
            (Kept::default(), 0, false, life)
        }
        Restore::Save { packed, mark, behind, save: id, life } => {
            let kept = match staged(&packed, dir) {
                Ok(kept) => kept,
                // this container's disk, not the save: ends, the save
                // untouched (its door starts it again)
                Err(save::SaveError::Io(why)) => {
                    ev!("door.restore_local_failed", { "save": id, "why": why });
                    std::process::exit(1);
                }
                Err(e) => {
                    let why = e.to_string();
                    door.living(life).unreadable(&id, &why).await;
                    unreadable(&why)
                }
            };
            assert!(behind || kept.mark >= mark, "a save its door says is not behind holds the door's newest mark");
            save::place(dir).expect("the staged databases placed");
            ev!("door.restored", { "save": id, "bytes": packed.len(), "mark": kept.mark, "doorMark": mark, "behind": behind, "life": life });
            let newest = mark.max(kept.mark);
            (kept, newest, behind, life)
        }
        // never a new profile over saves none of which restores
        Restore::Exhausted => {
            ev!("door.no_save_restores");
            std::process::exit(EXHAUSTED_EXIT);
        }
        Restore::Superseded => {
            ev!("door.superseded", { "at": "restore" });
            std::process::exit(1);
        }
    }
}

/// A save staged and checked whole (save.rs `stage`): its state, its
/// databases, and the contact its link is bound to among them. A failure
/// of this container's own (`SaveError::Io`) is told apart from the save's.
fn staged(packed: &[u8], dir: &Path) -> Result<Kept, save::SaveError> {
    let state = save::stage(packed, dir)?;
    let kept: Kept = serde_json::from_slice(&state).map_err(|e| save::SaveError::Corrupt(format!("its state: {e}")))?;
    if let Some(contact) = kept.link.contact() {
        if !save::staged_contact(dir, contact)? {
            return Err(save::SaveError::Corrupt(format!("its link's contact {contact} is not in its chat database")));
        }
    }
    Ok(kept)
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
    /// A start's runner: `kept` restored, `mark_seen` its door's newest.
    #[allow(clippy::too_many_arguments)]
    fn new(door: Door, dir: PathBuf, generation: u64, mut kept: Kept, mark_seen: u64, core: core::Core, watch: save::Watch, to: mpsc::Sender<Msg>) -> Runner {
        // the cursor is the item its door took last, which the core holds
        // unless it expired (a read on from it then seeks)
        let anchor = Some(kept.link.delivered).filter(|d| *d > 0);
        // the link never holds a reply's parts but the runner's own, which
        // its state holds whole (`Kept::replies`): sent again from there
        kept.link.outbox.clear();
        // each upload an earlier life had under way, waited for from now
        let sends = kept.sending.keys().map(|id| (*id, 0)).collect();
        Runner {
            door,
            dir,
            generation,
            kept,
            mark_seen,
            core,
            corr: 0,
            pending: BTreeMap::new(),
            sending: None,
            receipt: None,
            outgoing: None,
            copies: 0,
            sends,
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
            held: vec![],
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
            to,
        }
    }

    fn now(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    fn on(&mut self, m: Msg) {
        match m {
            Msg::Frame(text) => self.frame(&text),
            Msg::CoreExited(why) => {
                ev!("door.core_exited", { "why": why, "stopping": self.stopping.is_some() });
                if self.stopping.is_none() {
                    std::process::exit(1);
                }
                // what it never answered may or may not be in its databases:
                // the mark moves no more (`settle`), and a reply being sent
                // stays the runner's, sent again at the next start
                self.core_gone = true;
                self.pending.clear();
                self.sending = None;
                self.reading = None;
            }
            // its process may live on, unheard: outside a stop the runner
            // ends (the core with it); in one, its end is still waited for
            Msg::CoreLost => {
                ev!("door.core_lost", { "stopping": self.stopping.is_some() });
                if self.stopping.is_none() {
                    std::process::exit(1);
                }
            }
            Msg::Asked(a, answer) => {
                let _ = answer.send(self.asked(a));
            }
            Msg::Delivered { item, mark } => {
                let done = self.page.pop_front().expect("a delivery is the oldest read");
                assert_eq!(done.item, item, "deliveries end in order");
                // its file delivered (or its note): its copy here let go
                if let Some(r) = self.receipt.take().filter(|r| r.item == item) {
                    if let Got::Carried { path, .. } | Got::Whole { path, .. } | Got::Giving { path, .. } = &r.state {
                        let _ = std::fs::remove_file(path);
                    }
                }
                if self.kept.given.take().is_some() {
                    self.due.changed(self.now());
                }
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
            Msg::Given { item, result } => self.given(item, result),
            Msg::Fetched { key, result } => self.fetched(&key, result),
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
                    Cmd::Send { .. } => {
                        self.apply(Input::Failed { cmd: &cmd, what: "the core's writer was full" });
                        self.refused_send();
                    }
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
        self.end_replies();
        if self.kept != before {
            self.due.changed(self.now());
        }
    }

    /// The replies of a link that has ended (unlinked, its contact gone, or
    /// bound to another since) end with it: never sent, to it or to the
    /// next, each key told so if asked again; one being sent is let go of,
    /// so a late answer to it completes nothing of the next link's.
    fn end_replies(&mut self) {
        let contact = self.kept.link.contact();
        // uploads for a contact the link no longer holds: forgotten, their
        // copies let go (a late end of one tells no one)
        let gone: Vec<wire::FileId> = self.kept.sending.iter().filter(|(_, u)| Some(u.contact) != contact).map(|(id, _)| *id).collect();
        for id in gone {
            self.sends.remove(&id);
            if let Some(u) = self.kept.sending.remove(&id) {
                run::remove_copy(&self.dir, &u.folder);
            }
        }
        let ours = |r: &Reply| Some(r.contact) == contact;
        if self.kept.replies.iter().all(ours) {
            return;
        }
        if self.kept.replies.front().is_some_and(|r| !ours(r)) {
            self.sending = None;
            self.drop_outgoing();
        }
        let (kept, ended): (VecDeque<Reply>, VecDeque<Reply>) = std::mem::take(&mut self.kept.replies).into_iter().partition(ours);
        self.kept.replies = kept;
        for r in ended {
            ev!("door.reply_ended", { "key": r.key, "contact": r.contact });
            if self.kept.ended.len() == REPLIED_KEPT {
                self.kept.ended.pop_front();
            }
            self.kept.ended.push_back(r.key);
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
        // a reply waiting acts later than its mark: none may wait
        let quiet = self.up && !self.core_gone && self.pending.is_empty() && self.kept.replies.is_empty() && l.outbox.is_empty() && !l.syncing && !l.forced;
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
        if let (Some(c), Some(Cmd::SendFile { .. })) = (corr, &asked) {
            self.file_sent(c, &frame.resp);
        }
        self.file_event(&frame.resp);
        match (&asked, &frame.resp) {
            (Some(Cmd::Conns { again }), Resp::Conns(conns)) => {
                let since = self.core.spawned_ms;
                self.apply(Input::Conns { conns, since, again: *again });
            }
            (Some(cmd), Resp::Error { what }) => {
                ev!("door.refused", { "what": wire::cut(what, 200) });
                self.apply(Input::Failed { cmd, what });
                if let Cmd::Send { .. } = cmd {
                    self.refused_send();
                }
            }
            (Some(_), resp) => self.apply(Input::Said(resp)),
            (None, resp) => {
                self.events += 1;
                self.apply(Input::Said(resp));
            }
        }
        // the reply being sent, its last part answered: done, or (one was
        // refused) sent again whole once the link is in step
        if let (Some(c), Some((corrs, refused))) = (corr, self.sending.as_mut()) {
            if corrs.remove(&c) && corrs.is_empty() {
                let refused = *refused;
                self.sending = None;
                if !refused {
                    // its text said: its files next, or it is done
                    let r = self.kept.replies.front_mut().expect("a reply being sent is the oldest");
                    r.said = true;
                    if r.files.is_empty() {
                        self.reply_done();
                    }
                    self.due.changed(self.now());
                }
            }
        }
        self.settle();
    }

    /// The oldest reply sent whole, its text and its files: its key done.
    fn reply_done(&mut self) {
        let done = self.kept.replies.pop_front().expect("a reply done is the oldest");
        self.outgoing = None;
        if self.kept.replied.len() == REPLIED_KEPT {
            self.kept.replied.pop_front();
        }
        self.kept.replied.push_back(done.key);
        self.due.changed(self.now());
    }

    /// A send the core refused, or its writer had no room for: the link
    /// held the part and syncs, but the reply is the runner's (its state
    /// holds it whole), so the link holds none of it, and it is sent again.
    fn refused_send(&mut self) {
        self.kept.link.outbox.clear();
        if let Some((_, refused)) = self.sending.as_mut() {
            *refused = true;
        }
    }

    /// The oldest reply waiting, to the link, once it is in step and the
    /// core's writer has room for every part: one at a time, in order.
    fn send_reply(&mut self) {
        let l = &self.kept.link;
        if self.sending.is_some() || self.core_gone || l.syncing || l.forced || l.contact().is_none() {
            return;
        }
        let Some(r) = self.kept.replies.front().cloned() else { return };
        if r.said || r.text.trim().is_empty() {
            return self.send_files(r);
        }
        let parts = wire::parts(&r.text).len();
        if parts > self.core.writer.capacity() {
            return;
        }
        let first = self.corr + 1;
        self.apply(Input::Reply(&r.text));
        let corrs: BTreeSet<u64> = (first..=self.corr).filter(|c| self.pending.contains_key(c)).collect();
        // every part asked of the core, or the reply is sent again
        let refused = corrs.len() != parts;
        if corrs.is_empty() {
            self.refused_send();
        } else {
            self.sending = Some((corrs, refused));
        }
    }

    // ---- files (files.rs) ----

    /// The file of the message at the front, begun: one its door holds
    /// already (an earlier life gave it) carried as it is; one past
    /// SimpleX's bound here noted at once; one received whole already (a
    /// catch-up's) given to its door; else asked of the core.
    fn receive(&mut self, i: &Inbound, f: &wire::FileRef) {
        let state = if let Some((_, file)) = self.kept.given.as_ref().filter(|(item, _)| *item == i.item) {
            Got::Carried { path: self.dir.join("files").join(f.path.as_deref().unwrap_or("")), file: file.clone() }
        } else if f.size > run::FILE_BYTES_MAX {
            Got::Noted(run::file_note(&i.kind, &format!("was too large to carry here ({} MB)", f.size / 1_000_000)))
        } else if let Some(path) = &f.path {
            Got::Whole { path: self.dir.join("files").join(path), since: self.now() }
        } else {
            self.ask(Cmd::Receive { file: f.id });
            Got::Receiving { since: self.now() }
        };
        ev!("door.file_receiving", { "item": i.item, "file": f.id, "size": f.size });
        self.receipt = Some(Receipt { item: i.item, file: f.id, state });
    }

    /// A file's event at the core: its description ready (asked again), it
    /// whole (given to its door), or failed (noted); a reply's file
    /// uploaded, or not.
    fn file_event(&mut self, resp: &Resp) {
        if let Resp::FileSent { file } | Resp::FileUnsent { file } = resp {
            return self.send_ended(*file, matches!(resp, Resp::FileSent { .. }));
        }
        let kind = self.page.front().map(|i| i.kind.clone()).unwrap_or_default();
        let Some(r) = self.receipt.as_mut() else { return };
        match resp {
            Resp::FileReady { file } if *file == r.file && matches!(r.state, Got::Receiving { .. }) => self.ask(Cmd::Receive { file: *file }),
            Resp::FileDone { file, path } if *file == r.file && matches!(r.state, Got::Receiving { .. }) => r.state = Got::Whole { path: self.dir.join("files").join(path), since: self.born.elapsed().as_millis() as u64 },
            Resp::FileFailed { file } if *file == r.file && matches!(r.state, Got::Receiving { .. }) => r.state = Got::Noted(run::file_note(&kind, "did not arrive")),
            Resp::FileDone { file, .. } => ev!("door.file_unclaimed", { "file": file }),
            _ => {}
        }
    }

    /// The front message's file, given to its door, or not: one its door
    /// did not take now is given again later, until `run::FILE_WAIT_MS`
    /// after it was whole; one it refused, or still not taken then, is
    /// noted, so the messages after it go on.
    fn given(&mut self, item: i64, result: Result<door::File, NotGiven>) {
        let (kind, now) = (self.page.front().map(|i| i.kind.clone()).unwrap_or_default(), self.now());
        let Some(r) = self.receipt.as_mut().filter(|r| r.item == item) else { return };
        let Got::Giving { path, since } = &r.state else { return };
        let (path, since) = (path.clone(), *since);
        r.state = match result {
            Ok(file) => {
                // held by its door: so a later life knows, whatever its disk holds
                self.kept.given = Some((item, file.clone()));
                self.due.changed(now);
                Got::Carried { path, file }
            }
            Err(NotGiven::Later(why)) if now.saturating_sub(since) < run::FILE_WAIT_MS => {
                ev!("door.file_not_given", { "item": item, "why": why });
                self.delivery_at = Some(Instant::now() + Duration::from_millis(self.delivery_wait.next_ms()));
                Got::Whole { path, since }
            }
            Err(NotGiven::Later(why)) => {
                ev!("door.file_refused", { "item": item, "why": why });
                let _ = std::fs::remove_file(&path);
                Got::Noted(run::file_note(&kind, "did not arrive"))
            }
            Err(NotGiven::Refused(why)) => {
                ev!("door.file_refused", { "item": item, "why": why });
                let _ = std::fs::remove_file(&path);
                Got::Noted(run::file_note(&kind, "did not arrive"))
            }
        };
    }

    /// The front message's file, when it waits on it: true while it does
    /// (asked of the core, waited for, given to its door).
    fn awaiting_file(&mut self, i: &Inbound) -> bool {
        let Some(f) = &i.file else { return false };
        if self.receipt.as_ref().is_none_or(|r| r.item != i.item) {
            // never begun while a save copies the databases: one that
            // copied the core's receipt before its door held the file
            // would lose it on a new disk (`Receipt::unheld`)
            if self.saving {
                return true;
            }
            self.receive(i, f);
        }
        self.give_whole();
        !self.receipt.as_ref().is_some_and(Receipt::settled)
    }

    /// The front message's file, whole, given to its door (a planned stop
    /// gives it too, before its core ends).
    fn give_whole(&mut self) {
        let Some(kind) = self.page.front().filter(|i| self.receipt.as_ref().is_some_and(|r| r.item == i.item)).map(|i| i.kind.clone()) else { return };
        let r = self.receipt.as_mut().expect("its receipt");
        if let Got::Whole { path, since } = &r.state {
            let path = path.clone();
            r.state = Got::Giving { path: path.clone(), since: *since };
            let (door, to, item) = (self.door.clone(), self.to.clone(), r.item);
            tokio::spawn(async move {
                let result = files::give(&door, &path, &kind).await;
                let _ = to.send(Msg::Given { item, result }).await;
            });
        }
    }

    /// A file waited for past `run::FILE_WAIT_MS`: its receiving cancelled
    /// at the core (asked once: its answer settles nothing), its message
    /// delivered with its note.
    fn receive_overdue(&mut self) {
        let now = self.now();
        let kind = self.page.front().map(|i| i.kind.clone()).unwrap_or_default();
        let Some(r) = self.receipt.as_mut() else { return };
        let Got::Receiving { since } = r.state else { return };
        if now.saturating_sub(since) < run::FILE_WAIT_MS {
            return;
        }
        ev!("door.file_overdue", { "item": r.item, "file": r.file, "waitMs": run::FILE_WAIT_MS });
        r.state = Got::Noted(run::file_note(&kind, "did not arrive"));
        let file = r.file;
        self.ask(Cmd::CancelFile { file });
    }

    /// The oldest reply's files, its text said: read from its door (one
    /// read at a time, again later when its door did not answer), then
    /// each asked of the core while the link is in step and fewer than
    /// `SENDING_MAX` uploads are under way.
    fn send_files(&mut self, r: Reply) {
        if r.files.is_empty() {
            return self.reply_done();
        }
        if self.outgoing.as_ref().is_some_and(|o| o.key != r.key) {
            self.drop_outgoing();
        }
        if let Some(o) = &self.outgoing {
            // read, or being read, or asked: its answers end it
            if o.asked.is_some() || o.at.is_none_or(|t| Instant::now() < t) {
                return;
            }
        }
        if self.kept.sending.len() + r.files.len() > SENDING_MAX {
            return;
        }
        self.outgoing = Some(Outgoing { key: r.key.clone(), at: None, asked: None });
        let (door, to, out) = (self.door.clone(), self.to.clone(), self.dir.join(run::OUT_DIR));
        let (life, next) = (self.generation, self.copies);
        self.copies += r.files.len() as u64;
        tokio::spawn(async move {
            let result = files::fetch(&door, &out, life, next, &r.files).await;
            let _ = to.send(Msg::Fetched { key: r.key, result }).await;
        });
    }

    /// Reply `key`'s files read: each that goes asked of the core, a note
    /// in place of each that does not (no longer the reply's, so a restart
    /// sends neither again); none asked, the reply is done. Its door
    /// unanswering, or its link out of step meanwhile (a ratchet's sync):
    /// read again later, nothing let go but the copies.
    fn fetched(&mut self, key: &str, result: Result<Vec<Fetched>, String>) {
        let ours = self.outgoing.as_ref().is_some_and(|o| o.key == key && o.asked.is_none()) && self.kept.replies.front().is_some_and(|r| r.key == key);
        let let_go = |got: Vec<Fetched>| {
            for g in got {
                if let Fetched::Copied { path, .. } = g {
                    files::let_go(&path);
                }
            }
        };
        let got = match result {
            Ok(got) if ours => got,
            Ok(got) => return let_go(got),
            Err(e) => {
                ev!("door.files_unread", { "key": key, "error": e });
                if let Some(o) = self.outgoing.as_mut().filter(|o| o.key == key) {
                    o.at = Some(Instant::now() + Duration::from_millis(self.delivery_wait.next_ms()));
                }
                return;
            }
        };
        let l = &self.kept.link;
        let Some(contact) = l.contact().filter(|_| !l.syncing && !l.forced && !self.core_gone) else {
            // its link ended meanwhile (its reply ends with it), or is out of step: read again once it is in step
            self.outgoing = None;
            return let_go(got);
        };
        let files: Vec<door::File> = self.kept.replies.front().map(|r| r.files.clone()).unwrap_or_default();
        let mut asked = BTreeMap::new();
        for (g, file) in got.into_iter().zip(files) {
            match g {
                Fetched::Copied { path, kind } => {
                    self.ask(Cmd::SendFile { contact, path: path.to_string_lossy().into_owned(), kind });
                    match self.pending.get(&self.corr) {
                        Some(Cmd::SendFile { .. }) => {
                            asked.insert(self.corr, (path, kind, file));
                        }
                        _ => self.kept_file(&path, kind.words(), &file),
                    }
                }
                Fetched::Kept(words) => self.kept_file(Path::new(""), words, &file),
            }
        }
        if asked.is_empty() {
            return self.reply_done();
        }
        if let Some(o) = self.outgoing.as_mut() {
            o.asked = Some(asked);
        }
    }

    /// Reply's file `file` taken by the core, or one that does not go: no
    /// longer its reply's to send (the entry itself: two files of the same
    /// bytes may go under two names).
    fn file_done(&mut self, file: &door::File) {
        if let Some(r) = self.kept.replies.front_mut() {
            if let Some(i) = r.files.iter().position(|f| f == file) {
                r.files.remove(i);
                self.due.changed(self.born.elapsed().as_millis() as u64);
            }
        }
    }

    /// A reply's file that does not go: its copy let go, its note sent.
    fn kept_file(&mut self, path: &Path, words: &str, file: &door::File) {
        files::let_go(path);
        self.file_done(file);
        self.apply(Input::Reply(&run::kept_note(words)));
    }

    /// The core's answer to a reply's file's send (`corr`): the file it
    /// made of it, watched until its upload ends; or, refused, its note.
    /// Its reply is done once each is answered.
    fn file_sent(&mut self, corr: u64, resp: &Resp) {
        let Some((path, kind, file)) = self.outgoing.as_mut().and_then(|o| o.asked.as_mut()).and_then(|a| a.remove(&corr)) else { return };
        let id = match resp {
            Resp::Items(items) => items.iter().find(|i| !i.received).and_then(|i| i.file.as_ref()).map(|f| f.id),
            _ => None,
        };
        let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let contact = self.kept.replies.front().map(|r| r.contact);
        match (id, folder, contact) {
            (Some(id), Some(folder), Some(contact)) => {
                ev!("door.file_sending", { "file": id, "kind": kind.words() });
                self.kept.sending.insert(id, Upload { folder, kind, contact });
                self.sends.insert(id, self.now());
                self.file_done(&file);
            }
            _ => {
                ev!("door.file_unsendable", { "answer": wire::cut(&format!("{resp:?}"), 200) });
                self.kept_file(&path, kind.words(), &file);
            }
        }
        if self.outgoing.as_ref().and_then(|o| o.asked.as_ref()).is_some_and(BTreeMap::is_empty) {
            self.reply_done();
        }
    }

    /// An upload under way, ended: sent (`ok`), failed, or given up on.
    /// Its copy let go and it is forgotten; one that did not arrive is
    /// said so. One it does not name (ended already) is nothing.
    fn send_ended(&mut self, id: wire::FileId, ok: bool) {
        self.sends.remove(&id);
        let Some(upload) = self.kept.sending.remove(&id) else { return };
        run::remove_copy(&self.dir, &upload.folder);
        self.due.changed(self.now());
        ev!("door.file_sent", { "file": id, "ok": ok });
        if !ok && self.kept.link.contact() == Some(upload.contact) {
            self.apply(Input::Reply(&run::unsent_note(upload.kind.words())));
        }
    }

    /// Each upload under way past `run::SEND_WAIT_MS`: cancelled at the
    /// core (asked once: its answer settles nothing), its note sent.
    fn sends_overdue(&mut self) {
        let now = self.now();
        let overdue: Vec<wire::FileId> = self.sends.iter().filter(|(_, since)| now.saturating_sub(**since) >= run::SEND_WAIT_MS).map(|(id, _)| *id).collect();
        for id in overdue {
            ev!("door.send_overdue", { "file": id, "waitMs": run::SEND_WAIT_MS });
            self.ask(Cmd::CancelSend { file: id });
            self.send_ended(id, false);
        }
    }

    /// The files of a reply let go of (its link ended): their copies too.
    fn drop_outgoing(&mut self) {
        if let Some(asked) = self.outgoing.take().and_then(|o| o.asked) {
            for (path, _, _) in asked.values() {
                files::let_go(path);
            }
        }
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
        let mut held: Vec<wire::Item> = vec![];
        if let Cmd::Chat { .. } = cmd {
            let oldest = items.first().map(|i| i.id);
            if items.len() >= usize::from(PAGE) && oldest.is_some_and(|o| o > delivered) {
                self.held = items.into_iter().cloned().collect();
                self.seek = oldest;
                self.full = true;
                return;
            }
            self.seek = None;
            // none older than the last full page: that page is the oldest
            held = std::mem::take(&mut self.held);
            if items.is_empty() {
                items = held.iter().collect();
            }
        } else {
            self.full = items.len() >= usize::from(PAGE);
        }
        let newest = items.last().map(|i| i.id).or(newest);
        if let Cmd::Chat { .. } = cmd {
            // on forward from the oldest page, re-reading what the seek passed
            self.full = newest.is_some();
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
        if gone && matches!(asked, Some(Cmd::ChatAfter { .. } | Cmd::Chat { .. })) {
            self.anchor = None;
            self.seek = None;
            self.held.clear();
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
            if self.kept.ended.contains(key) {
                self.settle();
                return answer(StatusCode::GONE, r#"{"done":false,"ended":true}"#);
            }
            if self.kept.replied.contains(key) {
                self.settle();
                return answer(StatusCode::OK, r#"{"done":true}"#);
            }
            if self.kept.replies.iter().any(|r| r.key == *key) {
                return answer(StatusCode::ACCEPTED, r#"{"done":false}"#);
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
            Asked::Reply { key, text, files, .. } => {
                let Some(contact) = self.kept.link.contact() else {
                    return refuse(StatusCode::CONFLICT, "not linked");
                };
                if self.kept.replies.len() >= REPLIES_MAX {
                    return refuse(StatusCode::SERVICE_UNAVAILABLE, "replies are waiting: ask again");
                }
                self.kept.replies.push_back(Reply { key, text, contact, files, said: false });
                self.due.changed(self.now());
                self.send_reply();
                return answer(StatusCode::ACCEPTED, r#"{"done":false}"#);
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
            replies: self.kept.replies.len(),
            notices: &self.notices,
            qr: short.or(link).and_then(qr),
            files: true,
            sending: self.kept.sending.len(),
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
        self.receive_overdue();
        self.sends_overdue();
        self.settle();
        if let Some((why, since)) = self.stopping {
            self.give_whole();
            return self.stop(why, since);
        }
        self.send_reply();
        self.deliver(at);
        // no save while its core may hold a file its door does not (files.rs `unheld`)
        if !self.saving && !self.receipt.as_ref().is_some_and(Receipt::unheld) && self.due.take(now) {
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
        if self.awaiting_file(&i) {
            return;
        }
        let (file, note) = match self.receipt.as_ref().filter(|r| r.item == i.item).map(|r| &r.state) {
            Some(Got::Carried { file, .. }) => (Some(file.clone()), None),
            Some(Got::Noted(note)) => (None, Some(note.clone())),
            _ => (None, None),
        };
        self.delivering = true;
        self.delivery_at = None;
        let (door, to) = (self.door.clone(), self.to.clone());
        tokio::spawn(async move {
            let msg = match door.deliver(&Delivery::of(&i, file.as_ref(), note.as_deref())).await {
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
            let quiet = self.pending.is_empty() && !self.delivering && !self.receipt.as_ref().is_some_and(Receipt::unheld);
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
                Ok((key, text, files)) => Asked::Reply { key, text, files, mark },
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

/// A reply's body: `{key, text, files?}`, a key of letters, digits, `-`,
/// `_`, `:` and `.`, and something to say or a file to send (at most
/// `REPLY_FILES_MAX`, each named by its SHA-256, which its door holds).
fn reply(body: &Bytes) -> Result<(String, String, Vec<door::File>), &'static str> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        key: String,
        text: String,
        #[serde(default)]
        files: Vec<door::File>,
    }
    let r: Reply = serde_json::from_slice(body).map_err(|_| "a reply is {key, text, files?}")?;
    let key_ok = !r.key.is_empty() && r.key.len() <= KEY_MAX_BYTES && r.key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'));
    if !key_ok {
        return Err("a reply's key is 1-128 of letters, digits, -, _, : and .");
    }
    if (r.text.trim().is_empty() && r.files.is_empty()) || r.text.len() > REPLY_TEXT_MAX_BYTES {
        return Err("a reply says something, or sends a file, within its bound");
    }
    let file_ok = |f: &door::File| f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && f.name.len() <= 255 && f.media_type.len() <= 128;
    if r.files.len() > REPLY_FILES_MAX || !r.files.iter().all(file_ok) {
        return Err("a reply's files are at most a message's, each {sha256, name, type, size}");
    }
    Ok((r.key, r.text, r.files))
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

    /// Goal: while inviting, its state carries the QR code of its short
    /// link (else its link) whole, a square of modules; none otherwise, or
    /// past what a QR code holds. Method: the view of a scripted runner as
    /// its link's phase moves, and `qr` at its bound.
    #[tokio::test]
    async fn an_invitations_state_carries_its_qr_code() {
        async fn read(r: &Runner) -> serde_json::Value {
            serde_json::from_slice(&r.view().into_body().collect().await.unwrap().to_bytes()).unwrap()
        }
        let (mut r, _writes) = scripted("qr", false);
        assert!(read(&r).await["qr"].is_null(), "off: none");
        let short = "https://smp.example/i#short-invitation";
        for (link, short, drawn) in [("simplex:/invitation#/?v=2-7&smp=long", Some(short), short), ("simplex:/invitation#/?v=2-7&smp=long", None, "simplex:/invitation#/?v=2-7&smp=long")] {
            r.kept.link.phase = Phase::Inviting { conn: 7, link: link.into(), short: short.map(Into::into) };
            let v = read(&r).await;
            let code = qrcode::QrCode::with_error_correction_level(drawn.as_bytes(), qrcode::EcLevel::M).unwrap();
            assert_eq!(v["qr"]["size"], code.width(), "{drawn}");
            let dark = v["qr"]["dark"].as_str().unwrap();
            assert_eq!(dark.len(), code.width() * code.width());
            assert!(dark.bytes().all(|b| b == b'0' || b == b'1') && dark.contains('1') && dark.contains('0'));
            assert_eq!(dark, qr(drawn).unwrap().dark);
        }
        r.kept.link.phase = Phase::Linked { contact: 3, name: "paul".into() };
        assert!(read(&r).await["qr"].is_null(), "linked: none");
        let full = format!("simplex:/invitation#/?v=2-7&smp={}", "x".repeat(700));
        assert!(qr(&full).is_some(), "a full invitation fits");
        assert!(qr(&"x".repeat(5_000)).is_none(), "past what a QR code holds");
    }

    /// A runner on a core of the test's own (`Core::scripted`): what it
    /// writes to the core comes out of the channel returned, and the test
    /// answers as the core would; its door is nowhere (its calls fail).
    fn scripted(name: &str, linked: bool) -> (Runner, mpsc::Receiver<String>) {
        let dir = std::env::temp_dir().join(format!("door-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        save::wal(&dir).unwrap();
        let watch = save::Watch::open(&dir).unwrap();
        let (writer, writes) = mpsc::channel(64);
        let (to, msgs) = mpsc::channel(MSGS_MAX);
        std::mem::forget(msgs);
        let mut kept = Kept::default();
        if linked {
            kept.link.phase = Phase::Linked { contact: 3, name: "paul".into() };
        }
        let mut r = Runner::new(Door::new("http://127.0.0.1:9", 1).unwrap(), dir, 1, kept, 0, core::Core::scripted(writer), watch, to);
        r.up = true;
        (r, writes)
    }

    /// What the runner asked of the core since last read: each corr, and
    /// its command line.
    fn asked_of(writes: &mut mpsc::Receiver<String>) -> Vec<(u64, String)> {
        let mut out = vec![];
        while let Ok(f) = writes.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&f).unwrap();
            out.push((v["corrId"].as_str().unwrap().parse().unwrap(), v["cmd"].as_str().unwrap().to_string()));
        }
        out
    }

    fn item(id: i64, received: bool, text: &str) -> serde_json::Value {
        let (dir, content) = if received { ("directRcv", "rcvMsgContent") } else { ("directSnd", "sndMsgContent") };
        serde_json::json!({ "chatDir": { "type": dir }, "content": { "type": content, "msgContent": { "type": "text", "text": text } }, "meta": { "itemId": id, "itemSharedMsgId": format!("s{id}"), "itemTs": "2026-11-20T00:00:00Z" } })
    }

    fn sent_ok(corr: u64) -> String {
        let info = serde_json::json!({ "type": "direct", "contact": { "contactId": 3 } });
        serde_json::json!({ "corrId": corr.to_string(), "resp": { "type": "newChatItems", "chatItems": [{ "chatInfo": info, "chatItem": item(900 + corr as i64, false, "x") }] } }).to_string()
    }

    fn refused(corr: u64, what: &str) -> String {
        serde_json::json!({ "corrId": corr.to_string(), "resp": { "type": "chatCmdError", "chatError": { "type": "errorAgent", "agentError": { "type": what } } } }).to_string()
    }

    fn page_of(corr: u64, ids: &[i64]) -> String {
        let items: Vec<serde_json::Value> = ids.iter().map(|id| item(*id, true, &format!("m{id}"))).collect();
        serde_json::json!({ "corrId": corr.to_string(), "resp": { "type": "apiChat", "chat": { "chatInfo": { "type": "direct", "contact": { "contactId": 3 } }, "chatItems": items } } }).to_string()
    }

    async fn reply_ask(r: &mut Runner, key: &str, mark: u64) -> (u16, String) {
        let res = r.asked(Asked::Reply { key: key.into(), text: format!("text of {key}"), files: vec![], mark });
        let status = res.status().as_u16();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// Goal: a reply once taken is never lost (the review of #1430): taken
    /// while the link syncs, it waits in the runner's state, its text with
    /// it, in every save, and nothing of it in the link's outbox; it is
    /// done only once the core has taken every part; one the core refuses
    /// is sent again whole, never evicted, however many wait (the review's
    /// 33 refused sends against a 32-part outbox); past its bound a reply
    /// is refused before it is taken. Method: a scripted core, asks and
    /// answers in turn, the state read back as a save holds it.
    #[tokio::test]
    async fn a_reply_taken_is_never_lost() {
        let (mut r, mut writes) = scripted("replies", true);
        r.kept.link.syncing = true;
        for n in 0..REPLIES_MAX as u64 {
            assert_eq!(reply_ask(&mut r, &format!("k{n}"), n + 1).await, (202, r#"{"done":false}"#.into()));
        }
        assert_eq!(reply_ask(&mut r, "k-past", 100).await.0, 503, "past its bound, refused before it is taken");
        assert!(asked_of(&mut writes).is_empty(), "nothing is sent while the link syncs");
        let saved: Kept = serde_json::from_slice(&serde_json::to_vec(&r.kept).unwrap()).unwrap();
        assert_eq!(saved.replies.len(), REPLIES_MAX);
        assert_eq!(saved.replies[0], Reply { key: "k0".into(), text: "text of k0".into(), contact: 3, files: vec![], said: false }, "a save holds each waiting reply whole, for its link's contact");
        assert!(saved.link.outbox.is_empty());

        // in step: the oldest goes, and is done only once the core took it
        r.kept.link.syncing = false;
        r.send_reply();
        let sends = asked_of(&mut writes);
        assert_eq!(sends.len(), 1, "one reply at a time: {sends:?}");
        assert_eq!(reply_ask(&mut r, "k0", 101).await, (202, r#"{"done":false}"#.into()), "asked again while it is sent");
        r.frame(&sent_ok(sends[0].0));
        assert_eq!(reply_ask(&mut r, "k0", 102).await, (200, r#"{"done":true}"#.into()));

        // refused by the core: the link syncs, holds none of it, and the
        // reply waits, whole, to go again; every one of the 31 others too
        for _ in 0..40 {
            r.kept.link.syncing = false;
            r.send_reply();
            let sends = asked_of(&mut writes);
            assert_eq!(sends.len(), 1);
            r.frame(&refused(sends[0].0, "CMD"));
            assert!(r.kept.link.outbox.is_empty(), "the link holds no part of a reply");
            assert_eq!(r.kept.replies.len(), REPLIES_MAX - 1, "a refused reply is never let go");
            assert_eq!(r.kept.replies[0].key, "k1");
        }
        let saved: Kept = serde_json::from_slice(&serde_json::to_vec(&r.kept).unwrap()).unwrap();
        assert_eq!(saved.replies.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), (1..REPLIES_MAX).map(|n| format!("k{n}")).collect::<Vec<_>>());
    }

    /// Goal: a reply taken for one link is never sent to another (the
    /// third review of #1430: one waiting while contact 3 synced went to
    /// contact 4 after an unlink and a new link), whichever side ended the
    /// link, through a save; asked again, it is told its link ended; and a
    /// late answer to one being sent as its link ended completes nothing of
    /// the next link's. Method: a scripted core, the link unlinked by its
    /// owner or deleted by its contact, then linked to contact 4.
    #[tokio::test]
    async fn a_reply_never_outlives_its_link() {
        for by_them in [false, true] {
            let (mut r, mut writes) = scripted(&format!("ended-{by_them}"), true);
            r.kept.link.syncing = true;
            assert_eq!(reply_ask(&mut r, "old", 1).await.0, 202);
            r.kept = serde_json::from_slice(&serde_json::to_vec(&r.kept).unwrap()).unwrap();
            if by_them {
                r.frame(&serde_json::json!({ "resp": { "type": "contactDeletedByContact", "contact": { "contactId": 3 } } }).to_string());
            } else {
                let _ = r.asked(Asked::Unlink { mark: 2 });
            }
            r.kept.link.phase = Phase::Linked { contact: 4, name: "pat".into() };
            r.kept.link.syncing = false;
            r.kept.link.forced = false;
            r.send_reply();
            let sends = asked_of(&mut writes);
            assert!(!sends.iter().any(|(_, c)| c.contains("text of old")), "never to the next contact (ended by them: {by_them}): {sends:?}");
            assert_eq!(reply_ask(&mut r, "old", 3).await, (410, r#"{"done":false,"ended":true}"#.into()), "asked again: its link ended");
        }
        // a late answer to one being sent as its link ended
        let (mut r, mut writes) = scripted("ended-late", true);
        assert_eq!(reply_ask(&mut r, "first", 1).await.0, 202);
        let first = asked_of(&mut writes);
        assert_eq!(first.len(), 1, "sent at once, in step: {first:?}");
        let _ = r.asked(Asked::Unlink { mark: 2 });
        r.kept.link.phase = Phase::Linked { contact: 4, name: "pat".into() };
        assert_eq!(reply_ask(&mut r, "second", 3).await.0, 202);
        let second: Vec<(u64, String)> = asked_of(&mut writes).into_iter().filter(|(_, c)| c.contains("text of second")).collect();
        assert_eq!(second.len(), 1, "the next link's reply, sent to it: {second:?}");
        assert!(second[0].1.contains("@4"), "{second:?}");
        r.frame(&sent_ok(first[0].0));
        assert_eq!(reply_ask(&mut r, "second", 4).await, (202, r#"{"done":false}"#.into()), "a late answer completes nothing of the next link's");
        r.frame(&sent_ok(second[0].0));
        assert_eq!(reply_ask(&mut r, "second", 5).await, (200, r#"{"done":true}"#.into()));
    }

    /// Goal: a backlog is read whole however it lies (the review of #1430:
    /// with the cursor at 0 and exactly one page waiting, the seek read past
    /// the oldest page, found nothing, and stranded it), and a seek whose
    /// anchor is gone begins again from the newest. Method: a scripted core
    /// answering each read.
    #[tokio::test]
    async fn a_backlog_of_one_page_is_read() {
        let (mut r, mut writes) = scripted("seek", true);
        r.deliver(Instant::now());
        let (c, line) = asked_of(&mut writes).pop().unwrap();
        assert_eq!(line, format!("/_get chat @3 count={PAGE}"), "no anchor: the newest page first");
        r.frame(&page_of(c, &(1..=i64::from(PAGE)).collect::<Vec<_>>()));
        r.deliver(Instant::now());
        let (c, line) = asked_of(&mut writes).pop().unwrap();
        assert_eq!(line, format!("/_get chat @3 before=1 count={PAGE}"), "a full page past the cursor: older ones may be there");
        r.frame(&page_of(c, &[]));
        assert_eq!(r.page.iter().map(|i| i.item).collect::<Vec<_>>(), (1..=i64::from(PAGE)).collect::<Vec<_>>(), "none older: the last full page is the oldest");
        assert_eq!(r.anchor, Some(i64::from(PAGE)));

        // a seek's anchor gone (expired): from the newest again
        let (mut r, mut writes) = scripted("seek-gone", true);
        r.deliver(Instant::now());
        let (c, _) = asked_of(&mut writes).pop().unwrap();
        r.frame(&page_of(c, &(41..=40 + i64::from(PAGE)).collect::<Vec<_>>()));
        r.deliver(Instant::now());
        let (c, line) = asked_of(&mut writes).pop().unwrap();
        assert!(line.contains("before=41"), "{line}");
        r.frame(&refused(c, "chatItemNotFound").replace("errorAgent", "errorStore").replace("agentError", "storeError"));
        assert_eq!((r.anchor, r.seek, r.held.len(), r.poked), (None, None, 0, true), "begins again");
    }

    /// Goal: a planned stop saves only once the core's process has ended
    /// (the review of #1430: its socket closing was taken for its end),
    /// and the mark moves no more once the core is gone (what it never
    /// answered may or may not be in its databases). Method: a stop, the
    /// socket lost, then the process's end.
    #[tokio::test]
    async fn a_stop_saves_once_the_core_has_ended() {
        let (mut r, _writes) = scripted("stop", true);
        r.begin_stop("sigterm");
        let since = Instant::now();
        r.stop("sigterm", since);
        assert!(r.core_stop.is_some(), "the core is asked to end");
        r.on(Msg::CoreLost);
        r.stop("sigterm", since);
        assert!(!r.saving, "a socket's close is not the core's end: no save yet");
        r.mark_seen = 5;
        r.on(Msg::CoreExited("signal 15".into()));
        r.settle();
        assert_eq!(r.kept.mark, 0, "the mark moves no more once the core is gone");
        r.stop("sigterm", since);
        assert!(r.saving && r.last == Some(r.seq), "its last save, once it has ended");
    }

    /// Goal: a reply's body is taken only as `{key, text}` within bounds,
    /// and a text at its bound fits the body's bound however it is escaped
    /// (review of #1430: 52,891 bytes of JSON-heavy text was refused 413).
    /// Method: valid, each way to be invalid, and the worst escaping.
    #[test]
    fn a_reply_is_its_key_and_text() {
        assert_eq!(reply(&Bytes::from_static(br#"{"key":"chat:12","text":"hi"}"#)), Ok(("chat:12".into(), "hi".into(), vec![])));
        let sha = "a".repeat(64);
        let files = format!(r#"{{"key":"k","text":"","files":[{{"sha256":"{sha}","name":"a.mp3","type":"audio/mpeg","size":9}}]}}"#);
        assert_eq!(reply(&Bytes::from(files)).map(|r| r.2.len()), Ok(1), "a file alone, no words");
        let nine = format!(r#"{{"key":"k","text":"hi","files":[{}]}}"#, vec![format!(r#"{{"sha256":"{sha}","name":"a","type":"x/y","size":1}}"#); REPLY_FILES_MAX + 1].join(","));
        assert!(reply(&Bytes::from(nine)).is_err(), "past a message's files");
        assert!(reply(&Bytes::from(r#"{"key":"k","text":"hi","files":[{"sha256":"nope","name":"a","type":"x/y","size":1}]}"#)).is_err(), "a file named by no SHA-256");
        assert!(reply(&Bytes::from(r#"{"key":"k","text":"","files":[]}"#)).is_err(), "nothing to say or send");
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

    /// A message of the person's that carries file `file` (`kind`, `size`
    /// bytes; received whole already at `path`, when it is).
    fn carrying(item: i64, kind: &str, file: wire::FileId, size: u64, path: Option<&str>) -> Inbound {
        Inbound { id: format!("s{item}"), kind: kind.into(), text: String::new(), at: "2026-11-24T00:00:00Z".into(), item, file: Some(wire::FileRef { id: file, name: "memo.m4a".into(), size, path: path.map(Into::into) }) }
    }

    fn worded(item: i64, text: &str) -> Inbound {
        Inbound { id: format!("s{item}"), kind: "text".into(), text: text.into(), at: "2026-11-24T00:00:00Z".into(), item, file: None }
    }

    fn said_of(r: &mut Runner, resp: serde_json::Value) {
        r.frame(&serde_json::json!({ "resp": resp }).to_string());
    }

    fn rcv_file(kind: &str, file: wire::FileId, path: &str) -> serde_json::Value {
        serde_json::json!({ "type": kind, "chatItem": { "chatItem": { "file": { "fileId": file, "fileSource": { "filePath": path } } } } })
    }

    fn snd_file(kind: &str, file: wire::FileId) -> serde_json::Value {
        serde_json::json!({ "type": kind, "fileTransferMeta": { "fileId": file, "fileName": "notes.bin", "filePath": "/x", "fileSize": 9, "chunkSize": 9, "cancelled": false } })
    }

    /// The core's answer to a file's send: the item it made, its file `file`.
    fn sent_file(corr: u64, file: wire::FileId) -> String {
        let info = serde_json::json!({ "type": "direct", "contact": { "contactId": 3 } });
        let mut it = item(950 + corr as i64, false, "");
        it["file"] = serde_json::json!({ "fileId": file, "fileName": "notes.bin", "fileSize": 9, "fileStatus": { "type": "sndStored" } });
        serde_json::json!({ "corrId": corr.to_string(), "resp": { "type": "newChatItems", "chatItems": [{ "chatInfo": info, "chatItem": it }] } }).to_string()
    }

    fn file(sha: char) -> door::File {
        door::File { sha256: sha.to_string().repeat(64), name: "notes.bin".into(), media_type: "application/octet-stream".into(), size: 9 }
    }

    /// Goal (S1): its owner's file is received, given to its door, and its
    /// message delivered naming it, in order: the word after it waits; its
    /// description ready asks again; a file past SimpleX's bound here, one
    /// that fails, one its door refuses and one waited for past
    /// `FILE_WAIT_MS` (its receiving cancelled) each go as a note in their
    /// message's text; one its door did not take now is given again; one
    /// received whole before (a catch-up's) is given at once. Method: a
    /// scripted core, its events in turn, the door's answers handed in.
    #[tokio::test]
    async fn an_owners_file_is_received_given_and_delivered_in_order() {
        let (mut r, mut writes) = scripted("receive", true);
        r.page = VecDeque::from([carrying(10, "voice", 5, 300_000, None), worded(11, "after the memo")]);
        r.deliver(Instant::now());
        let asked = asked_of(&mut writes);
        assert_eq!(asked.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/freceive 5"]);
        assert!(!r.delivering, "its message waits for its file");
        said_of(&mut r, rcv_file("rcvFileDescrReady", 5, ""));
        assert_eq!(asked_of(&mut writes).iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/freceive 5"], "its description ready: asked again");
        said_of(&mut r, rcv_file("rcvFileComplete", 9, "other.m4a"));
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Receiving { .. }), "another file's end is not its");
        said_of(&mut r, rcv_file("rcvFileComplete", 5, "memo.m4a"));
        r.deliver(Instant::now());
        assert!(matches!(&r.receipt.as_ref().unwrap().state, Got::Giving { path, .. } if path.ends_with("files/memo.m4a")), "{:?}", r.receipt);
        assert!(!r.delivering);
        // its door did not take it now: given again later, never noted
        r.given(10, Err(NotGiven::Later("503: full".into())));
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Whole { .. }) && r.delivery_at.is_some());
        r.delivery_at = None;
        r.deliver(Instant::now());
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Giving { .. }));
        let f = door::File { sha256: "a".repeat(64), name: "memo.m4a".into(), media_type: "audio/mp4".into(), size: 300_000 };
        r.given(10, Ok(f.clone()));
        r.deliver(Instant::now());
        assert!(r.delivering, "its file given: its message goes");
        let front = r.page.front().unwrap().clone();
        let d = serde_json::to_value(Delivery::of(&front, Some(&f), None)).unwrap();
        assert_eq!((d["file"]["sha256"].as_str(), d["file"]["type"].as_str(), d["text"].as_str()), (Some("a".repeat(64).as_str()), Some("audio/mp4"), Some("")));
        r.on(Msg::Delivered { item: 10, mark: 1 });
        assert!(r.receipt.is_none());
        assert_eq!(r.page.front().map(|i| i.item), Some(11), "the word after it, next");

        // too large: its note, nothing asked
        let (mut r, mut writes) = scripted("receive-large", true);
        r.page = VecDeque::from([carrying(20, "image", 6, run::FILE_BYTES_MAX + 1, None)]);
        r.deliver(Instant::now());
        assert!(asked_of(&mut writes).is_empty() && r.delivering);
        assert!(matches!(&r.receipt.as_ref().unwrap().state, Got::Noted(n) if n.contains("a photo") && n.contains("too large")));
        let front = r.page.front().unwrap().clone();
        let note = match &r.receipt.as_ref().unwrap().state { Got::Noted(n) => n.clone(), _ => unreachable!() };
        assert_eq!(serde_json::to_value(Delivery::of(&front, None, Some(&note))).unwrap()["text"], note.as_str(), "its message's text is its note");

        // failed, refused, and late
        for how in ["failed", "refused", "late"] {
            let (mut r, mut writes) = scripted(&format!("receive-{how}"), true);
            r.page = VecDeque::from([carrying(30, "voice", 7, 1_000, None)]);
            r.deliver(Instant::now());
            asked_of(&mut writes);
            match how {
                "failed" => said_of(&mut r, rcv_file("rcvFileError", 7, "")),
                "refused" => {
                    said_of(&mut r, rcv_file("rcvFileComplete", 7, "memo.m4a"));
                    r.deliver(Instant::now());
                    r.given(30, Err(NotGiven::Refused("400".into())));
                }
                _ => {
                    r.born -= Duration::from_millis(run::FILE_WAIT_MS + 1);
                    r.tick();
                    assert_eq!(asked_of(&mut writes).iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/fcancel 7"], "its receiving cancelled");
                }
            }
            assert!(matches!(&r.receipt.as_ref().unwrap().state, Got::Noted(n) if n.contains("a voice message") && n.contains("did not arrive")), "{how}: {:?}", r.receipt);
            r.deliver(Instant::now());
            assert!(r.delivering, "{how}: delivered with its note");
        }

        // received whole before (a start's catch-up): given at once
        let (mut r, mut writes) = scripted("receive-whole", true);
        r.page = VecDeque::from([carrying(40, "voice", 8, 1_000, Some("memo.m4a"))]);
        r.deliver(Instant::now());
        assert!(asked_of(&mut writes).is_empty(), "nothing asked of the core");
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Giving { .. }));
    }

    /// Goal (S1): a reply's files go after its text, the reply done only
    /// once the core took each; one that cannot go is said so in its place;
    /// an upload that fails, or passes `SEND_WAIT_MS` (cancelled), is said
    /// so and its copy let go; a reply's text said survives a restart (its
    /// files sent then, its text never again); a reply with files and no
    /// words is taken. Method: a scripted core, the door's reads handed in.
    #[tokio::test]
    async fn a_replys_files_go_after_its_text() {
        let (mut r, mut writes) = scripted("files-out", true);
        let res = r.asked(Asked::Reply { key: "chat:1".into(), text: "here".into(), files: vec![file('a'), file('b')], mark: 1 });
        assert_eq!(res.status(), 202);
        let text = asked_of(&mut writes);
        assert_eq!(text.len(), 1, "its text first: {text:?}");
        r.frame(&sent_ok(text[0].0));
        assert!(r.kept.replies.front().is_some_and(|x| x.said), "its text said, its files next");
        assert_eq!(reply_ask(&mut r, "chat:1", 2).await.0, 202, "not done before its files");
        // its files read: one copied, one that does not go (its note)
        let out = r.dir.join(run::OUT_DIR).join("1-0");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("notes.bin"), b"123456789").unwrap();
        r.outgoing = Some(Outgoing { key: "chat:1".into(), at: None, asked: None });
        r.fetched("chat:1", Ok(vec![Fetched::Copied { path: out.join("notes.bin"), kind: wire::Kind::File }, Fetched::Kept("an image")]));
        let asked = asked_of(&mut writes);
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(asked[0].1.starts_with("/_send @3 json") && asked[0].1.contains("notes.bin"), "{asked:?}");
        assert!(asked[1].1.contains("(I sent an image, in our chat.)"), "{asked:?}");
        assert_eq!(reply_ask(&mut r, "chat:1", 3).await.0, 202, "not done before the core took its file");
        r.frame(&sent_file(asked[0].0, 44));
        assert_eq!(reply_ask(&mut r, "chat:1", 4).await, (200, r#"{"done":true}"#.into()), "done once the core took each");
        assert_eq!(r.kept.sending.keys().copied().collect::<Vec<_>>(), [44], "its upload watched");
        // its upload failed: said so, its copy let go
        said_of(&mut r, snd_file("sndFileError", 44));
        assert!(r.kept.sending.is_empty() && !out.exists(), "forgotten, its copy let go");
        assert!(asked_of(&mut writes).iter().any(|(_, l)| l.contains("A file I sent didn't arrive here")), "said so");

        // an upload past SEND_WAIT_MS: cancelled, said so
        let (mut r, mut writes) = scripted("files-late", true);
        r.kept.sending.insert(45, Upload { folder: "1-9".into(), kind: wire::Kind::Image, contact: 3 });
        r.sends.insert(45, 0);
        r.born -= Duration::from_millis(run::SEND_WAIT_MS + 1);
        r.tick();
        let asked = asked_of(&mut writes);
        assert!(asked.iter().any(|(_, l)| l == "/fcancel 45") && asked.iter().any(|(_, l)| l.contains("An image I sent didn't arrive here")), "{asked:?}");
        assert!(r.kept.sending.is_empty());

        // a reply of files alone; its text said survives a restart
        let (mut r, mut writes) = scripted("files-only", true);
        assert_eq!(r.asked(Asked::Reply { key: "chat:2".into(), text: String::new(), files: vec![file('c')], mark: 1 }).status(), 202);
        assert!(asked_of(&mut writes).is_empty(), "no words to send: its file is read first");
        assert!(r.outgoing.as_ref().is_some_and(|o| o.key == "chat:2"), "its file being read");
        let mut said = r.kept.replies[0].clone();
        said.said = true;
        let back: Reply = serde_json::from_str(&serde_json::to_string(&said).unwrap()).unwrap();
        assert_eq!(back, said, "a reply's files and its text said are in every save");
        let old: Reply = serde_json::from_str(r#"{"key":"k","text":"t","contact":3}"#).unwrap();
        assert!(old.files.is_empty() && !old.said, "an older save's reply: text alone");
        // its link ended: its files' read let go of
        r.kept.link.phase = Phase::Off;
        r.apply(Input::Resync);
        r.end_replies();
        assert!(r.outgoing.is_none() && r.kept.replies.is_empty());
        r.fetched("chat:2", Ok(vec![]));
    }

    /// Goal (Codex's review of S1): what a restart or a new disk meets
    /// loses no file and repeats none. No save is taken while its core may
    /// hold a file its door does not (a save from then, on a new disk,
    /// finds it received and gone), nor does a planned stop end the core
    /// before that; a file its door keeps refusing is noted once
    /// `FILE_WAIT_MS` passes, so the messages after it go on; a reply's
    /// file the core took is no longer its reply's (a restart sends only
    /// the rest); a link out of step while its files are read sends none
    /// and reads them again later, nothing noted; and an upload's failure
    /// is told only to the contact it was for. Method: a scripted core.
    #[tokio::test]
    async fn files_survive_restarts_and_relinks() {
        // saves held while a file is received and not yet its door's
        let (mut r, mut writes) = scripted("held", true);
        r.page = VecDeque::from([carrying(50, "voice", 9, 1_000, None)]);
        r.deliver(Instant::now());
        asked_of(&mut writes);
        r.due.changed(0);
        // its save due: past the debounce and the gap
        let due = Duration::from_millis(save::DEBOUNCE_MS + save::GAP_MS_MIN);
        r.born -= due;
        r.tick();
        assert!(!r.saving, "no save while it is received");
        said_of(&mut r, rcv_file("rcvFileComplete", 9, "memo.m4a"));
        r.tick();
        assert!(!r.saving, "nor while it is given to its door");
        r.given(50, Ok(door::File { sha256: "e".repeat(64), name: "memo.m4a".into(), media_type: "audio/mp4".into(), size: 1_000 }));
        r.born -= due;
        r.tick();
        assert!(r.saving, "its door holds it: saved");

        // a door that keeps refusing it: noted once FILE_WAIT_MS passes
        let (mut r, mut writes) = scripted("refusing", true);
        r.page = VecDeque::from([carrying(51, "image", 10, 1_000, Some("cat.jpg"))]);
        r.deliver(Instant::now());
        asked_of(&mut writes);
        r.given(51, Err(NotGiven::Later("503: files_full".into())));
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Whole { .. }), "given again later");
        r.receipt.as_mut().unwrap().state = Got::Giving { path: r.dir.join("files/cat.jpg"), since: 0 };
        r.born -= Duration::from_millis(run::FILE_WAIT_MS + 1);
        r.given(51, Err(NotGiven::Later("503: files_full".into())));
        assert!(matches!(&r.receipt.as_ref().unwrap().state, Got::Noted(n) if n.contains("a photo")), "{:?}", r.receipt);

        // a reply's files one at a time: a restart sends only the rest
        let (mut r, mut writes) = scripted("progress", true);
        r.asked(Asked::Reply { key: "chat:3".into(), text: String::new(), files: vec![file('a'), file('b')], mark: 1 });
        r.outgoing = Some(Outgoing { key: "chat:3".into(), at: None, asked: None });
        let out = r.dir.join(run::OUT_DIR);
        let copy = |n: &str| {
            let p = out.join(n).join("notes.bin");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"123456789").unwrap();
            p
        };
        r.fetched("chat:3", Ok(vec![Fetched::Copied { path: copy("1-0"), kind: wire::Kind::File }, Fetched::Copied { path: copy("1-1"), kind: wire::Kind::File }]));
        let sends = asked_of(&mut writes);
        assert_eq!(sends.len(), 2);
        r.frame(&sent_file(sends[0].0, 60));
        let saved: Kept = serde_json::from_slice(&serde_json::to_vec(&r.kept).unwrap()).unwrap();
        assert_eq!(saved.replies[0].files, vec![file('b')], "the file the core took is no longer its reply's, in its save");
        assert_eq!(saved.sending[&60], Upload { folder: "1-0".into(), kind: wire::Kind::File, contact: 3 });
        r.frame(&sent_file(sends[1].0, 61));
        assert!(r.kept.replies.is_empty() && r.kept.replied.contains(&"chat:3".to_string()), "done once both were taken");

        // a link out of step while its files are read: none sent, none noted, read again
        let (mut r, mut writes) = scripted("out-of-step", true);
        r.asked(Asked::Reply { key: "chat:4".into(), text: String::new(), files: vec![file('c')], mark: 1 });
        r.outgoing = Some(Outgoing { key: "chat:4".into(), at: None, asked: None });
        r.kept.link.syncing = true;
        let p = copy("1-2");
        r.fetched("chat:4", Ok(vec![Fetched::Copied { path: p.clone(), kind: wire::Kind::File }]));
        assert!(asked_of(&mut writes).is_empty(), "nothing sent, no note");
        assert!(r.outgoing.is_none() && !p.exists(), "its copy let go, to read again");
        assert_eq!(r.kept.replies[0].files, vec![file('c')], "still its reply's");

        // an upload's failure told only to the contact it was for
        let (mut r, mut writes) = scripted("relinked", true);
        r.kept.sending.insert(62, Upload { folder: "1-3".into(), kind: wire::Kind::File, contact: 3 });
        r.sends.insert(62, 0);
        r.send_ended(62, false);
        assert!(asked_of(&mut writes).iter().any(|(_, l)| l.contains("A file I sent didn't arrive here")), "told its own contact");
        r.kept.sending.insert(63, Upload { folder: "1-4".into(), kind: wire::Kind::File, contact: 3 });
        r.sends.insert(63, 0);
        r.kept.link.phase = Phase::Linked { contact: 4, name: "someone else".into() };
        r.end_replies();
        assert!(r.kept.sending.is_empty() && r.sends.is_empty(), "another contact's link: the upload forgotten");
        said_of(&mut r, snd_file("sndFileError", 63));
        assert!(asked_of(&mut writes).is_empty(), "its late end tells the new contact nothing");
    }

    /// Goal (Codex's fix check of S1): no receive begins while a save
    /// copies the databases; a file its door holds is carried by a later
    /// life whose disk lost the core's copy, never noted; a planned stop
    /// gives a file received whole before its core ends; and a reply's two
    /// files of the same bytes under two names are each its own. Method: a
    /// scripted core.
    #[tokio::test]
    async fn a_file_its_door_holds_is_never_lost() {
        // a save copying: the receive waits for it
        let (mut r, mut writes) = scripted("saving", true);
        r.page = VecDeque::from([carrying(70, "voice", 11, 1_000, None)]);
        r.saving = true;
        r.deliver(Instant::now());
        assert!(asked_of(&mut writes).is_empty() && r.receipt.is_none(), "nothing asked while a save copies");
        r.saving = false;
        r.deliver(Instant::now());
        assert_eq!(asked_of(&mut writes).iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/freceive 11"]);

        // given, then a new disk: the core's copy gone, its door's carried
        let held = door::File { sha256: "f".repeat(64), name: "memo.m4a".into(), media_type: "audio/mp4".into(), size: 1_000 };
        let (mut r, mut writes) = scripted("given", true);
        r.page = VecDeque::from([carrying(71, "voice", 12, 1_000, Some("memo.m4a"))]);
        r.deliver(Instant::now());
        asked_of(&mut writes);
        r.given(71, Ok(held.clone()));
        let saved: Kept = serde_json::from_slice(&serde_json::to_vec(&r.kept).unwrap()).unwrap();
        assert_eq!(saved.given, Some((71, held.clone())), "its save says its door holds it");
        let (mut r, mut writes) = scripted("new-disk", true);
        r.kept = saved;
        r.page = VecDeque::from([carrying(71, "voice", 12, 1_000, Some("memo.m4a"))]);
        r.deliver(Instant::now());
        assert!(asked_of(&mut writes).is_empty() && r.delivering, "delivered at once");
        assert!(matches!(&r.receipt.as_ref().unwrap().state, Got::Carried { file, .. } if *file == held), "carried as its door holds it, never noted: {:?}", r.receipt);
        r.on(Msg::Delivered { item: 71, mark: 1 });
        assert_eq!(r.kept.given, None, "delivered: let go");

        // a planned stop gives a file received whole
        let (mut r, mut writes) = scripted("stopping", true);
        r.page = VecDeque::from([carrying(72, "voice", 13, 1_000, None)]);
        r.deliver(Instant::now());
        asked_of(&mut writes);
        said_of(&mut r, rcv_file("rcvFileComplete", 13, "memo.m4a"));
        r.begin_stop("sigterm");
        r.tick();
        assert!(matches!(r.receipt.as_ref().unwrap().state, Got::Giving { .. }), "given before its core ends: {:?}", r.receipt);
        assert!(r.core_stop.is_none(), "its core not yet asked to end");

        // two files of the same bytes, two names: each its own
        let (mut r, mut writes) = scripted("twins", true);
        let (x, y) = (door::File { name: "x.bin".into(), ..file('a') }, door::File { name: "y.bin".into(), ..file('a') });
        r.asked(Asked::Reply { key: "chat:5".into(), text: String::new(), files: vec![x.clone(), y.clone()], mark: 1 });
        r.outgoing = Some(Outgoing { key: "chat:5".into(), at: None, asked: None });
        let out = r.dir.join(run::OUT_DIR);
        let copy = |n: &str, name: &str| {
            let p = out.join(n).join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"123456789").unwrap();
            p
        };
        r.fetched("chat:5", Ok(vec![Fetched::Copied { path: copy("1-7", "x.bin"), kind: wire::Kind::File }, Fetched::Copied { path: copy("1-8", "y.bin"), kind: wire::Kind::File }]));
        let sends = asked_of(&mut writes);
        r.frame(&sent_file(sends[1].0, 80));
        assert_eq!(r.kept.replies[0].files, vec![x], "the one the core took, by its name too");
    }
}
