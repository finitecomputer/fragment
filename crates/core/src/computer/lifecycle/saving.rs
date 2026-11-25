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
    /// `unheld`: awake, it let go of its hold once its home's record was
    /// taken (`Event::HomeSaved`), and takes its work's as its guest works.
    Save {
        since_ms: i64,
        held: bool,
        seq: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unheld: bool,
    },
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
        self.saving = Some(Saving::Save { since_ms: now_ms, held, seq: self.save_seq, unheld: false });
        step.actions.push(Action::Save { generation, held, seq: self.save_seq });
    }

    /// Save `seq`'s home's record is taken (`Event::HomeSaved`). The hold
    /// was for the home, whose databases the guest copied under it
    /// (docs/computers.md, "The hold"): an awake save lets go of it now,
    /// once, so its guest claims turns again while its work is copied, as
    /// tools already running always wrote to it during a save. A sleep's
    /// save keeps its hold until its container is gone.
    pub(super) fn home_saved(&mut self, generation: u64, seq: u64, step: &mut Step) {
        let Some(Saving::Save { seq: s, unheld, .. }) = &mut self.saving else { return };
        if *s == seq && !*unheld && matches!(self.phase, Phase::Awake { generation: g, .. } if g == generation) {
            *unheld = true;
            step.actions.push(Action::Unhold { generation });
        }
    }

    /// Whether the guest of start `generation` may be let go of now: it is
    /// awake (or starting, from a snapshot its sleep held), and no hold,
    /// nor a save that holds it, is under way (none, or an awake save past
    /// its home). The DO asks it as it unmarks, so an unhold never takes a
    /// newer hold's mark (a sleep's, asked meanwhile).
    pub fn lets_go(&self, generation: u64) -> bool {
        matches!(self.phase, Phase::Awake { generation: g, .. } | Phase::Starting { generation: g, .. } if g == generation) && matches!(self.saving, None | Some(Saving::Save { unheld: true, .. }))
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

    use serde_json::json;

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

    /// Goal (P9): an awake save holds its guest only while its home's
    /// record is taken: once that is in (`HomeSaved`) it lets go, once (its
    /// end lets go again, in case that exec failed); a sleep keeps its hold
    /// to its stop, and the DO may unmark only while `lets_go` says so.
    /// Method: the
    /// events as the DO reports them, valid, invalid (another save's,
    /// another start's, before its save began, a sleep's), replayed, and
    /// across a store and read back.
    #[test]
    fn an_awake_save_holds_its_home_only() {
        // a save asked once a turn's work settles, its hold answered
        let awake_save = || {
            let mut l = Lifecycle::new();
            let g = started(&mut l, T);
            l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
            l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
            let at = T + 10_000 + SAVE_SETTLE_MS;
            assert_eq!(l.apply(Event::Alarm, at).actions.last(), Some(&Action::Hold { generation: g }));
            let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
            (l, g, seq, at)
        };
        // valid: its home in, it lets go; its end lets go of nothing more
        let (mut l, g, seq, at) = awake_save();
        let alarm = l.next_alarm();
        assert!(!l.lets_go(g), "its home is not in yet");
        let s = l.apply(Event::HomeSaved { generation: g, seq }, at + 2_000);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }]);
        assert!(l.lets_go(g) && !l.lets_go(g + 1), "its own start's guest, its home in");
        assert_eq!((s.alarm_ms, l.saving().map(Saving::deadline_ms)), (alarm, Some(at + 500 + SAVE_DEADLINE_MS)), "its work's record is still the save's, under its deadline");
        // replay: once only
        let before = l.clone();
        assert!(l.apply(Event::HomeSaved { generation: g, seq }, at + 2_100).actions.is_empty());
        assert_eq!(l.saving(), before.saving());
        // restart: stored and read back between its home and its end
        let mut l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap().0;
        assert_eq!(l.saving(), before.saving());
        let s = l.apply(Event::Saved { generation: g, seq }, at + 60_000);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }], "let go again (its first unhold's exec may have failed)");
        assert!(l.saving().is_none() && l.lets_go(g));
        assert_eq!(s.note, SaveNote::Clear);
        assert!(l.apply(Event::HomeSaved { generation: g, seq }, at + 60_100).actions.is_empty(), "a late one, once it is done");
        // its work's record failing after it let go: tried again, its
        // unhold asked again (nothing holds it: an unhold is a no-op)
        let (mut l, g, seq, at) = awake_save();
        l.apply(Event::HomeSaved { generation: g, seq }, at + 2_000);
        let s = l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, at + 9_000);
        assert_eq!((s.actions, s.alarm_ms.is_some()), (vec![Action::Unhold { generation: g }], true));
        // a save whose isolate went with its report, after its home: failed
        // at its deadline, as any
        let (mut l, g, seq, at) = awake_save();
        l.apply(Event::HomeSaved { generation: g, seq }, at + 2_000);
        let s = l.apply(Event::Alarm, at + 500 + SAVE_DEADLINE_MS);
        assert_eq!(s.actions.last(), Some(&Action::Unhold { generation: g }), "{s:?}");
        assert!(l.saving().is_none());
        // invalid: another save's, another start's, before its save began
        let (mut l, g, seq, at) = awake_save();
        let before = l.clone();
        assert!(l.apply(Event::HomeSaved { generation: g, seq: seq + 1 }, at + 2_000).actions.is_empty());
        assert!(l.apply(Event::HomeSaved { generation: g + 1, seq }, at + 2_000).actions.is_empty());
        assert_eq!(l.saving(), before.saving());
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, at + 3_000).actions, vec![Action::Unhold { generation: g }], "never let go: held to its end, as before");
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        l.apply(Event::Alarm, T + 10_000 + SAVE_SETTLE_MS);
        assert!(!l.lets_go(g), "held: a hold under way is never let go of");
        // a start (from a snapshot its sleep held) lets go of its guest,
        // unless its owner's sleep held it meanwhile
        let mut st = Lifecycle::new();
        let s = st.apply(Event::Wake { why: Wake::Owner }, T);
        let Some(Action::Start { generation: sg }) = s.actions.first().cloned() else { panic!("{s:?}") };
        assert!(st.lets_go(sg) && !st.lets_go(sg + 1));
        st.apply(Event::Sleep, T + 100);
        assert!(!st.lets_go(sg), "its owner's sleep holds it");
        assert!(l.apply(Event::HomeSaved { generation: g, seq: 1 }, T + 41_000).actions.is_empty(), "at its hold, no save is under way");
        // a sleep's save keeps its hold to its stop
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Alarm, T + IDLE_MS);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + IDLE_MS + 500));
        assert!(l.apply(Event::HomeSaved { generation: g, seq }, T + IDLE_MS + 2_000).actions.is_empty());
        assert!(!l.lets_go(g), "a sleep's save holds its guest");
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, T + IDLE_MS + 4_000).actions, vec![Action::Stop { generation: g, saved: true }]);
        // its owner's sleep while an awake save copies its work: the sleep's
        // hold is never let go by the awake save's late reports
        let (mut l, g, seq, at) = awake_save();
        l.apply(Event::HomeSaved { generation: g, seq }, at + 2_000);
        assert_eq!(l.apply(Event::Sleep, at + 3_000).actions, vec![Action::Sleep { generation: g }]);
        assert!(l.apply(Event::HomeSaved { generation: g, seq }, at + 3_100).actions.is_empty());
        assert!(!l.lets_go(g), "the sleep's hold");
        assert!(matches!(l.saving(), Some(Saving::Hold { .. })), "the sleep's own hold: {:?}", l.saving());
        // the sleep's save asked as the awake one copies its work: the awake
        // one's late report changes nothing (`Saves::overtaken` says whether
        // the DO keeps it)
        let (_, _, newer) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 3_150));
        assert!(newer > seq);
        assert!(l.apply(Event::Saved { generation: g, seq }, at + 3_200).actions.is_empty());
        assert!(l.apply(Event::SaveFailed { generation: g, seq, why: "overtaken".into() }, at + 3_250).actions.is_empty());
        assert!(!l.lets_go(g));
        // a lifecycle stored before `unheld` was kept reads it unset: held to
        // its end, as that release would have
        let (l, g, seq, at) = awake_save();
        let mut v = serde_json::to_value(&l).unwrap();
        assert!(v["saving"].get("unheld").is_none(), "unset, not stored: {v}");
        v["saving"].as_object_mut().unwrap().remove("unheld");
        let (mut old, _) = Lifecycle::read(&v.to_string()).unwrap();
        assert_eq!(old.apply(Event::Saved { generation: g, seq }, at + 3_000).actions, vec![Action::Unhold { generation: g }]);
    }

    /// Goal (P9, alexlwn123 on #1378): an awake save past its home and its
    /// owner's sleep's or restart's save, asked as it copies its work, are
    /// kept in the order they began, whichever ends first: the newer
    /// capture is the newest kept, and the restart or sleep goes on saved.
    /// An older one ending last is let go; one ending first is kept until
    /// the newer is (and stays the newest if the newer fails), by their
    /// numbers, never their clocks, which two may share (Codex on #1378).
    /// Method: the lifecycle's events, each
    /// save kept or let go as the DO's `save` does (`Saves::overtaken` with
    /// its `seq`), valid (both orders), the newer failing, and the saves
    /// stored and read back between the two ends (a restart of the DO).
    #[test]
    fn overlapping_saves_are_kept_in_the_order_they_began() {
        // the DO's `save`, ending at `at`: kept unless a newer one was
        fn end(l: &mut Lifecycle, saves: &mut Saves, g: u64, seq: u64, id: &str, at: i64) -> Step {
            let records = vec![json!({ "id": id, "dir": "/data" }), json!({ "id": format!("{id}-work"), "dir": "/data/work" })];
            if saves.overtaken(g, seq) {
                assert!(saves.let_go(records).is_empty());
                return l.apply(Event::SaveFailed { generation: g, seq, why: "a newer save was kept as it ran".into() }, at);
            }
            saves.saved(g, records, seq, at, true);
            l.apply(Event::Saved { generation: g, seq }, at)
        }
        let newest = |saves: &Saves| saves.all().first().map(|s| s.id.clone());
        let reload = |saves: &Saves| serde_json::from_str::<Saves>(&serde_json::to_string(saves).unwrap()).unwrap();
        for owner in [Event::Sleep, Event::Restart { generation: 0 }] {
            let restart = matches!(owner, Event::Restart { .. });
            // an awake save (A) lets go past its home; its owner's sleep or
            // restart holds, and its save (B) begins as A copies its work
            let overlap = || {
                let mut l = Lifecycle::new();
                let g = started(&mut l, T);
                l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
                l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
                let at = T + 10_000 + SAVE_SETTLE_MS;
                l.apply(Event::Alarm, at);
                let (_, _, a) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
                l.apply(Event::HomeSaved { generation: g, seq: a }, at + 1_500);
                l.apply(owner.clone(), at + 3_000);
                let (_, _, b) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 4_000));
                assert!(b > a);
                (l, g, a, b, at)
            };
            let stops = |s: &Step, g: u64| s.actions == vec![Action::Stop { generation: g, saved: true }];
            // A ends first (it has only its work left): kept, then B is, the newest
            let (mut l, g, a, b, at) = overlap();
            let mut saves = Saves::default();
            assert!(end(&mut l, &mut saves, g, a, "a", at + 6_000).actions.is_empty(), "{owner:?}");
            let mut saves = reload(&saves);
            let s = end(&mut l, &mut saves, g, b, "b", at + 9_000);
            assert_eq!(newest(&saves).as_deref(), Some("b"), "the newer capture is the newest kept: {owner:?}");
            assert!(stops(&s, g), "its sleep goes on, saved: {owner:?}: {s:?}");
            assert_eq!(saves.all().len(), 2, "A, older, still kept behind it: {owner:?}");
            // B ends first: kept; A, ending after it, is let go
            let (mut l, g, a, b, at) = overlap();
            let mut saves = Saves::default();
            assert!(stops(&end(&mut l, &mut saves, g, b, "b", at + 6_000), g), "{owner:?}");
            let mut saves = reload(&saves);
            assert!(end(&mut l, &mut saves, g, a, "a", at + 9_000).actions.is_empty(), "{owner:?}");
            assert_eq!((newest(&saves).as_deref(), saves.all().len()), (Some("b"), 1), "A let go, never kept ahead of B: {owner:?}");
            assert_eq!(saves.forgetting().len(), 2, "its records to delete: {owner:?}");
            // B fails: A, ending before or after it, is the newest kept
            for a_first in [true, false] {
                let (mut l, g, a, b, at) = overlap();
                let mut saves = Saves::default();
                if a_first {
                    end(&mut l, &mut saves, g, a, "a", at + 6_000);
                }
                let failed = l.apply(Event::SaveFailed { generation: g, seq: b, why: "R2 is down".into() }, at + 8_000);
                if !a_first {
                    end(&mut l, &mut saves, g, a, "a", at + 9_000);
                }
                assert_eq!(newest(&saves).as_deref(), Some("a"), "a capture kept for a newer one only asked: {owner:?} {a_first}");
                if restart {
                    assert_eq!(failed.actions, vec![Action::Stop { generation: g, saved: false }], "a restart stops whether or not it saved: {owner:?}");
                }
            }
        }
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
                assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })), "it never sleeps (its awake time is metered, as any computer's): {s:?}");
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
    /// Goal (Codex on #1375): a stop's deadline is minutes, for its
    /// snapshot, yet one whose end its isolate lost is found within a minute,
    /// as when the deadline was one: its alarm looks every `STOP_LOOK_MS`
    /// whether its container is gone (`Action::Look`), even once a new
    /// isolate adopted it as it went down, whose exit no report ends (Codex's
    /// second round). A wake that came during the stop starts then, and the
    /// sleep is metered to that look, no later. Valid (a look finds it gone),
    /// replay (a look that finds it still stopping changes nothing, and looks
    /// again), the bound (asked again at its deadline). Method: the
    /// lifecycle's steps, as the DO hears them, its report lost.
    #[test]
    fn a_stop_whose_end_was_lost_is_found_within_a_minute() {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let slept = T + IDLE_MS;
        l.apply(Event::Alarm, slept);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, slept + 500));
        let stop = slept + 4_000;
        let s = l.apply(Event::Saved { generation: g, seq }, stop);
        assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }]);
        assert_eq!(s.alarm_ms, Some(stop + STOP_LOOK_MS), "looked at within a minute, its deadline minutes away");
        // a wake as it stops: a start once it ends
        let s = l.apply(Event::Wake { why: Wake::Record }, stop + 2_000);
        assert!(s.actions.is_empty() && s.alarm_ms == Some(stop + STOP_LOOK_MS), "{s:?}");
        // a look that finds it still stopping (its snapshot runs on, or a
        // new isolate adopted it as it went down): nothing more
        let before = l.clone();
        let s = l.apply(Event::Alarm, stop + STOP_LOOK_MS);
        assert!(s.actions == vec![Action::Look { generation: g }] && l == before, "a look, nothing else: {s:?}");
        assert_eq!(s.alarm_ms, Some(stop + 2 * STOP_LOOK_MS), "it looks again");
        // its end lost with its isolate: the next look finds its container
        // gone and reports it asleep (`Action::Look`)
        let found = stop + 2 * STOP_LOOK_MS;
        assert_eq!(l.apply(Event::Alarm, found).actions, vec![Action::Look { generation: g }]);
        let s = l.apply(Event::Asleep { generation: g }, found);
        assert!(s.actions.iter().any(|a| matches!(a, Action::Meter { to_ms, .. } if *to_ms == found)), "metered to the look: {s:?}");
        assert_eq!(l.apply(Event::Alarm, found).actions, vec![Action::Start { generation: g + 1 }], "the wake's start");
        // the bound: never found, its stop is asked again at its deadline
        let mut l = before;
        let s = l.apply(Event::Alarm, stop + SLEEP_DEADLINE_MS - 1);
        assert!(s.actions == vec![Action::Look { generation: g }] && s.alarm_ms == Some(stop + SLEEP_DEADLINE_MS), "its last look is its deadline: {s:?}");
        assert_eq!(l.apply(Event::Alarm, stop + SLEEP_DEADLINE_MS).actions, vec![Action::Stop { generation: g, saved: true }]);
    }

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
        assert_eq!(s.alarm_ms, Some(T + IDLE_MS + 4_000 + STOP_LOOK_MS), "looked at in a minute (its deadline is `SLEEP_DEADLINE_MS`)");
        let s = l.apply(Event::Asleep { generation: g }, T + IDLE_MS + 12_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: T + IDLE_MS, to_ms: T + IDLE_MS + 12_000 }]);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: T + IDLE_MS + 12_000, restart: false, quiet: true, unchanged: true }));
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
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: true, at_ms: at + 5_100 + SLEEP_DEADLINE_MS, restart: false, quiet: true, unchanged: true }));
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
