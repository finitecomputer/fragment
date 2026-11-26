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

/// Goal: an open keepalive keeps it awake past any hold, and closing it
/// holds it twenty minutes more (a tab holds it only by its use:
/// `a_tab_no_one_uses_lets_it_sleep`).
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
    assert_eq!((l.keepalives, l.tabs), (0, 1), "the tab is still counted open");
}

/// Its alarms from `l.next_alarm()` up to `until`, fired as the DO fires
/// them, each hold and save it asks for answered as the DO does (saved):
/// when each awake save was asked, and when it began to sleep, if it did.
fn run_alarms(l: &mut Lifecycle, until: i64) -> (Vec<i64>, Option<i64>) {
    let mut asked = vec![];
    // bounded: a day of alarms, every METER_EVERY_MS at the most
    for _ in 0..1_000 {
        let Some(at) = l.next_alarm().filter(|at| *at <= until) else { return (asked, None) };
        let s = l.apply(Event::Alarm, at);
        if let Some(Action::Sleep { .. }) = s.actions.last() {
            return (asked, Some(at));
        }
        if let Some(Action::Hold { generation }) = s.actions.last().cloned() {
            asked.push(at);
            let (_, _, seq) = save_asked(&l.apply(Event::Held { generation, held: true }, at + 100));
            assert_eq!(l.apply(Event::Saved { generation, seq }, at + 200).actions, vec![Action::Unhold { generation }]);
        }
    }
    panic!("more alarms than a day asks for");
}

/// The sleep `l` began (`Action::Sleep`, at `at`), its hold, save and stop
/// answered as the DO does: its life's end.
fn sleep_through(l: &mut Lifecycle, at: i64) -> Ended {
    let g = l.generation();
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
    assert_eq!(l.apply(Event::Saved { generation: g, seq }, at + 1_000).actions, vec![Action::Stop { generation: g, saved: true }]);
    l.apply(Event::Asleep { generation: g }, at + 2_000).ended.expect("a life ends at its sleep")
}

/// Goal (Paul, 2026-11-18: "an open tab, without any action touching the
/// agent, should not keep the agent awake indefinitely"): a port tab its
/// page says nothing on holds the computer `IDLE_MS` from its opening, as
/// a record does, then it sleeps as under no tab: its hold, its save, its
/// stop, metered up to that stop and no further. On master it stayed
/// awake, unsaved, as long as the tab was open (11 hours on the preview,
/// 2026-11-18). Method: its alarms fired for a day, a tab opened and
/// never used.
#[test]
fn a_tab_no_one_uses_lets_it_sleep() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    let opened = T + 10 * 60_000;
    l.apply(Event::Opened { socket: Socket::Tab }, opened);
    let (saves, slept) = run_alarms(&mut l, T + 24 * 3_600_000);
    assert_eq!(slept, Some(opened + IDLE_MS), "it sleeps IDLE_MS after the tab's last word: its opening");
    assert!(saves.is_empty(), "nothing was done to save before its sleep's own: {saves:?}");
    let ended = sleep_through(&mut l, opened + IDLE_MS);
    assert_eq!((ended.generation, ended.by, ended.saved), (g, LifeEnd::Sleep, true));
    assert_eq!(l.phase, Phase::ASLEEP);
    // its tab's socket closes as its container goes: that holds nothing,
    // starts nothing, and meters nothing
    let s = l.apply(Event::Closed { socket: Socket::Tab }, opened + IDLE_MS + 3_000);
    assert_eq!((s.actions.clone(), s.alarm_ms, l.phase.clone()), (vec![], None, Phase::ASLEEP), "{s:?}");
    assert!(l.apply(Event::TabUsed, opened + IDLE_MS + 4_000).actions.is_empty(), "a use heard after its stop starts nothing");
}

/// Goal: a port tab a person uses (its page speaks: the DO tells each
/// minute of it) keeps the computer awake as a turn does, and is saved
/// every `SAVE_EVERY_MS` while used, so a stop during long work through
/// a screen or a terminal loses at most about that; once its page goes
/// quiet it sleeps `IDLE_MS` after its last word. Valid: two hours of
/// use. Invalid: a use with no tab counted is nothing. Restart: a
/// lifecycle stored and read back mid-use saves on time. Always on: saved
/// the same, never slept nor metered. On master it was never saved while
/// the tab stayed open. Method: a minute's use at a time, the alarms in
/// between fired and answered as the DO does.
#[test]
fn a_tab_in_use_keeps_it_awake_and_saves_every_15_minutes() {
    for always_on in [false, true] {
        let mut l = Lifecycle::new();
        if always_on {
            l.apply(Event::AlwaysOn { on: true }, T);
            l.apply(Event::Ready { generation: 1 }, T + 3_000);
        } else {
            started(&mut l, T);
        }
        // invalid: a use with no tab counted holds nothing
        let held = l.held_until_ms();
        l.apply(Event::TabUsed, T + 5_000);
        assert_eq!(l.held_until_ms(), held, "no tab, no use");
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        let mut saves = vec![];
        let last = T + 2 * 3_600_000;
        let mut at = T + 60_000;
        while at <= last {
            let (asked, slept) = run_alarms(&mut l, at - 1);
            assert_eq!(slept, None, "in use, it never sleeps (always on: {always_on})");
            saves.extend(asked);
            l.apply(Event::TabUsed, at);
            if at == T + 3_600_000 {
                // restart: stored and read back as its isolate goes
                l = Lifecycle::read(&serde_json::to_string(&l).unwrap()).unwrap().0;
            }
            at += 60_000;
        }
        assert!(saves.len() >= 7, "two hours in use save every 15 minutes: {saves:?}");
        assert_eq!(saves[0], T + 60_000 + SAVE_EVERY_MS, "the first 15 minutes after its first use: {saves:?}");
        for w in saves.windows(2) {
            assert!(w[1] - w[0] >= SAVE_EVERY_MS && w[1] - w[0] <= SAVE_EVERY_MS + 60_000, "every 15 minutes, a minute's use apart at most: {saves:?}");
        }
        let (asked, slept) = run_alarms(&mut l, last + 3 * IDLE_MS);
        if always_on {
            assert_eq!(slept, None, "always on never sleeps");
            assert!(asked.len() <= 1, "quiet, it saves no more than its last use's: {asked:?}");
        } else {
            assert_eq!(slept, Some(last + IDLE_MS), "quiet, it sleeps IDLE_MS after its last use");
            assert!(asked.len() <= 1, "{asked:?}");
        }
    }
}

/// Goal: a computer that slept under a tab no one used wakes when its
/// person comes back to that tab (the screen opens its sockets again on
/// their next press or key: a wake, then a tab), and is held as any
/// tab's opening holds it; with no one back, nothing wakes it, however
/// long. On master it never slept under the tab. Method: the sleep, an
/// hour's silence, then the tab's wake.
#[test]
fn a_tab_wakes_it_after_it_slept_under_it() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    let (_, slept) = run_alarms(&mut l, T + 24 * 3_600_000);
    let slept = slept.expect("it sleeps under a tab no one uses");
    sleep_through(&mut l, slept);
    l.apply(Event::Closed { socket: Socket::Tab }, slept + 3_000);
    assert_eq!((l.next_alarm(), l.phase.clone()), (None, Phase::ASLEEP), "no alarm: nothing wakes it by itself");
    // an hour on, the person presses a key in the tab
    let back = slept + 3_600_000;
    let s = l.apply(Event::Wake { why: Wake::Tab }, back);
    let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("a tab's wake starts it: {s:?}") };
    l.apply(Event::Ready { generation }, back + 3_000);
    l.apply(Event::Opened { socket: Socket::Tab }, back + 3_500);
    assert!(matches!(l.phase, Phase::Awake { .. }) && l.held_until_ms() == back + 3_500 + IDLE_MS, "held IDLE_MS from the tab's opening: {l:?}");
}

/// A turn on `l` (a keepalive opened at `at`, closed a second on), and the
/// save its end asks for once it settles, answered as the DO does: when
/// that save was asked.
fn turn_saved_at(l: &mut Lifecycle, at: i64) -> i64 {
    let g = l.generation();
    l.apply(Event::Opened { socket: Socket::Keepalive }, at);
    l.apply(Event::Closed { socket: Socket::Keepalive }, at + 1_000);
    let asked = at + 1_000 + SAVE_SETTLE_MS;
    assert_eq!(l.apply(Event::Alarm, asked).actions.last(), Some(&Action::Hold { generation: g }));
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, asked + 100));
    l.apply(Event::Saved { generation: g, seq }, asked + 200);
    asked
}

/// Goal (review, 2026-11-18): a person's use of an open port tab is heard
/// a minute apart at most (`TAB_USED_EVERY_MS`), so one after a save may
/// not be heard when its container stops: a stop within that minute of a
/// use heard before the save is a crash, its loss told, never a quiet
/// sleep that drops it unsaid. A save past that minute holds all it did:
/// the same stop is a sleep. On the reviewed head it was a quiet sleep.
/// Method: a use, a save inside its minute, then the container's exit.
#[test]
fn a_tab_use_not_heard_yet_is_work_at_a_stop() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    l.apply(Event::TabUsed, T + 60_000);
    let saved = turn_saved_at(&mut l, T + 61_000);
    assert!(saved < T + 60_000 + TAB_USED_EVERY_MS, "the save is within the use's minute");
    let s = l.apply(Event::Exited { generation: g }, saved + 5_000);
    let ended = s.ended.expect("a life ends");
    assert_eq!((ended.by, ended.quiet, ended.unchanged), (LifeEnd::Exit, false, false), "a crash, its loss told: {ended:?}");
    // past the minute, a save holds every use: the same stop is a sleep
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    l.apply(Event::TabUsed, T + 60_000);
    let saved = turn_saved_at(&mut l, T + 60_000 + TAB_USED_EVERY_MS);
    let s = l.apply(Event::Exited { generation: g }, saved + 5_000);
    let ended = s.ended.expect("a life ends");
    assert_eq!((ended.by, ended.saved, ended.quiet), (LifeEnd::Sleep, true, true), "{ended:?}");
}

/// Goal (review, 2026-11-18, round 2): a sleep closes its port tabs as it
/// begins, before its hold (the cell's, `hold`), so nothing a person sends
/// after reaches the container its save holds, and a use heard as it
/// sleeps was sent before: it neither cancels the sleep nor moves it, idle
/// or its owner's, and its end is a sleep, saved. Its tabs stay counted
/// until each close is heard (a close carries a use not told yet).
#[test]
fn a_tab_use_heard_as_it_sleeps_was_before_it() {
    for owner in [false, true] {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
        let at = match owner {
            false => {
                let at = T + 10_000 + IDLE_MS;
                assert_eq!(l.apply(Event::Alarm, at).actions.last(), Some(&Action::Sleep { generation: g }), "idle, it sleeps");
                at
            }
            true => {
                l.apply(Event::Sleep, T + 20_000);
                T + 20_000
            }
        };
        assert_eq!(l.tabs, 1, "counted until its close is heard");
        let before = (l.phase.clone(), l.saving());
        assert!(l.apply(Event::TabUsed, at + 100).actions.is_empty(), "as it holds: nothing");
        assert_eq!((l.phase.clone(), l.saving()), before, "nor cancelled");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 500));
        assert!(l.apply(Event::TabUsed, at + 2_000).actions.is_empty(), "as it saves: nothing");
        l.apply(Event::Closed { socket: Socket::Tab }, at + 2_500);
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, at + 4_000).actions, vec![Action::Stop { generation: g, saved: true }]);
        let ended = l.apply(Event::Asleep { generation: g }, at + 6_000).ended.expect("a life ends");
        assert_eq!((ended.by, ended.saved), (LifeEnd::Sleep, true), "owner {owner}: {ended:?}");
    }
}

/// Goal (review, 2026-11-18, round 2): its owner's sleep leaves its tabs
/// counted until each close is heard, so its container exiting as the
/// sleep holds it, within a minute of a use heard before a save, still
/// counts a use not heard yet: a crash, its loss told. On the round-1
/// head the sleep zeroed its tabs, and the same exit was quiet.
#[test]
fn its_owners_sleep_still_counts_a_tab_use_not_heard_yet() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    l.apply(Event::TabUsed, T + 60_000);
    let saved = turn_saved_at(&mut l, T + 61_000);
    l.apply(Event::Sleep, saved + 1_000);
    let ended = l.apply(Event::Exited { generation: g }, saved + 2_000).ended.expect("a life ends");
    assert_eq!((ended.by, ended.quiet, ended.unchanged), (LifeEnd::Exit, false, false), "{ended:?}");
}

/// Goal (review, 2026-11-18, round 2): a tab's use is its life's own. A
/// life that came up after one used a tab, with only a silent tab opened
/// on it since, changed nothing at a stop within that minute: the opening
/// is work, so the stop is told, but as no change lost (`unchanged`). On
/// the round-1 head the earlier life's use counted: told as changes lost.
#[test]
fn an_earlier_lifes_tab_use_is_not_this_ones() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    l.apply(Event::TabUsed, T + 20_000);
    // its owner's restart: saved, then a fresh start, up within the minute
    l.apply(Event::Restart { generation: g }, T + 21_000);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 21_500));
    l.apply(Event::Saved { generation: g, seq }, T + 22_000);
    l.apply(Event::Closed { socket: Socket::Tab }, T + 22_100);
    let s = l.apply(Event::Asleep { generation: g }, T + 23_000);
    let s = owed(&mut l, &s, T + 23_000);
    let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
    l.apply(Event::Ready { generation }, T + 25_000);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 26_000);
    let ended = l.apply(Event::Exited { generation }, T + 30_000).ended.expect("a life ends");
    assert_eq!((ended.by, ended.quiet, ended.unchanged), (LifeEnd::Exit, false, true), "a silent tab's opening on a life that changed nothing: {ended:?}");
}

/// Goal (review, 2026-11-18): a deploy can cut a tab's socket without a
/// close the lifecycle hears, so it counts that tab open for good. A tab
/// used since the last save asked is saved once its close settles, however
/// many it still counts; one never used asks no save while another is
/// counted, as before. On the reviewed head only the last tab's close saved.
#[test]
fn closing_a_used_tab_saves_whatever_else_is_counted() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    // one cut by a deploy: counted open, never closed
    l.apply(Event::Opened { socket: Socket::Tab }, T + 5_000);
    l.apply(Event::Opened { socket: Socket::Tab }, T + 10_000);
    let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 20_000);
    assert_eq!(l.save_due_ms, None, "a tab never used asks no save while another is counted: {s:?}");
    l.apply(Event::Opened { socket: Socket::Tab }, T + 30_000);
    l.apply(Event::TabUsed, T + 40_000);
    let s = l.apply(Event::Closed { socket: Socket::Tab }, T + 50_000);
    assert_eq!((l.tabs, l.save_due_ms, s.alarm_ms), (1, Some(T + 50_000 + SAVE_SETTLE_MS), Some(T + 50_000 + SAVE_SETTLE_MS)), "its work ended: saved once it settles");
    let (saves, _) = run_alarms(&mut l, T + 50_000 + SAVE_SETTLE_MS);
    assert_eq!(saves, vec![T + 50_000 + SAVE_SETTLE_MS]);
}

/// Goal (Paul, 2026-11-18: one number): every wake holds the computer
/// the idle wait, its owner's (the shell's pre-wake) as a record does, and
/// a later one holds it from then.
#[test]
fn every_wake_holds_it_the_idle_wait() {
    for why in [Wake::Record, Wake::Tab, Wake::Owner, Wake::Joined] {
        let mut l = Lifecycle::new();
        let s = l.apply(Event::Wake { why }, T);
        let Action::Start { generation } = s.actions[0] else { panic!("{why:?}: {s:?}") };
        l.apply(Event::Ready { generation }, T + 2_000);
        assert_eq!(l.held_until_ms(), T + IDLE_MS, "{why:?}");
        assert!(!l.apply(Event::Alarm, T + IDLE_MS - 1).actions.iter().any(|a| matches!(a, Action::Sleep { .. })), "{why:?}");
        assert!(l.apply(Event::Alarm, T + IDLE_MS).actions.contains(&Action::Sleep { generation }), "{why:?}");
    }
    let mut l = Lifecycle::new();
    l.apply(Event::Wake { why: Wake::Tab }, T);
    l.apply(Event::Ready { generation: 1 }, T + 2_000);
    l.apply(Event::Wake { why: Wake::Tab }, T + 3_000);
    let s = l.apply(Event::Alarm, T + IDLE_MS);
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

/// Goal: the phase its owner is shown (and told of as it changes) follows
/// a wake, a sleep, a wake that cancels a sleep, a wake heard once its stop
/// began, and a start that kept failing; a start owed and due shows as
/// starting before the alarm makes it.
#[test]
fn the_phase_shown_follows_its_wakes_and_sleeps() {
    use fragment_proto::computer::ComputerPhase as Shown;
    let mut l = Lifecycle::new();
    assert_eq!(l.shown(T), Shown::Asleep);
    let s = l.apply(Event::Wake { why: Wake::Record }, T);
    let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("a wake starts it: {s:?}") };
    assert_eq!(l.shown(T), Shown::Starting, "while it starts, before its guest is up");
    l.apply(Event::Ready { generation: g }, T + 3_000);
    assert_eq!(l.shown(T + 3_000), Shown::Awake);
    l.apply(Event::Alarm, T + IDLE_MS);
    assert_eq!(l.shown(T + IDLE_MS), Shown::Sleeping);
    // a message heard before its stop began: awake again at once
    l.apply(Event::Wake { why: Wake::Record }, T + IDLE_MS + 100);
    assert_eq!(l.shown(T + IDLE_MS + 100), Shown::Awake, "{:?}", l.phase);
    // heard once its stop began: its start owed once it is down, shown as starting
    let later = T + 2 * IDLE_MS + 1_000;
    l.apply(Event::Alarm, later);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, later + 500));
    l.apply(Event::Saved { generation: g, seq }, later + 800);
    l.apply(Event::Wake { why: Wake::Record }, later + 1_000);
    assert_eq!(l.shown(later + 1_000), Shown::Sleeping, "{:?}", l.phase);
    let s = l.apply(Event::Asleep { generation: g }, later + 5_000);
    assert_eq!(l.shown(later + 5_000), Shown::Starting, "owed and due: {:?}", l.phase);
    owed(&mut l, &s, later + 5_000);
    assert_eq!(l.shown(later + 5_000), Shown::Starting);
    let failed = Lifecycle { phase: Phase::Failed { why: "pull failed".into(), since_ms: T }, ..Lifecycle::new() };
    assert_eq!(failed.shown(T), Shown::WontWake);
}

/// Goal: a wake racing a sleep whose stop began (its container is going)
/// wins, and starts a new generation once the old one is gone; the old
/// one's late reports change nothing.
#[test]
fn a_wake_during_sleep_starts_it_again_after() {
    let mut l = Lifecycle::new();
    let g1 = started(&mut l, T);
    l.apply(Event::Alarm, T + IDLE_MS);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g1, held: true }, T + IDLE_MS + 500));
    assert_eq!(l.apply(Event::Saved { generation: g1, seq }, T + IDLE_MS + 800).actions, vec![Action::Stop { generation: g1, saved: true }]);
    assert!(l.apply(Event::Wake { why: Wake::Record }, T + IDLE_MS + 1_000).actions.is_empty());
    assert!(matches!(l.phase, Phase::Sleeping { owes_start: true, .. }));
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
    Lifecycle::read(text).expect("this release reads the previous one's").0
}

/// Goal: what the previous release stored reads as the phase it stored,
/// its flags agreeing (nothing filled, nothing logged); and what this
/// release stores reads back, in the previous one (a rollback), as it
/// stored it: every key the same but its flags, which it writes again from
/// the phase as it reads. Method: the previous release's own stored
/// lifecycles, read here, then written.
#[test]
fn a_lifecycle_stored_by_the_previous_release_reads_as_it_was() {
    assert_eq!(prev("starting").phase, Phase::Starting { generation: 1, since_ms: T, destroying: false });
    assert_eq!(prev("restart_sleeping").phase, Phase::Sleeping { generation: 1, since_ms: T + 10_000, owner: true, restarting: true, owes_start: true });
    assert_eq!(prev("asleep_owed").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 15_000, owed: true }) });
    assert_eq!(prev("asleep_retry").phase, Phase::Asleep { retry: Some(Retry { at_ms: T + 21_000, owed: false }) });
    assert_eq!(prev("asleep_retry").failures, 1, "a count of the wake, kept beside the phases");
    assert!(matches!(prev("failed").phase, Phase::Failed { since_ms, .. } if since_ms == T + 33_000));
    for (name, text) in PREV {
        assert!(!Lifecycle::read(text).unwrap().1, "{name}: nothing filled");
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

/// What `s` meters, as `(from, to)`.
fn meters(s: &Step) -> Vec<(i64, i64)> {
    s.actions.iter().filter_map(|a| if let Action::Meter { from_ms, to_ms } = a { Some((*from_ms, *to_ms)) } else { None }).collect()
}

/// Three days, the silence an event of FIN-511's paths comes after.
const DAYS: i64 = 3 * 24 * 3_600_000;

/// Goal (FIN-511, path 1): an awake computer whose alarms went unheard,
/// heard again long after anything was heard of it (`ALARM_LATE_MS`), is
/// metered only up to the last moment it was shown up, never the silence
/// after it, which nothing shows it ran; metered on from there as it is
/// heard, and its exit, heard late, ends its life as before. Valid: heard
/// just within the bound, metered to it as ever. Method: a turn begun (a
/// keepalive opened), then its next alarm, or its exit, 3 days on.
#[test]
fn an_awake_computer_heard_late_is_metered_to_when_it_was_shown_up() {
    let awake = || {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Opened { socket: Socket::Keepalive }, T + 5_000);
        (l, g)
    };
    let late = T + DAYS;
    let (mut l, _) = awake();
    assert_eq!(meters(&l.apply(Event::Alarm, late)), vec![(T + 3_000, T + 5_000)], "its next alarm, 3 days late");
    assert_eq!(meters(&l.apply(Event::Alarm, late + METER_EVERY_MS)), vec![(late, late + METER_EVERY_MS)], "metered on from there, as it is heard again");
    let (mut l, g) = awake();
    let s = l.apply(Event::Exited { generation: g }, late);
    assert_eq!(meters(&s), vec![(T + 3_000, T + 5_000)], "its exit, heard 3 days late");
    assert!(s.ended.is_some_and(|e| e.by == LifeEnd::Exit && e.at_ms == late), "{s:?}");
    let (mut l, _) = awake();
    assert_eq!(meters(&l.apply(Event::Alarm, T + 5_000 + ALARM_LATE_MS)), vec![(T + 3_000, T + 5_000 + ALARM_LATE_MS)], "heard within the bound");
}

/// Goal (FIN-511, path 2; #1284's review): a computer going to sleep whose
/// alarms went unheard (its owner's restart, saved, its stop under way)
/// and whose end is heard days later is metered to its stop's start only;
/// its restart still ends as one, and its start is still owed. Method: the
/// restart's steps, then its `Asleep` 3 days on.
#[test]
fn a_sleep_heard_late_is_metered_to_its_last_step() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Restart { generation: g }, T + 10_000);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 10_500));
    assert_eq!(l.apply(Event::Saved { generation: g, seq }, T + 12_000).actions, vec![Action::Stop { generation: g, saved: true }]);
    let late = T + DAYS;
    let s = l.apply(Event::Asleep { generation: g }, late);
    assert_eq!(meters(&s), vec![(T + 3_000, T + 12_000)], "metered to its stop's start: {s:?}");
    assert!(s.ended.is_some_and(|e| e.restart && e.saved), "its restart ends as one: {s:?}");
    assert_eq!(owed(&mut l, &s, late).actions, vec![Action::Start { generation: g + 1 }], "and its start is owed");
}

/// Its owner's sleep at T + 10 s, its hold answered at T + 11 s: its save
/// under way (`seq`), as #1302's review traced it.
fn saving_sleep() -> (Lifecycle, u64, u64) {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Sleep, T + 10_000);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 11_000));
    (l, g, seq)
}

/// Goal (FIN-511, #1302's review): a sleep whose save's report was lost,
/// failed by its deadline's alarm heard days on, is metered to the start
/// of its try (its sleep), as a failed try heard on time is: none of the
/// try (its hold, its save: free) and none of the silence, nor after it.
/// Method: the review's trace.
#[test]
fn a_sleep_failed_by_a_late_alarm_meters_none_of_its_try() {
    let (mut l, g, _) = saving_sleep();
    let s = l.apply(Event::Alarm, T + DAYS);
    assert_eq!(meters(&s), vec![(T + 3_000, T + 10_000)], "to its sleep: {s:?}");
    assert!(s.actions.contains(&Action::Unhold { generation: g }) && l.unsaved_since_ms() == Some(T + DAYS), "its try failed, its window open: {s:?}");
    assert_eq!(meters(&l.apply(Event::Alarm, T + DAYS + 60_000)), vec![], "and nothing after it: its window, free");
}

/// Goal (FIN-511, #1302's review): a sleep whose save's failure is heard
/// days on is metered to the start of its try, as one heard on time is.
/// Method: the review's trace, its `SaveFailed` late.
#[test]
fn a_sleep_failed_late_meters_none_of_its_try() {
    let (mut l, g, seq) = saving_sleep();
    let s = l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, T + DAYS);
    assert_eq!(meters(&s), vec![(T + 3_000, T + 10_000)], "to its sleep: {s:?}");
    assert_eq!(meters(&l.apply(Event::Alarm, T + DAYS + 60_000)), vec![], "and nothing after it: its window, free");
}

/// Goal (#1302's review, round 2): a sleep's try heard late that then
/// saves is billed what it was shown running for, its sleep to its save's
/// start, beside its stop's seconds, and none of the silence, when the
/// save's report itself comes days on. A late wake first (a late event not
/// its outcome) passes over the silence from there, and its hold, shown
/// before it, goes unbilled: never a silence billed, at the cost of that
/// second (#1302's review, at 5e5ceb0cf). Method: the review's trace,
/// saved late.
#[test]
fn a_sleep_saved_late_is_billed_its_try() {
    let late = T + DAYS;
    let (mut l, g, seq) = saving_sleep();
    let mut m = meters(&l.apply(Event::Saved { generation: g, seq }, late));
    m.extend(meters(&l.apply(Event::Asleep { generation: g }, late + 1_000)));
    assert_eq!(m, vec![(T + 3_000, T + 10_000), (T + 10_000, T + 11_000), (late, late + 1_000)], "its saved report, days on");
    let (mut l, g, seq) = saving_sleep();
    let mut m = meters(&l.apply(Event::Wake { why: Wake::Record }, late));
    m.extend(meters(&l.apply(Event::Saved { generation: g, seq }, late + 1_000)));
    m.extend(meters(&l.apply(Event::Asleep { generation: g }, late + 2_000)));
    assert_eq!(m, vec![(T + 3_000, T + 10_000), (late, late + 2_000)], "a late wake, then its report: its hold unbilled, none of the silence");
}

/// Goal (#1302's review, Alex's, at 5e5ceb0cf): a try whose hold was
/// asked again after a silence its meter passed over, then saved late, is
/// billed none of that silence: only what was shown before it (its awake
/// time to its sleep), the hold asked again to its answer, and its stop.
/// Method: the review's sequence, exactly: `Ready` at T + 3 s, its owner's
/// sleep at T + 60 s, its next alarm at T + 1800 s (the hold asked again),
/// `Held` at T + 1801 s, `Saved` at T + 3600 s, `Asleep` at T + 3602 s.
#[test]
fn a_late_save_after_a_retried_hold_bills_none_of_the_silence() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    let mut m = meters(&l.apply(Event::Sleep, T + 60_000));
    let s = l.apply(Event::Alarm, T + 1_800_000);
    assert!(s.actions.contains(&Action::Sleep { generation: g }), "its hold asked again: {s:?}");
    m.extend(meters(&s));
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 1_801_000));
    m.extend(meters(&l.apply(Event::Saved { generation: g, seq }, T + 3_600_000)));
    m.extend(meters(&l.apply(Event::Asleep { generation: g }, T + 3_602_000)));
    assert!(m.iter().all(|(from, to)| *to <= T + 60_000 || *from >= T + 1_800_000), "nothing in the silence [T + 60 s, T + 1800 s): {m:?}");
    assert_eq!(m, vec![(T + 3_000, T + 60_000), (T + 1_800_000, T + 1_801_000), (T + 3_600_000, T + 3_602_000)]);
}

/// Goal (#1302's review, Alex's): a sleep's try stays free across a late
/// event that is not its outcome, then fails: none of the try, and none of
/// the silence, is charged, whether a late wake came first (its save under
/// way) or a late alarm that only asked its hold again. Method: each
/// sequence, then an alarm in its window.
#[test]
fn a_try_failed_after_a_late_event_is_free() {
    let late = T + DAYS;
    let (mut l, g, seq) = saving_sleep();
    let mut m = meters(&l.apply(Event::Wake { why: Wake::Record }, late));
    m.extend(meters(&l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, late + 1_000)));
    m.extend(meters(&l.apply(Event::Alarm, late + 60_000)));
    assert_eq!(m, vec![(T + 3_000, T + 10_000)], "a late wake, then its failure");
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Sleep, T + 10_000);
    let s = l.apply(Event::Alarm, late);
    assert!(s.actions.contains(&Action::Sleep { generation: g }), "its hold asked again: {s:?}");
    let mut m = meters(&s);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, late + 1_000));
    m.extend(meters(&l.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, late + 2_000)));
    m.extend(meters(&l.apply(Event::Alarm, late + 60_000)));
    assert_eq!(m, vec![(T + 3_000, T + 10_000)], "a late alarm, then its failure");
}

/// Goal (FIN-511, the other side): an ordinary sleep that saves is
/// metered as ever, its hold, save and stop to its end. Method: the
/// review's trace, its save reported and its stop done on time.
#[test]
fn a_sleep_that_saves_is_metered_to_its_end() {
    let (mut l, g, seq) = saving_sleep();
    let mut m = meters(&l.apply(Event::Saved { generation: g, seq }, T + 12_000));
    m.extend(meters(&l.apply(Event::Asleep { generation: g }, T + 13_000)));
    assert_eq!(m, vec![(T + 3_000, T + 13_000)]);
}

/// Goal (FIN-511, path 3): a start its owner puts to sleep before it comes up
/// meters only its sleep's steps, never the time before the start was
/// made: on a computer's first start (none metered before it) or one
/// asleep for days. Its save failing (the unsaved window) the same.
/// Method: the start, its owner's sleep, its hold, save and stop.
#[test]
fn a_start_put_to_sleep_meters_none_of_the_time_before_it() {
    let asleep_for_days = || {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 60_000);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 61_000));
        l.apply(Event::Saved { generation: g, seq }, T + 62_000);
        assert_eq!(meters(&l.apply(Event::Asleep { generation: g }, T + 63_000)), vec![(T + 3_000, T + 63_000)]);
        l
    };
    let at = T + DAYS;
    for (first, mut l) in [(true, Lifecycle::new()), (false, asleep_for_days())] {
        let s = l.apply(Event::Wake { why: Wake::Owner }, at);
        let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let mut failing = l.clone();
        let mut m = meters(&l.apply(Event::Sleep, at + 10_000));
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 11_000));
        m.extend(meters(&l.apply(Event::Saved { generation: g, seq }, at + 12_000)));
        m.extend(meters(&l.apply(Event::Asleep { generation: g }, at + 13_000)));
        assert_eq!(m, vec![(at + 10_000, at + 13_000)], "its sleep's steps only (first start {first})");
        // its save failing: nothing before the sleep, and the window free
        failing.apply(Event::Sleep, at + 10_000);
        let (_, _, seq) = save_asked(&failing.apply(Event::Held { generation: g, held: true }, at + 11_000));
        let s = failing.apply(Event::SaveFailed { generation: g, seq, why: "R2 is down".into() }, at + 12_000);
        assert_eq!(meters(&s), vec![], "nothing before its sleep (first start {first}): {s:?}");
    }
}

/// Goal (FIN-511, the other side): a life heard on time is metered as it
/// always was, every interval of it and nothing more: from its `Ready`,
/// each alarm's, its sleep's steps, to its end; the days asleep between
/// two lives, none. Method: two lives three days apart, each alarm at
/// the time it asked for.
#[test]
fn an_ordinary_life_is_metered_as_before() {
    let mut l = Lifecycle::new();
    let mut metered = vec![];
    let life = |l: &mut Lifecycle, at: i64, metered: &mut Vec<(i64, i64)>| {
        let g = started(l, at);
        // its record's hold, each alarm heard at the time it asked for
        let slept = loop {
            let next = l.next_alarm().expect("awake, an alarm is due");
            let s = l.apply(Event::Alarm, next);
            metered.extend(meters(&s));
            if s.actions.contains(&Action::Sleep { generation: g }) {
                break next;
            }
        };
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, slept + 500));
        l.apply(Event::Saved { generation: g, seq }, slept + 2_000);
        metered.extend(meters(&l.apply(Event::Asleep { generation: g }, slept + 4_000)));
        (at + 3_000, slept + 4_000)
    };
    let first = life(&mut l, T, &mut metered);
    let second = life(&mut l, T + DAYS, &mut metered);
    for (from, to) in [first, second] {
        let of_life: Vec<_> = metered.iter().filter(|(a, _)| *a >= from && *a < to).copied().collect();
        assert_eq!(of_life.first().map(|(a, _)| *a), Some(from), "from its Ready");
        assert_eq!(of_life.last().map(|(_, b)| *b), Some(to), "to its end");
        assert!(of_life.windows(2).all(|w| w[0].1 == w[1].0), "every interval, none twice, none missed: {of_life:?}");
        assert!(of_life.iter().all(|(a, b)| b - a <= METER_EVERY_MS), "each an alarm's: {of_life:?}");
    }
    assert_eq!(metered.iter().map(|(a, b)| b - a).sum::<i64>(), (first.1 - first.0) + (second.1 - second.0), "the days asleep, none");
}

/// Goal (FIN-511, the other side): a computer really awake for days, each
/// of its alarms heard, is metered for all of it, in `METER_EVERY_MS`
/// intervals. Method: a record for its agents at each of its alarms, for
/// three days, each alarm at the time it asked for (an open tab holds it
/// no longer: its use does).
#[test]
fn a_computer_awake_for_days_is_metered_for_all_of_it() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    let mut s = l.apply(Event::Wake { why: Wake::Record }, T + 5_000);
    let until = T + DAYS;
    let mut metered = vec![];
    loop {
        let at = s.alarm_ms.expect("awake, an alarm is due");
        if at > until {
            break;
        }
        l.apply(Event::Wake { why: Wake::Record }, at);
        s = l.apply(Event::Alarm, at);
        assert!(matches!(l.phase, Phase::Awake { .. }), "its records keep it awake");
        metered.extend(meters(&s));
    }
    assert!(metered.windows(2).all(|w| w[0].1 == w[1].0) && metered.iter().all(|(a, b)| b - a == METER_EVERY_MS), "one interval an alarm, none missed: {:?}", &metered[..3]);
    let (from, to) = (metered[0].0, metered[metered.len() - 1].1);
    assert_eq!((from, metered.iter().map(|(a, b)| b - a).sum::<i64>()), (T + 3_000, to - from));
    assert!(until - to < METER_EVERY_MS, "all three days");
}

/// The wakes there are.
const WAKES: [Wake; 4] = [Wake::Record, Wake::Tab, Wake::Owner, Wake::Joined];

/// A life started at T in the idle sleep its alarm began at `T + IDLE_MS`:
/// at its hold, or (`saving`) at its save, number 1, held, asked half a
/// second on. Its generation, and what was metered.
fn idle_sleep(saving: bool) -> (Lifecycle, u64, Vec<(i64, i64)>) {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    let s = l.apply(Event::Alarm, T + IDLE_MS);
    assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }), "its idle sleep: {s:?}");
    let mut m = meters(&s);
    if saving {
        let s = l.apply(Event::Held { generation: g, held: true }, T + IDLE_MS + 500);
        assert_eq!(s.actions, vec![Action::Save { generation: g, held: true, seq: 1 }]);
        m.extend(meters(&s));
    }
    (l, g, m)
}

/// Goal (FIN-474): a wake of any kind (a message's, a pre-wake, a port
/// tab's, its owner's, a join's) during an idle sleep's hold or save
/// cancels the sleep: awake again on the container it has, nothing
/// started or restored, held as that wake holds. At its hold, the hold is
/// let go at once, and the save it was for is due when work settles:
/// nothing coming, that save is made awake, and it sleeps once the wake's
/// hold runs out. At its save, the save goes on, holding the guest to its
/// end as an awake save does, and is kept as one: its end lets go of the
/// hold, nothing stops, and what this life did before its sleep is kept.
/// Method: each wake at each step, then the alarms and reports after it.
#[test]
fn any_wake_cancels_an_idle_sleep_before_its_stop() {
    let at = T + IDLE_MS + 1_000;
    for why in WAKES {
        // at its hold
        let (mut l, g, _) = idle_sleep(false);
        let s = l.apply(Event::Wake { why }, at);
        assert_eq!(s.actions, vec![Action::Unhold { generation: g }], "{why:?}");
        assert_eq!((&l.phase, l.saving()), (&Phase::Awake { generation: g, since_ms: T + IDLE_MS - 1 }, None), "awake as before its sleep: {why:?}");
        assert_eq!((l.held_until_ms(), l.save_due_ms), (at + why.hold_ms(), Some(at + SAVE_SETTLE_MS)), "{why:?}");
        let s = l.apply(Event::Alarm, at + SAVE_SETTLE_MS);
        assert_eq!(s.actions.last(), Some(&Action::Hold { generation: g }), "the save it was for, awake: {why:?}: {s:?}");
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + SAVE_SETTLE_MS + 500));
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, at + SAVE_SETTLE_MS + 1_000).actions, vec![Action::Unhold { generation: g }]);
        let s = l.apply(Event::Alarm, at + why.hold_ms());
        assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g }), "asleep once the wake's hold runs out: {why:?}: {s:?}");
        // at its save
        let (mut l, g, _) = idle_sleep(true);
        let s = l.apply(Event::Wake { why }, at);
        assert!(s.actions.is_empty(), "its save holds the guest to its end: {why:?}: {s:?}");
        assert_eq!(l.phase, Phase::Awake { generation: g, since_ms: T + IDLE_MS - 1 }, "awake as before its sleep: {why:?}");
        assert_eq!(l.saving(), Some(Saving::Save { since_ms: T + IDLE_MS + 500, held: true, seq: 1, unheld: false }), "its save goes on: {why:?}");
        let s = l.apply(Event::Saved { generation: g, seq: 1 }, at + 60_000);
        assert_eq!((s.actions, s.note, s.ended), (vec![Action::Unhold { generation: g }], SaveNote::Clear, None), "{why:?}");
        assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g) && l.saving().is_none(), "{why:?}");
        assert_eq!(l.kept_ms, T + IDLE_MS, "what it did before its sleep is kept: {why:?}");
    }
}

/// Goal: a wake never cancels its owner's sleep (at its hold or its
/// save), their restart (an image change is one), an idle sleep its owner
/// asked for again, or a sleep whose stop began: its save worked, its
/// saves failed past their bound, or a release before saves were its own
/// stored it in its stop. Its container is its owner's to stop, or going:
/// each owes a start once it is asleep, as before. Method: its owner's
/// wake at each.
#[test]
fn a_wake_never_cancels_its_owners_sleep_nor_a_stop() {
    fn woken(l: &mut Lifecycle, at: i64) {
        let s = l.apply(Event::Wake { why: Wake::Owner }, at);
        assert!(s.actions.is_empty() && matches!(l.phase, Phase::Sleeping { owes_start: true, .. }), "{s:?}: {l:?}");
    }
    // its owner's sleep, at its hold and at its save
    for saving in [false, true] {
        let mut l = Lifecycle::new();
        let g = started(&mut l, T);
        l.apply(Event::Sleep, T + 10_000);
        if !saving {
            woken(&mut l, T + 10_500);
        }
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 11_000));
        if saving {
            woken(&mut l, T + 11_500);
        }
        assert_eq!(l.apply(Event::Saved { generation: g, seq }, T + 12_000).actions, vec![Action::Stop { generation: g, saved: true }]);
        let s = l.apply(Event::Asleep { generation: g }, T + 13_000);
        assert_eq!(owed(&mut l, &s, T + 13_000).actions, vec![Action::Start { generation: g + 1 }]);
    }
    // their restart
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Restart { generation: g }, T + 10_000);
    woken(&mut l, T + 10_500);
    assert!(matches!(l.phase, Phase::Sleeping { restarting: true, .. }));
    // an idle sleep its owner asked for again
    let (mut l, _, _) = idle_sleep(false);
    l.apply(Event::Sleep, T + IDLE_MS + 100);
    woken(&mut l, T + IDLE_MS + 200);
    // its stop begun, its save kept
    let (mut l, g, _) = idle_sleep(true);
    assert_eq!(l.apply(Event::Saved { generation: g, seq: 1 }, T + IDLE_MS + 1_000).actions, vec![Action::Stop { generation: g, saved: true }]);
    woken(&mut l, T + IDLE_MS + 2_000);
    let s = l.apply(Event::Asleep { generation: g }, T + IDLE_MS + 5_000);
    assert_eq!(owed(&mut l, &s, T + IDLE_MS + 5_000).actions, vec![Action::Start { generation: g + 1 }]);
    // its stop begun, its saves failed past their bound
    let (mut l, g, _) = idle_sleep(true);
    let s = l.apply_with(Event::SaveFailed { generation: g, seq: 1, why: "no room".into() }, T + IDLE_MS + 1_000, &Rules { unsaved_max_ms: 0 });
    assert_eq!(s.actions.last(), Some(&Action::Stop { generation: g, saved: false }), "{s:?}");
    woken(&mut l, T + IDLE_MS + 2_000);
    // stored in its stop by a release before saves were its own (no save step)
    let mut old = Lifecycle { phase: Phase::Sleeping { generation: 1, since_ms: T, owner: false, restarting: false, owes_start: false }, generation: 1, ..Lifecycle::new() };
    woken(&mut old, T + 1_000);
}

/// Goal: what comes after a cancel, again or late, changes nothing: a
/// second wake only holds it longer; its hold's answer, whose touch may
/// have landed after the cancel let go of it, is let go again; its
/// sleep's end, a failure from its hold (no container found) or a report
/// of a save it never asked for is of a sleep no longer under way; and
/// the save that went on is kept once, a report of it again nothing.
/// Method: each, after a cancel at the hold and at the save.
#[test]
fn what_comes_after_a_cancel_changes_nothing() {
    let at = T + IDLE_MS + 1_000;
    // at its hold
    let (mut l, g, _) = idle_sleep(false);
    l.apply(Event::Wake { why: Wake::Tab }, at);
    let before = l.clone();
    assert_eq!(l.apply(Event::Held { generation: g, held: true }, at + 100).actions, vec![Action::Unhold { generation: g }]);
    assert_eq!(l, before, "let go again, and nothing else");
    for e in [Event::Asleep { generation: g }, Event::SaveFailed { generation: g, seq: 0, why: "gone".into() }, Event::Saved { generation: g, seq: 1 }] {
        assert!(l.apply(e, at + 200).actions.is_empty());
        assert_eq!(l, before);
    }
    let s = l.apply(Event::Wake { why: Wake::Record }, at + 300);
    assert!(s.actions.is_empty() && l.phase == before.phase && l.held_until_ms() == at + 300 + IDLE_MS, "{s:?}");
    // at its save
    let (mut l, g, _) = idle_sleep(true);
    l.apply(Event::Wake { why: Wake::Tab }, at);
    let s = l.apply(Event::Wake { why: Wake::Record }, at + 100);
    assert!(s.actions.is_empty() && matches!(l.saving(), Some(Saving::Save { seq: 1, .. })), "its save goes on: {s:?}");
    assert_eq!(l.apply(Event::Saved { generation: g, seq: 1 }, at + 200).actions, vec![Action::Unhold { generation: g }]);
    let before = l.clone();
    for e in [Event::Saved { generation: g, seq: 1 }, Event::SaveFailed { generation: g, seq: 1, why: "late".into() }, Event::Asleep { generation: g }] {
        assert!(l.apply(e, at + 300).actions.is_empty());
        assert_eq!(l, before);
    }
}

/// Goal: a cancel survives its isolate. Stored and read back with its
/// save under way, the save's report keeps it as any; that report lost
/// with the isolate, the save's deadline fails it as an awake save (held
/// no more, tried again after a pause), never a sleep's (no unsaved
/// window, no free time), and it stays awake. Stored at its hold, the
/// save it was for follows. Method: the lifecycle stored and read as the
/// DO does, at each point, then its alarms as it asks for them.
#[test]
fn a_cancel_survives_its_isolate() {
    let at = T + IDLE_MS + 1_000;
    let read = |l: &Lifecycle| Lifecycle::read(&serde_json::to_string(l).unwrap()).unwrap().0;
    let (mut l, g, _) = idle_sleep(true);
    l.apply(Event::Wake { why: Wake::Record }, at);
    let mut l = read(&l);
    assert_eq!(l.apply(Event::Saved { generation: g, seq: 1 }, at + 1_000).actions, vec![Action::Unhold { generation: g }]);
    // its report lost: the save's deadline fails it
    let (mut l, g, _) = idle_sleep(true);
    l.apply(Event::Wake { why: Wake::Record }, at);
    let mut l = read(&l);
    let deadline = T + IDLE_MS + 500 + SAVE_DEADLINE_MS;
    let mut s = Step::default();
    // bounded: a meter's alarm every five minutes up to the deadline
    for _ in 0..8 {
        let Some(next) = l.next_alarm().filter(|next| *next <= deadline) else { break };
        s = l.apply(Event::Alarm, next);
    }
    assert!(s.actions.contains(&Action::Unhold { generation: g }), "{s:?}");
    assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g) && l.saving().is_none(), "{l:?}");
    assert_eq!((l.unsaved_since_ms(), l.free_since_ms(), l.save_due_ms), (None, None, Some(deadline + SAVE_RETRY_MS)));
    // stored at its hold: the save it was for follows
    let (mut l, g, _) = idle_sleep(false);
    l.apply(Event::Wake { why: Wake::Record }, at);
    let mut l = read(&l);
    assert_eq!(l.apply(Event::Alarm, at + SAVE_SETTLE_MS).actions.last(), Some(&Action::Hold { generation: g }));
}

/// Goal (money): a cancelled sleep's awake time runs on, metered once and
/// without a gap: its sleep's steps and all that follows, to its next
/// sleep's end, in intervals that meet, at its hold and at its save. A
/// wake heard late into an idle sleep whose save's report was lost (its
/// isolate gone, days on) bills none of the silence (FIN-511): metered to
/// its sleep, then from the wake on, its lost save failed as an awake
/// one's (no free time). Method: every interval metered, each alarm as it
/// asked and each hold, save and stop answered.
#[test]
fn a_cancelled_sleep_is_metered_once_without_a_gap() {
    let at = T + IDLE_MS + 1_000;
    for saving in [false, true] {
        let (mut l, g, mut m) = idle_sleep(saving);
        m.extend(meters(&l.apply(Event::Wake { why: Wake::Record }, at)));
        if saving {
            m.extend(meters(&l.apply(Event::Saved { generation: g, seq: 1 }, at + 60_000)));
        }
        let (mut now, mut queue) = (at + 60_000, vec![]);
        // bounded: its alarms to its next sleep's end, twenty minutes on
        for _ in 0..32 {
            let event = match queue.pop() {
                Some(e) => e,
                None => match l.next_alarm() {
                    Some(alarm) => {
                        now = now.max(alarm);
                        Event::Alarm
                    }
                    None => break,
                },
            };
            now += 100;
            let s = l.apply(event, now);
            m.extend(meters(&s));
            for a in &s.actions {
                match *a {
                    Action::Sleep { generation } | Action::Hold { generation } => queue.push(Event::Held { generation, held: true }),
                    Action::Save { generation, seq, .. } => queue.push(Event::Saved { generation, seq }),
                    Action::Stop { generation, .. } => queue.push(Event::Asleep { generation }),
                    _ => {}
                }
            }
        }
        assert_eq!(l.phase, Phase::ASLEEP, "asleep once its hold ran out");
        assert!(m.iter().all(|(a, b)| a < b) && m.windows(2).all(|w| w[0].1 == w[1].0), "each interval meets the next, none twice: {m:?}");
        assert_eq!((m[0].0, m[m.len() - 1].1), (T + 3_000, now), "from its start to its sleep's end");
    }
    // a late wake, days on, into an idle sleep whose save's report was lost
    let (mut l, g, m) = idle_sleep(true);
    assert_eq!(m, vec![(T + 3_000, T + IDLE_MS)]);
    let late = T + DAYS;
    let s = l.apply(Event::Wake { why: Wake::Record }, late);
    assert_eq!((meters(&s), &l.phase), (vec![], &Phase::Awake { generation: g, since_ms: T + IDLE_MS - 1 }), "none of its hold, nor the silence");
    let s = l.apply(Event::Alarm, late + 1_000);
    assert_eq!(meters(&s), vec![(late, late + 1_000)], "from the wake on");
    assert!(s.actions.contains(&Action::Unhold { generation: g }) && l.unsaved_since_ms().is_none() && l.free_since_ms().is_none(), "its lost save failed as an awake one's: {s:?}");
}

/// Goal (#1357's review): a cancel is no new awake run. Its sleep's save,
/// kept, is one this run made, so a host's stop after it, nothing done
/// since (a pre-wake's cancel), is a sleep: no strike, nothing started,
/// its save holding all of it. A message's cancel is work its guest has
/// not taken up, so that stop stays a crash, started again. Method: a
/// cancel at the save, the save kept, then the container's exit.
#[test]
fn a_host_stop_after_a_cancelled_sleeps_save_is_a_sleep() {
    let at = T + IDLE_MS + 1_000;
    let (mut l, g, _) = idle_sleep(true);
    l.apply(Event::Wake { why: Wake::Tab }, at);
    l.apply(Event::Saved { generation: g, seq: 1 }, at + 1_000);
    let s = l.apply(Event::Exited { generation: g }, at + 2_000);
    assert_eq!(s.ended.map(|e| (e.by, e.saved, e.quiet)), Some((LifeEnd::Sleep, true, true)), "{s:?}");
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })) && l.failures() == 0 && !l.start_due(at + 2_000), "{l:?}");
    // a message's cancel: its work not taken up, a crash, started again
    let (mut l, g, _) = idle_sleep(true);
    l.apply(Event::Posted { at_ms: at - 100 }, at);
    l.apply(Event::Wake { why: Wake::Record }, at);
    l.apply(Event::Saved { generation: g, seq: 1 }, at + 1_000);
    let s = l.apply(Event::Exited { generation: g }, at + 2_000);
    assert_eq!(s.ended.map(|e| e.by), Some(LifeEnd::Exit), "{s:?}");
    assert!(l.start_due(at + 2_000), "started again for its message: {l:?}");
}

/// Goal (#1357's review): a keepalive its guest opened as its idle sleep
/// saved unheld (its hold unanswered, a claim in flight) is busy from the
/// cancel: saved every `SAVE_EVERY_MS` while it stays open, as any turn.
/// Method: the hold unanswered, the keepalive during the save, a wake, the
/// save kept, then the alarms of a long turn.
#[test]
fn a_turn_open_at_a_cancel_is_saved_as_it_runs() {
    let mut l = Lifecycle::new();
    let g = started(&mut l, T);
    l.apply(Event::Alarm, T + IDLE_MS);
    assert_eq!(save_asked(&l.apply(Event::Held { generation: g, held: false }, T + IDLE_MS + HOLD_WAIT_MS)), (g, false, 1));
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + IDLE_MS + HOLD_WAIT_MS + 100);
    let at = T + IDLE_MS + HOLD_WAIT_MS + 500;
    l.apply(Event::Wake { why: Wake::Record }, at);
    assert_eq!(l.apply(Event::Saved { generation: g, seq: 1 }, at + 1_000).actions, vec![Action::Unhold { generation: g }]);
    let due = l.next_alarm().filter(|due| *due <= at + SAVE_EVERY_MS);
    let mut s = Step::default();
    // bounded: a meter's alarm every five minutes, then the save's
    for _ in 0..4 {
        let Some(next) = l.next_alarm().filter(|next| *next <= at + SAVE_EVERY_MS) else { break };
        s = l.apply(Event::Alarm, next);
    }
    assert!(due.is_some() && s.actions.contains(&Action::Hold { generation: g }), "its turn's save, {} minutes on: {s:?}", SAVE_EVERY_MS / 60_000);
}

/// Goal (#1357's review, then #1378's): a save its owner's sleep or
/// restart asked anew after a cancel (the cancelled sleep's save still
/// under way) is the newest once kept: the cancelled one, ending after it,
/// is overtaken and never kept ahead of it (`Saves::overtaken`); one that
/// ends first, or whose newer save failed, is kept, a capture never lost
/// for a save only asked. Method: the lifecycle through its owner's sleep
/// to its end, the saves kept as the DO keeps them.
#[test]
fn a_save_asked_again_after_a_cancel_overtakes_the_one_under_way() {
    let at = T + IDLE_MS + 1_000;
    // the cancelled sleep's save (`idle_sleep`)
    let began = 1;
    for owner in [Event::Sleep, Event::Restart { generation: 0 }] {
        let (mut l, g, _) = idle_sleep(true);
        let mut saves = Saves::default();
        assert!(!saves.overtaken(g, began), "a save under way, alone, is kept: {owner:?}");
        l.apply(Event::Wake { why: Wake::Record }, at);
        l.apply(owner.clone(), at + 100);
        let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, at + 200));
        assert_eq!(seq, 2, "{owner:?}");
        assert!(!saves.overtaken(g, began), "a newer save only asked overtakes nothing: {owner:?}");
        saves.saved(g, vec![serde_json::json!({ "id": "newer", "dir": "/data" })], seq, at + 300, true);
        l.apply(Event::Saved { generation: g, seq: 2 }, at + 300);
        l.apply(Event::Asleep { generation: g }, at + 400);
        assert!(l.running().is_none(), "its sleep ended: {owner:?}");
        assert!(saves.overtaken(g, began) && !saves.overtaken(g, seq), "the cancelled one, asked before it, is overtaken: {owner:?}");
    }
}

/// Goal (docs/computers.md, "A disk"): a computer on a disk is never saved.
/// A turn held open for hours asks no awake save, nor does a turn's end;
/// its sleep's hold (the cell's wait for its disk to ship) leads straight
/// to its stop, saved, with no save between; a wake during that hold
/// cancels the sleep as any hold's does; and a lifecycle stored before the
/// mark reads as no disk. Method: a disk's day of alarms with a keepalive
/// open, then closed; its sleep, cancelled once, then through.
#[test]
fn a_disk_is_never_saved_and_its_sleep_is_its_stop() {
    let mut l = Lifecycle::new();
    l.set_disk();
    assert!(l.disk());
    let g = started(&mut l, T);
    l.apply(Event::Opened { socket: Socket::Keepalive }, T + 1_000);
    let (saves, slept) = run_alarms(&mut l, T + 6 * 3_600_000);
    assert!(saves.is_empty() && slept.is_none(), "busy for hours, nothing saved: {saves:?} {slept:?}");
    let closed = T + 6 * 3_600_000;
    l.apply(Event::Closed { socket: Socket::Keepalive }, closed);
    let (saves, slept) = run_alarms(&mut l, closed + 2 * IDLE_MS);
    assert!(saves.is_empty(), "a turn's end saves nothing: {saves:?}");
    let slept = slept.expect("it sleeps once idle");
    assert_eq!(slept, closed + IDLE_MS);
    // a wake during its hold cancels the sleep: awake on its container
    let s = l.apply(Event::Wake { why: Wake::Record }, slept + 100);
    assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g), "{s:?}");
    // its next idle sleep, through: the hold answered, then its stop
    let (_, slept) = run_alarms(&mut l, slept + 2 * IDLE_MS);
    let slept = slept.expect("it sleeps again");
    let s = l.apply(Event::Held { generation: g, held: true }, slept + 500);
    assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }], "no save between its hold and its stop");
    let ended = l.apply(Event::Asleep { generation: g }, slept + 2_000).ended.expect("its life ends");
    assert_eq!((ended.by, ended.saved), (LifeEnd::Sleep, true));
    // stored before the mark: no disk
    let stored = serde_json::to_value(Lifecycle::new()).unwrap();
    assert!(stored.get("disk").is_none(), "no disk writes no mark");
    let read: Lifecycle = serde_json::from_value(stored).unwrap();
    assert!(!read.disk());
}

/// Goal: a disk is saved for its owner's export alone, once, awake under
/// the hold as any awake save; a failed one is tried again after its
/// pause; one asked asleep is taken once it runs; a computer with saves of
/// its own asks none. Method: the export's ask valid (awake), invalid (its
/// save failed), replayed (asked again while one is under way, and after
/// it), and across a restart (asked asleep, stored, read back, woken).
#[test]
fn an_export_saves_a_disk_once_awake() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    let s = l.apply(Event::ExportAsked { since_ms: T }, T + 5_000);
    assert!(s.actions.is_empty() && s.alarm_ms == Some(T + 5_000), "the ask only marks it due, its alarm at once: {s:?}");
    assert_eq!(l.apply(Event::Alarm, T + 5_000).actions.iter().filter(|a| matches!(a, Action::Hold { .. })).count(), 1, "held by its alarm");
    assert!(l.apply(Event::ExportAsked { since_ms: T }, T + 5_100).actions.is_empty(), "asked again under way: the one save");
    let (_, held, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 5_500));
    assert!(held);
    // its save fails: due again after its pause, not before
    let failed = T + 6_000;
    l.apply(Event::SaveFailed { generation: g, seq, why: "a backup failed".into() }, failed);
    // its first failure's pause
    let again = failed + SAVE_RETRY_MS;
    assert!(l.next_alarm().is_some_and(|at| at > failed && at <= again), "an alarm by its retry");
    let holds = |s: &Step| s.actions.iter().filter(|a| matches!(a, Action::Hold { .. })).count();
    assert_eq!(holds(&l.apply(Event::Alarm, again - 1)), 0, "not before its pause");
    assert_eq!(holds(&l.apply(Event::ExportAsked { since_ms: T }, again - 1)), 0, "an ask meanwhile waits its pause too");
    assert_eq!(holds(&l.apply(Event::Alarm, again)), 1, "held again after it");
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, again + 100));
    assert_eq!(l.apply(Event::Saved { generation: g, seq }, again + 200).actions, vec![Action::Unhold { generation: g }]);
    // saved: nothing more is due, however long it runs
    let (saves, _) = run_alarms(&mut l, again + IDLE_MS / 2);
    assert!(saves.is_empty(), "{saves:?}");
    // asked asleep: nothing (its export wakes it, then asks); asked as it
    // starts: kept across a restart of the object, taken once it runs
    let mut asleep = Lifecycle::new();
    asleep.set_disk();
    asleep.apply(Event::ExportAsked { since_ms: T }, T);
    assert!(serde_json::to_value(&asleep).unwrap().get("export_due_ms").is_none(), "no ask kept for a computer that does not run");
    let s = asleep.apply(Event::Wake { why: Wake::Owner }, T + 1_000);
    let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("{s:?}") };
    asleep.apply(Event::ExportAsked { since_ms: T }, T + 1_100);
    let mut asleep: Lifecycle = serde_json::from_value(serde_json::to_value(&asleep).unwrap()).unwrap();
    asleep.apply(Event::Ready { generation: g }, T + 4_000);
    assert!(asleep.apply(Event::Alarm, T + 4_100).actions.contains(&Action::Hold { generation: g }), "taken once it runs");
    // its export gave up: the ask goes, whatever is due
    let mut gave_up = Lifecycle::new();
    gave_up.set_disk();
    let g = started(&mut gave_up, T);
    gave_up.apply(Event::ExportAsked { since_ms: T }, T + 5_000);
    gave_up.apply(Event::ExportDropped, T + 5_001);
    let (saves, _) = run_alarms(&mut gave_up, T + 5_000 + IDLE_MS / 2);
    assert!(saves.is_empty(), "nothing taken for an export that gave up: {saves:?}");
    let _ = g;
    // asked as a start begins, that start failing: dropped with it
    let mut failing = Lifecycle::new();
    failing.set_disk();
    let s = failing.apply(Event::Wake { why: Wake::Owner }, T);
    let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("{s:?}") };
    failing.apply(Event::ExportAsked { since_ms: T }, T + 100);
    failing.apply(Event::StartFailed { generation: g, why: "the test".into() }, T + 200);
    assert!(serde_json::to_value(&failing).unwrap().get("export_due_ms").is_none(), "dropped with the start it was asked in");
    // a computer with saves of its own asks none
    let mut saves = Lifecycle::new();
    started(&mut saves, T);
    let before = serde_json::to_value(&saves).unwrap();
    assert!(saves.apply(Event::ExportAsked { since_ms: T }, T + 5_000).actions.is_empty());
    assert_eq!(serde_json::to_value(&saves).unwrap(), before, "nothing changed");
}

/// Goal: an export's ask lasts the life it was asked in: a life that ends
/// before its save forgets it, so a later wake takes no save nobody waits
/// for (the export asks again while it waits, which wakes it). Method: an
/// ask awake, a sleep before its alarm (its hold answered, its stop), a
/// start after; and an ask asleep, kept until the start it brings.
#[test]
fn an_exports_ask_ends_with_its_life() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    l.apply(Event::ExportAsked { since_ms: T }, T + 5_000);
    let s = l.apply(Event::Sleep, T + 5_001);
    assert_eq!(s.actions, vec![Action::Sleep { generation: g }], "its owner's sleep first");
    assert_eq!(l.apply(Event::Held { generation: g, held: true }, T + 5_500).actions, vec![Action::Stop { generation: g, saved: true }]);
    l.apply(Event::Asleep { generation: g }, T + 6_000);
    let g = started(&mut l, T + 60_000);
    let (saves, _) = run_alarms(&mut l, T + 60_000 + IDLE_MS / 2);
    assert!(saves.is_empty(), "no save for an export that stopped asking: {saves:?}");
    // a crash forgets it as well
    l.apply(Event::ExportAsked { since_ms: T }, T + 70_000);
    l.apply(Event::Exited { generation: g }, T + 70_001);
    assert!(l.next_alarm().is_none_or(|at| at > T + 70_001 + 1_000) || !matches!(l.phase, Phase::Awake { .. }));
    let ended = serde_json::to_value(&l).unwrap();
    assert!(ended.get("export_due_ms").is_none(), "nothing due after its life ended: {ended}");
}

/// Goal: a computer goes on a disk only as it is made, never after its
/// first start. Method: the mark set on a computer that started.
#[test]
#[should_panic(expected = "a computer is put on a disk as it is made")]
fn a_disk_is_chosen_as_it_is_made() {
    let mut l = Lifecycle::new();
    started(&mut l, T);
    l.set_disk();
}

/// Goal (Codex on #1552): a disk's sleep never stops with writes still in
/// it unshipped. A wait that ended with some left (or no word of them)
/// keeps its container, awake again, and its next sleep waits again, as a
/// sleep whose save failed does; past the unsaved bound it stops unsaved
/// and says so. A wait that saw nothing left stops it, saved.
#[test]
fn a_disks_sleep_never_stops_with_writes_unshipped() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    let (_, slept) = run_alarms(&mut l, T + 2 * IDLE_MS);
    let slept = slept.expect("it sleeps once idle");
    let s = l.apply(Event::Held { generation: g, held: false }, slept + 45_500);
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Stop { .. })), "no stop with writes unshipped: {s:?}");
    assert!(matches!(l.phase, Phase::Awake { generation, .. } if generation == g), "awake again on its container: {:?}", l.phase);
    // its next sleep waits again; one that saw nothing left stops it
    let (_, again) = run_alarms(&mut l, slept + 45_500 + 2 * IDLE_MS);
    let again = again.expect("it sleeps again");
    let s = l.apply(Event::Held { generation: g, held: true }, again + 500);
    assert_eq!(s.actions, vec![Action::Stop { generation: g, saved: true }]);
    // past the unsaved bound, a wait still undrained stops it unsaved, saying so
    let mut late = Lifecycle::new();
    late.set_disk();
    let g = started(&mut late, T);
    let (_, slept) = run_alarms(&mut late, T + 2 * IDLE_MS);
    let s = late.apply_with(Event::Held { generation: g, held: false }, slept.expect("it sleeps") + 45_500, &Rules { unsaved_max_ms: 0 });
    assert_eq!(s.actions.iter().filter(|a| matches!(a, Action::Stop { saved: false, .. })).count(), 1, "{s:?}");
    assert!(matches!(s.note, SaveNote::Says(_)), "it says so: {:?}", s.note);
}

/// Goal (FIN-783, Codex on #1567): a disk's stop is the platform's time,
/// never its owner's. Its final flush may be kept for while its storage is
/// unreachable, up to the unsaved bound: its owner is metered up to the
/// stop's beginning and not for the stop, however long, nor after.
/// Method: a drained sleep, its stop's end 20 minutes later; the meters.
#[test]
fn a_disks_stop_kept_for_its_final_flush_is_never_its_owners_time() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    let (_, slept) = run_alarms(&mut l, T + 2 * IDLE_MS);
    let slept = slept.expect("it sleeps once idle");
    let stop = slept + 500;
    let s = l.apply(Event::Held { generation: g, held: true }, stop);
    assert_eq!(s.actions.iter().filter(|a| matches!(a, Action::Stop { saved: true, .. })).count(), 1, "{s:?}");
    let meters = |acts: &[Action]| -> Vec<(i64, i64)> { acts.iter().filter_map(|a| if let Action::Meter { from_ms, to_ms } = a { Some((*from_ms, *to_ms)) } else { None }).collect() };
    let end = stop + 20 * 60_000;
    let gone = l.apply(Event::Asleep { generation: g }, end);
    let all: Vec<(i64, i64)> = meters(&s.actions).into_iter().chain(meters(&gone.actions)).collect();
    assert!(all.iter().all(|(_, to)| *to <= stop), "nothing metered past its stop's beginning: {all:?}");
    assert!(matches!(l.phase, Phase::Asleep { .. }), "{:?}", l.phase);
}

/// Goal (Codex on #1552): an export's ask that comes once the save it waits
/// for already landed (its look raced that save) takes no second save.
#[test]
fn an_exports_ask_after_its_save_landed_takes_no_second() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    l.apply(Event::ExportAsked { since_ms: T }, T + 5_000);
    l.apply(Event::Alarm, T + 5_000);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 5_500));
    l.apply(Event::Saved { generation: g, seq }, T + 6_000);
    // the export's look that raced it asks again, its export begun before
    l.apply(Event::ExportAsked { since_ms: T }, T + 6_100);
    let (saves, _) = run_alarms(&mut l, T + 6_100 + IDLE_MS / 2);
    assert!(saves.is_empty(), "no second save: {saves:?}");
}

/// Goal: a disk's sleep is no save: an export begun before a sleep, its
/// computer then woken for it, still takes its save.
#[test]
fn an_export_begun_before_a_disks_sleep_still_takes_its_save() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    let (_, slept) = run_alarms(&mut l, T + 2 * IDLE_MS);
    let slept = slept.expect("it sleeps once idle");
    let began = slept - 1_000;
    l.apply(Event::Held { generation: g, held: true }, slept + 500);
    l.apply(Event::Asleep { generation: g }, slept + 2_000);
    let s = l.apply(Event::Wake { why: Wake::Owner }, slept + 3_000);
    let Some(Action::Start { generation: g }) = s.actions.first().cloned() else { panic!("{s:?}") };
    l.apply(Event::Ready { generation: g }, slept + 6_000);
    l.apply(Event::ExportAsked { since_ms: began }, slept + 6_100);
    assert!(l.apply(Event::Alarm, slept + 6_100).actions.contains(&Action::Hold { generation: g }), "its save is taken");
}

/// Goal (Codex on #1552): an export begun as a save is stored but before
/// its event (its records' cleanup between) finds that save too old, and
/// its ask is heard: a save is taken for it.
#[test]
fn an_export_begun_as_a_save_was_kept_still_takes_its_own() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    l.apply(Event::ExportAsked { since_ms: T }, T + 5_000);
    l.apply(Event::Alarm, T + 5_000);
    let (_, _, seq) = save_asked(&l.apply(Event::Held { generation: g, held: true }, T + 5_500));
    // a second export begins after that save was asked, before its event
    let began = T + 5_800;
    l.apply(Event::Saved { generation: g, seq }, T + 6_000);
    l.apply(Event::ExportAsked { since_ms: began }, T + 6_100);
    assert!(l.apply(Event::Alarm, T + 6_100).actions.contains(&Action::Hold { generation: g }), "its own save is taken");
}

/// Goal (Codex on #1552): a disk's sleep whose final flush ended in no
/// clean head ends unsaved, saying so: never recorded as saved.
#[test]
fn a_disks_sleep_whose_final_flush_did_not_finish_ends_unsaved() {
    let mut l = Lifecycle::new();
    l.set_disk();
    let g = started(&mut l, T);
    let (_, slept) = run_alarms(&mut l, T + 2 * IDLE_MS);
    let slept = slept.expect("it sleeps once idle");
    assert_eq!(l.apply(Event::Held { generation: g, held: true }, slept + 500).actions, vec![Action::Stop { generation: g, saved: true }]);
    let s = l.apply(Event::AsleepUnshipped { generation: g }, slept + 140_000);
    let ended = s.ended.expect("its life ends");
    assert_eq!((ended.by, ended.saved), (LifeEnd::Sleep, false), "{s:?}");
    assert!(matches!(s.note, SaveNote::Says(_)), "it says so: {:?}", s.note);
    assert!(matches!(l.phase, Phase::Asleep { .. }));
}
