//! A save that fails (docs/computers.md, Lifecycle): tried again after a
//! growing pause; a sleep's keeps its container up to its bound, the time
//! kept free, then sleeps unsaved and says so; and what its owner is told.

use serde::{Deserialize, Serialize};

use super::{Action, Lifecycle, Phase, Rules, SaveNote, Saving, Step};
use crate::computer::limits::{FAILED_WHY_MAX_BYTES, SAVE_DEADLINE_MS, SAVE_EVERY_MS, SAVE_RETRY_MS};
use crate::search::cut;

/// A run of saves that failed in a row in one life: since when, and the
/// last one's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failing {
    pub since_ms: i64,
    pub why: String,
}

/// What its saves failing means now, for its owner (`Lifecycle::unsaved`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsaved {
    /// Saves of this life that failed in a row, and the run's first.
    pub failures: u32,
    pub failing: Failing,
    /// A sleep's save failed since then: the bound runs from this.
    pub sleep_failed_since_ms: Option<i64>,
    /// When it is put to sleep unsaved if no save works first and nothing
    /// new holds it: `None` while it is in use (a socket open) or always on.
    pub stops_at_ms: Option<i64>,
}

impl Lifecycle {
    /// What its saves failing means for its owner now: `None` while none
    /// has failed since the last that worked (or this life's start).
    pub fn unsaved(&self, rules: &Rules) -> Option<Unsaved> {
        let failing = self.failing.clone()?;
        assert!(self.save_failures > 0, "a run of failures has one at least");
        let stops_at_ms = match self.unsaved_since_ms {
            Some(since) if !self.always_on && !self.open() => {
                // a sleep is tried when its holds run out and its next try
                // is due; the one at or past the bound stops it
                Some((since + rules.unsaved_max_ms).max(self.held_until_ms).max(self.save_due_ms.unwrap_or(i64::MIN)))
            }
            _ => None,
        };
        Some(Unsaved { failures: self.save_failures, failing, sleep_failed_since_ms: self.unsaved_since_ms, stops_at_ms })
    }

    /// Since when a sleep's save has kept failing (it stays awake), if one has.
    pub fn unsaved_since_ms(&self) -> Option<i64> {
        self.unsaved_since_ms
    }

    /// Since when its awake time is free, while it is (`unmetered`): the
    /// start of the try whose save failed first.
    pub fn free_since_ms(&self) -> Option<i64> {
        self.unmetered.map(|(from, _)| from)
    }

    /// A save of this life failed: one more in the run, since its first.
    fn save_failed(&mut self, why: &str, now_ms: i64) {
        self.save_failures += 1;
        let why = cut(why, FAILED_WHY_MAX_BYTES).to_string();
        match &mut self.failing {
            Some(f) => f.why = why,
            None => self.failing = Some(Failing { since_ms: now_ms, why }),
        }
        assert!(self.save_failures > 0 && self.failing.is_some(), "a failure is counted and told");
    }

    /// A sleep's save failed, the first since one worked: its unsaved
    /// window opens. The bound counts from this failure; the free time from
    /// the start of the try that failed (its sleep's hold), since the
    /// computer was kept for that save as long as it ran, a hung one to its
    /// bound: metered up to that start, then none of it, up to the bound.
    fn begin_unsaved(&mut self, now_ms: i64, rules: &Rules, step: &mut Step) {
        assert!(self.unsaved_since_ms.is_none() && self.unmetered.is_none(), "one unsaved window at a time");
        assert!(matches!(self.phase, Phase::Sleeping { .. }), "a sleep's failure opens the window");
        // the sleep set it as it began; what was metered stays metered
        let tried = self.save_asked_ms.max(self.metered_to_ms).min(now_ms);
        self.meter(tried, step);
        // and the try's time up to here is the window's: passed over
        self.metered_to_ms = self.metered_to_ms.max(now_ms);
        self.unsaved_since_ms = Some(now_ms);
        self.unmetered = Some((tried, now_ms + rules.unsaved_max_ms));
    }

    /// The unsaved window ends (a save worked, it stops, or its container
    /// went): its free time is passed over up to here, and what comes
    /// after is metered as any.
    pub(super) fn end_unsaved(&mut self, now_ms: i64, step: &mut Step) {
        if self.unmetered.is_some() {
            self.meter(now_ms, step);
        }
        self.unsaved_since_ms = None;
        self.unmetered = None;
    }

    /// The pause before the next try of a save that failed `save_failures`
    /// times in a row.
    fn retry_pause_ms(&self) -> i64 {
        assert!(self.save_failures > 0, "a retry follows a failure");
        (SAVE_RETRY_MS << (self.save_failures - 1).min(8)).min(SAVE_EVERY_MS)
    }

    /// A sleep's save failed: within the bound it keeps its container ,
    /// awake again and held no more, and its sleep is tried again after a
    /// pause, the last try at the bound itself; past it, it sleeps unsaved,
    /// and says so. Its owner's restart keeps nothing: it stops unsaved at
    /// once, as they asked, and starts again.
    pub(super) fn sleep_save_failed(&mut self, generation: u64, why: &str, now_ms: i64, rules: &Rules, step: &mut Step) {
        self.save_failed(why, now_ms);
        if matches!(self.phase, Phase::Sleeping { restarting: true, .. }) {
            self.end_unsaved(now_ms, step);
            self.saving = Some(Saving::Stop { since_ms: now_ms, saved: false });
            step.actions.push(Action::Stop { generation, saved: false });
            step.note = SaveNote::Says(format!("its restart could not save /data first ({why}), so it started again from its last save"));
            return;
        }
        if self.unsaved_since_ms.is_none() {
            self.begin_unsaved(now_ms, rules, step);
        }
        let since = self.unsaved_since_ms.expect("its window is open");
        let bound = since + rules.unsaved_max_ms;
        if now_ms < bound {
            // awake again: what its sleep was (its owner's, a start owed)
            // goes with it; a wake heard as it went held it
            self.phase = Phase::Awake { generation, since_ms: now_ms };
            self.saving = None;
            // the try at the bound is the one that stops it: its owner is
            // told that time, and it is kept
            self.save_due_ms = Some((now_ms + self.retry_pause_ms()).min(bound));
            step.actions.push(Action::Unhold { generation });
            step.note = SaveNote::Says(format!("its sleep could not save /data ({why}): it stays awake and tries again, for up to {} minutes", rules.unsaved_max_ms / 60_000));
        } else {
            // the try under way at the bound, begun within the window, was
            // kept for a failed save as the others were: free to its failure
            if let Some((from, to)) = self.unmetered {
                if self.save_asked_ms <= to {
                    self.unmetered = Some((from, to.max(now_ms)));
                }
            }
            self.end_unsaved(now_ms, step);
            self.saving = Some(Saving::Stop { since_ms: now_ms, saved: false });
            step.actions.push(Action::Stop { generation, saved: false });
            step.note = SaveNote::Says(format!(
                "it slept unsaved: its saves failed for {} minutes ({why}), so its next wake goes back to its last save",
                rules.unsaved_max_ms / 60_000
            ));
        }
    }

    /// An awake save failed: tried again after a pause, held no more.
    pub(super) fn awake_save_failed(&mut self, generation: u64, why: &str, now_ms: i64, step: &mut Step) {
        assert!(matches!(self.phase, Phase::Awake { .. }), "an awake save's failure is an awake computer's");
        self.saving = None;
        self.save_failed(why, now_ms);
        self.save_due_ms = Some(now_ms + self.retry_pause_ms());
        step.actions.push(Action::Unhold { generation });
    }

    /// A step of the save under way that ran past its deadline. A hold or a
    /// stop is asked for again: the cell bounds each well within its
    /// deadline, so one past it lost its report (its isolate died with it,
    /// say). A save past its deadline failed, counted as any failure is:
    /// asked again, a backup that never returns would keep its sleep held
    /// and billed for good, and the unsaved bound would never end it.
    pub(super) fn overdue(&mut self, generation: u64, saving: Saving, now_ms: i64, rules: &Rules, step: &mut Step) {
        let sleeping = matches!(self.phase, Phase::Sleeping { .. });
        match saving {
            Saving::Hold { .. } => {
                self.saving = Some(Saving::Hold { since_ms: now_ms });
                step.actions.push(if sleeping { Action::Sleep { generation } } else { Action::Hold { generation } });
            }
            Saving::Save { seq, .. } => {
                let why = format!("save {seq} did not report within {} minutes", SAVE_DEADLINE_MS / 60_000);
                match sleeping {
                    true => self.sleep_save_failed(generation, &why, now_ms, rules, step),
                    false => self.awake_save_failed(generation, &why, now_ms, step),
                }
            }
            Saving::Stop { saved, .. } => {
                assert!(sleeping, "only a sleep stops");
                self.saving = Some(Saving::Stop { since_ms: now_ms, saved });
                step.actions.push(Action::Stop { generation, saved });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{ComputerNotice, LifeEnd};

    use super::*;
    use crate::computer::testing::{rec, save_asked, started, Computer, T};
    use crate::computer::*;

    /// Goal : a sleep whose save fails keeps its container (awake, held
    /// no more, saying so) and tries again after a growing pause; past
    /// `unsaved_max_ms` it sleeps unsaved, saying so, and that life's end
    /// says it did not save. A save that works on a try sleeps it, and
    /// clears what it said. Method: one computer's tries, every one
    /// failing, up to the bound; then one that works.
    #[test]
    fn a_sleep_whose_save_fails_keeps_the_container() {
        let rules = Rules::default();
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let mut at = T + IDLE_MS;
        let mut s = l.apply_with(Event::Alarm, at, &rules);
        let first = at;
        let mut tries = 0;
        // bounded: a try a pause apart, the pause growing, for the bound
        let stopped = loop {
            assert!(tries < 40, "the bound ends the tries");
            assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }), "try {tries}: {s:?}");
            let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, at + 500, &rules));
            at += 2_000;
            s = l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, at, &rules);
            tries += 1;
            if s.actions.iter().any(|a| matches!(a, Action::Stop { .. })) {
                break s;
            }
            let metered = s.actions.iter().filter(|a| matches!(a, Action::Meter { .. })).count();
            assert!(s.actions.last() == Some(&Action::Unhold { generation: g }) && metered == 0, "it keeps its container, metered up to its first try's start (its alarm metered it) only: {s:?}");
            assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g), "the same container, awake");
            assert!(matches!(&s.note, SaveNote::Says(why) if why.contains("could not save") && why.contains("R2 is down")), "{:?}", s.note);
            assert_eq!(l.unsaved_since_ms(), Some(first + 2_000));
            let bound = first + 2_000 + rules.unsaved_max_ms;
            let due = l.save_due_ms.expect("its next try is due");
            assert_eq!(due - at, (SAVE_RETRY_MS << (tries - 1).min(8)).min(SAVE_EVERY_MS).min(bound - at), "try {tries}'s pause, the last at the bound");
            // its owner is told when it stops: the bound, nothing holding it
            let u = l.unsaved(&rules).expect("its saves are failing");
            assert_eq!((u.failures, u.failing.since_ms, u.sleep_failed_since_ms, u.stops_at_ms), (tries, first + 2_000, Some(first + 2_000), Some(bound)), "try {tries}");
            // the alarms before it meter, and try nothing: unwanted, it waits
            for _ in 0..8 {
                let Some(next) = s.alarm_ms.filter(|next| *next < due) else { break };
                s = l.apply_with(Event::Alarm, next, &rules);
                assert!(s.actions.iter().all(|a| matches!(a, Action::Meter { .. })), "before its try: {s:?}");
            }
            assert_eq!(s.alarm_ms, Some(due), "its try is the alarm it asks for");
            at = due;
            s = l.apply_with(Event::Alarm, at, &rules);
        };
        assert!(at - first >= rules.unsaved_max_ms && at - first < rules.unsaved_max_ms + 10_000, "it gave up at its bound, its last try at the bound itself: {} ms", at - first);
        // free from its first try to the failure of its last, the try at the
        // bound (Paul, 2026-10-08); its stop's seconds after it are its owner's
        assert_eq!(stopped.actions, vec![Action::Stop { generation: g, saved: false }], "nothing past the bound is metered for the try at it");
        assert!(matches!(&stopped.note, SaveNote::Says(why) if why.contains("slept unsaved")), "{:?}", stopped.note);
        let s = l.apply_with(Event::Asleep { generation: g }, at + 5_000, &rules);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: false, at_ms: at + 5_000, restart: false, quiet: true }), "its next wake is a rollback");
        assert_eq!(s.actions.first(), Some(&Action::Meter { from_ms: at, to_ms: at + 5_000 }), "the stop's own seconds");
        // what it said goes with the next life's first save that works
        let t = at + 60_000;
        let g = started(&mut l, t);
        l.apply(Event::Opened { socket: Socket::Keepalive }, t + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, t + 5_000);
        assert_eq!(l.apply(Event::Alarm, t + 5_000 + SAVE_SETTLE_MS).actions.last(), Some(&Action::Hold { generation: g }));
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, t + 36_000));
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, t + 37_000).note, SaveNote::Clear, "slept unsaved is said until a save works");

        // a try that works: it sleeps, and the note goes
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 10_500));
        let s = l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, T + 11_000);
        assert_eq!(s.actions, vec![Action::Meter { from_ms: T + 3_000, to_ms: T + 10_000 }, Action::Unhold { generation: g }], "an owner's sleep keeps its container too, metered to its try's start");
        assert_eq!(s.alarm_ms, Some(T + 11_000 + SAVE_RETRY_MS));
        let s = l.apply(Event::Alarm, T + 11_000 + SAVE_RETRY_MS);
        assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }), "unwanted still, its retry is a sleep");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 72_000));
        let s = l.apply(Event::Saved { generation: g, seq }, T + 73_000);
        assert_eq!((s.actions.clone(), s.note.clone()), (vec![Action::Stop { generation: g, saved: true }], SaveNote::Clear));
        assert_eq!(l.unsaved_since_ms(), None);
        // and awake, a save that fails is tried again later, held no more
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 4_000);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        l.apply(Event::Alarm, T + 40_000);
        let s = l.apply(Event::SaveFailed { generation: g, seq: 0, why: "no container".into() }, T + 41_000);
        assert_eq!((s.actions, s.alarm_ms), (vec![Action::Unhold { generation: g }], Some(T + 41_000 + SAVE_RETRY_MS)));
        let s = l.apply(Event::Alarm, T + 41_000 + SAVE_RETRY_MS);
        assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }), "wanted (a tab is open), it saves awake: {s:?}");
    }

    /// Goal (Paul, 2026-10-08): the awake time a computer is kept for saves
    /// the platform failed is free. A quick failure: from the start of the
    /// try whose save failed (its sleep's hold, seconds before) to the bound
    /// after the failure nothing is metered; before it, and after it (held
    /// past the bound by a record, and once a save works), it is metered as
    /// any; the bound counts from the failure, as before. Replay: a late
    /// failure's report, and an alarm asked again, meter nothing twice and
    /// move nothing. Restart: its lifecycle stored and read back mid-window
    /// meters the same. Method: one computer's alarms, every meter kept.
    #[test]
    fn the_time_kept_for_failed_saves_is_free() {
        let rules = Rules { unsaved_max_ms: 10 * 60_000 };
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let mut metered: Vec<(i64, i64)> = vec![];
        let keep = |s: &Step, metered: &mut Vec<(i64, i64)>| {
            for a in &s.actions {
                if let Action::Meter { from_ms, to_ms } = a {
                    metered.push((*from_ms, *to_ms));
                }
            }
        };
        // a hold the lifecycle asked for, answered: the save works, or fails
        let answer = |l: &mut Lifecycle, s: &Step, at: i64, works: bool, metered: &mut Vec<(i64, i64)>| {
            if s.actions.contains(&Action::Hold { generation: g }) {
                let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, at + 500, &rules));
                let done = match works {
                    true => Event::Saved { generation: g, seq },
                    false => Event::SaveFailed { generation: g, seq, why: "R2 is down".into() },
                };
                keep(&l.apply_with(done, at + 1_000, &rules), metered);
            }
        };
        let s = l.apply_with(Event::Alarm, T + IDLE_MS, &rules);
        keep(&s, &mut metered);
        assert!(s.actions.contains(&Action::Sleep { generation: g }));
        let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, T + IDLE_MS + 500, &rules));
        let (tried, f) = (T + IDLE_MS, T + IDLE_MS + 2_000);
        let s = l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, f, &rules);
        keep(&s, &mut metered);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Meter { .. })), "its try's start was metered to already (its alarm): {s:?}");
        assert_eq!(metered.last(), Some(&(T + 3_000, tried)), "metered up to its try's start");
        assert_eq!((l.unmetered, l.unsaved_since_ms()), (Some((tried, f + rules.unsaved_max_ms)), Some(f)), "free from its try's start; the bound counts from the failure");
        // replay: the same failure's late report is nothing
        let before = l.clone();
        assert!(l.apply_with(Event::SaveFailed { generation: g, seq, why: "late".into() }, f + 100, &rules).actions.is_empty() && l == before);
        // a record holds it past the bound; its alarms within the window meter nothing
        keep(&l.apply_with(Event::Wake { why: Wake::Record }, f + 500, &rules), &mut metered);
        let at = f + 5 * 60_000;
        let s = l.apply_with(Event::Alarm, at, &rules);
        keep(&s, &mut metered);
        answer(&mut l, &s, at, false, &mut metered);
        let s = l.apply_with(Event::Alarm, at, &rules);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Meter { .. })), "an alarm again meters nothing twice: {s:?}");
        assert!(metered.iter().all(|(_, to)| *to <= tried), "nothing in the window is metered: {metered:?}");
        // restart: stored mid-window and read back, it meters the same
        let mut l: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(l.unmetered, Some((tried, f + rules.unsaved_max_ms)));
        // past the bound, still held by the record: metered from the bound on
        let at = f + rules.unsaved_max_ms + 60_000;
        let s = l.apply_with(Event::Alarm, at, &rules);
        keep(&s, &mut metered);
        assert!(metered.contains(&(f + rules.unsaved_max_ms, at)), "from the bound on: {metered:?}");
        // a save that works ends the window; what comes after is metered as any
        answer(&mut l, &s, at, true, &mut metered);
        let at = if l.unmetered.is_some() {
            // its next awake try, that one working
            let due = l.save_due_ms.expect("a try is due");
            let s = l.apply_with(Event::Alarm, due, &rules);
            keep(&s, &mut metered);
            answer(&mut l, &s, due, true, &mut metered);
            due
        } else {
            at
        };
        assert_eq!((l.unmetered, l.unsaved_since_ms()), (None, None), "a save worked: the window is over");
        let end = at + 5 * 60_000;
        keep(&l.apply_with(Event::Alarm, end, &rules), &mut metered);
        // every metered interval runs forward, none overlaps another or the window
        metered.sort();
        assert!(metered.iter().all(|(a, b)| a < b) && metered.windows(2).all(|w| w[0].1 <= w[1].0), "{metered:?}");
        assert!(metered.iter().all(|(a, b)| *b <= tried || *a >= f + rules.unsaved_max_ms), "none in the window: {metered:?}");
        let total: i64 = metered.iter().map(|(a, b)| b - a).sum();
        assert_eq!(total, (metered.last().unwrap().1 - (T + 3_000)) - (f + rules.unsaved_max_ms - tried), "all but the window: {metered:?}");
    }

    /// Goal (r4, r6): a save that never reports failed at its deadline, as
    /// one that says so does, so the unsaved bound applies. A sleep's: kept awake
    /// (held no more, saying why), tried again after a pause, each try's
    /// save never answering either, until past `unsaved_max_ms` it sleeps
    /// unsaved, its life's end saying so; the time from the start of its
    /// first failing try (its ten hung minutes included) to the failure of
    /// its last, the try under way at the bound, is free (Paul, 2026-10-08),
    /// and the rest (before it, and its stop's seconds) metered once.
    /// Before, its save was asked for again at each deadline, for good: the
    /// computer held, answering no one, and its awake time owed. Invalid:
    /// before its deadline it waits. Replay: the hung save's late answers,
    /// and an alarm again, change nothing. Restart: stored mid-save and read
    /// back, it fails at the same deadline. Awake, a save that never
    /// reports fails too, counted, and told from the second in a row.
    #[test]
    fn a_save_that_never_answers_fails_at_its_deadline() {
        let rules = Rules::default();
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        let mut metered: Vec<(i64, i64)> = vec![];
        let keep = |s: &Step, metered: &mut Vec<(i64, i64)>| {
            for a in &s.actions {
                if let Action::Meter { from_ms, to_ms } = a {
                    metered.push((*from_ms, *to_ms));
                }
            }
        };
        // its idle sleep: the hold answers, the save never does
        let s = l.apply_with(Event::Alarm, T + IDLE_MS, &rules);
        keep(&s, &mut metered);
        assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }));
        let saving = T + IDLE_MS + 500;
        let (_, _, hung) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, saving, &rules));
        assert_eq!(l.next_alarm(), Some(saving + SAVE_DEADLINE_MS), "its deadline is the alarm it asks for");
        let s = l.apply_with(Event::Alarm, saving + SAVE_DEADLINE_MS - 1, &rules);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Sleeping { .. }), "before its deadline it waits: {s:?}");
        // restart: stored mid-save and read back
        let mut l: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        let first = saving + SAVE_DEADLINE_MS;
        let s = l.apply_with(Event::Alarm, first, &rules);
        keep(&s, &mut metered);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }], "failed: kept, held no more, and nothing metered since its try began (its alarm metered to it)");
        assert_eq!(l.phase, Phase::Awake { generation: g, since_ms: first }, "the same container, awake");
        assert!(matches!(&s.note, SaveNote::Says(why) if why.contains("could not save") && why.contains(&format!("save {hung} did not report within {} minutes", SAVE_DEADLINE_MS / 60_000))), "{:?}", s.note);
        let (tried, bound) = (T + IDLE_MS, first + rules.unsaved_max_ms);
        assert_eq!(l.unmetered, Some((tried, bound)), "free from its try's start, the hung save's minutes included, to the bound after its failure");
        let u = l.unsaved(&rules).expect("a failure is counted");
        assert_eq!((u.failures, u.failing.since_ms, u.sleep_failed_since_ms, u.stops_at_ms), (1, first, Some(first), Some(bound)), "the bound counts from the failure");
        // replay: the hung save's late answers, and the alarm again
        let before = l.clone();
        assert!(l.apply_with(Event::Saved { generation: g, seq: hung }, first + 100, &rules).actions.is_empty());
        assert!(l.apply_with(Event::SaveFailed { generation: g, seq: hung, why: "late".into() }, first + 200, &rules).actions.is_empty());
        assert!(l.apply_with(Event::Alarm, first, &rules).actions.is_empty());
        assert_eq!(l, before, "a late answer and an alarm again change nothing");
        // its tries, each save hung, until the bound ends them (bounded: the
        // pause doubles, and each try takes a deadline)
        let mut tries = 1;
        let mut at;
        let mut alarms = 0;
        let stopped = loop {
            alarms += 1;
            assert!(alarms < 64, "the bound ends the tries");
            at = l.next_alarm().expect("an alarm is due");
            let s = l.apply_with(Event::Alarm, at, &rules);
            keep(&s, &mut metered);
            if s.actions.contains(&Action::Sleep { generation: g }) {
                assert!(at <= bound, "no try begins past the bound: {at} > {bound}");
                save_asked(&l.apply_with(Event::Held { generation: g, held: true }, at + 500, &rules));
                continue;
            }
            if s.actions.iter().any(|a| matches!(a, Action::Stop { .. })) {
                break s;
            }
            if s.actions.contains(&Action::Unhold { generation: g }) {
                tries += 1;
                assert!(matches!(l.phase, Phase::Awake { .. }) && l.unsaved(&rules).is_some_and(|u| u.failures == tries), "try {tries} failed at its deadline");
            }
        };
        assert!(tries >= 3, "tried again within the bound: {tries}");
        assert_eq!(stopped.actions.last(), Some(&Action::Stop { generation: g, saved: false }), "past the bound it sleeps unsaved: {stopped:?}");
        assert!(at >= bound && at <= bound + HOLD_DEADLINE_MS + SAVE_DEADLINE_MS, "the try under way at the bound ends within its own deadline: {} ms past", at - bound);
        assert!(matches!(&stopped.note, SaveNote::Says(why) if why.contains("slept unsaved")), "{:?}", stopped.note);
        let s = l.apply_with(Event::Asleep { generation: g }, at + 5_000, &rules);
        keep(&s, &mut metered);
        assert_eq!(s.ended, Some(Ended { generation: g, by: LifeEnd::Sleep, saved: false, at_ms: at + 5_000, restart: false, quiet: true }), "its next wake is a rollback");
        assert!(!stopped.actions.iter().any(|a| matches!(a, Action::Meter { .. })), "the try at the bound is free past it too: {stopped:?}");
        // billing: nothing from its first try's start to its last's failure,
        // all else once: the time before, and its stop's seconds
        metered.sort();
        assert!(metered.iter().all(|(a, b)| a < b) && metered.windows(2).all(|w| w[0].1 <= w[1].0), "{metered:?}");
        assert!(metered.iter().all(|(a, b)| *b <= tried || *a >= at), "none in the window: {metered:?}");
        assert_eq!(metered.last(), Some(&(at, at + 5_000)), "its stop's seconds");
        let total: i64 = metered.iter().map(|(a, b)| b - a).sum();
        assert_eq!(total, (tried - (T + 3_000)) + 5_000, "all but the window: {metered:?}");

        // awake: a save that never reports fails at its deadline, tried again
        // after a pause, and its owner is told from the second in a row
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 4_000);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
        l.apply(Event::Closed { socket: Socket::Keepalive }, T + 10_000);
        let mut at = T + 10_000 + SAVE_SETTLE_MS;
        for n in 1..=UNSAVED_NOTICE_FAILURES {
            let s = l.apply(Event::Alarm, at);
            assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }), "save {n}, awake: {s:?}");
            let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
            let due = at + 500 + SAVE_DEADLINE_MS;
            let s = l.apply(Event::Alarm, due);
            assert_eq!(s.actions.last(), Some(&Action::Unhold { generation: g }), "save {n} failed: {s:?}");
            assert!(matches!(l.phase, Phase::Awake { .. }) && l.saving().is_none());
            let u = l.unsaved(&Rules::default()).expect("counted");
            assert_eq!((u.failures, u.sleep_failed_since_ms, u.stops_at_ms), (n, None, None), "an awake save opens no window");
            assert!(u.failing.why.contains(&format!("save {seq} did not report")), "{:?}", u.failing);
            assert!(l.apply(Event::Saved { generation: g, seq }, due + 1).actions.is_empty(), "its late answer changes nothing");
            at = l.save_due_ms.expect("its retry is due");
            assert_eq!(at, due + (SAVE_RETRY_MS << (n - 1)), "after a growing pause");
        }
    }

    /// Goal (Paul, 2026-10-08, the whole failing try): the time a computer
    /// is kept for a save the platform failed is free from the start of the
    /// try that failed, not from its failure: a hung first save's minutes
    /// are free. A quick failure is as it was, seconds earlier; the bound,
    /// its stop time and what its owner is told count from the failure
    /// still. Metering resumes when a save works, the working try's own
    /// time free; a restart of the DO mid-window meters the same, and its
    /// owner's restart mid-window ends the window at its own save's end.
    /// The try under way at the bound is free past it to its failure; one
    /// begun past the bound (a record held it there) is metered as any.
    /// Invalid: a window never covers time metered already. Method: each
    /// case's meters, against the times each try began and ended.
    #[test]
    fn the_whole_failing_try_is_free() {
        let rules = Rules { unsaved_max_ms: 10 * 60_000 };
        let meters = |s: &Step| s.actions.iter().filter_map(|a| if let Action::Meter { from_ms, to_ms } = a { Some((*from_ms, *to_ms)) } else { None }).collect::<Vec<_>>();
        // a computer awake since T + 3 s, its owner's sleep at `slept`: its
        // hold answers, and its save is `seq`
        let sleeping = |slept: i64| -> (Lifecycle, u64, u64) {
            let mut l = Lifecycle::new();
            let g = started(&mut l, T);
            let s = l.apply_with(Event::Sleep, slept, &rules);
            assert_eq!(s.actions, vec![Action::Sleep { generation: g }]);
            let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, slept + 500, &rules));
            (l, g, seq)
        };
        let slept = T + 60_000;

        // a hung first save: free from its try's start, its deadline before
        // it failed; the bound and its stop time from the failure
        let (mut l, _, _) = sleeping(slept);
        let failed = slept + 500 + SAVE_DEADLINE_MS;
        let s = l.apply_with(Event::Alarm, failed, &rules);
        assert_eq!(meters(&s), vec![(T + 3_000, slept)], "metered to its try's start only: {s:?}");
        assert_eq!((l.unmetered, l.unsaved_since_ms()), (Some((slept, failed + rules.unsaved_max_ms)), Some(failed)));
        assert_eq!(l.unsaved(&rules).and_then(|u| u.stops_at_ms), Some(failed + rules.unsaved_max_ms), "its owner is told the bound from the failure");

        // a quick failure: the same, seconds earlier
        let (mut l, g2, seq) = sleeping(slept);
        let s = l.apply_with(Event::SaveFailed { generation: g2, seq, why: "R2 is down".into() }, slept + 1_000, &rules);
        assert_eq!(meters(&s), vec![(T + 3_000, slept)]);
        assert_eq!((l.unmetered, l.unsaved_since_ms()), (Some((slept, slept + 1_000 + rules.unsaved_max_ms)), Some(slept + 1_000)));
        assert_eq!(l.unsaved(&rules).and_then(|u| u.stops_at_ms), Some(slept + 1_000 + rules.unsaved_max_ms));

        // recovery: its retry's save works, its own time free; metering
        // resumes from there, its stop's seconds the first metered
        let mut r = l.clone();
        let due = r.save_due_ms.expect("its retry is due");
        assert_eq!(r.apply_with(Event::Alarm, due, &rules).actions.last(), Some(&Action::Sleep { generation: g2 }));
        let (_, _, seq) = save_asked(&r.apply_with(Event::Held { generation: g2, held: true }, due + 500, &rules));
        let s = r.apply_with(Event::Saved { generation: g2, seq }, due + 2_000, &rules);
        assert_eq!((meters(&s), r.unmetered), (vec![], None), "the try that worked is free, and the window is over");
        let s = r.apply_with(Event::Asleep { generation: g2 }, due + 7_000, &rules);
        assert_eq!(meters(&s), vec![(due + 2_000, due + 7_000)], "metered again from the save that worked");

        // a restart of the DO mid-window: stored and read back, the same
        let mut back: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        // (an alarm before its retry is due: it meters, and tries nothing)
        let at = slept + 30_000;
        assert!(at < l.save_due_ms.expect("its retry is due"));
        assert_eq!((meters(&back.apply_with(Event::Alarm, at, &rules)), meters(&l.apply_with(Event::Alarm, at, &rules))), (vec![], vec![]), "nothing in the window, either way");
        assert_eq!(back, l);

        // its owner's restart mid-window, its save failing too: free to that
        // failure, then metered as any (its stop's seconds)
        let s = l.apply_with(Event::Restart { generation: g2 }, at + 1_000, &rules);
        assert_eq!(s.actions, vec![Action::Sleep { generation: g2 }]);
        let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g2, held: true }, at + 1_500, &rules));
        let s = l.apply_with(Event::SaveFailed { generation: g2, seq, why: "R2 is down".into() }, at + 3_000, &rules);
        assert_eq!((meters(&s), l.unmetered), (vec![], None), "free to the restart's failure, and the window over: {s:?}");
        let s = l.apply_with(Event::Asleep { generation: g2 }, at + 8_000, &rules);
        assert_eq!(meters(&s), vec![(at + 3_000, at + 8_000)]);

        // the try under way at the bound: free past it, to its failure
        let (mut l, g, seq) = sleeping(slept);
        let f = slept + 1_000;
        l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, f, &rules);
        let bound = f + rules.unsaved_max_ms;
        let mut alarms = 0;
        let tried = loop {
            alarms += 1;
            assert!(alarms < 64, "the bound ends the tries");
            let at = l.next_alarm().expect("an alarm is due");
            let s = l.apply_with(Event::Alarm, at, &rules);
            if !s.actions.contains(&Action::Sleep { generation: g }) {
                continue;
            }
            let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, at + 500, &rules));
            if at >= bound {
                break at;
            }
            l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, at + 1_000, &rules);
        };
        assert_eq!(tried, bound, "its last try, at the bound itself");
        let last = bound + SAVE_DEADLINE_MS + 500;
        let s = l.apply_with(Event::Alarm, last, &rules);
        assert_eq!((meters(&s), s.actions.last()), (vec![], Some(&Action::Stop { generation: g, saved: false })), "its hung last try is free past the bound: {s:?}");
        assert_eq!(meters(&l.apply_with(Event::Asleep { generation: g }, last + 5_000, &rules)), vec![(last, last + 5_000)], "its stop's seconds are its owner's");

        // one begun past the bound (a record held it there): metered as any
        let (mut l, g, seq) = sleeping(slept);
        let f = slept + 1_000;
        l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, f, &rules);
        let bound = f + rules.unsaved_max_ms;
        l.apply_with(Event::Wake { why: Wake::Record }, bound - 60_000, &rules);
        let held = bound - 60_000 + IDLE_MS;
        let s = l.apply_with(Event::Alarm, held, &rules);
        assert!(s.actions.contains(&Action::Sleep { generation: g }) && meters(&s) == vec![(bound, held)], "held past the bound, metered from it: {s:?}");
        let (_, _, seq) = save_asked(&l.apply_with(Event::Held { generation: g, held: true }, held + 500, &rules));
        let s = l.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, held + 1_000, &rules);
        assert_eq!(meters(&s), vec![(held, held + 1_000)], "a try begun past the bound is metered: {s:?}");

        // invalid: a window opens no earlier than what was metered (a
        // lifecycle stored before its tries were timed reads 0)
        let (l, g, seq) = sleeping(slept);
        let mut v = serde_json::to_value(&l).unwrap();
        v.as_object_mut().unwrap().remove("save_asked_ms");
        let mut old: Lifecycle = serde_json::from_value(v).unwrap();
        old.metered_to_ms = slept;
        let s = old.apply_with(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, slept + 1_000, &rules);
        assert_eq!((meters(&s), old.unmetered.map(|(from, _)| from)), (vec![], Some(slept)), "from what was metered to, never before it");
    }

    /// Goal (the warning): its owner is told while there is still time.
    /// Awake saves that fail are told once `UNSAVED_NOTICE_FAILURES` failed
    /// in a row; a sleep's at once, with when the bound stops it: the bound
    /// itself when nothing holds it, later while a record holds it, never
    /// while a socket is open (in use) or for an always-on computer. A save
    /// that works says no more. Method: `notices` read after each step.
    #[test]
    fn its_owner_is_told_while_saves_fail_and_when_it_stops() {
        let rules = Rules::default();
        let unsaved = |c: &Computer| notices(&c.life, &c.saves, &c.images, &rules).into_iter().find(|n| matches!(n, ComputerNotice::Unsaved { .. }));
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let r = c.wake();
        let g = r.generation;
        assert!(notices(&c.life, &c.saves, &c.images, &rules).is_empty(), "nothing to tell");
        // a turn, and its end's save fails: once is a blip
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.apply(Event::Closed { socket: Socket::Keepalive });
        c.now += SAVE_SETTLE_MS;
        assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        c.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() });
        assert_eq!(unsaved(&c), None, "one awake failure is not told");
        let first = c.now;
        // tried again a minute later: twice is told, since this life began
        c.now = c.life.save_due_ms.expect("tried again");
        assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        c.apply(Event::SaveFailed { generation: g, seq, why: "R2 is still down".into() });
        assert_eq!(
            unsaved(&c),
            Some(ComputerNotice::Unsaved { since: r.at, save: Some(1), failing_since: first, failures: 2, why: "R2 is still down".into(), stops_at: None }),
            "its work since it came up, back to save 1 if it stops; no sleep failed, so no stop is due"
        );
        // then a sleep's fails: the bound runs, and the stop is the bound
        c.now = c.life.held_until_ms().max(c.life.save_due_ms.unwrap_or(0));
        assert!(c.apply(Event::Alarm).actions.contains(&Action::Sleep { generation: g }));
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        c.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() });
        let since = c.life.unsaved_since_ms().expect("a sleep's save failed");
        let bound = since + rules.unsaved_max_ms;
        assert!(matches!(unsaved(&c), Some(ComputerNotice::Unsaved { failures: 3, stops_at: Some(at), .. }) if at == bound), "{:?}", unsaved(&c));
        // a record holds it past the bound: it stops when that hold runs out
        c.now = since + rules.unsaved_max_ms - 60_000;
        c.apply(Event::Wake { why: Wake::Record });
        assert!(matches!(unsaved(&c), Some(ComputerNotice::Unsaved { stops_at: Some(at), .. }) if at == c.now + IDLE_MS), "{:?}", unsaved(&c));
        // a tab open (in use): no stop is due while it is
        c.apply(Event::Opened { socket: Socket::Tab });
        assert!(matches!(unsaved(&c), Some(ComputerNotice::Unsaved { stops_at: None, .. })), "never while in use");
        c.apply(Event::Closed { socket: Socket::Tab });
        // always on, it never sleeps, so never stops
        c.apply(Event::AlwaysOn { on: true });
        assert!(matches!(unsaved(&c), Some(ComputerNotice::Unsaved { stops_at: None, .. })));
        c.apply(Event::AlwaysOn { on: false });
        // a save that works: nothing more to tell
        c.now = c.life.save_due_ms.expect("its next try");
        c.apply(Event::Alarm);
        let s = c.apply(Event::Alarm);
        let asked = s.actions.iter().any(|a| matches!(a, Action::Hold { .. } | Action::Sleep { .. }));
        assert!(asked || c.life.saving().is_some(), "a try is under way: {s:?}");
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        c.saves.saved(g, rec("b2"), c.now, c.now, true);
        c.apply(Event::Saved { generation: g, seq });
        assert_eq!(unsaved(&c), None, "a save worked");
        assert!(c.life.unsaved(&rules).is_none() && c.life.failing.is_none() && c.life.save_failures == 0);
    }

    /// Goal: why its saves fail is kept within `FAILED_WHY_MAX_BYTES`, cut
    /// at a character: whole up to its bound, and past it, what fits before
    /// the character that crosses it, of each width. Method: a failed save's
    /// why, as the lifecycle keeps it, against the longest prefix that fits.
    #[test]
    fn why_a_save_failed_is_cut_at_a_character() {
        for c in ['x', 'é', '€', '𝄞'] {
            for pad in FAILED_WHY_MAX_BYTES - 4..=FAILED_WHY_MAX_BYTES {
                let why = format!("{}{c}y", "x".repeat(pad));
                let fits = why.char_indices().map(|(i, ch)| i + ch.len_utf8()).take_while(|end| *end <= FAILED_WHY_MAX_BYTES).last().unwrap_or(0);
                let mut l = Lifecycle::new();
                l.save_failed(&why, T);
                assert_eq!(l.failing.map(|f| f.why), Some(why[..fits].to_string()), "{c:?} after {pad} bytes");
            }
        }
    }
}
