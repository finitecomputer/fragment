//! Its starts and lives: a start made or owed, one that fails (tried again
//! after a pause, then "won't wake" until its owner asks or the pause
//! passes), a life's beginning and end, and whether a start's guest is alive.

use fragment_proto::computer::LifeEnd;

use super::{Action, Ended, Lifecycle, Phase, Retry, Saving, Step};
use crate::computer::limits::{RETRY_PAUSE_MS, STARTS_FAILED_MAX, TAB_USED_EVERY_MS};
use crate::computer::Saves;

impl Lifecycle {
    /// Starts of this wake that failed in a row.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Whether its owner's restart of start `asked` (0: whatever runs) is
    /// one to make: it names its newest start, the one that runs, starts,
    /// sleeps, slept or would not start. A restart of an earlier one was
    /// made already (the same press, asked again): nothing now.
    pub fn restarts(&self, asked: u64) -> bool {
        asked == 0 || asked == self.generation
    }

    pub(super) fn start(&mut self, now_ms: i64, step: &mut Step) {
        self.generation += 1;
        // whatever started it, a start owed or a retry is made (`Asleep`'s
        // `retry` goes with the phase)
        self.ready = false;
        self.phase = Phase::Starting { generation: self.generation, since_ms: now_ms, destroying: false };
        step.actions.push(Action::Start { generation: self.generation });
    }

    /// A start came up: a life begins, saved from here on.
    pub(super) fn begin_life(&mut self, now_ms: i64) {
        self.saving = None;
        self.save_due_ms = None;
        self.busy_since_ms = (self.keepalives > 0).then_some(now_ms);
        self.save_asked_ms = now_ms;
        self.save_failures = 0;
        self.unsaved_since_ms = None;
        self.unmetered = None;
        self.restore_fallbacks = 0;
        self.failing = None;
        // its /data is the save it came up from; a socket open as it came
        // up is work under way
        self.kept_ms = now_ms;
        self.worked_ms = if self.open() { now_ms } else { 0 };
        self.changed_ms = Some(self.worked_ms);
        // a tab's use is its life's: none of an earlier one counts here
        self.tab_used_ms = 0;
        self.used_since_ms = None;
    }

    /// Something was done on this life: a keepalive or a port tab opened or
    /// closed, or a record or a join woke it. A socket that stays open across
    /// a save is not: the lifecycle can count one open that its guest let go
    /// of (a close lost across a deploy kept a keepalive counted open for 14
    /// hours on the preview, 2026-10-23), and a turn really running at a
    /// stop is ended as lost in its own chat.
    pub(super) fn worked(&mut self, now_ms: i64) {
        if self.ready {
            self.worked_ms = self.worked_ms.max(now_ms);
        }
    }

    /// This life ran something that may change its files: a keepalive
    /// opened or closed (a turn), or a person used a port tab. Always work
    /// too (`worked`), never the other way: a tab's opening, or a record's
    /// wake, changes nothing by itself.
    pub(super) fn changed(&mut self, now_ms: i64) {
        if self.ready {
            self.changed_ms = Some(self.changed_ms().max(now_ms));
        }
    }

    /// When this life last ran something that may change its files (its
    /// work's time, in a lifecycle stored before that was kept).
    fn changed_ms(&self) -> i64 {
        self.changed_ms.unwrap_or(self.worked_ms)
    }

    /// The running start's container is gone: if it came up, that was a
    /// life, and it ended `by` this. A sleep's container that went before
    /// the sleep stopped it (it died as it was held or saved) stopped on its
    /// own: an exit.
    pub(super) fn end_life(&mut self, by: LifeEnd, now_ms: i64, step: &mut Step) {
        let stopped = matches!(self.saving, Some(Saving::Stop { .. }));
        let by = if by == LifeEnd::Sleep && !stopped { LifeEnd::Exit } else { by };
        let saved = by == LifeEnd::Sleep && matches!(self.saving, Some(Saving::Stop { saved: true, .. }));
        self.life_ended(by, saved, now_ms, step);
    }

    /// The running start, if it came up, was a life that ended `by` this
    /// (`saved`: a sleep whose newest save holds all it did).
    fn life_ended(&mut self, by: LifeEnd, saved: bool, now_ms: i64, step: &mut Step) {
        if std::mem::take(&mut self.ready) {
            assert!(step.ended.is_none(), "one event ends one life");
            let restarting = matches!(self.phase, Phase::Sleeping { restarting: true, .. });
            let unheard = self.tab_unheard_ms(now_ms);
            let (quiet, unchanged) = (self.quiet(now_ms), self.kept_ms > 0 && self.changed_ms().max(unheard) < self.kept_ms);
            assert!(!quiet || unchanged, "a life nothing was done on changed nothing");
            step.ended = Some(Ended { generation: self.generation, by, saved, at_ms: now_ms, restart: restarting && by == LifeEnd::Sleep, quiet, unchanged });
        }
    }

    /// Nothing was done on this life since its newest save that worked, or
    /// since it came up when it has none, as far as was heard by `now`:
    /// what it ran is in that save.
    fn quiet(&self, now_ms: i64) -> bool {
        self.kept_ms > 0 && self.worked_ms.max(self.tab_unheard_ms(now_ms)) < self.kept_ms
    }

    /// The latest a person's use of an open port tab, not heard yet, may
    /// have reached its container by `now`: a use is heard
    /// `TAB_USED_EVERY_MS` after the last one heard at most (cell/entry.mjs),
    /// or with its tab's close, and one its container took with it as it
    /// stopped never is. 0 with no tab open, or none used.
    fn tab_unheard_ms(&self, now_ms: i64) -> i64 {
        if self.tabs > 0 && self.tab_used_ms > 0 {
            now_ms.min(self.tab_used_ms + TAB_USED_EVERY_MS)
        } else {
            0
        }
    }

    /// Whether its container, stopped now though nothing asked, ends a sleep
    /// and not a crash: a life that came up, awake, no keepalive open (a
    /// port tab open holds nothing: its use since its save is work), no save
    /// under way, a save it made since it was last awake (`Awake`'s
    /// `since_ms`) holds all it did (`quiet`, a tab's use not heard yet
    /// counted done), and no record or join woke
    /// it that its guest has not taken up (`owed_ms`). A host stops
    /// containers for its own reasons (its maintenance, a node short of
    /// memory); stopped so, such a computer loses nothing and leaves nothing
    /// it was asked to do undone. A wake not taken up is work no save holds
    /// (a message that came between a turn's end and its save), and a life
    /// with no save of its own may have been woken for work it has not
    /// begun, so either stop stays a crash, started again while that wake
    /// holds it.
    pub(super) fn idle_and_kept(&self, now_ms: i64) -> bool {
        let Phase::Awake { since_ms, .. } = self.phase else { return false };
        self.ready && !self.open() && self.saving.is_none() && self.quiet(now_ms) && self.kept_ms > since_ms && self.owed_ms.is_none()
    }

    /// Its container stopped unasked while it was idle and kept
    /// (`idle_and_kept`): a sleep its newest save made, no strike, and
    /// nothing holds it awake (the hold after its last work, or a wake's, is
    /// let go as its own idle sleep would have let it go), so its next want
    /// wakes it from that save. Always on, it is wanted still: it starts
    /// again, as after its owner's sleep of it.
    pub(super) fn slept_unasked(&mut self, now_ms: i64, step: &mut Step) {
        assert!(self.idle_and_kept(now_ms), "only an idle computer whose work is kept sleeps unasked");
        self.held_until_ms = now_ms;
        self.life_ended(LifeEnd::Sleep, true, now_ms, step);
        self.gone(now_ms, step);
    }

    /// A start that did not come up: tried again after a pause, or given
    /// up on.
    pub(super) fn failed(&mut self, why: String, now_ms: i64) {
        self.failures += 1;
        self.phase = if self.failures >= STARTS_FAILED_MAX {
            Phase::Failed { why, since_ms: now_ms }
        } else {
            Phase::Asleep { retry: Some(Retry { at_ms: now_ms + (RETRY_PAUSE_MS << (self.failures - 1)), owed: false }) }
        };
    }

    /// The container is gone: meter what it was awake, and start it again
    /// if something still wants it. Every socket into it went with it (a
    /// tab that comes back opens again, and wakes it), and so did any save
    /// under way.
    pub(super) fn gone(&mut self, now_ms: i64, step: &mut Step) {
        self.meter(now_ms, step);
        let owed = matches!(self.phase, Phase::Sleeping { owes_start: true, .. });
        self.phase = Phase::ASLEEP;
        self.keepalives = 0;
        self.tabs = 0;
        self.saving = None;
        self.save_due_ms = None;
        self.busy_since_ms = None;
        // metered above, its unsaved window passed over: it ends here
        self.unsaved_since_ms = None;
        self.unmetered = None;
        self.failing = None;
        self.save_failures = 0;
        // a start owed is due now, made by the alarm that asks for it (or
        // at once by a request: `start_due`), never in what is left of the
        // invocation that saw it go: a Durable Object's alarm runs for 15
        // minutes at most, and a sleep's steps take most of one, as a
        // failed restore's start does (the cell's `ALARM_WALL_MS`)
        if owed || self.wanted(now_ms) {
            self.phase = Phase::Asleep { retry: Some(Retry { at_ms: now_ms, owed: true }) };
        }
    }

    /// Whether a start is due at `now_ms` (asleep, its retry's pause or a
    /// start owed come): an alarm makes it, or a request may at once.
    pub fn start_due(&self, now_ms: i64) -> bool {
        matches!(self.phase, Phase::Asleep { retry: Some(r) } if now_ms >= r.at_ms)
    }

    /// The start due while asleep, if one is: made by the alarm (or a
    /// request) once due, if owed or something still wants it.
    fn retry(&self) -> Option<Retry> {
        match self.phase {
            Phase::Asleep { retry } => retry,
            _ => None,
        }
    }

    /// Whether its owner is told it is starting at `now_ms`: a start under
    /// way, or one owed and due (`start_due`), which its alarm makes at once
    /// (or a request before it). An owner's restart answers once its save is
    /// done or failed and its start is owed: `starting`, on every path, its
    /// generation the start it restarted until the alarm's start is made.
    pub fn starting(&self, now_ms: i64) -> bool {
        matches!(self.phase, Phase::Starting { .. }) || (self.start_due(now_ms) && self.retry().is_some_and(|r| r.owed || self.wanted(now_ms)))
    }
}

/// The start under way that has planned nothing yet (`Saves::start_from`
/// names none for it): it is still making sure the container before it is
/// gone (the cell's `earlier_runs`, its first step), so a container that
/// runs now is that one, never its own. A new isolate reports such a start
/// held up (`Event::StartBlocked`) rather than take that container over as
/// its own, and a port's request is not passed to it.
pub fn unplanned_start(life: &Lifecycle, saves: &Saves) -> Option<u64> {
    match life.phase {
        Phase::Starting { generation, destroying: false, .. } if saves.start_from(generation).is_none() => Some(generation),
        _ => None,
    }
}

/// What a start knows of its guest as it waits for it (`alive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alive {
    /// Its guest read its view since the container started: it is up.
    Heard,
    /// Not yet, and its wait has time left.
    Waiting,
    /// Its wait ran out unheard: a container that came up dead (#362).
    Dead,
}

/// Whether the guest of a start is alive at `now_ms`: the container started
/// at `started_ms`; the guest last read its own view at `heard_ms` (whose
/// guest it was: a read before the start was a container before it); and
/// the start's wait for it, `bound_ms` long, began at `waiting_ms`, once the
/// DO had done its own steps. A read after the start counts however early,
/// one before the wait included: the guest reads its view as it starts.
pub fn alive(heard_ms: Option<i64>, started_ms: i64, waiting_ms: i64, now_ms: i64, bound_ms: i64) -> Alive {
    assert!(started_ms <= waiting_ms && waiting_ms <= now_ms, "a start waits after it started, and time runs forward");
    assert!(bound_ms > 0, "a wait is bounded");
    match heard_ms {
        Some(at) if at >= started_ms => Alive::Heard,
        _ if now_ms - waiting_ms >= bound_ms => Alive::Dead,
        _ => Alive::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{ComputerNotice, LifeEnd, LossCause, RestoreSource};

    use super::*;
    use crate::computer::testing::{bare, owed, save_asked, started, Computer, IMAGE, T};
    use crate::computer::*;

    /// Goal (#1020's review): an idle sleep's hold, save and stop take most
    /// of the alarm that asked for them (a Durable Object's runs 15 minutes
    /// at most), so the start a wake during its stop owes is never made in
    /// what is left: the sleep's end asks for an alarm at once, an
    /// invocation of its own, and that alarm makes it, though the wake's
    /// own hold ran out during the stop. Valid (owed and made once), the
    /// bound (its save as long as its backups' bound, 11 minutes and more,
    /// its stop asked again at its deadline), replay (a second alarm makes
    /// no second start). Method: the lifecycle's steps, each what one
    /// invocation of the DO performs.
    #[test]
    fn a_start_owed_after_a_long_sleep_comes_from_an_alarm_of_its_own() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let slept = T + IDLE_MS;
        assert!(l.apply(Event::Alarm, slept).actions.contains(&Action::Sleep { generation: g }), "the idle sleep, in an alarm");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, slept + 2_000));
        let saved = slept + 2_000 + 11 * 60_000;
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, saved).actions, vec![Action::Stop { generation: g, saved: true }]);
        // a wake as it stops (a port tab's): its hold runs out before the stop ends
        assert!(l.apply(Event::Wake { why: Wake::Tab }, saved + 1_000).actions.is_empty());
        assert_eq!(l.apply(Event::Alarm, saved + SLEEP_DEADLINE_MS).actions, vec![Action::Stop { generation: g, saved: true }], "its stop, asked again");
        let asleep = saved + 1_000 + IDLE_MS + 10_000;
        assert!(asleep > saved + 1_000 + IDLE_MS, "the wake's hold ran out during the stop");
        let s = l.apply(Event::Asleep { generation: g }, asleep);
        assert!(s.ended.is_some_and(|e| e.by == LifeEnd::Sleep));
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "no start in what is left of the sleep's alarm: {s:?}");
        assert_eq!((s.alarm_ms, bare(&l.phase)), (Some(asleep), Phase::ASLEEP), "an alarm at once");
        // another event of the same invocation keeps it due, and its alarm
        let s = l.apply(Event::Closed { socket: Socket::Tab }, asleep);
        assert!(s.actions.is_empty() && s.alarm_ms == Some(asleep) && l.start_due(asleep), "{s:?}");
        // the alarm, a fresh invocation, makes it once
        let s = l.apply(Event::Alarm, asleep + 50);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 1 }]);
        assert!(!l.start_due(asleep + 50) && l.apply(Event::Alarm, asleep + 100).actions.is_empty(), "made once");
        // a request may make it first: a wake at once, and the alarm then nothing
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Exited { generation: g }, T + 60_000);
        assert!(l.start_due(T + 60_000));
        assert_eq!(l.apply(Event::Wake { why: Wake::Owner }, T + 60_010).actions, vec![Action::Start { generation: g + 1 }]);
        assert!(l.apply(Event::Alarm, T + 60_020).actions.is_empty(), "the start owed was made");
    }

    /// Goal (#1020's review): its owner's sleep is the newest ask, asleep
    /// too: a start owed after a sleep a wake came during its stop, not made yet,
    /// is let go, so neither the sleep's own request nor the alarm the
    /// sleep's end asked for (replayed) starts it; a failed start's retry
    /// goes the same way. (One always on keeps its start, as after a sleep
    /// of it awake: the simulation below holds that.) Method: the
    /// lifecycle's steps.
    #[test]
    fn its_owners_sleep_lets_go_of_a_start_owed() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let slept = T + IDLE_MS;
        l.apply(Event::Alarm, slept);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, slept + 2_000));
        l.apply(Event::Saved { generation: g, seq }, slept + 3_000);
        // a wake as it stops (one before would have cancelled it)
        l.apply(Event::Wake { why: Wake::Record }, slept + 3_500);
        l.apply(Event::Asleep { generation: g }, slept + 4_000);
        assert!(l.start_due(slept + 4_000), "the wake owes a start");
        let s = l.apply(Event::Sleep, slept + 4_100);
        assert!(s.actions.is_empty() && s.alarm_ms.is_none() && !l.start_due(slept + 4_100), "its request makes nothing after it: {s:?}");
        assert!(l.apply(Event::Alarm, slept + 4_200).actions.is_empty(), "nor the alarm, replayed");
        assert!(!l.wanted(slept + 4_200), "the wake's hold let go");
        // a failed start's retry, asleep: let go too
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why: Wake::Record }, T);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        l.apply(Event::StartFailed { generation, why: "pull".into() }, T + 1_000);
        let due = l.next_alarm().expect("its retry");
        assert!(l.apply(Event::Sleep, T + 2_000).actions.is_empty());
        assert!(l.apply(Event::Alarm, due).actions.is_empty() && l.next_alarm().is_none(), "no retry after its owner's sleep");
    }

    /// Goal (#1020's review): a request makes every start owed after the
    /// one before it failed, as it waits, within `STARTS_OWED_MAX`: its
    /// owner's restart whose newest save will not restore goes on to the
    /// one before it, and answers once that is up, never asleep between.
    /// Valid (the older save's start is up), the bound (the longest chain
    /// the fallbacks allow is `STARTS_OWED_MAX`, then a strike). Method:
    /// the cell's `drive_with` as a loop over the lifecycle, each start
    /// reported as the DO would.
    #[test]
    fn a_request_makes_each_start_owed_in_turn() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        c.wake();
        c.turn_saved("b2");
        let g = c.life.generation();
        // its owner's restart: the request's sleep saves b3, which will not restore
        c.saves.restart_fresh();
        assert_eq!(c.apply(Event::Restart { generation: g }).actions, vec![Action::Sleep { generation: g }]);
        c.saved_as("b3");
        let s = c.apply(Event::Asleep { generation: g });
        assert!(c.life.start_due(c.now), "{s:?}");
        let mut bad = vec!["b3"];
        let mut made = 0;
        let mut restored = None;
        while c.life.start_due(c.now) {
            made += 1;
            assert!(made <= STARTS_OWED_MAX, "a request's starts are bounded");
            let s = c.apply(Event::Alarm);
            let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("each start due is made: {s:?}") };
            match bad.pop() {
                Some(id) => {
                    let older = c.saves.unusable(id);
                    assert!(older, "an older save to fall back to");
                    c.apply(Event::RestoreFailed { generation, why: format!("{id}'s archive is corrupt"), older });
                }
                None => restored = c.come_up(generation).save,
            }
        }
        assert_eq!((made, restored.as_deref()), (2, Some("b2")), "the restart's start, then the one past its bad save");
        assert!(matches!(c.life.phase, Phase::Awake { .. }), "the request answers once it is up: {:?}", c.life.phase);
        // the longest chain: owed after a crash, past a snapshot, past every kept save
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let mut at = T + 60_000;
        l.apply(Event::Exited { generation: g }, at);
        let crashed = l.failures;
        let mut made = 0;
        while l.start_due(at) {
            made += 1;
            let s = l.apply(Event::Alarm, at);
            let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
            at += 1_000;
            let failed = if made == 1 { Event::SnapshotFailed { generation, why: "dead".into() } } else { Event::RestoreFailed { generation, why: "corrupt".into(), older: true } };
            l.apply(failed, at);
        }
        assert_eq!(made, STARTS_OWED_MAX, "the most a chain owes");
        assert!(l.failures == crashed + 1 && matches!(l.phase, Phase::Asleep { .. }) && !l.start_due(at), "then a strike, its retry after a pause: {l:?}");
    }

    /// Goal: failed starts are retried with a growing pause, then the
    /// computer won't wake until its owner asks (lesson 4).
    #[test]
    fn failing_starts_give_up_until_the_owner_asks() {
        let mut l = Lifecycle::new();
        l.apply(Event::Wake { why: Wake::Record }, T);
        let s = l.apply(Event::StartFailed { generation: 1, why: "pull failed".into() }, T + 1_000);
        assert_eq!((bare(&l.phase), s.alarm_ms), (Phase::ASLEEP, Some(T + 1_000 + RETRY_PAUSE_MS)));
        let s = l.apply(Event::Alarm, T + 1_000 + RETRY_PAUSE_MS);
        assert_eq!(s.actions, vec![Action::Start { generation: 2 }]);
        // the next never reports ready: its deadline fails it
        let s = l.apply(Event::Alarm, T + 1_000 + RETRY_PAUSE_MS + START_DEADLINE_MS);
        assert_eq!(s.alarm_ms, Some(T + 1_000 + RETRY_PAUSE_MS + START_DEADLINE_MS + 2 * RETRY_PAUSE_MS));
        let at = s.alarm_ms.unwrap();
        l.apply(Event::Alarm, at);
        l.apply(Event::StartFailed { generation: 3, why: "pull failed".into() }, at + 1_000);
        assert!(matches!(&l.phase, Phase::Failed { why, .. } if why == "pull failed"));
        let s = l.apply(Event::Wake { why: Wake::Record }, at + 2_000);
        assert!(s.actions.is_empty() && s.refused.is_some(), "a record does not wake it: {s:?}");
        let s = l.apply(Event::Wake { why: Wake::Owner }, at + 3_000);
        assert_eq!(s.actions, vec![Action::Start { generation: 4 }]);
    }

    /// Goal (2026-11-03, a computer refused every wake for four days after
    /// one storage error): "won't wake" is never for good. A wake not its
    /// owner's, `WONT_WAKE_RETRY_MS` after it gave up, tries one start, its
    /// last strike: failing, it gives up again at once, its pause anew;
    /// coming up, it is a computer like any. Sooner, such a wake is refused;
    /// its owner's never is. Valid, invalid (too soon), replay (a second
    /// wake as it starts), and a lifecycle stored before `since_ms` (its
    /// next wake tries). Method: the lifecycle's steps, as the DO hears them.
    #[test]
    fn a_computer_that_wont_wake_tries_again_after_its_pause() {
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why: Wake::Record }, T);
        let (mut at, mut next) = (T, s.actions.first().cloned());
        let mut gave_up = 0;
        while let Some(Action::Start { generation }) = next {
            at += 1_000;
            let s = l.apply(Event::StartFailed { generation, why: "storage error".into() }, at);
            gave_up = at;
            next = s.alarm_ms.and_then(|retry| {
                at = retry;
                l.apply(Event::Alarm, at).actions.first().cloned()
            });
        }
        assert!(matches!(&l.phase, Phase::Failed { why, .. } if why == "storage error") && l.generation() == STARTS_FAILED_MAX as u64, "{l:?}");
        // too soon: refused, as before
        let s = l.apply(Event::Wake { why: Wake::Tab }, gave_up + WONT_WAKE_RETRY_MS - 1);
        assert!(s.actions.is_empty() && s.refused.is_some(), "{s:?}");
        // its pause passed: one try, its last strike
        let s = l.apply(Event::Wake { why: Wake::Record }, gave_up + WONT_WAKE_RETRY_MS);
        let g = STARTS_FAILED_MAX as u64 + 1;
        assert_eq!((s.actions, s.refused, l.failures), (vec![Action::Start { generation: g }], None, STARTS_FAILED_MAX - 1));
        // another wake as it starts holds it, and starts nothing more
        assert!(l.apply(Event::Wake { why: Wake::Tab }, gave_up + WONT_WAKE_RETRY_MS + 1).actions.is_empty());
        // it fails again: it gives up at once, and its pause runs anew
        let again = gave_up + WONT_WAKE_RETRY_MS + 5_000;
        let s = l.apply(Event::StartFailed { generation: g, why: "storage error".into() }, again);
        assert!(matches!(l.phase, Phase::Failed { .. }) && s.alarm_ms.is_none(), "{s:?}");
        assert!(l.apply(Event::Wake { why: Wake::Joined }, again + WONT_WAKE_RETRY_MS - 1).refused.is_some());
        // its owner's wake never waits
        let mut owned = l.clone();
        assert_eq!(owned.apply(Event::Wake { why: Wake::Owner }, again + 1).actions, vec![Action::Start { generation: g + 1 }]);
        assert_eq!(owned.failures, 0, "its owner's wake starts the count again");
        // past it, a tab tries, and it comes up: a computer like any
        let s = l.apply(Event::Wake { why: Wake::Tab }, again + WONT_WAKE_RETRY_MS);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 1 }]);
        l.apply(Event::Ready { generation: g + 1 }, again + WONT_WAKE_RETRY_MS + 2_000);
        assert!(matches!(l.phase, Phase::Awake { .. }) && l.failures == 0, "{l:?}");
        // a lifecycle stored before it kept when it gave up tries at its next wake
        let mut stored = serde_json::to_value(Lifecycle { phase: Phase::Failed { why: "old".into(), since_ms: T }, ..Lifecycle::new() }).unwrap();
        assert!(stored["phase"].as_object_mut().unwrap().remove("since_ms").is_some());
        let (mut old, _) = Lifecycle::read(&stored.to_string()).unwrap();
        assert_eq!(old.apply(Event::Wake { why: Wake::Tab }, T).actions, vec![Action::Start { generation: 1 }]);
    }

    /// Goal (big-disk snapshot wakes on preview, 2026-11-18: 4 of 14 wakes
    /// on master failed so): a sleep's snapshot the runtime still takes
    /// holds its container past the sleep's destroy, so the start after it
    /// finds that container running. That start started nothing: no strike.
    /// It is made again at once (owed: its alarm's, or the waiting
    /// request's), told as starting all along, until the container before
    /// it has had a stop's deadline since the first such try; past that
    /// each try is a failure as any, so one that never goes gives up as
    /// failing starts do. Valid (held up, then up, its count clean), invalid
    /// (an earlier start's report; past the bound), replay (the same report
    /// twice), restart (the chain read back from storage keeps its bound).
    /// Method: the lifecycle's steps, as the DO hears them, each try a
    /// destroy's 15 s wait.
    #[test]
    fn a_start_held_up_by_the_container_before_it_waits_without_a_strike() {
        let mut l = Lifecycle::new();
        assert_eq!(l.apply(Event::Wake { why: Wake::Owner }, T).actions, vec![Action::Start { generation: 1 }]);
        let first = T + 15_000;
        let s = l.apply(Event::StartBlocked { generation: 1 }, first);
        assert!(s.actions.is_empty() && l.failures() == 0, "no strike, and no start in its own step: {s:?} {l:?}");
        assert_eq!(s.alarm_ms, Some(first), "its alarm at once");
        assert!(l.start_due(first) && l.starting(first), "told as starting: {l:?}");
        // replay, and an earlier start's report: nothing
        let before = l.clone();
        l.apply(Event::StartBlocked { generation: 1 }, first + 1);
        l.apply(Event::StartBlocked { generation: 0 }, first + 2);
        assert_eq!(l, before);
        let (mut at, mut g) = (first, 1);
        for _ in 0..3 {
            let s = l.apply(Event::Alarm, at);
            g += 1;
            assert_eq!(s.actions, vec![Action::Start { generation: g }]);
            at += 15_000;
            l.apply(Event::StartBlocked { generation: g }, at);
            assert!(l.failures() == 0 && l.starting(at), "{l:?}");
            // its isolate gone: read back, the same chain and bound
            let (read, _) = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap();
            assert_eq!(read, l);
        }
        // the container before it goes: the next comes up, its count clean
        let s = l.apply(Event::Alarm, at);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 1 }]);
        l.apply(Event::Ready { generation: g + 1 }, at + 3_000);
        assert!(matches!(l.phase, Phase::Awake { .. }) && l.failures() == 0 && l.blocked_since_ms.is_none(), "{l:?}");

        // one that never goes: tried at once for a stop's deadline, then
        // each try a strike, and it gives up as failing starts do
        let mut l = Lifecycle::new();
        l.apply(Event::Wake { why: Wake::Record }, T);
        let (mut at, mut g, mut struck_at) = (T, 1, None);
        // bounded: SLEEP_DEADLINE_MS of tries 15 s apart, then the strikes
        while !matches!(l.phase, Phase::Failed { .. }) {
            assert!(g < 64, "the bound ends the tries: {l:?}");
            at += 15_000;
            let s = l.apply(Event::StartBlocked { generation: g }, at);
            if l.failures() > 0 {
                struck_at.get_or_insert(at);
            } else {
                assert_eq!(s.alarm_ms, Some(at), "no strike yet: made again at once");
            }
            if let Some(next) = s.alarm_ms {
                at = at.max(next);
                g += 1;
                assert_eq!(l.apply(Event::Alarm, at).actions, vec![Action::Start { generation: g }]);
            }
        }
        let struck_at = struck_at.expect("struck");
        assert!((T + 15_000 + SLEEP_DEADLINE_MS..T + 30_000 + SLEEP_DEADLINE_MS).contains(&struck_at), "the first strike once a stop's deadline passed: {struck_at}");
        assert!(matches!(&l.phase, Phase::Failed { why, .. } if why.contains("still ran")), "{l:?}");
        assert!(l.failures() == STARTS_FAILED_MAX);
    }

    /// Goal (Codex on #1380): a start held up by the container before it
    /// is in `Starting` while it waits for that container to go, having
    /// planned nothing. Its isolate gone then (a deploy), the new one must
    /// not take that container over as the start's own (its exit would be
    /// a strike, and nothing of the start ran): the start is reported held
    /// up and made again. Valid (unplanned: held up), invalid (planned,
    /// destroying, awake: taken over as before), replay (once reported, it
    /// is asleep with its start owed: nothing more), restart (both read back
    /// from storage give the same answer). Method: the lifecycle and the
    /// saves as the DO keeps them.
    #[test]
    fn a_start_still_clearing_the_one_before_it_is_never_taken_over() {
        let mut c = Computer::new();
        assert_eq!(c.apply(Event::Wake { why: Wake::Tab }).actions, vec![Action::Start { generation: 1 }]);
        assert_eq!(unplanned_start(&c.life, &c.saves), Some(1), "nothing planned: whatever runs is not its own");
        // restart: stored and read back, the same answer
        let (life, _) = Lifecycle::read(&serde_json::to_string(&c.life).unwrap()).unwrap();
        let saves: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert_eq!(unplanned_start(&life, &saves), Some(1));
        // the new isolate's report: held up, no strike, made again at once
        let s = c.apply(Event::StartBlocked { generation: 1 });
        assert!(c.life.failures() == 0 && c.life.start_due(c.now) && s.alarm_ms == Some(c.now), "{:?}", c.life);
        assert_eq!(unplanned_start(&c.life, &c.saves), None, "replay: asleep, its start owed");
        let s = c.apply(Event::Alarm);
        assert_eq!(s.actions, vec![Action::Start { generation: 2 }]);
        // planned (past the container before it): its own, taken over as before
        c.saves.starting(2, RestoreSource::Nothing);
        assert_eq!(unplanned_start(&c.life, &c.saves), None);
        // destroying what it started, or awake: never this question
        c.apply(Event::StartFailing { generation: 2 });
        assert_eq!(unplanned_start(&c.life, &c.saves), None);
        let mut l = Lifecycle::new();
        started(&mut l, T);
        assert_eq!(unplanned_start(&l, &Saves::default()), None, "awake");
    }

    /// Goal: a container that stops on its own is started again while it
    /// is wanted, and metered up to its stop.
    #[test]
    fn an_exit_restarts_it_while_wanted() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let s = l.apply(Event::Exited { generation: g }, T + 60_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: T + 3_000, to_ms: T + 60_000 }]);
        assert_eq!(owed(&mut l, &s, T + 60_000).actions, vec![Action::Start { generation: g + 1 }]);
        // once nothing wants it, an exit leaves it asleep
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let s = l.apply(Event::Exited { generation: g }, T + IDLE_MS + 1);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })));
        assert_eq!(l.phase, Phase::ASLEEP);
    }

    /// What its owner is told now.
    fn told(c: &Computer) -> Vec<ComputerNotice> {
        notices(&c.life, &c.saves, &c.images, &Rules::default())
    }

    /// Goal: a container that stops unasked while its computer is idle,
    /// with all it did in a save of this life's own (its host's own stop:
    /// a host's maintenance, a node short of memory), is a sleep: no
    /// strike, no rollback, nothing told, and nothing starts it again,
    /// though the hold after its last turn still runs. Its next wake
    /// restores that save, as after any sleep. Valid (that exit), replay
    /// (its report again, and its old alarm, change nothing), restart (the
    /// isolate that hears it reads its stored lifecycle), and the wake after
    /// it. Method: the computer as the DO drives it, five minutes into the
    /// twenty its turn's end holds it.
    #[test]
    fn an_exit_while_idle_and_saved_is_a_sleep() {
        let mut c = Computer::new();
        c.wake();
        c.turn_saved("b1");
        let g = c.life.generation();
        c.now += 5 * 60_000;
        assert!(c.life.wanted(c.now), "its turn's end still holds it");
        // restart: the isolate that hears the exit reads it back
        c.life = Lifecycle::read(&serde_json::to_string(&c.life).unwrap()).unwrap().0;
        let s = c.apply(Event::Exited { generation: g });
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: c.now, restart: false, quiet: true, unchanged: true }), "{s:?}");
        assert_eq!((c.life.phase.clone(), c.life.failures(), s.alarm_ms), (Phase::ASLEEP, 0, None), "asleep with no strike and no start due: {s:?}");
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "{s:?}");
        assert_eq!(told(&c), vec![], "nothing to tell");
        // replay: its report again, then the alarm it had set
        for e in [Event::Exited { generation: g }, Event::Alarm] {
            let s = c.apply(e);
            assert!(s.actions.is_empty() && s.ended.is_none() && s.alarm_ms.is_none(), "{s:?}");
            assert_eq!((c.life.phase.clone(), c.life.failures()), (Phase::ASLEEP, 0));
        }
        // its next want wakes it from that save: no rollback, nothing told
        let s = c.apply(Event::Wake { why: Wake::Record });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.from, r.save.as_deref(), r.after, r.rollback, c.saves.rollbacks()), (RestoreSource::Backup, Some("b1"), Some(LifeEnd::Sleep), false, 0), "{r:?}");
        assert_eq!(told(&c), vec![]);
    }

    /// Goal: a message its guest took up owes nothing, however its wake
    /// arrives: before the turn (a keepalive opened after it), during it
    /// (a guest at work queues what comes), or after the turn ended, as a
    /// queued wake does once its record was already answered (it was
    /// posted before that turn's keepalive closed). The exit after the
    /// turn's save is a sleep each time. Replay: the same late wake again
    /// owes nothing. Invalid: one posted after the last turn ended owes it
    /// (`an_exit_with_work_not_kept_is_still_a_crash`). Restart: the
    /// lifecycle is stored and read back before the exit. Method: the
    /// three orders, each to its exit.
    #[test]
    fn a_message_its_guest_took_up_owes_nothing() {
        for order in ["before", "during", "after"] {
            let mut c = Computer::new();
            c.wake();
            let g = c.life.generation();
            let posted = c.now + 1;
            if order == "before" {
                c.apply(Event::Posted { at_ms: posted });
                c.apply(Event::Wake { why: Wake::Record });
            }
            c.apply(Event::Opened { socket: Socket::Keepalive });
            if order == "during" {
                c.apply(Event::Posted { at_ms: posted });
                c.apply(Event::Wake { why: Wake::Record });
            }
            c.apply(Event::Closed { socket: Socket::Keepalive });
            if order == "after" {
                for _ in 0..2 {
                    c.apply(Event::Posted { at_ms: posted });
                    c.apply(Event::Wake { why: Wake::Record });
                }
            }
            c.now += SAVE_SETTLE_MS;
            assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }), "{order}");
            c.saved_as("b1");
            c.life = Lifecycle::read(&serde_json::to_string(&c.life).unwrap()).unwrap().0;
            let s = c.apply(Event::Exited { generation: g });
            assert_eq!(s.ended.map(|e| (e.by, e.saved)), Some((LifeEnd::Sleep, true)), "{order}: {s:?}");
            assert_eq!((c.life.phase.clone(), s.alarm_ms), (Phase::ASLEEP, None), "{order}");
        }
    }

    /// Goal: an always-on computer's idle exit is the same sleep, no strike
    /// and no rollback, but it is wanted still: it starts again at once, as
    /// after its owner's sleep of it awake. Method: as above, always on.
    #[test]
    fn an_always_on_computers_idle_exit_starts_it_again() {
        let mut c = Computer::new();
        c.wake();
        c.apply(Event::AlwaysOn { on: true });
        c.turn_saved("b1");
        let g = c.life.generation();
        c.now += 5 * 60_000;
        let s = c.apply(Event::Exited { generation: g });
        assert_eq!(s.ended.map(|e| (e.by, e.saved)), Some((LifeEnd::Sleep, true)), "{s:?}");
        assert_eq!(c.life.failures(), 0);
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.save.as_deref(), r.rollback), (Some("b1"), false), "{r:?}");
        assert_eq!(told(&c), vec![]);
    }

    /// Goal: every other exit while awake stays a crash, as before: one
    /// strike, started again at once while wanted, and its owner told what
    /// it went back to when it lost work. Invalid, each: a turn ended but
    /// not yet saved (its save waits 30 s); a port tab used after the save;
    /// a keepalive open; a save under way; a life woken by a record and up
    /// before its turn began (no save of its own: that record is still
    /// owed, so it starts again, though nothing was lost); and a lifecycle
    /// stored before saves kept what they held (`kept_ms` 0). Method: each
    /// from a saved, idle computer, then the exit.
    #[test]
    fn an_exit_with_work_not_kept_is_still_a_crash() {
        // what is done between its save and the exit, and whether that loses work
        type Case = (&'static str, fn(&mut Computer), bool);
        let cases: [Case; 9] = [
            ("a turn ended 10 s ago", |c| {
                c.apply(Event::Opened { socket: Socket::Keepalive });
                c.apply(Event::Closed { socket: Socket::Keepalive });
                c.now += 10_000;
            }, true),
            ("a port tab used since its save", |c| {
                c.apply(Event::Opened { socket: Socket::Tab });
                c.now += 60_000;
                c.apply(Event::Closed { socket: Socket::Tab });
            }, true),
            ("a keepalive open", |c| {
                c.apply(Event::Opened { socket: Socket::Keepalive });
            }, true),
            ("a save under way", |c| {
                let g = c.life.generation();
                c.apply(Event::Opened { socket: Socket::Keepalive });
                c.apply(Event::Closed { socket: Socket::Keepalive });
                c.now += SAVE_SETTLE_MS;
                assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
            }, true),
            ("a record's wake, up before its turn", |c| {
                c.sleep(Some("b2"));
                let s = c.apply(Event::Wake { why: Wake::Record });
                let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
                c.come_up(generation);
            }, false),
            ("stored before kept_ms", |c| c.life.kept_ms = 0, true),
            // Codex on this PR: a message's wake after a turn ends and
            // before its save, then the save, then the exit before its guest
            // took the message up (no keepalive opened since): that message
            // is owed, so the exit is a crash, started again while its wake
            // holds it, though all that was done is saved
            ("a message's wake not taken up, then a save", |c| {
                c.apply(Event::Opened { socket: Socket::Keepalive });
                c.apply(Event::Closed { socket: Socket::Keepalive });
                c.apply(Event::Posted { at_ms: c.now + 500 });
                c.apply(Event::Wake { why: Wake::Record });
                c.now += SAVE_SETTLE_MS;
                let g = c.life.generation();
                assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
                c.saved_as("b2");
            }, false),
            // Codex's second round: a deploy cut its keepalive, the guest's
            // turn ended, a message came, and the object closed the lost
            // socket only as that message's wake reached it: a close it
            // infers says nothing of when the work ended
            ("a message after a keepalive a deploy cut, closed as its wake came", |c| {
                c.apply(Event::Opened { socket: Socket::Keepalive });
                c.now += 60_000;
                let posted = c.now;
                c.apply(Event::Closed { socket: Socket::Keepalive });
                c.apply(Event::Posted { at_ms: posted });
                c.apply(Event::Wake { why: Wake::Record });
                c.now += SAVE_SETTLE_MS;
                let g = c.life.generation();
                assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
                c.saved_as("b2");
            }, false),
            ("a join's wake not taken up, then a save", |c| {
                c.apply(Event::Opened { socket: Socket::Keepalive });
                c.apply(Event::Closed { socket: Socket::Keepalive });
                c.apply(Event::Posted { at_ms: c.now + 500 });
                c.apply(Event::Wake { why: Wake::Joined });
                c.now += SAVE_SETTLE_MS;
                let g = c.life.generation();
                assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
                c.saved_as("b2");
            }, false),
        ];
        for (case, before, loses) in cases {
            let mut c = Computer::new();
            c.wake();
            c.turn_saved("b1");
            before(&mut c);
            let g = c.life.generation();
            let s = c.apply(Event::Exited { generation: g });
            assert_eq!(s.ended.map(|e| e.by), Some(LifeEnd::Exit), "{case}: {s:?}");
            assert_eq!(c.life.failures(), 1, "{case}: one strike");
            let s = c.owed(&s);
            assert!(matches!(s.actions.first(), Some(Action::Start { .. })), "{case}: wanted, it starts again: {s:?}");
            let went_back = told(&c).into_iter().any(|n| matches!(n, ComputerNotice::WentBack { cause: LossCause::Crash, pending: true, .. }));
            assert_eq!(went_back, loses, "{case}: told it goes back only when it lost work");
        }
    }

    /// Goal: a container that dies while it starts, before it was ever
    /// ready, meters nothing: not the start, and not the sleep before it
    /// (the last awake interval was metered as it slept). It is a start that
    /// failed: tried again after its pause, and the next awake time is
    /// metered from its ready.
    #[test]
    fn an_exit_while_starting_meters_nothing() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let s = l.apply(Event::Asleep { generation: g }, T + 12_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: T + 3_000, to_ms: T + 12_000 }]);
        // asleep an hour, then woken: it dies as it starts
        let later = T + 3_600_000;
        let s = l.apply(Event::Wake { why: Wake::Owner }, later);
        let Some(Action::Start { generation: g2 }) = s.actions.first().cloned() else { panic!("a wake starts it: {s:?}") };
        let s = l.apply(Event::Exited { generation: g2 }, later + 2_000);
        assert!(s.actions.is_empty(), "nothing was awake, and its retry waits its pause: {s:?}");
        assert_eq!((bare(&l.phase), l.failures, s.alarm_ms), (Phase::ASLEEP, 1, Some(later + 2_000 + RETRY_PAUSE_MS)));
        // the same exit again (a replay) changes nothing
        assert!(l.apply(Event::Exited { generation: g2 }, later + 2_500).actions.is_empty());
        let s = l.apply(Event::Alarm, later + 2_000 + RETRY_PAUSE_MS);
        assert_eq!(s.actions, vec![Action::Start { generation: g2 + 1 }], "it is wanted, so its retry starts it");
        l.apply(Event::Ready { generation: g2 + 1 }, later + 10_000);
        let s = l.apply(Event::Sleep, later + 65_000);
        assert!(s.actions.contains(&Action::Sleep { generation: g2 + 1 }));
        let s = l.apply(Event::Asleep { generation: g2 + 1 }, later + 66_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: later + 10_000, to_ms: later + 66_000 }]);
        // its starts dying one after another: none of it metered, each after
        // its pause, then it won't wake, saying so
        let mut l = Lifecycle::new();
        let mut at = T;
        let mut s = l.apply(Event::Wake { why: Wake::Owner }, at);
        for _ in 0..STARTS_FAILED_MAX {
            let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("each retry starts it: {s:?}") };
            at += 1_000;
            s = l.apply(Event::Exited { generation }, at);
            assert!(!s.actions.iter().any(|a| matches!(a, Action::Meter { .. })), "{s:?}");
            if let Some(retry) = s.alarm_ms {
                at = retry;
                s = l.apply(Event::Alarm, at);
            }
        }
        assert!(matches!(&l.phase, Phase::Failed { why, .. } if why.contains("kept stopping")), "{:?}", l.phase);
    }

    /// Goal (a production computer's, 2026-10-30): a start that fails is
    /// one strike and its pause, however its failure is heard. A start that
    /// fails destroys its container, and that container's end reports too,
    /// in either order: the first counts, and the other is of a start no
    /// longer under way. So one failed restore never becomes three strikes
    /// in three seconds. Valid (each order), replay (the second report),
    /// and the pause kept. Method: the lifecycle's steps for each order.
    #[test]
    fn a_failed_start_counts_once_whichever_report_comes_first() {
        let failed = || Event::StartFailed { generation: 1, why: "restore: gateway rejected the range download with HTTP 502".into() };
        let exited = || Event::Exited { generation: 1 };
        for (first, second) in [(failed(), exited()), (exited(), failed())] {
            let mut l = Lifecycle::new();
            let s = l.apply(Event::Wake { why: Wake::Record }, T);
            assert_eq!(s.actions, vec![Action::Start { generation: 1 }]);
            let s = l.apply(first.clone(), T + 1_000);
            assert!(s.actions.is_empty(), "no start at once after {first:?}: {s:?}");
            assert_eq!((bare(&l.phase), l.failures, s.alarm_ms), (Phase::ASLEEP, 1, Some(T + 1_000 + RETRY_PAUSE_MS)), "{first:?}");
            let before = l.clone();
            let s = l.apply(second.clone(), T + 1_200);
            assert!(s.actions.is_empty() && l == before, "{second:?} after {first:?} changes nothing: {s:?}");
            let s = l.apply(Event::Alarm, T + 1_000 + RETRY_PAUSE_MS);
            assert_eq!(s.actions, vec![Action::Start { generation: 2 }], "its retry, after the pause");
        }
    }

    /// Goal (docs/computers.md, Lifecycle): a guest that dies
    /// while it holds its keepalive (a turn open, however long it has run)
    /// is started again, so its next life ends that turn. Method: a turn
    /// that runs past every hold, then a crash; on master `gone()` zeroed
    /// the keepalive before asking `wanted()`, and it stayed asleep.
    #[test]
    fn a_crash_while_busy_starts_it_again() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        // a long turn: the record's hold ran out (its alarm heard within
        // `ALARM_LATE_MS` of the turn's start, as one on time is)
        let late = T + IDLE_MS + 1_000;
        // busy, it stays awake: it is metered, and (a keepalive open past
        // SAVE_EVERY_MS) saved, never put to sleep
        assert_eq!(l.apply(Event::Alarm, late - 1).actions, vec![Action::Meter { from_ms: T + 3_000, to_ms: late - 1 }, Action::Hold { generation: g }], "busy, it stays awake");
        let s = l.apply(Event::Exited { generation: g }, late);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Exit, saved: false, at_ms: late, restart: false, quiet: false, unchanged: false }));
        let s = owed(&mut l, &s, late);
        assert!(s.actions.contains(&Action::Start { generation: g + 1 }), "busy when it died, it starts again: {s:?}");
        // the next life comes up and, its turns ended, is idle: it sleeps
        // once the crash's hold runs out, as after a record
        l.apply(Event::Ready { generation: g + 1 }, late + 3_000);
        let s = l.apply(Event::Alarm, late + IDLE_MS);
        assert!(s.actions.contains(&Action::Sleep { generation: g + 1 }), "{s:?}");
        // the keepalive's close reaching it first (the socket dies with its
        // container) holds it the same: the order of the two never matters
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, late);
        let s = l.apply(Event::Exited { generation: g }, late);
        let s = owed(&mut l, &s, late);
        assert!(s.actions.contains(&Action::Start { generation: g + 1 }), "{s:?}");
        // idle when it died, and nothing holding it: it stays asleep
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let s = l.apply(Event::Exited { generation: g }, late);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "{s:?}");
    }

    /// Goal: a start from a snapshot that fails starts again at
    /// once, from the image (the DO forgot the snapshot), with no strike
    /// against the computer and nothing metered; a second in one wake is a
    /// failure like any. Method: three snapshot failures in a row would be
    /// "won't wake" if each were a strike.
    #[test]
    fn a_failed_snapshot_start_falls_back_in_the_same_wake() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        l.apply(Event::Asleep { generation: g }, T + 12_000);
        let s = l.apply(Event::Wake { why: Wake::Record }, T + 60_000);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 1 }]);
        let s = l.apply(Event::SnapshotFailed { generation: g + 1, why: "no such snapshot".into() }, T + 61_000);
        let s = owed(&mut l, &s, T + 61_000);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 2 }], "the same wake, from the image, metering nothing");
        assert_eq!(l.failures, 0, "no strike");
        // a replay of it, or one from the start it superseded, changes nothing
        let before = l.clone();
        assert!(l.apply(Event::SnapshotFailed { generation: g + 1, why: "late".into() }, T + 62_000).actions.is_empty());
        assert_eq!(l, before);
        // the image's start fails too: that is a strike, with its pause
        let s = l.apply(Event::SnapshotFailed { generation: g + 2, why: "again".into() }, T + 63_000);
        assert_eq!((bare(&l.phase), l.failures, s.alarm_ms), (Phase::ASLEEP, 1, Some(T + 63_000 + RETRY_PAUSE_MS)));
        // and a start that comes up clears it, for the next wake's
        let s = l.apply(Event::Alarm, T + 63_000 + RETRY_PAUSE_MS);
        let Some(Action::Start { generation: g3 }) = s.actions.first().cloned() else { panic!("{s:?}") };
        l.apply(Event::Ready { generation: g3 }, T + 70_000);
        assert!(!l.fell_back && l.failures == 0);
        // with nothing wanting it any more, a fallback leaves it asleep
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why: Wake::Tab }, T);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let s = l.apply(Event::SnapshotFailed { generation, why: "slow".into() }, T + IDLE_MS + 1);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Asleep { .. }), "{s:?}");
    }

    /// Goal: a life is a start that came up, and it ends once: by its sleep
    /// (the exit it is signalled into included) or on its own. A start that
    /// never came up ends no life. Method: each way out, read from the step.
    #[test]
    fn a_life_ends_once_and_says_how() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let s = l.apply(Event::Exited { generation: g }, T + 11_000);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Exit, saved: false, at_ms: T + 11_000, restart: false, quiet: true, unchanged: true }), "gone as its sleep held it, before the sleep stopped it: on its own");
        assert!(l.apply(Event::Asleep { generation: g }, T + 12_000).ended.is_none(), "it ended once");
        // one that never came up
        let s = l.apply(Event::Wake { why: Wake::Owner }, T + 20_000);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        assert!(l.apply(Event::StartFailed { generation, why: "pull".into() }, T + 21_000).ended.is_none());
        // slept as it started, it never came up either
        let s = l.apply(Event::Wake { why: Wake::Owner }, T + 30_000);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        l.apply(Event::Sleep, T + 31_000);
        assert!(l.apply(Event::Asleep { generation }, T + 32_000).ended.is_none());
        // a crash that was its third strike still ends its life
        let mut l = Lifecycle { failures: STARTS_FAILED_MAX - 1, ..Lifecycle::new() };
        let s = l.apply(Event::Wake { why: Wake::Record }, T);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        l.apply(Event::Ready { generation }, T + 1_000);
        l.failures = STARTS_FAILED_MAX - 1;
        let s = l.apply(Event::Exited { generation }, T + 2_000);
        assert!(matches!(l.phase, Phase::Failed { .. }) && s.ended == Some(Ended { generation, by: LifeEnd::Exit, saved: false, at_ms: T + 2_000, restart: false, quiet: true, unchanged: true }), "{s:?} {:?}", l.phase);
    }

    /// Goal: a start from a snapshot that fails forgets it, and
    /// the start the lifecycle makes at once restores the image and the
    /// save: one wake, no strike, the rollback rule unchanged. Method: the
    /// DO's two paths, its own failure and its container's exit before it
    /// came up, each reported once.
    #[test]
    fn a_broken_snapshot_wakes_from_the_save() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        assert!(c.saves.snapshotted("b1", "s1", IMAGE));
        let s = c.apply(Event::Wake { why: Wake::Record });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let plan = c.saves.plan(Some(IMAGE));
        assert_eq!(plan, Plan::Snapshot { id: "s1".into() });
        c.saves.starting(generation, plan.source());
        assert_eq!(c.saves.start_from(generation), Some(RestoreSource::Snapshot), "its exit now is the start's own failure");
        assert_eq!(c.saves.snapshot_failed().map(|s| s.id), Some("s1".into()));
        let s = c.apply(Event::SnapshotFailed { generation, why: "expired".into() });
        let s = c.owed(&s);
        let Some(Action::Start { generation: next }) = s.actions.first().cloned() else { panic!("{s:?}") };
        assert_eq!(c.saves.plan(Some(IMAGE)), Plan::Backup);
        let r = c.come_up(next);
        assert_eq!((r.from, r.save.as_deref(), r.rollback, c.life.failures), (RestoreSource::Backup, Some("b1"), false, 0), "{r:?}");
        assert_eq!(c.saves.came_up(generation, RestoreSource::Snapshot, Some("b1"), Some(IMAGE), c.now), None, "the broken start never comes up after");
    }

    /// Goal (#362): a start from a snapshot is up only once its guest has
    /// read its own view since the container started, however early; past
    /// its wait unheard, it came up dead, and the lifecycle answers it as a
    /// snapshot that would not start: the same wake starts the image and
    /// the save, no strike; a second in that wake is a strike. Invalid: a
    /// read before the start (an earlier container's) is not this one's.
    /// Method: the decision at each edge, then the lifecycle's answer.
    #[test]
    fn a_snapshot_start_is_up_once_its_guest_is_heard() {
        let (started, waiting) = (T, T + 800);
        assert_eq!(alive(None, started, waiting, waiting, ALIVE_MS), Alive::Waiting);
        assert_eq!(alive(Some(T + 300), started, waiting, waiting, ALIVE_MS), Alive::Heard, "a read before its wait began is its guest's");
        assert_eq!(alive(Some(T), started, waiting, waiting + 10, ALIVE_MS), Alive::Heard, "at its start itself");
        assert_eq!(alive(Some(T - 1), started, waiting, waiting + ALIVE_MS - 1, ALIVE_MS), Alive::Waiting, "an earlier container's read is not its");
        assert_eq!(alive(Some(T - 1), started, waiting, waiting + ALIVE_MS, ALIVE_MS), Alive::Dead);
        assert_eq!(alive(None, started, waiting, waiting + ALIVE_MS, ALIVE_MS), Alive::Dead, "unheard at the bound");
        assert_eq!(alive(Some(waiting + ALIVE_MS), started, waiting, waiting + ALIVE_MS, ALIVE_MS), Alive::Heard, "heard at the bound: up");
        // the cell reports a dead one as a snapshot that would not start
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why: Wake::Record }, T);
        let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let s = l.apply(Event::SnapshotFailed { generation: g, why: "its guest did not read its view within 45 s".into() }, T + ALIVE_MS + 1_000);
        let s = owed(&mut l, &s, T + ALIVE_MS + 1_000);
        assert_eq!((s.actions, l.failures), (vec![Action::Start { generation: g + 1 }], 0), "the same wake, from the image and the save, no strike");
        let s = l.apply(Event::SnapshotFailed { generation: g + 1, why: "dead again".into() }, T + 2 * ALIVE_MS);
        assert_eq!((s.actions.len(), l.failures, bare(&l.phase)), (0, 1, Phase::ASLEEP), "a second in the wake is a strike, tried again after a pause");
        assert!(l.retry().is_some());
    }

    /// Goal : a start whose save will not restore (its archive gone or
    /// corrupt, or the image's check of it failed) starts again at once from
    /// the save before it, no strike against the computer, until none is
    /// left; then it is a strike like any. A start that fails for another
    /// reason is a strike, and tries the same save again. Method: three
    /// saves, each found unusable in turn, as the DO finds them.
    #[test]
    fn a_start_that_keeps_failing_tries_the_save_before() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        c.wake();
        c.sleep(Some("b2"));
        c.wake();
        c.sleep(Some("b3"));
        assert_eq!(c.saves.all().iter().map(|s| s.number).collect::<Vec<_>>(), vec![3, 2, 1], "three kept, newest first");
        let s = c.apply(Event::Wake { why: Wake::Record });
        let Some(Action::Start { generation: mut g }) = s.actions.first().cloned() else { panic!("{s:?}") };
        // a transient failure (a pull) is a strike, and the same save next
        let s = c.apply(Event::StartFailed { generation: g, why: "pull".into() });
        assert_eq!(c.life.failures, 1);
        assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some("b3"), "the same save, next");
        c.now = s.alarm_ms.expect("its retry is due") - 1_000;
        let s = c.apply(Event::Alarm);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("its retry starts it: {s:?}") };
        g = generation;
        // b3, then b2, would not restore: each starts again at once
        for (bad, next) in [("b3", "b2"), ("b2", "b1")] {
            assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some(bad));
            let older = c.saves.unusable(bad);
            assert!(older);
            let s = c.apply(Event::RestoreFailed { generation: g, why: format!("{bad}'s archive is corrupt"), older });
            let s = c.owed(&s);
            assert_eq!(s.actions, vec![Action::Start { generation: g + 1 }], "at once, from the save before it");
            assert_eq!(c.life.failures, 1, "no strike");
            assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some(next));
            g += 1;
        }
        // b1 comes up: a rollback, since its life's newest save was b3
        let r = c.come_up(g);
        assert_eq!((r.save.as_deref(), r.rollback, c.life.failures), (Some("b1"), true, 0), "{r:?}");
        // with no save left, a save that will not restore is a strike
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let s = c.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let older = c.saves.unusable("b1");
        assert!(!older);
        let s = c.apply(Event::RestoreFailed { generation, why: "corrupt".into(), older });
        assert!(s.actions.is_empty() && c.life.failures == 1 && matches!(c.life.phase, Phase::Asleep { .. }), "{s:?}");
        assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some("b1"), "tried again, as any start is");
        // and the lifecycle bounds what the DO says: no more fallbacks than saves
        let mut l = Lifecycle::new();
        l.apply(Event::Wake { why: Wake::Owner }, T);
        for i in 0..SAVES_KEPT as u64 {
            let s = l.apply(Event::RestoreFailed { generation: i + 1, why: "x".into(), older: true }, T + i as i64);
            let s = owed(&mut l, &s, T + i as i64);
            assert_eq!(s.actions, vec![Action::Start { generation: i + 2 }]);
        }
        let s = l.apply(Event::RestoreFailed { generation: SAVES_KEPT as u64 + 1, why: "x".into(), older: true }, T + 10);
        assert!(s.actions.is_empty() && l.failures == 1, "a fallback past the saves kept is a strike: {s:?}");
    }

    /// Goal: its owner's restart of a running computer is a sleep that
    /// saves, then a start owed at once (its alarm's), held as its owner's
    /// wake holds it; a keepalive does not cancel it. Its owner is told it
    /// is starting from the stop's end on, whatever it restarted (FIN-399:
    /// the restart answers so on every path, never awake on one and
    /// starting on another). Asked twice (the same start named), it is made
    /// once; an earlier start's restart is nothing; 0 restarts whatever
    /// runs. Valid, invalid (stale), replay. Method: the lifecycle's steps,
    /// as the DO would report them.
    #[test]
    fn a_restart_saves_then_starts_again_once() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        let s = l.apply(Event::Restart { generation: g }, T + 10_000);
        assert_eq!(s.actions, vec![Action::Sleep { generation: g }], "first its hold, as any sleep");
        assert!(matches!(l.phase, Phase::Sleeping { owner: true, restarting: true, owes_start: true, .. }) && l.keepalives == 0, "{l:?}");
        // the same press again, and a keepalive as it holds: nothing new
        assert!(l.apply(Event::Restart { generation: g }, T + 10_100).actions.is_empty(), "asked twice, made once");
        assert!(l.apply(Event::Opened { socket: Socket::Keepalive }, T + 10_200).actions.is_empty(), "no keepalive cancels a restart");
        assert!(matches!(l.phase, Phase::Sleeping { .. }));
        let (_, held, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 10_500));
        assert!(held);
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, T + 12_000).actions, vec![Action::Stop { generation: g, saved: true }]);
        let s = l.apply(Event::Asleep { generation: g }, T + 15_000);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: T + 15_000, restart: true, quiet: false, unchanged: false }), "saved: no rollback");
        assert!(l.starting(T + 15_000) && l.generation() == g, "told starting once its start is owed, before it is made");
        let s = owed(&mut l, &s, T + 15_000);
        assert!(s.actions.contains(&Action::Start { generation: g + 1 }), "then a start, at once: {s:?}");
        assert!(l.starting(T + 15_000) && l.generation() == g + 1);
        l.apply(Event::Ready { generation: g + 1 }, T + 18_000);
        assert!(matches!(l.phase, Phase::Awake { .. }));
        assert_eq!(l.held_until_ms(), T + 10_100 + IDLE_MS, "held as its owner's wake holds it, from the last press");
        // the first start's restart, asked late: made already
        let before = l.clone();
        assert!(l.apply(Event::Restart { generation: g }, T + 20_000).actions.is_empty() && l == before, "an earlier start's restart is nothing");
        // 0: whatever runs
        assert_eq!(l.apply(Event::Restart { generation: 0 }, T + 21_000).actions, vec![Action::Sleep { generation: g + 1 }]);
        // its owner's sleep after a restart asked: the newest ask wins, no start
        l.apply(Event::Sleep, T + 21_500);
        assert!(matches!(l.phase, Phase::Sleeping { owner: true, restarting: false, owes_start: false, .. }), "{l:?}");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g + 1, held: true }, T + 22_000));
        l.apply(Event::Saved { generation: g + 1, seq }, T + 23_000);
        let s = l.apply(Event::Asleep { generation: g + 1 }, T + 24_000);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })) && matches!(l.phase, Phase::Asleep { .. }), "{s:?}");
        assert!(s.ended.is_some_and(|e| !e.restart), "a sleep, not a restart");

        assert!(!l.starting(T + 24_000), "asleep, nothing owed");
        // asleep, a restart is a start owed, told as starting at once; won't
        // wake, it lifts it, as its owner's wake does
        let s = l.apply(Event::Restart { generation: g + 1 }, T + 30_000);
        assert!(l.starting(T + 30_000) && l.generation() == g + 1, "{l:?}");
        let again = l.apply(Event::Restart { generation: g + 1 }, T + 30_000);
        assert!(again.actions.is_empty() && l.starting(T + 30_000), "pressed again before its alarm, the same start owed");
        let s = owed(&mut l, &s, T + 30_000);
        assert_eq!(s.actions, vec![Action::Start { generation: g + 2 }]);
        assert!(l.apply(Event::Restart { generation: g + 2 }, T + 30_100).actions.is_empty(), "starting, the start under way is the restart's");
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why: Wake::Owner }, T);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        l.failures = STARTS_FAILED_MAX - 1;
        l.apply(Event::StartFailed { generation, why: "pull".into() }, T + 1_000);
        assert!(matches!(l.phase, Phase::Failed { .. }));
        assert!(l.apply(Event::Wake { why: Wake::Record }, T + 2_000).refused.is_some(), "a record does not lift it");
        assert!(l.apply(Event::Restart { generation: generation + 1 }, T + 2_500).actions.is_empty(), "a start it never made is restarted by no one");
        let s = l.apply(Event::Restart { generation }, T + 3_000);
        assert!(l.failures == 0 && l.starting(T + 3_000), "its owner's restart lifts it: {l:?}");
        assert_eq!(owed(&mut l, &s, T + 3_000).actions, vec![Action::Start { generation: generation + 1 }]);
        // what it knows survives a restart of its Durable Object
        let back: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    /// Goal (docs/self-host.md, found 21): the restart's sleep ends at its
    /// stop's end, never at the container's exit report, a request of its
    /// own, which comes in as the stop signals it, before the stop's own
    /// end. That report starts nothing: the stop's `Asleep` owes the start
    /// (its alarm makes it; the restart's request answers `starting`
    /// once the stop ended). Before, the report ended the sleep and made
    /// the start while the stop still watched the container that exited.
    /// Valid (saved, and its save failing), replay (the exit
    /// reported again, or after the start), and an exit before the stop (a
    /// crash as it was held) that still starts it. Method: the lifecycle's
    /// steps, the exit report applied before the stop's own end.
    #[test]
    fn a_restarts_start_is_its_stops_not_the_exit_reports() {
        for saved in [true, false] {
            let mut l = Lifecycle::new();
            let g = started(&mut l, T);
            assert_eq!(l.apply(Event::Restart { generation: g }, T + 10_000).actions, vec![Action::Sleep { generation: g }]);
            let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 10_500));
            let s = match saved {
                true => l.apply(Event::Saved { generation: g, seq }, T + 12_000),
                false => l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, T + 12_000),
            };
            assert_eq!(s.actions, vec![Action::Stop { generation: g, saved }]);
            // the stop signals it: its exit is reported, before the stop's end
            for at in [T + 12_100, T + 12_200] {
                let s = l.apply(Event::Exited { generation: g }, at);
                assert!(s.actions.is_empty() && s.ended.is_none(), "the exit report starts nothing (saved {saved}): {s:?}");
                assert!(matches!(l.phase, Phase::Sleeping { generation, .. } if generation == g), "still the restart's sleep: {:?}", l.phase);
            }
            let s = l.apply(Event::Asleep { generation: g }, T + 12_300);
            assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved, at_ms: T + 12_300, restart: true, quiet: true, unchanged: true }));
            let s = owed(&mut l, &s, T + 12_300);
            assert!(s.actions.contains(&Action::Start { generation: g + 1 }), "the stop's end owes it, made by its alarm: {s:?}");
            // a report of the stopped one, late, changes nothing
            let before = l.clone();
            assert!(l.apply(Event::Exited { generation: g }, T + 12_400).actions.is_empty() && l == before);
            l.apply(Event::Ready { generation: g + 1 }, T + 15_000);
            assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g + 1));
        }
        // its owner's plain sleep: the stop's end puts it to sleep, the exit
        // report before it nothing
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 10_500));
        l.apply(Event::Saved { generation: g, seq }, T + 12_000);
        assert!(l.apply(Event::Exited { generation: g }, T + 12_100).actions.is_empty());
        let s = l.apply(Event::Asleep { generation: g }, T + 12_300);
        assert!(matches!(l.phase, Phase::Asleep { .. }) && !s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "{s:?}");
        // a container that dies as the restart holds it, before any stop,
        // stopped on its own: an exit, and it starts again from that report
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Restart { generation: g }, T + 10_000);
        let s = l.apply(Event::Exited { generation: g }, T + 10_200);
        assert_eq!(s.ended.map(|e| e.by), Some(LifeEnd::Exit));
        let s = owed(&mut l, &s, T + 10_200);
        assert!(s.actions.contains(&Action::Start { generation: g + 1 }), "{s:?}");
        assert!(l.apply(Event::Held { generation: g, held: true }, T + 10_500).actions.is_empty(), "its hold's late answer is nothing");
    }

    /// Goal: a restart whose save fails keeps no container (its owner asked
    /// to start again: the bound is for a sleep no one asked for): it stops
    /// unsaved at once, says so, and starts again; that life's end says the
    /// restart did not save, so its start is a rollback, told as the
    /// restart's. Also when a sleep's failed save had kept it awake.
    #[test]
    fn a_restart_whose_save_fails_starts_again_unsaved() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let r = c.wake();
        let g = r.generation;
        // a turn, after b1: work that only a save of this life would keep
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.apply(Event::Closed { socket: Socket::Keepalive });
        // an idle sleep's save fails: kept awake, within the bound
        c.now = c.life.held_until_ms();
        let s = c.apply(Event::Alarm);
        assert!(s.actions.contains(&Action::Sleep { generation: g }), "{s:?}");
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        let s = c.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() });
        assert_eq!(s.actions.last(), Some(&Action::Unhold { generation: g }), "kept, within its bound");
        // its owner restarts it: the restart's save fails too, and it stops
        assert_eq!(c.apply(Event::Restart { generation: g }).actions, vec![Action::Sleep { generation: g }]);
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        assert!(c.saves.save_failed(g));
        let s = c.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() });
        assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: false }], "no container kept: they asked");
        assert!(matches!(&s.note, SaveNote::Says(why) if why.contains("restart could not save")), "{:?}", s.note);
        let s = c.apply(Event::Asleep { generation: g });
        assert!(s.ended.is_some_and(|e| e.restart && !e.saved && e.by == LifeEnd::Sleep), "{s:?}");
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("it starts again: {s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.save.as_deref(), r.rollback), (Some("b1"), true), "back to its last save: {r:?}");
        let w = c.saves.went_back().expect("told");
        assert_eq!((w.life, w.cause, w.save), (g, LossCause::Restart, Some(1)));
    }

    /// The 11-03 incident's sequences (#1004, #1005), each step an event and
    /// when it came, the lifecycle stored and read again (`evict`: an
    /// isolate that went) wherever a row says so.
    struct Row {
        name: &'static str,
        evict: bool,
        steps: Vec<(i64, Event)>,
    }

    /// Goal (#1004, #1005 as table rows): one R2 502 during a restore is one
    /// strike and its pause, not three strikes in three seconds, its words
    /// kept, however the start's own destroy is heard and whichever isolate
    /// hears it; a computer that won't wake tries once every half hour on a
    /// wake not its owner's, its time kept across a stored lifecycle; and
    /// its owner's restart always tries. Valid (each row), replay (each
    /// exit after its start's report), restart (`evict`). Method: the
    /// production log's order (exits of a start's own destroy before its
    /// `start_failed`), applied to the lifecycle.
    #[test]
    fn the_11_03_sequences_are_one_strike_each_and_a_way_back() {
        let r502 = || "restore '/data': gateway rejected the range download with HTTP 502".to_string();
        // a crash while awake (strike one), then each start's own destroy
        // reported before its 502, as the log has it
        let log_1004 = |g: u64, at: i64| -> Vec<(i64, Event)> {
            let mut v = vec![(at, Event::Exited { generation: g })];
            // the crash owes its start at once; the first 502 is strike two,
            // so the next try waits twice the pause (`failed`)
            for (n, t) in [(g + 1, at), (g + 2, at + 2_500 + 2 * RETRY_PAUSE_MS)] {
                v.push((t, Event::Alarm));
                v.push((t + 900, Event::StartFailing { generation: n }));
                v.push((t + 1_000, Event::Exited { generation: n }));
                v.push((t + 2_500, Event::StartFailed { generation: n, why: r502() }));
                v.push((t + 2_600, Event::Exited { generation: n }));
            }
            v
        };
        for evict in [false, true] {
            let rows = [Row { name: "#1004: a crash, then two starts that each fail on one 502, their destroys heard first", evict, steps: log_1004(1, T + 60_000) }];
            for row in rows {
                let mut l = Lifecycle::new();
                let g = started(&mut l, T);
                l.apply(Event::Opened { socket: Socket::Tab }, T + 5_000);
                assert_eq!(g, 1);
                let mut strikes = vec![];
                for (at, event) in row.steps {
                    let was = l.failures;
                    let s = l.apply(event.clone(), at);
                    if row.evict {
                        l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap().0;
                    }
                    if l.failures > was {
                        // each strike pauses before the next try (a crash's owed start aside)
                        strikes.push((at, s.alarm_ms, event));
                    }
                }
                assert_eq!(strikes.len(), 3, "{}: the crash and each start's 502, once each: {strikes:?}", row.name);
                assert!(strikes.iter().all(|(_, _, e)| !matches!(e, Event::Exited { generation } if *generation > 1)), "{}: no start's own destroy is a strike: {strikes:?}", row.name);
                assert!(matches!(&l.phase, Phase::Failed { why, .. } if why.contains("HTTP 502")), "{}: its words kept: {:?}", row.name, l.phase);
                let (first, last) = (strikes[0].0, strikes[2].0);
                assert!(last - first >= RETRY_PAUSE_MS, "{}: its strikes waited their pause, not three in three seconds", row.name);
            }
        }
        // #1005: won't wake, and a way back: a record tries once each half
        // hour, its time kept across a stored lifecycle, the owner always
        for evict in [false, true] {
            let gave_up = T + 33_000;
            let mut l = Lifecycle { phase: Phase::Failed { why: r502(), since_ms: gave_up }, generation: 4, failures: STARTS_FAILED_MAX, ..Lifecycle::new() };
            let steps = [
                (gave_up + 60_000, Event::Wake { why: Wake::Record }, None),
                (gave_up + WONT_WAKE_RETRY_MS, Event::Wake { why: Wake::Record }, Some(5)),
                (gave_up + WONT_WAKE_RETRY_MS + 1_000, Event::StartFailing { generation: 5 }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 1_100, Event::Exited { generation: 5 }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 1_200, Event::StartFailed { generation: 5, why: r502() }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 60_000, Event::Wake { why: Wake::Record }, None),
                (gave_up + 2 * WONT_WAKE_RETRY_MS + 1_200, Event::Wake { why: Wake::Tab }, Some(6)),
                (gave_up + 2 * WONT_WAKE_RETRY_MS + 1_300, Event::StartFailed { generation: 6, why: r502() }, None),
                (gave_up + 2 * WONT_WAKE_RETRY_MS + 2_000, Event::Restart { generation: 6 }, None),
                (gave_up + 2 * WONT_WAKE_RETRY_MS + 2_000, Event::Alarm, Some(7)),
            ];
            for (at, event, starts) in steps {
                let s = l.apply(event.clone(), at);
                let made = s.actions.iter().find_map(|a| match a {
                    Action::Start { generation } => Some(*generation),
                    _ => None,
                });
                assert_eq!(made, starts, "#1005 (evict {evict}): {event:?} at +{}: {s:?} {:?}", at - gave_up, l.phase);
                if evict {
                    l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap().0;
                }
            }
            assert_eq!(l.failures, 0, "its owner's restart starts the count again");
        }
    }
}
