//! A computer's lifecycle: where it is (`Phase`), what it keeps beside its
//! phase (`Lifecycle`), and each event applied to it (`Lifecycle::apply_with`).

use fragment_proto::computer::LifeEnd;
use serde::{Deserialize, Serialize};

use crate::computer::limits::{ALARM_LATE_MS, IDLE_MS, KEEPALIVES_LOST_MAX, METER_EVERY_MS, SAVES_KEPT, SAVE_EVERY_MS, SAVE_SETTLE_MS, SLEEP_DEADLINE_MS, STARTS_FAILED_MAX, START_DEADLINE_MS, STOP_LOOK_MS, WONT_WAKE_RETRY_MS};

mod event;
mod legacy;
mod saving;
mod starts;
mod unsaved;
#[cfg(test)]
mod simulation;
#[cfg(test)]
mod tests;

pub use event::{Action, Ended, Event, Rules, SaveNote, Socket, Step, Wake};
pub use saving::Saving;
pub use starts::{alive, unplanned_start, Alive};
pub use unsaved::{Failing, Unsaved};

/// Where its lifecycle is, each phase with what is only true of it. A
/// phase stored before FIN-395 C is filled from the flags that release
/// kept beside it (`Lifecycle::read`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// No container. `retry`: a start due at its time, a failed start's
    /// retry or one owed once its container went.
    Asleep {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<Retry>,
    },
    /// `destroying`: the start failed, and is destroying what it started
    /// (`Event::StartFailing`): that container's exit is its own destroy,
    /// never a crash, and the start's own report is the one that counts.
    Starting {
        generation: u64,
        since_ms: i64,
        #[serde(default)]
        destroying: bool,
    },
    Awake { generation: u64, since_ms: i64 },
    /// `owner`: its owner's sleep (no keepalive cancels it); `restarting`:
    /// their restart (a save that fails keeps no container); `owes_start`:
    /// a start follows its stop (a restart's, or a wake heard as it went
    /// that did not cancel it: `Lifecycle::cancels_sleep`).
    Sleeping {
        generation: u64,
        since_ms: i64,
        #[serde(default)]
        owner: bool,
        #[serde(default)]
        restarting: bool,
        #[serde(default)]
        owes_start: bool,
    },
    /// Its starts kept failing: it won't wake until its owner asks, or a
    /// wake not its owner's `WONT_WAKE_RETRY_MS` after `since_ms` (0 in a
    /// phase stored before it was kept: past that) tries once.
    Failed {
        why: String,
        #[serde(default)]
        since_ms: i64,
    },
}

/// For `skip_serializing_if`: a field at its default is not stored.
fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Phase {
    /// Asleep with no start due.
    pub const ASLEEP: Phase = Phase::Asleep { retry: None };
}

/// A start due while asleep: at `at_ms`, and `owed` (made whether or not
/// anything still wants it: a restart's, or a wake's after a sleep) or a
/// failed start's retry (made only while something wants it).
///
/// Stored as `[at_ms, owed]`: a pair costs the cell's Wasm less than a
/// struct of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(i64, bool)", into = "(i64, bool)")]
pub struct Retry {
    pub at_ms: i64,
    pub owed: bool,
}

impl From<(i64, bool)> for Retry {
    fn from((at_ms, owed): (i64, bool)) -> Retry {
        Retry { at_ms, owed }
    }
}

impl From<Retry> for (i64, bool) {
    fn from(r: Retry) -> (i64, bool) {
        (r.at_ms, r.owed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub phase: Phase,
    /// The last start's generation.
    generation: u64,
    keepalives: u32,
    /// Port tabs open. A tab holds it only by its use (`Event::TabUsed`),
    /// never by being open: a page left open says nothing.
    tabs: u32,
    /// With no keepalive open, it is held awake until this.
    held_until_ms: i64,
    /// Starts of this wake that failed in a row: a count across its phases
    /// (asleep for the retry, starting, gone), so beside them.
    failures: u32,
    always_on: bool,
    /// Awake time is metered up to here (while awake).
    metered_to_ms: i64,
    /// The running start came up (it was ready): its end is a life's end.
    /// A lifecycle stored before this field reads `false`, so a computer
    /// awake across that deploy reports no end for that one life.
    #[serde(default)]
    ready: bool,
    /// This wake already fell back from a snapshot that would not start:
    /// a second fallback before a start comes up is a failure like any.
    #[serde(default)]
    fell_back: bool,
    /// The save under way, awake or a sleep's. A lifecycle stored before
    /// saves were its own reads none: a sleep then under way is in its stop.
    #[serde(default)]
    saving: Option<Saving>,
    /// A save is asked for at this time: the settle after work ended, or a
    /// failed save's next try.
    #[serde(default)]
    save_due_ms: Option<i64>,
    /// Since when a keepalive has stayed open while awake (none while idle).
    #[serde(default)]
    busy_since_ms: Option<i64>,
    /// When the last save of this life was asked for (its start, before any).
    #[serde(default)]
    save_asked_ms: i64,
    /// The last save's number (`Saving::Save`'s `seq`): a late answer to an
    /// earlier one changes nothing.
    #[serde(default)]
    save_seq: u64,
    /// Saves of this life that failed in a row (the retry's pause grows).
    #[serde(default)]
    save_failures: u32,
    /// A sleep's save has failed since then, and no save has worked since:
    /// it stays awake, within `Rules::unsaved_max_ms` of this.
    #[serde(default)]
    unsaved_since_ms: Option<i64>,
    /// Starts of this wake that fell back from a save that would not
    /// restore (at most `SAVES_KEPT`: each is from an older save).
    #[serde(default)]
    restore_fallbacks: u32,
    /// This life's saves failing in a row, awake or a sleep's: since when,
    /// and why (none once one works).
    #[serde(default)]
    failing: Option<Failing>,
    /// Awake time its owner is not charged (Paul, 2026-10-08: the time a
    /// computer is kept for saves the platform failed is the platform's),
    /// `[from, to)` in ms: from the start of the try whose save failed
    /// first (its sleep's hold, however long it hung before it failed) to
    /// the bound after that failure, or past it to the failure of the try
    /// under way at the bound; none once the window ends (a save works, or
    /// it stops). Opened and ended beside `unsaved_since_ms`, the failure
    /// itself, which the bound counts from.
    #[serde(default)]
    unmetered: Option<(i64, i64)>,
    /// When this life last saw something done on it (`worked`): a keepalive
    /// or a port tab opened or closed, or a record or a join woke it. 0:
    /// nothing since it came up.
    #[serde(default)]
    worked_ms: i64,
    /// What this life did before this is in a save: its start, then the ask
    /// of each of its saves that worked. 0 in a lifecycle stored before it
    /// was kept: none of that life's work is counted kept (`Ended::quiet`).
    #[serde(default)]
    kept_ms: i64,
    /// A record or a join for its agents was posted at this time
    /// (`Event::Posted`) after its guest last opened a keepalive, with none
    /// open: work its guest has not taken up, which no save holds. Cleared
    /// when a keepalive opens, or at a sleep that owes no start. None in a
    /// lifecycle stored before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owed_ms: Option<i64>,
    /// When its guest last opened a keepalive (began work it was given): a
    /// record posted before it was there for that work to take up. Its
    /// opening, never a close: a close the object only infers (a socket a
    /// deploy cut) says nothing of when its guest's work ended. 0 when none
    /// has, or stored before it was kept.
    #[serde(default, skip_serializing_if = "is_zero")]
    opened_ms: i64,
    /// Since when a person has used its port tabs with no save asked
    /// since: their first use after the last ask (one before it is stale).
    /// While a tab is open, an awake save is due `SAVE_EVERY_MS` after it,
    /// as one is while a keepalive is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    used_since_ms: Option<i64>,
    /// When a person's use of its port tabs was last heard (`TabUsed`): one
    /// since may not be heard yet, up to `TAB_USED_EVERY_MS` after it,
    /// while a tab is open (`Lifecycle::tab_unheard_ms`). 0 for none.
    #[serde(default, skip_serializing_if = "is_zero")]
    tab_used_ms: i64,
    /// When this life last ran something that may change its files: a
    /// turn (a keepalive opened or closed) or a person's use of a port tab
    /// (`changed`). Narrower than `worked_ms`, which a tab's opening or a
    /// record's wake moves too. 0: nothing since it came up. None in a
    /// lifecycle stored before it was kept: its work counts as change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    changed_ms: Option<i64>,
    /// Since when the starts of this wake have found the container before
    /// them still running (`Event::StartBlocked`): none once one comes up,
    /// or as a wake or a restart counts its strikes anew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocked_since_ms: Option<i64>,
    /// Its `/data` is a disk (docs/computers.md, "A disk"; `fpac`): written
    /// to R2 as it goes, so it is never saved for itself. A sleep's hold
    /// waits for the disk to ship what it holds (the cell's), and its stop
    /// follows at once; no save is due awake but its owner's export's
    /// (`export_due_ms`). Set as it is made, for good: false in every
    /// lifecycle stored before it was kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    disk: bool,
    /// A disk's save its owner's export asked for (`Event::ExportAsked`):
    /// due from then, taken awake under the hold as any awake save; one
    /// that failed is due again after its pause. None once one is saved,
    /// as its export gives up, and whenever no start runs (asked asleep, or
    /// its start or life ended): the export asks again while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    export_due_ms: Option<i64>,
    /// When its last save that landed (`Event::Saved`; a disk's sleep is no
    /// save) was asked for: an export begun before then takes no second
    /// (`Event::ExportAsked`). Its ask, never its landing: the save is
    /// stored before its event, so an export begun between finds it too old
    /// and asks again, and is heard (Codex on #1552).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saved_asked_ms: Option<i64>,
}

impl Default for Lifecycle {
    fn default() -> Lifecycle {
        Lifecycle::new()
    }
}

/// Why a disk's sleep did not stop: its wait ended with writes unshipped.
const DISK_UNSHIPPED: &str = "its disk still had writes to ship when its wait ended";

impl Lifecycle {
    /// Whether it stays awake whatever is open (a `seat_always_on` seat's,
    /// as its owner's seat says: the registry tells it).
    pub fn always_on(&self) -> bool {
        self.always_on
    }

    /// Whether its `/data` is a disk (`disk`'s doc).
    pub fn disk(&self) -> bool {
        self.disk
    }

    /// It is made on a disk: before its first start, never after (a
    /// computer with saves keeps them; moving one is not this).
    pub fn set_disk(&mut self) {
        assert!(self.generation == 0 && matches!(self.phase, Phase::Asleep { .. }), "a computer is put on a disk as it is made");
        self.disk = true;
    }

    /// Whether this wake fell back from a snapshot that would not start
    /// (until a start comes up).
    pub fn fell_back(&self) -> bool {
        self.fell_back
    }

    pub fn new() -> Lifecycle {
        Lifecycle {
            phase: Phase::ASLEEP,
            generation: 0,
            keepalives: 0,
            tabs: 0,
            held_until_ms: 0,
            failures: 0,
            always_on: false,
            metered_to_ms: 0,
            ready: false,
            fell_back: false,
            saving: None,
            save_due_ms: None,
            busy_since_ms: None,
            save_asked_ms: 0,
            save_seq: 0,
            save_failures: 0,
            unsaved_since_ms: None,
            restore_fallbacks: 0,
            failing: None,
            unmetered: None,
            worked_ms: 0,
            kept_ms: 0,
            owed_ms: None,
            opened_ms: 0,
            used_since_ms: None,
            tab_used_ms: 0,
            changed_ms: None,
            blocked_since_ms: None,
            disk: false,
            export_due_ms: None,
            saved_asked_ms: None,
        }
    }

    /// A stored lifecycle, and whether its phase was filled from the flags
    /// a release before FIN-395 C kept beside it (`legacy`), which the cell
    /// logs. Decoded here, in one function the cell calls: its own decoder
    /// (`crate::json_text`) in its place costs the cell's Wasm 7,523 bytes
    /// more (measured 2026-11-16).
    pub fn read(text: &str) -> serde_json::Result<(Lifecycle, bool)> {
        // its bytes, never `from_str`: the cell decodes JSON text through
        // one decoder per type (`crate::json_text`, cell/clippy.toml)
        let mut life: Lifecycle = serde_json::from_slice(text.as_bytes())?;
        let filled = legacy::filled(text, &life.phase)?;
        let old = filled.is_some();
        if let Some(phase) = filled {
            life.phase = phase;
        }
        Ok((life, old))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The running start, when one is running (starting, awake, or going
    /// to sleep): the start a container that runs now belongs to.
    pub fn running(&self) -> Option<u64> {
        match self.phase {
            Phase::Starting { generation, .. } | Phase::Awake { generation, .. } | Phase::Sleeping { generation, .. } => Some(generation),
            Phase::Asleep { .. } | Phase::Failed { .. } => None,
        }
    }

    /// Whether something open keeps it awake: a keepalive (a turn running).
    /// A port tab never does by being open: its use holds it, `IDLE_MS`
    /// after each (`Event::TabUsed`), as a record does.
    pub fn open(&self) -> bool {
        self.keepalives > 0
    }

    /// The guest's keepalives it counts open now (its turns running).
    pub fn keepalives(&self) -> u32 {
        self.keepalives
    }

    /// The keepalives it counts beyond the `held` sockets its Durable
    /// Object's runtime still holds open for the running start, at most
    /// `KEEPALIVES_LOST_MAX`: each went without a close it heard. A deploy
    /// disconnects every socket, and the guest's close after it reaches no
    /// release (#557): counted open for good, the computer never slept,
    /// saved every 15 minutes and metered. The DO closes each
    /// (`Event::Closed`), as its close would have, before each event. One
    /// the runtime holds is never among them, and none is ever counted open
    /// for a socket the lifecycle did not hear open.
    pub fn keepalives_lost(&self, held: usize) -> u32 {
        let held = u32::try_from(held).unwrap_or(u32::MAX);
        self.keepalives.saturating_sub(held).min(KEEPALIVES_LOST_MAX)
    }

    pub fn held_until_ms(&self) -> i64 {
        self.held_until_ms
    }

    fn hold(&mut self, until_ms: i64) {
        self.held_until_ms = self.held_until_ms.max(until_ms);
    }

    /// Whether anything still wants it awake at `now`.
    fn wanted(&self, now_ms: i64) -> bool {
        self.always_on || self.open() || now_ms < self.held_until_ms
    }

    /// When it would sleep for want of anything holding it, if nothing
    /// opens: when its holds run out, or, while a sleep's failed save
    /// pauses before its next try, at that try.
    fn idle_at_ms(&self) -> Option<i64> {
        if self.always_on || self.open() {
            return None;
        }
        match (self.unsaved_since_ms, self.save_due_ms) {
            (Some(_), Some(due)) => Some(self.held_until_ms.max(due)),
            _ => Some(self.held_until_ms),
        }
    }

    /// When a save is due because it is in use: every `SAVE_EVERY_MS` while
    /// a keepalive stays open, counted from the last save asked for, or
    /// from a port tab's first use since that ask while a tab is open.
    fn busy_save_due_ms(&self) -> Option<i64> {
        let turn = self.busy_since_ms.filter(|_| self.keepalives > 0).map(|since| since.max(self.save_asked_ms));
        let tab = self.used_since_ms.filter(|since| self.tabs > 0 && *since >= self.save_asked_ms);
        Some(turn.into_iter().chain(tab).min()? + SAVE_EVERY_MS)
    }

    /// A person used one of its open port tabs (its page spoke): work, held
    /// `IDLE_MS` as a tab's opening is, and counted for its saves; for a
    /// start that came up only, as all work is (`worked`).
    fn tab_used(&mut self, now_ms: i64) {
        assert!(self.tabs > 0, "a use is of a tab it counts open");
        if !self.ready {
            return;
        }
        self.worked(now_ms);
        self.changed(now_ms);
        self.tab_used_ms = self.tab_used_ms.max(now_ms);
        if self.used_since_ms.is_none_or(|since| since < self.save_asked_ms) {
            self.used_since_ms = Some(now_ms);
        }
    }

    /// Awake time up to `now`, once, but what falls in the unsaved window
    /// (`unmetered`): its owner pays for none of it (at most two intervals,
    /// before the window and after it).
    fn meter(&mut self, now_ms: i64, step: &mut Step) {
        if now_ms > self.metered_to_ms {
            self.meter_span(self.metered_to_ms, now_ms, step);
            self.metered_to_ms = now_ms;
        }
    }

    /// A disk's stop's time is the platform's, never its owner's: its final
    /// flush awaited, kept while its storage is unreachable (FIN-783), for
    /// as long as the start that runs was told, whatever the deployment says
    /// now. Free from its beginning (or its unsaved window's) until it is
    /// gone, which closes it (`gone`). Off a disk, nothing.
    pub(super) fn stop_free(&mut self, now_ms: i64) {
        assert!(matches!(self.saving, Some(Saving::Stop { .. })), "a stop's window opens with its stop");
        if self.disk {
            self.unmetered = Some((self.unmetered.map_or(now_ms, |(from, _)| from.min(now_ms)), i64::MAX));
        }
    }

    /// Awake time `[from, to)`, but what falls in the unsaved window, and
    /// none while always on.
    fn meter_span(&self, from: i64, to: i64, step: &mut Step) {
        if from < to && !self.always_on {
            let (free_from, free_to) = self.unmetered.unwrap_or((to, to));
            assert!(free_from <= free_to, "a window runs forward");
            if from < free_from.min(to) {
                step.actions.push(Action::Meter { from_ms: from, to_ms: free_from.min(to) });
            }
            if free_to.max(from) < to && self.unmetered.is_some() {
                step.actions.push(Action::Meter { from_ms: free_to.max(from), to_ms: to });
            }
        }
    }

    /// The last moment its running container was shown up: its last meter,
    /// its phase's beginning or its sleep's step's, or work done on it.
    fn shown_up_ms(&self) -> i64 {
        let phase = match self.phase {
            Phase::Awake { since_ms, .. } | Phase::Sleeping { since_ms, .. } => since_ms,
            Phase::Asleep { .. } | Phase::Starting { .. } | Phase::Failed { .. } => i64::MIN,
        };
        let step = self.saving.map_or(i64::MIN, Saving::since_ms);
        self.metered_to_ms.max(phase).max(step).max(self.worked_ms)
    }

    fn next_alarm(&self) -> Option<i64> {
        match &self.phase {
            Phase::Asleep { retry } => retry.map(|r| r.at_ms),
            Phase::Failed { .. } => None,
            Phase::Starting { since_ms, .. } => Some(since_ms + START_DEADLINE_MS),
            Phase::Sleeping { since_ms, .. } => Some(self.saving.unwrap_or(Saving::Stop { since_ms: *since_ms, saved: false }).deadline_ms()),
            Phase::Awake { .. } => {
                let meter = self.metered_to_ms + METER_EVERY_MS;
                let mut at = match self.idle_at_ms() {
                    Some(idle) => meter.min(idle),
                    None => meter,
                };
                match self.saving {
                    Some(s) => at = at.min(s.deadline_ms()),
                    // a disk is saved awake only for its owner's export
                    None if self.disk => {
                        if let Some(due) = self.export_due_ms {
                            at = at.min(due);
                        }
                    }
                    None => {
                        if let Some(due) = self.save_due_ms.filter(|_| self.keepalives == 0) {
                            at = at.min(due);
                        }
                        if let Some(due) = self.busy_save_due_ms() {
                            at = at.min(due);
                        }
                    }
                }
                Some(at)
            }
        }
    }

    /// Applies `event` at `now_ms`, under the default rules.
    pub fn apply(&mut self, event: Event, now_ms: i64) -> Step {
        self.apply_with(event, now_ms, &Rules::default())
    }

    /// Applies `event` at `now_ms`, under the deployment's `rules`.
    pub fn apply_with(&mut self, event: Event, now_ms: i64, rules: &Rules) -> Step {
        assert!(rules.unsaved_max_ms >= 0, "a bound is a span of time");
        let mut step = Step::default();
        // what was metered, or passed over as silence, as this event came
        let metered_before = self.metered_to_ms;
        // heard late (`ALARM_LATE_MS`): metered to the last moment it was
        // shown up, and none of the time since (FIN-511); a sleep's try
        // under way, which this event may fail (its whole time free:
        // `begin_unsaved`), no further than its start
        if matches!(self.phase, Phase::Awake { .. } | Phase::Sleeping { .. }) {
            let shown = self.shown_up_ms();
            if now_ms > shown + ALARM_LATE_MS {
                let tried = match (&self.phase, self.saving) {
                    (Phase::Sleeping { .. }, Some(Saving::Hold { .. } | Saving::Save { .. })) => self.save_asked_ms,
                    _ => i64::MAX,
                };
                self.meter(shown.min(tried), &mut step);
                self.metered_to_ms = now_ms;
            }
        }
        match (self.phase.clone(), event) {
            (_, Event::Posted { at_ms }) => {
                // a guest at work queues it; one whose last work began after
                // it was posted took it up
                if self.keepalives == 0 && at_ms > self.opened_ms {
                    self.owed_ms = Some(self.owed_ms.map_or(at_ms, |owed| owed.min(at_ms)));
                }
            }
            (phase, Event::Wake { why }) => {
                // a message for its agents, or a fragment to follow: work
                if matches!(why, Wake::Record | Wake::Joined) {
                    self.worked(now_ms);
                }
                match phase {
                    Phase::Failed { why: failed, since_ms } if why != Wake::Owner && now_ms < since_ms + WONT_WAKE_RETRY_MS => {
                        step.refused = Some(format!("it won't wake: {failed} (its owner can try again)"));
                    }
                    Phase::Failed { .. } if why != Wake::Owner => {
                        // its pause passed and something wants it: one try,
                        // its last strike (failing, it gives up again at once)
                        self.failures = STARTS_FAILED_MAX - 1;
                        self.restore_fallbacks = 0;
                        self.blocked_since_ms = None;
                        self.hold(now_ms + why.hold_ms());
                        self.start(now_ms, &mut step);
                    }
                    Phase::Failed { .. } | Phase::Asleep { .. } => {
                        self.failures = 0;
                        self.restore_fallbacks = 0;
                        self.blocked_since_ms = None;
                        self.hold(now_ms + why.hold_ms());
                        self.start(now_ms, &mut step);
                    }
                    Phase::Sleeping { generation, .. } if self.cancels_sleep() => {
                        // wanted before its idle sleep's stop began: it
                        // stays awake on the container it has
                        self.hold(now_ms + why.hold_ms());
                        self.sleep_cancelled(generation, now_ms, &mut step);
                    }
                    Phase::Sleeping { generation, since_ms, owner, restarting, .. } => {
                        // its owner's sleep, or one whose stop began: a
                        // start once its container is gone
                        self.hold(now_ms + why.hold_ms());
                        self.phase = Phase::Sleeping { generation, since_ms, owner, restarting, owes_start: true };
                    }
                    Phase::Starting { .. } | Phase::Awake { .. } => self.hold(now_ms + why.hold_ms()),
                }
            }
            (_, Event::Restart { generation: asked }) if !self.restarts(asked) => {
                // an earlier start's restart, made already: nothing
            }
            (phase, Event::Restart { .. }) => {
                // what it starts to is held as its owner's wake holds it
                self.hold(now_ms + IDLE_MS);
                match phase {
                    Phase::Asleep { .. } | Phase::Failed { .. } => {
                        // as its owner's wake: a won't-wake computer tries
                        // again. Its start is owed, as after the stop of one
                        // awake: made by the alarm it asks for, never by the
                        // restart's own request, so a restart answers
                        // `starting` whatever it restarted (`starting`)
                        self.failures = 0;
                        self.restore_fallbacks = 0;
                        self.blocked_since_ms = None;
                        self.phase = Phase::Asleep { retry: Some(Retry { at_ms: now_ms, owed: true }) };
                    }
                    // the start under way is the restart's
                    Phase::Starting { .. } => {}
                    Phase::Awake { generation, .. } => {
                        // its keepalives close with its container, as at its
                        // owner's sleep; its tabs close as the sleep begins,
                        // each close (and a use it carries) heard
                        self.keepalives = 0;
                        self.sleep(generation, now_ms, (true, true, true), &mut step);
                    }
                    Phase::Sleeping { generation, since_ms, .. } => {
                        // the sleep under way is the restart's: nothing cancels it
                        self.phase = Phase::Sleeping { generation, since_ms, owner: true, restarting: true, owes_start: true };
                    }
                }
            }
            // its owner's export asks a disk for a save: due now, or when
            // one that failed may go again (asked again meanwhile, it waits
            // its pause), taken by its alarm once it runs (the export wakes
            // it), never in the ask's own step: the export only looks. A
            // computer with saves of its own has its newest, and asks none
            (_, Event::ExportAsked { since_ms }) if self.disk && self.running().is_some() && self.saved_asked_ms.is_none_or(|at| at < since_ms) => {
                self.export_due_ms.get_or_insert(now_ms);
            }
            (_, Event::ExportAsked { .. }) => {}
            (_, Event::ExportDropped) => self.export_due_ms = None,
            (Phase::Asleep { .. } | Phase::Failed { .. }, Event::SaveChosen) => {
                self.failures = 0;
                self.restore_fallbacks = 0;
                self.blocked_since_ms = None;
                if matches!(self.phase, Phase::Failed { .. }) {
                    self.phase = Phase::ASLEEP;
                }
            }
            (_, Event::SaveChosen) => {}
            (Phase::Starting { generation, .. }, Event::Ready { generation: g }) if g == generation => {
                self.failures = 0;
                self.blocked_since_ms = None;
                self.fell_back = false;
                self.ready = true;
                self.metered_to_ms = now_ms;
                self.phase = Phase::Awake { generation, since_ms: now_ms };
                self.begin_life(now_ms);
            }
            (Phase::Starting { generation, .. }, Event::StartFailed { generation: g, why }) if g == generation => self.failed(why, now_ms),
            (Phase::Starting { generation, .. }, Event::StartBlocked { generation: g }) if g == generation => {
                // nothing of it ran: made again at once, as it was asked,
                // no strike, while the container before it may still be its
                // sleep's, held by a snapshot; then each try is a failure
                let since = *self.blocked_since_ms.get_or_insert(now_ms);
                if now_ms < since + SLEEP_DEADLINE_MS {
                    self.phase = Phase::Asleep { retry: Some(Retry { at_ms: now_ms, owed: true }) };
                } else {
                    self.failed(format!("the container before it still ran {} s after its first try", (now_ms - since) / 1000), now_ms);
                }
            }
            (Phase::Starting { generation, .. }, Event::SnapshotFailed { generation: g, why }) if g == generation => {
                if std::mem::replace(&mut self.fell_back, true) {
                    // its snapshot went at the first: this one is the image's
                    self.failed(why, now_ms);
                } else {
                    // never awake: nothing since it last was is metered, and
                    // it starts again (from the image and the save) while wanted
                    self.metered_to_ms = now_ms;
                    self.gone(now_ms, &mut step);
                }
            }
            (Phase::Starting { generation, .. }, Event::RestoreFailed { generation: g, why, older }) if g == generation => {
                if older && (self.restore_fallbacks as usize) < SAVES_KEPT {
                    // the save was at fault, not the computer: the one
                    // before it, at once, metering nothing (it was never up),
                    // owed whether or not what woke it still holds it (a big
                    // save's restore takes minutes, and three can outlast a
                    // wake's hold: Codex on the no-room PR)
                    self.restore_fallbacks += 1;
                    self.metered_to_ms = now_ms;
                    self.gone(now_ms, &mut step);
                    self.phase = Phase::Asleep { retry: Some(Retry { at_ms: now_ms, owed: true }) };
                } else {
                    self.failed(why, now_ms);
                }
            }
            // a completion of an earlier start, or a report of one that is
            // not running: nothing it says is true of this one
            (
                _,
                Event::Ready { .. }
                | Event::StartFailed { .. }
                | Event::SnapshotFailed { .. }
                | Event::RestoreFailed { .. }
                | Event::Held { .. }
                | Event::HomeSaved { .. }
                | Event::Saved { .. }
                | Event::SaveFailed { .. }
                | Event::Asleep { .. }
                | Event::AsleepUnshipped { .. }
                | Event::Exited { .. }
                | Event::StartFailing { .. }
                | Event::StartBlocked { .. },
            ) if !matches!(&self.phase, Phase::Sleeping { .. } | Phase::Awake { .. } | Phase::Starting { .. }) => {}
            (Phase::Starting { generation, since_ms, .. }, Event::StartFailing { generation: g }) if g == generation => {
                self.phase = Phase::Starting { generation, since_ms, destroying: true };
            }
            // its own destroy (`StartFailing`): the start's own report counts,
            // a restore's or a snapshot's fallback no strike (#1004: as a
            // crash, one transient error was three strikes in three seconds)
            (Phase::Starting { generation, destroying: true, .. }, Event::Exited { generation: g }) if g == generation => {}
            (Phase::Sleeping { generation, owes_start, .. }, Event::Asleep { generation: g }) if g == generation => {
                // a sleep that owes no start let go of any wake it held: its
                // hold ran out, or its owner put it to sleep
                if !owes_start {
                    self.owed_ms = None;
                }
                self.end_life(LifeEnd::Sleep, now_ms, &mut step);
                self.gone(now_ms, &mut step);
            }
            (Phase::Sleeping { generation, owes_start, .. }, Event::AsleepUnshipped { generation: g }) if g == generation => {
                // its final flush ended in no clean head: its last moments'
                // writes may not have shipped, so its life ends unsaved,
                // saying so, never recorded as saved
                if let Some(Saving::Stop { since_ms, .. }) = self.saving {
                    self.saving = Some(Saving::Stop { since_ms, saved: false });
                }
                step.note = SaveNote::Says("its sleep's final flush did not finish: what it wrote in its last moments may not have shipped".into());
                if !owes_start {
                    self.owed_ms = None;
                }
                self.end_life(LifeEnd::Sleep, now_ms, &mut step);
                self.gone(now_ms, &mut step);
            }
            (Phase::Starting { generation, .. }, Event::Exited { generation: g }) if g == generation => {
                // it died before it was ready: a start that failed, one strike
                // and its pause, as the start's own report is. The two race
                // (a start that fails destroys what it started, and that
                // container's end reports too): whichever comes first counts,
                // and the other is of a start no longer under way. It was
                // never awake, so nothing since it last was is metered (its
                // sleep, its start). Its sockets went with it; a guest that
                // held one is held as a record holds it, so its retry runs.
                self.metered_to_ms = now_ms;
                if self.keepalives > 0 {
                    self.hold(now_ms + IDLE_MS);
                }
                self.keepalives = 0;
                self.tabs = 0;
                self.failed(format!("its container kept stopping: start {generation} stopped before it was ready"), now_ms);
            }
            // stopped unasked while idle, all it did kept: a sleep
            (Phase::Awake { generation, .. }, Event::Exited { generation: g }) if g == generation && self.idle_and_kept(now_ms) => self.slept_unasked(now_ms, &mut step),
            (Phase::Awake { generation, .. }, Event::Exited { generation: g }) if g == generation => {
                // a guest that died busy had turns open: its next life ends
                // them, so it is held as a record holds it, and starts again
                // however long the turn ran (docs/computers.md, Lifecycle).
                if self.keepalives > 0 {
                    self.hold(now_ms + IDLE_MS);
                }
                self.end_life(LifeEnd::Exit, now_ms, &mut step);
                // a container that died under us counts against its starts
                self.failures += 1;
                if self.failures >= STARTS_FAILED_MAX {
                    self.meter(now_ms, &mut step);
                    self.saving = None;
                    self.save_due_ms = None;
                    self.busy_since_ms = None;
                    self.unsaved_since_ms = None;
                    self.unmetered = None;
                    self.failing = None;
                    self.save_failures = 0;
                    self.phase = Phase::Failed { why: "its container kept stopping".into(), since_ms: now_ms };
                } else {
                    self.gone(now_ms, &mut step);
                }
            }
            // its stop signalled it: the stop's own end (`Asleep`) reports
            // that, so the start after it (a restart's, a wake's) is owed by
            // the stop's end, made by whatever performed the stop (the
            // restart's own request) or the alarm it asks for, never by this
            // report, a request of its own
            (Phase::Sleeping { generation, .. }, Event::Exited { generation: g }) if g == generation && matches!(self.saving, Some(Saving::Stop { .. })) => {}
            (Phase::Sleeping { generation, .. }, Event::Exited { generation: g }) if g == generation => {
                // it died as it was held or saved, before the sleep stopped it
                self.end_life(LifeEnd::Exit, now_ms, &mut step);
                self.gone(now_ms, &mut step);
            }
            (Phase::Sleeping { generation, .. }, Event::Held { generation: g, held }) if g == generation && self.disk && matches!(self.saving, Some(Saving::Hold { .. })) => {
                if held {
                    // a disk's hold waited for it to ship what it held:
                    // nothing to save, its stop now (its final flush sends
                    // the writes since its last look)
                    self.saved(now_ms, &mut step);
                    self.saving = Some(Saving::Stop { since_ms: now_ms, saved: true });
                    self.stop_free(now_ms);
                    step.actions.push(Action::Stop { generation, saved: true });
                } else {
                    // its wait ended with writes still unshipped, or no word
                    // of them: it never stops with them in it. As a sleep
                    // whose save failed, it stays awake on its container and
                    // its next sleep waits again; past the bound it stops
                    // unsaved, saying so (Codex on #1552)
                    self.sleep_save_failed(generation, DISK_UNSHIPPED, now_ms, rules, &mut step);
                }
            }
            (Phase::Awake { generation, .. } | Phase::Sleeping { generation, .. }, Event::Held { generation: g, held }) if g == generation && matches!(self.saving, Some(Saving::Hold { .. })) => {
                self.save(generation, held, now_ms, &mut step);
            }
            (Phase::Awake { .. } | Phase::Sleeping { .. }, Event::HomeSaved { generation, seq }) => self.home_saved(generation, seq, &mut step),
            (Phase::Awake { generation, .. }, Event::Held { generation: g, .. }) if g == generation && self.saving.is_none() => {
                // a hold whose sleep was cancelled as it was made: its touch
                // may have landed after the cancel let go of it, so it is
                // let go again, and no guest stays held for nothing
                step.actions.push(Action::Unhold { generation });
            }
            (Phase::Awake { generation, .. }, Event::Saved { generation: g, seq }) if g == generation && matches!(self.saving, Some(Saving::Save { seq: s, .. }) if s == seq) => {
                // let go again, though it let go once its home was in: that
                // unhold's exec may have failed (Codex on #1378)
                self.saving = None;
                self.export_due_ms = None;
                self.saved_asked_ms = Some(self.save_asked_ms);
                self.saved(now_ms, &mut step);
                step.actions.push(Action::Unhold { generation });
            }
            (Phase::Sleeping { generation, .. }, Event::Saved { generation: g, seq }) if g == generation && matches!(self.saving, Some(Saving::Save { seq: s, .. }) if s == seq) => {
                // its try heard late (its meter passed over the time since,
                // stopping at its start) and saved: what it was shown
                // running for up to its save's start is its owner's, from
                // its sleep or what was metered or passed over before this
                // event, whichever is later, so never a silence passed over
                // before it (a hold asked again after one). One passed over
                // by a late event before its outcome keeps what it showed
                // before that silence unbilled: never charged falsely
                if let Some(Saving::Save { since_ms, .. }) = self.saving {
                    if self.metered_to_ms > since_ms {
                        self.meter_span(self.save_asked_ms.max(metered_before), since_ms, &mut step);
                    }
                }
                self.export_due_ms = None;
                self.saved_asked_ms = Some(self.save_asked_ms);
                self.saved(now_ms, &mut step);
                self.saving = Some(Saving::Stop { since_ms: now_ms, saved: true });
                self.stop_free(now_ms);
                step.actions.push(Action::Stop { generation, saved: true });
            }
            (Phase::Awake { generation, .. }, Event::SaveFailed { generation: g, seq, why }) if g == generation && self.answers(seq) => {
                // awake, it is tried again after a pause; a sleep saves anyway
                self.awake_save_failed(generation, &why, now_ms, &mut step);
            }
            (Phase::Sleeping { generation, .. }, Event::SaveFailed { generation: g, seq, why }) if g == generation && self.answers(seq) => {
                self.sleep_save_failed(generation, &why, now_ms, rules, &mut step);
            }
            (
                _,
                Event::Ready { .. }
                | Event::StartFailed { .. }
                | Event::SnapshotFailed { .. }
                | Event::RestoreFailed { .. }
                | Event::Held { .. }
                | Event::HomeSaved { .. }
                | Event::Saved { .. }
                | Event::SaveFailed { .. }
                | Event::Asleep { .. }
                | Event::AsleepUnshipped { .. }
                | Event::Exited { .. }
                | Event::StartFailing { .. }
                | Event::StartBlocked { .. },
            ) => {}
            (_, Event::Opened { socket }) => {
                match socket {
                    Socket::Keepalive => {
                        self.keepalives += 1;
                        // its guest took up the work it was woken for
                        self.owed_ms = None;
                        self.opened_ms = self.opened_ms.max(now_ms);
                        self.changed(now_ms);
                    }
                    Socket::Tab => self.tabs += 1,
                }
                self.worked(now_ms);
                match self.phase {
                    Phase::Asleep { .. } => {
                        self.hold(now_ms + IDLE_MS);
                        self.start(now_ms, &mut step);
                    }
                    // a tab's opening holds it as a record does, and only
                    // its use after holds it longer
                    Phase::Starting { .. } | Phase::Awake { .. } if socket == Socket::Tab => self.hold(now_ms + IDLE_MS),
                    Phase::Awake { .. } if socket == Socket::Keepalive => {
                        if self.keepalives == 1 {
                            self.busy_since_ms = Some(now_ms);
                        }
                        // work began again: its end is what saves
                        self.save_due_ms = None;
                    }
                    Phase::Sleeping { generation, owner: false, .. } if socket == Socket::Keepalive && matches!(self.saving, Some(Saving::Hold { .. })) => {
                        // the guest took work as its idle sleep held it: it
                        // stays awake, and nothing it claimed is cut (P2)
                        self.sleep_cancelled(generation, now_ms, &mut step);
                    }
                    _ => {}
                }
            }
            (_, Event::Closed { socket }) => {
                let count = match socket {
                    Socket::Keepalive => &mut self.keepalives,
                    Socket::Tab => &mut self.tabs,
                };
                let was = *count;
                *count = count.saturating_sub(1);
                // one it counted (none after its owner's sleep let go of them)
                if was > 0 {
                    self.worked(now_ms);
                    if socket == Socket::Keepalive {
                        self.changed(now_ms);
                    }
                }
                // a socket that closes as its container goes down (or after)
                // holds nothing up
                if matches!(self.phase, Phase::Starting { .. } | Phase::Awake { .. }) {
                    self.hold(now_ms + IDLE_MS);
                }
                // a tab used since the last save asked: its person's work
                // through it ended, whatever other tabs it counts (one a
                // deploy cut may be counted still)
                let used = socket == Socket::Tab && was > 0 && self.used_since_ms.is_some_and(|since| since >= self.save_asked_ms);
                if (was == 1 || used) && matches!(self.phase, Phase::Awake { .. }) {
                    // work ended, its turns' or what its owner did through its
                    // ports: it is saved once it settles (a close within it
                    // puts that off, so a burst of them saves once)
                    if socket == Socket::Keepalive {
                        self.busy_since_ms = None;
                    }
                    self.save_due_ms = Some(now_ms + SAVE_SETTLE_MS);
                }
            }
            // a use of no tab it counts open (heard after its close, or as
            // its container went) is nothing
            (_, Event::TabUsed) if self.tabs == 0 || self.running().is_none() => {}
            (phase, Event::TabUsed) => {
                // a sleep closes its tabs as it begins, before its hold (the
                // cell's): a use heard during it was sent before, and is in
                // its save; it holds nothing up, nor cancels it
                self.tab_used(now_ms);
                if matches!(phase, Phase::Starting { .. } | Phase::Awake { .. }) {
                    self.hold(now_ms + IDLE_MS);
                }
            }
            (Phase::Starting { generation, .. } | Phase::Awake { generation, .. }, Event::Sleep) => {
                // a start that never came up was never metered: its sleep is
                // metered from here, as a life is from its `Ready` (FIN-511)
                if matches!(self.phase, Phase::Starting { .. }) {
                    self.metered_to_ms = now_ms;
                }
                self.held_until_ms = now_ms;
                self.keepalives = 0;
                self.sleep(generation, now_ms, (true, false, false), &mut step);
            }
            (Phase::Sleeping { generation, since_ms, .. }, Event::Sleep) => {
                // the newest ask wins: what held it is let go, as awake, so a
                // wake or a restart asked before it starts nothing after it
                self.held_until_ms = now_ms;
                self.phase = Phase::Sleeping { generation, since_ms, owner: true, restarting: false, owes_start: false };
            }
            (Phase::Asleep { retry }, Event::Sleep) => {
                // the newest ask wins, asleep as going to sleep: a start owed
                // after its stop (a wake's, a restart's), a failed start's
                // retry, and what held it, all let go; one always on is
                // wanted still, and starts again, as after a sleep of it awake
                self.held_until_ms = now_ms;
                let retry = retry.filter(|_| self.wanted(now_ms)).map(|r| Retry { owed: false, ..r });
                self.phase = Phase::Asleep { retry };
            }
            (Phase::Failed { .. }, Event::Sleep) => {}
            (_, Event::AlwaysOn { on }) => {
                self.always_on = on;
                if on && matches!(self.phase, Phase::Asleep { .. }) {
                    self.start(now_ms, &mut step);
                }
            }
            (Phase::Asleep { retry: Some(retry) }, Event::Alarm) => {
                if now_ms >= retry.at_ms {
                    if retry.owed || self.wanted(now_ms) {
                        self.start(now_ms, &mut step);
                    } else {
                        self.phase = Phase::ASLEEP;
                    }
                }
            }
            (Phase::Asleep { retry: None }, Event::Alarm) => {}
            (Phase::Starting { generation, since_ms, .. }, Event::Alarm) => {
                if now_ms >= since_ms + START_DEADLINE_MS {
                    self.failed(format!("start {generation} was not ready within {}s", START_DEADLINE_MS / 1000), now_ms);
                }
            }
            (Phase::Awake { generation, .. }, Event::Alarm) => {
                self.meter(now_ms, &mut step);
                if self.idle_at_ms().is_some_and(|idle| now_ms >= idle) {
                    // a sleep saves: any save that was due, or under way (its
                    // step lost with an isolate), is the sleep's own
                    self.sleep(generation, now_ms, (false, false, false), &mut step);
                } else if let Some(s) = self.saving {
                    if now_ms >= s.deadline_ms() {
                        self.overdue(generation, s, now_ms, rules, &mut step);
                    }
                } else if !self.disk {
                    let settled = self.keepalives == 0 && self.save_due_ms.is_some_and(|due| now_ms >= due);
                    let busy = self.busy_save_due_ms().is_some_and(|due| now_ms >= due);
                    if settled || busy {
                        self.hold_for_save(generation, now_ms, &mut step);
                    }
                } else if self.export_due_ms.is_some_and(|due| now_ms >= due) {
                    self.hold_for_save(generation, now_ms, &mut step);
                }
            }
            (Phase::Sleeping { generation, since_ms, .. }, Event::Alarm) => {
                let s = self.saving.unwrap_or(Saving::Stop { since_ms, saved: false });
                if now_ms >= s.deadline_ms() {
                    self.overdue(generation, s, now_ms, rules, &mut step);
                } else if matches!(self.saving, Some(Saving::Stop { .. })) {
                    step.actions.push(Action::Look { generation });
                }
            }
            (Phase::Failed { .. }, Event::Alarm) => {}
        }
        // an export's ask is for a computer that runs: one whose start
        // failed, or whose life ended, has none (its export asks again)
        if self.running().is_none() {
            self.export_due_ms = None;
        }
        step.alarm_ms = self.next_alarm().map(|at| match self.saving {
            // a stop under way is looked at every `STOP_LOOK_MS` up to its
            // deadline: a new isolate's look finds one its isolate lost
            Some(Saving::Stop { since_ms, .. }) => at.min(since_ms + ((now_ms - since_ms) / STOP_LOOK_MS + 1) * STOP_LOOK_MS),
            _ => at,
        });
        self.assert_valid();
        step
    }

    /// Whether a save's failure `seq` answers the save under way: its own
    /// number, or 0 from its hold (which found no container to save).
    fn answers(&self, seq: u64) -> bool {
        match self.saving {
            Some(Saving::Save { seq: s, .. }) => s == seq,
            Some(Saving::Hold { .. }) => seq == 0,
            Some(Saving::Stop { .. }) | None => false,
        }
    }

    fn assert_valid(&self) {
        match &self.phase {
            Phase::Starting { generation, .. } | Phase::Awake { generation, .. } | Phase::Sleeping { generation, .. } => {
                assert_eq!(*generation, self.generation, "the running phase is the last start's");
            }
            Phase::Asleep { .. } | Phase::Failed { .. } => {}
        }
        if let Phase::Sleeping { owner, restarting: true, owes_start, .. } = self.phase {
            assert!(owner && owes_start, "a restart is its owner's sleep, a start after it");
        }
        assert!(self.failures <= STARTS_FAILED_MAX, "failures stop counting at the limit");
        if self.ready {
            assert!(matches!(self.phase, Phase::Awake { .. } | Phase::Sleeping { .. }), "only a start that came up, and is not gone, is ready");
        }
        match self.saving {
            Some(Saving::Stop { .. }) => assert!(matches!(self.phase, Phase::Sleeping { .. }), "only a sleep stops"),
            Some(_) => assert!(matches!(self.phase, Phase::Awake { .. } | Phase::Sleeping { .. }), "a save is of a container that came up"),
            None => {}
        }
        if self.save_due_ms.is_some() || self.busy_since_ms.is_some() {
            assert!(matches!(self.phase, Phase::Awake { .. }), "only an awake computer has a save due");
        }
        if self.unsaved_since_ms.is_some() {
            assert!(matches!(self.phase, Phase::Awake { .. } | Phase::Sleeping { .. }), "a failed save keeps a running container");
        }
        assert!(self.restore_fallbacks as usize <= SAVES_KEPT, "each fallback is to an older save");
        // a free window is its unsaved window's (one stored before windows
        // were free has none: its time is metered as it was), or a disk's
        // stop's, its final flush awaited (FIN-783)
        if let Some((from, to)) = self.unmetered {
            let disks_stop = self.disk && matches!(self.saving, Some(Saving::Stop { .. }));
            assert!(disks_stop || self.unsaved_since_ms.is_some_and(|since| from <= since), "the free window opens with the unsaved one, at the start of the try that failed, or with a disk's stop");
            assert!(from <= to, "a window runs forward");
        }
        // (a lifecycle stored before `failing` was kept may count failures
        // with none told: its next failure tells it)
        if self.failing.is_some() {
            assert!(self.save_failures > 0, "a run of failed saves counts one at least");
            assert!(matches!(self.phase, Phase::Awake { .. } | Phase::Sleeping { .. }), "only a running container's saves fail");
        }
    }
}
