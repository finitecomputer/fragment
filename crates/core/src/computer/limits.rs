//! The bounds a computer's lifecycle keeps: its holds, deadlines, pauses and
//! counts, each checked against the others as it builds.

/// With nothing open, how long a record or a closed socket holds a
/// computer awake (decision 39).
pub const IDLE_MS: i64 = 20 * 60_000;
/// How long a pre-wake holds one that nothing else holds (decision 39:
/// "it stops again after 60 s if nothing arrives").
pub const PREWAKE_MS: i64 = 60_000;
/// Starts that fail in a row before the computer stops trying.
pub const STARTS_FAILED_MAX: u32 = 3;
/// How long a computer that won't wake waits before a wake that is not its
/// owner's (a record, a page's presence, a join, a port tab) tries one
/// start again: "won't wake" is never for good, since what failed may have
/// passed (a storage error, a disk a release made room on), yet a computer
/// that still fails costs at most one start this often, and only while
/// something wants it (2026-11-03: one storage error left a computer
/// refusing every wake for four days).
pub const WONT_WAKE_RETRY_MS: i64 = 30 * 60_000;
/// A start that has not reported ready by then has failed: a new image's
/// first pull at a location takes 24–40 s (spike S3b), and a start from a
/// save restores it whole first, 5.7 GB in 147 s on the production fleet
/// (2026-11-02). Three minutes cut a restore of 5.9 GB at 165 s, every
/// try, so that computer never started again (2026-11-03): a restore is
/// given nine minutes and more (the cell's `RESTORE_MS`, which also
/// leaves an alarm's other events room: a start made by an alarm fits it).
pub const START_DEADLINE_MS: i64 = 15 * 60_000;
/// A sleep's stop (snapshot, signal, five seconds of exit, destroy)
/// finishes within this, or it is asked again.
pub const SLEEP_DEADLINE_MS: i64 = 60_000;
/// Awake time is metered at least this often.
pub const METER_EVERY_MS: i64 = 5 * 60_000;
/// A running computer is heard from at least every `METER_EVERY_MS` (its
/// alarm), or each step of a save (`SAVE_DEADLINE_MS`), behind whose run
/// that alarm can wait (the cell's `ALARM_WALL_MS`). An event that comes
/// when nothing was heard of it for longer than this was heard late: its
/// alarms went unheard (a handler that failed before it asked for
/// another, or the platform's delay), and nothing shows it ran meanwhile,
/// so that time is not metered (FIN-511).
pub const ALARM_LATE_MS: i64 = 20 * 60_000;
/// No awake row longer than this is charged (`awake_batch`): the longest the
/// lifecycle meters is a meter's wait and a sleep's steps, each heard within
/// `ALARM_LATE_MS`, about twenty minutes in all, so one longer came from a
/// path it should not have, and is logged, never billed (FIN-511).
pub const AWAKE_ROW_MAX_MS: i64 = 60 * 60_000;
/// The pause before a failed start is tried again, doubled for each
/// failure before it.
pub const RETRY_PAUSE_MS: i64 = 5_000;
/// After the guest's last keepalive closes, a save is asked for this much
/// later, unless one opens first: turns back to back save once.
pub const SAVE_SETTLE_MS: i64 = 30_000;
/// While a keepalive stays open, it is saved this often (Cloudflare's
/// auto-save guide: a sandbox that never goes idle is saved on a timer).
pub const SAVE_EVERY_MS: i64 = 15 * 60_000;
/// The pause before a failed save is tried again, doubled for each failure
/// in a row before it, up to `SAVE_EVERY_MS`.
pub const SAVE_RETRY_MS: i64 = 60_000;
/// How long a computer stays awake for a sleep whose save keeps failing,
/// by default (`Rules`, the deployment's `computer_unsaved_max_ms`): thirty
/// minutes, Paul's (2026-10-08; docs/computers.md, Lifecycle).
pub const UNSAVED_MAX_MS_DEFAULT: i64 = 30 * 60_000;
/// Awake saves that fail in a row before its owner is told (`notices`): one
/// that fails is tried again a minute later, and a blip need not alarm
/// anyone. A sleep's that fails is told at once: the bound runs from it.
pub const UNSAVED_NOTICE_FAILURES: u32 = 2;
/// The words a failure keeps (`Failing::why`), cut at a character.
pub const FAILED_WHY_MAX_BYTES: usize = 300;
/// How long the DO waits for the guest's `held` after it touched the hold.
/// An image that never answers is saved anyway, not held.
pub const HOLD_WAIT_MS: i64 = 20_000;
/// A hold (the touch, the wait, the answer) reports within this, or it is
/// asked again.
pub const HOLD_DEADLINE_MS: i64 = 90_000;
/// A save reports within this, or it failed, as one that says so did. The
/// cell's own bounds on a save's steps end it sooner (cell/src/computer.rs,
/// `SAVE_STEPS_MS`: 12 minutes 10 seconds, its backups what a Durable
/// Object's alarm leaves them, so a full disk's save fits): this catches a
/// report its isolate lost.
pub const SAVE_DEADLINE_MS: i64 = 13 * 60_000;
/// Saves of `/data` kept: a wake restores the newest that restores, and
/// falls back to the one before it (docs/computers.md, "Saves and what a
/// wake restores"). Beside these, the save its running life came up from
/// is kept too, until a start comes up from a newer one: a long life saves
/// every 15 minutes, and its three newest would otherwise all be its own,
/// none a start ever came up from.
pub const SAVES_KEPT: usize = 3;
/// Starts owed one after another at most (`Lifecycle::gone`, each due at
/// once): the one after its container went, one past a snapshot that
/// would not start, and one past each kept save that would not restore
/// (`restore_fallbacks`); the next failure is a strike, retried after a
/// pause. A request makes them all as it waits (the cell's `drive_with`).
pub const STARTS_OWED_MAX: u32 = SAVES_KEPT as u32 + 2;
/// A start from a snapshot is up only once its guest has read its own view
/// (`GET /api/computer`, which an image reads as its entrypoint starts:
/// `alive`) within this of the DO's last step of that start: a container
/// Cloudflare restores from a snapshot can come up dead, taking execs for a
/// second and then nothing, its guest never heard from (#362). A healthy
/// guest reads it within 2 s (r4, r6); one whose first tries came before
/// its intercepts were set waits out its backoff, at most 30 s and half
/// again of jitter for our bridge's (`net::Backoff`).
pub const ALIVE_MS: i64 = 45_000;
/// Keepalives closed at most per event for want of a socket
/// (`Lifecycle::keepalives_lost`): each close is an event of its own, and
/// the next event closes the rest.
pub const KEEPALIVES_LOST_MAX: u32 = 16;

const _: () = assert!(PREWAKE_MS < IDLE_MS, "a pre-wake holds less than a record");
const _: () = assert!(RETRY_PAUSE_MS << STARTS_FAILED_MAX < START_DEADLINE_MS, "retries stay within a start's deadline");
const _: () = assert!(SAVE_SETTLE_MS < IDLE_MS, "work that ended is saved before an idle sleep would save it");
const _: () = assert!(SAVE_RETRY_MS < SAVE_EVERY_MS && SAVE_EVERY_MS < UNSAVED_MAX_MS_DEFAULT, "a failed save is tried again within its bound");
const _: () = assert!(HOLD_WAIT_MS < HOLD_DEADLINE_MS, "a hold's wait fits its deadline");
const _: () = assert!(SAVES_KEPT >= 2, "a save to fall back to");
const _: () = assert!(UNSAVED_NOTICE_FAILURES >= 1, "told after a failure, never before");
const _: () = assert!(ALIVE_MS < START_DEADLINE_MS, "a start's wait for its guest fits its deadline");
const _: () = assert!(WONT_WAKE_RETRY_MS > START_DEADLINE_MS, "a computer that won't wake tries one start at a time");
const _: () = assert!(ALARM_LATE_MS > SAVE_DEADLINE_MS && ALARM_LATE_MS > METER_EVERY_MS, "an alarm held up behind a save, or one meter's wait, is on time");
const _: () = assert!(
    METER_EVERY_MS + ALARM_LATE_MS + HOLD_DEADLINE_MS + SAVE_DEADLINE_MS + SLEEP_DEADLINE_MS < AWAKE_ROW_MAX_MS,
    "the longest row the lifecycle meters (a meter's wait heard late, then a sleep's steps) is charged"
);
