//! The lifecycle's own steps: a wake, what keeps it awake (an open socket, a
//! hold, always on), its sleep, and a stored lifecycle read back.

use serde_json::Value;

use super::*;
use crate::computer::testing::{owed, save_asked, started, T};
use crate::computer::*;

/// Goal: a record wakes a sleeping computer, which then sleeps twenty
/// minutes after nothing holds it, metering exactly its awake time.
#[test]
fn a_record_wakes_it_and_idleness_puts_it_to_sleep() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    assert!(matches!(l.phase, Phase::Awake { .. }));
    let s = l.apply(Event::Alarm, T + 3_000 + METER_EVERY_MS);
    assert_eq!(s.actions, vec![Action::Meter { from_ms: T + 3_000, to_ms: T + 3_000 + METER_EVERY_MS }]);
    let s = l.apply(Event::Alarm, T + IDLE_MS);
    assert_eq!(s.actions, vec![Action::Meter { from_ms: T + 3_000 + METER_EVERY_MS, to_ms: T + IDLE_MS }, Action::Sleep { generation: g }]);
    let s = l.apply(Event::Asleep { generation: g }, T + IDLE_MS + 6_000);
    assert_eq!(s.actions, vec![Action::Meter { from_ms: T + IDLE_MS, to_ms: T + IDLE_MS + 6_000 }]);
    assert_eq!((l.phase.clone(), s.alarm_ms), (Phase::ASLEEP, None));
}

/// Goal: an open keepalive or tab keeps it awake past any hold, and
/// closing it holds it twenty minutes more.
#[test]
fn open_sockets_keep_it_awake() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
    let s = l.apply(Event::Alarm, T + 2 * IDLE_MS);
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })), "{s:?}");
    l.apply(Event::Closed { socket: Socket::Keepalive }, T + 2 * IDLE_MS);
    let s = l.apply(Event::Alarm, T + 3 * IDLE_MS - 1);
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })));
    let s = l.apply(Event::Alarm, T + 3 * IDLE_MS);
    assert!(s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })));
    // a tab opened on a sleeping computer starts it
    let mut l = Lifecycle::new();
    let s = l.apply(Event::Opened { socket: Socket::Tab }, T);
    assert!(matches!(s.actions[..], [Action::Start { generation: 1 }]));
    // a close with nothing open never underflows
    l.apply(Event::Closed { socket: Socket::Keepalive }, T);
    assert!(l.open(), "the tab is still open");
}

/// Goal: a pre-wake that nothing follows stops after a minute, and one
/// a record follows holds the full twenty.
#[test]
fn a_prewake_stops_after_a_minute_unless_something_arrives() {
    let mut l = Lifecycle::new();
    let s = l.apply(Event::Wake { why: Wake::Presence }, T);
    let Action::Start { generation } = s.actions[0] else { panic!() };
    l.apply(Event::Ready { generation }, T + 2_000);
    let s = l.apply(Event::Alarm, T + PREWAKE_MS);
    assert!(s.actions.contains(&Action::Sleep { generation }), "{s:?}");
    let mut l = Lifecycle::new();
    l.apply(Event::Wake { why: Wake::Presence }, T);
    l.apply(Event::Ready { generation: 1 }, T + 2_000);
    l.apply(Event::Wake { why: Wake::Record }, T + 3_000);
    let s = l.apply(Event::Alarm, T + PREWAKE_MS);
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Sleep { .. })));
    assert_eq!(l.held_until_ms(), T + 3_000 + IDLE_MS);
}

/// Goal: an agent added to a fragment wakes its sleeping computer and
/// holds it as a record does; awake, it only holds it longer; a
/// computer that won't wake is not started by it (only its owner lifts
/// that).
#[test]
fn an_agent_joining_a_fragment_wakes_it() {
    let mut l = Lifecycle::new();
    let s = l.apply(Event::Wake { why: Wake::Joined }, T);
    assert_eq!(s.actions, vec![Action::Start { generation: 1 }]);
    l.apply(Event::Ready { generation: 1 }, T + 2_000);
    assert_eq!(l.held_until_ms(), T + IDLE_MS);
    let s = l.apply(Event::Wake { why: Wake::Joined }, T + 60_000);
    assert!(s.actions.is_empty(), "awake, a join only holds it: {s:?}");
    assert_eq!(l.held_until_ms(), T + 60_000 + IDLE_MS);
    let mut failed = Lifecycle { phase: Phase::Failed { why: "pull failed".into(), since_ms: T - 1_000 }, ..Lifecycle::new() };
    let s = failed.apply(Event::Wake { why: Wake::Joined }, T);
    assert!(s.actions.is_empty() && s.refused.is_some(), "{s:?}");
}

/// Goal: a wake racing a sleep wins, and starts a new generation once
/// the old one is gone; the old one's late reports change nothing.
#[test]
fn a_wake_during_sleep_starts_it_again_after() {
    let mut l = Lifecycle::new();
    let g1 = started(&mut l, T);
    l.apply(Event::Alarm, T + IDLE_MS);
    assert!(matches!(l.phase, Phase::Sleeping { .. }));
    assert!(l.apply(Event::Wake { why: Wake::Record }, T + IDLE_MS + 1_000).actions.is_empty());
    let s = l.apply(Event::Asleep { generation: g1 }, T + IDLE_MS + 5_000);
    let s = owed(&mut l, &s, T + IDLE_MS + 5_000);
    assert!(s.actions.contains(&Action::Start { generation: g1 + 1 }), "{s:?}");
    // the old generation's late reports are ignored
    for e in [Event::Ready { generation: g1 }, Event::Asleep { generation: g1 }, Event::Exited { generation: g1 }, Event::StartFailed { generation: g1, why: "late".into() }] {
        let before = l.clone();
        assert!(l.apply(e, T + IDLE_MS + 6_000).actions.is_empty());
        assert_eq!(l, before);
    }
}

/// Goal: the owner's sleep lets go of what held it, so it stays asleep
/// until the next wake; the guest's keepalive closing as its container
/// goes down holds nothing.
#[test]
fn the_owner_puts_it_to_sleep() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
    let s = l.apply(Event::Sleep, T + 5_000);
    assert!(s.actions.contains(&Action::Sleep { generation: g }), "{s:?}");
    l.apply(Event::Closed { socket: Socket::Keepalive }, T + 6_000);
    // a guest busy again as it goes down: its socket dies with it
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + 7_000);
    let s = l.apply(Event::Asleep { generation: g }, T + 9_000);
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "{s:?}");
    assert_eq!(l.phase, Phase::ASLEEP);
    assert!(l.apply(Event::Sleep, T + 10_000).actions.is_empty(), "asleep, a sleep does nothing");
}

/// Goal: an always-on computer never sleeps and its awake time is not
/// metered (decision 25).
#[test]
fn always_on_never_sleeps_nor_meters() {
    let mut l = Lifecycle::new();
    let s = l.apply(Event::AlwaysOn { on: true }, T);
    assert_eq!(s.actions, vec![Action::Start { generation: 1 }]);
    l.apply(Event::Ready { generation: 1 }, T + 3_000);
    for i in 1..20 {
        let s = l.apply(Event::Alarm, T + i * IDLE_MS);
        assert!(s.actions.is_empty(), "{s:?}");
    }
}

/// Goal: the state survives a restart of its Durable Object (it is kept
/// whole as JSON) and answers the same after.
#[test]
fn it_survives_a_restart() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 5_000);
    let mut back: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back, l);
    assert_eq!(back.apply(Event::Alarm, T + 2 * IDLE_MS), l.clone().apply(Event::Alarm, T + 2 * IDLE_MS));
    // a lifecycle stored before it knew which start came up reads as
    // one whose start did not: its end is no life's
    let mut old: Value = serde_json::to_value(&l).unwrap();
    old.as_object_mut().unwrap().retain(|k, _| k != "ready" && k != "fell_back");
    let mut old: Lifecycle = serde_json::from_value(old).unwrap();
    let s = old.apply(Event::Exited { generation: old.generation() }, T + 6_000);
    assert_eq!(s.ended, None, "{s:?}");
}

/// The previous release's stored lifecycles (master at 2c45cb5f7, its own
/// code: a start, its owner's restart as it saves, the start that restart
/// owed, that start failing on one R2 502, and the computer that then
/// won't wake), as this release finds them in a computer's storage. That
/// release also wrote the flags of the one before it beside the phases
/// (FIN-395 C); this one reads the phases alone.
const PREV: [(&str, &str); 5] = [
    ("starting", r#"{"phase":{"phase":"starting","generation":1,"since_ms":1800000000000,"destroying":false},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001200000,"failures":0,"always_on":false,"metered_to_ms":0,"ready":false,"fell_back":false,"saving":null,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":0,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":0,"wake_after_sleep":false,"retry_at_ms":null,"failed_ms":0,"owner_sleep":false,"restarting":false}"#),
    ("restart_sleeping", r#"{"phase":{"phase":"sleeping","generation":1,"since_ms":1800000010000,"owner":true,"restarting":true,"owes_start":true},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"failures":0,"always_on":false,"metered_to_ms":1800000003000,"ready":true,"fell_back":false,"saving":{"step":"hold","since_ms":1800000010000},"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":0,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000003000,"wake_after_sleep":true,"retry_at_ms":null,"failed_ms":0,"owner_sleep":true,"restarting":true}"#),
    ("asleep_owed", r#"{"phase":{"phase":"asleep","retry":[1800000015000,true]},"generation":1,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"failures":0,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000,"wake_after_sleep":true,"retry_at_ms":1800000015000,"failed_ms":0,"owner_sleep":false,"restarting":false}"#),
    ("asleep_retry", r#"{"phase":{"phase":"asleep","retry":[1800000021000,false]},"generation":2,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"failures":1,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000,"wake_after_sleep":false,"retry_at_ms":1800000021000,"failed_ms":0,"owner_sleep":false,"restarting":false}"#),
    ("failed", r#"{"phase":{"phase":"failed","why":"restore '/data': gateway rejected the range download with HTTP 502","since_ms":1800000033000},"generation":4,"keepalives":0,"tabs":0,"held_until_ms":1800001210000,"failures":3,"always_on":false,"metered_to_ms":1800000015000,"ready":false,"fell_back":false,"saving":null,"save_due_ms":null,"busy_since_ms":null,"save_asked_ms":1800000010000,"save_seq":1,"save_failures":0,"unsaved_since_ms":null,"restore_fallbacks":0,"failing":null,"unmetered":null,"worked_ms":0,"kept_ms":1800000010000,"wake_after_sleep":false,"retry_at_ms":null,"failed_ms":1800000033000,"owner_sleep":false,"restarting":false}"#),
];

/// The flags the previous release wrote beside the phases.
const PREV_FLAGS: [&str; 5] = ["wake_after_sleep", "retry_at_ms", "failed_ms", "owner_sleep", "restarting"];

fn prev(name: &str) -> Lifecycle {
    let (_, text) = PREV.iter().find(|(n, _)| *n == name).expect("a stored lifecycle of the previous release");
    Lifecycle::read(text).expect("this release reads the previous one's")
}

/// Goal: what the previous release stored reads as the phase it stored;
/// and what this release stores reads back, in the previous one (a
/// rollback), as it stored it: every key the same but its flags, which it
/// writes again from the phase as it reads. Method: the previous release's
/// own stored lifecycles, read here, then written.
#[test]
fn a_lifecycle_stored_by_the_previous_release_reads_as_it_was() {
    assert_eq!(prev("starting").phase, Phase::Starting { generation: 1, since_ms: T, destroying: false });
    assert_eq!(prev("restart_sleeping").phase, Phase::Sleeping { generation: 1, since_ms: T + 10_000, owner: true, restarting: true, owes_start: true });
    assert_eq!(prev("asleep_owed").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 15_000, owed: true }) });
    assert_eq!(prev("asleep_retry").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 21_000, owed: false }) });
    assert_eq!(prev("asleep_retry").failures, 1, "a count of the wake, kept beside the phases");
    assert!(matches!(prev("failed").phase, Phase::Failed { since_ms, .. } if since_ms == T + 33_000));
    for (name, text) in PREV {
        let mut theirs: Value = serde_json::from_str(text).unwrap();
        for flag in PREV_FLAGS {
            assert!(theirs.as_object_mut().unwrap().remove(flag).is_some(), "{name}: {flag}");
        }
        assert_eq!(serde_json::to_value(prev(name)).unwrap(), theirs, "{name}: what it stored, but its flags");
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

/// Goal (#557): a keepalive its Durable Object's runtime no longer holds
/// (a deploy disconnected it, and its guest's close reached no release)
/// is closed before the next event, as its close would have been: the
/// computer is saved once that work settles and sleeps once nothing
/// holds it, where on the preview it stayed awake 14 hours, saved every
/// 15 minutes and metered. One the runtime holds is never closed, however
/// long its turn runs; none is counted open that it did not hear open.
/// Replay: reconciled, nothing more is lost, and a late close, counted
/// against the sockets still open, closes nothing more. Restart: the
/// same from a lifecycle stored and read back (a deploy restarts the
/// DO). Method: the lifecycle's steps, as the DO drives them.
#[test]
fn a_keepalive_the_runtime_lost_is_closed_and_a_held_one_never() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
    // a turn that runs on, its keepalive held: never lost, saved on
    // the busy timer, never put to sleep
    let mut at = T + 4_000;
    for _ in 0..8 {
        at += SAVE_EVERY_MS;
        assert_eq!(l.keepalives_lost(1), 0, "one the runtime holds is never lost");
        assert_eq!(l.apply(Event::Alarm, at).actions.last(), Some(&Action::Hold { generation: g }), "busy: saved on its timer");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
        l.apply(Event::Saved { generation: g, seq }, at + 3_000);
        assert!(matches!(l.phase, Phase::Awake { .. }) && l.keepalives() == 1, "{l:?}");
    }
    // a deploy restarts the DO and disconnects the socket; its guest's
    // close after it never comes
    let mut l: Lifecycle = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!((l.keepalives_lost(1), l.keepalives_lost(0)), (0, 1), "read back, the same");
    at += 60_000;
    assert!(l.apply(Event::Closed { socket: Socket::Keepalive }, at).actions.is_empty());
    assert_eq!((l.keepalives(), l.keepalives_lost(0)), (0, 0), "reconciled, nothing more is lost");
    // its work ended there: saved once it settles, asleep once nothing holds it
    assert_eq!(l.apply(Event::Alarm, at + SAVE_SETTLE_MS).actions.last(), Some(&Action::Hold { generation: g }));
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + SAVE_SETTLE_MS + 500));
    l.apply(Event::Saved { generation: g, seq }, at + SAVE_SETTLE_MS + 3_000);
    let s = l.apply(Event::Alarm, at + IDLE_MS);
    assert!(s.actions.contains(&Action::Sleep { generation: g }), "asleep once its hold runs out: {s:?}");
    // two open, the runtime holding one: one is closed, and the other
    // keeps it busy; the lost one's late close then closes nothing more
    let mut two = Lifecycle::new();
    let g = started(&mut two, T);
    two.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000);
    two.apply(Event::Opened { socket: Socket::Keepalive }, T + 5_000);
    assert_eq!(two.keepalives_lost(1), 1);
    two.apply(Event::Closed { socket: Socket::Keepalive }, T + 6_000);
    assert_eq!((two.keepalives(), two.keepalives_lost(1)), (1, 0), "the held one stays counted");
    let s = two.apply(Event::Alarm, T + 6_000 + IDLE_MS);
    assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }), "busy: saved on its timer, never put to sleep: {s:?}");
    assert!(matches!(two.phase, Phase::Awake { generation, .. } if generation == g));
    // invalid: never more than it counts, nor more than the cap at once
    let mut many = Lifecycle::new();
    started(&mut many, T);
    for i in 0..40 {
        many.apply(Event::Opened { socket: Socket::Keepalive }, T + 4_000 + i);
    }
    assert_eq!((many.keepalives_lost(0), many.keepalives_lost(39), many.keepalives_lost(40)), (KEEPALIVES_LOST_MAX, 1, 0));
    assert_eq!((many.keepalives_lost(1_000), many.keepalives_lost(usize::MAX)), (0, 0), "none counted open that it did not hear open");
    assert_eq!(Lifecycle::new().keepalives_lost(0), 0, "asleep, nothing to close");
}
