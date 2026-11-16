//! The previous release's flags, written beside the phases that now carry
//! them (FIN-395 C). Debt: docs/technical-debt-ledger.md, "A computer's
//! lifecycle writes its old flags too".

use super::{Lifecycle, Phase, Retry};

/// The previous release's fields for what the phases now carry, written
/// beside them for one release so a rollback reads the same computer, and
/// read to fill a phase that release stored (`Lifecycle::read`). Debt:
/// docs/technical-debt-ledger.md, "A computer's lifecycle writes its old
/// flags too".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Legacy {
    wake_after_sleep: bool,
    retry_at_ms: Option<i64>,
    failed_ms: i64,
    owner_sleep: bool,
    restarting: bool,
}

impl Legacy {
    /// What the previous release keeps for `phase`.
    pub(super) fn of(phase: &Phase) -> Legacy {
        match *phase {
            Phase::Asleep { retry } => Legacy { wake_after_sleep: retry.is_some_and(|r| r.owed), retry_at_ms: retry.map(|r| r.at_ms), ..Legacy::default() },
            Phase::Sleeping { owner, restarting, owes_start, .. } => Legacy { wake_after_sleep: owes_start, owner_sleep: owner, restarting, ..Legacy::default() },
            Phase::Failed { since_ms, .. } => Legacy { failed_ms: since_ms, ..Legacy::default() },
            Phase::Starting { .. } | Phase::Awake { .. } => Legacy::default(),
        }
    }

    /// `phase` with what the previous release kept for it: the same phase
    /// when this release stored it (its fields and these agree).
    fn fill(self, phase: Phase) -> Phase {
        match phase {
            Phase::Asleep { retry } => Phase::Asleep { retry: retry.or(self.retry_at_ms.map(|at_ms| Retry { at_ms, owed: self.wake_after_sleep })) },
            Phase::Sleeping { generation, since_ms, owner, restarting, owes_start } => Phase::Sleeping {
                generation,
                since_ms,
                owner: owner || self.owner_sleep,
                restarting: restarting || self.restarting,
                owes_start: owes_start || self.wake_after_sleep,
            },
            Phase::Failed { why, since_ms } => Phase::Failed { why, since_ms: since_ms.max(self.failed_ms) },
            phase => phase,
        }
    }
}

impl Lifecycle {
    /// A stored lifecycle, this release's or the previous one's (which
    /// kept what the phases now carry beside them: `Legacy`): its phase
    /// filled from those fields, so a computer stored before this release
    /// comes up as it was.
    pub fn read(text: &str) -> serde_json::Result<Lifecycle> {
        // its bytes, never `from_str`: the cell decodes JSON text through
        // one decoder per type (`crate::json_text`, cell/clippy.toml)
        let mut life: Lifecycle = serde_json::from_slice(text.as_bytes())?;
        life.phase = life.legacy().fill(life.phase.clone());
        life.set_legacy();
        Ok(life)
    }

    /// The previous release's fields, as stored.
    pub(super) fn legacy(&self) -> Legacy {
        Legacy { wake_after_sleep: self.wake_after_sleep, retry_at_ms: self.retry_at_ms, failed_ms: self.failed_ms, owner_sleep: self.owner_sleep, restarting: self.restarting }
    }

    /// The previous release's fields, written from the phase.
    pub(super) fn set_legacy(&mut self) {
        let Legacy { wake_after_sleep, retry_at_ms, failed_ms, owner_sleep, restarting } = Legacy::of(&self.phase);
        (self.wake_after_sleep, self.retry_at_ms, self.failed_ms, self.owner_sleep, self.restarting) = (wake_after_sleep, retry_at_ms, failed_ms, owner_sleep, restarting);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::computer::testing::{owed, save_asked, started, T};
    use crate::computer::*;

    // ---- FIN-395 C: the machine's edges, kept across releases ----

    /// The previous release's stored lifecycles (master at abe084109, its
    /// own code: a start, its owner's restart as it saves, the start that
    /// restart owed, that start failing on one R2 502, and the computer
    /// that then won't wake), as this release finds them in a computer's
    /// storage.
    const PREV: [(&str, &str); 5] = [
        ("starting", r#"{"phase":{"phase":"starting","generation":1,"since_ms":1800000000000},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001200000,"wake_after_sleep":false,"failures":0,"retry_at_ms":null,"failed_ms":0,"always_on":false,"metered_to_ms":0,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":0,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":0}"#),
        ("restart_sleeping", r#"{"phase":{"phase":"sleeping","generation":1,"since_ms":1800000010000},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":true,"failures":0,"retry_at_ms":null,"failed_ms":0,"always_on":false,"metered_to_ms":1800000003000,"ready":true,"fell_back":false,"saving":{"step":"hold","since_ms":1800000010000},"owner_sleep":true,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":true,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000003000}"#),
        ("asleep_owed", r#"{"phase":{"phase":"asleep"},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":true,"failures":0,"retry_at_ms":1800000015000,"failed_ms":0,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
        ("asleep_retry", r#"{"phase":{"phase":"asleep"},"generation":2,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":false,"failures":1,"retry_at_ms":1800000021000,"failed_ms":0,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
        ("failed", r#"{"phase":{"phase":"failed","why":"restore '/data': gateway rejected the range download with HTTP 502"},"generation":4,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":false,"failures":3,"retry_at_ms":null,"failed_ms":1800000033000,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
    ];

    fn prev(name: &str) -> Lifecycle {
        let (_, text) = PREV.iter().find(|(n, _)| *n == name).expect("a stored lifecycle of the previous release");
        Lifecycle::read(text).expect("this release reads the previous one's")
    }

    /// `v` with only the keys `like` has (in its phase too): what the
    /// previous release reads of a lifecycle this one stored, as it skips
    /// the fields it does not know.
    fn as_prev_reads(v: &Value, like: &Value) -> Value {
        match (v, like) {
            (Value::Object(v), Value::Object(like)) => Value::Object(like.iter().map(|(k, l)| (k.clone(), as_prev_reads(&v[k], l))).collect()),
            (v, _) => v.clone(),
        }
    }

    /// Goal (the two-step cut: docs/technical-debt-ledger.md): what each
    /// phase now carries was stored beside it by the previous release, and
    /// this release reads it into the phase; and what this release stores
    /// reads back, in the previous one (a rollback), as it stored it: every
    /// key it knows, the same. Method: the previous release's own stored
    /// lifecycles, read here, then written, then cut to its keys.
    #[test]
    fn a_lifecycle_stored_by_the_previous_release_reads_into_its_phases_and_back() {
        assert_eq!(prev("starting").phase, Phase::Starting { generation: 1, since_ms: T, destroying: false });
        assert_eq!(prev("restart_sleeping").phase, Phase::Sleeping { generation: 1, since_ms: T + 10_000, owner: true, restarting: true, owes_start: true });
        assert_eq!(prev("asleep_owed").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 15_000, owed: true }) });
        assert_eq!(prev("asleep_retry").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 21_000, owed: false }) });
        assert_eq!(prev("asleep_retry").failures, 1, "a count of the wake, kept beside the phases");
        assert!(matches!(prev("failed").phase, Phase::Failed { since_ms, .. } if since_ms == T + 33_000));
        for (name, text) in PREV {
            let theirs: Value = serde_json::from_str(text).unwrap();
            let life = Lifecycle::read(text).unwrap();
            let ours = serde_json::to_value(&life).unwrap();
            assert_eq!(as_prev_reads(&ours, &theirs), theirs, "{name}: the previous release reads back what it stored");
            // and again: reading what this release stored changes nothing
            assert_eq!(Lifecycle::read(&ours.to_string()).unwrap(), life, "{name}");
        }
    }

    /// Goal: a computer the previous release stored goes on as it would
    /// have, in each phase that carried its flags: its owner's restart as
    /// it saves (a restart's end, its start owed), the start that restart
    /// owed (made though nothing holds it now), a failed start's retry
    /// (made only while wanted), a computer that won't wake (its half hour
    /// counted from when it gave up), and a start. Method: the simulation's
    /// own steps, from each stored lifecycle.
    #[test]
    fn a_computer_stored_by_the_previous_release_goes_on_as_it_would_have() {
        // its owner's restart, as it holds: saved, stopped, its start owed
        let mut l = prev("restart_sleeping");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: 1, held: true }, T + 10_500));
        assert_eq!(l.apply(Event::Saved { generation: 1, seq }, T + 12_000).actions, vec![Action::Stop { generation: 1, saved: true }]);
        let s = l.apply(Event::Asleep { generation: 1 }, T + 15_000);
        assert!(s.ended.is_some_and(|e| e.restart), "a restart's end: {s:?}");
        assert_eq!(owed(&mut l, &s, T + 15_000).actions, vec![Action::Start { generation: 2 }]);
        // the start that restart owed, long after anything held it: made
        let mut l = prev("asleep_owed");
        let late = T + 2 * IDLE_MS;
        assert!(l.start_due(late) && l.starting(late));
        assert_eq!(l.apply(Event::Alarm, late).actions, vec![Action::Start { generation: 2 }]);
        // a failed start's retry: while held, made; once nothing wants it, let go
        let mut held = prev("asleep_retry");
        assert_eq!(held.apply(Event::Alarm, T + 21_000).actions, vec![Action::Start { generation: 3 }]);
        assert_eq!(held.failures, 1, "the strike it had");
        let mut idle = prev("asleep_retry");
        assert!(idle.apply(Event::Alarm, late).actions.is_empty());
        assert_eq!(idle.phase, Phase::ASLEEP);
        // won't wake: a record within its half hour is refused, after it tries once
        let mut l = prev("failed");
        assert!(l.apply(Event::Wake { why: Wake::Record }, T + 33_000 + WONT_WAKE_RETRY_MS - 1).refused.is_some());
        assert_eq!(l.apply(Event::Wake { why: Wake::Record }, T + 33_000 + WONT_WAKE_RETRY_MS).actions, vec![Action::Start { generation: 5 }]);
        assert_eq!(l.failures, STARTS_FAILED_MAX - 1, "its last strike");
        // a start: it fails, and its own destroy's exit is no crash
        let mut l = prev("starting");
        l.apply(Event::StartFailing { generation: 1 }, T + 2_000);
        assert!(l.apply(Event::Exited { generation: 1 }, T + 2_100).actions.is_empty());
        assert_eq!(l.failures, 0, "no strike for its own destroy");
        let s = l.apply(Event::RestoreFailed { generation: 1, why: "an archive gone".into(), older: true }, T + 2_200);
        assert_eq!((l.failures, s.alarm_ms), (0, Some(T + 2_200)), "a fallback to an older save: no strike, at once");
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
                        l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap();
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
            let mut l = prev("failed");
            let gave_up = T + 33_000;
            let steps = [
                (gave_up + 60_000, Event::Wake { why: Wake::Record }, None),
                (gave_up + WONT_WAKE_RETRY_MS, Event::Wake { why: Wake::Record }, Some(5)),
                (gave_up + WONT_WAKE_RETRY_MS + 1_000, Event::StartFailing { generation: 5 }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 1_100, Event::Exited { generation: 5 }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 1_200, Event::StartFailed { generation: 5, why: r502() }, None),
                (gave_up + WONT_WAKE_RETRY_MS + 60_000, Event::Wake { why: Wake::Record }, None),
                (gave_up + 2 * WONT_WAKE_RETRY_MS + 1_200, Event::Wake { why: Wake::Presence }, Some(6)),
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
                    l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap();
                }
            }
            assert_eq!(l.failures, 0, "its owner's restart starts the count again");
        }
    }
}
