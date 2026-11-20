//! A computer's lifecycle (docs/computers.md), as a pure state machine the
//! Computer Durable Object drives. The DO keeps `Lifecycle` whole, hands
//! it each `Event` with the time it happened, and performs the `Action`s
//! it answers. Every start has a generation, and every completion names
//! the start it completes, so a late answer from an earlier start (a
//! container that came up after it was put to sleep) changes nothing.
//!
//! What it decides:
//! - **Waking.** A record on a subscribed channel, an open port tab, one
//!   of its agents added to a fragment, or the owner (also their shell's
//!   pre-wake, on a chat picked or the launcher opened). A wake while asleep
//!   starts the container; one while it starts or is awake holds it
//!   longer; one during an idle sleep's hold or save cancels that sleep,
//!   and it stays awake on the container it has; one during its owner's
//!   sleep, or once a sleep's stop began, starts it again once it is
//!   asleep (the newest push wins: lesson 3 of docs/cloudflare-v1.md).
//! - **Staying awake.** While the guest holds a keepalive socket; with
//!   none open, until the latest hold runs out (`IDLE_MS` after a wake,
//!   a keepalive's close, or a port tab's opening, close or last use:
//!   the one number). A port tab merely open holds nothing: its page's
//!   words do (`Event::TabUsed`). An always-on computer never sleeps.
//! - **Saving** (docs/computers.md, Durability, step 1). `/data` is saved when
//!   work ends (the guest's last keepalive or its owner's last port tab
//!   closes, and `SAVE_SETTLE_MS` pass with no keepalive opened again, so
//!   turns back to back, or tabs, save once), every
//!   `SAVE_EVERY_MS` while a keepalive stays open or a port tab is used,
//!   and at every sleep. A
//!   save is a hold (the DO touches `/run/computer/hold` and waits for the
//!   guest's `/run/computer/held`), the save, and, awake, the hold let go
//!   once its home's record is taken, before its work's.
//!   A save that fails is tried again after a growing pause.
//! - **Sleeping.** Its hold and its save, then its stop (snapshot, signal,
//!   wait, destroy), reported back by `Asleep`. A keepalive that opens
//!   during an idle sleep's hold cancels the sleep: the guest took work as
//!   it was held, so it stays awake (docs/computers.md, Lifecycle). So
//!   does a wake during its hold or its save: a save under way goes on as
//!   an awake save, and is kept or failed as one. Its port tabs close as
//!   it begins (the cell's), so nothing a person sends after reaches the
//!   container its save holds.
//!   A sleep whose save fails keeps its container and is tried again, for
//!   at most `Rules::unsaved_max_ms`; then it sleeps unsaved, and says so.
//!   A container its host stops unasked while it is idle, with all this
//!   life did in a save of its own and no wake its guest has not taken up,
//!   has slept too: no strike, no rollback, and nothing starts it again
//!   until something wants it.
//! - **Failing.** A start that fails, never reports ready, or whose
//!   container stops before it is ready, is tried again with a growing
//!   pause: one strike, however many ways its failure is heard. After
//!   `STARTS_FAILED_MAX` in a row the computer "won't wake" until its
//!   owner asks (lesson 4): no more
//!   container starts are paid for. A start whose save would not restore
//!   starts again at once from the save before it, no strike against it.
//! - **Trying again.** "Won't wake" is never for good: a wake not its
//!   owner's, `WONT_WAKE_RETRY_MS` after it gave up, tries one start, its
//!   last strike.
//! - **Restarting.** Its owner's restart (`Event::Restart`) is the way back
//!   to working: a sleep that saves if it can and stops whether or not it
//!   did, then a start owed at once (asleep, or won't wake: the start owed
//!   alone), which its alarm makes. It names the start it restarts, so
//!   asked twice it is made once.
//! - **Metering.** Awake time, in intervals that never overlap, every
//!   `METER_EVERY_MS` while awake and at sleep: only time it was shown up
//!   (`ALARM_LATE_MS`), and no row longer than `AWAKE_ROW_MAX_MS` charged
//!   (`awake_batch`).
//!
//! And, beside it, what the DO knows of its saves (`Saves`): the newest
//! `SAVES_KEPT` saves of `/data`, the snapshot that caches the current one,
//! what each start that came up restored, and whether that went back in
//! time (a rollback), and the saves an operator set aside as they chose an
//! older one for its next start (`Saves::choose`, `Event::SaveChosen`);
//! and of the image its next start runs (`Images`).
//! From them, what its owner is told (`notices`): a rollback only when
//! something was done on the life it lost after the save it went back to.

pub mod said;
pub mod start_line;

mod awake;
mod disk;
mod held;
mod images;
mod lifecycle;
mod limits;
mod notices;
mod saves;
#[cfg(test)]
mod testing;
mod which;

pub use awake::awake_batch;
pub use disk::{disk_clear, disk_filling, disk_of_df, reads_disk_after, ComputerDisk, DISK_CLEAR_FREE_MIN, DISK_CLEAR_USED_PCT, DISK_READ, DISK_TOLD_FREE_MAX, DISK_TOLD_USED_PCT};
pub use held::{left_out, unheld, work_left_out, HELD_ANSWER_MAX_BYTES, LEFT_OUT_MAX, LEFT_OUT_PATTERN_MAX_BYTES, SAVED_DIRS, UNHELD_MAX_BYTES};
pub use images::{does_simplex, image_running, valid_version, version_now, version_of, version_running, Images, VERSION_HEX};
pub use lifecycle::{alive, unplanned_start, Action, Alive, Ended, Event, Failing, Lifecycle, Phase, Retry, Rules, SaveNote, Saving, Socket, Step, Unsaved, Wake};
pub use limits::{
    ALARM_LATE_MS, ALIVE_MS, AWAKE_ROW_MAX_MS, FAILED_WHY_MAX_BYTES, HOLD_DEADLINE_MS, HOLD_WAIT_MS, IDLE_MS, KEEPALIVES_LOST_MAX, METER_EVERY_MS, RETRY_PAUSE_MS, SAVES_KEPT, SAVE_DEADLINE_MS, SAVE_EVERY_MS, SAVE_RETRY_MS,
    SAVE_SETTLE_MS, SET_ASIDE_KEPT_MS, SET_ASIDE_MAX, SLEEP_DEADLINE_MS, STARTS_FAILED_MAX, STARTS_OWED_MAX, START_DEADLINE_MS, STOP_LOOK_MS, TAB_USED_EVERY_MS, UNSAVED_MAX_MS_DEFAULT, UNSAVED_NOTICE_FAILURES,
    WONT_WAKE_RETRY_MS,
};
pub use notices::{notices, support_url_ok, SUPPORT_URL_MAX_BYTES};
pub use saves::{Plan, Running, Save, Saves, Snapshot, Starting, NotChosen, VaultAsked, WentBack, FORGETTING_MAX, SAVE_RECORDS_MAX};
pub use which::default_computer_of;
