//! What the Computer DO tells its lifecycle (`Event`, under its deployment's
//! `Rules`), and what each step answers (`Step`: its actions, its alarm, the
//! life it ended).

use fragment_proto::computer::LifeEnd;
use serde::{Deserialize, Serialize};

use crate::computer::limits::{IDLE_MS, PREWAKE_MS, UNSAVED_MAX_MS_DEFAULT};

/// The deployment's rules for its computers' lifecycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// A sleep whose save keeps failing keeps its container this long at
    /// most, then sleeps unsaved (docs/computers.md, Lifecycle).
    pub unsaved_max_ms: i64,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { unsaved_max_ms: UNSAVED_MAX_MS_DEFAULT }
    }
}

/// What woke it, which says how long it holds the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wake {
    /// A record on a channel one of its agents subscribed to.
    Record,
    /// A page opened a fragment it subscribes to, or someone is typing.
    Presence,
    /// A port tab is opening.
    Tab,
    /// Its owner asked (the shell, the CLI): also lifts "won't wake".
    Owner,
    /// One of its agents was added to a fragment (Paul, 2026-10-03: agents
    /// are woken eagerly, to hide a wake's latency): the guest lists its
    /// agent's fragments and follows the new one before anyone speaks
    /// there, and is held as a record holds it, since someone is about to.
    Joined,
}

impl Wake {
    pub(super) fn hold_ms(self) -> i64 {
        match self {
            Wake::Presence => PREWAKE_MS,
            Wake::Record | Wake::Tab | Wake::Owner | Wake::Joined => IDLE_MS,
        }
    }
}

/// A socket the Computer DO accepted that keeps it awake while open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Socket {
    /// The guest's own (`/api/computer/keepalive`): it is busy.
    Keepalive,
    /// A person's tab onto one of its ports.
    Tab,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Wake { why: Wake },
    /// A record its agents follow, or the notice of one's join, was posted
    /// at `at_ms` (the poster's clock), ahead of the wake it brings: work its
    /// guest has not taken up (`Lifecycle`'s `owed_ms`), unless a keepalive
    /// was open then (a guest at work queues what comes) or closed after it
    /// (its last work came after it), until a keepalive opens.
    Posted { at_ms: i64 },
    /// The start `generation` finished: running, its intercepts set, its
    /// data restored.
    Ready { generation: u64 },
    StartFailed { generation: u64, why: String },
    /// The start `generation` was from a snapshot, and failed (or its
    /// container stopped before it came up). The DO has forgotten the
    /// snapshot, so the next start is from the image and the save: it is
    /// started again at once, and the first in a wake is no strike against
    /// the computer (the snapshot was at fault, not the computer).
    SnapshotFailed { generation: u64, why: String },
    /// The start `generation` restored a save that is unusable (its archive
    /// is gone or corrupt, or the image's check of what it restored
    /// failed): the DO marked it so. With an `older` save left to try, it
    /// starts again at once from that one, no strike against it .
    RestoreFailed { generation: u64, why: String, older: bool },
    Opened { socket: Socket },
    Closed { socket: Socket },
    /// The DO touched the hold of start `generation` and waited: whether
    /// the guest answered `held` in time.
    Held { generation: u64, held: bool },
    /// Save `seq` of start `generation` worked: the DO keeps it.
    Saved { generation: u64, seq: u64 },
    /// Save `seq` of start `generation` failed (`seq` 0: its hold found no
    /// container to save).
    SaveFailed { generation: u64, seq: u64, why: String },
    /// The sleep `generation` finished: the container is gone.
    Asleep { generation: u64 },
    /// The container stopped on its own (a crash, the runtime's idle stop).
    /// Heard during a sleep's stop, which signalled it, it is nothing: that
    /// stop reports its end itself (`Asleep`).
    Exited { generation: u64 },
    /// The start `generation` failed, and destroys what it started next:
    /// that container's exit is no crash (`Phase::Starting`'s
    /// `destroying`), and the start's own report follows. Kept with the
    /// lifecycle, so an isolate that comes up after it knows it too.
    StartFailing { generation: u64 },
    Alarm,
    /// Its owner put it to sleep: what held it is let go (its sockets close
    /// with its container).
    Sleep,
    /// Its owner restarted it (the shell's Restart): running, a sleep that
    /// saves if it can and stops whether or not it did (its save failing
    /// keeps no container: they asked), then a start; asleep or won't wake,
    /// a start. `generation` is the start they restart (`restarts`), 0 for
    /// whatever runs: an earlier one's restart was made already, and is
    /// nothing now.
    Restart { generation: u64 },
    /// The owner's plan changed: an always-on computer never sleeps.
    AlwaysOn { on: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Start the container (from a snapshot, or the image and a restore:
    /// the DO's choice), then report `Ready` or `StartFailed`.
    Start { generation: u64 },
    /// Begin its sleep: hold the guest, then report `Held` (or `Asleep`
    /// when its container is gone already).
    Sleep { generation: u64 },
    /// Hold the guest for a save while it stays awake, then report `Held`.
    Hold { generation: u64 },
    /// Save `/data` as save `seq` (leaving out what a held guest copied, when
    /// `held`), then report `Saved` or `SaveFailed`.
    Save { generation: u64, held: bool, seq: u64 },
    /// Let go of the guest's hold: an awake save's end, or a sleep that
    /// will not stop it.
    Unhold { generation: u64 },
    /// End its sleep: a snapshot (when its save worked: a cache of it), the
    /// signal, the wait, the destroy; then report `Asleep`.
    Stop { generation: u64, saved: bool },
    /// Meter awake time from `from_ms` to `to_ms` to the owner.
    Meter { from_ms: i64, to_ms: i64 },
}

/// What the computer says of its saves in its view (its `why`), as one
/// step changes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SaveNote {
    /// As it was.
    #[default]
    Same,
    Says(String),
    /// A save worked: nothing to say.
    Clear,
}

/// What an event answered: the actions to perform, in order, and when the
/// DO's alarm should next fire (`None`: no alarm needed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    pub actions: Vec<Action>,
    pub alarm_ms: Option<i64>,
    /// Why a wake started nothing (it won't wake).
    pub refused: Option<String>,
    /// A life (a start that came up) ended with this event: the DO tells
    /// its `Saves` before it performs the actions (the next start reads it).
    pub ended: Option<Ended>,
    /// What its view says of its saves from now on.
    pub note: SaveNote,
}

/// How one life ended: the start it was, by what, and whether its sleep
/// saved it (a sleep's stop after a save that worked). A container that
/// died while its sleep held or saved it, before the sleep stopped it,
/// ended by `Exit`: it stopped on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub generation: u64,
    pub by: LifeEnd,
    #[serde(default)]
    pub saved: bool,
    /// When it ended (ms); 0 for an end recorded before ends were timed.
    #[serde(default)]
    pub at_ms: i64,
    /// Its sleep was its owner's restart.
    #[serde(default)]
    pub restart: bool,
    /// Nothing was done on it since its newest save that worked, or since
    /// it came up when it had none (`Lifecycle::worked`): what it ran from
    /// that save on is in that save. False for an end recorded before this
    /// was kept: told as any loss.
    #[serde(default)]
    pub quiet: bool,
}
