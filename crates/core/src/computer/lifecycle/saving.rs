//! Its saves, as the lifecycle runs them (docs/computers.md, Durability): an
//! awake save once work ends or runs long, and a sleep's hold, save and stop.

use serde::{Deserialize, Serialize};

use super::{Action, Lifecycle, Phase, SaveNote, Step};
use crate::computer::limits::{HOLD_DEADLINE_MS, SAVE_DEADLINE_MS, SAVE_SETTLE_MS, SLEEP_DEADLINE_MS};

/// The save under way (awake, or a sleep's), by its step, and since when
/// that step began (its deadline counts from there).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Saving {
    /// The DO touched the hold and waits for the guest's `held`.
    Hold { since_ms: i64 },
    /// The DO saves `/data` (`held`: the guest answered), as save `seq`.
    Save { since_ms: i64, held: bool, seq: u64 },
    /// A sleep's last step: snapshot (`saved`: its save worked), signal,
    /// wait, destroy.
    Stop { since_ms: i64, saved: bool },
}

impl Saving {
    pub(super) fn since_ms(self) -> i64 {
        match self {
            Saving::Hold { since_ms } | Saving::Save { since_ms, .. } | Saving::Stop { since_ms, .. } => since_ms,
        }
    }

    pub(super) fn deadline_ms(self) -> i64 {
        let ms = match self {
            Saving::Hold { .. } => HOLD_DEADLINE_MS,
            Saving::Save { .. } => SAVE_DEADLINE_MS,
            Saving::Stop { .. } => SLEEP_DEADLINE_MS,
        };
        self.since_ms() + ms
    }
}

impl Lifecycle {
    /// The save under way, if one is.
    pub fn saving(&self) -> Option<Saving> {
        self.saving
    }

    /// Begins a sleep: its hold first (any save that was due, or under way,
    /// is the sleep's own). `owner`, `restarting`, `owes_start`: the
    /// `Phase::Sleeping` it is.
    pub(super) fn sleep(&mut self, generation: u64, now_ms: i64, (owner, restarting, owes_start): (bool, bool, bool), step: &mut Step) {
        self.phase = Phase::Sleeping { generation, since_ms: now_ms, owner, restarting, owes_start };
        self.saving = Some(Saving::Hold { since_ms: now_ms });
        self.save_due_ms = None;
        self.busy_since_ms = None;
        self.save_asked_ms = now_ms;
        step.actions.push(Action::Sleep { generation });
    }

    /// Whether something that wants it now cancels the sleep under way: an
    /// idle one (not its owner's, nor a restart) still at its hold or its
    /// save. Once its stop begins, its container is going: not then.
    pub(super) fn cancels_sleep(&self) -> bool {
        matches!(self.phase, Phase::Sleeping { owner: false, .. }) && matches!(self.saving, Some(Saving::Hold { .. } | Saving::Save { .. }))
    }

    /// Its idle sleep is cancelled (`cancels_sleep`): awake again on the
    /// container it has, nothing restored and nothing started. A hold not
    /// answered yet is let go at once, and the save it was for is due when
    /// work settles (a turn's end saves instead). A save under way goes on
    /// as an awake save, held to its end as any is: a backup cut short is
    /// never sure to be whole or gone, and one that goes on is reported,
    /// and kept or failed, as every awake save is. It is awake as it was
    /// before its sleep began (a moment before it), so its sleep's save,
    /// asked as that began, is one this awake run made (`idle_and_kept`);
    /// a keepalive open is busy from now (its saves every `SAVE_EVERY_MS`).
    pub(super) fn sleep_cancelled(&mut self, generation: u64, now_ms: i64, step: &mut Step) {
        assert!(self.cancels_sleep(), "only an idle sleep, before its stop, is cancelled");
        let Phase::Sleeping { since_ms, .. } = self.phase else { unreachable!("a cancel is of a sleep") };
        self.phase = Phase::Awake { generation, since_ms: since_ms - 1 };
        if self.keepalives > 0 {
            self.busy_since_ms = Some(now_ms);
        }
        if matches!(self.saving, Some(Saving::Hold { .. })) {
            self.saving = None;
            if self.keepalives == 0 {
                self.save_due_ms = Some(now_ms + SAVE_SETTLE_MS);
            }
            step.actions.push(Action::Unhold { generation });
        }
    }

    /// Whether save `seq`, done, was superseded: a newer save was asked as
    /// it ran (its owner's sleep or restart after a cancel asked one while
    /// the cancelled sleep's save went on), kept or not. The DO lets its
    /// records go: never the newest kept, ahead of a newer capture.
    pub fn superseded(&self, seq: u64) -> bool {
        seq < self.save_seq
    }

    /// Begins a save while it stays awake: its hold first.
    pub(super) fn hold_for_save(&mut self, generation: u64, now_ms: i64, step: &mut Step) {
        assert!(matches!(self.phase, Phase::Awake { .. }) && self.saving.is_none(), "an awake save begins with none under way");
        self.saving = Some(Saving::Hold { since_ms: now_ms });
        self.save_due_ms = None;
        self.save_asked_ms = now_ms;
        step.actions.push(Action::Hold { generation });
    }

    /// The hold answered (or its wait ran out): the save itself.
    pub(super) fn save(&mut self, generation: u64, held: bool, now_ms: i64, step: &mut Step) {
        self.save_seq += 1;
        self.saving = Some(Saving::Save { since_ms: now_ms, held, seq: self.save_seq });
        step.actions.push(Action::Save { generation, held, seq: self.save_seq });
    }

    /// A save worked: nothing of this life is unsaved since it was asked,
    /// and what its view said of its saves goes, whichever life said it (a
    /// life before it that slept unsaved says so until a save works).
    pub(super) fn saved(&mut self, now_ms: i64, step: &mut Step) {
        // what was done before its ask is in it (stored before `kept_ms`, never)
        if self.kept_ms > 0 {
            self.kept_ms = self.kept_ms.max(self.save_asked_ms);
        }
        self.save_failures = 0;
        self.failing = None;
        self.end_unsaved(now_ms, step);
        step.note = SaveNote::Clear;
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::LifeEnd;

    use super::*;
    use crate::computer::testing::{save_asked, started, T};
    use crate::computer::*;

    /// Goal: awake, a computer is saved when its work ends: its
    /// last keepalive closes and `SAVE_SETTLE_MS` pass with none opened
    /// again; a keepalive that opens first cancels that save, so turns back
    /// to back save once; the save is a hold, the save, the hold let go.
    /// Method: the alarms the lifecycle asks for, around one turn and two.
    #[test]
    fn it_is_saved_when_work_ends() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        let s = l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        assert_eq!(s.alarm_ms, Some(T + 10_000 + SAVE_SETTLE_MS), "the settle is what it waits for");
        let s = l.apply(Event::Alarm, T + 10_000 + SAVE_SETTLE_MS - 1);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Hold { .. })), "not before it settles: {s:?}");
        let s = l.apply(Event::Alarm, T + 10_000 + SAVE_SETTLE_MS);
        assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }), "{s:?}");
        let s = l.apply(Event::Held { generation: g, held: true }, T + 41_000);
        assert_eq!(s.actions, vec![Action::Save { generation: g, held: true, seq: 1 }]);
        // a late or repeated answer to the hold asks nothing twice
        assert!(l.apply(Event::Held { generation: g, held: true }, T + 41_500).actions.is_empty());
        let s = l.apply(Event::Saved { generation: g, seq: 1 }, T + 42_000);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }]);
        assert!(matches!(l.phase, Phase::Awake { .. }) && l.saving().is_none(), "awake, held no more");
        assert_eq!(s.alarm_ms, Some((T + 10_000 + SAVE_SETTLE_MS + METER_EVERY_MS).min(T + 10_000 + IDLE_MS)), "nothing more to save until more work");
        assert!(l.apply(Event::Saved { generation: g, seq: 1 }, T + 43_000).actions.is_empty(), "a save is answered once");

        // two turns back to back: the first's settle is cancelled by the
        // second's keepalive, and its end saves once
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 20_000);
        let s = l.apply(Event::Alarm, T + 10_000 + SAVE_SETTLE_MS);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Hold { .. })), "a keepalive that opened first cancelled it: {s:?}");
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + 50_000);
        let s = l.apply(Event::Alarm, T + 50_000 + SAVE_SETTLE_MS);
        assert_eq!(s.actions.iter().filter(|a| matches!(a, Action::Hold { .. })).count(), 1, "{s:?}");
        assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }));
        // a socket that closes when none is open ends no work
        let mut l = Lifecycle::new();
        started(&mut l, T);
        let s = l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        assert_eq!(s.alarm_ms, Some((T + 3_000 + METER_EVERY_MS).min(T + 10_000 + IDLE_MS)), "{s:?}");
    }

    /// The saves the lifecycle asks for, firing the alarm it asks for each
    /// time (from `alarm`) up to `until`, each hold answered and its save
    /// kept as the DO does: when each was asked.
    fn saves_until(l: &mut Lifecycle, mut alarm: Option<i64>, until: i64) -> Vec<i64> {
        let mut asked = vec![];
        while let Some(at) = alarm.filter(|at| *at <= until) {
            let s = l.apply(Event::Alarm, at);
            assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })), "no sleep within the run: {s:?}");
            alarm = s.alarm_ms;
            if let Some(Action::Hold { generation }) = s.actions.last().cloned() {
                asked.push(at);
                let (_, _, seq) = save_asked(&l.apply(Event::Held { generation, held: true }, at + 100));
                alarm = l.apply(Event::Saved { generation, seq }, at + 200).alarm_ms;
            }
        }
        asked
    }

    /// Goal: what its owner does through its ports is saved as a turn's
    /// work is: the last port tab's close asks a save once it settles
    /// (`SAVE_SETTLE_MS`), so a host's stop after it loses nothing. Tabs
    /// opened and closed fast save once, after the last close (each close
    /// puts the settle off); one tab of two closing asks none; a keepalive
    /// open keeps the save for its own close, one save. Replay: a close
    /// with no tab counted asks nothing. Restart: a lifecycle stored and
    /// read back between the close and its settle still saves. Method: the
    /// alarms the lifecycle asks for, fired and answered as the DO does,
    /// the saves asked counted.
    #[test]
    fn closing_the_last_port_tab_saves_once_it_settles() {
        let until = T + 4 * 60_000;
        // valid: one tab, its close saved once it settles
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 70_000);
        assert_eq!(s.alarm_ms, Some(T + 70_000 + SAVE_SETTLE_MS), "the settle is what it waits for");
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), vec![T + 70_000 + SAVE_SETTLE_MS]);
        // replay: a close with no tab counted asks nothing
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 130_000);
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), Vec::<i64>::new());
        // ten tabs opened and closed a second apart: one save, after the last
        let mut l = Lifecycle::new();
        started(&mut l, T);
        let mut s = Step::default();
        for i in 0..10 {
            l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000 + i * 1_000);
            s = l.apply(Event::Closed { socket: Socket::Tab }, T + 10_500 + i * 1_000);
        }
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), vec![T + 19_500 + SAVE_SETTLE_MS], "ten closes, one save");
        // one tab of two closing asks none; the last one's close does
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 11_000);
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 20_000);
        assert_eq!(saves_until(&mut l, s.alarm_ms, T + 90_000), Vec::<i64>::new(), "a tab still open");
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 100_000);
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), vec![T + 100_000 + SAVE_SETTLE_MS]);
        // a keepalive open across the tab's close: the save waits for the
        // keepalive's close, and is one
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 5_000);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 20_000);
        assert_eq!(saves_until(&mut l, s.alarm_ms, T + 90_000), Vec::<i64>::new(), "busy: its turn's end saves");
        let s = l.apply(Event::Closed { socket: Socket::Keepalive }, T + 100_000);
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), vec![T + 100_000 + SAVE_SETTLE_MS]);
        // restart: stored and read back between the close and its settle
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 20_000);
        let mut l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap().0;
        assert_eq!(saves_until(&mut l, s.alarm_ms, until), vec![T + 20_000 + SAVE_SETTLE_MS]);
    }

    /// Goal : an always-on computer, which never sleeps, is saved when
    /// its work ends and every `SAVE_EVERY_MS` while it stays busy, its
    /// awake time never metered. On master it was never saved. Method: a
    /// day of 40-minute turns, an hour apart, every alarm it asks for
    /// answered as the DO would.
    #[test]
    fn always_on_is_saved_without_sleeping() {
        /// Every alarm `l` asks for before `until`, each save answered:
        /// the saves made.
        fn alarms_until(l: &mut Lifecycle, until: i64) -> u32 {
            let mut saves = 0;
            for _ in 0..64 {
                let Some(at) = l.next_alarm().filter(|at| *at < until) else { return saves };
                let s = l.apply(Event::Alarm, at);
                assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. } | Action::Meter { .. })), "it never sleeps, nor is metered: {s:?}");
                if s.actions.contains(&Action::Hold { generation: 1 }) {
                    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: 1, held: true }, at + 500));
                    assert_eq!(l.apply(Event::Saved { generation: 1, seq }, at + 1_000).actions, vec![Action::Unhold { generation: 1 }]);
                    saves += 1;
                }
            }
            panic!("an hour asks for fewer than 64 alarms");
        }
        let mut l = Lifecycle::new();
        let s = l.apply(Event::AlwaysOn { on: true }, T);
        assert_eq!(s.actions, vec![Action::Start { generation: 1 }]);
        l.apply(Event::Ready { generation: 1 }, T + 3_000);
        let mut saves = 0;
        for hour in 0..24 {
            let turn = T + hour * 3_600_000 + 10_000;
            assert!(l.apply(Event::Opened { socket: Socket::Keepalive }, turn).actions.is_empty());
            saves += alarms_until(&mut l, turn + 40 * 60_000);
            l.apply(Event::Closed { socket: Socket::Keepalive }, turn + 40 * 60_000);
            saves += alarms_until(&mut l, turn + 3_600_000);
            assert_eq!(saves, 3 * (hour as u32 + 1), "two while busy (its timer), one as its work ended, hour {hour}");
        }
        assert!(matches!(l.phase, Phase::Awake { generation: 1, .. }));
    }

    /// Goal: a sleep holds the guest, then saves (held, or not when
    /// the guest never answered), then stops; its life's end says it saved.
    /// A hold or a stop that never reports is asked for again at its
    /// deadline; a save that never reports failed
    /// (`a_save_that_never_answers_fails_at_its_deadline`).
    #[test]
    fn a_sleep_holds_then_saves_then_stops() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let s = l.apply(Event::Alarm, T + IDLE_MS);
        assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }), "first its hold");
        assert_eq!(s.alarm_ms, Some(T + IDLE_MS + HOLD_DEADLINE_MS));
        let s = l.apply(Event::Held { generation: g, held: true }, T + IDLE_MS + 500);
        assert_eq!(s.actions, vec![Action::Save { generation: g, held: true, seq: 1 }], "then its save");
        assert_eq!(s.alarm_ms, Some(T + IDLE_MS + 500 + SAVE_DEADLINE_MS));
        let s = l.apply(Event::Saved { generation: g, seq: 1 }, T + IDLE_MS + 4_000);
        assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }], "then its stop, a snapshot of the save first");
        assert_eq!(s.alarm_ms, Some(T + IDLE_MS + 4_000 + SLEEP_DEADLINE_MS));
        let s = l.apply(Event::Asleep { generation: g }, T + IDLE_MS + 12_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: T + IDLE_MS, to_ms: T + IDLE_MS + 12_000 }]);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: T + IDLE_MS + 12_000, restart: false, quiet: true }));
        assert_eq!((l.phase.clone(), s.alarm_ms), (Phase::ASLEEP, None));

        // an image that never answers is saved anyway, recorded as not held;
        // a hold or a stop that never reports is asked for again at its deadline
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let s = l.apply(Event::Alarm, T + 10_000 + HOLD_DEADLINE_MS);
        assert_eq!(s.actions, vec![Action::Sleep { generation: g }], "its hold, again");
        let s = l.apply(Event::Held { generation: g, held: false }, T + 10_000 + HOLD_DEADLINE_MS + HOLD_WAIT_MS);
        assert_eq!(s.actions, vec![Action::Save { generation: g, held: false, seq: 1 }]);
        let at = T + 10_000 + HOLD_DEADLINE_MS + HOLD_WAIT_MS;
        let s = l.apply(Event::Saved { generation: g, seq: 1 }, at + 4_000);
        assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }]);
        let s = l.apply(Event::Alarm, at + 4_000 + SLEEP_DEADLINE_MS);
        assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }], "its stop, again");
        // its container exits as it is signalled: the stop reports that
        // itself, and its end is the sleep's, saved
        assert!(l.apply(Event::Exited { generation: g }, at + 5_000 + SLEEP_DEADLINE_MS).actions.is_empty());
        let s = l.apply(Event::Asleep { generation: g }, at + 5_100 + SLEEP_DEADLINE_MS);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: at + 5_100 + SLEEP_DEADLINE_MS, restart: false, quiet: true }));
        // a lifecycle stored mid-sleep before saves were its own reads as in
        // its stop: its deadline asks for the stop, unsaved
        let mut v = serde_json::to_value(Lifecycle { phase: Phase::Sleeping { generation: 1, since_ms: T, owner: false, restarting: false, owes_start: false }, generation: 1, ..Lifecycle::new() }).unwrap();
        v.as_object_mut().unwrap().retain(|k, _| k != "saving");
        let (mut old, _) = Lifecycle::read(&v.to_string()).unwrap();
        assert_eq!(old.apply(Event::Alarm, T + SLEEP_DEADLINE_MS).actions, vec![Action::Stop { generation: 1, saved: false }]);
    }

    /// Goal (the hold race): a keepalive that opens while an idle sleep holds
    /// its guest (it took work as it was held) cancels the sleep: it stays
    /// awake, held no more, and is saved when that work ends. Once its save
    /// is under way, or for its owner's sleep, it does not: the sleep goes
    /// on, and the next life runs what it had not claimed.
    #[test]
    fn a_keepalive_during_the_hold_cancels_the_sleep() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Alarm, T + IDLE_MS);
        let s = l.apply(Event::Opened { socket: Socket::Keepalive }, T + IDLE_MS + 200);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }]);
        assert_eq!(l.phase, Phase::Awake { generation: g, since_ms: T + IDLE_MS - 1 }, "awake as before its sleep");
        assert!(l.saving().is_none(), "awake, held no more");
        // its hold's late answer changes nothing, but lets go again (its
        // touch may have landed after the cancel let go of it)
        let before = l.clone();
        assert_eq!(l.apply(Event::Held { generation: g, held: true }, T + IDLE_MS + 300).actions, vec![Action::Unhold { generation: g }]);
        assert_eq!(l, before);
        // the work ends: saved as any work is, then idle it sleeps again
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + IDLE_MS + 60_000);
        let s = l.apply(Event::Alarm, T + IDLE_MS + 60_000 + SAVE_SETTLE_MS);
        assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }));
        // once the sleep saves, the keepalive dies with its container
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Alarm, T + IDLE_MS);
        l.apply(Event::Held { generation: g, held: true }, T + IDLE_MS + 100);
        let s = l.apply(Event::Opened { socket: Socket::Keepalive }, T + IDLE_MS + 200);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Sleeping { .. }), "{s:?}");
        // and its owner's sleep is never cancelled
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let s = l.apply(Event::Opened { socket: Socket::Keepalive }, T + 10_100);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Sleeping { .. }), "{s:?}");
        // nor an idle sleep its owner asked for again as it held
        let mut l = Lifecycle::new();
        started(&mut l, T);
        l.apply(Event::Alarm, T + IDLE_MS);
        l.apply(Event::Sleep, T + IDLE_MS + 50);
        let s = l.apply(Event::Opened { socket: Socket::Keepalive }, T + IDLE_MS + 100);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Sleeping { .. }), "{s:?}");
    }
}
