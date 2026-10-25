//! The bridge's SimpleX (docs/bridge.md, "SimpleX"): one `simplex-chat`
//! core per agent its owner linked, each a child process of the bridge,
//! driven over its bot API by `link`'s rules. One task (`Manager`) owns
//! every core, so no two things ever drive one.
//!
//! - **Only where the image says, only while always on.** Nothing here
//!   runs unless the image names the core (`BRIDGE_SIMPLEX_BIN`), and a
//!   core runs only while the computer is always on (`alwaysOn` in its
//!   view: the always-on seat) and its agent's link is not off. A computer
//!   that is not always on runs none: a sleeping one would hear nothing.
//! - **Its state** is `<dir>/<agent>/` (`/data/simplex/<agent>/`): the
//!   core's two databases (`sx_chat.db`, `sx_agent.db`), its files and temp
//!   folders (on one device: a received file is renamed from one into the
//!   other), and `link.json` (the link, the agent it is of, the start it
//!   last ran in), written whole before what it causes is done.
//! - **The hold stops every core** before the bridge answers it (`quiet`),
//!   so a save takes the core's databases as they are, whole: planned stops
//!   (a sleep, a restart, an update) lose nothing. The core starts again
//!   when the hold goes. `/_start` after `/_stop` crashes v7.0.3, so it
//!   is the process that stops (SIGTERM; the lab: whole, no `-wal`).
//! - **A start that went back in time** (the platform's
//!   `restored.rollback`, once in a start, for a link last run before it)
//!   forces a ratchet sync (`link`).
//! - **A link changed asks for a save**: the keepalive held for a moment,
//!   so the platform saves as the work ends, rather than within 15
//!   minutes of the next turn.
//! - **What a core prints is not logged**: it echoes the commands it gets,
//!   message text among them. Its exits are, by their code.
//! - **Only its owner links or unlinks it**, from their Settings, which
//!   hand the link page a ticket the platform signed (`door`): the bridge
//!   takes each once, makes the link's door key, and registers it at the
//!   platform before any invitation exists; the invitation is shown only to
//!   the page whose ticket made it (`View`'s `session`).
//! - **The owner's messages** go to the agent's own chat with them (its Bot
//!   Chat) as relayed records (docs/chat-records.md, "A relayed message"),
//!   each by its sender's id, so a message read again is one record; a
//!   file it carries (a voice message's) is received first and carried as
//!   its attachment. **Replies** to them come back here once posted
//!   (`Handle::reply`), and go to the owner's app.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use super::door::{self, Key, Ticket};
use super::link::{Action, Inbound, Input, Link, Notice, Phase};
use super::wire::{self, Cmd, FileId, Resp};
use crate::api::{self, Api};
use crate::records;
use crate::runtime::{Agent, LocalFile};

/// Each agent's state, under its directory.
pub const LINK_FILE: &str = "link.json";
/// The bot API answers within this of its process starting, or the start
/// failed (the lab: about half a second).
const CONNECT_MS_MAX: u64 = 30_000;
/// A core is given this long to stop on SIGTERM, then killed.
const STOP_MS_MAX: u64 = 3_000;
/// A core that stops on its own this often in `EXITS_WINDOW_MS` is not
/// started again until its owner asks (a link, an unlink) or the bridge
/// starts again.
const EXITS_MAX: usize = 3;
const EXITS_WINDOW_MS: u64 = 10 * 60_000;
/// A core that stopped on its own is started again after this, doubled
/// for each exit in the window.
const RESTART_MS: u64 = 2_000;
/// A link's change holds the keepalive this long: the platform saves once
/// it closes (docs/computers.md, "Saves").
const PULSE_MS: u64 = 3_000;
/// A core told to stop while commands are under way gets this long to
/// answer them (an unlink's delete) first.
const DRAIN_MS: u64 = 5_000;
const TICK_MS: u64 = 100;
/// A forced sync that failed is asked again this often until it is `ok`.
const RESYNC_MS: u64 = 30_000;
const INBOX_MAX: usize = 256;
const WRITER_MAX: usize = 64;
/// What the core's last words (stderr) are kept to, for its exit's line.
const STDERR_TAIL_BYTES: usize = 2_048;
/// An agent's name, as its SimpleX profile shows it, at most.
const DISPLAY_NAME_MAX: usize = 32;
/// Messages waiting to be posted for one agent, at most; past it the
/// oldest wait for the next start's catch-up.
const POSTS_MAX: usize = 64;
/// Files a link waits to receive at once, at most.
const FILES_MAX: usize = 32;
/// A file not received this long after its message is given up on: the
/// message is posted with a note that it did not arrive. The core retries
/// a server it cannot reach for good, and says nothing (a computer on
/// Cloudflare reaches no XFTP server: docs/bridge.md, "SimpleX"); a voice
/// note or a photo arrives in seconds.
const FILE_WAIT_MS: u64 = 3 * 60 * 1000;
/// Files given up on that a link keeps, at most (`Saved::abandoned`).
const ABANDONED_MAX: usize = 64;
/// A cancel still to be taken is asked again this often while its core runs.
const CANCEL_RETRY_MS: u64 = 10_000;
/// The Bot Chat is looked for this often while none is found.
const CHAT_LOOK_MS: u64 = 30_000;
/// The tickets an agent's link took, kept until they would have run out
/// anyway, at most (each holds 10 minutes; its owner makes them by hand).
const USED_MAX: usize = 64;

/// What the image gives the bridge for SimpleX (main.rs reads it).
#[derive(Debug, Clone)]
pub struct Config {
    /// `BRIDGE_SIMPLEX_BIN`: the core.
    pub bin: PathBuf,
    /// `BRIDGE_SIMPLEX_DIR`: each agent's state (`/data/simplex`).
    pub dir: PathBuf,
    /// `BRIDGE_SIMPLEX_SMP`, `BRIDGE_SIMPLEX_XFTP`: servers to use (a
    /// test's); none, the core's own presets.
    pub smp: Option<String>,
    pub xftp: Option<String>,
}

/// What an agent's link page is told (`GET /simplex/state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub agent: String,
    /// `off`, `asking` (an invitation is being made), `inviting` (shown,
    /// unused), `linked`.
    pub phase: &'static str,
    /// The one-time invitation, while inviting: the full link every
    /// SimpleX app reads, and the short one when the server made it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    /// The SimpleX contact it is linked to, by their name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// While inviting: whether the invitation is for whoever asks (`link`
    /// and `short` are shown only to the page whose ticket made it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    pub always_on: bool,
    /// Its core runs now.
    pub running: bool,
    /// Why it does not run, or what last went wrong, in plain words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Why a link page's ask was refused: its status, and why in plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub why: String,
}

impl Refused {
    fn new(status: u16, why: impl Into<String>) -> Refused {
        Refused { status, why: why.into() }
    }
}

type Answer = oneshot::Sender<Result<View, Refused>>;

/// What reaches the manager.
pub enum Msg {
    /// The computer as the bridge last read it, and the agents it runs.
    Computer { computer: Box<api::Computer>, agents: Vec<Agent> },
    /// What `agent`'s page is told: its invitation only for `session`.
    View { agent: String, session: Option<String>, reply: oneshot::Sender<Option<View>> },
    /// A link's ticket, checked and taken (`door::check`, once).
    Ticket { agent: String, ticket: Box<Ticket>, reply: Answer },
    /// Its owner's link, its door's key registered: an invitation, shown
    /// to `session`.
    Want { agent: String, key: Box<Key>, session: String, reply: Answer },
    /// Its owner's unlink, by an unlink's ticket.
    Unlink { agent: String, ticket: Box<Ticket>, reply: Answer },
    /// An agent's name, as its owner titled it (looked up off the loop).
    Titled { agent: String, title: String },
    /// A core's bot API answered.
    Up { agent: String, epoch: u64 },
    /// A frame its core sent.
    Frame { agent: String, epoch: u64, text: String },
    /// A core's process ended (`why`: its code and last words).
    Exited { agent: String, epoch: u64, why: String },
    /// A core's bot API closed while its process may live on.
    Lost { agent: String, epoch: u64 },
    /// The agent's reply to a message relayed from SimpleX, posted: to
    /// its owner's app.
    Reply { agent: String, text: String },
    /// A message of its owner's, of this item, is on their chat (`ok`), or
    /// was refused there (its poster's word).
    Delivered { agent: String, item: i64, ok: bool },
}

/// How the rest of the bridge reaches the manager.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Msg>,
    api: Api,
}

impl Handle {
    /// The computer as just read: the manager starts and stops cores from it.
    pub fn computer(&self, computer: api::Computer, agents: Vec<Agent>) {
        if self.tx.try_send(Msg::Computer { computer: Box::new(computer), agents }).is_err() {
            crate::ev!("simplex.busy", { "dropped": "computer" });
        }
    }

    async fn ask(&self, make: impl FnOnce(oneshot::Sender<Option<View>>) -> Msg) -> Option<View> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(make(reply)).await.ok()?;
        answer.await.ok().flatten()
    }

    /// What `agent`'s link page shows, its invitation to `session` alone;
    /// none for an agent this bridge does not run.
    pub async fn view(&self, agent: &str, session: Option<&str>) -> Option<View> {
        self.ask(|reply| Msg::View { agent: agent.to_string(), session: session.map(str::to_string), reply }).await
    }

    async fn answered(&self, make: impl FnOnce(Answer) -> Msg) -> Result<View, Refused> {
        let (reply, answer) = oneshot::channel();
        let gone = || Refused::new(503, "this computer is stopping: try again in a moment");
        self.tx.send(make(reply)).await.map_err(|_| gone())?;
        answer.await.map_err(|_| gone())?
    }

    /// Its owner links `agent`, by the ticket their Settings handed its
    /// page: checked and taken, its door's key made and registered at the
    /// platform with it, then an invitation, shown to that page alone.
    pub async fn link(&self, agent: &str, ticket: Ticket) -> Result<View, Refused> {
        self.answered(|reply| Msg::Ticket { agent: agent.to_string(), ticket: Box::new(ticket.clone()), reply }).await?;
        let key = Key::generate().map_err(|e| Refused::new(500, format!("SimpleX could not make its key: {e}")))?;
        if let Err(e) = self.api.door(agent, door::DOOR, &ticket.nonce, &key.public_hex()).await {
            crate::ev!("simplex.door_refused", { "agent": agent, "error": e.to_string() });
            let status = if e.status() == Some(403) { 403 } else { 502 };
            return Err(Refused::new(status, "the platform did not take this link: start again from your Settings"));
        }
        crate::ev!("simplex.door_opened", { "agent": agent, "key": key.public_hex() });
        self.answered(|reply| Msg::Want { agent: agent.to_string(), key: Box::new(key), session: ticket.nonce.clone(), reply }).await
    }

    /// Its owner unlinks `agent`, by the ticket their Settings handed its
    /// page (the platform closed its door as it made it).
    pub async fn unlink(&self, agent: &str, ticket: Ticket) -> Result<View, Refused> {
        self.answered(|reply| Msg::Unlink { agent: agent.to_string(), ticket: Box::new(ticket), reply }).await
    }

    /// `agent`'s reply to a message its owner sent over SimpleX, posted on
    /// its chat: sent to their app too. Never waits (a lane calls it).
    pub fn reply(&self, agent: &str, text: &str) {
        if self.tx.try_send(Msg::Reply { agent: agent.to_string(), text: text.to_string() }).is_err() {
            crate::ev!("simplex.busy", { "dropped": "reply", "agent": agent });
        }
    }
}

/// A message of the owner's, to post on the agent's chat: with the file it
/// carries, once received, or a note of why none, signed by its door.
struct Post {
    inbound: Inbound,
    file: Option<LocalFile>,
    note: Option<String>,
    door: Key,
}

/// What a file a message carries is: by its name's extension when it is
/// one of these, else by the message's kind.
pub fn media_type(kind: &str, name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => match kind {
            // SimpleX's apps record a voice message as MPEG-4 AAC, a photo as a JPEG
            "voice" => "audio/mp4",
            "image" => "image/jpeg",
            "video" => "video/mp4",
            _ => "application/octet-stream",
        },
    }
}

/// What the agent's chat says of a file that could not be carried.
fn file_note(kind: &str, why: &str) -> String {
    let what = match kind {
        "voice" => "a voice message",
        "image" => "a photo",
        "video" => "a video",
        _ => "a file",
    };
    format!("(I sent {what} over SimpleX that {why}.)")
}

/// An agent's `link.json`: its link, the agent it is of (a fragment's name
/// may come back as another agent's), the computer's start it last ran
/// in, and the tickets it took (nonce → when it runs out), so none is
/// taken twice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub identity: String,
    #[serde(default)]
    pub generation: u64,
    #[serde(flatten)]
    pub link: Link,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub used: BTreeMap<String, i64>,
    /// The files it gave up on (`FILE_WAIT_MS`), each until its receiving
    /// is cancelled at the core, then settled: only these are ever deleted
    /// when they arrive anyway, and a message that carries one is posted
    /// with its note, never waited for again (Alex's review of #610). At
    /// most `ABANDONED_MAX`: only a settled one is ever let go (the oldest
    /// first), never a cancel still to be taken; no new file is received
    /// while those and the files waiting would pass it (`forward`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub abandoned: BTreeMap<FileId, Abandoned>,
    /// The files it asked its core to receive (`/freceive`) and that have
    /// not yet arrived, failed or been given up on. Kept across restarts, as
    /// the core resumes them from its own database: each holds its place
    /// within `ABANDONED_MAX`, and its message is waited for again however
    /// its catch-up and a newer message arrive (Alex's review of #612).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub receiving: BTreeSet<FileId>,
}

/// A file given up on: its cancel still to be taken by the core, or taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Abandoned {
    Cancelling,
    Settled,
}

/// Whether a core's answer to a cancel settles it: it took it, or refused
/// it for good (the file unknown, done, or cancelled already: a store or a
/// chat error). Its agent's or its database's failures are retried, as is
/// a cancel it was never sent or never answered.
pub fn cancel_settled(answer: &Resp) -> bool {
    match answer {
        Resp::Error { what } => !(what.starts_with("errorAgent") || what.starts_with("errorDatabase")),
        _ => true,
    }
}

/// Whether a core's first start in this life went back in time: the
/// platform says this start did, and the link last ran in an earlier one.
pub fn rolled_back(computer: &api::Computer, saved: &Saved) -> bool {
    computer.rolled_back() && saved.generation < computer.generation
}

/// An agent fragment's name, as a directory: `<label>--<suffix>`, of
/// lowercase letters, digits and `-`.
pub fn dir_name_ok(fragment: &str) -> bool {
    !fragment.is_empty() && fragment.len() <= 96 && fragment.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A core's command line: its state under `dir`, its bot API on `port`
/// (loopback), and the servers a test names (else the core's presets).
/// Every connection through the bridge's own proxy (`socks`, on loopback, at
/// `socks`), which sends SimpleX's file servers through the platform's XFTP
/// relay, and the rest straight out: the hosts it names, never their onion
/// addresses (`--host-mode public`: the proxy is no Tor).
/// SMP servers only on their own port (5223): the core reaches presets on
/// 443 by default, with no server name in its TLS hello (SNI: a SimpleX
/// server on 443 tells its protocol from its website by its absence), and
/// Cloudflare Containers reset every such hello on 443 (measured on a
/// preview computer, 2026-10-25: each SMP preset reset there, and the same
/// servers answering on 5223). XFTP's servers are on 443 alone, so a file
/// arrives only through the relay (docs/explorations/simplex-complexity.md).
fn core_args(cfg: &Config, dir: &Path, port: u16, name: &str, socks: Option<u16>) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec!["-d".into(), dir.join("sx").into(), "-p".into(), port.to_string().into()];
    args.extend(["--files-folder".into(), dir.join("files").into(), "--temp-folder".into(), dir.join("tmp").into()]);
    for a in ["-y", "--mute", "--create-bot-display-name", name, "--create-bot-allow-files", "--smp-web-port-servers", "off"] {
        args.push(a.into());
    }
    if let Some(p) = socks {
        args.extend(["--socks-proxy".into(), format!("127.0.0.1:{p}").into(), "--host-mode".into(), "public".into()]);
    }
    if let Some(smp) = &cfg.smp {
        args.extend(["-s".into(), smp.into()]);
    }
    if let Some(xftp) = &cfg.xftp {
        args.extend(["--xftp-server".into(), xftp.into()]);
    }
    args
}

/// The name an agent's SimpleX profile shows: its title, of letters, digits,
/// spaces and `-_.'`, at most `DISPLAY_NAME_MAX` bytes; else its label.
pub fn display_name(title: Option<&str>, label: &str) -> String {
    let clean = |s: &str| -> String {
        let kept: String = s.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '\'')).collect();
        wire::cut(kept.trim(), DISPLAY_NAME_MAX).trim().to_string()
    };
    let named = title.map(clean).unwrap_or_default();
    if !named.is_empty() {
        return named;
    }
    let label = clean(label);
    if label.is_empty() {
        "Agent".into()
    } else {
        label
    }
}

/// What the owner's SimpleX app is told as the link is made.
fn greeting(name: &str) -> String {
    format!("Hi, it's {name}. You're linked: write to me here any time, as you would in our chat.")
}

/// What the owner's SimpleX app is told after a start that went back in time.
const MAYBE_LOST: &str = "I restarted and may have missed a message you sent me just before. If I don't answer something, please send it again.";

/// What it is told when its message cannot be carried: this link has no
/// door key (one made before doors, or one a rollback took).
const NO_DOOR: &str = "Your message didn't reach me. Please link SimpleX to me again from your Settings.";

/// Reads an agent's `link.json`: none when there is none; one that does
/// not read, contradicts itself, or is another agent's is set aside (its
/// directory renamed, kept for whoever runs the computer) and none read.
pub fn load(dir: &Path, agent: &Agent) -> Option<Saved> {
    let path = dir.join(LINK_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            set_aside(dir, &format!("unreadable: {e}"));
            return None;
        }
    };
    let saved = serde_json::from_str::<Saved>(&text).map_err(|e| e.to_string()).and_then(|s| Link::from_json(&serde_json::to_string(&s.link).expect("a link serializes")).map(|_| s));
    match saved {
        Ok(s) if s.identity == agent.identity => Some(s),
        Ok(_) => {
            set_aside(dir, "another agent's");
            None
        }
        Err(e) => {
            set_aside(dir, &e);
            None
        }
    }
}

fn set_aside(dir: &Path, why: &str) {
    let to = dir.with_file_name(format!("{}.set-aside-{}", dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), crate::log::now_ms()));
    crate::ev!("simplex.set_aside", { "dir": dir.display().to_string(), "to": to.display().to_string(), "why": wire::cut(why, 200) });
    if let Err(e) = std::fs::rename(dir, &to) {
        crate::ev!("simplex.set_aside_failed", { "error": e.to_string() });
    }
}

/// Writes an agent's `link.json` whole (a temporary file, synced, renamed).
pub fn store(dir: &Path, saved: &Saved) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{LINK_FILE}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(serde_json::to_string(saved).expect("a link serializes").as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join(LINK_FILE))
}

/// The agents whose link is bound, as their files say (read at the
/// bridge's start, before any record: the engine honors their relayed
/// records from the first).
pub fn linked_on_disk(dir: &Path, agents: &[Agent]) -> Vec<String> {
    agents.iter().filter(|a| dir_name_ok(&a.fragment)).filter(|a| {
        let text = std::fs::read_to_string(dir.join(&a.fragment).join(LINK_FILE)).unwrap_or_default();
        serde_json::from_str::<Saved>(&text).is_ok_and(|s| s.identity == a.identity && matches!(s.link.phase, Phase::Linked { .. }))
    }).map(|a| a.fragment.clone()).collect()
}

/// A running core.
struct Core {
    pid: u32,
    writer: mpsc::Sender<String>,
    /// Turns true when its process has ended.
    exited: watch::Receiver<bool>,
    /// The bridge stopped it: its end is no failure.
    planned: bool,
}

/// One agent's SimpleX.
struct Slot {
    agent: Agent,
    dir: PathBuf,
    saved: Saved,
    title: Option<String>,
    epoch: u64,
    core: Option<Core>,
    up: bool,
    corr: u64,
    pending: BTreeMap<u64, Cmd>,
    /// Its core has started once in this life (its first start asks
    /// whether this start went back in time).
    started: bool,
    exits: VecDeque<Instant>,
    retry_at: Option<Instant>,
    /// When a forced sync still waiting is asked again (`Input::Resync`).
    resync_at: Option<Instant>,
    /// It stopped too often: not started again until its owner asks.
    broken: Option<String>,
    /// What last went wrong, or what the owner last did, for its page.
    note: Option<String>,
    /// Since when it should not run but has commands under way.
    draining_since: Option<Instant>,
    /// When its cancels still to be taken are asked of its core next.
    cancel_at: Option<Instant>,
    /// Messages whose file it waits to receive, by the file, and since when.
    files: BTreeMap<FileId, (Inbound, Instant)>,
    /// Its poster: its messages, posted in order on its chat.
    poster: Option<mpsc::Sender<Post>>,
    /// A message of this life was not posted (refused, or dropped past a
    /// bound): nothing more is acknowledged, so the next start reads back
    /// to before it and carries it then.
    gap: bool,
    /// The messages forwarded and not yet acknowledged, by item: receiving
    /// their file, queued, or being posted. What was delivered never moves
    /// past the oldest of them.
    outstanding: std::collections::BTreeSet<i64>,
    /// The newest message acknowledged as posted.
    acked: i64,
    /// Its link's door key (`door.key`): what it relays is signed by it.
    door: Option<Key>,
    /// The ticket nonce of the page that asked for its invitation last:
    /// that page alone is shown it.
    session: Option<String>,
    /// The link ticket taken last, its registration at the platform under
    /// way: only it installs its key (`want`).
    linking: Option<String>,
}

impl Slot {
    fn view(&self, always_on: bool, session: Option<&str>) -> View {
        let shown = session.is_some() && session == self.session.as_deref();
        let (phase, link, short, name, hidden) = match &self.saved.link.phase {
            Phase::Off => ("off", None, None, None, false),
            Phase::Asking => ("asking", None, None, None, false),
            Phase::Inviting { link, short, .. } if shown => ("inviting", Some(link.clone()), short.clone(), None, false),
            Phase::Inviting { .. } => ("inviting", None, None, None, true),
            Phase::Linked { name, .. } => ("linked", None, None, Some(name.clone()), false),
        };
        let why = if !always_on {
            Some("SimpleX works while your computer stays on, which the always-on plan does.".to_string())
        } else {
            self.broken.clone().or_else(|| self.note.clone())
        };
        View { agent: self.agent.fragment.clone(), phase, link, short, name, hidden, always_on, running: self.core.is_some() && self.up, why }
    }
}

/// The manager's own state.
struct Manager {
    cfg: Config,
    api: Api,
    quiet: Arc<AtomicBool>,
    hold: PathBuf,
    tx: mpsc::Sender<Msg>,
    slots: BTreeMap<String, Slot>,
    computer: Option<api::Computer>,
    held: bool,
    pulse: watch::Sender<bool>,
    pulse_until: Option<Instant>,
    /// The agents whose link is bound: the bridge's engine honors only
    /// their relayed records.
    relaying: watch::Sender<BTreeSet<String>>,
    /// The cores' way out (`socks`), on loopback: its port, once bound.
    socks: Option<u16>,
    /// What the platform's XFTP relay carries, as its view last said.
    offer: watch::Sender<Option<api::RelayOffer>>,
    stop: watch::Receiver<bool>,
}

/// Starts the manager: its handle. `quiet` is true while no core runs
/// (the bridge's answer to a hold waits on it); `hold` is the platform's
/// hold file.
pub fn start(cfg: Config, api: Api, quiet: Arc<AtomicBool>, hold: PathBuf, relaying: watch::Sender<BTreeSet<String>>, stop: watch::Receiver<bool>) -> Handle {
    let (tx, rx) = mpsc::channel(INBOX_MAX);
    let (pulse, pulse_rx) = watch::channel(false);
    // held for a moment at a link's change, at the pace the bridge's own pings by default
    tokio::spawn(crate::driver::keepalive(api.clone(), pulse_rx, Duration::from_millis(crate::limits::KEEPALIVE_PING_MS), stop.clone()));
    // the cores' way out: SimpleX's file servers through the platform's relay
    let (offer, offer_rx) = watch::channel(None);
    let socks = match std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.set_nonblocking(true).map(|()| l)).and_then(tokio::net::TcpListener::from_std) {
        Ok(l) => {
            let port = l.local_addr().map(|a| a.port()).ok();
            tokio::spawn(super::socks::serve(l, api.clone(), offer_rx, stop.clone()));
            port
        }
        Err(e) => {
            crate::ev!("simplex.socks_unbound", { "error": e.to_string() });
            None
        }
    };
    let m = Manager { cfg, api: api.clone(), quiet, hold, tx: tx.clone(), slots: BTreeMap::new(), computer: None, held: false, pulse, pulse_until: None, relaying, socks, offer, stop: stop.clone() };
    tokio::spawn(m.run(rx, stop));
    Handle { tx, api }
}

impl Manager {
    async fn run(mut self, mut rx: mpsc::Receiver<Msg>, mut stop: watch::Receiver<bool>) {
        crate::ev!("simplex.started", { "bin": self.cfg.bin.display().to_string(), "dir": self.cfg.dir.display().to_string(), "servers": self.cfg.smp.is_some() });
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // bounded by the bridge's life: one message or tick per pass, ended by `stop`
        loop {
            tokio::select! {
                biased;
                _ = crate::net::stopped(&mut stop) => {
                    self.stop_all().await;
                    return;
                }
                m = rx.recv() => match m {
                    Some(m) => self.on(m).await,
                    None => return,
                },
                _ = tick.tick() => self.tick().await,
            }
        }
    }

    async fn on(&mut self, m: Msg) {
        match m {
            Msg::Computer { computer, agents } => self.computer(*computer, agents).await,
            Msg::View { agent, session, reply } => {
                let _ = reply.send(self.view(&agent, session.as_deref()));
            }
            Msg::Ticket { agent, ticket, reply } => {
                let _ = reply.send(self.take(&agent, &ticket, "link").map(|()| self.view(&agent, None).expect("its slot")));
            }
            Msg::Want { agent, key, session, reply } => {
                let _ = reply.send(self.want(&agent, *key, session).await);
            }
            Msg::Unlink { agent, ticket, reply } => {
                let answer = self.take(&agent, &ticket, "unlink").map(|()| {
                    let s = self.slots.get_mut(&agent).expect("its slot");
                    s.broken = None;
                    s.note = None;
                    self.apply(&agent, Input::Unlink);
                    self.view(&agent, None).expect("its slot")
                });
                let _ = reply.send(answer);
            }
            Msg::Titled { agent, title } => {
                if let Some(s) = self.slots.get_mut(&agent) {
                    s.title = Some(title);
                }
            }
            Msg::Up { agent, epoch } => self.up(&agent, epoch),
            Msg::Frame { agent, epoch, text } => self.frame(&agent, epoch, &text),
            Msg::Exited { agent, epoch, why } => self.exited(&agent, epoch, &why),
            Msg::Lost { agent, epoch } => self.lost(&agent, epoch).await,
            Msg::Reply { agent, text } => self.apply(&agent, Input::Reply(&text)),
            Msg::Delivered { agent, item, ok } => self.delivered(&agent, item, ok),
        }
    }

    fn view(&self, agent: &str, session: Option<&str>) -> Option<View> {
        let always_on = self.computer.as_ref().is_some_and(|c| c.always_on);
        self.slots.get(agent).map(|s| s.view(always_on, session))
    }

    /// A ticket for `act` on `agent`, checked (`door::check`: this
    /// computer's, that agent's, signed by its key, in its time), and taken
    /// once: kept in its link's file until it would have run out. A link's
    /// is refused while one is bound.
    fn take(&mut self, agent: &str, t: &Ticket, act: &str) -> Result<(), Refused> {
        let computer = self.computer.as_ref().map(|c| c.computer.clone()).ok_or_else(|| Refused::new(503, "this computer is starting: try again in a moment"))?;
        let s = self.slots.get_mut(agent).ok_or_else(|| Refused::new(404, format!("no agent {agent} runs on this computer")))?;
        let key = s.agent.key.clone().ok_or_else(|| Refused::new(403, "this platform does not sign links yet"))?;
        let now = crate::log::now_ms() as i64;
        door::check(t, act, agent, &computer, &key, now).map_err(|why| Refused::new(403, why))?;
        s.saved.used.retain(|_, exp| *exp >= now);
        if s.saved.used.contains_key(&t.nonce) {
            return Err(Refused::new(403, "that ticket was used already: start again from your Settings"));
        }
        if act == "link" && matches!(s.saved.link.phase, Phase::Linked { .. }) {
            return Err(Refused::new(409, "it is linked already: unlink it first"));
        }
        // bounded, and never by forgetting a ticket that still holds
        if s.saved.used.len() >= USED_MAX {
            return Err(Refused::new(503, "too many links were started in the last ten minutes: wait a little, then start again"));
        }
        // taken once it is written, before anything it starts
        s.saved.used.insert(t.nonce.clone(), t.exp);
        if let Err(e) = store(&s.dir, &s.saved) {
            s.saved.used.remove(&t.nonce);
            crate::ev!("simplex.store_failed", { "agent": agent, "error": e.to_string() });
            return Err(Refused::new(500, format!("SimpleX could not keep that ticket as used: {e}")));
        }
        // a link's registration is this ticket's alone from here (`want`);
        // an unlink calls any under way off
        s.linking = (act == "link").then(|| t.nonce.clone());
        crate::ev!("simplex.ticket", { "agent": agent, "act": act });
        Ok(())
    }

    /// Its owner's link, its door's key registered: the key kept, then an
    /// invitation shown to `session`'s page (the same one while one is
    /// shown already).
    async fn want(&mut self, agent: &str, key: Key, session: String) -> Result<View, Refused> {
        let s = self.slots.get_mut(agent).ok_or_else(|| Refused::new(404, format!("no agent {agent} runs on this computer")))?;
        if matches!(s.saved.link.phase, Phase::Linked { .. }) {
            return Err(Refused::new(409, "it is linked already: unlink it first"));
        }
        // a link started since, or an unlink, made this one stale: its key
        // is not the one the platform holds now
        if s.linking.as_deref() != Some(session.as_str()) {
            return Err(Refused::new(409, "another link or an unlink was started meanwhile: start again from your Settings"));
        }
        s.linking = None;
        if let Err(e) = key.store(&s.dir) {
            crate::ev!("simplex.door_unkept", { "agent": agent, "error": e.to_string() });
            return Err(Refused::new(500, format!("SimpleX could not keep its key: {e}")));
        }
        s.door = Some(key);
        s.session = Some(session.clone());
        // asked by its owner: a core that stopped too often is tried again
        s.broken = None;
        s.exits.clear();
        s.retry_at = None;
        s.note = None;
        self.apply(agent, Input::Want);
        self.tick().await;
        Ok(self.view(agent, Some(&session)).expect("its slot"))
    }

    /// The computer as read: a slot for each agent it runs (its link read
    /// from its file), none for one that left (its core stopped; its
    /// state stays, as the agent's other files do).
    async fn computer(&mut self, computer: api::Computer, agents: Vec<Agent>) {
        self.offer.send_if_modified(|was| {
            let news = *was != computer.relay;
            if news {
                crate::ev!("simplex.relay_offered", { "hosts": computer.relay.as_ref().map_or(0, |o| o.hosts.len()) });
                was.clone_from(&computer.relay);
            }
            news
        });
        let gone: Vec<String> = self.slots.keys().filter(|f| !agents.iter().any(|a| &a.fragment == *f)).cloned().collect();
        for f in gone {
            self.stop_core(&f).await;
            self.slots.remove(&f);
            crate::ev!("simplex.agent_gone", { "agent": f });
        }
        for a in &agents {
            if self.slots.contains_key(&a.fragment) || !dir_name_ok(&a.fragment) {
                continue;
            }
            let dir = self.cfg.dir.join(&a.fragment);
            let saved = load(&dir, a).unwrap_or_else(|| Saved { identity: a.identity.clone(), generation: 0, link: Link::default(), used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() });
            let door = Key::load(&dir);
            self.slots.insert(a.fragment.clone(), Slot {
                agent: a.clone(),
                dir,
                saved,
                title: None,
                epoch: 0,
                core: None,
                up: false,
                corr: 0,
                pending: BTreeMap::new(),
                started: false,
                exits: VecDeque::new(),
                retry_at: None,
                resync_at: None,
                broken: None,
                note: None,
                draining_since: None,
                cancel_at: None,
                files: BTreeMap::new(),
                poster: None,
                gap: false,
                outstanding: Default::default(),
                acked: 0,
                door,
                session: None,
                linking: None,
            });
            self.look_up_title(a.clone());
        }
        self.computer = Some(computer);
        self.reconcile().await;
    }

    /// The agent's title, as its owner's list says it, read off the loop.
    fn look_up_title(&self, agent: Agent) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let Ok(list) = api.fragments_for(&agent.fragment, &agent.owner).await else { return };
            if let Some(title) = list.into_iter().find(|f| f.name == agent.fragment && !f.title.is_empty()).map(|f| f.title) {
                let _ = tx.send(Msg::Titled { agent: agent.fragment, title }).await;
            }
        });
    }

    async fn tick(&mut self) {
        let held = self.hold.exists();
        if held && !self.held {
            self.held = true;
            crate::ev!("simplex.hold", { "cores": self.running() });
            self.stop_all().await;
        } else if !held && self.held {
            self.held = false;
            crate::ev!("simplex.unhold");
        }
        if self.pulse_until.is_some_and(|t| Instant::now() >= t) {
            self.pulse_until = None;
            let _ = self.pulse.send(false);
        }
        self.reconcile().await;
        self.resync();
        self.files_overdue(Instant::now());
        self.quiet.store(self.running() == 0, Ordering::SeqCst);
    }

    /// A forced sync still waiting (one the core refused), asked again every
    /// `RESYNC_MS` while its core is up, until the ratchet is `ok`.
    fn resync(&mut self) {
        let now = Instant::now();
        let due: Vec<String> = self
            .slots
            .iter_mut()
            .filter_map(|(f, s)| {
                if !(s.up && s.saved.link.syncing && s.saved.link.forced) {
                    s.resync_at = None;
                    return None;
                }
                let at = *s.resync_at.get_or_insert(now + Duration::from_millis(RESYNC_MS));
                (now >= at).then(|| {
                    s.resync_at = Some(now + Duration::from_millis(RESYNC_MS));
                    f.clone()
                })
            })
            .collect();
        for f in due {
            crate::ev!("simplex.resync", { "agent": f });
            self.apply(&f, Input::Resync);
        }
    }

    /// Each file waited for past `FILE_WAIT_MS`: its receiving cancelled at
    /// the core (`/fcancel`, so it stops trying), and its message posted
    /// with a note that it did not arrive. One that completes anyway is
    /// removed as it does (`file_event`).
    fn files_overdue(&mut self, now: Instant) {
        let wait = Duration::from_millis(FILE_WAIT_MS);
        let overdue: Vec<(String, FileId)> = self.slots.iter().flat_map(|(f, s)| s.files.iter().filter(|(_, (_, since))| now.duration_since(*since) >= wait).map(|(id, _)| (f.clone(), *id))).collect();
        for (f, id) in overdue {
            let Some((inbound, _)) = self.slots.get_mut(&f).and_then(|s| s.files.remove(&id)) else { continue };
            crate::ev!("simplex.file_overdue", { "agent": f, "item": inbound.item, "waitMs": FILE_WAIT_MS });
            self.abandon(&f, id, Abandoned::Cancelling);
            self.reserve(&f, id, false);
            let note = file_note(&inbound.kind, "did not arrive");
            self.post(&f, inbound, None, Some(note));
        }
        self.cancels(now);
    }

    /// `id`'s receive asked of its core (`on`), or ended, in its link's file.
    fn reserve(&mut self, f: &str, id: FileId, on: bool) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let changed = if on { s.saved.receiving.insert(id) } else { s.saved.receiving.remove(&id) };
        if changed {
            if let Err(e) = store(&s.dir, &s.saved) {
                crate::ev!("simplex.store_failed", { "agent": f, "error": e.to_string() });
            }
        }
    }

    /// `id` given up on (or its cancel settled), in its link's file: at most
    /// `ABANDONED_MAX`, the oldest settled let go first (then the oldest).
    fn abandon(&mut self, f: &str, id: FileId, state: Abandoned) {
        let Some(s) = self.slots.get_mut(f) else { return };
        if s.saved.abandoned.get(&id) == Some(&state) {
            return;
        }
        if state == Abandoned::Cancelling {
            s.cancel_at = None;
        }
        s.saved.abandoned.insert(id, state);
        // bounded: a settled one goes for each past `ABANDONED_MAX`, never a
        // cancel still to be taken (`forward` keeps those within it)
        while s.saved.abandoned.len() > ABANDONED_MAX {
            let Some(oldest) = s.saved.abandoned.iter().find(|(_, a)| **a == Abandoned::Settled).map(|(k, _)| *k) else { break };
            s.saved.abandoned.remove(&oldest);
        }
        if let Err(e) = store(&s.dir, &s.saved) {
            crate::ev!("simplex.store_failed", { "agent": f, "error": e.to_string() });
        }
    }

    /// Each cancel still to be taken, asked of its core while it runs, every
    /// `CANCEL_RETRY_MS` until its answer settles it (`cancel_settled`):
    /// through holds (its core stopped), a writer full or gone (not sent),
    /// and restarts (its link's file keeps them). One already asked and not
    /// yet answered is not asked twice. It posts nothing.
    fn cancels(&mut self, now: Instant) {
        let due: Vec<(String, Vec<FileId>)> = self
            .slots
            .iter_mut()
            .filter(|(_, s)| s.up && s.core.is_some() && s.cancel_at.is_none_or(|at| now >= at))
            .filter_map(|(f, s)| {
                let asked: BTreeSet<FileId> = s.pending.values().filter_map(|c| if let Cmd::CancelFile { file } = c { Some(*file) } else { None }).collect();
                let ids: Vec<FileId> = s.saved.abandoned.iter().filter(|(id, a)| **a == Abandoned::Cancelling && !asked.contains(id)).map(|(id, _)| *id).collect();
                if ids.is_empty() {
                    return None;
                }
                s.cancel_at = Some(now + Duration::from_millis(CANCEL_RETRY_MS));
                Some((f.clone(), ids))
            })
            .collect();
        for (f, ids) in due {
            for id in ids {
                self.act(&f, Action::Ask(Cmd::CancelFile { file: id }));
            }
        }
    }

    fn running(&self) -> usize {
        self.slots.values().filter(|s| s.core.is_some()).count()
    }

    /// Each core started or stopped as the computer, the hold and its link
    /// say.
    async fn reconcile(&mut self) {
        let Some(always_on) = self.computer.as_ref().map(|c| c.always_on) else { return };
        let now = Instant::now();
        let names: Vec<String> = self.slots.keys().cloned().collect();
        for f in names {
            let s = &self.slots[&f];
            // linked, or with an unlink's let-go still to do
            let want = !self.held && always_on && (s.saved.link.phase != Phase::Off || s.saved.link.cleanup.is_some()) && s.broken.is_none();
            match (want, s.core.is_some()) {
                (true, false) if s.retry_at.is_none_or(|t| now >= t) => self.start_core(&f),
                (false, true) => {
                    // its last commands answered first (an unlink's delete), within a bound
                    let s = self.slots.get_mut(&f).expect("listed");
                    let since = *s.draining_since.get_or_insert(now);
                    if self.held || s.pending.is_empty() || since.elapsed() >= Duration::from_millis(DRAIN_MS) {
                        self.stop_core(&f).await;
                    }
                }
                _ => {
                    self.slots.get_mut(&f).expect("listed").draining_since = None;
                }
            }
        }
    }

    fn start_core(&mut self, f: &str) {
        // not running, then held: the bridge's answer to a hold that came
        // as this start began waits for it, and it is called off
        self.quiet.store(false, Ordering::SeqCst);
        if self.hold.exists() {
            self.quiet.store(self.running() == 0, Ordering::SeqCst);
            return;
        }
        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        let socks = self.socks;
        let s = self.slots.get_mut(f).expect("a slot");
        for sub in ["", "files", "tmp"] {
            if let Err(e) = std::fs::create_dir_all(s.dir.join(sub)) {
                s.note = Some(format!("SimpleX could not start: its folder: {e}"));
                return;
            }
        }
        let port = match std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()) {
            Ok(a) => a.port(),
            Err(e) => {
                s.note = Some(format!("SimpleX could not start: no port: {e}"));
                return;
            }
        };
        let name = display_name(s.title.as_deref(), &s.agent.name);
        let mut cmd = tokio::process::Command::new(&cfg.bin);
        // it dies with the bridge, however the bridge dies (a SIGKILL runs no
        // handler of ours): no core outlives it to run beside the next one's
        let parent = std::process::id();
        // SAFETY: only async-signal-safe calls, in the child before its exec
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // the bridge died before the line above took: so does this
                if libc::getppid() as u32 != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
        cmd.args(core_args(&cfg, &s.dir, port, &name, socks));
        cmd.current_dir(&s.dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                s.broken = Some(format!("SimpleX could not start: {e}"));
                crate::ev!("simplex.spawn_failed", { "agent": f, "error": e.to_string() });
                return;
            }
        };
        let Some(pid) = child.id() else {
            s.broken = Some("SimpleX could not start: it ended at once".into());
            return;
        };
        s.epoch += 1;
        s.up = false;
        s.pending.clear();
        s.draining_since = None;
        let epoch = s.epoch;
        let (writer, writes) = mpsc::channel::<String>(WRITER_MAX);
        let (ended, exited) = watch::channel(false);
        CORES.lock().expect("not poisoned").insert(pid);
        s.core = Some(Core { pid, writer, exited, planned: false });
        crate::ev!("simplex.core_started", { "agent": f, "epoch": epoch, "pid": pid });
        // its end, with its last words
        let (agent, tx2) = (f.to_string(), tx.clone());
        let stderr = child.stderr.take();
        tokio::spawn(async move {
            let mut tail = Vec::new();
            if let Some(mut err) = stderr {
                let mut buf = [0u8; 1024];
                // bounded by the process: its stderr ends with it
                while let Ok(n) = err.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        tail.drain(..tail.len() - STDERR_TAIL_BYTES);
                    }
                }
            }
            let status = child.wait().await;
            // reaped: no signal reaches its pid from here on
            CORES.lock().expect("not poisoned").remove(&pid);
            let _ = ended.send(true);
            let last = String::from_utf8_lossy(&tail).lines().rev().find(|l| !l.trim().is_empty() && !l.contains("received command")).unwrap_or("").to_string();
            let why = format!("{}{}", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()), if last.is_empty() { String::new() } else { format!(": {}", wire::cut(&last, 200)) });
            let _ = tx2.send(Msg::Exited { agent, epoch, why }).await;
        });
        // its bot API, then its frames
        let agent = f.to_string();
        tokio::spawn(async move {
            let url = format!("ws://127.0.0.1:{port}");
            let t0 = Instant::now();
            // bounded by CONNECT_MS_MAX
            let ws = loop {
                match tokio_tungstenite::connect_async(url.as_str()).await {
                    Ok((ws, _)) => break ws,
                    Err(_) if t0.elapsed() < Duration::from_millis(CONNECT_MS_MAX) => tokio::time::sleep(Duration::from_millis(200)).await,
                    Err(e) => {
                        crate::ev!("simplex.api_unreached", { "agent": agent, "error": e.to_string() });
                        // SAFETY: a signal to the core's own process, by its pid
                        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                        return;
                    }
                }
            };
            if tx.send(Msg::Up { agent: agent.clone(), epoch }).await.is_err() {
                return;
            }
            let (mut sink, mut stream) = ws.split();
            let mut writes = writes;
            // bounded by the socket, which closes with the process
            loop {
                tokio::select! {
                    w = writes.recv() => match w {
                        Some(frame) => if sink.send(Message::text(frame)).await.is_err() { break },
                        // its manager let it go (a planned stop)
                        None => return,
                    },
                    m = stream.next() => match m {
                        Some(Ok(Message::Text(t))) => {
                            if t.len() > wire::FRAME_MAX_BYTES {
                                crate::ev!("simplex.frame_too_large", { "agent": agent, "bytes": t.len() });
                                continue;
                            }
                            if tx.send(Msg::Frame { agent: agent.clone(), epoch, text: t.to_string() }).await.is_err() {
                                return;
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    },
                }
            }
            // closed under it: its process may live on, unheard
            let _ = tx.send(Msg::Lost { agent, epoch }).await;
        });
    }

    /// A core whose bot API closed: its process stopped (if it lives on)
    /// and counted as an exit of its own, so it is started again within
    /// the bound as one that crashed is. One that ended anyway is its
    /// `Exited`'s.
    async fn lost(&mut self, f: &str, epoch: u64) {
        let Some(s) = self.slots.get_mut(f).filter(|s| s.epoch == epoch) else { return };
        let Some(core) = s.core.take_if(|c| !c.planned && !*c.exited.borrow()) else { return };
        crate::ev!("simplex.api_lost", { "agent": f, "epoch": epoch });
        s.up = false;
        s.pending.clear();
        stop_process(f, core).await;
        self.count_exit(f, "its bot API closed");
    }

    async fn stop_core(&mut self, f: &str) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let Some(mut core) = s.core.take() else { return };
        core.planned = true;
        s.up = false;
        s.pending.clear();
        s.draining_since = None;
        stop_process(f, core).await;
        self.quiet.store(self.running() == 0, Ordering::SeqCst);
    }

    /// Every core stopped, together (a hold, the bridge's own stop).
    async fn stop_all(&mut self) {
        let mut stopping = vec![];
        for (f, s) in self.slots.iter_mut() {
            if let Some(mut core) = s.core.take() {
                core.planned = true;
                s.up = false;
                s.pending.clear();
                stopping.push(stop_process(f, core));
            }
        }
        futures_util::future::join_all(stopping).await;
        self.quiet.store(true, Ordering::SeqCst);
    }

    fn up(&mut self, f: &str, epoch: u64) {
        let Some(c) = self.computer.clone() else { return };
        let Some(s) = self.slots.get_mut(f).filter(|s| s.epoch == epoch && s.core.is_some()) else { return };
        s.up = true;
        let rollback = !s.started && rolled_back(&c, &s.saved);
        if !s.started && c.generation > s.saved.generation {
            s.saved.generation = c.generation;
        }
        s.started = true;
        crate::ev!("simplex.core_up", { "agent": f, "epoch": epoch, "rollback": rollback, "phase": s.view(c.always_on, None).phase });
        self.apply(f, Input::Started { rollback });
    }

    fn frame(&mut self, f: &str, epoch: u64, text: &str) {
        let Some(s) = self.slots.get_mut(f).filter(|s| s.epoch == epoch && s.core.is_some()) else { return };
        let frame = match wire::parse(text) {
            Ok(fr) => fr,
            Err(e) => {
                crate::ev!("simplex.frame_unread", { "agent": f, "error": wire::cut(&e.to_string(), 200) });
                return;
            }
        };
        let asked = frame.corr.and_then(|c| s.pending.remove(&c));
        if let Some(Cmd::CancelFile { file }) = &asked {
            let settled = cancel_settled(&frame.resp);
            crate::ev!("simplex.cancel_answered", { "agent": f, "file": file, "settled": settled });
            if settled {
                self.abandon(f, *file, Abandoned::Settled);
            }
        }
        match asked {
            Some(cmd) => match &frame.resp {
                Resp::Error { what } => {
                    crate::ev!("simplex.refused", { "agent": f, "cmd": kind_of(&cmd), "what": what });
                    self.apply(f, Input::Failed { cmd: &cmd, what })
                }
                resp => self.apply(f, Input::Said(resp)),
            },
            None => self.apply(f, Input::Said(&frame.resp)),
        }
        self.file_event(f, &frame.resp);
    }

    /// A file a forwarded message waits on: received once its description
    /// is ready, posted with its message once whole, or the message posted
    /// with a note once it will not arrive.
    fn file_event(&mut self, f: &str, resp: &Resp) {
        if let Resp::FileDone { file, .. } | Resp::FileFailed { file } = resp {
            self.reserve(f, *file, false);
        }
        let Some(s) = self.slots.get_mut(f) else { return };
        match resp {
            Resp::FileReady { file } if s.files.contains_key(file) => self.act(f, Action::Ask(Cmd::Receive { file: *file })),
            Resp::FileDone { file, path } => match s.files.remove(file) {
                Some((inbound, _)) => {
                    let local = LocalFile { path: s.dir.join("files").join(path), media_type: media_type(&inbound.kind, path).into(), name: wire::cut(path.rsplit('/').next().unwrap_or(path), 200).into(), size: inbound.file.as_ref().map_or(0, |r| r.size) };
                    self.post(f, inbound, Some(local), None);
                }
                // one given up on (`files_overdue`) that came anyway, its
                // message posted with a note: nothing carries it, so it goes
                None if s.saved.abandoned.contains_key(file) => {
                    let late = s.dir.join("files").join(path);
                    crate::ev!("simplex.file_late", { "agent": f, "file": file, "removed": std::fs::remove_file(&late).is_ok() });
                    self.abandon(f, *file, Abandoned::Settled);
                }
                // any other (one a restart resumed before its message's
                // catch-up, or a completion said again) is left as it is:
                // its message carries it
                None => crate::ev!("simplex.file_unclaimed", { "agent": f, "file": file }),
            },
            Resp::FileFailed { file } => {
                if let Some((inbound, _)) = s.files.remove(file) {
                    let note = file_note(&inbound.kind, "did not arrive");
                    self.post(f, inbound, None, Some(note));
                } else if s.saved.abandoned.contains_key(file) {
                    self.abandon(f, *file, Abandoned::Settled);
                }
            }
            _ => {}
        }
    }

    /// An owner's message the link forwards: posted now, or once the file
    /// it carries is received.
    fn forward(&mut self, f: &str, inbound: Inbound) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let Some(file) = inbound.file.clone() else {
            return self.post(f, inbound, None, None);
        };
        if file.size > crate::limits::ATTACHMENT_MAX_BYTES {
            let note = file_note(&inbound.kind, &format!("was too large to carry here ({} MB)", file.size / 1_000_000));
            return self.post(f, inbound, None, Some(note));
        }
        if s.saved.abandoned.contains_key(&file.id) {
            // given up on before (a catch-up's forward again): its note
            // again, by the same id, never a second wait
            let note = file_note(&inbound.kind, "did not arrive");
            return self.post(f, inbound, None, Some(note));
        }
        if let Some(path) = file.path.clone() {
            // received already (a catch-up of one received before)
            let local = LocalFile { path: s.dir.join("files").join(&path), media_type: media_type(&inbound.kind, &path).into(), name: wire::cut(path.rsplit('/').next().unwrap_or(&path), 200).into(), size: file.size };
            self.reserve(f, file.id, false);
            return self.post(f, inbound, Some(local), None);
        }
        if s.files.len() >= FILES_MAX && !s.files.contains_key(&file.id) {
            crate::ev!("simplex.files_bounded", { "agent": f, "max": FILES_MAX });
            return self.dropped(f, inbound.item);
        }
        // no new file while the cancels still to be taken and the files
        // waiting (each a cancel to come) would pass what the link keeps
        // (Alex's review): nothing is asked of the core, and its message is
        // posted with its note now, so no unsettled cancel is ever let go
        // (the receives asked count, kept across restarts: one the core
        // resumed holds its place, and is waited for again, never refused)
        let unsettled = s.saved.abandoned.values().filter(|a| **a == Abandoned::Cancelling).count();
        let reserved = s.saved.receiving.contains(&file.id) || s.files.contains_key(&file.id);
        if !reserved && unsettled + s.saved.receiving.len() >= ABANDONED_MAX {
            crate::ev!("simplex.files_held_back", { "agent": f, "unsettled": unsettled, "receiving": s.saved.receiving.len() });
            let note = file_note(&inbound.kind, "did not arrive");
            return self.post(f, inbound, None, Some(note));
        }
        // a catch-up's forward of one already waiting keeps its first time
        let id = file.id;
        s.files.entry(id).or_insert((inbound, Instant::now()));
        self.reserve(f, id, true);
        // its description may not be ready yet: then FileReady asks again
        self.act(f, Action::Ask(Cmd::Receive { file: id }));
    }

    /// A message's post, confirmed or refused: the link's delivered mark
    /// moves to it, short of a message still waiting for its file, and
    /// never past one this life did not post (`gap`).
    fn delivered(&mut self, f: &str, item: i64, ok: bool) {
        let Some(s) = self.slots.get_mut(f) else { return };
        s.outstanding.remove(&item);
        if !ok {
            s.gap = true;
        }
        if s.gap || item <= 0 {
            return;
        }
        s.acked = s.acked.max(item);
        let upto = s.outstanding.first().map_or(s.acked, |oldest| s.acked.min(oldest - 1));
        self.apply(f, Input::Delivered(upto));
    }

    /// A message forwarded that will not be posted this life (dropped past
    /// a bound): what was delivered moves no more (`gap`).
    fn dropped(&mut self, f: &str, item: i64) {
        if let Some(s) = self.slots.get_mut(f) {
            s.outstanding.remove(&item);
            s.gap = true;
        }
    }

    /// Hands a message to the agent's poster, started at its first, with
    /// its door's key to sign it; with none, its owner is told it did not
    /// arrive (the platform would refuse it).
    fn post(&mut self, f: &str, inbound: Inbound, file: Option<LocalFile>, note: Option<String>) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let Some(door) = s.door.clone() else {
            crate::ev!("simplex.no_door", { "agent": f });
            self.dropped(f, inbound.item);
            return self.apply(f, Input::Reply(NO_DOOR));
        };
        let p = Post { inbound, file, note, door };
        if s.poster.as_ref().is_none_or(|tx| tx.is_closed()) {
            let (tx, rx) = mpsc::channel(POSTS_MAX);
            tokio::spawn(poster(self.api.clone(), s.agent.clone(), rx, self.tx.clone(), self.stop.clone()));
            s.poster = Some(tx);
        }
        let tx = s.poster.as_ref().expect("its poster");
        if let Err(e) = tx.try_send(p) {
            // the next start's catch-up reads it again (by the same id)
            crate::ev!("simplex.posts_bounded", { "agent": f, "max": POSTS_MAX });
            let item = e.into_inner().inbound.item;
            self.dropped(f, item);
        }
    }

    /// The agents whose link is bound, for the bridge's engine.
    fn publish_relaying(&self) {
        let now: BTreeSet<String> = self.slots.iter().filter(|(_, s)| matches!(s.saved.link.phase, Phase::Linked { .. })).map(|(f, _)| f.clone()).collect();
        self.relaying.send_if_modified(|was| {
            if *was == now {
                false
            } else {
                *was = now;
                true
            }
        });
    }

    fn exited(&mut self, f: &str, epoch: u64, why: &str) {
        let Some(s) = self.slots.get_mut(f) else { return };
        if s.epoch != epoch {
            return;
        }
        let planned = s.core.as_ref().is_none_or(|c| c.planned);
        crate::ev!("simplex.core_exited", { "agent": f, "epoch": epoch, "planned": planned, "why": why });
        if planned {
            return;
        }
        s.core = None;
        s.up = false;
        s.pending.clear();
        self.count_exit(f, why);
    }

    /// An exit the bridge did not ask for: tried again after a while,
    /// doubling, until `EXITS_MAX` in the window.
    fn count_exit(&mut self, f: &str, why: &str) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let now = Instant::now();
        s.exits.push_back(now);
        while s.exits.front().is_some_and(|t| now.duration_since(*t) > Duration::from_millis(EXITS_WINDOW_MS)) {
            s.exits.pop_front();
        }
        if s.exits.len() >= EXITS_MAX {
            s.broken = Some(format!("SimpleX stopped {} times in 10 minutes ({why}), so it waits for you to link again.", s.exits.len()));
        } else {
            s.retry_at = Some(now + Duration::from_millis(RESTART_MS << (s.exits.len() - 1)));
        }
        self.quiet.store(self.running() == 0, Ordering::SeqCst);
    }

    /// The link's rules on `input` for agent `f`: its file written if it
    /// changed, then what it asks.
    fn apply(&mut self, f: &str, input: Input<'_>) {
        let Some(s) = self.slots.get_mut(f) else { return };
        let before = s.saved.clone();
        let actions = s.saved.link.apply(input);
        if s.saved != before {
            if let Err(e) = store(&s.dir, &s.saved) {
                // the link moves on; a restart before it is written may go back
                crate::ev!("simplex.store_failed", { "agent": f, "error": e.to_string() });
            }
        }
        // a link let go (unlinked, by either side) keeps no door key
        if s.saved.link.phase == Phase::Off && s.door.take().is_some() {
            Key::forget(&s.dir);
            s.session = None;
            crate::ev!("simplex.door_forgotten", { "agent": f });
        }
        for a in actions {
            self.act(f, a);
        }
        self.publish_relaying();
    }

    fn act(&mut self, f: &str, a: Action) {
        let Some(s) = self.slots.get_mut(f) else { return };
        match a {
            Action::Ask(cmd) => {
                let Some(core) = s.core.as_ref() else {
                    crate::ev!("simplex.unsent", { "agent": f, "cmd": kind_of(&cmd), "why": "its core does not run" });
                    return;
                };
                s.corr += 1;
                let corr = s.corr;
                match core.writer.try_send(cmd.frame(corr)) {
                    Ok(()) => {
                        s.pending.insert(corr, cmd);
                    }
                    Err(_) => crate::ev!("simplex.unsent", { "agent": f, "cmd": kind_of(&cmd), "why": "its core is not taking commands" }),
                }
            }
            Action::Forward(i) => {
                crate::ev!("simplex.forward", { "agent": f, "kind": i.kind, "bytes": i.text.len(), "file": i.file.is_some() });
                if i.item > 0 {
                    s.outstanding.insert(i.item);
                }
                self.forward(f, i);
            }
            Action::Tell(n) => {
                match &n {
                    Notice::Linked { .. } => s.note = None,
                    Notice::Refused => crate::ev!("simplex.refused_contact", { "agent": f }),
                    Notice::MaybeLost | Notice::Resynced => {}
                    Notice::Unlinked { by_them } => {
                        s.note = Some(if *by_them { "They removed this agent in their SimpleX app.".into() } else { "Unlinked.".into() });
                    }
                    Notice::Failed { what } => s.note = Some(format!("SimpleX said: {what}")),
                }
                crate::ev!("simplex.told", { "agent": f, "notice": format!("{n:?}") });
                let name = display_name(s.title.as_deref(), &s.agent.name);
                match n {
                    Notice::Linked { .. } => self.apply(f, Input::Reply(&greeting(&name))),
                    Notice::MaybeLost => self.apply(f, Input::Reply(MAYBE_LOST)),
                    _ => {}
                }
            }
            Action::Save => {
                let _ = self.pulse.send(true);
                self.pulse_until = Some(Instant::now() + Duration::from_millis(PULSE_MS));
                crate::ev!("simplex.save_asked", { "agent": f });
            }
        }
    }
}

/// Every core's process the bridge runs, by pid, from its spawn until it is
/// reaped: what the bridge stops before it exits (`stop_cores`), so no core
/// outlives it (a bridge started again would run a second core on the same
/// databases, outside its hold).
static CORES: std::sync::Mutex<std::collections::BTreeSet<u32>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

fn cores_alive() -> Vec<u32> {
    CORES.lock().expect("not poisoned").iter().copied().collect()
}

/// Every core this process runs stopped before the bridge exits (main.rs),
/// whatever its manager is doing: SIGTERM, `grace` to end, then SIGKILL,
/// until each is reaped (`grace` and a second more, at most).
pub async fn stop_cores(grace: Duration) {
    if cores_alive().is_empty() {
        return;
    }
    let t0 = Instant::now();
    for pid in cores_alive() {
        // SAFETY: a signal to a core this process spawned and has not reaped
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    // bounded by `grace`, then a second
    while !cores_alive().is_empty() && t0.elapsed() < grace {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    kill_cores();
    while !cores_alive().is_empty() && t0.elapsed() < grace + Duration::from_millis(1_000) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    crate::ev!("simplex.cores_stopped", { "ms": t0.elapsed().as_millis() as u64, "left": cores_alive().len() });
}

/// Any core still running as this computer's from a bridge before this one
/// (one whose death its child outlived: the image before this one, which
/// set no parent-death signal), killed before this bridge answers a hold or
/// starts a core of its own: a process whose command line names `cfg.bin`
/// and a `-d` under `cfg.dir`. Bounded: two seconds for each to end.
pub async fn kill_strays(cfg: &Config) {
    let bin = cfg.bin.to_string_lossy().to_string();
    let me = std::process::id();
    let mut strays: Vec<u32> = vec![];
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()).filter(|p| *p != me) else { continue };
        let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).to_string()).collect();
        let names_bin = args.contains(&bin);
        // under it by path components: a sibling that shares its prefix (`simplex-other`) is not
        let in_dir = args.windows(2).any(|w| w[0] == "-d" && Path::new(&w[1]).starts_with(&cfg.dir));
        if names_bin && in_dir {
            strays.push(pid);
        }
    }
    if strays.is_empty() {
        return;
    }
    for pid in &strays {
        // SAFETY: SIGKILL to a stray core of this computer's, by its pid
        unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
    }
    let alive = |pid: u32| std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit_once(')').is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z')));
    let t0 = Instant::now();
    // bounded: two seconds
    while strays.iter().any(|p| alive(*p)) && t0.elapsed() < Duration::from_millis(2_000) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    crate::ev!("simplex.strays_killed", { "pids": strays, "left": strays.iter().filter(|p| alive(**p)).count() });
}

/// SIGKILL to every core still running (the bridge's forced exit).
pub fn kill_cores() {
    for pid in cores_alive() {
        // SAFETY: a signal to a core this process spawned and has not reaped
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

/// One agent's poster: each of its owner's messages, in order, as a
/// relayed record on its Bot Chat (looked for until found), its file
/// uploaded first and let go once posted.
async fn poster(api: Api, agent: Agent, mut rx: mpsc::Receiver<Post>, manager: mpsc::Sender<Msg>, stop: watch::Receiver<bool>) {
    let mut chat: Option<String> = None;
    // bounded by the bridge's life: one message per pass
    while let Some(p) = rx.recv().await {
        let t0 = Instant::now();
        // bounded: a look every CHAT_LOOK_MS until found, or the bridge stops
        while chat.is_none() {
            match api.fragments_for(&agent.fragment, &agent.owner).await {
                Ok(list) => chat = api::bot_chat(&agent.fragment, &list),
                Err(e) => crate::ev!("simplex.chat_unread", { "agent": agent.fragment, "error": e.to_string() }),
            }
            if chat.is_some() {
                break;
            }
            crate::ev!("simplex.no_chat", { "agent": agent.fragment, "waitedMs": t0.elapsed().as_millis() as u64 });
            let mut stop = stop.clone();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(CHAT_LOOK_MS)) => {}
                _ = crate::net::stopped(&mut stop) => return,
            }
        }
        let chat = chat.clone().expect("found");
        let text = match &p.note {
            Some(note) if p.inbound.text.trim().is_empty() => note.clone(),
            Some(note) => format!("{}\n\n{note}", p.inbound.text),
            None => p.inbound.text.clone(),
        };
        let body = records::relayed_message(&text, &agent, records::VIA_SIMPLEX, &[]);
        let id = records::relayed_id(&agent.fragment, &p.inbound.id);
        let files: Vec<LocalFile> = p.file.iter().cloned().collect();
        match crate::driver::post(&api, &chat, &agent.fragment, records::CHAT, &id, body, &files, Some(&p.door), stop.clone()).await {
            crate::driver::Sent::Stopped => return,
            // its file kept: the message is read again at the next start's
            // catch-up (by the same id), and carried then if it can be
            crate::driver::Sent::Refused => {
                crate::ev!("simplex.unposted", { "agent": agent.fragment, "chat": chat, "id": id, "kind": p.inbound.kind, "file": p.file.is_some() });
                let _ = manager.send(Msg::Delivered { agent: agent.fragment.clone(), item: p.inbound.item, ok: false }).await;
            }
            crate::driver::Sent::Posted => {
                crate::ev!("simplex.posted", { "agent": agent.fragment, "chat": chat, "id": id, "kind": p.inbound.kind, "file": p.file.is_some() });
                for f in &files {
                    let _ = tokio::fs::remove_file(&f.path).await;
                }
                let _ = manager.send(Msg::Delivered { agent: agent.fragment.clone(), item: p.inbound.item, ok: true }).await;
            }
        }
    }
}

/// SIGTERM, then (past `STOP_MS_MAX`) SIGKILL; until its process ended.
async fn stop_process(f: &str, mut core: Core) {
    let t0 = Instant::now();
    // SAFETY: signals to the core's own process, by its pid, while it lives
    unsafe { libc::kill(core.pid as i32, libc::SIGTERM) };
    let ended = tokio::time::timeout(Duration::from_millis(STOP_MS_MAX), core.exited.wait_for(|e| *e)).await.is_ok();
    if !ended {
        unsafe { libc::kill(core.pid as i32, libc::SIGKILL) };
        let _ = tokio::time::timeout(Duration::from_millis(1_000), core.exited.wait_for(|e| *e)).await;
    }
    crate::ev!("simplex.core_stopped", { "agent": f, "ms": t0.elapsed().as_millis() as u64, "killed": !ended });
}

fn kind_of(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Invite => "invite",
        Cmd::Send { .. } => "send",
        Cmd::Sync { force: true, .. } => "sync_forced",
        Cmd::Sync { .. } => "sync",
        Cmd::Delete { .. } => "delete",
        Cmd::DeleteConn { .. } => "delete_conn",
        Cmd::Chat { .. } => "chat",
        Cmd::Receive { .. } => "receive",
        Cmd::CancelFile { .. } => "cancel_file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::Digest;

    fn agent() -> Agent {
        Agent { fragment: "juniper--k3x9".into(), identity: "npub1juniper".into(), name: "juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None }
    }

    fn computer(always_on: bool, generation: u64, restored: Option<(u64, bool)>) -> api::Computer {
        api::Computer {
            computer: "computer:00aa".into(),
            owner: "npub1paul".into(),
            image: String::new(),
            agents: vec![agent()],
            credential_env: vec![],
            relay: None,
            models: None,
            always_on,
            generation,
            restored: restored.map(|(generation, rollback)| api::Restored { generation, rollback }),
        }
    }

    /// Goal: a file never received (the core retries a server it cannot
    /// reach for good, and says nothing) is given up on after
    /// `FILE_WAIT_MS`: its receiving cancelled at the core and its message
    /// posted once, with a note, never left waiting silently. One received
    /// in time is posted with its file; a catch-up's forward of one waiting
    /// already waits for it once; one given up on that completes anyway is
    /// removed, never posted. Method: the manager's own methods, its core's
    /// commands and its poster's posts captured (Alex's review).
    #[tokio::test]
    async fn a_file_that_never_arrives_is_said_not_to() {
        let dir = std::env::temp_dir().join(format!("simplex-overdue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = agent_key(&dir.join("key"));
        let mut m = manager(&dir, &key, Phase::Linked { contact: 3, name: "paul".into() });
        let f = "juniper--k3x9";
        let (writer, mut sent) = mpsc::channel(16);
        let (posts_tx, mut posts) = mpsc::channel(16);
        {
            let s = m.slots.get_mut(f).expect("its slot");
            s.door = Some(key.clone());
            s.poster = Some(posts_tx);
            s.core = Some(Core { pid: 0, writer, exited: watch::channel(false).1, planned: false });
            s.up = true;
            std::fs::create_dir_all(s.dir.join("files")).expect("its files");
        }
        let cmds = |sent: &mut mpsc::Receiver<String>| std::iter::from_fn(|| sent.try_recv().ok()).map(|fr| serde_json::from_str::<serde_json::Value>(&fr).expect("a frame")["cmd"].as_str().expect("a line").to_string()).collect::<Vec<_>>();
        let voice = |item: i64| Inbound { id: format!("m{item}"), kind: "voice".into(), text: String::new(), at: "2026-10-25T17:00:00Z".into(), item, file: Some(wire::FileRef { id: item, name: "voice.m4a".into(), size: 9_000, path: None }) };
        let t0 = Instant::now();
        m.forward(f, voice(7));
        m.forward(f, voice(7));
        m.forward(f, voice(8));
        assert_eq!(m.slots[f].files.len(), 2, "7 forwarded twice waits once");
        assert_eq!(cmds(&mut sent), ["/freceive 7", "/freceive 7", "/freceive 8"]);
        m.files_overdue(t0 + Duration::from_millis(FILE_WAIT_MS / 2));
        assert_eq!(m.slots[f].files.len(), 2, "within its wait: still waiting");
        assert!(cmds(&mut sent).is_empty() && posts.try_recv().is_err(), "nothing asked or posted within it");
        // 8 arrives in time: posted with its file
        m.file_event(f, &Resp::FileDone { file: 8, path: "voice_8.m4a".into() });
        let p8 = posts.try_recv().expect("8 posted");
        assert!(p8.inbound.item == 8 && p8.file.is_some() && p8.note.is_none());
        // 7 never does: cancelled at the core, posted once with a note
        m.files_overdue(t0 + Duration::from_millis(FILE_WAIT_MS + 1_000));
        assert!(m.slots[f].files.is_empty());
        assert_eq!(cmds(&mut sent), ["/fcancel 7"]);
        let p7 = posts.try_recv().expect("7 posted");
        assert_eq!((p7.inbound.item, p7.file.is_none(), p7.note.as_deref()), (7, true, Some("(I sent a voice message over SimpleX that did not arrive.)")));
        m.files_overdue(t0 + Duration::from_millis(2 * FILE_WAIT_MS));
        assert!(posts.try_recv().is_err() && cmds(&mut sent).is_empty(), "given up on once");
        // its transfer completes anyway (the cancel raced it): removed, not posted
        let late = m.slots[f].dir.join("files").join("voice_7.m4a");
        std::fs::write(&late, b"late").expect("a late file");
        m.file_event(f, &Resp::FileDone { file: 7, path: "voice_7.m4a".into() });
        assert!(!late.exists(), "a file given up on that came anyway is removed");
        assert!(posts.try_recv().is_err(), "and not posted again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A manager with juniper's core up as epoch 1, its commands and its
    /// poster's posts captured, its files folder made; the core's writer
    /// holds `room` frames.
    fn captured(dir: &Path, room: usize) -> (Manager, mpsc::Receiver<String>, mpsc::Receiver<Post>) {
        let _ = std::fs::remove_dir_all(dir);
        let key = agent_key(&dir.join("key"));
        let mut m = manager(dir, &key, Phase::Linked { contact: 3, name: "paul".into() });
        let (writer, sent) = mpsc::channel(room);
        let (posts_tx, posts) = mpsc::channel(16);
        let s = m.slots.get_mut("juniper--k3x9").expect("its slot");
        s.door = Some(key.clone());
        s.poster = Some(posts_tx);
        s.core = Some(Core { pid: 0, writer, exited: watch::channel(false).1, planned: false });
        s.up = true;
        s.epoch = 1;
        std::fs::create_dir_all(s.dir.join("files")).expect("its files");
        (m, sent, posts)
    }

    fn voice(item: i64, path: Option<&str>) -> Inbound {
        Inbound { id: format!("m{item}"), kind: "voice".into(), text: String::new(), at: "2026-10-25T17:00:00Z".into(), item, file: Some(wire::FileRef { id: item, name: "voice.m4a".into(), size: 9_000, path: path.map(str::to_string) }) }
    }

    /// The core's commands sent so far: (corrId, line).
    fn sent_lines(sent: &mut mpsc::Receiver<String>) -> Vec<(String, String)> {
        std::iter::from_fn(|| sent.try_recv().ok())
            .map(|fr| {
                let v: serde_json::Value = serde_json::from_str(&fr).expect("a frame");
                (v["corrId"].as_str().expect("its corrId").to_string(), v["cmd"].as_str().expect("its line").to_string())
            })
            .collect()
    }

    /// Goal (Alex's review of #610): only a file the bridge gave up on is
    /// ever deleted. A transfer a restart resumed, completing before its
    /// message's catch-up, keeps its file, and the catch-up carries it; a
    /// completion said again never deletes the file a post is carrying;
    /// one given up on that comes anyway is deleted, and its message's
    /// catch-up posts the note again, never the file.
    #[tokio::test]
    async fn only_a_file_given_up_on_is_ever_deleted() {
        let dir = std::env::temp_dir().join(format!("simplex-abandon-{}", std::process::id()));
        let (mut m, mut sent, mut posts) = captured(&dir, 16);
        let f = "juniper--k3x9";
        let files = m.slots[f].dir.join("files");
        // resumed after a restart: done before its message is forwarded again
        std::fs::write(files.join("voice_9.m4a"), b"resumed").expect("a file");
        m.file_event(f, &Resp::FileDone { file: 9, path: "voice_9.m4a".into() });
        assert!(files.join("voice_9.m4a").exists(), "not given up on: kept");
        m.forward(f, voice(9, Some("voice_9.m4a")));
        let p = posts.try_recv().expect("posted");
        assert!(p.inbound.item == 9 && p.file.is_some() && p.note.is_none(), "its catch-up carries it");
        // done, posted with its file, then done again
        m.forward(f, voice(8, None));
        std::fs::write(files.join("voice_8.m4a"), b"queued").expect("a file");
        m.file_event(f, &Resp::FileDone { file: 8, path: "voice_8.m4a".into() });
        assert!(posts.try_recv().is_ok_and(|p| p.inbound.item == 8 && p.file.is_some()));
        m.file_event(f, &Resp::FileDone { file: 8, path: "voice_8.m4a".into() });
        assert!(files.join("voice_8.m4a").exists(), "a completion said again never deletes what a post carries");
        assert!(posts.try_recv().is_err(), "nor posts it again");
        // given up on, then done anyway: deleted; its catch-up posts the note
        let t0 = Instant::now();
        m.forward(f, voice(7, None));
        m.files_overdue(t0 + Duration::from_millis(FILE_WAIT_MS + 1_000));
        assert!(posts.try_recv().is_ok_and(|p| p.inbound.item == 7 && p.note.is_some()));
        std::fs::write(files.join("voice_7.m4a"), b"late").expect("a file");
        m.file_event(f, &Resp::FileDone { file: 7, path: "voice_7.m4a".into() });
        assert!(!files.join("voice_7.m4a").exists(), "one given up on is deleted as it comes");
        m.forward(f, voice(7, Some("voice_7.m4a")));
        let p = posts.try_recv().expect("posted again by its catch-up");
        assert!(p.file.is_none() && p.note.is_some(), "its note again, never a file it deleted");
        assert_eq!(load(&m.slots[f].dir, &agent()).expect("its file").abandoned.get(&7), Some(&Abandoned::Settled), "kept in its link's file");
        drop(sent_lines(&mut sent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goal (Alex's review of #610): a file's cancel is kept until its core
    /// takes it: with no core, with its writer full, after an answer that
    /// says to try again, and across a restart, it is asked again (never
    /// twice at once), and an answer that settles it ends the asking; its
    /// message is posted with its note once, whatever the asking.
    #[tokio::test]
    async fn a_cancel_is_kept_until_the_core_takes_it() {
        let dir = std::env::temp_dir().join(format!("simplex-cancel-{}", std::process::id()));
        let (mut m, mut sent, mut posts) = captured(&dir, 1);
        let f = "juniper--k3x9";
        let mut t = Instant::now();
        let retry = Duration::from_millis(CANCEL_RETRY_MS + 1);
        // given up on while its core is down (a hold): kept, not sent
        m.forward(f, voice(7, None));
        drop(sent_lines(&mut sent));
        let core = m.slots.get_mut(f).expect("its slot").core.take();
        m.slots.get_mut(f).expect("its slot").up = false;
        t += Duration::from_millis(FILE_WAIT_MS + 1_000);
        m.files_overdue(t);
        assert!(posts.try_recv().is_ok_and(|p| p.inbound.item == 7 && p.note.is_some()), "its note, once");
        assert_eq!(m.slots[f].saved.abandoned.get(&7), Some(&Abandoned::Cancelling));
        // its core back, its writer full: not sent, still kept
        {
            let s = m.slots.get_mut(f).expect("its slot");
            s.core = core;
            s.up = true;
            s.core.as_ref().expect("a core").writer.try_send("filler".into()).expect("the writer filled");
        }
        m.files_overdue(t);
        assert_eq!(sent.try_recv().ok().as_deref(), Some("filler"), "nothing but the filler");
        assert_eq!(m.slots[f].saved.abandoned.get(&7), Some(&Abandoned::Cancelling));
        // asked again past its pace; the core says to try again
        t += retry;
        m.files_overdue(t);
        let asked = sent_lines(&mut sent);
        assert_eq!(asked.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/fcancel 7"]);
        m.files_overdue(t + retry);
        assert!(sent_lines(&mut sent).is_empty(), "never asked twice while one is unanswered");
        let again = format!(r#"{{"corrId":"{}","resp":{{"type":"chatCmdError","chatError":{{"type":"errorAgent","agentError":{{"type":"BROKER","brokerErr":{{"type":"NETWORK"}}}}}}}}}}"#, asked[0].0);
        m.frame(f, 1, &again);
        assert_eq!(m.slots[f].saved.abandoned.get(&7), Some(&Abandoned::Cancelling), "an agent's failure is tried again");
        // a restart reads it back, and asks it again once its core runs
        let saved = load(&m.slots[f].dir, &agent()).expect("its file");
        assert_eq!(saved.abandoned.get(&7), Some(&Abandoned::Cancelling), "kept across a restart");
        t += retry;
        m.files_overdue(t);
        let asked = sent_lines(&mut sent);
        assert_eq!(asked.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["/fcancel 7"]);
        // an answer that settles it: asked no more
        let done = format!(r#"{{"corrId":"{}","resp":{{"type":"chatCmdError","chatError":{{"type":"errorStore","storeError":{{"type":"fileNotFound","fileId":7}}}}}}}}"#, asked[0].0);
        m.frame(f, 1, &done);
        assert_eq!(m.slots[f].saved.abandoned.get(&7), Some(&Abandoned::Settled));
        m.files_overdue(t + retry * 3);
        assert!(sent_lines(&mut sent).is_empty(), "settled: asked no more");
        assert!(posts.try_recv().is_err(), "its note was posted once, whatever the asking");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goal (Alex's review of #612): no cancel still to be taken is ever let
    /// go of. With every kept file's cancel unsettled (the core failing each
    /// one), a new file is not received at all (nothing asked of the core,
    /// its message posted with its note), the oldest cancel stays in its
    /// link's file across a restart and is asked again, and a late
    /// completion of it is deleted, which settles it and lets a new file in.
    #[tokio::test]
    async fn no_unsettled_cancel_is_let_go() {
        let dir = std::env::temp_dir().join(format!("simplex-unsettled-{}", std::process::id()));
        let (mut m, mut sent, mut posts) = captured(&dir, 4 * ABANDONED_MAX);
        let f = "juniper--k3x9";
        let mut t = Instant::now();
        let retry = Duration::from_millis(CANCEL_RETRY_MS + 1);
        let failed = |corr: &str| format!(r#"{{"corrId":"{corr}","resp":{{"type":"chatCmdError","chatError":{{"type":"errorDatabase","databaseError":{{"type":"errorSQL"}}}}}}}}"#);
        // every kept file given up on, in batches of what may wait, each cancel failing
        let mut next = 1;
        while next <= ABANDONED_MAX as i64 {
            let batch: Vec<i64> = (next..(next + FILES_MAX as i64).min(ABANDONED_MAX as i64 + 1)).collect();
            next += batch.len() as i64;
            for id in &batch {
                m.forward(f, voice(*id, None));
            }
            t += Duration::from_millis(FILE_WAIT_MS + 1_000);
            m.files_overdue(t);
            for (corr, line) in sent_lines(&mut sent) {
                if line.starts_with("/fcancel") {
                    m.frame(f, 1, &failed(&corr));
                }
            }
        }
        let unsettled = |m: &Manager| m.slots[f].saved.abandoned.values().filter(|a| **a == Abandoned::Cancelling).count();
        assert_eq!(unsettled(&m), ABANDONED_MAX, "every cancel kept, none settled");
        while posts.try_recv().is_ok() {}
        // one more: not received, posted with its note at once, nothing let go
        m.forward(f, voice(1_000, None));
        assert!(sent_lines(&mut sent).is_empty(), "nothing asked of the core for it");
        assert!(posts.try_recv().is_ok_and(|p| p.inbound.item == 1_000 && p.note.is_some() && p.file.is_none()));
        assert!(m.slots[f].files.is_empty() && !m.slots[f].saved.abandoned.contains_key(&1_000));
        assert_eq!(m.slots[f].saved.abandoned.get(&1), Some(&Abandoned::Cancelling), "the oldest still kept");
        // a restart reads them all back, the oldest among them, and asks it again
        let saved = load(&m.slots[f].dir, &agent()).expect("its file");
        assert_eq!((saved.abandoned.len(), saved.abandoned.get(&1)), (ABANDONED_MAX, Some(&Abandoned::Cancelling)));
        t += retry;
        m.files_overdue(t);
        let asked: Vec<String> = sent_lines(&mut sent).into_iter().map(|(_, l)| l).collect();
        assert!(asked.contains(&"/fcancel 1".to_string()) && asked.len() == ABANDONED_MAX, "every unsettled cancel asked again, the oldest too: {}", asked.len());
        // the oldest arrives anyway: deleted, settled, and a new file is let in
        let late = m.slots[f].dir.join("files").join("voice_1.m4a");
        std::fs::write(&late, b"late").expect("a file");
        m.file_event(f, &Resp::FileDone { file: 1, path: "voice_1.m4a".into() });
        assert!(!late.exists(), "its late file deleted");
        assert_eq!(m.slots[f].saved.abandoned.get(&1), Some(&Abandoned::Settled));
        m.forward(f, voice(1_001, None));
        assert_eq!(sent_lines(&mut sent).into_iter().map(|(_, l)| l).collect::<Vec<_>>(), ["/freceive 1001"], "room again: it is received");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goal (Alex's review of #612): a receive asked before a restart keeps
    /// its place after it. With the link one short of its bound (63 cancels
    /// unsettled) and one receive under way, then a restart (the core
    /// resumes it from its own database; the bridge's waits are gone), the
    /// receive is waited for again and carried, whichever comes first: a
    /// newer message's file (held back: its note) or the older message's
    /// catch-up.
    #[tokio::test]
    async fn a_receive_asked_before_a_restart_keeps_its_place() {
        for live_first in [true, false] {
            let dir = std::env::temp_dir().join(format!("simplex-resumed-{}-{live_first}", std::process::id()));
            let (mut m, mut sent, mut posts) = captured(&dir, 4 * ABANDONED_MAX);
            let f = "juniper--k3x9";
            let mut t = Instant::now();
            let failed = |corr: &str| format!(r#"{{"corrId":"{corr}","resp":{{"type":"chatCmdError","chatError":{{"type":"errorDatabase","databaseError":{{"type":"errorSQL"}}}}}}}}"#);
            // 63 cancels unsettled
            let mut next = 1;
            while next < ABANDONED_MAX as i64 {
                let batch: Vec<i64> = (next..(next + FILES_MAX as i64).min(ABANDONED_MAX as i64)).collect();
                next += batch.len() as i64;
                for id in &batch {
                    m.forward(f, voice(*id, None));
                }
                t += Duration::from_millis(FILE_WAIT_MS + 1_000);
                m.files_overdue(t);
                for (corr, line) in sent_lines(&mut sent) {
                    if line.starts_with("/fcancel") {
                        m.frame(f, 1, &failed(&corr));
                    }
                }
            }
            // one receive under way: the last place
            m.forward(f, voice(65, None));
            assert!(sent_lines(&mut sent).iter().any(|(_, l)| l == "/freceive 65"));
            while posts.try_recv().is_ok() {}
            // a restart: its waits gone, its link's file read back
            {
                let s = m.slots.get_mut(f).expect("its slot");
                s.files.clear();
                s.pending.clear();
                s.saved = load(&s.dir, &agent()).expect("its file");
                assert!(s.saved.receiving.contains(&65), "the receive under way, kept");
            }
            let order: [i64; 2] = if live_first { [66, 65] } else { [65, 66] };
            for item in order {
                m.forward(f, voice(item, None));
            }
            let asked: Vec<String> = sent_lines(&mut sent).into_iter().map(|(_, l)| l).collect();
            assert_eq!(asked, ["/freceive 65"], "live first {live_first}: 65 waited for again, 66 not received");
            let p = posts.try_recv().expect("66 posted");
            assert!(p.inbound.item == 66 && p.note.is_some() && p.file.is_none(), "live first {live_first}: the newer file held back, with its note");
            assert!(m.slots[f].files.contains_key(&65), "65 waited for");
            // the resumed receive completes: carried, its place let go
            std::fs::write(m.slots[f].dir.join("files").join("voice_65.m4a"), b"resumed").expect("a file");
            m.file_event(f, &Resp::FileDone { file: 65, path: "voice_65.m4a".into() });
            let p = posts.try_recv().expect("65 posted");
            assert!(p.inbound.item == 65 && p.file.is_some() && p.note.is_none(), "live first {live_first}: carried with its file");
            assert!(m.slots[f].dir.join("files").join("voice_65.m4a").exists(), "and kept for its post");
            assert!(load(&m.slots[f].dir, &agent()).expect("its file").receiving.is_empty(), "its place let go");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Goal: what settles a cancel. The core took it, or refused it for
    /// good (a store or a chat error: the file unknown, done, cancelled);
    /// its agent's or database's failures are tried again.
    #[test]
    fn what_settles_a_cancel() {
        for (answer, settled) in [
            (Resp::Other("rcvFileCancelled".into()), true),
            (Resp::Error { what: "errorStore fileNotFound".into() }, true),
            (Resp::Error { what: "error fileAlreadyReceiving".into() }, true),
            (Resp::Error { what: "errorAgent BROKER NETWORK NETLSError".into() }, false),
            (Resp::Error { what: "errorDatabase errorDatabase".into() }, false),
        ] {
            assert_eq!(cancel_settled(&answer), settled, "{answer:?}");
        }
    }

    /// Goal: a core reaches SMP servers on their own port, never on 443
    /// (Cloudflare Containers reset a TLS hello there that names no
    /// server), with the presets or a test's servers alike.
    #[test]
    fn a_core_reaches_smp_servers_off_443() {
        let words = |cfg: &Config| core_args(cfg, Path::new("/data/simplex/a"), 5225, "Juniper", Some(40123)).into_iter().map(|a| a.into_string().expect("text")).collect::<Vec<_>>();
        let presets = Config { bin: PathBuf::from("/nowhere"), dir: PathBuf::from("/data/simplex"), smp: None, xftp: None };
        let lab = Config { smp: Some("smp://fp@smp.lab".into()), xftp: Some("xftp://fp@xftp.lab".into()), ..presets.clone() };
        for args in [words(&presets), words(&lab)] {
            assert!(args.windows(2).any(|w| w == ["--smp-web-port-servers", "off"]), "{args:?}");
            assert!(args.windows(2).any(|w| w == ["-p", "5225"]) && args.windows(2).any(|w| w == ["-d", "/data/simplex/a/sx"]), "{args:?}");
        }
        assert!(!words(&presets).contains(&"-s".to_string()), "the presets: no server named");
        for args in [words(&presets), words(&lab)] {
            assert!(args.windows(2).any(|w| w == ["--socks-proxy", "127.0.0.1:40123"]) && args.windows(2).any(|w| w == ["--host-mode", "public"]), "through the bridge's proxy, by name: {args:?}");
        }
        let unproxied = core_args(&presets, Path::new("/d"), 1, "J", None).into_iter().map(|a| a.into_string().expect("text")).collect::<Vec<_>>();
        assert!(!unproxied.iter().any(|a| a.starts_with("--socks") || a == "--host-mode"), "no proxy bound: straight out");
        assert!(words(&lab).windows(2).any(|w| w == ["-s", "smp://fp@smp.lab"]));
    }

    /// Goal: a start went back in time only when the platform says this
    /// start did and the link last ran in an earlier one: a bridge started
    /// again in the same start forces nothing twice, and a platform that
    /// says nothing (an old one) forces nothing.
    #[test]
    fn a_rollback_is_this_starts_and_once() {
        let saved = |generation| Saved { identity: "npub1juniper".into(), generation, link: Link::default(), used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() };
        assert!(rolled_back(&computer(true, 5, Some((5, true))), &saved(4)));
        assert!(!rolled_back(&computer(true, 5, Some((5, true))), &saved(5)), "this start's, handled already");
        assert!(!rolled_back(&computer(true, 5, Some((4, true))), &saved(3)), "an older start's restore");
        assert!(!rolled_back(&computer(true, 5, Some((5, false))), &saved(4)), "a sleep that saved");
        assert!(!rolled_back(&computer(true, 0, None), &saved(0)), "a platform that says nothing");
    }

    /// Goal: a link's file reads back as written; one of another agent, or
    /// one that does not read, is set aside (kept, renamed), never guessed.
    #[test]
    fn a_links_file_is_its_agents_or_set_aside() {
        let root = std::env::temp_dir().join(format!("simplex-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("juniper--k3x9");
        assert_eq!(load(&dir, &agent()), None, "none yet");
        let saved = Saved { identity: "npub1juniper".into(), generation: 7, link: Link { phase: Phase::Linked { contact: 3, name: "paul".into() }, ..Link::default() }, used: BTreeMap::from([("0f".repeat(16), 5)]), abandoned: BTreeMap::from([(7, Abandoned::Cancelling)]), receiving: BTreeSet::from([9]) };
        store(&dir, &saved).expect("written");
        assert_eq!(load(&dir, &agent()), Some(saved.clone()));
        assert_eq!(linked_on_disk(&root, &[agent()]), vec!["juniper--k3x9".to_string()]);
        let other = Agent { identity: "npub1other".into(), ..agent() };
        assert_eq!(load(&dir, &other), None, "another agent's");
        assert!(!dir.exists(), "set aside");
        assert!(std::fs::read_dir(&root).expect("the root").any(|e| e.expect("an entry").file_name().to_string_lossy().starts_with("juniper--k3x9.set-aside-")));
        store(&dir, &saved).expect("written");
        std::fs::write(dir.join(LINK_FILE), r#"{"identity":"npub1juniper","phase":"off","syncing":true}"#).expect("written");
        assert_eq!(load(&dir, &agent()), None, "one that contradicts itself");
        assert!(linked_on_disk(&root, &[agent()]).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal: a file is named what it is, a voice message as SimpleX's apps
    /// record one; a file that cannot be carried says so in plain words.
    #[test]
    fn files_are_what_they_are() {
        assert_eq!(media_type("voice", "voice_2026.m4a"), "audio/mp4");
        assert_eq!(media_type("voice", "noext"), "audio/mp4");
        assert_eq!(media_type("voice", "memo.wav"), "audio/wav", "its name first");
        assert_eq!(media_type("image", "IMG_1.JPG"), "image/jpeg");
        assert_eq!(media_type("file", "report.pdf"), "application/pdf");
        assert_eq!(media_type("file", "x"), "application/octet-stream");
        assert_eq!(file_note("voice", "did not arrive"), "(I sent a voice message over SimpleX that did not arrive.)");
    }

    /// Goal: an agent's SimpleX name is its title, cleaned and cut, else its
    /// label; a fragment's name is a directory only in its own shape.
    #[test]
    fn names_are_clean() {
        assert_eq!(display_name(Some("Waffle"), "pulsar-360"), "Waffle");
        assert_eq!(display_name(Some("  Wa<ff>le! 🧇 "), "x"), "Waffle");
        assert_eq!(display_name(Some("<<>>"), "pulsar-360"), "pulsar-360");
        assert_eq!(display_name(None, ""), "Agent");
        assert_eq!(display_name(Some(&"a".repeat(80)), "x").len(), DISPLAY_NAME_MAX);
        assert!(dir_name_ok("juniper--k3x9"));
        for bad in ["", "../x", "a/b", "Juniper", "a.b"] {
            assert!(!dir_name_ok(bad), "{bad}");
        }
    }

    fn slot(dir: PathBuf, phase: Phase) -> Slot {
        Slot {
            agent: agent(),
            dir,
            saved: Saved { identity: "npub1juniper".into(), generation: 1, link: Link { phase, ..Link::default() }, used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() },
            title: None,
            epoch: 0,
            core: None,
            up: false,
            corr: 0,
            pending: BTreeMap::new(),
            started: false,
            exits: VecDeque::new(),
            retry_at: None,
            resync_at: None,
            broken: None,
            note: None,
            draining_since: None,
            cancel_at: None,
            files: BTreeMap::new(),
            poster: None,
            gap: false,
            outstanding: Default::default(),
            acked: 0,
            door: None,
            session: None,
            linking: None,
        }
    }

    /// Goal: the page says why SimpleX does not run, in plain words, and
    /// shows the invitation only while it is one, and only to the page
    /// whose ticket asked for it (the agent's tools reach this port too).
    #[test]
    fn its_view_says_what_it_is() {
        let mut s = slot(PathBuf::from("/x"), Phase::Inviting { conn: 1, link: "simplex:/invitation#/x".into(), short: None });
        s.session = Some("0f".repeat(16));
        let v = s.view(true, Some(&"0f".repeat(16)));
        assert_eq!((v.phase, v.link.as_deref(), v.hidden, v.running, v.why.as_deref()), ("inviting", Some("simplex:/invitation#/x"), false, false, None));
        for other in [None, Some("1f".repeat(16))] {
            let v = s.view(true, other.as_deref());
            assert_eq!((v.phase, v.link.as_deref(), v.short.as_deref(), v.hidden), ("inviting", None, None, true), "{other:?}: no invitation to another");
        }
        s.session = None;
        assert!(s.view(true, None).link.is_none(), "no page asked: no one is shown it");
        let v = s.view(false, None);
        assert!(v.why.is_some_and(|w| w.contains("always-on")), "not always on: says so");
        s.saved.link.phase = Phase::Linked { contact: 3, name: "paul".into() };
        s.broken = Some("SimpleX stopped 3 times".into());
        let v = s.view(true, None);
        assert_eq!((v.phase, v.link.as_deref(), v.name.as_deref()), ("linked", None, Some("paul")));
        assert_eq!(v.why.as_deref(), Some("SimpleX stopped 3 times"));
        let json = serde_json::to_value(&v).expect("a view");
        assert_eq!((json["alwaysOn"].clone(), json.get("hidden")), (json!(true), None));
    }

    /// A manager over one agent, juniper, whose key is `key`, on
    /// `computer:00aa` (always on), its state in `dir`; nothing started.
    fn manager(dir: &Path, key: &Key, phase: Phase) -> Manager {
        let (tx, _rx) = mpsc::channel(4);
        let (pulse, _) = watch::channel(false);
        let (relaying, _) = watch::channel(BTreeSet::new());
        let (_stop_tx, stop) = watch::channel(false);
        let cfg = Config { bin: PathBuf::from("/nowhere"), dir: dir.to_path_buf(), smp: None, xftp: None };
        let mut s = slot(dir.join("juniper--k3x9"), phase);
        s.agent.key = Some(key.public_hex());
        let api = Api::new("http://127.0.0.1:9").expect("an address");
        Manager { cfg, api, quiet: Arc::new(AtomicBool::new(true)), hold: dir.join("hold"), tx, slots: BTreeMap::from([("juniper--k3x9".to_string(), s)]), computer: Some(computer(true, 1, None)), held: false, pulse, pulse_until: None, relaying, socks: None, offer: watch::channel(None).0, stop }
    }

    /// The vectors' agent key (crates/core/src/door.rs: secret 0x07 × 32),
    /// made the way the bridge reads a door key.
    fn agent_key(dir: &Path) -> Key {
        std::fs::create_dir_all(dir).expect("a directory");
        std::fs::write(dir.join(door::KEY_FILE), "07".repeat(32)).expect("written");
        Key::load(dir).expect("a key")
    }

    /// A ticket for `act` signed by the agent's key, valid for 10 minutes.
    fn signed(act: &str, nonce: &str) -> Ticket {
        let exp = crate::log::now_ms() as i64 + 600_000;
        let mut t = Ticket { door: door::DOOR.into(), act: act.into(), agent: "juniper--k3x9".into(), computer: "computer:00aa".into(), nonce: nonce.into(), exp, sig: "ab".repeat(64) };
        let text = format!("fragment door ticket 1\n{}\n{}\n{}\n{}\n{}\n{}", t.door, t.act, t.agent, t.computer, t.nonce, t.exp);
        let digest: [u8; 32] = sha2::Sha256::digest(text.as_bytes()).into();
        assert_eq!(digest, t.digest(), "the same digest as door.rs");
        t.sig = k256::schnorr::SigningKey::from_bytes(&[7u8; 32]).expect("a key").sign_raw(&digest, &[0u8; 32]).map(|s| crate::records::hex(&s.to_bytes())).expect("signed");
        t
    }

    /// Goal (the signed link start): a link or an unlink is taken only by a
    /// ticket its agent's key signed, for this computer and agent, once
    /// (kept across a restart, in its file); a link's, never while linked.
    #[test]
    fn a_ticket_is_taken_signed_and_once() {
        let root = std::env::temp_dir().join(format!("simplex-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let key = agent_key(&root.join("keys"));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let _in = rt.enter();
        let mut m = manager(&root, &key, Phase::Off);
        let unsigned = Ticket { sig: String::new(), ..signed("link", &"01".repeat(16)) };
        assert_eq!(m.take("juniper--k3x9", &unsigned, "link").map_err(|r| r.status), Err(403), "unsigned");
        let forged = Ticket { sig: "ab".repeat(64), ..signed("link", &"02".repeat(16)) };
        assert_eq!(m.take("juniper--k3x9", &forged, "link").map_err(|r| r.status), Err(403), "forged");
        let t = signed("link", &"03".repeat(16));
        assert_eq!(m.take("juniper--k3x9", &t, "unlink").map_err(|r| r.status), Err(403), "a link's ticket unlinks nothing");
        assert_eq!(m.take("juniper--k3x9", &t, "link"), Ok(()));
        let replayed = m.take("juniper--k3x9", &t, "link").expect_err("replayed");
        assert!(replayed.status == 403 && replayed.why.contains("used already"), "{replayed:?}");
        // a restart reads the nonces it took back from its file
        let saved = load(&root.join("juniper--k3x9"), &agent()).expect("its file");
        assert!(saved.used.contains_key(&t.nonce));
        let mut again = manager(&root, &key, Phase::Off);
        again.slots.get_mut("juniper--k3x9").expect("its slot").saved = saved;
        assert_eq!(again.take("juniper--k3x9", &t, "link").map_err(|r| r.status), Err(403), "replayed after a restart");
        // linked: another link waits for an unlink, which an unlink's ticket takes
        let mut linked = manager(&root.join("linked"), &key, Phase::Linked { contact: 3, name: "paul".into() });
        assert_eq!(linked.take("juniper--k3x9", &signed("link", &"04".repeat(16)), "link").map_err(|r| r.status), Err(409));
        assert_eq!(linked.take("juniper--k3x9", &signed("unlink", &"05".repeat(16)), "unlink"), Ok(()));
        // no key in its view (a platform before doors): nothing is taken
        linked.slots.get_mut("juniper--k3x9").expect("its slot").agent.key = None;
        assert_eq!(linked.take("juniper--k3x9", &signed("unlink", &"06".repeat(16)), "unlink").map_err(|r| r.status), Err(403));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Alex's review of #560): a ticket is used only once it is
    /// written as used (a write that fails refuses it, and acts on
    /// nothing); at the bound a new ticket waits rather than a live one
    /// being forgotten; and a link's key is installed only by the link
    /// taken last, so a registration answered late (another link, or an
    /// unlink, started meanwhile) installs nothing.
    #[test]
    fn a_ticket_is_kept_used_and_only_the_last_link_installs_its_key() {
        let root = std::env::temp_dir().join(format!("simplex-take-bound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let key = agent_key(&root.join("keys"));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let _in = rt.enter();
        let mut m = manager(&root, &key, Phase::Off);
        let dir = root.join("juniper--k3x9");
        // its file cannot be written: refused, nothing taken
        std::fs::create_dir_all(dir.join("link.json.tmp")).expect("a directory where its file goes");
        let t = signed("unlink", &"11".repeat(16));
        assert_eq!(m.take("juniper--k3x9", &t, "unlink").map_err(|r| r.status), Err(500));
        assert!(m.slots["juniper--k3x9"].saved.used.is_empty(), "not taken");
        std::fs::remove_dir_all(dir.join("link.json.tmp")).expect("its place back");
        assert_eq!(m.take("juniper--k3x9", &t, "unlink"), Ok(()));
        assert_eq!(m.take("juniper--k3x9", &t, "unlink").map_err(|r| r.status), Err(403), "then once only");
        // the bound: a new ticket waits; none still holding is forgotten
        let later = crate::log::now_ms() as i64 + 600_000;
        let s = m.slots.get_mut("juniper--k3x9").expect("its slot");
        for i in 0..USED_MAX - 1 {
            s.saved.used.insert(format!("{i:032x}"), later);
        }
        assert_eq!(m.take("juniper--k3x9", &signed("unlink", &"12".repeat(16)), "unlink").map_err(|r| r.status), Err(503));
        assert_eq!(m.take("juniper--k3x9", &t, "unlink").map_err(|r| r.status), Err(403), "the first still refused");
        m.slots.get_mut("juniper--k3x9").expect("its slot").saved.used.clear();
        // two links, their registrations answered in the other order: the later one's key alone
        let (a, b) = (signed("link", &"13".repeat(16)), signed("link", &"14".repeat(16)));
        assert_eq!(m.take("juniper--k3x9", &a, "link"), Ok(()));
        assert_eq!(m.take("juniper--k3x9", &b, "link"), Ok(()));
        let (key_a, key_b) = (Key::generate().expect("a key"), Key::generate().expect("a key"));
        assert!(rt.block_on(m.want("juniper--k3x9", key_b.clone(), b.nonce.clone())).is_ok());
        assert_eq!(rt.block_on(m.want("juniper--k3x9", key_a, a.nonce.clone())).map_err(|r| r.status).err(), Some(409), "the earlier answer, late");
        assert_eq!(Key::load(&dir).map(|k| k.public_hex()), Some(key_b.public_hex()), "the key the platform holds");
        // an unlink while a link registers calls it off
        let c = signed("link", &"15".repeat(16));
        assert_eq!(m.take("juniper--k3x9", &c, "link"), Ok(()));
        assert_eq!(m.take("juniper--k3x9", &signed("unlink", &"16".repeat(16)), "unlink"), Ok(()));
        assert_eq!(rt.block_on(m.want("juniper--k3x9", Key::generate().expect("a key"), c.nonce.clone())).map_err(|r| r.status).err(), Some(409));
        let _ = std::fs::remove_dir_all(&root);
    }
    /// A child as a core is one: registered from its spawn, reaped and
    /// let go by its own task; `sh -c script`. Its pid, and its end.
    fn fake_core(script: &str) -> (u32, watch::Receiver<bool>) {
        let mut child = tokio::process::Command::new("sh").args(["-c", script]).spawn().expect("sh runs");
        let pid = child.id().expect("a pid");
        CORES.lock().expect("not poisoned").insert(pid);
        let (ended, exited) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            CORES.lock().expect("not poisoned").remove(&pid);
            let _ = ended.send(true);
        });
        (pid, exited)
    }

    /// Goal (Alex's review of #546): no core outlives the bridge: one that
    /// ignores SIGTERM is killed past the grace, and reaped, before
    /// `stop_cores` returns (main.rs calls it before it exits).
    #[tokio::test]
    async fn no_core_outlives_the_bridge() {
        let (stubborn, ended) = fake_core("trap '' TERM; sleep 30");
        let (willing, _) = fake_core("sleep 30");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t0 = Instant::now();
        stop_cores(Duration::from_millis(300)).await;
        assert!(t0.elapsed() < Duration::from_millis(1_500), "within the grace and a second: {:?}", t0.elapsed());
        assert!(*ended.borrow(), "the stubborn one ended");
        for pid in [stubborn, willing] {
            assert!(!cores_alive().contains(&pid), "{pid} reaped");
            // SAFETY: signal 0 only asks whether the pid exists
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1, "{pid} is gone");
        }
    }

    /// Goal (Alex's review of #546): a core whose bot API closed while its
    /// process lives on is stopped and counted as an exit, so it is started
    /// again within the bound, not left running unheard.
    #[tokio::test]
    async fn a_core_whose_api_closed_is_stopped_and_tried_again() {
        let (pid, exited) = fake_core("sleep 30");
        let (writer, _writes) = mpsc::channel(1);
        let (tx, _rx) = mpsc::channel(4);
        let (pulse, _) = watch::channel(false);
        let saved = Saved { identity: "npub1juniper".into(), generation: 1, link: Link { phase: Phase::Linked { contact: 3, name: "paul".into() }, ..Link::default() }, used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() };
        let slot = Slot {
            agent: agent(),
            dir: PathBuf::from("/nowhere"),
            saved,
            title: None,
            epoch: 4,
            core: Some(Core { pid, writer, exited: exited.clone(), planned: false }),
            up: true,
            corr: 0,
            pending: BTreeMap::new(),
            started: true,
            exits: VecDeque::new(),
            retry_at: None,
            resync_at: None,
            broken: None,
            note: None,
            draining_since: None,
            cancel_at: None,
            files: BTreeMap::new(),
            poster: None,
            gap: false,
            outstanding: Default::default(),
            acked: 0,
            door: None,
            session: None,
            linking: None,
        };
        let cfg = Config { bin: PathBuf::from("/nowhere"), dir: PathBuf::from("/nowhere"), smp: None, xftp: None };
        let mut m = Manager { cfg, api: Api::new("http://127.0.0.1:9").expect("an address"), quiet: Arc::new(AtomicBool::new(false)), hold: PathBuf::from("/nowhere/hold"), tx, slots: BTreeMap::from([("juniper--k3x9".to_string(), slot)]), computer: Some(computer(true, 1, None)), held: false, pulse, pulse_until: None, relaying: watch::channel(BTreeSet::new()).0, socks: None, offer: watch::channel(None).0, stop: watch::channel(false).1 };
        m.lost("juniper--k3x9", 3).await;
        assert!(m.slots["juniper--k3x9"].core.is_some(), "another life's word: nothing");
        m.lost("juniper--k3x9", 4).await;
        let s = &m.slots["juniper--k3x9"];
        assert!(s.core.is_none() && !s.up, "stopped");
        assert!(*exited.borrow(), "its process ended");
        assert_eq!(s.exits.len(), 1, "an exit of its own");
        assert!(s.retry_at.is_some() && s.broken.is_none(), "started again after a while");
    }

    /// Goal (Alex's re-review of #546): a core a bridge before this one left
    /// running (one whose death it outlived) is killed before this bridge
    /// answers a hold or starts its own; a process that is not this
    /// computer's core is left alone.
    #[tokio::test]
    async fn a_core_left_by_a_bridge_before_is_killed_first() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("simplex-strays-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a directory");
        let bin = root.join("simplex-chat");
        std::fs::write(&bin, "#!/bin/sh\nsleep 30\n").expect("a fake core");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        let dir = root.join("data");
        let spawn = |d: String| {
            // by `sh`, never exec'ing the file just written (another test's fork may hold it open: ETXTBSY)
            let mut child = tokio::process::Command::new("sh").arg(&bin).args(["-d", &d]).spawn().expect("it runs");
            let pid = child.id().expect("a pid");
            // reaped as the container's init would
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            pid
        };
        let stray = spawn(dir.join("juniper--k3x9/sx").to_string_lossy().to_string());
        let other = spawn("/elsewhere/sx".into());
        // a sibling directory sharing its prefix is no part of it
        let sibling = spawn(root.join("data-other/juniper--k3x9/sx").to_string_lossy().to_string());
        tokio::time::sleep(Duration::from_millis(200)).await;
        kill_strays(&Config { bin: bin.clone(), dir, smp: None, xftp: None }).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        // SAFETY: signal 0 only asks whether the pid exists
        assert_eq!(unsafe { libc::kill(stray as i32, 0) }, -1, "the stray core is gone");
        for (pid, what) in [(other, "another directory's"), (sibling, "a prefix sibling's")] {
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0, "{what} left alone");
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Alex's re-review of #523): a forced sync still waiting is asked
    /// again every `RESYNC_MS` while its core is up, and no more once the
    /// ratchet is ok.
    #[tokio::test]
    async fn a_forced_sync_still_waiting_is_asked_again() {
        let (tx, _rx) = mpsc::channel(4);
        let (pulse, _) = watch::channel(false);
        let mut link = Link { phase: Phase::Linked { contact: 3, name: "paul".into() }, ..Link::default() };
        link.apply(Input::Started { rollback: true });
        assert!(link.syncing && link.forced);
        let mut slot = Slot {
            agent: agent(),
            dir: PathBuf::from("/nowhere"),
            saved: Saved { identity: "npub1juniper".into(), generation: 1, link, used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() },
            title: None,
            epoch: 1,
            core: None,
            up: true,
            corr: 0,
            pending: BTreeMap::new(),
            started: true,
            exits: VecDeque::new(),
            retry_at: None,
            resync_at: None,
            broken: None,
            note: None,
            draining_since: None,
            cancel_at: None,
            files: BTreeMap::new(),
            poster: None,
            gap: false,
            outstanding: Default::default(),
            acked: 0,
            door: None,
            session: None,
            linking: None,
        };
        slot.up = true;
        let cfg = Config { bin: PathBuf::from("/nowhere"), dir: PathBuf::from("/nowhere"), smp: None, xftp: None };
        let mut m = Manager { cfg, api: Api::new("http://127.0.0.1:9").expect("an address"), quiet: Arc::new(AtomicBool::new(true)), hold: PathBuf::from("/nowhere/hold"), tx, slots: BTreeMap::from([("juniper--k3x9".to_string(), slot)]), computer: Some(computer(true, 1, None)), held: false, pulse, pulse_until: None, relaying: watch::channel(BTreeSet::new()).0, socks: None, offer: watch::channel(None).0, stop: watch::channel(false).1 };
        m.resync();
        let first = m.slots["juniper--k3x9"].resync_at.expect("its next ask, set");
        m.slots.get_mut("juniper--k3x9").expect("its slot").resync_at = Some(Instant::now() - Duration::from_millis(1));
        m.resync();
        assert!(m.slots["juniper--k3x9"].resync_at.is_some_and(|t| t > Instant::now()), "asked, and the next one set");
        assert!(first > Instant::now());
        m.apply("juniper--k3x9", Input::Said(&Resp::Sync { contact: 3, status: wire::SyncStatus::Ok }));
        m.resync();
        assert_eq!(m.slots["juniper--k3x9"].resync_at, None, "ok: nothing more");
    }

    /// Goal (Alex's re-review of #548): what a start reads back to moves
    /// only with posts confirmed on the chat: short of a message still
    /// waiting for its file, and never past one this life did not post (a
    /// refusal), so the next start reads that one again and carries it.
    #[tokio::test]
    async fn only_a_confirmed_post_moves_what_was_delivered() {
        let (tx, _rx) = mpsc::channel(4);
        let (pulse, _) = watch::channel(false);
        let slot = Slot {
            agent: agent(),
            dir: std::env::temp_dir().join(format!("simplex-ack-{}", std::process::id())),
            saved: Saved { identity: "npub1juniper".into(), generation: 1, link: Link { phase: Phase::Linked { contact: 3, name: "paul".into() }, ..Link::default() }, used: BTreeMap::new(), abandoned: BTreeMap::new(), receiving: BTreeSet::new() },
            title: None,
            epoch: 1,
            core: None,
            up: true,
            corr: 0,
            pending: BTreeMap::new(),
            started: true,
            exits: VecDeque::new(),
            retry_at: None,
            resync_at: None,
            broken: None,
            note: None,
            draining_since: None,
            cancel_at: None,
            files: BTreeMap::new(),
            poster: None,
            gap: false,
            outstanding: Default::default(),
            acked: 0,
            door: None,
            session: None,
            linking: None,
        };
        let cfg = Config { bin: PathBuf::from("/nowhere"), dir: PathBuf::from("/nowhere"), smp: None, xftp: None };
        let mut m = Manager { cfg, api: Api::new("http://127.0.0.1:9").expect("an address"), quiet: Arc::new(AtomicBool::new(true)), hold: PathBuf::from("/nowhere/hold"), tx, slots: BTreeMap::from([("juniper--k3x9".to_string(), slot)]), computer: Some(computer(true, 1, None)), held: false, pulse, pulse_until: None, relaying: watch::channel(BTreeSet::new()).0, socks: None, offer: watch::channel(None).0, stop: watch::channel(false).1 };
        let f = "juniper--k3x9";
        let delivered = |m: &Manager| m.slots[f].saved.link.delivered;
        let forwarded = |m: &mut Manager, item: i64| {
            m.slots.get_mut(f).expect("its slot").outstanding.insert(item);
        };
        forwarded(&mut m, 10);
        m.delivered(f, 10, true);
        assert_eq!(delivered(&m), 10);
        // 12 forwarded (its voice note receiving, then queued once whole), 15
        // after it posted first: the mark stops just before 12, whatever
        // state 12 is in, until 12's own post
        forwarded(&mut m, 12);
        forwarded(&mut m, 15);
        m.delivered(f, 15, true);
        assert_eq!(delivered(&m), 11, "12 still outstanding");
        m.delivered(f, 12, true);
        assert_eq!(delivered(&m), 15, "12 posted: on to the newest acknowledged");
        // 18 dropped past a bound (its files' or its poster's): nothing more moves it
        forwarded(&mut m, 18);
        m.dropped(f, 18);
        forwarded(&mut m, 19);
        m.delivered(f, 19, true);
        assert_eq!(delivered(&m), 15, "the next start reads back to before the dropped one");
        // a refusal does the same, in a life of its own
        m.slots.get_mut(f).expect("its slot").gap = false;
        forwarded(&mut m, 20);
        m.delivered(f, 20, false);
        forwarded(&mut m, 21);
        m.delivered(f, 21, true);
        assert_eq!(delivered(&m), 15, "the next start reads back to before the refused one");
        let _ = std::fs::remove_dir_all(m.slots[f].dir.clone());
    }
}
