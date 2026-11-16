//! The flags a release before FIN-395 C kept beside a computer's phase,
//! read (never written) to fill a phase that release stored and nothing
//! has rewritten since. Debt: docs/technical-debt-ledger.md, "A computer's
//! lifecycle reads its old flags".

use serde::Deserialize;

use super::{Phase, Retry};

/// What a release before FIN-395 C kept beside the phase, by its names.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Old {
    wake_after_sleep: bool,
    retry_at_ms: Option<i64>,
    failed_ms: i64,
    owner_sleep: bool,
    restarting: bool,
}

/// The phase stored in `text` (`phase`, as read) filled from the flags a
/// release before FIN-395 C kept beside it; `None` when they add nothing
/// (none kept, or a later release's, which agree with its phase).
pub(super) fn filled(text: &str, phase: &Phase) -> serde_json::Result<Option<Phase>> {
    let old: Old = serde_json::from_slice(text.as_bytes())?;
    let filled = match phase.clone() {
        Phase::Asleep { retry } => Phase::Asleep { retry: retry.or(old.retry_at_ms.map(|at_ms| Retry { at_ms, owed: old.wake_after_sleep })) },
        Phase::Sleeping { generation, since_ms, owner, restarting, owes_start } => Phase::Sleeping {
            generation,
            since_ms,
            owner: owner || old.owner_sleep,
            restarting: restarting || old.restarting,
            owes_start: owes_start || old.wake_after_sleep,
        },
        Phase::Failed { why, since_ms } => Phase::Failed { why, since_ms: since_ms.max(old.failed_ms) },
        phase => phase,
    };
    Ok((filled != *phase).then_some(filled))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::computer::lifecycle::Lifecycle;
    use crate::computer::testing::{owed, save_asked, T};
    use crate::computer::*;

    /// Lifecycles stored before FIN-395 C (master at abe084109, its own
    /// code: a start, its owner's restart as it saves, the start that restart
    /// owed, that start failing on one R2 502, and the computer that then
    /// won't wake), as this release may still find them in a computer's
    /// storage.
    const OLD: [(&str, &str); 5] = [
        ("starting", r#"{"phase":{"phase":"starting","generation":1,"since_ms":1800000000000},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001200000,"wake_after_sleep":false,"failures":0,"retry_at_ms":null,"failed_ms":0,"always_on":false,"metered_to_ms":0,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":0,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":0}"#),
        ("restart_sleeping", r#"{"phase":{"phase":"sleeping","generation":1,"since_ms":1800000010000},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":true,"failures":0,"retry_at_ms":null,"failed_ms":0,"always_on":false,"metered_to_ms":1800000003000,"ready":true,"fell_back":false,"saving":{"step":"hold","since_ms":1800000010000},"owner_sleep":true,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":true,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000003000}"#),
        ("asleep_owed", r#"{"phase":{"phase":"asleep"},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":true,"failures":0,"retry_at_ms":1800000015000,"failed_ms":0,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
        ("asleep_retry", r#"{"phase":{"phase":"asleep"},"generation":2,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":false,"failures":1,"retry_at_ms":1800000021000,"failed_ms":0,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
        ("failed", r#"{"phase":{"phase":"failed","why":"restore '/data': gateway rejected the range download with HTTP 502"},"generation":4,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":false,"failures":3,"retry_at_ms":null,"failed_ms":1800000033000,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"owner_sleep":false,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":false,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#),
    ];

    fn old(name: &str) -> Lifecycle {
        let (_, text) = OLD.iter().find(|(n, _)| *n == name).expect("a lifecycle stored before FIN-395 C");
        Lifecycle::read(text).expect("this release reads it").0
    }

    /// Goal: a lifecycle stored before FIN-395 C reads into its phases,
    /// filled from the flags kept beside them, and says it was (the cell
    /// logs it) where they said anything; written back, it has no flags,
    /// and reads the same, filling nothing. Method: that release's own
    /// stored lifecycles, read here, then written and read again.
    #[test]
    fn a_lifecycle_stored_before_fin_395_c_reads_into_its_phases() {
        assert_eq!(old("starting").phase, Phase::Starting { generation: 1, since_ms: T, destroying: false });
        assert_eq!(old("restart_sleeping").phase, Phase::Sleeping { generation: 1, since_ms: T + 10_000, owner: true, restarting: true, owes_start: true });
        assert_eq!(old("asleep_owed").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 15_000, owed: true }) });
        assert_eq!(old("asleep_retry").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 21_000, owed: false }) });
        assert_eq!(old("asleep_retry").failures, 1, "a count of the wake, kept beside the phases");
        assert!(matches!(old("failed").phase, Phase::Failed { since_ms, .. } if since_ms == T + 33_000));
        for (name, text) in OLD {
            let (life, filled) = Lifecycle::read(text).unwrap();
            assert_eq!(filled, name != "starting", "{name}: filled where its flags said anything");
            let ours = serde_json::to_value(&life).unwrap();
            assert!(["wake_after_sleep", "retry_at_ms", "failed_ms", "owner_sleep", "restarting"].iter().all(|k| ours.get(k).is_none()), "{name}: no flags written: {ours}");
            assert_eq!(Lifecycle::read(&ours.to_string()).unwrap(), (life, false), "{name}: read back the same, nothing filled");
        }
    }

    /// Goal: a computer stored before FIN-395 C goes on as it would have,
    /// in each phase that carried its flags: its owner's restart as
    /// it saves (a restart's end, its start owed), the start that restart
    /// owed (made though nothing holds it now), a failed start's retry
    /// (made only while wanted), a computer that won't wake (its half hour
    /// counted from when it gave up), and a start. Method: the simulation's
    /// own steps, from each stored lifecycle.
    #[test]
    fn a_computer_stored_before_fin_395_c_goes_on_as_it_would_have() {
        // its owner's restart, as it holds: saved, stopped, its start owed
        let mut l = old("restart_sleeping");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: 1, held: true }, T + 10_500));
        assert_eq!(l.apply(Event::Saved { generation: 1, seq }, T + 12_000).actions, vec![Action::Stop { generation: 1, saved: true }]);
        let s = l.apply(Event::Asleep { generation: 1 }, T + 15_000);
        assert!(s.ended.is_some_and(|e| e.restart), "a restart's end: {s:?}");
        assert_eq!(owed(&mut l, &s, T + 15_000).actions, vec![Action::Start { generation: 2 }]);
        // the start that restart owed, long after anything held it: made
        let mut l = old("asleep_owed");
        let late = T + 2 * IDLE_MS;
        assert!(l.start_due(late) && l.starting(late));
        assert_eq!(l.apply(Event::Alarm, late).actions, vec![Action::Start { generation: 2 }]);
        // a failed start's retry: while held, made; once nothing wants it, let go
        let mut held = old("asleep_retry");
        assert_eq!(held.apply(Event::Alarm, T + 21_000).actions, vec![Action::Start { generation: 3 }]);
        assert_eq!(held.failures, 1, "the strike it had");
        let mut idle = old("asleep_retry");
        assert!(idle.apply(Event::Alarm, late).actions.is_empty());
        assert_eq!(idle.phase, Phase::ASLEEP);
        // won't wake: a record within its half hour is refused, after it tries once
        let mut l = old("failed");
        assert!(l.apply(Event::Wake { why: Wake::Record }, T + 33_000 + WONT_WAKE_RETRY_MS - 1).refused.is_some());
        assert_eq!(l.apply(Event::Wake { why: Wake::Record }, T + 33_000 + WONT_WAKE_RETRY_MS).actions, vec![Action::Start { generation: 5 }]);
        assert_eq!(l.failures, STARTS_FAILED_MAX - 1, "its last strike");
        // a start: it fails, and its own destroy's exit is no crash
        let mut l = old("starting");
        l.apply(Event::StartFailing { generation: 1 }, T + 2_000);
        assert!(l.apply(Event::Exited { generation: 1 }, T + 2_100).actions.is_empty());
        assert_eq!(l.failures, 0, "no strike for its own destroy");
        let s = l.apply(Event::RestoreFailed { generation: 1, why: "an archive gone".into(), older: true }, T + 2_200);
        assert_eq!((l.failures, s.alarm_ms), (0, Some(T + 2_200)), "a fallback to an older save: no strike, at once");
    }

    /// Goal (#1284's review): a sleep stored before FIN-395 C that outlived
    /// its alarms keeps what its stop owes: before it, a takeover that
    /// failed used up its alarm without a write or another alarm, so such a
    /// row can wait for days with its stop under way. Its owner's restart
    /// ends as one and starts again; a wake heard as it went starts it
    /// again, though nothing holds it now. Method: that release's rows,
    /// their stop under way, and the container's end heard days later.
    #[test]
    fn a_sleep_stored_before_fin_395_c_keeps_its_restart_and_its_owed_start() {
        let restart = r#"{"phase":{"phase":"sleeping","generation":1,"since_ms":1800000010000},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"wake_after_sleep":true,"failures":0,"retry_at_ms":null,"failed_ms":0,"always_on":false,"metered_to_ms":1800000003000,"ready":true,"fell_back":false,"saving":{"step":"stop","since_ms":1800000012000,"saved":true},"owner_sleep":true,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"restarting":true,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000}"#;
        let woken: Value = {
            let mut v: Value = serde_json::from_str(restart).unwrap();
            v["owner_sleep"] = false.into();
            v["restarting"] = false.into();
            v
        };
        let late = T + 3 * 24 * 3_600_000;
        for (text, its_restart) in [(restart.to_string(), true), (woken.to_string(), false)] {
            let (mut l, filled) = Lifecycle::read(&text).unwrap();
            assert!(filled, "{text}");
            let s = l.apply(Event::Asleep { generation: 1 }, late);
            assert!(s.ended.is_some_and(|e| e.restart == its_restart && e.saved), "its end (restart {its_restart}): {s:?}");
            assert_eq!(owed(&mut l, &s, late).actions, vec![Action::Start { generation: 2 }], "its owed start (restart {its_restart})");
        }
    }
}
